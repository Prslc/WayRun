mod config;
mod edit;
mod hosts;

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use self::config::{Config, load_or_default};
use self::hosts::HostCache;
use crate::provider::external::HostMeta;

pub use self::edit::{add_plugin, remove_plugin, set_enabled, set_keyword};

#[derive(Clone)]
pub struct PendingHost {
    pub id: String,
    pub command: String,
    pub resident: bool,
}

pub(super) struct Entry {
    pub plugin: Box<dyn super::Plugin>,
    pub keyword: String,
    /// An external host forks a process per call, so the default chain bounds it
    /// with a deadline; a built-in answers off its own lists and is never cut off.
    pub external: bool,
    /// Set while an external plugin runs on its placeholder identity, so startup
    /// forks nothing; `resolve_pending` clears it on first use.
    pub pending: Option<PendingHost>,
}

pub(super) type PluginMap = HashMap<&'static str, Box<dyn super::Plugin>>;

/// How many external hosts may be forked at once while resolving identities.
/// Each is a fresh interpreter, so this is the memory ceiling of the walk.
const DISCOVERY_CONCURRENCY: usize = 2;

static CONFIG: tokio::sync::RwLock<Config> = tokio::sync::RwLock::const_new(Config {
    plugins: Vec::new(),
});
pub(super) static REGISTRY: tokio::sync::RwLock<Vec<Entry>> =
    tokio::sync::RwLock::const_new(Vec::new());
static REGISTRY_READY: AtomicBool = AtomicBool::new(false);
/// Serializes the one-time build and config reloads (both take INIT first).
static INIT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Lazy first build: load config + registry on first use, once.
pub(super) async fn ensure_loaded() {
    if !REGISTRY_READY.load(Ordering::Acquire) {
        let _guard = INIT.lock().await;
        if !REGISTRY_READY.load(Ordering::Acquire) {
            rebuild().await;
        }
    }
}

/// Re-read `plugins.toml` and rebuild the registry, so resident mode picks up an
/// edit without a restart; it lands on the next search / `?` / `list_plugins`.
pub async fn reload() {
    let _guard = INIT.lock().await;
    rebuild().await;
}

/// Rebuild only when `plugins.toml` actually changed, so one save's several
/// events do not re-fork every external host.
pub async fn reload_if_changed() -> bool {
    let _guard = INIT.lock().await;
    let new_config = load_or_default();
    if REGISTRY_READY.load(Ordering::Acquire) && *CONFIG.read().await == new_config {
        return false;
    }
    apply(new_config).await;
    true
}

async fn rebuild() {
    apply(load_or_default()).await;
}

async fn apply(new_config: Config) {
    let entries = build_entries(&new_config);
    // The index exists only for `f`/`d`; with both gone, leave no cache behind.
    let index_enabled = owns_index(&entries);
    *CONFIG.write().await = new_config;
    *REGISTRY.write().await = entries;
    crate::provider::file::index::sync_enabled(index_enabled);
    crate::provider::file::index::settings_changed();
    REGISTRY_READY.store(true, Ordering::Release);
}

/// The configured host executables, deduplicated, for the watcher.
pub fn host_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = load_or_default()
        .plugins
        .iter()
        .filter(|entry| entry.enabled)
        .filter_map(|entry| entry.command.as_deref())
        .filter_map(crate::provider::external::command_path)
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// The `plugins.toml` entries pointing at `command`, as `(id, keyword, enabled)`,
/// so `wayrun plugin check` can compare a script with its registration.
pub fn entries_for_command(command: &str) -> Vec<(String, String, bool)> {
    // The exec bit stays out of it: a script that lost it is still registered.
    let wanted = fs::canonicalize(command).unwrap_or_else(|_| PathBuf::from(command));
    load_or_default()
        .plugins
        .into_iter()
        .filter(|entry| {
            entry.command.as_deref().is_some_and(|configured| {
                fs::canonicalize(configured).unwrap_or_else(|_| PathBuf::from(configured)) == wanted
            })
        })
        .map(|entry| (entry.id, entry.keyword, entry.enabled))
        .collect()
}

/// Run one `Action::Plugin` command on the host that serves `id`; the action name
/// and payload are the plugin's own.
pub async fn run_command(
    id: &str,
    action: &str,
    payload: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    ensure_loaded().await;
    let reg = REGISTRY.read().await;
    let Some(entry) = reg
        .iter()
        .find(|entry| entry.plugin.meta().id.as_ref() == id)
    else {
        anyhow::bail!("no plugin {id:?} in the registry");
    };
    entry.plugin.command(action, payload).await
}

