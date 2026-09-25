use std::cmp::Reverse;
use std::ffi::OsStr;
use std::future::Future;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use anyhow::Result;
use gio::prelude::FileExt;
use walkdir::WalkDir;

use crate::config::Files;
use crate::plugin::{Meta, Plugin};
use crate::provider::push_lowered;
use crate::system::fs::get_home;
use crate::system::icon::{content_type_icon, resolve};
use crate::wire::{Action, ActionItem, PanelAction, ResultItem};
use rust_i18n::t;

// The index is a detail of this provider, but the registry has to know whether
// to keep its cache alive, so the crate may reach it.
pub(crate) mod index;

/// The icon the system MIME database assigns to `path`.
fn mime_icon(path: &Path) -> Option<String> {
    let (content_type, _) = gio::content_type_guess(Some(path), None);
    content_type_icon(&content_type)
}

macro_rules! search_plugin {
    ($name:ident, $id:literal, $display:expr, $icon:literal, $by_name:literal, $dirs:literal, $ready:expr) => {
        pub struct $name {
            meta: Meta,
        }

        impl $name {
            pub fn new() -> Self {
                Self {
                    meta: Meta {
                        id: $id,
                        name: $display,
                        icon: $icon,
                        ready: $ready,
                    },
                }
            }
        }

        impl Plugin for $name {
            fn meta(&self) -> &Meta {
                &self.meta
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
                    index::ensure().await;
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

/// A file or directory row's commands, in panel order: a terminal leads so the
/// two ways to open sit together, then reveal in the file manager, then copy its path.
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
            title: t!("action.terminal"),
            action: PanelAction::Execute {
                command: Action::Terminal { uri: uri.clone() },
            },
            icon: Some("builtin:terminal".to_string()),
            id: Some("terminal".to_string()),
            plugin: None,
            default: false,
        },
        ActionItem {
            title: t!("action.reveal"),
            action: PanelAction::Execute {
                command: Action::Reveal { uri: uri.clone() },
            },
            icon: Some("builtin:reveal".to_string()),
            id: Some("reveal".to_string()),
            plugin: None,
            default: false,
        },
        ActionItem {
            title: t!("action.copy_path"),
            action: PanelAction::Execute {
                command: Action::Copy {
                    text: path.into_owned(),
                },
            },
            icon: Some("builtin:copy".to_string()),
            id: Some("copy_path".to_string()),
            plugin: None,
            default: false,
        },
    ]
}

search_plugin!(
    FileSearch,
    "file-search",
    t!("plugin.file.name"),
    "builtin:file",
    true,
    false,
    t!("plugin.file.ready")
);
search_plugin!(
    PathSearch,
    "path-search",
    t!("plugin.directory.name"),
    "builtin:folder",
    false,
    true,
    t!("plugin.directory.ready")
);

/// `file-search` cares about the name only; shallower paths break ties.
/// `query_lower` is already lowercased.
pub(super) fn score_name(name: &str, query_lower: &str, depth: usize) -> u32 {
    kind_weight(name, query_lower).saturating_sub(depth as u32)
}

/// The shared kind's weight for a plain query: a scattered hit is not a row.
fn kind_weight(name: &str, query_lower: &str) -> u32 {
    crate::plugin::classify_ci_confident(name, query_lower).map_or(0, crate::plugin::Match::weight)
}

/// `path-search` matches when every token is somewhere on the path, but a name
/// hit still outranks a parent-directory-only hit. All inputs are lowercased.
pub(super) fn score_path(
    name: &str,
    path_lower: &str,
    query_lower: &str,
    tokens: &[&str],
    depth: usize,
) -> u32 {
    if !tokens.iter().all(|token| path_lower.contains(*token)) {
        return 0;
    }
    path_score(name, query_lower, depth)
}

/// [`score_path`] for a caller that holds the path as two lowercased halves:
/// the directory (no trailing separator) and the leaf, never joined per entry.
pub(super) fn score_split_path(
    name: &str,
    dir_lower: &str,
    name_lower: &str,
    query_lower: &str,
    tokens: &[&str],
    depth: usize,
) -> u32 {
    if !tokens
        .iter()
        .all(|token| token_on_path(dir_lower, name_lower, token))
    {
        return 0;
    }
    path_score(name, query_lower, depth)
}

/// The path-mode score once a match is known: a name hit beats a
/// parent-directory-only hit, and shallower paths break ties.
fn path_score(name: &str, query_lower: &str, depth: usize) -> u32 {
    kind_weight(name, query_lower)
        .max(PATH_ONLY)
        .saturating_sub(depth as u32)
}

/// A hit only on a parent directory still lists the row, below any name hit.
const PATH_ONLY: u32 = 200;

/// Whether `token` occurs in `dir_lower + "/" + name_lower`: inside either
/// half, or across the separator (`sub/deep` matching across the join).
fn token_on_path(dir_lower: &str, name_lower: &str, token: &str) -> bool {
    if dir_lower.contains(token) || name_lower.contains(token) {
        return true;
    }
    token
        .match_indices('/')
        .any(|(i, _)| dir_lower.ends_with(&token[..i]) && name_lower.starts_with(&token[i + 1..]))
}

/// The path a path-like query names, before any disk check: `~` and a relative
/// path resolve under `home`, absolute stays put; `None` without path syntax.
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

/// The existing path a query names, canonicalized: an absolute path is taken
/// wherever it points; `~` and relative resolve under `$HOME`, never climbing out.
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
pub(super) fn entry_item(path: &Path, is_dir: bool) -> ResultItem {
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
            resolve("builtin:folder")
        } else {
            mime_icon(path)
        },
        ephemeral: false,
        actions: Vec::new(),
        badge: None,
    }
}

