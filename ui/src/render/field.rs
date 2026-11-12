use std::sync::OnceLock;

use cosmic_text::Weight;
use rust_i18n::t;
use tiny_skia::Pixmap;

use crate::text::{TextEngine, line_height};
use crate::theme::tint;
use crate::view::Frame;

use crate::geom::Rect;

use super::DrawCtx;
use super::canvas::Canvas;

// The magnifier's box, the gap after it and the input inset.
const MAGNIFIER_BOX: f32 = 22.0;
const MAGNIFIER_GAP: f32 = 12.0;
const INPUT_PAD: f32 = 8.0;

/// The field's inner insets (the card's padding and the magnifier column); the
/// IME's caret rectangle shares it.
const TEXT_INSET: f32 = crate::geom::PAD + MAGNIFIER_BOX + MAGNIFIER_GAP + INPUT_PAD;

/// The field's own tokens: the muted guide, the glyphs and the two quads.
struct Style {
    muted: [u8; 4],
    glyph: [u8; 4],
    selection_quad: [u8; 4],
    preedit_quad: [u8; 4],
}

impl Style {
    fn of(frame: &Frame<'_>) -> Self {
        let fg = frame.theme.fg;
        Self {
            muted: frame.surfaces.muted,
            glyph: tint(fg, 1.0),
            selection_quad: tint(frame.theme.primary, 0.35),
            preedit_quad: tint(fg, 0.25),
        }
    }
}

pub(super) fn draw_magnifier(ctx: &mut DrawCtx, field: Rect) {
    // A 2px circle of radius 6.2 at (9, 9) with a handle to (19, 19) in a 22×22
    // box, drawn rather than loaded.
    let canvas = ctx.canvas;
    let x = field.x + 14.0;
    let y = field.center_y() - 11.0;
    let color = ctx.frame.fade_rgba(Style::of(ctx.frame).muted);

    canvas.stroke_circle(ctx.pixmap, (x + 9.0, y + 9.0), 6.2, 2.0, color);
    canvas.stroke_line(
        ctx.pixmap,
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

pub(super) fn draw_query(ctx: &mut DrawCtx, field: Rect) {
    let canvas = ctx.canvas;
    let frame = ctx.frame;
    let area = text_area(field);
    let (x, width) = area;
    let size = frame.font.query() * canvas.scale;
    let style = Style::of(frame);
    let clip = [
        canvas.px(x),
        canvas.px(field.y),
        canvas.px(width),
        canvas.px(field.h),
    ];

    let placeholder = frame.field.query.is_empty() && frame.field.preedit.is_none();
    if placeholder {
        let shaped = ctx.text.shape(placeholder_text(), size, Weight::MEDIUM);
        let top = field.center_y() - canvas.unpx(shaped.height) / 2.0;
        ctx.text.draw(
            ctx.pixmap,
            &shaped,
            frame.fade_rgba(style.muted),
            canvas.px(x),
            canvas.px(top),
            Some(clip),
        );
        return;
    }

    let query = frame.field.query;
    let shaped = ctx.text.shape(query, size, Weight::MEDIUM);
    let top = field.center_y() - canvas.unpx(shaped.height) / 2.0;
    // the drawn caret and the rectangle the IME is told about are one geometry
    let (caret, shift) = caret_box(frame, ctx.text);

    // A keyboard selection sits under the glyphs at the accent's 35%, clipped to
    // the same text area.
    if let Some((start, end)) = frame.field.selection {
        let from = ctx.text.shape(&query[..start], size, Weight::MEDIUM);
        let to = ctx.text.shape(&query[..end], size, Weight::MEDIUM);
        let left = (x + canvas.unpx(from.width) - shift).max(x);
        let right = (x + canvas.unpx(to.width) - shift).min(x + width);
        if right > left {
            canvas.fill_rect(
                ctx.pixmap,
                Rect {
                    x: left,
                    y: caret.y,
                    w: right - left,
                    h: caret.h,
                },
                frame.fade_rgba(style.selection_quad),
            );
        }
    }

    ctx.text.draw(
        ctx.pixmap,
        &shaped,
        frame.fade_rgba(style.glyph),
        canvas.px(x - shift),
        canvas.px(top),
        Some(clip),
    );

    // The preedit is drawn at the caret, never inserted into the query: glyphs
    // over a translucent quad, so a composition stays legible.
    if let Some(preedit) = frame.field.preedit {
        let shaped = ctx.text.shape(preedit, size, Weight::MEDIUM);
        let quad = Rect {
            x: caret.x,
            y: field.center_y() - 14.0,
            // a long composition is cut at the field's edge, not over the toolbar
            w: (x + width - caret.x).clamp(0.0, canvas.unpx(shaped.width) + 1.0),
            h: 28.0,
        };
        canvas.fill_rect(ctx.pixmap, quad, frame.fade_rgba(style.preedit_quad));
        ctx.text.draw(
            ctx.pixmap,
            &shaped,
            frame.fade_rgba(style.glyph),
            canvas.px(caret.x),
            canvas.px(top),
            Some(clip),
        );
    }
}

/// The caret's box and how far the query is scrolled so it fits. The one caret
/// geometry, shared with the IME cursor rectangle, so the two cannot drift.
fn caret_box(frame: &Frame<'_>, text: &mut TextEngine) -> (Rect, f32) {
    let field = frame.layout.field_rect(frame.surface);
    let scale = frame.scale;
    let canvas = Canvas { scale };
    let query_size = frame.font.query();
    let before = text.shape(
        &frame.field.query[..frame.field.caret],
        query_size * scale,
        Weight::MEDIUM,
    );
    let area = text_area(field);
    let shift = scroll_for(area.0 + canvas.unpx(before.width), area);

    // The caret is 1px wide at the *line box* height, not at the font size.
    let line_height = line_height(query_size);
    (
        Rect {
            x: area.0 + canvas.unpx(before.width) - shift,
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
    canvas.fill_rect(pixmap, caret, frame.fade_rgba(Style::of(frame).glyph));
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
