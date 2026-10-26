use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::notify::root_targets;

struct Signal {
    epoch: AtomicU64,
    rescan: AtomicBool,
}

#[derive(Default)]
struct State {
    targets: Vec<PathBuf>,
    watched: HashSet<PathBuf>,
    failed: HashSet<PathBuf>,
    fallback: Option<Vec<Stamp>>,
}

#[derive(Clone, PartialEq)]
struct Stamp {
    path: PathBuf,
    modified: Option<SystemTime>,
}

impl Stamp {
    fn of(path: &Path) -> Self {
        let modified = fs::metadata(path).and_then(|meta| meta.modified()).ok();
        Self {
            path: path.to_path_buf(),
            modified,
        }
    }
}

/// The roots a cache is built from, with the watcher and epoch that announce a
/// change. Events touch only atomics; the cache rebuilds on the next [`Live::get`].
pub struct Source {
    roots: fn() -> Vec<PathBuf>,
    signal: Arc<Signal>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    state: Mutex<State>,
}

impl Source {
    pub fn new(roots: fn() -> Vec<PathBuf>) -> Self {
        let signal = Arc::new(Signal {
            epoch: AtomicU64::new(0),
            rescan: AtomicBool::new(false),
        });
        let watcher = {
            let signal = Arc::clone(&signal);
            notify::recommended_watcher(move |result| observe(&signal, result)).ok()
        };
        let source = Self {
            roots,
            signal,
            watcher: Mutex::new(watcher),
            state: Mutex::new(State::default()),
        };
        source.arm();
        source
    }

    fn epoch(&self) -> u64 {
        self.signal.epoch.load(Ordering::Acquire)
    }

    /// Mark every reader stale without a filesystem event (a settings edit).
    pub fn invalidate(&self) {
        self.signal.epoch.fetch_add(1, Ordering::Release);
    }

    fn arm(&self) {
        self.rearm((self.roots)());
    }

    /// Resolve `roots` and watch each new target. `Watcher::watch` blocks on
    /// notify's own loop, so this runs on the search thread, never in the callback.
    fn rearm(&self, roots: Vec<PathBuf>) {
        self.poll_fallback();

        let mut targets = Vec::new();
        let mut seen = HashSet::new();
        for root in &roots {
            for target in root_targets(root) {
                if seen.insert(target.clone()) {
                    targets.push(target);
                }
            }
        }

        let rescan = self.signal.rescan.swap(false, Ordering::AcqRel);
        let install = {
            let mut state = lock(&self.state);
            if rescan {
                state.watched.clear();
                state.failed.clear();
            }
            if state.targets != targets {
                state.targets.clone_from(&targets);
                self.signal.epoch.fetch_add(1, Ordering::Release);
            }
            targets
                .into_iter()
                .filter(|target| !state.watched.contains(target) && !state.failed.contains(target))
                .collect::<Vec<_>>()
        };

        let mut installed = Vec::new();
        let mut failed = Vec::new();
        if !install.is_empty() {
            let mut watcher = lock(&self.watcher);
            for target in install {
                let Some(watcher) = watcher.as_mut() else {
                    failed.push(target);
                    continue;
                };
                if watcher.watch(&target, RecursiveMode::NonRecursive).is_ok() {
                    installed.push(target);
                } else {
                    failed.push(target);
                }
            }
        }

        let mut state = lock(&self.state);
        state.watched.extend(installed);
        state.failed.extend(failed);
        state.fallback = (!state.failed.is_empty()).then(|| stamps(&state.failed));
    }

    /// Without a working watch, compare the failed paths' mtimes, so adds and
    /// removals still land; in-place edits are out of reach.
    fn poll_fallback(&self) {
        let previous = {
            let state = lock(&self.state);
            let Some(previous) = state.fallback.as_ref() else {
                return;
            };
            previous.clone()
        };
        let current: Vec<Stamp> = {
            let state = lock(&self.state);
            stamps(&state.failed)
        };
        if previous != current {
            self.signal.epoch.fetch_add(1, Ordering::Release);
        }
        lock(&self.state).fallback = Some(current);
    }
}

