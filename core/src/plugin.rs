use std::future::Future;
use std::pin::Pin;

use crate::wire::{Action, ResultItem};

mod actions;
mod model;
mod registry;
mod search;

pub use actions::{decorate, forget_row};
pub use model::Meta;
pub use registry::{list_plugins, print_list, reload, reload_if_changed};
pub use search::dispatch;

pub trait Plugin: Send + Sync {
    fn meta(&self) -> &Meta;
    fn search(
        &self,
        query: &str,
        full: &str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<ResultItem>>> + Send + '_>>;
    /// Default view for a keyword-only query. `Ok(None)` keeps the identity card;
    /// external hosts override it with their `top` method.
    #[allow(clippy::type_complexity)] // same hand-rolled future type as `search`
    fn default_view(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<Vec<ResultItem>>>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }

    /// Drop a row's data (best effort, from `forget`); usage history is the
    /// caller's. `true` means this provider owned the row and dropped it.
    fn forget(
        &self,
        _command: &Action,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<bool>> + Send + '_>> {
        Box::pin(async { Ok(false) })
    }

    /// Type-specific commands for one of this plugin's rows, shown in the action
    /// panel. The core adds pin/unpin and history removal itself.
    fn actions(&self, _item: &ResultItem) -> Vec<crate::wire::ActionItem> {
        Vec::new()
    }
}

#[cfg(test)]
fn item(title: &str, command: Action) -> ResultItem {
    ResultItem {
        title: title.to_string(),
        summary: None,
        on_click: Some(command),
        icon: None,
        ephemeral: false,
        actions: Vec::new(),
        badge: None,
    }
}
