use std::cmp::Reverse;
use std::time::Duration;

use super::actions::{decorate, pin_scope};
use super::model::Meta;
use super::rank::Rank;
use super::registry::{REGISTRY, ensure_loaded, resolve_pending};
use crate::provider::SHOW_CAP;
use crate::system::icon::find_icon_path;
use crate::wire::Action;
use crate::wire::ResultItem;
use rust_i18n::t;

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
        let defaults = tokio::task::spawn_blocking(crate::system::db::defaults::all)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        return reg
            .iter()
            .map(|entry| {
                let meta = entry.plugin.meta();
                let usage = if entry.keyword.is_empty() {
                    t!("help.usage_default")
                } else {
                    format!("{} <query>", entry.keyword)
                };
                let default = defaults.get(meta.id).map(String::as_str);
                identity_card(meta, help_summary(usage, &meta.ready, default))
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
            return vec![identity_card(meta, meta.ready.clone())];
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

    // Every default provider answers and the merge orders them, so a prefix
    // command never hides an exact app.
    let mut rows: Vec<(Rank, u32, ResultItem)> = Vec::new();
    for (at, entry) in reg.iter().filter(|e| e.keyword.is_empty()).enumerate() {
        // only a forked host runs under the deadline; a built-in answers off its
        // own lists, where a broad query's first icon lookups can cost real time
        let answer = if entry.external {
            tokio::time::timeout(DEFAULT_DEADLINE, entry.plugin.search_ranked(query, input))
                .await
                .ok()
        } else {
            Some(entry.plugin.search_ranked(query, input).await)
        };
        if let Some(Ok(answered)) = answer {
            let fallback = find_icon_path(entry.plugin.meta().icon);
            rows.extend(answered.into_iter().map(|(rank, mut item)| {
                fill_icon(&fallback, &mut item);
                (rank, at as u32, item)
            }));
        }
    }
    // The query touches the shared connection, so it runs off the runtime's
    // workers; the keys go with it and come back, so they are built once.
    let keys = usage_keys(&rows);
    let Ok((counts, keys)) = tokio::task::spawn_blocking(move || {
        let counts = crate::system::db::usage::counts(&keys);
        (counts, keys)
    })
    .await
    else {
        return Vec::new();
    };
    merge_ranked(rows, &keys, &counts)
}

/// Each row's usage key, aligned with `rows`, so the counts query and the merge
/// share one pass; a row with no command carries an empty key.
fn usage_keys(rows: &[(Rank, u32, ResultItem)]) -> Vec<String> {
    rows.iter()
        .map(|(_, _, item)| item.on_click.as_ref().map(Action::key).unwrap_or_default())
        .collect()
}

/// How long the default chain waits for a keyword-less external host before
/// answering without it.
const DEFAULT_DEADLINE: Duration = Duration::from_millis(50);

/// Order the default providers' rows: the strongest kind first, then the most
/// picked of that kind, then the registry order that otherwise ties them.
fn merge_ranked(
    rows: Vec<(Rank, u32, ResultItem)>,
    keys: &[String],
    counts: &std::collections::HashMap<String, u32>,
) -> Vec<ResultItem> {
    // Materialize the sort keys first: a `sort_by_key` closure runs on every
    // comparison, and the usage lookup is a hash per call.
    let mut keyed: Vec<((u8, u32), u32, u32, ResultItem)> = rows
        .into_iter()
        .zip(keys)
        .map(|((rank, provider, item), key)| {
            let used = counts.get(key).copied().unwrap_or(0);
            (rank.key(), used, provider, item)
        })
        .collect();
    keyed.sort_by_key(|(rank, used, provider, _)| (Reverse(*rank), Reverse(*used), *provider));
    keyed
        .into_iter()
        .map(|(_, _, _, item)| item)
        .take(SHOW_CAP)
        .collect()
}

/// A row a provider left iconless takes the provider's identity icon.
fn fill_icon(fallback: &Option<String>, item: &mut ResultItem) {
    if item.icon.as_deref().is_none_or(str::is_empty) {
        item.icon.clone_from(fallback);
    }
}

/// Rows a provider left iconless take its identity icon, so no placeholder ever
/// reaches the shell; running before `decorate`, pins and history store the fill.
fn fill_icons(meta_icon: &str, mut items: Vec<ResultItem>) -> Vec<ResultItem> {
    let fallback = find_icon_path(meta_icon);
    for item in &mut items {
        fill_icon(&fallback, item);
    }
    items
}

/// A plugin's `?` help line: how to reach it, what it does, and the action its
/// `Enter` runs when the user has remembered one.
fn help_summary(usage: String, ready: &str, default: Option<&str>) -> String {
    match default {
        Some(action) => t!(
            "help.line_default",
            usage = usage,
            ready = ready,
            action = action
        ),
        None => t!("help.line", usage = usage, ready = ready),
    }
}

/// A plugin's identity card, also its `?` help row and empty default view.
fn identity_card(meta: &Meta, summary: String) -> ResultItem {
    ResultItem {
        title: meta.name.clone(),
        summary: Some(summary),
        on_click: None,
        icon: find_icon_path(meta.icon).or_else(|| Some(String::new())),
        ephemeral: false,
        actions: Vec::new(),
        badge: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{Match, item};
    use crate::wire::Action;
    use std::collections::HashMap;

    fn run(cmd: &str) -> Action {
        Action::Run {
            cmd: cmd.to_string(),
        }
    }

    #[test]
    fn the_help_line_names_a_remembered_default_action() {
        let usage = || "f <query>".to_string();
        let plain = help_summary(usage(), "Search files by name or path", None);
        assert!(plain.contains("f <query>"));
        assert!(plain.contains("Search files by name or path"));

        let marked = help_summary(usage(), "Search files by name or path", Some("terminal"));
        assert!(marked.contains("terminal"), "{marked}");
        assert!(marked.len() > plain.len());
    }

    #[test]
    fn rows_without_an_icon_take_the_plugin_identity_icon() {
        let mut items = vec![item("blank", run("blank")), item("kept", run("kept"))];
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

    fn row(title: &str, command: &str) -> ResultItem {
        ResultItem {
            title: title.to_string(),
            summary: None,
            on_click: Some(Action::Run {
                cmd: command.to_string(),
            }),
            icon: None,
            ephemeral: false,
            actions: Vec::new(),
            badge: None,
        }
    }

    fn ranked(kind: Match, provider: u32, title: &str) -> (Rank, u32, ResultItem) {
        (
            Rank::title(kind),
            provider,
            row(title, &format!("cmd {title}")),
        )
    }

    /// `merge_ranked` with the keys the caller derives for these rows.
    fn merge(rows: Vec<(Rank, u32, ResultItem)>, counts: &HashMap<String, u32>) -> Vec<ResultItem> {
        let keys = usage_keys(&rows);
        merge_ranked(rows, &keys, counts)
    }

    /// A prefix command never hides an exact app, whatever the registry order says.
    #[test]
    fn an_exact_row_leads_a_prefix_one_from_an_earlier_provider() {
        let rows = vec![
            ranked(Match::Prefix, 1, "command"),
            ranked(Match::Exact, 2, "the app"),
        ];
        let merged = merge(rows, &HashMap::default());
        assert_eq!(merged[0].title, "the app");
    }

    /// Within one kind the row picked more often leads, and otherwise the
    /// registry order stands.
    #[test]
    fn a_used_row_leads_its_own_kind() {
        let rows = vec![
            ranked(Match::Exact, 1, "first"),
            ranked(Match::Exact, 2, "second"),
        ];
        // the usage table is keyed by the canonical action, not the command
        let key = rows[1].2.on_click.as_ref().expect("clickable").key();
        let counts = HashMap::from([(key, 3)]);
        assert_eq!(merge(rows, &counts)[0].title, "second");

        let rows = vec![
            ranked(Match::Exact, 1, "first"),
            ranked(Match::Exact, 2, "second"),
        ];
        assert_eq!(merge(rows, &HashMap::default())[0].title, "first");
    }

    /// A provider that only lists sits below every scored row and keeps its own order.
    #[test]
    fn a_listed_row_sits_below_a_scored_one() {
        let rows = vec![
            (Rank::Listed(0), 0, row("first", "a")),
            (Rank::title(Match::Loose), 1, row("scored", "b")),
            (Rank::Listed(1), 0, row("second", "c")),
        ];
        let merged = merge(rows, &HashMap::default());
        let titles: Vec<&str> = merged.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(titles, ["scored", "first", "second"]);
    }

    /// The merge orders by the weight a provider computed, not by the kind band:
    /// a title word beats a description's prefix but not its exact match.
    #[test]
    fn a_scaled_weight_orders_across_kinds() {
        let rows = vec![
            (Rank::Scored(500), 0, row("title substring", "a")),
            (Rank::Scored(2500), 0, row("summary prefix", "b")),
            (Rank::title(Match::Word), 0, row("title word", "c")),
            (Rank::Scored(5000), 0, row("summary exact", "d")),
        ];
        let merged = merge(rows, &HashMap::default());
        let titles: Vec<&str> = merged.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "summary exact",
                "title word",
                "summary prefix",
                "title substring"
            ]
        );
    }

    #[test]
    fn the_payload_is_capped() {
        let rows: Vec<_> = (0..SHOW_CAP as u32 + 10)
            .map(|at| ranked(Match::Exact, 0, &format!("row {at}")))
            .collect();
        assert_eq!(merge(rows, &HashMap::default()).len(), SHOW_CAP);
    }
}
