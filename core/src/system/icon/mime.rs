use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use gio::prelude::Cast;

use super::find_first_icon_path;

static MIME_CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

fn mime_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    MIME_CACHE.get_or_init(|| Mutex::new(HashMap::default()))
}

/// The icon for a MIME type through its themed-icon priority chain (the first
/// name the theme ships wins); cached like [`resolve`], the theme being fixed.
pub fn content_type_icon(content_type: &str) -> Option<String> {
    if let Ok(cache) = mime_cache().lock()
        && let Some(cached) = cache.get(content_type)
    {
        return cached.clone();
    }

    let result = gio::content_type_get_icon(content_type)
        .downcast::<gio::ThemedIcon>()
        .ok()
        .and_then(|themed| find_first_icon_path(themed.names().iter().map(|name| name.as_str())));

    if let Ok(mut cache) = mime_cache().lock() {
        cache.insert(content_type.to_string(), result.clone());
    }

    result
}
