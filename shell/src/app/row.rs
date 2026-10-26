use std::collections::HashMap;
use std::mem;
use std::time::Instant;

use wayrun_core::wire::{Action, ResultItem};

use wayrun_ui::present::{ROW_RISE, RowMotion};

use super::State;

/// The selected row's commands: what usage records and what Enter runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    /// The row's own command, recorded in usage so the counts stay keyed to it.
    pub target: Action,
    /// The command Enter runs: the remembered default action when the row has
    /// one, else `target`.
    pub effective: Action,
}

/// The identity a row keeps across payloads: its command, else its title.
fn row_identity(item: &ResultItem) -> String {
    item.on_click
        .as_ref()
        .map(|action| action.key())
        .unwrap_or_else(|| item.title.clone())
}

/// A blank icon spec means "no icon", so a re-send compares equal.
fn normalize_icons(items: Vec<ResultItem>) -> Vec<ResultItem> {
    items
        .into_iter()
        .map(|mut item| {
            item.icon = item.icon.filter(|spec| !spec.is_empty());
            item
        })
        .collect()
}

impl State {
    /// The selected row's launch fields.
    pub fn selected_row(&self) -> Option<Launch> {
        let row = self.rows.get(self.cursor.selected)?;
        let target = row.on_click.clone()?;
        let effective = super::menu::default_entry(&row.actions)
            .and_then(|action| action.action.clone())
            .unwrap_or_else(|| target.clone());
        Some(Launch { target, effective })
    }

    pub fn apply_results(&mut self, items: Vec<ResultItem>, now: Instant) {
        self.search_started = None;
        self.show_results(items, now, true);
    }

    /// Enter while a search is pending: the rows still answer the previous
    /// query, so the press waits for the payload instead of launching one.
    pub fn hold_submit(&mut self) -> bool {
        if self.search_started.is_none() {
            return false;
        }
        self.submit_held = Some(self.query.clone());
        true
    }

    /// The held press, released only by the payload for the query it was made
    /// for; an edit in between has already made it stale.
    pub fn take_held_submit(&mut self) -> bool {
        self.submit_held
            .take()
            .is_some_and(|query| query == self.query)
    }

    /// A panel or a dismissal takes the keyboard: no held Enter may still fire.
    pub fn cancel_held_submit(&mut self) {
        self.submit_held = None;
    }

    /// A streaming search's intermediate payload: the list updates, but the
    /// search stays pending and only the final payload resumes a held panel.
    pub fn apply_partial(&mut self, items: Vec<ResultItem>, now: Instant) {
        self.show_results(items, now, false);
    }

