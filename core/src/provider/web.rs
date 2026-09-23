use std::future::Future;
use std::pin::Pin;

use crate::plugin::{Meta, Plugin};
use crate::wire::{Action, ActionItem, ResultItem};
use anyhow::{Context, Result};
use rust_i18n::t;

use super::copy_url_action;

/// One search backend. Every engine answers the Firefox-style suggest payload
/// (`["query", ["suggestion", …]]`), so the parser is shared.
struct Engine {
    /// A brand name, so it stays as it is; it is interpolated into the
    /// translated strings around it.
    name: &'static str,
    icon: &'static str,
    /// Search page prefix; the percent-encoded query is appended.
    search_url: &'static str,
    /// Suggest endpoint without `q`, which `with_param` adds encoded.
    suggest_url: &'static str,
}

const GOOGLE: Engine = Engine {
    name: "Google",
    icon: "builtin:globe",
    search_url: "https://www.google.com/search?q=",
    suggest_url: "https://suggestqueries.google.com/complete/search?client=firefox",
};

const DUCKDUCKGO: Engine = Engine {
    name: "DuckDuckGo",
    icon: "builtin:globe",
    search_url: "https://duckduckgo.com/?q=",
    suggest_url: "https://duckduckgo.com/ac/?type=list",
};

fn resolve(engine: &str) -> &'static Engine {
    match engine {
        "duckduckgo" => &DUCKDUCKGO,
        "google" => &GOOGLE,
        other => {
            eprintln!("wayrun-core: unknown search engine {other:?}; using google");
            &GOOGLE
        }
    }
}

fn meta_of(engine: &Engine) -> Meta {
    Meta {
        id: "web-search",
        name: engine.name.to_string(),
        icon: engine.icon,
        ready: t!("plugin.web.ready", engine = engine.name),
    }
}

pub struct WebSearch {
    engine: &'static Engine,
    meta: Meta,
}

impl WebSearch {
    pub fn new() -> Self {
        let engine = resolve(&crate::config::web_search_engine());
        Self {
            engine,
            meta: meta_of(engine),
        }
    }
}

impl Default for WebSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for WebSearch {
    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn search(
        &self,
        query: &str,
        _full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        spawn(self.engine, query)
    }

    fn actions(&self, item: &ResultItem) -> Vec<ActionItem> {
        copy_url_action(item)
    }
}

fn spawn(
    engine: &'static Engine,
    query: &str,
) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + 'static>> {
    let query = query.to_string();
    Box::pin(async move {
        tokio::task::spawn_blocking(move || do_search(engine, &query))
            .await
            .unwrap_or_else(|e| Err(anyhow::Error::from(e)))
    })
}

/// The HTTP CONNECT proxy the environment names, lowercase before uppercase as
/// curl reads it. `None` when neither is set or the value is unusable.
fn proxy_from_env() -> Option<minreq::Proxy> {
    let spec = ["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))?;
    match minreq::Proxy::new(&spec) {
        Ok(proxy) => Some(proxy),
        Err(e) => {
            eprintln!("wayrun-core: ignoring proxy {spec:?}: {e}");
            None
        }
    }
}

fn do_search(engine: &Engine, query: &str) -> Result<Vec<ResultItem>> {
    if query.is_empty() {
        return Ok(vec![]);
    }

    let request = minreq::get(engine.suggest_url).with_timeout(5);
    let request = match proxy_from_env() {
        Some(proxy) => request.with_proxy(proxy),
        None => request,
    };
    let response = request
        .with_param("q", query)
        .send()
        .context("fetching web suggestions")?;
    let json: Vec<serde_json::Value> = response.json().context("parsing web suggestions")?;

    // one resolved engine icon shared by every row (header + suggestions)
    let icon = crate::system::icon::resolve(engine.icon);

    let summary = t!("plugin.web.summary", engine = engine.name);
    let mut results = vec![ResultItem {
        title: t!("plugin.web.search", query = query),
        summary: Some(summary.clone()),
        on_click: Some(Action::Open {
            uri: result_url(engine, query),
        }),
        icon: icon.clone(),
        ephemeral: true,
        actions: Vec::new(),
        badge: None,
    }];

    if let Some(suggestions) = json.get(1).and_then(|s| s.as_array()) {
        // Same engine icon and summary as the header row, so every suggestion
        // renders uniformly.
        results.extend(
            suggestions
                .iter()
                .filter_map(|item| item.as_str())
                .map(|phrase| ResultItem {
                    title: phrase.to_string(),
                    summary: Some(summary.clone()),
                    on_click: Some(Action::Open {
                        uri: result_url(engine, phrase),
                    }),
                    icon: icon.clone(),
                    ephemeral: true,
                    actions: Vec::new(),
                    badge: None,
                }),
        );
    }

    Ok(results)
}

/// The row's open target: a search page with the query percent-encoded, so a
/// space or non-ASCII title stays a valid URI for the core's GLib `open`.
fn result_url(engine: &Engine, query: &str) -> String {
    format!("{}{}", engine.search_url, urlencoding::encode(query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_engine_falls_back_to_google() {
        assert_eq!(resolve("google").name, "Google");
        assert_eq!(resolve("duckduckgo").name, "DuckDuckGo");
        assert_eq!(resolve("nope").name, "Google");
    }

    #[test]
    fn each_engine_builds_its_own_search_url() {
        assert_eq!(
            result_url(&GOOGLE, "a b"),
            "https://www.google.com/search?q=a%20b"
        );
        assert_eq!(
            result_url(&DUCKDUCKGO, "a b"),
            "https://duckduckgo.com/?q=a%20b"
        );
    }

    #[test]
    fn an_unusable_proxy_spec_is_ignored_not_fatal() {
        assert!(minreq::Proxy::new("socks5://127.0.0.1:1080").is_err());
    }
}
