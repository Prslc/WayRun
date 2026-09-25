use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use super::model::{Config, Entry, HostCache, PendingHost, PluginMap};
use crate::provider::external::HostMeta;

const DEFAULT_CONFIG: &str = include_str!("../../default-plugins.toml");

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
pub async fn reload_if_changed() {
    let _guard = INIT.lock().await;
    let new_config = load_or_default();
    if REGISTRY_READY.load(Ordering::Acquire) && *CONFIG.read().await == new_config {
        return;
    }
    apply(new_config).await;
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
        let meta = cache
            .fresh(command)
            .and_then(|metas| metas.into_iter().find(|m| m.id == p.id));
        let pending = meta.is_none().then(|| PendingHost {
            id: p.id.clone(),
            command: command.clone(),
        });
        entries.push(Entry {
            plugin: Box::new(crate::provider::external::External::new(
                &p.id,
                command.clone(),
                meta,
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
    let _guard = INIT.lock().await;
    let pending: Vec<(usize, PendingHost)> = {
        let reg = REGISTRY.read().await;
        reg.iter()
            .enumerate()
            .filter(|(_, entry)| keyword.is_none_or(|kw| entry.keyword == kw))
            .filter_map(|(index, entry)| entry.pending.clone().map(|host| (index, host)))
            .collect()
    };
    if pending.is_empty() {
        return;
    }

    let mut commands: Vec<String> = pending
        .iter()
        .map(|(_, host)| host.command.clone())
        .collect();
    commands.sort();
    commands.dedup();

    // A command whose cached identity is still fresh is answered from disk;
    // only the rest are forked.
    let mut cache = HostCache::load();
    let mut discovered: HashMap<String, Vec<HostMeta>> = HashMap::default();
    let mut stale: Vec<String> = Vec::new();
    for command in commands {
        match cache.fresh(&command) {
            Some(metas) => {
                discovered.insert(command, metas);
            }
            None => stale.push(command),
        }
    }

    if !stale.is_empty() {
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(DISCOVERY_CONCURRENCY));
        let mut hosts = tokio::task::JoinSet::new();
        for command in stale {
            let permits = permits.clone();
            hosts.spawn(async move {
                let _permit = permits.acquire_owned().await;
                let metas = crate::provider::external::discover(&command).await;
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
        ));
        reg[index].pending = None;
    }
}

fn load_or_default() -> Config {
    let mut config: Config = toml::from_str(DEFAULT_CONFIG).expect("invalid default config");

    if let Ok(home) = crate::system::fs::get_home() {
        let path = home.join(".config/wayrun/plugins.toml");

        ensure_default_config(&path);

        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(user) = toml::from_str::<Config>(&content)
        {
            config = merge_config(config, user);
        }
    }

    config
}

/// The shipped `plugins.toml`, written once; a watcher reload must not recreate
/// the file an editor just replaced.
fn ensure_default_config(path: &std::path::Path) {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = crate::write_if_absent(path, DEFAULT_CONFIG);
    });
}

/// Overlay a user config on the shipped default: known ids take the user's
/// keyword/enabled/command, unknown ids are appended (host-less ones skipped).
fn merge_config(mut base: Config, user: Config) -> Config {
    for up in user.plugins {
        match base.plugins.iter_mut().find(|p| p.id == up.id) {
            Some(dp) => {
                dp.keyword = up.keyword;
                dp.enabled = up.enabled;
                if up.command.is_some() {
                    dp.command = up.command;
                }
            }
            None => base.plugins.push(up),
        }
    }
    base
}

/// Print the registry table (`--list-plugins`).
pub async fn print_list() {
    println!(
        "{:<24} {:<24} {:<12} {:<24} STATUS",
        "ID", "NAME", "KEYWORD", "ICON"
    );
    println!("{:-<24} {:-<24} {:-<12} {:-<24} {:-<8}", "", "", "", "", "");
    for (id, name, icon, keyword, enabled) in list_plugins().await {
        let kw = if keyword.is_empty() {
            "(default)"
        } else {
            &keyword
        };
        let status = if enabled { "" } else { "[disabled]" };
        println!("{id:<24} {name:<24} {kw:<12} {icon:<24}{status}");
    }
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
    fn user_overrides_known_ids() {
        let base = parse(
            r#"
            [[plugins]]
            id = "calculator"
            keyword = ""
            "#,
        );
        let user = parse(
            r#"
            [[plugins]]
            id = "calculator"
            keyword = "calc"
            enabled = false
            "#,
        );
        let merged = merge_config(base, user);
        assert_eq!(merged.plugins.len(), 1);
        assert_eq!(merged.plugins[0].keyword, "calc");
        assert!(!merged.plugins[0].enabled);
        assert!(merged.plugins[0].command.is_none());
    }

    #[test]
    fn user_appends_new_external_ids() {
        let base = parse(
            r#"
            [[plugins]]
            id = "calculator"
            keyword = ""
            "#,
        );
        let user = parse(
            r#"
            [[plugins]]
            id = "translate"
            keyword = "tr"
            command = "ext-host"
            "#,
        );
        let merged = merge_config(base, user);
        assert_eq!(merged.plugins.len(), 2);
        let external = &merged.plugins[1];
        assert_eq!(external.id, "translate");
        assert_eq!(external.keyword, "tr");
        assert_eq!(external.command.as_deref(), Some("ext-host"));
    }

    #[test]
    fn user_command_overrides_default_command() {
        let base = parse(
            r#"
            [[plugins]]
            id = "github"
            keyword = "g"
            command = "old-host"
            "#,
        );
        let user = parse(
            r#"
            [[plugins]]
            id = "github"
            keyword = "g"
            command = "ext-host"
            "#,
        );
        let merged = merge_config(base, user);
        assert_eq!(merged.plugins[0].command.as_deref(), Some("ext-host"));
        assert_eq!(merged.plugins[0].keyword, "g");
    }

    #[test]
    fn user_omitting_command_keeps_default_command() {
        let base = parse(
            r#"
            [[plugins]]
            id = "github"
            keyword = "g"
            command = "ext-host"
            "#,
        );
        let user = parse(
            r#"
            [[plugins]]
            id = "github"
            keyword = "gh"
            "#,
        );
        let merged = merge_config(base, user);
        assert_eq!(merged.plugins[0].command.as_deref(), Some("ext-host"));
        assert_eq!(merged.plugins[0].keyword, "gh");
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

    #[test]
    fn the_host_identity_cache_goes_stale_when_the_command_changes() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = dir.path().join("host");
        std::fs::write(&cmd, b"x").unwrap();
        let command = cmd.display().to_string();

        let mut cache = HostCache::default();
        let metas = vec![HostMeta {
            id: "ext".into(),
            name: "Ext".into(),
            icon: String::new(),
            ready: String::new(),
        }];
        cache.record(&command, &metas);
        assert!(cache.fresh(&command).is_some());

        // a changed file is a stale identity, so the host is asked again
        std::fs::write(&cmd, b"xy").unwrap();
        assert!(cache.fresh(&command).is_none());

        // a command that no longer exists is never fresh
        std::fs::remove_file(&cmd).unwrap();
        assert!(cache.fresh(&command).is_none());
    }
}
