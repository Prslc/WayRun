use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context, Result};
use mlua::{Lua, Table, Value};
use rusqlite::OptionalExtension;

/// The plugin a call is being served for; the bindings below resolve their
/// store under that id, so a multi-plugin script keeps one per plugin.
pub(super) type Active = Rc<RefCell<Option<String>>>;

/// Every declared plugin's `kv.db` path, filled once the script's ids are known.
pub(super) type Paths = Rc<RefCell<HashMap<String, PathBuf>>>;

#[derive(Default)]
struct Stores {
    open: HashMap<String, rusqlite::Connection>,
}

pub(super) fn lib(lua: &Lua, active: Active, paths: Paths) -> mlua::Result<Table> {
    let kv = lua.create_table()?;
    let stores = Rc::new(RefCell::new(Stores::default()));

    let get_stores = Rc::clone(&stores);
    let get_active = Rc::clone(&active);
    let get_paths = Rc::clone(&paths);
    kv.set(
        "get",
        lua.create_function(move |lua, key: String| {
            let (id, path) = database(&get_active, &get_paths)
                .map_err(|problem| mlua::Error::RuntimeError(format!("kv.get: {problem}")))?;
            let value = get_stores
                .borrow_mut()
                .get(&id, &path, &key, super::sdk::now_seconds())
                .map_err(|error| mlua::Error::RuntimeError(format!("kv.get: {error:#}")))?;
            match value {
                Some(text) => Ok(Value::String(lua.create_string(text)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    let set_stores = Rc::clone(&stores);
    let set_active = Rc::clone(&active);
    let set_paths = Rc::clone(&paths);
    kv.set(
        "set",
        lua.create_function(move |_, (key, value, ttl): (String, String, Option<u64>)| {
            let (id, path) = database(&set_active, &set_paths)
                .map_err(|problem| mlua::Error::RuntimeError(format!("kv.set: {problem}")))?;
            let expires_at = ttl.map(|secs| super::sdk::now_seconds() + secs as i64);
            set_stores
                .borrow_mut()
                .set(&id, &path, &key, &value, expires_at)
                .map_err(|error| mlua::Error::RuntimeError(format!("kv.set: {error:#}")))?;
            Ok(true)
        })?,
    )?;

    let delete_stores = Rc::clone(&stores);
    let delete_active = Rc::clone(&active);
    let delete_paths = Rc::clone(&paths);
    kv.set(
        "delete",
        lua.create_function(move |_, key: String| {
            let (id, path) = database(&delete_active, &delete_paths)
                .map_err(|problem| mlua::Error::RuntimeError(format!("kv.delete: {problem}")))?;
            delete_stores
                .borrow_mut()
                .delete(&id, &path, &key)
                .map_err(|error| mlua::Error::RuntimeError(format!("kv.delete: {error:#}")))?;
            Ok(true)
        })?,
    )?;

    Ok(kv)
}

fn database(active: &Active, paths: &Paths) -> std::result::Result<(String, PathBuf), String> {
    let Some(id) = active.borrow().clone() else {
        return Err("no plugin in the call context".to_string());
    };
    match paths.borrow().get(&id) {
        Some(path) => Ok((id, path.clone())),
        None => Err(format!("plugin {id:?} declares no store")),
    }
}

impl Stores {
    fn connection(&mut self, id: &str, path: &Path) -> Result<&rusqlite::Connection> {
        if !self.open.contains_key(id) {
            let connection = rusqlite::Connection::open(path)?;
            connection.busy_timeout(Duration::from_secs(2))?;
            connection.pragma_update(None, "journal_mode", "WAL")?;
            connection.pragma_update(None, "synchronous", "NORMAL")?;
            connection.execute_batch(
                "CREATE TABLE IF NOT EXISTS kv (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL,
                    expires_at INTEGER
                )",
            )?;
            self.open.insert(id.to_string(), connection);
        }
        Ok(self.open.get(id).expect("just inserted"))
    }

    fn get(&mut self, id: &str, path: &Path, key: &str, now: i64) -> Result<Option<String>> {
        if !path.is_file() {
            return Ok(None);
        }
        let connection = self.connection(id, path)?;
        let row = connection
            .prepare_cached("SELECT value, expires_at FROM kv WHERE key = ?1")?
            .query_row([key], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .optional()?;
        match row {
            None => Ok(None),
            // An expired key is dropped by the read that found it stale.
            Some((_, Some(expires_at))) if expires_at <= now => {
                connection.execute("DELETE FROM kv WHERE key = ?1", [key])?;
                Ok(None)
            }
            Some((value, _)) => Ok(Some(value)),
        }
    }

    fn set(
        &mut self,
        id: &str,
        path: &Path,
        key: &str,
        value: &str,
        expires_at: Option<i64>,
    ) -> Result<()> {
        let Some(parent) = path.parent() else {
            anyhow::bail!("{} has no parent directory", path.display());
        };
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        let connection = self.connection(id, path)?;
        connection.execute(
            "INSERT INTO kv (key, value, expires_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, expires_at = excluded.expires_at",
            rusqlite::params![key, value, expires_at],
        )?;
        Ok(())
    }

    fn delete(&mut self, id: &str, path: &Path, key: &str) -> Result<()> {
        if !path.is_file() {
            return Ok(());
        }
        let connection = self.connection(id, path)?;
        connection.execute("DELETE FROM kv WHERE key = ?1", [key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_with_kv(dir: &Path) -> (Lua, Active) {
        let lua = Lua::new();
        let active: Active = Rc::new(RefCell::new(Some("demo".to_string())));
        let paths: Paths = Rc::new(RefCell::new(
            ["demo", "other"]
                .into_iter()
                .map(|id| (id.to_string(), dir.join(id).join("kv.db")))
                .collect(),
        ));
        lua.globals()
            .set("kv", lib(&lua, Rc::clone(&active), paths).unwrap())
            .unwrap();
        (lua, active)
    }

    #[test]
    fn set_get_and_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let (lua, _) = lua_with_kv(dir.path());
        let stored: Option<String> = lua
            .load(r#"kv.set("k", "v"); return kv.get("k")"#)
            .eval()
            .unwrap();
        assert_eq!(stored.as_deref(), Some("v"));

        let gone: Option<String> = lua
            .load(r#"kv.delete("k"); return kv.get("k")"#)
            .eval()
            .unwrap();
        assert_eq!(gone, None);
    }

    #[test]
    fn a_ttl_expires_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let (lua, _) = lua_with_kv(dir.path());
        let expired: Option<String> = lua
            .load(r#"kv.set("k", "v", 0); return kv.get("k")"#)
            .eval()
            .unwrap();
        assert_eq!(expired, None, "a ttl of zero is already past");

        let kept: Option<String> = lua
            .load(r#"kv.set("k", "v", 60); return kv.get("k")"#)
            .eval()
            .unwrap();
        assert_eq!(kept.as_deref(), Some("v"));
    }

    #[test]
    fn a_read_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (lua, _) = lua_with_kv(dir.path());
        let value: Option<String> = lua.load(r#"return kv.get("k")"#).eval().unwrap();
        assert_eq!(value, None);
        assert!(!dir.path().join("demo").exists(), "nothing was created");
    }

    #[test]
    fn the_store_outlives_the_host_process() {
        let dir = tempfile::tempdir().unwrap();
        let (first, _) = lua_with_kv(dir.path());
        let stored: bool = first.load(r#"return kv.set("k", "v")"#).eval().unwrap();
        assert!(stored);

        let (second, _) = lua_with_kv(dir.path());
        let value: Option<String> = second.load(r#"return kv.get("k")"#).eval().unwrap();
        assert_eq!(value.as_deref(), Some("v"), "a fresh host reads it back");
    }

    #[test]
    fn each_plugin_keeps_its_own_store() {
        let dir = tempfile::tempdir().unwrap();
        let (lua, active) = lua_with_kv(dir.path());
        let stored: bool = lua.load(r#"return kv.set("k", "demo's")"#).eval().unwrap();
        assert!(stored);

        *active.borrow_mut() = Some("other".to_string());
        let other: Option<String> = lua.load(r#"return kv.get("k")"#).eval().unwrap();
        assert_eq!(other, None);

        *active.borrow_mut() = Some("demo".to_string());
        let demo: Option<String> = lua.load(r#"return kv.get("k")"#).eval().unwrap();
        assert_eq!(demo.as_deref(), Some("demo's"));
    }
}
