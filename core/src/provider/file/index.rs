use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, UNIX_EPOCH};

use memmap2::Mmap;
use tokio::sync::Notify;
use walkdir::WalkDir;

use super::{entry_item, keep_name, score_path, score_split_path};
use crate::provider::{name_tier_ci, push_lowered};
use crate::wire::ResultItem;

/// The index cache's magic and format version; either mismatch rebuilds.
const MAGIC: [u8; 4] = *b"WRFI";
const FORMAT_VERSION: u32 = 3;
/// The header before the tables, all little-endian: magic, version, dir_count,
/// file_count, names_len, home_len, paths_len, exclude hash.
const HEADER: usize = 36;
const H_VERSION: usize = 4;
const H_DIRS: usize = H_VERSION + 4;
const H_FILES: usize = H_DIRS + 4;
const H_NAMES: usize = H_FILES + 4;
const H_HOME: usize = H_NAMES + 4;
const H_PATHS: usize = H_HOME + 4;
const H_EXCLUDE: usize = H_PATHS + 4;
/// The directory record: parent slot, name offset, name length, depth (32-bit
/// each), ns mtime (64-bit), lowercased path offset and length (32-bit each);
/// `DirOut::bytes` writes it, `Index::dir` reads it.
const DIR_REC: usize = 32;
const D_PARENT: usize = 0;
const D_NAME_OFF: usize = D_PARENT + 4;
const D_NAME_LEN: usize = D_NAME_OFF + 4;
const D_DEPTH: usize = D_NAME_LEN + 4;
const D_MTIME: usize = D_DEPTH + 4;
const D_PATH_OFF: usize = D_MTIME + 8;
const D_PATH_LEN: usize = D_PATH_OFF + 4;
/// The file record: directory slot, name offset, name length (32-bit each).
const FILE_REC: usize = 12;
const F_DIR: usize = 0;
const F_NAME_OFF: usize = F_DIR + 4;
const F_NAME_LEN: usize = F_NAME_OFF + 4;
/// Records kept; beyond it the build stops and reports that on stderr.
const MAX_ENTRIES: usize = 1_000_000;
/// How long a mapped index may sit unused before it is unmapped.
const IDLE: Duration = Duration::from_secs(30);
/// How long before a query re-checks the walk's freshness in the background.
const REFRESH_TTL: Duration = Duration::from_secs(300);
/// The only providers the index serves.
const OWNERS: [&str; 2] = ["file-search", "path-search"];
/// Rows kept per search, matching `rank_results`' show cap.
const TOP: usize = 50;
/// Entries a scan must hold before it is split across threads; below it a
/// chunk's spawn costs more than the scan it would run.
const PARALLEL_FLOOR: u32 = 8192;

/// The index bytes as a parse view; nothing is copied. The directory table is
/// DFS pre-order and the file table is sorted by `dir`, which queries rely on.
struct Index<'a> {
    bytes: &'a [u8],
    // bytes, not `str`: `$HOME` need not be UTF-8
    home: &'a [u8],
    dirs: usize,
    files: usize,
    names: usize,
    paths: usize,
    dir_count: u32,
    file_count: u32,
    exclude_hash: u64,
}

struct DirRec<'a> {
    parent: u32,
    depth: u32,
    mtime_ns: i64,
    name: &'a [u8],
    path_lower: &'a str,
}

struct FileRec<'a> {
    dir: u32,
    name: &'a [u8],
}

impl<'a> Index<'a> {
    /// `None` on any mismatch of magic, version, length or `$HOME`, so a caller
    /// rebuilds rather than reading another home's paths.
    fn parse(bytes: &'a [u8], home: &Path) -> Option<Self> {
        if bytes.len() < HEADER || bytes[0..4] != MAGIC {
            return None;
        }
        if u32_at(bytes, H_VERSION) != FORMAT_VERSION {
            return None;
        }
        let dir_count = u32_at(bytes, H_DIRS);
        let file_count = u32_at(bytes, H_FILES);
        let names_len = u32_at(bytes, H_NAMES);
        let home_len = u32_at(bytes, H_HOME) as usize;
        let paths_len = u32_at(bytes, H_PATHS) as usize;

        let home_end = HEADER.checked_add(home_len)?;
        let home_bytes = bytes.get(HEADER..home_end)?;
        if home_bytes != home.as_os_str().as_bytes() {
            return None;
        }
        let dirs = home_end;
        let files = dirs.checked_add((dir_count as usize).checked_mul(DIR_REC)?)?;
        let names = files.checked_add((file_count as usize).checked_mul(FILE_REC)?)?;
        let paths = names.checked_add(names_len as usize)?;
        if paths.checked_add(paths_len)? != bytes.len() {
            return None;
        }
        Some(Index {
            bytes,
            home: home_bytes,
            dirs,
            files,
            names,
            paths,
            dir_count,
            file_count,
            exclude_hash: u64_at(bytes, H_EXCLUDE),
        })
    }

