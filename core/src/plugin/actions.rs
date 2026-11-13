use super::Produced;
use super::registry::{Entry, REGISTRY, ensure_loaded};
use crate::system::icon::find_icon_spec;
use crate::wire::{ActionItem, ResultItem};
use rust_i18n::t;
use std::collections::HashMap;

/// Attach each row's action panel.
pub async fn decorate(items: Vec<Produced>) -> Vec<ResultItem> {
    ensure_loaded().await;
    // The shared connection stays off the runtime's workers.
    let defaults = tokio::task::spawn_blocking(crate::system::db::defaults::all)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();

    // One registry read for the whole list; `Plugin::actions` is synchronous,
    // so the guard never spans an await.
    let reg = REGISTRY.read().await;
    let mut out = Vec::with_capacity(items.len());
    for Produced { plugin, mut item } in items {
        // A row without a click command gets no panel at all, so it needs no
        // lookup; the rest ask the plugin that produced them.
        let actions = if item.on_click.is_some() {
            plugin_actions(&reg, &plugin, &item)
        } else {
            Vec::new()
        };
        attach_actions(&mut item, &defaults, &plugin, actions);
        out.push(item);
    }
    out
}

/// The producer's own commands for one of its rows; a reload between the search
/// and this pass leaves only the actions the host attached.
fn plugin_actions(entries: &[Entry], owner: &str, item: &ResultItem) -> Vec<ActionItem> {
    entries
        .iter()
        .find(|entry| entry.plugin.meta().id.as_ref() == owner)
        .map_or_else(Vec::new, |entry| entry.plugin.actions(item))
}

/// The panel an actionable row gets: what its producer declares, then the row's
/// own command leading.
fn attach_actions(
    item: &mut ResultItem,
    defaults: &HashMap<String, String>,
    owner: &str,
    mut plugin_actions: Vec<ActionItem>,
) {
    let Some(on_click) = item.on_click.clone() else {
        return;
    };

    let mut actions: Vec<ActionItem> = Vec::new();
    actions.append(&mut plugin_actions);
    actions.append(&mut item.actions);

    // An explicit scope belongs to a shared action; a missing one is the row's
    // producer, which also scopes the default.
    walk(&mut actions, &mut |action| {
        if action.plugin.is_none() {
            action.plugin = Some(owner.to_string());
        }
    });

    // A remembered default elevates its action to Enter, whichever plugin the
    // scope belongs to: a host's action is remembered under the host's id.
    walk(&mut actions, &mut |action| {
        action.default = action
            .plugin
            .as_deref()
            .and_then(|plugin| defaults.get(plugin))
            .is_some_and(|id| action.id.as_deref() == Some(id.as_str()));
    });

    // The row's own command leads the panel when no plugin action already names
    // it; re-picking it takes the default back, and a lone command shows none.
    let named = actions
        .iter()
        .any(|action| action.action.as_ref() == Some(&on_click));
    if !actions.is_empty() && !named {
        actions.insert(
            0,
            ActionItem {
                title: t!("action.open"),
                action: Some(on_click.clone()),
                children: Vec::new(),
                icon: Some("builtin:open".to_string()),
                id: None,
                plugin: Some(owner.to_string()),
                default: false,
            },
        );
    }

    // Core and plugin actions carry `builtin:` specs; a host action's icon is
    // already absolute, so anything unresolved keeps no icon.
    walk(&mut actions, &mut |action| {
        action.icon = action
            .icon
            .as_deref()
            .filter(|spec| !spec.is_empty())
            .and_then(find_icon_spec);
    });

    item.actions = actions;
}

