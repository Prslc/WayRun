use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;

use crate::system::db::with_db;

/// Remember `action_id` as the default Enter action for one plugin scope.
pub fn set(scope: &str, action_id: &str) -> Result<()> {
    with_db(|conn| set_with(conn, scope, action_id))
}

/// Forget a scope's default action.
pub fn clear(scope: &str) -> Result<()> {
    with_db(|conn| clear_with(conn, scope))
}

/// Every remembered default, keyed by scope (plugin id).
pub fn all() -> Result<HashMap<String, String>> {
    with_db(all_with)
}

fn set_with(conn: &Connection, scope: &str, action_id: &str) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO defaults (scope, action_id) VALUES (?1, ?2)
         ON CONFLICT(scope) DO UPDATE SET action_id = ?2",
    )?
    .execute(rusqlite::params![scope, action_id])?;
    Ok(())
}

fn clear_with(conn: &Connection, scope: &str) -> Result<()> {
    conn.prepare_cached("DELETE FROM defaults WHERE scope = ?1")?
        .execute([scope])?;
    Ok(())
}

fn all_with(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare_cached("SELECT scope, action_id FROM defaults")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::system::db::init_schema(&conn);
        conn
    }

    #[test]
    fn a_default_round_trips_per_scope() {
        let conn = test_conn();
        set_with(&conn, "file-search", "terminal").unwrap();
        set_with(&conn, "web-search", "copy_url").unwrap();
        assert_eq!(all_with(&conn).unwrap()["file-search"], "terminal");

        // re-setting a scope replaces it
        set_with(&conn, "file-search", "reveal").unwrap();
        let all = all_with(&conn).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all["file-search"], "reveal");

        clear_with(&conn, "file-search").unwrap();
        assert!(!all_with(&conn).unwrap().contains_key("file-search"));
    }
}
