use wayrun_core::wire::{Action, ActionItem, PanelAction, ResultItem};

use super::State;
use super::cursor::Cursor;

/// The action panel (Wox-style) opened with Shift+Enter over the selected row. It
/// replaces the result list; `parent` is the row it belongs to.
pub struct Menu {
    pub parent: usize,
    pub actions: Vec<ActionItem>,
    pub cursor: Cursor,
}

impl Menu {
    /// The action the panel would run.
    pub fn selected_action(&self) -> Option<&ActionItem> {
        self.actions.get(self.cursor.selected)
    }
}

/// A panel entry, named so a re-emit can find it again: an action id survives
/// verbatim; the row's command is the id-less `Primary`, and `Pin` relabels as it lands.
pub(super) enum PanelKey {
    Id(String),
    Primary,
    Pin,
}

impl PanelKey {
    fn of(action: &ActionItem) -> Option<Self> {
        match &action.action {
            PanelAction::Pin { .. } | PanelAction::Unpin { .. } => Some(PanelKey::Pin),
            PanelAction::Execute { .. } => Some(match action.id.clone() {
                Some(id) => PanelKey::Id(id),
                None => PanelKey::Primary,
            }),
            // Removal takes the row the panel is about, so there is nothing to
            // come back to.
            PanelAction::Forget { .. } => None,
        }
    }

    fn matches(&self, action: &ActionItem) -> bool {
        match self {
            PanelKey::Id(id) => action.id.as_deref() == Some(id.as_str()),
            // The row's own command is the panel's leading entry, owned by a
            // plugin or not: a host's row has no plugin to hang it on.
            PanelKey::Primary => {
                action.id.is_none() && matches!(action.action, PanelAction::Execute { .. })
            }
            PanelKey::Pin => matches!(
                action.action,
                PanelAction::Pin { .. } | PanelAction::Unpin { .. }
            ),
        }
    }
}

/// Whether a panel entry runs the row's own command: the core's leading "Open"
/// entry and nothing else.
pub(super) fn is_row_command(row: &ResultItem, action: &ActionItem) -> bool {
    matches!(
        &action.action,
        PanelAction::Execute { command } if row.on_click.as_ref() == Some(command)
    )
}

/// The entry the panel dots: the remembered default, else the row's own command,
/// so exactly one entry shows what the row's `Enter` runs.
pub fn marked_entry(row: &ResultItem) -> Option<usize> {
    row.actions
        .iter()
        .position(|action| action.default)
        .or_else(|| {
            row.actions
                .iter()
                .position(|action| is_row_command(row, action))
        })
}

/// The action a row's `Enter` runs instead of the row's own command, which only
/// a remembered default changes.
pub fn effective_action(row: &ResultItem) -> Option<&ActionItem> {
    let action = row.actions.iter().find(|action| action.default)?;
    let PanelAction::Execute { command } = &action.action else {
        return None;
    };
    (row.on_click.as_ref() != Some(command)).then_some(action)
}

impl State {
    /// Open the selected row's action panel; whether it had one to open.
    pub fn open_actions(&mut self) -> bool {
        let Some(row) = self.rows.get(self.cursor.selected) else {
            return false;
        };
        if row.actions.is_empty() {
            return false;
        }
        self.menu = Some(Menu {
            parent: self.cursor.selected,
            actions: row.actions.clone(),
            cursor: Cursor::default(),
        });
        true
    }

    pub fn close_actions(&mut self) {
        self.menu = None;
    }

    /// Remember the panel's row and highlighted entry, so the re-emit the action
    /// is about to trigger brings the panel back instead of dropping it.
    pub fn keep_panel(&mut self, action: &ActionItem) {
        let Some(key) = PanelKey::of(action) else {
            return;
        };
        let Some(parent) = self.menu.as_ref().map(|menu| menu.parent) else {
            return;
        };
        let Some(on_click) = self.rows.get(parent).and_then(|row| row.on_click.clone()) else {
            return;
        };
        self.panel_resume = Some((on_click, key));
    }

