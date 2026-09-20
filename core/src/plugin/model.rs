use serde::Deserialize;

use crate::provider::external::HostMeta;

#[derive(Deserialize, PartialEq)]
pub struct Config {
    pub plugins: Vec<PluginEntry>,
}

#[derive(Deserialize, PartialEq)]
pub struct PluginEntry {
    pub id: String,
    pub keyword: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// External JSON-RPC host (resolved on PATH). When set, the plugin is not
    /// compiled in: it is spawned per call and relays `search` to the host.
    #[serde(default)]
    pub command: Option<String>,
}

const fn default_enabled() -> bool {
    true
}

pub struct Meta {
    pub id: &'static str,
    pub name: &'static str,
    pub icon: &'static str,
    pub ready: &'static str,
}

#[derive(Clone)]
pub struct PendingHost {
    pub id: String,
    pub command: String,
}

pub(super) struct Entry {
    pub(super) plugin: Box<dyn super::Plugin>,
    pub(super) keyword: String,
    /// Set while an external plugin runs on its placeholder identity, so startup
    /// forks nothing; `resolve_pending` clears it on first use.
    pub(super) pending: Option<PendingHost>,
}

pub(super) type PluginMap = std::collections::HashMap<&'static str, Box<dyn super::Plugin>>;

/// A discovered host identity, keyed by command and stamped with the file's
/// `(mtime, size)`, so an unchanged host is never forked again.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct HostCache {
    hosts: std::collections::HashMap<String, CachedHost>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedHost {
    mtime: u64,
    size: u64,
    metas: Vec<HostMeta>,
}

impl HostCache {
    fn path() -> Option<std::path::PathBuf> {
        dirs::cache_dir().map(|dir| dir.join("wayrun/plugin-hosts.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// The cached identities for `command`, only while the file it names is
    /// unchanged. `None` means the host must be asked.
    pub fn fresh(&self, command: &str) -> Option<Vec<HostMeta>> {
        let (mtime, size) = crate::provider::external::command_stamp(command)?;
        let cached = self.hosts.get(command)?;
        (cached.mtime == mtime && cached.size == size).then(|| cached.metas.clone())
    }

    pub fn record(&mut self, command: &str, metas: &[HostMeta]) {
        let Some((mtime, size)) = crate::provider::external::command_stamp(command) else {
            return;
        };
        self.hosts.insert(
            command.to_string(),
            CachedHost {
                mtime,
                size,
                metas: metas.to_vec(),
            },
        );
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::write(path, text);
        }
    }
}
