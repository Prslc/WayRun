use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use gio::prelude::{AppInfoExt, IconExt};

use crate::live::{Live, Source};
use crate::system::xdg;

/// One installed application that can open a content type.
pub struct Handler {
    pub desktop_id: String,
    pub title: String,
    /// GIO's serialized icon spec; resolved by the theme-keyed icon cache.
    pub icon_spec: Option<String>,
}

/// Beyond the usual handful, a type's handler list is noise in a flat panel.
const MAX_HANDLERS: usize = 8;

fn handler_roots() -> Vec<PathBuf> {
    let mut roots = xdg::desktop_dirs();
    roots.extend(xdg::mimeapps_files());
    roots
}

static SOURCE: LazyLock<Arc<Source>> = LazyLock::new(|| Arc::new(Source::new(handler_roots)));

static CACHE: LazyLock<Live<HashMap<String, Arc<[Handler]>>>> =
    LazyLock::new(|| Live::shared(Arc::clone(&SOURCE)).bounded(256));

/// The alternative handlers for `content_type`, in the system's order; the
/// default handler is left out, since the row's own `open` already runs it.
pub fn for_content_type(content_type: &str) -> Arc<[Handler]> {
    CACHE.memoized(content_type, || lookup(content_type))
}

fn lookup(content_type: &str) -> Arc<[Handler]> {
    let default = gio::AppInfo::default_for_type(content_type, false)
        .and_then(|app| app.id().map(|id| id.to_string()));
    // A recommendation can be empty or hold only the default, so `all` is the net.
    let mut handlers = handlers_from(
        gio::AppInfo::recommended_for_type(content_type),
        default.as_deref(),
    );
    if handlers.is_empty() {
        handlers = handlers_from(gio::AppInfo::all_for_type(content_type), default.as_deref());
    }
    Arc::from(handlers)
}

fn handlers_from(apps: Vec<gio::AppInfo>, default: Option<&str>) -> Vec<Handler> {
    apps.into_iter()
        .filter(|app| app.should_show())
        .filter_map(|app| {
            let desktop_id = app.id()?.to_string();
            if Some(desktop_id.as_str()) == default {
                return None;
            }
            Some(Handler {
                title: app.display_name().to_string(),
                icon_spec: app.icon().and_then(|icon| icon.to_string()).map(Into::into),
                desktop_id,
            })
        })
        .take(MAX_HANDLERS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_handler_is_left_out() {
        let content_type = "inode/directory";
        let default = gio::AppInfo::default_for_type(content_type, false)
            .and_then(|app| app.id().map(|id| id.to_string()));
        let handlers = for_content_type(content_type);
        assert!(handlers.len() <= MAX_HANDLERS);
        assert!(
            default.is_none()
                || handlers
                    .iter()
                    .all(|handler| Some(handler.desktop_id.as_str()) != default.as_deref())
        );
    }
}
