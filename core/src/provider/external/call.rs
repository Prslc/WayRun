use std::io;
use std::process;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::HOST_TIMEOUT;
use super::parse::{host_error, parse_result_items, parse_rows};
use crate::plugin::Emit;
use crate::wire::ResultItem;

/// The id of the next host request; partial notifications carry it back.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// One call against a host: `resident` keeps the process across calls; the
/// default spawns it fresh, so nothing a host kept outlives the call.
pub(super) async fn host_call(
    command: &str,
    resident: bool,
    request: &serde_json::Value,
    limit: Duration,
) -> Option<serde_json::Value> {
    if resident {
        super::super::resident::call(command, request, limit).await
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
    limit: Duration,
    partial: impl FnMut(serde_json::Value) + Send + 'static,
) -> Option<serde_json::Value> {
    if resident {
        super::super::resident::call_stream(command, request, limit, partial).await
    } else {
        let mut partial = partial;
        rpc_call_streaming(command, request, limit, &mut partial).await
    }
}

/// One fork-per-call round trip with no partial sink.
async fn rpc_call_within(
    command: &str,
    request: &serde_json::Value,
    limit: Duration,
) -> Option<serde_json::Value> {
    rpc_call_streaming(command, request, limit, &mut |_| {}).await
}

/// One fork-per-call round trip: write the request, close stdin, then read the
/// host's lines; an id-less line is a partial, the first with an id the reply.
async fn rpc_call_streaming(
    command: &str,
    request: &serde_json::Value,
    limit: Duration,
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
            .stdin(process::Stdio::piped())
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::inherit())
            .spawn();
        match spawned {
            Ok(child) => break child,
            // Another thread's fork held a just-written script open until it
            // exec'd; a brief retry outlasts that window.
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy && tries < 3 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(2)).await;
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
        Ok::<_, io::Error>(reply)
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

pub(super) async fn query_external(
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
pub(super) async fn query_external_streaming(
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
    let last = Arc::new(Mutex::new(None::<Vec<ResultItem>>));
    let partial = {
        let last = Arc::clone(&last);
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

/// Ask the host for its default view (its `top` method). `Ok(None)` when it has
/// no such method, errored, or returned nothing, so the caller shows the card.
pub(super) async fn query_default(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

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
        fs::write(&path, "#!/bin/sh\ncat >/dev/null\nexec sleep 600\n").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&path, perms).unwrap();

        let request = serde_json::json!({"jsonrpc": "2.0", "method": "search", "id": 1});
        let limit = Duration::from_millis(200);
        let started = Instant::now();
        let reply = rpc_call_within(&path.display().to_string(), &request, limit).await;
        assert!(reply.is_none(), "a stalled host has no answer");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "gave up on the deadline, not on the host: {:?}",
            started.elapsed()
        );
    }

    fn streaming_host(dir: &tempfile::TempDir, name: &str, body: &str) -> String {
        let path = dir.path().join(name);
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&path, perms).unwrap();
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
