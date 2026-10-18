use std::future::Future;
use std::pin::Pin;

use anyhow::Result;

use crate::plugin::{Match, Meta, Plugin, Rank, Ranked};
use crate::wire::{ActionItem, ResultItem};
use rust_i18n::t;

use super::file::{PathHit, file_actions, is_path_query, path_rows};

/// The keywordless path lookup: a path-shaped query opens the file or directory
/// it names, without taking the `f`/`d` namespace.
pub struct PathLookup {
    meta: Meta,
}

impl PathLookup {
    pub fn new() -> Self {
        Self {
            meta: Meta {
                id: "path-lookup".into(),
                name: t!("plugin.path.name"),
                icon: "builtin:folder".into(),
                ready: t!("plugin.path.ready"),
            },
        }
    }
}

/// The exact path leads; completions follow in the directory's listed order.
fn rank(hits: Vec<PathHit>) -> Ranked {
    hits.into_iter()
        .enumerate()
        .map(|(index, hit)| {
            let rank = if hit.exact {
                Rank::Scored(Match::Exact.weight())
            } else {
                Rank::Listed(index as u32)
            };
            (rank, hit.item)
        })
        .collect()
}

impl Plugin for PathLookup {
    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn search(
        &self,
        _query: &str,
        full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        let input = full.to_string();
        Box::pin(async move {
            if !is_path_query(input.trim()) {
                return Ok(Vec::new());
            }
            let hits = tokio::task::spawn_blocking(move || path_rows(&input))
                .await
                .unwrap_or_default();
            Ok(hits.into_iter().map(|hit| hit.item).collect())
        })
    }

    fn search_ranked(
        &self,
        _query: &str,
        full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Ranked>> + Send + '_>> {
        let input = full.to_string();
        Box::pin(async move {
            if !is_path_query(input.trim()) {
                return Ok(Vec::new());
            }
            let hits = tokio::task::spawn_blocking(move || path_rows(&input))
                .await
                .unwrap_or_default();
            Ok(rank(hits))
        })
    }

    fn actions(&self, item: &ResultItem) -> Vec<ActionItem> {
        file_actions(item)
    }
}
