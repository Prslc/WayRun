use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::plugin::{Meta, Plugin};
use crate::system::icon::host_icon_path;
use crate::wire::{Action, ResultItem};
use rust_i18n::t;

/// Identity of one plugin as described by an external host's `list_plugins`.
/// The host owns its own name/icon/ready hint; the core just relays them.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct HostMeta {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub ready: String,
}

/// `(mtime, size)` of the resolved `command`, to validate a cached identity.
/// `None` when it cannot be resolved or stat'd, so the cache is not trusted.
pub fn command_stamp(command: &str) -> Option<(u64, u64)> {
    let path = resolve_command(command);
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((mtime, meta.len()))
}

/// A plugin backed by an external JSON-RPC host declared via `command`. The core
/// is a generic client: it spawns the host and relays `search` to it.
pub struct External {
    meta: Meta,
    command: String,
    resident: bool,
}

impl External {
    /// Build from the configured id, host command and discovered identity. A
    /// missing identity degrades to the id as display name.
    pub fn new(id: &str, command: String, discovered: Option<HostMeta>, resident: bool) -> Self {
        let (name, icon, ready) = match discovered {
            Some(m) => (m.name, m.icon, m.ready),
            None => (
                id.to_string(),
                String::new(),
                t!("plugin.external.ready", command = command),
            ),
        };
        // A symbolic identity icon is a miss: a host ships its own absolute paths.
        let icon = host_icon_path(&icon).unwrap_or_default();
        Self {
            meta: Meta {
                id: id.to_string().into(),
                name,
                icon: icon.into(),
                ready,
            },
            command,
            resident,
        }
    }
}

impl Plugin for External {
    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn search(
        &self,
        query: &str,
        _full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        let command = self.command.clone();
        let plugin = self.meta.id.to_string();
        let query = query.to_string();
        let icon = self.meta.icon.to_string();
        let resident = self.resident;
        Box::pin(async move { query_external(&command, resident, &plugin, &query, &icon).await })
    }

    fn default_view(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<ResultItem>>>> + Send + '_>> {
        let command = self.command.clone();
        let plugin = self.meta.id.to_string();
        let icon = self.meta.icon.to_string();
        let resident = self.resident;
        Box::pin(async move { query_default(&command, resident, &plugin, &icon).await })
    }

    fn forget(&self, command: &Action) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + '_>> {
        let host = self.command.clone();
        let command = command.clone();
        let resident = self.resident;
        Box::pin(async move { forget_external(&host, resident, &command).await })
    }
}

/// Ceiling for one host call: a stalled host must cost seconds, never the
/// session. Discovery holds the registry's init lock until it returns.
const HOST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// One call against a host: `resident` keeps the process across calls; the
/// default spawns it fresh, so nothing a host kept outlives the call.
async fn host_call(
    command: &str,
    resident: bool,
    request: &serde_json::Value,
    limit: std::time::Duration,
) -> Option<serde_json::Value> {
    if resident {
        super::resident::call(command, request, limit).await
    } else {
        rpc_call_within(command, request, limit).await
    }
}

/// One fork-per-call round trip: spawn, write, close stdin, reap, return the
/// first parseable response line; `None` on a missing or stalled host.
async fn rpc_call_within(
    command: &str,
    request: &serde_json::Value,
    limit: std::time::Duration,
) -> Option<serde_json::Value> {
    let mut req_str = serde_json::to_string(request).ok()?;
    req_str.push('\n');

    let mut child = Command::new(command)
        // The deadline drops `wait_with_output`'s future; without this the host
        // would keep running (and keep holding whatever it is stuck on).
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?; // command not found -> no host

    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(req_str.as_bytes()).await.is_err() {
            let _ = child.kill().await;
            return None;
        }
        drop(stdin); // close stdin so the host sees EOF (one-shot)
    }

    // `wait_with_output` drains stdout/stderr and reaps the child in one step;
    // reading stdout then `wait()` separately can double-poll the join handle.
    let output = match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            // The core has no logging crate; stderr reaches the unit's journal.
            eprintln!("wayrun-core: external host {command} failed: {err}");
            return None;
        }
        Err(_) => {
            eprintln!(
                "wayrun-core: external host {command} overran {}ms and was killed",
                limit.as_millis()
            );
            return None;
        }
    };

    let out = std::str::from_utf8(&output.stdout).unwrap_or_default();
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
}

