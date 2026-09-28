use anyhow::{Context, Result};
use rusqlite::Connection;

use super::with_db;
use crate::wire::{Action, ResultItem};

pub fn record(item_json: &str) -> Result<()> {
    with_db(|conn| record_with(conn, item_json))
}

/// The usage counts behind the given action keys, for a caller ordering a whole
/// payload at once: one query, not one per row.
pub fn counts(keys: &[String]) -> std::collections::HashMap<String, u32> {
    if keys.is_empty() {
        return std::collections::HashMap::default();
    }
    with_db(|conn| counts_with(conn, keys)).unwrap_or_default()
}

/// Rows per `IN (...)` batch. Batching keeps the SQL text one of two shapes, so
/// sqlite parses it once instead of once per payload size.
const COUNT_BATCH: usize = 100;

fn counts_with(
    conn: &Connection,
    keys: &[String],
) -> Result<std::collections::HashMap<String, u32>> {
    let mut counts = std::collections::HashMap::default();
    let (full, tail) = keys.split_at(keys.len() / COUNT_BATCH * COUNT_BATCH);
    for keys in full
        .chunks(COUNT_BATCH)
        .chain((!tail.is_empty()).then_some(tail))
    {
        let placeholders = vec!["?"; keys.len()].join(",");
        let sql = format!("SELECT on_click, count FROM usage WHERE on_click IN ({placeholders})");
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(keys), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;
        for row in rows {
            let (key, count) = row?;
            counts.insert(key, count);
        }
    }
    Ok(counts)
}

fn record_with(conn: &Connection, item_json: &str) -> Result<()> {
    let item: ResultItem = serde_json::from_str(item_json)?;
    let command = item.on_click.as_ref().context("item missing on_click")?;
    // A copy is no re-launchable target, so it stays out of the counts.
    if matches!(command, Action::Copy { .. }) {
        return Ok(());
    }
    let key = command.key();

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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::system::db::test_support::init_schema(&conn);
        conn
    }

    fn run(cmd: &str) -> serde_json::Value {
        serde_json::json!({ "type": "run", "cmd": cmd })
    }

    fn item(title: &str, command: serde_json::Value) -> String {
        serde_json::json!({ "title": title, "on_click": command }).to_string()
    }

    #[test]
    fn record_accumulates_a_count() {
        let conn = test_conn();
        let json = item("Firefox", run("firefox"));
        record_with(&conn, &json).unwrap();
        record_with(&conn, &json).unwrap();

        let count: i64 = conn
            .query_row("SELECT count FROM usage WHERE key = 'Firefox'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn counts_span_more_than_one_batch() {
        let conn = test_conn();
        for i in 0..COUNT_BATCH + 3 {
            record_with(&conn, &item(&format!("Row {i}"), run(&format!("cmd{i}")))).unwrap();
        }
        let keys: Vec<String> = (0..COUNT_BATCH + 3)
            .map(|i| {
                Action::Run {
                    cmd: format!("cmd{i}"),
                }
                .key()
            })
            .collect();

        let counts = counts_with(&conn, &keys).unwrap();

        assert_eq!(counts.len(), COUNT_BATCH + 3);
        assert!(counts.values().all(|count| *count == 1));
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
    }

    #[test]
    fn empty_title_falls_back_to_action() {
        let conn = test_conn();
        record_with(&conn, &item("", run("a"))).unwrap();
        record_with(&conn, &item("", run("b"))).unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM usage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);
    }

    #[test]
    fn a_copy_row_is_not_recorded() {
        let conn = test_conn();
        let copy = serde_json::json!({
            "title": "Clipboard",
            "on_click": { "type": "copy", "text": "clip" },
        })
        .to_string();
        record_with(&conn, &copy).unwrap();

        // A URL, a launch, a command and a desktop action are all re-launchable targets.
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

        let rows: i64 = conn
            .query_row("SELECT count(*) FROM usage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 4, "only the recordable rows landed");
    }
}
