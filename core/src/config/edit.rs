use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use toml_edit::{DocumentMut, Item, Table};

use super::{Config, DEFAULT_TEMPLATE, path};

/// Every effective setting as a dotted key and its rendered value.
pub fn list() -> anyhow::Result<Vec<(String, String)>> {
    let value = effective()?;
    let mut entries = Vec::new();
    flatten(&value, "", &mut |key, value| {
        entries.push((key, render(value)))
    });
    Ok(entries)
}

/// One effective setting, as the terminal shows it.
pub fn get(key: &str) -> anyhow::Result<String> {
    let defaults = defaults()?;
    check_key(key, &defaults)?;
    let value = effective()?;
    let found = lookup(&value, key).context("the effective config lost a key")?;
    Ok(render(found))
}

/// Set one key in the user's `config.toml`, keeping the file's comments.
pub fn set(key: &str, raw: &str) -> anyhow::Result<()> {
    let defaults = defaults()?;
    let default = lookup(&defaults, key).with_context(|| unknown(key, &defaults))?;
    let input =
        parse_input(raw, default).with_context(|| format!("{key} takes {}", describe(default)))?;
    if std::mem::discriminant(&input) != std::mem::discriminant(default) {
        bail!("{key} takes {}", describe(default));
    }
    let (path, mut doc) = load_doc()?;
    assign(&mut doc, key, to_item(&input)?)?;
    save(&path, &doc)
}

/// Drop one key from the user's `config.toml`, so the default applies again.
pub fn unset(key: &str) -> anyhow::Result<()> {
    let defaults = defaults()?;
    check_key(key, &defaults)?;
    let (path, mut doc) = load_doc()?;
    remove(&mut doc, key);
    save(&path, &doc)
}

fn effective() -> anyhow::Result<toml::Value> {
    Ok(toml::Value::try_from(super::get().as_ref())?)
}

fn defaults() -> anyhow::Result<toml::Value> {
    Ok(toml::Value::try_from(Config::default())?)
}

fn check_key(key: &str, defaults: &toml::Value) -> anyhow::Result<()> {
    lookup(defaults, key)
        .map(|_| ())
        .with_context(|| unknown(key, defaults))
}

fn lookup<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut value = root;
    for part in key.split('.') {
        value = value.get(part)?;
    }
    Some(value)
}

fn flatten(value: &toml::Value, prefix: &str, visit: &mut impl FnMut(String, &toml::Value)) {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(child, &path, visit);
            }
        }
        _ => visit(prefix.to_string(), value),
    }
}

fn render(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) if text.is_empty() => "\"\"".to_string(),
        toml::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn parse_input(raw: &str, default: &toml::Value) -> anyhow::Result<toml::Value> {
    match raw.parse::<toml::Value>() {
        Ok(value) => Ok(value),
        Err(_) if default.is_str() => Ok(toml::Value::String(raw.to_string())),
        Err(error) => bail!("not a TOML value: {error}"),
    }
}

fn describe(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "a string",
        toml::Value::Boolean(_) => "true or false",
        toml::Value::Array(_) => "a TOML array",
        _ => "a TOML value",
    }
}

fn unknown(key: &str, defaults: &toml::Value) -> String {
    let mut keys = Vec::new();
    flatten(defaults, "", &mut |path, _| keys.push(path));
    format!("no config key {key:?}; one of: {}", keys.join(", "))
}

fn load_doc() -> anyhow::Result<(PathBuf, DocumentMut)> {
    let path = path().context("no config directory")?;
    let _ = crate::write_if_absent(&path, DEFAULT_TEMPLATE);
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

fn assign(doc: &mut DocumentMut, key: &str, item: Item) -> anyhow::Result<()> {
    let (path, leaf) = key.rsplit_once('.').unwrap_or(("", key));
    let mut table = doc.as_table_mut();
    for part in path.split('.').filter(|part| !part.is_empty()) {
        table = table
            .entry(part)
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .with_context(|| format!("{part} is not a section"))?;
    }
    table[leaf] = item;
    Ok(())
}

fn remove(doc: &mut DocumentMut, key: &str) {
    let (path, leaf) = key.rsplit_once('.').unwrap_or(("", key));
    let mut current = Some(doc.as_table_mut());
    for part in path.split('.').filter(|part| !part.is_empty()) {
        current = current
            .and_then(|table| table.get_mut(part))
            .and_then(Item::as_table_mut);
    }
    if let Some(table) = current {
        table.remove(leaf);
    }
}

fn to_item(value: &toml::Value) -> anyhow::Result<Item> {
    let mut doc: DocumentMut = format!("v = {value}")
        .parse()
        .context("encoding the value")?;
    Ok(doc.as_table_mut().remove("v").expect("just parsed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_key_flattens_to_a_path() {
        let defaults = toml::Value::try_from(Config::default()).unwrap();
        let mut keys = Vec::new();
        flatten(&defaults, "", &mut |path, _| keys.push(path));
        assert!(keys.contains(&"ui.locale".to_string()));
        assert!(keys.contains(&"files.exclude".to_string()));
        assert!(lookup(&defaults, "files.index").is_some());
        assert!(lookup(&defaults, "files.nope").is_none());
    }

    #[test]
    fn a_bare_string_falls_back_only_for_text_keys() {
        let text = toml::Value::String(String::new());
        assert_eq!(
            parse_input("zh_cn", &text).unwrap(),
            toml::Value::String("zh_cn".to_string())
        );
        assert!(parse_input("zh_cn", &toml::Value::Boolean(true)).is_err());
        assert_eq!(
            parse_input("[\"a\", \"b\"]", &toml::Value::Array(Vec::new())).unwrap(),
            toml::Value::Array(vec![
                toml::Value::String("a".to_string()),
                toml::Value::String("b".to_string()),
            ])
        );
    }

    #[test]
    fn a_set_keeps_the_template_comments() {
        let mut doc: DocumentMut = DEFAULT_TEMPLATE.parse().unwrap();
        let item = to_item(&toml::Value::Boolean(false)).unwrap();
        assign(&mut doc, "files.index", item).unwrap();
        let text = doc.to_string();
        assert!(text.contains("# locale = \"zh_cn\""));
        assert!(text.contains("index = false"));
        toml::from_str::<Config>(&text).unwrap();
    }

    #[test]
    fn an_unset_removes_only_the_key() {
        let mut doc: DocumentMut = "[files]\nindex = false\n# keep\nexclude = []\n"
            .parse()
            .unwrap();
        remove(&mut doc, "files.index");
        let text = doc.to_string();
        assert!(!text.contains("index = false"));
        assert!(text.contains("# keep"));
        assert!(text.contains("exclude = []"));
        remove(&mut doc, "files.index");
        remove(&mut doc, "nope.path");
    }

    #[test]
    fn values_render_for_the_terminal() {
        assert_eq!(render(&toml::Value::String("zh_cn".to_string())), "zh_cn");
        assert_eq!(render(&toml::Value::String(String::new())), "\"\"");
        assert_eq!(render(&toml::Value::Boolean(true)), "true");
        assert_eq!(render(&toml::Value::Array(Vec::new())), "[]");
    }
}
