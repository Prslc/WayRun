use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::system::db::with_db;
use crate::wire::{Action, ResultItem};

pub fn record(item_json: &str) -> Result<()> {
    with_db(|conn| record_with(conn, item_json))
}

/// Drop one history entry by its command key, reporting whether a row was there.
pub fn forget(key: &str) -> Result<bool> {
    with_db(|conn| forget_with(conn, key))
}

pub fn get_top(limit: i32) -> Result<Vec<ResultItem>> {
    with_db(|conn| get_top_with(conn, limit))
}

/// The usage counts behind the given action keys, for a caller ordering a whole
/// payload at once: one query, not one per row.
pub fn counts(keys: &[String]) -> std::collections::HashMap<String, u32> {
    if keys.is_empty() {
        return std::collections::HashMap::default();
    }
    with_db(|conn| counts_with(conn, keys)).unwrap_or_default()
}

fn counts_with(
    conn: &Connection,
    keys: &[String],
) -> Result<std::collections::HashMap<String, u32>> {
    let placeholders = vec!["?"; keys.len()].join(",");
    let sql = format!("SELECT on_click, count FROM usage WHERE on_click IN ({placeholders})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(keys), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
    })?;
    let mut counts = std::collections::HashMap::default();
    for row in rows {
        let (key, count) = row?;
        counts.insert(key, count);
    }
    Ok(counts)
}

/// An `ephemeral` row or a `copy` command is not re-launchable and stays out of
/// history; the field/variant carries the semantics for every source.
pub(crate) fn is_recordable(ephemeral: bool, command: Option<&Action>) -> bool {
    !ephemeral && !matches!(command, Some(Action::Copy { .. }))
}

fn record_with(conn: &Connection, item_json: &str) -> Result<()> {
    let item: ResultItem = serde_json::from_str(item_json)?;
    if !is_recordable(item.ephemeral, item.on_click.as_ref()) {
        return Ok(());
    }
    let key = item.on_click.context("item missing on_click")?.key();

    // Key by display title so alternate launch actions for one app merge
    // into a single entry; empty titles fall back to the command key.
    let key_column = if item.title.is_empty() {
        key.clone()
    } else {
        item.title.clone()
    };

    conn.prepare_cached(
        "INSERT INTO usage (key, on_click, count, last_used_at, item_json)
         VALUES (?1, ?2, 1, datetime('now'), ?3)
         ON CONFLICT(key) DO UPDATE SET
             count = count + 1,
             last_used_at = datetime('now'),
             on_click = ?2,
             item_json = ?3",
    )?
    .execute(rusqlite::params![key_column, key, item_json])?;
    Ok(())
}

/// Drop one history entry by its command key, resolved through the row's title so
/// a merged row (same title, several actions) is removed whole. `true` if deleted.
fn forget_with(conn: &Connection, key: &str) -> Result<bool> {
    let title: Option<String> = conn
        .prepare_cached("SELECT key FROM usage WHERE on_click = ?1 LIMIT 1")?
        .query_row([key], |r| r.get(0))
        .ok();
    let deleted = conn
        .prepare_cached("DELETE FROM usage WHERE key = ?1")?
        .execute([title.as_deref().unwrap_or(key)])?;
    Ok(deleted > 0)
}

