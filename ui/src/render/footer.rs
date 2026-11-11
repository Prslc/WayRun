use std::sync::OnceLock;

use cosmic_text::Weight;
use rust_i18n::t;
use tiny_skia::Pixmap;

use crate::geom;
use crate::text::{Shaped, TextEngine};
use crate::view::Frame;

use crate::geom::Rect;

use super::canvas::Canvas;

/// One key hint: a keycap and its label. An empty key is a plain note; an
/// `effective` label is the action a remembered default makes `Enter` run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Hint {
    key: &'static str,
    label: String,
    effective: bool,
}

fn hint(key: &'static str, label: String) -> Hint {
    Hint {
        key,
        label,
        effective: false,
    }
}

/// An `Enter` hint, whose label yields to the row's effective action.
fn enter(label: String) -> Hint {
    Hint {
        key: "⏎",
        label,
        effective: true,
    }
}

/// The footer's hint sets, translated once: a locale is fixed for the process
/// and the footer is redrawn on every frame.
struct Hints {
    panel: Vec<Hint>,
    panel_default: Vec<Hint>,
    panel_clear_default: Vec<Hint>,
    row: Vec<Hint>,
    launch: Vec<Hint>,
    no_match: Vec<Hint>,
}

fn hints() -> &'static Hints {
    static HINTS: OnceLock<Hints> = OnceLock::new();
    HINTS.get_or_init(|| {
        let run = || hint("⏎", t!("footer.run"));
        let back = || hint("Esc", t!("footer.back"));
        Hints {
            panel: vec![run(), back()],
            panel_default: vec![run(), hint("Alt⏎", t!("footer.default")), back()],
            panel_clear_default: vec![run(), hint("Alt⏎", t!("footer.clear_default")), back()],
            row: vec![enter(t!("footer.launch")), hint("⇧⏎", t!("footer.actions"))],
            launch: vec![enter(t!("footer.launch"))],
            no_match: vec![hint("", t!("footer.no_results"))],
        }
    })
}

/// The footer's left hints: panel keys, launch keys once rows exist, a note for
/// a failed search, nothing for an untouched field or a search in flight.
pub(super) fn footer_hints(
    rows: usize,
    query_empty: bool,
    panel: bool,
    has_actions: bool,
    can_default: bool,
    is_default: bool,
    searching: bool,
) -> &'static [Hint] {
    let hints = hints();
    if panel {
        if !can_default {
            &hints.panel
        } else if is_default {
            &hints.panel_clear_default
        } else {
            &hints.panel_default
        }
    } else if rows > 0 {
        if has_actions {
            &hints.row
        } else {
            &hints.launch
        }
    } else if query_empty || searching {
        &[]
    } else {
        &hints.no_match
    }
}

const KEYCAP_PAD_X: f32 = 5.0;
const KEYCAP_RADIUS: f32 = 4.0;
const KEY_LABEL_GAP: f32 = 6.0;
const ITEM_GAP: f32 = 16.0;
const COUNT_GAP: f32 = 14.0;
/// The panel title is a label, not a hint: below this width it would paint a
/// lone ellipsis, so it is dropped rather than drawn that way.
const TITLE_MIN_W: f32 = 40.0;

fn count_label(n: usize, one: &'static str, many: &'static str) -> String {
    if n == 1 { t!(one) } else { t!(many, count = n) }
}

fn label_chip_w(shaped: &Shaped, scale: f32) -> f32 {
    shaped.width / scale + 2.0 * KEYCAP_PAD_X
}

/// A label-only chip: the count pill and the no-match note share its style.
fn draw_label_chip(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    frame: &Frame<'_>,
    text: &mut TextEngine,
    shaped: &Shaped,
    x: f32,
    center_y: f32,
) {
    let width = label_chip_w(shaped, canvas.scale);
    let height = shaped.height / canvas.scale;
    let cap_h = (height + 6.0).min(geom::FOOTER_H);
    canvas.fill_round(
        pixmap,
        Rect {
            x,
            y: center_y - cap_h / 2.0,
            w: width,
            h: cap_h,
        },
        KEYCAP_RADIUS,
        frame.fade(frame.theme.fg, 0.06),
    );
    text.draw(
        pixmap,
        shaped,
        frame.fade_rgba(frame.surfaces.muted),
        canvas.px(x + KEYCAP_PAD_X),
        canvas.px(center_y - height / 2.0),
        None,
    );
}

