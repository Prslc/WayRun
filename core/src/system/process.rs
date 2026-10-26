use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Default deadline for a blocking helper process.
pub const DEADLINE: Duration = Duration::from_secs(2);

const POLL: Duration = Duration::from_millis(10);
const GRACE: Duration = Duration::from_millis(200);

/// Run `command` with piped output and a deadline; `None` on spawn failure or
/// timeout (the child is killed and reaped).
pub fn output_within(command: Command, limit: Duration) -> Option<Output> {
    output_within_capped(command, limit, usize::MAX, usize::MAX)
}

/// [`output_within`] retaining at most `stdout_cap`/`stderr_cap` bytes per
/// stream; the rest is drained, so a chatty child never blocks on a full pipe.
pub fn output_within_capped(
    mut command: Command,
    limit: Duration,
    stdout_cap: usize,
    stderr_cap: usize,
) -> Option<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: the closure runs after fork and before exec, where `signal` is
    // async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut child = command.spawn().ok()?;

    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    if let Some(mut pipe) = child.stdout.take() {
        thread::spawn(move || {
            let _ = out_tx.send(read_capped(&mut pipe, stdout_cap));
        });
    }
    if let Some(mut pipe) = child.stderr.take() {
        thread::spawn(move || {
            let _ = err_tx.send(read_capped(&mut pipe, stderr_cap));
        });
    }

    let status = wait_status(&mut child, limit)?;
    Some(Output {
        status,
        stdout: out_rx.recv_timeout(GRACE).unwrap_or_default(),
        stderr: err_rx.recv_timeout(GRACE).unwrap_or_default(),
    })
}

/// Read to EOF, keeping only the first `cap` bytes.
fn read_capped(pipe: &mut impl Read, cap: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => return kept,
            Ok(read) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..read.min(room)]);
            }
            Err(_) => return kept,
        }
    }
}

/// Wait for `child`, killing it at the deadline; true when it exited in time.
pub fn wait_within(child: &mut Child, limit: Duration) -> bool {
    wait_status(child, limit).is_some()
}

fn wait_status(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        if started.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stalled_helper_is_killed_at_the_deadline() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 5"]);
        let started = Instant::now();
        assert!(output_within(command, Duration::from_millis(100)).is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_helper_gets_the_default_sigpipe() {
        use std::os::unix::process::ExitStatusExt;
        let mut command = Command::new("sh");
        command.args(["-c", "kill -s PIPE $$"]);
        let output = output_within(command, DEADLINE).unwrap();
        assert_eq!(output.status.signal(), Some(libc::SIGPIPE));
    }

    #[test]
    fn a_capped_stream_is_drained_but_truncated() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'abcdefghij'"]);
        let output = output_within_capped(command, DEADLINE, 4, 8).unwrap();
        assert_eq!(output.stdout, b"abcd");
        assert!(output.status.success());
    }

    #[test]
    fn output_and_status_are_returned_in_time() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf ok; printf err >&2; exit 3"]);
        let output = output_within(command, DEADLINE).unwrap();
        assert_eq!(output.stdout, b"ok");
        assert_eq!(output.stderr, b"err");
        assert_eq!(output.status.code(), Some(3));
    }
}
