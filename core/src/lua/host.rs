use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use mlua::{Function, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value};
use serde_json::json;

use crate::wire::ResultItem;

use super::manifest::{self, Plugin, read_plugins};
use super::sandbox::script_env;
use super::{kv, log::Log, module, sdk, warn};

/// `wayrun --lua-host <script.lua>`: answer the external-host JSON-RPC
/// contract from one Lua script, with the VM living in a process of its own.
pub fn run(script: &str) -> Result<()> {
    if script.is_empty() {
        anyhow::bail!("usage: wayrun --lua-host <script.lua>");
    }
    let source =
        fs::read_to_string(script).with_context(|| format!("reading lua script {script}"))?;
    let mut host = Host::new(script, strip_shebang(&source))
        .with_context(|| format!("loading lua script {script}"))?;
    host.serve()
}

/// A script launched by its shebang keeps the `#!` line; Lua's own interpreter
/// skips it, and `lua.load` does not.
pub(super) fn strip_shebang(source: &str) -> &str {
    match source.strip_prefix("#!") {
        Some(rest) => rest.split_once('\n').map(|(_, body)| body).unwrap_or(""),
        None => source,
    }
}

/// Where the host writes its replies: stdout in `serve`, a capture in tests.
type Sink = Rc<RefCell<Box<dyn Write>>>;

/// Evaluate `source` in a fresh VM and register the plugin table's scopes.
fn load(
    script: &str,
    source: &str,
    shared: &sdk::Shared,
    imports: &module::Imports,
    log: &Log,
) -> mlua::Result<(Lua, Vec<Plugin>, Table)> {
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())?;
    let wayrun = sdk::build(&lua, script, shared, log.clone())?;
    let env = script_env(&lua, &wayrun, script, log.clone(), imports)?;
    let declared: Table = lua
        .load(source)
        .set_name(script)
        .set_environment(env)
        .eval()?;
    let plugins = read_plugins(&declared, script)?;
    fill_scopes(shared, &plugins);
    Ok((lua, plugins, declared))
}

fn scopes(active: &kv::Active) -> sdk::Shared {
    sdk::Shared {
        active: Rc::clone(active),
        kv_paths: Rc::new(RefCell::new(HashMap::new())),
        env_scope: Rc::new(RefCell::new(HashMap::new())),
        exec_scope: Rc::new(RefCell::new(HashMap::new())),
        read_areas: Rc::new(RefCell::new(HashMap::new())),
    }
}

/// One fresh load for `wayrun plugin check`: the plugin tables, and per plugin
/// the keys the table does not define.
pub(super) struct Inspected {
    pub plugins: Vec<Plugin>,
    pub unknown: Vec<Vec<String>>,
    /// A `Plugin` carries the table's functions, so the VM stays alive beside them.
    _lua: Lua,
}

pub(super) fn inspect(script: &str, source: &str) -> Result<Inspected> {
    let active: kv::Active = Rc::new(RefCell::new(None));
    let log = Log::new(script, Rc::clone(&active));
    let shared = scopes(&active);
    let imports: module::Imports = Rc::new(RefCell::new(Vec::new()));
    let (lua, plugins, declared) = load(script, source, &shared, &imports, &log)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut unknown = Vec::new();
    for entry in declared.sequence_values::<Table>() {
        let entry = entry.map_err(|error| anyhow::anyhow!("{error}"))?;
        let keys = manifest::unknown_keys(&entry).map_err(|error| anyhow::anyhow!("{error}"))?;
        unknown.push(keys);
    }
    Ok(Inspected {
        plugins,
        unknown,
        _lua: lua,
    })
}

