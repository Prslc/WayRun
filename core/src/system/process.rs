use std::io::Read;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Default deadline for a blocking helper process.
pub const DEADLINE: Duration = Duration::from_secs(2);

const POLL: Duration = Duration::from_millis(10);
const GRACE: Duration = Duration::from_millis(200);

/// Run `command` with piped output and a deadline; `None` on spawn failure or
/// timeout (the child is killed and reaped).
pub fn output_within(mut command: Command, limit: Duration) -> Option<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().ok()?;

    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    if let Some(mut pipe) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = pipe.read_to_end(&mut buffer);
            let _ = out_tx.send(buffer);
        });
    }
    if let Some(mut pipe) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = pipe.read_to_end(&mut buffer);
            let _ = err_tx.send(buffer);
        });
    }

    let status = wait_status(&mut child, limit)?;
    Some(Output {
        status,
        stdout: out_rx.recv_timeout(GRACE).unwrap_or_default(),
        stderr: err_rx.recv_timeout(GRACE).unwrap_or_default(),
    })
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
        std::thread::sleep(POLL);
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
    fn output_and_status_are_returned_in_time() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf ok; printf err >&2; exit 3"]);
        let output = output_within(command, DEADLINE).unwrap();
        assert_eq!(output.stdout, b"ok");
        assert_eq!(output.stderr, b"err");
        assert_eq!(output.status.code(), Some(3));
    }
}
