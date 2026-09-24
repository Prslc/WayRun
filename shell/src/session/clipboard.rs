use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use calloop::channel::Sender as EventSender;

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
    let output = Command::new("wl-paste")
        .arg("--no-newline")
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}
