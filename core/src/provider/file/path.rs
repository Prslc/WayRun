use std::fs;
use std::path::{Path, PathBuf};

use crate::wire::ResultItem;

use super::entry_item;

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
pub(super) fn resolve_path_query(query: &str, home: &Path) -> Option<PathBuf> {
    if query == "~" {
        Some(home.to_path_buf())
    } else if let Some(rest) = query.strip_prefix("~/") {
        Some(home.join(rest))
    } else if query.starts_with('~') {
        // `~user` is someone else's home, not `$HOME/user`; leave it unresolved.
        None
    } else if query.starts_with('/') {
        Some(PathBuf::from(query))
    } else if query.contains('/') {
        Some(home.join(query))
    } else {
        None
    }
}

/// The directory a listing query names, each component resolved exactly then by
/// case-insensitive name; a relative path never climbs out of `home`.
pub(super) fn resolve_dir(query: &str, home: &Path) -> Option<PathBuf> {
    let relative = !query.starts_with('/');
    let (mut base, rest) = if let Some(rest) = query.strip_prefix("~/") {
        (home.to_path_buf(), rest)
    } else if let Some(rest) = query.strip_prefix('/') {
        (PathBuf::from("/"), rest)
    } else {
        (home.to_path_buf(), query)
    };

    for part in rest.split('/').filter(|part| !part.is_empty()) {
        match part {
            "." => continue,
            ".." => {
                base = base.parent()?.to_path_buf();
                if relative && !base.starts_with(home) {
                    return None;
                }
                continue;
            }
            _ => {}
        }

        let direct = base.join(part);
        if direct.is_dir() {
            base = direct;
            continue;
        }

        let lower = part.to_lowercase();
        let found = fs::read_dir(&base)
            .ok()?
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().to_lowercase() == lower)
            .map(|entry| entry.path())?;
        if !found.is_dir() {
            return None;
        }
        base = found;
    }
    Some(base)
}

/// The existing path a query names, canonicalized: an absolute path is taken
/// wherever it points; `~` and relative resolve under `$HOME`, never climbing out.
pub(super) fn exact_path(query: &str, home: &Path) -> Option<PathBuf> {
    let absolute = query.starts_with('/');
    let candidate = resolve_path_query(query, home)?.canonicalize().ok()?;
    if absolute {
        return Some(candidate);
    }
    let home = home.canonicalize().ok()?;
    candidate.starts_with(home).then_some(candidate)
}

/// A stat'd path as one row, or `None` when it is not the kind this provider
/// lists (`f` files, `d` directories).
pub(super) fn path_item(path: &Path, want_dir: bool) -> Option<ResultItem> {
    let is_dir = fs::metadata(path).ok()?.is_dir();
    (is_dir == want_dir).then(|| entry_item(path, is_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn tokens(query: &str) -> Vec<&str> {
        query.split_whitespace().collect()
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
        // `~user` is not `$HOME/user`
        assert_eq!(resolve_path_query("~someone/x", home), None);
        // a bare name is not a path query
        assert_eq!(resolve_path_query("wayrun", home), None);
    }

    #[test]
    fn an_absolute_path_is_allowed_outside_home_but_a_relative_escape_is_not() {
        let home = env::temp_dir();
        if Path::new("/etc/hosts").exists() {
            let canonical = Path::new("/etc/hosts").canonicalize().unwrap();
            assert_eq!(exact_path("/etc/hosts", &home), Some(canonical));
            // `~` and relative paths resolve under home and cannot climb out
            assert!(exact_path("~/../etc/hosts", &home).is_none());
            assert!(exact_path("../../etc/hosts", &home).is_none());
        }
    }
}
