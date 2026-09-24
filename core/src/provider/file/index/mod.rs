mod build;
mod format;
mod scan;
#[cfg(test)]
mod test_support;
mod update;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, UNIX_EPOCH};

use memmap2::Mmap;
use tokio::sync::Notify;

use self::build::build;
use self::format::{Index, exclude_hash, layout_ok, u32_at};
use self::scan::search_in;
use self::update::{changed_dirs, patch, worth_patching};
use crate::wire::ResultItem;

/// Records kept; beyond it the build stops and reports that on stderr.
pub(super) const MAX_ENTRIES: usize = 1_000_000;
/// How long a mapped index may sit unused before it is unmapped; above
/// [`REFRESH_TTL`] so an idle reopen answers from the map, not the walk.
const IDLE: Duration = Duration::from_secs(120);
/// How long before a query re-checks the walk's freshness in the background.
const REFRESH_TTL: Duration = Duration::from_secs(60);
/// The only providers the index serves.
const OWNERS: [&str; 2] = ["file-search", "path-search"];

struct State {
    /// The mapped image and when a query last touched it. One field, so a map
    /// can never be held without its idle clock.
    live: Option<(Arc<Mmap>, Instant)>,
    last_check: Option<Instant>,
    /// Set by [`discard`] once it has unlinked the cache and cleared before any
    /// write: while it holds, no cache file is on disk, so unlinks can be skipped.
    clean: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    live: None,
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
        let (map, used) = state.live.as_mut()?;
        *used = Instant::now();
        Arc::clone(map)
    };
    let index = Index::parse(&map[..], home)?;
    Some(search_in(&index, query_lower, want_dir, name_only))
}

/// Called by every `f`/`d` search: drops the cache when the setting is off, else
/// puts an index in place for it unless the sweep finds the home changed.
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
    let (recheck, mapped) = {
        let mut state = lock();
        let now = Instant::now();
        let checked = state
            .last_check
            .is_some_and(|last| now.duration_since(last) < REFRESH_TTL);
        if checked && state.live.is_some() {
            return;
        }
        // remember the attempt, not the success, so a failure waits a TTL too
        if !checked {
            state.last_check = Some(now);
        }
        (!checked, state.live.is_some())
    };
    if !recheck {
        // no sweep is due, so this is a plain remap: bounded work this search
        // can wait for instead of answering from the walk
        let _ = tokio::task::spawn_blocking(move || remap(&home)).await;
    } else if mapped {
        // the mapped index answers this search while the sweep re-checks it
        tokio::task::spawn_blocking(move || refresh(&home));
    } else {
        // nothing to answer from: the sweep's verdict decides, and only a
        // changed home leaves the walk to answer while `refresh` writes behind it
        let fallback = home.clone();
        let loaded = tokio::task::spawn_blocking(move || validate(&home))
            .await
            .unwrap_or(false);
        if !loaded {
            tokio::task::spawn_blocking(move || refresh(&fallback));
        }
    }
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

/// Drop the refresh clock, so an edited `exclude` list applies on the next
/// `f`/`d` search instead of waiting out [`REFRESH_TTL`]; the registry calls it.
pub fn settings_changed() {
    lock().last_check = None;
}

/// Hand a mapped index to the queries; the idle clock starts now, so a map that
/// is stored but never searched still gets unmapped.
fn store(map: Mmap) {
    {
        let mut state = lock();
        state.live = Some((Arc::new(map), Instant::now()));
    }
    // the reaper may be parked with nothing to watch
    WAKE.notify_one();
}

