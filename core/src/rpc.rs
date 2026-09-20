use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::protocol;
use crate::wire::Action;

const PARSE_ERROR: (i64, &str) = (-32700, "Parse error");
const INVALID_REQUEST: (i64, &str) = (-32600, "Invalid Request");
const METHOD_NOT_FOUND: (i64, &str) = (-32601, "Method not found");
const INVALID_PARAMS: (i64, &str) = (-32602, "Invalid params");

async fn respond(tx: &mpsc::Sender<String>, id: Value, result: Result<Value, (i64, &str)>) {
    let payload = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "result": result, "id": id }),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "error": { "code": code, "message": message },
            "id": id,
        }),
    };
    protocol::emit(tx, &payload).await;
}

fn search_text(params: Option<&Value>) -> Result<String, ()> {
    match params {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::Object(map)) => match map.get("text") {
            Some(Value::String(s)) => Ok(s.clone()),
            _ => Err(()), // text missing or not a string
        },
        _ => Err(()),
    }
}

fn select_payload(params: Option<&Value>) -> Result<String, ()> {
    match params {
        Some(obj @ Value::Object(_)) => Ok(obj.to_string()),
        _ => Err(()),
    }
}

fn string_param(params: Option<&Value>, key: &str) -> Result<String, ()> {
    match params {
        Some(Value::Object(map)) => map
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .ok_or(()),
        _ => Err(()),
    }
}

/// The `command` method's params are a `Action` object; a nested param is a
/// `Action` under `key`.
fn command_param(params: Option<&Value>, key: &str) -> Result<Action, ()> {
    match params {
        Some(Value::Object(map)) => map
            .get(key)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .ok_or(()),
        _ => Err(()),
    }
}

fn command_payload(params: Option<&Value>) -> Result<Action, ()> {
    let value = params.cloned().ok_or(())?;
    serde_json::from_value(value).map_err(|_| ())
}

