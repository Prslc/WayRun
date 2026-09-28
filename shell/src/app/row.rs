use std::time::Instant;

use wayrun_core::wire::{Action, PanelAction, ResultItem};

use super::State;

/// The selected row's fields that `select` and the launch command need.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub title: String,
    pub summary: Option<String>,
    pub icon: Option<String>,
    /// The row's own command, recorded in usage so the counts stay keyed to it.
    pub target: Action,
    /// The command Enter runs: the remembered default action when the row has
    /// one, else `target`.
    pub effective: Action,
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
            .map(|action| {
                let PanelAction::Execute { command } = &action.action;
                command.clone()
            })
            .unwrap_or_else(|| target.clone());
        Some(Launch {
            title: row.title.clone(),
            summary: row.summary.clone(),
            icon: row.icon.clone(),
            target,
            effective,
        })
    }

    pub fn apply_results(&mut self, items: Vec<ResultItem>, now: Instant) {
        let items = normalize_icons(items);
        // A panel action holds its place across the reply it asked for.
        let resume = self.panel_resume.take();
        // An identical re-send is dropped so it cannot reset the selection.
        if self.rows == items && !self.rows.is_empty() {
            return;
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
            action: PanelAction::Execute {
                command: Action::Terminal {
                    uri: uri.to_string(),
                },
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
