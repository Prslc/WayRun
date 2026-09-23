use std::future::Future;
use std::pin::Pin;
use std::process::Command;

use crate::plugin::{Meta, Plugin};
use crate::system::icon::resolve;
use crate::wire::{Action, ResultItem};
use anyhow::Result;
use rust_i18n::t;

pub struct Clipboard {
    meta: Meta,
}

impl Clipboard {
    pub fn new() -> Self {
        Self {
            meta: Meta {
                id: "clipboard",
                name: t!("plugin.clipboard.name"),
                icon: "builtin:clipboard",
                ready: t!("plugin.clipboard.ready"),
            },
        }
    }
}

impl Plugin for Clipboard {
    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn search(
        &self,
        query: &str,
        _full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        let query = query.to_lowercase();
        Box::pin(async move {
            Ok(tokio::task::spawn_blocking(move || do_search(&query))
                .await
                .unwrap_or_default())
        })
    }
}

fn do_search(query: &str) -> Vec<ResultItem> {
    if query.is_empty() {
        return vec![];
    }

    let Ok(output) = Command::new("cliphist").arg("list").output() else {
        return vec![];
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut results = parse_entries(query, &text);
    let icon = resolve("builtin:clipboard");
    for r in &mut results {
        r.icon = icon.clone();
    }
    results
}

fn parse_entries(query: &str, raw: &str) -> Vec<ResultItem> {
    let mut results = Vec::new();

    for line in raw.lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        let (id, preview) = match parts.as_slice() {
            [id, _, preview] => (*id, *preview),
            [id, preview] => (*id, *preview),
            _ => continue,
        };

        if !query.is_empty() && !preview.to_lowercase().contains(query) {
            continue;
        }

        results.push(ResultItem {
            title: truncate_preview(preview),
            summary: None,
            on_click: Some(Action::Run {
                cmd: format!("sh -c 'cliphist decode {id} | wl-copy'"),
            }),
            icon: Some(String::new()),
            ephemeral: true,
            actions: Vec::new(),
            badge: None,
        });

        if results.len() >= 50 {
            break;
        }
    }

    results
}

const PREVIEW_MAX: usize = 80;

/// Cut a preview to [`PREVIEW_MAX`] bytes without splitting a codepoint.
fn truncate_preview(preview: &str) -> String {
    if preview.len() <= PREVIEW_MAX {
        return preview.to_string();
    }
    let end = preview
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= PREVIEW_MAX)
        .last()
        .unwrap_or(0);
    format!("{}…", &preview[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_format() {
        let raw = "1\thttps://example.com\thello world\n2\timage/png\tscreenshot";
        let entries = parse_entries("", raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "hello world");
        let Some(Action::Run { cmd }) = entries[0].on_click.as_ref() else {
            panic!("a clipboard row runs a command");
        };
        assert!(cmd.contains("decode 1"));
        assert_eq!(entries[1].title, "screenshot");
        assert!(entries.iter().all(|e| e.ephemeral));
    }

    #[test]
    fn filter_by_query() {
        let raw = "1\ttext/plain\tfirefox\n2\ttext/plain\tterminal";
        assert_eq!(parse_entries("fire", raw).len(), 1);
        assert_eq!(parse_entries("xyz", raw).len(), 0);
    }

    #[test]
    fn truncate_long_preview() {
        let long = "a".repeat(200);
        let raw = format!("1\ttext/plain\t{long}");
        let entries = parse_entries("", &raw);
        assert!(entries[0].title.len() <= 83); // 80 chars max + "…"
        assert!(entries[0].title.ends_with('…'));
    }

    #[test]
    fn a_multibyte_preview_cuts_on_a_char_boundary() {
        let long = "汉".repeat(40);
        let raw = format!("1\ttext/plain\t{long}");
        let entries = parse_entries("", &raw);
        let cut = entries[0].title.trim_end_matches('…');
        assert_eq!(cut.len(), 78, "the last boundary at or below 80 bytes");
        assert!(long.starts_with(cut));
    }

    #[test]
    fn empty_input() {
        assert!(parse_entries("", "").is_empty());
    }
}
