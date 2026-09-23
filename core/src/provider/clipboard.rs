use std::future::Future;
use std::pin::Pin;
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

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

/// A typing burst shares one `cliphist list` round trip; a just-copied item
/// showing up half a second late is fine.
const CACHE_TTL: Duration = Duration::from_millis(500);

const MAX_RESULTS: usize = 50;

/// One `cliphist list` entry with the match form precomputed, so a keystroke
/// does not re-split and re-lower the whole list.
struct Entry {
    id: String,
    title: String,
    preview_lower: String,
}

impl Entry {
    /// The row that copies this entry back to the clipboard.
    fn row(&self, icon: &Option<String>) -> ResultItem {
        ResultItem {
            title: self.title.clone(),
            summary: None,
            on_click: Some(Action::Run {
                cmd: format!("sh -c 'cliphist decode {} | wl-copy'", self.id),
            }),
            icon: icon.clone(),
            ephemeral: true,
            actions: Vec::new(),
            badge: None,
        }
    }
}

/// The cached entry list and when it was fetched.
type ListCache = Mutex<Option<(Instant, Arc<Vec<Entry>>)>>;

static LIST: LazyLock<ListCache> = LazyLock::new(|| Mutex::new(None));

/// The clipboard history from a short-lived cache, so a burst of keystrokes
/// does not fork `cliphist list` on every one.
fn cached_entries() -> Option<Arc<Vec<Entry>>> {
    let mut cache = LIST.lock().unwrap_or_else(|err| err.into_inner());
    if let Some((at, entries)) = cache.as_ref()
        && at.elapsed() < CACHE_TTL
    {
        return Some(Arc::clone(entries));
    }
    let output = Command::new("cliphist").arg("list").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let entries = Arc::new(parse_entries(&text));
    *cache = Some((Instant::now(), Arc::clone(&entries)));
    Some(entries)
}

fn do_search(query: &str) -> Vec<ResultItem> {
    if query.is_empty() {
        return vec![];
    }

    let Some(entries) = cached_entries() else {
        return vec![];
    };
    let icon = resolve("builtin:clipboard");
    matching_rows(&entries, query, &icon)
}

/// The rows whose preview contains `query_lower`, capped to [`MAX_RESULTS`].
fn matching_rows(entries: &[Entry], query_lower: &str, icon: &Option<String>) -> Vec<ResultItem> {
    entries
        .iter()
        .filter(|entry| entry.preview_lower.contains(query_lower))
        .take(MAX_RESULTS)
        .map(|entry| entry.row(icon))
        .collect()
}

fn parse_entries(raw: &str) -> Vec<Entry> {
    let mut entries = Vec::new();

    for line in raw.lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        let (id, preview) = match parts.as_slice() {
            [id, _, preview] => (*id, *preview),
            [id, preview] => (*id, *preview),
            _ => continue,
        };

        entries.push(Entry {
            id: id.to_string(),
            title: truncate_preview(preview),
            preview_lower: preview.to_lowercase(),
        });
    }

    entries
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
        let entries = parse_entries(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "hello world");
        let row = entries[0].row(&None);
        let Some(Action::Run { cmd }) = row.on_click.as_ref() else {
            panic!("a clipboard row runs a command");
        };
        assert!(cmd.contains("decode 1"));
        assert_eq!(entries[1].title, "screenshot");
        assert!(row.ephemeral);
    }

    #[test]
    fn filter_by_query() {
        let raw = "1\ttext/plain\tfirefox\n2\ttext/plain\tterminal";
        let entries = parse_entries(raw);
        assert_eq!(matching_rows(&entries, "fire", &None).len(), 1);
        assert_eq!(matching_rows(&entries, "xyz", &None).len(), 0);
    }

    #[test]
    fn a_fresh_cache_is_reused() {
        let entries = Arc::new(parse_entries("1\ttext/plain\thello"));
        *LIST.lock().unwrap() = Some((Instant::now(), Arc::clone(&entries)));
        assert!(Arc::ptr_eq(&entries, &cached_entries().unwrap()));
    }

    #[test]
    fn truncate_long_preview() {
        let long = "a".repeat(200);
        let raw = format!("1\ttext/plain\t{long}");
        let entries = parse_entries(&raw);
        assert!(entries[0].title.len() <= 83); // 80 chars max + "…"
        assert!(entries[0].title.ends_with('…'));
    }

    #[test]
    fn a_multibyte_preview_cuts_on_a_char_boundary() {
        let long = "汉".repeat(40);
        let raw = format!("1\ttext/plain\t{long}");
        let entries = parse_entries(&raw);
        let cut = entries[0].title.trim_end_matches('…');
        assert_eq!(cut.len(), 78, "the last boundary at or below 80 bytes");
        assert!(long.starts_with(cut));
    }

    #[test]
    fn empty_input() {
        assert!(parse_entries("").is_empty());
    }
}
