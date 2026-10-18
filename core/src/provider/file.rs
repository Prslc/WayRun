use std::ffi::OsStr;
use std::future::Future;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::pin::Pin;

use anyhow::Result;
use gio::prelude::FileExt;

use crate::plugin::{Meta, Plugin};
use crate::system::fs::get_home;
use crate::system::icon::{content_type_icon, resolve};
use crate::wire::{Action, ActionItem, ResultItem};
use rust_i18n::t;

// The index is a detail of this provider, but the registry has to know whether
// to keep its cache alive, so the crate may reach it.
pub(crate) mod index;
mod path;
mod walk;

use self::path::{exact_path, path_item};
use self::walk::quick_search;

/// The icon the system MIME database assigns to `path`.
fn mime_icon(path: &Path) -> Option<String> {
    let (content_type, _) = gio::content_type_guess(Some(path), None);
    content_type_icon(&content_type)
}

macro_rules! search_plugin {
    ($name:ident, $id:literal, $display:expr, $icon:literal, $by_name:literal, $dirs:literal, $ready:expr) => {
        pub struct $name {
            meta: Meta,
        }

        impl $name {
            pub fn new() -> Self {
                Self {
                    meta: Meta {
                        id: $id.into(),
                        name: $display,
                        icon: $icon.into(),
                        ready: $ready,
                    },
                }
            }
        }

        impl Plugin for $name {
            fn meta(&self) -> &Meta {
                &self.meta
            }

            fn search(
                &self,
                query: &str,
                _full: &str,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
                // Keep the query's case: a path query is stat'd against the real
                // filesystem, where case matters.
                let query = query.to_string();
                Box::pin(async move {
                    index::ensure().await;
                    Ok(
                        tokio::task::spawn_blocking(move || do_search(&query, $dirs, $by_name))
                            .await
                            .unwrap_or_default(),
                    )
                })
            }

            fn actions(&self, item: &ResultItem) -> Vec<ActionItem> {
                file_actions(item)
            }
        }
    };
}

/// A file or directory row's commands, in panel order: a terminal leads so the
/// two ways to open sit together, then reveal in the file manager, then copy its path.
fn file_actions(item: &ResultItem) -> Vec<ActionItem> {
    let Some(Action::Open { uri }) = item.on_click.as_ref() else {
        return Vec::new();
    };
    if !uri.starts_with("file:") {
        return Vec::new();
    }
    let Some(path) = gio::File::for_uri(uri).path() else {
        return Vec::new();
    };
    let path = path.to_string_lossy();
    vec![
        ActionItem {
            title: t!("action.terminal"),
            action: Action::Terminal { uri: uri.clone() },
            icon: Some("builtin:terminal".to_string()),
            id: Some("terminal".to_string()),
            plugin: None,
            default: false,
        },
        ActionItem {
            title: t!("action.reveal"),
            action: Action::Reveal { uri: uri.clone() },
            icon: Some("builtin:reveal".to_string()),
            id: Some("reveal".to_string()),
            plugin: None,
            default: false,
        },
        ActionItem {
            title: t!("action.copy_path"),
            action: Action::Copy {
                text: path.into_owned(),
            },
            icon: Some("builtin:copy".to_string()),
            id: Some("copy_path".to_string()),
            plugin: None,
            default: false,
        },
    ]
}

search_plugin!(
    FileSearch,
    "file-search",
    t!("plugin.file.name"),
    "builtin:file",
    true,
    false,
    t!("plugin.file.ready")
);
search_plugin!(
    PathSearch,
    "path-search",
    t!("plugin.directory.name"),
    "builtin:folder",
    false,
    true,
    t!("plugin.directory.ready")
);

/// One walked or stat'd path as a result row. GLib builds the URI, because raw
/// paths are invalid for spaces and non-ASCII.
pub(super) fn entry_item(path: &Path, is_dir: bool) -> ResultItem {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    ResultItem {
        on_click: Some(Action::Open {
            uri: gio::File::for_path(path).uri().to_string(),
        }),
        title: if is_dir { format!("{name}/") } else { name },
        summary: Some(path.to_string_lossy().into_owned()),
        icon: if is_dir {
            resolve("builtin:folder")
        } else {
            mime_icon(path)
        },
        actions: Vec::new(),
    }
}

/// Walk filter: hidden entries and the configured names never enter the index
/// or a result.
pub(super) fn keep_name(name: &OsStr, exclude: &[String]) -> bool {
    let name = name.as_bytes();
    !name.starts_with(b".") && !exclude.iter().any(|entry| entry.as_bytes() == name)
}

