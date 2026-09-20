use std::future::Future;
use std::pin::Pin;

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
            icon: "window-duplicate",
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

/// Fuzzy-match the query against every open window's title/app_id and emit a
/// `run:` row that focuses the winner; empty without a compositor backend.
fn do_search(query: &str) -> Vec<ResultItem> {
    if query.is_empty() {
        return Vec::new();
    }

    let Some(compositor) = compositor::detect() else {
        return Vec::new();
    };
    let Ok(windows) = compositor.windows() else {
        return Vec::new();
    };

    let mut matcher = nucleo::Matcher::new(nucleo::Config::DEFAULT);
    let pattern = nucleo::Utf32String::from(query.to_lowercase());
    let mut results: Vec<(u16, ResultItem)> = Vec::new();

    for window in windows {
        let score = score_window(&window, &pattern, &mut matcher);
        if score > 0 {
            results.push((score, row(compositor, window)));
        }
    }

    rank_results(results, false, 50)
}

fn score_window(
    window: &Window,
    pattern: &nucleo::Utf32String,
    matcher: &mut nucleo::Matcher,
) -> u16 {
    let title = nucleo::Utf32String::from(window.title.to_lowercase());
    let title_score = matcher
        .fuzzy_match(title.slice(..), pattern.slice(..))
        .unwrap_or(0);
    let app_score = window
        .app_id
        .as_ref()
        .and_then(|app| {
            let app = nucleo::Utf32String::from(app.to_lowercase());
            matcher.fuzzy_match(app.slice(..), pattern.slice(..))
        })
        .unwrap_or(0);
    title_score.max(app_score)
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

    #[test]
    fn empty_query_matches_nothing() {
        assert!(do_search("").is_empty());
    }
}
