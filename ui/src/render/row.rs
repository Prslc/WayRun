use std::time::Instant;

use cosmic_text::Weight;
use wayrun_core::wire::IconShape;

use crate::geom;
use crate::geom::{Layout, Rect};
use crate::icons::IconDraw;
use crate::present::RowMotion;
use crate::theme::tint;
use crate::view::Frame;

use super::DrawCtx;

/// The return hint uses the covered `⏎` form rather than `↵`, which the shaping
/// family may lack and then fall back widely.
const ENTER_GLYPH: &str = "⏎";

/// A group entry's trailing mark: Enter opens a further panel level.
const SUBMENU_GLYPH: &str = "›";

/// One row the window draws: its item index, its rect (motion included) and the
/// motion's opacity.
pub(super) struct RowSlot {
    pub index: usize,
    pub rect: Rect,
    pub alpha: f32,
}

/// The fixed [`Layout::max_rows`] window the list and the panel share: pitch,
/// motion and the cap live here, the caller only fills each row's content.
pub(super) struct RowWindow<'a> {
    pub layout: Layout,
    pub surface: (u32, u32),
    pub split: bool,
    pub first: usize,
    pub count: usize,
    pub motion: &'a [Option<RowMotion>],
    pub now: Instant,
}

impl RowWindow<'_> {
    pub(super) fn slots(&self) -> impl Iterator<Item = RowSlot> + '_ {
        let end = (self.first + self.layout.max_rows).min(self.count);
        (self.first..end).map(move |index| {
            let (alpha, offset) = self
                .motion
                .get(index)
                .copied()
                .flatten()
                .map_or((1.0, 0.0), |motion| motion.sample(self.now));
            let rect = self
                .layout
                .row_rect(self.surface, self.split, index, self.first);
            RowSlot {
                index,
                rect: Rect {
                    y: rect.y + offset,
                    ..rect
                },
                alpha,
            }
        })
    }
}

/// The row's own tokens: the highlight, the labels and the tail marks. A row's
/// motion alpha scales these at draw time.
struct Style {
    selection: [u8; 4],
    hover: [u8; 4],
    accent: [u8; 4],
    title: [u8; 4],
    summary: [u8; 4],
    enter: [u8; 4],
    chevron: [u8; 4],
    marker: [u8; 4],
    action_icon_tint: [u8; 3],
}

impl Style {
    fn of(frame: &Frame<'_>) -> Self {
        let theme = frame.theme;
        let muted = frame.muted_alpha();
        Self {
            selection: frame.surfaces.selection,
            hover: frame.surfaces.hover,
            accent: frame.surfaces.accent,
            title: tint(theme.fg, 1.0),
            summary: frame.surfaces.summary,
            enter: tint(theme.primary, muted),
            chevron: tint(theme.fg, muted),
            marker: tint(theme.primary, 1.0),
            action_icon_tint: theme.fg,
        }
    }
}

/// The panel's action icons take the foreground role.
pub(super) fn action_icon_tint(frame: &Frame<'_>) -> [u8; 3] {
    Style::of(frame).action_icon_tint
}

