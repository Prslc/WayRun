use super::model::Entry;
use super::registry::{REGISTRY, ensure_loaded};
use crate::system::icon::find_icon_path;
use crate::wire::{Action, ActionItem, PanelAction, ResultItem};
use rust_i18n::t;

/// Drop a row across the registry. `true` when an external host owned and dropped
/// it, so `forget` answers truthfully instead of claiming a deletion.
pub async fn forget_row(command: &Action) -> bool {
    ensure_loaded().await;
    let reg = REGISTRY.read().await;
    let mut owned = false;
    for entry in reg.iter() {
        owned |= entry.plugin.forget(command).await.unwrap_or(false);
    }
    owned
}

/// A pin's scope is the exact trimmed query, so a bare keyword never summons it;
/// `?` is help, not a result set, so it has no pins.
pub(super) fn pin_scope(input: &str) -> Option<&str> {
    let input = input.trim();
    (input != "?").then_some(input)
}

/// Prepend the scope's pins and attach each row's actions. A pinned row is
/// re-emitted from storage, so its fresh copy is dropped as a duplicate.
pub async fn decorate(items: Vec<ResultItem>, scope: &str, history: bool) -> Vec<ResultItem> {
    ensure_loaded().await;
    let pins: Vec<ResultItem> = crate::system::pins::get_pins(scope)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect();
    let (mut out, pinned) = merge_pins(pins, items);
    // One query for every plugin's remembered default, not one per row.
    let defaults = crate::system::defaults::all().unwrap_or_default();

    // One registry read for the whole list; `Plugin::actions` is synchronous,
    // so the guard never spans an await.
    let reg = REGISTRY.read().await;
    for item in &mut out {
        // A row without a click command gets no panel at all, so it needs no
        // scan; the rest ask the plugins which of them owns them.
        let plugin_actions = if item.on_click.is_some() {
            plugin_actions(&reg, item)
        } else {
            (None, Vec::new())
        };
        attach_actions(item, scope, &pinned, history, &defaults, plugin_actions);
    }
    out
}

/// Pins lead the results, deduplicated, so each appears once at the top. Returns
/// the merged list and the pinned commands the action labels use.
fn merge_pins(
    mut pins: Vec<ResultItem>,
    mut results: Vec<ResultItem>,
) -> (Vec<ResultItem>, Vec<Action>) {
    let pinned: Vec<Action> = pins
        .iter()
        .filter_map(|item| item.on_click.clone())
        .collect();
    results.retain(|item| {
        !item
            .on_click
            .as_ref()
            .is_some_and(|on_click| pinned.iter().any(|pin| pin == on_click))
    });
    pins.append(&mut results);
    (pins, pinned)
}

/// The first plugin that recognises the row and declares actions for it, with
/// the plugin's id, which scopes a remembered default action.
fn plugin_actions(entries: &[Entry], item: &ResultItem) -> (Option<String>, Vec<ActionItem>) {
    for entry in entries {
        let actions = entry.plugin.actions(item);
        if !actions.is_empty() {
            return (Some(entry.plugin.meta().id.to_string()), actions);
        }
    }
    (None, Vec::new())
}

