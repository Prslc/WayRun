use anyhow::Result;
use rusqlite::Connection;

use super::with_db;
use crate::wire::Action;

/// Bump one command's launch count.
pub fn record(command: &Action) -> Result<()> {
    with_db(|conn| record_with(conn, command))
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

fn record_with(conn: &Connection, command: &Action) -> Result<()> {
    // A copy is no re-launchable target, so it stays out of the counts.
    if matches!(command, Action::Copy { .. }) {
        return Ok(());
    }
    conn.prepare_cached(
        "INSERT INTO usage (on_click, count) VALUES (?1, 1)
         ON CONFLICT(on_click) DO UPDATE SET count = count + 1",
    )?
    .execute([command.key()])?;
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

    fn run(cmd: &str) -> Action {
        Action::Run {
            cmd: cmd.to_string(),
        }
    }

    fn count(conn: &Connection, command: &Action) -> u32 {
        conn.query_row(
            "SELECT count FROM usage WHERE on_click = ?1",
            [command.key()],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM usage", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn record_accumulates_a_count() {
        let conn = test_conn();
        let firefox = run("firefox");
        record_with(&conn, &firefox).unwrap();
        record_with(&conn, &firefox).unwrap();

        assert_eq!(count(&conn, &firefox), 2);
    }

    #[test]
    fn each_action_counts_on_its_own() {
        let conn = test_conn();
        let launch = Action::Launch {
            desktop_id: "org.telegram.desktop.desktop".to_string(),
        };
        record_with(&conn, &run("Telegram --")).unwrap();
        record_with(&conn, &launch).unwrap();
        record_with(&conn, &launch).unwrap();

        assert_eq!(count(&conn, &run("Telegram --")), 1);
        assert_eq!(count(&conn, &launch), 2);
    }

    #[test]
    fn counts_span_more_than_one_batch() {
        let conn = test_conn();
        for i in 0..COUNT_BATCH + 3 {
            record_with(&conn, &run(&format!("cmd{i}"))).unwrap();
        }
        let keys: Vec<String> = (0..COUNT_BATCH + 3)
            .map(|i| run(&format!("cmd{i}")).key())
            .collect();

        let counts = counts_with(&conn, &keys).unwrap();

        assert_eq!(counts.len(), COUNT_BATCH + 3);
        assert!(counts.values().all(|count| *count == 1));
    }

    #[test]
    fn a_copy_is_not_recorded() {
        let conn = test_conn();
        record_with(
            &conn,
            &Action::Copy {
                text: "clip".to_string(),
            },
        )
        .unwrap();
        record_with(&conn, &run("btop")).unwrap();

        assert_eq!(rows(&conn), 1, "only the re-launchable command landed");
    }
}
