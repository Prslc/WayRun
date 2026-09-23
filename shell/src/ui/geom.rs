pub const ROW_H: f32 = 64.0;
pub const PAD: f32 = 14.0;
pub const SEARCH_H: f32 = 52.0;
pub const GAP: f32 = 10.0;
pub const FOOTER_H: f32 = 28.0;

/// The card's horizontal anchor on the output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

/// The runtime geometry, overridable from `theme.toml`. The defaults reproduce
/// the original metrics, so an untouched config renders the same frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub width_ratio: f32,
    pub width_min: f32,
    pub width_max: f32,
    pub top_ratio: f32,
    pub align: Align,
    pub offset_x: f32,
    pub offset_y: f32,
    pub radius: f32,
    // Explicit inner radii; `None` keeps the base each one derives from.
    pub field_radius: Option<f32>,
    pub row_radius: Option<f32>,
    pub chip_radius: Option<f32>,
    pub hairline_width: f32,
    pub accent_width: f32,
    pub accent_height: f32,
    pub max_rows: usize,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            width_ratio: 0.38,
            width_min: 560.0,
            width_max: 760.0,
            top_ratio: 0.28,
            align: Align::Center,
            offset_x: 0.0,
            offset_y: 0.0,
            radius: 16.0,
            field_radius: None,
            row_radius: None,
            chip_radius: None,
            hairline_width: 1.0,
            accent_width: 3.0,
            accent_height: 28.0,
            max_rows: 5,
        }
    }
}

