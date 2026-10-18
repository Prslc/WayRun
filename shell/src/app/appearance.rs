use std::time::Instant;

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
        self.reset_height(now);
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
        self.surfaces = Surfaces::resolve(self.theme, &colors, self.mode);
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
        assert_eq!(state.card_target, state.appearance.layout.empty_h());
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
