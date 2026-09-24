use std::time::Instant;

use cosmic_text::Weight;
use tiny_skia::Pixmap;

use crate::app::State;
use crate::ui::icons::IconCache;
use crate::ui::text::TextEngine;

use self::canvas::{Canvas, ENTER_GLYPH, Rect};

pub use self::bench::bench;
pub use self::field::{caret_rect, draw_caret};

mod bench;
mod canvas;
mod field;
mod footer;
mod list;
mod panel;

/// The highlight shared by the result list and the action panel: a primary tint
/// for the selected or hovered row, plus the accent bar on the selected one.
pub(super) fn draw_row_chrome(
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

pub fn draw(
    pixmap: &mut Pixmap,
    state: &State,
    text: &mut TextEngine,
    icons: &mut IconCache,
    now: Instant,
    full: bool,
) {
    let timing = state.timing;
    let mut marks: Vec<(&str, Instant)> = Vec::new();
    let mut mark = |name: &'static str| {
        if timing {
            marks.push((name, Instant::now()));
        }
    };
    mark("start");
    let canvas = Canvas {
        scale: state.scale_factor(),
    };
    let surface = state.surface;
    let appearance = &state.appearance;
    let layout = appearance.layout;
    let dim_rgb = state.dim_color();

    let card = Rect {
        x: layout.card_x(surface),
        y: layout.card_top(surface),
        w: layout.card_w(surface),
        h: state.card_height(now),
    };

    // The frame is retained, so a settled frame repaints only the card and
    // leaves the dim in place; an animation or fresh frame repaints it all.
    let dim = state.dim_alpha(now);
    if full {
        canvas.fill_all(
            pixmap,
            [
                dim_rgb[0],
                dim_rgb[1],
                dim_rgb[2],
                (dim * 255.0).round() as u8,
            ],
        );
    } else {
        // A shrink has to erase the old card's rows too, so the region spans the
        // union of the previous and current bottoms, not just the current card.
        let base_bottom = card.bottom().max(state.last_card_bottom);
        canvas.restore_dim(
            pixmap,
            Rect {
                x: card.x,
                y: card.y,
                w: card.w,
                h: base_bottom - card.y,
            },
            dim,
            dim_rgb,
        );
    }
    mark("clear");

    // The card: a translucent fill with a 1px hairline. The hairline is a *ring*,
    // not a base fill, which would raise the interior's alpha.
    canvas.fill_round(
        pixmap,
        Rect {
            x: card.x + 1.0,
            y: card.y + 1.0,
            w: card.w - 2.0,
            h: card.h - 2.0,
        },
        layout.radius - 1.0,
        state.fade_rgba(state.surfaces.card, now),
    );
    canvas.stroke_round(
        pixmap,
        Rect {
            x: card.x + 0.5,
            y: card.y + 0.5,
            w: card.w - 1.0,
            h: card.h - 1.0,
        },
        layout.hairline_radius(),
        layout.hairline_width,
        state.fade_rgba(state.surfaces.hairline, now),
    );

    mark("card");
    let field = Rect::field_at(card.x, card.y, card.w);
    canvas.fill_round(
        pixmap,
        field,
        layout.field_radius(),
        state.fade_rgba(state.surfaces.field, now),
    );

    mark("shapes");

    field::draw_magnifier(&canvas, pixmap, field, state, now);
    field::draw_query(&canvas, pixmap, field, state, text, now);
    field::draw_toolbar(&canvas, pixmap, state, text, now);
    mark("query");

    if state.menu.is_some() {
        panel::draw_actions(&canvas, pixmap, surface, state, text, icons, now);
    } else if !state.rows.is_empty() {
        list::draw_list(&canvas, pixmap, surface, state, text, icons, now);
    }
    mark("list");

    footer::draw_footer(&canvas, pixmap, surface, state, text, now);
    mark("footer");

    // A growing payload lays rows out immediately while the height animates, so
    // the band below the card's bottom belongs to the backdrop.
    let resting = layout.card_top(surface) + state.content_height();
    let bottom = card.y + card.h;
    let mut band = None;
    if full && bottom < resting {
        canvas.restore_dim_below(pixmap, bottom, surface, dim, dim_rgb);
        band = Some(bottom);
    }

    if timing && marks.len() > 1 {
        let mut previous = marks[0].1;
        let mut line = String::from("wayrun: draw");
        for (name, at) in &marks[1..] {
            line.push_str(&format!(" {}={:?}", name, at.duration_since(previous)));
            previous = *at;
        }
        line.push_str(&format!(
            " total={:?}",
            marks[marks.len() - 1].1.duration_since(marks[0].1)
        ));
        if let Some(bottom) = band {
            line.push_str(&format!(" reflow_band_from={bottom}"));
        }
        eprintln!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::canvas::dim_u8;
    use super::*;
    use tiny_skia::Color;

    /// A cache whose queue nobody drains: these frames draw no icons.
    fn idle_icons() -> IconCache {
        let (jobs, _queued) = std::sync::mpsc::channel();
        IconCache::new(jobs)
    }

    #[test]
    fn the_base_fill_is_the_dim_with_no_clear_under_it() {
        let mut state = State::new();
        state.surface = (64, 64);
        // Start at the settled end state so the dim is at full strength.
        state.reduce_motion = true;

        let mut pixmap = Pixmap::new(64, 64).unwrap();
        let mut text = TextEngine::with_family("Source Han Sans CN".to_string());
        let mut icons = idle_icons();
        draw(
            &mut pixmap,
            &state,
            &mut text,
            &mut icons,
            Instant::now(),
            true,
        );

        // Above the card only the dim exists; a clear under the dim would show
        // up as alpha 0.
        let pixel = pixmap.pixel(0, 0).unwrap();
        assert_eq!(pixel.alpha(), dim_u8(), "the dim is the base");
        assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (0, 0, 0));
    }

    #[test]
    fn a_settled_repaint_only_touches_the_card_rectangle() {
        let mut state = State::new();
        state.surface = (1600, 1080);
        state.reduce_motion = true;

        let mut text = TextEngine::with_family("Source Han Sans CN".to_string());
        let mut icons = idle_icons();
        let mut pixmap = Pixmap::new(1600, 1080).unwrap();
        // A sentinel everywhere: a region repaint must leave the backdrop alone.
        pixmap.fill(Color::from_rgba8(255, 0, 255, 255));

        draw(
            &mut pixmap,
            &state,
            &mut text,
            &mut icons,
            Instant::now(),
            false,
        );

        // A point outside the card keeps the sentinel untouched.
        let outside = pixmap.pixel(0, 0).unwrap();
        assert_eq!((outside.red(), outside.blue()), (255, 255), "outside");

        // A point inside the card was restored to the dim and painted over, so
        // it can no longer be the sentinel.
        let inside = pixmap
            .pixel(
                (state.appearance.layout.card_x(state.surface)
                    + state.appearance.layout.card_w(state.surface) / 2.0) as u32,
                (state.appearance.layout.card_top(state.surface) + 10.0) as u32,
            )
            .unwrap();
        assert_ne!((inside.red(), inside.green(), inside.blue()), (255, 0, 255));
    }

    #[test]
    fn a_settled_shrink_erases_the_old_cards_rows() {
        let mut state = State::new();
        state.surface = (1600, 1080);
        state.reduce_motion = true;
        let card_h = state.appearance.layout.content_h(0);
        // The previous card was 200px taller than the current one.
        state.last_card_bottom = state.appearance.layout.card_top(state.surface) + card_h + 200.0;

        let mut text = TextEngine::with_family("Source Han Sans CN".to_string());
        let mut icons = idle_icons();
        let mut pixmap = Pixmap::new(1600, 1080).unwrap();
        pixmap.fill(Color::from_rgba8(255, 0, 255, 255));

        draw(
            &mut pixmap,
            &state,
            &mut text,
            &mut icons,
            Instant::now(),
            false,
        );

        // Below the current card but inside the old one: the dim, not the
        // sentinel and not a leftover row.
        let y = (state.appearance.layout.card_top(state.surface) + card_h + 100.0) as u32;
        let pixel = pixmap.pixel(800, y).unwrap();
        assert_ne!(
            (pixel.red(), pixel.green(), pixel.blue()),
            (255, 0, 255),
            "the old card's rows were erased"
        );
    }
}