    fn show_results(&mut self, items: Vec<ResultItem>, now: Instant, final_payload: bool) {
        let items = normalize_icons(items);
        // A panel action holds its place across the reply it asked for.
        let resume = if final_payload {
            self.panel_resume.take()
        } else {
            None
        };
        // An identical re-send is dropped so it cannot reset the selection.
        if self.rows == items && !self.rows.is_empty() {
            return;
        }

        // A list appearing from nothing plays the entrance; a refresh swaps its
        // rows in place, since a fade per keystroke reads as a flicker.
        let entering = self.rows.is_empty();
        // The selection resets to row 0 below, so the new window starts at 0 and
        // `index` is its screen slot; the old offset is measured from the old one.
        let max_rows = self.layout().max_rows;
        let old_first = self.cursor.first as isize;
        let old_motion = mem::take(&mut self.row_motion);
        let old: HashMap<String, usize> = self
            .rows
            .iter()
            .enumerate()
            .map(|(index, item)| (row_identity(item), index))
            .collect();
        self.row_motion = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let enter = RowMotion {
                    offset: ROW_RISE,
                    alpha: 0.0,
                    started: now,
                };
                if index >= max_rows {
                    return None;
                }
                let Some(&previous) = old.get(&row_identity(item)) else {
                    return entering.then_some(enter);
                };
                let from_slot = previous as isize - old_first;
                // Only a row that was on screen slides; one from outside the
                // window would otherwise fly in from far below the card.
                if !(0..max_rows as isize).contains(&from_slot) {
                    return entering.then_some(enter);
                }
                // Start from where the row is drawn, so a payload landing
                // mid-slide continues the motion instead of snapping back.
                let (alpha, offset) = old_motion
                    .get(previous)
                    .copied()
                    .flatten()
                    .map_or((1.0, 0.0), |motion| motion.sample(now));
                let offset = offset + (from_slot as f32 - index as f32) * wayrun_ui::geom::ROW_H;
                (offset.abs() > 0.5 || alpha < 1.0).then_some(RowMotion {
                    offset,
                    alpha,
                    started: now,
                })
            })
            .collect();
        if self.presentation.reduce_motion {
            self.row_motion.fill(None);
        }
        self.rows = items;

        // A fresh payload starts at the top row, and any open panel is stale; a
        // local removal and an identical re-send keep the cursor.
        self.cursor.selected = 0;
        self.menu = None;
        if let Some((row_key, key)) = resume {
            self.resume_panel(&row_key, &key);
        }
        self.contain();
        self.retarget_height(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{item, run, state};
    use std::time::Duration;
    use std::time::Instant;
    use wayrun_core::wire::ActionItem;

    #[test]
    fn enter_waits_for_the_payload_of_the_query_it_was_pressed_for() {
        let mut state = state();
        let now = Instant::now();
        state.query = "s rust".to_string();
        assert!(state.search_sent(now));

        assert!(state.hold_submit(), "a pending search holds the press");
        state.query.push_str(" async");
        state.apply_results(vec![item("Search: s rust", None, None, None)], now);

        assert!(
            !state.take_held_submit(),
            "the edit made the held query stale"
        );
    }

    #[test]
    fn the_held_enter_runs_once_the_payload_lands() {
        let mut state = state();
        let now = Instant::now();
        state.query = "s rust".to_string();
        assert!(state.search_sent(now));
        assert!(state.hold_submit());

        state.apply_results(vec![item("Search: s rust", None, None, None)], now);
        assert!(state.take_held_submit(), "the payload releases the press");
        assert!(!state.take_held_submit(), "a press is released once");
    }

    #[test]
    fn enter_on_a_settled_list_does_not_hold() {
        let mut state = state();
        assert!(!state.hold_submit());
    }

    #[test]
    fn a_pending_search_shows_loading_after_the_delay() {
        let mut state = state();
        state.query = "query".to_string();
        let now = Instant::now();
        state.search_started = Some(now);

        assert!(!state.frame(now + Duration::from_millis(100)).footer.loading);
        assert!(state.frame(now + Duration::from_millis(200)).footer.loading);

        state.apply_results(Vec::new(), now + Duration::from_millis(200));
        assert!(!state.frame(now + Duration::from_millis(300)).footer.loading);
    }

    #[test]
    fn a_second_keystroke_keeps_the_loading_clock() {
        let mut state = state();
        let now = Instant::now();
        assert!(state.search_sent(now));
        assert!(!state.search_sent(now + Duration::from_millis(100)));
        assert_eq!(state.search_started, Some(now));

        state.apply_results(Vec::new(), now + Duration::from_millis(200));
        assert!(state.search_sent(now + Duration::from_millis(300)));
    }

    #[test]
    fn a_partial_payload_keeps_the_loading_state() {
        let mut state = state();
        state.query = "query".to_string();
        let now = Instant::now();
        state.search_started = Some(now);

        state.apply_partial(vec![item("Files", None, None, None)], now);
        assert!(state.frame(now + Duration::from_millis(200)).footer.loading);

        state.apply_results(Vec::new(), now + Duration::from_millis(300));
        assert!(!state.frame(now + Duration::from_millis(400)).footer.loading);
    }

    #[test]
    fn a_final_payload_identical_to_the_partial_still_completes() {
        let mut state = state();
        state.query = "query".to_string();
        let now = Instant::now();
        state.search_started = Some(now);
        let rows = vec![item("Files", None, None, None)];

        state.apply_partial(rows.clone(), now);
        state.apply_results(rows, now + Duration::from_millis(10));

        assert!(!state.frame(now + Duration::from_millis(300)).footer.loading);
    }

    #[test]
    fn the_result_dedupe_matches_the_payload_it_was_built_from() {
        let mut state = state();
        let items = vec![
            item("Files", None, None, None),
            item(
                "Firefox",
                Some("Browser"),
                Some(Action::Launch {
                    desktop_id: "firefox.desktop".to_string(),
                }),
                Some("/i.svg"),
            ),
        ];
        let now = Instant::now();
        state.apply_results(items.clone(), now);

        state.cursor.selected = 1;
        // an identical re-send must not reset the selection
        state.apply_results(items.clone(), now);
        assert_eq!(state.cursor.selected, 1);

        let changed = vec![
            item("Files", None, None, None),
            item(
                "Firefox",
                Some("Browser"),
                Some(run("firefox")),
                Some("/i.svg"),
            ),
        ];
        state.apply_results(changed, now);
        // a genuinely new payload starts from the top row: what the cursor
        // pointed at has changed
        assert_eq!(state.cursor.selected, 0);
        assert_eq!(state.rows[1].on_click.as_ref(), Some(&run("firefox")));
    }

    #[test]
    fn the_card_height_springs_towards_a_new_payload() {
        let mut state = state();
        state.query = "query".to_string();
        state.presentation.entrance_started = true;
        let now = Instant::now();
        let rows = |count: usize| {
            (0..count)
                .map(|index| item(&format!("row{index}"), None, None, None))
                .collect::<Vec<_>>()
        };
        let start = state.card_height();
        assert_eq!(start, state.layout().empty_h());

        state.apply_results(rows(4), now);
        let target = state.layout().content_h(4);
        assert_eq!(state.presentation.card_target, target);
        assert_eq!(state.card_height(), start, "the event itself does not jump");

        state.advance(now + Duration::from_millis(40));
        let mid = state.card_height();
        assert!(
            mid > start && mid < target,
            "{mid} between {start} and {target}"
        );

        state.apply_results(rows(1), now + Duration::from_millis(60));
        let lower = state.layout().content_h(1);
        assert_eq!(state.presentation.card_target, lower);
        state.advance(now + Duration::from_secs(2));
        assert_eq!(state.card_height(), lower);
    }

    #[test]
    fn reduced_motion_settles_new_rows_at_once() {
        let mut state = state();
        state.query = "query".to_string();
        state.presentation.reduce_motion = true;
        state.apply_results(vec![item("A", None, Some(run("a")), None)], Instant::now());
        assert!(state.row_motion.iter().all(Option::is_none));
    }

    #[test]
    fn a_row_from_outside_the_window_appears_in_place() {
        let mut state = state();
        state.query = "query".to_string();
        let now = Instant::now();
        let rows: Vec<_> = (0..8)
            .map(|index| {
                item(
                    &format!("r{index}"),
                    None,
                    Some(run(&format!("r{index}"))),
                    None,
                )
            })
            .collect();
        state.apply_results(rows.clone(), now);

        let mut next = rows;
        next.swap(0, 6);
        state.apply_results(next, now + Duration::from_millis(10));
        assert!(
            state.row_motion[0].is_none(),
            "r6 was off screen, no fly-in"
        );
        assert!(state.row_motion[6].is_none(), "r0 left the window");
    }

    #[test]
    fn only_a_list_appearing_from_nothing_plays_the_enter_fade() {
        let mut state = state();
        state.query = "query".to_string();
        let now = Instant::now();
        let a = item("A", None, Some(run("a")), None);
        let b = item("B", None, Some(run("b")), None);
        state.apply_results(vec![a.clone(), b], now);
        assert!(
            state
                .row_motion
                .iter()
                .flatten()
                .all(|motion| motion.alpha == 0.0)
        );

        let c = item("C", None, Some(run("c")), None);
        state.apply_results(vec![a.clone(), c.clone()], now + Duration::from_millis(300));
        assert!(
            state.row_motion.iter().all(Option::is_none),
            "a refresh swaps rows in place"
        );

        // Swapping two rows slides both instead of fading.
        state.apply_results(vec![c, a], now + Duration::from_millis(310));
        assert!(
            state
                .row_motion
                .iter()
                .flatten()
                .all(|motion| motion.alpha == 1.0)
        );
        assert_eq!(state.row_motion[0].unwrap().offset, wayrun_ui::geom::ROW_H);
    }

    #[test]
    fn a_slide_continues_from_where_the_row_is_drawn() {
        let mut state = state();
        state.query = "query".to_string();
        let now = Instant::now();
        let a = item("A", None, Some(run("a")), None);
        let b = item("B", None, Some(run("b")), None);
        state.apply_results(vec![a.clone(), b.clone()], now);
        state.apply_results(vec![b.clone(), a.clone()], now + Duration::from_millis(300));
        assert_eq!(state.row_motion[0].unwrap().offset, wayrun_ui::geom::ROW_H);

        // Halfway through that slide B is already most of the way up, so the
        // payload that sends it back down starts from there, not from slot 0.
        let halfway = now + Duration::from_millis(400);
        state.apply_results(vec![a, b], halfway);
        let offset = state.row_motion[1].unwrap().offset;
        assert!(
            offset > -wayrun_ui::geom::ROW_H && offset < -59.0,
            "carried {offset}"
        );
    }

    #[test]
    fn nothing_is_launched_without_a_target() {
        let mut state = state();
        let items = vec![item("Files", None, None, None)];
        state.apply_results(items, Instant::now());
        assert_eq!(state.selected_row(), None);
    }

    #[test]
    fn enter_uses_the_default_action_but_records_the_row_command() {
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        let mut row = item(
            "a.txt",
            None,
            Some(Action::Open {
                uri: uri.to_string(),
            }),
            None,
        );
        row.actions = vec![ActionItem {
            title: "Open in terminal".to_string(),
            action: Some(Action::Terminal {
                uri: uri.to_string(),
            }),
            children: Vec::new(),
            icon: None,
            id: Some("terminal".to_string()),
            plugin: Some("file-search".to_string()),
            default: true,
        }];
        state.apply_results(vec![row], Instant::now());

        let launch = state.selected_row().unwrap();
        // what Enter runs
        assert_eq!(
            launch.effective,
            Action::Terminal {
                uri: uri.to_string()
            }
        );
        // what usage records, so the counts stay keyed to the row
        assert_eq!(
            launch.target,
            Action::Open {
                uri: uri.to_string()
            }
        );
    }
}
