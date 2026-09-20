#[cfg(feature = "compositor-hyprland")]
mod hyprland;
#[cfg(feature = "compositor-niri")]
mod niri;

use anyhow::Result;

/// A window the compositor can switch to, normalised across backends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Backend-native handle, echoed back to [`Compositor::focus_argv`].
    pub id: String,
    /// Non-empty display title: a window without one falls back to its
    /// `app_id`, then to `"Untitled"`.
    pub title: String,
    pub app_id: Option<String>,
    /// Workspace name or index, as the compositor reports it.
    pub workspace: Option<String>,
}

/// A window-listing/focusing backend for one Wayland compositor. Every method
/// blocks: the backends shell out to the compositor's IPC.
pub trait Compositor: Send + Sync {
    /// Whether this process runs under this compositor. An env probe only,
    /// never a subprocess, because [`detect`] is called on every search.
    fn available(&self) -> bool;

    /// Windows in the compositor's own order.
    fn windows(&self) -> Result<Vec<Window>>;

    /// argv that focuses `id` when run detached.
    fn focus_argv(&self, id: &str) -> Vec<String>;
}

/// Every compiled-in backend, probed in declaration order by [`detect`]; a
/// build with no compositor feature carries an empty list.
static BACKENDS: &[&dyn Compositor] = &[
    #[cfg(feature = "compositor-niri")]
    &niri::Niri,
    #[cfg(feature = "compositor-hyprland")]
    &hyprland::Hyprland,
];

/// The compositor hosting this process, or `None` when no backend recognises
/// the environment.
pub fn detect() -> Option<&'static dyn Compositor> {
    BACKENDS.iter().copied().find(|backend| backend.available())
}
