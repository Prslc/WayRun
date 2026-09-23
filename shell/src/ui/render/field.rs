use std::sync::OnceLock;
use std::time::Instant;

use cosmic_text::Weight;
use rust_i18n::t;
use tiny_skia::Pixmap;

use crate::app::{Hover, State};
use crate::ui::text::TextEngine;

use super::canvas::{CLEAR_GLYPH, Canvas, Rect, TEXT_INSET};

pub(super) fn draw_magnifier(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    field: Rect,
    state: &State,
    now: Instant,
) {
    // A 2px circle of radius 6.2 at (9, 9) with a handle to (19, 19) in a 22×22
    // box, drawn rather than loaded.
    let x = field.x + 14.0;
    let y = field.center_y() - 11.0;
    let color = state.fade_rgba(state.surfaces.muted, now);

    canvas.stroke_circle(pixmap, (x + 9.0, y + 9.0), 6.2, 2.0, color);
    canvas.stroke_line(
        pixmap,
        (x + 13.8, y + 13.8),
        (x + 19.0, y + 19.0),
        2.0,
        color,
    );
}

/// The field's text area: left edge and width. The field is single-line, so a
/// wider query scrolls inside the box rather than clipping the caret.
fn text_area(field: Rect) -> (f32, f32) {
    (field.x + TEXT_INSET, field.w - TEXT_INSET - 40.0)
}

/// The field's placeholder, translated once: it is drawn every frame while empty.
fn placeholder_text() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| t!("field.placeholder"))
}

/// How far the query has to be scrolled left for a caret at `caret_x` to stay
/// inside the text area.
fn scroll_for(caret_x: f32, area: (f32, f32)) -> f32 {
    (caret_x + 2.0 - (area.0 + area.1)).max(0.0)
}

pub(super) fn draw_query(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    field: Rect,
    state: &State,
    text: &mut TextEngine,
    now: Instant,
) {
    let area = text_area(field);
    let (x, width) = area;
    let size = state.appearance.font.query() * canvas.scale;
    let fg = state.theme.fg;
    let clip = [
        canvas.px(x),
        canvas.px(field.y),
        canvas.px(width),
        canvas.px(field.h),
    ];

    let placeholder = state.query.is_empty() && state.preedit.is_none();
    if placeholder {
        let shaped = text.shape(placeholder_text(), size, Weight::MEDIUM);
        let top = field.center_y() - shaped.height / (2.0 * canvas.scale);
        text.draw(
            pixmap,
            &shaped,
            state.fade_rgba(state.surfaces.muted, now),
            canvas.px(x),
            canvas.px(top),
            Some(clip),
        );
        return;
    }

    let shaped = text.shape(&state.query, size, Weight::MEDIUM);
    let top = field.center_y() - shaped.height / (2.0 * canvas.scale);
    // the drawn caret and the rectangle the IME is told about are one geometry
    let (caret, shift) = caret_box(state, text);

    // A keyboard selection sits under the glyphs at the accent's 35%, clipped to
    // the same text area.
    if let Some((start, end)) = state.selection() {
        let from = text.shape(&state.query[..start], size, Weight::MEDIUM);
        let to = text.shape(&state.query[..end], size, Weight::MEDIUM);
        let left = (x + from.width / canvas.scale - shift).max(x);
        let right = (x + to.width / canvas.scale - shift).min(x + width);
        if right > left {
            canvas.fill_rect(
                pixmap,
                Rect {
                    x: left,
                    y: caret.y,
                    w: right - left,
                    h: caret.h,
                },
                state.fade(state.theme.primary, 0.35, now),
            );
        }
    }

    text.draw(
        pixmap,
        &shaped,
        state.fade(fg, 1.0, now),
        canvas.px(x - shift),
        canvas.px(top),
        Some(clip),
    );

    // The preedit is drawn at the caret, never inserted into the query: glyphs
    // over a translucent quad, so a composition stays legible.
    if let Some(preedit) = &state.preedit {
        let shaped = text.shape(preedit, size, Weight::MEDIUM);
        let quad = Rect {
            x: caret.x,
            y: field.center_y() - 14.0,
            // a long composition is cut at the field's edge, not over the toolbar
            w: (x + width - caret.x).clamp(0.0, shaped.width / canvas.scale + 1.0),
            h: 28.0,
        };
        canvas.fill_rect(pixmap, quad, state.fade(fg, 0.25, now));
        text.draw(
            pixmap,
            &shaped,
            state.fade(fg, 1.0, now),
            canvas.px(caret.x),
            canvas.px(top),
            Some(clip),
        );
    }
}

