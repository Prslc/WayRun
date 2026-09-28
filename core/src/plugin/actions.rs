use super::registry::{Entry, REGISTRY, ensure_loaded};
use crate::system::icon::find_icon_spec;
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

/// Attach each row's action panel.
pub async fn decorate(items: Vec<ResultItem>, history: bool) -> Vec<ResultItem> {
    ensure_loaded().await;
    // The shared connection stays off the runtime's workers.
    let defaults = tokio::task::spawn_blocking(crate::system::db::defaults::all)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();

    // One registry read for the whole list; `Plugin::actions` is synchronous,
    // so the guard never spans an await.
    let mut out = items;
    let reg = REGISTRY.read().await;
    for item in &mut out {
        // A row without a click command gets no panel at all, so it needs no
        // scan; the rest ask the plugins which of them owns them.
        let plugin_actions = if item.on_click.is_some() {
            plugin_actions(&reg, item)
        } else {
            (None, Vec::new())
        };
        attach_actions(item, history, &defaults, plugin_actions);
    }
    out
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

/// The launcher-level entries an actionable row gets (history removal only on a
/// recordable history row), after its type and host actions.
fn attach_actions(
    item: &mut ResultItem,
    history: bool,
    defaults: &std::collections::HashMap<String, String>,
    plugin_actions: (Option<String>, Vec<ActionItem>),
) {
    let Some(on_click) = item.on_click.clone() else {
        return;
    };
    let (owner, mut plugin_actions) = plugin_actions;

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

    // The history entry comes last: it is launcher state rather than what the
    // row offers, and the first slot belongs to the row's own command.
    if history && crate::system::db::usage::is_recordable(item.ephemeral, Some(&on_click)) {
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
            .and_then(find_icon_spec);
    }

    item.actions = actions;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::item;
    use std::collections::HashMap;

    fn run(cmd: &str) -> Action {
        Action::Run {
            cmd: cmd.to_string(),
        }
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
                t!("action.remove_history")
            ]
        );
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
        attach_actions(&mut history, true, &HashMap::new(), (None, Vec::new()));
        assert_eq!(
            titles(&history),
            [t!("action.open"), t!("action.remove_history")]
        );

        // a fresh search result is not sourced from the history view
        let mut search = item(
            "Firefox",
            Action::Launch {
                desktop_id: "firefox.desktop".to_string(),
            },
        );
        attach_actions(&mut search, false, &HashMap::new(), (None, Vec::new()));
        assert!(
            titles(&search).is_empty(),
            "a row with nothing else to offer has no panel"
        );

        // an ephemeral row is never recorded, so it has no history entry
        let mut one_shot = item("window", run("wctl activate 1"));
        one_shot.ephemeral = true;
        attach_actions(&mut one_shot, true, &HashMap::new(), (None, Vec::new()));
        assert!(titles(&one_shot).is_empty());

        // a copy row is not recorded either
        let mut copy = item(
            "translated",
            Action::Copy {
                text: "hi".to_string(),
            },
        );
        attach_actions(&mut copy, true, &HashMap::new(), (None, Vec::new()));
        assert!(titles(&copy).is_empty());
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
        attach_actions(&mut row, false, &defaults, (None, Vec::new()));

        let titles: Vec<&str> = row.actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, [t!("action.open"), "Mark done".to_string()]);
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
