use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use anyhow::Result;
use gio::prelude::{Cast, FileExt};
use walkdir::WalkDir;

use crate::plugin::{Meta, Plugin};
use crate::system::fs::get_home;
use crate::system::icon::{find_first_icon_path, resolve};
use crate::wire::{ActionItem, ResultItem};

/// The icon the system MIME database assigns to `path`. Its themed-icon list is
/// a priority order, so the first name the theme actually ships wins.
fn mime_icon(path: &Path) -> Option<String> {
    let (content_type, _) = gio::content_type_guess(Some(path), None);
    let icon = gio::content_type_get_icon(&content_type);
    let themed = icon.downcast::<gio::ThemedIcon>().ok()?;
    let names = themed.names();
    find_first_icon_path(names.iter().map(|name| name.as_str()))
}

macro_rules! search_plugin {
    ($name:ident, $id:literal, $display:literal, $matcher:ident, $ready:literal) => {
        pub struct $name;

        impl Plugin for $name {
            fn meta(&self) -> &Meta {
                &Meta {
                    id: $id,
                    name: $display,
                    icon: "folder",
                    ready: $ready,
                }
            }

            fn search(
                &self,
                query: &str,
                _full: &str,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
                let query = query.to_lowercase();
                Box::pin(async move {
                    Ok(
                        tokio::task::spawn_blocking(move || do_search(&query, $matcher))
                            .await
                            .unwrap_or_default(),
                    )
                })
            }

            fn actions(&self, item: &ResultItem) -> Vec<ActionItem> {
                reveal_action(item)
            }
        }
    };
}

/// A file row's one extra command: hand its URI to the file manager.
fn reveal_action(item: &ResultItem) -> Vec<ActionItem> {
    let Some(uri) = item
        .on_click
        .as_deref()
        .filter(|on_click| on_click.starts_with("file:"))
    else {
        return Vec::new();
    };
    vec![ActionItem {
        title: "Reveal in file manager".to_string(),
        on_click: format!("reveal:{uri}"),
        icon: Some("folder-open".to_string()),
    }]
}

search_plugin!(
    FileSearch,
    "file-search",
    "Files",
    match_name,
    "Search files by name"
);
search_plugin!(
    PathSearch,
    "path-search",
    "Paths",
    match_path,
    "Search files by path"
);

fn match_name(entry_name: &str, _entry_path: &str, query: &str) -> bool {
    entry_name.to_lowercase().contains(query)
}

fn match_path(_entry_name: &str, entry_path: &str, query: &str) -> bool {
    let path_lower = entry_path.to_lowercase();
    query
        .split_whitespace()
        .all(|token| path_lower.contains(token))
}

/// Walk filter: skip hidden dirs and build caches, and skip the three roots at
/// depth 1 under `~` (they are walked on their own), so nothing is doubled.
fn keep_entry(name: &str, depth: usize) -> bool {
    if name.starts_with('.') || name == "node_modules" || name == "target" || name == "__pycache__"
    {
        return false;
    }
    !(depth == 1 && matches!(name, "Desktop" | "Documents" | "Downloads"))
}

fn do_search(query: &str, matcher: fn(&str, &str, &str) -> bool) -> Vec<ResultItem> {
    if query.is_empty() {
        return vec![];
    }

    let Ok(home) = get_home() else {
        return vec![];
    };

    let roots = [
        home.join("Desktop"),
        home.join("Documents"),
        home.join("Downloads"),
        home.clone(),
    ];

    let mut results = Vec::new();

    for root in &roots {
        if !root.exists() {
            continue;
        }

        let walker = WalkDir::new(root)
            .max_depth(3)
            .into_iter()
            .filter_entry(|e| keep_entry(&e.file_name().to_string_lossy(), e.depth()));

        for entry in walker.filter_map(Result::ok) {
            let ft = entry.file_type();
            let is_dir = ft.is_dir();
            if !is_dir && !ft.is_file() {
                continue;
            }

            let path = entry.path().to_string_lossy().into_owned();
            let name = entry.file_name().to_string_lossy();

            if !matcher(&name, &path, query) {
                continue;
            }

            // GLib builds the URI: raw paths are invalid for spaces/non-ASCII.
            let file_url = gio::File::for_path(&path).uri().to_string();

            let title = if is_dir {
                format!("{name}/")
            } else {
                name.into_owned()
            };
            let icon = if is_dir {
                resolve("folder")
            } else {
                mime_icon(Path::new(&path))
            };

            results.push(ResultItem {
                title,
                summary: Some(path),
                on_click: Some(file_url),
                icon,
                ephemeral: false,
                actions: Vec::new(),
                badge: None,
            });

            if results.len() >= 50 {
                break;
            }
        }

        if results.len() >= 50 {
            break;
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{ActionItem, ResultItem};

    fn row(title: &str, on_click: Option<&str>) -> ResultItem {
        ResultItem {
            title: title.to_string(),
            summary: None,
            on_click: on_click.map(str::to_string),
            icon: None,
            ephemeral: false,
            actions: Vec::new(),
            badge: None,
        }
    }

    #[test]
    fn only_a_file_row_offers_a_reveal() {
        let actions: Vec<ActionItem> = reveal_action(&row("a.txt", Some("file:///tmp/a.txt")));
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].title, "Reveal in file manager");
        assert_eq!(actions[0].on_click, "reveal:file:///tmp/a.txt");

        assert!(reveal_action(&row("run", Some("run:ls"))).is_empty());
        assert!(reveal_action(&row("none", None)).is_empty());
    }

    #[test]
    fn a_suffix_maps_to_its_mime_icon_without_a_hand_kept_table() {
        if !Path::new("/usr/share/icons/Papirus").exists() {
            return;
        }
        // `g_content_type_guess` needs only the name; no file is read.
        let script = mime_icon(Path::new("/tmp/build.sh")).unwrap();
        assert!(
            script.ends_with("text-x-shellscript.svg") || script.ends_with("text-x-script.svg"),
            "{script}"
        );
        let unknown = mime_icon(Path::new("/tmp/notes.zzz")).unwrap();
        assert!(
            unknown.ends_with("application-octet-stream.svg")
                || unknown.ends_with("application-x-generic.svg"),
            "{unknown}"
        );
        assert_ne!(script, unknown);
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(do_search("", match_name).is_empty());
        assert!(do_search("", match_path).is_empty());
    }

    #[test]
    fn the_home_roots_are_skipped_only_under_the_home_root() {
        // depth 1 under `~`: walked as its own root, so skip it here.
        assert!(!keep_entry("Desktop", 1));
        assert!(!keep_entry("Documents", 1));
        assert!(!keep_entry("Downloads", 1));
        // a same-named dir deeper, or one directly inside a root, is kept.
        assert!(keep_entry("Desktop", 2));
        assert!(keep_entry("Desktop", 0));
        assert!(keep_entry("Documents", 2));
        // build caches and dotdirs are skipped anywhere.
        assert!(!keep_entry(".config", 1));
        assert!(!keep_entry("node_modules", 1));
        assert!(!keep_entry("target", 3));
        assert!(!keep_entry("__pycache__", 2));
        assert!(keep_entry("notes.txt", 2));
    }
}
