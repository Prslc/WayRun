use crate::geom::{Align, Layout};
use crate::theme;

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

/// One `[surfaces]` entry: `color` and `alpha` replace the derived field they
/// name, and either may be omitted to keep deriving it.
#[derive(serde::Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfaceOverride {
    #[serde(default, deserialize_with = "lenient_rgb")]
    pub color: Option<[u8; 3]>,
    #[serde(default)]
    pub alpha: Option<f32>,
}

impl SurfaceOverride {
    fn normalized(self) -> Self {
        Self {
            color: self.color,
            alpha: self.alpha.and_then(clamp01),
        }
    }
}

/// One surface table: each entry pins a surface the renderer would otherwise
/// derive from a role.
#[derive(serde::Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfaceOverrides {
    pub card: Option<SurfaceOverride>,
    pub selection: Option<SurfaceOverride>,
    pub hover: Option<SurfaceOverride>,
    pub preview: Option<SurfaceOverride>,
    pub hairline: Option<SurfaceOverride>,
    pub muted: Option<SurfaceOverride>,
    pub summary: Option<SurfaceOverride>,
    pub footer: Option<SurfaceOverride>,
    pub accent: Option<SurfaceOverride>,
    pub dim: Option<SurfaceOverride>,
}

impl SurfaceOverrides {
    fn normalized(self) -> Self {
        Self {
            card: self.card.map(SurfaceOverride::normalized),
            selection: self.selection.map(SurfaceOverride::normalized),
            hover: self.hover.map(SurfaceOverride::normalized),
            preview: self.preview.map(SurfaceOverride::normalized),
            hairline: self.hairline.map(SurfaceOverride::normalized),
            muted: self.muted.map(SurfaceOverride::normalized),
            summary: self.summary.map(SurfaceOverride::normalized),
            footer: self.footer.map(SurfaceOverride::normalized),
            accent: self.accent.map(SurfaceOverride::normalized),
            dim: self.dim.map(SurfaceOverride::normalized),
        }
    }
}

/// The `[surfaces]` root plus the `[surfaces.dark]`/`[surfaces.light]` tables
/// layered on top while that mode is active, surface by surface and field by field.
#[derive(serde::Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfaceConfig {
    #[serde(flatten)]
    pub shared: SurfaceOverrides,
    #[serde(default)]
    pub dark: SurfaceOverrides,
    #[serde(default)]
    pub light: SurfaceOverrides,
}

impl SurfaceConfig {
    /// The shared table with `mode`'s table on top: `color` and `alpha` layer
    /// separately, so a mode may change one and keep the other.
    pub fn for_mode(&self, mode: Mode) -> SurfaceOverrides {
        let mode = match mode {
            Mode::Dark => &self.dark,
            Mode::Light => &self.light,
        };
        SurfaceOverrides {
            card: layer(self.shared.card, mode.card),
            selection: layer(self.shared.selection, mode.selection),
            hover: layer(self.shared.hover, mode.hover),
            preview: layer(self.shared.preview, mode.preview),
            hairline: layer(self.shared.hairline, mode.hairline),
            muted: layer(self.shared.muted, mode.muted),
            summary: layer(self.shared.summary, mode.summary),
            footer: layer(self.shared.footer, mode.footer),
            accent: layer(self.shared.accent, mode.accent),
            dim: layer(self.shared.dim, mode.dim),
        }
    }

    fn normalized(self) -> Self {
        Self {
            shared: self.shared.normalized(),
            dark: self.dark.normalized(),
            light: self.light.normalized(),
        }
    }
}

fn layer(
    shared: Option<SurfaceOverride>,
    mode: Option<SurfaceOverride>,
) -> Option<SurfaceOverride> {
    match (shared, mode) {
        (Some(shared), Some(mode)) => Some(SurfaceOverride {
            color: mode.color.or(shared.color),
            alpha: mode.alpha.or(shared.alpha),
        }),
        (shared, mode) => shared.or(mode),
    }
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
    pub surfaces: SurfaceConfig,
    pub blur: bool,
    pub font: FontConfig,
    pub layout: Layout,
    pub preview: bool,
    pub reduced: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            colors: ColorConfig::default(),
            surfaces: SurfaceConfig::default(),
            blur: true,
            font: FontConfig::default(),
            layout: Layout::default(),
            preview: true,
            reduced: false,
        }
    }
}

#[derive(serde::Deserialize, Default)]
struct ThemeFile {
    #[serde(default)]
    colors: ColorConfig,
    #[serde(default)]
    surfaces: SurfaceConfig,
    #[serde(default)]
    blur: BlurFile,
    #[serde(default)]
    layout: LayoutFile,
    #[serde(default)]
    font: FontFile,
    #[serde(default)]
    motion: MotionFile,
    #[serde(default)]
    preview: PreviewFile,
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
    panel_header: Option<f32>,
}

