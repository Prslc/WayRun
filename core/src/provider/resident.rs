use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// A host with no calls for this long is dropped and the next call pays a
/// fresh spawn; the file index holds its map for the same window.
const IDLE: Duration = Duration::from_secs(120);

/// One `command`'s host: the process kept across calls, plus the newest call's
/// ticket, so a queued call a later one overtook is skipped instead of stacking.
struct Slot {
    ticket: AtomicU64,
    host: tokio::sync::Mutex<Option<Host>>,
}

struct Host {
    /// Held only for its drop: `kill_on_drop` ends the process.
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    last_used: Instant,
}

static SLOTS: LazyLock<Mutex<HashMap<String, Arc<Slot>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// One JSON-RPC round trip against `command`'s resident host, spawned on first
/// use and restarted after a crash or a stall; `None` reads like a missing host.
pub(super) async fn call(
    command: &str,
    request: &serde_json::Value,
    limit: Duration,
) -> Option<serde_json::Value> {
    ensure_reaper();
    let slot = slot_for(command);
    let ticket = slot.ticket.fetch_add(1, Ordering::SeqCst) + 1;
    let line = serde_json::to_string(request).ok()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let owned = Arc::clone(&slot);
    let command = command.to_string();
    // The exchange runs detached, so a caller dropped mid-await (a superseded
    // search) cannot leave an unread reply ahead of the next call's.
    tokio::spawn(async move {
        let mut guard = owned.host.lock().await;
        if owned.ticket.load(Ordering::SeqCst) != ticket {
            let _ = tx.send(None);
            return;
        }
        if guard.is_none() {
            match spawn(&command).await {
                Ok(host) => *guard = Some(host),
                Err(_) => {
                    let _ = tx.send(None);
                    return;
                }
            }
        }
        let outcome = {
            let Some(host) = guard.as_mut() else {
                let _ = tx.send(None);
                return;
            };
            match tokio::time::timeout(limit, host.exchange(&line)).await {
                Ok(Ok(reply)) => {
                    host.last_used = Instant::now();
                    Ok(reply)
                }
                Ok(Err(error)) => Err(format!("failed: {error}")),
                Err(_) => Err(format!("overran {}ms and was killed", limit.as_millis())),
            }
        };
        match outcome {
            Ok(reply) => {
                let _ = tx.send(Some(reply));
            }
            Err(error) => {
                eprintln!("wayrun-core: resident host {command} {error}");
                *guard = None;
                let _ = tx.send(None);
            }
        }
    });
    rx.await.ok().flatten()
}

async fn spawn(command: &str) -> io::Result<Host> {
    let mut child = Command::new(command)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // A resident host lives in the unit's journal: its warnings are ours.
        .stderr(std::process::Stdio::inherit())
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("no stdin pipe"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("no stdout pipe"))?;
    Ok(Host {
        _child: child,
        stdin,
        stdout: BufReader::new(stdout),
        last_used: Instant::now(),
    })
}

impl Host {
    /// One request line in, one response line out; a short or unparseable line
    /// names a host that cannot be resynchronized, so the caller replaces it.
    async fn exchange(&mut self, line: &str) -> io::Result<serde_json::Value> {
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        let mut reply = String::new();
        if self.stdout.read_line(&mut reply).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the host closed its pipe",
            ));
        }
        serde_json::from_str(reply.trim())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

fn slot_for(command: &str) -> Arc<Slot> {
    let mut slots = SLOTS.lock().unwrap_or_else(|error| error.into_inner());
    Arc::clone(slots.entry(command.to_string()).or_insert_with(|| {
        Arc::new(Slot {
            ticket: AtomicU64::new(0),
            host: tokio::sync::Mutex::new(None),
        })
    }))
}

fn ensure_reaper() {
    static REAPER: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    REAPER.get_or_init(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(IDLE).await;
                reap(IDLE, None);
            }
        });
    });
}

