use anyhow::Result;

// The launcher's strings, shared with the shell: both crates read the same
// tables, so one locale covers the core's rows and the shell's own chrome.
rust_i18n::i18n!("../locales", fallback = "en");

pub mod config;
mod live;
pub mod locale;
pub mod lua;
mod notify;
mod plugin;
mod protocol;
mod provider;
mod rpc;
mod session;
mod system;
mod watchers;
pub mod wire;

/// Call back on every write to one file (and its parent dir); shared by the
/// core's watchers and the shell's `theme.toml` watcher.
pub use notify::watch;

/// Create a file only when it is absent; shared by the template writers so a
/// watcher reload cannot truncate an editor's save.
pub use system::fs::write_if_absent;

/// Return the allocator's free pages to the kernel; shared by the shell's
/// dismissal and the index reaper, both of which drop a large buffer.
pub use system::fs::trim_allocator;

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

fn prepare() {
    locale::init();
    // Before the runtime exists: the env write must be single-threaded.
    system::fs::ensure_flatpak_data_dirs();
    system::icon::warn_if_no_icon_theme();
}

/// Run the core on stdin/stdout. `wayrun --core` (or invoking the binary as
/// `wayrun-core`) calls this and blocks for the process's life.
pub fn run() -> Result<()> {
    prepare();
    runtime()?.block_on(session::serve())
}

/// Print the plugin registry for `wayrun plugin list`.
pub fn print_plugins() -> Result<()> {
    prepare();
    runtime()?.block_on(plugin::print_list())?;
    Ok(())
}
