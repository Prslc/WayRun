use std::future::Future;
use std::pin::Pin;

use crate::plugin::{Match, Meta, Plugin, Rank, Ranked};
use crate::system::icon::find_icon_spec;
use crate::wire::{Action, ResultItem, SessionVerb};
use anyhow::Result;
use rust_i18n::t;

pub struct SystemCommands {
    meta: Meta,
}

impl SystemCommands {
    pub fn new() -> Self {
        Self {
            meta: Meta {
                id: "system-commands".into(),
                name: t!("plugin.system.name"),
                icon: "builtin:power".into(),
                ready: t!("plugin.system.ready"),
            },
        }
    }
}

struct Command {
    title: String,
    summary: String,
    keyword: &'static str,
    icon: &'static str,
    verb: SessionVerb,
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
        Box::pin(async move {
            Ok(do_search(&input)
                .into_iter()
                .map(|(_, item)| item)
                .collect())
        })
    }

    /// A command the query spells whole is `Exact`, a prefix of one `Prefix`.
    fn search_ranked(
        &self,
        _query: &str,
        full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Ranked>> + Send + '_>> {
        let input = full.to_lowercase();
        Box::pin(async move { Ok(do_search(&input)) })
    }
}

fn do_search(input: &str) -> Vec<(Rank, ResultItem)> {
    if input.is_empty() {
        return vec![];
    }

    // The keyword is what an English query matches, so it stays put; the title
    // translates and matches too.
    let commands = [
        Command {
            title: t!("command.lock"),
            summary: t!("command.lock.description"),
            keyword: "lock",
            icon: "builtin:lock",
            verb: SessionVerb::Lock,
        },
        Command {
            title: t!("command.suspend"),
            summary: t!("command.suspend.description"),
            keyword: "suspend",
            icon: "builtin:suspend",
            verb: SessionVerb::Suspend,
        },
        Command {
            title: t!("command.reboot"),
            summary: t!("command.reboot.description"),
            keyword: "reboot",
            icon: "builtin:reboot",
            verb: SessionVerb::Reboot,
        },
        Command {
            title: t!("command.shutdown"),
            summary: t!("command.shutdown.description"),
            keyword: "shutdown",
            icon: "builtin:power",
            verb: SessionVerb::Shutdown,
        },
        Command {
            title: t!("command.logout"),
            summary: t!("command.logout.description"),
            keyword: "logout",
            icon: "builtin:logout",
            verb: SessionVerb::Logout,
        },
    ];

    // Prefix only: the five commands are fixed, so a mid-word hit ("ock") is
    // noise rather than a match.
    commands
        .into_iter()
        .filter(|command| {
            command.title.to_lowercase().starts_with(input) || command.keyword.starts_with(input)
        })
        .map(|command| {
            let kind = if command.title.to_lowercase() == input || command.keyword == input {
                Match::Exact
            } else {
                Match::Prefix
            };
            (
                Rank::title(kind),
                ResultItem {
                    title: command.title,
                    summary: Some(command.summary),
                    on_click: Some(Action::Session { verb: command.verb }),
                    icon: find_icon_spec(command.icon),
                    icon_shape: None,
                    actions: Vec::new(),
                    preview: None,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(input: &str) -> ResultItem {
        do_search(input)
            .into_iter()
            .next()
            .expect("one matching command")
            .1
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(do_search("").is_empty());
    }

    #[test]
    fn prefix_matches_by_name_or_keyword() {
        assert_eq!(do_search("re")[0].1.title, t!("command.reboot"));
        assert_eq!(do_search("susp")[0].1.title, t!("command.suspend"));
    }

    #[test]
    fn a_command_row_carries_its_session_verb() {
        for (input, verb) in [
            ("lock", SessionVerb::Lock),
            ("suspend", SessionVerb::Suspend),
            ("reboot", SessionVerb::Reboot),
            ("shutdown", SessionVerb::Shutdown),
            ("logout", SessionVerb::Logout),
        ] {
            assert_eq!(only(input).on_click, Some(Action::Session { verb }));
        }
    }

    #[test]
    fn a_command_summary_describes_the_action() {
        for input in ["lock", "suspend", "reboot", "shutdown", "logout"] {
            let summary = only(input).summary.expect("a summary");
            assert!(!summary.contains("systemctl"), "{input}: {summary}");
            assert!(!summary.contains("loginctl"), "{input}: {summary}");
            assert!(!summary.contains("gdbus"), "{input}: {summary}");
        }
    }
}