fn observe(signal: &Signal, result: notify::Result<Event>) {
    match result {
        Ok(event) => {
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            if event.need_rescan()
                || matches!(
                    event.kind,
                    EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
                )
            {
                signal.rescan.store(true, Ordering::Release);
            }
            signal.epoch.fetch_add(1, Ordering::Release);
        }
        Err(_) => {
            signal.rescan.store(true, Ordering::Release);
            signal.epoch.fetch_add(1, Ordering::Release);
        }
    }
}

fn stamps(paths: &HashSet<PathBuf>) -> Vec<Stamp> {
    let mut paths: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    paths.sort();
    paths.into_iter().map(Stamp::of).collect()
}

/// A value rebuilt lazily whenever its [`Source`] epoch moves.
pub struct Live<T> {
    source: Arc<Source>,
    value: Mutex<Option<(u64, Arc<T>)>>,
    bound: Option<usize>,
}

impl<T: Send + Sync + 'static> Live<T> {
    pub fn new(roots: fn() -> Vec<PathBuf>) -> Self {
        Self::shared(Arc::new(Source::new(roots)))
    }

    /// Several caches over one watched source share a watcher and an epoch.
    pub fn shared(source: Arc<Source>) -> Self {
        Self {
            source,
            value: Mutex::new(None),
            bound: None,
        }
    }

    /// Drop the map whole once it holds `bound` entries.
    pub fn bounded(mut self, bound: usize) -> Self {
        self.bound = Some(bound);
        self
    }

    /// The last built value, stale by design: no arm, no build, no filesystem.
    pub fn snapshot(&self) -> Option<Arc<T>> {
        lock(&self.value)
            .as_ref()
            .map(|(_, value)| Arc::clone(value))
    }

    /// Blocking: arms the source and rebuilds only when the epoch moved. Call
    /// this from `spawn_blocking`; `snapshot` is the lock-only reader.
    pub fn get(&self, build: impl FnOnce() -> T) -> Arc<T> {
        self.source.arm();
        let epoch = self.source.epoch();
        if let Some((at, value)) = lock(&self.value).as_ref()
            && *at == epoch
        {
            return Arc::clone(value);
        }
        let value = Arc::new(build());
        let mut slot = lock(&self.value);
        if slot.as_ref().is_none_or(|(at, _)| *at <= epoch) {
            *slot = Some((epoch, Arc::clone(&value)));
        }
        value
    }
}

