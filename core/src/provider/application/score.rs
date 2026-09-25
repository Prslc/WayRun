use crate::plugin::Match;

use super::model::{DesktopMeta, Field, Query};

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

pub(super) fn action_score(name_lower: &str, query_lower: &str) -> Option<(Match, u32)> {
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
pub(super) fn score_app(
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

#[cfg(test)]
mod tests {
    use super::super::do_search;
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

    /// The share makes the surfaces comparable: a title match beats a description
    /// match of comparable strength, while an exact keyword still beats a title hit.
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

    #[test]
    fn empty_query_matches_nothing() {
        let m = meta(None, &[]);
        assert_eq!(s("Anything", None, Some(&m), "a.desktop", ""), 0);
        assert!(do_search("").is_empty());
    }
}
