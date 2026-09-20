use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use anyhow::Result;
use gio::prelude::{Cast, FileExt};
use walkdir::WalkDir;

use crate::plugin::{Meta, Plugin};
use crate::system::fs::get_home;
use crate::system::icon::{find_first_icon_path, resolve};
use crate::wire::{Action, ActionItem, PanelAction, ResultItem};

/// The icon the system MIME database assigns to `path`. Its themed-icon list is
/// a priority order, so the first name the theme actually ships wins.
fn mime_icon(path: &Path) -> Option<String> {
    let (content_type, _) = gio::content_type_guess(Some(path), None);
    let icon = gio::content_type_get_icon(&content_type);
    let themed = icon.downcast::<gio::ThemedIcon>().ok()?;
    let names = themed.names();
    find_first_icon_path(names.iter().map(|name| name.as_str()))
}

macro_rules! search_plugin {
    ($name:ident, $id:literal, $display:literal, $by_name:literal, $dirs:literal, $ready:literal) => {
        pub struct $name;

        impl Plugin for $name {
            fn meta(&self) -> &Meta {
                &Meta {
                    id: $id,
                    name: $display,
                    icon: "folder",
                    ready: $ready,
                }
            }

            fn search(
                &self,
                query: &str,
                _full: &str,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
                // Keep the query's case: a path query is stat'd against the real
                // filesystem, where case matters.
                let query = query.to_string();
                Box::pin(async move {
                    Ok(
                        tokio::task::spawn_blocking(move || do_search(&query, $dirs, $by_name))
                            .await
                            .unwrap_or_default(),
                    )
                })
            }

            fn actions(&self, item: &ResultItem) -> Vec<ActionItem> {
                file_actions(item)
            }
        }
    };
}

/// The commands a file or directory row carries: show it in the file manager,
/// copy its decoded path, or open a terminal in it (its parent when a file).
fn file_actions(item: &ResultItem) -> Vec<ActionItem> {
    let Some(Action::Open { uri }) = item.on_click.as_ref() else {
        return Vec::new();
    };
    if !uri.starts_with("file:") {
        return Vec::new();
    }
    let Some(path) = gio::File::for_uri(uri).path() else {
        return Vec::new();
    };
    let path = path.to_string_lossy();
    vec![
        ActionItem {
            title: "Reveal in file manager".to_string(),
            action: PanelAction::Execute {
                command: Action::Reveal { uri: uri.clone() },
            },
            icon: Some("folder-open".to_string()),
        },
        ActionItem {
            title: "Copy path".to_string(),
            action: PanelAction::Execute {
                command: Action::Copy {
                    text: path.into_owned(),
                },
            },
            icon: Some("edit-copy".to_string()),
        },
        ActionItem {
            title: "Open in terminal".to_string(),
            action: PanelAction::Execute {
                command: Action::Terminal { uri: uri.clone() },
            },
            icon: Some("utilities-terminal".to_string()),
        },
    ]
}

search_plugin!(
    FileSearch,
    "file-search",
    "Files",
    true,
    false,
    "Search files by name or path"
);
search_plugin!(
    PathSearch,
    "path-search",
    "Directories",
    false,
    true,
    "Search directories by path"
);

/// Exact, then prefix, then substring, so a short exact name outranks a longer
/// name that merely contains the query. Inputs are lowercased.
fn name_tier(name: &str, query: &str) -> u32 {
    if name == query {
        1000
    } else if name.starts_with(query) {
        700
    } else if name.contains(query) {
        400
    } else {
        0
    }
}

/// `file-search` cares about the name only; shallower paths break ties.
fn score_name(name: &str, _path: &str, query: &str, depth: usize) -> u32 {
    name_tier(&name.to_lowercase(), query).saturating_sub(depth as u32)
}

/// `path-search` matches when every token is somewhere on the path, but a name
/// hit still outranks a parent-directory-only hit.
fn score_path(name: &str, path: &str, query: &str, depth: usize) -> u32 {
    let path = path.to_lowercase();
    if !query.split_whitespace().all(|token| path.contains(token)) {
        return 0;
    }
    name_tier(&name.to_lowercase(), query)
        .max(200)
        .saturating_sub(depth as u32)
}

/// The path a path-like query names, before it is checked against the disk:
/// `~` and a relative path resolve under `home`, an absolute path stays put.
/// `None` when the query has no path syntax.
fn resolve_path_query(query: &str, home: &Path) -> Option<PathBuf> {
    if query.starts_with('~') {
        Some(home.join(query.trim_start_matches('~').trim_start_matches('/')))
    } else if query.starts_with('/') {
        Some(PathBuf::from(query))
    } else if query.contains('/') {
        Some(home.join(query))
    } else {
        None
    }
}

