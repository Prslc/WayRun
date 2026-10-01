use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
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

/// The successful reply to a request whose result is a typed payload: it is
/// serialized straight from the value, with no `Value` tree in between.
#[derive(serde::Serialize)]
struct OkReply<'a, T> {
    jsonrpc: &'static str,
    result: &'a T,
    id: Value,
}

async fn respond_ok<T: serde::Serialize>(tx: &mpsc::Sender<String>, id: Value, result: &T) {
    protocol::emit(
        tx,
        &OkReply {
            jsonrpc: "2.0",
            result,
            id,
        },
    )
    .await;
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

/// The `record` method's params: a `{"on_click": Action}` object.
fn record_param(params: Option<&Value>) -> Result<Action, ()> {
    match params {
        Some(Value::Object(map)) => map
            .get("on_click")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .ok_or(()),
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

fn command_payload(params: Option<Value>) -> Result<Action, ()> {
    serde_json::from_value(params.ok_or(())?).map_err(|_| ())
}

/// Handle one JSON-RPC 2.0 line. A line that is not valid JSON answers the
/// standard `-32700`; an invalid request object answers `-32600`.
pub async fn handle(
    line: &str,
    tx: &mpsc::Sender<String>,
    search: &Search,
    pending: &mut Vec<JoinHandle<()>>,
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
    let params = value.get("params");

    match method {
        "search" => {
            match search_text(params) {
                Ok(text) if !text.is_empty() => {
                    if has_id {
                        let results = crate::plugin::dispatch(&text).await;
                        respond_ok(tx, id, &results).await;
                    } else {
                        search.request(&text);
                    }
                }
                // an empty text is not a search; a cleared field cancels the
                // pending one, so its payload cannot land in an empty list
                _ => {
                    if has_id {
                        respond(tx, id, Err(INVALID_PARAMS)).await;
                    } else {
                        search.cancel();
                    }
                }
            }
        }
        "dismiss" => {
            search.cancel();
            // A dismissal ends the session, so the warm hosts go with it.
            crate::provider::resident::reap_all();
        }
        "record" => {
            let Ok(command) = record_param(params) else {
                if has_id {
                    respond(tx, id, Err(INVALID_PARAMS)).await;
                }
                return;
            };
            let _ = crate::system::db::usage::record(&command);
            if has_id {
                respond(tx, id, Ok(Value::Null)).await;
            }
        }
        "command" => {
            let Ok(command) = command_payload(params.cloned()) else {
                if has_id {
                    respond(tx, id, Err(INVALID_PARAMS)).await;
                }
                return;
            };
            // Executing reaches the session bus and waits on children, so it runs
            // off the read loop; the reply then still lands for a one-shot client.
            let tx = tx.clone();
            pending.retain(|handle| !handle.is_finished());
            pending.push(tokio::spawn(async move {
                let execute =
                    tokio::task::spawn_blocking(move || crate::system::executor::execute(&command));
                let _ = execute.await;
                if has_id {
                    respond(&tx, id, Ok(Value::Null)).await;
                }
            }));
        }
        "default" => {
            // scope is the owning plugin id; a null action_id clears the default
            let Ok(scope) = string_param(params, "scope") else {
                if has_id {
                    respond(tx, id, Err(INVALID_PARAMS)).await;
                }
                return;
            };
            let action_id = match params.as_ref().and_then(|params| params.get("action_id")) {
                None | Some(Value::Null) => None,
                Some(Value::String(action_id)) => Some(action_id.as_str()),
                Some(_) => {
                    if has_id {
                        respond(tx, id, Err(INVALID_PARAMS)).await;
                    }
                    return;
                }
            };
            match action_id {
                Some(action_id) => {
                    let _ = crate::system::db::defaults::set(&scope, action_id);
                }
                None => {
                    let _ = crate::system::db::defaults::clear(&scope);
                }
            }
            if has_id {
                respond(tx, id, Ok(Value::Null)).await;
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
            if has_id {
                respond_ok(tx, id, &theme).await;
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

/// The streaming search's one worker: a new request supersedes the pending one,
/// and a superseded payload is dropped instead of emitted.
pub struct Search {
    request: watch::Sender<Option<String>>,
}

impl Search {
    /// Start the session's worker; it exits when the returned `Search` drops.
    pub fn spawn(tx: mpsc::Sender<String>) -> Self {
        let (request, mut rx) = watch::channel(None::<String>);
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let Some(query) = rx.borrow_and_update().clone() else {
                    continue;
                };
                // Each query gets its own task: a panicking provider must not
                // silence the worker that answers every later search.
                let results = match tokio::spawn(
                    async move { crate::plugin::dispatch(&query).await },
                )
                .await
                {
                    Ok(results) => results,
                    Err(_) => continue,
                };
                // A newer request, or a cancel, supersedes this payload.
                if !rx.has_changed().unwrap_or(true) {
                    protocol::emit(&tx, &protocol::results_notification(&results)).await;
                }
            }
        });
        Self { request }
    }

    /// Queue a query, superseding anything pending.
    pub fn request(&self, query: &str) {
        self.request.send_replace(Some(query.to_string()));
    }

    /// Drop the pending request, so a payload still in flight is not emitted.
    pub fn cancel(&self) {
        self.request.send_replace(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Returns the messages emitted while one line is handled.
    async fn run(line: &str) -> Vec<String> {
        let (tx, mut rx) = mpsc::channel::<String>(32);
        let search = Search::spawn(tx.clone());
        let mut pending = Vec::new();
        handle(line, &tx, &search, &mut pending).await;
        for handle in pending {
            let _ = handle.await;
        }
        let mut msgs = Vec::new();
        while let Ok(m) = rx.try_recv() {
            msgs.push(m);
        }
        msgs
    }

    #[tokio::test]
    async fn ping_returns_pong_with_id() {
        let msgs = run(r#"{"jsonrpc":"2.0","method":"ping","id":1}"#).await;
        assert_eq!(msgs.len(), 1);
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["result"], "pong");
        assert_eq!(v["id"], 1);
    }

    #[tokio::test]
    async fn a_notification_sends_no_response() {
        let msgs = run(r#"{"jsonrpc":"2.0","method":"ping"}"#).await;
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn a_search_notification_queues_the_query_without_a_reply() {
        let msgs = run(r#"{"jsonrpc":"2.0","method":"search","params":{"text":"x"}}"#).await;
        assert!(msgs.is_empty(), "the worker streams the payload later");
    }

    #[tokio::test]
    async fn an_empty_search_line_cancels_the_pending_search() {
        let (tx, mut rx) = mpsc::channel::<String>(32);
        let search = Search::spawn(tx.clone());
        let mut pending = Vec::new();
        handle(
            r#"{"jsonrpc":"2.0","method":"search","params":{"text":"x"}}"#,
            &tx,
            &search,
            &mut pending,
        )
        .await;
        handle(
            r#"{"jsonrpc":"2.0","method":"search","params":{"text":""}}"#,
            &tx,
            &search,
            &mut pending,
        )
        .await;

        // the cancel lands before the test yields, so the query's payload is
        // superseded: nothing may stream
        let outcome = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
        assert!(outcome.is_err(), "the cancelled payload must not stream");
    }

    #[tokio::test]
    async fn a_non_json_line_returns_32700() {
        let msgs = run("firefox").await;
        assert_eq!(msgs.len(), 1);
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32700);
        assert_eq!(v["id"], Value::Null);
    }

    #[tokio::test]
    async fn a_missing_method_returns_32600() {
        let msgs = run(r#"{"jsonrpc":"2.0","params":null,"id":8}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn a_non_20_object_returns_32600() {
        let msgs = run(r#"{"method":"ping","id":1}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn unknown_method_returns_32601() {
        let msgs = run(r#"{"jsonrpc":"2.0","method":"nope","id":7}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn a_command_with_a_bad_payload_returns_32602() {
        let msgs =
            run(r#"{"jsonrpc":"2.0","method":"command","params":{"type":"nope"},"id":3}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn a_command_with_a_target_returns_null() {
        let msgs = run(
            r#"{"jsonrpc":"2.0","method":"command","params":{"type":"open","uri":"file:///no/such"},"id":4}"#,
        )
        .await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["result"], Value::Null);
    }

    #[tokio::test]
    async fn default_requires_a_scope() {
        // no scope means no DB write, so this cannot touch real history
        let msgs = run(r#"{"jsonrpc":"2.0","method":"default","id":12}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn default_rejects_a_non_string_action_id() {
        let msgs = run(
            r#"{"jsonrpc":"2.0","method":"default","params":{"scope":"x","action_id":42},"id":13}"#,
        )
        .await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn invalid_params_returns_32602() {
        let msgs = run(r#"{"jsonrpc":"2.0","method":"search","params":42,"id":9}"#).await;
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn search_with_empty_text_returns_32602() {
        // an absent/empty query is not a search
        let msgs = run(r#"{"jsonrpc":"2.0","method":"search","id":10}"#).await;
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
    fn record_param_reads_a_nested_command() {
        let p: Value = serde_json::from_str(r#"{"on_click":{"type":"run","cmd":"ls"}}"#).unwrap();
        assert_eq!(
            record_param(Some(&p)).unwrap(),
            Action::Run {
                cmd: "ls".to_string()
            }
        );
        let bare: Value = serde_json::from_str(r#"{"type":"run","cmd":"ls"}"#).unwrap();
        assert!(record_param(Some(&bare)).is_err());
    }
}