    /// Drop a pending resume: an edit or a dismissal means the panel must not
    /// come back when the reply it asked for finally lands.
    pub fn cancel_panel_resume(&mut self) {
        self.panel_resume = None;
    }

    /// Put the panel back on the row `row_key` names with `key` highlighted. A
    /// row or entry that did not survive the re-emit leaves the panel closed.
    pub(super) fn resume_panel(&mut self, row_key: &Action, key: &PanelKey) {
        let Some(index) = self
            .rows
            .iter()
            .position(|row| row.on_click.as_ref() == Some(row_key))
        else {
            return;
        };
        self.cursor.selected = index;
        let Some(row) = self.rows.get(index) else {
            return;
        };
        if row.actions.is_empty() {
            return;
        }
        let actions = row.actions.clone();
        let selected = actions
            .iter()
            .position(|action| key.matches(action))
            .unwrap_or(0);
        self.menu = Some(Menu {
            parent: index,
            actions,
            cursor: Cursor { selected, first: 0 },
        });
        self.menu_contain();
    }

    /// The action Enter would run in the open panel.
    pub fn selected_action(&self) -> Option<ActionItem> {
        self.menu.as_ref().and_then(Menu::selected_action).cloned()
    }

    /// What `Alt+Enter` would remember for the panel's selection, or `None` if it
    /// cannot be a default: a scope and action id; a `None` id clears the scope.
    pub fn selected_default(&self) -> Option<(&str, Option<&str>)> {
        let menu = self.menu.as_ref()?;
        let action = menu.selected_action()?;
        let plugin = action.plugin.as_deref();
        if action.default {
            return Some((plugin?, None));
        }
        if let Some(id) = action.id.as_deref() {
            return Some((plugin?, Some(id)));
        }
        let row = self.rows.get(menu.parent)?;
        if !is_row_command(row, action) {
            return None;
        }
        // The row's own command is the implicit default, so picking it drops what
        // the row remembered — whichever plugin's scope remembered it.
        row.actions
            .iter()
            .find(|action| action.default)
            .and_then(|action| action.plugin.as_deref())
            .or(plugin)
            .map(|scope| (scope, None))
    }

    /// The title of the row the panel belongs to, for its header.
    pub fn menu_parent_title(&self) -> Option<&str> {
        let menu = self.menu.as_ref()?;
        self.rows.get(menu.parent).map(|row| row.title.as_str())
    }

    /// The command of the row the open panel belongs to, which is what a panel
    /// `pin` names it by.
    pub fn menu_parent_command(&self) -> Option<Action> {
        let menu = self.menu.as_ref()?;
        self.rows.get(menu.parent)?.on_click.clone()
    }

    pub fn menu_up(&mut self) {
        if let Some(menu) = &mut self.menu {
            menu.cursor.up();
        }
        self.menu_contain();
    }

    pub fn menu_down(&mut self) {
        if let Some(menu) = &mut self.menu {
            menu.cursor.down(menu.actions.len());
        }
        self.menu_contain();
    }

