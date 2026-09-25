use std::collections::HashSet;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use crate::provider::file::index::build::{
    Root, Tables, read_dir_entries, walk_into, walk_pool, walk_threads,
};
use crate::provider::file::index::format::{
    DirOut, DirRec, FileOut, Index, bloom64, bloom128, name_haystack, push_dir_path, push_name,
};
use crate::provider::file::index::mtime_ns;
use crate::provider::file::index::scan::{par_chunks, threads_for};
use crate::provider::push_lowered;

/// The changed share, as `dir_count / PATCH_SHARE`, past which a fresh walk is
/// the better answer than re-reading the moved listings one by one.
const PATCH_SHARE: usize = 4;

/// Every recorded directory whose mtime differs from the stored one, so a patch
/// re-reads exactly the listings that moved; empty means the image is fresh.
pub(super) fn changed_dirs(index: &Index) -> Vec<u32> {
    if index.dir_count == 0 {
        return Vec::new();
    }
    let mut moved: Vec<u32> = par_chunks(
        index.dir_count,
        threads_for(index.dir_count),
        |start, end| {
            let mut path = PathBuf::new();
            let mut chain = Vec::new();
            let mut moved = Vec::new();
            for i in start..end {
                let rec = index.dir(i);
                push_dir_path(index, i, &mut path, &mut chain);
                if mtime_ns(&path) != Some(rec.mtime_ns) {
                    moved.push(i);
                }
            }
            moved
        },
    )
    .into_iter()
    .flatten()
    .collect();
    moved.sort_unstable();
    moved
}

/// Whether the moved listings are few enough for a patch to beat a fresh walk.
pub(super) fn worth_patching(index: &Index, changed: &[u32]) -> bool {
    changed.len() * PATCH_SHARE <= index.dir_count as usize
}

/// A directory's children as the sibling chain the DFS pre-order implies;
/// `None` when the table is not that pre-order, which no patch may be applied to.
struct Tree {
    first: Vec<u32>,
    next: Vec<u32>,
}

impl Tree {
    fn build(index: &Index) -> Option<Self> {
        let count = index.dir_count as usize;
        let mut first = vec![u32::MAX; count];
        let mut last = vec![u32::MAX; count];
        let mut next = vec![u32::MAX; count];
        let mut stack: Vec<u32> = vec![0];
        for i in 1..index.dir_count {
            let depth = index.dir(i).depth as usize;
            stack.truncate(depth);
            let parent = *stack.last()?;
            // the slot a pre-order walk would call the parent must be the stored one
            if index.dir(i).parent != parent {
                return None;
            }
            if first[parent as usize] == u32::MAX {
                first[parent as usize] = i;
            } else {
                next[last[parent as usize] as usize] = i;
            }
            last[parent as usize] = i;
            stack.push(i);
        }
        Some(Tree { first, next })
    }
}

/// Splice the moved listings into the image, re-reading only the flagged
/// directories; `None` when a full walk must produce it instead.
pub(super) fn patch(
    index: &Index,
    changed: &[u32],
    exclude: &[String],
    cap: usize,
) -> Option<Tables> {
    let mut patcher = Patcher {
        index: *index,
        tree: Tree::build(index)?,
        changed: changed.iter().copied().collect(),
        exclude,
        cap,
        pool: walk_pool(walk_threads())?,
        tables: Tables::new(),
    };
    patcher.emit(0, u32::MAX)?;
    Some(patcher.tables)
}

struct Patcher<'a> {
    index: Index<'a>,
    tree: Tree,
    changed: HashSet<u32>,
    exclude: &'a [String],
    cap: usize,
    /// One pool for the whole patch: a newly found directory must not rebuild it.
    pool: rayon::ThreadPool,
    tables: Tables,
}

impl<'a> Patcher<'a> {
    fn emit(&mut self, slot: u32, parent: u32) -> Option<()> {
        if self.changed.contains(&slot) {
            self.emit_fresh(slot, parent)
        } else {
            self.emit_verbatim(slot, parent)
        }
    }

