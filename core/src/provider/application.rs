use std::cmp::Reverse;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::LazyLock;

use anyhow::Result;
use freedesktop_desktop_entry::DesktopEntry;
use gio::prelude::{AppInfoExt, IconExt};

use crate::plugin::{Match, Meta, Plugin, Rank, Ranked};
use crate::system::icon::resolve;
use crate::wire::{Action, ActionItem, PanelAction, ResultItem};
use rust_i18n::t;

// A surface's share of the kind's weight, in tenths: the merge orders by the
// product, so a title hit leads an action label, and both lead the metadata.
const TITLE: u32 = 10;
const ACTION: u32 = 8;
const SUMMARY: u32 = 5;
const KEYWORD: u32 = 3;
const GENERIC: u32 = 2;
const ID: u32 = 1;

/// The typo tier's weight, under a real title kind but over the metadata tails:
/// a near spelling is strong evidence of intent, weak evidence of a match.
const TYPO_WEIGHT: u32 = 400;
/// The typo tier is a title surface, near or not, only from three characters.
const TYPO_SIMILARITY: f64 = 0.7;

/// One constant match surface — a name, comment, keyword or generic — with the
/// forms a query needs precomputed, so scoring never re-lowers or re-tokenizes.
struct Field {
    lower: String,
    chars: Vec<char>,
    /// `(start, end)` char spans of the tokens in `lower`, split on ` `, `-` and `_`.
    word_spans: Vec<(usize, usize)>,
}

