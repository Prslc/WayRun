mod theme;

use std::path::PathBuf;
use std::sync::OnceLock;

use wayrun_core::{watch as watch_file, write_if_absent};

pub use self::theme::{AppearanceConfig, ColorOverrides, Mode};

const DEFAULT_TEMPLATE: &str = include_str!("../default-theme.toml");

/// `~/.config/wayrun/theme.toml`. A missing file (or any missing key) keeps the
/// default, so an absent config is the shipped look.
fn theme_path() -> Option<PathBuf> {
    Some(wayrun_core::config::dir()?.join("theme.toml"))
}

/// Write the shipped template once, on first use: a watcher reload must not
/// recreate the file an editor's atomic save briefly removed.
fn ensure_template() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        if let Some(path) = theme_path() {
            let _ = write_if_absent(&path, DEFAULT_TEMPLATE);
        }
    });
}

/// Watch `theme.toml`; a failed read keeps the applied config and an unchanged
/// value is dropped, so a save settles to one update with no debounce.
pub fn watch(tx: calloop::channel::Sender<AppearanceConfig>) -> Option<notify::RecommendedWatcher> {
    let path = theme_path()?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut last = AppearanceConfig::load();
    watch_file(&path, move || {
        let Some(config) = AppearanceConfig::load_checked() else {
            return;
        };
        if config == last {
            return;
        }
        last = config.clone();
        let _ = tx.send(config);
    })
}
