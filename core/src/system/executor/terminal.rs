use std::env;
use std::path::{Path, PathBuf};

use gio::prelude::FileExt;

use super::{program_on_path, spawn_argv};

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
    let Some(mut argv) = terminal_command_from(env::var("TERMINAL").ok().as_deref()) else {
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
    run_in_terminal_in(cmd, None);
}

pub fn run_in_terminal_in(cmd: &str, dir: Option<&Path>) {
    let Some(argv) = terminal_run_argv(env::var("TERMINAL").ok().as_deref(), cmd, dir) else {
        return;
    };
    spawn_argv(&argv, dir);
}

/// The terminal argv that runs `cmd`: the emulator, its command separator and
/// `sh -c cmd`, so a run keeps the shell semantics `run:` already had.
fn terminal_run_argv(terminal: Option<&str>, cmd: &str, dir: Option<&Path>) -> Option<Vec<String>> {
    let mut argv = terminal_command_from(terminal)?;
    if let Some(dir) = dir
        && let Some(flag) = terminal_dir_arg(&argv, dir)
    {
        argv.extend(flag);
    }
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

    // The XDG terminal-exec helper knows the user's preference and each
    // emulator's flags; the table below is only the last resort.
    if program_on_path("xdg-terminal-exec") {
        return Some(vec!["xdg-terminal-exec".to_string()]);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|t| t.to_string()).collect()
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
    fn the_xdg_helper_is_preferred_over_the_candidate_list() {
        if !program_on_path("xdg-terminal-exec") {
            return;
        }
        assert_eq!(
            terminal_command_from(None),
            Some(argv(&["xdg-terminal-exec"]))
        );
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
            terminal_run_argv(Some("kitty"), "/usr/bin/btop", None),
            Some(argv(&["kitty", "sh", "-c", "/usr/bin/btop"]))
        );
        assert_eq!(
            terminal_run_argv(Some("alacritty"), "htop", None),
            Some(argv(&["alacritty", "-e", "sh", "-c", "htop"]))
        );
        assert_eq!(
            terminal_run_argv(Some("gnome-terminal"), "nvim", None),
            Some(argv(&["gnome-terminal", "--", "sh", "-c", "nvim"]))
        );
        assert_eq!(
            terminal_run_argv(Some("wezterm"), "btop", None),
            Some(argv(&["wezterm", "start", "--", "sh", "-c", "btop"]))
        );
    }

    #[test]
    fn a_wezterm_already_given_a_subcommand_does_not_repeat_it() {
        assert_eq!(
            terminal_run_argv(Some("wezterm start"), "btop", None),
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
            terminal_run_argv(Some("/usr/bin/gnome-terminal"), "nvim", None),
            Some(argv(&["/usr/bin/gnome-terminal", "--", "sh", "-c", "nvim"]))
        );
    }
}