/// The highlight shared by the result list and the action panel: a primary tint
/// for the selected or hovered row, plus the accent bar on the selected one.
fn draw_row_chrome(ctx: &mut DrawCtx, body: &RowBody<'_>) {
    let rect = body.rect;
    let alpha = body.alpha;
    let layout = ctx.frame.layout;
    let style = Style::of(ctx.frame);
    let background = match (body.selected, body.hovered) {
        (true, _) => Some(ctx.frame.fade_rgba_scaled(style.selection, alpha)),
        (false, true) => Some(ctx.frame.fade_rgba_scaled(style.hover, alpha)),
        (false, false) => None,
    };
    if let Some(background) = background {
        ctx.canvas
            .fill_round(ctx.pixmap, rect, layout.row_radius(), background);
    }

    if body.selected {
        ctx.canvas.fill_round(
            ctx.pixmap,
            Rect {
                x: rect.x + 3.0,
                y: rect.center_y() - geom::ACCENT_H / 2.0,
                w: geom::ACCENT_W,
                h: geom::ACCENT_H,
            },
            layout.accent_radius(),
            ctx.frame.fade_rgba_scaled(style.accent, alpha),
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
    pub icon_shape: IconShape,
    pub title: &'a str,
    pub bold: bool,
    pub summary: Option<&'a str>,
    pub default_marker: bool,
    /// Whether Enter opens a further level, drawn as a trailing chevron.
    pub expand: bool,
    /// A new row's enter fade; 1 once it has settled.
    pub alpha: f32,
}

/// A row's chrome, icon, labels column and tail marks: the result list and the
/// action panel draw rows through this one path.
pub(super) fn draw_row(ctx: &mut DrawCtx, body: &RowBody<'_>) {
    let frame = ctx.frame;
    let style = Style::of(frame);
    let font = &frame.font;
    let rect = body.rect;
    let canvas = ctx.canvas;
    draw_row_chrome(ctx, body);

    // A constant icon x, so the accent bar never shifts it.
    let icon_x = rect.x + 11.0;
    if let Some(path) = body.icon {
        let pos = (
            canvas.px(icon_x),
            canvas.px(rect.center_y() - font.icon() / 2.0),
        );
        let size = (font.icon() * canvas.scale).round() as u32;
        let draw = IconDraw {
            path,
            x: pos.0,
            y: pos.1,
            size,
            opacity: frame.opacity * body.alpha,
            shape: body.icon_shape,
        };
        match body.icon_tint {
            Some(color) => ctx.icons.draw_tinted(ctx.pixmap, draw, color),
            None => ctx.icons.draw(ctx.pixmap, draw),
        }
    }

    let labels_x = icon_x + font.icon() + 12.0;
    // The ↵ hint, the chevron and the default marker are part of the layout:
    // the labels must leave room for all three.
    let enter = (body.selected && !body.expand).then(|| {
        ctx.text
            .shape(ENTER_GLYPH, 13.0 * canvas.scale, Weight::NORMAL)
    });
    let enter_w = enter
        .as_ref()
        .map_or(0.0, |shaped| canvas.unpx(shaped.width) + 12.0);
    let chevron = body.expand.then(|| {
        ctx.text
            .shape(SUBMENU_GLYPH, 13.0 * canvas.scale, Weight::NORMAL)
    });
    let chevron_w = chevron
        .as_ref()
        .map_or(0.0, |shaped| canvas.unpx(shaped.width) + 12.0);
    let marker_w = if body.default_marker { 12.0 } else { 0.0 };
    let labels_max = (rect.right() - 10.0 - labels_x - enter_w - chevron_w - marker_w).max(0.0);

    let title = ctx.text.fit(
        body.title,
        font.title() * canvas.scale,
        if body.bold {
            Weight::BOLD
        } else {
            Weight::NORMAL
        },
        labels_max * canvas.scale,
    );
    let title_h = canvas.unpx(title.height);
    let summary = body.summary.map(|summary| {
        ctx.text.fit(
            summary,
            font.summary() * canvas.scale,
            Weight::NORMAL,
            labels_max * canvas.scale,
        )
    });
    let summary_h = summary
        .as_ref()
        .map_or(0.0, |shaped| canvas.unpx(shaped.height));

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

    ctx.text.draw(
        ctx.pixmap,
        &title,
        frame.fade_rgba_scaled(style.title, body.alpha),
        canvas.px(labels_x),
        canvas.px(labels_top),
        Some(clip),
    );
    if let Some(summary) = &summary {
        ctx.text.draw(
            ctx.pixmap,
            summary,
            frame.fade_rgba_scaled(style.summary, body.alpha),
            canvas.px(labels_x),
            canvas.px(labels_top + title_h + 2.0),
            Some(clip),
        );
    }

    if let Some(check) = &enter {
        ctx.text.draw(
            ctx.pixmap,
            check,
            frame.fade_rgba_scaled(style.enter, body.alpha),
            canvas.px(rect.right() - 10.0) - check.width,
            canvas.px(rect.center_y()) - check.height / 2.0,
            None,
        );
    }

    if let Some(chevron) = &chevron {
        ctx.text.draw(
            ctx.pixmap,
            chevron,
            frame.fade_rgba_scaled(style.chevron, body.alpha),
            canvas.px(rect.right() - 10.0) - chevron.width,
            canvas.px(rect.center_y()) - chevron.height / 2.0,
            None,
        );
    }

    if body.default_marker {
        let cx = rect.right() - 10.0 - enter_w - 6.0;
        ctx.canvas.fill_round(
            ctx.pixmap,
            Rect {
                x: cx - 3.0,
                y: rect.center_y() - 3.0,
                w: 6.0,
                h: 6.0,
            },
            3.0,
            frame.fade_rgba_scaled(style.marker, body.alpha),
        );
    }
}