    fn dir(&self, i: u32) -> DirRec<'a> {
        let rec = self.rec(self.dirs, i, DIR_REC);
        DirRec {
            parent: u32_at(rec, D_PARENT),
            depth: u32_at(rec, D_DEPTH),
            mtime_ns: i64::from_le_bytes(rec[D_MTIME..D_MTIME + 8].try_into().unwrap()),
            name: self.name(u32_at(rec, D_NAME_OFF), u32_at(rec, D_NAME_LEN)),
            path_lower: self.path_lower(u32_at(rec, D_PATH_OFF), u32_at(rec, D_PATH_LEN)),
        }
    }

    fn file(&self, i: u32) -> FileRec<'a> {
        let rec = self.rec(self.files, i, FILE_REC);
        FileRec {
            dir: u32_at(rec, F_DIR),
            name: self.name(u32_at(rec, F_NAME_OFF), u32_at(rec, F_NAME_LEN)),
        }
    }

    /// A record's bytes, or a zeroed one when the index is short: a corrupt
    /// cache degrades to an empty match, never a panic.
    fn rec(&self, base: usize, i: u32, width: usize) -> &'a [u8] {
        static ZERO: [u8; DIR_REC] = [0; DIR_REC];
        let start = base + i as usize * width;
        self.bytes.get(start..start + width).unwrap_or(&ZERO)
    }

    fn name(&self, off: u32, len: u32) -> &'a [u8] {
        let start = self.names.saturating_add(off as usize);
        let end = start.saturating_add(len as usize);
        self.bytes.get(start..end).unwrap_or(&[])
    }

    /// A stored lowercased path, or `""` for a corrupt span: like `rec`, it
    /// degrades to an empty haystack and never panics.
    fn path_lower(&self, off: u32, len: u32) -> &'a str {
        let start = self.paths.saturating_add(off as usize);
        let end = start.saturating_add(len as usize);
        std::str::from_utf8(self.bytes.get(start..end).unwrap_or_default()).unwrap_or_default()
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// `$HOME`, `$XDG_CACHE_HOME/wayrun/file-index.bin`.
fn path() -> Option<PathBuf> {
    Some(crate::system::fs::cache_dir()?.join("file-index.bin"))
}

fn load() -> Option<Mmap> {
    let file = std::fs::File::open(path()?).ok()?;
    // SAFETY: the cache is only ever replaced by an atomic rename, never
    // written in place or truncated, so the mapping stays valid.
    unsafe { Mmap::map(&file).ok() }
}

fn mtime_ns(path: &Path) -> Option<i64> {
    let time = std::fs::metadata(path).ok()?.modified().ok()?;
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => Some(since.as_nanos() as i64),
        Err(before) => Some(-(before.duration().as_nanos() as i64)),
    }
}

struct DirOut {
    parent: u32,
    name_off: u32,
    name_len: u32,
    depth: u32,
    mtime_ns: i64,
    path_off: u32,
    path_len: u32,
}

impl DirOut {
    /// The record's bytes; `Index::dir` reads the same fields back at the `D_*` offsets.
    fn bytes(&self) -> [u8; DIR_REC] {
        let mut rec = [0; DIR_REC];
        rec[D_PARENT..D_PARENT + 4].copy_from_slice(&self.parent.to_le_bytes());
        rec[D_NAME_OFF..D_NAME_OFF + 4].copy_from_slice(&self.name_off.to_le_bytes());
        rec[D_NAME_LEN..D_NAME_LEN + 4].copy_from_slice(&self.name_len.to_le_bytes());
        rec[D_DEPTH..D_DEPTH + 4].copy_from_slice(&self.depth.to_le_bytes());
        rec[D_MTIME..D_MTIME + 8].copy_from_slice(&self.mtime_ns.to_le_bytes());
        rec[D_PATH_OFF..D_PATH_OFF + 4].copy_from_slice(&self.path_off.to_le_bytes());
        rec[D_PATH_LEN..D_PATH_LEN + 4].copy_from_slice(&self.path_len.to_le_bytes());
        rec
    }
}

