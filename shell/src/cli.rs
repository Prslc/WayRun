use clap::{Parser, Subcommand};

/// The launcher, its backend and its plugin tools.
#[derive(Parser)]
#[command(name = "wayrun", version, about)]
pub struct Cli {
    /// Run the core's stdin JSON-RPC loop; the shell re-execs itself with this.
    #[arg(long, hide = true)]
    pub core: bool,
    /// Serve the plugin contract for one Lua script.
    #[arg(long, value_name = "SCRIPT", hide = true)]
    pub lua_host: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Show the launcher.
    Open,
    /// Hide the launcher.
    Close,
    /// Show or hide the launcher.
    Toggle,
    /// Report whether the launcher is shown.
    Status,
    /// Run the rasteriser micro-benchmark; no Wayland.
    Bench,
    /// Read and write config.toml.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Inspect plugins and their registration.
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print every effective setting.
    List,
    /// Print one setting.
    Get {
        /// A dotted key, e.g. ui.locale.
        key: String,
    },
    /// Set one key.
    Set {
        /// A dotted key, e.g. files.index.
        key: String,
        /// A TOML value; a bare string works for text keys.
        value: String,
    },
    /// Drop a key, so its default applies again.
    Unset {
        /// A dotted key, e.g. icon.theme.
        key: String,
    },
}

#[derive(Subcommand)]
pub enum PluginAction {
    /// Print the plugin registry.
    List,
    /// Load a Lua script as the host does and report what keeps it from working.
    Check {
        /// The script to check.
        script: String,
    },
    /// Register a Lua host in plugins.toml.
    Add {
        /// The script to register.
        script: String,
        /// The routing keyword; defaults to the plugin id.
        #[arg(long)]
        keyword: Option<String>,
        /// Keep the host process alive across calls.
        #[arg(long)]
        resident: bool,
    },
    /// Enable a plugin entry.
    Enable {
        /// The plugin id.
        id: String,
    },
    /// Disable a plugin entry.
    Disable {
        /// The plugin id.
        id: String,
    },
    /// Drop the user entry for a plugin.
    Remove {
        /// The plugin id.
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn the_plugin_shebang_parses() {
        let cli = parse(&["wayrun", "--lua-host", "/home/me/plugin.lua"]);
        assert_eq!(cli.lua_host.as_deref(), Some("/home/me/plugin.lua"));
    }

    #[test]
    fn a_bare_invocation_runs_the_shell() {
        let cli = parse(&["wayrun"]);
        assert!(cli.command.is_none());
        assert!(!cli.core && cli.lua_host.is_none());
    }

    #[test]
    fn the_backend_flags_parse() {
        assert!(parse(&["wayrun", "--core"]).core);
    }

    #[test]
    fn the_ipc_verbs_parse() {
        assert!(matches!(
            parse(&["wayrun", "open"]).command,
            Some(Command::Open)
        ));
        assert!(matches!(
            parse(&["wayrun", "close"]).command,
            Some(Command::Close)
        ));
        assert!(matches!(
            parse(&["wayrun", "toggle"]).command,
            Some(Command::Toggle)
        ));
        assert!(matches!(
            parse(&["wayrun", "status"]).command,
            Some(Command::Status)
        ));
    }

    #[test]
    fn the_config_commands_parse() {
        assert!(matches!(
            parse(&["wayrun", "config", "list"]).command,
            Some(Command::Config {
                action: ConfigAction::List
            })
        ));
        match parse(&["wayrun", "config", "get", "ui.locale"]).command {
            Some(Command::Config {
                action: ConfigAction::Get { key },
            }) => assert_eq!(key, "ui.locale"),
            _ => panic!("config get did not parse"),
        }
        match parse(&["wayrun", "config", "set", "files.index", "false"]).command {
            Some(Command::Config {
                action: ConfigAction::Set { key, value },
            }) => {
                assert_eq!(key, "files.index");
                assert_eq!(value, "false");
            }
            _ => panic!("config set did not parse"),
        }
        match parse(&["wayrun", "config", "unset", "icon.theme"]).command {
            Some(Command::Config {
                action: ConfigAction::Unset { key },
            }) => assert_eq!(key, "icon.theme"),
            _ => panic!("config unset did not parse"),
        }
    }

    #[test]
    fn the_plugin_commands_parse() {
        assert!(matches!(
            parse(&["wayrun", "plugin", "list"]).command,
            Some(Command::Plugin {
                action: PluginAction::List
            })
        ));
        match parse(&["wayrun", "plugin", "check", "/tmp/a.lua"]).command {
            Some(Command::Plugin {
                action: PluginAction::Check { script },
            }) => assert_eq!(script, "/tmp/a.lua"),
            _ => panic!("plugin check did not parse"),
        }
        match parse(&[
            "wayrun",
            "plugin",
            "add",
            "/tmp/a.lua",
            "--keyword",
            "a",
            "--resident",
        ])
        .command
        {
            Some(Command::Plugin {
                action:
                    PluginAction::Add {
                        script,
                        keyword,
                        resident,
                    },
            }) => {
                assert_eq!(script, "/tmp/a.lua");
                assert_eq!(keyword.as_deref(), Some("a"));
                assert!(resident);
            }
            _ => panic!("plugin add did not parse"),
        }
        for verb in ["enable", "disable", "remove"] {
            match parse(&["wayrun", "plugin", verb, "note"]).command {
                Some(Command::Plugin { action }) => {
                    let id = match action {
                        PluginAction::Enable { id }
                        | PluginAction::Disable { id }
                        | PluginAction::Remove { id } => id,
                        _ => panic!("{verb} parsed as another action"),
                    };
                    assert_eq!(id, "note");
                }
                _ => panic!("plugin {verb} did not parse"),
            }
        }
    }
}
