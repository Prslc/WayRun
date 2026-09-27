use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use serde::Deserialize;

/// Core behaviour loaded from `~/.config/wayrun/config.toml`, whose types are
/// also the file's; every field defaults to the compiled-in constant.
#[derive(serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Config {
    pub ui: Ui,
    pub web_search: WebSearch,
    pub font: Font,
    pub icon: Icon,
    pub files: Files,
}

#[derive(serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Ui {
    /// Interface language: a `locales/<locale>.yml` stem such as `zh_cn`; empty
    /// follows the session's `$LC_ALL`/`$LC_MESSAGES`/`$LANG`.
    #[serde(deserialize_with = "lenient_blank")]
    pub locale: String,
}

#[derive(serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct WebSearch {
    /// `google` or `duckduckgo`; an unknown value falls back to google.
    pub engine: String,
}

#[derive(serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Font {
    /// The family the UI shapes with; missing glyphs fall back to the system, so
    /// a family covering only the scripts you read keeps the rest out of memory.
    #[serde(deserialize_with = "lenient_family")]
    pub family: String,
}

#[derive(serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Icon {
    /// Icon theme name; empty follows the desktop's own setting.
    #[serde(deserialize_with = "lenient_blank")]
    pub theme: String,
}

#[derive(serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Files {
    /// Index `$HOME` so `f`/`d` match at any depth; the cost is one cache file.
    pub index: bool,
    /// Directory names the walk never enters and a match never shows (a hidden
    /// entry, a leading `.`, is skipped anyway); a user's list replaces this one.
    pub exclude: Vec<String>,
}

/// A blank text key means "follow the session/desktop", the same as absent.
fn lenient_blank<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let text = String::deserialize(d)?;
    Ok(if text.trim().is_empty() {
        String::new()
    } else {
        text
    })
}

/// A blank family keeps the shipped default instead of shaping with nothing.
fn lenient_family<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let family = String::deserialize(d)?;
    Ok(if family.trim().is_empty() {
        Font::default().family
    } else {
        family
    })
}

impl Default for Files {
    fn default() -> Self {
        Self {
            index: true,
            exclude: SHIPPED_EXCLUDES
                .iter()
                .map(|name| name.to_string())
                .collect(),
        }
    }
}

/// Names the walk never enters unless the user writes their own `exclude` list;
/// the shipped template shows them commented out.
const SHIPPED_EXCLUDES: [&str; 3] = ["node_modules", "target", "__pycache__"];

impl Default for WebSearch {
    fn default() -> Self {
        Self {
            engine: "google".to_string(),
        }
    }
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: "Source Han Sans CN".to_string(),
        }
    }
}

impl Config {
    /// The file's config, or `None` when it is absent or unparseable. A watcher
    /// reload keeps what is applied rather than resetting to defaults.
    fn load_checked() -> Option<Self> {
        let path = path()?;
        let text = std::fs::read_to_string(path).ok()?;
        toml::from_str(&text).ok()
    }

    fn load() -> Self {
        Self::load_checked().unwrap_or_default()
    }
}

pub(crate) const DEFAULT_TEMPLATE: &str = include_str!("../default-config.toml");

/// `~/.config/wayrun`, the one config directory the shell and core share.
pub fn dir() -> Option<PathBuf> {
    Some(crate::system::fs::get_home().ok()?.join(".config/wayrun"))
}

pub fn path() -> Option<PathBuf> {
    Some(dir()?.join("config.toml"))
}

/// Write the shipped template on first use so the settings are discoverable.
/// Absence is already the default, so a write failure changes nothing.
fn write_template() {
    let Some(path) = path() else {
        return;
    };
    let _ = crate::write_if_absent(&path, DEFAULT_TEMPLATE);
}

static CONFIG: OnceLock<RwLock<Arc<Config>>> = OnceLock::new();

fn cell() -> &'static RwLock<Arc<Config>> {
    CONFIG.get_or_init(|| {
        write_template();
        RwLock::new(Arc::new(Config::load()))
    })
}

/// The config as it stands; a reader clones the `Arc`, so a provider resolving a
/// setting per keystroke copies nothing.
pub fn get() -> Arc<Config> {
    Arc::clone(&cell().read().expect("config lock poisoned"))
}

/// Reload `config.toml`, returning whether the value changed; a watcher rebuilds
/// the plugin registry (how a provider sees a setting) only when it did.
pub fn reload() -> bool {
    let Some(new) = Config::load_checked() else {
        return false;
    };
    let new = Arc::new(new);
    let mut guard = cell().write().expect("config lock poisoned");
    if **guard == *new {
        return false;
    }
    *guard = new;
    true
}

pub fn web_search_engine() -> String {
    cell()
        .read()
        .expect("config lock poisoned")
        .web_search
        .engine
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Config {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn a_key_of_the_wrong_type_rejects_the_file() {
        assert!(toml::from_str::<Config>("[files]\nexclude = \"node_modules\"").is_err());
        assert!(toml::from_str::<Config>("[font]\nfamily = 5").is_err());
    }

    #[test]
    fn absent_keys_keep_the_defaults() {
        assert_eq!(parse(""), Config::default());
    }

    #[test]
    fn present_keys_override_field_by_field() {
        let config = parse(
            r#"
            [web_search]
            engine = "duckduckgo"

            [font]
            family = "Noto Sans"

            [icon]
            theme = "Papirus"
            "#,
        );
        assert_eq!(config.web_search.engine, "duckduckgo");
        assert_eq!(config.font.family, "Noto Sans");
        assert_eq!(config.icon.theme, "Papirus");
        assert_eq!(
            parse("[web_search]\nengine = \"google\"").font,
            Font::default()
        );
    }

    #[test]
    fn a_blank_theme_keeps_the_default() {
        assert_eq!(parse("[icon]\ntheme = \"  \"").icon, Icon::default());
    }

    #[test]
    fn a_blank_family_keeps_the_default() {
        assert_eq!(parse("[font]\nfamily = \"  \"").font, Font::default());
    }

    #[test]
    fn the_file_index_can_be_turned_off() {
        assert!(parse("").files.index, "indexing is on unless turned off");
        assert!(!parse("[files]\nindex = false").files.index);
    }

    #[test]
    fn the_excluded_names_default_to_the_shipped_list_and_a_users_list_replaces_it() {
        assert_eq!(
            parse("").files.exclude,
            ["node_modules", "target", "__pycache__"]
        );
        assert_eq!(
            parse("[files]\nexclude = [\"vendor\", \"dist\"]")
                .files
                .exclude,
            ["vendor", "dist"]
        );
        assert!(parse("[files]\nexclude = []").files.exclude.is_empty());
    }

    #[test]
    fn a_locale_is_read_and_a_blank_one_is_absent() {
        assert_eq!(parse("[ui]\nlocale = \"zh_cn\"").ui.locale, "zh_cn");
        // a blank key follows the session locale instead of pinning English
        assert_eq!(parse("[ui]\nlocale = \"  \"").ui, Ui::default());
    }

    #[test]
    fn the_shipped_template_is_all_comments_and_its_example_matches_the_default() {
        assert_eq!(parse(DEFAULT_TEMPLATE), Config::default());
        // the example a user uncomments must name what is compiled in
        assert!(
            DEFAULT_TEMPLATE.contains(r#"# exclude = ["node_modules", "target", "__pycache__"]"#)
        );
    }
}
