use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

mod model;
pub use model::{Config, Font, Icon, WebSearch};

const DEFAULT_TEMPLATE: &str = include_str!("../default-config.toml");

/// `~/.config/wayrun`, the one config directory the shell and core share.
pub fn dir() -> Option<PathBuf> {
    Some(crate::system::fs::get_home().ok()?.join(".config/wayrun"))
}

pub fn path() -> Option<PathBuf> {
    Some(dir()?.join("config.toml"))
}

/// Write the shipped template on first use so the settings are discoverable.
/// Absence is already the default, so a write failure changes nothing.
fn write_template() {
    let Some(path) = path() else {
        return;
    };
    let _ = crate::write_if_absent(&path, DEFAULT_TEMPLATE);
}

static CONFIG: OnceLock<RwLock<Config>> = OnceLock::new();

fn cell() -> &'static RwLock<Config> {
    CONFIG.get_or_init(|| {
        write_template();
        RwLock::new(Config::load())
    })
}

pub fn get() -> Config {
    cell().read().expect("config lock poisoned").clone()
}

/// Reload `config.toml`, returning whether the value changed; a watcher rebuilds
/// the plugin registry (how a provider sees a setting) only when it did.
pub fn reload() -> bool {
    let Some(new) = Config::load_checked() else {
        return false;
    };
    let mut guard = cell().write().expect("config lock poisoned");
    if *guard == new {
        return false;
    }
    *guard = new;
    true
}

pub fn web_search_engine() -> String {
    cell()
        .read()
        .expect("config lock poisoned")
        .web_search
        .engine
        .clone()
}
