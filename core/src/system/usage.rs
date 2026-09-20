use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::system::db::with_db;

pub fn record(item_json: &str) -> Result<()> {
    with_db(|conn| record_with(conn, item_json))
}

/// Drop one history entry, reporting whether a row was really there.
pub fn forget(on_click: &str) -> Result<bool> {
    with_db(|conn| forget_with(conn, on_click))
}

pub fn get_top(limit: i32) -> Result<Vec<serde_json::Value>> {
    with_db(|conn| get_top_with(conn, limit))
}

/// An `ephemeral` row or a `copy:` write is not re-launchable and stays out of
/// history; the field/scheme carries the semantics for every source.
pub(crate) fn is_recordable(ephemeral: bool, on_click: Option<&str>) -> bool {
    !ephemeral && !on_click.is_some_and(|on_click| on_click.starts_with("copy:"))
}

fn record_with(conn: &Connection, item_json: &str) -> Result<()> {
    let item: serde_json::Value = serde_json::from_str(item_json)?;
    if !is_recordable(
        item["ephemeral"].as_bool().unwrap_or(false),
        item["on_click"].as_str(),
    ) {
        return Ok(());
    }
    let on_click = item["on_click"].as_str().context("item missing on_click")?;

    // Key by display title so alternate launch actions for one app merge
    // into a single entry; empty titles fall back to the action.
    let title = item["title"].as_str().context("item missing title")?;
    let key = if title.is_empty() {
        on_click.to_string()
    } else {
        title.to_string()
    };

    conn.execute(
        "INSERT INTO usage (key, on_click, count, last_used_at, item_json)
         VALUES (?1, ?2, 1, datetime('now'), ?3)
         ON CONFLICT(key) DO UPDATE SET
             count = count + 1,
             last_used_at = datetime('now'),
             on_click = ?2,
             item_json = ?3",
        rusqlite::params![key, on_click, item_json],
    )?;
    Ok(())
}

/// Drop one history entry by its `on_click`, resolved through the row's title so
/// a merged row (same title, several actions) is removed whole. `true` if deleted.
fn forget_with(conn: &Connection, on_click: &str) -> Result<bool> {
    let title: Option<String> = conn
        .query_row(
            "SELECT key FROM usage WHERE on_click = ?1 LIMIT 1",
            [on_click],
            |r| r.get(0),
        )
        .ok();
    let deleted = match title {
        Some(t) => conn.execute("DELETE FROM usage WHERE key = ?1", [t])?,
        // legacy keyed-by-action rows that never went through migrate
        None => conn.execute("DELETE FROM usage WHERE key = ?1", [on_click])?,
    };
    Ok(deleted > 0)
}

/// Drop `copy:` rows, which are not re-launchable targets. Runs at core
/// startup so an older database heals on upgrade; idempotent.
pub fn purge_ephemeral() -> Result<()> {
    with_db(purge_with)
}

fn purge_with(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM usage WHERE on_click LIKE 'copy:%'", [])?;
    Ok(())
}

