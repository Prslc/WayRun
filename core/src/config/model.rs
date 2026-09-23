use super::path;
use serde::Deserialize;

/// Core behaviour loaded from `~/.config/wayrun/config.toml`. Every field
/// defaults to the compiled-in constant, so an absent file changes nothing.
///
/// These types are also the file's: one definition of every key serves the
/// file and the runtime both, with each group's rule stated on its field.
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
    /// How many levels the search descends from each root when it is not indexed.
    #[serde(deserialize_with = "lenient_depth")]
    pub depth: usize,
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

/// The walk depth, clamped to its documented range.
fn lenient_depth<'de, D: serde::Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    Ok(usize::deserialize(d)?.clamp(MIN_DEPTH, MAX_DEPTH))
}

impl Default for Files {
    fn default() -> Self {
        Self {
            index: true,
            depth: 3,
        }
    }
}

/// Bounds for `[files] depth`: 0 would search nothing but a root itself, and a
/// deep miss walks the whole tree, which is what the index is for.
const MIN_DEPTH: usize = 1;
const MAX_DEPTH: usize = 16;

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
    pub(super) fn load_checked() -> Option<Self> {
        let path = path()?;
        let text = std::fs::read_to_string(path).ok()?;
        toml::from_str(&text).ok()
    }

    pub(super) fn load() -> Self {
        Self::load_checked().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Config {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn a_key_of_the_wrong_type_rejects_the_file() {
        assert!(toml::from_str::<Config>("[files]\ndepth = \"deep\"").is_err());
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
        // an absent section keeps its default
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
    fn the_search_depth_is_bounded_to_its_documented_range() {
        assert_eq!(parse("").files.depth, 3, "the shipped depth is unchanged");
        assert_eq!(parse("[files]\ndepth = 1").files.depth, 1);
        assert_eq!(parse("[files]\ndepth = 16").files.depth, 16);
        assert_eq!(parse("[files]\ndepth = 0").files.depth, 1);
        assert_eq!(parse("[files]\ndepth = 999").files.depth, 16);
    }

    #[test]
    fn a_locale_is_read_and_a_blank_one_is_absent() {
        assert_eq!(parse("[ui]\nlocale = \"zh_cn\"").ui.locale, "zh_cn");
        // a blank key follows the session locale instead of pinning English
        assert_eq!(parse("[ui]\nlocale = \"  \"").ui, Ui::default());
    }

    #[test]
    fn the_shipped_template_comments_out_every_key() {
        for line in super::super::DEFAULT_TEMPLATE.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            assert!(line.starts_with('['), "uncommented key: {line}");
        }

        // commented keys are absent, so the whole file is the built-in defaults
        assert_eq!(parse(super::super::DEFAULT_TEMPLATE), Config::default());
    }
}
