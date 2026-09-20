use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::plugin::{Meta, Plugin};
use crate::system::icon::find_icon_path;
use crate::wire::{Action, ResultItem};

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
}

/// The registry is built once per process, so these small strings live exactly
/// the process lifetime that `Meta`'s `&'static str` requires.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

impl External {
    /// Build from the configured id, host command and discovered identity. A
    /// missing identity degrades to the id as display name.
    pub fn new(id: &str, command: String, discovered: Option<HostMeta>) -> Self {
        let (name, icon, ready) = match discovered {
            Some(m) => (m.name, m.icon, m.ready),
            None => (
                id.to_string(),
                String::new(),
                format!("External plugin via {command}"),
            ),
        };
        // The UI renders only absolute paths, so a `papirus:` spec or theme name
        // is resolved here rather than leaking into `Meta`.
        let icon = if icon.is_empty() {
            icon
        } else {
            find_icon_path(&icon).unwrap_or_default()
        };
        Self {
            meta: Meta {
                id: leak(id.to_string()),
                name: leak(name),
                icon: leak(icon),
                ready: leak(ready),
            },
            command,
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
        Box::pin(async move { query_external(&command, &plugin, &query, &icon).await })
    }

    fn default_view(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<ResultItem>>>> + Send + '_>> {
        let command = self.command.clone();
        let plugin = self.meta.id.to_string();
        let icon = self.meta.icon.to_string();
        Box::pin(async move { query_default(&command, &plugin, &icon).await })
    }

    fn forget(&self, command: &Action) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + '_>> {
        let host = self.command.clone();
        let command = command.clone();
        Box::pin(async move { forget_external(&host, &command).await })
    }
}

/// Ceiling for one host call: a stalled host must cost seconds, never the
/// session. Discovery holds the registry's init lock until it returns.
const HOST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// One JSON-RPC round trip against `command`: spawn, write, close stdin, reap,
/// return the first parseable response line; `None` on a missing/stalled host.
async fn rpc_call(command: &str, request: &serde_json::Value) -> Option<serde_json::Value> {
    rpc_call_within(command, request, HOST_TIMEOUT).await
}

/// [`rpc_call`] with an explicit deadline, so the stall path is testable.
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
pub async fn discover(command: &str) -> Vec<HostMeta> {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "list_plugins",
        "id": 1,
    });
    let Some(response) = rpc_call(command, &request).await else {
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

/// Normalize a host response's `result` array into rows, resolving each icon to
/// what the UI can render. `None` when there is no usable `result` array.
fn parse_result_items(response: &serde_json::Value, icon: &str) -> Option<Vec<ResultItem>> {
    let items = response.get("result")?.as_array()?;
    let mut parsed: Vec<ResultItem> = items
        .iter()
        .filter_map(|it| serde_json::from_value(it.clone()).ok())
        .collect();
    let icon_path = find_icon_path(icon);
    for item in &mut parsed {
        let spec = item.icon.as_deref().unwrap_or("");
        item.icon = resolve_item_icon(spec, icon_path.clone());
    }
    Some(parsed)
}

async fn query_external(
    command: &str,
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
    let Some(response) = rpc_call(command, &request).await else {
        return Ok(Vec::new());
    };
    Ok(parse_result_items(&response, icon).unwrap_or_default())
}

/// Ask the host for its default view (its `top` method). `Ok(None)` when it has
/// no such method, errored, or returned nothing, so the caller shows the card.
async fn query_default(command: &str, plugin: &str, icon: &str) -> Result<Option<Vec<ResultItem>>> {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "top",
        "params": { "plugin": plugin },
        "id": 1,
    });
    let Some(response) = rpc_call(command, &request).await else {
        return Ok(None);
    };
    if response.get("error").is_some() {
        return Ok(None);
    }
    Ok(parse_result_items(&response, icon))
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
async fn forget_external(command: &str, row: &Action) -> Result<bool> {
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
    let response = rpc_call(command, &request).await;
    Ok(response.is_some_and(|reply| reply.get("error").is_none()))
}
/// Resolve one result icon to an absolute path: empty falls back to the plugin's
/// icon, and any spec goes through the one resolver.
fn resolve_item_icon(icon: &str, fallback: Option<String>) -> Option<String> {
    if icon.is_empty() {
        return fallback;
    }
    find_icon_path(icon).or(fallback)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
    fn papirus_spec_is_resolved_to_absolute_path() {
        if !Path::new("/usr/share/icons/Papirus").exists() {
            return; // theme not installed on this machine
        }
        let resolved = resolve_item_icon("papirus:folder-open", None).unwrap();
        assert!(resolved.contains("/Papirus/"));
        assert!(resolved.ends_with(".svg"));
    }

    #[test]
    fn a_theme_name_is_resolved_to_an_absolute_path() {
        // Hosts may name a theme icon rather than a file; the shell only renders
        // absolute paths, so the core resolves it before the row leaves.
        let resolved = resolve_item_icon("firefox", None).unwrap();
        assert!(resolved.starts_with('/'), "{resolved}");
    }

    #[test]
    fn an_ephemeral_host_row_stays_ephemeral() {
        let response = serde_json::json!({
            "result": [
                { "title": "repo", "on_click": {"type":"open","uri":"https://github.com/x/y"}, "ephemeral": true },
                { "title": "Firefox", "on_click": {"type":"launch","desktop_id":"firefox.desktop"} },
            ]
        });
        let items = parse_result_items(&response, "system-search").unwrap();
        assert!(items[0].ephemeral, "the host's flag is carried through");
        assert!(!items[1].ephemeral, "an absent flag means record it");
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
        let items = query_external("/nonexistent/host", "p", "", "")
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
        let owned = forget_external("/usr/bin/definitely-not-this", &row)
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
        let owned = forget_external(&command, &row).await.unwrap();
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
        let owned = forget_external(&command, &row).await.unwrap();
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