/// Handle one JSON-RPC 2.0 line. A line that is not valid JSON answers the
/// standard `-32700`; an invalid request object answers `-32600`.
pub async fn handle(
    line: &str,
    tx: &mpsc::Sender<String>,
    search: &mut Option<JoinHandle<()>>,
    forgets: &mut Vec<JoinHandle<()>>,
) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        respond(tx, Value::Null, Err(PARSE_ERROR)).await;
        return;
    };

    let has_id = value.get("id").is_some();
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    if value.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
        respond(tx, id, Err(INVALID_REQUEST)).await;
        return;
    }
    let Some(method) = value.get("method").and_then(|v| v.as_str()) else {
        respond(tx, id, Err(INVALID_REQUEST)).await;
        return;
    };
    let params = value.get("params").cloned();

    match method {
        "search" => {
            match search_text(params.as_ref()) {
                Ok(text) if !text.is_empty() => {
                    if has_id {
                        let results = crate::plugin::dispatch(&text).await;
                        respond(tx, id, Ok(json!(results))).await;
                    } else {
                        protocol::start_search(tx, search, &text);
                    }
                }
                // an empty text is not a search; `top` serves the empty query
                _ => {
                    if has_id {
                        respond(tx, id, Err(INVALID_PARAMS)).await;
                    }
                }
            }
        }
        "top" => {
            protocol::abort_search(search);
            if has_id {
                respond(tx, id, Ok(json!(protocol::history_items().await))).await;
            } else {
                protocol::emit_history(tx).await;
            }
        }
        "select" => {
            let Ok(payload) = select_payload(params.as_ref()) else {
                if has_id {
                    respond(tx, id, Err(INVALID_PARAMS)).await;
                }
                return;
            };
            let _ = crate::system::usage::record(&payload);
            if has_id {
                respond(tx, id, Ok(Value::Null)).await;
            }
        }
        "command" => {
            let Ok(command) = command_payload(params.as_ref()) else {
                if has_id {
                    respond(tx, id, Err(INVALID_PARAMS)).await;
                }
                return;
            };
            crate::system::executor::execute(&command);
            if has_id {
                respond(tx, id, Ok(Value::Null)).await;
            }
        }
        "pin" => {
            let (Ok(scope), Some(Value::Object(map))) =
                (string_param(params.as_ref(), "scope"), params.as_ref())
            else {
                if has_id {
                    respond(tx, id, Err(INVALID_PARAMS)).await;
                }
                return;
            };
            let Some(item) = map.get("item") else {
                if has_id {
                    respond(tx, id, Err(INVALID_PARAMS)).await;
                }
                return;
            };
            let pinned = crate::system::pins::pin(&scope, &item.to_string()).is_ok();
            if has_id {
                respond(tx, id, Ok(json!({ "pinned": pinned }))).await;
            }
        }
        "unpin" => {
            let (Ok(scope), Ok(command)) = (
                string_param(params.as_ref(), "scope"),
                command_param(params.as_ref(), "on_click"),
            ) else {
                if has_id {
                    respond(tx, id, Err(INVALID_PARAMS)).await;
                }
                return;
            };
            let unpinned = crate::system::pins::unpin(&scope, &command.key()).unwrap_or(false);
            if has_id {
                respond(tx, id, Ok(json!({ "unpinned": unpinned }))).await;
            }
        }
        "forget" => {
            let Ok(command) = command_param(params.as_ref(), "on_click") else {
                if has_id {
                    respond(tx, id, Err(INVALID_PARAMS)).await;
                }
                return;
            };
            let removed = crate::system::usage::forget(&command.key()).unwrap_or(false);
            // The provider walk waits on external hosts, so it must not hold the
            // read loop: the reply carries the id and may land out of order.
            let tx = tx.clone();
            forgets.retain(|handle| !handle.is_finished());
            forgets.push(tokio::spawn(async move {
                let owned = crate::plugin::forget_row(&command).await;
                if has_id {
                    respond(&tx, id, Ok(json!({ "forgotten": removed || owned }))).await;
                }
            }));
        }
        "resolve_icon" => {
            // Resolve any icon spec (absolute path, theme name, or the
            // `papirus:<name>` scheme) to the absolute path the UI renders.
            let Some(Value::Object(map)) = params else {
                if has_id {
                    respond(tx, id, Err(INVALID_PARAMS)).await;
                }
                return;
            };
            let name = match map.get("name") {
                Some(Value::String(s)) if !s.is_empty() => s.clone(),
                _ => {
                    if has_id {
                        respond(tx, id, Err(INVALID_PARAMS)).await;
                    }
                    return;
                }
            };
            if has_id {
                respond(
                    tx,
                    id,
                    Ok(json!(crate::system::icon::find_icon_path(&name))),
                )
                .await;
            }
        }
        "list_plugins" => {
            let plugins: Vec<Value> = crate::plugin::list_plugins()
                .await
                .into_iter()
                .map(|(pid, name, icon, keyword, enabled)| {
                    json!({
                        "id": pid,
                        "name": name,
                        "icon": icon,
                        "keyword": keyword,
                        "enabled": enabled,
                    })
                })
                .collect();
            if has_id {
                respond(tx, id, Ok(json!(plugins))).await;
            }
        }
        "theme" => {
            let theme = crate::system::theme::load_theme();
            let value = serde_json::to_value(&theme).unwrap_or(Value::Null);
            if has_id {
                respond(tx, id, Ok(value)).await;
            }
        }
        "ping" => {
            if has_id {
                respond(tx, id, Ok(json!("pong"))).await;
            }
        }
        _ => {
            if has_id {
                respond(tx, id, Err(METHOD_NOT_FOUND)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Returns whether a streaming search was started, and the emitted messages.
    async fn run(line: &str) -> (bool, Vec<String>) {
        let (tx, mut rx) = mpsc::channel::<String>(32);
        let mut search = None;
        let mut forgets = Vec::new();
        handle(line, &tx, &mut search, &mut forgets).await;
        for handle in forgets {
            let _ = handle.await;
        }
        let mut msgs = Vec::new();
        while let Ok(m) = rx.try_recv() {
            msgs.push(m);
        }
        (search.is_some(), msgs)
    }

    #[tokio::test]
    async fn ping_returns_pong_with_id() {
        let (_, msgs) = run(r#"{"jsonrpc":"2.0","method":"ping","id":1}"#).await;
        assert_eq!(msgs.len(), 1);
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["result"], "pong");
        assert_eq!(v["id"], 1);
    }

    #[tokio::test]
    async fn a_notification_sends_no_response() {
        let (_, msgs) = run(r#"{"jsonrpc":"2.0","method":"ping"}"#).await;
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn a_search_notification_starts_the_streaming_search() {
        let (started, msgs) =
            run(r#"{"jsonrpc":"2.0","method":"search","params":{"text":"x"}}"#).await;
        assert!(started, "a search notification streams its results later");
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn a_non_json_line_returns_32700() {
        let (_, msgs) = run("firefox").await;
        assert_eq!(msgs.len(), 1);
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32700);
        assert_eq!(v["id"], Value::Null);
    }

    #[tokio::test]
    async fn a_missing_method_returns_32600() {
        let (_, msgs) = run(r#"{"jsonrpc":"2.0","params":null,"id":8}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn a_non_20_object_returns_32600() {
        let (_, msgs) = run(r#"{"method":"ping","id":1}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn unknown_method_returns_32601() {
        let (_, msgs) = run(r#"{"jsonrpc":"2.0","method":"nope","id":7}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn a_command_with_a_bad_payload_returns_32602() {
        let (_, msgs) =
            run(r#"{"jsonrpc":"2.0","method":"command","params":{"type":"nope"},"id":3}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn a_command_with_a_target_returns_null() {
        let (_, msgs) = run(
            r#"{"jsonrpc":"2.0","method":"command","params":{"type":"open","uri":"file:///no/such"},"id":4}"#,
        )
        .await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["result"], Value::Null);
    }

    #[tokio::test]
    async fn resolve_icon_returns_absolute_path() {
        let (_, msgs) = run(
            r#"{"jsonrpc":"2.0","method":"resolve_icon","params":{"name":"papirus:folder-open"},"id":4}"#,
        )
        .await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        let path = v["result"].as_str().expect("result must be a string");
        assert!(path.starts_with('/'));
        if std::path::Path::new("/usr/share/icons/Papirus").exists() {
            assert!(path.contains("/Papirus/"));
        }
    }

    #[tokio::test]
    async fn resolve_icon_passes_absolute_path_through() {
        let (_, msgs) = run(
            r#"{"jsonrpc":"2.0","method":"resolve_icon","params":{"name":"/usr/share/icons/Papirus/48x48/apps/github.svg"},"id":8}"#,
        )
        .await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(
            v["result"],
            "/usr/share/icons/Papirus/48x48/apps/github.svg"
        );
    }

    #[tokio::test]
    async fn resolve_icon_requires_name() {
        for line in [
            r#"{"jsonrpc":"2.0","method":"resolve_icon","id":5}"#,
            r#"{"jsonrpc":"2.0","method":"resolve_icon","params":{"name":""},"id":6}"#,
            r#"{"jsonrpc":"2.0","method":"resolve_icon","params":{"name":42},"id":7}"#,
        ] {
            let (_, msgs) = run(line).await;
            let v: Value = serde_json::from_str(&msgs[0]).unwrap();
            assert_eq!(v["error"]["code"], -32602);
        }
    }

    #[tokio::test]
    async fn invalid_params_returns_32602() {
        let (_, msgs) = run(r#"{"jsonrpc":"2.0","method":"search","params":42,"id":9}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn search_with_empty_text_returns_32602() {
        // an absent/empty query is not a search; `top` serves the empty query
        let (_, msgs) = run(r#"{"jsonrpc":"2.0","method":"search","id":10}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32602);
    }

    #[test]
    fn search_text_accepts_text_object() {
        let p: Value = serde_json::from_str(r#"{"text":"firefox"}"#).unwrap();
        assert_eq!(search_text(Some(&p)).unwrap(), "firefox");
    }

    #[test]
    fn search_text_absent_or_null_means_empty() {
        assert_eq!(search_text(None).unwrap(), "");
        assert_eq!(search_text(Some(&Value::Null)).unwrap(), "");
    }

    #[test]
    fn search_text_rejects_non_text_params() {
        // text must be an object field; bare strings and a `query` alias are rejected
        assert!(search_text(Some(&Value::String("firefox".into()))).is_err());
        let empty: Value = serde_json::from_str("{}").unwrap();
        let query: Value = serde_json::from_str(r#"{"query":"x"}"#).unwrap();
        let num: Value = serde_json::from_str(r#"{"text":42}"#).unwrap();
        assert!(search_text(Some(&empty)).is_err());
        assert!(search_text(Some(&query)).is_err());
        assert!(search_text(Some(&num)).is_err());
    }

    #[test]
    fn select_payload_accepts_item_object() {
        let p: Value =
            serde_json::from_str(r#"{"title":"x","on_click":{"type":"run","cmd":"ls"}}"#).unwrap();
        assert!(select_payload(Some(&p)).unwrap().contains("run"));
        assert!(select_payload(Some(&Value::String("run:ls".into()))).is_err());
    }

    #[test]
    fn command_param_reads_a_nested_command() {
        let p: Value = serde_json::from_str(r#"{"on_click":{"type":"run","cmd":"ls"}}"#).unwrap();
        assert_eq!(
            command_param(Some(&p), "on_click").unwrap(),
            Action::Run {
                cmd: "ls".to_string()
            }
        );
        let bad: Value = serde_json::from_str(r#"{"on_click":"run:ls"}"#).unwrap();
        assert!(command_param(Some(&bad), "on_click").is_err());
    }
}