/// One rect of the blur region, in surface-local coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlurRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Layout {
    pub fn card_w(&self, surface: (u32, u32)) -> f32 {
        ((surface.0 as f32) * self.width_ratio).clamp(self.width_min, self.width_max)
    }

    pub fn card_x(&self, surface: (u32, u32)) -> f32 {
        let width = self.card_w(surface);
        let base = match self.align {
            Align::Left => 0.0,
            Align::Center => ((surface.0 as f32) - width) / 2.0,
            Align::Right => (surface.0 as f32) - width,
        };
        (base + self.offset_x).round()
    }

    /// Fixed card top: results only extend the card downward, so the search bar
    /// never moves when the row count changes.
    pub fn card_top(&self, surface: (u32, u32)) -> f32 {
        ((surface.1 as f32) * self.top_ratio + self.offset_y).round()
    }

    /// The ✕ button's circle in logical pixels: centre x, centre y, radius. The
    /// toolbar draws it and `State::clear_hit` claims it.
    pub fn clear_circle(&self, surface: (u32, u32)) -> (f32, f32, f32) {
        let right = self.card_x(surface) + self.card_w(surface) - PAD - 8.0;
        let center_y = self.card_top(surface) + PAD + SEARCH_H / 2.0;
        (right - 13.0, center_y, 13.0)
    }

    /// The y of the first list row: the card's padding, the search field and the
    /// column's spacing.
    pub fn rows_top(&self, surface: (u32, u32)) -> f32 {
        self.card_top(surface) + PAD + SEARCH_H + GAP
    }

    pub fn list_h(&self, rows: usize) -> f32 {
        (rows as f32 * ROW_H).min(self.max_rows as f32 * ROW_H)
    }

    /// `pad + search + gap + footer + pad`, plus a second gap and the list when
    /// rows exist. The list is capped at `max_rows`, so the card needs no cap.
    pub fn content_h(&self, rows: usize) -> f32 {
        let base = PAD + SEARCH_H + GAP + FOOTER_H + PAD;
        if rows == 0 {
            base
        } else {
            base + GAP + self.list_h(rows)
        }
    }

    /// The action panel's header band: the parent row's title, above the actions offered.
    pub fn panel_header_h(&self) -> f32 {
        28.0
    }

    /// The card height with the action panel open: like `content_h`, but the
    /// list band holds the actions under a header instead of the results.
    pub fn panel_h(&self, actions: usize) -> f32 {
        let base = PAD + SEARCH_H + GAP + FOOTER_H + PAD;
        base + GAP + self.panel_header_h() + self.list_h(actions)
    }

    /// The y of the first action row: the panel header sits where the first
    /// result row would.
    pub fn actions_top(&self, surface: (u32, u32)) -> f32 {
        self.rows_top(surface) + self.panel_header_h()
    }

    /// The action index under a surface-local point, `None` outside the panel.
    pub fn action_at(
        &self,
        surface: (u32, u32),
        first: usize,
        count: usize,
        x: f32,
        y: f32,
    ) -> Option<usize> {
        let left = self.card_x(surface);
        if x < left || x > left + self.card_w(surface) {
            return None;
        }

        let top = self.actions_top(surface);
        if y < top || y >= top + self.list_h(count) {
            return None;
        }

        let index = first + ((y - top) / ROW_H) as usize;
        (index < count).then_some(index)
    }

    /// The blur region covering the card's rounded rect, as 2px scanline bands
    /// inset by `r - sqrt(r² - (r - dy)²)` so no rect pokes past the edge.
    pub fn blur_rects(&self, surface: (u32, u32), card_height: f32) -> Vec<BlurRect> {
        // Floor to 4px so an in-flight reflow does not commit a region per frame.
        let height = ((card_height / 4.0).floor() * 4.0).max(4.0);
        let (x, y) = (
            self.card_x(surface).round() as i32,
            self.card_top(surface).round() as i32,
        );
        let width = self.card_w(surface).round() as i32;
        let height = height.round() as i32;
        let radius = self.radius.round() as i32;

        let mut rects = Vec::with_capacity(radius as usize + 4);

        if height <= 2 * radius {
            rects.push(BlurRect {
                x,
                y,
                width,
                height,
            });
            return rects;
        }

        rects.push(BlurRect {
            x,
            y: y + radius,
            width,
            height: height - 2 * radius,
        });

        let r = radius as f32;
        for offset in (0..radius).step_by(2) {
            let f = offset as f32;
            let inset = (r - (r * r - (r - f) * (r - f)).max(0.0).sqrt()).floor() as i32;
            let band = BlurRect {
                x: x + inset,
                y: y + offset,
                width: width - 2 * inset,
                height: 2,
            };
            rects.push(band);
            rects.push(BlurRect {
                y: y + height - offset - 2,
                ..band
            });
        }

        rects
    }

    /// The row index under a surface-local point, `None` outside the list band.
    pub fn row_at(
        &self,
        surface: (u32, u32),
        first: usize,
        rows: usize,
        x: f32,
        y: f32,
    ) -> Option<usize> {
        let left = self.card_x(surface);
        if x < left || x > left + self.card_w(surface) {
            return None;
        }

        let top = self.rows_top(surface);
        if y < top || y >= top + self.list_h(rows) {
            return None;
        }

        let index = first + ((y - top) / ROW_H) as usize;
        (index < rows).then_some(index)
    }

    /// `Contain` semantics for the fixed [`Layout::max_rows`]-row window.
    pub fn contain(&self, selected: usize, first: usize, rows: usize) -> usize {
        if rows <= self.max_rows {
            return 0;
        }

        let mut first = first.min(rows - self.max_rows);
        if selected < first {
            first = selected;
        }
        if selected >= first + self.max_rows {
            first = selected + 1 - self.max_rows;
        }
        first
    }

    /// The card's 1px hairline, half a pixel inside the fill.
    pub fn hairline_radius(&self) -> f32 {
        (self.radius - 0.5).max(0.0)
    }

    /// Nested radii stay inside the card: at the default 16 they are the
    /// designed 9/8/6, and a smaller card radius pulls them in with it.
    fn inner(&self, base: f32, explicit: Option<f32>) -> f32 {
        explicit
            .unwrap_or(base)
            .min((self.radius - 1.0).max(0.0))
            .max(0.0)
    }

    pub fn field_radius(&self) -> f32 {
        self.inner(9.0, self.field_radius)
    }

    pub fn row_radius(&self) -> f32 {
        self.inner(8.0, self.row_radius)
    }

    pub fn chip_radius(&self) -> f32 {
        self.inner(6.0, self.chip_radius)
    }

    /// The selected row's accent bar, a pill half as wide as it is thick.
    pub fn accent_radius(&self) -> f32 {
        self.accent_width / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_at_inverts_the_layout_the_renderer_draws() {
        let l = Layout::default();
        let surface = (1920, 1080);
        let left = l.card_x(surface) + 10.0;
        let top = l.rows_top(surface);

        assert_eq!(l.row_at(surface, 0, 20, left, top), Some(0));
        assert_eq!(l.row_at(surface, 0, 20, left, top + ROW_H + 1.0), Some(1));
        // `first` is the window's top row, so surface row 0 is `first`
        assert_eq!(l.row_at(surface, 2, 20, left, top + 0.5), Some(2));

        // only the five-row window is drawn
        assert_eq!(
            l.row_at(surface, 0, 20, left, top + l.max_rows as f32 * ROW_H),
            None
        );
        // outside the card, and above the list
        assert_eq!(l.row_at(surface, 0, 20, l.card_x(surface) - 1.0, top), None);
        assert_eq!(l.row_at(surface, 0, 20, left, top - 1.0), None);
        // a short list has nothing below its last row
        assert_eq!(l.row_at(surface, 0, 2, left, top + 2.0 * ROW_H + 1.0), None);
    }

    #[test]
    fn the_blur_region_covers_the_card_and_stays_inside_it() {
        let l = Layout::default();
        let surface = (1920, 1080);
        let height = l.content_h(l.max_rows);
        let (x, y) = (
            l.card_x(surface).round() as i32,
            l.card_top(surface).round() as i32,
        );
        let (w, h) = (l.card_w(surface).round() as i32, height.round() as i32);
        let rects = l.blur_rects(surface, height);

        // the region is always the card's outer rect plus its scanline bands
        for rect in &rects {
            assert!(rect.width > 0 && rect.height > 0, "{rect:?}");
            assert!(rect.x >= x && rect.x + rect.width <= x + w, "{rect:?}");
            assert!(rect.y >= y && rect.y + rect.height <= y + h, "{rect:?}");
        }
        // the first band is the middle one: inset by the radius top and bottom
        assert_eq!(rects[0].y, y + l.radius as i32);
        assert_eq!(rects[0].height, h - 2 * l.radius as i32);
        // the corner bands are pulled in by the chord inset, most at the very
        // top of the card, and they alternate top/bottom per offset
        assert_eq!(rects[1].x, x + l.radius as i32);
        assert_eq!(rects[1].y, y);
        assert_eq!(rects[2].y + 2, y + h);
        // a card shorter than two radii degrades to one rect
        assert_eq!(l.blur_rects(surface, 20.0).len(), 1);
    }

    #[test]
    fn the_defaults_reproduce_the_original_metrics() {
        let l = Layout::default();
        assert_eq!(l.content_h(0), 118.0);
        assert_eq!(l.content_h(5), 448.0);
        assert_eq!(l.field_radius(), 9.0);
        assert_eq!(l.row_radius(), 8.0);
        assert_eq!(l.chip_radius(), 6.0);
        assert_eq!(l.hairline_radius(), 15.5);
    }

    #[test]
    fn the_action_panel_adds_a_header_above_the_actions() {
        let l = Layout::default();
        assert_eq!(l.panel_h(3), l.content_h(3) + l.panel_header_h());
        // the actions start where the first result row would, one header lower
        let surface = (1920, 1080);
        assert_eq!(
            l.actions_top(surface),
            l.rows_top(surface) + l.panel_header_h()
        );

        let left = l.card_x(surface) + 10.0;
        let top = l.actions_top(surface);
        assert_eq!(l.action_at(surface, 0, 4, left, top), Some(0));
        assert_eq!(l.action_at(surface, 0, 4, left, top + ROW_H + 1.0), Some(1));
        // a `first` window shifts surface row 0 to that action
        assert_eq!(l.action_at(surface, 2, 20, left, top + 0.5), Some(2));
        // above the actions is the header, not an action
        assert_eq!(l.action_at(surface, 0, 4, left, top - 1.0), None);
        // only the `max_rows` window is drawn
        assert_eq!(
            l.action_at(surface, 0, 20, left, top + l.max_rows as f32 * ROW_H),
            None
        );
        // a short panel has nothing below its last action
        assert_eq!(
            l.action_at(surface, 0, 2, left, top + 2.0 * ROW_H + 1.0),
            None
        );
    }

    #[test]
    fn a_smaller_card_radius_pulls_the_nested_radii_in() {
        let l = Layout {
            radius: 8.0,
            ..Layout::default()
        };
        assert_eq!(l.field_radius(), 7.0);
        assert_eq!(l.row_radius(), 7.0);
        assert_eq!(l.chip_radius(), 6.0);
        assert_eq!(l.hairline_radius(), 7.5);
    }

    #[test]
    fn align_and_offsets_place_the_card() {
        let surface = (1920, 1080);
        let center = Layout::default();
        let width = center.card_w(surface);
        assert_eq!(center.card_x(surface), ((1920.0 - width) / 2.0).round());
        assert_eq!(center.card_top(surface), (1080.0_f32 * 0.28).round());

        let left = Layout {
            align: Align::Left,
            offset_x: 20.0,
            ..Layout::default()
        };
        assert_eq!(left.card_x(surface), 20.0);

        let right = Layout {
            align: Align::Right,
            ..Layout::default()
        };
        assert_eq!(right.card_x(surface), (1920.0_f32 - width).round());

        let shifted = Layout {
            offset_y: -30.0,
            ..Layout::default()
        };
        assert_eq!(
            shifted.card_top(surface),
            (1080.0_f32 * 0.28 - 30.0).round()
        );
    }

    #[test]
    fn an_explicit_inner_radius_is_pulled_inside_the_card() {
        let l = Layout {
            radius: 10.0,
            field_radius: Some(20.0),
            row_radius: Some(4.0),
            ..Layout::default()
        };
        assert_eq!(l.field_radius(), 9.0);
        assert_eq!(l.row_radius(), 4.0);
        assert_eq!(l.accent_radius(), 1.5);
    }
}