/// The declared ids are known only after evaluation; the kv paths, the readable
/// env names and the read areas fill here.
fn fill_scopes(shared: &sdk::Shared, plugins: &[Plugin]) {
    let mut paths = HashMap::new();
    let mut envs = HashMap::new();
    let mut execs = HashMap::new();
    let mut areas = HashMap::new();
    for plugin in plugins {
        envs.insert(plugin.id.clone(), plugin.env.clone());
        execs.insert(plugin.id.clone(), plugin.exec.clone());
        areas.insert(plugin.id.clone(), plugin.read.clone());
        if let Some(dir) = sdk::plugin_dir(&plugin.id) {
            paths.insert(plugin.id.clone(), dir.join("kv.db"));
        }
    }
    *shared.kv_paths.borrow_mut() = paths;
    *shared.env_scope.borrow_mut() = envs;
    *shared.exec_scope.borrow_mut() = execs;
    *shared.read_areas.borrow_mut() = areas;
}

/// A watched file's `(mtime_ns, size)`; `None` when it is missing.
type Stamp = Option<(i64, u64)>;

/// The script plus the files it imported, with the stamps they had at load.
fn watch_stamps(script: &str, imports: &module::Imports) -> Vec<(PathBuf, Stamp)> {
    let mut files = vec![PathBuf::from(script)];
    for file in imports.borrow().iter() {
        if !files.contains(file) {
            files.push(file.clone());
        }
    }
    files
        .into_iter()
        .map(|path| {
            let stamp = file_stamp(&path);
            (path, stamp)
        })
        .collect()
}

fn file_stamp(path: &Path) -> Stamp {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos() as i64;
    Some((mtime, meta.len()))
}

struct Host {
    lua: Lua,
    script: String,
    plugins: Vec<Plugin>,
    active: kv::Active,
    log: Log,
    out: Sink,
    shared: sdk::Shared,
    imports: module::Imports,
    stamps: RefCell<Vec<(PathBuf, Stamp)>>,
}

impl Host {
    fn new(script: &str, source: &str) -> Result<Self> {
        let out: Sink = Rc::new(RefCell::new(Box::new(io::stdout())));
        Self::with_sink(script, source, out)
    }

    fn with_sink(script: &str, source: &str, out: Sink) -> Result<Self> {
        // mlua's error carries no Send bound, so it converts here, once.
        let active: kv::Active = Rc::new(RefCell::new(None));
        let log = Log::new(script, Rc::clone(&active));
        let shared = scopes(&active);
        let imports: module::Imports = Rc::new(RefCell::new(Vec::new()));
        let (lua, plugins, _) = load(script, source, &shared, &imports, &log)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let stamps = RefCell::new(watch_stamps(script, &imports));
        Ok(Self {
            lua,
            script: script.to_string(),
            plugins,
            active,
            log,
            out,
            shared,
            imports,
            stamps,
        })
    }

    /// Reload the script and its imports when a watched file changed; a failed
    /// reload keeps the last good VM serving until the next change.
    fn refresh(&mut self) -> bool {
        if !self.stale() {
            return false;
        }
        let source = match fs::read_to_string(&self.script) {
            Ok(source) => source,
            Err(error) => {
                warn(&self.script, &format!("reload failed: {error}"));
                *self.stamps.borrow_mut() = watch_stamps(&self.script, &self.imports);
                return false;
            }
        };
        self.imports.borrow_mut().clear();
        match load(
            &self.script,
            strip_shebang(&source),
            &self.shared,
            &self.imports,
            &self.log,
        ) {
            Ok((lua, plugins, _)) => {
                self.lua = lua;
                self.plugins = plugins;
                *self.stamps.borrow_mut() = watch_stamps(&self.script, &self.imports);
                true
            }
            Err(error) => {
                warn(&self.script, &format!("reload failed: {error}"));
                *self.stamps.borrow_mut() = watch_stamps(&self.script, &self.imports);
                false
            }
        }
    }

    fn stale(&self) -> bool {
        self.stamps
            .borrow()
            .iter()
            .any(|(path, stamp)| file_stamp(path) != *stamp)
    }

