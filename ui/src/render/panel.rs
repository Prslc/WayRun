use tiny_skia::Pixmap;
use wayrun_core::wire::IconShape;

use crate::icons::IconCache;
use crate::text::TextEngine;
use crate::view::{Body, Frame};

use super::canvas::Canvas;
use super::{RowBody, draw_row};

/// The action panel (Shift+Enter): the row's secondary commands in the list
/// band. The parent row or group title is drawn by the footer.
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

    // The dot marks the entry the row's Enter runs: the remembered default, or
    // the row's own command while none is remembered.
    let layout = frame.layout;
    for (index, action) in panel
        .actions
        .iter()
        .enumerate()
        .skip(panel.first)
        .take(layout.max_rows)
    {
        let body = RowBody {
            rect: layout.row_rect(frame.surface, false, index, panel.first),
            selected: index == panel.selected,
            hovered: panel.hovered == Some(index),
            icon: action.icon.as_deref(),
            icon_tint: Some(theme.fg),
            icon_shape: IconShape::Square,
            title: &action.title,
            bold: false,
            summary: None,
            default_marker: panel.marked == Some(index),
            expand: !action.children.is_empty(),
            alpha: 1.0,
        };
        draw_row(canvas, pixmap, frame, text, icons, &body);
    }
}
