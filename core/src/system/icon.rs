use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::system::xdg;
/// Papirus category dirs. Not every size ships every category: `panel` and
/// friends only exist in the small sizes, so lookup must scan sizes × categories.
const PAPIRUS_CATEGORIES: &[&str] = &[
    "actions",
    "apps",
    "categories",
    "devices",
    "emblems",
    "emotes",
    "mimetypes",
    "panel",
    "places",
    "status",
];
/// Preferred size order — the UI renders rows at 22–30px, so 48x48 gives crisp
/// downscale headroom without wasting load time on the 128px+ variants.
const PAPIRUS_SIZES: &[&str] = &[
    "48x48", "32x32", "64x64", "128x128", "96x96", "84x84", "42x42", "24x24", "22x22", "18x18",
    "16x16", "8x8",
];

/// Theme precedence, then categories and sizes, for the generic scan. Papirus
/// has the widest coverage here and leads.
const THEMES: &[&str] = &["Papirus", "breeze", "Adwaita", "hicolor"];
const THEME_CATEGORIES: &[&str] = &["places", "apps", "mimetypes", "devices", "panel", "actions"];
const ICON_SIZES: &[&str] = &[
    "scalable", "48x48", "32x32", "256x256", "128x128", "64x64", "24x24", "16x16",
];
const ICON_EXTS: &[&str] = &["svg", "png"];

/// `papirus:name` -> `(None, "name")`; `papirus:category/name` ->
/// `(Some("category"), "name")`.
fn parse_papirus_spec(spec: &str) -> (Option<&str>, &str) {
    match spec.split_once('/') {
        Some((cat, name)) => (Some(cat), name),
        None => (None, spec),
    }
}

fn find_papirus(spec: &str) -> Option<String> {
    let (hint, name) = parse_papirus_spec(spec);

    // category hint first when it names a real Papirus category, then the rest
    // (some names live under multiple categories)
    let mut categories: Vec<&str> = Vec::with_capacity(PAPIRUS_CATEGORIES.len() + 1);
    if let Some(h) = hint.filter(|h| PAPIRUS_CATEGORIES.contains(h)) {
        categories.push(h);
    }
    categories.extend(
        PAPIRUS_CATEGORIES
            .iter()
            .copied()
            .filter(|c| Some(*c) != hint),
    );

    let bases = xdg::icon_theme_dirs();

    // Cartesian scan in base × size × category order; first existing file wins.
    for base in &bases {
        for size in PAPIRUS_SIZES.iter().copied() {
            for category in categories.iter().copied() {
                let path = base
                    .join("Papirus")
                    .join(size)
                    .join(category)
                    .join(format!("{name}.svg"));
                if path.exists() {
                    return Some(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::default()))
}

/// A real icon file for `name`, or `None` when nothing matches. Cached, because
/// a miss scans the whole theme space.
fn find_exact(name: &str) -> Option<String> {
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
    find_exact(name).or_else(|| xdg::resource_path("images/application_default.png"))
}

/// The first name in `names` that resolves to a real icon file, without the
/// bundled default: a MIME type's themed-icon list is a priority chain.
pub fn find_first_icon_path<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<String> {
    names.into_iter().find_map(find_exact)
}

/// A theme name found under any `base/{theme}/{size}/{category}/`. The scan is
/// lazy and first-hit-wins, so a hit costs a few stats and a miss the full space.
fn find_theme_icon(name: &str) -> Option<String> {
    for base in xdg::icon_theme_dirs() {
        for theme in THEMES.iter().copied() {
            for category in THEME_CATEGORIES.iter().copied() {
                for size in ICON_SIZES.iter().copied() {
                    for ext in ICON_EXTS {
                        let path = base
                            .join(theme)
                            .join(size)
                            .join(category)
                            .join(format!("{name}.{ext}"));
                        if path.exists() {
                            return Some(path.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Flat raster icons in `/usr/share/pixmaps`, outside any theme.
fn find_pixmap_icon(name: &str) -> Option<String> {
    for base in xdg::pixmap_dirs() {
        for ext in ICON_EXTS {
            let path = base.join(format!("{name}.{ext}"));
            if path.exists() {
                return Some(path.to_string_lossy().into_owned());
            }
        }
    }
    None
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

    if let Some(p) = find_theme_icon(name) {
        return Some(p);
    }
    if let Some(p) = find_pixmap_icon(name) {
        return Some(p);
    }

    // project images (plugin identity icons, e.g. application_default)
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
    use std::path::Path;

    #[test]
    fn papirus_spec_splits_category_hint() {
        assert_eq!(parse_papirus_spec("folder-open"), (None, "folder-open"));
        assert_eq!(
            parse_papirus_spec("panel/system-shutdown"),
            (Some("panel"), "system-shutdown")
        );
    }

    #[test]
    fn papirus_resolves_to_installed_path() {
        if !Path::new("/usr/share/icons/Papirus").exists() {
            return; // theme not installed on this machine
        }
        let path = find_icon_path("papirus:folder-open").unwrap();
        assert!(path.contains("/Papirus/"));
        assert!(path.ends_with(".svg"));
    }
    #[test]
    fn absolute_path_passes_through_unchanged() {
        // An absolute path must short-circuit, or it re-enters the theme
        // search and falls back to the default placeholder.
        let p = "/usr/share/icons/Papirus/48x48/apps/github.svg";
        assert_eq!(find_icon_path(p), Some(p.to_string()));
    }

    #[test]
    fn papirus_category_hint_resolves() {
        if !Path::new("/usr/share/icons/Papirus").exists() {
            return;
        }
        // `system-shutdown` also lives under `apps`; the hint scopes to `panel`
        // first, then falls back to the other categories.
        let path = find_icon_path("papirus:apps/system-shutdown").unwrap();
        assert!(path.contains("/apps/system-shutdown.svg"));
    }

    #[test]
    fn papirus_unknown_name_falls_back_to_default() {
        let path = find_icon_path("papirus:definitely-not-an-icon-xyz");
        assert!(path.is_some()); // default icon, same semantics as any miss
    }

    #[test]
    fn a_bundled_image_resolves_when_the_theme_misses() {
        // No theme ships `application_default`, so the bundled `images/` copy is
        // what answers.
        let path = find_icon_path("application_default").unwrap();
        assert!(path.ends_with("images/application_default.png"), "{path}");
    }

    #[test]
    fn the_first_resolving_name_in_a_chain_wins() {
        if !Path::new("/usr/share/icons/Papirus").exists() {
            return;
        }
        let path = find_first_icon_path(["definitely-not-an-icon-xyz", "text-x-generic"]).unwrap();
        assert!(path.ends_with("text-x-generic.svg"), "{path}");
        // The bundled default is a `find_icon_path` concern, not a chain entry.
        assert!(find_first_icon_path(["definitely-not-an-icon-xyz"]).is_none());
    }
}