/// Every entry of the panel tree in turn, an entry before its children.
fn walk(actions: &mut [ActionItem], f: &mut impl FnMut(&mut ActionItem)) {
    for action in actions {
        f(action);
        walk(&mut action.children, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::item;
    use crate::wire::Action;
    use std::collections::HashMap;

    fn run(cmd: &str) -> Action {
        Action::Run {
            cmd: cmd.to_string(),
        }
    }

    #[test]
    fn the_row_command_leads_the_panel() {
        let mut row = item(
            "a.txt",
            Action::Open {
                uri: "file:///tmp/a.txt".to_string(),
            },
        );
        let reveal = ActionItem {
            title: "Reveal in file manager".to_string(),
            action: Some(Action::Reveal {
                uri: "file:///tmp/a.txt".to_string(),
            }),
            children: Vec::new(),
            icon: None,
            id: Some("reveal".to_string()),
            plugin: None,
            default: false,
        };
        attach_actions(&mut row, &HashMap::new(), "file-search", vec![reveal]);

        let titles: Vec<&str> = row
            .actions
            .iter()
            .map(|action| action.title.as_str())
            .collect();
        assert_eq!(
            titles,
            [t!("action.open"), "Reveal in file manager".to_string()]
        );
    }

    #[test]
    fn a_plugin_action_naming_the_row_command_leads_the_panel() {
        let copy = Action::Copy {
            text: "download".to_string(),
        };
        let mut row = item("download", copy.clone());
        row.actions = vec![ActionItem {
            title: "Copy name".to_string(),
            action: Some(copy),
            children: Vec::new(),
            icon: None,
            id: Some("copy_name".to_string()),
            plugin: Some("icons".to_string()),
            default: false,
        }];
        attach_actions(&mut row, &HashMap::new(), "icons", Vec::new());

        let titles: Vec<&str> = row.actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Copy name"],
            "no second entry for the same command"
        );
    }

    #[test]
    fn a_lone_row_command_shows_no_panel() {
        let mut row = item(
            "Firefox",
            Action::Launch {
                desktop_id: "firefox.desktop".to_string(),
            },
        );
        attach_actions(&mut row, &HashMap::new(), "app-search", Vec::new());
        assert!(
            row.actions.is_empty(),
            "a row with nothing else to offer has no panel"
        );
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
            action: Some(run("done")),
            children: Vec::new(),
            icon: None,
            id: Some("done".to_string()),
            plugin: Some("todo".to_string()),
            default: false,
        }];
        let mut defaults = HashMap::new();
        defaults.insert("todo".to_string(), "done".to_string());
        attach_actions(&mut row, &defaults, "todo", Vec::new());

        let titles: Vec<&str> = row.actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, [t!("action.open"), "Mark done".to_string()]);
        let marked = row.actions.iter().find(|a| a.default).unwrap();
        assert_eq!(marked.id.as_deref(), Some("done"));
        assert_eq!(marked.plugin.as_deref(), Some("todo"));
    }

    #[test]
    fn a_hosts_row_shows_only_its_own_actions() {
        let uri = "file:///tmp/a.txt";
        let mut row = item(
            "a.txt",
            Action::Open {
                uri: uri.to_string(),
            },
        );
        row.actions = vec![ActionItem {
            title: "Mark done".to_string(),
            action: Some(run("done")),
            children: Vec::new(),
            icon: None,
            id: Some("done".to_string()),
            plugin: Some("todo".to_string()),
            default: false,
        }];
        let mut defaults = HashMap::new();
        defaults.insert("todo".to_string(), "done".to_string());
        attach_actions(&mut row, &defaults, "todo", Vec::new());

        let titles: Vec<&str> = row.actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, [t!("action.open"), "Mark done".to_string()]);
        assert!(
            row.actions
                .iter()
                .all(|action| action.plugin.as_deref() == Some("todo")),
            "a host's actions are the only ones on its row"
        );
    }

    #[test]
    fn a_default_follows_the_rows_producer() {
        let uri = "file:///tmp/a.txt";
        let terminal = || {
            file_action(
                "Open in terminal",
                "terminal",
                Action::Terminal {
                    uri: uri.to_string(),
                },
            )
        };
        let mut from_d = item(
            "a.txt",
            Action::Open {
                uri: uri.to_string(),
            },
        );
        let mut from_host = item(
            "a.txt",
            Action::Open {
                uri: uri.to_string(),
            },
        );
        let defaults = HashMap::from([("path-search".to_string(), "terminal".to_string())]);
        attach_actions(&mut from_d, &defaults, "path-search", vec![terminal()]);
        attach_actions(&mut from_host, &defaults, "wayrun-config", vec![terminal()]);

        assert!(
            from_d.actions.iter().any(|action| action.default),
            "the producer's remembered action is the default"
        );
        assert!(
            from_host.actions.iter().all(|action| !action.default),
            "another producer's identical action is untouched"
        );
    }

    #[test]
    fn an_explicit_shared_scope_is_kept() {
        let mut row = item("link", run("true"));
        row.actions = vec![ActionItem {
            title: "Copy link".to_string(),
            action: Some(Action::Copy {
                text: "https://example.com".to_string(),
            }),
            children: Vec::new(),
            icon: None,
            id: Some("copy_url".to_string()),
            plugin: Some("copy-url".to_string()),
            default: false,
        }];
        attach_actions(&mut row, &HashMap::new(), "web-search", Vec::new());

        let copy = row
            .actions
            .iter()
            .find(|action| action.id.as_deref() == Some("copy_url"))
            .unwrap();
        assert_eq!(copy.plugin.as_deref(), Some("copy-url"));
        assert_eq!(row.actions[0].plugin.as_deref(), Some("web-search"));
    }

    fn entry(plugin: Box<dyn crate::plugin::Plugin>) -> Entry {
        Entry {
            plugin,
            keyword: String::new(),
            external: false,
            pending: None,
        }
    }

    #[test]
    fn only_the_producer_declares_the_rows_actions() {
        let row = item(
            "a.txt",
            Action::Open {
                uri: "file:///tmp/a.txt".to_string(),
            },
        );
        let entries = vec![
            entry(Box::new(crate::provider::file::FileSearch::new())),
            entry(Box::new(crate::provider::application::AppSearch::new())),
        ];

        assert!(
            !plugin_actions(&entries, "file-search", &row).is_empty(),
            "the file provider declares the file row's actions"
        );
        assert!(
            plugin_actions(&entries, "app-search", &row).is_empty(),
            "a later entry is not asked for a row it did not produce"
        );
        assert!(
            plugin_actions(&entries, "wayrun-config", &row).is_empty(),
            "a producer that is not in the registry contributes nothing"
        );
    }

    fn file_action(title: &str, id: &str, command: Action) -> ActionItem {
        ActionItem {
            title: title.to_string(),
            action: Some(command),
            children: Vec::new(),
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
        attach_actions(&mut row, &defaults, "file-search", actions);

        // the row's own command leads the panel, owned by the plugin so the
        // panel can make it the default again
        let open = &row.actions[0];
        assert_eq!(open.title, t!("action.open"));
        assert!(open.id.is_none());
        assert_eq!(open.plugin.as_deref(), Some("file-search"));
        assert_eq!(
            open.action,
            Some(Action::Open {
                uri: uri.to_string()
            })
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
            &HashMap::new(),
            "file-search",
            vec![file_action(
                "Reveal in file manager",
                "reveal",
                Action::Reveal {
                    uri: uri.to_string(),
                },
            )],
        );
        assert!(row.actions.iter().all(|a| !a.default));
        assert_eq!(
            row.actions[0].title,
            t!("action.open"),
            "the row command always leads"
        );
    }

    #[test]
    fn a_grouped_entry_is_stamped_and_marked_like_a_flat_one() {
        let uri = "file:///tmp/a.txt";
        let mut row = item(
            "a.txt",
            Action::Open {
                uri: uri.to_string(),
            },
        );
        let group = ActionItem {
            title: "Open with".to_string(),
            action: None,
            children: vec![file_action(
                "Firefox",
                "open_with:firefox.desktop",
                Action::OpenWith {
                    desktop_id: "firefox.desktop".to_string(),
                    uri: uri.to_string(),
                },
            )],
            icon: None,
            id: Some("open_with".to_string()),
            plugin: None,
            default: false,
        };
        let mut defaults = HashMap::new();
        defaults.insert(
            "file-search".to_string(),
            "open_with:firefox.desktop".to_string(),
        );
        attach_actions(&mut row, &defaults, "file-search", vec![group]);

        let group = &row.actions[1];
        assert_eq!(group.plugin.as_deref(), Some("file-search"));
        assert!(!group.default, "the group itself is never the default");
        let child = &group.children[0];
        assert_eq!(child.plugin.as_deref(), Some("file-search"));
        assert!(child.default, "a grouped entry can carry the default");
    }
}
