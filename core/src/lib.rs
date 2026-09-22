use anyhow::Result;

// The launcher's strings, shared with the shell: both crates read the same
// tables, so one locale covers the core's rows and the shell's own chrome.
rust_i18n::i18n!("../locales", fallback = "en");

pub mod config;
pub mod i18n;
mod notify;
mod plugin;
mod protocol;
mod provider;
mod rpc;
mod system;
mod watchers;
pub mod wire;

/// Call back on every write to one file (and its parent dir); shared by the
/// core's watchers and the shell's `theme.toml` watcher.
pub use notify::watch;

/// Create a file only when it is absent; shared by the template writers so a
/// watcher reload cannot truncate an editor's save.
pub use system::fs::write_if_absent;

/// `--list-plugins` prints the registry and exits; otherwise serve the protocol.
async fn serve_or_list() -> Result<()> {
    if std::env::args().any(|a| a == "--list-plugins") {
        plugin::print_list().await;
        return Ok(());
    }

    protocol::serve().await
}

/// Run the core on stdin/stdout. `wayrun --core` (or invoking the binary as
/// `wayrun-core`) calls this and blocks for the process's life.
pub fn run() -> Result<()> {
    i18n::init();
    // Before the runtime exists: the env write must be single-threaded.
    system::fs::ensure_flatpak_data_dirs();
    system::icon::warn_if_no_icon_theme();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve_or_list())
}
