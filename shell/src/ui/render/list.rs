use std::time::Instant;

use cosmic_text::Weight;
use tiny_skia::Pixmap;

use crate::app::{Hover, State, effective_action};
use crate::ui::geom;
use crate::ui::icons::IconCache;
use crate::ui::text::TextEngine;

use super::canvas::{Canvas, ENTER_GLYPH, Rect};

pub(super) fn draw_list(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    surface: (u32, u32),
    state: &State,
    text: &mut TextEngine,
    icons: &mut IconCache,
    now: Instant,
) {
    let theme = state.theme;
    let appearance = &state.appearance;
    let layout = appearance.layout;
    let font = appearance.font;
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
        let y = top + (index - state.first) as f32 * geom::ROW_H;
        let rect = Rect {
            x: left,
            y,
            w: width,
            h: geom::ROW_H,
        };

        let selected = index == state.selected;
        let hovered = state.hovered == Some(Hover::Row(index));
        super::draw_row_chrome(canvas, pixmap, rect, selected, hovered, state, now);

        // A constant icon x, so the accent bar never shifts it.
        let icon_x = rect.x + 11.0;
        if let Some(path) = row.icon.as_deref() {
            icons.draw(
                pixmap,
                path,
                canvas.px(icon_x),
                canvas.px(rect.center_y() - font.icon() / 2.0),
                (font.icon() * canvas.scale).round() as u32,
                state.entrance(now),
            );
        }

        let labels_x = icon_x + font.icon() + 12.0;
        // The selected row's ↵ hint, the pinned badge and the default marker are
        // part of the layout: the labels must leave room for all of them.
        let enter = selected.then(|| text.shape(ENTER_GLYPH, 13.0 * canvas.scale, Weight::NORMAL));
        let enter_w = enter
            .as_ref()
            .map_or(0.0, |shaped| shaped.width / canvas.scale + 12.0);
        let badge_w = row.badge.as_deref().map_or(0.0, |_| font.badge() + 8.0);
        // The same dot the panel puts on the action, so a row whose Enter runs
        // something else is marked without opening the panel.
        let marker = effective_action(row).is_some();
        let marker_w = if marker { 12.0 } else { 0.0 };
        let labels_max = (rect.right() - 10.0 - labels_x - enter_w - badge_w - marker_w).max(0.0);

        let title = text.fit(
            &row.title,
            font.title() * canvas.scale,
            Weight::BOLD,
            labels_max * canvas.scale,
        );
        let title_h = title.height / canvas.scale;
        let summary = row.summary.as_deref().map(|summary| {
            text.fit(
                summary,
                font.summary() * canvas.scale,
                Weight::NORMAL,
                labels_max * canvas.scale,
            )
        });
        let summary_h = summary
            .as_ref()
            .map_or(0.0, |shaped| shaped.height / canvas.scale);

        // The labels column holds title + 2px + summary, centred in the row.
        let block = title_h
            + if summary.is_some() {
                2.0 + summary_h
            } else {
                0.0
            };
        let labels_top = rect.center_y() - block / 2.0;
        let clip = [
            canvas.px(labels_x),
            canvas.px(rect.y),
            canvas.px(labels_max),
            canvas.px(rect.h),
        ];

        text.draw(
            pixmap,
            &title,
            state.fade(theme.fg, 1.0, now),
            canvas.px(labels_x),
            canvas.px(labels_top),
            Some(clip),
        );
        if let Some(summary) = &summary {
            text.draw(
                pixmap,
                summary,
                state.fade_rgba(state.surfaces.summary, now),
                canvas.px(labels_x),
                canvas.px(labels_top + title_h + 2.0),
                Some(clip),
            );
        }

        if let Some(check) = &enter {
            text.draw(
                pixmap,
                check,
                state.fade(theme.primary, state.muted_alpha(), now),
                canvas.px(rect.right() - 10.0) - check.width,
                canvas.px(rect.center_y()) - check.height / 2.0,
                None,
            );
        }

        if marker {
            let cx = rect.right() - 10.0 - enter_w - badge_w - 6.0;
            canvas.fill_round(
                pixmap,
                Rect {
                    x: cx - 3.0,
                    y: rect.center_y() - 3.0,
                    w: 6.0,
                    h: 6.0,
                },
                3.0,
                state.fade(theme.primary, 1.0, now),
            );
        }

        if let Some(path) = row.badge.as_deref() {
            let x = rect.right() - 10.0 - enter_w - font.badge();
            icons.draw_tinted(
                pixmap,
                path,
                (
                    canvas.px(x),
                    canvas.px(rect.center_y() - font.badge() / 2.0),
                ),
                (font.badge() * canvas.scale).round() as u32,
                state.entrance(now),
                theme.primary,
            );
        }
    }
}
