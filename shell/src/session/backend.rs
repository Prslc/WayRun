use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender as StdSender, channel};
use std::sync::{LazyLock, Mutex, PoisonError};

use calloop::channel::Sender;
use serde_json::{Value, json};
use wayrun_core::wire::{Action, ResultItem, ThemeConfig};

#[derive(Debug, Clone)]
pub enum BackendEvent {
    Theme(ThemeConfig),
    Results(Vec<ResultItem>),
    /// A JSON-RPC `forget` reply: the command key the UI asked to drop, and
    /// whether the core really dropped anything.
    Forgotten {
        key: String,
        forgotten: bool,
    },
    /// The core's stdout closed: nothing can be searched or launched anymore.
    CoreExited,
}

/// Lines bound for the core's stdin, drained by one writer thread.
static OUTBOX: LazyLock<Mutex<Option<StdSender<String>>>> = LazyLock::new(|| Mutex::new(None));

/// The command key of each in-flight `forget`, so its reply can be routed back.
static REQUESTS: LazyLock<Mutex<HashMap<u64, String>>> = LazyLock::new(Default::default);
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

/// Send one protocol line to the core (newline added); a no-op before the core
/// exists. Callers use the typed methods below; nothing raw slips past the framing.
fn send(line: &str) {
    let guard = OUTBOX.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(tx) = guard.as_ref() {
        let _ = tx.send(line.to_string());
    }
}

/// Send a JSON-RPC notification.
fn notify(method: &str, params: Value) {
    send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string());
}

/// Send a JSON-RPC request, remembering the command key its reply answers.
fn request(method: &str, params: Value, key: String) -> u64 {
    let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    REQUESTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id, key);

    send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string());

    id
}

/// One query change: a streaming search notification.
pub fn search(text: &str) {
    notify("search", json!({ "text": text }));
}

/// The empty query: the history view.
pub fn top() {
    notify("top", Value::Null);
}

/// Record the row the user launched.
pub fn select(item: &Value) {
    notify("select", item.clone());
}

/// Run one row or panel command.
pub fn command(action: &Action) {
    notify(
        "command",
        serde_json::to_value(action).unwrap_or(Value::Null),
    );
}

/// Pin the row `on_click` names to one exact query. A notification, not a
/// request: the caller re-searches and the pin leads the reply.
pub fn pin(scope: &str, on_click: &Action) {
    notify("pin", json!({ "scope": scope, "on_click": on_click }));
}

/// Drop one pin of an exact query.
pub fn unpin(scope: &str, on_click: &Action) {
    notify("unpin", json!({ "scope": scope, "on_click": on_click }));
}

/// Remember (or, with `None`, clear) the default Enter action for a plugin scope.
pub fn default(scope: &str, action_id: Option<&str>) {
    notify("default", json!({ "scope": scope, "action_id": action_id }));
}

/// Ask the core to forget a row; the reply arrives as
/// [`BackendEvent::Forgotten`] and says whether anything was really dropped.
pub fn forget_row(action: &Action) {
    let key = action.key();
    request("forget", json!({ "on_click": action }), key);
}

/// Spawn the core and wire the reader/writer threads: one writer owns stdin (no
/// interleaved lines), the reader reaps the child when stdout closes.
pub fn start(tx: Sender<BackendEvent>) {
    // Re-exec this same binary in core mode; stderr is inherited so the core's
    // warnings land in the unit's journal.
    let spawned = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .arg("--core")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
    });

    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            eprintln!("wayrun: cannot start the core: {error}");
            let _ = tx.send(BackendEvent::CoreExited);
            return;
        }
    };

    let stdin = child.stdin.take();
    let stdout = child.stdout.take();

    if let Some(stdin) = stdin {
        let (sender, receiver) = channel::<String>();
        *OUTBOX.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
        std::thread::spawn(move || drain(stdin, receiver));
    }

    if let Some(stdout) = stdout {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(event) = parse(&line)
                    && tx.send(event).is_err()
                {
                    break;
                }
            }
            let _ = tx.send(BackendEvent::CoreExited);
            let _ = child.wait();
        });
    }

    // Warm the core's caches here, not on the first keystroke: the daemon boots
    // long before the launcher is shown; a real search supersedes this one.
    search("a");
}

