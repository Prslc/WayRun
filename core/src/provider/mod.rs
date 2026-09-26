pub mod application;
pub mod calculator;
pub mod clipboard;
pub mod external;
pub mod file;
pub(crate) mod resident;
pub mod runner;
pub mod shipped;
pub mod system_commands;
pub mod window;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::plugin::Plugin;
use crate::wire::ResultItem;

/// Rows a provider shows: the one cap every scored provider passes to
/// [`rank_results`], so the index and the fallback walk cannot diverge.
pub const SHOW_CAP: usize = 50;

/// How long a provider's fetch stays fresh: a typing burst shares one
/// shell-out, and a row a beat stale is harmless.
pub const BURST_TTL: Duration = Duration::from_millis(500);

/// A provider's short-lived fetch cache, so a burst of keystrokes does not
/// re-run the same shell-out on every one; failures are never cached.
pub struct FreshCache<T> {
    state: Arc<Mutex<State<T>>>,
}

struct State<T> {
    rows: Option<(Instant, Arc<Vec<T>>)>,
    refreshing: bool,
}

impl<T> State<T> {
    /// The rows while the entry is still inside [`BURST_TTL`].
    fn fresh(&self) -> Option<Arc<Vec<T>>> {
        let (at, rows) = self.rows.as_ref()?;
        (at.elapsed() < BURST_TTL).then(|| Arc::clone(rows))
    }
}

impl<T: Send + Sync + 'static> FreshCache<T> {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                rows: None,
                refreshing: false,
            })),
        }
    }

    /// The cached rows: a fresh entry answers directly, a stale one answers
    /// while a background refetch replaces it, and only a cold cache waits.
    pub async fn get(
        &self,
        fetch: impl FnOnce() -> Option<Vec<T>> + Send + 'static,
    ) -> Option<Arc<Vec<T>>> {
        let (stale, start) = {
            let mut state = self.lock();
            if let Some(rows) = state.fresh() {
                return Some(rows);
            }
            let stale = state.rows.as_ref().map(|(_, rows)| Arc::clone(rows));
            let start = stale.is_some() && !state.refreshing;
            state.refreshing |= start;
            (stale, start)
        };
        match stale {
            Some(rows) => {
                if start {
                    self.refresh(fetch);
                }
                Some(rows)
            }
            None => self.fetch_now(fetch).await,
        }
    }

    /// The first read waits for the fetch; a cold search has no rows to answer
    /// with, and every later one rides the cache.
    async fn fetch_now(
        &self,
        fetch: impl FnOnce() -> Option<Vec<T>> + Send + 'static,
    ) -> Option<Arc<Vec<T>>> {
        let rows = tokio::task::spawn_blocking(fetch).await.ok().flatten()?;
        let rows = Arc::new(rows);
        self.lock().rows = Some((Instant::now(), Arc::clone(&rows)));
        Some(rows)
    }

    /// Revalidate past [`BURST_TTL`] without blocking the caller; the flag
    /// clears on any outcome, so a failed fetch retries on the next read.
    fn refresh(&self, fetch: impl FnOnce() -> Option<Vec<T>> + Send + 'static) {
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let rows = tokio::task::spawn_blocking(fetch).await.ok().flatten();
            let mut state = state.lock().unwrap_or_else(|err| err.into_inner());
            if let Some(rows) = rows {
                state.rows = Some((Instant::now(), Arc::new(rows)));
            }
            state.refreshing = false;
        });
    }

    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(|err| err.into_inner())
    }
}

/// The common tail every scored provider shares: strongest score first,
/// optionally one row per title, capped at `max` results.
pub fn rank_results<T: Ord>(
    mut scored: Vec<(T, ResultItem)>,
    dedup_titles: bool,
    max: usize,
) -> Vec<ResultItem> {
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    if dedup_titles {
        scored.dedup_by(|a, b| a.1.title == b.1.title);
    }
    scored.truncate(max);
    scored.into_iter().map(|(_, item)| item).collect()
}

/// Append `s` lowercased without allocating: ASCII goes byte by byte, anything
/// else falls back to `to_lowercase`.
pub fn push_lowered(out: &mut String, s: &str) {
    if s.is_ascii() {
        for b in s.bytes() {
            out.push(b.to_ascii_lowercase() as char);
        }
    } else {
        out.push_str(&s.to_lowercase());
    }
}