/// The existing path a query names, canonicalized. An absolute path is taken
/// as-is, wherever it points; `~` and a relative path resolve under `$HOME` and
/// may not climb back out of it.
fn exact_path(query: &str, home: &Path) -> Option<PathBuf> {
    let absolute = query.starts_with('/');
    let candidate = resolve_path_query(query, home)?.canonicalize().ok()?;
    if absolute {
        return Some(candidate);
    }
    let home = home.canonicalize().ok()?;
    candidate.starts_with(home).then_some(candidate)
}

/// One walked or stat'd path as a result row. GLib builds the URI, because raw
/// paths are invalid for spaces and non-ASCII.
fn entry_item(path: &Path, is_dir: bool) -> ResultItem {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    ResultItem {
        on_click: Some(Action::Open {
            uri: gio::File::for_path(path).uri().to_string(),
        }),
        title: if is_dir { format!("{name}/") } else { name },
        summary: Some(path.to_string_lossy().into_owned()),
        icon: if is_dir {
            resolve("folder")
        } else {
            mime_icon(path)
        },
        ephemeral: false,
        actions: Vec::new(),
        badge: None,
    }
}

/// Walk filter: skip hidden dirs and build caches, and skip the three roots at
/// depth 1 under `~` (they are walked on their own), so nothing is doubled.
fn keep_entry(name: &str, depth: usize) -> bool {
    if name.starts_with('.') || name == "node_modules" || name == "target" || name == "__pycache__"
    {
        return false;
    }
    !(depth == 1 && matches!(name, "Desktop" | "Documents" | "Downloads"))
}

/// A walk may match thousands of entries; rank the first `MATCH_CAP` and show
/// the best 50, so one keystroke cannot walk an unbounded tree.
const MATCH_CAP: usize = 200;

/// `want_dir` keeps `f` to files and `d` to directories, so the two providers
/// stay complementary. `by_name` is `f`'s name-only match; a path query always
/// matches the path, whichever provider is asking.
fn do_search(query: &str, want_dir: bool, by_name: bool) -> Vec<ResultItem> {
    let query = query.trim();
    if query.is_empty() {
        return vec![];
    }

    let Ok(home) = get_home() else {
        return vec![];
    };

    // An existing path is the answer itself, outside the walk's depth and
    // roots; an absolute path may point anywhere, a relative one stays home.
    if let Some(path) = exact_path(query, &home)
        && let Some(item) = path_item(&path, want_dir)
    {
        return vec![item];
    }

    let path_mode = query.starts_with('~') || query.contains('/');
    let query_lower = query.to_lowercase();
    let scorer = if by_name && !path_mode {
        score_name
    } else {
        score_path
    };

    let roots = [
        home.join("Desktop"),
        home.join("Documents"),
        home.join("Downloads"),
        home.clone(),
    ];

    let mut scored: Vec<(u32, ResultItem)> = Vec::new();

    for root in &roots {
        if !root.exists() {
            continue;
        }

        let walker = WalkDir::new(root)
            .max_depth(3)
            .into_iter()
            .filter_entry(|e| keep_entry(&e.file_name().to_string_lossy(), e.depth()));

        for entry in walker.filter_map(Result::ok) {
            let ft = entry.file_type();
            let is_dir = ft.is_dir();
            if (!is_dir && !ft.is_file()) || is_dir != want_dir {
                continue;
            }

            let path = entry.path();
            let name = entry.file_name().to_string_lossy();
            let score = scorer(&name, &path.to_string_lossy(), &query_lower, entry.depth());
            if score == 0 {
                continue;
            }

            scored.push((score, entry_item(path, is_dir)));

            if scored.len() >= MATCH_CAP {
                break;
            }
        }

        if scored.len() >= MATCH_CAP {
            break;
        }
    }

    crate::provider::rank_results(scored, false, 50)
}

