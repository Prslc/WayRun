use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, UNIX_EPOCH};

use gio::prelude::FileExt;
use mlua::{Lua, LuaSerdeExt, Table, Value};

use super::areas;
use super::crypto;
use super::fuzzy;
use super::http;
use super::kv;
use super::log::Log;
use super::sqlite;

/// Every declared plugin's readable environment names, by plugin id; filled
/// after the script is read, refused for a name the calling plugin omits.
pub(super) type EnvScope = Rc<RefCell<HashMap<String, Vec<String>>>>;

/// Every declared plugin's runnable program names, by plugin id; a name the
/// calling plugin omits is refused.
pub(super) type ExecScope = Rc<RefCell<HashMap<String, Vec<String>>>>;

/// `$XDG_CONFIG_HOME/wayrun/plugins/<id>`, the directory a plugin reads its own
/// files from; the user places them, nothing here is created for the script.
pub(super) fn plugin_dir(id: &str) -> Option<PathBuf> {
    Some(crate::config::dir()?.join("plugins").join(id))
}

/// The scope handles a script's bindings share: filled after the plugin table
/// is read, then resolved by id at call time.
pub(super) struct Shared {
    pub active: kv::Active,
    pub kv_paths: kv::Paths,
    pub env_scope: EnvScope,
    pub exec_scope: ExecScope,
    pub read_areas: areas::Areas,
}

/// The session variables a declared program keeps: enough to reach a socket or
/// a display, with the rest of the launcher's environment hidden.
const EXEC_ENV: [&str; 8] = [
    "PATH",
    "HOME",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "DBUS_SESSION_BUS_ADDRESS",
    "WAYLAND_DISPLAY",
    "DISPLAY",
];
const EXEC_TIMEOUT: Duration = Duration::from_millis(1500);
const EXEC_STDOUT_CAP: usize = 64 * 1024;
const EXEC_STDERR_CAP: usize = 8 * 1024;