/// Ask the host who it serves. Empty when the host is missing, stalled or not
/// self-describing, so a query for it falls through instead of hanging.
pub async fn discover(command: &str, resident: bool) -> Vec<HostMeta> {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "list_plugins",
        "id": 1,
    });
    let Some(response) = host_call(command, resident, &request, HOST_TIMEOUT).await else {
        eprintln!(
            "wayrun-core: external host {command} did not answer list_plugins; its plugins stay unregistered"
        );
        return Vec::new();
    };
    let Some(list) = response.get("result").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| {
            Some(HostMeta {
                id: entry.get("id")?.as_str()?.to_string(),
                name: entry
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                icon: entry
                    .get("icon")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                ready: entry
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

/// A host `result` array as rows: icons resolved and `plugin` stamped as the
/// actions' owner; `None` if unusable. Consumes the reply, so no row is cloned.
fn parse_result_items(
    response: serde_json::Value,
    plugin: &str,
    icon: &str,
) -> Option<Vec<ResultItem>> {
    let serde_json::Value::Object(mut object) = response else {
        return None;
    };
    let items = match object.remove("result")? {
        serde_json::Value::Array(items) => items,
        _ => return None,
    };
    let mut parsed: Vec<ResultItem> = items
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect();
    let identity = host_icon_path(icon);
    let owner = plugin.to_string();
    for item in &mut parsed {
        let spec = item.icon.as_deref().unwrap_or("");
        item.icon = resolve_item_icon(spec, identity.clone());
        item.badge = item.badge.as_deref().and_then(host_icon_path);
        for action in &mut item.actions {
            action.icon = action.icon.as_deref().and_then(host_icon_path);
            // The core owns this field; whatever the host sent is ignored.
            action.plugin = Some(owner.clone());
        }
    }
    Some(parsed)
}

async fn query_external(
    command: &str,
    resident: bool,
    plugin: &str,
    text: &str,
    icon: &str,
) -> Result<Vec<ResultItem>> {
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "search",
        "params": { "plugin": plugin, "text": text },
        "id": 1,
    });
    let Some(response) = host_call(command, resident, &request, HOST_TIMEOUT).await else {
        return Ok(Vec::new());
    };
    Ok(parse_result_items(response, plugin, icon).unwrap_or_default())
}

/// Ask the host for its default view (its `top` method). `Ok(None)` when it has
/// no such method, errored, or returned nothing, so the caller shows the card.
async fn query_default(
    command: &str,
    resident: bool,
    plugin: &str,
    icon: &str,
) -> Result<Option<Vec<ResultItem>>> {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "top",
        "params": { "plugin": plugin },
        "id": 1,
    });
    let Some(response) = host_call(command, resident, &request, HOST_TIMEOUT).await else {
        return Ok(None);
    };
    if response.get("error").is_some() {
        return Ok(None);
    }
    Ok(parse_result_items(response, plugin, icon))
}

/// First shell token of a `run` command (argv0), or `None` for any other
/// variant; hosts emit single-token commands, so a whitespace split suffices.
fn run_argv0(command: &Action) -> Option<&str> {
    let Action::Run { cmd } = command else {
        return None;
    };
    cmd.split_whitespace().next()
}

/// Resolve a plugins.toml `command` to an absolute path when it is a bare
/// name (PATH lookup); absolute paths pass through unchanged.
fn resolve_command(command: &str) -> String {
    if command.contains('/') {
        return command.to_string();
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(command);
            if candidate.is_file() {
                return candidate.display().to_string();
            }
        }
    }
    command.to_string()
}

