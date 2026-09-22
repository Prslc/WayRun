use std::future::Future;
use std::pin::Pin;

use crate::plugin::{Meta, Plugin};
use crate::system::icon::find_icon_path;
use crate::wire::{Action, ResultItem};
use anyhow::Result;
use rust_i18n::t;

pub struct SystemCommands {
    meta: Meta,
}

impl SystemCommands {
    pub fn new() -> Self {
        Self {
            meta: Meta {
                id: "system-commands",
                name: t!("plugin.system.name"),
                icon: "builtin:power",
                ready: t!("plugin.system.ready"),
            },
        }
    }
}

impl Plugin for SystemCommands {
    fn meta(&self) -> &Meta {
        &self.meta
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

    // The keyword is what an English query matches, so it stays put; the title
    // translates and matches too.
    let commands = [
        (
            t!("command.lock"),
            "lock",
            "builtin:lock",
            "loginctl lock-session",
        ),
        (
            t!("command.suspend"),
            "suspend",
            "builtin:suspend",
            "systemctl suspend",
        ),
        (
            t!("command.reboot"),
            "reboot",
            "builtin:reboot",
            "systemctl reboot",
        ),
        (
            t!("command.shutdown"),
            "shutdown",
            "builtin:power",
            "systemctl poweroff",
        ),
        (
            t!("command.logout"),
            "logout",
            "builtin:logout",
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
            title: name.clone(),
            summary: Some(cmd.to_string()),
            on_click: Some(Action::Run {
                cmd: (*cmd).to_string(),
            }),
            icon: find_icon_path(icon),
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
        assert_eq!(do_search("re")[0].title, t!("command.reboot"));
        assert_eq!(do_search("susp")[0].title, t!("command.suspend"));
    }
}
