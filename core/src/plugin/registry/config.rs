use serde::Deserialize;

#[derive(Deserialize, PartialEq)]
pub(super) struct Config {
    pub plugins: Vec<PluginEntry>,
}

#[derive(Deserialize, PartialEq)]
pub(super) struct PluginEntry {
    pub id: String,
    pub keyword: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// External JSON-RPC 2.0 host (resolved on PATH): not compiled in, a fresh
    /// process answers its `search` per call, or a kept one when `resident`.
    #[serde(default)]
    pub command: Option<String>,
    /// Keep the host process alive across calls; dropped on dismissal and
    /// after two idle minutes.
    #[serde(default)]
    pub resident: bool,
}

const fn default_enabled() -> bool {
    true
}

const DEFAULT_CONFIG: &str = include_str!("../../../default-plugins.toml");

pub(super) fn load_or_default() -> Config {
    let mut config: Config = toml::from_str(DEFAULT_CONFIG).expect("invalid default config");

    if let Some(dir) = crate::config::dir() {
        let path = dir.join("plugins.toml");

        ensure_default_config(&path);

        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(user) = toml::from_str::<Config>(&content)
        {
            config = merge_config(config, user);
        }
    }

    config
}

/// The shipped `plugins.toml`, written once; a watcher reload must not recreate
/// the file an editor just replaced.
fn ensure_default_config(path: &std::path::Path) {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = crate::write_if_absent(path, DEFAULT_CONFIG);
    });
}

/// Overlay a user config on the shipped default: known ids take the user's
/// keyword/enabled and any host field it set; unknown ids are appended as-is.
fn merge_config(mut base: Config, user: Config) -> Config {
    for up in user.plugins {
        match base.plugins.iter_mut().find(|p| p.id == up.id) {
            Some(dp) => {
                dp.keyword = up.keyword;
                dp.enabled = up.enabled;
                if up.command.is_some() {
                    dp.command = up.command;
                }
            }
            None => base.plugins.push(up),
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Config {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn user_overrides_known_ids() {
        let base = parse(
            r#"
            [[plugins]]
            id = "calculator"
            keyword = ""
            "#,
        );
        let user = parse(
            r#"
            [[plugins]]
            id = "calculator"
            keyword = "calc"
            enabled = false
            "#,
        );
        let merged = merge_config(base, user);
        assert_eq!(merged.plugins.len(), 1);
        assert_eq!(merged.plugins[0].keyword, "calc");
        assert!(!merged.plugins[0].enabled);
        assert!(merged.plugins[0].command.is_none());
    }

    #[test]
    fn user_appends_new_external_ids() {
        let base = parse(
            r#"
            [[plugins]]
            id = "calculator"
            keyword = ""
            "#,
        );
        let user = parse(
            r#"
            [[plugins]]
            id = "translate"
            keyword = "tr"
            command = "ext-host"
            "#,
        );
        let merged = merge_config(base, user);
        assert_eq!(merged.plugins.len(), 2);
        let external = &merged.plugins[1];
        assert_eq!(external.id, "translate");
        assert_eq!(external.keyword, "tr");
        assert_eq!(external.command.as_deref(), Some("ext-host"));
    }

    #[test]
    fn user_command_overrides_default_command() {
        let base = parse(
            r#"
            [[plugins]]
            id = "github"
            keyword = "g"
            command = "old-host"
            "#,
        );
        let user = parse(
            r#"
            [[plugins]]
            id = "github"
            keyword = "g"
            command = "ext-host"
            "#,
        );
        let merged = merge_config(base, user);
        assert_eq!(merged.plugins[0].command.as_deref(), Some("ext-host"));
        assert_eq!(merged.plugins[0].keyword, "g");
    }

    #[test]
    fn user_omitting_command_keeps_default_command() {
        let base = parse(
            r#"
            [[plugins]]
            id = "github"
            keyword = "g"
            command = "ext-host"
            "#,
        );
        let user = parse(
            r#"
            [[plugins]]
            id = "github"
            keyword = "gh"
            "#,
        );
        let merged = merge_config(base, user);
        assert_eq!(merged.plugins[0].command.as_deref(), Some("ext-host"));
        assert_eq!(merged.plugins[0].keyword, "gh");
    }
}
