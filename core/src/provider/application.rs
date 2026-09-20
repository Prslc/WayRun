use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;

use anyhow::Result;
use freedesktop_desktop_entry::DesktopEntry;
use gio::prelude::{AppInfoExt, IconExt};

use crate::plugin::{Meta, Plugin};
use crate::system::icon::resolve;
use crate::wire::{Action, ActionItem, PanelAction, ResultItem};

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

/// `GenericName` + `Keywords` from the app's `.desktop` file, localised through
/// its own `Key[locale]=` entries. gio's `AppInfo` does not expose them.
struct DesktopMeta {
    generic: Option<String>,
    keywords: Vec<String>,
    actions: Vec<DesktopAction>,
}

/// One `[Desktop Action <id>]` group, surfaced as its own row (DMS-style).
struct DesktopAction {
    id: String,
    name: String,
    name_lower: String,
}

/// One installed application, precomputed at first search and reused for the
/// process lifetime; lowercased fields avoid per-query re-lowering.
struct CachedApp {
    id: String,
    title: String,
    title_lower: String,
    comment: Option<String>,
    comment_lower: Option<String>,
    icon_spec: Option<String>,
    meta: Option<DesktopMeta>,
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
            Some(CachedApp {
                title_lower: title.to_lowercase(),
                comment_lower: comment.as_ref().map(|c| c.to_lowercase()),
                meta: desktop_meta(&id),
                title,
                comment,
                id,
                icon_spec,
            })
        })
        .collect()
});

pub struct AppSearch;

impl Plugin for AppSearch {
    fn meta(&self) -> &Meta {
        &Meta {
            id: "app-search",
            name: "Applications",
            icon: "application_default",
            ready: "Search installed applications",
        }
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
            })
            .collect()
    }
}

