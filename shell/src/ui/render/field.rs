use std::sync::OnceLock;

use cosmic_text::Weight;
use rust_i18n::t;
use tiny_skia::Pixmap;

use crate::ui::text::TextEngine;
use crate::ui::view::Frame;

use super::canvas::{Canvas, Rect, TEXT_INSET};

pub(super) fn draw_magnifier(canvas: &Canvas, pixmap: &mut Pixmap, field: Rect, frame: &Frame<'_>) {
    // A 2px circle of radius 6.2 at (9, 9) with a handle to (19, 19) in a 22×22
    // box, drawn rather than loaded.
    let x = field.x + 14.0;
    let y = field.center_y() - 11.0;
    let color = frame.fade_rgba(frame.surfaces.muted);

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
    (field.x + TEXT_INSET, field.w - TEXT_INSET - 12.0)
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
    frame: &Frame<'_>,
    text: &mut TextEngine,
) {
    let area = text_area(field);
    let (x, width) = area;
    let size = frame.font.query() * canvas.scale;
    let fg = frame.theme.fg;
    let clip = [
        canvas.px(x),
        canvas.px(field.y),
        canvas.px(width),
        canvas.px(field.h),
    ];

    let placeholder = frame.field.query.is_empty() && frame.field.preedit.is_none();
    if placeholder {
        let shaped = text.shape(placeholder_text(), size, Weight::MEDIUM);
        let top = field.center_y() - shaped.height / (2.0 * canvas.scale);
        text.draw(
            pixmap,
            &shaped,
            frame.fade_rgba(frame.surfaces.muted),
            canvas.px(x),
            canvas.px(top),
            Some(clip),
        );
        return;
    }

    let query = frame.field.query;
    let shaped = text.shape(query, size, Weight::MEDIUM);
    let top = field.center_y() - shaped.height / (2.0 * canvas.scale);
    // the drawn caret and the rectangle the IME is told about are one geometry
    let (caret, shift) = caret_box(frame, text);

    // A keyboard selection sits under the glyphs at the accent's 35%, clipped to
    // the same text area.
    if let Some((start, end)) = frame.field.selection {
        let from = text.shape(&query[..start], size, Weight::MEDIUM);
        let to = text.shape(&query[..end], size, Weight::MEDIUM);
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
                frame.fade(frame.theme.primary, 0.35),
            );
        }
    }

    text.draw(
        pixmap,
        &shaped,
        frame.fade(fg, 1.0),
        canvas.px(x - shift),
        canvas.px(top),
        Some(clip),
    );

    // The preedit is drawn at the caret, never inserted into the query: glyphs
    // over a translucent quad, so a composition stays legible.
    if let Some(preedit) = frame.field.preedit {
        let shaped = text.shape(preedit, size, Weight::MEDIUM);
        let quad = Rect {
            x: caret.x,
            y: field.center_y() - 14.0,
            // a long composition is cut at the field's edge, not over the toolbar
            w: (x + width - caret.x).clamp(0.0, shaped.width / canvas.scale + 1.0),
            h: 28.0,
        };
        canvas.fill_rect(pixmap, quad, frame.fade(fg, 0.25));
        text.draw(
            pixmap,
            &shaped,
            frame.fade(fg, 1.0),
            canvas.px(caret.x),
            canvas.px(top),
            Some(clip),
        );
    }
}

/// The caret's box and how far the query is scrolled so it fits. The one caret
/// geometry, shared with the IME cursor rectangle, so the two cannot drift.
fn caret_box(frame: &Frame<'_>, text: &mut TextEngine) -> (Rect, f32) {
    let surface = frame.surface;
    let layout = frame.layout;
    let field = Rect::field_at(
        layout.card_x(surface),
        layout.card_top(surface),
        layout.card_w(surface),
    );
    let scale = frame.scale;
    let query_size = frame.font.query();
    let before = text.shape(
        &frame.field.query[..frame.field.caret],
        query_size * scale,
        Weight::MEDIUM,
    );
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
pub fn caret_rect(frame: &Frame<'_>, text: &mut TextEngine) -> (i32, i32, i32, i32) {
    let (caret, _) = caret_box(frame, text);
    (
        caret.x.round() as i32,
        caret.y.round() as i32,
        (caret.w.round() as i32).max(1),
        caret.h.round() as i32,
    )
}

/// The caret, drawn in its own pass: it is the only thing that changes on a
/// blink, and a blink must not re-render the whole surface.
pub fn draw_caret(pixmap: &mut Pixmap, frame: &Frame<'_>, text: &mut TextEngine) {
    if !frame.field.caret_visible || frame.field.preedit.is_some() {
        return;
    }

    let canvas = Canvas { scale: frame.scale };
    let (caret, _) = caret_box(frame, text);
    canvas.fill_rect(pixmap, caret, frame.fade(frame.theme.fg, 1.0));
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
