use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use mlua::{Lua, LuaSerdeExt, Table, Value};
use serde_json::json;

/// Open connections kept across a script's calls, so a keystroke is one query
/// rather than a copy, a connect and a schema parse as well.
#[derive(Default)]
struct Snapshots {
    next: u64,
    open: HashMap<u64, rusqlite::Connection>,
}

pub(super) fn lib(lua: &Lua) -> mlua::Result<Table> {
    let sqlite = lua.create_table()?;
    let snapshots = Rc::new(RefCell::new(Snapshots::default()));
    let opener = Rc::clone(&snapshots);
    sqlite.set(
        "snapshot",
        lua.create_function(move |_, path: String| {
            let cache = crate::system::fs::cache_dir().ok_or_else(|| {
                mlua::Error::RuntimeError("sqlite.snapshot: no cache directory".to_string())
            })?;
            opener
                .borrow_mut()
                .snapshot(&path, &cache)
                .map_err(|error| mlua::Error::RuntimeError(format!("sqlite.snapshot: {error:#}")))
        })?,
    )?;
    sqlite.set(
        "query",
        lua.create_function(
            move |lua, (id, sql, params): (u64, String, Option<Table>)| {
                let params = sqlite_params(params)?;
                let rows = snapshots
                    .borrow()
                    .query(id, &sql, params)
                    .map_err(|error| {
                        mlua::Error::RuntimeError(format!("sqlite.query: {error:#}"))
                    })?;
                lua.to_value(&serde_json::Value::Array(rows))
            },
        )?,
    )?;
    Ok(sqlite)
}

impl Snapshots {
    fn snapshot(&mut self, source: &str, cache: &Path) -> Result<u64> {
        let copy = snapshot_copy(Path::new(source), cache)?;
        let connection = rusqlite::Connection::open_with_flags(
            snapshot_uri(&copy),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        self.next += 1;
        self.open.insert(self.next, connection);
        Ok(self.next)
    }

    fn query(
        &self,
        id: u64,
        sql: &str,
        params: Vec<rusqlite::types::Value>,
    ) -> Result<Vec<serde_json::Value>> {
        let connection = self.open.get(&id).context("unknown sqlite handle")?;
        let mut statement = connection.prepare_cached(sql)?;
        let columns: Vec<String> = statement
            .column_names()
            .iter()
            .map(|name| name.to_string())
            .collect();
        let mut rows = statement.query(rusqlite::params_from_iter(params))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut record = serde_json::Map::new();
            for (index, name) in columns.iter().enumerate() {
                match row.get_ref(index)? {
                    rusqlite::types::ValueRef::Null => {}
                    rusqlite::types::ValueRef::Text(text) => {
                        record.insert(
                            name.clone(),
                            json!(String::from_utf8_lossy(text).into_owned()),
                        );
                    }
                    rusqlite::types::ValueRef::Integer(number) => {
                        record.insert(name.clone(), json!(number));
                    }
                    rusqlite::types::ValueRef::Real(number) => {
                        record.insert(name.clone(), json!(number));
                    }
                    // No consumer reads blobs; a blob column reads as absent.
                    rusqlite::types::ValueRef::Blob(_) => {}
                }
            }
            out.push(serde_json::Value::Object(record));
        }
        Ok(out)
    }
}

fn sqlite_params(params: Option<Table>) -> mlua::Result<Vec<rusqlite::types::Value>> {
    let mut out = Vec::new();
    if let Some(params) = params {
        for value in params.sequence_values::<Value>() {
            out.push(match value? {
                Value::Integer(number) => rusqlite::types::Value::Integer(number),
                Value::Number(number) => rusqlite::types::Value::Real(number),
                Value::String(text) => rusqlite::types::Value::Text(text.to_string_lossy()),
                Value::Boolean(flag) => rusqlite::types::Value::Integer(flag as i64),
                Value::Nil => rusqlite::types::Value::Null,
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "sqlite params must be scalars, got {other:?}"
                    )));
                }
            });
        }
    }
    Ok(out)
}

/// The cached immutable copy of `source` for its current `(mtime, size)`; the
/// snapshot keeps the writer's locking, `-wal` and `-shm` out of the picture.
fn snapshot_copy(source: &Path, cache: &Path) -> Result<PathBuf> {
    let meta = std::fs::metadata(source)?;
    let size = meta.len();
    let nanos = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);

    let stem = format!("snap-{:016x}", hash_path(source));
    let target = cache.join(format!("{stem}-{nanos}-{size}.sqlite"));
    if target.is_file() {
        return Ok(target);
    }

    // A pid-suffixed tmp keeps two racing hosts from interleaving one copy.
    let tmp = cache.join(format!("{stem}.{}.tmp", std::process::id()));
    std::fs::copy(source, &tmp)?;
    std::fs::rename(&tmp, &target)?;
    prune(cache, &target);
    Ok(target)
}

/// Leave one `snap-*` snapshot behind (a superseded one and a crashed copy's
/// tmp), so the cache holds a single copy per source.
fn prune(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let stale = (name.starts_with("snap-") && name.ends_with(".sqlite") && path != keep)
            || (name.starts_with("snap-") && name.ends_with(".tmp"));
        if stale {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn hash_path(path: &Path) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A snapshot's `file:` URI: `immutable=1` fits a copy that is never modified
/// in place, and drops the locking, `-shm` and `-wal` machinery entirely.
fn snapshot_uri(path: &Path) -> PathBuf {
    let mut bytes = b"file:".to_vec();
    for &byte in path.as_os_str().as_encoded_bytes() {
        // escape everything outside the URI unreserved set, non-UTF-8 bytes included
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            bytes.push(byte);
        } else {
            bytes.extend_from_slice(format!("%{byte:02X}").as_bytes());
        }
    }
    bytes.extend_from_slice(b"?immutable=1");
    PathBuf::from(OsString::from_vec(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_query_reads_rows_and_absent_nulls() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("places.sqlite");
        {
            let connection = rusqlite::Connection::open(&source).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, note TEXT);
                     INSERT INTO t (name) VALUES ('alpha'), ('beta');",
                )
                .unwrap();
        }

        let mut snapshots = Snapshots::default();
        let id = snapshots
            .snapshot(source.to_str().unwrap(), dir.path())
            .unwrap();
        let rows = snapshots
            .query(
                id,
                "SELECT name FROM t WHERE name LIKE ?1 ORDER BY name",
                vec![rusqlite::types::Value::Text("%a%".to_string())],
            )
            .unwrap();
        assert_eq!(rows[0]["name"], "alpha");

        let rows = snapshots.query(id, "SELECT note FROM t", vec![]).unwrap();
        assert!(rows[0].as_object().unwrap().get("note").is_none());
    }
}
