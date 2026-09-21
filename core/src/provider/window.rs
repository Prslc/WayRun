use std::future::Future;
use std::pin::Pin;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::plugin::{Meta, Plugin};
use crate::provider::rank_results;
use crate::system::compositor::{self, Compositor, Window};
use crate::system::desktop_action;
use crate::system::executor::shell_join;
use crate::wire::{Action, ResultItem};

pub struct WindowPlugin;

impl Plugin for WindowPlugin {
    fn meta(&self) -> &Meta {
        &Meta {
            id: "window",
            name: "Window",
            icon: "builtin:window",
            ready: "Switch open windows",
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

/// A typing burst shares one `niri msg` round trip; a stale row's focus is a
/// harmless no-op, so a short lag is fine.
const CACHE_TTL: Duration = Duration::from_millis(500);

/// The cached window list and when it was fetched.
type WindowCache = Mutex<Option<(Instant, Vec<Window>)>>;

static WINDOWS: LazyLock<WindowCache> = LazyLock::new(|| Mutex::new(None));

/// The compositor's windows from a short-lived cache, so a burst of keystrokes
/// does not shell out to the compositor on every one.
fn cached_windows(compositor: &dyn Compositor) -> Option<Vec<Window>> {
    let mut cache = WINDOWS.lock().unwrap_or_else(|err| err.into_inner());
    if let Some((at, windows)) = cache.as_ref()
        && at.elapsed() < CACHE_TTL
    {
        return Some(windows.clone());
    }
    let windows = compositor.windows().ok()?;
    *cache = Some((Instant::now(), windows.clone()));
    Some(windows)
}

/// Fuzzy-match the query against every open window's title/app_id and emit a
/// `run:` row that focuses the winner; empty without a compositor backend.
fn do_search(query: &str) -> Vec<ResultItem> {
    if query.is_empty() {
        return Vec::new();
    }

    let Some(compositor) = compositor::detect() else {
        return Vec::new();
    };
    let Some(windows) = cached_windows(compositor) else {
        return Vec::new();
    };

    let query = query.to_lowercase();
    let mut matcher = nucleo::Matcher::new(nucleo::Config::DEFAULT);
    let pattern = nucleo::Utf32String::from(query.as_str());
    let mut results: Vec<(u32, ResultItem)> = Vec::new();

    for window in windows {
        let score = score_window(&window, &query, &pattern, &mut matcher);
        if score > 0 {
            results.push((score, row(compositor, window)));
        }
    }

    rank_results(results, false, 50)
}

/// Title and app_id each get a name tier; the strongest tier dominates and the
/// fuzzy score breaks ties and matches a mere subsequence.
fn score_window(
    window: &Window,
    query_lower: &str,
    pattern: &nucleo::Utf32String,
    matcher: &mut nucleo::Matcher,
) -> u32 {
    let title = window.title.to_lowercase();
    let title_fuzzy = matcher
        .fuzzy_match(
            nucleo::Utf32String::from(title.as_str()).slice(..),
            pattern.slice(..),
        )
        .unwrap_or(0) as u32;
    let app = window.app_id.as_deref().map(str::to_lowercase);
    let app_fuzzy = app
        .as_deref()
        .and_then(|app| {
            matcher.fuzzy_match(nucleo::Utf32String::from(app).slice(..), pattern.slice(..))
        })
        .unwrap_or(0) as u32;

    let title_tier = crate::provider::name_tier(&title, query_lower);
    let app_tier = app
        .as_deref()
        .map_or(0, |app| crate::provider::name_tier(app, query_lower));
    title_tier.max(app_tier) * 1000 + title_fuzzy.max(app_fuzzy)
}

fn row(compositor: &dyn Compositor, window: Window) -> ResultItem {
    let summary = window.app_id.as_ref().map(|app| match &window.workspace {
        Some(ws) => format!("{app} · workspace {ws}"),
        None => app.clone(),
    });
    ResultItem {
        title: window.title,
        summary,
        on_click: Some(Action::Run {
            cmd: shell_join(&compositor.focus_argv(&window.id)),
        }),
        icon: window
            .app_id
            .as_deref()
            .and_then(desktop_action::icon_for_app_id),
        ephemeral: true,
        actions: Vec::new(),
        badge: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting {
        calls: AtomicUsize,
    }

    impl Compositor for Counting {
        fn available(&self) -> bool {
            true
        }
        fn windows(&self) -> Result<Vec<Window>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![Window {
                id: "1".into(),
                title: "kitty".into(),
                app_id: Some("kitty".into()),
                workspace: None,
            }])
        }
        fn focus_argv(&self, _id: &str) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn a_typing_burst_shells_out_once() {
        *WINDOWS.lock().unwrap() = None;
        let fake = Counting {
            calls: AtomicUsize::new(0),
        };
        let first = cached_windows(&fake).unwrap();
        let second = cached_windows(&fake).unwrap();
        assert_eq!(first, second);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(do_search("").is_empty());
    }

    #[test]
    fn an_exact_app_id_outranks_a_title_substring() {
        let mut matcher = nucleo::Matcher::new(nucleo::Config::DEFAULT);
        let query = "firefox";
        let pattern = nucleo::Utf32String::from(query);
        let exact = Window {
            id: "1".into(),
            title: "Mozilla Firefox".into(),
            app_id: Some("firefox".into()),
            workspace: None,
        };
        let substring = Window {
            id: "2".into(),
            title: "firefox docs".into(),
            app_id: Some("kitty".into()),
            workspace: None,
        };
        assert!(
            score_window(&exact, query, &pattern, &mut matcher)
                > score_window(&substring, query, &pattern, &mut matcher)
        );
    }
}
