use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::plugin::{Meta, Plugin};
use crate::provider::rank_results;
use crate::system::compositor::{self, Compositor, Window};
use crate::system::desktop_action;
use crate::system::executor::shell_join;
use crate::wire::{Action, ResultItem};
use rust_i18n::t;

pub struct WindowPlugin {
    meta: Meta,
}

impl WindowPlugin {
    pub fn new() -> Self {
        Self {
            meta: Meta {
                id: "window",
                name: t!("plugin.window.name"),
                icon: "builtin:window",
                ready: t!("plugin.window.ready"),
            },
        }
    }
}

impl Plugin for WindowPlugin {
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

/// A typing burst shares one `niri msg` round trip; a stale row's focus is a
/// harmless no-op, so a short lag is fine.
const CACHE_TTL: Duration = Duration::from_millis(500);

/// A window with the match forms a search needs, so a keystroke never re-lowers
/// a title or re-encodes a nucleo key.
struct CachedWindow {
    window: Window,
    title_lower: String,
    title_key: nucleo::Utf32String,
    app_lower: Option<String>,
    app_key: Option<nucleo::Utf32String>,
}

impl CachedWindow {
    fn new(window: Window) -> Self {
        let title_lower = window.title.to_lowercase();
        let app_lower = window.app_id.as_deref().map(str::to_lowercase);
        Self {
            title_key: nucleo::Utf32String::from(title_lower.as_str()),
            app_key: app_lower.as_deref().map(nucleo::Utf32String::from),
            window,
            title_lower,
            app_lower,
        }
    }
}

/// The cached window list and when it was fetched.
type WindowCache = Mutex<Option<(Instant, Arc<Vec<CachedWindow>>)>>;

static WINDOWS: LazyLock<WindowCache> = LazyLock::new(|| Mutex::new(None));

/// The compositor's windows from a short-lived cache, so a burst of keystrokes
/// does not shell out to the compositor on every one.
fn cached_windows(compositor: &dyn Compositor) -> Option<Arc<Vec<CachedWindow>>> {
    let mut cache = WINDOWS.lock().unwrap_or_else(|err| err.into_inner());
    if let Some((at, windows)) = cache.as_ref()
        && at.elapsed() < CACHE_TTL
    {
        return Some(Arc::clone(windows));
    }
    let windows = Arc::new(
        compositor
            .windows()
            .ok()?
            .into_iter()
            .map(CachedWindow::new)
            .collect(),
    );
    *cache = Some((Instant::now(), Arc::clone(&windows)));
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

    for cached in windows.iter() {
        let score = score_window(cached, &query, &pattern, &mut matcher);
        if score > 0 {
            results.push((score, row(compositor, &cached.window)));
        }
    }

    rank_results(results, false, 50)
}

/// Title and app_id each get a name tier; the strongest tier dominates and the
/// fuzzy score breaks ties and matches a mere subsequence.
fn score_window(
    cached: &CachedWindow,
    query_lower: &str,
    pattern: &nucleo::Utf32String,
    matcher: &mut nucleo::Matcher,
) -> u32 {
    let title_fuzzy = matcher
        .fuzzy_match(cached.title_key.slice(..), pattern.slice(..))
        .unwrap_or(0) as u32;
    let app_fuzzy = cached
        .app_key
        .as_ref()
        .and_then(|key| matcher.fuzzy_match(key.slice(..), pattern.slice(..)))
        .unwrap_or(0) as u32;

    let title_tier = crate::provider::name_tier(&cached.title_lower, query_lower);
    let app_tier = cached
        .app_lower
        .as_deref()
        .map_or(0, |app| crate::provider::name_tier(app, query_lower));
    title_tier.max(app_tier) * 1000 + title_fuzzy.max(app_fuzzy)
}

fn row(compositor: &dyn Compositor, window: &Window) -> ResultItem {
    let summary = window.app_id.as_ref().map(|app| match &window.workspace {
        Some(ws) => format!("{app} · workspace {ws}"),
        None => app.clone(),
    });
    ResultItem {
        title: window.title.clone(),
        summary,
        on_click: Some(Action::Run {
            cmd: shell_join(&compositor.focus_argv(&window.id)),
        }),
        icon: window.app_id.as_deref().and_then(app_icon),
        ephemeral: true,
        actions: Vec::new(),
        badge: None,
    }
}

/// The app's icon, resolved once per app id: the `.desktop` read must not
/// repeat for every keystroke that still lists the window.
fn app_icon(app_id: &str) -> Option<String> {
    static CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> =
        LazyLock::new(|| Mutex::new(HashMap::default()));
    if let Ok(cache) = CACHE.lock()
        && let Some(icon) = cache.get(app_id)
    {
        return icon.clone();
    }
    let icon = desktop_action::icon_for_app_id(app_id);
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(app_id.to_string(), icon.clone());
    }
    icon
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
        assert!(Arc::ptr_eq(&first, &second));
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
            score_window(&CachedWindow::new(exact), query, &pattern, &mut matcher)
                > score_window(&CachedWindow::new(substring), query, &pattern, &mut matcher)
        );
    }
}
