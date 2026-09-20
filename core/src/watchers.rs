use notify::RecommendedWatcher;
use tokio::sync::mpsc;

use crate::notify::watch;

/// Watch the DMS palette and re-emit a theme message to the UI on change, since
/// a resident core otherwise reads the theme once at start.
pub fn watch_theme(tx: &mpsc::Sender<String>) -> Option<RecommendedWatcher> {
    let path = crate::system::theme::dms_colors_path()?;

    // Seed the dedup with the theme emitted at startup, so an unchanged file
    // never re-emits.
    let mut last_sent = serde_json::to_string(&crate::protocol::theme_notification()).ok();
    let tx = tx.clone();

    watch(&path, move || {
        let Ok(json) = serde_json::to_string(&crate::protocol::theme_notification()) else {
            return;
        };
        // Dedup: skip unchanged themes (a single write can produce several
        // events; only the last value change should emit).
        if last_sent.as_deref() == Some(json.as_str()) {
            return;
        }
        last_sent = Some(json.clone());
        // try_send: a theme message is replaceable; drop it if the queue is
        // momentarily full rather than block.
        let _ = tx.try_send(json);
    })
}

/// Watch `config.toml` and reload behaviour; the registry is rebuilt too,
/// because a provider resolves its settings when it is built.
pub fn watch_config() -> Option<RecommendedWatcher> {
    // Touch the config so the template exists and is watched from the start.
    let _ = crate::config::get();
    let path = crate::config::path()?;
    let handle = tokio::runtime::Handle::current();

    watch(&path, move || {
        let handle = handle.clone();
        handle.spawn(async move {
            if crate::config::reload() {
                crate::plugin::reload().await;
            }
        });
    })
}

/// Watch `plugins.toml` and rebuild the registry, so resident mode picks up an
/// edit without a restart; an unchanged file is a no-op.
pub fn watch_plugins() -> Option<RecommendedWatcher> {
    let path = crate::system::fs::get_home()
        .ok()?
        .join(".config/wayrun/plugins.toml");
    let handle = tokio::runtime::Handle::current();

    watch(&path, move || {
        // notify's callback runs off the runtime; hop back in to await.
        let handle = handle.clone();
        handle.spawn(async move {
            crate::plugin::reload_if_changed().await;
        });
    })
}
