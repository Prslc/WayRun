use crate::ui::geom::{Align, Layout};
use crate::ui::theme;

/// Assign the parsed key when present; an absent key keeps the current value.
macro_rules! assign {
    ($slot:expr, $value:expr) => {
        if let Some(value) = $value {
            $slot = value;
        }
    };
}

/// The base roles are RGB; every surface takes a colour with inline alpha, and
/// the type is also the `[colors]` table, a bad colour dropped like an absent key.
#[derive(serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorOverrides {
    #[serde(default, deserialize_with = "lenient_rgb")]
    pub primary: Option<[u8; 3]>,
    #[serde(default, deserialize_with = "lenient_rgb")]
    pub fg: Option<[u8; 3]>,
    #[serde(default, deserialize_with = "lenient_rgb")]
    pub container: Option<[u8; 3]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub card: Option<[u8; 4]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub field: Option<[u8; 4]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub selection: Option<[u8; 4]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub hover: Option<[u8; 4]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub hairline: Option<[u8; 4]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub muted: Option<[u8; 4]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub summary: Option<[u8; 4]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub footer: Option<[u8; 4]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub accent: Option<[u8; 4]>,
    #[serde(default, deserialize_with = "lenient_rgba")]
    pub dim: Option<[u8; 4]>,
    /// Ignore every override in this section and follow the system palette.
    #[serde(default)]
    pub follow_system: bool,
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
            card: mode.card.or(self.shared.card),
            field: mode.field.or(self.shared.field),
            selection: mode.selection.or(self.shared.selection),
            hover: mode.hover.or(self.shared.hover),
            hairline: mode.hairline.or(self.shared.hairline),
            muted: mode.muted.or(self.shared.muted),
            summary: mode.summary.or(self.shared.summary),
            footer: mode.footer.or(self.shared.footer),
            accent: mode.accent.or(self.shared.accent),
            dim: mode.dim.or(self.shared.dim),
            follow_system: self.shared.follow_system || mode.follow_system,
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

    pub fn badge(&self) -> f32 {
        self.size * 15.0 / 14.0
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
    pub entrance_ms: u64,
    pub reflow_ms: u64,
    pub reduced: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            colors: ColorConfig::default(),
            blur: true,
            font: FontConfig::default(),
            layout: Layout::default(),
            entrance_ms: 240,
            reflow_ms: 150,
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
    field_radius: Option<f32>,
    row_radius: Option<f32>,
    chip_radius: Option<f32>,
    width_ratio: Option<f32>,
    width_min: Option<f32>,
    width_max: Option<f32>,
    top_ratio: Option<f32>,
    align: Option<String>,
    offset_x: Option<f32>,
    offset_y: Option<f32>,
    hairline_width: Option<f32>,
    accent_width: Option<f32>,
    accent_height: Option<f32>,
    max_rows: Option<usize>,
}

#[derive(serde::Deserialize, Default)]
struct FontFile {
    size: Option<f32>,
}

#[derive(serde::Deserialize, Default)]
struct MotionFile {
    entrance_ms: Option<u64>,
    reflow_ms: Option<u64>,
    reduced: Option<bool>,
}

impl AppearanceConfig {
    pub fn load() -> Self {
        Self::load_checked().unwrap_or_default()
    }