    /// The listing did not move: copy the record and its files, and recurse into
    /// the children, whose own order is still the one the image stored.
    fn emit_verbatim(&mut self, slot: u32, parent: u32) -> Option<()> {
        let rec = self.index.dir(slot);
        let new_slot = self.push_dir(rec, parent, rec.mtime_ns)?;
        for file in file_range(&self.index, slot) {
            let rec = self.index.file(file);
            let (name_off, name_len) = push_name(&mut self.tables.names, rec.name);
            self.tables.files.push(FileOut {
                dir: new_slot,
                name_off,
                name_len,
                name_bloom: bloom64(&name_haystack(rec.name)).to_le_bytes(),
            });
        }
        let mut child = self.tree.first[slot as usize];
        while child != u32::MAX {
            self.emit(child, new_slot)?;
            child = self.tree.next[child as usize];
        }
        Some(())
    }

    /// The listing moved: re-read it, matching the children the image has by
    /// name so their subtrees survive, and walking the children it lacks.
    fn emit_fresh(&mut self, slot: u32, parent: u32) -> Option<()> {
        let rec = self.index.dir(slot);
        let path = dir_path(&self.index, slot);
        // stat before listing, as the walk does: a change landing between the two
        // then leaves a stored mtime older than the disk, so the sweep re-reads
        let mtime_ns = mtime_ns(&path)?;
        let entries = read_dir_entries(&path, self.exclude)?;
        let new_slot = self.push_dir(rec, parent, mtime_ns)?;
        for name in &entries.files {
            let bytes = name.as_os_str().as_bytes();
            let (name_off, name_len) = push_name(&mut self.tables.names, bytes);
            self.tables.files.push(FileOut {
                dir: new_slot,
                name_off,
                name_len,
                name_bloom: bloom64(&name_haystack(bytes)).to_le_bytes(),
            });
        }
        let mut known = Vec::new();
        let mut child = self.tree.first[slot as usize];
        while child != u32::MAX {
            known.push((self.index.dir(child).name, child));
            child = self.tree.next[child as usize];
        }
        for name in &entries.dirs {
            let kept = name.as_os_str().as_bytes();
            match known.iter().find(|(name, _)| *name == kept) {
                Some((_, child)) => self.emit(*child, new_slot)?,
                None => {
                    let lower = self.child_lower(slot, kept);
                    walk_into(
                        &mut self.tables,
                        Root {
                            path: &path.join(name),
                            parent: new_slot,
                            depth: rec.depth + 1,
                            lower: &lower,
                        },
                        self.cap,
                        self.exclude,
                        &self.pool,
                    )?;
                }
            }
        }
        Some(())
    }

    /// Copy a directory record whose fields stand, giving it its new slot.
    fn push_dir(&mut self, rec: DirRec<'_>, parent: u32, mtime_ns: i64) -> Option<u32> {
        if self.tables.dirs.len() + self.tables.files.len() >= self.cap {
            return None;
        }
        let (name_off, name_len) = push_name(&mut self.tables.names, rec.name);
        let (path_off, path_len) = push_name(&mut self.tables.paths, rec.path_lower.as_bytes());
        let slot = self.tables.dirs.len() as u32;
        self.tables.dirs.push(DirOut {
            parent,
            name_off,
            name_len,
            depth: rec.depth,
            mtime_ns,
            path_off,
            path_len,
            path_bloom: bloom128(rec.path_lower.as_bytes()).to_le_bytes(),
        });
        Some(slot)
    }

    /// The lowered path the walk would store for `slot`'s child `name`: the
    /// parent's stored path plus the lowercased name.
    fn child_lower(&self, slot: u32, name: &[u8]) -> String {
        let mut lower = String::from(self.index.dir(slot).path_lower);
        lower.push('/');
        push_lowered(&mut lower, &String::from_utf8_lossy(name));
        lower
    }
}

fn dir_path(index: &Index, slot: u32) -> PathBuf {
    let mut path = PathBuf::new();
    let mut chain = Vec::new();
    push_dir_path(index, slot, &mut path, &mut chain);
    path
}

