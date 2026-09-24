use std::io::Write as _;

use anyhow::Result;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::wire::{ResultItem, ThemeConfig};
use crate::{plugin, rpc, system, watchers};

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

/// The `results` notification for one search payload.
pub fn results_notification(items: &[ResultItem]) -> Notification<&[ResultItem]> {
    Notification {
        jsonrpc: "2.0",
        method: "results",
        params: items,
    }
}

/// Drain sentinel: the writer flushes and returns. The watchers hold sender
/// clones for the process lifetime, so the read loop must send it explicitly.
const DRAIN: &str = "\0";

/// Own stdout for the process lifetime: one channel, so no two producers
/// interleave. A plain thread, because it must outlive the runtime to flush.
fn spawn_writer(mut rx: mpsc::Receiver<String>) -> std::thread::JoinHandle<()> {
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

/// Serve the JSON-RPC protocol until stdin closes. Any line that is not valid
/// JSON answers the standard `-32700` parse error.
pub async fn serve() -> Result<()> {
    let (tx, rx) = mpsc::channel::<String>(32);
    let writer = spawn_writer(rx);

    emit(&tx, &theme_notification()).await;

    // Hold the watchers for the core's lifetime: dropping them stops live theme
    // and plugin-registry updates.
    let _theme_watcher = watchers::watch_theme(&tx);

    let _config_watcher = watchers::watch_config();

    let _plugins_watcher = watchers::watch_plugins();

    let mut reader = BufReader::new(io::stdin()).lines();
    let search = Search::spawn(tx.clone());
    // `forget` waits on external hosts, so it runs in a task; the handles are
    // awaited before returning so a one-shot client still gets its reply.
    let mut forgets: Vec<JoinHandle<()>> = Vec::new();

    while let Some(line) = reader.next_line().await? {
        rpc::handle(&line, &tx, &search, &mut forgets).await;
    }

    // A pending search or forget still holds a sender clone; cancel or reap
    // them, then drain the writer so a one-shot client gets its last response.
    search.cancel();
    for handle in forgets {
        let _ = handle.await;
    }

    let _ = tx.send(DRAIN.to_string()).await;
    let _ = writer.join();

    Ok(())
}

/// The empty query: the full, uncapped history so deleting a row converges,
/// with the scope's pins leading and every row's action panel attached.
pub async fn history_items() -> Vec<ResultItem> {
    // The history is uncapped, so this is a read plus a JSON parse per row: real
    // work, and it runs on the blocking pool rather than a runtime worker.
    let items = tokio::task::spawn_blocking(|| system::usage::get_top(i32::MAX))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    plugin::decorate(items, "", true).await
}

/// The streaming search's one worker: a new request supersedes the pending one,
/// and a superseded payload is dropped instead of emitted.
pub struct Search {
    request: watch::Sender<Option<Request>>,
}

/// What the worker answers: a query, or the empty-query history.
#[derive(Clone)]
enum Request {
    Query(String),
    Top,
}

impl Search {
    /// Start the session's worker; it exits when the returned `Search` drops.
    pub fn spawn(tx: mpsc::Sender<String>) -> Self {
        let (request, mut rx) = watch::channel(None::<Request>);
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let Some(request) = rx.borrow_and_update().clone() else {
                    continue;
                };
                let results = match request {
                    // Each query gets its own task: a panicking provider must not
                    // silence the worker that answers every later search.
                    Request::Query(query) => {
                        match tokio::spawn(async move { plugin::dispatch(&query).await }).await {
                            Ok(results) => results,
                            Err(_) => continue,
                        }
                    }
                    Request::Top => history_items().await,
                };
                // A newer request, or a cancel, supersedes this payload.
                if !rx.has_changed().unwrap_or(true) {
                    emit(&tx, &results_notification(&results)).await;
                }
            }
        });
        Self { request }
    }

    /// Queue a query, superseding anything pending.
    pub fn request(&self, query: &str) {
        self.request
            .send_replace(Some(Request::Query(query.to_string())));
    }

    /// Queue the empty-query history, superseding anything pending.
    pub fn request_top(&self) {
        self.request.send_replace(Some(Request::Top));
    }

    /// Drop the pending request, so a payload still in flight is not emitted and
    /// the history it superseded stays the last one.
    pub fn cancel(&self) {
        self.request.send_replace(None);
    }
}