    /// The file's config, or `None` when it is absent or unparseable; a reload
    /// keeps the applied config instead of resetting to defaults.
    pub fn load_checked() -> Option<Self> {
        super::ensure_template();
        let path = super::theme_path()?;
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
        self.layout.field_radius = non_negative(layout.field_radius);
        self.layout.row_radius = non_negative(layout.row_radius);
        self.layout.chip_radius = non_negative(layout.chip_radius);
        assign!(
            self.layout.width_ratio,
            layout.width_ratio.filter(|v| v.is_finite() && *v > 0.0)
        );
        assign!(
            self.layout.width_min,
            layout.width_min.filter(|v| v.is_finite() && *v > 0.0)
        );
        assign!(
            self.layout.width_max,
            layout.width_max.filter(|v| v.is_finite() && *v > 0.0)
        );
        assign!(self.layout.top_ratio, layout.top_ratio.and_then(clamp01));
        assign!(
            self.layout.align,
            layout.align.as_deref().and_then(parse_align)
        );
        assign!(self.layout.offset_x, finite(layout.offset_x));
        assign!(self.layout.offset_y, finite(layout.offset_y));
        assign!(
            self.layout.hairline_width,
            finite(layout.hairline_width).map(|v| v.clamp(0.0, 8.0))
        );
        assign!(
            self.layout.accent_width,
            finite(layout.accent_width).map(|v| v.clamp(0.0, 40.0))
        );
        assign!(
            self.layout.accent_height,
            finite(layout.accent_height).map(|v| v.clamp(0.0, 200.0))
        );
        assign!(self.layout.max_rows, layout.max_rows.map(|v| v.clamp(1, 8)));

        assign!(self.entrance_ms, motion.entrance_ms.filter(|v| *v > 0));
        assign!(self.reflow_ms, motion.reflow_ms.filter(|v| *v > 0));
        assign!(self.reduced, motion.reduced);

        if self.layout.width_min > self.layout.width_max {
            self.layout.width_min = self.layout.width_max;
        }
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

fn finite(v: Option<f32>) -> Option<f32> {
    v.filter(|v| v.is_finite())
}

/// A finite, non-negative value, else `None` so the base is kept.
fn non_negative(v: Option<f32>) -> Option<f32> {
    v.filter(|v| v.is_finite()).map(|v| v.max(0.0))
}

/// A finite, positive value clamped into `(0, max]`, else `None`.
fn positive(v: f32, max: f32) -> Option<f32> {
    (v.is_finite() && v > 0.0).then(|| v.clamp(1.0, max))
}

fn clamp01(v: f32) -> Option<f32> {
    v.is_finite().then(|| v.clamp(0.0, 1.0))
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
            max_rows = 3

            [motion]
            reduced = true
            "##,
        );
        assert_eq!(config.colors.shared.primary, Some([0x7a, 0xa2, 0xf7]));
        assert_eq!(config.colors.shared.fg, None);
        assert!(!config.blur);
        assert_eq!(config.layout.radius, 20.0);
        assert_eq!(config.layout.max_rows, 3);
        assert!(config.reduced);
        // untouched defaults survive
        assert_eq!(config.font.size, 14.0);
        assert_eq!(config.layout.width_ratio, 0.38);
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

            [motion]
            entrance_ms = 0
            "#,
        );
        assert_eq!(config.font.size, 96.0);
        assert_eq!(config.layout.max_rows, 8);
        assert_eq!(config.entrance_ms, 240);
    }

    #[test]
    fn a_surface_colour_carries_its_own_alpha() {
        let config = parse(
            r##"
            [colors]
            card = "#11223380"
            muted = "#445566"
            dim = "#000000"
            accent = "nonsense"
            follow_system = false
            "##,
        );
        assert_eq!(config.colors.shared.card, Some([0x11, 0x22, 0x33, 0x80]));
        assert_eq!(config.colors.shared.muted, Some([0x44, 0x55, 0x66, 255]));
        assert_eq!(config.colors.shared.dim, Some([0, 0, 0, 255]));
        assert_eq!(config.colors.shared.accent, None);
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
            card = "#24283b80"

            [colors.dark]
            fg = "#111111"
            muted = "#222222"

            [colors.light]
            fg = "#eeeeee"
            "##,
        );

        let dark = config.colors.for_mode(Mode::Dark);
        assert_eq!(dark.primary, Some([0x7a, 0xa2, 0xf7]));
        assert_eq!(dark.fg, Some([0x11, 0x11, 0x11]));
        assert_eq!(dark.muted, Some([0x22, 0x22, 0x22, 255]));
        // the shared surface survives both modes
        assert_eq!(dark.card, Some([0x24, 0x28, 0x3b, 0x80]));

        let light = config.colors.for_mode(Mode::Light);
        assert_eq!(light.fg, Some([0xee, 0xee, 0xee]));
        assert_eq!(light.primary, Some([0x7a, 0xa2, 0xf7]));
        // a key the light table does not set keeps the shared value
        assert_eq!(light.muted, None);
    }

    #[test]
    fn follow_system_from_either_level_wins() {
        let shared = parse("[colors]\nfollow_system = true\n[colors.dark]\nfg = \"#111111\"");
        assert!(shared.colors.for_mode(Mode::Dark).follow_system);
        // a mode flag follows through too, and the other level stays quiet
        let mode = parse("[colors.light]\nfollow_system = true");
        assert!(mode.colors.for_mode(Mode::Light).follow_system);
        assert!(!mode.colors.for_mode(Mode::Dark).follow_system);
    }

    #[test]
    fn the_core_mode_names_map_to_a_table() {
        assert_eq!(Mode::from_wire(Some("light")), Mode::Light);
        assert_eq!(Mode::from_wire(Some("dark")), Mode::Dark);
        assert_eq!(Mode::from_wire(None), Mode::Dark);
        assert_eq!(Mode::from_wire(Some("sepia")), Mode::Dark);
    }

    #[test]
    fn follow_system_is_a_plain_flag() {
        assert!(
            parse("[colors]\nfollow_system = true")
                .colors
                .shared
                .follow_system
        );
    }

    #[test]
    fn one_size_scales_every_role_by_its_shipped_ratio() {
        let config = parse("[font]\nsize = 28.0");
        assert_eq!(config.font.query(), 36.0);
        assert_eq!(config.font.title(), 28.0);
        assert_eq!(config.font.summary(), 24.0);
        assert_eq!(config.font.suggestion(), 22.0);
        assert_eq!(config.font.icon(), 60.0);
        assert_eq!(config.font.badge(), 30.0);

        // zero and negative keep the default
        assert_eq!(parse("[font]\nsize = -3.0").font.size, 14.0);
        assert_eq!(parse("[font]\nsize = 0.0").font.size, 14.0);
    }

    #[test]
    fn align_offsets_and_stroke_sizes_apply() {
        let config = parse(
            r#"
            [layout]
            align = "left"
            offset_x = 12.0
            offset_y = -8.0
            field_radius = 4.0
            row_radius = 3.0
            chip_radius = 2.0
            hairline_width = 2.0
            accent_width = 5.0
            accent_height = 20.0
            "#,
        );
        assert_eq!(config.layout.align, Align::Left);
        assert_eq!(config.layout.offset_x, 12.0);
        assert_eq!(config.layout.offset_y, -8.0);
        assert_eq!(config.layout.field_radius, Some(4.0));
        assert_eq!(config.layout.row_radius, Some(3.0));
        assert_eq!(config.layout.chip_radius, Some(2.0));
        assert_eq!(config.layout.hairline_width, 2.0);
        assert_eq!(config.layout.accent_width, 5.0);
        assert_eq!(config.layout.accent_height, 20.0);

        let unknown = parse("[layout]\nalign = \"diagonal\"");
        assert_eq!(unknown.layout.align, Align::Center);
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
        let mut config = AppearanceConfig::default();
        config.apply(toml::from_str(super::super::DEFAULT_TEMPLATE).unwrap());
        assert_eq!(config, AppearanceConfig::default());
    }
}