pub fn plugin_map() -> HashMap<&'static str, Box<dyn Plugin>> {
    let mut m: HashMap<&'static str, Box<dyn Plugin>> = HashMap::default();
    m.insert("calculator", Box::new(calculator::Calculator::new()));
    m.insert("app-search", Box::new(application::AppSearch::new()));
    m.insert("file-search", Box::new(file::FileSearch::new()));
    m.insert("path-search", Box::new(file::PathSearch::new()));
    m.insert("clipboard", Box::new(clipboard::Clipboard::new()));
    m.insert(
        "system-commands",
        Box::new(system_commands::SystemCommands::new()),
    );
    m.insert("runner", Box::new(runner::Runner::new()));
    m.insert("window", Box::new(window::WindowPlugin::new()));
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{
        Match, classify, classify_bytes_confident, classify_ci, classify_ci_confident,
        classify_confident,
    };

    #[test]
    fn a_case_insensitive_classify_matches_the_lowered_one() {
        for (name, query) in [
            ("WayRun", "wayrun"),
            ("wayrun", "wayrun"),
            ("wayrun-x86_64-unknown-linux-gnu.zip", "wayrun"),
            ("NOTES.txt", "notes.txt"),
            ("annual-Report.pdf", "report"),
            ("报告.TXT", "报告"),
            ("报告.txt", "report"),
            ("Ünicode", "ünicode"),
            ("x", "wayrun"),
            ("", ""),
        ] {
            assert_eq!(
                classify_ci(name, query),
                classify(&name.to_lowercase(), query),
                "{name} / {query}"
            );
        }
    }

    /// The vocabulary's order is the contract: no provider may rank a weaker
    /// kind over a stronger one.
    #[test]
    fn the_kinds_read_the_way_the_tiers_always_did() {
        assert_eq!(classify("telegram", "telegram"), Some(Match::Exact));
        assert_eq!(classify("telegram", "tele"), Some(Match::Prefix));
        assert_eq!(
            classify("dank material shell", "material"),
            Some(Match::Word)
        );
        assert_eq!(classify("libreoffice", "office"), Some(Match::Substring));
        assert_eq!(classify("chart", "cat"), Some(Match::Loose));
        assert_eq!(classify("ls", "cat"), None);
        assert!(Match::Exact > Match::Prefix);
        assert!(Match::Prefix > Match::Word);
        assert!(Match::Word > Match::Substring);
        assert!(Match::Substring > Match::Loose);
    }

    /// A plain query stops at `Substring`: the confident classifiers refuse the
    /// scattered hit the full one still reports for the fuzzy consumers.
    #[test]
    fn a_confident_classifier_refuses_a_scattered_hit() {
        assert_eq!(classify("chart", "cat"), Some(Match::Loose));
        assert_eq!(classify_confident("chart", "cat"), None);
        assert_eq!(classify_bytes_confident(b"chart", b"cat"), None);
        assert_eq!(classify_ci_confident("chart", "cat"), None);
        assert_eq!(
            classify_confident("libreoffice", "office"),
            Some(Match::Substring)
        );
        assert_eq!(
            classify_ci_confident("Ünïcode", "nïc"),
            Some(Match::Substring)
        );
        assert_eq!(classify_confident("x", ""), None);
        assert_eq!(classify_bytes_confident(b"x", b""), None);
    }

    #[test]
    fn push_lowered_matches_to_lowercase() {
        for s in ["", "ABC", "WayRun.zip", "Ünïcode", "报告.TXT", "İstanbul"] {
            let mut out = String::from("keep/");
            push_lowered(&mut out, s);
            assert_eq!(out, format!("keep/{}", s.to_lowercase()), "{s}");
        }
    }

    /// A cache holding `rows` as if it fetched them past [`BURST_TTL`] ago.
    fn stale_cache<T: Send + Sync + 'static>(rows: Vec<T>) -> FreshCache<T> {
        let cache: FreshCache<T> = FreshCache::new();
        let at = Instant::now() - BURST_TTL - Duration::from_millis(1);
        cache.lock().rows = Some((at, Arc::new(rows)));
        cache
    }

    async fn wait_refreshed<T: Send + Sync + 'static>(cache: &FreshCache<T>) {
        for _ in 0..1000 {
            if !cache.lock().refreshing {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("the background refetch never finished");
    }

    #[tokio::test]
    async fn a_fresh_entry_answers_without_fetching() {
        let cache: FreshCache<u32> = FreshCache::new();
        let first = cache.get(|| Some(vec![1])).await.unwrap();
        let second = cache
            .get(|| panic!("a fresh entry must not refetch"))
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn a_stale_entry_answers_while_the_refetch_runs_behind_it() {
        let cache: FreshCache<u32> = stale_cache(vec![1]);
        let (release, latch) = std::sync::mpsc::channel::<()>();
        let rows = cache
            .get(move || {
                latch.recv().ok()?;
                Some(vec![2])
            })
            .await;
        assert_eq!(*rows.unwrap(), vec![1], "the stale rows answer at once");

        // one refetch is in flight, so a second stale read must not start another
        let rows = cache.get(|| panic!("a refetch is already in flight")).await;
        assert_eq!(*rows.unwrap(), vec![1]);

        release.send(()).unwrap();
        wait_refreshed(&cache).await;
        let rows = cache.get(|| panic!("the refetch just revalidated")).await;
        assert_eq!(*rows.unwrap(), vec![2]);
    }

    #[tokio::test]
    async fn a_failed_refetch_keeps_the_stale_rows_and_retries() {
        let cache: FreshCache<u32> = stale_cache(vec![1]);
        let rows = cache.get(|| None).await;
        assert_eq!(*rows.unwrap(), vec![1]);
        wait_refreshed(&cache).await;

        // still stale, so the next read starts the refetch again
        let rows = cache.get(|| Some(vec![3])).await;
        assert_eq!(*rows.unwrap(), vec![1]);
        wait_refreshed(&cache).await;
        let rows = cache.get(|| panic!("the retry just landed")).await;
        assert_eq!(*rows.unwrap(), vec![3]);
    }

    #[tokio::test]
    async fn an_empty_fetch_is_a_cached_value() {
        let cache: FreshCache<u32> = FreshCache::new();
        let rows = cache.get(|| Some(vec![])).await.unwrap();
        assert!(rows.is_empty());

        // an empty list is a value, not a miss: it must not refetch
        let rows = cache
            .get(|| panic!("an empty list is not a miss"))
            .await
            .unwrap();
        assert!(rows.is_empty());
    }
}