/// The caret's box and how far the query is scrolled so it fits. The one caret
/// geometry, shared with the IME cursor rectangle, so the two cannot drift.
fn caret_box(state: &State, text: &mut TextEngine) -> (Rect, f32) {
    let surface = state.surface;
    let layout = state.appearance.layout;
    let field = Rect::field_at(
        layout.card_x(surface),
        layout.card_top(surface),
        layout.card_w(surface),
    );
    let scale = state.scale_factor();
    let query_size = state.appearance.font.query();
    let before = text.shape(state.before_caret(), query_size * scale, Weight::MEDIUM);
    let area = text_area(field);
    let shift = scroll_for(area.0 + before.width / scale, area);

    // The caret is 1px wide at the *line box* height, not at the font size.
    let line_height = (query_size * crate::ui::text::LINE_HEIGHT).round();
    (
        Rect {
            x: area.0 + before.width / scale - shift,
            y: field.center_y() - line_height / 2.0,
            w: 1.0,
            h: line_height,
        },
        shift,
    )
}

/// The caret as the IME's `set_cursor_rectangle` wants it: surface-local
/// logical coordinates, so it follows the same scroll as the drawn caret.
pub fn caret_rect(state: &State, text: &mut TextEngine) -> (i32, i32, i32, i32) {
    let (caret, _) = caret_box(state, text);
    (
        caret.x.round() as i32,
        caret.y.round() as i32,
        (caret.w.round() as i32).max(1),
        caret.h.round() as i32,
    )
}

/// The caret, drawn in its own pass: it is the only thing that changes on a
/// blink, and a blink must not re-render the whole surface.
pub fn draw_caret(pixmap: &mut Pixmap, state: &State, text: &mut TextEngine, now: Instant) {
    if !state.caret_visible || state.preedit.is_some() {
        return;
    }

    let canvas = Canvas {
        scale: state.scale_factor(),
    };
    let (caret, _) = caret_box(state, text);
    canvas.fill_rect(pixmap, caret, state.fade(state.theme.fg, 1.0, now));
}

pub(super) fn draw_toolbar(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    state: &State,
    text: &mut TextEngine,
    now: Instant,
) {
    let font = state.appearance.font.suggestion() * canvas.scale;
    let (cx, cy, r) = state.appearance.layout.clear_circle(state.surface);
    // A keyword prefix implies a non-empty query, so the ✕ is always drawn when
    // the chip is: the chip sits against the button's left edge.
    let right = cx - r - 6.0;

    if !state.query.is_empty() {
        let circle = Rect {
            x: cx - r,
            y: cy - r,
            w: 2.0 * r,
            h: 2.0 * r,
        };
        let hovered = state.hovered == Some(Hover::Clear);
        canvas.fill_round(
            pixmap,
            circle,
            r,
            state.fade(state.theme.fg, if hovered { 0.18 } else { 0.10 }, now),
        );

        let shaped = text.shape(CLEAR_GLYPH, 12.0 * canvas.scale, Weight::NORMAL);
        // the ×'s ink sits above and left of its line box; these shifts put the
        // centred box back on the circle
        text.draw(
            pixmap,
            &shaped,
            state.fade_rgba(state.surfaces.muted, now),
            canvas.px(circle.x + r) - shaped.width / 2.0 + 0.5,
            canvas.px(circle.center_y()) - shaped.height / 2.0 + canvas.px(1.5),
            None,
        );
    }

    let Some(prefix) = state.keyword_prefix() else {
        return;
    };
    let shaped = text.shape(prefix, font, Weight::BOLD);
    let chip = Rect {
        x: right - shaped.width / canvas.scale - 16.0,
        y: cy - 12.0,
        w: shaped.width / canvas.scale + 16.0,
        h: 24.0,
    };
    canvas.fill_round(
        pixmap,
        chip,
        state.appearance.layout.chip_radius(),
        state.fade(state.theme.primary, 0.16, now),
    );
    text.draw(
        pixmap,
        &shaped,
        state.fade(state.theme.primary, 1.0, now),
        canvas.px(chip.x) + canvas.px(8.0),
        canvas.px(chip.center_y()) - shaped.height / 2.0,
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_query_scrolls_so_the_caret_stays_in_the_field() {
        let field = Rect {
            x: 100.0,
            y: 10.0,
            w: 532.0,
            h: 52.0,
        };
        let area = text_area(field);

        // a caret inside the area does not scroll the query
        assert_eq!(scroll_for(area.0 + 10.0, area), 0.0);
        // one past the right edge scrolls by exactly the overflow, leaving the
        // caret two logical pixels inside the area
        let caret = area.0 + area.1 + 5.0;
        assert_eq!(caret - scroll_for(caret, area), area.0 + area.1 - 2.0);
    }
}
