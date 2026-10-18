use std::collections::BinaryHeap;
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
pub(super) fn file_actions(item: &ResultItem) -> Vec<ActionItem> {
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

/// Whether the input names a path: `~` or a separator, the same trigger `f`/`d`
/// use for their direct-path branch and the keywordless path lookup uses.
pub(super) fn is_path_query(query: &str) -> bool {
    query.starts_with('~') || query.contains('/')
}

/// One row the keywordless path lookup offers: an existing path (`exact`), or a
/// completion under the typed directory.
pub(super) struct PathHit {
    pub item: ResultItem,
    pub exact: bool,
}

/// The rows for a path-shaped query: the existing path itself, or the typed
/// directory's matching entries. Only that directory is read, never a walk.
pub(super) fn path_rows(query: &str) -> Vec<PathHit> {
    let Ok(home) = get_home() else {
        return Vec::new();
    };
    path_rows_in(query, &home)
}

fn path_rows_in(query: &str, home: &Path) -> Vec<PathHit> {
    let query = query.trim();
    if !is_path_query(query) {
        return Vec::new();
    }
    // An existing path with no trailing separator is the answer itself; a
    // trailing separator, or a `.`/`..` component, asks for a listing.
    let leaf = query.rsplit('/').next().unwrap_or(query);
    let listing = query.ends_with('/') || leaf == "." || leaf == "..";
    if !listing && let Some(path) = path::exact_path(query, home) {
        let is_dir = std::fs::metadata(&path)
            .ok()
            .is_some_and(|meta| meta.is_dir());
        return vec![PathHit {
            item: entry_item(&path, is_dir),
            exact: true,
        }];
    }
    let Some(slash) = query.rfind('/') else {
        return Vec::new();
    };
    let Some(dir) = path::resolve_path_query(&query[..=slash], home) else {
        return Vec::new();
    };
    let leaf = query[slash + 1..].to_lowercase();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    // Keep the smallest names only, so a huge directory is one readdir pass
    // and O(n log cap) work, not a full sort.
    let cap = crate::provider::SHOW_CAP;
    let mut kept: BinaryHeap<(String, std::ffi::OsString)> = BinaryHeap::with_capacity(cap + 1);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(text) = name.to_str() else {
            continue;
        };
        let lower = text.to_lowercase();
        if (leaf.starts_with('.') || !lower.starts_with('.')) && lower.starts_with(&leaf) {
            kept.push((lower, name));
            if kept.len() > cap {
                kept.pop();
            }
        }
    }
    let mut kept = kept.into_vec();
    kept.sort_by(|a, b| a.0.cmp(&b.0));
    kept.into_iter()
        .map(|(_, name)| {
            let path = dir.join(name);
            PathHit {
                item: entry_item(&path, path.is_dir()),
                exact: false,
            }
        })
        .collect()
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

    let path_mode = is_path_query(query);
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
    fn only_path_syntax_reaches_the_path_lookup() {
        assert!(is_path_query("Project/WayRun"));
        assert!(is_path_query("~"));
        assert!(is_path_query("~/Project"));
        assert!(is_path_query("/etc/hosts"));
        assert!(!is_path_query("firefox"));
        assert!(path_rows_in("firefox", Path::new("/")).is_empty());
    }

    #[test]
    fn a_path_query_completes_the_directory_it_names() {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("Project");
        std::fs::create_dir_all(project.join("WayRun")).unwrap();
        std::fs::create_dir_all(project.join("Wayland")).unwrap();
        std::fs::write(project.join("Other.txt"), "x").unwrap();
        std::fs::write(project.join(".hidden"), "x").unwrap();

        let titles = |query: &str| -> Vec<String> {
            path_rows_in(query, home.path())
                .into_iter()
                .map(|hit| hit.item.title)
                .collect()
        };
        assert_eq!(titles("Project/Way"), ["Wayland/", "WayRun/"]);
        // a trailing separator lists everything, hidden names only on demand
        assert_eq!(titles("Project/"), ["Other.txt", "Wayland/", "WayRun/"]);
        assert_eq!(titles("Project/."), [".hidden"]);

        // an existing path is the answer itself, not a listing
        let exact = path_rows_in("Project/WayRun", home.path());
        assert_eq!(exact.len(), 1);
        assert!(exact[0].exact);
        assert_eq!(exact[0].item.title, "WayRun/");

        assert!(path_rows_in("Project/Nope", home.path()).is_empty());
    }

    #[test]
    fn a_listing_is_capped_and_sorted() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("Many");
        std::fs::create_dir_all(&dir).unwrap();
        for at in 0..60 {
            std::fs::write(dir.join(format!("f{at:02}.txt")), "x").unwrap();
        }

        let rows = path_rows_in("Many/", home.path());
        assert_eq!(rows.len(), crate::provider::SHOW_CAP);
        assert_eq!(rows[0].item.title, "f00.txt");
        assert_eq!(rows.last().unwrap().item.title, "f49.txt");
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
