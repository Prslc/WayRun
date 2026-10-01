use crate::config::ColorOverrides;
use wayrun_core::wire::ThemeConfig;

/// The three base roles every surface derives from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub primary: [u8; 3],
    pub fg: [u8; 3],
    pub container: [u8; 3],
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            primary: [0x7a, 0xa2, 0xf7],
            fg: [0xc0, 0xca, 0xf5],
            container: [0x24, 0x28, 0x3b],
        }
    }
}

impl Theme {
    /// Field-by-field: a missing or unparseable entry keeps the fallback.
    pub fn from_config(config: &ThemeConfig) -> Self {
        let fallback = Self::default();

        Self {
            primary: config
                .primary
                .as_deref()
                .and_then(parse_hex)
                .unwrap_or(fallback.primary),
            fg: config
                .fg
                .as_deref()
                .and_then(parse_hex)
                .unwrap_or(fallback.fg),
            container: config
                .container
                .as_deref()
                .and_then(parse_hex)
                .unwrap_or(fallback.container),
        }
    }

    /// The system theme with `theme.toml`'s per-field overrides on top; a key
    /// the file does not set keeps tracking the system.
    pub fn overlay(system: Self, colors: &ColorOverrides) -> Self {
        Self {
            primary: colors.primary.unwrap_or(system.primary),
            fg: colors.fg.unwrap_or(system.fg),
            container: colors.container.unwrap_or(system.container),
        }
    }
}

const CARD_ALPHA: f32 = 0.72;
const SELECTION_ALPHA: f32 = 0.15;
const HOVER_ALPHA: f32 = 0.08;
const HAIRLINE_ALPHA: f32 = 0.35;
const MUTED_ALPHA: f32 = 0.55;
const SUMMARY_ALPHA: f32 = 0.7;
const FOOTER_ALPHA: f32 = 0.5;
const ACCENT_ALPHA: f32 = 1.0;

/// The backdrop dim's shipped colour and alpha, overridden by `[colors].dim`.
pub const DEFAULT_DIM: [u8; 4] = [0, 0, 0, alpha_u8(0.3)];

/// The per-surface colours the renderer draws: a base role at its shipped alpha
/// by default, or the `theme.toml` override with its own inline alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surfaces {
    pub card: [u8; 4],
    pub selection: [u8; 4],
    pub hover: [u8; 4],
    pub hairline: [u8; 4],
    pub muted: [u8; 4],
    pub summary: [u8; 4],
    pub footer: [u8; 4],
    pub accent: [u8; 4],
    pub dim: [u8; 4],
}

impl Surfaces {
    /// The surfaces derived from the theme, with `[colors].dim`'s own override
    /// on top; every other surface follows its role.
    pub fn resolve(theme: Theme, colors: &ColorOverrides) -> Self {
        let mut surfaces = Self::derive(theme);
        if let Some(color) = colors.dim {
            surfaces.dim = color;
        }
        surfaces
    }

    fn derive(theme: Theme) -> Self {
        Self {
            card: tint(theme.container, CARD_ALPHA),
            selection: tint(theme.primary, SELECTION_ALPHA),
            hover: tint(theme.primary, HOVER_ALPHA),
            hairline: [255, 255, 255, alpha_u8(HAIRLINE_ALPHA)],
            muted: tint(theme.fg, MUTED_ALPHA),
            summary: tint(theme.fg, SUMMARY_ALPHA),
            footer: tint(theme.fg, FOOTER_ALPHA),
            accent: tint(theme.primary, ACCENT_ALPHA),
            dim: DEFAULT_DIM,
        }
    }
}

fn tint(rgb: [u8; 3], alpha: f32) -> [u8; 4] {
    [rgb[0], rgb[1], rgb[2], alpha_u8(alpha)]
}

/// Rounds to the nearest 8-bit alpha; `round` is not const.
const fn alpha_u8(alpha: f32) -> u8 {
    (alpha * 255.0 + 0.5) as u8
}

