use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use mlua::{Function, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value};
use serde_json::json;

use crate::wire::ResultItem;

mod sdk;
mod sqlite;

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
            let wayrun = sdk::build(&lua, script)?;
            let env = script_env(&lua, &wayrun)?;
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

/// The script's whole world: a curated core of Lua plus `wayrun`. `os`, `io`,
/// `package` and `load` are absent, and so is `print` (stdout is the wire).
fn script_env(lua: &Lua, wayrun: &Table) -> mlua::Result<Table> {
    let globals = lua.globals();
    let env = lua.create_table()?;
    for name in SAFE_GLOBALS {
        let value: Value = globals.get(*name)?;
        env.set(*name, value)?;
    }
    env.set("wayrun", wayrun)?;
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

pub(super) fn warn(script: &str, message: &str) {
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
}