/// Unmap and delete the cache; idempotent, and the deletion itself runs once
/// per write — with indexing off this is called on every search.
pub fn discard() {
    let already_clean = {
        let mut state = lock();
        state.live = None;
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
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(crate::system::fs::tmp_path(&path));
}

/// Unmap an index nothing has used for [`IDLE`], so an idle launcher holds no
/// index pages; with nothing mapped the reaper parks until the next [`store`].
async fn reaper() {
    loop {
        let wait = {
            let state = lock();
            // with no map there is nothing to unmap: park until a store, not every IDLE
            state
                .live
                .as_ref()
                .map(|(_, used)| IDLE.saturating_sub(used.elapsed()))
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
        let Some((_, used)) = state.live.as_ref() else {
            continue;
        };
        if used.elapsed() >= IDLE {
            state.live = None;
            // `last_check` stands, or the next search would sweep again
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

/// The cached image for `home` plus the directories whose mtime moved with it;
/// `None` when it cannot serve queries. Only a due [`REFRESH_TTL`] check sweeps.
fn load_swept(home: &Path, exclude: &[String], sweep: bool) -> Option<(Mmap, Vec<u32>)> {
    let map = load()?;
    let index = Index::parse(&map[..], home)?;
    if !layout_ok(&index) || index.exclude_hash != exclude_hash(exclude) {
        return None;
    }
    let moved = if sweep {
        changed_dirs(&index)
    } else {
        Vec::new()
    };
    Some((map, moved))
}

/// Hand a loaded image to the queries and report it the way a build reports
/// itself, so tools waiting on either line are not left hanging.
fn store_loaded(map: Mmap) {
    let dirs = u32_at(&map[..], 8);
    let files = u32_at(&map[..], 12);
    let mb = map.len() / 1_000_000;
    store(map);
    eprintln!("wayrun: file index {dirs} dirs, {files} files, {mb}MB (loaded)");
}

/// Map the cache for a search whose sweep is not due: a cache that will not load
/// leaves the walk to answer this search and the next due sweep to repair it.
fn remap(home: &Path) {
    if BUILDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let _guard = BuildGuard;

    let exclude = crate::config::get().files.exclude;
    if let Some((map, _)) = load_swept(home, &exclude, false) {
        store_loaded(map);
    }
}

/// Run the due sweep for a search with nothing mapped: `true` when every
/// directory still matches, so the index answers; `false` leaves it to `refresh`.
fn validate(home: &Path) -> bool {
    if BUILDING.swap(true, Ordering::AcqRel) {
        return false;
    }
    let _guard = BuildGuard;

    let exclude = crate::config::get().files.exclude;
    match load_swept(home, &exclude, true) {
        // moved listings need the patch, which belongs off this search's path
        Some((_, moved)) if !moved.is_empty() => false,
        Some((map, _)) => {
            store_loaded(map);
            true
        }
        None => false,
    }
}

/// Sweep the cache and write the next image: a patch of the moved listings when
/// that is cheaper than a walk, a full walk otherwise.
fn refresh(home: &Path) {
    if BUILDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let _guard = BuildGuard;
    let exclude = crate::config::get().files.exclude;

    let patched = match load_swept(home, &exclude, true) {
        Some((map, moved)) if moved.is_empty() => {
            store_loaded(map);
            return;
        }
        Some((map, moved)) => Index::parse(&map[..], home).and_then(|index| {
            worth_patching(&index, &moved)
                .then(|| patch(&index, &moved, &exclude, MAX_ENTRIES))
                .flatten()
        }),
        None => None,
    };

    let started = Instant::now();
    let (tables, patched) = match patched {
        Some(tables) => (tables, true),
        None => {
            let Some(tables) = build(home, MAX_ENTRIES, &exclude) else {
                eprintln!("wayrun: file index build skipped: the walk did not finish");
                return;
            };
            (tables, false)
        }
    };
    if !crate::config::get().files.index {
        return; // indexing was turned off while this ran
    }
    let Some(path) = path() else {
        return;
    };
    // a cache file, or its `.tmp`, may exist from here on
    lock().clean = false;
    // the image streams to the file: materialising 30+ MB to hand to one
    // `write_all` would hold both copies at the peak
    let mut shape = (0u32, 0u32, 0usize);
    let write = crate::system::fs::write_atomic_with(&path, |file| {
        let mut buf = std::io::BufWriter::new(file);
        shape = tables.write_into(home, &exclude, &mut buf)?;
        buf.flush()
    });
    if let Err(err) = write {
        eprintln!("wayrun: file index write failed: {err}");
        return;
    }
    // the map only reaches the queries if the image it came from can be
    // walked: the load gate is the one gate for that
    if let Some((map, _)) = load_swept(home, &exclude, false) {
        store(map);
    }

    let (dir_count, file_count, len) = shape;
    let capped = dir_count as usize + file_count as usize >= MAX_ENTRIES;
    let tag = if patched {
        " (patched)"
    } else if capped {
        " (capped)"
    } else {
        ""
    };
    eprintln!(
        "wayrun: file index {dir_count} dirs, {file_count} files, {}MB, {:.1}s{tag}",
        len / 1_000_000,
        started.elapsed().as_secs_f64(),
    );
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

pub(super) fn mtime_ns(path: &Path) -> Option<i64> {
    let time = std::fs::metadata(path).ok()?.modified().ok()?;
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => Some(since.as_nanos() as i64),
        Err(before) => Some(-(before.duration().as_nanos() as i64)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_owns_exactly_the_two_file_providers() {
        assert!(owns("file-search"));
        assert!(owns("path-search"));
        assert!(!owns("app-search"));
    }
}
