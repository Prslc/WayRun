use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;

use crate::provider::external::HostMeta;
use crate::system::icon::find_icon_path;
use crate::wire::ResultItem;

mod model;
use model::{Config, HostCache, PendingHost};

pub use model::Meta;

const DEFAULT_CONFIG: &str = include_str!("../default-plugins.toml");

pub trait Plugin: Send + Sync {
    fn meta(&self) -> &Meta;
    fn search(
        &self,
        query: &str,
        full: &str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<ResultItem>>> + Send + '_>>;
    /// Default view for a keyword-only query. `Ok(None)` keeps the identity card;
    /// external hosts override it with their `top` method.
    #[allow(clippy::type_complexity)] // same hand-rolled future type as `search`
    fn default_view(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<Vec<ResultItem>>>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }

    /// Drop a row's data (best effort, from `forget`); usage history is the
    /// caller's. `true` means this provider owned the row and dropped it.
    fn forget(
        &self,
        _on_click: &str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<bool>> + Send + '_>> {
        Box::pin(async { Ok(false) })
    }

    /// Type-specific commands for one of this plugin's rows, shown in the action
    /// panel. The core adds pin/unpin and history removal itself.
    fn actions(&self, _item: &ResultItem) -> Vec<crate::wire::ActionItem> {
        Vec::new()
    }
}

type PluginMap = HashMap<&'static str, Box<dyn Plugin>>;

struct Entry {
    plugin: Box<dyn Plugin>,
    keyword: String,
    /// Set while an external plugin runs on its placeholder identity, so startup
    /// forks nothing; `resolve_pending` clears it on first use.
    pending: Option<PendingHost>,
}

/// How many external hosts may be forked at once while resolving identities.
/// Each is a fresh interpreter, so this is the memory ceiling of the walk.
const DISCOVERY_CONCURRENCY: usize = 2;

static CONFIG: tokio::sync::RwLock<Config> = tokio::sync::RwLock::const_new(Config {
    plugins: Vec::new(),
});
static REGISTRY: tokio::sync::RwLock<Vec<Entry>> = tokio::sync::RwLock::const_new(Vec::new());
static REGISTRY_READY: AtomicBool = AtomicBool::new(false);
/// Serializes the one-time build and config reloads (both take INIT first).
static INIT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Lazy first build: load config + registry on first use, once.
async fn ensure_loaded() {
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
    *CONFIG.write().await = new_config;
    *REGISTRY.write().await = entries;
    REGISTRY_READY.store(true, Ordering::Release);
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
        if p.id == "web-search" {
            entries.push(Entry {
                plugin: Box::new(crate::provider::web::WebSearch::new()),
                keyword: p.keyword.clone(),
                pending: None,
            });
            continue;
        }
        if let Some(plugin) = map.remove(p.id.as_str()) {
            entries.push(Entry {
                plugin,
                keyword: p.keyword.clone(),
                pending: None,
            });
            continue;
        }
        let Some(command) = &p.command else {
            continue; // unknown id without an external host -> skipped
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
            pending,
        });
    }

    entries
}

