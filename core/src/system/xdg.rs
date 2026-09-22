use std::env;
use std::path::PathBuf;

/// `$XDG_DATA_HOME`, or `~/.local/share` when unset.
pub fn data_home() -> Option<PathBuf> {
    dirs::data_dir()
}

/// `$XDG_DATA_DIRS` split on `:`, or the spec default. `fs::ensure_flatpak_data_dirs`
/// has already padded it with the flatpak exports before any lookup runs.
pub fn data_dirs() -> Vec<PathBuf> {
    match env::var("XDG_DATA_DIRS") {
        Ok(list) => list
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect(),
        Err(_) => vec![
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ],
    }
}

fn push_unique(dirs: &mut Vec<PathBuf>, path: PathBuf) {
    if !dirs.contains(&path) {
        dirs.push(path);
    }
}

/// Every base to search for `subdir`, the user's data home first so a local file
/// overrides a packaged one.
fn data_subdirs(subdir: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = data_home() {
        push_unique(&mut dirs, home.join(subdir));
    }
    for root in data_dirs() {
        push_unique(&mut dirs, root.join(subdir));
    }
    dirs
}

/// `.desktop` roots in precedence order.
pub fn desktop_dirs() -> Vec<PathBuf> {
    data_subdirs("applications")
}

/// Theme roots a named icon may live under, user first.
pub fn icon_theme_dirs() -> Vec<PathBuf> {
    let mut dirs = data_subdirs("icons");
    // `~/.icons` is the Icon Theme spec's legacy user location.
    if let Ok(home) = crate::system::fs::get_home() {
        push_unique(&mut dirs, home.join(".icons"));
    }
    dirs
}

/// Flat raster icons drop here outside any theme.
pub fn pixmap_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/share/pixmaps")]
}

/// The icon theme the desktop settings name (`gtk-icon-theme-name`), which GTK
/// and matugen write; the newest settings file that names one wins.
pub fn settings_icon_theme() -> Option<String> {
    let config = dirs::config_dir()?;
    ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"]
        .iter()
        .find_map(|sub| icon_theme_name(&std::fs::read_to_string(config.join(sub)).ok()?))
}

fn icon_theme_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "gtk-icon-theme-name")
            .then(|| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn find_up_from_bin(sub_path: &str) -> Option<String> {
    let mut dir = env::current_exe().ok()?.parent()?.to_path_buf();
    loop {
        let candidate = dir.join(sub_path);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().into_owned());
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn find_in_xdg_data(sub_path: &str) -> Option<String> {
    for dir in data_subdirs("wayrun") {
        let candidate = dir.join(sub_path);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// A file shipped with the project (plugin identity icons, the default icon),
/// found next to the executable, under an XDG data dir, or in the dev tree.
pub fn resource_path(sub_path: &str) -> Option<String> {
    if let Ok(dir) = env::var("WAYRUN_RESOURCE_DIR") {
        let p = PathBuf::from(dir).join(sub_path);
        if p.exists() {
            return Some(p.to_string_lossy().into_owned());
        }
    }

    if let Some(p) = find_up_from_bin(sub_path) {
        return Some(p);
    }

    if let Some(p) = find_in_xdg_data(sub_path) {
        return Some(p);
    }

    // relative to the Cargo workspace root, debug builds only
    #[cfg(debug_assertions)]
    {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join(sub_path);
        if dev.exists() {
            return Some(dev.to_string_lossy().into_owned());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_data_home_comes_first_and_is_never_repeated() {
        let dirs = desktop_dirs();
        if let Some(home) = data_home() {
            assert_eq!(dirs.first(), Some(&home.join("applications")));
            assert_eq!(
                dirs.iter()
                    .filter(|d| **d == home.join("applications"))
                    .count(),
                1
            );
        }
        assert!(!dirs.is_empty(), "the spec default joins in when unset");
    }

    #[test]
    fn icon_theme_dirs_lead_with_the_user_data_home() {
        let dirs = icon_theme_dirs();
        if let Some(home) = data_home() {
            assert_eq!(dirs.first(), Some(&home.join("icons")));
        }
        assert!(!dirs.is_empty());
    }

    #[test]
    fn a_bundled_resource_is_found_up_from_the_binary() {
        // The test binary lives under the workspace's target dir, so the
        // `images/` tree at the root is reachable by walking up from it.
        let p = resource_path("images/logo.svg");
        assert!(p.is_some_and(|p| p.ends_with("images/logo.svg")));
    }
}
