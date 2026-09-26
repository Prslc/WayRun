use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use mlua::{Function, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value};
use serde_json::json;

use crate::wire::ResultItem;

/// `wayrun --lua-host <script.lua>`: answer the external-host JSON-RPC
/// contract from one Lua script, with the VM living in a process of its own.
pub fn run(script: &str) -> Result<()> {
    if script.is_empty() {
        anyhow::bail!("usage: wayrun --lua-host <script.lua>");
    }
    let source =
        std::fs::read_to_string(script).with_context(|| format!("reading lua script {script}"))?;
    let host = Host::new(script, strip_shebang(&source))
        .with_context(|| format!("loading lua script {script}"))?;
    host.serve()
}

/// A script launched by its shebang keeps the `#!` line; Lua's own interpreter
/// skips it, and `lua.load` does not.
fn strip_shebang(source: &str) -> &str {
    match source.strip_prefix("#!") {
        Some(rest) => rest.split_once('\n').map(|(_, body)| body).unwrap_or(""),
        None => source,
    }
}

struct Plugin {
    id: String,
    name: String,
    icon: Option<String>,
    description: Option<String>,
    search: Option<Function>,
    top: Option<Function>,
    forget: Option<Function>,
}

struct Host {
    lua: Lua,
    script: String,
    plugins: Vec<Plugin>,
}

