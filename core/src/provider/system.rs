use std::future::Future;
use std::pin::Pin;

use crate::plugin::{Meta, Plugin};
use crate::system::icon::resolve;
use crate::wire::{Action, ResultItem};
use anyhow::Result;

pub struct SystemCommands;

impl Plugin for SystemCommands {
    fn meta(&self) -> &Meta {
        &Meta {
            id: "system-commands",
            name: "System Commands",
            icon: "system-shutdown",
            ready: "Search system commands",
        }
    }

    fn search(
        &self,
        _query: &str,
        full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        let input = full.to_lowercase();
        Box::pin(async move { Ok(do_search(&input)) })
    }
}

fn do_search(input: &str) -> Vec<ResultItem> {
    if input.is_empty() {
        return vec![];
    }

    let commands = [
        (
            "Lock",
            "lock",
            "system-lock-screen",
            "loginctl lock-session",
        ),
        ("Suspend", "suspend", "system-suspend", "systemctl suspend"),
        ("Reboot", "reboot", "system-reboot", "systemctl reboot"),
        (
            "Shutdown",
            "shutdown",
            "system-shutdown",
            "systemctl poweroff",
        ),
        (
            "Logout",
            "logout",
            "system-log-out",
            "loginctl terminate-session $XDG_SESSION_ID",
        ),
    ];

    // Prefix match only: the fallback chain short-circuits on the first non-empty
    // plugin, so a mid-word hit would shadow app-search.
    commands
        .iter()
        .filter(|(name, keyword, _, _)| {
            name.to_lowercase().starts_with(input) || keyword.starts_with(input)
        })
        .map(|(name, _, icon, cmd)| ResultItem {
            title: name.to_string(),
            summary: Some(cmd.to_string()),
            on_click: Some(Action::Run {
                cmd: (*cmd).to_string(),
            }),
            icon: resolve(icon),
            ephemeral: true,
            actions: Vec::new(),
            badge: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_nothing() {
        assert!(do_search("").is_empty());
    }

    #[test]
    fn prefix_matches_by_name_or_keyword() {
        assert_eq!(do_search("re")[0].title, "Reboot");
        assert_eq!(do_search("susp")[0].title, "Suspend");
    }
}
