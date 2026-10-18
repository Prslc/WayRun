use cosmic_text::Weight;
use tiny_skia::Pixmap;

use crate::ui::geom;
use crate::ui::icons::IconCache;
use crate::ui::text::TextEngine;
use crate::ui::view::Frame;

use super::canvas::{Canvas, ENTER_GLYPH, Rect};

/// The highlight shared by the result list and the action panel: a primary tint
/// for the selected or hovered row, plus the accent bar on the selected one.
fn draw_row_chrome(canvas: &Canvas, pixmap: &mut Pixmap, frame: &Frame<'_>, body: &RowBody<'_>) {
    let rect = body.rect;
    let alpha = body.alpha;
    let layout = frame.layout;
    let background = match (body.selected, body.hovered) {
        (true, _) => Some(frame.fade_rgba_scaled(frame.surfaces.selection, alpha)),
        (false, true) => Some(frame.fade_rgba_scaled(frame.surfaces.hover, alpha)),
        (false, false) => None,
    };
    if let Some(background) = background {
        canvas.fill_round(pixmap, rect, layout.row_radius(), background);
    }

    if body.selected {
        canvas.fill_round(
            pixmap,
            Rect {
                x: rect.x + 3.0,
                y: rect.center_y() - geom::ACCENT_H / 2.0,
                w: geom::ACCENT_W,
                h: geom::ACCENT_H,
            },
            layout.accent_radius(),
            frame.fade_rgba_scaled(frame.surfaces.accent, alpha),
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
    pub default_marker: bool,
    /// A new row's enter fade; 1 once it has settled.
    pub alpha: f32,
}

/// A row's chrome, icon, labels column and tail marks: the result list and the
/// action panel draw rows through this one path.
pub(super) fn draw_row(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    frame: &Frame<'_>,
    text: &mut TextEngine,
    icons: &mut IconCache,
    body: &RowBody<'_>,
) {
    let theme = frame.theme;
    let font = frame.font;
    let rect = body.rect;
    draw_row_chrome(canvas, pixmap, frame, body);

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
                let opacity = frame.opacity * body.alpha;
                icons.draw_tinted(pixmap, path, pos, size, opacity, color);
            }
            None => {
                let opacity = frame.opacity * body.alpha;
                icons.draw(pixmap, path, pos.0, pos.1, size, opacity);
            }
        }
    }

    let labels_x = icon_x + font.icon() + 12.0;
    // The selected row's ↵ hint and the default marker are part of the layout:
    // the labels must leave room for both.
    let enter = body
        .selected
        .then(|| text.shape(ENTER_GLYPH, 13.0 * canvas.scale, Weight::NORMAL));
    let enter_w = enter
        .as_ref()
        .map_or(0.0, |shaped| shaped.width / canvas.scale + 12.0);
    let marker_w = if body.default_marker { 12.0 } else { 0.0 };
    let labels_max = (rect.right() - 10.0 - labels_x - enter_w - marker_w).max(0.0);

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
        frame.fade_scaled(theme.fg, 1.0, body.alpha),
        canvas.px(labels_x),
        canvas.px(labels_top),
        Some(clip),
    );
    if let Some(summary) = &summary {
        text.draw(
            pixmap,
            summary,
            frame.fade_rgba_scaled(frame.surfaces.summary, body.alpha),
            canvas.px(labels_x),
            canvas.px(labels_top + title_h + 2.0),
            Some(clip),
        );
    }

    if let Some(check) = &enter {
        text.draw(
            pixmap,
            check,
            frame.fade_scaled(theme.primary, frame.muted_alpha(), body.alpha),
            canvas.px(rect.right() - 10.0) - check.width,
            canvas.px(rect.center_y()) - check.height / 2.0,
            None,
        );
    }

    if body.default_marker {
        let cx = rect.right() - 10.0 - enter_w - 6.0;
        canvas.fill_round(
            pixmap,
            Rect {
                x: cx - 3.0,
                y: rect.center_y() - 3.0,
                w: 6.0,
                h: 6.0,
            },
            3.0,
            frame.fade_scaled(theme.primary, 1.0, body.alpha),
        );
    }
}
