use std::collections::HashMap;
use std::time::Instant;

use wayrun_core::wire::{Action, ResultItem};

use crate::ui::present::{ROW_RISE, RowMotion};

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
        let effective = row
            .actions
            .iter()
            .find(|action| action.default)
            .map(|action| action.action.clone())
            .unwrap_or_else(|| target.clone());
        Some(Launch { target, effective })
    }

    pub fn apply_results(&mut self, items: Vec<ResultItem>, now: Instant) {
        let items = normalize_icons(items);
        // A panel action holds its place across the reply it asked for.
        let resume = self.panel_resume.take();
        // An identical re-send is dropped so it cannot reset the selection.
        if self.rows == items && !self.rows.is_empty() {
            return;
        }

        // The selection resets to row 0 below, so the new window starts at 0 and
        // `index` is its screen slot; the old offset is measured from the old one.
        let max_rows = self.layout().max_rows;
        let old_first = self.cursor.first as isize;
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
                    enter: true,
                    started: now,
                };
                if index >= max_rows {
                    return None;
                }
                match old.get(&row_identity(item)) {
                    // Only a row that was on screen slides; one from outside the
                    // window would otherwise fly in from far below the card.
                    Some(&previous)
                        if (0..max_rows as isize).contains(&(previous as isize - old_first)) =>
                    {
                        let from_slot = previous as isize - old_first;
                        (from_slot != index as isize).then_some(RowMotion {
                            offset: (from_slot as f32 - index as f32) * crate::ui::geom::ROW_H,
                            enter: false,
                            started: now,
                        })
                    }
                    Some(_) => Some(enter),
                    None => Some(enter),
                }
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
    use wayrun_core::wire::ActionItem;

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
        let now = std::time::Instant::now();
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
        let now = std::time::Instant::now();
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

        state.advance(now + std::time::Duration::from_millis(40));
        let mid = state.card_height();
        assert!(
            mid > start && mid < target,
            "{mid} between {start} and {target}"
        );

        state.apply_results(rows(1), now + std::time::Duration::from_millis(60));
        let lower = state.layout().content_h(1);
        assert_eq!(state.presentation.card_target, lower);
        state.advance(now + std::time::Duration::from_secs(2));
        assert_eq!(state.card_height(), lower);
    }

    #[test]
    fn reduced_motion_settles_new_rows_at_once() {
        let mut state = state();
        state.query = "query".to_string();
        state.presentation.reduce_motion = true;
        state.apply_results(
            vec![item("A", None, Some(run("a")), None)],
            std::time::Instant::now(),
        );
        assert!(state.row_motion.iter().all(Option::is_none));
    }

    #[test]
    fn a_row_from_outside_the_window_enters_instead_of_sliding() {
        let mut state = state();
        state.query = "query".to_string();
        let now = std::time::Instant::now();
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
        state.apply_results(next, now + std::time::Duration::from_millis(10));
        assert!(
            state.row_motion[0].is_some_and(|motion| motion.enter),
            "r6 was off screen and enters"
        );
        assert!(state.row_motion[6].is_none(), "r0 left the window");
    }

    #[test]
    fn only_rows_new_to_the_payload_animate_in() {
        let mut state = state();
        state.query = "query".to_string();
        let now = std::time::Instant::now();
        let a = item("A", None, Some(run("a")), None);
        let b = item("B", None, Some(run("b")), None);
        state.apply_results(vec![a.clone(), b], now);
        assert!(state.row_motion.iter().all(Option::is_some));
        assert!(state.row_motion.iter().flatten().all(|motion| motion.enter));

        let later = now + std::time::Duration::from_millis(60);
        let c = item("C", None, Some(run("c")), None);
        state.apply_results(vec![a.clone(), c.clone()], later);
        assert!(state.row_motion[0].is_none(), "A kept its place");
        assert!(state.row_motion[1].is_some_and(|motion| motion.enter));

        // Swapping two rows slides both instead of fading.
        state.apply_results(vec![c, a], later);
        assert!(
            state
                .row_motion
                .iter()
                .flatten()
                .all(|motion| !motion.enter)
        );
        assert_eq!(state.row_motion[0].unwrap().offset, crate::ui::geom::ROW_H);
    }

    #[test]
    fn nothing_is_launched_without_a_target() {
        let mut state = state();
        let items = vec![item("Files", None, None, None)];
        state.apply_results(items, std::time::Instant::now());
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
            action: Action::Terminal {
                uri: uri.to_string(),
            },
            icon: None,
            id: Some("terminal".to_string()),
            plugin: Some("file-search".to_string()),
            default: true,
        }];
        state.apply_results(vec![row], std::time::Instant::now());

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