pub(super) fn build(lua: &Lua, script: &str, shared: &Shared, log: Log) -> mlua::Result<Table> {
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
    sdk.set("api", crate::wire::PLUGIN_API)?;
    sdk.set(
        "icon",
        lua.create_function(|_, spec: String| Ok(crate::system::icon::resolve_for_host(&spec)))?,
    )?;
    sdk.set(
        "urlencode",
        lua.create_function(|_, text: String| Ok(urlencoding::encode(&text).into_owned()))?,
    )?;
    sdk.set(
        "file_uri",
        lua.create_function(|_, path: String| Ok(file_uri(&path)))?,
    )?;
    sdk.set("time", lua.create_function(|_, ()| Ok(now_seconds()))?)?;
    sdk.set(
        "locale",
        lua.create_function(|_, ()| Ok(rust_i18n::locale()))?,
    )?;
    let env_active = Rc::clone(&shared.active);
    let env_scope = Rc::clone(&shared.env_scope);
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
                    "env {name}: plugin {id} does not declare it; add the name to the plugin table's env"
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
    let exec_active = Rc::clone(&shared.active);
    let exec_scope = Rc::clone(&shared.exec_scope);
    let exec_env = Rc::clone(&shared.env_scope);
    sdk.set(
        "exec",
        lua.create_function(move |lua, (program, args): (String, Option<Vec<String>>)| {
            exec_program(
                lua,
                &program,
                args.unwrap_or_default(),
                &exec_active,
                &exec_scope,
                &exec_env,
            )
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
        "log",
        lua.create_function(move |_, message: String| {
            log.write(&message);
            Ok(())
        })?,
    )?;
    let read_guard: areas::Guard = {
        let read_areas = Rc::clone(&shared.read_areas);
        let guard_active = Rc::clone(&shared.active);
        let script = dir.clone();
        Rc::new(move |path: &str| {
            let Some(id) = guard_active.borrow().clone() else {
                return Err("this read ran outside a plugin call".to_string());
            };
            let Some(own) = plugin_dir(&id) else {
                return Err("no plugin directory".to_string());
            };
            let areas = read_areas.borrow();
            let declared = areas.get(&id).map(Vec::as_slice).unwrap_or(&[]);
            read_path(&own, declared, script.as_deref(), path)
        })
    };
    let write_guard: areas::Guard = {
        let guard_active = Rc::clone(&shared.active);
        Rc::new(move |path: &str| {
            let Some(id) = guard_active.borrow().clone() else {
                return Err("this write ran outside a plugin call".to_string());
            };
            let Some(own) = plugin_dir(&id) else {
                return Err("no plugin directory".to_string());
            };
            write_path(&own, path)
        })
    };
    sdk.set("json", json_lib(lua)?)?;
    sdk.set("toml", toml_lib(lua)?)?;
    sdk.set("fs", fs_lib(lua, Rc::clone(&read_guard), write_guard)?)?;
    let stores: kv::Handle = Rc::new(RefCell::new(kv::Stores::default()));
    sdk.set(
        "http",
        http::lib(
            lua,
            script,
            Rc::clone(&stores),
            Rc::clone(&shared.active),
            Rc::clone(&shared.kv_paths),
        )?,
    )?;
    sdk.set("sqlite", sqlite::lib(lua, read_guard)?)?;
    sdk.set(
        "kv",
        kv::lib(
            lua,
            stores,
            Rc::clone(&shared.active),
            Rc::clone(&shared.kv_paths),
        )?,
    )?;
    sdk.set("fuzzy", fuzzy::lib(lua)?)?;
    sdk.set("crypto", crypto::lib(lua)?)?;
    Ok(sdk)
}

/// Run a declared PATH program with argv and a deadline. The plugin's declared
/// `env` names survive `env_clear`, so `exec("env")` cannot widen that list.
fn exec_program(
    lua: &Lua,
    program: &str,
    args: Vec<String>,
    active: &kv::Active,
    exec_scope: &ExecScope,
    env_scope: &EnvScope,
) -> mlua::Result<Table> {
    let Some(id) = active.borrow().clone() else {
        return Err(mlua::Error::RuntimeError(format!(
            "exec {program}: this call ran outside a plugin call"
        )));
    };
    let declared = exec_scope.borrow().get(&id).cloned().unwrap_or_default();
    if !declared.iter().any(|name| name == program) {
        return Err(mlua::Error::RuntimeError(format!(
            "exec {program}: plugin {id} does not declare it; add the name to the plugin table's exec"
        )));
    }
    let path = crate::system::fs::which_in(std::env::var_os("PATH").unwrap_or_default(), program)
        .ok_or_else(|| {
        mlua::Error::RuntimeError(format!("exec {program}: not on the launcher's PATH"))
    })?;
    let mut command = std::process::Command::new(path);
    command.args(&args).env_clear();
    if let Ok(home) = crate::system::fs::get_home() {
        command.current_dir(home);
    }
    for name in EXEC_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let declared_env = env_scope.borrow().get(&id).cloned().unwrap_or_default();
    for name in declared_env {
        if let Some(value) = std::env::var_os(&name) {
            command.env(name, value);
        }
    }
    let output = crate::system::process::output_within_capped(
        command,
        EXEC_TIMEOUT,
        EXEC_STDOUT_CAP,
        EXEC_STDERR_CAP,
    )
    .ok_or_else(|| {
        mlua::Error::RuntimeError(format!(
            "exec {program}: could not start, or still running after {}s",
            EXEC_TIMEOUT.as_secs_f32()
        ))
    })?;

    let result = lua.create_table()?;
    result.set("code", output.status.code().unwrap_or(-1))?;
    result.set(
        "stdout",
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )?;
    result.set(
        "stderr",
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )?;
    Ok(result)
}

/// The `file://` URI for an absolute path, encoded by GLib exactly as a built-in
/// row's action is; a relative path has no meaning to the launcher.
pub(super) fn file_uri(path: &str) -> Option<String> {
    path.starts_with('/')
        .then(|| gio::File::for_path(path).uri().to_string())
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
pub(super) fn script_dir(script: &str) -> Option<PathBuf> {
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

/// A read path: relative names live under the plugin's own directory; absolute
/// and `~/` paths must lie in a declared area, that directory or the script's.
fn read_path(
    own: &Path,
    declared: &[PathBuf],
    script: Option<&Path>,
    path: &str,
) -> Result<PathBuf, String> {
    if areas::relative(path) {
        return areas::inside(own, path);
    }
    let mut roots = declared.to_vec();
    roots.push(own.to_path_buf());
    if let Some(dir) = script {
        roots.push(dir.to_path_buf());
    }
    areas::check(&roots, path)
}

/// A write path: only inside the plugin's own directory, and never the store.
fn write_path(own: &Path, path: &str) -> Result<PathBuf, String> {
    let resolved = if areas::relative(path) {
        areas::inside(own, path)?
    } else {
        let Some(expanded) = areas::expand(path) else {
            return Err("the name must be absolute or start with ~/".to_string());
        };
        let resolved = areas::resolve(&expanded);
        if !resolved.starts_with(areas::resolve(own)) {
            return Err("writes stay inside the plugin directory".to_string());
        }
        resolved
    };
    let name = resolved
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if matches!(name, "kv.db" | "kv.db-wal" | "kv.db-shm" | "kv.db-journal") {
        return Err("kv owns this file".to_string());
    }
    Ok(resolved)
}

fn fs_lib(lua: &Lua, read: areas::Guard, write: areas::Guard) -> mlua::Result<Table> {
    let fs = lua.create_table()?;
    let list_guard = Rc::clone(&read);
    fs.set(
        "list",
        lua.create_function(move |lua, dir: String| {
            let dir = list_guard(&dir).map_err(|problem| {
                mlua::Error::RuntimeError(format!("fs.list {dir:?}: {problem}"))
            })?;
            let Ok(entries) = std::fs::read_dir(&dir) else {
                return Ok(Value::Nil);
            };
            // readdir order is unspecified; sort so the listing is stable.
            let mut names: Vec<String> = entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                .collect();
            names.sort();
            let list = lua.create_table()?;
            for (at, name) in names.into_iter().enumerate() {
                list.set(at + 1, name)?;
            }
            Ok(Value::Table(list))
        })?,
    )?;
    let stat_guard = Rc::clone(&read);
    fs.set(
        "stat",
        lua.create_function(move |lua, path: String| {
            let path = stat_guard(&path).map_err(|problem| {
                mlua::Error::RuntimeError(format!("fs.stat {path:?}: {problem}"))
            })?;
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
        lua.create_function(move |lua, name: String| {
            let path = read(&name).map_err(|problem| {
                mlua::Error::RuntimeError(format!("fs.read {name:?}: {problem}"))
            })?;
            match std::fs::read_to_string(&path) {
                Ok(text) => Ok(Value::String(lua.create_string(text)?)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Nil),
                Err(error) => Err(mlua::Error::RuntimeError(format!(
                    "fs.read {name:?}: {error}"
                ))),
            }
        })?,
    )?;
    fs.set(
        "write",
        lua.create_function(move |_, (path, data): (String, mlua::LuaString)| {
            let target = write(&path).map_err(|problem| {
                mlua::Error::RuntimeError(format!("fs.write {path:?}: {problem}"))
            })?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    mlua::Error::RuntimeError(format!("fs.write {path:?}: {error}"))
                })?;
            }
            std::fs::write(&target, data.as_bytes()).map_err(|error| {
                mlua::Error::RuntimeError(format!("fs.write {path:?}: {error}"))
            })?;
            Ok(true)
        })?,
    )?;
    Ok(fs)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::*;

    #[test]
    fn a_read_path_uses_the_plugin_dir_or_a_declared_area() {
        let own = tempfile::tempdir().unwrap();
        let area = tempfile::tempdir().unwrap();
        let script = tempfile::tempdir().unwrap();
        std::fs::write(own.path().join("local.txt"), "x").unwrap();
        std::fs::write(area.path().join("note.txt"), "y").unwrap();
        let declared = [area.path().to_path_buf()];
        let script_dir = script.path().to_path_buf();

        assert!(read_path(own.path(), &declared, Some(&script_dir), "local.txt").is_ok());
        assert!(
            read_path(
                own.path(),
                &declared,
                Some(&script_dir),
                area.path().join("note.txt").to_str().unwrap()
            )
            .is_ok()
        );
        assert!(
            read_path(
                own.path(),
                &declared,
                Some(&script_dir),
                script.path().join("shipped.txt").to_str().unwrap()
            )
            .is_ok(),
            "the script's own directory is readable"
        );
        assert!(read_path(own.path(), &declared, Some(&script_dir), "/etc/hostname").is_err());
        assert!(read_path(own.path(), &declared, Some(&script_dir), "../escape").is_err());
    }

    #[test]
    fn a_file_uri_percent_encodes_an_absolute_path() {
        assert_eq!(
            file_uri("/tmp/a b.png").as_deref(),
            Some("file:///tmp/a%20b.png")
        );
        assert_eq!(
            file_uri("/tmp/中文.txt").as_deref(),
            Some("file:///tmp/%E4%B8%AD%E6%96%87.txt")
        );
        assert_eq!(file_uri("relative.png"), None);
    }

    #[test]
    fn a_write_path_stays_in_the_plugin_dir_and_leaves_kv_alone() {
        let own = tempfile::tempdir().unwrap();
        assert!(write_path(own.path(), "data/cache.json").is_ok());
        assert!(write_path(own.path(), own.path().join("x.json").to_str().unwrap()).is_ok());
        assert!(write_path(own.path(), "kv.db").is_err());
        assert!(write_path(own.path(), "kv.db-wal").is_err());
        assert!(write_path(own.path(), "/etc/hostname").is_err());
        assert!(write_path(own.path(), "../escape").is_err());
    }

    /// Every name the sandbox binds, one level into a sub-table: the leaves the
    /// reference page has to name, read off the built table, not the source.
    fn sdk_names() -> BTreeSet<String> {
        let lua = Lua::new_with(mlua::StdLib::ALL_SAFE, mlua::LuaOptions::default()).unwrap();
        let active = Rc::new(RefCell::new(None));
        let shared = Shared {
            active: Rc::clone(&active),
            kv_paths: Rc::new(RefCell::new(HashMap::new())),
            env_scope: Rc::new(RefCell::new(HashMap::new())),
            exec_scope: Rc::new(RefCell::new(HashMap::new())),
            read_areas: Rc::new(RefCell::new(HashMap::new())),
        };
        let table = build(
            &lua,
            "/test/script.lua",
            &shared,
            Log::new("/test/script.lua", Rc::clone(&active)),
        )
        .unwrap();
        // `wayrun.import` is bound by the sandbox, after the environment exists.
        crate::lua::sandbox::script_env(
            &lua,
            &table,
            "/test/script.lua",
            Log::new("/test/script.lua", active),
        )
        .unwrap();
        let mut names = BTreeSet::new();
        for pair in table.pairs::<String, Value>() {
            let (name, value) = pair.unwrap();
            match value {
                Value::Table(sub) => {
                    for member in sub.pairs::<String, Value>() {
                        names.insert(format!("{name}.{}", member.unwrap().0));
                    }
                }
                _ => {
                    names.insert(name);
                }
            }
        }
        names
    }

    /// Every `wayrun.<name>` a page writes, in prose or in a table cell.
    fn page_names(text: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let mut rest = text;
        while let Some(at) = rest.find("wayrun.") {
            // a path such as lua/wayrun.lua is not a binding
            let standalone = rest[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_' && c != '/');
            rest = &rest[at + "wayrun.".len()..];
            if !standalone {
                continue;
            }
            let name: String = rest
                .chars()
                .take_while(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '.'
                })
                .collect();
            if !name.is_empty() {
                names.insert(name);
            }
        }
        names
    }

    fn page(locale: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the core lives in the workspace")
            .join("docs")
            .join(locale)
            .join("lua.md");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    fn stub() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the core lives in the workspace")
            .join("lua")
            .join("wayrun.lua");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    /// The `wayrun.<name>` declarations a LuaLS stub file holds; a parent table
    /// assignment (`wayrun.fs = {}`) is a name in its own right.
    fn stub_names(text: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for line in text.lines() {
            let line = line.trim();
            let rest = line
                .strip_prefix("function wayrun.")
                .or_else(|| line.strip_prefix("wayrun."));
            let Some(rest) = rest else { continue };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            if !name.is_empty() {
                names.insert(name);
            }
        }
        names
    }

    #[test]
    fn the_lua_stub_names_every_binding() {
        let names = sdk_names();
        let stub = stub_names(&stub());
        let missing: Vec<&String> = names.difference(&stub).collect();
        assert!(missing.is_empty(), "bound but not stubbed: {missing:?}");
    }

    #[test]
    fn the_lua_stub_names_nothing_missing() {
        let names = sdk_names();
        let stub = stub_names(&stub());
        let stale: Vec<&String> = stub
            .difference(&names)
            .filter(|name| {
                !stub
                    .iter()
                    .any(|other| other.starts_with(&format!("{name}.")))
            })
            .collect();
        assert!(stale.is_empty(), "stubbed but not bound: {stale:?}");
    }

    #[test]
    fn the_lua_page_names_every_binding() {
        let names = sdk_names();
        // a scan that found nothing would make this vacuous
        assert!(names.len() > 20, "found only {} bindings", names.len());
        let documented = page_names(&page("en"));
        let missing: Vec<&String> = names.difference(&documented).collect();
        assert!(missing.is_empty(), "bound but undocumented: {missing:?}");
    }

    #[test]
    fn nothing_the_lua_page_names_is_missing_from_the_sdk() {
        let names = sdk_names();
        let documented = page_names(&page("en"));
        let stale: Vec<&String> = documented.difference(&names).collect();
        assert!(stale.is_empty(), "documented but not bound: {stale:?}");
    }

    #[test]
    fn the_two_lua_pages_name_the_same_bindings() {
        assert_eq!(page_names(&page("en")), page_names(&page("zh_cn")));
    }
}