/// Walk filter: hidden entries and the configured names never enter the index
/// or a result.
pub(super) fn keep_name(name: &OsStr, exclude: &[String]) -> bool {
    let name = name.as_bytes();
    !name.starts_with(b".") && !exclude.iter().any(|entry| entry.as_bytes() == name)
}

/// The quick walk's four roots: `Desktop`/`Documents`/`Downloads` are walked on
/// their own, so skip them at depth 1 under `~` rather than twice.
fn keep_entry(name: &str, depth: usize, exclude: &[String]) -> bool {
    keep_name(OsStr::new(name), exclude)
        && !(depth == 1 && matches!(name, "Desktop" | "Documents" | "Downloads"))
}

/// A walk may match thousands of entries; rank the first `MATCH_CAP` and show
/// the best `SHOW_CAP`, so one keystroke cannot walk an unbounded tree.
const MATCH_CAP: usize = 200;

/// `want_dir` keeps `f` to files and `d` to directories, so the two providers stay
/// complementary; `by_name` is `f`'s name-only match; a path query matches the path.
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
    let name_only = by_name && !path_mode;
    let query_lower = query.to_lowercase();

    if let Some(items) = index::search(&home, &query_lower, want_dir, name_only) {
        return items;
    }

    quick_search(
        &home,
        &query_lower,
        want_dir,
        name_only,
        &crate::config::get().files,
    )
}

/// Fallback walk depth: levels searched from each root while nothing is indexed.
/// Not a config key: with the index on (the default) it has no effect at all.
const FALLBACK_DEPTH: usize = 3;

