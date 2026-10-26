mod mime;
mod papirus;
mod remote;
mod theme;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use crate::live::{Live, Source};
use crate::system::xdg;
use crate::wire::{BUILTIN_FALLBACK, BUILTIN_GLYPHS};

use self::papirus::find_papirus;
use self::theme::{find_pixmap_icon, find_theme_icon};

pub use self::mime::content_type_icon;
pub(crate) use self::remote::{
    icon as remote_icon, preview as remote_preview, start as start_remote,
    supersede as remote_supersede,
};
pub use self::theme::warn_if_no_icon_theme;

/// Distinct specs kept before the map is dropped whole; an app's icon name can
/// be any string, and tiny values need a bound, not an eviction order.
const CACHE_MAX: usize = 4096;

/// Icon themes, pixmap roots and the GTK settings that name the start theme.
fn theme_roots() -> Vec<PathBuf> {
    let mut roots = xdg::icon_theme_dirs();
    roots.extend(xdg::pixmap_dirs());
    roots.extend(xdg::icon_settings_files());
    roots
}

static THEME_SOURCE: LazyLock<Arc<Source>> = LazyLock::new(|| Arc::new(Source::new(theme_roots)));

fn theme_source() -> Arc<Source> {
    Arc::clone(&THEME_SOURCE)
}

/// A `[icon] theme` edit arrives through the config watcher, not a watched root.
pub fn invalidate() {
    THEME_SOURCE.invalidate();
}

static CACHE: LazyLock<Live<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Live::shared(theme_source()).bounded(CACHE_MAX));

/// A renderable icon for `name` — an absolute path, or a `builtin:` glyph the shell
/// draws — or `None` (unknown glyphs included); cached, as a miss scans the theme space.
pub fn resolve(name: &str) -> Option<String> {
    CACHE.memoized(name, || lookup(name))
}

pub fn find_icon_spec(name: &str) -> Option<String> {
    resolve(name).or_else(|| resolve(BUILTIN_FALLBACK))
}

/// An external host's icon: an absolute path to a file the host ships; the
/// theme's namespace and the compiled glyphs stay out of reach.
pub fn host_icon_spec(spec: &str) -> Option<String> {
    spec.starts_with('/').then(|| spec.to_string())
}

/// A GIO icon serialized as `!!/abs/path` for a file icon, else a theme name.
pub fn gio_icon_path(spec: &str) -> Option<String> {
    if let Some(path) = spec.strip_prefix("!!") {
        return (!path.is_empty()).then(|| path.to_string());
    }
    resolve(spec)
}

/// What the Lua SDK's `wayrun.icon()` binds: an absolute path, a theme icon
/// name or a `papirus:` spec; a `builtin:` glyph resolves to nil.
pub fn resolve_for_host(name: &str) -> Option<String> {
    resolve(name).filter(|spec| !spec.starts_with("builtin:"))
}

/// The first name in `names` that resolves, without the bundled default: a MIME
/// type's themed-icon list is a priority chain.
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
    // reference, resolved here; the shell walks no theme.
    if let Some(spec) = name.strip_prefix("papirus:") {
        return find_papirus(spec);
    }
    if let Some(glyph) = name.strip_prefix("builtin:") {
        return BUILTIN_GLYPHS.contains(&glyph).then(|| name.to_string());
    }

    if let Some(p) = find_theme_icon(name) {
        return Some(p);
    }
    if let Some(p) = find_pixmap_icon(name) {
        return Some(p);
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
        assert_eq!(find_icon_spec(p), Some(p.to_string()));
    }

    #[test]
    fn resolve_leaves_a_miss_empty_and_find_icon_spec_fills_it() {
        assert!(resolve("definitely-not-an-icon-xyz").is_none());
        assert!(resolve("builtin:definitely-not-a-glyph").is_none());
        let path = find_icon_spec("definitely-not-an-icon-xyz").unwrap();
        assert_eq!(path, "builtin:app");
    }

    #[test]
    fn the_first_resolving_name_in_a_chain_wins() {
        let path = find_first_icon_path(["definitely-not-an-icon-xyz", "/tmp/icon.svg"]).unwrap();
        assert_eq!(path, "/tmp/icon.svg");
        // The app glyph is a `find_icon_spec` concern, not a chain entry.
        assert!(find_first_icon_path(["definitely-not-an-icon-xyz"]).is_none());
    }

    #[test]
    fn host_icon_spec_takes_shipped_files_only() {
        assert_eq!(
            host_icon_spec("/usr/share/icons/x.svg").as_deref(),
            Some("/usr/share/icons/x.svg")
        );
        // the theme namespace, the compiled glyphs and unknown names stay out of reach
        assert!(host_icon_spec("builtin:power").is_none());
        assert!(host_icon_spec("papirus:folder-open").is_none());
        assert!(host_icon_spec("firefox").is_none());
        assert!(host_icon_spec("").is_none());
    }

    #[test]
    fn a_host_resolves_names_but_gets_no_glyphs() {
        assert_eq!(
            resolve_for_host("/tmp/icon.svg").as_deref(),
            Some("/tmp/icon.svg")
        );
        assert!(resolve_for_host("builtin:app").is_none());
        assert!(resolve_for_host("definitely-not-an-icon-xyz").is_none());
    }
}
