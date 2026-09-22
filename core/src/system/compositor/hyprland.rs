use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Compositor, Window};
use rust_i18n::t;

pub struct Hyprland;

/// The subset of `hyprctl -j clients` the launcher uses.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawClient {
    address: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    class: String,
    #[serde(default)]
    initial_class: Option<String>,
    workspace: RawWorkspace,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
}

#[derive(Deserialize)]
struct RawWorkspace {
    id: i64,
    #[serde(default)]
    name: String,
}

impl Compositor for Hyprland {
    fn available(&self) -> bool {
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
            || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| {
                desktop
                    .split(':')
                    .any(|d| d.eq_ignore_ascii_case("hyprland"))
            })
    }

    fn windows(&self) -> Result<Vec<Window>> {
        let output = std::process::Command::new("hyprctl")
            .args(["-j", "clients"])
            .output()
            .context("running `hyprctl -j clients`")?;
        anyhow::ensure!(
            output.status.success(),
            "`hyprctl -j clients` exited with failure"
        );
        let clients: Vec<RawClient> =
            serde_json::from_slice(&output.stdout).context("parsing `hyprctl -j clients`")?;
        // An unmapped client has no surface yet; a hidden one sits on a special
        // workspace that is not shown.
        Ok(clients
            .into_iter()
            .filter(|client| client.mapped && !client.hidden)
            .map(Window::from)
            .collect())
    }

    fn focus_argv(&self, id: &str) -> Vec<String> {
        vec![
            "hyprctl".to_string(),
            "dispatch".to_string(),
            "focuswindow".to_string(),
            format!("address:{id}"),
        ]
    }
}

impl From<RawClient> for Window {
    fn from(client: RawClient) -> Self {
        let app_id = match client.class.trim() {
            "" => client
                .initial_class
                .filter(|class| !class.trim().is_empty()),
            class => Some(class.to_string()),
        };
        let raw_title = client.title.trim();
        let title = if raw_title.is_empty() {
            app_id.clone().unwrap_or_default()
        } else {
            raw_title.to_string()
        };
        let workspace = if client.workspace.name.trim().is_empty() {
            client.workspace.id.to_string()
        } else {
            client.workspace.name
        };
        Window {
            id: client.address,
            title: if title.is_empty() {
                t!("window.untitled")
            } else {
                title
            },
            app_id,
            workspace: Some(workspace),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Vec<Window> {
        let raw: Vec<RawClient> = serde_json::from_str(json).unwrap();
        raw.into_iter()
            .filter(|c| c.mapped && !c.hidden)
            .map(Window::from)
            .collect()
    }

    #[test]
    fn parses_hyprctl_client_json() {
        let json = r#"[{"address":"0x55aa","title":"~/P/WayRun","class":"kitty","initialClass":"kitty","workspace":{"id":1,"name":"1"},"mapped":true,"hidden":false,"pid":42,"xwayland":false}]"#;
        let windows = parse(json);
        assert_eq!(windows.len(), 1);
        let w = &windows[0];
        assert_eq!(w.id, "0x55aa");
        assert_eq!(w.title, "~/P/WayRun");
        assert_eq!(w.app_id.as_deref(), Some("kitty"));
        assert_eq!(w.workspace.as_deref(), Some("1"));
    }

    #[test]
    fn unmapped_and_hidden_clients_are_dropped() {
        let json = r#"[
            {"address":"0x1","title":"early","class":"x","workspace":{"id":1,"name":"1"},"mapped":false},
            {"address":"0x2","title":"scratch","class":"y","workspace":{"id":-99,"name":"special:scratchpad"},"mapped":true,"hidden":true},
            {"address":"0x3","title":"shown","class":"z","workspace":{"id":2,"name":"2"},"mapped":true,"hidden":false}
        ]"#;
        let windows = parse(json);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "0x3");
    }

    #[test]
    fn untitled_clients_fall_back_to_class_then_placeholder() {
        let json = r#"[
            {"address":"0x1","title":"  ","class":"firefox","workspace":{"id":1,"name":"1"},"mapped":true},
            {"address":"0x2","title":"","class":"","initialClass":"kitty","workspace":{"id":1,"name":"1"},"mapped":true},
            {"address":"0x3","title":"","class":"","workspace":{"id":1,"name":"1"},"mapped":true}
        ]"#;
        let windows = parse(json);
        assert_eq!(windows[0].title, "firefox");
        assert_eq!(windows[1].title, "kitty");
        assert_eq!(windows[2].title, t!("window.untitled"));
        assert_eq!(windows[2].app_id, None);
    }

    #[test]
    fn a_nameless_workspace_falls_back_to_its_id() {
        let json = r#"[{"address":"0x1","title":"x","class":"x","workspace":{"id":7,"name":""},"mapped":true}]"#;
        assert_eq!(parse(json)[0].workspace.as_deref(), Some("7"));
    }

    #[test]
    fn focus_argv_targets_the_window_by_address() {
        let argv = Hyprland.focus_argv("0x55aa");
        assert_eq!(
            argv,
            ["hyprctl", "dispatch", "focuswindow", "address:0x55aa"]
        );
    }
}