/// The fallback when no index is available: the four roots, [`FALLBACK_DEPTH`]
/// levels.
fn quick_search(
    home: &Path,
    query_lower: &str,
    want_dir: bool,
    name_only: bool,
    files: &Files,
) -> Vec<ResultItem> {
    let roots = [
        home.join("Desktop"),
        home.join("Documents"),
        home.join("Downloads"),
        home.to_path_buf(),
    ];

    let mut scored: Vec<(u32, PathBuf, bool)> = Vec::new();
    let mut path_lower = String::new();
    let tokens: Vec<&str> = query_lower.split_whitespace().collect();

    for root in &roots {
        if !root.exists() {
            continue;
        }

        let walker = WalkDir::new(root)
            .max_depth(FALLBACK_DEPTH)
            .into_iter()
            .filter_entry(|e| {
                keep_entry(&e.file_name().to_string_lossy(), e.depth(), &files.exclude)
            });

        for entry in walker.filter_map(Result::ok) {
            let ft = entry.file_type();
            let is_dir = ft.is_dir();
            if (!is_dir && !ft.is_file()) || is_dir != want_dir {
                continue;
            }

            let depth = entry.depth();
            let path = entry.into_path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            // the name tier is all `f` needs; only a path query lowercases the full path
            let score = if name_only {
                score_name(&name, query_lower, depth)
            } else {
                path_lower.clear();
                push_lowered(&mut path_lower, &path.to_string_lossy());
                score_path(&name, &path_lower, query_lower, &tokens, depth)
            };
            if score == 0 {
                continue;
            }

            scored.push((score, path, is_dir));

            if scored.len() >= MATCH_CAP {
                break;
            }
        }

        if scored.len() >= MATCH_CAP {
            break;
        }
    }

    // Rows only for the survivors: `entry_item` builds a gio URI and looks up a
    // MIME icon, work the discarded rest of `MATCH_CAP` would waste.
    scored.sort_by_key(|a| Reverse(a.0));
    scored.truncate(crate::provider::SHOW_CAP);
    let rows: Vec<(u32, ResultItem)> = scored
        .into_iter()
        .map(|(score, path, is_dir)| (score, entry_item(&path, is_dir)))
        .collect();
    crate::provider::rank_results(rows, false, crate::provider::SHOW_CAP)
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

    fn tokens(query: &str) -> Vec<&str> {
        query.split_whitespace().collect()
    }

    /// The names a stock install skips: the shipped template's list, the one
    /// place those defaults are written down.
    fn shipped_exclude() -> Vec<String> {
        toml::from_str::<crate::config::Config>(crate::config::DEFAULT_TEMPLATE)
            .expect("the template parses")
            .files
            .exclude
    }

    #[test]
    fn a_file_row_offers_terminal_reveal_and_copy() {
        let actions: Vec<ActionItem> = file_actions(&row("a.txt", file("file:///tmp/a.txt")));
        let titles: Vec<&str> = actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                t!("action.terminal"),
                t!("action.reveal"),
                t!("action.copy_path")
            ],
            "opening leads, so it sits next to the row's own command"
        );
        assert_eq!(
            actions[0].action,
            PanelAction::Execute {
                command: Action::Terminal {
                    uri: "file:///tmp/a.txt".to_string()
                }
            }
        );
        assert_eq!(
            actions[1].action,
            PanelAction::Execute {
                command: Action::Reveal {
                    uri: "file:///tmp/a.txt".to_string()
                }
            }
        );
        assert_eq!(
            actions[2].action,
            PanelAction::Execute {
                command: Action::Copy {
                    text: "/tmp/a.txt".to_string()
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
        let copy = actions
            .iter()
            .find(|action| action.id.as_deref() == Some("copy_path"))
            .expect("the row offers a copy");
        assert_eq!(
            copy.action,
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
        // the same type answers the same icon on a repeat lookup
        assert_eq!(
            mime_icon(Path::new("/tmp/build.sh")).as_ref(),
            Some(&script)
        );
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(do_search("", false, true).is_empty());
        assert!(do_search("", true, false).is_empty());
    }

    #[test]
    fn an_exact_name_outranks_a_longer_prefix_match() {
        let query = "wayrun";
        let exact = score_name("WayRun", query, 2);
        let prefix = score_name("wayrun-x86_64-unknown-linux-gnu.zip", query, 1);
        let contains = score_name("my-wayrun-notes.txt", query, 1);
        assert!(exact > prefix, "{exact} > {prefix}");
        assert!(prefix > contains, "{prefix} > {contains}");
        assert_eq!(score_name("unrelated.txt", query, 0), 0);
    }

    #[test]
    fn a_path_only_hit_ranks_below_a_name_hit() {
        let query = "wayrun";
        let named = score_path("WayRun", "/home/u/project/wayrun", query, &tokens(query), 2);
        let nested = score_path(
            "core",
            "/home/u/project/wayrun/core",
            query,
            &tokens(query),
            3,
        );
        assert!(named > nested, "{named} > {nested}");
        assert_eq!(
            score_path("core", "/home/u/other/core", query, &tokens(query), 1),
            0
        );
    }

    #[test]
    fn the_split_path_scores_the_same_as_the_joined_one() {
        for (dir, name) in [
            ("/home/u/project", "wayrun"),
            ("/home/u/Project", "WayRun.zip"),
            ("/home/u/报告", "笔记.txt"),
            ("/home", "u"),
        ] {
            let joined = format!("{dir}/{name}").to_lowercase();
            let dir_lower = dir.to_lowercase();
            let name_lower = name.to_lowercase();
            for query in [
                "wayrun",
                "u/wayrun",
                "project/wayrun",
                "u/pro",
                "/wayrun",
                "run",
                "报告",
                "/u",
                "nope",
                "u wayrun",
                "u //wayrun",
            ] {
                assert_eq!(
                    score_split_path(name, &dir_lower, &name_lower, query, &tokens(query), 1),
                    score_path(name, &joined, query, &tokens(query), 1),
                    "{query:?} on {joined:?}"
                );
            }
        }
    }

    #[test]
    fn a_shallower_path_breaks_a_tier_tie() {
        let query = "wayrun";
        assert!(score_name("wayrun", query, 1) > score_name("wayrun", query, 3));
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
        let exclude = shipped_exclude();
        // depth 1 under `~`: walked as its own root, so skip it here.
        assert!(!keep_entry("Desktop", 1, &exclude));
        assert!(!keep_entry("Documents", 1, &exclude));
        assert!(!keep_entry("Downloads", 1, &exclude));
        // a same-named dir deeper, or one directly inside a root, is kept.
        assert!(keep_entry("Desktop", 2, &exclude));
        assert!(keep_entry("Desktop", 0, &exclude));
        assert!(keep_entry("Documents", 2, &exclude));
        // build caches and dotdirs are skipped anywhere.
        assert!(!keep_entry(".config", 1, &exclude));
        assert!(!keep_entry("node_modules", 1, &exclude));
        assert!(!keep_entry("target", 3, &exclude));
        assert!(!keep_entry("__pycache__", 2, &exclude));
        assert!(keep_entry("notes.txt", 2, &exclude));
    }

    #[test]
    fn the_index_prunes_dotdirs_and_whatever_the_config_lists() {
        let shipped = shipped_exclude();
        for name in [".config", ".git", "node_modules", "target", "__pycache__"] {
            assert!(!keep_name(OsStr::new(name), &shipped), "{name}");
        }
        for name in ["Desktop", "Documents", "Downloads", "notes.txt", "src"] {
            assert!(keep_name(OsStr::new(name), &shipped), "{name}");
        }
        // the dot rule holds with no list at all, and a listed name need not be
        // hidden; the shipped caches are the template's list, not a walk rule
        let custom = ["vendor".to_string()];
        assert!(!keep_name(OsStr::new(".git"), &[]));
        assert!(!keep_name(OsStr::new("vendor"), &custom));
        assert!(keep_name(OsStr::new("target"), &[]));
        assert!(keep_name(OsStr::new("notes.txt"), &custom));
    }

    #[test]
    fn the_quick_walk_stops_at_the_fallback_depth() {
        let dir = tempfile::tempdir().unwrap();
        let shallow = dir.path().join("Documents/a/b");
        let deep = shallow.join("c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(shallow.join("shallow.txt"), "x").unwrap();
        std::fs::write(deep.join("deep.txt"), "x").unwrap();

        let files = Files::default();
        let found = quick_search(dir.path(), "shallow.txt", false, true, &files);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].title, "shallow.txt");

        assert!(
            quick_search(dir.path(), "deep.txt", false, true, &files).is_empty(),
            "one level past the fallback depth is out of reach"
        );
    }
}
