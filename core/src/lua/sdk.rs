use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::UNIX_EPOCH;

use mlua::{Lua, LuaSerdeExt, Table, Value};

use super::areas;
use super::crypto;
use super::fuzzy;
use super::http;
use super::kv;
use super::sqlite;
use super::warn;

/// The plugin directories this script may read from, one per declared id;
/// filled after the script is read, resolved by the bindings at call time.
pub(super) type Scope = Rc<RefCell<Vec<PathBuf>>>;

/// Every declared plugin's readable environment names, by plugin id; filled
/// after the script is read, refused for a name the calling plugin omits.
pub(super) type EnvScope = Rc<RefCell<HashMap<String, Vec<String>>>>;

/// `<home>/.config/wayrun/plugins/<id>`, the directory a plugin reads its own
/// files from; the user places them, nothing here is created for the script.
pub(super) fn plugin_dir(id: &str) -> Option<PathBuf> {
    let dir = crate::system::fs::get_home()
        .ok()?
        .join(".config/wayrun/plugins");
    Some(dir.join(id))
}

/// The text of `name` under the first scope root that holds it; `Ok(None)`
/// when none does, and an error when the name itself is not allowed.
fn scoped_read(roots: &[PathBuf], name: &str) -> Result<Option<String>, String> {
    let allowed = !name.is_empty()
        && !name.starts_with('/')
        && !name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if !allowed {
        return Err("name must be a relative path without `..`".to_string());
    }
    for root in roots {
        match std::fs::read_to_string(root.join(name)) {
            Ok(text) => return Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(None)
}

pub(super) fn build(
    lua: &Lua,
    script: &str,
    scope: Scope,
    active: kv::Active,
    paths: kv::Paths,
    env_scope: EnvScope,
    read_areas: areas::Areas,
) -> mlua::Result<Table> {
    let sdk = lua.create_table()?;
    sdk.set(
        "home",
        lua.create_function(|_, ()| {
            Ok(crate::system::fs::get_home()
                .ok()
                .map(|path| path.display().to_string()))
        })?,
    )?;
    sdk.set(
        "cache_dir",
        lua.create_function(|_, ()| {
            Ok(crate::system::fs::cache_dir().map(|path| path.display().to_string()))
        })?,
    )?;
    sdk.set(
        "icon",
        lua.create_function(|_, spec: String| Ok(crate::system::icon::resolve(&spec)))?,
    )?;
    sdk.set(
        "urlencode",
        lua.create_function(|_, text: String| Ok(urlencoding::encode(&text).into_owned()))?,
    )?;
    sdk.set(
        "web_search_engine",
        lua.create_function(|_, ()| Ok(crate::config::web_search_engine()))?,
    )?;
    sdk.set("time", lua.create_function(|_, ()| Ok(now_seconds()))?)?;
    let env_active = Rc::clone(&active);
    sdk.set(
        "env",
        lua.create_function(move |_, name: String| {
            let Some(id) = env_active.borrow().clone() else {
                return Err(mlua::Error::RuntimeError(format!(
                    "env {name}: this read ran outside a plugin call"
                )));
            };
            let allowed = env_scope
                .borrow()
                .get(&id)
                .is_some_and(|names| names.contains(&name));
            if !allowed {
                return Err(mlua::Error::RuntimeError(format!(
                    "env {name}: plugin {id} does not declare it"
                )));
            }
            Ok(std::env::var(name).ok())
        })?,
    )?;
    sdk.set(
        "which",
        lua.create_function(|_, name: String| {
            let path = std::env::var_os("PATH").unwrap_or_default();
            Ok(crate::system::fs::which_in(path, &name).map(|path| path.display().to_string()))
        })?,
    )?;
    let dir = script_dir(script);
    let dir_name = dir.as_ref().map(|dir| dir.display().to_string());
    sdk.set(
        "script_dir",
        lua.create_function(move |_, ()| Ok(dir_name.clone()))?,
    )?;
    sdk.set(
        "plugin_dir",
        lua.create_function(|_, id: String| {
            Ok(plugin_dir(&id).map(|dir| dir.display().to_string()))
        })?,
    )?;
    sdk.set(
        "t",
        lua.create_function(|_, (key, args): (String, Option<Table>)| Ok(translate(&key, args)))?,
    )?;
    let name = script.to_string();
    sdk.set(
        "log",
        lua.create_function(move |_, message: String| {
            warn(&name, &message);
            Ok(())
        })?,
    )?;
    let guard: areas::Guard = {
        let read_areas = Rc::clone(&read_areas);
        let guard_active = Rc::clone(&active);
        let script = dir.clone();
        Rc::new(move |path: &str| {
            let Some(id) = guard_active.borrow().clone() else {
                return Err("this read ran outside a plugin call".to_string());
            };
            let mut roots = read_areas.borrow().get(&id).cloned().unwrap_or_default();
            if let Some(own) = plugin_dir(&id) {
                roots.push(own);
            }
            if let Some(dir) = &script {
                roots.push(dir.clone());
            }
            areas::check(&roots, path)
        })
    };
    sdk.set("json", json_lib(lua)?)?;
    sdk.set("toml", toml_lib(lua)?)?;
    sdk.set("fs", fs_lib(lua, scope, Rc::clone(&guard))?)?;
    let stores: kv::Handle = Rc::new(RefCell::new(kv::Stores::default()));
    sdk.set(
        "http",
        http::lib(
            lua,
            script,
            Rc::clone(&stores),
            Rc::clone(&active),
            Rc::clone(&paths),
        )?,
    )?;
    sdk.set("sqlite", sqlite::lib(lua, guard)?)?;
    sdk.set("kv", kv::lib(lua, stores, active, paths)?)?;
    sdk.set("fuzzy", fuzzy::lib(lua)?)?;
    sdk.set("crypto", crypto::lib(lua)?)?;
    Ok(sdk)
}

/// Unix seconds, the host clock a signature or a cache TTL needs.
pub(super) fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// The script's own directory, so a plugin can ship an icon beside itself; the
/// host has just read the file, so canonicalize only fails in theory.
fn script_dir(script: &str) -> Option<PathBuf> {
    let path = std::fs::canonicalize(script).unwrap_or_else(|_| PathBuf::from(script));
    path.parent().map(|dir| dir.to_path_buf())
}

fn json_lib(lua: &Lua) -> mlua::Result<Table> {
    let json = lua.create_table()?;
    json.set(
        "decode",
        lua.create_function(|lua, source: String| {
            let value: serde_json::Value = serde_json::from_str(&source)
                .map_err(|error| mlua::Error::RuntimeError(format!("json.decode: {error}")))?;
            lua.to_value(&value)
        })?,
    )?;
    json.set(
        "encode",
        lua.create_function(|lua, value: Value| {
            let value: serde_json::Value = lua
                .from_value(value)
                .map_err(|error| mlua::Error::RuntimeError(format!("json.encode: {error}")))?;
            serde_json::to_string(&value)
                .map_err(|error| mlua::Error::RuntimeError(format!("json.encode: {error}")))
        })?,
    )?;
    Ok(json)
}

fn toml_lib(lua: &Lua) -> mlua::Result<Table> {
    let toml = lua.create_table()?;
    toml.set(
        "decode",
        lua.create_function(|lua, source: String| {
            let value: toml::Value = toml::from_str(&source)
                .map_err(|error| mlua::Error::RuntimeError(format!("toml.decode: {error}")))?;
            let value = serde_json::to_value(&value)
                .map_err(|error| mlua::Error::RuntimeError(format!("toml.decode: {error}")))?;
            lua.to_value(&value)
        })?,
    )?;
    Ok(toml)
}

fn fs_lib(lua: &Lua, scope: Scope, guard: areas::Guard) -> mlua::Result<Table> {
    let fs = lua.create_table()?;
    let list_guard = Rc::clone(&guard);
    fs.set(
        "list",
        lua.create_function(move |lua, dir: String| {
            if let Err(problem) = list_guard(&dir) {
                return Err(mlua::Error::RuntimeError(format!(
                    "fs.list {dir:?}: {problem}"
                )));
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                return Ok(Value::Nil);
            };
            let list = lua.create_table()?;
            let mut at = 0;
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    at += 1;
                    list.set(at, name.to_string())?;
                }
            }
            Ok(Value::Table(list))
        })?,
    )?;
    let stat_guard = Rc::clone(&guard);
    fs.set(
        "stat",
        lua.create_function(move |lua, path: String| {
            if let Err(problem) = stat_guard(&path) {
                return Err(mlua::Error::RuntimeError(format!(
                    "fs.stat {path:?}: {problem}"
                )));
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                return Ok(Value::Nil);
            };
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|since| since.as_nanos() as i64)
                .unwrap_or(0);
            let stat = lua.create_table()?;
            stat.set("mtime_ns", mtime_ns)?;
            stat.set("size", meta.len() as i64)?;
            Ok(Value::Table(stat))
        })?,
    )?;
    fs.set(
        "read",
        lua.create_function(
            move |lua, name: String| match scoped_read(&scope.borrow(), &name) {
                Ok(Some(text)) => Ok(Value::String(lua.create_string(text)?)),
                Ok(None) => Ok(Value::Nil),
                Err(problem) => Err(mlua::Error::RuntimeError(format!(
                    "fs.read {name:?}: {problem}"
                ))),
            },
        )?,
    )?;
    Ok(fs)
}

