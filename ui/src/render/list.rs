use std::time::Instant;

use tiny_skia::Pixmap;

use crate::geom::Rect;
use crate::icons::IconCache;
use crate::text::TextEngine;
use crate::view::{Body, Frame};

use super::canvas::Canvas;
use super::{RowBody, draw_row};

pub(super) fn draw_list(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    frame: &Frame<'_>,
    text: &mut TextEngine,
    icons: &mut IconCache,
    now: Instant,
) {
    let Body::List(list) = &frame.body else {
        return;
    };
    let layout = frame.layout;
    let split = list.preview.is_some();

    for (index, row) in list
        .rows
        .iter()
        .enumerate()
        .skip(list.first)
        .take(layout.max_rows)
    {
        let (alpha, offset) = list
            .motion
            .get(index)
            .copied()
            .flatten()
            .map_or((1.0, 0.0), |motion| motion.sample(now));
        let rect = layout.row_rect(frame.surface, split, index, list.first);
        let body = RowBody {
            rect: Rect {
                y: rect.y + offset,
                ..rect
            },
            alpha,
            selected: index == list.selected,
            hovered: list.hovered == Some(index),
            icon: row.icon.as_deref(),
            icon_tint: None,
            icon_shape: row.icon_shape.unwrap_or_default(),
            title: &row.title,
            bold: true,
            summary: row.summary.as_deref(),
            default_marker: false,
            expand: false,
        };
        draw_row(canvas, pixmap, frame, text, icons, &body);
    }
}
