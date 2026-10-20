use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result};

use super::host::{inspect, strip_shebang};
use super::manifest::Plugin;
use crate::wire::PLUGIN_API;

const SHEBANG: &str = "#!/usr/bin/env -S wayrun --lua-host";

/// `wayrun plugin check <script.lua>`: load the script the way the host does
/// and report what keeps it from working; `Ok(false)` when a check failed.
pub fn run(script: &str) -> Result<bool> {
    let source =
        fs::read_to_string(script).with_context(|| format!("reading lua script {script}"))?;
    let meta = fs::metadata(script).with_context(|| format!("reading lua script {script}"))?;

    let mut problems = Vec::new();
    let mut notes = Vec::new();
    if source.lines().next() != Some(SHEBANG) {
        problems.push(format!("first line must be `{SHEBANG}`"));
    }
    if meta.permissions().mode() & 0o111 == 0 {
        problems.push("file is not executable (chmod +x)".to_string());
    }

    let checked = inspect(script, strip_shebang(&source))
        .with_context(|| format!("loading lua script {script}"))?;

    println!("{script}");
    for (plugin, unknown) in checked.plugins.iter().zip(&checked.unknown) {
        report(plugin, unknown, &mut notes);
    }
    registration(script, &checked.plugins, &mut problems, &mut notes);

    for problem in &problems {
        println!("error: {problem}");
    }
    for note in &notes {
        println!("warning: {note}");
    }
    println!("{}", if problems.is_empty() { "ok" } else { "failed" });
    Ok(problems.is_empty())
}

fn report(plugin: &Plugin, unknown: &[String], notes: &mut Vec<String>) {
    println!(
        "  plugin {} ({}), api {}",
        plugin.id, plugin.name, plugin.api
    );
    if let Some(icon) = plugin.icon.as_deref() {
        println!("    icon {icon}");
    }
    if !unknown.is_empty() {
        notes.push(format!(
            "plugin {}: unknown keys: {}",
            plugin.id,
            unknown.join(", ")
        ));
    }
    if plugin.api > PLUGIN_API {
        notes.push(format!(
            "plugin {}: api {} is newer than this launcher's {}",
            plugin.id, plugin.api, PLUGIN_API
        ));
    }
    if plugin.search.is_none() && plugin.top.is_none() {
        notes.push(format!(
            "plugin {}: declares neither search nor top",
            plugin.id
        ));
    }
    match plugin.icon.as_deref() {
        Some(icon) if Path::new(icon).exists() => {}
        Some(icon) => notes.push(format!("plugin {}: icon {icon} does not exist", plugin.id)),
        None => notes.push(format!("plugin {}: no identity icon", plugin.id)),
    }
}

fn registration(
    script: &str,
    plugins: &[Plugin],
    problems: &mut Vec<String>,
    notes: &mut Vec<String>,
) {
    let entries = crate::plugin::entries_for_command(script);
    if entries.is_empty() {
        notes.push("not registered in plugins.toml: no entry points at this command".to_string());
        return;
    }
    for (id, keyword, enabled) in &entries {
        println!("  plugins.toml: {id} (keyword {keyword:?})");
        if !plugins.iter().any(|plugin| plugin.id == *id) {
            problems.push(format!(
                "plugins.toml entry {id:?} is not declared by the script"
            ));
        }
        if !enabled {
            notes.push(format!("plugins.toml entry {id:?} is disabled"));
        }
    }
    for plugin in plugins {
        if !entries.iter().any(|(id, _, _)| id == &plugin.id) {
            notes.push(format!("plugin {} has no plugins.toml entry", plugin.id));
        }
    }
}
