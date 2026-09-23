use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The index cache's magic and format version; either mismatch rebuilds.
pub(super) const MAGIC: [u8; 4] = *b"WRFI";
pub(super) const FORMAT_VERSION: u32 = 3;
/// The header before the tables, all little-endian: magic, version, dir_count,
/// file_count, names_len, home_len, paths_len, exclude hash.
pub(super) const HEADER: usize = 36;
pub(super) const H_VERSION: usize = 4;
pub(super) const H_DIRS: usize = H_VERSION + 4;
pub(super) const H_FILES: usize = H_DIRS + 4;
pub(super) const H_NAMES: usize = H_FILES + 4;
pub(super) const H_HOME: usize = H_NAMES + 4;
pub(super) const H_PATHS: usize = H_HOME + 4;
pub(super) const H_EXCLUDE: usize = H_PATHS + 4;
/// The directory record: parent, name offset and length, depth (32-bit each),
/// ns mtime (64-bit), then the lowered path's offset and length (32-bit each).
pub(super) const DIR_REC: usize = 32;
pub(super) const D_PARENT: usize = 0;
pub(super) const D_NAME_OFF: usize = D_PARENT + 4;
pub(super) const D_NAME_LEN: usize = D_NAME_OFF + 4;
pub(super) const D_DEPTH: usize = D_NAME_LEN + 4;
pub(super) const D_MTIME: usize = D_DEPTH + 4;
pub(super) const D_PATH_OFF: usize = D_MTIME + 8;
pub(super) const D_PATH_LEN: usize = D_PATH_OFF + 4;
/// The file record: directory slot, name offset, name length (32-bit each).
pub(super) const FILE_REC: usize = 12;
pub(super) const F_DIR: usize = 0;
pub(super) const F_NAME_OFF: usize = F_DIR + 4;
pub(super) const F_NAME_LEN: usize = F_NAME_OFF + 4;

/// The index bytes as a parse view; nothing is copied. The directory table is
/// DFS pre-order and the file table is sorted by `dir`, which queries rely on.
#[derive(Clone, Copy)]
pub(super) struct Index<'a> {
    pub(super) bytes: &'a [u8],
    // bytes, not `str`: `$HOME` need not be UTF-8
    pub(super) home: &'a [u8],
    dirs: usize,
    files: usize,
    names: usize,
    paths: usize,
    pub(super) dir_count: u32,
    pub(super) file_count: u32,
    pub(super) exclude_hash: u64,
}

#[derive(Clone, Copy)]
pub(super) struct DirRec<'a> {
    pub(super) parent: u32,
    pub(super) depth: u32,
    pub(super) mtime_ns: i64,
    pub(super) name: &'a [u8],
    pub(super) path_lower: &'a str,
}

#[derive(Clone, Copy)]
pub(super) struct FileRec<'a> {
    pub(super) dir: u32,
    pub(super) name: &'a [u8],
}

impl<'a> Index<'a> {
    /// `None` on any mismatch of magic, version, length or `$HOME`, so a caller
    /// rebuilds rather than reading another home's paths.
    pub(super) fn parse(bytes: &'a [u8], home: &Path) -> Option<Self> {
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

    pub(super) fn dir(&self, i: u32) -> DirRec<'a> {
        let rec = self.rec(self.dirs, i, DIR_REC);
        DirRec {
            parent: u32_at(rec, D_PARENT),
            depth: u32_at(rec, D_DEPTH),
            mtime_ns: i64::from_le_bytes(rec[D_MTIME..D_MTIME + 8].try_into().unwrap()),
            name: self.name(u32_at(rec, D_NAME_OFF), u32_at(rec, D_NAME_LEN)),
            path_lower: self.path_lower(u32_at(rec, D_PATH_OFF), u32_at(rec, D_PATH_LEN)),
        }
    }

    pub(super) fn file(&self, i: u32) -> FileRec<'a> {
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
    pub(super) fn path_lower(&self, off: u32, len: u32) -> &'a str {
        let start = self.paths.saturating_add(off as usize);
        let end = start.saturating_add(len as usize);
        std::str::from_utf8(self.bytes.get(start..end).unwrap_or_default()).unwrap_or_default()
    }
}

pub(super) fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

pub(super) fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// Directory `i`'s absolute path; the root uses the stored `$HOME`. `chain` is
/// the caller's scratch, and `dir_count` bounds a corrupt parent cycle.
pub(super) fn push_dir_path(index: &Index, i: u32, out: &mut PathBuf, chain: &mut Vec<u32>) {
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
pub(super) fn layout_ok(index: &Index) -> bool {
    (0..index.dir_count).all(|i| {
        let rec = index.dir(i);
        if i == 0 {
            rec.parent == u32::MAX && rec.depth == 0
        } else {
            rec.parent < i && index.dir(rec.parent).depth + 1 == rec.depth
        }
    })
}

pub(super) struct DirOut {
    pub(super) parent: u32,
    pub(super) name_off: u32,
    pub(super) name_len: u32,
    pub(super) depth: u32,
    pub(super) mtime_ns: i64,
    pub(super) path_off: u32,
    pub(super) path_len: u32,
}

impl DirOut {
    /// The record's bytes; `Index::dir` reads the same fields back at the `D_*` offsets.
    pub(super) fn bytes(&self) -> [u8; DIR_REC] {
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

pub(super) struct FileOut {
    pub(super) dir: u32,
    pub(super) name_off: u32,
    pub(super) name_len: u32,
}

impl FileOut {
    /// The record's bytes; `Index::file` reads the same fields back at the `F_*` offsets.
    pub(super) fn bytes(&self) -> [u8; FILE_REC] {
        let mut rec = [0; FILE_REC];
        rec[F_DIR..F_DIR + 4].copy_from_slice(&self.dir.to_le_bytes());
        rec[F_NAME_OFF..F_NAME_OFF + 4].copy_from_slice(&self.name_off.to_le_bytes());
        rec[F_NAME_LEN..F_NAME_LEN + 4].copy_from_slice(&self.name_len.to_le_bytes());
        rec
    }
}

pub(super) fn push_name(names: &mut Vec<u8>, bytes: &[u8]) -> (u32, u32) {
    let off = names.len() as u32;
    names.extend_from_slice(bytes);
    (off, bytes.len() as u32)
}

/// The header's bytes; `parse` reads the same fields back at the `H_*` offsets.
pub(super) fn header_bytes(
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
pub(super) fn exclude_hash(names: &[String]) -> u64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::file::index::MAX_ENTRIES;
    use crate::provider::file::index::test_support::{build_ok, write};

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

    #[test]
    fn the_exclude_hash_needs_the_length_prefix_and_is_stable() {
        let joined = exclude_hash(&["ab".to_string()]);
        let split = exclude_hash(&["a".to_string(), "b".to_string()]);
        assert_ne!(joined, split, "a length prefix keeps the two apart");
        assert_eq!(joined, exclude_hash(&["ab".to_string()]));
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
}
