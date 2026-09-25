mod desktop;
mod model;
mod score;

use std::cmp::Reverse;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;

use anyhow::Result;
use gio::prelude::{AppInfoExt, IconExt};

use crate::plugin::{Meta, Plugin, Rank, Ranked};
use crate::system::desktop_action;
use crate::wire::{Action, ActionItem, PanelAction, ResultItem};
use rust_i18n::t;

use self::desktop::meta;
use self::model::{CachedApp, Field, Hit, Query};
use self::score::{action_score, score_app};

static APPS: LazyLock<Vec<CachedApp>> = LazyLock::new(|| {
    let locales = desktop_action::locales();
    gio::AppInfo::all()
        .into_iter()
        .filter_map(|app| {
            if !app.should_show() {
                return None;
            }
            let id = app.id().map(|s| s.to_string())?;
            let title = app.name().to_string();
            let comment = app.description().map(|s| s.to_string());
            let icon_spec = app
                .icon()
                .and_then(|i| i.to_string())
                .map(|s| s.to_string());
            // One read of the entry, interpreted once: gio's `AppInfo` exposes
            // neither `Exec=`, `GenericName=`, `Keywords=` nor `Actions=`.
            let (exec, terminal, meta) = match desktop_action::details(&id, &locales) {
                Some(details) => {
                    let meta = meta(&details);
                    (details.exec, details.terminal, Some(meta))
                }
                None => (None, false, None),
            };
            Some(CachedApp {
                title_field: Field::new(&title),
                comment_field: comment.as_deref().map(Field::new),
                id_field: Field::new(id.to_lowercase().trim_end_matches(".desktop")),
                meta,
                exec,
                terminal,
                title,
                comment,
                id,
                icon_spec,
            })
        })
        .collect()
});

/// Whether the app behind `desktop_id` asks for a terminal, off the entry the
/// app list already parsed.
pub(super) fn needs_terminal(desktop_id: &str) -> bool {
    APPS.iter()
        .find(|app| app.id == desktop_id)
        .is_some_and(|app| app.terminal)
}

/// The desktop id of the app whose `Exec=` names `executable`: a PATH hit that
/// is an installed app launches through `gio`, so its `Terminal=` decides.
pub fn desktop_id_for_exec(executable: &str) -> Option<&'static str> {
    APPS.iter()
        .find(|app| app.exec.as_deref() == Some(executable))
        .map(|app| app.id.as_str())
}

pub struct AppSearch {
    meta: Meta,
}

impl AppSearch {
    pub fn new() -> Self {
        Self {
            meta: Meta {
                id: "app-search".into(),
                name: t!("plugin.app.name"),
                icon: "builtin:app".into(),
                ready: t!("plugin.app.ready"),
            },
        }
    }
}

impl Plugin for AppSearch {
    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn search_ranked(
        &self,
        _query: &str,
        full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Ranked>> + Send + '_>> {
        // The keyword is empty, so the whole input is the query: with no plugin
        // owning the first word, `foo bar` must match on `foo bar`, not `bar`.
        let input = full.to_string();
        Box::pin(async move {
            Ok(tokio::task::spawn_blocking(move || do_search(&input))
                .await
                .unwrap_or_default())
        })
    }

    fn search(
        &self,
        _query: &str,
        full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        // the same whole-input rule as `search_ranked`
        let input = full.to_string();
        Box::pin(async move {
            Ok(tokio::task::spawn_blocking(move || do_search(&input))
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(_, item)| item)
                .collect())
        })
    }

    /// An application row's declared `[Desktop Action …]` groups, read from the
    /// same file the row's `launch` command uses.
    fn actions(&self, item: &ResultItem) -> Vec<ActionItem> {
        let Some(Action::Launch { desktop_id }) = item.on_click.as_ref() else {
            return Vec::new();
        };
        let Some(app) = APPS.iter().find(|app| app.id == *desktop_id) else {
            return Vec::new();
        };
        let Some(meta) = app.meta.as_ref() else {
            return Vec::new();
        };

        meta.actions
            .iter()
            .map(|action| ActionItem {
                title: action.name.clone(),
                action: PanelAction::Execute {
                    command: Action::DesktopAction {
                        desktop_id: desktop_id.clone(),
                        action_id: action.id.clone(),
                    },
                },
                icon: app.icon_path(),
                // scope the id to the app, so a remembered default stays with it
                id: Some(format!("desktop_action:{}:{}", desktop_id, action.id)),
                plugin: None,
                default: false,
            })
            .collect()
    }
}

/// Score every cached app against the query; the query text is lowercased
/// and tokenized once, not per app.
fn do_search(query: &str) -> Vec<(Rank, ResultItem)> {
    let query = Query::new(query.trim());
    if query.lower.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(u32, Hit)> = Vec::new();
    for app in APPS.iter() {
        let app_score = score_app(
            &app.title_field,
            app.comment_field.as_ref(),
            app.meta.as_ref(),
            &app.id_field,
            &query,
        );
        if let Some((_, weight)) = app_score {
            scored.push((weight, Hit::App(app)));
        }

        for action in app.meta.iter().flat_map(|m| &m.actions) {
            if let Some((_, weight)) = action_score(&action.name_lower, &query.lower) {
                scored.push((weight, Hit::Action(app, action)));
            }
        }
    }

    // The cap is below the merge, so it keeps what the merge would rank first:
    // the same relevance the merge orders by.
    scored.sort_by_key(|a| Reverse(a.0));
    scored.dedup_by(|a, b| a.1.title() == b.1.title());
    scored.truncate(crate::provider::SHOW_CAP);
    scored
        .into_iter()
        .map(|(weight, hit)| (Rank::Scored(weight), hit.row()))
        .collect()
}
