use serde::{Deserialize, Serialize};

/// One thing a row can do: what Enter or an action runs. Internally tagged, so
/// the wire carries `{"type":"run","cmd":"…"}` rather than a scheme string.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Run {
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
}

impl Action {
    /// Canonical key for usage history and pins; internal, never emitted.
    pub fn key(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// A row's panel command: execute the row's own `Action`, or a launcher-level
/// pin/unpin/history operation the shell turns into its own RPC call.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PanelAction {
    Execute {
        command: Action,
    },
    Pin {
        scope: String,
        item: Box<ResultItem>,
    },
    Unpin {
        scope: String,
        on_click: Action,
    },
    Forget {
        on_click: Action,
    },
}

/// One action-panel entry of a row, never run by Enter unless it is the default.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ActionItem {
    pub title: String,
    pub action: PanelAction,
    /// The icon spec; the core resolves it to an absolute path before emitting,
    /// like a row's `icon`.
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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ResultItem {
    pub title: String,
    pub summary: Option<String>,
    pub on_click: Option<Action>,
    pub icon: Option<String>,
    /// The host asked for this row not to enter usage history — a one-shot
    /// search hit, for instance. Absent on the wire means "record it".
    #[serde(default)]
    pub ephemeral: bool,
    /// Secondary commands for the row's action panel. Built-ins are attached by
    /// the core before emitting; a host may supply its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionItem>,
    /// A small status glyph shown at the row's right edge (a pin for a pinned
    /// row), resolved to an absolute path like `icon`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
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
