use rusqlite::Connection;

use super::SCHEMA;

pub(super) fn init_schema(conn: &Connection) {
    conn.execute_batch(SCHEMA).ok();
}
