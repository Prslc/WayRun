use std::env;
use std::path::{Path, PathBuf};

/// `$XDG_DATA_HOME`, or `~/.local/share` when unset.
pub fn data_home() -> Option<PathBuf> {
    dirs::data_dir()
}

/// `$XDG_DATA_DIRS` split on `:`, or the spec default when unset or empty.
/// `fs::ensure_flatpak_data_dirs` has already padded it with the flatpak exports.
pub fn data_dirs() -> Vec<PathBuf> {
    data_dir_list(&env::var("XDG_DATA_DIRS").unwrap_or_default())
}

fn data_dir_list(text: &str) -> Vec<PathBuf> {
    let dirs: Vec<PathBuf> = text
        .split(':')
        .filter(|dir| !dir.is_empty() && Path::new(dir).is_absolute())
        .map(PathBuf::from)
        .collect();
    if dirs.is_empty() {
        vec![
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]
    } else {
        dirs
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

/// The `mimeapps.list` overrides that add or pin a content type's handlers; the
/// copies under an applications directory come with [`desktop_dirs`].
pub fn mimeapps_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(config) = dirs::config_dir() {
        push_unique(&mut files, config.join("mimeapps.list"));
    }
    for config in config_dirs() {
        push_unique(&mut files, config.join("mimeapps.list"));
    }
    files
}

/// Theme roots a named icon may live under, user first.
pub fn icon_theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = data_home() {
        push_unique(&mut dirs, home.join("icons"));
    }
    // The legacy user location, ahead of the system roots.
    if let Ok(home) = crate::system::fs::get_home() {
        push_unique(&mut dirs, home.join(".icons"));
    }
    for root in data_dirs() {
        push_unique(&mut dirs, root.join("icons"));
    }
    dirs
}

/// Flat raster icons live in the icon roots themselves, plus `/usr/share/pixmaps`.
pub fn pixmap_dirs() -> Vec<PathBuf> {
    let mut dirs = icon_theme_dirs();
    push_unique(&mut dirs, PathBuf::from("/usr/share/pixmaps"));
    dirs
}

/// `$XDG_CONFIG_DIRS` (absolute entries only), or `/etc/xdg`.
pub fn config_dirs() -> Vec<PathBuf> {
    let dirs: Vec<PathBuf> = env::var("XDG_CONFIG_DIRS")
        .unwrap_or_default()
        .split(':')
        .filter(|dir| !dir.is_empty() && Path::new(dir).is_absolute())
        .map(PathBuf::from)
        .collect();
    if dirs.is_empty() {
        vec![PathBuf::from("/etc/xdg")]
    } else {
        dirs
    }
}

/// The GTK settings files that can name the icon theme, user first.
pub fn icon_settings_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut push = |base: &Path| {
        for sub in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
            push_unique(&mut files, base.join(sub));
        }
    };
    if let Some(config) = dirs::config_dir() {
        push(&config);
    }
    for config in config_dirs() {
        push(&config);
    }
    files
}

/// The icon theme the desktop settings name (`gtk-icon-theme-name`), which GTK
/// and matugen write; the newest settings file that names one wins.
pub fn settings_icon_theme() -> Option<String> {
    icon_settings_files()
        .iter()
        .find_map(|path| icon_theme_name(&std::fs::read_to_string(path).ok()?))
}

fn icon_theme_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "gtk-icon-theme-name")
            .then(|| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
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
    fn an_unset_or_empty_data_dirs_list_uses_the_spec_default() {
        let defaults = || {
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
        };
        assert_eq!(data_dir_list(""), defaults());
        assert_eq!(data_dir_list("::"), defaults());
        assert_eq!(
            data_dir_list("/a::/b"),
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
        assert_eq!(data_dir_list("relative"), defaults());
        assert_eq!(
            data_dir_list("/a:relative:/b"),
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn flat_pixmap_lookup_includes_the_icon_roots() {
        let pixmaps = pixmap_dirs();
        for root in icon_theme_dirs() {
            assert!(pixmaps.contains(&root), "{root:?}");
        }
        assert!(pixmaps.contains(&PathBuf::from("/usr/share/pixmaps")));
    }

    #[test]
    fn icon_theme_dirs_lead_with_the_user_data_home() {
        let dirs = icon_theme_dirs();
        if let Some(home) = data_home() {
            assert_eq!(dirs.first(), Some(&home.join("icons")));
        }
        if let Ok(home) = crate::system::fs::get_home() {
            let legacy = home.join(".icons");
            let at = dirs
                .iter()
                .position(|dir| *dir == legacy)
                .expect("legacy root");
            let system = PathBuf::from("/usr/share/icons");
            if let Some(system_at) = dirs.iter().position(|dir| *dir == system) {
                assert!(
                    at < system_at,
                    "~/.icons precedes the system roots: {dirs:?}"
                );
            }
        }
        assert!(!dirs.is_empty());
    }
}