/// Relay a row's removal to its host: the command must be a `run` whose first
/// token is this host's `command`. `true` when the host acknowledged it.
async fn forget_external(command: &str, resident: bool, row: &Action) -> Result<bool> {
    let Some(argv0) = run_argv0(row) else {
        return Ok(false);
    };
    let resolved = resolve_command(command);
    if argv0 != command && argv0 != resolved {
        return Ok(false);
    }
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "forget",
        "params": { "on_click": row },
        "id": 1,
    });
    let response = host_call(command, resident, &request, HOST_TIMEOUT).await;
    Ok(response.is_some_and(|reply| reply.get("error").is_none()))
}
fn resolve_item_icon(icon: &str, fallback: Option<String>) -> Option<String> {
    host_icon_path(icon).or(fallback)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_icon_falls_back_to_plugin_icon() {
        assert_eq!(
            resolve_item_icon("", Some("/x.png".into())),
            Some("/x.png".into())
        );
        assert_eq!(resolve_item_icon("", None), None);
    }

    #[test]
    fn absolute_path_passes_through() {
        assert_eq!(
            resolve_item_icon("/home/u/icon.svg", None),
            Some("/home/u/icon.svg".into())
        );
    }

    #[test]
    fn an_unresolved_spec_prefers_the_identity_icon() {
        assert_eq!(
            resolve_item_icon("definitely-not-an-icon-xyz", Some("/identity.png".into())),
            Some("/identity.png".into())
        );
    }

    #[test]
    fn a_symbolic_spec_is_no_icon_for_a_host() {
        // the core resolves these for its own rows, but not for an external host
        assert_eq!(resolve_item_icon("papirus:folder-open", None), None);
        assert_eq!(resolve_item_icon("builtin:power", None), None);
        assert_eq!(resolve_item_icon("firefox", None), None);
    }

    #[test]
    fn a_hosts_action_and_badge_icons_must_be_absolute() {
        let response = serde_json::json!({
            "result": [{
                "title": "r",
                "on_click": {"type":"run","cmd":"x"},
                "badge": "builtin:pin",
                "actions": [
                    {"title":"a", "action":{"type":"execute","command":{"type":"run","cmd":"y"}}, "icon": "builtin:open"},
                    {"title":"b", "action":{"type":"execute","command":{"type":"run","cmd":"z"}}, "icon": "/tmp/a.svg"},
                ],
            }]
        });
        let items = parse_result_items(response, "system-search", "").unwrap();
        assert!(items[0].badge.is_none(), "a symbolic badge is dropped");
        assert_eq!(
            items[0].actions[0].plugin.as_deref(),
            Some("system-search"),
            "the host owns the actions it attaches"
        );
        assert!(
            items[0].actions[0].icon.is_none(),
            "a symbolic action icon is dropped"
        );
        assert_eq!(items[0].actions[1].icon.as_deref(), Some("/tmp/a.svg"));
    }

    #[test]
    fn an_ephemeral_host_row_stays_ephemeral() {
        let response = serde_json::json!({
            "result": [
                { "title": "repo", "on_click": {"type":"open","uri":"https://github.com/x/y"}, "ephemeral": true },
                { "title": "Firefox", "on_click": {"type":"launch","desktop_id":"firefox.desktop"} },
            ]
        });
        let items = parse_result_items(response, "system-search", "").unwrap();
        assert!(items[0].ephemeral, "the host's flag is carried through");
        assert!(!items[1].ephemeral, "an absent flag means record it");
    }

    /// The golden corpus the Python SDK pins on its side (WayRun-Plugin,
    /// `tests/test_golden.py`); both ends accept the same shapes.
    #[test]
    fn every_command_variant_parses_from_a_host() {
        let response = serde_json::json!({
            "result": [
                {"title": "run", "on_click": {"type": "run", "cmd": "echo hi"}},
                {"title": "terminal run", "on_click": {"type": "run_in_terminal", "cmd": "htop"}},
                {"title": "open", "on_click": {"type": "open", "uri": "https://example.com"}},
                {"title": "copy", "on_click": {"type": "copy", "text": "x"}},
                {"title": "launch", "on_click": {"type": "launch", "desktop_id": "firefox.desktop"}},
                {"title": "desktop action", "on_click": {"type": "desktop_action", "desktop_id": "firefox.desktop", "action_id": "new-private-window"}},
                {"title": "reveal", "on_click": {"type": "reveal", "uri": "file:///home/u"}},
                {"title": "terminal", "on_click": {"type": "terminal", "uri": "file:///home/u"}},
                {"title": "ephemeral", "ephemeral": true},
                {"title": "nulls", "summary": null, "on_click": null, "icon": null},
            ]
        });
        let items = parse_result_items(response, "golden", "/opt/identity.svg").unwrap();
        assert_eq!(items.len(), 10, "every corpus row survives");
        assert_eq!(
            items[0].on_click,
            Some(Action::Run {
                cmd: "echo hi".to_string()
            })
        );
        assert_eq!(
            items[1].on_click,
            Some(Action::RunInTerminal {
                cmd: "htop".to_string()
            })
        );
        assert_eq!(
            items[2].on_click,
            Some(Action::Open {
                uri: "https://example.com".to_string()
            })
        );
        assert_eq!(
            items[3].on_click,
            Some(Action::Copy {
                text: "x".to_string()
            })
        );
        assert_eq!(
            items[4].on_click,
            Some(Action::Launch {
                desktop_id: "firefox.desktop".to_string()
            })
        );
        assert_eq!(
            items[5].on_click,
            Some(Action::DesktopAction {
                desktop_id: "firefox.desktop".to_string(),
                action_id: "new-private-window".to_string()
            })
        );
        assert_eq!(
            items[6].on_click,
            Some(Action::Reveal {
                uri: "file:///home/u".to_string()
            })
        );
        assert_eq!(
            items[7].on_click,
            Some(Action::Terminal {
                uri: "file:///home/u".to_string()
            })
        );
        assert!(items[8].ephemeral);
        assert!(items[9].summary.is_none() && items[9].on_click.is_none());
        assert_eq!(
            items[9].icon.as_deref(),
            Some("/opt/identity.svg"),
            "an unset icon falls back to the identity"
        );
    }

    /// A host that answers the way a plugin framework does: it consumes the
    /// request and prints one response line.
    fn host(dir: &tempfile::TempDir, reply: &str) -> String {
        use std::io::Write;
        let path = dir.path().join("host.sh");
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "#!/bin/sh").unwrap();
        writeln!(file, "cat >/dev/null").unwrap();
        writeln!(file, "printf '%s\\n' '{reply}'").unwrap();
        drop(file);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path.display().to_string()
    }

    #[tokio::test]
    async fn an_empty_query_never_reaches_the_host() {
        // No host is contacted: an empty text is not a search.
        let items = query_external("/nonexistent/host", false, "p", "", "")
            .await
            .unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn a_row_another_command_owns_is_left_alone() {
        // No host is contacted: the on_click's command is not this one.
        let row = Action::Run {
            cmd: "/bin/other del 1".to_string(),
        };
        let owned = forget_external("/usr/bin/definitely-not-this", false, &row)
            .await
            .unwrap();
        assert!(!owned);
    }

    #[tokio::test]
    async fn a_host_that_answers_owns_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let command = host(&dir, r#"{"jsonrpc":"2.0","result":null,"id":1}"#);
        let row = Action::Run {
            cmd: format!("{command} del 1"),
        };
        let owned = forget_external(&command, false, &row).await.unwrap();
        assert!(owned, "the host dropped its own data, so the row may leave");
    }

    #[tokio::test]
    async fn a_host_without_a_forget_method_disowns_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let reply =
            r#"{"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":1}"#;
        let command = host(&dir, reply);
        let row = Action::Run {
            cmd: format!("{command} del 1"),
        };
        let owned = forget_external(&command, false, &row).await.unwrap();
        assert!(!owned, "-32601 means the row is not this host's to drop");
    }

    /// A host that never answers must cost the deadline, not the session.
    #[tokio::test]
    async fn a_stalled_host_is_given_up_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stall.sh");
        std::fs::write(&path, "#!/bin/sh\ncat >/dev/null\nsleep 600\n").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();

        let request = serde_json::json!({"jsonrpc": "2.0", "method": "search", "id": 1});
        let limit = std::time::Duration::from_millis(200);
        let started = std::time::Instant::now();
        let reply = rpc_call_within(&path.display().to_string(), &request, limit).await;
        assert!(reply.is_none(), "a stalled host has no answer");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "gave up on the deadline, not on the host: {:?}",
            started.elapsed()
        );
    }
}
