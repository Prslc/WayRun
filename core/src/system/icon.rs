use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
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

/// The row box is 22-30px, so a 48px source gives crisp downscale headroom.
const TARGET_SIZE: u16 = 48;
const ICON_EXTS: &[&str] = &["svg", "png"];

/// The glyph a miss falls back to, so a row always has an icon.
const APP_ICON: &str = "builtin:app";

/// UI glyphs compiled in and referenced as `builtin:<name>`, so the panel, badges
/// and built-in plugin identities never depend on an installed theme. See the
/// NOTICE beside them.
const BUILTIN_ICONS: &[(&str, &[u8])] = &[
    ("app", include_bytes!("../../assets/icons/app.svg")),
    (
        "bookmark",
        include_bytes!("../../assets/icons/bookmark.svg"),
    ),
    (
        "calculator",
        include_bytes!("../../assets/icons/calculator.svg"),
    ),
    (
        "clipboard",
        include_bytes!("../../assets/icons/clipboard.svg"),
    ),
    ("clock", include_bytes!("../../assets/icons/clock.svg")),
    ("copy", include_bytes!("../../assets/icons/copy.svg")),
    ("file", include_bytes!("../../assets/icons/file.svg")),
    ("folder", include_bytes!("../../assets/icons/folder.svg")),
    ("globe", include_bytes!("../../assets/icons/globe.svg")),
    ("lock", include_bytes!("../../assets/icons/lock.svg")),
    ("logout", include_bytes!("../../assets/icons/logout.svg")),
    ("open", include_bytes!("../../assets/icons/open.svg")),
    ("pin", include_bytes!("../../assets/icons/pin.svg")),
    ("power", include_bytes!("../../assets/icons/power.svg")),
    ("reboot", include_bytes!("../../assets/icons/reboot.svg")),
    ("remove", include_bytes!("../../assets/icons/remove.svg")),
    ("reveal", include_bytes!("../../assets/icons/reveal.svg")),
    ("suspend", include_bytes!("../../assets/icons/suspend.svg")),
    (
        "terminal",
        include_bytes!("../../assets/icons/terminal.svg"),
    ),
    ("unpin", include_bytes!("../../assets/icons/unpin.svg")),
    ("window", include_bytes!("../../assets/icons/window.svg")),
];

/// `papirus:name` -> `(None, "name")`; `papirus:category/name` ->
/// `(Some("category"), "name")`.
fn parse_papirus_spec(spec: &str) -> (Option<&str>, &str) {
    match spec.split_once('/') {
        Some((cat, name)) => (Some(cat), name),
        None => (None, spec),
    }
}

