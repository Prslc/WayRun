use serde_json::json;

use super::registry::{REGISTRY, ensure_loaded};
use crate::system::icon::find_icon_path;
use crate::wire::ResultItem;

/// Drop a row across the registry. `true` when an external host owned and dropped
/// it, so `forget` answers truthfully instead of claiming a deletion.
pub async fn forget_row(on_click: &str) -> bool {
    ensure_loaded().await;
    let reg = REGISTRY.read().await;
    let mut owned = false;
    for entry in reg.iter() {
        owned |= entry.plugin.forget(on_click).await.unwrap_or(false);
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

    for item in &mut out {
        let plugin_actions = plugin_actions(item).await;
        attach_actions(item, scope, &pinned, history, plugin_actions);
    }
    out
}

/// Pins lead the results, deduplicated, so each appears once at the top. Returns
/// the merged list and the pinned `on_click` keys the action labels use.
fn merge_pins(
    mut pins: Vec<ResultItem>,
    mut results: Vec<ResultItem>,
) -> (Vec<ResultItem>, Vec<String>) {
    let pinned: Vec<String> = pins
        .iter()
        .filter_map(|item| item.on_click.clone())
        .collect();
    results.retain(|item| {
        !item
            .on_click
            .as_deref()
            .is_some_and(|on_click| pinned.iter().any(|pin| pin == on_click))
    });
    pins.append(&mut results);
    (pins, pinned)
}

/// The first plugin that recognises the row and declares actions for it, from
/// its provider or the host's inline `actions`.
async fn plugin_actions(item: &ResultItem) -> Vec<crate::wire::ActionItem> {
    let reg = REGISTRY.read().await;
    for entry in reg.iter() {
        let actions = entry.plugin.actions(item);
        if !actions.is_empty() {
            return actions;
        }
    }
    Vec::new()
}

/// The launcher-level commands an actionable row gets (pin/unpin always, history
/// removal only on a recordable history row), before its type and host actions.
fn attach_actions(
    item: &mut ResultItem,
    scope: &str,
    pinned: &[String],
    history: bool,
    mut plugin_actions: Vec<crate::wire::ActionItem>,
) {
    let Some(on_click) = item.on_click.clone() else {
        return;
    };
    let is_pinned = pinned.iter().any(|pin| pin == &on_click);
    if is_pinned {
        item.badge = find_icon_path("pin");
    }

    let mut actions: Vec<crate::wire::ActionItem> = Vec::new();
    if is_pinned {
        let payload = json!({ "scope": scope, "on_click": on_click });
        actions.push(crate::wire::ActionItem {
            title: "Unpin".to_string(),
            on_click: format!("unpin:{payload}"),
            icon: Some("window-unpin".to_string()),
        });
    } else if !item.ephemeral {
        // Snapshot the row before the launcher actions are appended, so the pin
        // never embeds the action that stores it. A host's own actions stay.
        let payload = json!({ "scope": scope, "item": item });
        actions.push(crate::wire::ActionItem {
            title: "Pin to top".to_string(),
            on_click: format!("pin:{payload}"),
            icon: Some("pin".to_string()),
        });
    }

    if history && crate::system::usage::is_recordable(item.ephemeral, Some(&on_click)) {
        actions.push(crate::wire::ActionItem {
            title: "Remove from history".to_string(),
            on_click: format!("forget:{on_click}"),
            icon: Some("edit-delete".to_string()),
        });
    }

    actions.append(&mut plugin_actions);
    actions.append(&mut item.actions);

    // Resolve every icon spec (built-in or host-supplied) to the absolute path
    // the shell renders; an unresolved action keeps no icon.
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
    use crate::wire::ActionItem;

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

    #[test]
    fn pinned_rows_lead_and_their_duplicate_is_dropped() {
        let pins = vec![item("Pinned", "run:pinned")];
        let results = vec![item("Other", "run:other"), item("Pinned", "run:pinned")];

        let (merged, pinned) = merge_pins(pins, results);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].title, "Pinned");
        assert_eq!(merged[1].title, "Other");
        assert_eq!(pinned, ["run:pinned"]);
    }

    #[test]
    fn launcher_actions_lead_the_row_and_carry_its_scope() {
        let mut row = item("Firefox", "launch:firefox.desktop");
        attach_actions(&mut row, "b", &[], false, Vec::new());

        let titles: Vec<&str> = row
            .actions
            .iter()
            .map(|action| action.title.as_str())
            .collect();
        assert_eq!(titles, ["Pin to top"]);
        assert!(row.actions[0].on_click.starts_with("pin:"));
        assert!(row.actions[0].on_click.contains(r#""scope":"b""#));
        assert!(row.badge.is_none(), "an unpinned row carries no badge");
    }

    #[test]
    fn a_pinned_row_offers_unpin_before_its_type_actions() {
        let mut row = item("a.txt", "file:///tmp/a.txt");
        let reveal = ActionItem {
            title: "Reveal in file manager".to_string(),
            on_click: "reveal:file:///tmp/a.txt".to_string(),
            icon: None,
        };
        attach_actions(
            &mut row,
            "",
            &["file:///tmp/a.txt".to_string()],
            true,
            vec![reveal],
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
        let mut row = item("Firefox", "launch:firefox.desktop");
        row.actions = vec![ActionItem {
            title: "Host".to_string(),
            on_click: "run:host".to_string(),
            icon: None,
        }];
        attach_actions(&mut row, "b", &[], false, Vec::new());

        // The snapshot is the host's row, not the decorated one: the pin action
        // must not embed itself, and the host action must survive the round trip.
        let payload = row.actions[0].on_click.clone();
        assert!(payload.contains(r#""title":"Host""#));
        assert!(!payload.contains("Pin to top"));
    }

    #[test]
    fn launcher_entries_follow_the_row() {
        fn titles(row: &ResultItem) -> Vec<&str> {
            row.actions.iter().map(|a| a.title.as_str()).collect()
        }

        let mut history = item("Firefox", "launch:firefox.desktop");
        attach_actions(&mut history, "", &[], true, Vec::new());
        assert_eq!(titles(&history), ["Pin to top", "Remove from history"]);

        // a fresh search result is not sourced from the history view
        let mut search = item("Firefox", "launch:firefox.desktop");
        attach_actions(&mut search, "fire", &[], false, Vec::new());
        assert_eq!(titles(&search), ["Pin to top"]);

        // an ephemeral row is neither a durable pin target nor recorded
        let mut one_shot = item("window", "run:wctl activate 1");
        one_shot.ephemeral = true;
        attach_actions(&mut one_shot, "", &[], true, Vec::new());
        assert!(titles(&one_shot).is_empty());

        // an existing pin must stay removable even on an ephemeral row
        let mut pinned = item("clip", "run:cliphist decode 1");
        pinned.ephemeral = true;
        attach_actions(
            &mut pinned,
            "",
            &["run:cliphist decode 1".to_string()],
            true,
            Vec::new(),
        );
        assert_eq!(titles(&pinned), ["Unpin"]);

        // a copy: row is not recorded, but it is a stable pin target
        let mut copy = item("translated", r#"copy:{"text":"hi"}"#);
        attach_actions(&mut copy, "", &[], true, Vec::new());
        assert_eq!(titles(&copy), ["Pin to top"]);
    }
}
