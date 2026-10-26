use cosmic_text::Weight;
use tiny_skia::Pixmap;
use wayrun_core::wire::IconShape;

use crate::geom;
use crate::icons::IconCache;
use crate::text::TextEngine;
use crate::view::{Body, Frame};

use super::canvas::{Canvas, Rect};
use super::{RowBody, draw_row};

/// The action panel (Shift+Enter): the parent row's title as a header, then the
/// row's secondary commands as list rows in the same fixed window.
pub(super) fn draw_actions(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    frame: &Frame<'_>,
    text: &mut TextEngine,
    icons: &mut IconCache,
) {
    let Body::Panel(panel) = &frame.body else {
        return;
    };
    let theme = frame.theme;
    let layout = frame.layout;
    let font = frame.font;
    let (left, width) = layout.row_band(frame.surface);

    if let Some(title) = panel.parent_title {
        let size = font.suggestion() * canvas.scale;
        let shaped = text.fit(title, size, Weight::BOLD, width * canvas.scale);
        let y = layout.rows_top(frame.surface);
        let clip = [
            canvas.px(left),
            canvas.px(y),
            canvas.px(width),
            canvas.px(layout.panel_header_h),
        ];
        text.draw(
            pixmap,
            &shaped,
            frame.fade(theme.primary, 0.9),
            canvas.px(left),
            canvas.px(y + (layout.panel_header_h - shaped.height / canvas.scale) / 2.0),
            Some(clip),
        );
    }

    // The dot marks the entry the row's Enter runs: the remembered default, or
    // the row's own command while none is remembered.
    let top = layout.actions_top(frame.surface);
    for (index, action) in panel
        .actions
        .iter()
        .enumerate()
        .skip(panel.first)
        .take(layout.max_rows)
    {
        let body = RowBody {
            rect: Rect {
                x: left,
                y: top + (index - panel.first) as f32 * geom::ROW_H,
                w: width,
                h: geom::ROW_H,
            },
            selected: index == panel.selected,
            hovered: panel.hovered == Some(index),
            icon: action.icon.as_deref(),
            icon_tint: Some(theme.fg),
            icon_shape: IconShape::Square,
            title: &action.title,
            bold: false,
            summary: None,
            default_marker: panel.marked == Some(index),
            alpha: 1.0,
        };
        draw_row(canvas, pixmap, frame, text, icons, &body);
    }
}
