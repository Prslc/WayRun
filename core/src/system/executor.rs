use gio::prelude::{AppInfoExt, FileExt};
use std::path::{Path, PathBuf};
use std::process;

/// Run a shell command detached from the backend (system commands, …). Shell
/// is intended here: `%u`/`%f` leftovers are stripped before execution.
pub fn execute_command(cmd: &str) {
    let clean_cmd = cmd
        .replace("%u", "")
        .replace("%U", "")
        .replace("%f", "")
        .replace("%F", "");

    process::Command::new("sh")
        .arg("-c")
        .arg(format!("setsid {clean_cmd} >/dev/null 2>&1 &"))
        .spawn()
        .ok();
}

/// Join an argv into a `sh` command line, quoting tokens that need it: the `run:`
/// path is parsed by a shell, so safe argv must be re-quoted.
pub fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|token| shell_quote(token))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quote one token for `sh` only when it contains something the shell would
/// treat specially; a bare word stays readable.
fn shell_quote(token: &str) -> String {
    let safe = !token.is_empty()
        && token.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b'%' | b'+' | b'=' | b','
                )
        });
    if safe {
        return token.to_string();
    }
    format!("'{}'", token.replace('\'', r"'\''"))
}

/// Run an argv detached, no shell: a `.desktop` `Exec=` already is argv, and
/// `sh -c` would make a `;` or `$` inside an argument syntax again.
pub fn execute_argv(argv: &[String]) {
    let Some((program, args)) = argv.split_first() else {
        return;
    };

    process::Command::new("setsid")
        .arg(program)
        .args(args)
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .spawn()
        .ok();
}

/// Launch an app by desktop id via GLib's `GAppInfo`, honoring Exec quoting,
/// field codes, env and `DBusActivatable`; a no-op when the id is unknown.
pub fn launch_app(desktop_id: &str) {
    for app in gio::AppInfo::all() {
        if app.id().as_deref() == Some(desktop_id) {
            let _ = app.launch(&[], None::<&gio::AppLaunchContext>);
            return;
        }
    }
}

/// Open a URI with the default handler via `GLib`. `xdg-open` would drop
/// `Terminal=true` outside a Flatpak/Snap sandbox.
pub fn open_uri(uri: &str) {
    let _ = gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>);
}

/// Show a file in the file manager: `org.freedesktop.FileManager1.ShowItems`
/// selects it, else fall back to opening the containing directory.
pub fn reveal(uri: &str) {
    if reveal_via_file_manager(uri) {
        return;
    }
    if let Some(parent) = gio::File::for_uri(uri)
        .path()
        .and_then(|path| path.parent().map(ToOwned::to_owned))
    {
        open_uri(&gio::File::for_path(parent).uri());
    }
}

fn reveal_via_file_manager(uri: &str) -> bool {
    use gio::glib::variant::ToVariant;

    let Ok(connection) = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>) else {
        return false;
    };
    let params = ("", vec![uri.to_string()]).to_variant();
    connection
        .call_sync(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
            "ShowItems",
            Some(&params),
            None::<&gio::glib::VariantTy>,
            gio::DBusCallFlags::NONE,
            2000,
            None::<&gio::Cancellable>,
        )
        .is_ok()
}

/// Open a terminal in a row's directory: the directory itself, or a file's
/// parent. The emulator is `$TERMINAL` when set, else the first on `PATH`.
pub fn open_terminal(uri: &str) {
    let Some(path) = gio::File::for_uri(uri).path() else {
        return;
    };
    let is_dir = path.is_dir();
    let Some(dir) = terminal_dir(&path, is_dir) else {
        return;
    };
    let Some(mut argv) = terminal_command_from(std::env::var("TERMINAL").ok().as_deref()) else {
        return;
    };
    if let Some(extra) = terminal_dir_arg(&argv, &dir) {
        argv.extend(extra);
    }

    let Some((program, args)) = argv.split_first() else {
        return;
    };
    process::Command::new("setsid")
        .arg(program)
        .args(args)
        .current_dir(&dir)
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .spawn()
        .ok();
}

/// The directory a terminal starts in: the path itself when a directory, else
/// its parent.
fn terminal_dir(path: &Path, is_dir: bool) -> Option<PathBuf> {
    if is_dir {
        return Some(path.to_path_buf());
    }
    path.parent().map(Path::to_path_buf)
}

