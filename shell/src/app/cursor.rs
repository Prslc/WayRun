use crate::ui::geom::Layout;

use super::State;

/// What the pointer is over: a list row's tint, an action-panel row's, or the ✕ button's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hover {
    Row(usize),
    Action(usize),
    Clear,
}

/// The window a list moves through: the highlighted index and the top row of
/// the fixed `max_rows` window; list and panel move theirs by the same rules.
#[derive(Default)]
pub struct Cursor {
    pub selected: usize,
    pub first: usize,
}

impl Cursor {
    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn down(&mut self, len: usize) {
        if self.selected + 1 < len {
            self.selected += 1;
        }
    }

    pub fn page_up(&mut self, page: usize) {
        self.selected = self.selected.saturating_sub(page);
    }

    pub fn page_down(&mut self, len: usize, page: usize) {
        self.selected = (self.selected + page).min(len.saturating_sub(1));
    }

    /// Move by whole rows (the wheel).
    pub fn scroll(&mut self, rows: i32, len: usize) {
        let last = len.saturating_sub(1) as i32;
        self.selected = (self.selected as i32 + rows).clamp(0, last) as usize;
    }

    pub fn contain(&mut self, len: usize, layout: Layout) {
        self.first = layout.contain(self.selected, self.first, len);
    }
}

impl State {
    /// The pointer position and the hover it implies; returns whether the hover
    /// changed. The ✕ button sits outside the list and wins over a row.
    pub fn hover_at(&mut self, x: f32, y: f32) -> bool {
        self.pointer = Some((x, y));
        let hover = if self.clear_hit(x, y) {
            Some(Hover::Clear)
        } else if let Some(menu) = &self.menu {
            self.appearance
                .layout
                .action_at(self.surface, menu.cursor.first, menu.actions.len(), x, y)
                .map(Hover::Action)
        } else {
            self.appearance
                .layout
                .row_at(self.surface, self.cursor.first, self.rows.len(), x, y)
                .map(Hover::Row)
        };

        if hover == self.hovered {
            return false;
        }
        self.hovered = hover;
        true
    }

    /// The pointer left the surface.
    pub fn pointer_left(&mut self) {
        self.pointer = None;
        self.hovered = None;
    }

    /// Rows that move under a stationary pointer are different rows and fire no
    /// enter/exit, so re-derive the hover; `Hover::Clear` is left alone.
    pub fn resync_hover(&mut self) {
        let hit = self.pointer.and_then(|(x, y)| {
            if let Some(menu) = &self.menu {
                self.appearance
                    .layout
                    .action_at(self.surface, menu.cursor.first, menu.actions.len(), x, y)
                    .map(Hover::Action)
            } else {
                self.appearance
                    .layout
                    .row_at(self.surface, self.cursor.first, self.rows.len(), x, y)
                    .map(Hover::Row)
            }
        });

        match hit {
            Some(hit) => self.hovered = Some(hit),
            None if matches!(self.hovered, Some(Hover::Row(_) | Hover::Action(_))) => {
                self.hovered = None
            }
            None => {}
        }
    }

    /// Whether a surface-local point is on the ✕ button, whose circle comes from
    /// `Layout::clear_circle` so the hit test cannot drift from what is drawn.
    pub fn clear_hit(&self, x: f32, y: f32) -> bool {
        if self.query.is_empty() {
            return false;
        }

        let (cx, cy, r) = self.appearance.layout.clear_circle(self.surface);
        (x - cx).abs() <= r && (y - cy).abs() <= r
    }

    /// Move the selection by whole rows (the wheel): the highlighted row is what
    /// Enter launches, so the selection moves, not just the window.
    pub fn scroll(&mut self, rows: i32) {
        if self.rows.is_empty() || rows == 0 {
            return;
        }
        self.cursor.scroll(rows, self.rows.len());
        self.contain();
    }

    pub fn up(&mut self) {
        self.cursor.up();
        self.contain();
    }

    pub fn down(&mut self) {
        self.cursor.down(self.rows.len());
        self.contain();
    }

    pub fn page_up(&mut self) {
        self.cursor.page_up(self.appearance.layout.max_rows);
        self.contain();
    }