/// A script's `wayrun.t(key, args)`: the same tables the core reads, with the
/// `%{name}` placeholders filled from `args`.
fn translate(key: &str, args: Option<Table>) -> String {
    let mut message = crate::_rust_i18n_translate(&rust_i18n::locale(), key);
    if let Some(args) = args {
        for pair in args.pairs::<String, Value>() {
            let Ok((name, value)) = pair else { continue };
            let text = match value {
                Value::Integer(number) => number.to_string(),
                Value::Number(number) => number.to_string(),
                Value::String(text) => text.to_string_lossy(),
                other => format!("{other:?}"),
            };
            message = message.replace(&format!("%{{{name}}}"), &text);
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_read_stays_inside_its_roots() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "a = 1").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/data.txt"), "x").unwrap();
        let roots = vec![dir.path().to_path_buf()];

        assert_eq!(
            scoped_read(&roots, "config.toml").unwrap().as_deref(),
            Some("a = 1")
        );
        assert_eq!(
            scoped_read(&roots, "sub/data.txt").unwrap().as_deref(),
            Some("x")
        );
        assert_eq!(scoped_read(&roots, "missing.txt").unwrap(), None);
        assert!(scoped_read(&roots, "../escape.txt").is_err());
        assert!(scoped_read(&roots, "a/../b").is_err());
        assert!(scoped_read(&roots, "/etc/hostname").is_err());
        assert!(scoped_read(&roots, "").is_err());
    }
}