/// The terminal argv: `$TERMINAL` shell-parsed so it may carry arguments, else
/// the first common emulator found on `PATH`.
fn terminal_command_from(terminal: Option<&str>) -> Option<Vec<String>> {
    if let Some(terminal) = terminal.map(str::trim).filter(|name| !name.is_empty())
        && let Ok(argv) = gio::glib::shell_parse_argv(terminal)
    {
        let argv: Vec<String> = argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        if !argv.is_empty() {
            return Some(argv);
        }
    }

    const CANDIDATES: [&str; 8] = [
        "kitty",
        "foot",
        "wezterm",
        "alacritty",
        "ghostty",
        "gnome-terminal",
        "konsole",
        "xterm",
    ];
    CANDIDATES
        .into_iter()
        .find(|name| program_on_path(name))
        .map(|name| vec![name.to_string()])
}

/// Whether a bare program name resolves on `PATH`.
fn program_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

/// The directory flag a known emulator needs; the rest inherit the process cwd,
/// and wezterm needs its `start` subcommand before `--cwd`.
fn terminal_dir_arg(argv: &[String], dir: &Path) -> Option<Vec<String>> {
    let program = argv.first()?;
    let name = Path::new(program.as_str())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program.as_str());
    let dir = dir.to_string_lossy().into_owned();
    match name {
        "gnome-terminal" | "kgx" => Some(vec!["--working-directory".to_string(), dir]),
        "konsole" => Some(vec!["--workdir".to_string(), dir]),
        "wezterm" => {
            let mut extra = Vec::new();
            if !argv
                .iter()
                .any(|arg| matches!(arg.as_str(), "start" | "connect" | "ssh"))
            {
                extra.push("start".to_string());
            }
            extra.push("--cwd".to_string());
            extra.push(dir);
            Some(extra)
        }
        _ => None,
    }
}

/// Write text to the Wayland clipboard via `wl-copy`, no shell. The `copy:`
/// scheme carries JSON, so a parse failure or missing `wl-copy` is a no-op.
pub fn copy_json(payload: &str) {
    let Ok(req) = serde_json::from_str::<CopyRequest>(payload) else {
        return;
    };

    let Some(mut child) = process::Command::new("wl-copy")
        .stdin(process::Stdio::piped())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .spawn()
        .ok()
    else {
        return;
    };
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        let _ = stdin.write_all(req.text.as_bytes());
    }
    drop(child.stdin.take());
    let _ = child.wait();
}

#[derive(serde::Deserialize)]
struct CopyRequest {
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn safe_tokens_stay_bare() {
        assert_eq!(
            shell_join(&argv(&[
                "niri",
                "msg",
                "action",
                "focus-window",
                "--id",
                "42"
            ])),
            "niri msg action focus-window --id 42"
        );
    }

    #[test]
    fn unsafe_tokens_are_single_quoted() {
        assert_eq!(shell_join(&argv(&["echo", "a b;c"])), "echo 'a b;c'");
        assert_eq!(shell_join(&argv(&["echo", "it's"])), r"echo 'it'\''s'");
    }

    #[test]
    fn an_empty_token_is_quoted_to_survive_the_shell() {
        assert_eq!(shell_join(&argv(&["foo", ""])), "foo ''");
    }

    #[test]
    fn a_terminal_starts_in_the_directory_or_a_files_parent() {
        assert_eq!(
            terminal_dir(Path::new("/tmp/project"), true),
            Some(PathBuf::from("/tmp/project"))
        );
        assert_eq!(
            terminal_dir(Path::new("/tmp/project/file.txt"), false),
            Some(PathBuf::from("/tmp/project"))
        );
        assert_eq!(terminal_dir(Path::new("/"), false), None);
    }

    #[test]
    fn the_terminal_command_comes_from_the_environment() {
        assert_eq!(
            terminal_command_from(Some("kitty --single-instance")),
            Some(argv(&["kitty", "--single-instance"]))
        );
    }

    #[test]
    fn a_missing_program_is_not_on_path() {
        assert!(!program_on_path("wayrun-no-such-binary-xyz"));
    }

    #[test]
    fn known_emulators_get_their_directory_flag() {
        let dir = Path::new("/tmp/project");
        assert_eq!(
            terminal_dir_arg(&argv(&["kitty"]), dir),
            None,
            "kitty inherits the process cwd"
        );
        assert_eq!(
            terminal_dir_arg(&argv(&["gnome-terminal"]), dir),
            Some(argv(&["--working-directory", "/tmp/project"]))
        );
        assert_eq!(
            terminal_dir_arg(&argv(&["wezterm"]), dir),
            Some(argv(&["start", "--cwd", "/tmp/project"]))
        );
        assert_eq!(
            terminal_dir_arg(&argv(&["wezterm", "start"]), dir),
            Some(argv(&["--cwd", "/tmp/project"]))
        );
    }
}