#[derive(serde::Deserialize, Default)]
struct FontFile {
    size: Option<f32>,
}

#[derive(serde::Deserialize, Default)]
struct MotionFile {
    reduced: Option<bool>,
}

#[derive(serde::Deserialize, Default)]
struct PreviewFile {
    enabled: Option<bool>,
}

impl AppearanceConfig {
    /// The file's contents as a config, or `None` when unparseable: a reload
    /// keeps the applied config instead of resetting to defaults.
    pub fn parse(text: &str) -> Option<Self> {
        let file = toml::from_str::<ThemeFile>(text).ok()?;
        let mut config = Self::default();
        config.apply(file);
        Some(config)
    }

    fn apply(&mut self, file: ThemeFile) {
        let ThemeFile {
            colors,
            surfaces,
            blur,
            layout,
            font,
            motion,
            preview,
        } = file;

        self.colors = colors;
        self.surfaces = surfaces.normalized();

        assign!(self.blur, blur.enabled);
        assign!(self.preview, preview.enabled);
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
        assign!(
            self.layout.panel_header_h,
            layout
                .panel_header
                .filter(|v| v.is_finite())
                .map(|v| v.max(0.0))
        );

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

            [preview]
            enabled = false

            [layout]
            radius = 20.0
            width = 800.0
            max_rows = 3
            panel_header = 34.0

            [motion]
            reduced = true
            "##,
        );
        assert_eq!(config.colors.shared.primary, Some([0x7a, 0xa2, 0xf7]));
        assert_eq!(config.colors.shared.fg, None);
        assert!(!config.blur);
        assert!(!config.preview);
        assert_eq!(config.layout.radius, 20.0);
        assert_eq!(config.layout.width, 800.0);
        assert_eq!(config.layout.max_rows, 3);
        assert_eq!(config.layout.panel_header_h, 34.0);
        assert!(config.reduced);
        // untouched defaults survive
        assert_eq!(config.font.size, 14.0);
        assert_eq!(config.layout.top_ratio, 0.20);
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
            panel_header = -5.0
            "#,
        );
        assert_eq!(config.font.size, 96.0);
        assert_eq!(config.layout.max_rows, 8);
        assert_eq!(config.layout.panel_header_h, 0.0);
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
    fn surface_overrides_set_colour_and_alpha_independently() {
        let config = parse(
            r##"
            [surfaces]
            card = { alpha = 0.9 }
            hairline = { color = "#112233" }
            selection = { color = "#445566", alpha = 0.2 }
            muted = { color = "nonsense", alpha = 2.0 }
            "##,
        );
        let card = config.surfaces.shared.card.unwrap();
        assert_eq!((card.color, card.alpha), (None, Some(0.9)));
        let hairline = config.surfaces.shared.hairline.unwrap();
        assert_eq!(
            (hairline.color, hairline.alpha),
            (Some([0x11, 0x22, 0x33]), None)
        );
        let selection = config.surfaces.shared.selection.unwrap();
        assert_eq!(
            (selection.color, selection.alpha),
            (Some([0x44, 0x55, 0x66]), Some(0.2))
        );
        // a bad colour is dropped and a finite alpha is clamped
        let muted = config.surfaces.shared.muted.unwrap();
        assert_eq!((muted.color, muted.alpha), (None, Some(1.0)));
    }

    #[test]
    fn a_mode_table_layers_surface_fields() {
        let config = parse(
            r##"
            [surfaces]
            card = { color = "#112233", alpha = 0.8 }
            selection = { alpha = 0.2 }

            [surfaces.dark]
            card = { alpha = 0.9 }

            [surfaces.light]
            selection = { color = "#445566" }
            "##,
        );

        let dark = config.surfaces.for_mode(Mode::Dark);
        let card = dark.card.unwrap();
        assert_eq!(
            (card.color, card.alpha),
            (Some([0x11, 0x22, 0x33]), Some(0.9))
        );
        // a surface the dark table does not name keeps the shared entry
        assert_eq!(dark.selection.unwrap().alpha, Some(0.2));

        let light = config.surfaces.for_mode(Mode::Light);
        let selection = light.selection.unwrap();
        assert_eq!(
            (selection.color, selection.alpha),
            (Some([0x44, 0x55, 0x66]), Some(0.2))
        );
        assert_eq!(light.card.unwrap().alpha, Some(0.8));
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
        assert!(AppearanceConfig::parse("[colors]\nprimary = 5").is_none());
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
}
