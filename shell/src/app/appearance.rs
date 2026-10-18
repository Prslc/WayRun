use std::time::{Duration, Instant};

use crate::config::{AppearanceConfig, Mode};
use crate::ui::theme::{Surfaces, Theme};

use super::State;

/// The entrance fade's duration: a flat pixmap cannot scale, so a show fades in.
pub const ENTRANCE_MS: u64 = 240;
/// How far below its resting place the card starts; the entrance rises into it.
const ENTRANCE_SLIDE: f32 = 8.0;
/// The card height's spring rates: a growth settles in ~150ms so results
/// appear promptly, a shrink in ~240ms so the card relaxes gently.
const CARD_GROW_OMEGA: f32 = 32.0;
const CARD_SHRINK_OMEGA: f32 = 20.0;

impl State {
    /// The `theme.toml` config changed. Recompute the theme and the derived
    /// geometry; the caller repaints the whole frame.
    pub fn apply_appearance(&mut self, config: AppearanceConfig, now: Instant) {
        self.reduce_motion = self.reduced_env || config.reduced;
        self.appearance = config;
        self.refresh_theme();
        self.card_h = self.content_height();
        self.card_v = 0.0;
        self.card_target = self.card_h;
        self.card_updated = now;
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
        ease_in_out_cubic((elapsed / ENTRANCE_MS as f32).clamp(0.0, 1.0))
    }

    /// How far the card still has to rise: the entrance starts it `SLIDE` px
    /// low and settles at the configured position.
    pub fn entrance_offset(&self, now: Instant) -> f32 {
        (1.0 - self.entrance(now)) * ENTRANCE_SLIDE
    }

    /// The geometry a frame draws with: the card slides while the entrance runs,
    /// and the dim and the stored layout stay put.
    pub fn frame_layout(&self, now: Instant) -> crate::ui::geom::Layout {
        self.appearance.layout.shifted(self.entrance_offset(now))
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

    /// [`Self::fade`] with a per-row alpha on top of the entrance.
    pub fn fade_scaled(&self, color: [u8; 3], alpha: f32, now: Instant, scale: f32) -> [u8; 4] {
        self.fade(color, alpha * scale, now)
    }

    /// [`Self::fade_rgba`] with a per-row alpha on top of the entrance.
    pub fn fade_rgba_scaled(&self, color: [u8; 4], now: Instant, scale: f32) -> [u8; 4] {
        self.fade(
            [color[0], color[1], color[2]],
            color[3] as f32 / 255.0 * scale,
            now,
        )
    }

    pub fn animating(&self, now: Instant) -> bool {
        if self.reduce_motion || !self.entrance_started {
            return false;
        }

        let entrance =
            now.saturating_duration_since(self.shown_at) < Duration::from_millis(ENTRANCE_MS);
        let height = (self.card_target - self.card_h).abs() > 0.5 || self.card_v.abs() > 5.0;
        let moving = self.row_motion.iter().flatten().any(|motion| {
            now.saturating_duration_since(motion.started)
                < Duration::from_millis(super::ROW_MOTION_MS)
        });
        entrance || height || moving
    }

    /// Advance the card height one frame: the spring carries velocity across
    /// target changes, so a new payload redirects the motion instead of restarting.
    pub fn advance(&mut self, now: Instant) {
        if self.reduce_motion {
            self.card_h = self.card_target;
            self.card_v = 0.0;
            self.card_updated = now;
            return;
        }

        let dt = now
            .saturating_duration_since(self.card_updated)
            .as_secs_f32();
        if dt <= 0.0 {
            return;
        }
        self.card_updated = now;

        let offset = self.card_h - self.card_target;
        let omega = if offset > 0.0 {
            CARD_SHRINK_OMEGA
        } else {
            CARD_GROW_OMEGA
        };
        // The exact critically damped step: x(t) = (x0 + (v0 + ωx0)t)e^{-ωt}.
        let decay = (-omega * dt).exp();
        let slope = self.card_v + omega * offset;
        let next = (offset + slope * dt) * decay;
        self.card_v = (self.card_v - omega * slope * dt) * decay;
        self.card_h = self.card_target + next;
        if next.abs() < 0.5 && self.card_v.abs() < 5.0 {
            self.card_h = self.card_target;
            self.card_v = 0.0;
        }
    }

    /// The first frame of a show: this is where the entrance fade begins.
    pub fn start_entrance(&mut self, now: Instant) {
        if self.entrance_started {
            return;
        }
        self.entrance_started = true;
        self.shown_at = now;
        self.card_h = self.content_height();
        self.card_v = 0.0;
        self.card_target = self.card_h;
        self.card_updated = now;
    }

    pub fn retarget_height(&mut self, now: Instant) {
        self.card_target = self.content_height();
        if self.reduce_motion {
            self.card_h = self.card_target;
            self.card_v = 0.0;
        }
        // An idle gap must not settle the spring in one frame; keep the clock
        // at the event, and the velocity carries into the new target.
        self.card_updated = now;
    }
}

/// MD3's emphasized-decelerate curve, for a row entering or moving into place.
pub(crate) fn emphasized_decelerate(t: f32) -> f32 {
    cubic_bezier(t.clamp(0.0, 1.0), 0.05, 0.7, 0.1, 1.0)
}

fn cubic_bezier(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let curve = |a: f32, b: f32, u: f32| {
        3.0 * (1.0 - u).powi(2) * u * a + 3.0 * (1.0 - u) * u * u * b + u.powi(3)
    };
    let mut u = x;
    for _ in 0..8 {
        let error = curve(x1, x2, u) - x;
        if error.abs() < 1e-5 {
            break;
        }
        let slope = 3.0 * (1.0 - u).powi(2) * x1
            + 6.0 * (1.0 - u) * u * (x2 - x1)
            + 3.0 * u * u * (1.0 - x2);
        if slope.abs() < 1e-6 {
            break;
        }
        u = (u - error / slope).clamp(0.0, 1.0);
    }
    curve(y1, y2, u)
}

fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
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

    #[test]
    fn the_emphasized_curve_hits_its_endpoints() {
        assert_eq!(emphasized_decelerate(0.0), 0.0);
        assert_eq!(emphasized_decelerate(1.0), 1.0);
        assert!(emphasized_decelerate(0.5) > 0.5);
    }

    #[test]
    fn the_entrance_rises_into_the_resting_layout() {
        let mut state = state();
        state.reduce_motion = false;
        state.entrance_started = true;
        let start = std::time::Instant::now();
        state.shown_at = start;

        assert!((state.entrance_offset(start) - 8.0).abs() < 0.001);
        assert!(state.frame_layout(start).offset_y > 0.0);

        let settled = start + Duration::from_millis(ENTRANCE_MS);
        assert_eq!(state.entrance_offset(settled), 0.0);
        assert_eq!(state.frame_layout(settled).offset_y, 0.0);

        state.reduce_motion = true;
        assert_eq!(state.entrance_offset(start), 0.0);
    }
}
