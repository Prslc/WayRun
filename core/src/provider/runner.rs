use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::pin::Pin;
use std::sync::{LazyLock, Mutex};

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::plugin::{Meta, Plugin};
use crate::wire::{Action, ResultItem};
use rust_i18n::t;

pub struct Runner {
    meta: Meta,
}

impl Runner {
    pub fn new() -> Self {
        Self {
            meta: Meta {
                id: "runner",
                name: t!("plugin.runner.name"),
                icon: "builtin:terminal",
                ready: t!("plugin.runner.ready"),
            },
        }
    }
}

impl Plugin for Runner {
    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn search(
        &self,
        query: &str,
        _full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        let input = query.to_string();
        Box::pin(async move {
            Ok(tokio::task::spawn_blocking(move || do_search(&input))
                .await
                .unwrap_or_default())
        })
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

/// One executable on `$PATH` with the keys a search matches on, precomputed so
/// a search does not re-lower the whole PATH on every keystroke.
struct Binary {
    name: String,
    name_lower: String,
    key: nucleo::Utf32String,
    path: String,
}

/// Every executable on `$PATH`, first match wins, scanned once per process.
/// Dot-containing names are skipped as library noise.
fn binaries() -> &'static Vec<Binary> {
    static LIST: LazyLock<Vec<Binary>> = LazyLock::new(scan_path);
    &LIST
}

fn scan_path() -> Vec<Binary> {
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
                let name_lower = name.to_lowercase();
                out.push(Binary {
                    key: nucleo::Utf32String::from(name_lower.as_str()),
                    name: name.to_string(),
                    name_lower,
                    path: path.to_string_lossy().into_owned(),
                });
            }
        }
    }
    out
}

/// The command for a hit: an installed app launches through `gio` so its
/// `Terminal=` and env are honored; anything else needs a tty (a PATH tool is
/// usually interactive) and runs in a terminal.
fn action_for(desktop_id: Option<&str>, terminal: bool, has_args: bool, run_cmd: String) -> Action {
    match desktop_id {
        Some(id) if !has_args => Action::Launch {
            desktop_id: id.to_string(),
        },
        Some(_) if !terminal => Action::Run { cmd: run_cmd },
        _ => Action::RunInTerminal { cmd: run_cmd },
    }
}

/// The row score: a name tier dominates (exact > prefix > substring), the fuzzy
/// score breaks ties within a tier and is the only match for a subsequence.
fn score(name_lower: &str, query_lower: &str, fuzzy: u32) -> u32 {
    let tier = crate::provider::name_tier(name_lower, query_lower);
    if tier > 0 { tier * 1000 + fuzzy } else { fuzzy }
}

/// Rows are built only for the winners, so a broad query allocates no row per PATH hit.
const MAX_RESULTS: usize = 20;

fn do_search(input: &str) -> Vec<ResultItem> {
    let (cmd, args) = split_command(input);
    if cmd.is_empty() {
        return Vec::new();
    }

    let query = cmd.to_lowercase();
    let mut matcher = nucleo::Matcher::new(nucleo::Config::DEFAULT);
    let pattern = nucleo::Utf32String::from(query.as_str());

    let mut hits: Vec<(u32, &'static Binary)> = Vec::new();
    for binary in binaries() {
        let fuzzy = matcher
            .fuzzy_match(binary.key.slice(..), pattern.slice(..))
            .unwrap_or(0) as u32;
        let score = score(&binary.name_lower, &query, fuzzy);
        if score > 0 {
            hits.push((score, binary));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0));

    hits.into_iter()
        .take(MAX_RESULTS)
        .map(|(_, binary)| row_for(binary, &args))
        .collect()
}

/// One hit as a row; a query with args carries them along the command.
fn row_for(binary: &Binary, args: &str) -> ResultItem {
    let run_cmd = if args.is_empty() {
        binary.path.clone()
    } else {
        format!("{} {args}", binary.path)
    };
    let desktop_id = crate::provider::application::desktop_id_for_exec(&binary.name);
    let terminal = !args.is_empty() && desktop_id.is_some_and(needs_terminal);
    ResultItem {
        title: binary.name.clone(),
        summary: Some(run_cmd.clone()),
        on_click: Some(action_for(desktop_id, terminal, !args.is_empty(), run_cmd)),
        // `fill_icons` gives every row the plugin's terminal icon
        icon: None,
        ephemeral: false,
        actions: Vec::new(),
        badge: None,
    }
}

/// Whether the app's entry asks for a terminal, read once per desktop id.
fn needs_terminal(desktop_id: &str) -> bool {
    static CACHE: LazyLock<Mutex<HashMap<String, bool>>> =
        LazyLock::new(|| Mutex::new(HashMap::default()));
    if let Ok(cache) = CACHE.lock()
        && let Some(terminal) = cache.get(desktop_id)
    {
        return *terminal;
    }
    let terminal = crate::system::desktop_action::entry(desktop_id, None)
        .is_some_and(|entry| entry.terminal());
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(desktop_id.to_string(), terminal);
    }
    terminal
}

#[cfg(test)]
mod tests {
    use super::{action_for, score, split_command};
    use crate::wire::Action;

    #[test]
    fn an_exact_name_outranks_a_higher_scoring_prefix() {
        assert!(
            score("cat", "cat", 100) > score("catatonit", "cat", 60_000),
            "an exact name must beat a prefix however fuzzy scores it"
        );
        assert!(
            score("catatonit", "cat", 0) > score("pw-cat", "cat", 60_000),
            "a prefix must beat a substring however fuzzy scores it"
        );
        assert!(
            score("pw-cat", "cat", 0) > score("chart", "cat", 60_000),
            "a substring must beat a mere subsequence however fuzzy scores it"
        );
    }

    #[test]
    fn a_non_matching_name_scores_zero() {
        assert_eq!(score("ls", "cat", 0), 0);
    }

    #[test]
    fn an_installed_app_launches_through_gio() {
        assert_eq!(
            action_for(Some("btop.desktop"), true, false, "/usr/bin/btop".into()),
            Action::Launch {
                desktop_id: "btop.desktop".into()
            }
        );
    }

    #[test]
    fn a_terminal_app_with_args_runs_in_a_terminal() {
        assert_eq!(
            action_for(
                Some("nvim.desktop"),
                true,
                true,
                "/usr/bin/nvim main.rs".into()
            ),
            Action::RunInTerminal {
                cmd: "/usr/bin/nvim main.rs".into()
            }
        );
    }

    #[test]
    fn a_gui_app_with_args_runs_detached() {
        assert_eq!(
            action_for(
                Some("firefox.desktop"),
                false,
                true,
                "/usr/bin/firefox --new-window".into()
            ),
            Action::Run {
                cmd: "/usr/bin/firefox --new-window".into()
            }
        );
    }

    #[test]
    fn a_bare_binary_runs_in_a_terminal() {
        assert_eq!(
            action_for(None, false, true, "/usr/bin/htop".into()),
            Action::RunInTerminal {
                cmd: "/usr/bin/htop".into()
            }
        );
    }

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
