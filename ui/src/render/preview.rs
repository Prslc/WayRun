use std::rc::Rc;

use cosmic_text::Weight;
use wayrun_core::wire::IconShape;

use crate::geom::Rect;
use crate::icons::IconDraw;
use crate::text::{LINE_HEIGHT, Shaped, TextEngine, TextStyle};
use crate::theme::tint;
use crate::view::{Frame, PreviewView};

use super::DrawCtx;

/// The pane sits inside the body, leaving a small card margin around it.
const PANE_INSET: f32 = 4.0;
pub(super) const PANE_PAD: f32 = 10.0;

/// The split view's right column: a tonal pane holding an image, plain text or
/// rendered Markdown.
pub(super) fn draw_preview(ctx: &mut DrawCtx, preview: &PreviewView<'_>) {
    match preview {
        PreviewView::Image { path } => draw_image(ctx, path),
        PreviewView::Text { text: body } => {
            let pane = text_pane(ctx.frame);
            if pane.w <= 0.0 || pane.h <= 0.0 {
                return;
            }
            fill_pane(ctx, pane);
            draw_text(ctx, pane, body);
        }
        PreviewView::Markdown { text: body } => {
            let pane = text_pane(ctx.frame);
            if pane.w <= 0.0 || pane.h <= 0.0 {
                return;
            }
            fill_pane(ctx, pane);
            super::markdown::draw_markdown(ctx, pane, body);
        }
    }
}

/// The preview's own tokens: the tonal pane and the plain-text ink.
struct Style {
    pane: [u8; 4],
    text: [u8; 4],
}

impl Style {
    fn of(frame: &Frame<'_>) -> Self {
        Self {
            pane: frame.surfaces.preview,
            text: tint(frame.theme.fg, 0.85),
        }
    }
}

fn fill_pane(ctx: &mut DrawCtx, pane: Rect) {
    let frame = ctx.frame;
    ctx.canvas.fill_round(
        ctx.pixmap,
        pane,
        frame.layout.row_radius(),
        frame.fade_rgba(Style::of(frame).pane),
    );
}

fn text_pane(frame: &Frame<'_>) -> Rect {
    let layout = frame.layout;
    let (left, width) = layout.preview_band(frame.surface);
    Rect {
        x: left,
        y: layout.rows_top(frame.surface) + PANE_INSET,
        w: width,
        h: (layout.body_h(frame.content_h) - PANE_INSET * 2.0).max(0.0),
    }
}

