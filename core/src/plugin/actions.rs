use super::registry::{REGISTRY, ensure_loaded};
use crate::system::icon::find_icon_path;
use crate::wire::{Action, ActionItem, PanelAction, ResultItem};

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
    let pins: Vec<ResultItem> = crate::system::pins::get_pins(scope)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect();
    let (mut out, pinned) = merge_pins(pins, items);
    // One query for every plugin's remembered default, not one per row.
    let defaults = crate::system::defaults::all().unwrap_or_default();

    for item in &mut out {
        let plugin_actions = plugin_actions(item).await;
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
async fn plugin_actions(item: &ResultItem) -> (Option<String>, Vec<ActionItem>) {
    let reg = REGISTRY.read().await;
    for entry in reg.iter() {
        let actions = entry.plugin.actions(item);
        if !actions.is_empty() {
            return (Some(entry.plugin.meta().id.to_string()), actions);
        }
    }
    (None, Vec::new())
}

/// The command an `execute` action runs, if it is one.
fn action_command(action: &PanelAction) -> Option<&Action> {
    match action {
        PanelAction::Execute { command } => Some(command),
        _ => None,
    }
}

/// The launcher-level commands an actionable row gets (pin/unpin always, history
/// removal only on a recordable history row), before its type and host actions.
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

    // Every plugin action carries its owner, so the shell can scope a default.
    if let Some(owner) = &owner {
        for action in &mut plugin_actions {
            if action.plugin.is_none() {
                action.plugin = Some(owner.clone());
            }
        }
    }

    // A remembered default elevates its action to Enter. The row's own command
    // stays reachable as an "Open" action when the default is a different one.
    if let Some(default_id) = owner.as_deref().and_then(|owner| defaults.get(owner))
        && let Some(index) = plugin_actions
            .iter()
            .position(|action| action.id.as_deref() == Some(default_id.as_str()))
    {
        let is_primary = action_command(&plugin_actions[index].action) == Some(&on_click);
        plugin_actions[index].default = true;
        if !is_primary {
            plugin_actions.insert(
                0,
                ActionItem {
                    title: "Open".to_string(),
                    action: PanelAction::Execute {
                        command: on_click.clone(),
                    },
                    icon: Some("builtin:open".to_string()),
                    id: None,
                    plugin: None,
                    default: false,
                },
            );
        }
    }

    let mut actions: Vec<ActionItem> = Vec::new();
    if is_pinned {
        actions.push(ActionItem {
            title: "Unpin".to_string(),
            action: PanelAction::Unpin {
                scope: scope.to_string(),
                on_click: on_click.clone(),
            },
            icon: Some("builtin:unpin".to_string()),
            id: None,
            plugin: None,
            default: false,
        });
    } else if !item.ephemeral {
        // Snapshot the row before the launcher actions are appended, so the pin
        // never embeds the action that stores it. A host's own actions stay.
        actions.push(ActionItem {
            title: "Pin to top".to_string(),
            action: PanelAction::Pin {
                scope: scope.to_string(),
                item: Box::new(item.clone()),
            },
            icon: Some("builtin:pin".to_string()),
            id: None,
            plugin: None,
            default: false,
        });
    }

    if history && crate::system::usage::is_recordable(item.ephemeral, Some(&on_click)) {
        actions.push(ActionItem {
            title: "Remove from history".to_string(),
            action: PanelAction::Forget {
                on_click: on_click.clone(),
            },
            icon: Some("builtin:remove".to_string()),
            id: None,
            plugin: None,
            default: false,
        });
    }

    actions.append(&mut plugin_actions);
    actions.append(&mut item.actions);

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
    fn launcher_actions_lead_the_row_and_carry_its_scope() {
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
        assert_eq!(titles, ["Pin to top"]);
        match &row.actions[0].action {
            PanelAction::Pin { scope, item } => {
                assert_eq!(scope, "b");
                assert_eq!(item.title, "Firefox");
            }
            other => panic!("expected a pin, got {other:?}"),
        }
        assert!(row.badge.is_none(), "an unpinned row carries no badge");
    }

    #[test]
    fn a_pinned_row_offers_unpin_before_its_type_actions() {
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
            ["Unpin", "Remove from history", "Reveal in file manager"]
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
        let PanelAction::Pin { item, .. } = &row.actions[0].action else {
            panic!("expected a pin");
        };
        assert_eq!(item.actions.len(), 1);
        assert_eq!(item.actions[0].title, "Host");
    }

    #[test]
    fn launcher_entries_follow_the_row() {
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
        assert_eq!(titles(&history), ["Pin to top", "Remove from history"]);

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
        assert_eq!(titles(&search), ["Pin to top"]);

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
        assert!(titles(&one_shot).is_empty());

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
        assert_eq!(titles(&pinned), ["Unpin"]);

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
        assert_eq!(titles(&copy), ["Pin to top"]);
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

        // the row's own command survives as an "Open" action
        assert!(
            row.actions
                .iter()
                .any(|a| a.title == "Open" && a.id.is_none()),
            "the primary stays reachable"
        );
        let default = row.actions.iter().find(|a| a.default).unwrap();
        assert_eq!(default.id.as_deref(), Some("terminal"));
        assert_eq!(default.plugin.as_deref(), Some("file-search"));
        assert!(
            row.actions.iter().filter(|a| a.default).count() == 1,
            "only the remembered action is the default"
        );
    }

    #[test]
    fn no_remembered_default_leaves_the_panel_alone() {
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
        assert!(row.actions.iter().all(|a| a.title != "Open"));
    }
}
