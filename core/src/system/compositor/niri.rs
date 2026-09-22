use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Compositor, Window};
use rust_i18n::t;

pub struct Niri;

/// The subset of `niri msg -j windows` the launcher uses.
#[derive(Deserialize)]
struct RawWindow {
    id: u64,
    title: String,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<u64>,
}

impl Compositor for Niri {
    fn available(&self) -> bool {
        std::env::var_os("NIRI_SOCKET").is_some()
            || std::env::var("XDG_CURRENT_DESKTOP")
                .is_ok_and(|desktop| desktop.split(':').any(|d| d.eq_ignore_ascii_case("niri")))
    }

    fn windows(&self) -> Result<Vec<Window>> {
        let output = std::process::Command::new("niri")
            .args(["msg", "-j", "windows"])
            .output()
            .context("running `niri msg -j windows`")?;
        anyhow::ensure!(
            output.status.success(),
            "`niri msg -j windows` exited with failure"
        );
        let windows: Vec<RawWindow> =
            serde_json::from_slice(&output.stdout).context("parsing `niri msg -j windows`")?;
        Ok(windows.into_iter().map(Window::from).collect())
    }

    fn focus_argv(&self, id: &str) -> Vec<String> {
        ["niri", "msg", "action", "focus-window", "--id", id]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
}

impl From<RawWindow> for Window {
    fn from(w: RawWindow) -> Self {
        // Some apps leave the title empty (or untitled); fall back to app_id.
        let raw_title = w.title.trim();
        let title = if raw_title.is_empty() {
            w.app_id.clone().unwrap_or_default()
        } else {
            raw_title.to_string()
        };
        Window {
            id: w.id.to_string(),
            title: if title.is_empty() {
                t!("window.untitled")
            } else {
                title
            },
            app_id: w.app_id,
            workspace: w.workspace_id.map(|ws| ws.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Vec<Window> {
        let raw: Vec<RawWindow> = serde_json::from_str(json).unwrap();
        raw.into_iter().map(Window::from).collect()
    }

    #[test]
    fn parses_niri_window_json() {
        let json = r#"[{"id":5,"title":"kitty","app_id":"kitty","workspace_id":1,"is_focused":false,"layout":{}}]"#;
        let windows = parse(json);
        assert_eq!(windows.len(), 1);
        let w = &windows[0];
        assert_eq!(w.id, "5");
        assert_eq!(w.title, "kitty");
        assert_eq!(w.app_id.as_deref(), Some("kitty"));
        assert_eq!(w.workspace.as_deref(), Some("1"));
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        let json = r#"[{"id":2,"title":"foo"}]"#;
        let windows = parse(json);
        assert_eq!(windows[0].app_id, None);
        assert_eq!(windows[0].workspace, None);
    }

    #[test]
    fn untitled_windows_fall_back_to_app_id_then_placeholder() {
        let json = r#"[{"id":1,"title":"  ","app_id":"firefox"},{"id":2,"title":""}]"#;
        let windows = parse(json);
        assert_eq!(windows[0].title, "firefox");
        assert_eq!(windows[1].title, t!("window.untitled"));
    }

    #[test]
    fn focus_argv_targets_the_window_by_id() {
        let argv = Niri.focus_argv("42");
        assert_eq!(
            argv,
            ["niri", "msg", "action", "focus-window", "--id", "42"]
        );
    }
}
