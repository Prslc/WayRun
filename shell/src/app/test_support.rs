use wayrun_core::wire::{Action, ActionItem, PanelAction, ResultItem};

use super::State;

pub(super) fn run(cmd: &str) -> Action {
    Action::Run {
        cmd: cmd.to_string(),
    }
}

pub(super) fn item(
    title: &str,
    summary: Option<&str>,
    on_click: Option<Action>,
    icon: Option<&str>,
) -> ResultItem {
    ResultItem {
        title: title.into(),
        summary: summary.map(str::to_string),
        on_click,
        icon: icon.map(str::to_string),
        ephemeral: false,
        actions: Vec::new(),
        badge: None,
    }
}

pub(super) fn state() -> State {
    let mut state = State::new();
    state.surface = (1920, 1080);
    state
}

pub(super) fn with_actions(mut row: ResultItem, titles: &[&str]) -> ResultItem {
    row.actions = titles
        .iter()
        .map(|title| ActionItem {
            title: title.to_string(),
            action: PanelAction::Execute {
                command: run(title),
            },
            icon: None,
            id: None,
            plugin: None,
            default: false,
        })
        .collect();
    row
}

/// A file row: the row's own command, with the panel entries a plugin and
/// the launcher put on it.
pub(super) fn file_row(uri: &str, actions: Vec<ActionItem>) -> ResultItem {
    let mut row = item(
        "a.txt",
        None,
        Some(Action::Open {
            uri: uri.to_string(),
        }),
        None,
    );
    row.actions = actions;
    row
}

pub(super) fn plugin_action(title: &str, command: Action, id: &str, default: bool) -> ActionItem {
    ActionItem {
        title: title.to_string(),
        action: PanelAction::Execute { command },
        icon: None,
        id: Some(id.to_string()),
        plugin: Some("file-search".to_string()),
        default,
    }
}

pub(super) fn pin_entry(uri: &str, unpin: bool) -> ActionItem {
    let on_click = Action::Open {
        uri: uri.to_string(),
    };
    let action = if unpin {
        PanelAction::Unpin {
            scope: "f a".to_string(),
            on_click,
        }
    } else {
        PanelAction::Pin {
            scope: "f a".to_string(),
        }
    };
    ActionItem {
        title: if unpin { "Unpin" } else { "Pin to top" }.to_string(),
        action,
        icon: None,
        id: None,
        plugin: None,
        default: false,
    }
}
