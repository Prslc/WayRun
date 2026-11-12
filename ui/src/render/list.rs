use crate::view::Body;

use super::{DrawCtx, RowBody, RowWindow, draw_row};

pub(super) fn draw_list(ctx: &mut DrawCtx) {
    let Body::List(list) = &ctx.frame.body else {
        return;
    };
    let rows = list.rows;
    let (first, selected, hovered) = (list.first, list.selected, list.hovered);
    let split = list.preview.is_some();
    let window = RowWindow {
        layout: ctx.frame.layout,
        surface: ctx.frame.surface,
        split,
        first,
        count: rows.len(),
        motion: list.motion,
        now: ctx.now,
    };

    for slot in window.slots() {
        let row = &rows[slot.index];
        let body = RowBody {
            rect: slot.rect,
            alpha: slot.alpha,
            selected: slot.index == selected,
            hovered: hovered == Some(slot.index),
            icon: row.icon.as_deref(),
            icon_tint: None,
            icon_shape: row.icon_shape.unwrap_or_default(),
            title: &row.title,
            bold: true,
            summary: row.summary.as_deref(),
            default_marker: false,
            expand: false,
        };
        draw_row(ctx, &body);
    }
}