/// `#rrggbb` (or `#rgb`), nothing else: the base roles are RGB, the surfaces in
/// `[colors]` take the alpha-carrying [`parse_color`] instead.
pub fn parse_hex(spec: &str) -> Option<[u8; 3]> {
    let hex = spec.trim().strip_prefix('#')?;
    if !hex.is_ascii() {
        return None;
    }

    let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    match hex.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?]),
        3 => {
            let one = |at: usize| {
                u8::from_str_radix(&hex[at..at + 1], 16)
                    .ok()
                    .map(|v| v * 17)
            };
            Some([one(0)?, one(1)?, one(2)?])
        }
        _ => None,
    }
}

/// `#rrggbb`, `#rgb`, `#rrggbbaa` or `#rgba`; a missing alpha is opaque.
pub fn parse_color(spec: &str) -> Option<[u8; 4]> {
    let hex = spec.trim().strip_prefix('#')?;
    if !hex.is_ascii() {
        return None;
    }

    let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    let nibble = |at: usize| {
        u8::from_str_radix(&hex[at..at + 1], 16)
            .ok()
            .map(|v| v * 17)
    };
    match hex.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        3 => Some([nibble(0)?, nibble(1)?, nibble(2)?, 255]),
        4 => Some([nibble(0)?, nibble(1)?, nibble(2)?, nibble(3)?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayrun_core::wire::ThemeConfig;

    #[test]
    fn parses_hex_and_falls_back() {
        let config = ThemeConfig {
            mode: None,
            primary: Some("#7aa2f7".into()),
            on_primary: None,
            bg: None,
            fg: Some("nonsense".into()),
            container: None,
        };
        let theme = Theme::from_config(&config);
        assert_eq!(theme.primary, [0x7a, 0xa2, 0xf7]);
        assert_eq!(theme.fg, Theme::default().fg);
        assert_eq!(theme.container, Theme::default().container);
    }

    #[test]
    fn non_ascii_values_do_not_panic() {
        assert!(parse_hex("主色").is_none());
        assert!(parse_hex("#主色").is_none());
        assert!(parse_hex("🦀🦀🦀").is_none());
        assert!(parse_hex("#abc").is_some());
    }

    #[test]
    fn a_colour_carries_its_alpha_but_a_role_does_not() {
        assert_eq!(parse_color("#112233").unwrap(), [0x11, 0x22, 0x33, 255]);
        assert_eq!(parse_color("#1234").unwrap(), [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(parse_color("#11223380").unwrap(), [0x11, 0x22, 0x33, 0x80]);
        // a base role is RGB-only, so an eight-digit spec is not a role colour
        assert!(parse_hex("#11223380").is_none());
        assert!(parse_color("nonsense").is_none());
    }

    #[test]
    fn overrides_win_field_by_field() {
        let system = Theme {
            primary: [1, 2, 3],
            fg: [4, 5, 6],
            container: [7, 8, 9],
        };
        let colors = ColorOverrides {
            primary: Some([0xaa, 0xbb, 0xcc]),
            container: Some([0x11, 0x22, 0x33]),
            ..ColorOverrides::default()
        };
        let theme = Theme::overlay(system, &colors);
        assert_eq!(theme.primary, [0xaa, 0xbb, 0xcc]);
        assert_eq!(theme.fg, [4, 5, 6]);
        assert_eq!(theme.container, [0x11, 0x22, 0x33]);
    }

    #[test]
    fn a_surface_derives_from_its_role_except_dim() {
        let theme = Theme::default();
        let derived = Surfaces::resolve(theme, &ColorOverrides::default());
        assert_eq!(derived.card, tint(theme.container, CARD_ALPHA));
        assert_eq!(derived.muted, tint(theme.fg, MUTED_ALPHA));
        assert_eq!(derived.accent, tint(theme.primary, ACCENT_ALPHA));
        assert_eq!(derived.dim, DEFAULT_DIM);

        let colors = ColorOverrides {
            dim: parse_color("#010203cc"),
            ..ColorOverrides::default()
        };
        let overridden = Surfaces::resolve(theme, &colors);
        assert_eq!(overridden.dim, [1, 2, 3, 0xcc]);
        // every other surface still derives
        assert_eq!(overridden.card, tint(theme.container, CARD_ALPHA));
    }
}
