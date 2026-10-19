use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::plugin::{Emit, Meta, Plugin};
use crate::system::icon::host_icon_spec;
use crate::wire::{Preview, ResultItem};
use rust_i18n::t;

/// The id of the next host request; partial notifications carry it back.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Identity of one plugin as described by an external host's `list_plugins`.
/// The host owns its own name/icon/ready hint; the core just relays them.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct HostMeta {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub ready: String,
    pub api: u32,
}

/// The executable behind `command`, when it resolves on `PATH` (or is a path).
pub fn command_path(command: &str) -> Option<PathBuf> {
    resolve_command(command)
}

/// `(mtime, size)` of the resolved `command`, to validate a cached identity.
/// `None` when it cannot be resolved or stat'd, so the cache is not trusted.
pub fn command_stamp(command: &str) -> Option<(u64, u64)> {
    let path = resolve_command(command)?;
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
            Some(m) => {
                if m.api != crate::wire::PLUGIN_API {
                    eprintln!(
                        "wayrun-core: plugin {id}: api {}, this launcher speaks {}",
                        m.api,
                        crate::wire::PLUGIN_API
                    );
                }
                (m.name, m.icon, m.ready)
            }
            None => (
                id.to_string(),
                String::new(),
                t!("plugin.external.ready", command = command),
            ),
        };
        // An unrenderable identity icon is a miss: a host names a file it ships
        // or a compiled glyph, nothing else.
        let icon = host_icon_spec(&icon).unwrap_or_default();
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

    fn search_streaming<'a>(
        &'a self,
        query: &'a str,
        full: &'a str,
        emit: Option<Emit>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + 'a>> {
        if emit.is_none() {
            return self.search(query, full);
        }
        let command = self.command.clone();
        let plugin = self.meta.id.to_string();
        let query = query.to_string();
        let icon = self.meta.icon.to_string();
        let resident = self.resident;
        Box::pin(async move {
            query_external_streaming(&command, resident, &plugin, &query, &icon, emit).await
        })
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
}

/// Ceiling for one host call: a stalled host must cost seconds, never the
/// session. Discovery holds the registry's init lock until it returns.
pub(crate) const HOST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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

/// A call that forwards the host's `partial` notifications as they arrive; a
/// resident host's reader task runs the callback, a forked host's reader too.
async fn host_call_streaming(
    command: &str,
    resident: bool,
    request: &serde_json::Value,
    limit: std::time::Duration,
    partial: impl FnMut(serde_json::Value) + Send + 'static,
) -> Option<serde_json::Value> {
    if resident {
        super::resident::call_stream(command, request, limit, partial).await
    } else {
        let mut partial = partial;
        rpc_call_streaming(command, request, limit, &mut partial).await
    }
}

/// One fork-per-call round trip with no partial sink.
async fn rpc_call_within(
    command: &str,
    request: &serde_json::Value,
    limit: std::time::Duration,
) -> Option<serde_json::Value> {
    rpc_call_streaming(command, request, limit, &mut |_| {}).await
}

/// One fork-per-call round trip: write the request, close stdin, then read the
/// host's lines; an id-less line is a partial, the first with an id the reply.
async fn rpc_call_streaming(
    command: &str,
    request: &serde_json::Value,
    limit: std::time::Duration,
    partial: &mut impl FnMut(serde_json::Value),
) -> Option<serde_json::Value> {
    let mut req_str = serde_json::to_string(request).ok()?;
    req_str.push('\n');

    let mut tries = 0;
    let mut child = loop {
        // The deadline drops the exchange; without this the host would keep
        // running (and keep holding whatever it is stuck on).
        let spawned = Command::new(command)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn();
        match spawned {
            Ok(child) => break child,
            // Another thread's fork held a just-written script open until it
            // exec'd; a brief retry outlasts that window.
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 3 => {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
            Err(_) => return None, // command not found -> no host
        }
    };

    let stdout = child.stdout.take()?;
    let mut lines = tokio::io::BufReader::new(stdout).lines();

    // The whole exchange is bounded: a host that never reads stdin cannot block
    // the write past the deadline, and draining stdout lets it exit.
    let exchange = async {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(req_str.as_bytes()).await?;
            drop(stdin); // close stdin so the host sees EOF (one-shot)
        }
        let mut reply = None;
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if value.get("id").is_some() {
                reply.get_or_insert(value);
                continue;
            }
            partial(value);
        }
        let _ = child.wait().await;
        Ok::<_, std::io::Error>(reply)
    };

    match tokio::time::timeout(limit, exchange).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(err)) => {
            // The core has no logging crate; stderr reaches the unit's journal.
            eprintln!("wayrun-core: external host {command} failed: {err}");
            None
        }
        Err(_) => {
            eprintln!(
                "wayrun-core: external host {command} overran {}ms and was killed",
                limit.as_millis()
            );
            None
        }
    }
}

