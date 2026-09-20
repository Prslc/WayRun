use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::pin::Pin;
use std::sync::LazyLock;

use anyhow::Result;
use std::collections::HashSet;

use crate::plugin::{Meta, Plugin};
use crate::system::icon::resolve;
use crate::wire::{Action, ResultItem};

pub struct Runner;

impl Plugin for Runner {
    fn meta(&self) -> &Meta {
        &Meta {
            id: "runner",
            name: "Run Action",
            icon: "utilities-terminal",
            ready: "Run an executable on PATH",
        }
    }

    fn search(
        &self,
        query: &str,
        _full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        let input = query.to_string();
        Box::pin(async move { Ok(do_search(&input)) })
    }
}

/// Split `full` into a command token and trailing args: the first whitespace
/// token is the candidate executable, the rest runs with it.
fn split_command(input: &str) -> (String, String) {
    let trimmed = input.trim();
    match trimmed.find(char::is_whitespace) {
        Some(idx) => (
            trimmed[..idx].to_string(),
            trimmed[idx..].trim().to_string(),
        ),
        None => (trimmed.to_string(), String::new()),
    }
}

/// `(name, full path)` for every executable on `$PATH`, first match wins, scanned
/// once per process. Dot-containing names are skipped as library noise.
fn path_binaries() -> &'static Vec<(String, String)> {
    static LIST: LazyLock<Vec<(String, String)>> = LazyLock::new(scan_path);
    &LIST
}

fn scan_path() -> Vec<(String, String)> {
    let path = std::env::var("PATH").unwrap_or_default();
    let home = std::env::var("HOME").ok();

    let mut seen: HashSet<String> = HashSet::default();
    let mut out = Vec::new();

    for raw_dir in path.split(':') {
        if raw_dir.is_empty() {
            continue;
        }
        // expand a leading `~` — shells usually expand PATH at export, be lenient
        let dir_str = match raw_dir.strip_prefix("~/") {
            Some(rest) => home
                .as_ref()
                .map(|h| format!("{h}/{rest}"))
                .unwrap_or_default(),
            None => raw_dir.to_string(),
        };
        let Ok(entries) = std::fs::read_dir(Path::new(&dir_str)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `std::fs::metadata` follows symlinks: PATH entries are often
            // symlinks to the real binary, so the target's exec bit decides.
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') || name.contains('.') {
                continue;
            }
            if seen.insert(name.to_string()) {
                out.push((name.to_string(), path.to_string_lossy().into_owned()));
            }
        }
    }
    out
}

fn do_search(input: &str) -> Vec<ResultItem> {
    let (cmd, args) = split_command(input);
    if cmd.is_empty() {
        return Vec::new();
    }

    let mut matcher = nucleo::Matcher::new(nucleo::Config::DEFAULT);
    let pattern = nucleo::Utf32String::from(cmd.to_lowercase());

    let mut results: Vec<(u16, ResultItem)> = Vec::new();

    for (name, full_path) in path_binaries() {
        let name_utf32 = nucleo::Utf32String::from(name.to_lowercase());
        let score = matcher
            .fuzzy_match(name_utf32.slice(..), pattern.slice(..))
            .unwrap_or(0);
        if score > 0 {
            let run_cmd = if args.is_empty() {
                full_path.clone()
            } else {
                format!("{full_path} {args}")
            };
            results.push((
                score,
                ResultItem {
                    title: name.clone(),
                    summary: Some(run_cmd.clone()),
                    on_click: Some(Action::Run { cmd: run_cmd }),
                    icon: resolve(name),
                    ephemeral: false,
                    actions: Vec::new(),
                    badge: None,
                },
            ));
        }
    }

    crate::provider::rank_results(results, false, 20)
}

#[cfg(test)]
mod tests {
    use super::split_command;

    #[test]
    fn splits_command_and_args() {
        assert_eq!(split_command("htop"), ("htop".into(), String::new()));
        assert_eq!(split_command("htop -c"), ("htop".into(), "-c".into()));
        assert_eq!(
            split_command("nvim  main.rs"),
            ("nvim".into(), "main.rs".into())
        );
        assert_eq!(
            split_command("  git pull --rebase"),
            ("git".into(), "pull --rebase".into())
        );
        assert_eq!(split_command(""), (String::new(), String::new()));
        assert_eq!(split_command("   "), (String::new(), String::new()));
    }
}
