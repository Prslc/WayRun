mod desktop;
mod score;

use std::cmp::Reverse;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, OnceLock};

use anyhow::Result;
use gio::prelude::{AppInfoExt, IconExt};

use crate::live::{Live, Source};
use crate::plugin::{Meta, Plugin, Rank, Ranked};
use crate::system::desktop_action;
use crate::system::icon::resolve;
use crate::system::xdg;
use crate::wire::{Action, ActionItem, ResultItem};
use rust_i18n::t;

use self::desktop::meta;
use self::score::{action_score, score_app};

/// One constant match surface — a name, comment, keyword or generic — with the
/// forms a query needs precomputed, so scoring never re-lowers or re-tokenizes.
pub(super) struct Field {
    pub lower: String,
    pub chars: Vec<char>,
    /// `(start, end)` char spans of the tokens in `lower`, split on ` `, `-` and `_`.
    pub word_spans: Vec<(usize, usize)>,
}

impl Field {
    pub(super) fn new(text: &str) -> Self {
        let lower = text.to_lowercase();
        let chars: Vec<char> = lower.chars().collect();
        let mut word_spans: Vec<(usize, usize)> = Vec::new();
        let mut start = None;
        for (index, ch) in chars.iter().enumerate() {
            if matches!(ch, ' ' | '-' | '_') {
                if let Some(from) = start.take() {
                    word_spans.push((from, index));
                }
            } else if start.is_none() {
                start = Some(index);
            }
        }
        if let Some(from) = start {
            word_spans.push((from, chars.len()));
        }
        Self {
            lower,
            chars,
            word_spans,
        }
    }

    pub(super) fn word(&self, span: (usize, usize)) -> &[char] {
        &self.chars[span.0..span.1]
    }
}

/// The lowercased query with its tokens and char form, built once per search.
pub(super) struct Query {
    pub lower: String,
    pub chars: Vec<char>,
}

impl Query {
    pub(super) fn new(text: &str) -> Self {
        let lower = text.to_lowercase();
        let chars = lower.chars().collect();
        Self { lower, chars }
    }
}

/// `GenericName` + `Keywords` from the app's `.desktop` file, localised through
/// its own `Key[locale]=` entries. gio's `AppInfo` does not expose them.
pub(super) struct DesktopMeta {
    pub generic: Option<Field>,
    pub keywords: Vec<Field>,
    pub actions: Vec<DesktopAction>,
}

/// One `[Desktop Action <id>]` group, surfaced as its own row (DMS-style).
pub(super) struct DesktopAction {
    pub id: String,
    pub name: String,
    pub name_lower: String,
}

/// One installed application, rebuilt when the desktop roots change; the
/// `Field`s carry every match form a query needs.
pub(super) struct CachedApp {
    pub id: String,
    pub title: String,
    pub title_field: Field,
    pub comment: Option<String>,
    pub comment_field: Option<Field>,
    pub icon_spec: Option<String>,
    /// Basename of the entry's `Exec=`, so the runner can match a PATH hit to a
    /// desktop app without reading every `.desktop` file again.
    pub exec: Option<String>,
    /// `Terminal=` of the entry, read here so the runner never reopens it.
    pub terminal: bool,
    pub meta: Option<DesktopMeta>,
    /// The id without its `.desktop` suffix, the last-resort match surface.
    pub id_field: Field,
}

impl CachedApp {
    /// Resolve the gio icon spec (`!!/path` for file icons, otherwise a theme
    /// name) to the absolute path the UI renders.
    pub(super) fn icon_path(&self) -> Option<String> {
        let spec = self.icon_spec.as_deref()?;
        if let Some(path) = spec.strip_prefix("!!") {
            (!path.is_empty()).then(|| path.to_string())
        } else {
            resolve(spec)
        }
    }
}

/// One scored candidate before its row exists: only the survivors of the cap
/// pay for the icon lookup a row needs.
pub(super) enum Hit<'a> {
    App(&'a CachedApp),
    Action(&'a CachedApp, &'a DesktopAction),
}

impl Hit<'_> {
    pub(super) fn title(&self) -> &str {
        match self {
            Hit::App(app) => &app.title,
            Hit::Action(_, action) => &action.name,
        }
    }

    pub(super) fn row(&self) -> ResultItem {
        match self {
            Hit::App(app) => ResultItem {
                title: app.title.clone(),
                summary: app.comment.clone(),
                on_click: Some(Action::Launch {
                    desktop_id: app.id.clone(),
                }),
                icon: app.icon_path(),
                icon_shape: None,
                preview: None,
                actions: Vec::new(),
            },
            Hit::Action(app, action) => ResultItem {
                title: action.name.clone(),
                summary: Some(app.title.clone()),
                on_click: Some(Action::DesktopAction {
                    desktop_id: app.id.clone(),
                    action_id: action.id.clone(),
                }),
                icon: app.icon_path(),
                icon_shape: None,
                preview: None,
                actions: Vec::new(),
            },
        }
    }
}

static SOURCE: LazyLock<Arc<Source>> = LazyLock::new(|| Arc::new(Source::new(xdg::desktop_dirs)));

/// The watched desktop roots, shared by caches keyed to a desktop entry.
pub(super) fn app_source() -> Arc<Source> {
    Arc::clone(&SOURCE)
}

static APPS: OnceLock<Live<Vec<CachedApp>>> = OnceLock::new();

fn app_cache() -> &'static Live<Vec<CachedApp>> {
    APPS.get_or_init(|| Live::shared(app_source()))
}

/// Build the list for a caller on the blocking pool that needs it before any
/// app search has run.
pub(super) fn ensure_apps() {
    let _ = app_cache().get(build_apps);
}

fn build_apps() -> Vec<CachedApp> {
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
}

/// Whether the app behind `desktop_id` asks for a terminal, off the entry the
/// app list already parsed.
pub(super) fn needs_terminal(desktop_id: &str) -> bool {
    APPS.get()
        .and_then(|cache| cache.snapshot())
        .is_some_and(|apps| apps.iter().any(|app| app.id == desktop_id && app.terminal))
}

/// The desktop id of the app whose `Exec=` names `executable`: a PATH hit that
/// is an installed app launches through `gio`, so its `Terminal=` decides.
pub fn desktop_id_for_exec(executable: &str) -> Option<String> {
    let apps = APPS.get()?.snapshot()?;
    apps.iter()
        .find(|app| app.exec.as_deref() == Some(executable))
        .map(|app| app.id.clone())
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
        let Some(apps) = APPS.get().and_then(|cache| cache.snapshot()) else {
            return Vec::new();
        };
        let Some(app) = apps.iter().find(|app| app.id == *desktop_id) else {
            return Vec::new();
        };
        let Some(meta) = app.meta.as_ref() else {
            return Vec::new();
        };

        meta.actions
            .iter()
            .map(|action| ActionItem {
                title: action.name.clone(),
                action: Action::DesktopAction {
                    desktop_id: desktop_id.clone(),
                    action_id: action.id.clone(),
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

    let cache = app_cache();
    let apps = cache.get(build_apps);
    let mut scored: Vec<(u32, Hit<'_>)> = Vec::new();
    for app in apps.iter() {
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
