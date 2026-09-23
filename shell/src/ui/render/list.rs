use std::time::Instant;

use tiny_skia::Pixmap;

use crate::app::{Hover, State, effective_action};
use crate::ui::geom;
use crate::ui::icons::IconCache;
use crate::ui::text::TextEngine;

use super::canvas::{Canvas, Rect};
use super::{RowBody, draw_row};

pub(super) fn draw_list(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    surface: (u32, u32),
    state: &State,
    text: &mut TextEngine,
    icons: &mut IconCache,
    now: Instant,
) {
    let layout = state.appearance.layout;
    let left = layout.card_x(surface) + geom::PAD;
    let width = layout.card_w(surface) - 2.0 * geom::PAD;
    let top = layout.rows_top(surface);

    for (index, row) in state
        .rows
        .iter()
        .enumerate()
        .skip(state.first)
        .take(layout.max_rows)
    {
        let body = RowBody {
            rect: Rect {
                x: left,
                y: top + (index - state.first) as f32 * geom::ROW_H,
                w: width,
                h: geom::ROW_H,
            },
            selected: index == state.selected,
            hovered: state.hovered == Some(Hover::Row(index)),
            icon: row.icon.as_deref(),
            icon_tint: None,
            title: &row.title,
            bold: true,
            summary: row.summary.as_deref(),
            badge: row.badge.as_deref(),
            default_marker: effective_action(row).is_some(),
        };
        draw_row(canvas, pixmap, state, text, icons, now, &body);
    }
}