/// `want_dir` keeps `f` to files and `d` to directories, so the two providers stay
/// complementary; `by_name` is `f`'s name-only match; a path query matches the path.
fn do_search(query: &str, want_dir: bool, by_name: bool) -> Vec<ResultItem> {
    let query = query.trim();
    if query.is_empty() {
        return vec![];
    }

    let Ok(home) = get_home() else {
        return vec![];
    };

    // An existing path is the answer itself, outside the walk's depth and
    // roots; an absolute path may point anywhere, a relative one stays home.
    if let Some(path) = exact_path(query, &home)
        && let Some(item) = path_item(&path, want_dir)
    {
        return vec![item];
    }

    let path_mode = query.starts_with('~') || query.contains('/');
    let name_only = by_name && !path_mode;
    let query_lower = query.to_lowercase();

    if let Some(items) = index::search(&home, &query_lower, want_dir, name_only) {
        return items;
    }

    quick_search(
        &home,
        &query_lower,
        want_dir,
        name_only,
        &crate::config::get().files,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, on_click: Option<Action>) -> ResultItem {
        ResultItem {
            title: title.to_string(),
            summary: None,
            on_click,
            icon: None,
            actions: Vec::new(),
        }
    }

    fn file(uri: &str) -> Option<Action> {
        Some(Action::Open {
            uri: uri.to_string(),
        })
    }

    /// The names a stock install skips: the shipped template's list, the one
    /// place those defaults are written down.
    fn shipped_exclude() -> Vec<String> {
        toml::from_str::<crate::config::Config>(crate::config::DEFAULT_TEMPLATE)
            .expect("the template parses")
            .files
            .exclude
    }

    #[test]
    fn a_file_row_offers_terminal_reveal_and_copy() {
        let actions: Vec<ActionItem> = file_actions(&row("a.txt", file("file:///tmp/a.txt")));
        let titles: Vec<&str> = actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                t!("action.terminal"),
                t!("action.reveal"),
                t!("action.copy_path")
            ],
            "opening leads, so it sits next to the row's own command"
        );
        assert_eq!(
            actions[0].action,
            Action::Terminal {
                uri: "file:///tmp/a.txt".to_string()
            }
        );
        assert_eq!(
            actions[1].action,
            Action::Reveal {
                uri: "file:///tmp/a.txt".to_string()
            }
        );
        assert_eq!(
            actions[2].action,
            Action::Copy {
                text: "/tmp/a.txt".to_string()
            }
        );

        assert!(
            file_actions(&row(
                "run",
                Some(Action::Run {
                    cmd: "ls".to_string()
                })
            ))
            .is_empty()
        );
        assert!(file_actions(&row("none", None)).is_empty());
    }

    #[test]
    fn the_copied_path_is_percent_decoded() {
        let actions = file_actions(&row("a b", file("file:///tmp/a%20b")));
        let copy = actions
            .iter()
            .find(|action| action.id.as_deref() == Some("copy_path"))
            .expect("the row offers a copy");
        assert_eq!(
            copy.action,
            Action::Copy {
                text: "/tmp/a b".to_string()
            }
        );
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
        // the same type answers the same icon on a repeat lookup
        assert_eq!(
            mime_icon(Path::new("/tmp/build.sh")).as_ref(),
            Some(&script)
        );
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(do_search("", false, true).is_empty());
        assert!(do_search("", true, false).is_empty());
    }

    #[test]
    fn the_index_prunes_dotdirs_and_whatever_the_config_lists() {
        let shipped = shipped_exclude();
        for name in [".config", ".git", "node_modules", "target", "__pycache__"] {
            assert!(!keep_name(OsStr::new(name), &shipped), "{name}");
        }
        for name in ["Desktop", "Documents", "Downloads", "notes.txt", "src"] {
            assert!(keep_name(OsStr::new(name), &shipped), "{name}");
        }
        // the dot rule holds with no list at all, and a listed name need not be
        // hidden; the shipped caches are the template's list, not a walk rule
        let custom = ["vendor".to_string()];
        assert!(!keep_name(OsStr::new(".git"), &[]));
        assert!(!keep_name(OsStr::new("vendor"), &custom));
        assert!(keep_name(OsStr::new("target"), &[]));
        assert!(keep_name(OsStr::new("notes.txt"), &custom));
    }
}