pub(super) fn draw_footer(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    frame: &Frame<'_>,
    text: &mut TextEngine,
) {
    let footer = &frame.footer;
    let panel = footer.panel;
    let empty = footer.rows == 0;
    let hints = footer_hints(
        footer.rows,
        footer.query_empty,
        panel,
        footer.has_actions,
        footer.can_default,
        footer.is_default,
        footer.searching,
    );

    let count = if panel {
        count_label(
            footer.action_count,
            "footer.count_action_one",
            "footer.count_action_many",
        )
    } else if footer.loading {
        t!("footer.searching").to_string()
    } else if empty {
        String::new()
    } else {
        count_label(
            footer.rows,
            "footer.count_result_one",
            "footer.count_result_many",
        )
    };

    // The footer is the card's last band, so it rides the drawn height rather
    // than jumping ahead to the resting one.
    let band = frame.layout.footer_rect(frame.surface, frame.card_h);
    let center_y = band.center_y();
    let left = band.x;
    let right = band.right();
    let font_size = frame.font.suggestion();
    let label_size = font_size * canvas.scale;
    let key_size = (font_size - 1.0).max(6.0) * canvas.scale;

    let count_shaped = (!count.is_empty()).then(|| text.shape(&count, label_size, Weight::NORMAL));
    let count_cap_w = count_shaped.as_ref().map_or(0.0, |shaped| {
        shaped.width / canvas.scale + 2.0 * KEYCAP_PAD_X
    });
    let limit = right
        - count_cap_w
        - if count_shaped.is_some() {
            COUNT_GAP
        } else {
            0.0
        };

    let mut x = left;
    let effective = footer.effective_label;
    for hint in hints {
        if hint.key.is_empty() {
            let shaped = text.shape(&hint.label, label_size, Weight::NORMAL);
            draw_label_chip(canvas, pixmap, frame, text, &shaped, x, center_y);
            break;
        }

        let key = text.shape(hint.key, key_size, Weight::NORMAL);
        let key_w = key.width / canvas.scale;
        let cap_w = key_w + 2.0 * KEYCAP_PAD_X;
        // An action name is arbitrary, so it is elided to the room left instead
        // of dropping the whole hint the way a fixed label does.
        let label = if hint.effective {
            let room = limit - x - cap_w - KEY_LABEL_GAP;
            text.fit(
                effective.unwrap_or(&hint.label),
                label_size,
                Weight::NORMAL,
                room * canvas.scale,
            )
        } else {
            text.shape(&hint.label, label_size, Weight::NORMAL)
        };
        let label_w = label.width / canvas.scale;
        let cap_h = (key.height / canvas.scale + 6.0).min(geom::FOOTER_H);
        if x + cap_w + KEY_LABEL_GAP + label_w > limit {
            break;
        }

        // The keycap is a chip around the key glyph, centred in the footer band.
        canvas.fill_round(
            pixmap,
            Rect {
                x,
                y: center_y - cap_h / 2.0,
                w: cap_w,
                h: cap_h,
            },
            KEYCAP_RADIUS,
            frame.fade(frame.theme.fg, 0.09),
        );
        text.draw(
            pixmap,
            &key,
            frame.fade(frame.theme.fg, 0.75),
            canvas.px(x + KEYCAP_PAD_X),
            canvas.px(center_y - key.height / canvas.scale / 2.0),
            None,
        );
        text.draw(
            pixmap,
            &label,
            frame.fade_rgba(frame.surfaces.footer),
            canvas.px(x + cap_w + KEY_LABEL_GAP),
            canvas.px(center_y - label.height / canvas.scale / 2.0),
            None,
        );
        x += cap_w + KEY_LABEL_GAP + label_w + ITEM_GAP;
    }

    if let Some(title) = footer.panel_title.filter(|title| !title.trim().is_empty()) {
        // `x` already carries the ITEM_GAP after the last drawn hint; `limit`
        // sits COUNT_GAP left of the count pill.
        let room = limit - x;
        if room >= TITLE_MIN_W {
            let shaped = text.fit(title, label_size, Weight::BOLD, room * canvas.scale);
            let width = shaped.width / canvas.scale;
            let height = shaped.height / canvas.scale;
            text.draw(
                pixmap,
                &shaped,
                frame.fade(frame.theme.primary, 0.9),
                canvas.px(limit - width),
                canvas.px(center_y - height / 2.0),
                None,
            );
        }
    }

    if let Some(shaped) = &count_shaped {
        let x = right - label_chip_w(shaped, canvas.scale);
        draw_label_chip(canvas, pixmap, frame, text, shaped, x, center_y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_footer_separates_no_results_from_an_untouched_field() {
        // an untouched field stays silent: its placeholder is the guide
        assert!(footer_hints(0, true, false, false, false, false, false).is_empty());
        assert_eq!(
            footer_hints(0, false, false, false, false, false, false),
            hints().no_match.as_slice()
        );
        assert_eq!(
            footer_hints(3, false, false, true, false, false, false),
            hints().row.as_slice()
        );
        // the panel owns the footer while it is open
        assert_eq!(
            footer_hints(3, false, true, true, false, false, false),
            hints().panel.as_slice()
        );
    }

    #[test]
    fn a_pending_search_silences_the_no_match_note() {
        assert!(
            footer_hints(0, false, false, false, false, false, true).is_empty(),
            "the right side says searching; the left has no note"
        );
        assert_eq!(
            footer_hints(3, false, false, false, false, false, true),
            hints().launch.as_slice(),
            "stale rows keep their launch hints"
        );
    }

    #[test]
    fn a_row_without_actions_drops_the_actions_hint() {
        assert_eq!(
            footer_hints(3, false, false, false, false, false, false),
            hints().launch.as_slice()
        );
        // the panel hint outlives the selected row's actions while it is open
        assert_eq!(
            footer_hints(3, false, true, false, false, false, false),
            hints().panel.as_slice()
        );
    }

    #[test]
    fn a_defaultable_panel_action_shows_the_alt_hint() {
        // a plugin action that is not yet the default
        assert_eq!(
            footer_hints(3, false, true, true, true, false, false),
            hints().panel_default.as_slice()
        );
        // already the default: the hint offers to clear it
        assert_eq!(
            footer_hints(3, false, true, true, true, true, false),
            hints().panel_clear_default.as_slice()
        );
        // a launcher-level or host action has no id, so no hint
        assert_eq!(
            footer_hints(3, false, true, true, false, false, false),
            hints().panel.as_slice()
        );
    }

    #[test]
    fn a_single_count_is_singular() {
        let one = |n| count_label(n, "footer.count_result_one", "footer.count_result_many");
        let many = |n| count_label(n, "footer.count_action_one", "footer.count_action_many");
        assert!(one(1).starts_with('1') && many(1).starts_with('1'));
        assert!(one(0).starts_with('0') && many(2).starts_with('2'));
        // the singular form is a different string from the plural one
        assert_ne!(one(1), one(2));
        assert_ne!(many(1), many(2));
    }
}
