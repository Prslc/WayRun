use std::time::Instant;

use wayrun_ui::config::{AppearanceConfig, Mode};
use wayrun_ui::theme::Theme;

use super::State;

impl State {
    /// The `theme.toml` config changed. Recompute the theme and the derived
    /// geometry; the caller repaints the whole frame.
    pub fn apply_appearance(&mut self, config: AppearanceConfig, now: Instant) {
        self.presentation.apply_appearance(config);
        self.presentation.snap_to(self.content_height(), now);
    }

    /// The core's system theme changed, with the mode whose palette it holds.
    pub fn set_system_theme(&mut self, system: Theme, mode: Mode) {
        self.presentation.set_system_theme(system, mode);
    }
}