/// Score every cached app against the query; the query text is lowercased
/// and tokenized once, not per app.
fn do_search(query: &str) -> Vec<ResultItem> {
    let query_lower = query.trim().to_lowercase();
    if query_lower.is_empty() {
        return Vec::new();
    }
    let query_words = tokenize(&query_lower);

    let mut results: Vec<(u32, ResultItem)> = Vec::new();
    for app in APPS.iter() {
        let score = score_app(
            &app.title_lower,
            app.comment_lower.as_deref(),
            app.meta.as_ref(),
            &app.id,
            &query_lower,
            &query_words,
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
            let action_score = action_score(&action.name_lower, &query_lower);
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
/// word-boundary, substring, then edit-distance fuzz. Inputs must be lowercased.
fn field_score(field_lower: &str, query_lower: &str, query_words: &[String]) -> u32 {
    // An empty query would prefix-match every field; treat it as no match so
    // the scorer cannot turn into a "list everything" path.
    if query_lower.is_empty() {
        return 0;
    }
    if field_lower == query_lower {
        return W_EXACT;
    }
    if field_lower.starts_with(query_lower) {
        return W_PREFIX;
    }

    let words = tokenize(field_lower);
    if query_words.len() <= words.len() {
        let bounded = (0..=words.len() - query_words.len())
            .any(|i| (0..query_words.len()).all(|j| words[i + j].starts_with(&query_words[j])));
        if bounded {
            return W_WORD_BOUNDARY;
        }
    }

    if field_lower.contains(query_lower) {
        return W_SUBSTRING;
    }

    if query_lower.chars().count() >= 3 {
        let fs = fuzzy_score(field_lower, query_lower);
        if fs > 0.0 {
            return (fs * f64::from(W_FUZZY)) as u32;
        }
    }
    0
}

/// Edit-distance similarity (0..1) between a whole text or any of its words
/// and the query, within a tight per-length tolerance window.
fn fuzzy_score(text: &str, query: &str) -> f64 {
    let text_chars: Vec<char> = text.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();
    let max_dist = match query_chars.len() {
        3 => 1,
        4..=6 => 2,
        _ => 3,
    };

    let mut best = 0.0f64;
    if (text_chars.len() as isize - query_chars.len() as isize).unsigned_abs() <= max_dist {
        let dist = levenshtein(&text_chars, &query_chars);
        if dist <= max_dist {
            best = 1.0 - dist as f64 / text_chars.len().max(query_chars.len()) as f64;
        }
    }

    for word in tokenize(text) {
        if best >= 0.8 {
            break;
        }
        let word_chars: Vec<char> = word.chars().collect();
        if (word_chars.len() as isize - query_chars.len() as isize).unsigned_abs() > max_dist {
            continue;
        }
        let dist = levenshtein(&word_chars, &query_chars);
        if dist <= max_dist {
            let score = 1.0 - dist as f64 / word_chars.len().max(query_chars.len()) as f64;
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
/// then the desktop id; first non-zero tier wins. Inputs must be lowercased.
fn score_app(
    name_lower: &str,
    comment_lower: Option<&str>,
    meta: Option<&DesktopMeta>,
    id: &str,
    query_lower: &str,
    query_words: &[String],
) -> u32 {
    if query_lower.is_empty() {
        return 0;
    }

    let mut score = field_score(name_lower, query_lower, query_words);
    if score == 0
        && let Some(c) = comment_lower
    {
        score = field_score(c, query_lower, query_words) * 5 / 10;
    }
    if score == 0
        && let Some(v) = meta.and_then(|m| {
            m.keywords.iter().find_map(|keyword| {
                let ks = field_score(&keyword.to_lowercase(), query_lower, query_words);
                (ks > 0).then_some(ks * 3 / 10)
            })
        })
    {
        score = v;
    }
    if score == 0
        && let Some(g) = meta.and_then(|m| m.generic.as_deref())
    {
        let generic_lower = g.to_lowercase();
        score = if generic_lower.starts_with(query_lower) {
            W_GENERIC_PREFIX
        } else if generic_lower.contains(query_lower) {
            W_GENERIC
        } else {
            0
        };
    }
    if score == 0 {
        let id_lower = id.to_lowercase().trim_end_matches(".desktop").to_string();
        if id_lower.contains(query_lower) {
            score = W_ID;
        }
    }
    score
}

/// Read `[Desktop Entry]`'s `GenericName`/`Keywords` and its `Actions=` groups,
/// localised through the file's own `Key[locale]=` entries.
fn parse_meta(entry: &DesktopEntry, locales: &[String]) -> DesktopMeta {
    let generic = entry
        .generic_name(locales)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());

    let keywords = entry
        .keywords(locales)
        .unwrap_or_default()
        .into_iter()
        .map(|word| word.trim().to_string())
        .filter(|word| !word.is_empty())
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

/// Read `GenericName`/`Keywords`/`Actions` from the `.desktop` file the XDG data
/// dirs resolve for `id`; gio-rs binds no `GDesktopAppInfo`.
fn desktop_meta(id: &str) -> Option<DesktopMeta> {
    let locales = desktop_locales();
    let entry = crate::system::desktop_action::entry(id, Some(&locales))?;

    Some(parse_meta(&entry, &locales))
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
            generic: generic.map(String::from),
            keywords: keywords.iter().map(ToString::to_string).collect(),
            actions: Vec::new(),
        }
    }

    fn s(name: &str, comment: Option<&str>, m: Option<&DesktopMeta>, id: &str, q: &str) -> u32 {
        let name_lower = name.to_lowercase();
        let comment_lower = comment.map(str::to_lowercase);
        let query_lower = q.trim().to_lowercase();
        let query_words = tokenize(&query_lower);
        score_app(
            &name_lower,
            comment_lower.as_deref(),
            m,
            id,
            &query_lower,
            &query_words,
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
        assert_eq!(meta.generic.as_deref(), Some("Virtualization Software"));
        assert_eq!(meta.keywords, ["virtualization"]);
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