    /// The wheel moves the panel's selection, like the arrows.
    pub fn menu_scroll(&mut self, rows: i32) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        if menu.actions.is_empty() || rows == 0 {
            return;
        }
        menu.cursor.scroll(rows, menu.actions.len());
        self.menu_contain();
    }

    pub fn menu_page_up(&mut self) {
        let page = self.appearance.layout.max_rows;
        if let Some(menu) = &mut self.menu {
            menu.cursor.page_up(page);
        }
        self.menu_contain();
    }

    pub fn menu_page_down(&mut self) {
        let page = self.appearance.layout.max_rows;
        if let Some(menu) = &mut self.menu {
            menu.cursor.page_down(menu.actions.len(), page);
        }
        self.menu_contain();
    }

    fn menu_contain(&mut self) {
        let layout = self.appearance.layout;
        if let Some(menu) = &mut self.menu {
            menu.cursor.contain(menu.actions.len(), layout);
        }
        self.resync_hover();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{
        file_row, item, pin_entry, plugin_action, run, state, with_actions,
    };

    #[test]
    fn the_rows_own_command_clears_a_remembered_default() {
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        let execute =
            |title: &str, command: Action, id: Option<&str>, plugin: Option<&str>| ActionItem {
                title: title.to_string(),
                action: PanelAction::Execute { command },
                icon: None,
                id: id.map(str::to_string),
                plugin: plugin.map(str::to_string),
                default: false,
            };
        let mut row = item(
            "a.txt",
            None,
            Some(Action::Open {
                uri: uri.to_string(),
            }),
            None,
        );
        let mut terminal = execute(
            "Open in terminal",
            Action::Terminal {
                uri: uri.to_string(),
            },
            Some("terminal"),
            Some("file-search"),
        );
        terminal.default = true;
        row.actions = vec![
            execute(
                "Open",
                Action::Open {
                    uri: uri.to_string(),
                },
                None,
                Some("file-search"),
            ),
            execute(
                "Copy path",
                Action::Run {
                    cmd: "true".to_string(),
                },
                Some("copy_path"),
                Some("file-search"),
            ),
            terminal,
            ActionItem {
                title: "Pin to top".to_string(),
                action: PanelAction::Pin {
                    scope: String::new(),
                },
                icon: None,
                id: None,
                plugin: None,
                default: false,
            },
        ];
        state.apply_results(vec![row], std::time::Instant::now());
        assert!(state.open_actions());

        fn remember(state: &mut State, selected: usize) -> Option<(String, Option<String>)> {
            state.menu.as_mut().unwrap().cursor.selected = selected;
            state
                .selected_default()
                .map(|(plugin, id)| (plugin.to_string(), id.map(str::to_string)))
        }
        // the row's own command is the implicit default: picking it clears the
        // scope, so a row can always go back to opening normally
        assert_eq!(
            remember(&mut state, 0),
            Some(("file-search".to_string(), None))
        );
        // a plugin action is remembered by id, and re-picking it clears it
        assert_eq!(
            remember(&mut state, 1),
            Some(("file-search".to_string(), Some("copy_path".to_string())))
        );
        assert_eq!(
            remember(&mut state, 2),
            Some(("file-search".to_string(), None))
        );
        // a launcher-level action belongs to no plugin
        assert_eq!(remember(&mut state, 3), None);
    }

    #[test]
    fn the_panel_opens_over_the_selected_row_and_runs_its_action() {
        let mut state = state();
        let now = std::time::Instant::now();
        state.apply_results(
            vec![
                item(
                    "Files",
                    None,
                    Some(Action::Open {
                        uri: "file:///tmp".to_string(),
                    }),
                    None,
                ),
                item(
                    "Firefox",
                    None,
                    Some(Action::Launch {
                        desktop_id: "firefox.desktop".to_string(),
                    }),
                    None,
                ),
            ],
            now,
        );
        state.cursor.selected = 1;
        state.rows[1].actions = vec![ActionItem {
            title: "Pin to top".to_string(),
            action: PanelAction::Pin {
                scope: String::new(),
            },
            icon: None,
            id: None,
            plugin: None,
            default: false,
        }];

        assert!(state.open_actions());
        assert_eq!(state.menu_parent_title(), Some("Firefox"));
        assert_eq!(state.selected_action().unwrap().title, "Pin to top");
        // the panel is taller than the one-row list it replaced
        assert_eq!(state.content_height(), state.appearance.layout.panel_h(1));

        state.close_actions();
        assert!(state.menu.is_none());
        assert_eq!(state.content_height(), state.appearance.layout.content_h(2));
    }

    #[test]
    fn a_pinned_row_keeps_its_badge_and_offers_unpin() {
        let mut state = state();
        let mut row = item(
            "YouTube",
            None,
            Some(Action::Open {
                uri: "https://youtube.com/".to_string(),
            }),
            None,
        );
        row.badge = Some("/usr/share/icons/Papirus/24x24/actions/pin.svg".to_string());
        row.actions = vec![ActionItem {
            title: "Unpin".to_string(),
            action: PanelAction::Unpin {
                scope: "b youtube".to_string(),
                on_click: Action::Open {
                    uri: "https://youtube.com/".to_string(),
                },
            },
            icon: None,
            id: None,
            plugin: None,
            default: false,
        }];
        state.apply_results(vec![row], std::time::Instant::now());

        assert!(state.rows[0].badge.is_some());
        assert!(state.open_actions());
        assert_eq!(state.selected_action().unwrap().title, "Unpin");
    }

    #[test]
    fn a_row_without_actions_has_no_panel() {
        let mut state = state();
        state.apply_results(
            vec![item(
                "Files",
                None,
                Some(Action::Open {
                    uri: "file:///tmp".to_string(),
                }),
                None,
            )],
            std::time::Instant::now(),
        );
        assert!(!state.open_actions());
        assert!(state.menu.is_none());
    }

    #[test]
    fn a_new_payload_closes_the_panel() {
        let mut state = state();
        let now = std::time::Instant::now();
        state.apply_results(
            vec![with_actions(
                item("a", None, Some(run("a")), None),
                &["Pin to top"],
            )],
            now,
        );
        assert!(state.open_actions());

        state.apply_results(vec![item("b", None, Some(run("b")), None)], now);
        assert!(state.menu.is_none(), "a new list invalidates the panel");
    }

    #[test]
    fn the_panel_dots_the_entry_enter_runs() {
        let uri = "file:///tmp/a.txt";
        let open = ActionItem {
            title: "Open".to_string(),
            action: PanelAction::Execute {
                command: Action::Open {
                    uri: uri.to_string(),
                },
            },
            icon: None,
            id: None,
            plugin: Some("file-search".to_string()),
            default: false,
        };
        let reveal = plugin_action(
            "Reveal in file manager",
            Action::Reveal {
                uri: uri.to_string(),
            },
            "reveal",
            false,
        );
        let terminal = |default| {
            plugin_action(
                "Open in terminal",
                Action::Terminal {
                    uri: uri.to_string(),
                },
                "terminal",
                default,
            )
        };

        let mut row = file_row(uri, vec![open.clone(), reveal.clone(), terminal(false)]);
        assert_eq!(
            marked_entry(&row),
            Some(0),
            "no remembered default: the row's own command is what Enter runs"
        );

        row.actions = vec![open, reveal, terminal(true)];
        assert_eq!(
            marked_entry(&row),
            Some(2),
            "the remembered action carries it"
        );
    }

    #[test]
    fn picking_the_row_command_clears_the_scope_that_remembered_it() {
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
        // A host row: its actions carry the host's scope, and the leading Open
        // entry carries none of its own.
        row.actions = vec![
            ActionItem {
                title: "Open".to_string(),
                action: PanelAction::Execute {
                    command: Action::Open {
                        uri: uri.to_string(),
                    },
                },
                icon: None,
                id: None,
                plugin: None,
                default: false,
            },
            ActionItem {
                title: "Mark done".to_string(),
                action: PanelAction::Execute {
                    command: Action::Run {
                        cmd: "done".to_string(),
                    },
                },
                icon: None,
                id: Some("done".to_string()),
                plugin: Some("todo".to_string()),
                default: true,
            },
        ];
        state.apply_results(vec![row], std::time::Instant::now());
        assert!(state.open_actions());

        assert_eq!(
            state.selected_default(),
            Some(("todo", None)),
            "the scope that remembered the action is the one to clear"
        );
    }

    #[test]
    fn a_default_gesture_keeps_the_panel_on_the_entry_it_marked() {
        let now = std::time::Instant::now();
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        let reveal = |default| {
            plugin_action(
                "Reveal in file manager",
                Action::Reveal {
                    uri: uri.to_string(),
                },
                "reveal",
                default,
            )
        };
        let terminal = |default| {
            plugin_action(
                "Open in terminal",
                Action::Terminal {
                    uri: uri.to_string(),
                },
                "terminal",
                default,
            )
        };
        state.apply_results(
            vec![file_row(uri, vec![reveal(false), terminal(false)])],
            now,
        );
        assert!(state.open_actions());
        state.menu.as_mut().unwrap().cursor.selected = 1;

        // Alt+Enter on "Open in terminal": the reply carries the dot and the
        // panel must land back on that entry, not on the top of the list.
        let chosen = state.selected_action().unwrap();
        state.keep_panel(&chosen);
        state.apply_results(
            vec![file_row(uri, vec![reveal(false), terminal(true)])],
            now,
        );

        let menu = state.menu.as_ref().expect("the panel comes back");
        assert_eq!(menu.cursor.selected, 1);
        assert!(
            menu.actions[1].default,
            "the highlighted entry is the marked one"
        );
        assert_eq!(state.cursor.selected, 0, "the panel's row is the selection");
    }

    #[test]
    fn a_pin_gesture_keeps_the_panel_on_the_entry_it_flipped() {
        let now = std::time::Instant::now();
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        state.apply_results(vec![file_row(uri, vec![pin_entry(uri, false)])], now);
        assert!(state.open_actions());

        let chosen = state.selected_action().unwrap();
        state.keep_panel(&chosen);
        state.apply_results(vec![file_row(uri, vec![pin_entry(uri, true)])], now);

        let menu = state.menu.as_ref().expect("the panel comes back");
        assert_eq!(menu.actions[menu.cursor.selected].title, "Unpin");
    }

    #[test]
    fn a_panel_gesture_on_a_vanished_row_leaves_the_panel_closed() {
        let now = std::time::Instant::now();
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        state.apply_results(vec![file_row(uri, vec![pin_entry(uri, false)])], now);
        assert!(state.open_actions());

        let chosen = state.selected_action().unwrap();
        state.keep_panel(&chosen);
        state.apply_results(vec![item("b", None, Some(run("b")), None)], now);
        assert!(state.menu.is_none(), "the panel's row is gone");
    }

    #[test]
    fn a_resume_dropped_before_the_reply_lands_leaves_the_panel_closed() {
        let now = std::time::Instant::now();
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        let row = file_row(uri, vec![pin_entry(uri, false)]);
        state.apply_results(vec![row.clone()], now);
        assert!(state.open_actions());

        // the row is still there and the payload changed: only the dropped resume keeps
        // the panel closed; a dismissal or an edit must not let a slow reply resurrect it
        let chosen = state.selected_action().unwrap();
        state.keep_panel(&chosen);
        state.hidden();
        state.apply_results(vec![file_row(uri, vec![pin_entry(uri, true)])], now);
        assert!(state.menu.is_none());

        state.apply_results(vec![row], now);
        assert!(state.open_actions());
        let chosen = state.selected_action().unwrap();
        state.keep_panel(&chosen);
        state.cancel_panel_resume();
        state.apply_results(vec![file_row(uri, vec![pin_entry(uri, true)])], now);
        assert!(state.menu.is_none(), "an edit cancels the pending resume");
    }

    #[test]
    fn the_panel_window_follows_the_selection_past_max_rows() {
        let mut state = state();
        let now = std::time::Instant::now();
        let actions: Vec<&str> = (0..10).map(|_| "act").collect();
        state.apply_results(
            vec![with_actions(
                item("a", None, Some(run("a")), None),
                &actions,
            )],
            now,
        );
        assert!(state.open_actions());

        let max = state.appearance.layout.max_rows;
        state.menu_down();
        state.menu_down();
        state.menu_down();
        state.menu_down();
        state.menu_down();
        assert_eq!(state.menu.as_ref().unwrap().cursor.selected, 5);
        assert_eq!(state.menu.as_ref().unwrap().cursor.first, 5 - max + 1);

        state.menu_page_up();
        assert_eq!(state.menu.as_ref().unwrap().cursor.selected, 5 - max);
    }
}
