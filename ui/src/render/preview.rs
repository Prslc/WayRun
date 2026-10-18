use tiny_skia::Pixmap;

use crate::icons::IconCache;
use crate::view::{Frame, PreviewView};

use super::canvas::Canvas;

/// The split view's right pane: a square, contain-fitted image aligned with the
/// first row; its transparent letterbox shows the card, not a pale box.
pub(super) fn draw_preview(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    frame: &Frame<'_>,
    rows: usize,
    preview: &PreviewView<'_>,
    icons: &mut IconCache,
) {
    let layout = frame.layout;
    let (band_left, band_w) = layout.preview_band(frame.surface);
    let top = layout.rows_top(frame.surface);
    let height = layout.split_h(rows);
    let side = band_w.min(height);
    if side <= 0.0 {
        return;
    }
    let left = band_left + (band_w - side) / 2.0;

    let PreviewView::Image { path } = preview;
    if path.is_empty() {
        return;
    }
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
