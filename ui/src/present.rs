use std::time::{Duration, Instant};

use crate::config::{AppearanceConfig, Mode};
use crate::geom::Layout;
use crate::theme::{Surfaces, Theme};

/// The entrance fade's duration: a flat pixmap cannot scale, so a show fades in.
pub const ENTRANCE_MS: u64 = 240;
/// How far below its resting place the card starts; the entrance rises into it.
const ENTRANCE_SLIDE: f32 = 8.0;
/// The card height's spring rates: a growth settles in ~150ms so results
/// appear promptly, a shrink in ~240ms so the card relaxes gently.
const CARD_GROW_OMEGA: f32 = 32.0;
const CARD_SHRINK_OMEGA: f32 = 20.0;
/// A row's move or entry animation, on MD3's short4 duration.
pub const ROW_MOTION_MS: u64 = 200;
/// How far below its place a new row starts, in logical pixels.
pub const ROW_RISE: f32 = 4.0;

/// A row's layout motion: how far from its final place it starts (positive is
/// below), the opacity it fades in from, and when the motion began.
#[derive(Clone, Copy)]
pub struct RowMotion {
    pub offset: f32,
    pub alpha: f32,
    pub started: Instant,
}

impl RowMotion {
    /// The row's opacity and remaining offset at `now`.
    pub fn sample(&self, now: Instant) -> (f32, f32) {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32() * 1000.0;
        let t = (elapsed / ROW_MOTION_MS as f32).clamp(0.0, 1.0);
        let eased = emphasized_decelerate(t);
        (
            self.alpha + (1.0 - self.alpha) * eased,
            self.offset * (1.0 - eased),
        )
    }

    pub fn is_active(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) < Duration::from_millis(ROW_MOTION_MS)
    }
}

/// The appearance and motion state a frame is drawn from, so the renderer
/// reads only [`crate::view::Frame`].
pub struct Presentation {
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
    reduced_env: bool,
    pub reduce_motion: bool,
    /// Whether the entrance animation has started on the first drawn frame.
    pub entrance_started: bool,
    shown_at: Instant,
    pub card_h: f32,
    pub card_v: f32,
    pub card_target: f32,
    card_updated: Instant,
}

impl Presentation {
    pub fn new(reduced_env: bool) -> Self {
        let now = Instant::now();
        let appearance = AppearanceConfig::default();
        let reduce_motion = reduced_env || appearance.reduced;
        let theme = Theme::default();
        let mode = Mode::default();
        let colors = appearance.colors.for_mode(mode);
        let overrides = appearance.surfaces.for_mode(mode);
        let surfaces = Surfaces::resolve(theme, &colors, &overrides, mode);
        let card = appearance.layout.empty_h();
        Self {
            system_theme: theme,
            mode,
            appearance,
            theme,
            surfaces,
            reduced_env,
            reduce_motion,
            entrance_started: false,
            shown_at: now,
            card_h: card,
            card_v: 0.0,
            card_target: card,
            card_updated: now,
        }
    }

    /// The `theme.toml` config changed; the caller repaints the whole frame.
    pub fn apply_appearance(&mut self, config: AppearanceConfig) {
        self.reduce_motion = self.reduced_env || config.reduced;
        self.appearance = config;
        self.refresh_theme();
    }

    /// The core's system theme changed, with the mode whose palette it holds.
    pub fn set_system_theme(&mut self, system: Theme, mode: Mode) {
        self.system_theme = system;
        self.mode = mode;
        self.refresh_theme();
    }

    fn refresh_theme(&mut self) {
        let colors = self.appearance.colors.for_mode(self.mode);
        let overrides = self.appearance.surfaces.for_mode(self.mode);
        self.theme = Theme::overlay(self.system_theme, &colors);
        self.surfaces = Surfaces::resolve(self.theme, &colors, &overrides, self.mode);
    }

    /// A show starts with the entrance unstarted; the first drawn frame arms it.
    pub fn begin_show(&mut self, now: Instant) {
        self.entrance_started = false;
        self.shown_at = now;
    }

    /// The first frame of a show: this is where the entrance fade begins.
    pub fn start_entrance(&mut self, now: Instant) {
        self.entrance_started = true;
        self.shown_at = now;
    }

    /// Snap the card to the current content height: a fresh show, a config
    /// change or the first frame starts from the resting geometry.
    pub fn snap_to(&mut self, content_h: f32, now: Instant) {
        self.card_h = content_h;
        self.card_v = 0.0;
        self.card_target = content_h;
        self.card_updated = now;
    }

