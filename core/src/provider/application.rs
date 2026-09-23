use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::LazyLock;

use anyhow::Result;
use freedesktop_desktop_entry::DesktopEntry;
use gio::prelude::{AppInfoExt, IconExt};

use crate::plugin::{Meta, Plugin};
use crate::system::icon::resolve;
use crate::wire::{Action, ActionItem, PanelAction, ResultItem};
use rust_i18n::t;

// Tiered weights: a strong textual tier wins outright and fuzzy matching is a
// last resort for 3+ char queries, so a short query hits a strong tier or misses.
const W_EXACT: u32 = 10_000;
const W_PREFIX: u32 = 5_000;
const W_WORD_BOUNDARY: u32 = 3_000;
const W_SUBSTRING: u32 = 500;
const W_GENERIC_PREFIX: u32 = 800;
const W_GENERIC: u32 = 400;
const W_ID: u32 = 350;
const W_FUZZY: u32 = 100;
const W_ACTION_EXACT: u32 = 8_000;
const W_ACTION_PREFIX: u32 = 4_000;
const W_ACTION_SUBSTRING: u32 = 400;

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
    words: Vec<String>,
    chars: Vec<char>,
}

impl Query {
    fn new(text: &str) -> Self {
        let lower = text.to_lowercase();
        let words = tokenize(&lower);
        let chars = lower.chars().collect();
        Self {
            lower,
            words,
            chars,
        }
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
            let meta = entry.as_ref().map(|entry| parse_meta(entry, &locales));
            Some(CachedApp {
                title_field: Field::new(&title),
                comment_field: comment.as_deref().map(Field::new),
                id_field: Field::new(id.to_lowercase().trim_end_matches(".desktop")),
                meta,
                exec,
                title,
                comment,
                id,
                icon_spec,
            })
        })
        .collect()
});

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

    fn search(
        &self,
        _query: &str,
        full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        let input = full.to_string();
        Box::pin(async move {
            Ok(tokio::task::spawn_blocking(move || do_search(&input))
                .await
                .unwrap_or_default())
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
fn do_search(query: &str) -> Vec<ResultItem> {
    let query = Query::new(query.trim());
    if query.lower.is_empty() {
        return Vec::new();
    }

    let mut results: Vec<(u32, ResultItem)> = Vec::new();
    for app in APPS.iter() {
        let score = score_app(
            &app.title_field,
            app.comment_field.as_ref(),
            app.meta.as_ref(),
            &app.id_field,
            &query,
        );
        if score > 0 {
            results.push((
                score,
                ResultItem {
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
            ));
        }

        // Each action is its own row (DMS-style).
        for action in app.meta.iter().flat_map(|m| &m.actions) {
            let action_score = action_score(&action.name_lower, &query.lower);
            if action_score > 0 {
                results.push((
                    action_score,
                    ResultItem {
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
                ));
            }
        }
    }

    crate::provider::rank_results(results, true, 50)
}

fn tokenize(s: &str) -> Vec<String> {
    s.split([' ', '-', '_'])
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// DMS's action tiers: exact, prefix, substring.
fn action_score(name_lower: &str, query_lower: &str) -> u32 {
    if name_lower == query_lower {
        W_ACTION_EXACT
    } else if name_lower.starts_with(query_lower) {
        W_ACTION_PREFIX
    } else if name_lower.contains(query_lower) {
        W_ACTION_SUBSTRING
    } else {
        0
    }
}

/// Score one match surface, fields tried in order: exact name, prefix,
/// word-boundary, substring, then edit-distance fuzz.
fn field_score(field: &Field, query: &Query) -> u32 {
    // An empty query would prefix-match every field; treat it as no match so
    // the scorer cannot turn into a "list everything" path.
    if query.lower.is_empty() {
        return 0;
    }
    if field.lower == query.lower {
        return W_EXACT;
    }
    if field.lower.starts_with(&query.lower) {
        return W_PREFIX;
    }

    let spans = &field.word_spans;
    if query.words.len() <= spans.len() {
        let bounded = (0..=spans.len() - query.words.len()).any(|i| {
            (0..query.words.len())
                .all(|j| char_starts_with(field.word(spans[i + j]), &query.words[j]))
        });
        if bounded {
            return W_WORD_BOUNDARY;
        }
    }

    if field.lower.contains(&query.lower) {
        return W_SUBSTRING;
    }

    if query.chars.len() >= 3 {
        let fs = fuzzy_score(field, query);
        if fs > 0.0 {
            return (fs * f64::from(W_FUZZY)) as u32;
        }
    }
    0
}

/// Whether `hay` starts with `needle`, so a precomputed word is not rebuilt
/// into a `String` for every boundary check.
fn char_starts_with(hay: &[char], needle: &str) -> bool {
    let mut needle = needle.chars();
    for ch in hay {
        match needle.next() {
            Some(expected) if expected == *ch => {}
            Some(_) => return false,
            None => return true,
        }
    }
    needle.next().is_none()
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

/// Full app relevance: name, comment (0.5x), keywords (0.3x), `GenericName`,
/// then the desktop id; first non-zero tier wins.
fn score_app(
    name: &Field,
    comment: Option<&Field>,
    meta: Option<&DesktopMeta>,
    id: &Field,
    query: &Query,
) -> u32 {
    if query.lower.is_empty() {
        return 0;
    }

    let mut score = field_score(name, query);
    if score == 0
        && let Some(comment) = comment
    {
        score = field_score(comment, query) * 5 / 10;
    }
    if score == 0
        && let Some(ks) = meta.and_then(|m| {
            m.keywords.iter().find_map(|keyword| {
                let ks = field_score(keyword, query);
                (ks > 0).then_some(ks * 3 / 10)
            })
        })
    {
        score = ks;
    }
    if score == 0
        && let Some(generic) = meta.and_then(|m| m.generic.as_ref())
    {
        score = if generic.lower.starts_with(&query.lower) {
            W_GENERIC_PREFIX
        } else if generic.lower.contains(&query.lower) {
            W_GENERIC
        } else {
            0
        };
    }
    if score == 0 && id.lower.contains(&query.lower) {
        score = W_ID;
    }
    score
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

/// The program a desktop `Exec=` runs, from its parsed argv. `env` is unwrapped
/// (`Exec=env VAR=… prog`) so the real program is the match; a shell, sandbox or
/// interpreter wrapper runs no PATH program of its own, so it maps to nothing.
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

/// Basename of the program a desktop entry's `Exec=` runs, for the runner's PATH
/// hits.
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

    fn s(name: &str, comment: Option<&str>, m: Option<&DesktopMeta>, id: &str, q: &str) -> u32 {
        score_app(
            &Field::new(name),
            comment.map(Field::new).as_ref(),
            m,
            &Field::new(id.to_lowercase().trim_end_matches(".desktop")),
            &Query::new(q.trim()),
        )
    }

    #[test]
    fn exact_beats_prefix_beats_substring() {
        let m = meta(None, &[]);
        assert_eq!(
            s("Telegram", None, Some(&m), "telegram.desktop", "telegram"),
            W_EXACT
        );
        assert_eq!(
            s("Telegram", None, Some(&m), "telegram.desktop", "tele"),
            W_PREFIX
        );
        assert!(s("Bottles", None, Some(&m), "bottles.desktop", "bo") == W_PREFIX);
        assert_eq!(
            s(
                "LibreOffice",
                None,
                Some(&m),
                "libreoffice.desktop",
                "office"
            ),
            W_SUBSTRING
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
        assert_eq!(sc, W_EXACT * 3 / 10);
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
            W_WORD_BOUNDARY
        );
        // non-consecutive order is not a word-boundary hit
        assert!(
            s(
                "Dank Material Shell Settings",
                None,
                Some(&m),
                "dms.desktop",
                "shell material"
            ) < W_WORD_BOUNDARY
        );
    }

    #[test]
    fn generic_name_fallback() {
        let m = meta(Some("Text Editor"), &[]);
        assert_eq!(
            s("DMS Notes", None, Some(&m), "dms-notes.desktop", "editor"),
            W_GENERIC
        );
        assert_eq!(
            s("DMS Notes", None, Some(&m), "dms-notes.desktop", "text"),
            W_GENERIC_PREFIX
        );
    }

    #[test]
    fn desktop_id_is_last_resort() {
        let m = meta(None, &[]);
        assert_eq!(
            s("Strange Name", None, Some(&m), "firefox.desktop", "firefox"),
            W_ID
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
        assert!(s("Krita", None, Some(&m), "krita.desktop", "krta") < W_FUZZY);
        // two-char typo is not enough to matter
        assert_eq!(s("Krita", None, Some(&m), "krita.desktop", "kt"), 0);
    }

    #[test]
    fn char_starts_with_matches_str_starts_with() {
        let hay: Vec<char> = "manager".chars().collect();
        assert!(char_starts_with(&hay, "man"));
        assert!(char_starts_with(&hay, ""));
        assert!(!char_starts_with(&hay, "manager "));
        assert!(!char_starts_with(&hay[..3], "manager"));
    }

    #[test]
    fn action_rows_use_dms_tiers() {
        assert_eq!(
            action_score("open vm manager", "open vm manager"),
            W_ACTION_EXACT
        );
        assert_eq!(action_score("open vm manager", "open"), W_ACTION_PREFIX);
        assert_eq!(action_score("open vm manager", "vm"), W_ACTION_SUBSTRING);
        assert_eq!(action_score("open vm manager", "zzz"), 0);
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