impl Host {
    fn new(script: &str, source: &str) -> Result<Self> {
        // mlua's error carries no Send bound, so it converts here, once.
        let load = || -> mlua::Result<(Lua, Vec<Plugin>)> {
            let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())?;
            let sdk = sdk(&lua, script)?;
            let env = script_env(&lua, &sdk)?;
            let declared: Table = lua
                .load(source)
                .set_name(script)
                .set_environment(env)
                .eval()?;
            let plugins = read_plugins(&declared, script)?;
            Ok((lua, plugins))
        };
        let (lua, plugins) = load().map_err(|error| anyhow::anyhow!("{error}"))?;
        Ok(Self {
            lua,
            script: script.to_string(),
            plugins,
        })
    }

    fn serve(&self) -> Result<()> {
        let stdin = std::io::stdin();
        let mut out = std::io::stdout();
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let request: serde_json::Value = match serde_json::from_str(&line) {
                Ok(request) => request,
                Err(error) => {
                    respond_error(
                        &mut out,
                        &json!(null),
                        -32700,
                        &format!("parse error: {error}"),
                    )?;
                    continue;
                }
            };
            let id = request.get("id").cloned().unwrap_or(json!(null));
            match self.dispatch(&request) {
                Ok(result) => respond(&mut out, &id, result)?,
                Err((code, message)) => respond_error(&mut out, &id, code, &message)?,
            }
        }
        Ok(())
    }

    fn dispatch(
        &self,
        request: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, (i64, String)> {
        let params = request.get("params");
        match request.get("method").and_then(|method| method.as_str()) {
            Some("list_plugins") => Ok(json!(
                self.plugins
                    .iter()
                    .map(|plugin| json!({
                        "id": plugin.id,
                        "name": plugin.name,
                        "icon": plugin.icon.clone().unwrap_or_default(),
                        "description": plugin.description.clone().unwrap_or_default(),
                    }))
                    .collect::<Vec<_>>()
            )),
            Some("search") => {
                let plugin = self.plugin(params)?;
                let text = params
                    .and_then(|params| params.get("text"))
                    .and_then(|text| text.as_str())
                    .unwrap_or("");
                self.call(plugin, "search", plugin.search.as_ref(), text)
            }
            Some("top") => {
                let plugin = self.plugin(params)?;
                self.call(plugin, "top", plugin.top.as_ref(), ())
            }
            Some("forget") => match params.and_then(|params| params.get("on_click")) {
                Some(action) => self.forget(action),
                None => Err((-32602, "forget needs an on_click action".to_string())),
            },
            Some(other) => Err((-32601, format!("unknown method {other:?}"))),
            None => Err((-32600, "missing method".to_string())),
        }
    }

    fn plugin(
        &self,
        params: Option<&serde_json::Value>,
    ) -> std::result::Result<&Plugin, (i64, String)> {
        let Some(id) = params
            .and_then(|params| params.get("plugin"))
            .and_then(|id| id.as_str())
        else {
            return Err((-32602, "missing plugin id".to_string()));
        };
        self.plugins
            .iter()
            .find(|plugin| plugin.id == id)
            .ok_or_else(|| (-32602, format!("unknown plugin {id:?}")))
    }

    fn call<A: mlua::IntoLuaMulti>(
        &self,
        plugin: &Plugin,
        method: &str,
        function: Option<&Function>,
        arg: A,
    ) -> std::result::Result<serde_json::Value, (i64, String)> {
        let Some(function) = function else {
            return Err((
                -32601,
                format!("plugin {} has no {method} method", plugin.id),
            ));
        };
        let returned: Value = function.call(arg).map_err(|error| {
            warn(
                &self.script,
                &format!("plugin {} {method}: {error}", plugin.id),
            );
            (-32000, format!("{method} failed: {error}"))
        })?;
        Ok(self.rows_json(plugin, returned))
    }

    /// A returned table as the wire shape: every row is deserialized into the
    /// core's own [`ResultItem`], and one bad row is dropped, never the answer.
    fn rows_json(&self, plugin: &Plugin, returned: Value) -> serde_json::Value {
        let table = match returned {
            Value::Nil => return json!([]),
            Value::Table(table) => table,
            other => {
                warn(
                    &self.script,
                    &format!(
                        "plugin {} returned {other:?}, not a table of rows",
                        plugin.id
                    ),
                );
                return json!([]);
            }
        };
        let mut rows = Vec::new();
        for value in table.sequence_values::<Value>() {
            let Ok(value) = value else { continue };
            match self.lua.from_value::<ResultItem>(value) {
                Ok(item) => {
                    rows.push(serde_json::to_value(item).unwrap_or(serde_json::Value::Null))
                }
                Err(error) => warn(
                    &self.script,
                    &format!("plugin {} dropped a row: {error}", plugin.id),
                ),
            }
        }
        json!(rows)
    }

    /// `forget` carries no plugin id, so every plugin answers for the row and
    /// the first claim wins; none is an error, which the core reads as "not ours".
    fn forget(
        &self,
        action: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, (i64, String)> {
        let action = self
            .lua
            .to_value(action)
            .map_err(|error| (-32000, format!("unreadable action: {error}")))?;
        for plugin in &self.plugins {
            let Some(function) = &plugin.forget else {
                continue;
            };
            match function.call::<Value>(action.clone()) {
                Ok(Value::Boolean(true)) => return Ok(json!(true)),
                Ok(_) => {}
                Err(error) => warn(
                    &self.script,
                    &format!("plugin {} forget: {error}", plugin.id),
                ),
            }
        }
        Err((-32000, "no plugin owns the row".to_string()))
    }
}

fn sdk(lua: &Lua, script: &str) -> mlua::Result<Table> {
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
    sdk.set("sqlite", sqlite_lib(lua)?)?;
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

/// Open connections kept across a script's calls, so a keystroke is one query
/// rather than a copy, a connect and a schema parse as well.
#[derive(Default)]
struct Snapshots {
    next: u64,
    open: HashMap<u64, rusqlite::Connection>,
}

fn sqlite_lib(lua: &Lua) -> mlua::Result<Table> {
    let sqlite = lua.create_table()?;
    let snapshots = Rc::new(RefCell::new(Snapshots::default()));
    let opener = Rc::clone(&snapshots);
    sqlite.set(
        "snapshot",
        lua.create_function(move |_, path: String| {
            let cache = crate::system::fs::cache_dir().ok_or_else(|| {
                mlua::Error::RuntimeError("sqlite.snapshot: no cache directory".to_string())
            })?;
            opener
                .borrow_mut()
                .snapshot(&path, &cache)
                .map_err(|error| mlua::Error::RuntimeError(format!("sqlite.snapshot: {error:#}")))
        })?,
    )?;
    sqlite.set(
        "query",
        lua.create_function(
            move |lua, (id, sql, params): (u64, String, Option<Table>)| {
                let params = sqlite_params(params)?;
                let rows = snapshots
                    .borrow()
                    .query(id, &sql, params)
                    .map_err(|error| {
                        mlua::Error::RuntimeError(format!("sqlite.query: {error:#}"))
                    })?;
                lua.to_value(&serde_json::Value::Array(rows))
            },
        )?,
    )?;
    Ok(sqlite)
}

impl Snapshots {
    fn snapshot(&mut self, source: &str, cache: &Path) -> Result<u64> {
        let copy = snapshot_copy(Path::new(source), cache)?;
        let connection = rusqlite::Connection::open_with_flags(
            snapshot_uri(&copy),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        self.next += 1;
        self.open.insert(self.next, connection);
        Ok(self.next)
    }

    fn query(
        &self,
        id: u64,
        sql: &str,
        params: Vec<rusqlite::types::Value>,
    ) -> Result<Vec<serde_json::Value>> {
        let connection = self.open.get(&id).context("unknown sqlite handle")?;
        let mut statement = connection.prepare_cached(sql)?;
        let columns: Vec<String> = statement
            .column_names()
            .iter()
            .map(|name| name.to_string())
            .collect();
        let mut rows = statement.query(rusqlite::params_from_iter(params))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut record = serde_json::Map::new();
            for (index, name) in columns.iter().enumerate() {
                match row.get_ref(index)? {
                    rusqlite::types::ValueRef::Null => {}
                    rusqlite::types::ValueRef::Text(text) => {
                        record.insert(
                            name.clone(),
                            json!(String::from_utf8_lossy(text).into_owned()),
                        );
                    }
                    rusqlite::types::ValueRef::Integer(number) => {
                        record.insert(name.clone(), json!(number));
                    }
                    rusqlite::types::ValueRef::Real(number) => {
                        record.insert(name.clone(), json!(number));
                    }
                    // No consumer reads blobs; a blob column reads as absent.
                    rusqlite::types::ValueRef::Blob(_) => {}
                }
            }
            out.push(serde_json::Value::Object(record));
        }
        Ok(out)
    }
}

fn sqlite_params(params: Option<Table>) -> mlua::Result<Vec<rusqlite::types::Value>> {
    let mut out = Vec::new();
    if let Some(params) = params {
        for value in params.sequence_values::<Value>() {
            out.push(match value? {
                Value::Integer(number) => rusqlite::types::Value::Integer(number),
                Value::Number(number) => rusqlite::types::Value::Real(number),
                Value::String(text) => rusqlite::types::Value::Text(text.to_string_lossy()),
                Value::Boolean(flag) => rusqlite::types::Value::Integer(flag as i64),
                Value::Nil => rusqlite::types::Value::Null,
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "sqlite params must be scalars, got {other:?}"
                    )));
                }
            });
        }
    }
    Ok(out)
}

