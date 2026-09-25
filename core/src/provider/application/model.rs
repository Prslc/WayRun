use crate::system::icon::resolve;
use crate::wire::{Action, ResultItem};

/// One constant match surface — a name, comment, keyword or generic — with the
/// forms a query needs precomputed, so scoring never re-lowers or re-tokenizes.
pub(super) struct Field {
    pub(super) lower: String,
    pub(super) chars: Vec<char>,
    /// `(start, end)` char spans of the tokens in `lower`, split on ` `, `-` and `_`.
    pub(super) word_spans: Vec<(usize, usize)>,
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
    pub(super) lower: String,
    pub(super) chars: Vec<char>,
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
    pub(super) generic: Option<Field>,
    pub(super) keywords: Vec<Field>,
    pub(super) actions: Vec<DesktopAction>,
}

/// One `[Desktop Action <id>]` group, surfaced as its own row (DMS-style).
pub(super) struct DesktopAction {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) name_lower: String,
}

/// One installed application, precomputed at first search and reused for the
/// process lifetime; the `Field`s carry every match form a query needs.
pub(super) struct CachedApp {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) title_field: Field,
    pub(super) comment: Option<String>,
    pub(super) comment_field: Option<Field>,
    pub(super) icon_spec: Option<String>,
    /// Basename of the entry's `Exec=`, so the runner can match a PATH hit to a
    /// desktop app without reading every `.desktop` file again.
    pub(super) exec: Option<String>,
    /// `Terminal=` of the entry, read here so the runner never reopens it.
    pub(super) terminal: bool,
    pub(super) meta: Option<DesktopMeta>,
    /// The id without its `.desktop` suffix, the last-resort match surface.
    pub(super) id_field: Field,
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
pub(super) enum Hit {
    App(&'static CachedApp),
    Action(&'static CachedApp, &'static DesktopAction),
}

impl Hit {
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
                ephemeral: false,
                actions: Vec::new(),
                badge: None,
            },
            Hit::Action(app, action) => ResultItem {
                title: action.name.clone(),
                summary: Some(app.title.clone()),
                on_click: Some(Action::DesktopAction {
                    desktop_id: app.id.clone(),
                    action_id: action.id.clone(),
                }),
                icon: app.icon_path(),
                ephemeral: false,
                actions: Vec::new(),
                badge: None,
            },
        }
    }
}
