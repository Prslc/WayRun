use anyhow::{Context, Result};
use rusqlite::Connection;

use super::with_db;
use crate::wire::ResultItem;

/// Pin one row to the top of one exact query (`""` is the empty-query history).
/// The whole item is stored, re-emitted before its plugin runs.
pub fn pin(scope: &str, item: &ResultItem) -> Result<()> {
    with_db(|conn| pin_with(conn, scope, item))
}

/// Drop one pin by its command key, reporting whether one was really there.
pub fn unpin(scope: &str, key: &str) -> Result<bool> {
    with_db(|conn| unpin_with(conn, scope, key))
}

/// Every pin of one scope, most recently pinned first.
pub fn get_pins(scope: &str) -> Result<Vec<ResultItem>> {
    with_db(|conn| get_pins_with(conn, scope))
}

fn pin_with(conn: &Connection, scope: &str, item: &ResultItem) -> Result<()> {
    let command = item.on_click.as_ref().context("item missing on_click")?;
    let key = command.key();
    let item_json = serde_json::to_string(item)?;
    // Delete-and-insert, not an upsert: a re-pin must get a fresh `id` so it
    // rises to the top of `id`-descending order.
    conn.prepare_cached("DELETE FROM pins WHERE scope = ?1 AND on_click = ?2")?
        .execute(rusqlite::params![scope, key])?;
    conn.prepare_cached(
        "INSERT INTO pins (scope, on_click, item_json, created_at)
         VALUES (?1, ?2, ?3, datetime('now'))",
    )?
    .execute(rusqlite::params![scope, key, item_json])?;
    Ok(())
}

fn unpin_with(conn: &Connection, scope: &str, key: &str) -> Result<bool> {
    let deleted = conn
        .prepare_cached("DELETE FROM pins WHERE scope = ?1 AND on_click = ?2")?
        .execute(rusqlite::params![scope, key])?;
    Ok(deleted > 0)
}

fn get_pins_with(conn: &Connection, scope: &str) -> Result<Vec<ResultItem>> {
    let mut stmt = conn.prepare_cached(
        "SELECT item_json FROM pins WHERE scope = ?1
         ORDER BY id DESC",
    )?;
    let rows = stmt.query_map([scope], |row| row.get::<_, String>(0))?;
    // Same heal the history has: a corrupt row is skipped rather than emitted as
    // a null that would make the shell reject the whole payload.
    Ok(rows
        .filter_map(Result::ok)
        .filter_map(|json| serde_json::from_str::<ResultItem>(&json).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Action;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::system::db::test_support::init_schema(&conn);
        conn
    }

    fn item(title: &str, command: serde_json::Value) -> ResultItem {
        serde_json::from_value(serde_json::json!({ "title": title, "on_click": command }))
            .expect("a test row")
    }

    fn key(command: serde_json::Value) -> String {
        serde_json::from_value::<Action>(command).unwrap().key()
    }

    #[test]
    fn pins_round_trip_per_scope() {
        let conn = test_conn();
        let github = serde_json::json!({ "type": "open", "uri": "https://github.com" });
        let docs = serde_json::json!({ "type": "open", "uri": "https://docs.rs" });
        let files = serde_json::json!({ "type": "launch", "desktop_id": "files.desktop" });
        pin_with(&conn, "b firefox", &item("GitHub", github.clone())).unwrap();
        pin_with(&conn, "b firefox", &item("Docs", docs.clone())).unwrap();
        pin_with(&conn, "", &item("Files", files)).unwrap();

        // a scope sees only its own pins, most recent first
        let items = get_pins_with(&conn, "b firefox").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Docs");
        assert_eq!(get_pins_with(&conn, "").unwrap().len(), 1);

        assert!(unpin_with(&conn, "b firefox", &key(github.clone())).unwrap());
        assert!(!unpin_with(&conn, "b firefox", &key(github)).unwrap());
        assert_eq!(get_pins_with(&conn, "b firefox").unwrap().len(), 1);
    }

    #[test]
    fn pinning_the_same_target_moves_it_to_the_front() {
        let conn = test_conn();
        let a = serde_json::json!({ "type": "run", "cmd": "a" });
        let b = serde_json::json!({ "type": "run", "cmd": "b" });
        pin_with(&conn, "gh", &item("A", a.clone())).unwrap();
        pin_with(&conn, "gh", &item("B", b)).unwrap();
        pin_with(&conn, "gh", &item("A v2", a)).unwrap();

        let items = get_pins_with(&conn, "gh").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "A v2", "the re-pin wins and surfaces");
        assert_eq!(items[1].title, "B");
    }

    #[test]
    fn a_pin_without_a_target_is_rejected() {
        let conn = test_conn();
        let row: ResultItem = serde_json::from_str(r#"{"title":"no target"}"#).unwrap();
        assert!(pin_with(&conn, "gh", &row).is_err());
    }
}