fn drain(mut stdin: ChildStdin, receiver: std::sync::mpsc::Receiver<String>) {
    while let Ok(line) = receiver.recv() {
        if stdin.write_all(line.as_bytes()).is_err() || stdin.write_all(b"\n").is_err() {
            break;
        }
        let _ = stdin.flush();
    }
}

/// A core → shell notification. Deserialized straight into its payload, so the
/// hot `results` path never builds an intermediate `Value`.
#[derive(serde::Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "lowercase")]
enum Notification {
    Theme(ThemeConfig),
    Results(Vec<ResultItem>),
}

/// A JSON-RPC response; only `forget` answers are expected.
#[derive(serde::Deserialize)]
struct Reply {
    jsonrpc: String,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
}

/// Parse one JSON-RPC 2.0 line: a notification the shell renders, or the reply
/// to an in-flight `forget`.
fn parse(line: &str) -> Option<BackendEvent> {
    let line = line.trim();
    if line.starts_with('{')
        && let Ok(notification) = serde_json::from_str::<Notification>(line)
    {
        return Some(match notification {
            Notification::Theme(config) => BackendEvent::Theme(config),
            Notification::Results(items) => BackendEvent::Results(items),
        });
    }

    let reply: Reply = serde_json::from_str(line).ok()?;
    if reply.jsonrpc != "2.0" {
        return None;
    }

    // a reply: only `forget` answers are expected (matched back by request id)
    let id = reply.id?;
    let key = REQUESTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&id)?;

    // a reply with no `forgotten` (or an error) means nothing was dropped,
    // which is the safe answer for the UI
    let forgotten = reply
        .result
        .as_ref()
        .and_then(|result| result.get("forgotten"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(BackendEvent::Forgotten { key, forgotten })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_notifications_the_core_sends() {
        let theme =
            parse(r##"{"jsonrpc":"2.0","method":"theme","params":{"primary":"#fff"}}"##).unwrap();
        assert!(matches!(theme, BackendEvent::Theme(_)));

        let results =
            parse(r#"{"jsonrpc":"2.0","method":"results","params":[{"title":"a"}]}"#).unwrap();
        match results {
            BackendEvent::Results(items) => assert_eq!(items[0].title, "a"),
            other => panic!("expected results, got {other:?}"),
        }
    }

    #[test]
    fn present_nulls_and_the_ephemeral_flag_do_not_reject_the_payload() {
        // Help rows carry `on_click: null`, and usage opt-out rows carry
        // `ephemeral: true`; a present `null` must not fail the whole `Vec`.
        let line = r#"{"jsonrpc":"2.0","method":"results","params":[{"title":"Calculator","summary":"* (default)","on_click":null,"icon":null,"ephemeral":true}]}"#;
        match parse(line).unwrap() {
            BackendEvent::Results(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].title, "Calculator");
                assert_eq!(items[0].on_click, None);
                assert_eq!(items[0].icon, None);
                assert!(items[0].ephemeral);
            }
            other => panic!("expected results, got {other:?}"),
        }
    }

    #[test]
    fn ignores_lines_the_shell_does_not_render() {
        assert!(parse("").is_none());
        assert!(parse("not json").is_none());
        assert!(parse(r#"{"jsonrpc":"2.0","method":"unknown","params":[]}"#).is_none());
        assert!(parse(r#"{"jsonrpc":"2.0","id":999,"result":"/x.svg"}"#).is_none());
    }

    #[test]
    fn a_forget_reply_says_whether_anything_was_dropped() {
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        REQUESTS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, "run:never-used".to_string());

        let event = parse(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{{"forgotten":false}}}}"#
        ))
        .unwrap();
        match event {
            BackendEvent::Forgotten { key, forgotten } => {
                assert_eq!(key, "run:never-used");
                assert!(!forgotten);
            }
            other => panic!("expected a forget reply, got {other:?}"),
        }

        // an error reply (or a reply without the field) must not claim a drop
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        REQUESTS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, "run:a".to_string());
        let event = parse(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32601,"message":"no"}}}}"#
        ))
        .unwrap();
        assert!(matches!(
            event,
            BackendEvent::Forgotten {
                forgotten: false,
                ..
            }
        ));
    }
}