/// A stat'd path as one row, or `None` when it is not the kind this provider
/// lists (`f` files, `d` directories).
fn path_item(path: &Path, want_dir: bool) -> Option<ResultItem> {
    let is_dir = std::fs::metadata(path).ok()?.is_dir();
    (is_dir == want_dir).then(|| entry_item(path, is_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, on_click: Option<Action>) -> ResultItem {
        ResultItem {
            title: title.to_string(),
            summary: None,
            on_click,
            icon: None,
            ephemeral: false,
            actions: Vec::new(),
            badge: None,
        }
    }

    fn file(uri: &str) -> Option<Action> {
        Some(Action::Open {
            uri: uri.to_string(),
        })
    }

    #[test]
    fn a_file_row_offers_reveal_copy_and_terminal() {
        let actions: Vec<ActionItem> = file_actions(&row("a.txt", file("file:///tmp/a.txt")));
        let titles: Vec<&str> = actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Reveal in file manager", "Copy path", "Open in terminal"]
        );
        assert_eq!(
            actions[0].action,
            PanelAction::Execute {
                command: Action::Reveal {
                    uri: "file:///tmp/a.txt".to_string()
                }
            }
        );
        assert_eq!(
            actions[1].action,
            PanelAction::Execute {
                command: Action::Copy {
                    text: "/tmp/a.txt".to_string()
                }
            }
        );
        assert_eq!(
            actions[2].action,
            PanelAction::Execute {
                command: Action::Terminal {
                    uri: "file:///tmp/a.txt".to_string()
                }
            }
        );

        assert!(
            file_actions(&row(
                "run",
                Some(Action::Run {
                    cmd: "ls".to_string()
                })
            ))
            .is_empty()
        );
        assert!(file_actions(&row("none", None)).is_empty());
    }

    #[test]
    fn the_copied_path_is_percent_decoded() {
        let actions = file_actions(&row("a b", file("file:///tmp/a%20b")));
        assert_eq!(
            actions[1].action,
            PanelAction::Execute {
                command: Action::Copy {
                    text: "/tmp/a b".to_string()
                }
            }
        );
    }

    #[test]
    fn a_suffix_maps_to_its_mime_icon_without_a_hand_kept_table() {
        if !Path::new("/usr/share/icons/Papirus").exists() {
            return;
        }
        // `g_content_type_guess` needs only the name; no file is read.
        let script = mime_icon(Path::new("/tmp/build.sh")).unwrap();
        assert!(
            script.ends_with("text-x-shellscript.svg") || script.ends_with("text-x-script.svg"),
            "{script}"
        );
        let unknown = mime_icon(Path::new("/tmp/notes.zzz")).unwrap();
        assert!(
            unknown.ends_with("application-octet-stream.svg")
                || unknown.ends_with("application-x-generic.svg"),
            "{unknown}"
        );
        assert_ne!(script, unknown);
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(do_search("", false, true).is_empty());
        assert!(do_search("", true, false).is_empty());
    }

    #[test]
    fn an_exact_name_outranks_a_longer_prefix_match() {
        let query = "wayrun";
        let exact = score_name("WayRun", "", query, 2);
        let prefix = score_name("wayrun-x86_64-unknown-linux-gnu.zip", "", query, 1);
        let contains = score_name("my-wayrun-notes.txt", "", query, 1);
        assert!(exact > prefix, "{exact} > {prefix}");
        assert!(prefix > contains, "{prefix} > {contains}");
        assert_eq!(score_name("unrelated.txt", "", query, 0), 0);
    }

    #[test]
    fn a_path_only_hit_ranks_below_a_name_hit() {
        let query = "wayrun";
        let named = score_path("WayRun", "/home/u/Project/WayRun", query, 2);
        let nested = score_path("core", "/home/u/Project/WayRun/core", query, 3);
        assert!(named > nested, "{named} > {nested}");
        assert_eq!(score_path("core", "/home/u/other/core", query, 1), 0);
    }

    #[test]
    fn a_shallower_path_breaks_a_tier_tie() {
        let query = "wayrun";
        assert!(score_name("wayrun", "", query, 1) > score_name("wayrun", "", query, 3));
    }

    #[test]
    fn a_path_query_resolves_under_home() {
        let home = Path::new("/home/u");
        assert_eq!(
            resolve_path_query("~/Project/WayRun", home),
            Some(PathBuf::from("/home/u/Project/WayRun"))
        );
        assert_eq!(
            resolve_path_query("~", home),
            Some(PathBuf::from("/home/u"))
        );
        assert_eq!(
            resolve_path_query("/etc/hosts", home),
            Some(PathBuf::from("/etc/hosts"))
        );
        assert_eq!(
            resolve_path_query("Project/WayRun", home),
            Some(PathBuf::from("/home/u/Project/WayRun"))
        );
        // a bare name is not a path query
        assert_eq!(resolve_path_query("wayrun", home), None);
    }

    #[test]
    fn an_absolute_path_is_allowed_outside_home_but_a_relative_escape_is_not() {
        let home = std::env::temp_dir();
        if Path::new("/etc/hosts").exists() {
            let canonical = Path::new("/etc/hosts").canonicalize().unwrap();
            assert_eq!(exact_path("/etc/hosts", &home), Some(canonical));
            // `~` and relative paths resolve under home and cannot climb out
            assert!(exact_path("~/../etc/hosts", &home).is_none());
            assert!(exact_path("../../etc/hosts", &home).is_none());
        }
    }

    #[test]
    fn the_home_roots_are_skipped_only_under_the_home_root() {
        // depth 1 under `~`: walked as its own root, so skip it here.
        assert!(!keep_entry("Desktop", 1));
        assert!(!keep_entry("Documents", 1));
        assert!(!keep_entry("Downloads", 1));
        // a same-named dir deeper, or one directly inside a root, is kept.
        assert!(keep_entry("Desktop", 2));
        assert!(keep_entry("Desktop", 0));
        assert!(keep_entry("Documents", 2));
        // build caches and dotdirs are skipped anywhere.
        assert!(!keep_entry(".config", 1));
        assert!(!keep_entry("node_modules", 1));
        assert!(!keep_entry("target", 3));
        assert!(!keep_entry("__pycache__", 2));
        assert!(keep_entry("notes.txt", 2));
    }
}
