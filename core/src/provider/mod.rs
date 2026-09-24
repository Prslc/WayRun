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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::plugin::Plugin;
use crate::wire::{Action, ActionItem, PanelAction, ResultItem};
use rust_i18n::t;

/// Rows a provider shows: the one cap every scored provider passes to
/// [`rank_results`], so the index and the fallback walk cannot diverge.
pub const SHOW_CAP: usize = 50;

/// How long a provider's burst cache keeps a fetch: a typing burst shares one
/// shell-out, and a row a beat stale is harmless.
pub const BURST_TTL: Duration = Duration::from_millis(500);

/// A provider's short-lived fetch cache, so a burst of keystrokes does not
/// re-run the same shell-out on every one; failures are never cached.
pub struct FreshCache<T> {
    rows: Mutex<Option<(Instant, Arc<Vec<T>>)>>,
}

impl<T> FreshCache<T> {
    pub const fn new() -> Self {
        Self {
            rows: Mutex::new(None),
        }
    }

    /// The cached rows, refetched when stale or empty.
    pub fn get(&self, fetch: impl FnOnce() -> Option<Vec<T>>) -> Option<Arc<Vec<T>>> {
        let mut cache = self.rows.lock().unwrap_or_else(|err| err.into_inner());
        if let Some((at, rows)) = cache.as_ref()
            && at.elapsed() < BURST_TTL
        {
            return Some(Arc::clone(rows));
        }
        let rows = Arc::new(fetch()?);
        *cache = Some((Instant::now(), Arc::clone(&rows)));
        Some(rows)
    }
}

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
    use crate::plugin::{
        Match, classify, classify_bytes_confident, classify_ci, classify_ci_confident,
        classify_confident,
    };

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
