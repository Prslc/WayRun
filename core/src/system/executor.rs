use gio::prelude::{AppInfoExt, AppLaunchContextExt, FileExt};
use std::env;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process;
use std::sync::OnceLock;

use crate::system::process::{DEADLINE, output_within, wait_within};

mod terminal;

pub use self::terminal::{open_terminal, run_in_terminal, run_in_terminal_in};

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
        let mut probe = process::Command::new("systemd-run");
        probe.args(SCOPE_ARGS).arg("true");
        program_on_path("systemd-run")
            && output_within(probe, DEADLINE).is_some_and(|output| output.status.success())
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
    // A launched app gets the session's language back: `locale` pins the
    // launcher's own into `LANGUAGE`, and an app must not inherit that choice.
    match crate::locale::ambient_language() {
        Some(language) => command.env("LANGUAGE", language),
        None => command.env_remove("LANGUAGE"),
    };
    command.env_remove("WAYRUN_LOCALE");
    restore_signals(&mut command);
    command
}

/// Rust ignores SIGPIPE for itself, and an ignored disposition survives exec;
/// launched programs get the default back.
fn restore_signals(command: &mut process::Command) {
    // SAFETY: the closure runs after fork and before exec, where `signal` is
    // async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            Ok(())
        });
    }
}

/// [`scoped_command`] with the probe's verdict.
fn scoped(program: &str, args: &[String]) -> process::Command {
    scoped_command(scope_available(), program, args)
}

/// Spawn detached; a task waits the child, so the runtime reaps it — dropping
/// the handle alone would leave a finished app as a zombie.
fn spawn_detached(mut command: process::Command) {
    // A detached child must not inherit the core's JSON-RPC pipes.
    command
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null());
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

/// Run a shell command detached from the backend (system commands, …). Shell is
/// intended here: the line is the caller's, field codes and all.
pub fn execute_command(cmd: &str) {
    let args = vec!["-c".to_string(), detached_line(cmd)];
    // stdin/stdout are detached by `spawn_detached`; stderr stays on the journal.
    spawn_detached(scoped("sh", &args));
}

/// The shell line that runs `cmd` and lets the launcher return: detached from
/// the session, with stdout discarded and stderr left on the journal.
fn detached_line(cmd: &str) -> String {
    format!("setsid {cmd} >/dev/null &")
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
pub fn execute_argv(argv: &[String], dir: Option<&Path>) {
    spawn_argv(argv, dir);
}

/// Launch an app by desktop id via GLib's `GAppInfo`, honoring Exec quoting,
/// field codes, env and `DBusActivatable`; a no-op when the id is unknown.
pub fn launch_app(desktop_id: &str) {
    if let Some(path) = crate::system::desktop_action::find(desktop_id)
        && spawn_gio_scoped("launch", &path.to_string_lossy())
    {
        return;
    }
    let context = launch_context();
    for app in gio::AppInfo::all() {
        if app.id().as_deref() == Some(desktop_id) {
            let _ = app.launch(&[], Some(&context));
            return;
        }
    }
}

/// A launch context that hands the app the session's LANGUAGE, not the UI pin.
pub(crate) fn launch_context() -> gio::AppLaunchContext {
    let context = gio::AppLaunchContext::new();
    match crate::locale::ambient_language() {
        Some(language) => context.setenv("LANGUAGE", language),
        None => context.unsetenv("LANGUAGE"),
    }
    context.unsetenv("WAYRUN_LOCALE");
    context
}

/// Open a URI with the default handler via `GLib`. `xdg-open` would drop
/// `Terminal=true` outside a Flatpak/Snap sandbox.
pub fn open_uri(uri: &str) {
    if spawn_gio_scoped("open", uri) {
        return;
    }
    let _ = gio::AppInfo::launch_default_for_uri(uri, Some(&launch_context()));
}

/// Open `uri` with one named handler; a handler that vanished since the panel
/// was built runs nothing.
pub fn open_with(desktop_id: &str, uri: &str) {
    let context = launch_context();
    for app in gio::AppInfo::all() {
        if app.id().as_deref() == Some(desktop_id) {
            let _ = app.launch_uris(&[uri], Some(&context));
            return;
        }
    }
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
    // `ShowItems(in as URIs, in s StartupId)`.
    let params = (vec![uri.to_string()], "").to_variant();
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

/// Whether a bare program name resolves to an executable on `PATH`.
fn program_on_path(name: &str) -> bool {
    let path = env::var_os("PATH").unwrap_or_default();
    crate::system::fs::which_in(path, name).is_some()
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
        Action::OpenWith { desktop_id, uri } => open_with(desktop_id, uri),
    }
}

/// Write text to the Wayland clipboard via `wl-copy`, no shell. A missing
/// `wl-copy` is a no-op.
pub fn copy_text(text: &str) {
    let mut command = process::Command::new("wl-copy");
    command
        .stdin(process::Stdio::piped())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null());
    restore_signals(&mut command);
    let Some(mut child) = command.spawn().ok() else {
        return;
    };
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        let _ = stdin.write_all(text.as_bytes());
    }
    drop(child.stdin.take());
    let _ = wait_within(&mut child, DEADLINE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn a_launched_program_gets_the_default_sigpipe() {
        use std::os::unix::process::ExitStatusExt;
        let mut command = process::Command::new("sh");
        restore_signals(&mut command);
        let status = command.args(["-c", "kill -s PIPE $$"]).status().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGPIPE));
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

    /// Only `launch` and `desktop_action` read `.desktop` field codes; a `run`
    /// line is the caller's whole shell line, so a `date +%F` must survive.
    #[test]
    fn a_run_line_reaches_the_shell_untouched() {
        for cmd in [
            "/usr/bin/date +%F",
            "/usr/bin/date +%u",
            "/usr/bin/sed s/%f/X/",
            "/usr/bin/curl %U",
        ] {
            assert!(detached_line(cmd).contains(cmd), "{cmd}");
        }
    }

    #[test]
    fn a_missing_program_is_not_on_path() {
        assert!(!program_on_path("wayrun-no-such-binary-xyz"));
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
