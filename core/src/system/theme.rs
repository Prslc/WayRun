use crate::wire::ThemeConfig;
use std::env;
use std::fs;
use std::path::PathBuf;

/// The Material palette DankMaterialShell generated with matugen. It holds both
/// modes and a `mode` key, so a switch is a plain file change.
pub fn dms_colors_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("DankMaterialShell/dms-colors.json"))
}

/// KDE's merged colour configuration: the applied scheme's roles and the accent,
/// rewritten here on every scheme or wallpaper-accent change.
pub fn kde_globals_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("kdeglobals"))
}

/// Whether this process runs inside a KDE session, where `kdeglobals` is the
/// system palette and a DMS file may be left over from another desktop.
fn kde_session() -> bool {
    env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| {
        desktop
            .split(':')
            .any(|name| name.eq_ignore_ascii_case("kde"))
    }) || env::var("KDE_FULL_SESSION").is_ok_and(|full| full == "true")
}

/// The palette file the current session follows, watched for live changes.
pub fn system_theme_path() -> Option<PathBuf> {
    if kde_session() {
        kde_globals_path()
    } else {
        dms_colors_path()
    }
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
    let theme = if kde_session() {
        load_kde()
    } else {
        load_dms()
    };
    theme.unwrap_or_else(default_theme)
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

/// A KDE scheme has no mode key: the window background's luminance names the
/// mode, and the button background is the raised surface under the card.
fn load_kde() -> Option<ThemeConfig> {
    let text = fs::read_to_string(kde_globals_path()?).ok()?;
    from_kde(&text)
}

fn from_kde(text: &str) -> Option<ThemeConfig> {
    let background = color(text, "Colors:Window", "BackgroundNormal")?;
    let foreground = color(text, "Colors:Window", "ForegroundNormal")?;
    let container = color(text, "Colors:Button", "BackgroundNormal").unwrap_or(background);
    // `AccentColor` is the explicit or wallpaper-derived accent; without it the
    // scheme's own focus colour is the accent.
    let primary = color(text, "General", "AccentColor")
        .or_else(|| color(text, "Colors:Window", "DecorationFocus"))
        .or_else(|| color(text, "Colors:View", "DecorationFocus"))?;
    let on_primary =
        color(text, "Colors:Selection", "ForegroundNormal").unwrap_or_else(|| contrast(primary));
    let mode = if is_dark(background) { "dark" } else { "light" };

    Some(ThemeConfig {
        mode: Some(mode.into()),
        primary: Some(hex(primary)),
        on_primary: Some(hex(on_primary)),
        bg: Some(hex(background)),
        fg: Some(hex(foreground)),
        container: Some(hex(container)),
    })
}

/// The value of `key` in `section` of KDE's INI text; the first hit wins.
fn ini_value<'a>(text: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            in_section = name.trim() == section;
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if in_section && name.trim() == key {
            return Some(value.trim());
        }
    }
    None
}

/// A KDE colour is `r,g,b` with an optional trailing alpha, which is ignored.
fn color(text: &str, section: &str, key: &str) -> Option<[u8; 3]> {
    let mut parts = ini_value(text, section, key)?
        .split(',')
        .map(|part| part.trim().parse::<u8>().ok());
    Some([
        parts.next().flatten()?,
        parts.next().flatten()?,
        parts.next().flatten()?,
    ])
}

/// sRGB-weighted luminance against mid-grey.
fn is_dark([r, g, b]: [u8; 3]) -> bool {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) < 128.0
}

fn contrast(color: [u8; 3]) -> [u8; 3] {
    if is_dark(color) { [255; 3] } else { [0; 3] }
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
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

    fn sample_kde(window_bg: &str, button_bg: &str, accent: Option<&str>) -> String {
        let accent = accent
            .map(|accent| format!("AccentColor={accent}\n"))
            .unwrap_or_default();
        format!(
            r##"
[Colors:Button]
BackgroundNormal={button_bg}

[Colors:Selection]
ForegroundNormal=252,252,252

[Colors:View]
DecorationFocus=61,174,233

[Colors:Window]
BackgroundNormal={window_bg}
ForegroundNormal=252,252,252
DecorationFocus=61,174,233

[General]
{accent}"##
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

    #[test]
    fn the_window_background_names_the_kde_mode() {
        let dark = from_kde(&sample_kde("32,35,38", "41,44,48", None)).unwrap();
        assert_eq!(dark.mode.as_deref(), Some("dark"));
        assert_eq!(dark.bg.as_deref(), Some("#202326"));
        assert_eq!(dark.fg.as_deref(), Some("#fcfcfc"));
        assert_eq!(dark.container.as_deref(), Some("#292c30"));
        assert_eq!(dark.on_primary.as_deref(), Some("#fcfcfc"));

        let light = from_kde(&sample_kde("252,252,252", "245,245,245", None)).unwrap();
        assert_eq!(light.mode.as_deref(), Some("light"));
        assert_eq!(light.container.as_deref(), Some("#f5f5f5"));
    }

    #[test]
    fn an_explicit_accent_wins_over_the_schemes_focus_colour() {
        let fallback = from_kde(&sample_kde("32,35,38", "41,44,48", None)).unwrap();
        assert_eq!(fallback.primary.as_deref(), Some("#3daee9"));

        let explicit = from_kde(&sample_kde("32,35,38", "41,44,48", Some("255,0,0,128"))).unwrap();
        assert_eq!(explicit.primary.as_deref(), Some("#ff0000"));
    }

    #[test]
    fn a_role_kde_does_not_define_is_none() {
        assert!(from_kde("[Colors:Button]\nBackgroundNormal=1,2,3\n").is_none());
        assert!(from_kde("not ini").is_none());
    }

    #[test]
    fn comments_and_unknown_sections_do_not_leak_values() {
        let text = "\
[Colors:Window]
# BackgroundNormal=1,1,1
; ForegroundNormal=2,2,2

[Other]
BackgroundNormal=9,9,9
ForegroundNormal=9,9,9
";
        assert!(from_kde(text).is_none());
        assert_eq!(
            ini_value(
                "[General]\nAccentColor = 1, 2, 3 \n",
                "General",
                "AccentColor"
            ),
            Some("1, 2, 3")
        );
    }
}