struct FileOut {
    dir: u32,
    name_off: u32,
    name_len: u32,
}

impl FileOut {
    /// The record's bytes; `Index::file` reads the same fields back at the `F_*` offsets.
    fn bytes(&self) -> [u8; FILE_REC] {
        let mut rec = [0; FILE_REC];
        rec[F_DIR..F_DIR + 4].copy_from_slice(&self.dir.to_le_bytes());
        rec[F_NAME_OFF..F_NAME_OFF + 4].copy_from_slice(&self.name_off.to_le_bytes());
        rec[F_NAME_LEN..F_NAME_LEN + 4].copy_from_slice(&self.name_len.to_le_bytes());
        rec
    }
}

fn push_name(names: &mut Vec<u8>, bytes: &[u8]) -> (u32, u32) {
    let off = names.len() as u32;
    names.extend_from_slice(bytes);
    (off, bytes.len() as u32)
}

/// The header's bytes; `parse` reads the same fields back at the `H_*` offsets.
fn header_bytes(
    dir_count: u32,
    file_count: u32,
    names_len: u32,
    home_len: u32,
    paths_len: u32,
    exclude_hash: u64,
) -> [u8; HEADER] {
    let mut head = [0; HEADER];
    head[0..4].copy_from_slice(&MAGIC);
    head[H_VERSION..H_VERSION + 4].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    head[H_DIRS..H_DIRS + 4].copy_from_slice(&dir_count.to_le_bytes());
    head[H_FILES..H_FILES + 4].copy_from_slice(&file_count.to_le_bytes());
    head[H_NAMES..H_NAMES + 4].copy_from_slice(&names_len.to_le_bytes());
    head[H_HOME..H_HOME + 4].copy_from_slice(&home_len.to_le_bytes());
    head[H_PATHS..H_PATHS + 4].copy_from_slice(&paths_len.to_le_bytes());
    head[H_EXCLUDE..H_EXCLUDE + 8].copy_from_slice(&exclude_hash.to_le_bytes());
    head
}

/// A stable hash of the excluded names, length-prefixed so `["ab"]` and
/// `["a", "b"]` cannot hash alike; the index records it to rebuild on a change.
fn exclude_hash(names: &[String]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for name in names {
        hash ^= name.len() as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Walk `home` into the index image; `cap` bounds dirs and files together, and
/// `exclude` holds the names the walk never enters. `None` when a partial walk
/// must not be persisted as an authoritative index.
fn build(home: &Path, cap: usize, exclude: &[String]) -> Option<Vec<u8>> {
    if !home.is_dir() {
        return None;
    }
    let home_bytes = home.as_os_str().as_bytes();
    let root_name = home.file_name().map(OsStr::as_bytes).unwrap_or(home_bytes);
    let mut names: Vec<u8> = Vec::new();
    let (name_off, name_len) = push_name(&mut names, root_name);
    let mut paths: Vec<u8> = Vec::new();
    let mut lower = String::new();
    // lossy only for the haystack: a UTF-8 query can never carry replaced bytes
    push_lowered(&mut lower, &String::from_utf8_lossy(home_bytes));
    let (path_off, path_len) = push_name(&mut paths, lower.as_bytes());
    let mut dirs = vec![DirOut {
        parent: u32::MAX,
        name_off,
        name_len,
        depth: 0,
        mtime_ns: mtime_ns(home)?,
        path_off,
        path_len,
    }];
    let mut files: Vec<FileOut> = Vec::new();

    // One directory index per depth along the current path, so a child's parent
    // is the entry its own depth names; the pre-recorded root is depth 0.
    let mut stack: Vec<u32> = vec![0];
    // the lowercased path span of the directory each depth names
    let mut spans: Vec<(u32, u32)> = vec![(path_off, path_len)];
    let walker = WalkDir::new(home)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || keep_name(e.file_name(), exclude));
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            // an unlistable `$HOME` would index as empty; deeper errors drop the subtree
            Err(err) if err.depth() == 0 => return None,
            Err(_) => continue,
        };
        if dirs.len() + files.len() >= cap {
            break;
        }
        let depth = entry.depth() as u32;
        stack.truncate(depth as usize);
        spans.truncate(depth as usize);
        let parent = *stack.last().unwrap_or(&0);
        let file_type = entry.file_type();
        if file_type.is_dir() {
            let (name_off, name_len) = push_name(&mut names, entry.file_name().as_bytes());
            // the mtime must be read before walkdir reads the contents below
            let mtime_ns = mtime_ns(entry.path())?;
            // the parent's stored path is extended, so lowercasing only the name
            // leaves the same bytes as lowercasing the joined path
            let (parent_off, parent_len) = *spans.last().unwrap_or(&(0, 0));
            lower.clear();
            lower.push_str(
                std::str::from_utf8(
                    &paths[parent_off as usize..(parent_off + parent_len) as usize],
                )
                .unwrap_or(""),
            );
            lower.push('/');
            push_lowered(
                &mut lower,
                &String::from_utf8_lossy(entry.file_name().as_bytes()),
            );
            let (path_off, path_len) = push_name(&mut paths, lower.as_bytes());
            stack.push(dirs.len() as u32);
            spans.push((path_off, path_len));
            dirs.push(DirOut {
                parent,
                name_off,
                name_len,
                depth,
                mtime_ns,
                path_off,
                path_len,
            });
        } else if file_type.is_file() {
            let (name_off, name_len) = push_name(&mut names, entry.file_name().as_bytes());
            files.push(FileOut {
                dir: parent,
                name_off,
                name_len,
            });
        }
    }
    files.sort_by_key(|rec| rec.dir);

    let mut out = Vec::with_capacity(
        HEADER
            + home_bytes.len()
            + dirs.len() * DIR_REC
            + files.len() * FILE_REC
            + names.len()
            + paths.len(),
    );
    out.extend_from_slice(&header_bytes(
        dirs.len() as u32,
        files.len() as u32,
        names.len() as u32,
        home_bytes.len() as u32,
        paths.len() as u32,
        exclude_hash(exclude),
    ));
    out.extend_from_slice(home_bytes);
    for d in &dirs {
        out.extend_from_slice(&d.bytes());
    }
    for f in &files {
        out.extend_from_slice(&f.bytes());
    }
    out.extend_from_slice(&names);
    out.extend_from_slice(&paths);
    Some(out)
}

