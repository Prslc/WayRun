use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use super::config::{Config, load_or_default};

/// Register one Lua host in the user's `plugins.toml`.
pub fn add_plugin(id: &str, keyword: &str, script: &Path, resident: bool) -> anyhow::Result<()> {
    let effective = load_or_default();
    if effective.plugins.iter().any(|entry| entry.id == id) {
        bail!("plugin {id:?} is already registered");
    }
    check_keyword(&effective, id, keyword)?;
    let command =
        fs::canonicalize(script).with_context(|| format!("resolving {}", script.display()))?;

    let (path, mut doc) = load_doc()?;
    let mut table = Table::new();
    table["id"] = value(id);
    table["keyword"] = value(keyword);
    table["command"] = value(command.to_string_lossy().into_owned());
    if resident {
        table["resident"] = value(true);
    }
    plugins(&mut doc)?.push(table);
    save(&path, &doc)
}

/// Enable or disable one entry; a built-in gets a user entry that overrides it.
pub fn set_enabled(id: &str, enabled: bool) -> anyhow::Result<()> {
    let effective = load_or_default();
    let Some(source) = effective.plugins.iter().find(|entry| entry.id == id) else {
        bail!("no plugin {id:?}");
    };
    if source.enabled == enabled {
        return Ok(());
    }
    let keyword = source.keyword.clone();

    let (path, mut doc) = load_doc()?;
    write_enabled(&mut doc, id, &keyword, enabled)?;
    save(&path, &doc)
}

/// Drop the user entry for `id`; a built-in falls back to its shipped entry.
pub fn remove_plugin(id: &str) -> anyhow::Result<()> {
    if !load_or_default().plugins.iter().any(|entry| entry.id == id) {
        bail!("no plugin {id:?}");
    }
    let (path, mut doc) = load_doc()?;
    if !drop_entry(&mut doc, id)? {
        bail!("{id:?} is a built-in, so it has no user entry; disable it instead");
    }
    save(&path, &doc)
}

fn load_doc() -> anyhow::Result<(PathBuf, DocumentMut)> {
    // `load_or_default` writes the shipped template when the file is absent.
    let _ = load_or_default();
    let path = crate::config::dir()
        .context("no config directory")?
        .join("plugins.toml");
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok((path, doc))
}

fn save(path: &Path, doc: &DocumentMut) -> anyhow::Result<()> {
    let text = doc.to_string();
    toml::from_str::<Config>(&text)
        .with_context(|| format!("the edited {} does not parse", path.display()))?;
    crate::system::fs::write_atomic(path, text.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

fn plugins(doc: &mut DocumentMut) -> anyhow::Result<&mut ArrayOfTables> {
    doc.as_table_mut()
        .entry("plugins")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .context("plugins must be an array of tables")
}

fn write_enabled(
    doc: &mut DocumentMut,
    id: &str,
    keyword: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    let entries = plugins(doc)?;
    let at = entries
        .iter()
        .position(|table| table.get("id").and_then(Item::as_str) == Some(id));
    match at.and_then(|at| entries.get_mut(at)) {
        Some(table) => table["enabled"] = value(enabled),
        None => {
            let mut table = Table::new();
            table["id"] = value(id);
            table["keyword"] = value(keyword);
            table["enabled"] = value(enabled);
            entries.push(table);
        }
    }
    Ok(())
}

/// Whether an entry was removed.
fn drop_entry(doc: &mut DocumentMut, id: &str) -> anyhow::Result<bool> {
    let entries = plugins(doc)?;
    let Some(at) = entries
        .iter()
        .position(|table| table.get("id").and_then(Item::as_str) == Some(id))
    else {
        return Ok(false);
    };
    entries.remove(at);
    Ok(true)
}

fn check_keyword(config: &Config, id: &str, keyword: &str) -> anyhow::Result<()> {
    if keyword == "?" {
        bail!("`?` is the launcher's help query, not a keyword");
    }
    if keyword.split_whitespace().count() > 1 {
        bail!("a keyword is one word");
    }
    if keyword.is_empty() {
        return Ok(());
    }
    if let Some(other) = config
        .plugins
        .iter()
        .find(|entry| entry.enabled && entry.keyword == keyword && entry.id != id)
    {
        bail!("keyword {keyword:?} is already taken by {:?}", other.id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_keeps_the_file_comments() {
        let mut doc: DocumentMut = "# keep me\n\n[[plugins]]\nid = \"runner\"\nkeyword = \"r\"\n"
            .parse()
            .unwrap();
        write_enabled(&mut doc, "runner", "r", false).unwrap();
        let text = doc.to_string();
        assert!(text.starts_with("# keep me"));
        assert!(text.contains("keyword = \"r\""));
        assert!(text.contains("enabled = false"));
    }

    #[test]
    fn a_built_in_override_carries_the_effective_keyword() {
        let mut doc = DocumentMut::new();
        write_enabled(&mut doc, "file-search", "f", false).unwrap();
        let text = doc.to_string();
        assert!(text.contains("id = \"file-search\""));
        assert!(text.contains("keyword = \"f\""));
        assert!(text.contains("enabled = false"));
    }

    #[test]
    fn removing_one_entry_leaves_the_others_alone() {
        let mut doc: DocumentMut = "[[plugins]]\nid = \"a\"\nkeyword = \"x\"\n\n[[plugins]]\nid = \"b\"\nkeyword = \"y\"\n"
            .parse()
            .unwrap();
        assert!(drop_entry(&mut doc, "a").unwrap());
        let text = doc.to_string();
        assert!(!text.contains("id = \"a\""));
        assert!(text.contains("id = \"b\""));
        assert!(!drop_entry(&mut doc, "a").unwrap());
    }

    #[test]
    fn a_keyword_owned_by_an_enabled_entry_is_refused() {
        let config: Config = toml::from_str(
            r#"
            [[plugins]]
            id = "note"
            keyword = "note"

            [[plugins]]
            id = "off"
            keyword = "later"
            enabled = false
            "#,
        )
        .unwrap();
        assert!(check_keyword(&config, "new", "note").is_err());
        assert!(check_keyword(&config, "note", "note").is_ok());
        assert!(check_keyword(&config, "new", "later").is_ok());
        assert!(check_keyword(&config, "new", "").is_ok());
        assert!(check_keyword(&config, "new", "two words").is_err());
        assert!(check_keyword(&config, "new", "?").is_err());
    }
}
