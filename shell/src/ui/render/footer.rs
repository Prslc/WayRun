use std::time::Instant;

use cosmic_text::Weight;
use tiny_skia::Pixmap;

use crate::app::State;
use crate::ui::geom;
use crate::ui::text::TextEngine;

use super::canvas::{Canvas, Rect};

/// One key hint: a keycap and its label. An empty key is a plain note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Hint {
    key: &'static str,
    label: &'static str,
}

const PANEL_HINTS: &[Hint] = &[
    Hint {
        key: "⏎",
        label: "Run",
    },
    Hint {
        key: "Esc",
        label: "Back",
    },
];

const PANEL_DEFAULT_HINTS: &[Hint] = &[
    Hint {
        key: "⏎",
        label: "Run",
    },
    Hint {
        key: "Alt⏎",
        label: "Default",
    },
    Hint {
        key: "Esc",
        label: "Back",
    },
];

const PANEL_CLEAR_DEFAULT_HINTS: &[Hint] = &[
    Hint {
        key: "⏎",
        label: "Run",
    },
    Hint {
        key: "Alt⏎",
        label: "Clear default",
    },
    Hint {
        key: "Esc",
        label: "Back",
    },
];

const ROW_HINTS: &[Hint] = &[
    Hint {
        key: "⏎",
        label: "Launch",
    },
    Hint {
        key: "⇧⏎",
        label: "Actions",
    },
];

const LAUNCH_HINT: &[Hint] = &[Hint {
    key: "⏎",
    label: "Launch",
}];

const HELP_HINT: &[Hint] = &[Hint {
    key: "",
    label: "Type ? for help",
}];

const NO_MATCH_HINT: &[Hint] = &[Hint {
    key: "",
    label: "No results",
}];

/// The footer's left hints: the panel's keys when open, the launch keys once
/// rows exist, a help note for an untouched field, else "No results".
pub(super) fn footer_hints(
    rows: usize,
    query_empty: bool,
    panel: bool,
    has_actions: bool,
    can_default: bool,
    is_default: bool,
) -> &'static [Hint] {
    if panel {
        if !can_default {
            PANEL_HINTS
        } else if is_default {
            PANEL_CLEAR_DEFAULT_HINTS
        } else {
            PANEL_DEFAULT_HINTS
        }
    } else if rows > 0 {
        if has_actions { ROW_HINTS } else { LAUNCH_HINT }
    } else if query_empty {
        HELP_HINT
    } else {
        NO_MATCH_HINT
    }
}

const KEYCAP_PAD_X: f32 = 5.0;
const KEYCAP_RADIUS: f32 = 4.0;
const KEY_LABEL_GAP: f32 = 6.0;
const ITEM_GAP: f32 = 16.0;
const COUNT_GAP: f32 = 14.0;

fn count_label(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {plural}")
    }
}

