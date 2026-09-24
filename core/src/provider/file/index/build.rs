use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::provider::file::index::format::{
    DIR_BLOOM, DIR_REC, DirOut, FILE_BLOOM, FILE_REC, FileOut, HEADER, bloom64, bloom128,
    exclude_hash, header_bytes, name_haystack, push_name,
};
use crate::provider::file::index::mtime_ns;
use crate::provider::file::keep_name;
use crate::provider::push_lowered;

/// The tables an image is assembled from: the record bytes the walk and the
/// incremental update produce, plus the name and path blobs their offsets index.
pub(super) struct Tables {
    pub(super) dirs: Vec<DirOut>,
    pub(super) files: Vec<FileOut>,
    pub(super) names: Vec<u8>,
    pub(super) paths: Vec<u8>,
}

impl Tables {
    pub(super) fn new() -> Self {
        Tables {
            dirs: Vec::new(),
            files: Vec::new(),
            names: Vec::new(),
            paths: Vec::new(),
        }
    }

    /// The image bytes: header, home, both tables and their blooms, then the
    /// blobs. The one place an image is written, so the walk and the update
    /// cannot drift apart.
    pub(super) fn assemble(mut self, home: &Path, exclude: &[String]) -> Vec<u8> {
        self.files.sort_by_key(|rec| rec.dir);
        let home_bytes = home.as_os_str().as_bytes();
        let mut out = Vec::with_capacity(
            HEADER
                + home_bytes.len()
                + self.dirs.len() * DIR_REC
                + self.files.len() * FILE_REC
                + self.files.len() * FILE_BLOOM
                + self.dirs.len() * DIR_BLOOM
                + self.names.len()
                + self.paths.len(),
        );
        out.extend_from_slice(&header_bytes(
            self.dirs.len() as u32,
            self.files.len() as u32,
            self.names.len() as u32,
            home_bytes.len() as u32,
            self.paths.len() as u32,
            exclude_hash(exclude),
        ));
        out.extend_from_slice(home_bytes);
        for d in &self.dirs {
            out.extend_from_slice(&d.bytes());
        }
        for f in &self.files {
            out.extend_from_slice(&f.bytes());
        }
        for f in &self.files {
            out.extend_from_slice(&f.name_bloom.to_le_bytes());
        }
        for d in &self.dirs {
            out.extend_from_slice(&d.path_bloom.to_le_bytes());
        }
        out.extend_from_slice(&self.names);
        out.extend_from_slice(&self.paths);
        out
    }
}

/// One directory's kept entries in readdir order: the child directories and the
/// files, with hidden names and `[files] exclude` filtered out.
pub(super) struct DirEntries {
    pub(super) dirs: Vec<OsString>,
    pub(super) files: Vec<OsString>,
}

pub(super) fn read_dir_entries(path: &Path, exclude: &[String]) -> Option<DirEntries> {
    let mut entries = DirEntries {
        dirs: Vec::new(),
        files: Vec::new(),
    };
    for entry in std::fs::read_dir(path).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name();
        if !keep_name(&name, exclude) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            entries.dirs.push(name);
        } else if kind.is_file() {
            entries.files.push(name);
        }
    }
    Some(entries)
}

/// One directory waiting to be read: its DFS order key (the ordinal it holds
/// among its siblings, from the root down), its path and lowered path.
struct Task {
    chain: Vec<u32>,
    path: PathBuf,
    lower: String,
    depth: u32,
}

/// A directory read into its record fields and its files; the merge orders
/// whole segments by [`Task::chain`] into the walk's visit order.
struct Seg {
    chain: Vec<u32>,
    depth: u32,
    mtime_ns: i64,
    name: Vec<u8>,
    lower: String,
    files: Vec<Vec<u8>>,
}

/// The walk's limits: the entries produced so far, and whether a directory's
/// mtime could not be read at all.
struct Limits {
    cap: usize,
    count: AtomicUsize,
    over: AtomicBool,
    failed: AtomicBool,
}

impl Limits {
    /// Past `cap` with headroom the merge truncates anyway, and the count is
    /// only there to bound what the walk holds.
    fn add(&self, entries: usize) {
        let total = self.count.fetch_add(entries, Ordering::Relaxed) + entries;
        if total > self.cap + self.cap / 2 {
            self.over.store(true, Ordering::Relaxed);
        }
    }

    fn stop(&self) -> bool {
        self.over.load(Ordering::Relaxed)
    }
}

