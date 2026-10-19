use std::rc::Rc;

use cosmic_text::Weight;
use tiny_skia::Pixmap;
use wayrun_core::wire::IconShape;

use crate::geom::PreviewKind;
use crate::icons::{IconCache, IconDraw};
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
    match preview {
        PreviewView::Image { path } => draw_image(canvas, pixmap, frame, rows, path, icons),
        PreviewView::Text { text: body } => {
            let pane = text_pane(frame, rows);
            if pane.w <= 0.0 || pane.h <= 0.0 {
                return;
            }
            fill_pane(canvas, pixmap, frame, pane);
            draw_text(canvas, pixmap, frame, pane, body, text);
        }
    }
}

fn fill_pane(canvas: &Canvas, pixmap: &mut Pixmap, frame: &Frame<'_>, pane: Rect) {
    canvas.fill_round(
        pixmap,
        pane,
        frame.layout.row_radius(),
        frame.fade_rgba(frame.surfaces.preview),
    );
}

fn text_pane(frame: &Frame<'_>, rows: usize) -> Rect {
    let layout = frame.layout;
    let (left, width) = layout.preview_band(frame.surface);
    Rect {
        x: left,
        y: layout.rows_top(frame.surface) + PANE_INSET,
        w: width,
        h: (layout.split_h(rows, PreviewKind::Text) - PANE_INSET * 2.0).max(0.0),
    }
}

/// A pane fitted to the image's aspect, centered in the body. The square render
/// is centered on that pane; its transparent letterbox never paints.
fn draw_image(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    frame: &Frame<'_>,
    rows: usize,
    path: &str,
    icons: &mut IconCache,
) {
    if path.is_empty() {
        return;
    }
    let layout = frame.layout;
    let (band_left, band_w) = layout.preview_band(frame.surface);
    let body_top = layout.rows_top(frame.surface);
    let body_h = layout.split_h(rows, PreviewKind::Image);
    let max_w = (band_w - PANE_PAD * 2.0).max(1.0);
    let max_h = (body_h - PANE_INSET * 2.0 - PANE_PAD * 2.0).max(1.0);
    let (image_w, image_h) = match icons.aspect(path) {
        Some((width, height)) if width > 0 && height > 0 => fit_image(width, height, max_w, max_h),
        _ => {
            let side = max_w.min(max_h);
            (side, side)
        }
    };

    let pane = Rect {
        x: band_left,
        y: body_top + (body_h - image_h - PANE_PAD * 2.0) / 2.0,
        w: band_w,
        h: image_h + PANE_PAD * 2.0,
    };
    if pane.w <= 0.0 || pane.h <= 0.0 {
        return;
    }
    fill_pane(canvas, pixmap, frame, pane);

    // The contain-fit square is centered on the fitted image, so the visible
    // pixels land inside the pane while the letterbox falls outside it.
    let square = image_w.max(image_h);
    let square_x = pane.x + (pane.w - square) / 2.0;
    let square_y = pane.y + (pane.h - square) / 2.0;
    let size = (square * canvas.scale).round().max(1.0) as u32;
    icons.draw(
        pixmap,
        IconDraw {
            path,
            x: canvas.px(square_x),
            y: canvas.px(square_y),
            size,
            opacity: frame.opacity,
            shape: IconShape::Square,
        },
    );
}

fn fit_image(width: u32, height: u32, max_w: f32, max_h: f32) -> (f32, f32) {
    let scale = (max_w / width as f32).min(max_h / height as f32);
    (
        (width as f32 * scale).max(1.0),
        (height as f32 * scale).max(1.0),
    )
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

    #[test]
    fn an_image_fits_its_aspect_inside_the_pane() {
        let (width, height) = fit_image(1920, 1080, 300.0, 400.0);
        assert!((width / height - 16.0 / 9.0).abs() < 0.001);
        assert!(width <= 300.0 + f32::EPSILON);
        assert!(height <= 400.0 + f32::EPSILON);
    }
}
