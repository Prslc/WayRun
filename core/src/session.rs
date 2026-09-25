use anyhow::Result;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::protocol;
use crate::rpc::{self, Search};
use crate::watchers;

/// Serve the JSON-RPC protocol until stdin closes. The transport belongs to
/// `protocol`, the method table to `rpc`; this owns the session that drives them:
/// the writer, the watchers, the search worker and the read loop.
pub async fn serve() -> Result<()> {
    let (tx, rx) = mpsc::channel::<String>(32);
    let writer = protocol::spawn_writer(rx);

    protocol::emit(&tx, &protocol::theme_notification()).await;

    // Hold the watchers for the core's lifetime: dropping them stops live theme
    // and plugin-registry updates.
    let _theme_watcher = watchers::watch_theme(&tx);

    let _config_watcher = watchers::watch_config();

    let _plugins_watcher = watchers::watch_plugins();

    let mut reader = BufReader::new(io::stdin()).lines();
    let search = Search::spawn(tx.clone());
    // `forget` and `command` wait on external hosts, so they run in tasks; the
    // handles are awaited before returning so a one-shot client still replies.
    let mut pending: Vec<JoinHandle<()>> = Vec::new();

    while let Some(line) = reader.next_line().await? {
        rpc::handle(&line, &tx, &search, &mut pending).await;
    }

    // A pending search or task still holds a sender clone; cancel or reap them,
    // then drain the writer so a one-shot client gets its last response.
    search.cancel();
    for handle in pending {
        let _ = handle.await;
    }

    let _ = tx.send(protocol::DRAIN.to_string()).await;
    let _ = writer.join();

    Ok(())
}