/// The old file records belonging to directory `slot`: the file table is sorted
/// by `dir`, so they are one contiguous run.
fn file_range(index: &Index, slot: u32) -> std::ops::Range<u32> {
    let bound = |after: bool| {
        let (mut lo, mut hi) = (0, index.file_count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let dir = index.file(mid).dir;
            if dir < slot || (after && dir == slot) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    };
    bound(false)..bound(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::file::index::MAX_ENTRIES;
    use crate::provider::file::index::build::build;
    use crate::provider::file::index::format::{header_bytes, layout_ok};
    use crate::provider::file::index::test_support::{parsed, write};
    use std::path::Path;

    /// The image's tables as text, in slot order, blooms included: what a patch
    /// and a walk of the same state must agree on.
    fn tables_of(bytes: &[u8], home: &Path) -> Vec<String> {
        let index = parsed(bytes, home);
        let dirs = (0..index.dir_count).map(|i| {
            let d = index.dir(i);
            format!(
                "d {} {} {} {} {} {:032x}",
                d.parent,
                d.depth,
                d.mtime_ns,
                String::from_utf8_lossy(d.name),
                d.path_lower,
                index.dir_bloom(i)
            )
        });
        let files = (0..index.file_count).map(|i| {
            let f = index.file(i);
            format!(
                "f {} {} {:016x}",
                f.dir,
                String::from_utf8_lossy(f.name),
                index.file_bloom(i)
            )
        });
        dirs.chain(files).collect()
    }

    /// The patch must land on the tables a full walk of the same state builds:
    /// anything less means the two paths can drift apart.
    #[test]
    fn a_patch_matches_a_full_build() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("keep/a.txt"));
        write(&home.join("keep/sub/b.txt"));
        write(&home.join("other/c.txt"));
        let before = build(home, MAX_ENTRIES, &[]).unwrap().assemble(home, &[]);

        // one of each kind the patch has to splice
        write(&home.join("keep/new.txt"));
        write(&home.join("keep/sub/deep/one.txt"));
        write(&home.join("fresh/dir/two.txt"));
        std::fs::remove_file(home.join("keep/a.txt")).unwrap();
        std::fs::remove_dir_all(home.join("other")).unwrap();

        let index = parsed(&before, home);
        let changed = changed_dirs(&index);
        assert!(!changed.is_empty(), "the sweep sees the moved listings");
        let tables = patch(&index, &changed, &[], MAX_ENTRIES).expect("the images splice");
        assert_eq!(
            tables_of(&tables.assemble(home, &[]), home),
            tables_of(
                &build(home, MAX_ENTRIES, &[]).unwrap().assemble(home, &[]),
                home
            ),
            "the patch matches a full walk"
        );
    }

    /// A table that is not the pre-order the walk emits cannot be patched: the
    /// depths alone would place a record under a parent its slot does not name.
    #[test]
    fn a_patch_refuses_a_table_that_is_not_pre_order() {
        let home = b"/home/x";
        let mut names = Vec::new();
        let mut paths = Vec::new();
        let mut dirs = Vec::new();
        // root, two top dirs, then a depth-2 record whose stored parent is the
        // first while its slot follows the second: the pre-order must reject it
        for (parent, depth, name) in [
            (u32::MAX, 0, &home[..]),
            (0, 1, &b"P"[..]),
            (0, 1, &b"Q"[..]),
            (1, 2, &b"C"[..]),
        ] {
            let (name_off, name_len) = push_name(&mut names, name);
            let (path_off, path_len) = push_name(&mut paths, name);
            dirs.push(DirOut {
                parent,
                name_off,
                name_len,
                depth,
                mtime_ns: 0,
                path_off,
                path_len,
                path_bloom: bloom128(name).to_le_bytes(),
            });
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header_bytes(
            dirs.len() as u32,
            0,
            names.len() as u32,
            home.len() as u32,
            paths.len() as u32,
            0,
        ));
        bytes.extend_from_slice(home);
        for dir in &dirs {
            bytes.extend_from_slice(&dir.bytes());
        }
        for dir in &dirs {
            bytes.extend_from_slice(&dir.path_bloom);
        }
        bytes.extend_from_slice(&names);
        bytes.extend_from_slice(&paths);

        let index = Index::parse(&bytes, Path::new("/home/x")).expect("the table parses");
        assert!(layout_ok(&index), "the depths still chain legally");
        assert!(patch(&index, &[1], &[], MAX_ENTRIES).is_none());
        assert!(worth_patching(&index, &[1]));
        assert!(!worth_patching(&index, &[1, 2, 3]));
    }

    #[test]
    fn a_changed_directory_shows_up_in_the_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("a/keep.txt"));

        let bytes = build(home, MAX_ENTRIES, &[]).unwrap().assemble(home, &[]);
        let index = parsed(&bytes, home);
        assert!(changed_dirs(&index).is_empty());

        write(&home.join("a/added.txt"));
        assert_eq!(
            changed_dirs(&index),
            vec![1],
            "only the directory whose listing moved is flagged"
        );
    }
}
