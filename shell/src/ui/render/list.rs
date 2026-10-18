use std::time::Instant;

use tiny_skia::Pixmap;

use crate::ui::geom;
use crate::ui::icons::IconCache;
use crate::ui::text::TextEngine;
use crate::ui::view::{Body, Frame};

use super::canvas::{Canvas, Rect};
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
    let (left, width) = layout.row_band(frame.surface);
    let top = layout.rows_top(frame.surface);

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
        let body = RowBody {
            rect: Rect {
                x: left,
                y: top + (index - list.first) as f32 * geom::ROW_H + offset,
                w: width,
                h: geom::ROW_H,
            },
            alpha,
            selected: index == list.selected,
            hovered: list.hovered == Some(index),
            icon: row.icon.as_deref(),
            icon_tint: None,
            title: &row.title,
            bold: true,
            summary: row.summary.as_deref(),
            default_marker: false,
        };
        draw_row(canvas, pixmap, frame, text, icons, &body);
    }
}
