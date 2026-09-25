use std::time::Instant;

use cosmic_text::Weight;
use tiny_skia::Pixmap;

use crate::app::State;
use crate::ui::icons::IconCache;
use crate::ui::text::TextEngine;

use super::canvas::{Canvas, ENTER_GLYPH, Rect};

/// The highlight shared by the result list and the action panel: a primary tint
/// for the selected or hovered row, plus the accent bar on the selected one.
fn draw_row_chrome(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    rect: Rect,
    selected: bool,
    hovered: bool,
    state: &State,
    now: Instant,
) {
    let appearance = &state.appearance;
    let layout = appearance.layout;
    let background = match (selected, hovered) {
        (true, _) => Some(state.fade_rgba(state.surfaces.selection, now)),
        (false, true) => Some(state.fade_rgba(state.surfaces.hover, now)),
        (false, false) => None,
    };
    if let Some(background) = background {
        canvas.fill_round(pixmap, rect, layout.row_radius(), background);
    }

    if selected {
        canvas.fill_round(
            pixmap,
            Rect {
                x: rect.x + 3.0,
                y: rect.center_y() - layout.accent_height / 2.0,
                w: layout.accent_width,
                h: layout.accent_height,
            },
            layout.accent_radius(),
            state.fade_rgba(state.surfaces.accent, now),
        );
    }
}

/// One row to draw, filled from a result row or a panel action: the two item
/// types differ only in the fields kept here, so the whole body draw is shared.
pub(super) struct RowBody<'a> {
    pub rect: Rect,
    pub selected: bool,
    pub hovered: bool,
    pub icon: Option<&'a str>,
    pub icon_tint: Option<[u8; 3]>,
    pub title: &'a str,
    pub bold: bool,
    pub summary: Option<&'a str>,
    pub badge: Option<&'a str>,
    pub default_marker: bool,
}

/// A row's chrome, icon, labels column and tail marks: the result list and the
/// action panel draw rows through this one path.
pub(super) fn draw_row(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    state: &State,
    text: &mut TextEngine,
    icons: &mut IconCache,
    now: Instant,
    body: &RowBody<'_>,
) {
    let theme = state.theme;
    let font = state.appearance.font;
    let rect = body.rect;
    draw_row_chrome(
        canvas,
        pixmap,
        rect,
        body.selected,
        body.hovered,
        state,
        now,
    );

    // A constant icon x, so the accent bar never shifts it.
    let icon_x = rect.x + 11.0;
    if let Some(path) = body.icon {
        let pos = (
            canvas.px(icon_x),
            canvas.px(rect.center_y() - font.icon() / 2.0),
        );
        let size = (font.icon() * canvas.scale).round() as u32;
        match body.icon_tint {
            Some(color) => {
                icons.draw_tinted(pixmap, path, pos, size, state.entrance(now), color);
            }
            None => icons.draw(pixmap, path, pos.0, pos.1, size, state.entrance(now)),
        }
    }

    let labels_x = icon_x + font.icon() + 12.0;
    // The selected row's ↵ hint, the pinned badge and the default marker are
    // part of the layout: the labels must leave room for all of them.
    let enter = body
        .selected
        .then(|| text.shape(ENTER_GLYPH, 13.0 * canvas.scale, Weight::NORMAL));
    let enter_w = enter
        .as_ref()
        .map_or(0.0, |shaped| shaped.width / canvas.scale + 12.0);
    let badge_w = body.badge.map_or(0.0, |_| font.badge() + 8.0);
    let marker_w = if body.default_marker { 12.0 } else { 0.0 };
    let labels_max = (rect.right() - 10.0 - labels_x - enter_w - badge_w - marker_w).max(0.0);

    let title = text.fit(
        body.title,
        font.title() * canvas.scale,
        if body.bold {
            Weight::BOLD
        } else {
            Weight::NORMAL
        },
        labels_max * canvas.scale,
    );
    let title_h = title.height / canvas.scale;
    let summary = body.summary.map(|summary| {
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

    if body.default_marker {
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

    if let Some(path) = body.badge {
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
