mod appearance;
mod cursor;
mod edit;
mod menu;
mod row;
#[cfg(test)]
mod test_support;

use std::time::Instant;

use crate::config::{AppearanceConfig, Mode};
use crate::ui::theme::{Surfaces, Theme};
use wayrun_core::wire::{Action, ResultItem};

use self::appearance::ease_out_cubic;
use self::menu::PanelKey;

pub use self::cursor::{Cursor, Hover, whole_rows};
pub use self::menu::{Menu, effective_action, marked_entry};
pub use self::row::Launch;

/// Launch dismissals wait this long before the surface goes away.
pub const EXIT_DELAY_MS: u64 = 150;
/// The field's caret blink interval.
pub const CARET_BLINK_MS: u64 = 500;

pub struct State {
    pub query: String,
    /// Caret, as a byte index into `query`, always on a char boundary.
    pub caret: usize,
    /// The fixed end of a keyboard selection; `caret` is the moving end. A
    /// selection exists while the two differ (Ctrl+A, Shift+arrows).
    pub anchor: Option<usize>,
    /// The live IME preedit, drawn at the caret (never inserted into `query`).
    pub preedit: Option<String>,
    pub rows: Vec<ResultItem>,
    pub cursor: Cursor,
    /// The open action panel, if any. It takes over the list and the keyboard.
    pub menu: Option<Menu>,
    /// The panel's row and entry, held across the re-emit a panel action sends,
    /// so the panel comes back where the user left it instead of dropping.
    panel_resume: Option<(Action, PanelKey)>,
    /// The core's system theme (the DMS palette, or the fallback).
    pub system_theme: Theme,
    /// Which system palette is active, so `[colors.dark]`/`[colors.light]`
    /// pick the matching table.
    pub mode: Mode,
    /// The `theme.toml` appearance config.
    pub appearance: AppearanceConfig,
    /// The effective theme: `system_theme` with `appearance.colors` on top.
    pub theme: Theme,
    /// The per-surface colours derived from `theme` and the colour overrides.
    pub surfaces: Surfaces,
    /// The layer surface's logical size.
    pub surface: (u32, u32),
    /// The integer buffer scale `wl_surface` reports, used when the compositor
    /// offers no fractional scale; it is the ceiling of the exact ratio.
    pub scale: i32,
    /// The exact ratio from `wp_fractional_scale_v1`: the buffer is
    /// `surface × this` and a viewport maps it back, so `scale` is not used.
    pub fractional: Option<f32>,
    /// Whether the entrance animation has started on the first drawn frame; the
    /// layer surface configures a round trip after `shown`.
    pub entrance_started: bool,
    /// `WAYRUN_TIMING=1`: the per-frame draw marks and the frame-pacing logs.
    pub timing: bool,
    /// `WAYRUN_REDUCED_MOTION`: ORed with the config's `motion.reduced`.
    reduced_env: bool,
    pub reduce_motion: bool,
    /// What the pointer is over, if anything.
    pub hovered: Option<Hover>,
    /// The pointer's last position over the surface, so a moving list can
    /// re-derive the hover.
    pointer: Option<(f32, f32)>,
    pub pointer_on_card: bool,
    /// fcitx can deliver Enter while a preedit is live; Enter over composition
    /// must not launch a row.
    pub preedit_active: bool,
    pub caret_visible: bool,
    pub caret_at: Instant,
    shown_at: Instant,
    card_from: f32,
    card_to: f32,
    card_at: Instant,
    pub dismiss_at: Option<Instant>,
    /// The bottom edge of the card drawn last. A settle repaint covers only the
    /// card, so a shrink must restore the dim over the old edge too.
    pub last_card_bottom: f32,
}