/// Rebuild the registry when a host file changed, then ask only those hosts
/// for their identity again.
pub async fn refresh_hosts() {
    let stale: Vec<String> = {
        let _guard = INIT.lock().await;
        if !REGISTRY_READY.load(Ordering::Acquire) {
            return;
        }
        let config = CONFIG.read().await;
        let cache = HostCache::load();
        let stale: Vec<String> = config
            .plugins
            .iter()
            .filter(|entry| entry.enabled)
            .filter_map(|entry| entry.command.clone())
            .filter(|command| cache.stale(command))
            .collect();
        if stale.is_empty() {
            return;
        }
        *REGISTRY.write().await = build_entries(&config);
        stale
    };
    // A changed script is reloaded by a fresh process, not the one that already
    // loaded it; discovery below must see the new code too.
    for command in &stale {
        crate::provider::resident::reap_command(command);
    }
    resolve_pending_filtered(None, Some(&stale)).await;
}

/// Whether either provider the file index serves is among `entries`.
fn owns_index(entries: &[Entry]) -> bool {
    entries
        .iter()
        .any(|entry| crate::provider::file::index::owns(&entry.plugin.meta().id))
}

/// Build registry entries from a config without contacting a host: a cached
/// identity is reused and the rest wait on [`resolve_pending`].
fn build_entries(config: &Config) -> Vec<Entry> {
    let mut map: PluginMap = crate::provider::plugin_map();
    let mut entries = Vec::new();
    let cache = HostCache::load();

    for p in &config.plugins {
        if !p.enabled {
            continue;
        }
        if let Some(plugin) = map.remove(p.id.as_str()) {
            entries.push(Entry {
                plugin,
                keyword: p.keyword.clone(),
                external: false,
                pending: None,
            });
            continue;
        }
        let Some(command) = &p.command else {
            continue;
        };
        let resident = p.resident;
        let meta = cache
            .fresh(command)
            .and_then(|metas| metas.into_iter().find(|m| m.id == p.id));
        let pending = meta.is_none().then(|| PendingHost {
            id: p.id.clone(),
            command: command.clone(),
            resident,
        });
        entries.push(Entry {
            plugin: Box::new(crate::provider::external::External::new(
                &p.id,
                command.clone(),
                meta,
                resident,
            )),
            keyword: p.keyword.clone(),
            external: true,
            pending,
        });
    }

    entries
}

/// Ask the placeholder hosts for their identity, bounded to
/// [`DISCOVERY_CONCURRENCY`] forks and cached; `keyword` limits the walk.
pub(super) async fn resolve_pending(keyword: Option<&str>) {
    resolve_pending_filtered(keyword, None).await;
}

async fn resolve_pending_filtered(keyword: Option<&str>, commands: Option<&[String]>) {
    let _guard = INIT.lock().await;
    let pending: Vec<(usize, PendingHost)> = {
        let reg = REGISTRY.read().await;
        reg.iter()
            .enumerate()
            .filter(|(_, entry)| keyword.is_none_or(|kw| entry.keyword == kw))
            .filter(|(_, entry)| {
                commands.is_none_or(|commands| {
                    entry
                        .pending
                        .as_ref()
                        .is_some_and(|host| commands.contains(&host.command))
                })
            })
            .filter_map(|(index, entry)| entry.pending.clone().map(|host| (index, host)))
            .collect()
    };
    if pending.is_empty() {
        return;
    }

    let mut commands: Vec<(String, bool)> = pending
        .iter()
        .map(|(_, host)| (host.command.clone(), host.resident))
        .collect();
    commands.sort();
    commands.dedup();

    // A command whose cached identity is still fresh is answered from disk;
    // only the rest are forked.
    let mut cache = HostCache::load();
    let mut discovered: HashMap<String, Vec<HostMeta>> = HashMap::default();
    let mut stale: Vec<(String, bool)> = Vec::new();
    for (command, resident) in commands {
        match cache.fresh(&command) {
            Some(metas) => {
                discovered.insert(command, metas);
            }
            None => stale.push((command, resident)),
        }
    }

    if !stale.is_empty() {
        let permits = Arc::new(tokio::sync::Semaphore::new(DISCOVERY_CONCURRENCY));
        let mut hosts = tokio::task::JoinSet::new();
        for (command, resident) in stale {
            let permits = permits.clone();
            hosts.spawn(async move {
                let _permit = permits.acquire_owned().await;
                let metas = crate::provider::external::discover(&command, resident).await;
                (command, metas)
            });
        }
        while let Some(Ok((command, metas))) = hosts.join_next().await {
            // An empty answer is a missing or stalled host, not an identity:
            // keep it out of the cache so the next use retries.
            if !metas.is_empty() {
                cache.record(&command, &metas);
            }
            discovered.insert(command, metas);
        }
        cache.save();
    }

    let mut reg = REGISTRY.write().await;
    for (index, host) in pending {
        let Some(meta) = discovered
            .get(&host.command)
            .and_then(|metas| metas.iter().find(|m| m.id == host.id))
            .cloned()
        else {
            continue; // leave the placeholder; a later use retries
        };
        reg[index].plugin = Box::new(crate::provider::external::External::new(
            &host.id,
            host.command,
            Some(meta),
            host.resident,
        ));
        reg[index].pending = None;
    }
}

