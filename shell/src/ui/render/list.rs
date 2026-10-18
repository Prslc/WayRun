use std::time::Instant;

use tiny_skia::Pixmap;

use crate::app::{Hover, State};
use crate::app::{ROW_MOTION_MS, emphasized_decelerate};
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
    let layout = state.frame_layout(now);
    let (left, width) = layout.row_band(surface);
    let top = layout.rows_top(surface);

    for (index, row) in state
        .rows
        .iter()
        .enumerate()
        .skip(state.cursor.first)
        .take(layout.max_rows)
    {
        let motion = state.row_motion.get(index).copied().flatten();
        let (alpha, offset) = match motion {
            Some(motion) => {
                let elapsed = now.saturating_duration_since(motion.started).as_secs_f32() * 1000.0;
                let t = (elapsed / ROW_MOTION_MS as f32).clamp(0.0, 1.0);
                let eased = emphasized_decelerate(t);
                (
                    if motion.enter { eased } else { 1.0 },
                    motion.offset * (1.0 - eased),
                )
            }
            None => (1.0, 0.0),
        };
        let body = RowBody {
            rect: Rect {
                x: left,
                y: top + (index - state.cursor.first) as f32 * geom::ROW_H + offset,
                w: width,
                h: geom::ROW_H,
            },
            alpha,
            selected: index == state.cursor.selected,
            hovered: state.hovered == Some(Hover::Row(index)),
            icon: row.icon.as_deref(),
            icon_tint: None,
            title: &row.title,
            bold: true,
            summary: row.summary.as_deref(),
            default_marker: false,
        };
        draw_row(canvas, pixmap, state, text, icons, now, &body);
    }
}