/// Ask the placeholder hosts for their identity, bounded to
/// [`DISCOVERY_CONCURRENCY`] forks and cached; `keyword` limits the walk.
async fn resolve_pending(keyword: Option<&str>) {
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
                || p.id == "web-search"
                || map.contains_key(p.id.as_str())
                || reg.iter().any(|e| e.plugin.meta().id == p.id.as_str())
        })
        .map(|p| {
            let keyword = p.keyword.clone();
            if let Some(entry) = reg.iter().find(|e| e.plugin.meta().id == p.id.as_str()) {
                let m = entry.plugin.meta();
                (
                    p.id.clone(),
                    m.name.to_string(),
                    m.icon.to_string(),
                    keyword,
                    p.enabled,
                )
            } else if p.id == "web-search" {
                let m = crate::provider::web::meta_for(&crate::config::web_search_engine());
                (
                    p.id.clone(),
                    m.name.to_string(),
                    m.icon.to_string(),
                    keyword,
                    p.enabled,
                )
            } else if let Some(meta) = map.get(p.id.as_str()).map(|plugin| plugin.meta()) {
                // disabled built-in: still listed from the compiled map
                (
                    p.id.clone(),
                    meta.name.to_string(),
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

/// Drop a row across the registry. `true` when an external host owned and dropped
/// it, so `forget` answers truthfully instead of claiming a deletion.
pub async fn forget_row(on_click: &str) -> bool {
    ensure_loaded().await;
    let reg = REGISTRY.read().await;
    let mut owned = false;
    for entry in reg.iter() {
        owned |= entry.plugin.forget(on_click).await.unwrap_or(false);
    }
    owned
}

/// Run a search and surface its action panel: the query's pins are prepended
/// and every row is decorated with its secondary commands.
pub async fn dispatch(input: &str) -> Vec<ResultItem> {
    let items = search(input).await;
    let Some(scope) = pin_scope(input) else {
        return items;
    };
    decorate(items, scope, false).await
}

/// A pin's scope is the exact trimmed query, so a bare keyword never summons it;
/// `?` is help, not a result set, so it has no pins.
fn pin_scope(input: &str) -> Option<&str> {
    let input = input.trim();
    (input != "?").then_some(input)
}

/// Prepend the scope's pins and attach each row's actions. A pinned row is
/// re-emitted from storage, so its fresh copy is dropped as a duplicate.
pub async fn decorate(items: Vec<ResultItem>, scope: &str, history: bool) -> Vec<ResultItem> {
    let pins: Vec<ResultItem> = crate::system::pins::get_pins(scope)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect();
    let (mut out, pinned) = merge_pins(pins, items);

    for item in &mut out {
        let plugin_actions = plugin_actions(item).await;
        attach_actions(item, scope, &pinned, history, plugin_actions);
    }
    out
}

/// Pins lead the results, deduplicated, so each appears once at the top. Returns
/// the merged list and the pinned `on_click` keys the action labels use.
fn merge_pins(
    mut pins: Vec<ResultItem>,
    mut results: Vec<ResultItem>,
) -> (Vec<ResultItem>, Vec<String>) {
    let pinned: Vec<String> = pins
        .iter()
        .filter_map(|item| item.on_click.clone())
        .collect();
    results.retain(|item| {
        !item
            .on_click
            .as_deref()
            .is_some_and(|on_click| pinned.iter().any(|pin| pin == on_click))
    });
    pins.append(&mut results);
    (pins, pinned)
}

/// The first plugin that recognises the row and declares actions for it, from
/// its provider or the host's inline `actions`.
async fn plugin_actions(item: &ResultItem) -> Vec<crate::wire::ActionItem> {
    let reg = REGISTRY.read().await;
    for entry in reg.iter() {
        let actions = entry.plugin.actions(item);
        if !actions.is_empty() {
            return actions;
        }
    }
    Vec::new()
}

/// The launcher-level commands an actionable row gets (pin/unpin always, history
/// removal only on a recordable history row), before its type and host actions.
fn attach_actions(
    item: &mut ResultItem,
    scope: &str,
    pinned: &[String],
    history: bool,
    mut plugin_actions: Vec<crate::wire::ActionItem>,
) {
    let Some(on_click) = item.on_click.clone() else {
        return;
    };
    let is_pinned = pinned.iter().any(|pin| pin == &on_click);
    if is_pinned {
        item.badge = find_icon_path("pin");
    }

    let mut actions: Vec<crate::wire::ActionItem> = Vec::new();
    if is_pinned {
        let payload = json!({ "scope": scope, "on_click": on_click });
        actions.push(crate::wire::ActionItem {
            title: "Unpin".to_string(),
            on_click: format!("unpin:{payload}"),
            icon: Some("window-unpin".to_string()),
        });
    } else if !item.ephemeral {
        // Snapshot the row before the launcher actions are appended, so the pin
        // never embeds the action that stores it. A host's own actions stay.
        let payload = json!({ "scope": scope, "item": item });
        actions.push(crate::wire::ActionItem {
            title: "Pin to top".to_string(),
            on_click: format!("pin:{payload}"),
            icon: Some("pin".to_string()),
        });
    }

    if history && crate::system::usage::is_recordable(item.ephemeral, Some(&on_click)) {
        actions.push(crate::wire::ActionItem {
            title: "Remove from history".to_string(),
            on_click: format!("forget:{on_click}"),
            icon: Some("edit-delete".to_string()),
        });
    }

    actions.append(&mut plugin_actions);
    actions.append(&mut item.actions);

    // Resolve every icon spec (built-in or host-supplied) to the absolute path
    // the shell renders; an unresolved action keeps no icon.
    for action in &mut actions {
        action.icon = action
            .icon
            .as_deref()
            .filter(|spec| !spec.is_empty())
            .and_then(find_icon_path);
    }

    item.actions = actions;
}

async fn search(input: &str) -> Vec<ResultItem> {
    ensure_loaded().await;
    // A search that names a keyword only wakes that plugin's host; `?` lists
    // every name, so it resolves them all. A plain query wakes none.
    if input.trim() == "?" {
        resolve_pending(None).await;
    } else if let Some((keyword, _)) = input.split_once(' ')
        && !keyword.trim().is_empty()
    {
        resolve_pending(Some(keyword.trim())).await;
    }
    let reg = REGISTRY.read().await;

    if input.trim() == "?" {
        return reg
            .iter()
            .map(|entry| {
                let meta = entry.plugin.meta();
                let usage = if entry.keyword.is_empty() {
                    "* (default)".to_string()
                } else {
                    format!("{} <query>", entry.keyword)
                };
                ResultItem {
                    title: meta.name.to_string(),
                    summary: Some(format!("{usage} - {}", meta.ready)),
                    on_click: None,
                    icon: find_icon_path(meta.icon).or_else(|| Some(String::new())),
                    ephemeral: false,
                    actions: Vec::new(),
                    badge: None,
                }
            })
            .collect();
    }

    let (keyword, query) = input
        .split_once(' ')
        .map_or(("", input), |(k, q)| (k.trim(), q.trim()));

    // A keyword some plugin owns is its own namespace: a miss stays empty and
    // never falls through to the default providers.
    let routed = !keyword.is_empty() && reg.iter().any(|entry| entry.keyword == keyword);

    if routed {
        if query.is_empty()
            && let Some(entry) = reg.iter().find(|entry| entry.keyword == keyword)
        {
            // Keyword-only input opens the plugin: a host may serve a default
            // view (`top`), else the identity card.
            if let Ok(Some(items)) = entry.plugin.default_view().await
                && !items.is_empty()
            {
                return fill_icons(entry.plugin.meta().icon, items);
            }
            let meta = entry.plugin.meta();
            return vec![ResultItem {
                title: meta.name.to_string(),
                summary: Some(meta.ready.to_string()),
                on_click: None,
                icon: find_icon_path(meta.icon).or_else(|| Some(String::new())),
                ephemeral: false,
                actions: Vec::new(),
                badge: None,
            }];
        }

        for entry in reg.iter().filter(|e| e.keyword == keyword) {
            if let Ok(results) = entry.plugin.search(query, input).await
                && !results.is_empty()
            {
                return fill_icons(entry.plugin.meta().icon, results);
            }
        }

        return vec![];
    }

    for entry in reg.iter().filter(|e| e.keyword.is_empty()) {
        if let Ok(results) = entry.plugin.search(query, input).await
            && !results.is_empty()
        {
            return fill_icons(entry.plugin.meta().icon, results);
        }
    }

    vec![]
}

/// Rows a provider left iconless take its identity icon, so a command or window
/// row never shows the app placeholder before the shell sees it. Running before
/// `decorate`, this is also what pins and usage history store.
fn fill_icons(meta_icon: &str, mut items: Vec<ResultItem>) -> Vec<ResultItem> {
    let fallback = find_icon_path(meta_icon);
    for item in &mut items {
        if item.icon.as_deref().is_none_or(str::is_empty) {
            item.icon.clone_from(&fallback);
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{ActionItem, ResultItem};

    fn parse(src: &str) -> Config {
        toml::from_str(src).unwrap()
    }

    fn item(title: &str, on_click: &str) -> ResultItem {
        ResultItem {
            title: title.to_string(),
            summary: None,
            on_click: Some(on_click.to_string()),
            icon: None,
            ephemeral: false,
            actions: Vec::new(),
            badge: None,
        }
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
        // keyword untouched when the user only sets command
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

    #[test]
    fn rows_without_an_icon_take_the_plugin_identity_icon() {
        let mut items = vec![item("blank", "run:blank"), item("kept", "run:kept")];
        items[0].icon = Some(String::new());
        items[1].icon = Some("/tmp/kept.svg".into());

        let filled = fill_icons("utilities-terminal", items);
        assert_eq!(
            filled[0].icon,
            find_icon_path("utilities-terminal"),
            "an empty spec is a miss, not an icon"
        );
        assert_eq!(filled[1].icon.as_deref(), Some("/tmp/kept.svg"));
    }

    #[test]
    fn a_pin_is_scoped_to_the_exact_query_not_its_keyword() {
        assert_eq!(pin_scope("b firefox"), Some("b firefox"));
        assert_eq!(pin_scope("b"), Some("b"));
        assert_eq!(pin_scope("  firefox  "), Some("firefox"));
        // the empty query is the history view, which still has its own scope
        assert_eq!(pin_scope(""), Some(""));
        // `?` is the help table, never a pin scope
        assert_eq!(pin_scope("?"), None);
        assert_eq!(pin_scope("  ?  "), None);
    }

    #[test]
    fn pinned_rows_lead_and_their_duplicate_is_dropped() {
        let pins = vec![item("Pinned", "run:pinned")];
        let results = vec![item("Other", "run:other"), item("Pinned", "run:pinned")];

        let (merged, pinned) = merge_pins(pins, results);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].title, "Pinned");
        assert_eq!(merged[1].title, "Other");
        assert_eq!(pinned, ["run:pinned"]);
    }

    #[test]
    fn launcher_actions_lead_the_row_and_carry_its_scope() {
        let mut row = item("Firefox", "launch:firefox.desktop");
        attach_actions(&mut row, "b", &[], false, Vec::new());

        let titles: Vec<&str> = row
            .actions
            .iter()
            .map(|action| action.title.as_str())
            .collect();
        assert_eq!(titles, ["Pin to top"]);
        assert!(row.actions[0].on_click.starts_with("pin:"));
        assert!(row.actions[0].on_click.contains(r#""scope":"b""#));
        assert!(row.badge.is_none(), "an unpinned row carries no badge");
    }

    #[test]
    fn a_pinned_row_offers_unpin_before_its_type_actions() {
        let mut row = item("a.txt", "file:///tmp/a.txt");
        let reveal = ActionItem {
            title: "Reveal in file manager".to_string(),
            on_click: "reveal:file:///tmp/a.txt".to_string(),
            icon: None,
        };
        attach_actions(
            &mut row,
            "",
            &["file:///tmp/a.txt".to_string()],
            true,
            vec![reveal],
        );

        let titles: Vec<&str> = row
            .actions
            .iter()
            .map(|action| action.title.as_str())
            .collect();
        assert_eq!(
            titles,
            ["Unpin", "Remove from history", "Reveal in file manager"]
        );
        assert!(row.badge.is_some(), "a pinned row carries the pin badge");
    }

    #[test]
    fn the_pin_snapshot_keeps_host_actions_but_not_launcher_ones() {
        let mut row = item("Firefox", "launch:firefox.desktop");
        row.actions = vec![ActionItem {
            title: "Host".to_string(),
            on_click: "run:host".to_string(),
            icon: None,
        }];
        attach_actions(&mut row, "b", &[], false, Vec::new());

        // The snapshot is the host's row, not the decorated one: the pin action
        // must not embed itself, and the host action must survive the round trip.
        let payload = row.actions[0].on_click.clone();
        assert!(payload.contains(r#""title":"Host""#));
        assert!(!payload.contains("Pin to top"));
    }

    #[test]
    fn launcher_entries_follow_the_row() {
        fn titles(row: &ResultItem) -> Vec<&str> {
            row.actions.iter().map(|a| a.title.as_str()).collect()
        }

        let mut history = item("Firefox", "launch:firefox.desktop");
        attach_actions(&mut history, "", &[], true, Vec::new());
        assert_eq!(titles(&history), ["Pin to top", "Remove from history"]);

        // a fresh search result is not sourced from the history view
        let mut search = item("Firefox", "launch:firefox.desktop");
        attach_actions(&mut search, "fire", &[], false, Vec::new());
        assert_eq!(titles(&search), ["Pin to top"]);

        // an ephemeral row is neither a durable pin target nor recorded
        let mut one_shot = item("window", "run:wctl activate 1");
        one_shot.ephemeral = true;
        attach_actions(&mut one_shot, "", &[], true, Vec::new());
        assert!(titles(&one_shot).is_empty());

        // an existing pin must stay removable even on an ephemeral row
        let mut pinned = item("clip", "run:cliphist decode 1");
        pinned.ephemeral = true;
        attach_actions(
            &mut pinned,
            "",
            &["run:cliphist decode 1".to_string()],
            true,
            Vec::new(),
        );
        assert_eq!(titles(&pinned), ["Unpin"]);

        // a copy: row is not recorded, but it is a stable pin target
        let mut copy = item("translated", r#"copy:{"text":"hi"}"#);
        attach_actions(&mut copy, "", &[], true, Vec::new());
        assert_eq!(titles(&copy), ["Pin to top"]);
    }
}
