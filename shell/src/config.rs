use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use wayrun_core::{watch as watch_file, write_if_absent};
use wayrun_ui::config::AppearanceConfig;

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

/// The applied `theme.toml`, or the shipped defaults when the file is absent
/// or unparseable.
pub fn load() -> AppearanceConfig {
    load_checked().unwrap_or_default()
}

/// The file's config, or `None` when it is absent or unparseable; a reload
/// keeps the applied config instead of resetting to defaults.
fn load_checked() -> Option<AppearanceConfig> {
    ensure_template();
    let path = theme_path()?;
    let text = fs::read_to_string(path).ok()?;
    AppearanceConfig::parse(&text)
}

/// Watch `theme.toml`; a failed read keeps the applied config and an unchanged
/// value is dropped, so a save settles to one update with no debounce.
pub fn watch(tx: calloop::channel::Sender<AppearanceConfig>) -> Option<notify::RecommendedWatcher> {
    let path = theme_path()?;
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let mut last = load();
    watch_file(&path, move || {
        let Some(config) = load_checked() else {
            return;
        };
        if config == last {
            return;
        }
        last = config.clone();
        let _ = tx.send(config);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_template_comments_out_every_key() {
        for line in DEFAULT_TEMPLATE.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            assert!(line.starts_with('['), "uncommented key: {line}");
        }

        // commented keys are absent, so the whole file is the built-in defaults
        assert_eq!(
            AppearanceConfig::parse(DEFAULT_TEMPLATE).unwrap(),
            AppearanceConfig::default()
        );
    }
}
