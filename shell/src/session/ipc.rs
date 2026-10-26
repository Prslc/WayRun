use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use calloop::channel::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Open,
    Close,
    Toggle,
    Status,
}

/// One line from a client. An unknown verb is answered, not ignored, so a typo
/// reaches the user instead of looking like a wedged daemon.
pub fn parse_request(line: &str) -> Result<Command, String> {
    match line.trim() {
        "open" => Ok(Command::Open),
        "close" => Ok(Command::Close),
        "toggle" => Ok(Command::Toggle),
        "status" => Ok(Command::Status),
        other => Err(format!("error: unknown verb {other:?}")),
    }
}

/// Written by the event loop, read by a `status` client: the socket lives in
/// the daemon, so the answer comes from the state the loop owns.
pub static VISIBLE: AtomicBool = AtomicBool::new(false);

/// Both sides give up after this: the daemon so a silent client cannot wedge the
/// accept thread, the client so a wedged daemon cannot hang the keybind.
const TIMEOUT: Duration = Duration::from_secs(2);
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

/// `$XDG_RUNTIME_DIR/wayrun.sock`, the single-instance guard, bound before
/// Wayland; a missing value is an error, not a shared `/tmp` path.
pub fn socket_path() -> Option<PathBuf> {
    let dir = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    dir.is_absolute().then(|| dir.join("wayrun.sock"))
}

/// Bind the listener (also the single-instance guard) and spawn the accept loop.
pub fn serve(tx: Sender<Command>) -> io::Result<()> {
    let Some(path) = socket_path() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "XDG_RUNTIME_DIR is not set; the launcher socket has no safe location",
        ));
    };
    // Single-instance guard: a live listener means another daemon owns the
    // launcher; the lock makes the check race-free.
    lock_instance(&path)?;
    if UnixStream::connect(&path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("another wayrun is listening on {}", path.display()),
        ));
    }
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    // `UnixListener::bind` honours the umask, which can leave the launcher
    // toggleable by other local users.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                // A persistent accept error must not spin the CPU.
                thread::sleep(ACCEPT_BACKOFF);
                continue;
            };
            let tx = tx.clone();
            thread::spawn(move || handle(stream, tx));
        }
    });

    Ok(())
}

/// The instance lock, held for the process's life: the connect probe alone
/// races, since two starts can both find no listener and both unlink the socket.
static INSTANCE_LOCK: Mutex<Option<File>> = Mutex::new(None);

/// Take the lock beside the socket, or refuse when another daemon holds it.
fn lock_instance(path: &Path) -> io::Result<()> {
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.with_extension("lock"))?;
    let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    // SAFETY: `flock` takes an owned fd and a flag word; the file outlives it.
    let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if !taken {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("another wayrun holds {}", path.display()),
        ));
    }
    *INSTANCE_LOCK.lock().unwrap_or_else(PoisonError::into_inner) = Some(file);
    Ok(())
}

fn handle(stream: UnixStream, tx: Sender<Command>) {
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };

    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }

    let response = match parse_request(&line) {
        Ok(Command::Status) => {
            if VISIBLE.load(Ordering::Relaxed) {
                "visible".to_string()
            } else {
                "hidden".to_string()
            }
        }
        Ok(command) => {
            let _ = tx.send(command);
            "ok".to_string()
        }
        Err(reason) => reason,
    };

    let _ = writer.write_all(response.as_bytes());
    let _ = writer.write_all(b"\n");
    let _ = writer.flush();
}

/// The client end: `wayrun open|close|toggle|status`.
pub fn client(verb: &str) -> process::ExitCode {
    let Some(path) = socket_path() else {
        eprintln!("wayrun: XDG_RUNTIME_DIR is not set; no daemon can be reached");
        return process::ExitCode::FAILURE;
    };
    let Ok(mut stream) = UnixStream::connect(&path) else {
        eprintln!("wayrun: no daemon listening on {}", path.display());
        return process::ExitCode::FAILURE;
    };

    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    if stream.write_all(verb.as_bytes()).is_err() || stream.write_all(b"\n").is_err() {
        return process::ExitCode::FAILURE;
    }

    let mut response = String::new();
    if BufReader::new(&stream).read_line(&mut response).is_err() {
        return process::ExitCode::FAILURE;
    }
    print!("{response}");

    if response.trim_start().starts_with("error") {
        process::ExitCode::FAILURE
    } else {
        process::ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::process;

    use super::{Command, lock_instance, parse_request};

    #[test]
    fn a_second_instance_lock_is_refused() {
        let dir = env::temp_dir().join(format!("wayrun-lock-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wayrun.sock");

        lock_instance(&path).unwrap();
        let lock = fs::metadata(path.with_extension("lock")).unwrap();
        assert_eq!(lock.permissions().mode() & 0o777, 0o600);
        let refused = lock_instance(&path).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::AddrInUse);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_client_verb_is_answered() {
        assert_eq!(parse_request("open\n"), Ok(Command::Open));
        assert_eq!(parse_request("close\n"), Ok(Command::Close));
        assert_eq!(parse_request("toggle\n"), Ok(Command::Toggle));
        assert_eq!(parse_request("status\n"), Ok(Command::Status));
        assert_eq!(parse_request("  toggle \n"), Ok(Command::Toggle));
    }

    #[test]
    fn a_typo_is_answered_rather_than_ignored() {
        let error = parse_request("toggl\n").unwrap_err();
        assert!(error.contains("toggl"), "{error}");
    }
}
