pub mod application;
pub mod calculator;
pub mod clipboard;
pub mod external;
pub mod file;
pub mod firefox;
pub mod runner;
pub mod system;
pub mod web;
pub mod window;

use std::collections::HashMap;

use crate::plugin::Plugin;
use crate::wire::{Action, ActionItem, PanelAction, ResultItem};
use rust_i18n::t;

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

const TIER_EXACT: u32 = 1000;
const TIER_PREFIX: u32 = 700;
const TIER_CONTAINS: u32 = 400;

/// Exact, then prefix, then substring, so a short exact name outranks a longer
/// name that merely contains the query. Inputs are lowercased.
pub fn name_tier(name: &str, query: &str) -> u32 {
    if name == query {
        TIER_EXACT
    } else if name.starts_with(query) {
        TIER_PREFIX
    } else if name.contains(query) {
        TIER_CONTAINS
    } else {
        0
    }
}

/// [`name_tier`] without lowercasing: an ASCII name is compared byte by byte,
/// and only a non-ASCII one pays for `to_lowercase`.
pub fn name_tier_ci(name: &str, query: &str) -> u32 {
    if !name.is_ascii() {
        return name_tier(&name.to_lowercase(), query);
    }
    name_tier_bytes(name.as_bytes(), query.as_bytes())
}

fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle))
}

/// [`name_tier_ci`] over raw name bytes, for a name the caller checked is ASCII:
/// a byte compare is the same test without building a lowercased string.
pub fn name_tier_bytes(name: &[u8], query: &[u8]) -> u32 {
    if name.eq_ignore_ascii_case(query) {
        TIER_EXACT
    } else if name.len() >= query.len() && name[..query.len()].eq_ignore_ascii_case(query) {
        TIER_PREFIX
    } else if contains_ci(name, query) {
        TIER_CONTAINS
    } else {
        0
    }
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

/// A URL row's extra command: copy the link instead of opening it. Every
/// provider whose rows are URLs shares this one action.
pub fn copy_url_action(item: &ResultItem) -> Vec<ActionItem> {
    let Some(Action::Open { uri }) = item.on_click.as_ref() else {
        return Vec::new();
    };
    if !uri.starts_with("http") {
        return Vec::new();
    }
    vec![ActionItem {
        title: t!("action.copy_url"),
        action: PanelAction::Execute {
            command: Action::Copy { text: uri.clone() },
        },
        icon: Some("builtin:copy".to_string()),
        id: Some("copy_url".to_string()),
        plugin: None,
        default: false,
    }]
}

pub fn plugin_map() -> HashMap<&'static str, Box<dyn Plugin>> {
    let mut m: HashMap<&'static str, Box<dyn Plugin>> = HashMap::default();
    m.insert("calculator", Box::new(calculator::Calculator::new()));
    m.insert("app-search", Box::new(application::AppSearch::new()));
    m.insert(
        "firefox-bookmarks",
        Box::new(firefox::FirefoxBookmarks::new()),
    );
    m.insert("firefox-history", Box::new(firefox::FirefoxHistory::new()));
    m.insert("file-search", Box::new(file::FileSearch::new()));
    m.insert("path-search", Box::new(file::PathSearch::new()));
    m.insert("clipboard", Box::new(clipboard::Clipboard::new()));
    m.insert("system-commands", Box::new(system::SystemCommands::new()));
    m.insert("runner", Box::new(runner::Runner::new()));
    m.insert("window", Box::new(window::WindowPlugin::new()));
    m.insert("web-search", Box::new(web::WebSearch::new()));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(on_click: Option<Action>) -> ResultItem {
        ResultItem {
            title: "x".to_string(),
            summary: None,
            on_click,
            icon: None,
            ephemeral: false,
            actions: Vec::new(),
            badge: None,
        }
    }

    #[test]
    fn only_a_url_row_offers_a_copy_link() {
        let actions = copy_url_action(&row(Some(Action::Open {
            uri: "https://example.com".to_string(),
        })));
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].title, t!("action.copy_url"));
        assert_eq!(
            actions[0].action,
            PanelAction::Execute {
                command: Action::Copy {
                    text: "https://example.com".to_string()
                }
            }
        );

        assert!(
            copy_url_action(&row(Some(Action::Run {
                cmd: "ls".to_string()
            })))
            .is_empty()
        );
        assert!(copy_url_action(&row(None)).is_empty());
    }

    #[test]
    fn a_case_insensitive_tier_matches_the_lowered_one() {
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
                name_tier_ci(name, query),
                name_tier(&name.to_lowercase(), query),
                "{name} / {query}"
            );
        }
    }

    #[test]
    fn the_byte_tier_matches_the_text_tier_for_ascii_names() {
        for (name, query) in [
            ("WayRun", "wayrun"),
            ("NOTES.txt", "notes.txt"),
            ("report", "report"),
            ("x", "wayrun"),
            ("abc", "报告"),
            ("", ""),
        ] {
            assert_eq!(
                name_tier_bytes(name.as_bytes(), query.as_bytes()),
                name_tier(&name.to_lowercase(), query),
                "{name} / {query}"
            );
        }
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
