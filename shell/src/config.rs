use std::path::PathBuf;
use std::sync::OnceLock;

use wayrun_core::{watch as watch_file, write_if_absent};

use crate::ui::geom::{Align, Layout};
use crate::ui::theme;

const DEFAULT_TEMPLATE: &str = include_str!("../default-theme.toml");

/// Assign the parsed key when present; an absent key keeps the current value.
macro_rules! assign {
    ($slot:expr, $value:expr) => {
        if let Some(value) = $value {
            $slot = value;
        }
    };
}

/// This is also the `[colors]` table itself: the base roles are `#rrggbb` and
/// `dim` takes inline alpha, a bad colour dropped like an absent key.
#[derive(serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorOverrides {
    #[serde(default, deserialize_with = "lenient_rgb")]
    pub primary: Option<[u8; 3]>,
    #[serde(default, deserialize_with = "lenient_rgb")]
    pub fg: Option<[u8; 3]>,
    #[serde(default, deserialize_with = "lenient_rgb")]
    pub container: Option<[u8; 3]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub dim: Option<[u8; 4]>,
}

/// A hand-written `#rrggbb` role.
fn lenient_rgb<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<[u8; 3]>, D::Error> {
    lenient(d, theme::parse_hex)
}

/// A hand-written surface colour, alpha included.
fn lenient_rgba<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<[u8; 4]>, D::Error> {
    lenient(d, theme::parse_color)
}

fn lenient<'de, D: serde::Deserializer<'de>, T>(
    d: D,
    parse: fn(&str) -> Option<T>,
) -> Result<Option<T>, D::Error> {
    let text = <Option<String> as serde::Deserialize>::deserialize(d)?;
    Ok(text.as_deref().and_then(parse))
}

/// Which system palette is active; selects the matching colour table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

impl Mode {
    /// The core sends `"light"` or `"dark"`; anything else is treated as dark.
    pub fn from_wire(mode: Option<&str>) -> Self {
        match mode {
            Some("light") => Self::Light,
            _ => Self::Dark,
        }
    }
}

/// The `[colors]` root plus the `[colors.dark]`/`[colors.light]` tables layered
/// on top while that mode is active.
#[derive(serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorConfig {
    #[serde(flatten)]
    pub shared: ColorOverrides,
    #[serde(default)]
    pub dark: ColorOverrides,
    #[serde(default)]
    pub light: ColorOverrides,
}

impl ColorConfig {
    /// The shared keys with `mode`'s table on top, field by field: a mode states
    /// only what it changes, and everything else inherits the shared value.
    pub fn for_mode(&self, mode: Mode) -> ColorOverrides {
        let mode = match mode {
            Mode::Dark => &self.dark,
            Mode::Light => &self.light,
        };
        ColorOverrides {
            primary: mode.primary.or(self.shared.primary),
            fg: mode.fg.or(self.shared.fg),
            container: mode.container.or(self.shared.container),
            dim: mode.dim.or(self.shared.dim),
        }
    }
}

/// One interface size; every text and icon role keeps its shipped ratio to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontConfig {
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self { size: 14.0 }
    }
}

impl FontConfig {
    pub fn query(&self) -> f32 {
        self.size * 18.0 / 14.0
    }

    pub fn title(&self) -> f32 {
        self.size
    }

    pub fn summary(&self) -> f32 {
        self.size * 12.0 / 14.0
    }

    pub fn suggestion(&self) -> f32 {
        self.size * 11.0 / 14.0
    }

    pub fn icon(&self) -> f32 {
        self.size * 30.0 / 14.0
    }
}

/// The shell's appearance, loaded from `theme.toml`. Every default equals the
/// renderer's constant, so an unset field changes nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct AppearanceConfig {
    pub colors: ColorConfig,
    pub blur: bool,
    pub font: FontConfig,
    pub layout: Layout,
    pub reduced: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            colors: ColorConfig::default(),
            blur: true,
            font: FontConfig::default(),
            layout: Layout::default(),
            reduced: false,
        }
    }
}

#[derive(serde::Deserialize, Default)]
struct ThemeFile {
    #[serde(default)]
    colors: ColorConfig,
    #[serde(default)]
    blur: BlurFile,
    #[serde(default)]
    layout: LayoutFile,
    #[serde(default)]
    font: FontFile,
    #[serde(default)]
    motion: MotionFile,
}

#[derive(serde::Deserialize, Default)]
struct BlurFile {
    enabled: Option<bool>,
}

