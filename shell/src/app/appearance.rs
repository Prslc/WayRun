use std::time::{Duration, Instant};

use crate::config::{AppearanceConfig, Mode};
use crate::ui::theme::{Surfaces, Theme};

use super::State;

impl State {
    /// The `theme.toml` config changed. Recompute the theme and the derived
    /// geometry; the caller repaints the whole frame.
    pub fn apply_appearance(&mut self, config: AppearanceConfig, now: Instant) {
        self.reduce_motion = self.reduced_env || config.reduced;
        self.appearance = config;
        self.refresh_theme();
        self.card_from = self.content_height();
        self.card_to = self.card_from;
        self.card_at = now;
    }

    /// The core's system theme changed, with the mode whose palette it holds.
    pub fn set_system_theme(&mut self, system: Theme, mode: Mode) {
        self.system_theme = system;
        self.mode = mode;
        self.refresh_theme();
    }

    fn refresh_theme(&mut self) {
        let colors = self.appearance.colors.for_mode(self.mode);
        self.theme = Theme::overlay(self.system_theme, &colors);
        self.surfaces = Surfaces::resolve(self.theme, &colors);
    }

    /// The backdrop dim's alpha (0 → the surface's alpha), not a progress fraction.
    pub fn dim_alpha(&self, now: Instant) -> f32 {
        (self.surfaces.dim[3] as f32 / 255.0) * self.entrance(now)
    }

    /// The backdrop dim's colour, its alpha handled by [`Self::dim_alpha`].
    pub fn dim_color(&self) -> [u8; 3] {
        [
            self.surfaces.dim[0],
            self.surfaces.dim[1],
            self.surfaces.dim[2],
        ]
    }

    /// The muted text alpha, shared by the `fg`-tinted hints and the `primary`
    /// `⏎` markers, which take the same alpha over a different role.
    pub fn muted_alpha(&self) -> f32 {
        self.surfaces.muted[3] as f32 / 255.0
    }

    /// The card's fill alpha for the entrance's opacity fade.
    pub fn entrance(&self, now: Instant) -> f32 {
        if self.reduce_motion {
            // Reduced motion starts at the end state: returning 0 here leaves
            // the dim and every faded colour invisible, not "no animation".
            return 1.0;
        }
        if !self.entrance_started {
            return 0.0;
        }

        let elapsed = now.saturating_duration_since(self.shown_at).as_secs_f32() * 1000.0;
        ease_out_quint((elapsed / self.appearance.entrance_ms as f32).clamp(0.0, 1.0))
    }

    /// A card-content colour at `alpha`, scaled by the entrance fade: the whole
    /// card arrives as one unit and the first frame really is invisible.
    pub fn fade(&self, color: [u8; 3], alpha: f32, now: Instant) -> [u8; 4] {
        [
            color[0],
            color[1],
            color[2],
            ((alpha * self.entrance(now)).clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }

    /// A resolved surface colour, whose own inline alpha rides the entrance.
    pub fn fade_rgba(&self, color: [u8; 4], now: Instant) -> [u8; 4] {
        self.fade([color[0], color[1], color[2]], color[3] as f32 / 255.0, now)
    }

    pub fn animating(&self, now: Instant) -> bool {
        if self.reduce_motion || !self.entrance_started {
            return false;
        }

        let entrance = now.saturating_duration_since(self.shown_at)
            < Duration::from_millis(self.appearance.entrance_ms);
        let reflow = now.saturating_duration_since(self.card_at)
            < Duration::from_millis(self.appearance.reflow_ms);
        entrance || reflow
    }

    /// The first frame of a show: this is where the entrance fade begins.
    pub fn start_entrance(&mut self, now: Instant) {
        if self.entrance_started {
            return;
        }
        self.entrance_started = true;
        self.shown_at = now;
        self.card_at = now;
        self.card_from = self.content_height();
        self.card_to = self.card_from;
    }

    pub fn retarget_height(&mut self, now: Instant) {
        let target = self.content_height();
        if target != self.card_to {
            self.card_from = self.card_height(now);
            self.card_to = target;
            self.card_at = now;
        }
    }
}

pub(super) fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_out_quint(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::state;

    #[test]
    fn an_appearance_change_rebuilds_the_theme_and_geometry() {
        let mut state = state();
        let now = std::time::Instant::now();

        let mut config = AppearanceConfig::default();
        config.colors.shared.fg = Some([0x10, 0x20, 0x30]);
        config.layout.max_rows = 2;
        state.set_system_theme(
            Theme {
                primary: [1, 2, 3],
                fg: [4, 5, 6],
                container: [7, 8, 9],
            },
            Mode::Dark,
        );
        state.apply_appearance(config, now);

        assert_eq!(state.theme.primary, [1, 2, 3]);
        assert_eq!(state.theme.fg, [0x10, 0x20, 0x30]);
        assert_eq!(state.theme.container, [7, 8, 9]);
        assert_eq!(state.surfaces.dim, crate::ui::theme::DEFAULT_DIM);
        assert_eq!(state.card_to, state.appearance.layout.content_h(0));
    }

    #[test]
    fn the_env_reduced_motion_flag_survives_a_config_reload() {
        let mut state = state();
        state.reduced_env = true;
        state.reduce_motion = true;
        let config = AppearanceConfig {
            reduced: false,
            ..AppearanceConfig::default()
        };
        state.apply_appearance(config, std::time::Instant::now());
        assert!(state.reduce_motion);
    }
}
