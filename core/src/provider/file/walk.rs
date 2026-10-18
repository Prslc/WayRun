use std::cmp::Reverse;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::config::Files;
use crate::provider::{SHOW_CAP, push_lowered, rank_results};

use super::entry_item;
use super::keep_name;
use super::path::{score_name, score_path};

/// The user roots are walked on their own, so skip them at depth 1 under `~`
/// rather than twice.
fn keep_entry(name: &str, depth: usize, exclude: &[String], user_dirs: &[String]) -> bool {
    keep_name(OsStr::new(name), exclude) && !(depth == 1 && user_dirs.iter().any(|dir| dir == name))
}

/// The XDG user dirs (Desktop, Documents, Downloads), else the conventional
/// names under `home`; a different root is a test fixture and takes the names.
fn user_roots(home: &Path) -> Vec<PathBuf> {
    let conventional = || {
        vec![
            home.join("Desktop"),
            home.join("Documents"),
            home.join("Downloads"),
        ]
    };
    let roots = if dirs::home_dir().is_some_and(|real| real.as_path() == home) {
        let known = dirs::config_dir().is_some_and(|dir| dir.join("user-dirs.dirs").is_file());
        if known {
            [
                dirs::desktop_dir(),
                dirs::document_dir(),
                dirs::download_dir(),
            ]
            .into_iter()
            .flatten()
            .collect()
        } else {
            conventional()
        }
    } else {
        conventional()
    };
    let mut unique: Vec<PathBuf> = Vec::new();
    for root in roots {
        if !unique.contains(&root) {
            unique.push(root);
        }
    }
    if !unique.iter().any(|root| root.as_path() == home) {
        unique.push(home.to_path_buf());
    }
    unique
}

/// A walk may match thousands of entries; rank the first `MATCH_CAP` and show
/// the best `SHOW_CAP`, so one keystroke cannot walk an unbounded tree.
const MATCH_CAP: usize = 200;

/// Fallback walk depth: levels searched from each root while nothing is indexed.
/// Not a config key: with the index on (the default) it has no effect at all.
const FALLBACK_DEPTH: usize = 3;

/// The fallback when no index is available: the user roots plus `~`, at most
/// [`FALLBACK_DEPTH`] levels deep.
pub(super) fn quick_search(
    home: &Path,
    query_lower: &str,
    want_dir: bool,
    name_only: bool,
    files: &Files,
) -> Vec<crate::wire::ResultItem> {
    let roots = user_roots(home);
    let user_dirs: Vec<String> = roots
        .iter()
        .filter(|root| root.parent() == Some(home))
        .filter_map(|root| {
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect();

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
                keep_entry(
                    &e.file_name().to_string_lossy(),
                    e.depth(),
                    &files.exclude,
                    &user_dirs,
                )
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
    scored.truncate(SHOW_CAP);
    let rows: Vec<(u32, crate::wire::ResultItem)> = scored
        .into_iter()
        .map(|(score, path, is_dir)| (score, entry_item(&path, is_dir)))
        .collect();
    rank_results(rows, false, SHOW_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names a stock install skips: the shipped template's list, the one
    /// place those defaults are written down.
    fn shipped_exclude() -> Vec<String> {
        toml::from_str::<crate::config::Config>(crate::config::DEFAULT_TEMPLATE)
            .expect("the template parses")
            .files
            .exclude
    }

    #[test]
    fn the_home_roots_are_skipped_only_under_the_home_root() {
        let exclude = shipped_exclude();
        let user_dirs = ["Desktop", "Documents", "Downloads"]
            .map(str::to_string)
            .to_vec();
        // depth 1 under `~`: walked as its own root, so skip it here.
        assert!(!keep_entry("Desktop", 1, &exclude, &user_dirs));
        assert!(!keep_entry("Documents", 1, &exclude, &user_dirs));
        assert!(!keep_entry("Downloads", 1, &exclude, &user_dirs));
        // a same-named dir deeper, or one directly inside a root, is kept.
        assert!(keep_entry("Desktop", 2, &exclude, &user_dirs));
        assert!(keep_entry("Desktop", 0, &exclude, &user_dirs));
        assert!(keep_entry("Documents", 2, &exclude, &user_dirs));
        // a localized user dir is skipped the same way.
        let localized = ["文档".to_string()];
        assert!(!keep_entry("文档", 1, &exclude, &localized));
        assert!(keep_entry("文档", 2, &exclude, &localized));
        // build caches and dotdirs are skipped anywhere.
        assert!(!keep_entry(".config", 1, &exclude, &user_dirs));
        assert!(!keep_entry("node_modules", 1, &exclude, &user_dirs));
        assert!(!keep_entry("target", 3, &exclude, &user_dirs));
        assert!(!keep_entry("__pycache__", 2, &exclude, &user_dirs));
        assert!(keep_entry("notes.txt", 2, &exclude, &user_dirs));
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
