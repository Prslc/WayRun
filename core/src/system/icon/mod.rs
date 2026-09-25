mod builtin;
mod mime;
mod papirus;
mod theme;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::system::xdg;

use self::builtin::builtin_icon;
use self::papirus::find_papirus;
use self::theme::{ICON_EXTS, find_pixmap_icon, find_theme_icon};

pub use self::mime::content_type_icon;
pub use self::theme::warn_if_no_icon_theme;

/// The glyph a miss falls back to, so a row always has an icon.
const APP_ICON: &str = "builtin:app";

static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::default()))
}

/// A real icon file for `name`, or `None` on a miss; cached, because a miss
/// scans the whole theme space, and the row stays iconless for the identity fill.
pub fn resolve(name: &str) -> Option<String> {
    if let Ok(cache) = cache().lock()
        && let Some(cached) = cache.get(name)
    {
        return cached.clone();
    }

    let result = lookup(name);

    if let Ok(mut cache) = cache().lock() {
        cache.insert(name.to_string(), result.clone());
    }

    result
}

pub fn find_icon_path(name: &str) -> Option<String> {
    resolve(name).or_else(|| resolve(APP_ICON))
}

/// An external host's icon: only an absolute path to a file the host ships is
/// accepted; the core offers hosts no icon namespace.
pub fn host_icon_path(spec: &str) -> Option<String> {
    spec.starts_with('/').then(|| spec.to_string())
}

/// The first name in `names` that resolves to a real icon file, without the
/// bundled default: a MIME type's themed-icon list is a priority chain.
pub fn find_first_icon_path<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<String> {
    names.into_iter().find_map(resolve)
}

fn lookup(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if name.starts_with('/') {
        return Some(name.to_string());
    }
    // `papirus:<name>` (or `papirus:<category>/<name>`) is an explicit Papirus
    // reference; resolve it here, the UI renders only absolute paths.
    if let Some(spec) = name.strip_prefix("papirus:") {
        return find_papirus(spec);
    }
    if let Some(builtin) = name.strip_prefix("builtin:") {
        return builtin_icon(builtin);
    }

    if let Some(p) = find_theme_icon(name) {
        return Some(p);
    }
    if let Some(p) = find_pixmap_icon(name) {
        return Some(p);
    }

    // a bundled image in the resource dir, named directly
    for ext in ICON_EXTS {
        if let Some(p) = xdg::resource_path(&format!("images/{name}.{ext}")) {
            return Some(p);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_path_passes_through_unchanged() {
        // An absolute path must short-circuit, or it re-enters the theme
        // search and falls back to the default placeholder.
        let p = "/usr/share/icons/Papirus/48x48/apps/github.svg";
        assert_eq!(find_icon_path(p), Some(p.to_string()));
    }

    #[test]
    fn resolve_leaves_a_miss_empty_and_find_icon_path_fills_it() {
        assert!(resolve("definitely-not-an-icon-xyz").is_none());
        let path = find_icon_path("definitely-not-an-icon-xyz").unwrap();
        assert!(path.ends_with("builtin/app.svg"), "{path}");
    }

    #[test]
    fn the_first_resolving_name_in_a_chain_wins() {
        let path = find_first_icon_path(["definitely-not-an-icon-xyz", "/tmp/icon.svg"]).unwrap();
        assert_eq!(path, "/tmp/icon.svg");
        // The app glyph is a `find_icon_path` concern, not a chain entry.
        assert!(find_first_icon_path(["definitely-not-an-icon-xyz"]).is_none());
    }

    #[test]
    fn host_icon_path_accepts_only_absolute_paths() {
        assert_eq!(
            host_icon_path("/usr/share/icons/x.svg").as_deref(),
            Some("/usr/share/icons/x.svg")
        );
        // the core's own namespace is not offered to external hosts
        assert!(host_icon_path("builtin:power").is_none());
        assert!(host_icon_path("papirus:folder-open").is_none());
        assert!(host_icon_path("firefox").is_none());
        assert!(host_icon_path("").is_none());
    }
}