#[derive(serde::Deserialize, Default)]
struct LayoutFile {
    radius: Option<f32>,
    width: Option<f32>,
    top_ratio: Option<f32>,
    align: Option<String>,
    max_rows: Option<usize>,
}

#[derive(serde::Deserialize, Default)]
struct FontFile {
    size: Option<f32>,
}

#[derive(serde::Deserialize, Default)]
struct MotionFile {
    reduced: Option<bool>,
}

impl AppearanceConfig {
    pub fn load() -> Self {
        Self::load_checked().unwrap_or_default()
    }

    /// The file's config, or `None` when it is absent or unparseable; a reload
    /// keeps the applied config instead of resetting to defaults.
    pub fn load_checked() -> Option<Self> {
        ensure_template();
        let path = theme_path()?;
        let text = std::fs::read_to_string(path).ok()?;
        let file = toml::from_str::<ThemeFile>(&text).ok()?;
        let mut config = Self::default();
        config.apply(file);
        Some(config)
    }

    fn apply(&mut self, file: ThemeFile) {
        let ThemeFile {
            colors,
            blur,
            layout,
            font,
            motion,
        } = file;

        self.colors = colors;

        assign!(self.blur, blur.enabled);
        assign!(self.font.size, font.size.and_then(|v| positive(v, 96.0)));

        assign!(
            self.layout.radius,
            layout.radius.filter(|v| v.is_finite()).map(|v| v.max(0.0))
        );
        assign!(
            self.layout.width,
            layout.width.filter(|v| v.is_finite() && *v > 0.0)
        );
        assign!(self.layout.top_ratio, layout.top_ratio.and_then(clamp01));
        assign!(
            self.layout.align,
            layout.align.as_deref().and_then(parse_align)
        );
        assign!(self.layout.max_rows, layout.max_rows.map(|v| v.clamp(1, 8)));

        assign!(self.reduced, motion.reduced);
    }
}

fn parse_align(value: &str) -> Option<Align> {
    match value {
        "left" => Some(Align::Left),
        "center" => Some(Align::Center),
        "right" => Some(Align::Right),
        _ => None,
    }
}

/// A finite, positive value clamped into `(0, max]`, else `None`.
fn positive(v: f32, max: f32) -> Option<f32> {
    (v.is_finite() && v > 0.0).then(|| v.clamp(1.0, max))
}

fn clamp01(v: f32) -> Option<f32> {
    v.is_finite().then(|| v.clamp(0.0, 1.0))
}

/// `~/.config/wayrun/theme.toml`. A missing file (or any missing key) keeps the
/// default, so an absent config is the shipped look.
fn theme_path() -> Option<PathBuf> {
    Some(wayrun_core::config::dir()?.join("theme.toml"))
}

/// Write the shipped template once, on first use: a watcher reload must not
/// recreate the file an editor's atomic save briefly removed.
fn ensure_template() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        if let Some(path) = theme_path() {
            let _ = write_if_absent(&path, DEFAULT_TEMPLATE);
        }
    });
}