/// Ask the host who it serves. Empty when the host is missing, stalled or not
/// self-describing, so a query for it falls through instead of hanging.
pub async fn discover(command: &str, resident: bool) -> Vec<HostMeta> {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "list_plugins",
        "id": next_id(),
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
                api: entry.get("api").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
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
    Some(parse_rows(items, plugin, icon))
}

/// One payload's rows with their icons, previews and actions resolved; a row
/// that does not deserialize is dropped, never the payload.
fn parse_rows(items: Vec<serde_json::Value>, plugin: &str, icon: &str) -> Vec<ResultItem> {
    let mut parsed: Vec<ResultItem> = items
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect();
    let identity = host_icon_spec(icon);
    let owner = plugin.to_string();
    for item in &mut parsed {
        let spec = item.icon.as_deref().unwrap_or("");
        item.icon = resolve_item_icon(spec, identity.clone());
        // A preview is a file the host ships or plain text; the same icon rule
        // validates the path, and the text cannot grow the wire without bound.
        if let Some(Preview::Image { path }) = &item.preview {
            item.preview = crate::system::icon::remote_preview(path)
                .or_else(|| host_icon_spec(path))
                .map(|path| Preview::Image { path });
        }
        if let Some(Preview::Text { text }) = &mut item.preview {
            const MAX: usize = 64 * 1024;
            if text.len() > MAX {
                let mut end = MAX;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
        }
        for action in &mut item.actions {
            action.icon = action.icon.as_deref().and_then(host_icon_spec);
            // The core owns this field; whatever the host sent is ignored.
            action.plugin = Some(owner.clone());
        }
    }
    parsed
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
        "id": next_id(),
    });
    let Some(response) = host_call(command, resident, &request, HOST_TIMEOUT).await else {
        return Ok(Vec::new());
    };
    if let Some(row) = host_error(&response) {
        return Ok(vec![row]);
    }
    Ok(parse_result_items(response, plugin, icon).unwrap_or_default())
}

/// A host's search with its `partial` notifications forwarded as rows.
async fn query_external_streaming(
    command: &str,
    resident: bool,
    plugin: &str,
    text: &str,
    icon: &str,
    emit: Option<Emit>,
) -> Result<Vec<ResultItem>> {
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "search",
        "params": { "plugin": plugin, "text": text, "stream": true },
        "id": next_id(),
    });
    let last = std::sync::Arc::new(std::sync::Mutex::new(None::<Vec<ResultItem>>));
    let partial = {
        let last = std::sync::Arc::clone(&last);
        let plugin = plugin.to_string();
        let icon = icon.to_string();
        move |value: serde_json::Value| {
            let Some(rows) = value
                .get("params")
                .and_then(|params| params.get("rows"))
                .and_then(serde_json::Value::as_array)
                .cloned()
            else {
                return;
            };
            let rows = parse_rows(rows, &plugin, &icon);
            if let Some(emit) = &emit {
                // Partial rows are best-effort; the final payload is the answer.
                let _ = emit.try_send(rows.clone());
            }
            *last.lock().unwrap_or_else(|error| error.into_inner()) = Some(rows);
        }
    };
    let Some(response) =
        host_call_streaming(command, resident, &request, HOST_TIMEOUT, partial).await
    else {
        let last = last
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        return Ok(last.unwrap_or_default());
    };
    let last = last
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    if let Some(row) = host_error(&response) {
        return Ok(vec![row]);
    }
    Ok(parse_result_items(response, plugin, icon)
        .or(last)
        .unwrap_or_default())
}