/// The walk's thread count: one per core, since a directory task is small.
pub(super) fn walk_threads() -> u32 {
    std::thread::available_parallelism().map_or(1, |num| num.get() as u32)
}

/// Where a walk starts: the directory, the slot its record goes in (`u32::MAX`
/// for the image's own root), its depth and its lowered path.
pub(super) struct Root<'a> {
    pub(super) path: &'a Path,
    pub(super) parent: u32,
    pub(super) depth: u32,
    pub(super) lower: &'a str,
}

/// Walk `root`'s subtree into `tables`. `cap` bounds the tables' entries; `None`
/// when a partial walk must not be persisted as an authoritative index.
pub(super) fn walk_into(
    tables: &mut Tables,
    root: Root<'_>,
    cap: usize,
    exclude: &[String],
    threads: u32,
) -> Option<()> {
    let Root {
        path: dir,
        parent,
        depth,
        lower: root_lower,
    } = root;
    let root = dir;
    if !root.is_dir() {
        return None;
    }
    let root_bytes = root.as_os_str().as_bytes();
    let name = root.file_name().map(OsStr::as_bytes).unwrap_or(root_bytes);
    let (name_off, name_len) = push_name(&mut tables.names, name);
    let (path_off, path_len) = push_name(&mut tables.paths, root_lower.as_bytes());
    tables.dirs.push(DirOut {
        parent,
        name_off,
        name_len,
        depth,
        mtime_ns: mtime_ns(root)?,
        path_off,
        path_len,
        path_bloom: bloom128(root_lower.as_bytes()),
    });
    let root_slot = tables.dirs.len() as u32 - 1;

    let entries = read_dir_entries(root, exclude)?;
    for name in &entries.files {
        let bytes = name.as_os_str().as_bytes();
        let (name_off, name_len) = push_name(&mut tables.names, bytes);
        tables.files.push(FileOut {
            dir: root_slot,
            name_off,
            name_len,
            name_bloom: bloom64(&name_haystack(bytes)),
        });
    }
    let limits = Limits {
        cap,
        count: AtomicUsize::new(1 + entries.files.len()),
        over: AtomicBool::new(false),
        failed: AtomicBool::new(false),
    };
    let segs = Mutex::new(Vec::new());
    // a pool per walk keeps the thread count a caller's parameter, not global state
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.clamp(1, walk_threads()) as usize)
        .build()
        .ok()?;
    pool.scope(|scope| {
        for (ordinal, name) in entries.dirs.iter().enumerate() {
            let task = task_of(&[], ordinal as u32, name, root, root_lower, depth);
            scope.spawn(|scope| visit(scope, task, exclude, &segs, &limits));
        }
    });
    if limits.failed.load(Ordering::Relaxed) {
        return None;
    }
    let mut segs = segs.into_inner().unwrap_or_else(PoisonError::into_inner);
    segs.sort_unstable_by(|a, b| a.chain.cmp(&b.chain));
    merge(tables, root_slot, depth, cap, segs);
    Some(())
}

/// The task for the child directory `name`, `ordinal` in its parent's listing.
fn task_of(
    chain: &[u32],
    ordinal: u32,
    name: &OsStr,
    parent: &Path,
    parent_lower: &str,
    depth: u32,
) -> Task {
    let mut lower = String::from(parent_lower);
    lower.push('/');
    push_lowered(&mut lower, &String::from_utf8_lossy(name.as_bytes()));
    let mut chain = chain.to_vec();
    chain.push(ordinal);
    Task {
        chain,
        path: parent.join(name),
        lower,
        depth: depth + 1,
    }
}

/// Read one directory and spawn its children, so the whole walk rides rayon's
/// scheduler.
fn visit<'s>(
    scope: &rayon::Scope<'s>,
    task: Task,
    exclude: &'s [String],
    segs: &'s Mutex<Vec<Seg>>,
    limits: &'s Limits,
) {
    let (seg, children) = read_task(task, exclude, limits);
    if let Some(seg) = seg {
        segs.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(seg);
    }
    for child in children {
        scope.spawn(move |scope| visit(scope, child, exclude, segs, limits));
    }
}

