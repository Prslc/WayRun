use serde::{Deserialize, Serialize};

/// The plugin API version this launcher speaks; what a host or Lua plugin
/// reports is compared against it, and a mismatch is logged rather than refused.
pub const PLUGIN_API: u32 = 1;

/// One thing a row can do: what Enter or an action runs. Internally tagged, so
/// the wire carries `{"type":"run","cmd":"…"}` rather than a scheme string.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Run {
        cmd: String,
    },
    /// Run a command inside a terminal emulator, for a binary that needs a tty.
    RunInTerminal {
        cmd: String,
    },
    Launch {
        desktop_id: String,
    },
    Copy {
        text: String,
    },
    DesktopAction {
        desktop_id: String,
        action_id: String,
    },
    Reveal {
        uri: String,
    },
    Terminal {
        uri: String,
    },
    Open {
        uri: String,
    },
    /// Open one URI with a named handler instead of the default one.
    OpenWith {
        desktop_id: String,
        uri: String,
    },
}

impl Action {
    /// Canonical key for the usage database; internal, never emitted.
    pub fn key(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// One action-panel entry of a row, never run by Enter unless it is the default.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ActionItem {
    pub title: String,
    pub action: Action,
    /// The icon spec; the core resolves system lookups and passes a `builtin:`
    /// glyph through, like a row's `icon`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The stable action kind (`reveal`, `terminal`, …) a remembered default
    /// names; a host may omit it, and then the action cannot be made default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The plugin that owns the action, so the shell can scope a default to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// Whether Enter runs this action, when its plugin has a remembered default.
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// What the selected row shows beside the list, tagged by `type` like `Action`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Preview {
    Image { path: String },
    Text { text: String },
}

/// The shape an icon box is clipped to; absent draws the icon as it is.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IconShape {
    #[default]
    Square,
    Circle,
}

/// One result row, shared by the core, the shell and the UI.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ResultItem {
    pub title: String,
    pub summary: Option<String>,
    pub on_click: Option<Action>,
    pub icon: Option<String>,
    /// Clip the icon box to this shape; absent draws the file as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_shape: Option<IconShape>,
    /// An optional split-view payload for the selected row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<Preview>,
    /// Secondary commands for the row's action panel. Built-ins are attached by
    /// the core before emitting; a host may supply its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionItem>,
}

/// The `theme` notification's params. Every role is optional, so a partial
/// payload still applies; the shell draws only the roles it models.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ThemeConfig {
    /// `"dark"` or `"light"`: which palette was resolved, so a host's per-mode
    /// overrides can follow. Absent means the host did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_primary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
}

/// The glyphs a `builtin:<name>` icon spec can name, bare; the shell renders
/// each from its own compiled copy of the SVGs.
pub const BUILTIN_GLYPHS: &[&str] = &[
    "app",
    "calculator",
    "clipboard",
    "copy",
    "file",
    "folder",
    "lock",
    "logout",
    "open",
    "power",
    "reboot",
    "reveal",
    "suspend",
    "terminal",
    "window",
];

/// The spec a missing icon falls back to; its name is in [`BUILTIN_GLYPHS`].
pub const BUILTIN_FALLBACK: &str = "builtin:app";

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_row_omits_an_unset_icon_shape() {
        let row = ResultItem {
            title: "x".into(),
            summary: None,
            on_click: None,
            icon: None,
            icon_shape: None,
            preview: None,
            actions: Vec::new(),
        };
        let value = serde_json::to_value(&row).unwrap();
        assert!(value.get("icon_shape").is_none());
    }

    #[test]
    fn a_circle_icon_shape_round_trips() {
        let row: ResultItem = serde_json::from_value(json!({
            "title": "x",
            "summary": null,
            "on_click": null,
            "icon": "/a.png",
            "icon_shape": "circle",
        }))
        .unwrap();
        assert_eq!(row.icon_shape, Some(IconShape::Circle));
        let value = serde_json::to_value(&row).unwrap();
        assert_eq!(value["icon_shape"], "circle");
    }
}
