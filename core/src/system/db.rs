pub mod defaults;
#[cfg(test)]
mod test_support;
pub mod usage;

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};

use crate::system::fs::get_home;

/// Bump when the wire types change; a mismatch drops every table, since a stored
/// `on_click` is a serialized command and cannot survive.
const SCHEMA_VERSION: i64 = 3;

/// The schema every open ensures. `usage` holds the per-action launch counts
/// that rank results, `defaults` the remembered Enter actions.
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS usage (
        on_click TEXT PRIMARY KEY,
        count INTEGER NOT NULL DEFAULT 1
    );
    CREATE TABLE IF NOT EXISTS defaults (
        scope TEXT PRIMARY KEY,
        action_id TEXT NOT NULL
    );";

fn db_path() -> Result<PathBuf> {
    let dir = get_home()?.join(".local/share/wayrun");
    std::fs::create_dir_all(&dir).context("creating the wayrun data directory")?;
    Ok(dir.join("state.db"))
}

fn open_conn() -> Result<Connection> {
    let conn = Connection::open(db_path()?)?;
    // WAL keeps a commit off the fsync path in front of the launch line; a
    // refusal (exotic filesystem) must not empty the history, so it is not propagated.
    let _ =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0));
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    prepare(&conn)?;
    Ok(conn)
}

/// Create the schema at the current version, dropping any older tables first.
fn prepare(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != SCHEMA_VERSION {
        conn.execute_batch("DROP TABLE IF EXISTS usage; DROP TABLE IF EXISTS defaults;")?;
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
                VALUES ('old', 'run:x', '{\"title\":\"old\"}');",
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
    }

    #[test]
    fn a_current_schema_keeps_its_rows() {
        let conn = Connection::open_in_memory().unwrap();
        prepare(&conn).unwrap();
        conn.execute("INSERT INTO usage (on_click) VALUES ('run:x')", [])
            .unwrap();

        prepare(&conn).unwrap();

        assert_eq!(
            conn.query_row("SELECT count(*) FROM usage", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