fn get_top_with(conn: &Connection, limit: i32) -> Result<Vec<ResultItem>> {
    let mut stmt = conn.prepare_cached(
        "SELECT item_json FROM usage ORDER BY count DESC, last_used_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |row| row.get::<_, String>(0))?;

    // A corrupt row, or one with no `title`, is skipped rather than emitted as
    // `null`: `title` is required on the wire, and one bad entry rejects all.
    Ok(rows
        .filter_map(Result::ok)
        .filter_map(|json| serde_json::from_str::<ResultItem>(&json).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::system::db::init_schema(&conn);
        conn
    }

    fn run(cmd: &str) -> serde_json::Value {
        serde_json::json!({ "type": "run", "cmd": cmd })
    }

    fn item(title: &str, command: serde_json::Value) -> String {
        serde_json::json!({ "title": title, "on_click": command }).to_string()
    }

    #[test]
    fn record_and_get_top() {
        let conn = test_conn();
        let json = item("Firefox", run("firefox"));
        record_with(&conn, &json).unwrap();
        record_with(&conn, &json).unwrap();

        let items = get_top_with(&conn, 10).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Firefox");
    }

    #[test]
    fn a_corrupt_or_titleless_row_does_not_poison_the_history() {
        let conn = test_conn();
        record_with(&conn, &item("Good", run("good"))).unwrap();
        conn.execute(
            "INSERT INTO usage (key, on_click, item_json) VALUES ('bad', 'x', 'not json')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage (key, on_click, item_json) VALUES ('empty', 'x', '{\"on_click\":{}}')",
            [],
        )
        .unwrap();

        let items = get_top_with(&conn, 10).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Good");
    }

    #[test]
    fn forget_removes_entry() {
        let conn = test_conn();
        record_with(&conn, &item("A", run("a"))).unwrap();
        assert_eq!(get_top_with(&conn, 10).unwrap().len(), 1);

        let key = Action::Run {
            cmd: "a".to_string(),
        }
        .key();
        assert!(forget_with(&conn, &key).unwrap(), "a row was there");
        assert!(get_top_with(&conn, 10).unwrap().is_empty());
        assert!(
            !forget_with(&conn, &key).unwrap(),
            "nothing left to drop, so `forget` answers false"
        );
    }

    #[test]
    fn top_is_sorted_by_count() {
        let conn = test_conn();
        record_with(&conn, &item("A", run("a"))).unwrap();
        record_with(&conn, &item("B", run("b"))).unwrap();
        record_with(&conn, &item("B", run("b"))).unwrap();

        let items = get_top_with(&conn, 10).unwrap();
        assert_eq!(items[0].title, "B");
        assert_eq!(items[1].title, "A");
    }

    #[test]
    fn record_missing_on_click() {
        let conn = test_conn();
        assert!(record_with(&conn, r#"{"title":"no key"}"#).is_err());
    }

    #[test]
    fn same_title_actions_merge_into_one_entry() {
        let conn = test_conn();
        record_with(&conn, &item("Telegram", run("Telegram --"))).unwrap();
        let launch = serde_json::json!({
            "type": "launch",
            "desktop_id": "org.telegram.desktop.desktop",
        });
        record_with(&conn, &item("Telegram", launch.clone())).unwrap();
        record_with(&conn, &item("Telegram", launch)).unwrap();

        let count: i64 = conn
            .query_row("SELECT count FROM usage WHERE key = 'Telegram'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 3);

        let items = get_top_with(&conn, 10).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Telegram");
        // the merged entry keeps the most recently used command
        assert!(matches!(items[0].on_click, Some(Action::Launch { .. })));
    }

    #[test]
    fn forget_removes_merged_siblings() {
        let conn = test_conn();
        record_with(&conn, &item("Telegram", run("Telegram --"))).unwrap();
        let launch = serde_json::json!({
            "type": "launch",
            "desktop_id": "org.telegram.desktop.desktop",
        });
        record_with(&conn, &item("Telegram", launch.clone())).unwrap();

        // `forget` passes the current command, dropping the whole merged entry
        let key = Action::Launch {
            desktop_id: "org.telegram.desktop.desktop".to_string(),
        }
        .key();
        forget_with(&conn, &key).unwrap();
        assert!(get_top_with(&conn, 10).unwrap().is_empty());
    }

    #[test]
    fn empty_title_falls_back_to_action() {
        let conn = test_conn();
        record_with(&conn, &item("", run("a"))).unwrap();
        record_with(&conn, &item("", run("b"))).unwrap();
        assert_eq!(get_top_with(&conn, 10).unwrap().len(), 2);
    }

    #[test]
    fn one_shot_rows_are_never_recorded() {
        let conn = test_conn();
        let copy = serde_json::json!({
            "title": "Clipboard",
            "on_click": { "type": "copy", "text": "clip" },
            "ephemeral": false,
        })
        .to_string();
        let github = serde_json::json!({
            "title": "github hit",
            "on_click": { "type": "open", "uri": "https://github.com/Prslc/WayRun" },
            "ephemeral": true,
        })
        .to_string();
        record_with(&conn, &copy).unwrap();
        record_with(&conn, &github).unwrap();

        // A URL the host did not mark, plus a launch, a command and a desktop
        // action, are all re-launchable targets.
        for (title, command) in [
            (
                "Prslc/WayRun",
                serde_json::json!({ "type": "open", "uri": "https://github.com/Prslc/WayRun" }),
            ),
            (
                "Firefox",
                serde_json::json!({ "type": "launch", "desktop_id": "firefox.desktop" }),
            ),
            ("btop", serde_json::json!({ "type": "run", "cmd": "btop" })),
            (
                "New Window",
                serde_json::json!({
                    "type": "desktop_action",
                    "desktop_id": "firefox.desktop",
                    "action_id": "new-window",
                }),
            ),
        ] {
            record_with(&conn, &item(title, command)).unwrap();
        }

        let mut titles: Vec<String> = get_top_with(&conn, 10)
            .unwrap()
            .iter()
            .map(|item| item.title.clone())
            .collect();
        titles.sort();
        assert_eq!(titles, ["Firefox", "New Window", "Prslc/WayRun", "btop"]);
    }
}
