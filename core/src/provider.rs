pub mod application;
pub mod calculator;
pub mod external;
pub mod file;
pub mod path;
pub(crate) mod resident;
pub mod runner;
pub mod system_commands;

use std::collections::HashMap;

use crate::plugin::Plugin;
use crate::wire::ResultItem;

/// Rows a provider shows: the one cap every scored provider passes to
/// [`rank_results`], so the index and the fallback walk cannot diverge.
pub const SHOW_CAP: usize = 50;

/// The common tail every scored provider shares: strongest score first,
/// optionally one row per title, capped at `max` results.
pub fn rank_results<T: Ord>(
    mut scored: Vec<(T, ResultItem)>,
    dedup_titles: bool,
    max: usize,
) -> Vec<ResultItem> {
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    if dedup_titles {
        scored.dedup_by(|a, b| a.1.title == b.1.title);
    }
    scored.truncate(max);
    scored.into_iter().map(|(_, item)| item).collect()
}

/// Append `s` lowercased without allocating: ASCII goes byte by byte, anything
/// else falls back to `to_lowercase`.
pub fn push_lowered(out: &mut String, s: &str) {
    if s.is_ascii() {
        for b in s.bytes() {
            out.push(b.to_ascii_lowercase() as char);
        }
    } else {
        out.push_str(&s.to_lowercase());
    }
}

pub fn plugin_map() -> HashMap<&'static str, Box<dyn Plugin>> {
    let mut m: HashMap<&'static str, Box<dyn Plugin>> = HashMap::default();
    m.insert("calculator", Box::new(calculator::Calculator::new()));
    m.insert("app-search", Box::new(application::AppSearch::new()));
    m.insert("file-search", Box::new(file::FileSearch::new()));
    m.insert("path-search", Box::new(file::PathSearch::new()));
    m.insert("path-lookup", Box::new(path::PathLookup::new()));
    m.insert(
        "system-commands",
        Box::new(system_commands::SystemCommands::new()),
    );
    m.insert("runner", Box::new(runner::Runner::new()));
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{
        Match, classify, classify_bytes_confident, classify_ci, classify_ci_confident,
        classify_confident,
    };
    #[test]
    fn a_case_insensitive_classify_matches_the_lowered_one() {
        for (name, query) in [
            ("WayRun", "wayrun"),
            ("wayrun", "wayrun"),
            ("wayrun-x86_64-unknown-linux-gnu.zip", "wayrun"),
            ("NOTES.txt", "notes.txt"),
            ("annual-Report.pdf", "report"),
            ("报告.TXT", "报告"),
            ("报告.txt", "report"),
            ("Ünicode", "ünicode"),
            ("x", "wayrun"),
            ("", ""),
        ] {
            assert_eq!(
                classify_ci(name, query),
                classify(&name.to_lowercase(), query),
                "{name} / {query}"
            );
        }
    }

    /// The vocabulary's order is the contract: no provider may rank a weaker
    /// kind over a stronger one.
    #[test]
    fn the_kinds_read_the_way_the_tiers_always_did() {
        assert_eq!(classify("telegram", "telegram"), Some(Match::Exact));
        assert_eq!(classify("telegram", "tele"), Some(Match::Prefix));
        assert_eq!(
            classify("dank material shell", "material"),
            Some(Match::Word)
        );
        assert_eq!(classify("libreoffice", "office"), Some(Match::Substring));
        assert_eq!(classify("chart", "cat"), Some(Match::Loose));
        assert_eq!(classify("ls", "cat"), None);
        assert!(Match::Exact > Match::Prefix);
        assert!(Match::Prefix > Match::Word);
        assert!(Match::Word > Match::Substring);
        assert!(Match::Substring > Match::Loose);
    }

    /// A plain query stops at `Substring`: the confident classifiers refuse the
    /// scattered hit the full one still reports for the fuzzy consumers.
    #[test]
    fn a_confident_classifier_refuses_a_scattered_hit() {
        assert_eq!(classify("chart", "cat"), Some(Match::Loose));
        assert_eq!(classify_confident("chart", "cat"), None);
        assert_eq!(classify_bytes_confident(b"chart", b"cat"), None);
        assert_eq!(classify_ci_confident("chart", "cat"), None);
        assert_eq!(
            classify_confident("libreoffice", "office"),
            Some(Match::Substring)
        );
        assert_eq!(
            classify_ci_confident("Ünïcode", "nïc"),
            Some(Match::Substring)
        );
        assert_eq!(classify_confident("x", ""), None);
        assert_eq!(classify_bytes_confident(b"x", b""), None);
    }

    #[test]
    fn push_lowered_matches_to_lowercase() {
        for s in ["", "ABC", "WayRun.zip", "Ünïcode", "报告.TXT", "İstanbul"] {
            let mut out = String::from("keep/");
            push_lowered(&mut out, s);
            assert_eq!(out, format!("keep/{}", s.to_lowercase()), "{s}");
        }
    }
}