/// Watch `theme.toml`; a failed read keeps the applied config and an unchanged
/// value is dropped, so a save settles to one update with no debounce.
pub fn watch(tx: calloop::channel::Sender<AppearanceConfig>) -> Option<notify::RecommendedWatcher> {
    let path = theme_path()?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut last = AppearanceConfig::load();
    watch_file(&path, move || {
        let Some(config) = AppearanceConfig::load_checked() else {
            return;
        };
        if config == last {
            return;
        }
        last = config.clone();
        let _ = tx.send(config);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> AppearanceConfig {
        let mut config = AppearanceConfig::default();
        config.apply(toml::from_str(src).unwrap());
        config
    }

    #[test]
    fn absent_keys_keep_the_defaults() {
        assert_eq!(parse(""), AppearanceConfig::default());
    }

    #[test]
    fn present_keys_override_field_by_field() {
        let config = parse(
            r##"
            [colors]
            primary = "#7aa2f7"
            fg = "nonsense"

            [blur]
            enabled = false

            [layout]
            radius = 20.0
            width = 800.0
            max_rows = 3

            [motion]
            reduced = true
            "##,
        );
        assert_eq!(config.colors.shared.primary, Some([0x7a, 0xa2, 0xf7]));
        assert_eq!(config.colors.shared.fg, None);
        assert!(!config.blur);
        assert_eq!(config.layout.radius, 20.0);
        assert_eq!(config.layout.width, 800.0);
        assert_eq!(config.layout.max_rows, 3);
        assert!(config.reduced);
        // untouched defaults survive
        assert_eq!(config.font.size, 14.0);
        assert_eq!(config.layout.top_ratio, 0.28);
    }

    #[test]
    fn layout_numbers_accept_integers() {
        let config = parse("[layout]\nradius = 0\nmax_rows = 3");
        assert_eq!(config.layout.radius, 0.0);
        assert_eq!(config.layout.max_rows, 3);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let config = parse(
            r#"
            [font]
            size = 500.0

            [layout]
            max_rows = 99
            "#,
        );
        assert_eq!(config.font.size, 96.0);
        assert_eq!(config.layout.max_rows, 8);
    }

    #[test]
    fn an_inline_alpha_parses_on_dim() {
        let config = parse(
            r##"
            [colors]
            dim = "#00000080"
            primary = "#112233"
            "##,
        );
        assert_eq!(config.colors.shared.dim, Some([0, 0, 0, 0x80]));
        assert_eq!(config.colors.shared.primary, Some([0x11, 0x22, 0x33]));
    }

    #[test]
    fn dropped_keys_are_inert() {
        let config = parse(
            r##"
            [colors]
            card = "#11223380"
            follow_system = true

            [layout]
            hairline_width = 2.0
            offset_x = 12.0
            row_radius = 3.0
            width_ratio = 0.5

            [motion]
            entrance_ms = 100
            "##,
        );
        assert_eq!(config, AppearanceConfig::default());
    }

    #[test]
    fn a_colour_of_the_wrong_type_rejects_the_file() {
        // only an unparseable string is dropped key-by-key; a value of the
        // wrong type still fails the whole file, as with every other key
        assert!(toml::from_str::<ThemeFile>("[colors]\nprimary = 5").is_err());
    }

    #[test]
    fn a_mode_table_layers_over_the_shared_keys() {
        let config = parse(
            r##"
            [colors]
            primary = "#7aa2f7"
            fg = "#c0caf5"
            dim = "#00000080"

            [colors.dark]
            fg = "#111111"

            [colors.light]
            fg = "#eeeeee"
            container = "#222222"
            "##,
        );

        let dark = config.colors.for_mode(Mode::Dark);
        assert_eq!(dark.primary, Some([0x7a, 0xa2, 0xf7]));
        assert_eq!(dark.fg, Some([0x11, 0x11, 0x11]));
        // a key the dark table does not set keeps the shared value
        assert_eq!(dark.dim, Some([0, 0, 0, 0x80]));

        let light = config.colors.for_mode(Mode::Light);
        assert_eq!(light.fg, Some([0xee, 0xee, 0xee]));
        assert_eq!(light.container, Some([0x22, 0x22, 0x22]));
        assert_eq!(light.primary, Some([0x7a, 0xa2, 0xf7]));
    }

    #[test]
    fn the_core_mode_names_map_to_a_table() {
        assert_eq!(Mode::from_wire(Some("light")), Mode::Light);
        assert_eq!(Mode::from_wire(Some("dark")), Mode::Dark);
        assert_eq!(Mode::from_wire(None), Mode::Dark);
        assert_eq!(Mode::from_wire(Some("sepia")), Mode::Dark);
    }

    #[test]
    fn one_size_scales_every_role_by_its_shipped_ratio() {
        let config = parse("[font]\nsize = 28.0");
        assert_eq!(config.font.query(), 36.0);
        assert_eq!(config.font.title(), 28.0);
        assert_eq!(config.font.summary(), 24.0);
        assert_eq!(config.font.suggestion(), 22.0);
        assert_eq!(config.font.icon(), 60.0);

        // zero and negative keep the default
        assert_eq!(parse("[font]\nsize = -3.0").font.size, 14.0);
        assert_eq!(parse("[font]\nsize = 0.0").font.size, 14.0);
    }

    #[test]
    fn align_is_parsed_and_an_unknown_value_keeps_the_default() {
        let config = parse("[layout]\nalign = \"left\"");
        assert_eq!(config.layout.align, Align::Left);

        let unknown = parse("[layout]\nalign = \"diagonal\"");
        assert_eq!(unknown.layout.align, Align::Center);
    }

    #[test]
    fn the_shipped_template_comments_out_every_key() {
        for line in DEFAULT_TEMPLATE.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            assert!(line.starts_with('['), "uncommented key: {line}");
        }

        // commented keys are absent, so the whole file is the built-in defaults
        let mut config = AppearanceConfig::default();
        config.apply(toml::from_str(DEFAULT_TEMPLATE).unwrap());
        assert_eq!(config, AppearanceConfig::default());
    }
}