/// The cached immutable copy of `source` for its current `(mtime, size)`; the
/// snapshot keeps the writer's locking, `-wal` and `-shm` out of the picture.
fn snapshot_copy(source: &Path, cache: &Path) -> Result<PathBuf> {
    let meta = std::fs::metadata(source)?;
    let size = meta.len();
    let nanos = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);

    let stem = format!("snap-{:016x}", hash_path(source));
    let target = cache.join(format!("{stem}-{nanos}-{size}.sqlite"));
    if target.is_file() {
        return Ok(target);
    }

    // A pid-suffixed tmp keeps two racing hosts from interleaving one copy.
    let tmp = cache.join(format!("{stem}.{}.tmp", std::process::id()));
    std::fs::copy(source, &tmp)?;
    std::fs::rename(&tmp, &target)?;
    prune(cache, &target);
    Ok(target)
}

/// Leave one `snap-*` snapshot behind (a superseded one and a crashed copy's
/// tmp), so the cache holds a single copy per source.
fn prune(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let stale = (name.starts_with("snap-") && name.ends_with(".sqlite") && path != keep)
            || (name.starts_with("snap-") && name.ends_with(".tmp"));
        if stale {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn hash_path(path: &Path) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A snapshot's `file:` URI: `immutable=1` fits a copy that is never modified
/// in place, and drops the locking, `-shm` and `-wal` machinery entirely.
fn snapshot_uri(path: &Path) -> PathBuf {
    let mut bytes = b"file:".to_vec();
    for &byte in path.as_os_str().as_encoded_bytes() {
        // escape everything outside the URI unreserved set, non-UTF-8 bytes included
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            bytes.push(byte);
        } else {
            bytes.extend_from_slice(format!("%{byte:02X}").as_bytes());
        }
    }
    bytes.extend_from_slice(b"?immutable=1");
    PathBuf::from(OsString::from_vec(bytes))
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

/// The script's whole world: a curated core of Lua plus `wayrun`. `os`, `io`,
/// `package` and `load` are absent, and so is `print` (stdout is the wire).
fn script_env(lua: &Lua, sdk: &Table) -> mlua::Result<Table> {
    let globals = lua.globals();
    let env = lua.create_table()?;
    for name in SAFE_GLOBALS {
        let value: Value = globals.get(*name)?;
        env.set(*name, value)?;
    }
    env.set("wayrun", sdk)?;
    Ok(env)
}

const SAFE_GLOBALS: &[&str] = &[
    "assert",
    "error",
    "getmetatable",
    "ipairs",
    "next",
    "pairs",
    "pcall",
    "rawequal",
    "rawget",
    "rawlen",
    "rawset",
    "select",
    "setmetatable",
    "tonumber",
    "tostring",
    "type",
    "xpcall",
    "string",
    "table",
    "math",
    "utf8",
    "coroutine",
];

fn read_plugins(declared: &Table, script: &str) -> mlua::Result<Vec<Plugin>> {
    let mut plugins = Vec::new();
    for entry in declared.sequence_values::<Table>() {
        let entry = entry?;
        let id: String = entry.get("id")?;
        if plugins.iter().any(|plugin: &Plugin| plugin.id == id) {
            return Err(mlua::Error::RuntimeError(format!(
                "{script}: duplicate plugin id {id:?}"
            )));
        }
        let name: String = entry
            .get::<Option<String>>("name")?
            .unwrap_or_else(|| id.clone());
        let icon: Option<String> = entry.get("icon")?;
        let icon = match icon {
            Some(spec) if spec.starts_with('/') => Some(spec),
            Some(spec) => {
                warn(
                    script,
                    &format!("plugin {id}: icon {spec:?} is not an absolute path; dropped"),
                );
                None
            }
            None => None,
        };
        plugins.push(Plugin {
            id,
            name,
            icon,
            description: entry.get("description")?,
            search: entry.get("search")?,
            top: entry.get("top")?,
            forget: entry.get("forget")?,
        });
    }
    if plugins.is_empty() {
        return Err(mlua::Error::RuntimeError(format!(
            "{script} declares no plugins"
        )));
    }
    Ok(plugins)
}

fn warn(script: &str, message: &str) {
    eprintln!("wayrun-core: lua plugin {script}: {message}");
}

fn respond(out: &mut impl Write, id: &serde_json::Value, result: serde_json::Value) -> Result<()> {
    let line = json!({ "jsonrpc": "2.0", "result": result, "id": id });
    writeln!(out, "{line}")?;
    out.flush()?;
    Ok(())
}

fn respond_error(
    out: &mut impl Write,
    id: &serde_json::Value,
    code: i64,
    message: &str,
) -> Result<()> {
    let line = json!({ "jsonrpc": "2.0", "error": { "code": code, "message": message }, "id": id });
    writeln!(out, "{line}")?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(source: &str) -> Host {
        Host::new("/test/script.lua", source).unwrap()
    }

    fn call(
        host: &Host,
        method: &str,
        params: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, (i64, String)> {
        host.dispatch(&json!({ "method": method, "params": params }))
    }

    fn search(host: &Host, plugin: &str, text: &str) -> serde_json::Value {
        call(host, "search", json!({ "plugin": plugin, "text": text })).unwrap()
    }

    #[test]
    fn os_io_package_load_and_print_are_unreachable() {
        host(
            r#"
            assert(os == nil, "os leaked")
            assert(io == nil, "io leaked")
            assert(package == nil, "package leaked")
            assert(load == nil, "load leaked")
            assert(print == nil, "print leaked")
            return { { id = "demo", search = function() return {} end } }
            "#,
        );
    }

    #[test]
    fn time_env_and_script_dir_are_bound() {
        let host = host(
            r#"
            return {
              {
                id = "demo",
                search = function()
                  local path = wayrun.env("PATH")
                  return {
                    { title = tostring(type(wayrun.time()) == "number") },
                    { title = tostring(path ~= nil and #path > 0) },
                    { title = tostring(wayrun.script_dir()) },
                  }
                end,
              },
            }
            "#,
        );
        let rows = search(&host, "demo", "x");
        assert_eq!(rows[0]["title"], "true", "time is a number");
        assert_eq!(rows[1]["title"], "true", "PATH is readable");
        // The test script path is fabricated, so the fallback parent wins.
        assert_eq!(rows[2]["title"], "/test");
    }

    #[test]
    fn a_leading_shebang_line_is_skipped() {
        let source = "#!/usr/bin/env -S wayrun --lua-host\nreturn { { id = \"demo\", search = function() return {} end } }\n";
        let host = Host::new("/test/demo.lua", strip_shebang(source)).unwrap();
        assert_eq!(host.plugins[0].id, "demo");
    }

    #[test]
    fn a_row_crosses_the_host_contract() {
        let host = host(
            r#"
            return {
              {
                id = "demo", name = "Demo", description = "A demo",
                search = function(text)
                  return {
                    {
                      title = "Hello " .. text,
                      summary = "https://example.com",
                      on_click = { type = "open", uri = "https://example.com" },
                    },
                  }
                end,
              },
            }
            "#,
        );
        let rows = search(&host, "demo", "world");
        assert_eq!(rows[0]["title"], "Hello world");
        assert_eq!(rows[0]["on_click"]["type"], "open");
        assert_eq!(rows[0]["ephemeral"], false);
    }

    #[test]
    fn a_bad_row_is_dropped_and_the_rest_survive() {
        let host = host(
            r#"
            return {
              {
                id = "demo",
                search = function()
                  return { { title = "ok" }, { summary = "no title" }, { title = "also ok" } }
                end,
              },
            }
            "#,
        );
        let rows = search(&host, "demo", "x");
        assert_eq!(rows.as_array().unwrap().len(), 2);
        assert_eq!(rows[1]["title"], "also ok");
    }

    #[test]
    fn an_empty_table_is_no_rows() {
        let host = host(r#"return { { id = "demo", search = function() return {} end } }"#);
        assert_eq!(search(&host, "demo", "x"), json!([]));
    }

    #[test]
    fn t_fills_placeholders_from_the_core_tables() {
        let host = host(
            r#"
            return {
              {
                id = "demo",
                search = function()
                  return { { title = wayrun.t("plugin.web.summary", { engine = "Google" }) } }
                end,
              },
            }
            "#,
        );
        let rows = search(&host, "demo", "x");
        assert_eq!(rows[0]["title"], "Search on Google");
    }

    #[test]
    fn list_plugins_carries_identity() {
        let host = host(r#"return { { id = "demo", name = "Demo", description = "A demo" } }"#);
        let plugins = call(&host, "list_plugins", json!(null)).unwrap();
        assert_eq!(plugins[0]["id"], "demo");
        assert_eq!(plugins[0]["name"], "Demo");
        assert_eq!(plugins[0]["description"], "A demo");
        assert_eq!(plugins[0]["icon"], "");
    }

    #[test]
    fn a_missing_method_and_a_raising_method_are_errors() {
        let host = host(r#"return { { id = "demo", search = function() error("boom") end } }"#);
        assert!(call(&host, "top", json!({ "plugin": "demo" })).is_err());
        assert!(search_error(&host, "demo", "x").is_err());
    }

    fn search_error(
        host: &Host,
        plugin: &str,
        text: &str,
    ) -> std::result::Result<serde_json::Value, (i64, String)> {
        call(host, "search", json!({ "plugin": plugin, "text": text }))
    }

    #[test]
    fn a_snapshot_query_reads_rows_and_absent_nulls() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("places.sqlite");
        {
            let connection = rusqlite::Connection::open(&source).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, note TEXT);
                     INSERT INTO t (name) VALUES ('alpha'), ('beta');",
                )
                .unwrap();
        }

        let mut snapshots = Snapshots::default();
        let id = snapshots
            .snapshot(source.to_str().unwrap(), dir.path())
            .unwrap();
        let rows = snapshots
            .query(
                id,
                "SELECT name FROM t WHERE name LIKE ?1 ORDER BY name",
                vec![rusqlite::types::Value::Text("%a%".to_string())],
            )
            .unwrap();
        assert_eq!(rows[0]["name"], "alpha");

        let rows = snapshots.query(id, "SELECT note FROM t", vec![]).unwrap();
        assert!(rows[0].as_object().unwrap().get("note").is_none());
    }
}