    /// Imports can also run lazily inside a search; a file first seen there
    /// joins the watch list with the stamp it had when it loaded.
    fn track_imports(&self) {
        let imports = self.imports.borrow();
        let mut stamps = self.stamps.borrow_mut();
        for file in imports.iter() {
            if !stamps.iter().any(|(path, _)| path == file) {
                stamps.push((file.clone(), file_stamp(file)));
            }
        }
    }

    fn serve(&mut self) -> Result<()> {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            self.refresh();
            let request: serde_json::Value = match serde_json::from_str(&line) {
                Ok(request) => request,
                Err(error) => {
                    respond_error(
                        &mut *self.out.borrow_mut(),
                        &json!(null),
                        -32700,
                        &format!("parse error: {error}"),
                    )?;
                    continue;
                }
            };
            let id = request.get("id").cloned().unwrap_or(json!(null));
            let outcome = self.dispatch(&request);
            let mut out = self.out.borrow_mut();
            match outcome {
                Ok(result) => respond(&mut *out, &id, result)?,
                Err((code, message)) => respond_error(&mut *out, &id, code, &message)?,
            }
        }
        Ok(())
    }

    fn dispatch(
        &self,
        request: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, (i64, String)> {
        let params = request.get("params");
        let id = request.get("id").cloned().unwrap_or(json!(null));
        let stream = params
            .and_then(|params| params.get("stream"))
            .and_then(|stream| stream.as_bool())
            .unwrap_or(false);
        match request.get("method").and_then(|method| method.as_str()) {
            Some("list_plugins") => Ok(json!(
                self.plugins
                    .iter()
                    .map(|plugin| json!({
                        "id": plugin.id,
                        "name": plugin.name,
                        "icon": plugin.icon.clone().unwrap_or_default(),
                        "description": plugin.description.clone().unwrap_or_default(),
                        "api": plugin.api,
                    }))
                    .collect::<Vec<_>>()
            )),
            Some("search") => {
                let plugin = self.plugin(params)?;
                let text = params
                    .and_then(|params| params.get("text"))
                    .and_then(|text| text.as_str())
                    .unwrap_or("");
                let emit = if stream {
                    Some(
                        self.emitter(plugin, &id)
                            .map_err(|error| (-32000, error.to_string()))?,
                    )
                } else {
                    None
                };
                self.call(plugin, "search", plugin.search.as_ref(), text, emit)
            }
            Some("top") => {
                let plugin = self.plugin(params)?;
                self.call(plugin, "top", plugin.top.as_ref(), (), None)
            }
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
        emit: Option<(Function, Rc<Cell<bool>>)>,
    ) -> std::result::Result<serde_json::Value, (i64, String)> {
        let Some(function) = function else {
            return Err((
                -32601,
                format!("plugin {} has no {method} method", plugin.id),
            ));
        };
        *self.active.borrow_mut() = Some(plugin.id.clone());
        self.log.reset();
        let (emit, active) = match emit {
            Some((emit, active)) => (Value::Function(emit), Some(active)),
            None => (Value::Nil, None),
        };
        let mut args = arg
            .into_lua_multi(&self.lua)
            .map_err(|error| (-32000, format!("{method} failed: {error}")))?;
        args.push_back(emit);
        let returned = function.call(args);
        self.track_imports();
        if let Some(active) = active {
            active.set(false);
        }
        let returned: Value = returned.map_err(|error| {
            warn(
                &self.script,
                &format!("plugin {} {method}: {error}", plugin.id),
            );
            (-32000, format!("{method} failed: {error}"))
        })?;
        Ok(rows_json(&self.lua, &self.script, &plugin.id, returned))
    }

    /// The `emit` a streaming search receives: one `partial` line per call, with
    /// the request's own id. The flag stops a saved function after its search.
    fn emitter(
        &self,
        plugin: &Plugin,
        id: &serde_json::Value,
    ) -> mlua::Result<(Function, Rc<Cell<bool>>)> {
        let out = Rc::clone(&self.out);
        let script = self.script.clone();
        let plugin = plugin.id.clone();
        let id = id.clone();
        let active = Rc::new(Cell::new(true));
        let alive = Rc::clone(&active);
        let emit = self.lua.create_function(move |lua, rows: Value| {
            if !alive.get() {
                return Err(mlua::Error::RuntimeError(
                    "emit outside the search that received it".to_string(),
                ));
            }
            let rows = rows_json(lua, &script, &plugin, rows);
            respond_partial(&mut *out.borrow_mut(), &id, rows)
                .map_err(|error| mlua::Error::RuntimeError(format!("partial: {error}")))
        })?;
        Ok((emit, active))
    }
}

/// A returned table as the wire shape: every row is deserialized into the
/// core's own [`ResultItem`], and one bad row is dropped, never the answer.
fn rows_json(lua: &Lua, script: &str, plugin: &str, returned: Value) -> serde_json::Value {
    let table = match returned {
        Value::Nil => return json!([]),
        Value::Table(table) => table,
        other => {
            warn(
                script,
                &format!("plugin {plugin} returned {other:?}, not a table of rows"),
            );
            return json!([]);
        }
    };
    let mut rows = Vec::new();
    for value in table.sequence_values::<Value>() {
        let Ok(value) = value else { continue };
        match lua.from_value::<ResultItem>(value) {
            Ok(item) => rows.push(serde_json::to_value(item).unwrap_or(serde_json::Value::Null)),
            Err(error) => warn(script, &format!("plugin {plugin} dropped a row: {error}")),
        }
    }
    json!(rows)
}

fn respond(out: &mut impl Write, id: &serde_json::Value, result: serde_json::Value) -> Result<()> {
    let line = json!({ "jsonrpc": "2.0", "result": result, "id": id });
    writeln!(out, "{line}")?;
    out.flush()?;
    Ok(())
}

fn respond_partial(
    out: &mut impl Write,
    id: &serde_json::Value,
    rows: serde_json::Value,
) -> Result<()> {
    let line = json!({
        "jsonrpc": "2.0",
        "method": "partial",
        "params": { "id": id, "rows": rows },
    });
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

    struct Capture(Rc<RefCell<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn host_with_sink(source: &str) -> (Host, Rc<RefCell<Vec<u8>>>) {
        let buffer = Rc::new(RefCell::new(Vec::new()));
        let out: Sink = Rc::new(RefCell::new(Box::new(Capture(Rc::clone(&buffer)))));
        let host = Host::with_sink("/test/script.lua", source, out).unwrap();
        (host, buffer)
    }

    /// A script on disk, so a reload re-reads the real file.
    fn file_host(dir: &tempfile::TempDir, source: &str) -> (Host, Rc<RefCell<Vec<u8>>>) {
        let path = dir.path().join("main.lua");
        fs::write(&path, source).unwrap();
        fs::write(dir.path().join("util.lua"), "return { value = 'one' }").unwrap();
        let buffer = Rc::new(RefCell::new(Vec::new()));
        let out: Sink = Rc::new(RefCell::new(Box::new(Capture(Rc::clone(&buffer)))));
        let host = Host::with_sink(path.to_str().unwrap(), source, out).unwrap();
        (host, buffer)
    }

    const IMPORTING_SCRIPT: &str = r#"
        local util = wayrun.import("util")
        return { { id = "demo", search = function()
            return { { title = util.value } }
        end } }
    "#;

    /// The same import made lazily, inside the search rather than at load.
    const LAZY_IMPORT_SCRIPT: &str = r#"
        return { { id = "demo", search = function()
            return { { title = wayrun.import("util").value } }
        end } }
    "#;

    #[test]
    fn an_edited_import_reloads_before_the_next_request() {
        let dir = tempfile::tempdir().unwrap();
        let (mut host, _buffer) = file_host(&dir, LAZY_IMPORT_SCRIPT);
        assert_eq!(search(&host, "demo", "x")[0]["title"], "one");
        assert!(!host.refresh(), "an unchanged script is not reloaded");

        fs::write(dir.path().join("util.lua"), "return { value = 'two!' }").unwrap();
        assert!(host.refresh(), "the imported file changed");
        assert_eq!(search(&host, "demo", "x")[0]["title"], "two!");
    }

    #[test]
    fn a_failed_reload_keeps_the_last_good_vm() {
        let dir = tempfile::tempdir().unwrap();
        let (mut host, _buffer) = file_host(&dir, IMPORTING_SCRIPT);
        assert_eq!(search(&host, "demo", "x")[0]["title"], "one");

        fs::write(dir.path().join("util.lua"), "this is not lua").unwrap();
        assert!(!host.refresh(), "the reload failed");
        assert_eq!(
            search(&host, "demo", "x")[0]["title"],
            "one",
            "the last good VM still answers"
        );

        fs::write(dir.path().join("util.lua"), "return { value = 'three' }").unwrap();
        assert!(host.refresh(), "a later fix is picked up");
        assert_eq!(search(&host, "demo", "x")[0]["title"], "three");
    }

    #[test]
    fn an_edited_script_reloads_the_same_way() {
        let dir = tempfile::tempdir().unwrap();
        let (mut host, _buffer) = file_host(&dir, IMPORTING_SCRIPT);
        let rewritten = r#"
            return { { id = "demo", search = function()
                return { { title = "rewritten" } }
            end } }
        "#;
        fs::write(dir.path().join("main.lua"), rewritten).unwrap();
        assert!(host.refresh(), "the script itself changed");
        assert_eq!(search(&host, "demo", "x")[0]["title"], "rewritten");
    }

    #[test]
    fn a_streaming_search_emits_partial_rows() {
        let (host, buffer) = host_with_sink(
            r#"
            return { { id = "demo", search = function(text, emit)
                assert(type(emit) == "function", "a streaming search has no emit")
                emit({ { title = "first" }, { title = 5 } })
                return { { title = "final" } }
            end } }
            "#,
        );
        let rows = host
            .dispatch(&json!({
                "method": "search",
                "params": { "plugin": "demo", "text": "x", "stream": true },
                "id": 17,
            }))
            .unwrap();
        assert_eq!(rows[0]["title"], "final");

        let out = String::from_utf8(buffer.borrow().clone()).unwrap();
        let lines: Vec<serde_json::Value> = out
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["method"], "partial");
        assert_eq!(lines[0]["params"]["id"], 17);
        // one bad row is dropped, never the partial
        assert_eq!(lines[0]["params"]["rows"].as_array().unwrap().len(), 1);
        assert_eq!(lines[0]["params"]["rows"][0]["title"], "first");
    }

    #[test]
    fn a_plain_search_gets_no_emitter() {
        let (host, buffer) = host_with_sink(
            r#"
            return { { id = "demo", search = function(text, emit)
                return { { title = tostring(emit) } }
            end } }
            "#,
        );
        let rows = host
            .dispatch(&json!({
                "method": "search",
                "params": { "plugin": "demo", "text": "x" },
                "id": 1,
            }))
            .unwrap();
        assert_eq!(rows[0]["title"], "nil");
        assert!(buffer.borrow().is_empty());
    }

    #[test]
    fn a_saved_emitter_stops_after_its_search() {
        let (host, _buffer) = host_with_sink(
            r#"
            local saved
            return { { id = "demo", search = function(text, emit)
                if text == "save" then
                    saved = emit
                    return {}
                end
                saved({ { title = "late" } })
                return {}
            end } }
            "#,
        );
        let request = |text: &str| {
            json!({
                "method": "search",
                "params": { "plugin": "demo", "text": text, "stream": true },
                "id": 1,
            })
        };
        assert!(host.dispatch(&request("save")).is_ok());
        assert!(host.dispatch(&request("later")).is_err());
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
    fn os_io_package_and_load_are_unreachable() {
        host(
            r#"
            assert(os == nil, "os leaked")
            assert(io == nil, "io leaked")
            assert(package == nil, "package leaked")
            assert(load == nil, "load leaked")
            return { { id = "demo", search = function() return {} end } }
            "#,
        );
    }

    #[test]
    fn exec_runs_a_declared_program_with_argv() {
        let host = host(
            r#"
            return { { id = "demo", exec = { "sh" }, search = function()
                local out = wayrun.exec("sh", { "-c", "printf out; printf err >&2; exit 3" })
                return { { title = out.stdout, summary = out.stderr .. " " .. out.code } }
            end } }
            "#,
        );
        let rows = search(&host, "demo", "x");
        assert_eq!(rows[0]["title"], "out");
        assert_eq!(rows[0]["summary"], "err 3");
    }

    #[test]
    fn exec_refuses_an_undeclared_program() {
        let host = host(
            r#"
            return { { id = "demo", search = function()
                wayrun.exec("sh", { "-c", "true" })
                return {}
            end } }
            "#,
        );
        assert!(call(&host, "search", json!({ "plugin": "demo", "text": "x" })).is_err());
    }

    #[test]
    fn exec_declarations_are_program_names() {
        let error = Host::new(
            "/test/script.lua",
            r#"return { { id = "demo", exec = { "/bin/sh" }, search = function() return {} end } }"#,
        )
        .err()
        .expect("a path is not a program name");
        assert!(error.to_string().contains("program name"), "{error}");
    }

    #[test]
    fn print_writes_to_the_journal() {
        host(
            r#"
            print("debug", 1)
            return { { id = "demo", search = function() return {} end } }
            "#,
        );
    }

    #[test]
    fn import_loads_a_module_next_to_the_script() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("util.lua"), "return { answer = 42 }").unwrap();
        let script = dir.path().join("main.lua");
        fs::write(
            &script,
            r#"return { { id = "demo", search = function()
                local util = wayrun.import("util")
                return { { title = tostring(util.answer) } }
            end } }"#,
        )
        .unwrap();
        let source = fs::read_to_string(&script).unwrap();
        let host = Host::new(script.to_str().unwrap(), &source).unwrap();
        assert_eq!(search(&host, "demo", "x")[0]["title"], "42");
    }

    #[test]
    fn a_manifest_icon_is_a_shipped_file_or_nothing() {
        let host = host(
            r#"
            return {
              { id = "file", icon = "/tmp/x.svg", search = function() return {} end },
              { id = "glyph", icon = "builtin:power", search = function() return {} end },
              { id = "theme", icon = "firefox", search = function() return {} end },
            }
            "#,
        );
        let icons: Vec<_> = host.plugins.iter().map(|p| p.icon.as_deref()).collect();
        assert_eq!(icons, [Some("/tmp/x.svg"), None, None]);
    }

    #[test]
    fn a_manifest_api_reaches_the_identity_reply() {
        let host = host(
            r#"
            return {
              { id = "current", api = 1, search = function() return {} end },
              { id = "unversioned", search = function() return {} end },
            }
            "#,
        );
        let list = call(&host, "list_plugins", json!({})).unwrap();
        let apis: Vec<_> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["api"].as_u64())
            .collect();
        assert_eq!(apis, [Some(1), Some(0)]);
    }

    #[test]
    fn the_sdk_icon_helper_resolves_a_path_but_no_glyph() {
        let host = host(
            r#"
            return {
              { id = "path", icon = wayrun.icon("/tmp/x.svg"),
                search = function() return {} end },
              { id = "glyph", icon = wayrun.icon("builtin:app"),
                search = function() return {} end },
            }
            "#,
        );
        let icons: Vec<_> = host.plugins.iter().map(|p| p.icon.as_deref()).collect();
        assert_eq!(icons, [Some("/tmp/x.svg"), None]);
    }

    #[test]
    fn time_env_and_script_dir_are_bound() {
        let host = host(
            r#"
            return {
              {
                id = "demo",
                env = { "PATH" },
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
        assert_eq!(rows[1]["title"], "true", "a declared PATH is readable");
        // The test script path is fabricated, so the fallback parent wins.
        assert_eq!(rows[2]["title"], "/test");
    }

    #[test]
    fn an_undeclared_env_name_is_refused() {
        let host = host(
            r#"
            return {
              {
                id = "demo",
                env = { "PATH" },
                search = function()
                  local ok = pcall(wayrun.env, "HOME")
                  return { { title = tostring(ok) } }
                end,
              },
            }
            "#,
        );
        let rows = search(&host, "demo", "x");
        assert_eq!(rows[0]["title"], "false", "an undeclared name raises");
    }

    #[test]
    fn each_plugin_reads_only_its_own_env() {
        let host = host(
            r#"
            return {
              {
                id = "one",
                env = { "PATH" },
                search = function()
                  return { { title = tostring(wayrun.env("PATH") ~= nil) } }
                end,
              },
              {
                id = "two",
                search = function()
                  local ok = pcall(wayrun.env, "PATH")
                  return { { title = tostring(ok) } }
                end,
              },
            }
            "#,
        );
        assert_eq!(search(&host, "one", "x")[0]["title"], "true");
        assert_eq!(
            search(&host, "two", "x")[0]["title"],
            "false",
            "a sibling plugin's declaration does not carry over"
        );
    }

    #[test]
    fn an_env_read_while_the_script_loads_is_refused() {
        let loaded = Host::new(
            "/test/script.lua",
            r#"
            wayrun.env("PATH")
            return { { id = "demo", search = function() return {} end } }
            "#,
        );
        assert!(
            loaded.is_err(),
            "a load-time read has no plugin to attribute it to"
        );
    }

    #[test]
    fn an_env_pattern_is_refused_at_load() {
        let loaded = Host::new(
            "/test/script.lua",
            r#"return { { id = "demo", env = { "YOUDAO_*" }, search = function() return {} end } }"#,
        );
        assert!(loaded.is_err(), "env names are exact");
    }

    #[test]
    fn an_undeclared_area_is_refused() {
        let host = host(
            r#"
            return {
              {
                id = "demo",
                search = function()
                  local list = pcall(wayrun.fs.list, "/etc")
                  local stat = pcall(wayrun.fs.stat, "/etc/hostname")
                  local db = pcall(wayrun.sqlite.snapshot, "/etc/hostname")
                  return { { title = tostring(list) .. tostring(stat) .. tostring(db) } }
                end,
              },
            }
            "#,
        );
        let rows = search(&host, "demo", "x");
        assert_eq!(rows[0]["title"], "falsefalsefalse");
    }

    #[test]
    fn a_declared_area_serves_list_stat_and_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.txt"), "hi").unwrap();
        {
            let connection = rusqlite::Connection::open(dir.path().join("data.sqlite")).unwrap();
            connection
                .execute_batch("CREATE TABLE t (name TEXT); INSERT INTO t VALUES ('row');")
                .unwrap();
        }
        let source = r#"
            return {
              {
                id = "demo",
                read = { "{{DIR}}" },
                search = function()
                  local list = wayrun.fs.list("{{DIR}}") or {}
                  local stat = wayrun.fs.stat("{{DIR}}/note.txt")
                  local db = wayrun.sqlite.snapshot("{{DIR}}/data.sqlite")
                  local rows = wayrun.sqlite.query(db, "SELECT name FROM t")
                  return {
                    { title = table.concat(list, ",") },
                    { title = tostring(stat ~= nil) },
                    { title = rows[1].name },
                  }
                end,
              },
            }
            "#
        .replace("{{DIR}}", &dir.path().display().to_string());
        let host = host(&source);
        let rows = search(&host, "demo", "x");
        assert_eq!(rows[0]["title"], "data.sqlite,note.txt");
        assert_eq!(rows[1]["title"], "true");
        assert_eq!(rows[2]["title"], "row");
    }

    #[test]
    fn a_sibling_cannot_query_another_plugins_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        {
            let connection = rusqlite::Connection::open(dir.path().join("data.sqlite")).unwrap();
            connection
                .execute_batch("CREATE TABLE t (name TEXT); INSERT INTO t VALUES ('row');")
                .unwrap();
        }
        let source = r#"
            return {
              {
                id = "owner",
                read = { "{{DIR}}" },
                search = function()
                  return { { title = tostring(wayrun.sqlite.snapshot("{{DIR}}/data.sqlite")) } }
                end,
              },
              {
                id = "sibling",
                search = function()
                  local ok = pcall(wayrun.sqlite.query, 1, "SELECT name FROM t")
                  return { { title = tostring(ok) } }
                end,
              },
              {
                id = "same-area",
                read = { "{{DIR}}" },
                search = function()
                  local handle = wayrun.sqlite.snapshot("{{DIR}}/data.sqlite")
                  local rows = wayrun.sqlite.query(handle, "SELECT name FROM t")
                  return { { title = rows[1].name } }
                end,
              },
            }
            "#
        .replace("{{DIR}}", &dir.path().display().to_string());
        let host = host(&source);
        assert_eq!(
            search(&host, "owner", "x")[0]["title"],
            "1",
            "the owner opens handle 1"
        );
        assert_eq!(
            search(&host, "sibling", "x")[0]["title"],
            "false",
            "a plugin without the area cannot borrow the view"
        );
        assert_eq!(
            search(&host, "same-area", "x")[0]["title"],
            "row",
            "a plugin declaring the same area shares the handle"
        );
    }

    #[test]
    fn a_load_time_area_read_is_refused() {
        let loaded = Host::new(
            "/test/script.lua",
            r#"local _ = wayrun.fs.list("/etc") return { { id = "demo", search = function() return {} end } }"#,
        );
        assert!(loaded.is_err(), "the read has no plugin to attribute");
    }

    #[test]
    fn a_read_pattern_is_refused_at_load() {
        let loaded = Host::new(
            "/test/script.lua",
            r#"return { { id = "demo", read = { "~/Documents/*" }, search = function() return {} end } }"#,
        );
        assert!(loaded.is_err(), "areas are exact paths");
    }

    #[test]
    fn which_finds_a_command_on_the_path() {
        let host = host(
            r#"
            return {
              {
                id = "demo",
                search = function()
                  return {
                    { title = tostring(wayrun.which("sh") ~= nil) },
                    { title = tostring(wayrun.which("wayrun-not-a-command") == nil) },
                  }
                end,
              },
            }
            "#,
        );
        let rows = search(&host, "demo", "x");
        assert_eq!(rows[0]["title"], "true", "sh is on the PATH");
        assert_eq!(rows[1]["title"], "true", "an unknown name is nil");
    }

    #[test]
    fn fs_read_is_scoped_and_toml_decodes() {
        let host = host(
            r#"
            return {
              {
                id = "demo",
                search = function()
                  local decoded = wayrun.toml.decode("name = 'demo'\ncount = 3")
                  return {
                    { title = tostring(decoded.name) .. "/" .. tostring(decoded.count) },
                    { title = tostring(wayrun.fs.read("nope.txt") == nil) },
                    { title = tostring(not pcall(wayrun.fs.read, "../plugins.toml")) },
                    { title = tostring(not pcall(wayrun.fs.read, "/etc/hostname")) },
                    {
                      title = tostring(
                        (wayrun.plugin_dir("demo") or ""):match("/wayrun/plugins/demo$") ~= nil
                      ),
                    },
                  }
                end,
              },
            }
            "#,
        );
        let rows = search(&host, "demo", "x");
        assert_eq!(rows[0]["title"], "demo/3");
        assert_eq!(
            rows[1]["title"], "true",
            "a missing file is nil, not an error"
        );
        assert_eq!(rows[2]["title"], "true", "a `..` name is refused");
        assert_eq!(rows[3]["title"], "true", "an absolute name is refused");
        assert_eq!(
            rows[4]["title"], "true",
            "the plugin dir sits in the config tree"
        );
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
}