    pub fn page_down(&mut self) {
        self.cursor
            .page_down(self.rows.len(), self.appearance.layout.max_rows);
        self.contain();
    }
}

/// Whole rows from a fractional wheel delta, carrying the remainder. One notch
/// arrives as a line and its pixel half, so pixels must not add a second row.
pub fn whole_rows(accum: &mut f32, delta: f32) -> i32 {
    // A reversal starts a new gesture: without this the previous direction's
    // slack would swallow the first notch back.
    if accum.signum() * delta.signum() < 0.0 {
        *accum = 0.0;
    }

    *accum += delta;
    let whole = accum.trunc();
    *accum -= whole;
    whole as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{item, run, state};
    use crate::ui::geom;
    use wayrun_core::wire::ResultItem;

    #[test]
    fn contain_moves_only_as_far_as_the_selection_needs() {
        let l = Layout::default();
        let max = l.max_rows;
        assert_eq!(l.contain(0, 0, 20), 0);
        assert_eq!(l.contain(max - 1, 0, 20), 0);
        // stepping past the window scrolls by exactly one row
        assert_eq!(l.contain(max, 0, 20), 1);
        assert_eq!(l.contain(max + 1, 1, 20), 2);
        // moving up *inside* the window keeps it put
        assert_eq!(l.contain(3, 1, 20), 1);
        // leaving it upwards follows the selection
        assert_eq!(l.contain(0, 3, 20), 0);
        // the window never runs past the last row
        assert_eq!(l.contain(19, 0, 20), 15);
        assert_eq!(l.contain(19, 17, 20), 15);
        // a list shorter than the window cannot scroll
        assert_eq!(l.contain(0, 5, 3), 0);
        assert_eq!(l.contain(2, 5, 3), 0);
    }

    #[test]
    fn a_wheel_notch_moves_exactly_one_row() {
        // the line delta and its pixel twin both arrive for one notch
        let mut accum = 0.0;
        assert_eq!(whole_rows(&mut accum, 1.0), 1);
        assert_eq!(whole_rows(&mut accum, -2.0 / 64.0), 0);
        // a trackpad accumulates pixels into whole rows
        let mut accum = 0.0;
        assert_eq!(whole_rows(&mut accum, 0.5), 0);
        assert_eq!(whole_rows(&mut accum, 0.5), 1);
        // a reversal drops the previous gesture's slack
        let mut accum = -1.0;
        assert_eq!(whole_rows(&mut accum, 0.5), 0);
        assert_eq!(whole_rows(&mut accum, 1.0), 1);
    }

    #[test]
    fn a_moving_list_keeps_the_hover_on_the_row_under_the_pointer() {
        let mut state = state();
        let items: Vec<ResultItem> = (0..20)
            .map(|i| item(&format!("row {i}"), None, Some(run("x")), None))
            .collect();
        state.apply_results(items, std::time::Instant::now());

        // the pointer sits on the second visible row
        let layout = state.appearance.layout;
        let x = layout.card_x(state.surface) + 10.0;
        let y = layout.rows_top(state.surface) + geom::ROW_H + 1.0;
        assert!(state.hover_at(x, y));
        assert_eq!(state.hovered, Some(Hover::Row(1)));

        // a page turn draws different rows under the same pointer: the hover has
        // to follow the row now under it, which is one further down the list
        state.cursor.selected = layout.max_rows;
        state.contain();
        assert_eq!(state.cursor.first, 1);
        assert_eq!(state.hovered, Some(Hover::Row(2)));

        // and the ✕ button's hover is not a row, so a page turn leaves it alone
        state.query = "q".into();
        let clear = (
            layout.card_x(state.surface) + layout.card_w(state.surface) - geom::PAD - 8.0 - 13.0,
            layout.card_top(state.surface) + geom::PAD + geom::SEARCH_H / 2.0,
        );
        assert!(state.hover_at(clear.0, clear.1));
        assert_eq!(state.hovered, Some(Hover::Clear));
        state.contain();
        assert_eq!(state.hovered, Some(Hover::Clear));

        state.pointer_left();
        assert_eq!(state.hovered, None);
    }
}
