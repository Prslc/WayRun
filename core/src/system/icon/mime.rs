use std::collections::HashMap;
use std::sync::LazyLock;

use gio::prelude::Cast;

use super::{find_first_icon_path, theme_source};
use crate::live::Live;

static MIME_CACHE: LazyLock<Live<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Live::shared(theme_source()));

/// The icon for a MIME type through its themed-icon priority chain (the first
/// name the theme ships wins); cached like [`resolve`] over the same epoch.
pub fn content_type_icon(content_type: &str) -> Option<String> {
    MIME_CACHE.memoized(content_type, || {
        gio::content_type_get_icon(content_type)
            .downcast::<gio::ThemedIcon>()
            .ok()
            .and_then(|themed| {
                find_first_icon_path(themed.names().iter().map(|name| name.as_str()))
            })
    })
}
