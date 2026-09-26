use std::time::UNIX_EPOCH;

use mlua::{Lua, LuaSerdeExt, Table, Value};

use super::sqlite;
use super::warn;

pub(super) fn build(lua: &Lua, script: &str) -> mlua::Result<Table> {
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
    sdk.set(
        "env",
        lua.create_function(|_, name: String| Ok(std::env::var(name).ok()))?,
    )?;
    let dir = script_dir(script);
    sdk.set(
        "script_dir",
        lua.create_function(move |_, ()| Ok(dir.clone()))?,
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
    sdk.set("json", json_lib(lua)?)?;
    sdk.set("fs", fs_lib(lua)?)?;
    sdk.set("http", http_lib(lua)?)?;
    sdk.set("sqlite", sqlite::lib(lua)?)?;
    Ok(sdk)
}

/// Unix seconds, the host clock a signature or a cache TTL needs.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// The script's own directory, so a plugin can ship an icon beside itself; the
/// host has just read the file, so canonicalize only fails in theory.
fn script_dir(script: &str) -> Option<String> {
    let path = std::fs::canonicalize(script).unwrap_or_else(|_| std::path::PathBuf::from(script));
    path.parent().map(|dir| dir.display().to_string())
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

fn fs_lib(lua: &Lua) -> mlua::Result<Table> {
    let fs = lua.create_table()?;
    fs.set(
        "list",
        lua.create_function(|lua, dir: String| {
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
    fs.set(
        "stat",
        lua.create_function(|lua, path: String| {
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
    Ok(fs)
}

fn http_lib(lua: &Lua) -> mlua::Result<Table> {
    let http = lua.create_table()?;
    http.set(
        "get",
        lua.create_function(
            |_, (url, params, timeout_ms): (String, Option<Table>, Option<u64>)| {
                let seconds = (timeout_ms.unwrap_or(5_000) / 1_000).max(1);
                let mut request = minreq::get(&url).with_timeout(seconds);
                if let Some(params) = params {
                    for pair in params.pairs::<String, Value>() {
                        let (name, value) = pair?;
                        let value = match value {
                            Value::Integer(number) => number.to_string(),
                            Value::Number(number) => number.to_string(),
                            Value::String(text) => text.to_string_lossy(),
                            Value::Boolean(flag) => flag.to_string(),
                            other => {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "http params must be scalars, got {other:?}"
                                )));
                            }
                        };
                        request = request.with_param(name, value);
                    }
                }
                let request = match proxy_from_env() {
                    Some(proxy) => request.with_proxy(proxy),
                    None => request,
                };
                let response = request.send().map_err(|error| {
                    mlua::Error::RuntimeError(format!("http.get {url}: {error}"))
                })?;
                response
                    .as_str()
                    .map(str::to_string)
                    .map_err(|error| mlua::Error::RuntimeError(format!("http.get {url}: {error}")))
            },
        )?,
    )?;
    Ok(http)
}

fn proxy_from_env() -> Option<minreq::Proxy> {
    let spec = ["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))?;
    match minreq::Proxy::new(&spec) {
        Ok(proxy) => Some(proxy),
        Err(error) => {
            warn("sdk", &format!("ignoring proxy {spec:?}: {error}"));
            None
        }
    }
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
