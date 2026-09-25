use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::Result;

use crate::plugin::{Match, classify};
use crate::plugin::{Meta, Plugin};
use crate::provider::{FreshCache, rank_results};
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
                id: "window".into(),
                name: t!("plugin.window.name"),
                icon: "builtin:window".into(),
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
            if input.is_empty() {
                return Ok(Vec::new());
            }
            let Some(windows) = cached_windows().await else {
                return Ok(Vec::new());
            };
            Ok(
                tokio::task::spawn_blocking(move || do_search(&input, &windows))
                    .await
                    .unwrap_or_default(),
            )
        })
    }
}

/// A window with its lowercased title and app id, so a keystroke never re-lowers a title.
struct CachedWindow {
    window: Window,
    title_lower: String,
    app_lower: Option<String>,
}

impl CachedWindow {
    fn new(window: Window) -> Self {
        let title_lower = window.title.to_lowercase();
        let app_lower = window.app_id.as_deref().map(str::to_lowercase);
        Self {
            window,
            title_lower,
            app_lower,
        }
    }
}

/// The compositor's window list, shared across a typing burst, so one `niri
/// msg` round trip serves the whole burst.
static WINDOWS: LazyLock<FreshCache<CachedWindow>> = LazyLock::new(FreshCache::new);

async fn cached_windows() -> Option<Arc<Vec<CachedWindow>>> {
    WINDOWS
        .get(|| {
            let compositor = compositor::detect()?;
            let windows = compositor.windows().ok()?;
            Some(windows.into_iter().map(CachedWindow::new).collect())
        })
        .await
}

/// Fuzzy-match the query against every open window's title/app_id and emit a
/// `run:` row that focuses the winner; empty without a compositor backend.
fn do_search(query: &str, windows: &[CachedWindow]) -> Vec<ResultItem> {
    let Some(compositor) = compositor::detect() else {
        return Vec::new();
    };

    let query = query.to_lowercase();
    let mut results: Vec<(u32, ResultItem)> = Vec::new();

    for cached in windows {
        let score = score_window(cached, &query);
        if score > 0 {
            results.push((score, row(compositor, &cached.window)));
        }
    }

    rank_results(results, false, 50)
}

/// The strongest kind either surface reaches, so a window whose app id is exact
/// outranks one whose title merely contains the query.
fn score_window(cached: &CachedWindow, query_lower: &str) -> u32 {
    let title = classify(&cached.title_lower, query_lower);
    let app = cached
        .app_lower
        .as_deref()
        .and_then(|app| classify(app, query_lower));
    title.max(app).map_or(0, Match::weight)
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

    #[tokio::test]
    async fn a_typing_burst_shells_out_once() {
        let fake = Arc::new(Counting {
            calls: AtomicUsize::new(0),
        });
        let cache: FreshCache<Window> = FreshCache::new();
        let fetch = {
            let fake = Arc::clone(&fake);
            move || fake.windows().ok()
        };
        let first = cache.get(fetch).await.unwrap();
        let second = cache
            .get(|| panic!("a burst shares one shell out"))
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn empty_query_matches_nothing() {
        let plugin = WindowPlugin::new();
        assert!(plugin.search("", "").await.unwrap().is_empty());
    }

    #[test]
    fn an_exact_app_id_outranks_a_title_substring() {
        let query = "firefox";
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
            score_window(&CachedWindow::new(exact), query)
                > score_window(&CachedWindow::new(substring), query)
        );
    }
}
