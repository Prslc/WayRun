use mlua::{Function, Lua, Table, Value, Variadic};

use super::log::Log;

/// The script's whole world: a curated core of Lua plus `wayrun`. `os`, `io`,
/// `package` and `load` are absent; `print` writes to the journal, never stdout.
pub(super) fn script_env(lua: &Lua, wayrun: &Table, log: Log) -> mlua::Result<Table> {
    let globals = lua.globals();
    let env = lua.create_table()?;
    for name in SAFE_GLOBALS {
        let value: Value = globals.get(*name)?;
        env.set(*name, value)?;
    }
    env.set("wayrun", wayrun)?;
    env.set(
        "print",
        lua.create_function(move |lua, args: Variadic<Value>| {
            let tostring: Function = lua.globals().get("tostring")?;
            let mut parts = Vec::with_capacity(args.len());
            for value in args {
                parts.push(tostring.call::<String>(value)?);
            }
            log.write(&parts.join("\t"));
            Ok(())
        })?,
    )?;
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::lua::kv::Active;

    #[test]
    fn print_formats_like_lua_and_goes_to_the_log() {
        let lua = Lua::new();
        let wayrun = lua.create_table().unwrap();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = {
            let seen = Rc::clone(&seen);
            Rc::new(move |line: &str| seen.borrow_mut().push(line.to_string()))
        };
        let active: Active = Rc::new(RefCell::new(Some("p".to_string())));
        let log = Log::with_sink("/x.lua", active, sink);
        let env = script_env(&lua, &wayrun, log).unwrap();

        let print: Function = env.get("print").unwrap();
        print.call::<()>((1, "x", true)).unwrap();

        assert_eq!(
            seen.borrow().as_slice(),
            ["wayrun-core: lua plugin /x.lua (p): 1\tx\ttrue"]
        );
    }
}