fn find_papirus(spec: &str) -> Option<String> {
    // `papirus:symbolic/[<category>/]<name>` scans the symbolic tree, which only
    // ships at the small sizes but is monochrome line art that tints cleanly.
    let (symbolic, spec) = match spec.strip_prefix("symbolic/") {
        Some(rest) => (true, rest),
        None => (false, spec),
    };
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
                let mut dir = base.join("Papirus").join(size);
                if symbolic {
                    dir = dir.join("symbolic");
                }
                let path = dir.join(category).join(format!("{name}.svg"));
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
/// a miss scans the whole theme space. A row's own spec uses this, so a miss
/// stays empty and the owning plugin's identity icon can answer for it.
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

/// Write `bytes` as `name` under `dir`, replacing a stale copy; returns the path
/// the shell reads. A glyph's bytes change with the binary, so the cache cannot
/// be trusted to match it.
fn write_cached(dir: &Path, name: &str, bytes: &[u8]) -> Option<String> {
    std::fs::create_dir_all(dir).ok()?;
    let target = dir.join(name);
    if std::fs::read(&target).ok().as_deref() != Some(bytes) {
        crate::system::fs::write_atomic(&target, bytes).ok()?;
    }
    Some(target.to_string_lossy().into_owned())
}

/// A `builtin:<name>` glyph written into the cache.
fn builtin_icon(name: &str) -> Option<String> {
    let (name, bytes) = BUILTIN_ICONS.iter().find(|(n, _)| *n == name)?;
    let dir = crate::system::fs::cache_dir()?.join("builtin");
    write_cached(&dir, &format!("{name}.svg"), bytes)
}

pub fn warn_if_no_icon_theme() {
    let roots = xdg::icon_theme_dirs();
    let found = theme_chain(&roots).iter().any(|theme| {
        roots
            .iter()
            .any(|root| root.join(theme).join("index.theme").is_file())
    });
    if !found {
        eprintln!("wayrun: no icon theme found; row and action icons use the built-in placeholder");
    }
}

/// The first name in `names` that resolves to a real icon file, without the
/// bundled default: a MIME type's themed-icon list is a priority chain.
pub fn find_first_icon_path<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<String> {
    names.into_iter().find_map(resolve)
}

/// One directory a theme's `index.theme` declares.
struct ThemeDir {
    path: String,
    size: u16,
    min: u16,
    max: u16,
    scale: u16,
}

/// A theme's `index.theme`: the directories it lists and the themes it inherits.
#[derive(Default)]
struct ThemeIndex {
    dirs: Vec<ThemeDir>,
    inherits: Vec<String>,
}

/// Parse the `Directories=` list, each directory's size range and `Inherits` from
/// an `index.theme`; a file without `[Icon Theme]` yields an empty index.
fn parse_index(text: &str) -> ThemeIndex {
    let mut order: Vec<String> = Vec::new();
    let mut inherits: Vec<String> = Vec::new();
    let mut attrs: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut section = String::new();
    let mut header = false;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
            header = section == "Icon Theme";
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if header {
            match key {
                "Directories" => order = split_list(value),
                "Inherits" => inherits = split_list(value),
                _ => {}
            }
        } else if !section.is_empty() {
            attrs
                .entry(section.clone())
                .or_default()
                .insert(key.to_string(), value.to_string());
        }
    }

    let dirs = order
        .iter()
        .map(|name| {
            let attrs = attrs.get(name);
            let num = |key: &str| {
                attrs
                    .and_then(|a| a.get(key))
                    .and_then(|v| v.parse::<u16>().ok())
            };
            let size = num("Size").unwrap_or(0);
            let scale = num("Scale").unwrap_or(1);
            let ty = attrs
                .and_then(|a| a.get("Type"))
                .map(|t| t.to_ascii_lowercase());
            let (min, max) = match ty.as_deref() {
                Some("fixed") => (size, size),
                Some("scalable") => (
                    num("MinSize").unwrap_or(size),
                    num("MaxSize").unwrap_or(size),
                ),
                _ => {
                    let threshold = num("Threshold").unwrap_or(2);
                    (
                        size.saturating_sub(threshold),
                        size.saturating_add(threshold),
                    )
                }
            };
            // A missing `Size` leaves the range open, so it is never skipped.
            let (min, max) = if size == 0 { (0, u16::MAX) } else { (min, max) };
            ThemeDir {
                path: name.clone(),
                size,
                min,
                max,
                scale,
            }
        })
        .collect();

    ThemeIndex { dirs, inherits }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Rank a directory against the target size: an in-range directory first, then
/// larger sizes (downscale headroom), then smaller.
fn rank(dir: &ThemeDir) -> (u8, u16) {
    if dir.min <= TARGET_SIZE && TARGET_SIZE <= dir.max {
        (0, 0)
    } else if dir.size > TARGET_SIZE {
        (1, dir.size - TARGET_SIZE)
    } else {
        (2, TARGET_SIZE - dir.size)
    }
}

/// The first `name.svg`/`name.png` in the theme's declared directories, best size
/// match first. Scale-2 (`@2`) directories are skipped: the shell scales itself.
fn find_in_theme(dir: &Path, index: &ThemeIndex, name: &str) -> Option<String> {
    let mut dirs: Vec<&ThemeDir> = index.dirs.iter().filter(|d| d.scale <= 1).collect();
    dirs.sort_by_key(|d| rank(d));
    for entry in dirs {
        for ext in ICON_EXTS {
            let path = dir.join(&entry.path).join(format!("{name}.{ext}"));
            if path.is_file() {
                return Some(path.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Fallback for a theme without a usable `index.theme`: try `name.<ext>` one and
/// two directories down, which covers every layout the spec allows.
fn scan_theme(dir: &Path, name: &str) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let sub = entry.path();
        if !sub.is_dir() {
            continue;
        }
        for ext in ICON_EXTS {
            let path = sub.join(format!("{name}.{ext}"));
            if path.is_file() {
                return Some(path.to_string_lossy().into_owned());
            }
            let Ok(inner) = std::fs::read_dir(&sub) else {
                continue;
            };
            for inner in inner.flatten() {
                let inner = inner.path();
                let path = inner.join(format!("{name}.{ext}"));
                if inner.is_dir() && path.is_file() {
                    return Some(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

/// The parsed `index.theme` of `theme`, from the first root that ships one.
fn index_for(theme: &str, roots: &[PathBuf]) -> Option<ThemeIndex> {
    roots.iter().find_map(|root| {
        std::fs::read_to_string(root.join(theme).join("index.theme"))
            .ok()
            .map(|text| parse_index(&text))
    })
}

/// The theme lookup order: `start`, then each theme's `Inherits` breadth-first,
/// then `hicolor`, deduplicated and cycle-safe.
fn theme_chain_from(start: &str, roots: &[PathBuf]) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::from([start.to_string()]);
    while let Some(theme) = queue.pop_front() {
        if !seen.insert(theme.clone()) {
            continue;
        }
        if let Some(index) = index_for(&theme, roots) {
            queue.extend(index.inherits);
        }
        chain.push(theme);
    }
    if seen.insert("hicolor".to_string()) {
        chain.push("hicolor".to_string());
    }
    chain
}

fn theme_chain(roots: &[PathBuf]) -> Vec<String> {
    let start = configured_theme()
        .or_else(xdg::settings_icon_theme)
        .unwrap_or_else(|| "hicolor".to_string());
    theme_chain_from(&start, roots)
}

/// The `[icon] theme` override, read without creating `config.toml` so an icon
/// lookup is never what writes the user's config.
fn configured_theme() -> Option<String> {
    if !crate::config::path()?.is_file() {
        return None;
    }
    let theme = crate::config::get().icon.theme;
    let theme = theme.trim();
    (!theme.is_empty()).then(|| theme.to_string())
}

/// A theme name resolved through the theme chain; the first hit wins. A theme is
/// spread across roots, so all of them are searched before the next theme.
fn find_theme_icon(name: &str) -> Option<String> {
    let roots = xdg::icon_theme_dirs();
    for theme in theme_chain(&roots) {
        for root in &roots {
            let dir = root.join(&theme);
            if !dir.is_dir() {
                continue;
            }
            let index = std::fs::read_to_string(dir.join("index.theme"))
                .ok()
                .map(|text| parse_index(&text));
            let hit = match &index {
                Some(index) if !index.dirs.is_empty() => find_in_theme(&dir, index, name),
                _ => scan_theme(&dir, name),
            };
            if hit.is_some() {
                return hit;
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
    fn a_symbolic_spec_resolves_under_the_symbolic_tree() {
        if !Path::new("/usr/share/icons/Papirus").exists() {
            return;
        }
        // the monochrome line-art variant, which the panel tints to the fg
        let path = find_icon_path("papirus:symbolic/apps/utilities-terminal-symbolic").unwrap();
        assert!(path.contains("/symbolic/"), "{path}");
        assert!(path.ends_with("utilities-terminal-symbolic.svg"), "{path}");
    }

    #[test]
    fn papirus_unknown_name_falls_back_to_default() {
        let path = find_icon_path("papirus:definitely-not-an-icon-xyz");
        assert!(path.is_some()); // default icon, same semantics as any miss
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

    #[test]
    fn a_stale_cached_glyph_is_replaced() {
        let dir = std::env::temp_dir().join(format!("wayrun-glyph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pin.svg"), b"stale").unwrap();

        let path = write_cached(&dir, "pin.svg", b"fresh").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_builtin_is_a_colourful_svg() {
        for (name, bytes) in BUILTIN_ICONS {
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(
                text.contains("<svg") && !text.contains("currentColor"),
                "{name} is not a colourful svg"
            );
        }
        assert!(builtin_icon("definitely-not-a-glyph").is_none());
    }

    #[test]
    fn the_notice_names_every_builtin() {
        let notice = include_str!("../../assets/icons/NOTICE");
        for (name, _) in BUILTIN_ICONS {
            assert!(
                notice
                    .lines()
                    .any(|line| line.split_whitespace().next() == Some(*name)),
                "NOTICE does not name the {name} glyph"
            );
        }
    }

    #[test]
    fn an_index_lists_its_directories_and_inherits() {
        let index = parse_index(
            "[Icon Theme]\n\
             Name=Test\n\
             Directories=16x16/apps,scalable/apps\n\
             Inherits=base,hicolor\n\
             \n\
             [16x16/apps]\nSize=16\nType=Fixed\n\
             \n\
             [scalable/apps]\nSize=128\nMinSize=8\nMaxSize=512\nType=Scalable\n",
        );
        assert_eq!(index.inherits, ["base", "hicolor"]);
        assert_eq!(index.dirs.len(), 2);
        assert_eq!((index.dirs[0].min, index.dirs[0].max), (16, 16));
        assert_eq!((index.dirs[1].min, index.dirs[1].max), (8, 512));
        // a directory without a section keeps an open range instead of dropping
        let open = parse_index("[Icon Theme]\nDirectories=orphan\n");
        assert_eq!((open.dirs[0].min, open.dirs[0].max), (0, u16::MAX));
    }

    #[test]
    fn the_chain_follows_inherits_and_ends_at_hicolor() {
        let root = std::env::temp_dir().join(format!("wayrun-chain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("child")).unwrap();
        std::fs::create_dir_all(root.join("base")).unwrap();
        std::fs::write(
            root.join("child/index.theme"),
            "[Icon Theme]\nDirectories=\nInherits=base\n",
        )
        .unwrap();
        std::fs::write(
            root.join("base/index.theme"),
            "[Icon Theme]\nDirectories=\n",
        )
        .unwrap();

        assert_eq!(
            theme_chain_from("child", std::slice::from_ref(&root)),
            ["child", "base", "hicolor"]
        );
        // an unknown theme still resolves to the hicolor fallback
        assert_eq!(
            theme_chain_from("nope", std::slice::from_ref(&root)),
            ["nope", "hicolor"]
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_theme_prefers_a_larger_directory_over_a_smaller_one() {
        let root = std::env::temp_dir().join(format!("wayrun-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let theme = root.join("T");
        std::fs::create_dir_all(theme.join("16x16/apps")).unwrap();
        std::fs::create_dir_all(theme.join("64x64/apps")).unwrap();
        std::fs::write(
            theme.join("index.theme"),
            "[Icon Theme]\nDirectories=16x16/apps,64x64/apps\n\n\
             [16x16/apps]\nSize=16\nType=Fixed\n\n\
             [64x64/apps]\nSize=64\nType=Fixed\n",
        )
        .unwrap();
        // only the too-small variant exists, so it is still the hit
        std::fs::write(theme.join("16x16/apps/foo.png"), "x").unwrap();
        std::fs::write(theme.join("64x64/apps/foo.svg"), "x").unwrap();

        let index = parse_index(&std::fs::read_to_string(theme.join("index.theme")).unwrap());
        let path = find_in_theme(&theme, &index, "foo").unwrap();
        // 48 is closer to 64 than to 16
        assert!(path.ends_with("64x64/apps/foo.svg"), "{path}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