impl Field {
    fn new(text: &str) -> Self {
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

    fn word(&self, span: (usize, usize)) -> &[char] {
        &self.chars[span.0..span.1]
    }
}

/// The lowercased query with its tokens and char form, built once per search.
struct Query {
    lower: String,
    chars: Vec<char>,
}

impl Query {
    fn new(text: &str) -> Self {
        let lower = text.to_lowercase();
        let chars = lower.chars().collect();
        Self { lower, chars }
    }
}

/// `GenericName` + `Keywords` from the app's `.desktop` file, localised through
/// its own `Key[locale]=` entries. gio's `AppInfo` does not expose them.
struct DesktopMeta {
    generic: Option<Field>,
    keywords: Vec<Field>,
    actions: Vec<DesktopAction>,
}

/// One `[Desktop Action <id>]` group, surfaced as its own row (DMS-style).
struct DesktopAction {
    id: String,
    name: String,
    name_lower: String,
}

/// One installed application, precomputed at first search and reused for the
/// process lifetime; the `Field`s carry every match form a query needs.
struct CachedApp {
    id: String,
    title: String,
    title_field: Field,
    comment: Option<String>,
    comment_field: Option<Field>,
    icon_spec: Option<String>,
    /// Basename of the entry's `Exec=`, so the runner can match a PATH hit to a
    /// desktop app without reading every `.desktop` file again.
    exec: Option<String>,
    /// `Terminal=` of the entry, read here so the runner never reopens it.
    terminal: bool,
    meta: Option<DesktopMeta>,
    /// The id without its `.desktop` suffix, the last-resort match surface.
    id_field: Field,
}

impl CachedApp {
    /// Resolve the gio icon spec (`!!/path` for file icons, otherwise a theme
    /// name) to the absolute path the UI renders.
    fn icon_path(&self) -> Option<String> {
        let spec = self.icon_spec.as_deref()?;
        if let Some(path) = spec.strip_prefix("!!") {
            (!path.is_empty()).then(|| path.to_string())
        } else {
            resolve(spec)
        }
    }
}

static APPS: LazyLock<Vec<CachedApp>> = LazyLock::new(|| {
    let locales = desktop_locales();
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
            let entry = crate::system::desktop_action::entry(&id, Some(&locales));
            let exec = entry.as_ref().and_then(exec_basename);
            let terminal = entry.as_ref().is_some_and(DesktopEntry::terminal);
            let meta = entry.as_ref().map(|entry| parse_meta(entry, &locales));
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
                id: "app-search",
                name: t!("plugin.app.name"),
                icon: "builtin:app",
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

/// One scored candidate before its row exists: only the survivors of the cap
/// pay for the icon lookup a row needs.
enum Hit {
    App(&'static CachedApp),
    Action(&'static CachedApp, &'static DesktopAction),
}

impl Hit {
    fn title(&self) -> &str {
        match self {
            Hit::App(app) => &app.title,
            Hit::Action(_, action) => &action.name,
        }
    }

    fn row(&self) -> ResultItem {
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

        // Each action is its own row (DMS-style).
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

fn action_score(name_lower: &str, query_lower: &str) -> Option<(Match, u32)> {
    kind_of(name_lower, query_lower).map(|kind| (kind, kind.weight() * ACTION / 10))
}

/// The shared kind of a surface, refusing `Loose`: an app id or a description
/// is long enough that a scattered hit means nothing.
fn kind_of(surface_lower: &str, query_lower: &str) -> Option<Match> {
    crate::plugin::classify_confident(surface_lower, query_lower)
}

/// Score one match surface: the shared kind, scaled by what the surface is
/// worth. The kind travels with the weight, for a caller that merges providers.
fn field_score(field: &Field, query: &Query, share: u32) -> Option<(Match, u32)> {
    kind_of(&field.lower, &query.lower).map(|kind| (kind, kind.weight() * share / 10))
}

/// The typo tier: a near spelling of a 3+ char query, for the title only, so a
/// misspelling still finds the app without a keyword asking for it.
fn name_typo(name: &Field, query: &Query) -> Option<(Match, u32)> {
    if query.chars.len() < 3 {
        return None;
    }
    let similarity = fuzzy_score(name, query);
    (similarity >= TYPO_SIMILARITY)
        .then_some((Match::Loose, (TYPO_WEIGHT as f64 * similarity) as u32))
}

/// Edit-distance similarity (0..1) between the whole field or any of its words
/// and the query, within a tight per-length tolerance window.
fn fuzzy_score(field: &Field, query: &Query) -> f64 {
    let max_dist = match query.chars.len() {
        3 => 1,
        4..=6 => 2,
        _ => 3,
    };

    let mut best = 0.0f64;
    if (field.chars.len() as isize - query.chars.len() as isize).unsigned_abs() <= max_dist {
        let dist = levenshtein(&field.chars, &query.chars);
        if dist <= max_dist {
            best = 1.0 - dist as f64 / field.chars.len().max(query.chars.len()) as f64;
        }
    }

    for span in &field.word_spans {
        if best >= 0.8 {
            break;
        }
        let word = field.word(*span);
        if (word.len() as isize - query.chars.len() as isize).unsigned_abs() > max_dist {
            continue;
        }
        let dist = levenshtein(word, &query.chars);
        if dist <= max_dist {
            let score = 1.0 - dist as f64 / word.len().max(query.chars.len()) as f64;
            best = best.max(score);
        }
    }
    best
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Full app relevance: the title, then the metadata surfaces — a description,
/// keywords, a `GenericName`, then the desktop id; first non-zero surface wins.
fn score_app(
    name: &Field,
    comment: Option<&Field>,
    meta: Option<&DesktopMeta>,
    id: &Field,
    query: &Query,
) -> Option<(Match, u32)> {
    if query.lower.is_empty() {
        // an empty query would prefix-match every surface
        return None;
    }

    // The title takes any confident kind, and the typo tier of its own.
    if let Some(scored) = field_score(name, query, TITLE).or_else(|| name_typo(name, query)) {
        return Some(scored);
    }
    // The metadata surfaces answer in a plain query too, each at its own
    // strength: keywords and generics by word, a description only by prefix.
    let at_least =
        |scored: Option<(Match, u32)>, floor: Match| scored.filter(|(kind, _)| *kind >= floor);
    let comment = comment
        .and_then(|comment| field_score(comment, query, SUMMARY))
        .filter(|(kind, _)| *kind >= Match::Prefix);
    if let Some(scored) = comment {
        return Some(scored);
    }
    let keywords = meta.and_then(|meta| {
        meta.keywords
            .iter()
            .find_map(|keyword| at_least(field_score(keyword, query, KEYWORD), Match::Word))
    });
    if let Some(scored) = keywords {
        return Some(scored);
    }
    let generic = meta
        .and_then(|meta| meta.generic.as_ref())
        .and_then(|generic| at_least(field_score(generic, query, GENERIC), Match::Word));
    if let Some(scored) = generic {
        return Some(scored);
    }
    // The id answers where a word starts only: its middle spells anything.
    field_score(id, query, ID).filter(|(kind, _)| *kind >= Match::Word)
}

/// Read `[Desktop Entry]`'s `GenericName`/`Keywords` and its `Actions=` groups,
/// localised through the file's own `Key[locale]=` entries.
fn parse_meta(entry: &DesktopEntry, locales: &[String]) -> DesktopMeta {
    let generic = entry
        .generic_name(locales)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .map(|name| Field::new(&name));

    let keywords = entry
        .keywords(locales)
        .unwrap_or_default()
        .into_iter()
        .map(|word| word.trim().to_string())
        .filter(|word| !word.is_empty())
        .map(|word| Field::new(&word))
        .collect();

    let actions = entry
        .actions()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| {
            let name = entry.action_name(id, locales)?.trim().to_string();
            (!name.is_empty()).then(|| DesktopAction {
                id: id.to_string(),
                name_lower: name.to_lowercase(),
                name,
            })
        })
        .collect();

    DesktopMeta {
        generic,
        keywords,
        actions,
    }
}

/// The program a desktop `Exec=` runs, from its parsed argv: `env` is unwrapped
/// to the real program; a shell, sandbox or interpreter wrapper maps to nothing.
fn exec_program(argv: &[String]) -> Option<String> {
    let mut words = argv.iter();
    let first = program_name(words.next()?)?;
    let program = if first == "env" {
        let found = words.find(|word| !word.starts_with('-') && !word.contains('='))?;
        program_name(found)
    } else {
        Some(first)
    }?;
    const WRAPPERS: [&str; 8] = [
        "sh", "bash", "dash", "zsh", "flatpak", "snap", "python", "python3",
    ];
    (!WRAPPERS.contains(&program.as_str())).then_some(program)
}

/// The basename of a program word, e.g. `/usr/bin/foo` or `foo` -> `foo`.
fn program_name(word: &str) -> Option<String> {
    Path::new(word)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

/// Basename of the program a desktop entry's `Exec=` runs, for the runner's PATH hits.
fn exec_basename(entry: &DesktopEntry) -> Option<String> {
    let argv: Vec<String> = gio::glib::shell_parse_argv(entry.exec()?)
        .ok()?
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    exec_program(&argv)
}

/// The locale list gio localises `.desktop` keys with, from
/// `g_get_language_names()`; `.encoding` variants are dropped so `zh_CN` matches.
fn desktop_locales() -> Vec<String> {
    desktop_locales_from(gio::glib::language_names().into_iter().map(Into::into))
}

fn desktop_locales_from(names: impl IntoIterator<Item = String>) -> Vec<String> {
    names
        .into_iter()
        .filter(|name| !name.contains('.'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(generic: Option<&str>, keywords: &[&str]) -> DesktopMeta {
        DesktopMeta {
            generic: generic.map(Field::new),
            keywords: keywords.iter().map(|word| Field::new(word)).collect(),
            actions: Vec::new(),
        }
    }

    /// The relevance a plain query gives one app, or `0` when nothing matches:
    /// the shared kind's weight, scaled by whatever surface matched.
    fn s(name: &str, comment: Option<&str>, m: Option<&DesktopMeta>, id: &str, q: &str) -> u32 {
        score_app(
            &Field::new(name),
            comment.map(Field::new).as_ref(),
            m,
            &Field::new(id.to_lowercase().trim_end_matches(".desktop")),
            &Query::new(q.trim()),
        )
        .map_or(0, |(_, weight)| weight)
    }

    #[test]
    fn exact_beats_prefix_beats_substring() {
        let m = meta(None, &[]);
        assert_eq!(
            s("Telegram", None, Some(&m), "telegram.desktop", "telegram"),
            Match::Exact.weight()
        );
        assert_eq!(
            s("Telegram", None, Some(&m), "telegram.desktop", "tele"),
            Match::Prefix.weight()
        );
        assert_eq!(
            s("Bottles", None, Some(&m), "bottles.desktop", "bo"),
            Match::Prefix.weight()
        );
        assert_eq!(
            s(
                "LibreOffice",
                None,
                Some(&m),
                "libreoffice.desktop",
                "office"
            ),
            Match::Substring.weight()
        );
    }

    /// A plain query takes the title, plus each metadata surface at the strength
    /// that surface can carry.
    #[test]
    fn metadata_surfaces_speak_at_their_own_strength() {
        let m = meta(Some("Text Editor"), &["notes"]);
        let app = |q: &str| {
            s(
                "Zed",
                Some("A note taking app"),
                Some(&m),
                "dev.zed.Zed.desktop",
                q,
            )
        };
        assert_eq!(app("ze"), Match::Prefix.weight(), "the title still leads");
        assert_eq!(app("editor"), Match::Word.weight() * GENERIC / 10);
        assert_eq!(app("notes"), Match::Exact.weight() * KEYWORD / 10);
        assert_eq!(app("a note"), Match::Prefix.weight() * SUMMARY / 10);
        assert_eq!(app("taking"), 0, "a word picked out of prose does not");
        assert_eq!(app("dev"), Match::Prefix.weight() * ID / 10);
        assert_eq!(app("zed dev"), 0, "and prose never prefix-matches here");
    }

    /// The share is what makes the surfaces comparable: a title match beats a
    /// description match of comparable strength, while an exact keyword still
    /// beats a title hit the query only sits inside of.
    #[test]
    fn the_surface_share_orders_against_the_kind() {
        let m = meta(None, &[]);
        let title_word = s("Qt D-Bus Viewer", None, Some(&m), "qtv.desktop", "viewer");
        let summary_prefix = s(
            "Capture Tool",
            Some("viewer for V4L2 devices"),
            Some(&m),
            "v4l.desktop",
            "viewer",
        );
        assert!(
            title_word > summary_prefix,
            "a title word beats a description prefix: {title_word} > {summary_prefix}"
        );

        let keyword_exact = s(
            "GNU Image Manipulation Program",
            None,
            Some(&meta(None, &["gimp"])),
            "org.gimp.GIMP.desktop",
            "gimp",
        );
        let title_substring = s("Supergimp", None, Some(&m), "sg.desktop", "gimp");
        assert!(
            keyword_exact > title_substring,
            "an exact keyword beats a title substring: {keyword_exact} > {title_substring}"
        );
    }

    #[test]
    fn short_query_never_fuzzes() {
        // two chars: only strong tiers count — no weak mid-word hits
        let m = meta(None, &[]);
        assert_eq!(
            s("Yazi File Manager", None, Some(&m), "yazi.desktop", "bo"),
            0
        );
        assert_eq!(
            s(
                "GNU Image Manipulation Program",
                None,
                Some(&m),
                "gimp.desktop",
                "bo"
            ),
            0
        );
    }

    #[test]
    fn gimp_found_via_keyword_tier() {
        // Name/Comment are prose; 'gimp' lives only in Keywords (flatpak GIMP)
        let m = meta(Some("Image Editor"), &["GIMP", "graphic", "design"]);
        let sc = s(
            "GNU Image Manipulation Program",
            Some("Create images and edit photographs"),
            Some(&m),
            "org.gimp.GIMP.desktop",
            "gimp",
        );
        assert_eq!(sc, Match::Exact.weight() * KEYWORD / 10);
    }

    #[test]
    fn word_boundary_handles_multiword() {
        let m = meta(None, &[]);
        assert_eq!(
            s(
                "Dank Material Shell Settings",
                None,
                Some(&m),
                "dms.desktop",
                "material shell"
            ),
            Match::Word.weight()
        );
        // non-consecutive order is not a word-boundary hit
        assert!(
            s(
                "Dank Material Shell Settings",
                None,
                Some(&m),
                "dms.desktop",
                "shell material"
            ) < Match::Word.weight()
        );
    }

    #[test]
    fn generic_name_fallback() {
        let m = meta(Some("Text Editor"), &[]);
        assert_eq!(
            s("DMS Notes", None, Some(&m), "dms-notes.desktop", "editor"),
            Match::Word.weight() * GENERIC / 10
        );
        assert_eq!(
            s("DMS Notes", None, Some(&m), "dms-notes.desktop", "text"),
            Match::Prefix.weight() * GENERIC / 10
        );
    }

    #[test]
    fn desktop_id_is_last_resort() {
        let m = meta(None, &[]);
        assert_eq!(
            s("Strange Name", None, Some(&m), "firefox.desktop", "firefox"),
            Match::Exact.weight() * ID / 10
        );
        assert_eq!(
            s("Strange Name", None, Some(&m), "firefox.desktop", "zzz"),
            0
        );
    }

    #[test]
    fn fuzzy_only_from_three_chars() {
        let m = meta(None, &[]);
        // krta vs Krita: one transposition-ish edit, len 4
        assert!(s("Krita", None, Some(&m), "krita.desktop", "krta") > 0);
        assert!(
            s("Krita", None, Some(&m), "krita.desktop", "krta") < Match::Substring.weight(),
            "a typo is a title hit, but never a real title kind"
        );
        // two-char typo is not enough to matter
        assert_eq!(s("Krita", None, Some(&m), "krita.desktop", "kt"), 0);
    }

    /// An action's own label is a surface between the title and the summary.
    #[test]
    fn an_action_label_ranks_between_the_title_and_the_summary() {
        let scored = |q: &str| action_score("open vm manager", q).map(|(_, weight)| weight);
        assert_eq!(
            scored("open vm manager"),
            Some(Match::Exact.weight() * ACTION / 10)
        );
        assert_eq!(scored("open"), Some(Match::Prefix.weight() * ACTION / 10));
        assert_eq!(scored("vm"), Some(Match::Word.weight() * ACTION / 10));
        assert_eq!(scored("zzz"), None);
    }

    /// `parse_meta` against a `.desktop` body, with the locales pinned so the
    /// result cannot depend on the test runner's `$LANG`.
    fn meta_of(content: &str, locales: &[&str]) -> DesktopMeta {
        let locales: Vec<String> = locales.iter().map(|l| (*l).to_string()).collect();
        let entry = DesktopEntry::from_str("app.desktop", content, Some(&locales)).unwrap();
        parse_meta(&entry, &locales)
    }

    #[test]
    fn an_env_wrapper_maps_to_the_real_program() {
        let argv = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            exec_program(&argv(&[
                "env",
                "GTK_IM_MODULE=fcitx",
                "nautilus",
                "--new-window"
            ])),
            Some("nautilus".into())
        );
        assert_eq!(
            exec_program(&argv(&["/usr/lib/firefox/firefox", "%u"])),
            Some("firefox".into())
        );
        assert_eq!(exec_program(&argv(&["sh", "-c", "scrcpy"])), None);
        assert_eq!(
            exec_program(&argv(&["env", "A=1", "flatpak", "run", "x"])),
            None
        );
    }

    #[test]
    fn parse_meta_reads_action_groups() {
        let meta = meta_of(
            "[Desktop Entry]\n\
             GenericName=Virtualization Software\n\
             Keywords=virtualization;\n\
             Actions=Manager;\n\
             Name[de]=Oracle VirtualBox\n\
             \n\
             [Desktop Action Manager]\n\
             Name=Open VM Manager\n\
             Name[de]=VM Manager oeffnen\n\
             Exec=VirtualBox\n\
             \n\
             [Desktop Action Broken]\n\
             Name[de]=Nur auf Deutsch\n",
            &["en"],
        );
        assert_eq!(
            meta.generic.as_ref().map(|g| g.lower.as_str()),
            Some("virtualization software")
        );
        assert_eq!(meta.keywords.len(), 1);
        assert_eq!(meta.keywords[0].lower, "virtualization");
        // an undeclared group is not a row, and neither is a localised-only name
        assert_eq!(meta.actions.len(), 1);
        assert_eq!(meta.actions[0].id, "Manager");
        assert_eq!(meta.actions[0].name, "Open VM Manager");
        assert_eq!(meta.actions[0].name_lower, "open vm manager");
    }

    #[test]
    fn a_localised_action_name_wins_in_its_locale() {
        let entry = "[Desktop Entry]\n\
                     Name=VirtualBox\n\
                     Actions=Manager;\n\
                     [Desktop Action Manager]\n\
                     Name=Open VM Manager\n\
                     Name[de]=VM Manager oeffnen\n";

        assert_eq!(
            meta_of(entry, &["de_DE"]).actions[0].name,
            "VM Manager oeffnen"
        );
        assert_eq!(meta_of(entry, &["en"]).actions[0].name, "Open VM Manager");
    }

    #[test]
    fn empty_query_matches_nothing() {
        let m = meta(None, &[]);
        assert_eq!(s("Anything", None, Some(&m), "a.desktop", ""), 0);
        assert!(do_search("").is_empty());
    }

    #[test]
    fn desktop_locales_drop_encoding_variants() {
        // the shape g_get_language_names returns for zh_CN.UTF-8, with the bare
        // language before an encoded key can match it
        let names = ["zh_CN.UTF-8", "zh_CN", "zh.UTF-8", "zh", "C"].map(String::from);
        assert_eq!(desktop_locales_from(names), ["zh_CN", "zh", "C"]);
        // a @modifier survives, its encoded spelling does not
        let names = ["sr_RS.UTF-8@latin", "sr_RS@latin", "sr@latin"].map(String::from);
        assert_eq!(desktop_locales_from(names), ["sr_RS@latin", "sr@latin"]);
    }
}