impl State {
    pub fn new() -> Self {
        let now = Instant::now();
        let appearance = AppearanceConfig::default();
        let reduced_env = std::env::var_os("WAYRUN_REDUCED_MOTION").is_some();
        let timing = std::env::var_os("WAYRUN_TIMING").is_some();
        let reduce_motion = reduced_env || appearance.reduced;
        let theme = Theme::default();
        let mode = Mode::default();
        let colors = appearance.colors.for_mode(mode);
        let surfaces = Surfaces::resolve(theme, &colors);
        let card = appearance.layout.content_h(0);
        Self {
            query: String::new(),
            caret: 0,
            anchor: None,
            preedit: None,
            rows: Vec::new(),
            cursor: Cursor::default(),
            menu: None,
            panel_resume: None,
            system_theme: theme,
            mode,
            appearance,
            theme,
            surfaces,
            surface: (1920, 1080),
            scale: 1,
            fractional: None,
            entrance_started: false,
            timing,
            reduced_env,
            reduce_motion,
            hovered: None,
            pointer: None,
            pointer_on_card: false,
            preedit_active: false,
            caret_visible: true,
            caret_at: now,
            shown_at: now,
            card_from: card,
            card_to: card,
            card_at: now,
            dismiss_at: None,
            last_card_bottom: 0.0,
        }
    }

    /// The buffer scale to render at: the compositor's fractional ratio when
    /// there is one, else the integer scale.
    pub fn scale_factor(&self) -> f32 {
        self.fractional.unwrap_or(self.scale.max(1) as f32)
    }

    /// Whether the buffers are mapped through a `wp_viewport`: their size is
    /// `surface × scale_factor` and the surface's own buffer scale stays 1.
    pub fn uses_viewport(&self) -> bool {
        self.fractional.is_some()
    }

    /// The card's resting height (panel when open, else the list). The footer,
    /// blur region and reflow all read it, so they agree with the drawn frame.
    pub fn content_height(&self) -> f32 {
        match &self.menu {
            Some(menu) => self.appearance.layout.panel_h(menu.actions.len()),
            None => self.appearance.layout.content_h(self.rows.len()),
        }
    }

    pub fn card_height(&self, now: Instant) -> f32 {
        let to = self.card_to;
        if self.reduce_motion {
            return to;
        }

        let elapsed = now.saturating_duration_since(self.card_at).as_secs_f32() * 1000.0;
        let t = (elapsed / self.appearance.reflow_ms as f32).clamp(0.0, 1.0);
        self.card_from + (to - self.card_from) * ease_out_cubic(t)
    }

    pub fn shown(&mut self, now: Instant) {
        self.query.clear();
        self.caret = 0;
        self.anchor = None;
        self.preedit = None;
        self.preedit_active = false;
        self.cursor.selected = 0;
        self.menu = None;
        self.hovered = None;
        self.dismiss_at = None;
        self.shown_at = now;
        self.card_at = now;
        self.card_from = self.content_height();
        self.card_to = self.card_from;
        self.contain();
        self.caret_visible = true;
        self.caret_at = now;
        self.entrance_started = false;
    }

    pub fn hidden(&mut self) {
        self.dismiss_at = None;
        self.menu = None;
        // A reply that arrives while hidden must not resurrect the panel.
        self.panel_resume = None;
        // The surface is gone, so nothing is composing on it any more.
        self.preedit = None;
        self.preedit_active = false;
        // With a long history the payload is megabytes, and the rest of a
        // dismissal already frees what a hidden launcher holds.
        self.rows = Vec::new();
    }

    /// Keep the selection inside the `max_rows` window, minimally.
    pub fn contain(&mut self) {
        self.cursor.contain(self.rows.len(), self.appearance.layout);
        self.resync_hover();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fractional_scale_overrides_the_integer_one() {
        let mut state = State::new();
        // what niri reports for a 1.25x output
        state.scale = 2;
        state.surface = (1536, 864);
        assert_eq!(state.scale_factor(), 2.0);
        assert!(!state.uses_viewport());

        state.fractional = Some(1.25);
        assert_eq!(state.scale_factor(), 1.25);
        assert!(state.uses_viewport());
        // the same logical surface asks for 1920 physical pixels through the
        // fractional ratio and 3072 through the integer scale
        assert_eq!((1536.0 * state.scale_factor()).round() as u32, 1920);
        assert_eq!((1536.0 * 2.0) as u32, 3072);
    }
}