    /// The entrance's raw progress: 0 before the first drawn frame, 1 once
    /// reduced motion or the fade is over.
    fn entrance_progress(&self, now: Instant) -> f32 {
        if self.reduce_motion {
            return 1.0;
        }
        if !self.entrance_started {
            return 0.0;
        }
        let elapsed = now.saturating_duration_since(self.shown_at).as_secs_f32() * 1000.0;
        (elapsed / ENTRANCE_MS as f32).clamp(0.0, 1.0)
    }

    /// The entrance's opacity factor: ease-out makes the card visible on the
    /// first drawn frame and decelerates into place.
    pub fn entrance(&self, now: Instant) -> f32 {
        ease_out_cubic(self.entrance_progress(now))
    }

    /// How far the card still has to rise: the entrance starts it `SLIDE` px
    /// low and settles at the configured position; the slide eases in and out.
    pub fn entrance_offset(&self, now: Instant) -> f32 {
        (1.0 - ease_in_out_cubic(self.entrance_progress(now))) * ENTRANCE_SLIDE
    }

    /// The geometry a frame draws with: the card slides while the entrance runs,
    /// and the dim and the stored layout stay put.
    pub fn frame_layout(&self, now: Instant) -> Layout {
        self.appearance.layout.shifted(self.entrance_offset(now))
    }

    pub fn animating(&self, now: Instant) -> bool {
        if self.reduce_motion || !self.entrance_started {
            return false;
        }

        let entrance =
            now.saturating_duration_since(self.shown_at) < Duration::from_millis(ENTRANCE_MS);
        let height = (self.card_target - self.card_h).abs() > 0.5 || self.card_v.abs() > 5.0;
        entrance || height
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

    pub fn retarget(&mut self, content_h: f32, now: Instant) {
        self.card_target = content_h;
        if self.reduce_motion {
            self.card_h = self.card_target;
            self.card_v = 0.0;
        }
        // An idle gap must not settle the spring in one frame; keep the clock
        // at the event, and the velocity carries into the new target.
        self.card_updated = now;
    }

    pub fn card_height(&self) -> f32 {
        self.card_h
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

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
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

    #[test]
    fn the_emphasized_curve_hits_its_endpoints() {
        assert_eq!(emphasized_decelerate(0.0), 0.0);
        assert_eq!(emphasized_decelerate(1.0), 1.0);
        assert!(emphasized_decelerate(0.5) > 0.5);
    }

    #[test]
    fn the_entrance_rises_into_the_resting_layout() {
        let mut present = Presentation::new(false);
        present.reduce_motion = false;
        present.entrance_started = true;
        let start = Instant::now();
        present.shown_at = start;

        assert!((present.entrance_offset(start) - 8.0).abs() < 0.001);
        assert!(present.frame_layout(start).offset_y > 0.0);
        let visible = start + Duration::from_millis(17);
        assert!(
            present.entrance(visible) > 0.15,
            "the first visible frame is not blank"
        );

        let settled = start + Duration::from_millis(ENTRANCE_MS);
        assert_eq!(present.entrance_offset(settled), 0.0);
        assert_eq!(present.frame_layout(settled).offset_y, 0.0);

        present.reduce_motion = true;
        assert_eq!(present.entrance_offset(start), 0.0);
    }

    #[test]
    fn an_appearance_change_rebuilds_the_theme_and_geometry() {
        let mut present = Presentation::new(false);
        let now = Instant::now();

        let mut config = AppearanceConfig::default();
        config.colors.shared.fg = Some([0x10, 0x20, 0x30]);
        config.surfaces.dark.card = Some(crate::config::SurfaceOverride {
            color: Some([9, 8, 7]),
            alpha: Some(0.5),
        });
        config.layout.max_rows = 2;
        present.set_system_theme(
            Theme {
                primary: [1, 2, 3],
                fg: [4, 5, 6],
                container: [7, 8, 9],
            },
            Mode::Dark,
        );
        present.apply_appearance(config);
        present.snap_to(present.appearance.layout.empty_h(), now);

        assert_eq!(present.theme.primary, [1, 2, 3]);
        assert_eq!(present.theme.fg, [0x10, 0x20, 0x30]);
        assert_eq!(present.theme.container, [7, 8, 9]);
        assert_eq!(present.surfaces.card, [9, 8, 7, 128]);
        assert_eq!(present.surfaces.dim, crate::theme::DEFAULT_DIM);
        assert_eq!(present.card_target, present.appearance.layout.empty_h());
    }

    #[test]
    fn the_env_reduced_motion_flag_survives_a_config_reload() {
        let mut present = Presentation::new(true);
        present.reduce_motion = true;
        let config = AppearanceConfig {
            reduced: false,
            ..AppearanceConfig::default()
        };
        present.apply_appearance(config);
        assert!(present.reduce_motion);
    }
}
