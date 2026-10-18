use std::rc::Rc;

use cosmic_text::Weight;
use tiny_skia::Pixmap;

use crate::geom::PreviewKind;
use crate::icons::IconCache;
use crate::text::{LINE_HEIGHT, Shaped, TextEngine};
use crate::view::{Frame, PreviewView};

use super::canvas::{Canvas, Rect};

/// The pane sits inside the body, leaving a small card margin around it.
const PANE_INSET: f32 = 4.0;
const PANE_PAD: f32 = 10.0;

/// The split view's right column: a tonal pane holding an image or a wrapped
/// plain-text excerpt.
pub(super) fn draw_preview(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    frame: &Frame<'_>,
    rows: usize,
    preview: &PreviewView<'_>,
    text: &mut TextEngine,
    icons: &mut IconCache,
) {
    let kind = preview.kind();
    let pane = pane_rect(frame, rows, kind);
    if pane.w <= 0.0 || pane.h <= 0.0 {
        return;
    }
    canvas.fill_round(
        pixmap,
        pane,
        frame.layout.row_radius(),
        frame.fade_rgba(frame.surfaces.preview),
    );
    match preview {
        PreviewView::Image { path } => draw_image(canvas, pixmap, frame, pane, path, icons),
        PreviewView::Text { text: body } => draw_text(canvas, pixmap, frame, pane, body, text),
    }
}

fn pane_rect(frame: &Frame<'_>, rows: usize, kind: PreviewKind) -> Rect {
    let layout = frame.layout;
    let (left, width) = layout.preview_band(frame.surface);
    Rect {
        x: left,
        y: layout.rows_top(frame.surface) + PANE_INSET,
        w: width,
        h: (layout.split_h(frame.surface, rows, kind) - PANE_INSET * 2.0).max(0.0),
    }
}

/// A square, contain-fitted image at the pane's top.
fn draw_image(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    frame: &Frame<'_>,
    pane: Rect,
    path: &str,
    icons: &mut IconCache,
) {
    let side = (pane.w.min(pane.h) - PANE_PAD * 2.0).max(0.0);
    if side <= 0.0 || path.is_empty() {
        return;
    }
    let left = pane.x + (pane.w - side) / 2.0;
    let top = pane.y + PANE_PAD;
    let size = (side * canvas.scale).round().max(1.0) as u32;
    icons.draw(
        pixmap,
        path,
        canvas.px(left),
        canvas.px(top),
        size,
        frame.opacity,
    );
}

/// Plain text wrapped to the pane, clipped to it; no scrolling yet.
fn draw_text(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    frame: &Frame<'_>,
    pane: Rect,
    body: &str,
    text: &mut TextEngine,
) {
    let width = (pane.w - PANE_PAD * 2.0).max(1.0);
    let height = (pane.h - PANE_PAD * 2.0).max(1.0);
    let size = frame.font.title() * canvas.scale;
    let line_h = (frame.font.title() * LINE_HEIGHT).round();
    if line_h <= 0.0 {
        return;
    }
    let max_lines = (height / line_h).floor().max(1.0) as usize;
    let clip = [
        canvas.px(pane.x + PANE_PAD),
        canvas.px(pane.y + PANE_PAD),
        canvas.px(width),
        canvas.px(height),
    ];
    let color = frame.fade(frame.theme.fg, 0.85);
    let lines = wrap(text, body, size, width * canvas.scale, max_lines);
    let x = pane.x + PANE_PAD;
    let mut y = pane.y + PANE_PAD;
    for line in &lines {
        text.draw(pixmap, line, color, canvas.px(x), canvas.px(y), Some(clip));
        y += line_h;
    }
}

/// Greedy word wrap into at most `max_lines` shaped lines. Whitespace is
/// collapsed; a line wider than the pane is elided by `fit`.
fn wrap(
    engine: &mut TextEngine,
    body: &str,
    size: f32,
    max_w: f32,
    max_lines: usize,
) -> Vec<Rc<Shaped>> {
    let mut lines = Vec::new();
    for paragraph in body.split('\n') {
        if lines.len() >= max_lines {
            break;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            let too_wide = engine.shape(&candidate, size, Weight::NORMAL).width > max_w;
            if too_wide && !current.is_empty() {
                lines.push(engine.fit(&current, size, Weight::NORMAL, max_w));
                if lines.len() >= max_lines {
                    return lines;
                }
                current = word.to_string();
            } else {
                current = candidate;
            }
        }
        if lines.len() >= max_lines {
            break;
        }
        lines.push(if current.is_empty() {
            engine.shape("", size, Weight::NORMAL)
        } else {
            engine.fit(&current, size, Weight::NORMAL, max_w)
        });
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> TextEngine {
        TextEngine::with_family("Source Han Sans CN".to_string())
    }

    #[test]
    fn a_long_line_wraps_and_stops_at_the_line_budget() {
        let mut text = engine();
        let lines = wrap(&mut text, "one two three four five six", 14.0, 60.0, 10);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| line.width <= 60.5));

        let capped = wrap(&mut text, &"word ".repeat(100), 14.0, 60.0, 3);
        assert_eq!(capped.len(), 3);
    }

    #[test]
    fn a_blank_line_keeps_its_line() {
        let mut text = engine();
        assert_eq!(wrap(&mut text, "a\n\nb", 14.0, 100.0, 10).len(), 3);
    }
}
