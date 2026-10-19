use crate::provider::external::HostMeta;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// A discovered host identity, keyed by `command` and stamped with the file's
/// `(mtime, size)`: a changed host is asked again, a stale `api` never read.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct HostCache {
    hosts: HashMap<String, CachedHost>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedHost {
    mtime: u64,
    size: u64,
    metas: Vec<HostMeta>,
}

impl HostCache {
    fn path() -> Option<PathBuf> {
        Some(crate::system::fs::cache_dir()?.join("plugin-hosts.json"))
    }

    pub(super) fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// The cached identities for `command`, only while the file it names is
    /// unchanged. `None` means the host must be asked.
    pub(super) fn fresh(&self, command: &str) -> Option<Vec<HostMeta>> {
        let (mtime, size) = crate::provider::external::command_stamp(command)?;
        let cached = self.hosts.get(command)?;
        (cached.mtime == mtime && cached.size == size).then(|| cached.metas.clone())
    }

    /// Whether a cached identity's file moved; an uncached host is not stale.
    pub(super) fn stale(&self, command: &str) -> bool {
        let Some(cached) = self.hosts.get(command) else {
            return false;
        };
        crate::provider::external::command_stamp(command)
            .is_none_or(|(mtime, size)| cached.mtime != mtime || cached.size != size)
    }

    pub(super) fn record(&mut self, command: &str, metas: &[HostMeta]) {
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

    pub(super) fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Ok(text) = serde_json::to_string(self) {
            let _ = crate::system::fs::write_atomic(&path, text.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_identity_cache_goes_stale_when_the_command_changes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cmd = dir.path().join("host");
        fs::write(&cmd, b"x").unwrap();
        fs::set_permissions(&cmd, fs::Permissions::from_mode(0o755)).unwrap();
        let command = cmd.display().to_string();

        let mut cache = HostCache::default();
        let metas = vec![HostMeta {
            id: "ext".into(),
            name: "Ext".into(),
            icon: String::new(),
            ready: String::new(),
            api: crate::wire::PLUGIN_API,
        }];
        cache.record(&command, &metas);
        assert!(cache.fresh(&command).is_some());

        // a changed file is a stale identity, so the host is asked again
        fs::write(&cmd, b"xy").unwrap();
        assert!(cache.fresh(&command).is_none());

        // a command that no longer exists is never fresh
        fs::remove_file(&cmd).unwrap();
        assert!(cache.fresh(&command).is_none());
    }

    #[test]
    fn a_cache_written_before_versioning_is_dropped_rather_than_read() {
        let old = r#"{"hosts":{"/bin/x":{"mtime":1,"size":1,
            "metas":[{"id":"ext","name":"Ext","icon":"","ready":""}]}}}"#;
        assert!(serde_json::from_str::<HostCache>(old).is_err());
    }
}