/// A host's `search` error becomes one display-only row, so a plugin that raises
/// is visible instead of indistinguishable from an empty result.
fn host_error(response: &serde_json::Value) -> Option<ResultItem> {
    let error = response.get("error")?;
    let message = error
        .get("message")
        .and_then(|message| message.as_str())
        .unwrap_or_default();
    let summary = message.lines().next().unwrap_or_default();
    Some(ResultItem {
        title: t!("plugin.search_failed").to_string(),
        summary: Some(summary.to_string()),
        on_click: None,
        icon: None,
        icon_shape: None,
        preview: None,
        actions: Vec::new(),
    })
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
        "id": next_id(),
    });
    let Some(response) = host_call(command, resident, &request, HOST_TIMEOUT).await else {
        return Ok(None);
    };
    if response.get("error").is_some() {
        return Ok(None);
    }
    Ok(parse_result_items(response, plugin, icon))
}

/// Resolve a plugins.toml `command` the way execvp will: a name with a slash is
/// a path; a bare name walks `$PATH`, skipping empty segments and non-executables.
fn resolve_command(command: &str) -> Option<PathBuf> {
    crate::system::fs::which_in(std::env::var_os("PATH").unwrap_or_default(), command)
}

fn resolve_item_icon(icon: &str, fallback: Option<String>) -> Option<String> {
    crate::system::icon::remote_icon(icon)
        .or_else(|| host_icon_spec(icon))
        .or(fallback)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Action, IconShape};

    #[test]
    fn a_command_resolves_to_an_existing_file() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("host.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(command_path(script.to_str().unwrap()), Some(script.clone()));
        assert_eq!(
            command_path(temp.path().join("missing.sh").to_str().unwrap()),
            None
        );
    }

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
    fn a_theme_glyph_or_papirus_spec_is_no_icon_for_a_host() {
        // the core resolves these for its own rows, but not for an external host
        assert_eq!(resolve_item_icon("papirus:folder-open", None), None);
        assert_eq!(resolve_item_icon("firefox", None), None);
        assert_eq!(resolve_item_icon("builtin:power", None), None);
    }

    #[test]
    fn a_hosts_action_icons_take_paths_only() {
        let response = serde_json::json!({
            "result": [{
                "title": "r",
                "on_click": {"type":"run","cmd":"x"},
                "actions": [
                    {"title":"a", "action":{"type":"run","cmd":"y"}, "icon": "builtin:open"},
                    {"title":"b", "action":{"type":"run","cmd":"z"}, "icon": "/tmp/a.svg"},
                    {"title":"c", "action":{"type":"run","cmd":"w"}, "icon": "firefox"},
                ],
            }]
        });
        let items = parse_result_items(response, "system-search", "").unwrap();
        assert_eq!(
            items[0].actions[0].plugin.as_deref(),
            Some("system-search"),
            "the host owns the actions it attaches"
        );
        assert!(
            items[0].actions[0].icon.is_none(),
            "a glyph is no host icon"
        );
        assert_eq!(items[0].actions[1].icon.as_deref(), Some("/tmp/a.svg"));
        assert!(
            items[0].actions[2].icon.is_none(),
            "a theme name is no host icon"
        );
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
                {"title": "nulls", "summary": null, "on_click": null, "icon": null, "icon_shape": null},
                {"title": "circle", "icon_shape": "circle"},
            ]
        });
        let items = parse_result_items(response, "golden", "/opt/identity.svg").unwrap();
        assert_eq!(items.len(), 11, "every corpus row survives");
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
        assert_eq!(
            items[8].title, "ephemeral",
            "a stale field an old host sends is ignored"
        );
        assert!(items[9].summary.is_none() && items[9].on_click.is_none());
        assert_eq!(
            items[9].icon.as_deref(),
            Some("/opt/identity.svg"),
            "an unset icon falls back to the identity"
        );
        assert!(
            items[9].icon_shape.is_none(),
            "a null shape draws the file as it is"
        );
        assert_eq!(
            items[10].icon_shape,
            Some(IconShape::Circle),
            "the host's shape reaches the row"
        );
    }

    #[test]
    fn a_host_error_becomes_a_display_row() {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "error": {"code": -32000, "message": "boom\nstack traceback:"},
            "id": 1
        });
        let row = host_error(&response).unwrap();
        assert_eq!(row.title, t!("plugin.search_failed").to_string());
        assert_eq!(row.summary.as_deref(), Some("boom"));
        assert!(row.on_click.is_none());
    }

    #[test]
    fn a_hosts_preview_is_an_absolute_path_or_https_url() {
        let response = serde_json::json!({
            "result": [
                {"title": "ok", "preview": {"type": "image", "path": "/tmp/a.png"}},
                {"title": "url", "preview": {"type": "image", "path": "https://example.com/a.png"}},
                {"title": "theme", "preview": {"type": "image", "path": "theme:icon"}},
                {"title": "relative", "preview": {"type": "image", "path": "a.png"}}
            ]
        });
        let items = parse_result_items(response, "p", "").unwrap();
        assert_eq!(
            items[0].preview,
            Some(Preview::Image {
                path: "/tmp/a.png".to_string()
            })
        );
        assert!(matches!(
            &items[1].preview,
            Some(Preview::Image { path }) if path.contains("/icons/") && path.ends_with(".png")
        ));
        assert_eq!(items[2].preview, None);
        assert_eq!(items[3].preview, None);
    }

    #[test]
    fn a_hosts_row_icon_may_be_an_https_url() {
        let response = serde_json::json!({
            "result": [{"title": "url", "icon": "https://example.com/a.png"}]
        });
        let items = parse_result_items(response, "p", "/opt/identity.svg").unwrap();
        assert!(
            items[0]
                .icon
                .as_deref()
                .is_some_and(|path| path.contains("/icons/") && path.ends_with(".png"))
        );
    }

    #[test]
    fn a_hosts_preview_text_is_capped() {
        let long = "x".repeat(70 * 1024);
        let response = serde_json::json!({
            "result": [{"title": "t", "preview": {"type": "text", "text": long}}]
        });
        let items = parse_result_items(response, "p", "").unwrap();
        match &items[0].preview {
            Some(Preview::Text { text }) => assert_eq!(text.len(), 64 * 1024),
            other => panic!("expected a capped text preview, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_empty_query_never_reaches_the_host() {
        // No host is contacted: an empty text is not a search.
        let items = query_external("/nonexistent/host", false, "p", "", "")
            .await
            .unwrap();
        assert!(items.is_empty());
    }

    /// A host that never answers must cost the deadline, not the session.
    #[tokio::test]
    async fn a_stalled_host_is_given_up_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stall.sh");
        std::fs::write(&path, "#!/bin/sh\ncat >/dev/null\nexec sleep 600\n").unwrap();
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

    fn streaming_host(dir: &tempfile::TempDir, name: &str, body: &str) -> String {
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path.display().to_string()
    }

    #[tokio::test]
    async fn a_streaming_host_answers_with_its_partials() {
        let dir = tempfile::tempdir().unwrap();
        let command = streaming_host(
            &dir,
            "stream.sh",
            r#"#!/bin/sh
read line
printf '%s\n' '{"jsonrpc":"2.0","method":"partial","params":{"id":1,"rows":[{"title":"first"}]}}'
printf '%s\n' '{"jsonrpc":"2.0","result":[{"title":"final"}],"id":1}'
"#,
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let rows = query_external_streaming(&command, false, "demo", "x", "", Some(tx))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "final");
        let partial = rx.try_recv().expect("the partial was forwarded");
        assert_eq!(partial[0].title, "first");
    }

    #[tokio::test]
    async fn a_host_that_dies_after_its_partial_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let command = streaming_host(
            &dir,
            "half.sh",
            r#"#!/bin/sh
read line
printf '%s\n' '{"jsonrpc":"2.0","method":"partial","params":{"id":1,"rows":[{"title":"kept"}]}}'
"#,
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let rows = query_external_streaming(&command, false, "demo", "x", "", Some(tx))
            .await
            .unwrap();
        assert_eq!(rows[0].title, "kept");
        assert!(rx.try_recv().is_ok());
    }
}
