use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};

use crate::system::fs::get_home;

/// Bump when the wire types change; a mismatch drops the history and pins, since
/// a row's `on_click` is stored as its serialized command and cannot survive.
const SCHEMA_VERSION: i64 = 2;

/// The schema every open ensures. `usage` is the ranked history behind an empty
/// query, `pins` the per-query favorites.
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS usage (
        key TEXT PRIMARY KEY,
        on_click TEXT,
        count INTEGER NOT NULL DEFAULT 1,
        last_used_at TEXT NOT NULL DEFAULT (datetime('now')),
        item_json TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS pins (
        id INTEGER PRIMARY KEY,
        scope TEXT NOT NULL,
        on_click TEXT NOT NULL,
        item_json TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        UNIQUE (scope, on_click)
    );";

fn db_path() -> Result<PathBuf> {
    let dir = get_home()?.join(".local/share/wayrun");
    std::fs::create_dir_all(&dir).context("creating the wayrun data directory")?;
    Ok(dir.join("usage.db"))
}

fn open_conn() -> Result<Connection> {
    let conn = Connection::open(db_path()?)?;
    prepare(&conn)?;
    Ok(conn)
}

/// Create the schema at the current version, dropping any older tables first.
fn prepare(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != SCHEMA_VERSION {
        conn.execute_batch("DROP TABLE IF EXISTS usage; DROP TABLE IF EXISTS pins;")?;
    }
    conn.execute_batch(SCHEMA)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// One connection for the process lifetime, opened lazily and left unset on
/// failure (no `$HOME`, full disk) so history degrades to empty, not a panic.
static DB: LazyLock<Mutex<Option<Connection>>> = LazyLock::new(|| Mutex::new(None));

/// Run `f` on the shared connection, surviving lock poisoning.
pub fn with_db<T>(f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let mut guard = DB.lock().unwrap_or_else(PoisonError::into_inner);
    if guard.is_none() {
        *guard = Some(open_conn()?);
    }
    f(guard.as_ref().expect("opened just above"))
}

#[cfg(test)]
pub fn init_schema(conn: &Connection) {
    conn.execute_batch(SCHEMA).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_schema_is_dropped_and_recreated() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE usage (
                key TEXT PRIMARY KEY,
                on_click TEXT,
                count INTEGER NOT NULL DEFAULT 1,
                last_used_at TEXT NOT NULL DEFAULT (datetime('now')),
                item_json TEXT NOT NULL
            );
            INSERT INTO usage (key, on_click, item_json)
                VALUES ('old', 'run:x', '{\"title\":\"old\"}');
            CREATE TABLE pins (scope TEXT, on_click TEXT, item_json TEXT);",
        )
        .unwrap();

        prepare(&conn).unwrap();

        assert_eq!(
            conn.query_row("SELECT count(*) FROM usage", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0,
            "an old scheme row does not survive"
        );
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        // the current pins shape was rebuilt
        assert!(column_exists(&conn, "pins", "id"));
    }

    #[test]
    fn a_current_schema_keeps_its_rows() {
        let conn = Connection::open_in_memory().unwrap();
        prepare(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage (key, on_click, item_json) VALUES ('k', 'c', '{}')",
            [],
        )
        .unwrap();

        prepare(&conn).unwrap();

        assert_eq!(
            conn.query_row("SELECT count(*) FROM usage", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        let mut names = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
        names.any(|name| name.is_ok_and(|name| name == column))
    }
}
