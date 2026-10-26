use crate::system::icon::host_icon_spec;
use crate::wire::{Preview, ResultItem};
use rust_i18n::t;

/// A host `result` array as rows: icons resolved and `plugin` stamped as the
/// actions' owner; `None` if unusable. Consumes the reply, so no row is cloned.
pub(super) fn parse_result_items(
    response: serde_json::Value,
    plugin: &str,
    icon: &str,
) -> Option<Vec<ResultItem>> {
    let serde_json::Value::Object(mut object) = response else {
        return None;
    };
    let items = match object.remove("result")? {
        serde_json::Value::Array(items) => items,
        _ => return None,
    };
    Some(parse_rows(items, plugin, icon))
}

/// One payload's rows with their icons, previews and actions resolved; a row
/// that does not deserialize is dropped, never the payload.
pub(super) fn parse_rows(
    items: Vec<serde_json::Value>,
    plugin: &str,
    icon: &str,
) -> Vec<ResultItem> {
    let mut parsed: Vec<ResultItem> = items
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect();
    let identity = host_icon_spec(icon);
    let owner = plugin.to_string();
    for item in &mut parsed {
        let spec = item.icon.as_deref().unwrap_or("");
        item.icon = resolve_item_icon(spec, identity.clone());
        // A preview is a file the host ships or plain text; the same icon rule
        // validates the path, and the text cannot grow the wire without bound.
        if let Some(Preview::Image { path }) = &item.preview {
            item.preview = crate::system::icon::remote_preview(path)
                .or_else(|| host_icon_spec(path))
                .map(|path| Preview::Image { path });
        }
        if let Some(Preview::Text { text }) = &mut item.preview {
            const MAX: usize = 64 * 1024;
            if text.len() > MAX {
                let mut end = MAX;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
        }
        for action in &mut item.actions {
            action.icon = action.icon.as_deref().and_then(host_icon_spec);
            // The core owns this field; whatever the host sent is ignored.
            action.plugin = Some(owner.clone());
        }
    }
    parsed
}

/// A host's `search` error becomes one display-only row, so a plugin that raises
/// is visible instead of indistinguishable from an empty result.
pub(super) fn host_error(response: &serde_json::Value) -> Option<ResultItem> {
    let error = response.get("error")?;
    let message = error
        .get("message")
        .and_then(|message| message.as_str())
        .unwrap_or_default();
    let summary = message.lines().next().unwrap_or_default();
    Some(ResultItem {
        title: t!("plugin.search_failed").to_string(),
        summary: Some(summary.to_string()),
        on_click: None,
        icon: None,
        icon_shape: None,
        preview: None,
        actions: Vec::new(),
    })
}

fn resolve_item_icon(icon: &str, fallback: Option<String>) -> Option<String> {
    crate::system::icon::remote_icon(icon)
        .or_else(|| host_icon_spec(icon))
        .or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Action, IconShape};

    #[test]
    fn empty_icon_falls_back_to_plugin_icon() {
        assert_eq!(
            resolve_item_icon("", Some("/x.png".into())),
            Some("/x.png".into())
        );
        assert_eq!(resolve_item_icon("", None), None);
    }

    #[test]
    fn absolute_path_passes_through() {
        assert_eq!(
            resolve_item_icon("/home/u/icon.svg", None),
            Some("/home/u/icon.svg".into())
        );
    }

    #[test]
    fn an_unresolved_spec_prefers_the_identity_icon() {
        assert_eq!(
            resolve_item_icon("definitely-not-an-icon-xyz", Some("/identity.png".into())),
            Some("/identity.png".into())
        );
    }

    #[test]
    fn a_theme_glyph_or_papirus_spec_is_no_icon_for_a_host() {
        // the core resolves these for its own rows, but not for an external host
        assert_eq!(resolve_item_icon("papirus:folder-open", None), None);
        assert_eq!(resolve_item_icon("firefox", None), None);
        assert_eq!(resolve_item_icon("builtin:power", None), None);
    }

    #[test]
    fn a_hosts_action_icons_take_paths_only() {
        let response = serde_json::json!({
            "result": [{
                "title": "r",
                "on_click": {"type":"run","cmd":"x"},
                "actions": [
                    {"title":"a", "action":{"type":"run","cmd":"y"}, "icon": "builtin:open"},
                    {"title":"b", "action":{"type":"run","cmd":"z"}, "icon": "/tmp/a.svg"},
                    {"title":"c", "action":{"type":"run","cmd":"w"}, "icon": "firefox"},
                ],
            }]
        });
        let items = parse_result_items(response, "system-search", "").unwrap();
        assert_eq!(
            items[0].actions[0].plugin.as_deref(),
            Some("system-search"),
            "the host owns the actions it attaches"
        );
        assert!(
            items[0].actions[0].icon.is_none(),
            "a glyph is no host icon"
        );
        assert_eq!(items[0].actions[1].icon.as_deref(), Some("/tmp/a.svg"));
        assert!(
            items[0].actions[2].icon.is_none(),
            "a theme name is no host icon"
        );
    }

    /// The golden corpus the Python SDK pins on its side (WayRun-Plugin,
    /// `tests/test_golden.py`); both ends accept the same shapes.
    #[test]
    fn every_command_variant_parses_from_a_host() {
        let response = serde_json::json!({
            "result": [
                {"title": "run", "on_click": {"type": "run", "cmd": "echo hi"}},
                {"title": "terminal run", "on_click": {"type": "run_in_terminal", "cmd": "htop"}},
                {"title": "open", "on_click": {"type": "open", "uri": "https://example.com"}},
                {"title": "copy", "on_click": {"type": "copy", "text": "x"}},
                {"title": "launch", "on_click": {"type": "launch", "desktop_id": "firefox.desktop"}},
                {"title": "desktop action", "on_click": {"type": "desktop_action", "desktop_id": "firefox.desktop", "action_id": "new-private-window"}},
                {"title": "reveal", "on_click": {"type": "reveal", "uri": "file:///home/u"}},
                {"title": "terminal", "on_click": {"type": "terminal", "uri": "file:///home/u"}},
                {"title": "ephemeral", "ephemeral": true},
                {"title": "nulls", "summary": null, "on_click": null, "icon": null, "icon_shape": null},
                {"title": "circle", "icon_shape": "circle"},
            ]
        });
        let items = parse_result_items(response, "golden", "/opt/identity.svg").unwrap();
        assert_eq!(items.len(), 11, "every corpus row survives");
        assert_eq!(
            items[0].on_click,
            Some(Action::Run {
                cmd: "echo hi".to_string()
            })
        );
        assert_eq!(
            items[1].on_click,
            Some(Action::RunInTerminal {
                cmd: "htop".to_string()
            })
        );
        assert_eq!(
            items[2].on_click,
            Some(Action::Open {
                uri: "https://example.com".to_string()
            })
        );
        assert_eq!(
            items[3].on_click,
            Some(Action::Copy {
                text: "x".to_string()
            })
        );
        assert_eq!(
            items[4].on_click,
            Some(Action::Launch {
                desktop_id: "firefox.desktop".to_string()
            })
        );
        assert_eq!(
            items[5].on_click,
            Some(Action::DesktopAction {
                desktop_id: "firefox.desktop".to_string(),
                action_id: "new-private-window".to_string()
            })
        );
        assert_eq!(
            items[6].on_click,
            Some(Action::Reveal {
                uri: "file:///home/u".to_string()
            })
        );
        assert_eq!(
            items[7].on_click,
            Some(Action::Terminal {
                uri: "file:///home/u".to_string()
            })
        );
        assert_eq!(
            items[8].title, "ephemeral",
            "a stale field an old host sends is ignored"
        );
        assert!(items[9].summary.is_none() && items[9].on_click.is_none());
        assert_eq!(
            items[9].icon.as_deref(),
            Some("/opt/identity.svg"),
            "an unset icon falls back to the identity"
        );
        assert!(
            items[9].icon_shape.is_none(),
            "a null shape draws the file as it is"
        );
        assert_eq!(
            items[10].icon_shape,
            Some(IconShape::Circle),
            "the host's shape reaches the row"
        );
    }

    #[test]
    fn a_host_error_becomes_a_display_row() {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "error": {"code": -32000, "message": "boom\nstack traceback:"},
            "id": 1
        });
        let row = host_error(&response).unwrap();
        assert_eq!(row.title, t!("plugin.search_failed").to_string());
        assert_eq!(row.summary.as_deref(), Some("boom"));
        assert!(row.on_click.is_none());
    }

    #[test]
    fn a_hosts_preview_is_an_absolute_path_or_https_url() {
        let response = serde_json::json!({
            "result": [
                {"title": "ok", "preview": {"type": "image", "path": "/tmp/a.png"}},
                {"title": "url", "preview": {"type": "image", "path": "https://example.com/a.png"}},
                {"title": "theme", "preview": {"type": "image", "path": "theme:icon"}},
                {"title": "relative", "preview": {"type": "image", "path": "a.png"}}
            ]
        });
        let items = parse_result_items(response, "p", "").unwrap();
        assert_eq!(
            items[0].preview,
            Some(Preview::Image {
                path: "/tmp/a.png".to_string()
            })
        );
        assert!(matches!(
            &items[1].preview,
            Some(Preview::Image { path }) if path.contains("/icons/") && path.ends_with(".png")
        ));
        assert_eq!(items[2].preview, None);
        assert_eq!(items[3].preview, None);
    }

    #[test]
    fn a_hosts_row_icon_may_be_an_https_url() {
        let response = serde_json::json!({
            "result": [{"title": "url", "icon": "https://example.com/a.png"}]
        });
        let items = parse_result_items(response, "p", "/opt/identity.svg").unwrap();
        assert!(
            items[0]
                .icon
                .as_deref()
                .is_some_and(|path| path.contains("/icons/") && path.ends_with(".png"))
        );
    }

    #[test]
    fn a_hosts_preview_text_is_capped() {
        let long = "x".repeat(70 * 1024);
        let response = serde_json::json!({
            "result": [{"title": "t", "preview": {"type": "text", "text": long}}]
        });
        let items = parse_result_items(response, "p", "").unwrap();
        match &items[0].preview {
            Some(Preview::Text { text }) => assert_eq!(text.len(), 64 * 1024),
            other => panic!("expected a capped text preview, got {other:?}"),
        }
    }
}