/// Directory `i`'s absolute path; the root uses the stored `$HOME`. `chain` is
/// the caller's scratch, and `dir_count` bounds a corrupt parent cycle.
fn push_dir_path(index: &Index, i: u32, out: &mut PathBuf, chain: &mut Vec<u32>) {
    out.clear();
    out.push(OsStr::from_bytes(index.home));
    if i == 0 {
        return;
    }
    chain.clear();
    let mut cur = i;
    while cur != 0 && chain.len() < index.dir_count as usize {
        let rec = index.dir(cur);
        chain.push(cur);
        if rec.parent == u32::MAX {
            break;
        }
        cur = rec.parent;
    }
    for &dir in chain.iter().rev() {
        out.push(OsStr::from_bytes(index.dir(dir).name));
    }
}

/// The layout the queries rely on: the root at slot 0, every other directory
/// exactly one level below an earlier parent; a broken chain cannot be walked safely.
fn layout_ok(index: &Index) -> bool {
    (0..index.dir_count).all(|i| {
        let rec = index.dir(i);
        if i == 0 {
            rec.parent == u32::MAX && rec.depth == 0
        } else {
            rec.parent < i && index.dir(rec.parent).depth + 1 == rec.depth
        }
    })
}

/// A change inside a directory updates its mtime and a new directory its
/// parent's, so one mtime per recorded directory covers all of them.
fn fresh(index: &Index) -> bool {
    if index.dir_count == 0 {
        return false;
    }
    let mut path = PathBuf::new();
    let mut chain = Vec::new();
    for i in 0..index.dir_count {
        let rec = index.dir(i);
        push_dir_path(index, i, &mut path, &mut chain);
        if mtime_ns(&path) != Some(rec.mtime_ns) {
            return false;
        }
    }
    true
}

/// The best [`TOP`] candidates by score, ties keeping the earlier position,
/// with no row built until the scan is over.
#[derive(Default)]
struct Top {
    heap: BinaryHeap<(Reverse<u32>, u32)>,
}

