use gio::prelude::{AppInfoExt, FileExt};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::OnceLock;

/// Shared by every scoped spawn and the probe, so an option an older
/// `systemd-run` rejects fails the probe; `${VAR}` in an argument stays literal.
const SCOPE_ARGS: [&str; 7] = [
    "--user",
    "--scope",
    "--collect",
    "--quiet",
    "--expand-environment=no",
    "--slice=app.slice",
    "--",
];

/// Whether a transient systemd scope can be made: `systemd-run` on `PATH` and a
/// reachable user manager. Probed once, since the probe itself spawns.
fn scope_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        program_on_path("systemd-run")
            && process::Command::new("systemd-run")
                .args(SCOPE_ARGS)
                .arg("true")
                .stdin(process::Stdio::null())
                .stdout(process::Stdio::null())
                .stderr(process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
    })
}

/// A command for `program` and `args`: wrapped in a transient systemd scope when
/// `scope`, so a launched app stays out of the launcher unit's cgroup, else direct.
fn scoped_command(scope: bool, program: &str, args: &[String]) -> process::Command {
    let mut command = if scope {
        let mut command = process::Command::new("systemd-run");
        command.args(SCOPE_ARGS).arg(program);
        command
    } else {
        process::Command::new(program)
    };
    command.args(args);
    command
}

/// [`scoped_command`] with the probe's verdict.
fn scoped(program: &str, args: &[String]) -> process::Command {
    scoped_command(scope_available(), program, args)
}

/// Spawn detached; a task waits the child, so the runtime reaps it — dropping
/// the handle alone would leave a finished app as a zombie.
fn spawn_detached(command: process::Command) {
    if let Ok(mut child) = tokio::process::Command::from(command).spawn() {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}

/// The scoped `gio <verb> <arg>` line, or `None` without transient scopes or the
/// gio CLI — the caller's fallback then forks into the launcher's own cgroup.
fn gio_line(scope: bool, gio: bool, verb: &str, arg: &str) -> Option<process::Command> {
    (scope && gio).then(|| scoped_command(true, "gio", &[verb.to_string(), arg.to_string()]))
}

/// [`gio_line`] with the probes' verdicts; spawns the lookup when it can.
fn spawn_gio_scoped(verb: &str, arg: &str) -> bool {
    match gio_line(scope_available(), program_on_path("gio"), verb, arg) {
        Some(command) => {
            spawn_detached(command);
            true
        }
        None => false,
    }
}

/// Run a shell command detached from the backend (system commands, …). Shell
/// is intended here: `%u`/`%f` leftovers are stripped before execution.
pub fn execute_command(cmd: &str) {
    let clean_cmd = cmd
        .replace("%u", "")
        .replace("%U", "")
        .replace("%f", "")
        .replace("%F", "");

    let args = vec![
        "-c".to_string(),
        format!("setsid {clean_cmd} >/dev/null 2>&1 &"),
    ];
    let mut command = scoped("sh", &args);
    // `sh` inherits the JSON-RPC pipe otherwise; its errors belong on the journal.
    command.stdout(process::Stdio::null());
    spawn_detached(command);
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
    if argv.is_empty() {
        return;
    }

    let mut command = scoped("setsid", argv);
    command
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null());
    spawn_detached(command);
}

/// Launch an app by desktop id via GLib's `GAppInfo`, honoring Exec quoting,
/// field codes, env and `DBusActivatable`; a no-op when the id is unknown.
pub fn launch_app(desktop_id: &str) {
    if let Some(path) = crate::system::desktop_action::find(desktop_id)
        && spawn_gio_scoped("launch", &path.to_string_lossy())
    {
        return;
    }
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
    if spawn_gio_scoped("open", uri) {
        return;
    }
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
    spawn_argv(&argv, Some(&dir));
}

/// Run a command in a terminal: a PATH binary needs a tty to be usable (btop,
/// htop, nvim), so it gets an interactive shell instead of a detached run.
pub fn run_in_terminal(cmd: &str) {
    let Some(argv) = terminal_run_argv(std::env::var("TERMINAL").ok().as_deref(), cmd) else {
        return;
    };
    spawn_argv(&argv, None);
}

/// Spawn an argv detached in its own session, stdio discarded.
fn spawn_argv(argv: &[String], dir: Option<&Path>) {
    if argv.is_empty() {
        return;
    }
    let mut command = scoped("setsid", argv);
    command
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    spawn_detached(command);
}

/// The terminal argv that runs `cmd`: the emulator, its command separator and
/// `sh -c cmd`, so a run keeps the shell semantics `run:` already had.
fn terminal_run_argv(terminal: Option<&str>, cmd: &str) -> Option<Vec<String>> {
    let mut argv = terminal_command_from(terminal)?;
    if let Some(flag) = terminal_exec_flag(&argv) {
        argv.extend(flag);
    }
    argv.push("sh".to_string());
    argv.push("-c".to_string());
    argv.push(cmd.to_string());
    Some(argv)
}

/// The emulator's name: the program's basename, so an absolute or aliased
/// `$TERMINAL` argument still names the emulator it is.
fn terminal_name(argv: &[String]) -> Option<&str> {
    let program = argv.first()?;
    Some(
        Path::new(program.as_str())
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(program.as_str()),
    )
}

/// Whether wezterm's argv still needs its subcommand: one that already names
/// `start`, `connect` or `ssh` must not get a second.
fn wezterm_subcommand_missing(argv: &[String]) -> bool {
    !argv
        .iter()
        .any(|arg| matches!(arg.as_str(), "start" | "connect" | "ssh"))
}