impl<V: Clone + Send + Sync + 'static> Live<HashMap<String, V>> {
    /// The key's cached value, built on a miss; a stale epoch drops the map
    /// whole. The source is armed on a miss only, so hits stay atomic-only.
    pub fn memoized(&self, key: &str, build: impl FnOnce() -> V) -> V {
        if let Some((at, map)) = lock(&self.value).as_ref()
            && *at == self.source.epoch()
            && let Some(value) = map.get(key)
        {
            return value.clone();
        }

        self.source.arm();
        let epoch = self.source.epoch();
        let value = build();
        let mut slot = lock(&self.value);
        match slot.as_mut() {
            Some((at, map)) if *at == epoch => {
                let map = Arc::make_mut(map);
                if self.bound.is_some_and(|bound| map.len() >= bound) {
                    map.clear();
                }
                map.insert(key.to_string(), value.clone());
            }
            _ => {
                let mut map = HashMap::new();
                map.insert(key.to_string(), value.clone());
                *slot = Some((epoch, Arc::new(map)));
            }
        }
        value
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;

    use notify::Event;
    use notify::event::{AccessKind, ModifyKind, RemoveKind};

    use super::{Live, Source, lock, observe};

    fn no_roots() -> Vec<PathBuf> {
        Vec::new()
    }

    #[test]
    fn snapshot_is_empty_until_the_first_build() {
        let live = Live::new(no_roots);
        assert!(live.snapshot().is_none());
        let value = live.get(|| 1);
        assert_eq!(*live.snapshot().unwrap(), *value);
    }

    #[test]
    fn get_builds_once_per_epoch() {
        let live = Live::new(no_roots);
        let builds = AtomicUsize::new(0);
        let first = live.get(|| {
            builds.fetch_add(1, Ordering::Relaxed);
            7
        });
        let second = live.get(|| {
            builds.fetch_add(1, Ordering::Relaxed);
            8
        });
        assert_eq!(builds.load(Ordering::Relaxed), 1);
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(*second, 7);
    }

    #[test]
    fn an_epoch_move_rebuilds_and_snapshot_stays_stale() {
        let live = Live::new(no_roots);
        let first = live.get(|| 1);
        live.source.signal.epoch.fetch_add(1, Ordering::Release);
        assert!(Arc::ptr_eq(&live.snapshot().unwrap(), &first));
        let second = live.get(|| 2);
        assert_eq!(*second, 2);
        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn an_older_build_does_not_overwrite_a_newer_epoch() {
        let live = Arc::new(Live::new(no_roots));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let slow = {
            let live = Arc::clone(&live);
            thread::spawn(move || {
                live.get(|| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    1
                })
            })
        };
        started_rx.recv().unwrap();
        live.source.signal.epoch.fetch_add(1, Ordering::Release);
        let fresh = live.get(|| 2);
        assert_eq!(*fresh, 2);
        release_tx.send(()).unwrap();
        assert_eq!(*slow.join().unwrap(), 1);
        assert_eq!(*live.snapshot().unwrap(), 2);
    }

    #[test]
    fn a_memoized_value_survives_until_the_epoch_moves() {
        let live = Live::new(no_roots);
        assert_eq!(live.memoized("key", || 1), 1);
        assert_eq!(live.memoized("key", || 2), 1);
        live.source.invalidate();
        assert_eq!(live.memoized("key", || 2), 2);
    }

    #[test]
    fn a_bounded_map_is_dropped_whole_at_capacity() {
        let live = Live::new(no_roots).bounded(2);
        assert_eq!(live.memoized("a", || 1), 1);
        assert_eq!(live.memoized("b", || 2), 2);
        assert_eq!(live.memoized("c", || 3), 3);
        assert_eq!(live.memoized("a", || 9), 9);
        assert_eq!(live.memoized("c", || 9), 3);
    }

    #[test]
    fn access_events_do_not_advance_the_epoch() {
        let source = Source::new(no_roots);
        let before = source.epoch();
        observe(
            &source.signal,
            Ok(Event::new(notify::EventKind::Access(AccessKind::Any))),
        );
        assert_eq!(source.epoch(), before);
    }

    #[test]
    fn changes_and_errors_advance_the_epoch() {
        let source = Source::new(no_roots);
        let before = source.epoch();
        observe(
            &source.signal,
            Ok(Event::new(notify::EventKind::Modify(ModifyKind::Any))),
        );
        assert_eq!(source.epoch(), before + 1);

        observe(
            &source.signal,
            Ok(Event::new(notify::EventKind::Remove(RemoveKind::Any))),
        );
        assert!(source.signal.rescan.load(Ordering::Acquire));

        observe(&source.signal, Err(notify::Error::generic("gone")));
        assert_eq!(source.epoch(), before + 3);
    }

    #[test]
    fn a_missing_root_falls_back_to_its_ancestor_and_rearms() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("applications");
        let source = Source::new(no_roots);
        source.rearm(vec![missing.clone()]);
        assert_eq!(lock(&source.state).targets, vec![temp.path().to_path_buf()]);

        fs::create_dir(&missing).unwrap();
        source.rearm(vec![missing.clone()]);
        assert_eq!(lock(&source.state).targets, vec![missing]);
    }
}
