use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use crate::plugin::{Match, classify_bytes_confident, classify_ci_confident};
use crate::provider::file::index::format::{Index, push_dir_path};
use crate::provider::file::{entry_item, score_path, score_split_path};
use crate::provider::{SHOW_CAP, push_lowered};
use crate::wire::ResultItem;

/// Entries a scan must hold before it is split across threads; below it a
/// chunk's spawn costs more than the scan it would run.
pub(super) const PARALLEL_FLOOR: u32 = 8192;

/// The chunk count `n` entries split into: one per core, but never fewer than
/// [`PARALLEL_FLOOR`] entries per chunk, so a spawn buys more than it costs.
pub(super) fn threads_for(n: u32) -> u32 {
    std::thread::available_parallelism()
        .map_or(1, |num| num.get() as u32)
        .min(n.div_ceil(PARALLEL_FLOOR).max(1))
}

/// Run `chunk(start, end)` for each of `threads` disjoint ranges of `0..n` on
/// its own thread, and collect the results in range order.
pub(super) fn par_chunks<T: Send>(
    n: u32,
    threads: u32,
    chunk: impl Fn(u32, u32) -> T + Sync,
) -> Vec<T> {
    if threads <= 1 {
        return vec![chunk(0, n)];
    }
    let len = n.div_ceil(threads);
    std::thread::scope(|scope| {
        (0..threads)
            .map(|t| {
                let chunk = &chunk;
                let start = t * len;
                scope.spawn(move || chunk(start, (start + len).min(n)))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().expect("a scan thread panicked"))
            .collect()
    })
}

/// The best [`SHOW_CAP`] candidates by score, ties keeping the earlier position,
/// with no row built until the scan is over.
#[derive(Default)]
pub(super) struct Top {
    heap: BinaryHeap<(Reverse<u32>, u32)>,
}

impl Top {
    pub(super) fn offer(&mut self, score: u32, position: u32) {
        if score == 0 {
            return;
        }
        if self.heap.len() == SHOW_CAP {
            // the worst kept row: the lowest score, and among equals the latest
            // position, which a new offer can only lose to
            if score <= self.heap.peek().expect("full").0.0 {
                return;
            }
            self.heap.pop();
        }
        self.heap.push((Reverse(score), position));
    }

    /// The kept `(score, position)` pairs in heap order; the merge orders them.
    fn into_candidates(self) -> Vec<(u32, u32)> {
        self.heap
            .into_vec()
            .into_iter()
            .map(|(Reverse(score), position)| (score, position))
            .collect()
    }
}

/// One chunk of a scan: `init` builds the thread's scratch, `scan` scores every
/// entry of `start..end` into the chunk's [`Top`].
fn scan_chunk<S>(
    start: u32,
    end: u32,
    init: &impl Fn() -> S,
    scan: &impl Fn(&mut S, u32, &mut Top),
) -> Vec<(u32, u32)> {
    let mut scratch = init();
    let mut top = Top::default();
    for i in start..end {
        scan(&mut scratch, i, &mut top);
    }
    top.into_candidates()
}

/// Merges per-chunk candidates into the best [`SHOW_CAP`] by (score, earlier
/// position), the order `rank_results` keeps among equal scores.
fn merge_chunks(chunks: impl IntoIterator<Item = Vec<(u32, u32)>>) -> Vec<(u32, u32)> {
    let mut all: Vec<(u32, u32)> = chunks.into_iter().flatten().collect();
    all.sort_by_key(|&(score, position)| (Reverse(score), position));
    all.truncate(SHOW_CAP);
    all
}

/// The best [`SHOW_CAP`] of `0..n` as scored by `scan`, in chunks on separate
/// threads; merging by (score, earlier position) keeps it thread-count-proof.
fn scan_top<S>(
    n: u32,
    init: impl Fn() -> S + Sync,
    scan: impl Fn(&mut S, u32, &mut Top) + Sync,
) -> Vec<(u32, u32)> {
    let threads = threads_for(n);
    merge_chunks(par_chunks(n, threads, |start, end| {
        scan_chunk(start, end, &init, &scan)
    }))
}

/// Answer one query on an already-built index. `name_only` matches the name
/// alone (`f` with a plain query); otherwise the whole path is matched.
pub(super) fn search_in(
    index: &Index,
    query_lower: &str,
    want_dir: bool,
    name_only: bool,
) -> Vec<ResultItem> {
    let query_bytes = query_lower.as_bytes();
    let tier = |name: &[u8]| -> u32 {
        let kind = if name.is_ascii() {
            classify_bytes_confident(name, query_bytes)
        } else {
            classify_ci_confident(&String::from_utf8_lossy(name), query_lower)
        };
        kind.map_or(0, Match::weight)
    };

    let candidates = if want_dir && name_only {
        scan_top(
            index.dir_count,
            || (),
            |_, i, top| {
                let rec = index.dir(i);
                top.offer(tier(rec.name).saturating_sub(rec.depth), i);
            },
        )
    } else if want_dir {
        scan_top(
            index.dir_count,
            || (),
            |_, i, top| {
                let rec = index.dir(i);
                let score = score_path(
                    &String::from_utf8_lossy(rec.name),
                    rec.path_lower,
                    query_lower,
                    rec.depth as usize,
                );
                top.offer(score, i);
            },
        )
    } else if name_only {
        scan_top(
            index.file_count,
            || (),
            |_, i, top| {
                let rec = index.file(i);
                // a zero score is dropped, so only a hit pays for the depth lookup
                let score = tier(rec.name);
                if score > 0 {
                    top.offer(score.saturating_sub(index.dir(rec.dir).depth + 1), i);
                }
            },
        )
    } else {
        scan_top(index.file_count, String::new, |name_lower, i, top| {
            let rec = index.file(i);
            let dir = index.dir(rec.dir);
            let name = String::from_utf8_lossy(rec.name);
            name_lower.clear();
            push_lowered(name_lower, &name);
            let score = score_split_path(
                &name,
                dir.path_lower,
                name_lower,
                query_lower,
                dir.depth as usize + 1,
            );
            top.offer(score, i);
        })
    };

    let mut path = PathBuf::new();
    let mut chain = Vec::new();
    let mut scored = Vec::with_capacity(candidates.len());
    for (score, slot) in candidates {
        if want_dir {
            push_dir_path(index, slot, &mut path, &mut chain);
        } else {
            let rec = index.file(slot);
            push_dir_path(index, rec.dir, &mut path, &mut chain);
            path.push(OsStr::from_bytes(rec.name));
        }
        scored.push((score, entry_item(&path, want_dir)));
    }
    crate::provider::rank_results(scored, false, SHOW_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::file::index::MAX_ENTRIES;
    use crate::provider::file::index::test_support::{build_ok, parsed, summaries, write};

    #[test]
    fn an_index_round_trip_finds_a_path_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let deep = home.join("a/b/c/d/e");
        write(&deep.join("deep.txt"));

        let bytes = build_ok(home, MAX_ENTRIES);
        let index = parsed(&bytes, home);

        let files = search_in(&index, "deep.txt", false, true);
        assert_eq!(files.len(), 1, "{:?}", summaries(&files));
        assert_eq!(files[0].title, "deep.txt");
        assert_eq!(
            files[0].summary.as_deref(),
            Some(deep.join("deep.txt").to_string_lossy().as_ref())
        );
        // `d` asks for directories only
        assert!(search_in(&index, "deep.txt", true, true).is_empty());
    }

    #[test]
    fn a_deeper_hit_ranks_below_a_shallower_one() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("x.txt"));
        write(&home.join("a/b/x.txt"));

        let bytes = build_ok(home, MAX_ENTRIES);
        let index = parsed(&bytes, home);

        let files = search_in(&index, "x.txt", false, true);
        assert_eq!(files.len(), 2, "{:?}", summaries(&files));
        assert_eq!(
            files[0].summary.as_deref(),
            Some(home.join("x.txt").to_string_lossy().as_ref()),
            "the shallower hit leads"
        );
    }

    #[test]
    fn the_top_fifty_keep_the_highest_scores() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for i in 0..30 {
            write(&home.join(format!("p{i}/hit.txt")));
            write(&home.join(format!("q{i}/sub/hit.txt")));
        }

        let bytes = build_ok(home, MAX_ENTRIES);
        let index = parsed(&bytes, home);

        let files = search_in(&index, "hit.txt", false, true);
        assert_eq!(files.len(), SHOW_CAP);
        let deep = summaries(&files)
            .iter()
            .filter(|path| path.contains("/sub/"))
            .count();
        assert_eq!(
            deep, 20,
            "all 30 shallow hits survive, 20 deep ones fill it"
        );
    }

    #[test]
    fn a_chunked_scan_keeps_the_serial_top() {
        // wide enough to be chunked wherever the test runs; ties and zero
        // scores are what the merge can get wrong
        let n = 3 * PARALLEL_FLOOR;
        let score = |i: u32| (i * 37) % 130;
        let mut serial = Top::default();
        for i in 0..n {
            serial.offer(score(i), i);
        }
        let mut expected = serial.into_candidates();
        expected.sort_by_key(|&(score, position)| (Reverse(score), position));

        let scanned = scan_top(n, || (), |_, i, top| top.offer(score(i), i));
        assert_eq!(scanned, expected);
    }

    #[test]
    fn a_path_mode_query_matches_a_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("sub/deep/x.txt"));

        let bytes = build_ok(home, MAX_ENTRIES);
        let index = parsed(&bytes, home);

        assert_eq!(search_in(&index, "sub/deep", false, false).len(), 1);
        assert_eq!(search_in(&index, "sub/deep", true, false).len(), 1);
        // the name alone does not carry it
        assert!(search_in(&index, "sub/deep", false, true).is_empty());
    }

    #[test]
    fn a_deeply_nested_hit_reports_its_full_path() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let mut deep = home.to_path_buf();
        for _ in 0..300 {
            deep.push("d");
        }
        write(&deep.join("deep.txt"));

        let bytes = build_ok(home, MAX_ENTRIES);
        let index = parsed(&bytes, home);

        let files = search_in(&index, "deep.txt", false, true);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].summary.as_deref(),
            Some(deep.join("deep.txt").to_string_lossy().as_ref())
        );
    }

    #[test]
    fn a_non_ascii_name_is_tiered_like_the_text_tier() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        // U+212A KELVIN SIGN lowercases to ASCII `k`, so this name is only
        // reachable through the text tier
        write(&home.join("\u{212A}elvin.txt"));

        let bytes = build_ok(home, MAX_ENTRIES);
        let index = parsed(&bytes, home);

        assert_eq!(search_in(&index, "k", false, true).len(), 1);
    }

    #[test]
    fn a_non_utf8_home_still_indexes_and_searches() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(OsStr::from_bytes(b"h\xffme"));
        let sub = OsStr::from_bytes(b"d\xffir");
        std::fs::create_dir(&home).unwrap();
        write(&home.join(sub).join("x.txt"));

        let bytes = build_ok(&home, MAX_ENTRIES);
        let index = parsed(&bytes, &home);

        let files = search_in(&index, "x.txt", false, true);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].summary.as_deref(),
            Some(home.join(sub).join("x.txt").to_string_lossy().as_ref()),
            "the reported path keeps the raw bytes"
        );
        // a path-mode `d` scan rebuilds each path from the stored home, so the
        // query's replacement char, not the temp dir's suffix, carries the match
        assert_eq!(search_in(&index, "\u{fffd}ir", true, false).len(), 1);
    }
}