/// Print the registry table for `plugin list`; a reader that closes the pipe
/// early (`| head`) ends the listing quietly.
pub async fn print_list() -> io::Result<()> {
    let mut out = io::stdout().lock();
    match table(&mut out).await {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

async fn table(out: &mut impl Write) -> io::Result<()> {
    const ID: usize = 24;
    const NAME: usize = 24;
    const KEYWORD: usize = 12;
    const ICON: usize = 24;
    const STATUS: usize = 10;

    writeln!(
        out,
        "{} {} {} {} {}",
        cell("ID", ID, false),
        cell("NAME", NAME, false),
        cell("KEYWORD", KEYWORD, false),
        cell("ICON", ICON, true),
        cell("STATUS", STATUS, false)
    )?;
    writeln!(
        out,
        "{} {} {} {} {}",
        "-".repeat(ID),
        "-".repeat(NAME),
        "-".repeat(KEYWORD),
        "-".repeat(ICON),
        "-".repeat(STATUS)
    )?;
    for (id, name, icon, keyword, enabled) in list_plugins().await {
        let kw = if keyword.is_empty() {
            "(default)"
        } else {
            &keyword
        };
        let status = if enabled { "enabled" } else { "disabled" };
        writeln!(
            out,
            "{} {} {} {} {}",
            cell(&id, ID, false),
            cell(&name, NAME, false),
            cell(kw, KEYWORD, false),
            cell(&icon, ICON, true),
            cell(status, STATUS, false)
        )?;
    }
    Ok(())
}

/// A cell of `width` display columns, padded or clipped; `tail` keeps the end of
/// a clipped value, for a path whose file name matters more than its root.
fn cell(text: &str, width: usize, tail: bool) -> String {
    let (text, used) = if text.width() <= width {
        (text.to_string(), text.width())
    } else if tail {
        clip_tail(text, width)
    } else {
        clip_head(text, width)
    };
    let padding = width.saturating_sub(used);
    format!("{text}{}", " ".repeat(padding))
}

fn clip_head(text: &str, width: usize) -> (String, usize) {
    let limit = width.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > limit {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    (out, used + 1)
}

fn clip_tail(text: &str, width: usize) -> (String, usize) {
    let limit = width.saturating_sub(1);
    let mut tail = Vec::new();
    let mut used = 0;
    for ch in text.chars().rev() {
        let w = ch.width().unwrap_or(0);
        if used + w > limit {
            break;
        }
        tail.push(ch);
        used += w;
    }
    tail.push('…');
    tail.reverse();
    (tail.into_iter().collect(), used + 1)
}

pub async fn list_plugins() -> Vec<(String, String, String, String, bool)> {
    let map = crate::provider::plugin_map();
    ensure_loaded().await;
    resolve_pending(None).await;
    let config = CONFIG.read().await;
    let reg = REGISTRY.read().await;
    config
        .plugins
        .iter()
        .filter(|p| {
            // Unknown ids without a host are ignored (per the config contract):
            // they are neither built-ins nor declared external plugins.
            p.command.is_some()
                || map.contains_key(p.id.as_str())
                || reg.iter().any(|e| e.plugin.meta().id == p.id.as_str())
        })
        .map(|p| {
            let keyword = p.keyword.clone();
            if let Some(entry) = reg.iter().find(|e| e.plugin.meta().id == p.id.as_str()) {
                let m = entry.plugin.meta();
                (
                    p.id.clone(),
                    m.name.clone(),
                    m.icon.to_string(),
                    keyword,
                    p.enabled,
                )
            } else if let Some(meta) = map.get(p.id.as_str()).map(|plugin| plugin.meta()) {
                // disabled built-in: still listed from the compiled map
                (
                    p.id.clone(),
                    meta.name.clone(),
                    meta.icon.to_string(),
                    keyword,
                    p.enabled,
                )
            } else {
                // disabled (or undiscoverable) external plugin: no host contact
                (
                    p.id.clone(),
                    p.id.clone(),
                    String::new(),
                    keyword,
                    p.enabled,
                )
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Config {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn a_cell_fits_its_display_width() {
        // a CJK name takes two columns per character
        let name = cell("计算器", 10, false);
        assert_eq!(name, "计算器    ");
        assert_eq!(name.width(), 10);

        // a long path keeps its file name
        let path = "/home/me/.config/wayrun/plugins/note/assets/an-icon.svg";
        let icon = cell(path, 20, true);
        assert_eq!(icon.width(), 20);
        assert!(icon.starts_with('…'), "{icon}");
        assert!(icon.trim_end().ends_with("icon.svg"), "{icon}");

        // a long name keeps its start
        let cut = cell("a very long plugin display name", 12, false);
        assert_eq!(cut.width(), 12);
        assert!(cut.trim_end().ends_with('…'), "{cut}");
    }

    #[test]
    fn an_external_plugin_starts_on_a_placeholder_without_a_host_call() {
        let config = parse(
            r#"
            [[plugins]]
            id = "ext"
            keyword = "e"
            command = "/nonexistent/wayrun-test-host"
            "#,
        );
        let entries = build_entries(&config);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].pending.is_some(), "the host is not asked yet");
        // the placeholder identity is the configured id
        assert_eq!(entries[0].plugin.meta().name, "ext");
    }
}
