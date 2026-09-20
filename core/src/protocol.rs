use std::io::Write as _;

use anyhow::Result;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{plugin, rpc, system, watchers};

/// One line of the text protocol; JSON-RPC requests are handled before this.
enum Request<'a> {
    /// Empty query: the usage-ranked history.
    History,
    Select(&'a str),
    Forget(&'a str),
    Run(&'a str),
    Copy(&'a str),
    Open(&'a str),
    Reveal(&'a str),
    Launch(&'a str),
    /// `action:<desktop-id>:<action-id>`.
    Action(&'a str),
    /// Anything else is a query.
    Search(&'a str),
}

impl<'a> Request<'a> {
    fn parse(input: &'a str) -> Self {
        if input.trim().is_empty() {
            return Self::History;
        }
        // verbatim argument: `run run tests` runs "run tests"
        let Some((name, argument)) = input.split_once(' ') else {
            return Self::Search(input);
        };
        match name {
            "select" => Self::Select(argument),
            "forget" => Self::Forget(argument),
            "run" => Self::Run(argument),
            "copy" => Self::Copy(argument),
            "open" => Self::Open(argument),
            "reveal" => Self::Reveal(argument),
            "launch" => Self::Launch(argument),
            "action" => Self::Action(argument),
            _ => Self::Search(input),
        }
    }
}

/// Serialize `payload` onto the stdout stream owned by [`spawn_writer`].
pub async fn emit(tx: &mpsc::Sender<String>, payload: &serde_json::Value) {
    if let Ok(json) = serde_json::to_string(payload) {
        let _ = tx.send(json).await;
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

/// Serve the line protocol until stdin closes.
pub async fn serve() -> Result<()> {
    let (tx, rx) = mpsc::channel::<String>(32);
    let writer = spawn_writer(rx);

    emit(
        &tx,
        &serde_json::json!({
            "type": "theme",
            "data": system::theme::load_theme()
        }),
    )
    .await;

    // Hold the watchers for the core's lifetime: dropping them stops live theme
    // and plugin-registry updates.
    let _theme_watcher = watchers::watch_theme(&tx);

    let _config_watcher = watchers::watch_config();

    let _plugins_watcher = watchers::watch_plugins();

    // Drop copy:-keyed history rows; idempotent, and usage::record keeps new
    // ones out.
    let _ = system::usage::purge_ephemeral();

    let mut reader = BufReader::new(io::stdin()).lines();
    let mut search: Option<JoinHandle<()>> = None;
    // `forget` waits on external hosts, so it runs in a task; the handles are
    // awaited before returning so a one-shot client still gets its reply.
    let mut forgets: Vec<JoinHandle<()>> = Vec::new();

    while let Some(line) = reader.next_line().await? {
        let input = line.trim_start();

        // JSON-RPC 2.0 requests — independent of the text protocol
        if rpc::handle(input, &tx, &mut forgets).await {
            continue;
        }

        match Request::parse(input) {
            Request::History => emit_history(&tx).await,
            Request::Select(item) => {
                let _ = system::usage::record(item);
            }
            Request::Forget(key) => {
                let _ = system::usage::forget(key);
                forgets.retain(|handle| !handle.is_finished());
                let key = key.to_string();
                forgets.push(tokio::spawn(async move {
                    plugin::forget_row(&key).await;
                }));
            }
            Request::Run(cmd) => system::executor::execute_command(cmd),
            Request::Copy(payload) => system::executor::copy_json(payload),
            Request::Open(uri) => system::executor::open_uri(uri),
            Request::Reveal(uri) => system::executor::reveal(uri),
            Request::Launch(id) => system::executor::launch_app(id),
            Request::Action(spec) => {
                if let Some((desktop_id, action_id)) = spec.split_once(':') {
                    system::desktop_action::launch(desktop_id, action_id);
                }
            }
            Request::Search(query) => start_search(&tx, &mut search, query),
        }
    }

    // A pending search or forget still holds a sender clone; reap them, then
    // drain the writer so a one-shot client gets its last response.
    if let Some(handle) = search.take() {
        handle.abort();
        let _ = handle.await;
    }
    for handle in forgets {
        let _ = handle.await;
    }

    let _ = tx.send(DRAIN.to_string()).await;
    let _ = writer.join();

    Ok(())
}

/// The empty query: the full, uncapped history so deleting a row converges,
/// with the scope's pins leading and every row's action panel attached.
async fn emit_history(tx: &mpsc::Sender<String>) {
    let items: Vec<crate::wire::ResultItem> = system::usage::get_top(i32::MAX)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect();
    let items = plugin::decorate(items, "", true).await;
    emit(tx, &serde_json::json!({ "type": "results", "data": items })).await;
}

/// Searches supersede each other: the pending one is aborted, the new one emits
/// its payload when it lands.
fn start_search(tx: &mpsc::Sender<String>, pending: &mut Option<JoinHandle<()>>, query: &str) {
    if let Some(handle) = pending.take() {
        handle.abort();
    }

    let tx = tx.clone();
    let query = query.to_string();
    *pending = Some(tokio::spawn(async move {
        let results = plugin::dispatch(&query).await;
        emit(
            &tx,
            &serde_json::json!({ "type": "results", "data": results }),
        )
        .await;
    }));
}

#[cfg(test)]
mod tests {
    use super::Request;

    #[test]
    fn parse_splits_command_and_verbatim_argument() {
        assert!(matches!(Request::parse("  "), Request::History));
        assert!(matches!(Request::parse("select {}"), Request::Select("{}")));
        assert!(matches!(
            Request::parse("open file:///tmp/x"),
            Request::Open("file:///tmp/x")
        ));
        // the argument keeps its own leading words: this runs `run tests`
        assert!(matches!(
            Request::parse("run run tests"),
            Request::Run("run tests")
        ));
        // no separator, or an unknown first word, is a query — line preserved
        assert!(matches!(Request::parse("run"), Request::Search("run")));
        assert!(matches!(
            Request::parse("firefox"),
            Request::Search("firefox")
        ));
        assert!(matches!(
            Request::parse("selects x"),
            Request::Search("selects x")
        ));
        // a desktop action is a verb, not a query
        assert!(matches!(
            Request::parse("action org.x:new-window"),
            Request::Action("org.x:new-window")
        ));
    }

    #[test]
    fn a_desktop_action_splits_into_its_two_ids() {
        let Request::Action(spec) = Request::parse("action org.gnome.Nautilus:new-window") else {
            panic!("expected an action");
        };
        assert_eq!(
            spec.split_once(':'),
            Some(("org.gnome.Nautilus", "new-window"))
        );
    }
}
