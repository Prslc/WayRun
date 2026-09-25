use std::io::Write as _;

use tokio::sync::mpsc;

use crate::system;
use crate::wire::{ResultItem, ThemeConfig};

/// Serialize `payload` onto the stdout stream owned by [`spawn_writer`].
pub async fn emit(tx: &mpsc::Sender<String>, payload: &impl serde::Serialize) {
    if let Ok(json) = serde_json::to_string(payload) {
        let _ = tx.send(json).await;
    }
}

/// A core-to-UI notification: no `id`, so no reply is expected. `params` is
/// serialized straight from the typed value, with no `Value` tree in between.
#[derive(serde::Serialize)]
pub struct Notification<T> {
    jsonrpc: &'static str,
    method: &'static str,
    params: T,
}

/// The `theme` notification, shared with the file watcher's live re-emit.
pub fn theme_notification() -> Notification<ThemeConfig> {
    Notification {
        jsonrpc: "2.0",
        method: "theme",
        params: system::theme::load_theme(),
    }
}

pub fn results_notification(items: &[ResultItem]) -> Notification<&[ResultItem]> {
    Notification {
        jsonrpc: "2.0",
        method: "results",
        params: items,
    }
}

/// Drain sentinel: the writer flushes and returns. The watchers hold sender
/// clones for the process lifetime, so the read loop must send it explicitly.
pub(super) const DRAIN: &str = "\0";

/// Own stdout for the process lifetime: one channel, so no two producers
/// interleave. A plain thread, because it must outlive the runtime to flush.
pub(super) fn spawn_writer(mut rx: mpsc::Receiver<String>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut stdout = std::io::stdout();
        while let Some(json) = rx.blocking_recv() {
            if json == DRAIN {
                break;
            }
            if stdout.write_all(json.as_bytes()).is_err() || stdout.write_all(b"\n").is_err() {
                break;
            }
            let _ = stdout.flush();
        }
    })
}
