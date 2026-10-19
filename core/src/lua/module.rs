use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mlua::{Lua, Table, Value};

use super::areas;
use super::sdk::script_dir;

/// The files a script imported, so a resident host can reload when one changes.
pub(super) type Imports = Rc<RefCell<Vec<PathBuf>>>;

/// `wayrun.import(name)`: load a module from the script's own directory, in the
/// same sandbox `_ENV`, once; a nil return caches as `true`, as `require` does.
pub(super) fn bind(
    lua: &Lua,
    wayrun: &Table,
    env: &Table,
    script: &str,
    imports: &Imports,
) -> mlua::Result<()> {
    let root = script_dir(script);
    let imports = Rc::clone(imports);
    let env_key = lua.create_registry_value(env.clone())?;
    let cache_key = lua.create_registry_value(lua.create_table()?)?;
    let loading_key = lua.create_registry_value(lua.create_table()?)?;
    wayrun.set(
        "import",
        lua.create_function(move |lua, name: String| {
            let Some(root) = &root else {
                return Err(mlua::Error::RuntimeError(
                    "import: the script has no directory".to_string(),
                ));
            };
            let file = module_path(root, &name)?;
            // Recorded before the read, so a module that fails to load is
            // watched for the fix too.
            {
                let mut seen = imports.borrow_mut();
                if !seen.contains(&file) {
                    seen.push(file.clone());
                }
            }
            let key = file.to_string_lossy().to_string();

            let cache: Table = lua.registry_value(&cache_key)?;
            let loading: Table = lua.registry_value(&loading_key)?;
            if loading.get::<Option<Value>>(key.as_str())?.is_some() {
                return Err(mlua::Error::RuntimeError(format!(
                    "import {name:?}: circular import"
                )));
            }
            if let Some(value) = cache.get::<Option<Value>>(key.as_str())? {
                return Ok(value);
            }

            let source = std::fs::read_to_string(&file)
                .map_err(|error| mlua::Error::RuntimeError(format!("import {name:?}: {error}")))?;
            loading.set(key.as_str(), true)?;
            let env: Table = lua.registry_value(&env_key)?;
            let loaded = lua
                .load(&source)
                .set_name(key.as_str())
                .set_environment(env)
                .eval::<Value>();
            loading.set(key.as_str(), Value::Nil)?;

            let value = loaded
                .map_err(|error| mlua::Error::RuntimeError(format!("import {name:?}: {error}")))?;
            let cached = if value.is_nil() {
                Value::Boolean(true)
            } else {
                value
            };
            cache.set(key.as_str(), &cached)?;
            Ok(cached)
        })?,
    )?;
    Ok(())
}

/// A module name resolves under the script's directory; `.lua` is implied. It
/// may not climb out, and a symlink may not resolve outside that directory.
fn module_path(root: &Path, name: &str) -> Result<PathBuf, mlua::Error> {
    if name.is_empty() {
        return Err(mlua::Error::RuntimeError(
            "import: the module name is empty".to_string(),
        ));
    }
    let name = if name.ends_with(".lua") {
        name.to_string()
    } else {
        format!("{name}.lua")
    };
    areas::inside(root, &name)
        .map_err(|problem| mlua::Error::RuntimeError(format!("import {name:?}: {problem}")))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use mlua::{Function, Lua, LuaOptions, StdLib};

    use super::*;
    use crate::lua::log::Log;
    use crate::lua::sandbox::script_env;

    fn wayrun_for(dir: &Path) -> (Lua, Table) {
        let script = dir.join("main.lua");
        std::fs::write(&script, "").unwrap();
        let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
        let wayrun = lua.create_table().unwrap();
        let active = Rc::new(RefCell::new(None));
        let log = Log::new(script.to_str().unwrap(), active);
        let imports = Rc::new(RefCell::new(Vec::new()));
        script_env(&lua, &wayrun, script.to_str().unwrap(), log, &imports).unwrap();
        (lua, wayrun)
    }

    #[test]
    fn an_import_loads_once_and_caches_the_result() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("util.lua"),
            "count = (count or 0) + 1; return { n = count }",
        )
        .unwrap();
        let (_lua, wayrun) = wayrun_for(dir.path());
        let import: Function = wayrun.get("import").unwrap();

        let first: Table = import.call("util").unwrap();
        let again: Table = import.call("util.lua").unwrap();
        assert_eq!(first.get::<i32>("n").unwrap(), 1);
        assert_eq!(again.get::<i32>("n").unwrap(), 1, "the module is cached");
    }

    #[test]
    fn a_circular_import_raises() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.lua"), "return wayrun.import('b')").unwrap();
        std::fs::write(dir.path().join("b.lua"), "return wayrun.import('a')").unwrap();
        let (_lua, wayrun) = wayrun_for(dir.path());
        let import: Function = wayrun.get("import").unwrap();
        let error = import.call::<Value>("a").unwrap_err().to_string();
        assert!(error.contains("circular"), "{error}");
    }

    #[test]
    fn an_import_cannot_escape_the_script_dir() {
        let dir = tempfile::tempdir().unwrap();
        let (_lua, wayrun) = wayrun_for(dir.path());
        let import: Function = wayrun.get("import").unwrap();
        assert!(import.call::<Value>("../x").is_err());
        assert!(import.call::<Value>("/etc/hostname").is_err());
        assert!(import.call::<Value>("").is_err());
    }
}
