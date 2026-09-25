use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::system::xdg;

/// The row box is 22-30px, so a 48px source gives crisp downscale headroom.
const TARGET_SIZE: u16 = 48;
pub(super) const ICON_EXTS: &[&str] = &["svg", "png"];

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

/// The theme lookup order with the `index.theme` each directory resolved to, so
/// walking it reads and parses nothing again.
type Chain = Vec<(PathBuf, Option<ThemeIndex>)>;

/// Chains by start theme. The parsed indexes are held for the process the way
/// `resolve`'s cache is: the theme is fixed, and a `config.toml` or settings edit
/// lands under a new key.
static CHAINS: OnceLock<Mutex<HashMap<String, Arc<Chain>>>> = OnceLock::new();

fn chain_for(start: &str) -> Arc<Chain> {
    let chains = CHAINS.get_or_init(|| Mutex::new(HashMap::default()));
    let mut chains = chains.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(chain) = chains.get(start) {
        return Arc::clone(chain);
    }

    let roots = xdg::icon_theme_dirs();
    let built: Chain = theme_chain_from(start, &roots)
        .into_iter()
        .flat_map(|theme| {
            // a theme is spread across roots; a root without it has nothing to say
            roots.iter().filter_map(move |root| {
                let dir = root.join(&theme);
                dir.is_dir().then(|| {
                    let index = std::fs::read_to_string(dir.join("index.theme"))
                        .ok()
                        .map(|text| parse_index(&text));
                    (dir, index)
                })
            })
        })
        .collect();

    let chain = Arc::new(built);
    chains.insert(start.to_string(), Arc::clone(&chain));
    chain
}

/// The theme a lookup starts from: the config's, else the desktop's, else the
/// spec's fallback.
fn start_theme() -> String {
    configured_theme()
        .or_else(xdg::settings_icon_theme)
        .unwrap_or_else(|| "hicolor".to_string())
}

/// The `[icon] theme` override, read without creating `config.toml` so an icon
/// lookup is never what writes the user's config.
fn configured_theme() -> Option<String> {
    if !crate::config::path()?.is_file() {
        return None;
    }
    let config = crate::config::get();
    let theme = config.icon.theme.trim();
    (!theme.is_empty()).then(|| theme.to_string())
}

/// A theme name resolved through the cached chain; the first hit wins. A theme is
/// spread across roots, so all of them are searched before the next theme.
pub(super) fn find_theme_icon(name: &str) -> Option<String> {
    for (dir, index) in chain_for(&start_theme()).iter() {
        let hit = match index {
            Some(index) if !index.dirs.is_empty() => find_in_theme(dir, index, name),
            _ => scan_theme(dir, name),
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

/// Flat raster icons in `/usr/share/pixmaps`, outside any theme.
pub(super) fn find_pixmap_icon(name: &str) -> Option<String> {
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

pub fn warn_if_no_icon_theme() {
    let found = chain_for(&start_theme())
        .iter()
        .any(|(_, index)| index.is_some());
    if !found {
        eprintln!("wayrun: no icon theme found; row and action icons use the built-in placeholder");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
