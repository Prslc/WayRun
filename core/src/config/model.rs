use super::path;

/// Core behaviour loaded from `~/.config/wayrun/config.toml`. Every field
/// defaults to the compiled-in constant, so an absent file changes nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub web_search: WebSearch,
    pub font: Font,
    pub icon: Icon,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WebSearch {
    /// `google` or `duckduckgo`; an unknown value falls back to google.
    pub engine: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    /// The family the UI shapes with; missing glyphs fall back to the system, so
    /// a family covering only the scripts you read keeps the rest out of memory.
    pub family: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Icon {
    /// Icon theme name; empty follows the desktop's own setting.
    pub theme: String,
}

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

#[derive(serde::Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    web_search: WebSearchFile,
    #[serde(default)]
    font: FontFile,
    #[serde(default)]
    icon: IconFile,
}

#[derive(serde::Deserialize, Default)]
struct WebSearchFile {
    engine: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct FontFile {
    family: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct IconFile {
    theme: Option<String>,
}

impl Config {
    /// The file's config, or `None` when it is absent or unparseable. A watcher
    /// reload keeps what is applied rather than resetting to defaults.
    pub(super) fn load_checked() -> Option<Self> {
        let path = path()?;
        let text = std::fs::read_to_string(path).ok()?;
        let file = toml::from_str::<ConfigFile>(&text).ok()?;
        let mut config = Self::default();
        config.apply(file);
        Some(config)
    }

    pub(super) fn load() -> Self {
        Self::load_checked().unwrap_or_default()
    }

    fn apply(&mut self, file: ConfigFile) {
        if let Some(engine) = file.web_search.engine {
            self.web_search.engine = engine;
        }
        if let Some(family) = file.font.family.filter(|f| !f.trim().is_empty()) {
            self.font.family = family;
        }
        if let Some(theme) = file.icon.theme.filter(|t| !t.trim().is_empty()) {
            self.icon.theme = theme;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Config {
        let mut config = Config::default();
        config.apply(toml::from_str(src).unwrap());
        config
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
