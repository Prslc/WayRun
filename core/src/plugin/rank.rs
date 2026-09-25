use memchr::memchr2;

use crate::wire::ResultItem;

/// One provider's rows, each with how the query matched it.
pub type Ranked = Vec<(Rank, ResultItem)>;

/// Where a provider's rows sit next to another provider's: the relevance a
/// scoring provider computed, or its own listed order when it cannot score.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rank {
    /// A row from a provider that does not score: the lower the position, the
    /// better, and every one of them sits below a scored row.
    Listed(u32),
    /// The row's relevance: the shared kind's weight, scaled by the surface it
    /// matched on. The product, not either alone, is what the merge orders by.
    Scored(u32),
}

impl Rank {
    /// A row that matched on its own title, the common case: the kind's weight,
    /// unscaled. A provider with metadata surfaces computes its own weights.
    pub fn title(kind: Match) -> Self {
        Rank::Scored(kind.weight())
    }

    /// The sort key: a scored row by relevance; a listed row below every scored
    /// one and in its provider's own order.
    pub fn key(self) -> (u8, u32) {
        match self {
            Rank::Listed(at) => (0, u32::MAX - at),
            Rank::Scored(weight) => (1, weight),
        }
    }
}

/// How a query matches one surface: the vocabulary every provider ranks by, so
/// "exact beats prefix" means one thing; weakest first, for the derived `Ord`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Match {
    /// The query's letters appear in order, scattered.
    Loose,
    /// The query sits inside the surface.
    Substring,
    /// The query starts a word inside the surface.
    Word,
    /// The surface starts with the query.
    Prefix,
    /// The surface is the query.
    Exact,
}

impl Match {
    /// One weight table; its gaps are wider than any depth or length modifier
    /// a provider subtracts, so a modifier can only break ties within a kind.
    pub fn weight(self) -> u32 {
        match self {
            Match::Exact => 10_000,
            Match::Prefix => 5_000,
            Match::Word => 3_000,
            Match::Substring => 500,
            Match::Loose => 100,
        }
    }
}

/// The strongest way a lowercased `query` matches an already lowercased
/// surface, stopping at `Substring` for a caller a scattered hit cannot answer.
pub fn classify_confident(surface: &str, query: &str) -> Option<Match> {
    if query.is_empty() {
        return None;
    }
    if surface == query {
        return Some(Match::Exact);
    }
    if surface.starts_with(query) {
        return Some(Match::Prefix);
    }
    if at_word(surface, query) {
        return Some(Match::Word);
    }
    surface.contains(query).then_some(Match::Substring)
}

/// The strongest way a lowercased `query` matches an already lowercased surface.
pub fn classify(surface: &str, query: &str) -> Option<Match> {
    if query.is_empty() {
        return None;
    }
    classify_confident(surface, query).or_else(|| scattered(surface, query).then_some(Match::Loose))
}

/// [`classify`] without a lowercased surface: an ASCII surface compares byte by
/// byte, and only a non-ASCII one pays for `to_lowercase`.
pub fn classify_ci(surface: &str, query: &str) -> Option<Match> {
    if surface.is_ascii() {
        return classify_bytes(surface.as_bytes(), query.as_bytes());
    }
    classify(&surface.to_lowercase(), query)
}

/// [`classify_confident`] without a lowercased surface.
pub fn classify_ci_confident(surface: &str, query: &str) -> Option<Match> {
    if surface.is_ascii() {
        return classify_bytes_confident(surface.as_bytes(), query.as_bytes());
    }
    classify_confident(&surface.to_lowercase(), query)
}

/// [`classify_confident`] over raw bytes, for a surface and query the caller
/// checked are ASCII: a byte compare is the same test without building a string.
pub fn classify_bytes_confident(surface: &[u8], query: &[u8]) -> Option<Match> {
    if query.is_empty() {
        return None;
    }
    if surface.eq_ignore_ascii_case(query) {
        return Some(Match::Exact);
    }
    if surface.len() >= query.len() && surface[..query.len()].eq_ignore_ascii_case(query) {
        return Some(Match::Prefix);
    }
    // One walk answers both kinds: `memchr2` skips positions the query's first
    // byte cannot start at, and a word-start hit outranks a plain one.
    let lower = query[0].to_ascii_lowercase();
    let upper = query[0].to_ascii_uppercase();
    let mut hit = false;
    let mut from = 1; // a match at 0 returned `Prefix` above
    while from + query.len() <= surface.len() {
        let Some(offset) = memchr2(lower, upper, &surface[from..]) else {
            break;
        };
        let at = from + offset;
        if at + query.len() > surface.len() {
            break;
        }
        if surface[at..at + query.len()].eq_ignore_ascii_case(query) {
            if !surface[at - 1].is_ascii_alphanumeric() {
                return Some(Match::Word);
            }
            hit = true;
        }
        from = at + 1;
    }
    hit.then_some(Match::Substring)
}

/// [`classify`] over raw bytes, for a surface and query the caller checked are
/// ASCII: a byte compare is the same test without building a string.
pub fn classify_bytes(surface: &[u8], query: &[u8]) -> Option<Match> {
    if query.is_empty() {
        return None;
    }
    classify_bytes_confident(surface, query)
        .or_else(|| scattered_ci(surface, query).then_some(Match::Loose))
}

/// Whether `query` occurs in `surface` at the start of a word.
fn at_word(surface: &str, query: &str) -> bool {
    surface.match_indices(query).any(|(at, _)| {
        at > 0
            && !surface[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
    })
}

/// Whether `query`'s characters appear in `surface` in order.
fn scattered(surface: &str, query: &str) -> bool {
    let mut chars = surface.chars();
    query.chars().all(|wanted| chars.any(|c| c == wanted))
}

fn scattered_ci(surface: &[u8], query: &[u8]) -> bool {
    let mut at = 0;
    for &wanted in query {
        while at < surface.len() && !surface[at].eq_ignore_ascii_case(&wanted) {
            at += 1;
        }
        if at == surface.len() {
            return false;
        }
        at += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_byte_classify_matches_the_text_one_for_ascii_names() {
        for (name, query) in [
            ("WayRun", "wayrun"),
            ("NOTES.txt", "notes.txt"),
            ("report", "report"),
            ("xreport-report.txt", "report"),
            ("xreport.txt", "report"),
            ("xreport", "report"),
            ("abcr", "report"),
            ("x", "wayrun"),
            ("abc", "报告"),
            ("", ""),
        ] {
            assert_eq!(
                classify_bytes(name.as_bytes(), query.as_bytes()),
                classify(&name.to_lowercase(), query),
                "{name} / {query}"
            );
        }
    }
}
