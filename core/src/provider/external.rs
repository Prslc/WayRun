use std::env;
use std::fs;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;
use std::time::UNIX_EPOCH;

use anyhow::Result;

use self::call::{host_call, next_id, query_default, query_external, query_external_streaming};
use crate::plugin::{Emit, Meta, Plugin};
use crate::system::icon::host_icon_spec;
use crate::wire::ResultItem;
use rust_i18n::t;

mod call;
mod parse;

/// Identity of one plugin as described by an external host's `list_plugins`.
/// The host owns its own name/icon/ready hint; the core just relays them.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct HostMeta {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub ready: String,
    pub api: u32,
}

/// The executable behind `command`, when it resolves on `PATH` (or is a path).
pub fn command_path(command: &str) -> Option<PathBuf> {
    resolve_command(command)
}

/// `(mtime, size, mode)` of the resolved `command`, to validate a cached identity.
/// `None` when it cannot be resolved or stat'd, so the cache is not trusted.
pub fn command_stamp(command: &str) -> Option<(u64, u64, u32)> {
    let path = resolve_command(command)?;
    let meta = fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((mtime, meta.len(), meta.permissions().mode()))
}

/// A plugin backed by an external JSON-RPC host declared via `command`. The core
/// is a generic client: it spawns the host and relays `search` to it.
pub struct External {
    meta: Meta,
    command: String,
    resident: bool,
}

impl External {
    /// Build from the configured id, host command and discovered identity. A
    /// missing identity degrades to the id as display name.
    pub fn new(id: &str, command: String, discovered: Option<HostMeta>, resident: bool) -> Self {
        let (name, icon, ready) = match discovered {
            Some(m) => {
                if m.api != crate::wire::PLUGIN_API {
                    eprintln!(
                        "wayrun-core: plugin {id}: api {}, this launcher speaks {}",
                        m.api,
                        crate::wire::PLUGIN_API
                    );
                }
                (m.name, m.icon, m.ready)
            }
            None => (
                id.to_string(),
                String::new(),
                t!("plugin.external.ready", command = command),
            ),
        };
        // An unrenderable identity icon is a miss: a host names a file it ships
        // or a compiled glyph, nothing else.
        let icon = host_icon_spec(&icon).unwrap_or_default();
        Self {
            meta: Meta {
                id: id.to_string().into(),
                name,
                icon: icon.into(),
                ready,
            },
            command,
            resident,
        }
    }
}

impl Plugin for External {
    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn search(
        &self,
        query: &str,
        _full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        let command = self.command.clone();
        let plugin = self.meta.id.to_string();
        let query = query.to_string();
        let icon = self.meta.icon.to_string();
        let resident = self.resident;
        Box::pin(async move { query_external(&command, resident, &plugin, &query, &icon).await })
    }

    fn search_streaming<'a>(
        &'a self,
        query: &'a str,
        full: &'a str,
        emit: Option<Emit>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + 'a>> {
        if emit.is_none() {
            return self.search(query, full);
        }
        let command = self.command.clone();
        let plugin = self.meta.id.to_string();
        let query = query.to_string();
        let icon = self.meta.icon.to_string();
        let resident = self.resident;
        Box::pin(async move {
            query_external_streaming(&command, resident, &plugin, &query, &icon, emit).await
        })
    }

    fn default_view(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<ResultItem>>>> + Send + '_>> {
        let command = self.command.clone();
        let plugin = self.meta.id.to_string();
        let icon = self.meta.icon.to_string();
        let resident = self.resident;
        Box::pin(async move { query_default(&command, resident, &plugin, &icon).await })
    }

    fn command<'a>(
        &'a self,
        action: &'a str,
        payload: Option<&'a serde_json::Value>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        let command = self.command.clone();
        let plugin = self.meta.id.to_string();
        let action = action.to_string();
        let payload = payload.cloned();
        let resident = self.resident;
        Box::pin(async move {
            call::query_command(&command, resident, &plugin, &action, payload.as_ref()).await
        })
    }
}

/// Ceiling for one host call: a stalled host must cost seconds, never the
/// session. Discovery holds the registry's init lock until it returns.
pub(crate) const HOST_TIMEOUT: Duration = Duration::from_secs(5);

/// Ask the host who it serves. Empty when the host is missing, stalled or not
/// self-describing, so a query for it falls through instead of hanging.
pub async fn discover(command: &str, resident: bool) -> Vec<HostMeta> {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "list_plugins",
        "id": next_id(),
    });
    let Some(response) = host_call(command, resident, &request, HOST_TIMEOUT).await else {
        eprintln!(
            "wayrun-core: external host {command} did not answer list_plugins; its plugins stay unregistered"
        );
        return Vec::new();
    };
    let Some(list) = response.get("result").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| {
            Some(HostMeta {
                id: entry.get("id")?.as_str()?.to_string(),
                name: entry
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                icon: entry
                    .get("icon")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                ready: entry
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                api: entry.get("api").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            })
        })
        .collect()
}

/// Resolve a plugins.toml `command` the way execvp will: a name with a slash is
/// a path; a bare name walks `$PATH`, skipping empty segments and non-executables.
fn resolve_command(command: &str) -> Option<PathBuf> {
    crate::system::fs::which_in(env::var_os("PATH").unwrap_or_default(), command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_resolves_to_an_existing_file() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("host.sh");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(command_path(script.to_str().unwrap()), Some(script.clone()));
        assert_eq!(
            command_path(temp.path().join("missing.sh").to_str().unwrap()),
            None
        );
    }
}