impl Top {
    fn offer(&mut self, score: u32, position: u32) {
        if score == 0 {
            return;
        }
        if self.heap.len() == TOP {
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

/// Merges per-chunk candidates into the best [`TOP`] by (score, earlier
/// position), the order `rank_results` keeps among equal scores.
fn merge_chunks(chunks: impl IntoIterator<Item = Vec<(u32, u32)>>) -> Vec<(u32, u32)> {
    let mut all: Vec<(u32, u32)> = chunks.into_iter().flatten().collect();
    all.sort_by_key(|&(score, position)| (Reverse(score), position));
    all.truncate(TOP);
    all
}

/// The best [`TOP`] of `0..n` as scored by `scan`, in chunks on separate
/// threads; merging per-chunk tops by (score, earlier position) keeps the
/// answer independent of the thread and chunk count.
fn scan_top<S>(
    n: u32,
    init: impl Fn() -> S + Sync,
    scan: impl Fn(&mut S, u32, &mut Top) + Sync,
) -> Vec<(u32, u32)> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |num| num.get() as u32)
        .min(n.div_ceil(PARALLEL_FLOOR).max(1));
    let chunk = n.div_ceil(threads);
    let chunks = if threads == 1 {
        vec![scan_chunk(0, n, &init, &scan)]
    } else {
        std::thread::scope(|scope| {
            (0..threads)
                .map(|t| {
                    let (init, scan) = (&init, &scan);
                    let start = t * chunk;
                    scope.spawn(move || scan_chunk(start, (start + chunk).min(n), init, scan))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().expect("a scan thread panicked"))
                .collect()
        })
    };
    merge_chunks(chunks)
}

/// Answer one query on an already-built index. `name_only` matches the name
/// alone (`f` with a plain query); otherwise the whole path is matched.
fn search_in(index: &Index, query_lower: &str, want_dir: bool, name_only: bool) -> Vec<ResultItem> {
    let query_bytes = query_lower.as_bytes();
    let tier = |name: &[u8]| -> u32 {
        if name.is_ascii() {
            crate::provider::name_tier_bytes(name, query_bytes)
        } else {
            name_tier_ci(&String::from_utf8_lossy(name), query_lower)
        }
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
                let depth = index.dir(rec.dir).depth + 1;
                top.offer(tier(rec.name).saturating_sub(depth), i);
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
    crate::provider::rank_results(scored, false, TOP)
}

struct State {
    map: Option<Arc<Mmap>>,
    last_used: Option<Instant>,
    last_check: Option<Instant>,
    /// Set by [`discard`] once it has unlinked the cache and cleared before any
    /// write: while it holds, no cache file is on disk, so unlinks can be skipped.
    clean: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    map: None,
    last_used: None,
    last_check: None,
    // a previous process may have left cache files behind
    clean: false,
});
static BUILDING: AtomicBool = AtomicBool::new(false);
static REAPER: OnceLock<()> = OnceLock::new();
/// Wakes the reaper when an index is stored; it parks on this while nothing is
/// mapped, so an unmapped index costs no wakeups.
static WAKE: Notify = Notify::const_new();

fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Answer a query from the mapped index, or `None` when there is none so the
/// caller falls back to the depth-3 walk.
pub fn search(
    home: &Path,
    query_lower: &str,
    want_dir: bool,
    name_only: bool,
) -> Option<Vec<ResultItem>> {
    let map = {
        let mut state = lock();
        let map = state.map.clone()?;
        state.last_used = Some(Instant::now());
        map
    };
    let index = Index::parse(&map[..], home)?;
    Some(search_in(&index, query_lower, want_dir, name_only))
}

/// Called by every `f`/`d` search: drops the cache when the setting is off and
/// re-checks freshness in the background at most once per [`REFRESH_TTL`].
pub async fn ensure() {
    if !crate::config::get().files.index {
        discard();
        return;
    }
    if REAPER.set(()).is_ok() {
        tokio::spawn(reaper());
    }
    let Ok(home) = crate::system::fs::get_home() else {
        return;
    };
    {
        let mut state = lock();
        let now = Instant::now();
        if state
            .last_check
            .is_some_and(|last| now.duration_since(last) < REFRESH_TTL)
        {
            return;
        }
        // remember the attempt, not the success, so a failure waits a TTL too
        state.last_check = Some(now);
    }
    tokio::task::spawn_blocking(move || refresh(&home));
}

pub fn owns(plugin_id: &str) -> bool {
    OWNERS.contains(&plugin_id)
}

/// Keep the cache only while the index has an owner; the registry calls this on
/// every rebuild, including the plugins.toml watcher's.
pub fn sync_enabled(enabled: bool) {
    if !enabled {
        discard();
    }
}

/// Re-check the cache on the next `f`/`d` search, so an edited `exclude` list
/// applies without waiting out [`REFRESH_TTL`]; the registry calls this on every
/// config reload.
pub fn settings_changed() {
    lock().last_check = None;
}

/// Hand a mapped index to the queries; the idle clock starts now, so a map that
/// is stored but never searched still gets unmapped.
fn store(map: Mmap) {
    {
        let mut state = lock();
        state.map = Some(Arc::new(map));
        state.last_used = Some(Instant::now());
    }
    // the reaper may be parked with nothing to watch
    WAKE.notify_one();
}

/// Unmap and delete the cache; idempotent, and the deletion itself runs once
/// per write — with indexing off this is called on every search.
pub fn discard() {
    let already_clean = {
        let mut state = lock();
        state.map = None;
        state.last_used = None;
        // re-enabling must rebuild at once, not wait out the refresh TTL
        state.last_check = None;
        let clean = state.clean;
        state.clean = true;
        clean
    };
    if already_clean {
        return;
    }
    let Some(path) = path() else {
        return;
    };
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(PathBuf::from(tmp));
}

/// Unmap an index nothing has used for [`IDLE`], so an idle launcher holds no
/// index pages; with nothing mapped the reaper parks until the next [`store`].
async fn reaper() {
    loop {
        let wait = {
            let state = lock();
            let until_due = state
                .last_used
                .map_or(IDLE, |used| IDLE.saturating_sub(used.elapsed()));
            // with no map there is nothing to unmap: park until a store, not every IDLE
            state.map.as_ref().map(|_| until_due)
        };
        let Some(wait) = wait else {
            WAKE.notified().await;
            continue;
        };
        tokio::select! {
            _ = tokio::time::sleep(wait.max(Duration::from_secs(1))) => {}
            // a store restarted the idle clock; compute the wait from it
            _ = WAKE.notified() => continue,
        }
        let mut state = lock();
        if state.map.is_none() {
            continue;
        }
        if state.last_used.is_some_and(|used| used.elapsed() >= IDLE) {
            state.map = None;
            state.last_used = None;
            // the map is gone, so the next search must be allowed to remap at
            // once instead of falling back to the walk for a whole TTL
            state.last_check = None;
            // drop the guard first so a search never queues behind the arena walk
            drop(state);
            trim_allocator();
        }
    }
}

/// Release the allocator's free pages back to the kernel. Only glibc's
/// `malloc_trim` does this; other targets leave it to their allocator.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim_allocator() {
    // SAFETY: `malloc_trim` is a plain libc allocator call with no
    // preconditions and no memory effects beyond returning free pages.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim_allocator() {}

/// Reset [`BUILDING`] on every exit path out of [`refresh`].
struct BuildGuard;

impl Drop for BuildGuard {
    fn drop(&mut self) {
        BUILDING.store(false, Ordering::Release);
    }
}

fn refresh(home: &Path) {
    if BUILDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let _guard = BuildGuard;

    let exclude = crate::config::get().files.exclude;
    if let Some(map) = load() {
        let usable = Index::parse(&map[..], home).is_some_and(|index| {
            layout_ok(&index) && fresh(&index) && index.exclude_hash == exclude_hash(&exclude)
        });
        if usable {
            let dirs = u32_at(&map[..], 8);
            let files = u32_at(&map[..], 12);
            let mb = map.len() / 1_000_000;
            store(map);
            // queries answer from the walk until the index is mapped, so the
            // load reports like the build and tools can wait on either line
            eprintln!("wayrun: file index {dirs} dirs, {files} files, {mb}MB (loaded)");
            return;
        }
    }

    let started = Instant::now();
    let Some(bytes) = build(home, MAX_ENTRIES, &exclude) else {
        eprintln!("wayrun: file index build skipped: the walk was not fully readable");
        return;
    };
    if !crate::config::get().files.index {
        return; // indexing was turned off while this build ran
    }
    let Some(path) = path() else {
        return;
    };
    // a cache file, or its `.tmp`, may exist from here on
    lock().clean = false;
    if let Err(err) = crate::system::fs::write_atomic(&path, &bytes) {
        eprintln!("wayrun: file index write failed: {err}");
        return;
    }
    if let Some(map) = load() {
        // the map only reaches the queries if the image it came from can be
        // walked: `store` is the one gate for that
        if Index::parse(&map[..], home).is_some_and(|index| layout_ok(&index)) {
            store(map);
        }
    }

    let dir_count = u32_at(&bytes, 8);
    let file_count = u32_at(&bytes, 12);
    let capped = dir_count as usize + file_count as usize >= MAX_ENTRIES;
    eprintln!(
        "wayrun: file index {dir_count} dirs, {file_count} files, {}MB, {:.1}s{}",
        bytes.len() / 1_000_000,
        started.elapsed().as_secs_f64(),
        if capped { " (capped)" } else { "" }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, "x").unwrap();
    }

    fn parsed<'a>(bytes: &'a [u8], home: &Path) -> Index<'a> {
        let index = Index::parse(bytes, home).expect("a freshly built index parses");
        assert!(layout_ok(&index), "a freshly built index keeps the layout");
        index
    }

    /// The names a stock install skips: the shipped template's list, the one
    /// place those defaults are written down.
    fn shipped_exclude() -> Vec<String> {
        toml::from_str::<crate::config::Config>(crate::config::DEFAULT_TEMPLATE)
            .expect("the template parses")
            .files
            .exclude
    }

    /// A stock install's behaviour: the shipped `[files] exclude` list.
    fn build_ok(home: &Path, cap: usize) -> Vec<u8> {
        build(home, cap, &shipped_exclude()).expect("a readable home builds")
    }

    fn summaries(items: &[ResultItem]) -> Vec<String> {
        items
            .iter()
            .map(|item| item.summary.clone().unwrap_or_default())
            .collect()
    }

    /// The golden bytes below pin the on-disk layout: changing any `H_*`, `D_*`
    /// or `F_*` offset must be a deliberate format change (`FORMAT_VERSION`).
    #[test]
    fn the_record_layouts_are_the_documented_bytes() {
        let head = header_bytes(1, 2, 3, 4, 5, 6);
        assert_eq!(head.len(), HEADER);
        assert_eq!(&head[0..8], b"WRFI\x03\0\0\0");
        assert_eq!(u32_at(&head, H_DIRS), 1);
        assert_eq!(u32_at(&head, H_FILES), 2);
        assert_eq!(u32_at(&head, H_NAMES), 3);
        assert_eq!(u32_at(&head, H_HOME), 4);
        assert_eq!(u32_at(&head, H_PATHS), 5);
        assert_eq!(u64_at(&head, H_EXCLUDE), 6);

        let dir = DirOut {
            parent: 1,
            name_off: 2,
            name_len: 3,
            depth: 4,
            mtime_ns: 5,
            path_off: 6,
            path_len: 7,
        };
        assert_eq!(
            dir.bytes(),
            [
                1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0,
                7, 0, 0, 0
            ]
        );

        let file = FileOut {
            dir: 1,
            name_off: 2,
            name_len: 3,
        };
        assert_eq!(file.bytes(), [1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]);
    }

    /// The path-mode scans read the stored lowercased path instead of rebuilding
    /// one per query, so what the build stores is what matching depends on.
    #[test]
    fn a_directories_lowercased_path_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("MiXeD/CaSe/hit.txt"));

        let bytes = build_ok(home, MAX_ENTRIES);
        let index = parsed(&bytes, home);

        let root = index.dir(0);
        let mut expect = String::new();
        push_lowered(&mut expect, &home.to_string_lossy());
        assert_eq!(root.path_lower, expect, "the root keeps the home path");

        let mixed = index.dir(1);
        assert_eq!(mixed.name, b"MiXeD");
        expect.push_str("/mixed");
        assert_eq!(mixed.path_lower, expect);

        let case = index.dir(2);
        assert_eq!(case.name, b"CaSe");
        expect.push_str("/case");
        assert_eq!(case.path_lower, expect);
        // Only the stored path can satisfy "mixed/case": the name and parent cannot.
        assert_eq!(search_in(&index, "mixed/case", true, false).len(), 1);
    }

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
        assert_eq!(files.len(), TOP);
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

    #[test]
    fn the_build_skips_dot_dirs_and_build_caches() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for pruned in [".hidden", "node_modules", "target", "__pycache__"] {
            write(&home.join(pruned).join("pruned.txt"));
        }
        write(&home.join("Desktop/a/b/c.txt"));

        let bytes = build_ok(home, MAX_ENTRIES);
        let index = parsed(&bytes, home);

        for pruned in [
            "pruned.txt",
            ".hidden",
            "node_modules",
            "target",
            "__pycache__",
        ] {
            assert!(
                search_in(&index, pruned, false, true).is_empty(),
                "{pruned} slipped into the index"
            );
            assert!(search_in(&index, pruned, true, true).is_empty(), "{pruned}");
        }
        // no root is special to the index, so a depth-3 hit under Desktop is in
        assert_eq!(search_in(&index, "c.txt", false, true).len(), 1);
    }

    #[test]
    fn a_configured_list_replaces_the_shipped_caches_and_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("vendor/kept.txt"));
        write(&home.join("node_modules/pruned.txt"));

        let custom = ["vendor".to_string()];
        let bytes = build(home, MAX_ENTRIES, &custom).unwrap();
        let index = parsed(&bytes, home);

        assert!(search_in(&index, "kept.txt", false, true).is_empty());
        // the list is the whole rule: an unlisted build cache is walked again
        assert_eq!(search_in(&index, "pruned.txt", false, true).len(), 1);
        // the header pins the list, so a config edit rebuilds instead of reusing
        assert_eq!(index.exclude_hash, exclude_hash(&custom));
        assert_ne!(index.exclude_hash, exclude_hash(&shipped_exclude()));
    }

    #[test]
    fn the_exclude_hash_needs_the_length_prefix_and_is_stable() {
        let joined = exclude_hash(&["ab".to_string()]);
        let split = exclude_hash(&["a".to_string(), "b".to_string()]);
        assert_ne!(joined, split, "a length prefix keeps the two apart");
        assert_eq!(joined, exclude_hash(&["ab".to_string()]));
    }

    #[test]
    fn a_changed_directory_makes_the_index_stale() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("a/keep.txt"));

        let bytes = build_ok(home, MAX_ENTRIES);
        let index = parsed(&bytes, home);
        assert!(fresh(&index));

        write(&home.join("a/added.txt"));
        assert!(!fresh(&index), "an added file changes the parent's mtime");
    }

    #[test]
    fn a_foreign_or_truncated_index_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("a/x.txt"));
        let bytes = build_ok(home, MAX_ENTRIES);
        assert!(Index::parse(&bytes, home).is_some());

        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert!(Index::parse(&bad_magic, home).is_none());

        let mut bad_version = bytes.clone();
        bad_version[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert!(Index::parse(&bad_version, home).is_none());

        assert!(Index::parse(&bytes, Path::new("/nonexistent-home")).is_none());
        assert!(Index::parse(&bytes[..bytes.len() - 1], home).is_none());
    }

    #[test]
    fn an_entry_cap_stops_a_runaway_build() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("a/b/c/d/e/deep.txt"));

        let bytes = build_ok(home, 3);
        let index = parsed(&bytes, home);
        assert_eq!(index.dir_count as usize + index.file_count as usize, 3);
    }

    #[test]
    fn an_unusable_home_is_never_written_out_as_empty() {
        assert!(
            build(Path::new("/nonexistent-home-of-wayrun"), MAX_ENTRIES, &[]).is_none(),
            "an unstatable root must not become an authoritative empty index"
        );

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("not-a-dir");
        write(&home);
        assert!(
            build(&home, MAX_ENTRIES, &[]).is_none(),
            "a root that is not a directory can never be walked"
        );

        // a directory that stats but cannot be listed: root ignores the mode
        // bits, so the case cannot be staged there
        let chmod = |mode| {
            std::fs::set_permissions(&home, std::os::unix::fs::PermissionsExt::from_mode(mode))
                .unwrap()
        };
        chmod(0o000);
        if std::fs::read_dir(&home).is_ok() {
            chmod(0o755);
            return;
        }
        let bytes = build(&home, MAX_ENTRIES, &[]);
        chmod(0o755);
        assert!(
            bytes.is_none(),
            "an unlistable root must not become an authoritative empty index"
        );
    }

    #[test]
    fn an_index_whose_dir_layout_breaks_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("a/b/x.txt"));
        // dir 1 is `a`: its record sits past the header, the home and the root
        let rec = HEADER + home.as_os_str().as_bytes().len() + DIR_REC;

        let mut bytes = build_ok(home, MAX_ENTRIES);
        bytes[rec + 12..rec + 16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            !layout_ok(&Index::parse(&bytes, home).unwrap()),
            "a depth no path can sit under must be rejected"
        );

        let mut bytes = build_ok(home, MAX_ENTRIES);
        bytes[rec..rec + 4].copy_from_slice(&1u32.to_le_bytes());
        assert!(
            !layout_ok(&Index::parse(&bytes, home).unwrap()),
            "a parent that is not an earlier slot must be rejected"
        );
    }

    #[test]
    fn the_index_owns_exactly_the_two_file_providers() {
        assert!(owns("file-search"));
        assert!(owns("path-search"));
        assert!(!owns("app-search"));
    }
}
