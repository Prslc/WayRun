use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{Sender as StdSender, channel};
use std::sync::{LazyLock, Mutex, PoisonError};

use calloop::channel::Sender;
use serde_json::{Value, json};
use wayrun_core::wire::{Action, ResultItem, ThemeConfig};

#[derive(Debug, Clone)]
pub enum BackendEvent {
    Theme(ThemeConfig),
    Results(Vec<ResultItem>),
    /// The core's stdout closed: nothing can be searched or launched anymore.
    CoreExited,
}

/// Lines bound for the core's stdin, drained by one writer thread.
static OUTBOX: LazyLock<Mutex<Option<StdSender<String>>>> = LazyLock::new(|| Mutex::new(None));

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

/// One query change: a streaming search notification.
pub fn search(text: &str) {
    notify("search", json!({ "text": text }));
}

/// The launcher was dismissed: the core drops any search still in flight.
pub fn dismiss() {
    notify("dismiss", Value::Null);
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

/// Remember (or, with `None`, clear) the default Enter action for a plugin scope.
pub fn default(scope: &str, action_id: Option<&str>) {
    notify("default", json!({ "scope": scope, "action_id": action_id }));
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

/// Parse one JSON-RPC 2.0 line into the event the shell renders; a line it does
/// not model is ignored.
fn parse(line: &str) -> Option<BackendEvent> {
    let notification = serde_json::from_str::<Notification>(line.trim()).ok()?;
    Some(match notification {
        Notification::Theme(config) => BackendEvent::Theme(config),
        Notification::Results(items) => BackendEvent::Results(items),
    })
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
}