/// The launcher-level entries an actionable row gets (pin/unpin always, history
/// removal only on a recordable history row), after its type and host actions.
fn attach_actions(
    item: &mut ResultItem,
    scope: &str,
    pinned: &[Action],
    history: bool,
    defaults: &std::collections::HashMap<String, String>,
    plugin_actions: (Option<String>, Vec<ActionItem>),
) {
    let Some(on_click) = item.on_click.clone() else {
        return;
    };
    let (owner, mut plugin_actions) = plugin_actions;
    let is_pinned = pinned.iter().any(|pin| pin == &on_click);
    if is_pinned {
        item.badge = find_icon_path("builtin:pin");
    }

    // The pin stores the row as it arrived: its host actions, and none of the
    // launcher's own entries, so a pinned row never embeds the action storing it.
    let pin_snapshot = (!is_pinned && !item.ephemeral).then(|| Box::new(item.clone()));

    let mut actions: Vec<ActionItem> = Vec::new();
    actions.append(&mut plugin_actions);
    actions.append(&mut item.actions);

    // Every plugin action carries its owner, so the shell can scope a default.
    if let Some(owner) = &owner {
        for action in &mut actions {
            if action.plugin.is_none() {
                action.plugin = Some(owner.clone());
            }
        }
    }

    // A remembered default elevates its action to Enter, whichever plugin the
    // scope belongs to: a host's action is remembered under the host's id.
    for action in &mut actions {
        action.default = action
            .plugin
            .as_deref()
            .and_then(|plugin| defaults.get(plugin))
            .is_some_and(|id| action.id.as_deref() == Some(id.as_str()));
    }

    // Launcher-level entries come last: they are launcher state rather than what
    // the row offers, and the first slot belongs to the row's own command.
    if is_pinned {
        actions.push(ActionItem {
            title: t!("action.unpin"),
            action: PanelAction::Unpin {
                scope: scope.to_string(),
                on_click: on_click.clone(),
            },
            icon: Some("builtin:unpin".to_string()),
            id: None,
            plugin: None,
            default: false,
        });
    } else if let Some(pinned_row) = pin_snapshot {
        actions.push(ActionItem {
            title: t!("action.pin"),
            action: PanelAction::Pin {
                scope: scope.to_string(),
                item: pinned_row,
            },
            icon: Some("builtin:pin".to_string()),
            id: None,
            plugin: None,
            default: false,
        });
    }

    if history && crate::system::usage::is_recordable(item.ephemeral, Some(&on_click)) {
        actions.push(ActionItem {
            title: t!("action.remove_history"),
            action: PanelAction::Forget {
                on_click: on_click.clone(),
            },
            icon: Some("builtin:remove".to_string()),
            id: None,
            plugin: None,
            default: false,
        });
    }

    // The row's own command leads the panel: Enter runs it while no default is
    // remembered; re-picking takes the default back, and a lone command shows no panel.
    if !actions.is_empty() {
        actions.insert(
            0,
            ActionItem {
                title: t!("action.open"),
                action: PanelAction::Execute {
                    command: on_click.clone(),
                },
                icon: Some("builtin:open".to_string()),
                id: None,
                plugin: owner.clone(),
                default: false,
            },
        );
    }

    // Core and plugin actions carry `builtin:` specs; a host action's icon is
    // already absolute, so anything unresolved keeps no icon.
    for action in &mut actions {
        action.icon = action
            .icon
            .as_deref()
            .filter(|spec| !spec.is_empty())
            .and_then(find_icon_path);
    }

    item.actions = actions;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::item;
    use std::collections::HashMap;

    #[test]
    fn a_pin_is_scoped_to_the_exact_query_not_its_keyword() {
        assert_eq!(pin_scope("b firefox"), Some("b firefox"));
        assert_eq!(pin_scope("b"), Some("b"));
        assert_eq!(pin_scope("  firefox  "), Some("firefox"));
        // the empty query is the history view, which still has its own scope
        assert_eq!(pin_scope(""), Some(""));
        // `?` is the help table, never a pin scope
        assert_eq!(pin_scope("?"), None);
        assert_eq!(pin_scope("  ?  "), None);
    }

    fn run(cmd: &str) -> Action {
        Action::Run {
            cmd: cmd.to_string(),
        }
    }

    #[test]
    fn pinned_rows_lead_and_their_duplicate_is_dropped() {
        let pins = vec![item("Pinned", run("pinned"))];
        let results = vec![item("Other", run("other")), item("Pinned", run("pinned"))];

        let (merged, pinned) = merge_pins(pins, results);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].title, "Pinned");
        assert_eq!(merged[1].title, "Other");
        assert_eq!(pinned, [run("pinned")]);
    }

    #[test]
    fn launcher_entries_carry_the_rows_scope() {
        let mut row = item(
            "Firefox",
            Action::Launch {
                desktop_id: "firefox.desktop".to_string(),
            },
        );
        attach_actions(
            &mut row,
            "b",
            &[],
            false,
            &HashMap::new(),
            (None, Vec::new()),
        );

        let titles: Vec<&str> = row
            .actions
            .iter()
            .map(|action| action.title.as_str())
            .collect();
        assert_eq!(titles, [t!("action.open"), t!("action.pin")]);
        match &row.actions[1].action {
            PanelAction::Pin { scope, item } => {
                assert_eq!(scope, "b");
                assert_eq!(item.title, "Firefox");
            }
            other => panic!("expected a pin, got {other:?}"),
        }
        assert!(row.badge.is_none(), "an unpinned row carries no badge");
    }

    #[test]
    fn the_row_command_leads_and_launcher_entries_trail() {
        let mut row = item(
            "a.txt",
            Action::Open {
                uri: "file:///tmp/a.txt".to_string(),
            },
        );
        let reveal = ActionItem {
            title: "Reveal in file manager".to_string(),
            action: PanelAction::Execute {
                command: Action::Reveal {
                    uri: "file:///tmp/a.txt".to_string(),
                },
            },
            icon: None,
            id: Some("reveal".to_string()),
            plugin: None,
            default: false,
        };
        attach_actions(
            &mut row,
            "",
            &[Action::Open {
                uri: "file:///tmp/a.txt".to_string(),
            }],
            true,
            &HashMap::new(),
            (Some("file-search".to_string()), vec![reveal]),
        );

        let titles: Vec<&str> = row
            .actions
            .iter()
            .map(|action| action.title.as_str())
            .collect();
        assert_eq!(
            titles,
            [
                t!("action.open"),
                "Reveal in file manager".to_string(),
                t!("action.unpin"),
                t!("action.remove_history")
            ]
        );
        assert!(row.badge.is_some(), "a pinned row carries the pin badge");
    }

    #[test]
    fn the_pin_snapshot_keeps_host_actions_but_not_launcher_ones() {
        let mut row = item(
            "Firefox",
            Action::Launch {
                desktop_id: "firefox.desktop".to_string(),
            },
        );
        row.actions = vec![ActionItem {
            title: "Host".to_string(),
            action: PanelAction::Execute {
                command: run("host"),
            },
            icon: None,
            id: None,
            plugin: None,
            default: false,
        }];
        attach_actions(
            &mut row,
            "b",
            &[],
            false,
            &HashMap::new(),
            (None, Vec::new()),
        );

        // The snapshot is the host's row, not the decorated one: the pin action
        // must not embed itself, and the host action must survive the round trip.
        let pinned = row
            .actions
            .iter()
            .find_map(|action| match &action.action {
                PanelAction::Pin { item, .. } => Some(item),
                _ => None,
            })
            .expect("the row offers a pin");
        assert_eq!(pinned.actions.len(), 1);
        assert_eq!(pinned.actions[0].title, "Host");
    }

    #[test]
    fn launcher_entries_trail_the_row() {
        fn titles(row: &ResultItem) -> Vec<&str> {
            row.actions.iter().map(|a| a.title.as_str()).collect()
        }

        let mut history = item(
            "Firefox",
            Action::Launch {
                desktop_id: "firefox.desktop".to_string(),
            },
        );
        attach_actions(
            &mut history,
            "",
            &[],
            true,
            &HashMap::new(),
            (None, Vec::new()),
        );
        assert_eq!(
            titles(&history),
            [
                t!("action.open"),
                t!("action.pin"),
                t!("action.remove_history")
            ]
        );

        // a fresh search result is not sourced from the history view
        let mut search = item(
            "Firefox",
            Action::Launch {
                desktop_id: "firefox.desktop".to_string(),
            },
        );
        attach_actions(
            &mut search,
            "fire",
            &[],
            false,
            &HashMap::new(),
            (None, Vec::new()),
        );
        assert_eq!(titles(&search), [t!("action.open"), t!("action.pin")]);

        // an ephemeral row is neither a durable pin target nor recorded
        let mut one_shot = item("window", run("wctl activate 1"));
        one_shot.ephemeral = true;
        attach_actions(
            &mut one_shot,
            "",
            &[],
            true,
            &HashMap::new(),
            (None, Vec::new()),
        );
        assert!(
            titles(&one_shot).is_empty(),
            "a row with nothing else to offer has no panel"
        );

        // an existing pin must stay removable even on an ephemeral row
        let mut pinned = item("clip", run("cliphist decode 1"));
        pinned.ephemeral = true;
        attach_actions(
            &mut pinned,
            "",
            &[run("cliphist decode 1")],
            true,
            &HashMap::new(),
            (None, Vec::new()),
        );
        assert_eq!(titles(&pinned), [t!("action.open"), t!("action.unpin")]);

        // a copy row is not recorded, but it is a stable pin target
        let mut copy = item(
            "translated",
            Action::Copy {
                text: "hi".to_string(),
            },
        );
        attach_actions(
            &mut copy,
            "",
            &[],
            true,
            &HashMap::new(),
            (None, Vec::new()),
        );
        assert_eq!(titles(&copy), [t!("action.open"), t!("action.pin")]);
    }

    #[test]
    fn a_host_action_is_remembered_under_the_hosts_scope() {
        let mut row = item(
            "repo",
            Action::Run {
                cmd: "true".to_string(),
            },
        );
        row.actions = vec![ActionItem {
            title: "Mark done".to_string(),
            action: PanelAction::Execute {
                command: run("done"),
            },
            icon: None,
            id: Some("done".to_string()),
            plugin: Some("todo".to_string()),
            default: false,
        }];
        let mut defaults = HashMap::new();
        defaults.insert("todo".to_string(), "done".to_string());
        attach_actions(
            &mut row,
            "todo x",
            &[],
            false,
            &defaults,
            (None, Vec::new()),
        );

        let titles: Vec<&str> = row.actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(
            titles,
            [t!("action.open"), "Mark done".to_string(), t!("action.pin")]
        );
        let marked = row.actions.iter().find(|a| a.default).unwrap();
        assert_eq!(marked.id.as_deref(), Some("done"));
        assert_eq!(marked.plugin.as_deref(), Some("todo"));
    }

    #[test]
    fn a_host_action_survives_a_built_in_claiming_the_row() {
        // The row's command is a file, so file-search decorates it too; the
        // host's own action still answers to the host's remembered default.
        let uri = "file:///tmp/a.txt";
        let mut row = item(
            "a.txt",
            Action::Open {
                uri: uri.to_string(),
            },
        );
        row.actions = vec![ActionItem {
            title: "Mark done".to_string(),
            action: PanelAction::Execute {
                command: run("done"),
            },
            icon: None,
            id: Some("done".to_string()),
            plugin: Some("todo".to_string()),
            default: false,
        }];
        let mut defaults = HashMap::new();
        defaults.insert("todo".to_string(), "done".to_string());
        attach_actions(
            &mut row,
            "todo x",
            &[],
            false,
            &defaults,
            (
                Some("file-search".to_string()),
                vec![file_action(
                    "Reveal in file manager",
                    "reveal",
                    Action::Reveal {
                        uri: uri.to_string(),
                    },
                )],
            ),
        );

        let marked: Vec<&str> = row
            .actions
            .iter()
            .filter(|a| a.default)
            .map(|a| a.title.as_str())
            .collect();
        assert_eq!(marked, ["Mark done"], "exactly one entry is the default");
    }

    fn file_action(title: &str, id: &str, command: Action) -> ActionItem {
        ActionItem {
            title: title.to_string(),
            action: PanelAction::Execute { command },
            icon: None,
            id: Some(id.to_string()),
            plugin: None,
            default: false,
        }
    }

    #[test]
    fn a_remembered_default_is_marked_and_keeps_the_row_command() {
        let uri = "file:///tmp/a.txt";
        let mut row = item(
            "a.txt",
            Action::Open {
                uri: uri.to_string(),
            },
        );
        let actions = vec![
            file_action(
                "Reveal in file manager",
                "reveal",
                Action::Reveal {
                    uri: uri.to_string(),
                },
            ),
            file_action(
                "Open in terminal",
                "terminal",
                Action::Terminal {
                    uri: uri.to_string(),
                },
            ),
        ];
        let mut defaults = HashMap::new();
        defaults.insert("file-search".to_string(), "terminal".to_string());
        attach_actions(
            &mut row,
            "",
            &[],
            false,
            &defaults,
            (Some("file-search".to_string()), actions),
        );

        // the row's own command leads the panel, owned by the plugin so the
        // panel can make it the default again
        let open = &row.actions[0];
        assert_eq!(open.title, t!("action.open"));
        assert!(open.id.is_none());
        assert_eq!(open.plugin.as_deref(), Some("file-search"));
        assert!(matches!(
            &open.action,
            PanelAction::Execute { command } if command == &Action::Open { uri: uri.to_string() }
        ));
        let default = row.actions.iter().find(|a| a.default).unwrap();
        assert_eq!(default.id.as_deref(), Some("terminal"));
        assert_eq!(default.plugin.as_deref(), Some("file-search"));
        assert!(
            row.actions.iter().filter(|a| a.default).count() == 1,
            "only the remembered action is the default"
        );
    }

    #[test]
    fn no_remembered_default_marks_nothing() {
        let uri = "file:///tmp/a.txt";
        let mut row = item(
            "a.txt",
            Action::Open {
                uri: uri.to_string(),
            },
        );
        attach_actions(
            &mut row,
            "",
            &[],
            false,
            &HashMap::new(),
            (
                Some("file-search".to_string()),
                vec![file_action(
                    "Reveal in file manager",
                    "reveal",
                    Action::Reveal {
                        uri: uri.to_string(),
                    },
                )],
            ),
        );
        assert!(row.actions.iter().all(|a| !a.default));
        assert_eq!(
            row.actions[0].title,
            t!("action.open"),
            "the row command always leads"
        );
    }
}
