use super::actions::{decorate, pin_scope};
use super::registry::{REGISTRY, ensure_loaded, resolve_pending};
use crate::system::icon::find_icon_path;
use crate::wire::ResultItem;

/// Run a search and surface its action panel: the query's pins are prepended
/// and every row is decorated with its secondary commands.
pub async fn dispatch(input: &str) -> Vec<ResultItem> {
    let items = search(input).await;
    let Some(scope) = pin_scope(input) else {
        return items;
    };
    decorate(items, scope, false).await
}

async fn search(input: &str) -> Vec<ResultItem> {
    ensure_loaded().await;
    // A search that names a keyword only wakes that plugin's host; `?` lists
    // every name, so it resolves them all. A plain query wakes none.
    if input.trim() == "?" {
        resolve_pending(None).await;
    } else if let Some((keyword, _)) = input.split_once(' ')
        && !keyword.trim().is_empty()
    {
        resolve_pending(Some(keyword.trim())).await;
    }
    let reg = REGISTRY.read().await;

    if input.trim() == "?" {
        return reg
            .iter()
            .map(|entry| {
                let meta = entry.plugin.meta();
                let usage = if entry.keyword.is_empty() {
                    "* (default)".to_string()
                } else {
                    format!("{} <query>", entry.keyword)
                };
                ResultItem {
                    title: meta.name.to_string(),
                    summary: Some(format!("{usage} - {}", meta.ready)),
                    on_click: None,
                    icon: find_icon_path(meta.icon).or_else(|| Some(String::new())),
                    ephemeral: false,
                    actions: Vec::new(),
                    badge: None,
                }
            })
            .collect();
    }

    let (keyword, query) = input
        .split_once(' ')
        .map_or(("", input), |(k, q)| (k.trim(), q.trim()));

    // A keyword some plugin owns is its own namespace: a miss stays empty and
    // never falls through to the default providers.
    let routed = !keyword.is_empty() && reg.iter().any(|entry| entry.keyword == keyword);

    if routed {
        if query.is_empty()
            && let Some(entry) = reg.iter().find(|entry| entry.keyword == keyword)
        {
            // Keyword-only input opens the plugin: a host may serve a default
            // view (`top`), else the identity card.
            if let Ok(Some(items)) = entry.plugin.default_view().await
                && !items.is_empty()
            {
                return fill_icons(entry.plugin.meta().icon, items);
            }
            let meta = entry.plugin.meta();
            return vec![ResultItem {
                title: meta.name.to_string(),
                summary: Some(meta.ready.to_string()),
                on_click: None,
                icon: find_icon_path(meta.icon).or_else(|| Some(String::new())),
                ephemeral: false,
                actions: Vec::new(),
                badge: None,
            }];
        }

        for entry in reg.iter().filter(|e| e.keyword == keyword) {
            if let Ok(results) = entry.plugin.search(query, input).await
                && !results.is_empty()
            {
                return fill_icons(entry.plugin.meta().icon, results);
            }
        }

        return vec![];
    }

    for entry in reg.iter().filter(|e| e.keyword.is_empty()) {
        if let Ok(results) = entry.plugin.search(query, input).await
            && !results.is_empty()
        {
            return fill_icons(entry.plugin.meta().icon, results);
        }
    }

    vec![]
}

/// Rows a provider left iconless take its identity icon, so a command or window
/// row never shows the app placeholder before the shell sees it. Running before
/// `decorate`, this is also what pins and usage history store.
fn fill_icons(meta_icon: &str, mut items: Vec<ResultItem>) -> Vec<ResultItem> {
    let fallback = find_icon_path(meta_icon);
    for item in &mut items {
        if item.icon.as_deref().is_none_or(str::is_empty) {
            item.icon.clone_from(&fallback);
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::item;

    #[test]
    fn rows_without_an_icon_take_the_plugin_identity_icon() {
        let mut items = vec![item("blank", "run:blank"), item("kept", "run:kept")];
        items[0].icon = Some(String::new());
        items[1].icon = Some("/tmp/kept.svg".into());

        let filled = fill_icons("utilities-terminal", items);
        assert_eq!(
            filled[0].icon,
            find_icon_path("utilities-terminal"),
            "an empty spec is a miss, not an icon"
        );
        assert_eq!(filled[1].icon.as_deref(), Some("/tmp/kept.svg"));
    }
}