/// The tokens a known emulator needs before the program; a terminal that takes
/// the program directly (kitty, foot) needs none.
fn terminal_exec_flag(argv: &[String]) -> Option<Vec<String>> {
    match terminal_name(argv)? {
        "gnome-terminal" | "kgx" => Some(vec!["--".to_string()]),
        "konsole" | "xterm" | "alacritty" => Some(vec!["-e".to_string()]),
        "wezterm" => {
            let mut flag = Vec::new();
            if wezterm_subcommand_missing(argv) {
                flag.push("start".to_string());
            }
            flag.push("--".to_string());
            Some(flag)
        }
        _ => None,
    }
}

/// The directory a terminal starts in: the path itself when a directory, else its parent.
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
    let name = terminal_name(argv)?;
    let dir = dir.to_string_lossy().into_owned();
    match name {
        "gnome-terminal" | "kgx" => Some(vec!["--working-directory".to_string(), dir]),
        "konsole" => Some(vec!["--workdir".to_string(), dir]),
        "wezterm" => {
            let mut extra = Vec::new();
            if wezterm_subcommand_missing(argv) {
                extra.push("start".to_string());
            }
            extra.push("--cwd".to_string());
            extra.push(dir);
            Some(extra)
        }
        _ => None,
    }
}

/// Run one row or panel command; the single dispatch behind the `command` RPC.
pub fn execute(command: &crate::wire::Action) {
    use crate::wire::Action;
    match command {
        Action::Run { cmd } => execute_command(cmd),
        Action::RunInTerminal { cmd } => run_in_terminal(cmd),
        Action::Launch { desktop_id } => launch_app(desktop_id),
        Action::Copy { text } => copy_text(text),
        Action::DesktopAction {
            desktop_id,
            action_id,
        } => {
            crate::system::desktop_action::launch(desktop_id, action_id);
        }
        Action::Reveal { uri } => reveal(uri),
        Action::Terminal { uri } => open_terminal(uri),
        Action::Open { uri } => open_uri(uri),
    }
}

/// Write text to the Wayland clipboard via `wl-copy`, no shell. A missing
/// `wl-copy` is a no-op.
pub fn copy_text(text: &str) {
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
        let _ = stdin.write_all(text.as_bytes());
    }
    drop(child.stdin.take());
    let _ = child.wait();
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

    #[test]
    fn a_terminal_runs_the_command_through_a_shell() {
        assert_eq!(
            terminal_run_argv(Some("kitty"), "/usr/bin/btop"),
            Some(argv(&["kitty", "sh", "-c", "/usr/bin/btop"]))
        );
        assert_eq!(
            terminal_run_argv(Some("alacritty"), "htop"),
            Some(argv(&["alacritty", "-e", "sh", "-c", "htop"]))
        );
        assert_eq!(
            terminal_run_argv(Some("gnome-terminal"), "nvim"),
            Some(argv(&["gnome-terminal", "--", "sh", "-c", "nvim"]))
        );
        assert_eq!(
            terminal_run_argv(Some("wezterm"), "btop"),
            Some(argv(&["wezterm", "start", "--", "sh", "-c", "btop"]))
        );
    }

    #[test]
    fn a_wezterm_already_given_a_subcommand_does_not_repeat_it() {
        assert_eq!(
            terminal_run_argv(Some("wezterm start"), "btop"),
            Some(argv(&["wezterm", "start", "--", "sh", "-c", "btop"]))
        );
    }

    #[test]
    fn an_absolute_terminal_path_still_names_the_emulator() {
        let dir = Path::new("/tmp/project");
        assert_eq!(
            terminal_dir_arg(&argv(&["/usr/bin/wezterm"]), dir),
            Some(argv(&["start", "--cwd", "/tmp/project"]))
        );
        assert_eq!(
            terminal_run_argv(Some("/usr/bin/gnome-terminal"), "nvim"),
            Some(argv(&["/usr/bin/gnome-terminal", "--", "sh", "-c", "nvim"]))
        );
    }

    #[test]
    fn a_scoped_command_runs_systemd_run_before_the_program() {
        let command = scoped_command(true, "sh", &argv(&["-c", "echo hi"]));
        assert_eq!(command.get_program().to_string_lossy(), "systemd-run");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--user",
                "--scope",
                "--collect",
                "--quiet",
                "--expand-environment=no",
                "--slice=app.slice",
                "--",
                "sh",
                "-c",
                "echo hi"
            ]
        );
    }

    #[test]
    fn an_unscoped_command_is_the_program_itself() {
        let command = scoped_command(false, "sh", &argv(&["-c", "echo hi"]));
        assert_eq!(command.get_program().to_string_lossy(), "sh");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["-c", "echo hi"]);
    }

    #[test]
    fn a_scoped_gio_lookup_wraps_the_cli_call() {
        let command = gio_line(true, true, "launch", "/tmp/app.desktop").expect("wrapped");
        assert_eq!(command.get_program().to_string_lossy(), "systemd-run");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--user",
                "--scope",
                "--collect",
                "--quiet",
                "--expand-environment=no",
                "--slice=app.slice",
                "--",
                "gio",
                "launch",
                "/tmp/app.desktop"
            ]
        );
        // without a scope or the CLI the caller keeps its in-process call
        assert!(gio_line(false, true, "open", "x").is_none());
        assert!(gio_line(true, false, "open", "x").is_none());
    }

    #[test]
    fn a_hostile_argument_stays_one_argv_element() {
        let uri = "https://x/a b?q=${HOME}&r=%20";
        let command = gio_line(true, true, "open", uri).expect("wrapped");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.last().map(String::as_str), Some(uri));
        assert_eq!(
            args.len(),
            SCOPE_ARGS.len() + 3,
            "no splitting, no extra words"
        );
    }
}
