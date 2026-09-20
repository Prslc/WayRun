use serde::{Deserialize, Serialize};

/// One action-panel command of a row, never run by Enter. `on_click` uses a
/// row's schemes plus the panel-only `pin:`/`unpin:`/`forget:`/`reveal:`/
/// `terminal:`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ActionItem {
    pub title: String,
    pub on_click: String,
    /// The icon spec; the core resolves it to an absolute path before emitting,
    /// like a row's `icon`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ResultItem {
    pub title: String,
    pub summary: Option<String>,
    pub on_click: Option<String>,
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

/// The `{"type":"theme","data":{…}}` payload. Every role is optional, so a
/// partial payload still applies; the shell draws only the roles it models.
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