fn get_top_with(conn: &Connection, limit: i32) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn
        .prepare("SELECT item_json FROM usage ORDER BY count DESC, last_used_at DESC LIMIT ?1")?;
    let rows = stmt.query_map([limit], |row| row.get::<_, String>(0))?;

    // A corrupt row, or one with no `title`, is skipped rather than emitted as
    // `null`: `title` is required on the wire, and one bad entry rejects all.
    Ok(rows
        .filter_map(Result::ok)
        .filter_map(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .filter(|value| value.get("title").and_then(|t| t.as_str()).is_some())
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

    #[test]
    fn record_and_get_top() {
        let conn = test_conn();
        let json = r#"{"title":"Firefox","on_click":"run:firefox"}"#;
        record_with(&conn, json).unwrap();
        record_with(&conn, json).unwrap();

        let items = get_top_with(&conn, 10).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "Firefox");
    }

    #[test]
    fn a_corrupt_or_titleless_row_does_not_poison_the_history() {
        let conn = test_conn();
        record_with(&conn, r#"{"title":"Good","on_click":"run:good"}"#).unwrap();
        conn.execute(
            "INSERT INTO usage (key, on_click, item_json) VALUES ('bad', 'run:bad', 'not json')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage (key, on_click, item_json) VALUES ('empty', 'run:empty', '{\"on_click\":\"run:empty\"}')",
            [],
        )
        .unwrap();

        let items = get_top_with(&conn, 10).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "Good");
    }

    #[test]
    fn forget_removes_entry() {
        let conn = test_conn();
        record_with(&conn, r#"{"title":"A","on_click":"run:a"}"#).unwrap();
        assert_eq!(get_top_with(&conn, 10).unwrap().len(), 1);

        assert!(forget_with(&conn, "run:a").unwrap(), "a row was there");
        assert!(get_top_with(&conn, 10).unwrap().is_empty());
        assert!(
            !forget_with(&conn, "run:a").unwrap(),
            "nothing left to drop, so `forget` answers false"
        );
    }

    #[test]
    fn top_is_sorted_by_count() {
        let conn = test_conn();
        record_with(&conn, r#"{"title":"A","on_click":"run:a"}"#).unwrap();
        record_with(&conn, r#"{"title":"B","on_click":"run:b"}"#).unwrap();
        record_with(&conn, r#"{"title":"B","on_click":"run:b"}"#).unwrap();

        let items = get_top_with(&conn, 10).unwrap();
        assert_eq!(items[0]["title"], "B");
        assert_eq!(items[1]["title"], "A");
    }

    #[test]
    fn record_missing_on_click() {
        let conn = test_conn();
        assert!(record_with(&conn, r#"{"title":"no key"}"#).is_err());
    }

    #[test]
    fn same_title_actions_merge_into_one_entry() {
        let conn = test_conn();
        // both on_click forms for the same app
        record_with(
            &conn,
            r#"{"title":"Telegram","on_click":"run:Telegram --"}"#,
        )
        .unwrap();
        record_with(
            &conn,
            r#"{"title":"Telegram","on_click":"launch:org.telegram.desktop.desktop"}"#,
        )
        .unwrap();
        record_with(
            &conn,
            r#"{"title":"Telegram","on_click":"launch:org.telegram.desktop.desktop"}"#,
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT count FROM usage WHERE key = 'Telegram'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 3);

        let items = get_top_with(&conn, 10).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "Telegram");
        // the merged entry keeps the most recently used action
        assert!(
            items[0]["on_click"]
                .as_str()
                .unwrap()
                .starts_with("launch:")
        );
    }

    #[test]
    fn forget_removes_merged_siblings() {
        let conn = test_conn();
        record_with(
            &conn,
            r#"{"title":"Telegram","on_click":"run:Telegram --"}"#,
        )
        .unwrap();
        record_with(
            &conn,
            r#"{"title":"Telegram","on_click":"launch:org.telegram.desktop.desktop"}"#,
        )
        .unwrap();

        // the panel's `forget` passes the current launch: action — the whole
        // merged entry goes
        forget_with(&conn, "launch:org.telegram.desktop.desktop").unwrap();
        assert!(get_top_with(&conn, 10).unwrap().is_empty());
    }

    #[test]
    fn empty_title_falls_back_to_action() {
        let conn = test_conn();
        record_with(&conn, r#"{"title":"","on_click":"run:a"}"#).unwrap();
        record_with(&conn, r#"{"title":"","on_click":"run:b"}"#).unwrap();
        assert_eq!(get_top_with(&conn, 10).unwrap().len(), 2);
    }

    #[test]
    fn one_shot_rows_are_never_recorded() {
        let conn = test_conn();
        for (title, on_click, ephemeral) in [
            ("Clipboard", r#"copy:{"text": "clip"}"#, false),
            ("github hit", "https://github.com/Prslc/WayRun", true),
        ] {
            record_with(
                &conn,
                &serde_json::json!({
                    "title": title,
                    "on_click": on_click,
                    "ephemeral": ephemeral,
                })
                .to_string(),
            )
            .unwrap();
        }
        // A URL the host did not mark, plus a launch, a command and a desktop
        // action, are all re-launchable targets.
        for (title, on_click) in [
            ("Prslc/WayRun", "https://github.com/Prslc/WayRun"),
            ("Firefox", "launch:firefox.desktop"),
            ("btop", "run:btop"),
            ("New Window", "action:firefox.desktop:new-window"),
        ] {
            record_with(
                &conn,
                &serde_json::json!({ "title": title, "on_click": on_click }).to_string(),
            )
            .unwrap();
        }

        let mut titles: Vec<String> = get_top_with(&conn, 10)
            .unwrap()
            .iter()
            .filter_map(|item| item["title"].as_str().map(str::to_owned))
            .collect();
        titles.sort();
        assert_eq!(titles, ["Firefox", "New Window", "Prslc/WayRun", "btop"]);
    }

    #[test]
    fn purge_removes_legacy_copy_rows() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO usage (key, on_click, count, last_used_at, item_json)
             VALUES ('copy:{\"text\": \"old\"}', 'copy:{\"text\": \"old\"}', 4,
                     datetime('now'), '{\"title\":\"old\"}')",
            [],
        )
        .unwrap();
        record_with(&conn, r#"{"title":"A","on_click":"run:a"}"#).unwrap();
        record_with(&conn, r#"{"title":"URL","on_click":"https://example.com"}"#).unwrap();

        purge_with(&conn).unwrap();

        let mut titles: Vec<String> = get_top_with(&conn, 10)
            .unwrap()
            .iter()
            .filter_map(|item| item["title"].as_str().map(str::to_owned))
            .collect();
        titles.sort();
        assert_eq!(titles, ["A", "URL"], "a URL row is not ephemeral");
    }
}
