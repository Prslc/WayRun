use std::time::Instant;

use cosmic_text::Weight;
use tiny_skia::Pixmap;

use crate::app::{Hover, State, marked_entry};
use crate::ui::geom;
use crate::ui::icons::IconCache;
use crate::ui::text::TextEngine;

use super::canvas::{Canvas, Rect};
use super::{RowBody, draw_row};

/// The action panel (Shift+Enter): the parent row's title as a header, then the
/// row's secondary commands as list rows in the same fixed window.
pub(super) fn draw_actions(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    surface: (u32, u32),
    state: &State,
    text: &mut TextEngine,
    icons: &mut IconCache,
    now: Instant,
) {
    let Some(menu) = &state.menu else {
        return;
    };
    let theme = state.theme;
    let layout = state.appearance.layout;
    let font = state.appearance.font;
    let left = layout.card_x(surface) + geom::PAD;
    let width = layout.card_w(surface) - 2.0 * geom::PAD;

    if let Some(title) = state.menu_parent_title() {
        let size = font.suggestion() * canvas.scale;
        let shaped = text.fit(title, size, Weight::BOLD, width * canvas.scale);
        let y = layout.rows_top(surface);
        let clip = [
            canvas.px(left),
            canvas.px(y),
            canvas.px(width),
            canvas.px(layout.panel_header_h()),
        ];
        text.draw(
            pixmap,
            &shaped,
            state.fade(theme.primary, 0.9, now),
            canvas.px(left),
            canvas.px(y + (layout.panel_header_h() - shaped.height / canvas.scale) / 2.0),
            Some(clip),
        );
    }

    // The dot marks the entry the row's Enter runs: the remembered default, or
    // the row's own command while none is remembered.
    let marked = state.rows.get(menu.parent).and_then(marked_entry);
    let top = layout.actions_top(surface);
    for (index, action) in menu
        .actions
        .iter()
        .enumerate()
        .skip(menu.first)
        .take(layout.max_rows)
    {
        let body = RowBody {
            rect: Rect {
                x: left,
                y: top + (index - menu.first) as f32 * geom::ROW_H,
                w: width,
                h: geom::ROW_H,
            },
            selected: index == menu.selected,
            hovered: state.hovered == Some(Hover::Action(index)),
            icon: action.icon.as_deref(),
            icon_tint: Some(theme.fg),
            title: &action.title,
            bold: false,
            summary: None,
            badge: None,
            default_marker: marked == Some(index),
        };
        draw_row(canvas, pixmap, state, text, icons, now, &body);
    }
}