/// A pane fitted to the image's aspect, centered in the body. The square render
/// is centered on that pane; its transparent letterbox never paints.
fn draw_image(ctx: &mut DrawCtx, path: &str) {
    if path.is_empty() {
        return;
    }
    let frame = ctx.frame;
    let canvas = ctx.canvas;
    let layout = frame.layout;
    let (band_left, band_w) = layout.preview_band(frame.surface);
    let body_top = layout.rows_top(frame.surface);
    let body_h = layout.body_h(frame.content_h);
    let max_w = (band_w - PANE_PAD * 2.0).max(1.0);
    let max_h = (body_h - PANE_INSET * 2.0 - PANE_PAD * 2.0).max(1.0);
    let (image_w, image_h) = match ctx.icons.aspect(path) {
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
    fill_pane(ctx, pane);

    // The contain-fit square is centered on the fitted image, so the visible
    // pixels land inside the pane while the letterbox falls outside it.
    let square = image_w.max(image_h);
    let square_x = pane.x + (pane.w - square) / 2.0;
    let square_y = pane.y + (pane.h - square) / 2.0;
    let size = (square * canvas.scale).round().max(1.0) as u32;
    ctx.icons.draw(
        ctx.pixmap,
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
fn draw_text(ctx: &mut DrawCtx, pane: Rect, body: &str) {
    let frame = ctx.frame;
    let canvas = ctx.canvas;
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
    let color = frame.fade_rgba(Style::of(frame).text);
    let lines = wrap(ctx.text, body, size, width * canvas.scale, max_lines);
    let x = pane.x + PANE_PAD;
    let mut y = pane.y + PANE_PAD;
    for line in &lines {
        ctx.text.draw(
            ctx.pixmap,
            line,
            color,
            canvas.px(x),
            canvas.px(y),
            Some(clip),
        );
        y += line_h;
    }
}

/// Greedy word wrap into `max_lines` shaped lines; a token with no break
/// opportunity (CJK, a long URL) is split by character, not elided.
fn wrap(
    engine: &mut TextEngine,
    body: &str,
    size: f32,
    max_w: f32,
    max_lines: usize,
) -> Vec<Rc<Shaped>> {
    let style = TextStyle::normal(Weight::NORMAL);
    let mut lines = Vec::new();
    for paragraph in body.split('\n') {
        if lines.len() >= max_lines {
            break;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if !current.is_empty() {
                let candidate = format!("{current} {word}");
                if engine.shape(&candidate, size, Weight::NORMAL).width <= max_w {
                    current = candidate;
                    continue;
                }
                lines.push(engine.fit(&current, size, Weight::NORMAL, max_w));
                if lines.len() >= max_lines {
                    return lines;
                }
                current.clear();
            }

            let chunks = if engine.shape(word, size, Weight::NORMAL).width > max_w {
                split_wide(engine, word, size, style, max_w, max_lines - lines.len())
            } else {
                vec![word.to_string()]
            };
            if chunks.is_empty() {
                break;
            }
            let last = chunks.len() - 1;
            for chunk in &chunks[..last] {
                lines.push(engine.shape(chunk, size, Weight::NORMAL));
                if lines.len() >= max_lines {
                    return lines;
                }
            }
            current = chunks[last].clone();
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

/// Split one over-long token into chunks that each fit `max_w`, on char
/// boundaries; a single character wider than the column is kept alone.
pub(super) fn split_wide(
    engine: &mut TextEngine,
    text: &str,
    size: f32,
    style: TextStyle,
    max_w: f32,
    budget: usize,
) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() && chunks.len() < budget {
        let end = longest_prefix(engine, rest, size, style, max_w);
        chunks.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    chunks
}

/// The byte end of the longest prefix that fits `max_w`; probing doubles from
/// a few chars and then binary-searches, so only the cut chunk is shaped.
fn longest_prefix(
    engine: &mut TextEngine,
    text: &str,
    size: f32,
    style: TextStyle,
    max_w: f32,
) -> usize {
    let mut low = 0usize;
    let mut high;
    let mut probe = 8usize;
    loop {
        let end = char_end(text, probe);
        if engine.shape_styled(&text[..end], size, style).width <= max_w {
            if end == text.len() {
                return end;
            }
            low = probe;
            probe *= 2;
        } else {
            high = probe;
            break;
        }
    }
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        let end = char_end(text, mid);
        if engine.shape_styled(&text[..end], size, style).width <= max_w {
            low = mid;
        } else {
            high = mid;
        }
    }
    char_end(text, low.max(1))
}

fn char_end(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(index, _)| index)
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
    fn a_long_cjk_run_is_split_instead_of_elided() {
        let mut text = engine();
        let lines = wrap(
            &mut text,
            "中文中文中文中文中文中文中文中文",
            14.0,
            40.0,
            10,
        );
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| line.width <= 40.5));
    }

    #[test]
    fn a_wide_run_is_chunked_within_the_line_budget() {
        let mut text = engine();
        let chunks = split_wide(
            &mut text,
            &"中".repeat(200),
            14.0,
            TextStyle::normal(Weight::NORMAL),
            40.0,
            5,
        );
        assert_eq!(chunks.len(), 5);
        assert!(chunks.iter().all(|chunk| !chunk.is_empty()));
    }

    #[test]
    fn an_image_fits_its_aspect_inside_the_pane() {
        let (width, height) = fit_image(1920, 1080, 300.0, 400.0);
        assert!((width / height - 16.0 / 9.0).abs() < 0.001);
        assert!(width <= 300.0 + f32::EPSILON);
        assert!(height <= 400.0 + f32::EPSILON);
    }
}