/// Kill the hosts idle past `idle` and forget the slots whose host is gone;
/// `try_lock` skips a busy slot, and `only` narrows the sweep to one command.
fn reap(idle: Duration, only: Option<&str>) {
    let mut slots = SLOTS.lock().unwrap_or_else(|error| error.into_inner());
    slots.retain(|command, slot| {
        if only.is_some_and(|wanted| wanted != command) {
            return true;
        }
        let Ok(mut guard) = slot.host.try_lock() else {
            return true;
        };
        match guard.as_ref() {
            Some(host) if host.last_used.elapsed() < idle => true,
            Some(_) => {
                *guard = None;
                true
            }
            None => false,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: Duration = Duration::from_millis(500);

    fn script(dir: &tempfile::TempDir, name: &str, body: &str) -> String {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        drop(file);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path.display().to_string()
    }

    fn request() -> serde_json::Value {
        serde_json::json!({"jsonrpc": "2.0", "method": "ping", "id": 1})
    }

    /// A counting host: one `start` line per process, one reply per request.
    fn echo_host(dir: &tempfile::TempDir, counter: &std::path::Path) -> String {
        script(
            dir,
            "echo.sh",
            &format!(
                "#!/bin/sh\necho start >> {}\nwhile IFS= read -r line; do printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"result\":\"pong\",\"id\":1}}'; done\n",
                counter.display()
            ),
        )
    }

    fn starts(counter: &std::path::Path) -> usize {
        std::fs::read_to_string(counter).unwrap().lines().count()
    }

    #[tokio::test]
    async fn a_resident_host_serves_many_calls_from_one_process() {
        let dir = tempfile::tempdir().unwrap();
        let counter = dir.path().join("starts");
        let command = echo_host(&dir, &counter);
        for _ in 0..3 {
            let reply = call(&command, &request(), LIMIT).await.unwrap();
            assert_eq!(reply["result"], "pong");
        }
        assert_eq!(starts(&counter), 1, "one process serves every call");
    }

    #[tokio::test]
    async fn a_stalled_host_is_killed_and_the_next_call_restarts_it() {
        let dir = tempfile::tempdir().unwrap();
        let counter = dir.path().join("starts");
        let command = script(
            &dir,
            "stall.sh",
            &format!(
                "#!/bin/sh\necho start >> {}\ncat >/dev/null\nsleep 600\n",
                counter.display()
            ),
        );
        let limit = Duration::from_millis(150);
        assert!(call(&command, &request(), limit).await.is_none());
        assert!(call(&command, &request(), limit).await.is_none());
        assert_eq!(starts(&counter), 2, "the stalled host was replaced");
    }

    #[tokio::test]
    async fn a_dead_host_is_restarted_on_the_next_call() {
        let dir = tempfile::tempdir().unwrap();
        let counter = dir.path().join("starts");
        let command = script(
            &dir,
            "once.sh",
            &format!(
                "#!/bin/sh\necho start >> {}\nIFS= read -r line\nprintf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"result\":\"pong\",\"id\":1}}'\n",
                counter.display()
            ),
        );
        assert!(call(&command, &request(), LIMIT).await.is_some());
        assert!(
            call(&command, &request(), LIMIT).await.is_none(),
            "the pipe is closed"
        );
        assert!(
            call(&command, &request(), LIMIT).await.is_some(),
            "restarted"
        );
        assert_eq!(starts(&counter), 2);
    }

    #[tokio::test]
    async fn a_queued_call_a_later_one_overtook_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let served = dir.path().join("served");
        let command = script(
            &dir,
            "slow.sh",
            &format!(
                "#!/bin/sh\nwhile IFS= read -r line; do echo x >> {}; sleep 0.3; printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"result\":\"pong\",\"id\":1}}'; done\n",
                served.display()
            ),
        );
        let limit = Duration::from_secs(2);
        let first = tokio::spawn({
            let command = command.clone();
            async move { call(&command, &request(), limit).await }
        });
        for _ in 0..200 {
            if std::fs::read_to_string(&served).is_ok_and(|text| !text.is_empty()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let second = tokio::spawn({
            let command = command.clone();
            async move { call(&command, &request(), limit).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let third = tokio::spawn({
            let command = command.clone();
            async move { call(&command, &request(), limit).await }
        });
        assert!(first.await.unwrap().is_some());
        assert!(second.await.unwrap().is_none(), "overtaken while it waited");
        assert!(third.await.unwrap().is_some());
        assert_eq!(std::fs::read_to_string(&served).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn an_idle_host_is_reaped_and_its_slot_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let counter = dir.path().join("starts");
        let command = echo_host(&dir, &counter);
        assert!(call(&command, &request(), LIMIT).await.is_some());
        reap(Duration::ZERO, Some(&command));
        assert!(
            call(&command, &request(), LIMIT).await.is_some(),
            "restarted"
        );
        assert_eq!(starts(&counter), 2);
        // The exchange task releases the slot a beat after it replies, and a
        // sweep skips a slot a call holds; sweep until the removal lands.
        for _ in 0..200 {
            reap(Duration::ZERO, Some(&command));
            if !SLOTS.lock().unwrap().contains_key(&command) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("a dead slot is forgotten");
    }
}