pub(super) fn draw_footer(
    canvas: &Canvas,
    pixmap: &mut Pixmap,
    surface: (u32, u32),
    state: &State,
    text: &mut TextEngine,
    now: Instant,
) {
    let panel = state.menu.is_some();
    let empty = state.rows.is_empty();
    let no_match = empty && !state.query.is_empty();
    let has_actions = state
        .rows
        .get(state.selected)
        .is_some_and(|row| !row.actions.is_empty());
    // A highlighted panel action with a plugin id can be made the default.
    let (can_default, is_default) = state
        .menu
        .as_ref()
        .and_then(|menu| menu.selected_action())
        .map_or((false, false), |action| {
            (
                action.id.is_some() && action.plugin.is_some(),
                action.default,
            )
        });
    let hints = footer_hints(
        state.rows.len(),
        state.query.is_empty(),
        panel,
        has_actions,
        can_default,
        is_default,
    );

    let count = if panel {
        count_label(
            state.menu.as_ref().map_or(0, |menu| menu.actions.len()),
            "action",
            "actions",
        )
    } else if empty {
        String::new()
    } else {
        count_label(state.rows.len(), "result", "results")
    };

    // The footer is the last band of the card, derived from the same height the
    // card itself animates to.
    let layout = state.appearance.layout;
    let top = layout.card_top(surface) + state.content_height() - geom::PAD - geom::FOOTER_H;
    let center_y = top + geom::FOOTER_H / 2.0;
    let left = layout.card_x(surface) + geom::PAD;
    let right = layout.card_x(surface) + layout.card_w(surface) - geom::PAD;
    let font_size = state.appearance.font.suggestion();
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
    for hint in hints {
        if hint.key.is_empty() {
            let color = if no_match {
                state.fade(state.theme.primary, 0.7, now)
            } else {
                state.fade_rgba(state.surfaces.footer, now)
            };
            let shaped = text.shape(hint.label, label_size, Weight::NORMAL);
            let height = shaped.height / canvas.scale;
            text.draw(
                pixmap,
                &shaped,
                color,
                canvas.px(x),
                canvas.px(center_y - height / 2.0),
                None,
            );
            break;
        }

        let key = text.shape(hint.key, key_size, Weight::NORMAL);
        let label = text.shape(hint.label, label_size, Weight::NORMAL);
        let key_w = key.width / canvas.scale;
        let label_w = label.width / canvas.scale;
        let cap_w = key_w + 2.0 * KEYCAP_PAD_X;
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
            state.fade(state.theme.fg, 0.09, now),
        );
        text.draw(
            pixmap,
            &key,
            state.fade(state.theme.fg, 0.75, now),
            canvas.px(x + KEYCAP_PAD_X),
            canvas.px(center_y - key.height / canvas.scale / 2.0),
            None,
        );
        text.draw(
            pixmap,
            &label,
            state.fade_rgba(state.surfaces.footer, now),
            canvas.px(x + cap_w + KEY_LABEL_GAP),
            canvas.px(center_y - label.height / canvas.scale / 2.0),
            None,
        );
        x += cap_w + KEY_LABEL_GAP + label_w + ITEM_GAP;
    }

    if let Some(shaped) = &count_shaped {
        let width = shaped.width / canvas.scale;
        let height = shaped.height / canvas.scale;
        let cap_w = width + 2.0 * KEYCAP_PAD_X;
        let cap_h = (height + 6.0).min(geom::FOOTER_H);
        let x = right - cap_w;
        canvas.fill_round(
            pixmap,
            Rect {
                x,
                y: center_y - cap_h / 2.0,
                w: cap_w,
                h: cap_h,
            },
            KEYCAP_RADIUS,
            state.fade(state.theme.fg, 0.06, now),
        );
        text.draw(
            pixmap,
            shaped,
            state.fade_rgba(state.surfaces.muted, now),
            canvas.px(x + KEYCAP_PAD_X),
            canvas.px(center_y - height / 2.0),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_footer_separates_no_results_from_an_untouched_field() {
        // an empty field is the history view, not a failed search
        assert_eq!(footer_hints(0, true, false, false, false, false), HELP_HINT);
        assert_eq!(
            footer_hints(0, false, false, false, false, false),
            NO_MATCH_HINT
        );
        assert_eq!(footer_hints(3, false, false, true, false, false), ROW_HINTS);
        // the panel owns the footer while it is open
        assert_eq!(
            footer_hints(3, false, true, true, false, false),
            PANEL_HINTS
        );
    }

    #[test]
    fn a_row_without_actions_drops_the_actions_hint() {
        assert_eq!(
            footer_hints(3, false, false, false, false, false),
            LAUNCH_HINT
        );
        // the panel hint outlives the selected row's actions while it is open
        assert_eq!(
            footer_hints(3, false, true, false, false, false),
            PANEL_HINTS
        );
    }

    #[test]
    fn a_defaultable_panel_action_shows_the_alt_hint() {
        // a plugin action that is not yet the default
        assert_eq!(
            footer_hints(3, false, true, true, true, false),
            PANEL_DEFAULT_HINTS
        );
        // already the default: the hint offers to clear it
        assert_eq!(
            footer_hints(3, false, true, true, true, true),
            PANEL_CLEAR_DEFAULT_HINTS
        );
        // a launcher-level or host action has no id, so no hint
        assert_eq!(
            footer_hints(3, false, true, true, false, false),
            PANEL_HINTS
        );
    }

    #[test]
    fn a_single_count_is_singular() {
        assert_eq!(count_label(1, "result", "results"), "1 result");
        assert_eq!(count_label(0, "result", "results"), "0 results");
        assert_eq!(count_label(2, "action", "actions"), "2 actions");
    }
}
