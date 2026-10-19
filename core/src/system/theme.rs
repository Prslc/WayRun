use crate::wire::ThemeConfig;
use std::fs;
use std::path::PathBuf;

/// The Material palette DankMaterialShell generated with matugen. It holds both
/// modes and a `mode` key, so a switch is a plain file change.
pub fn dms_colors_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("DankMaterialShell/dms-colors.json"))
}

#[derive(serde::Deserialize)]
struct DmsColors {
    mode: String,
    colors: DmsModes,
}

#[derive(serde::Deserialize)]
struct DmsModes {
    light: DmsPalette,
    dark: DmsPalette,
}

/// Only the roles the shell draws with; unknown keys are ignored.
#[derive(serde::Deserialize)]
struct DmsPalette {
    primary: String,
    on_primary: String,
    background: String,
    on_surface: String,
    surface_container_high: String,
}

/// The built-in dark palette, used when no system theme is available.
pub fn default_theme() -> ThemeConfig {
    ThemeConfig {
        mode: Some("dark".into()),
        primary: Some("#ffb59f".into()),
        on_primary: Some("#561f0f".into()),
        bg: Some("#1a110f".into()),
        fg: Some("#f1dfda".into()),
        container: Some("#271d1b".into()),
    }
}

pub fn load_theme() -> ThemeConfig {
    load_dms().unwrap_or_else(default_theme)
}

/// The shell draws the container from `surface_container_high` (what the GTK
/// template fed `popover_bg_color`), so the card keeps its depth.
fn load_dms() -> Option<ThemeConfig> {
    let text = fs::read_to_string(dms_colors_path()?).ok()?;
    from_json(&text)
}

fn from_json(text: &str) -> Option<ThemeConfig> {
    let file: DmsColors = serde_json::from_str(text).ok()?;
    // anything that is not `dark` resolves to the light palette, and the mode
    // sent on the wire must agree with the palette it names.
    let (mode, palette) = match file.mode.as_str() {
        "dark" => ("dark", file.colors.dark),
        _ => ("light", file.colors.light),
    };
    Some(ThemeConfig {
        mode: Some(mode.into()),
        primary: Some(palette.primary),
        on_primary: Some(palette.on_primary),
        bg: Some(palette.background),
        fg: Some(palette.on_surface),
        container: Some(palette.surface_container_high),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(mode: &str) -> String {
        format!(
            r##"{{
                "mode": "{mode}",
                "colors": {{
                    "light": {{
                        "primary": "#35618e",
                        "on_primary": "#ffffff",
                        "background": "#f8f9ff",
                        "on_surface": "#191c20",
                        "surface_container_high": "#eceef4"
                    }},
                    "dark": {{
                        "primary": "#a0cafd",
                        "on_primary": "#003257",
                        "background": "#101418",
                        "on_surface": "#e1e2e8",
                        "surface_container_high": "#1d2024"
                    }}
                }}
            }}"##
        )
    }

    #[test]
    fn the_mode_key_selects_the_palette() {
        let light = from_json(&sample("light")).unwrap();
        assert_eq!(light.mode.as_deref(), Some("light"));
        assert_eq!(light.primary.as_deref(), Some("#35618e"));
        assert_eq!(light.bg.as_deref(), Some("#f8f9ff"));
        assert_eq!(light.fg.as_deref(), Some("#191c20"));
        assert_eq!(light.container.as_deref(), Some("#eceef4"));

        let dark = from_json(&sample("dark")).unwrap();
        assert_eq!(dark.mode.as_deref(), Some("dark"));
        assert_eq!(dark.primary.as_deref(), Some("#a0cafd"));
        assert_eq!(dark.bg.as_deref(), Some("#101418"));
        assert_eq!(dark.container.as_deref(), Some("#1d2024"));
    }

    #[test]
    fn an_unknown_mode_falls_back_to_light() {
        let theme = from_json(&sample("sepia")).unwrap();
        assert_eq!(theme.mode.as_deref(), Some("light"));
        assert_eq!(theme.bg.as_deref(), Some("#f8f9ff"));
    }

    #[test]
    fn a_missing_role_or_bad_json_is_none() {
        assert!(from_json("not json").is_none());
        assert!(from_json(r#"{"mode":"light","colors":{"light":{},"dark":{}}}"#).is_none());
    }
}