/// Read one directory into a segment and hand back its children as tasks; a
/// failed mtime fails the walk, while an unlistable directory keeps its record.
fn read_task(task: Task, exclude: &[String], limits: &Limits) -> (Option<Seg>, Vec<Task>) {
    let Some(mtime_ns) = mtime_ns(&task.path) else {
        limits.failed.store(true, Ordering::Relaxed);
        return (None, Vec::new());
    };
    let mut seg = Seg {
        name: task
            .path
            .file_name()
            .expect("a task's path names a directory")
            .as_bytes()
            .to_vec(),
        chain: task.chain,
        depth: task.depth,
        mtime_ns,
        lower: task.lower,
        files: Vec::new(),
    };
    let mut children = Vec::new();
    if !limits.stop()
        && let Some(entries) = read_dir_entries(&task.path, exclude)
    {
        limits.add(entries.dirs.len() + entries.files.len());
        for (ordinal, name) in entries.dirs.iter().enumerate() {
            children.push(task_of(
                &seg.chain,
                ordinal as u32,
                name,
                &task.path,
                &seg.lower,
                seg.depth,
            ));
        }
        seg.files = entries
            .files
            .iter()
            .map(|name| name.as_os_str().as_bytes().to_vec())
            .collect();
    }
    (Some(seg), children)
}

/// Order the segments into the tables: the chains are the DFS pre-order, so
/// sorting by chain and keeping a slot per depth emits the walk's visit order.
fn merge(tables: &mut Tables, root_slot: u32, depth: u32, cap: usize, segs: Vec<Seg>) {
    let mut stack = vec![root_slot; depth as usize + 1];
    for seg in segs {
        stack.truncate(seg.depth as usize);
        let Some(&parent) = stack.last() else {
            continue;
        };
        if tables.dirs.len() + tables.files.len() >= cap {
            return;
        }
        let (name_off, name_len) = push_name(&mut tables.names, &seg.name);
        let (path_off, path_len) = push_name(&mut tables.paths, seg.lower.as_bytes());
        let slot = tables.dirs.len() as u32;
        tables.dirs.push(DirOut {
            parent,
            name_off,
            name_len,
            depth: seg.depth,
            mtime_ns: seg.mtime_ns,
            path_off,
            path_len,
            path_bloom: bloom128(seg.lower.as_bytes()),
        });
        stack.push(slot);
        for file in seg.files {
            if tables.dirs.len() + tables.files.len() >= cap {
                return;
            }
            let (name_off, name_len) = push_name(&mut tables.names, &file);
            tables.files.push(FileOut {
                dir: slot,
                name_off,
                name_len,
                name_bloom: bloom64(&name_haystack(&file)),
            });
        }
    }
}

/// Walk `home` into the index image; `cap` bounds dirs and files together, and
/// `exclude` holds the names the walk never enters; `None` if it must not persist.
pub(super) fn build(home: &Path, cap: usize, exclude: &[String]) -> Option<Vec<u8>> {
    let mut root_lower = String::new();
    // lossy only for the haystack: a UTF-8 query can never carry replaced bytes
    push_lowered(
        &mut root_lower,
        &String::from_utf8_lossy(home.as_os_str().as_bytes()),
    );
    let mut tables = Tables::new();
    walk_into(
        &mut tables,
        Root {
            path: home,
            parent: u32::MAX,
            depth: 0,
            lower: &root_lower,
        },
        cap,
        exclude,
        walk_threads(),
    )?;
    Some(tables.assemble(home, exclude))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::file::index::MAX_ENTRIES;
    use crate::provider::file::index::scan::search_in;
    use crate::provider::file::index::test_support::{build_ok, parsed, shipped_exclude, write};

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

    /// The walk's answer cannot depend on how many threads ran it: the merge
    /// orders segments by the DFS chain, not by the order they finished in.
    #[test]
    fn the_thread_count_does_not_move_the_image() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for i in 0..6 {
            write(&home.join(format!("d{i}/sub/x.txt")));
            write(&home.join(format!("d{i}/y{i}.txt")));
        }
        write(&home.join("top.txt"));

        let walk = |threads: u32| {
            let mut tables = Tables::new();
            let mut lower = String::new();
            push_lowered(
                &mut lower,
                &String::from_utf8_lossy(home.as_os_str().as_bytes()),
            );
            walk_into(
                &mut tables,
                Root {
                    path: home,
                    parent: u32::MAX,
                    depth: 0,
                    lower: &lower,
                },
                MAX_ENTRIES,
                &[],
                threads,
            )
            .unwrap();
            tables.assemble(home, &[])
        };
        assert_eq!(walk(1), walk(walk_threads()));
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
}
