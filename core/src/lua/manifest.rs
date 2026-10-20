use std::path::PathBuf;

use mlua::{Function, Table, Value};

use super::areas;
use super::warn;

pub(super) struct Plugin {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub api: u32,
    pub description: Option<String>,
    pub env: Vec<String>,
    pub exec: Vec<String>,
    pub read: Vec<PathBuf>,
    pub search: Option<Function>,
    pub top: Option<Function>,
    pub command: Option<Function>,
}

/// The keys a plugin table may define; `wayrun plugin check` names anything else.
const KEYS: &[&str] = &[
    "id",
    "name",
    "icon",
    "description",
    "api",
    "env",
    "exec",
    "read",
    "search",
    "top",
    "command",
];

pub(super) fn unknown_keys(entry: &Table) -> mlua::Result<Vec<String>> {
    let mut unknown = Vec::new();
    for pair in entry.pairs::<String, Value>() {
        let (key, _) = pair?;
        if !KEYS.contains(&key.as_str()) {
            unknown.push(key);
        }
    }
    Ok(unknown)
}

pub(super) fn read_plugins(declared: &Table, script: &str) -> mlua::Result<Vec<Plugin>> {
    let mut plugins = Vec::new();
    for entry in declared.sequence_values::<Table>() {
        let entry = entry?;
        let id: String = entry.get("id")?;
        // The id becomes a filesystem scope root, so it must be a plain name.
        if id.is_empty() || id.contains('/') || id == "." || id == ".." {
            return Err(mlua::Error::RuntimeError(format!(
                "{script}: plugin id {id:?} is not a plain name"
            )));
        }
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
            Some(spec) => {
                let kept = crate::system::icon::host_icon_spec(&spec);
                if kept.is_none() {
                    warn(
                        script,
                        &format!("plugin {id}: icon {spec:?} is not an absolute path; dropped"),
                    );
                }
                kept
            }
            None => None,
        };
        let env: Vec<String> = entry.get::<Option<Vec<String>>>("env")?.unwrap_or_default();
        for name in &env {
            // Names are exact: a pattern would defeat the manifest's point, a
            // plugin naming the secrets it reads.
            if name.is_empty() || name.contains('*') {
                return Err(mlua::Error::RuntimeError(format!(
                    "{script}: plugin {id}: env {name:?} is not a plain name"
                )));
            }
        }
        let exec: Vec<String> = entry
            .get::<Option<Vec<String>>>("exec")?
            .unwrap_or_default();
        for name in &exec {
            // A PATH name, never a path: a plugin reuses the session's tools
            // instead of shipping an executable of its own.
            if name.is_empty() || name.contains('/') || name.contains('\0') {
                return Err(mlua::Error::RuntimeError(format!(
                    "{script}: plugin {id}: exec {name:?} is not a program name"
                )));
            }
        }
        let mut read = Vec::new();
        for spec in entry
            .get::<Option<Vec<String>>>("read")?
            .unwrap_or_default()
        {
            // A pattern would defeat the manifest's point, a plugin naming the
            // areas it looks at; `expand` already refuses `..` and relatives.
            match areas::expand(&spec) {
                Some(path) if !spec.contains('*') => read.push(path),
                _ => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "{script}: plugin {id}: read {spec:?} is not a plain path"
                    )));
                }
            }
        }
        plugins.push(Plugin {
            id,
            name,
            icon,
            api: entry.get::<Option<u32>>("api")?.unwrap_or(0),
            description: entry.get("description")?,
            env,
            exec,
            read,
            search: entry.get("search")?,
            top: entry.get("top")?,
            command: entry.get("command")?,
        });
    }
    if plugins.is_empty() {
        return Err(mlua::Error::RuntimeError(format!(
            "{script} declares no plugins"
        )));
    }
    Ok(plugins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_the_table_does_not_define_is_named() {
        let lua = mlua::Lua::new();
        let entry: Table = lua
            .load(r#"return { id = "x", keyword = "y", search = function() end }"#)
            .eval()
            .unwrap();
        assert_eq!(unknown_keys(&entry).unwrap(), ["keyword"]);
    }
}
