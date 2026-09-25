use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Duration;

use calloop::channel::Sender as EventSender;

/// How long a selection read may take before its child is killed.
const PASTE_TIMEOUT: Duration = Duration::from_secs(1);

/// Read the selection on one worker thread, so a held Ctrl+V queues generations
/// instead of growing a thread per press.
pub fn spawn(tx: EventSender<(u64, Option<String>)>) -> Sender<u64> {
    let (jobs, queue) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(mut generation) = queue.recv() {
            // A newer press supersedes what is queued behind it: the superseded
            // generations would be dropped on arrival anyway.
            while let Ok(newer) = queue.try_recv() {
                generation = newer;
            }
            if tx.send((generation, paste())).is_err() {
                break;
            }
        }
    });
    jobs
}

/// Ask the worker for the selection. `generation` is the show it belongs to, so
/// a read finishing after a dismissal is dropped instead of pasted later.
pub fn read(jobs: &Sender<u64>, generation: u64) {
    let _ = jobs.send(generation);
}

fn paste() -> Option<String> {
    let mut child = Command::new("wl-paste")
        .arg("--no-newline")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // Read the pipe from a thread: a child whose output outgrows the pipe buffer
    // would otherwise block against a wait on this one.
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = Vec::new();
        let read = std::io::Read::read_to_end(&mut stdout, &mut text);
        let _ = tx.send((read, text));
    });

    match rx.recv_timeout(PASTE_TIMEOUT) {
        Ok((Ok(_), text)) if child.wait().is_ok_and(|status| status.success()) => {
            Some(String::from_utf8_lossy(&text).into_owned())
        }
        // A selection owner that never serves the read must not wedge the worker
        // (and with it every later paste), so the child is killed instead.
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}
