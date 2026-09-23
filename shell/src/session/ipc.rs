use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// `$XDG_RUNTIME_DIR/wayrun.sock`, which is also the single-instance guard: the
/// daemon binds it before touching Wayland.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("wayrun.sock")
}

/// Bind the listener (also the single-instance guard) and spawn the accept loop.
pub fn serve(tx: Sender<Command>) -> std::io::Result<()> {
    let path = socket_path();
    // Single-instance guard: a live listener here means another daemon owns the
    // launcher, and stealing the socket would leave it holding a stale surface.
    if UnixStream::connect(&path).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("another wayrun is listening on {}", path.display()),
        ));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    // `UnixListener::bind` honours the umask, which can leave the launcher
    // toggleable by other local users.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || handle(stream, tx));
        }
    });

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
pub fn client(verb: &str) -> std::process::ExitCode {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        eprintln!("wayrun: no daemon listening on {}", socket_path().display());
        return std::process::ExitCode::FAILURE;
    };

    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    if stream.write_all(verb.as_bytes()).is_err() || stream.write_all(b"\n").is_err() {
        return std::process::ExitCode::FAILURE;
    }

    let mut response = String::new();
    if BufReader::new(&stream).read_line(&mut response).is_err() {
        return std::process::ExitCode::FAILURE;
    }
    print!("{response}");

    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{Command, parse_request};

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
