use mlua::{Lua, Table, Value};

/// The script's whole world: a curated core of Lua plus `wayrun`. `os`, `io`,
/// `package` and `load` are absent, and so is `print` (stdout is the wire).
pub(super) fn script_env(lua: &Lua, wayrun: &Table) -> mlua::Result<Table> {
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
