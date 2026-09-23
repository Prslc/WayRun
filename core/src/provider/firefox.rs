use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result};
use rusqlite::Connection;
use tokio::task;

use crate::plugin::{Meta, Plugin};
use crate::system::fs::get_home;
use crate::system::icon::resolve;
use crate::wire::{Action, ActionItem, ResultItem};
use rust_i18n::t;

use super::copy_url_action;

enum Mode {
    Bookmarks,
    History,
}

fn find_db() -> Result<PathBuf> {
    let home = get_home()?;
    let bases = [
        home.join(".mozilla/firefox"),
        home.join(".config/mozilla/firefox"),
    ];

    // `read_dir` order is arbitrary and a machine can hold several profiles, so
    // take the most recently written one instead of the first listed.
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for base in &bases {
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let db = entry.path().join("places.sqlite");
                let Ok(modified) = fs::metadata(&db).and_then(|meta| meta.modified()) else {
                    continue;
                };
                if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
                    newest = Some((modified, db));
                }
            }
        }
    }

    newest
        .map(|(_, path)| path)
        .context("finding a Firefox profile with places.sqlite")
}

/// One copy at a time: a concurrent bookmark and history search share the cache.
static COPY_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// The path of `source`'s cached copy for its current `(mtime, size)`.
fn cached_copy(source: &Path) -> Result<PathBuf> {
    let meta = fs::metadata(source)?;
    let size = meta.len();
    let nanos = meta
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let dir = crate::system::fs::cache_dir().context("creating the cache directory")?;
    let stem = format!("places-{:016x}", hash_path(source));
    let target = dir.join(format!("{stem}-{nanos}-{size}.sqlite"));
    if target.is_file() {
        return Ok(target);
    }

    let _guard = COPY_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if target.is_file() {
        return Ok(target);
    }
    let tmp = dir.join(format!("{stem}.tmp"));
    fs::copy(source, &tmp)?;
    fs::rename(&tmp, &target)?;
    // Prune under the lock, so no concurrent copy owns a `.tmp` we remove.
    prune(&dir, &target);
    Ok(target)
}

/// Drop a superseded copy (an older snapshot or another profile) and a crashed
/// copy's `.tmp`, so the cache holds one `places` copy.
fn prune(dir: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let stale = (name.starts_with("places-") && name.ends_with(".sqlite") && path != keep)
            || (name.starts_with("places-") && name.ends_with(".tmp"));
        if stale {
            let _ = fs::remove_file(path);
        }
    }
}

/// A stable FNV-1a hash of the source path, for a profile-specific cache name.
fn hash_path(path: &Path) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

async fn do_search(mode: Mode, query: &str) -> Result<Vec<ResultItem>> {
    if query.is_empty() {
        return Ok(vec![]);
    }

    let query = query.to_string();
    task::spawn_blocking(move || {
        let db_path = find_db()?;
        let copy = cached_copy(&db_path)?;

        let conn = Connection::open(&copy)?;

        let sql = match mode {
            Mode::Bookmarks => {
                // Prefer the bookmark's own title (user-edited) over
                // `moz_places.title`, which Firefox overwrites each visit.
                "
                SELECT COALESCE(NULLIF(moz_bookmarks.title, ''), moz_places.title) AS title,
                       moz_places.url
                FROM moz_bookmarks
                JOIN moz_places ON moz_bookmarks.fk = moz_places.id
                WHERE moz_places.url <> ''
                  AND (?1 = ''
                       OR COALESCE(NULLIF(moz_bookmarks.title, ''), moz_places.title) LIKE ?2
                       OR moz_places.url LIKE ?2)
                ORDER BY moz_bookmarks.dateAdded DESC
                LIMIT 50
            "
            }
            Mode::History => {
                // `moz_places.last_visit_date` is the newest visit time, so the
                // join to `moz_historyvisits` only multiplies rows per visit.
                "
                SELECT moz_places.title, moz_places.url
                FROM moz_places
                WHERE moz_places.url <> ''
                  AND moz_places.last_visit_date IS NOT NULL
                  AND (?1 = '' OR moz_places.title LIKE ?2 OR moz_places.url LIKE ?2)
                ORDER BY moz_places.last_visit_date DESC
                LIMIT 50
            "
            }
        };

        let pattern = format!("%{query}%");
        let firefox_icon = resolve(match mode {
            Mode::Bookmarks => "builtin:bookmark",
            Mode::History => "builtin:clock",
        });
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([query.as_str(), &pattern], move |row| {
            let title: Option<String> = row.get(0)?;
            let url: String = row.get(1)?;
            Ok(ResultItem {
                title: title.unwrap_or_else(|| "[no title]".to_string()),
                summary: Some(url.clone()),
                on_click: Some(Action::Open { uri: url }),
                icon: firefox_icon.clone(),
                ephemeral: false,
                actions: Vec::new(),
                badge: None,
            })
        })?;

        Ok(rows.collect::<rusqlite::Result<Vec<ResultItem>>>()?)
    })
    .await?
}

macro_rules! firefox_plugin {
    ($name:ident, $mode:ident, $id:literal, $display:expr, $icon:literal, $ready:expr) => {
        pub struct $name {
            meta: Meta,
        }

        impl $name {
            pub fn new() -> Self {
                Self {
                    meta: Meta {
                        id: $id,
                        name: $display,
                        icon: $icon,
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
                let query = query.to_string();
                Box::pin(async move { do_search(Mode::$mode, &query).await })
            }

            fn actions(&self, item: &ResultItem) -> Vec<ActionItem> {
                copy_url_action(item)
            }
        }
    };
}

firefox_plugin!(
    FirefoxBookmarks,
    Bookmarks,
    "firefox-bookmarks",
    t!("plugin.bookmarks.name"),
    "builtin:bookmark",
    t!("plugin.bookmarks.ready")
);
firefox_plugin!(
    FirefoxHistory,
    History,
    "firefox-history",
    t!("plugin.history.name"),
    "builtin:clock",
    t!("plugin.history.ready")
);

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_query_matches_nothing() {
        assert!(do_search(Mode::Bookmarks, "").await.unwrap().is_empty());
        assert!(do_search(Mode::History, "").await.unwrap().is_empty());
    }

    #[test]
    fn prune_keeps_only_the_current_copy() {
        let dir = tempfile::tempdir().unwrap();
        let keep = dir.path().join("places-aa-1-2.sqlite");
        let old = dir.path().join("places-aa-0-2.sqlite");
        let tmp = dir.path().join("places-aa.tmp");
        let other = dir.path().join("plugin-hosts.json");
        for path in [&keep, &old, &tmp, &other] {
            fs::write(path, b"x").unwrap();
        }

        prune(dir.path(), &keep);

        assert!(keep.exists());
        assert!(!old.exists(), "a superseded snapshot is removed");
        assert!(!tmp.exists(), "a crashed copy's tmp is removed");
        assert!(other.exists(), "an unrelated cache file is left alone");
    }

    #[test]
    fn a_profile_path_hashes_stably() {
        let a = Path::new("/home/x/.mozilla/firefox/aaa/places.sqlite");
        let b = Path::new("/home/x/.mozilla/firefox/bbb/places.sqlite");
        assert_eq!(hash_path(a), hash_path(a));
        assert_ne!(hash_path(a), hash_path(b));
    }
}
