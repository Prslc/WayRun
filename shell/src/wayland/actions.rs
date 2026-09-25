use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};

use crate::app;
use crate::session::backend;
use wayrun_core::wire::{Action, ActionItem, PanelAction};

use super::Shell;

impl Shell {
    /// Every editing path — typing, paste, IME, ✕ — funnels here into one search
    /// line, and cancels a panel resume before its reply re-opens the panel.
    pub(super) fn query_changed(&mut self) {
        self.app.cancel_panel_resume();
        self.resend_query();
    }

    /// Re-send the current query, leaving a pending panel resume in place.
    fn resend_query(&self) {
        if self.app.query.is_empty() {
            // an empty query means the usage-ranked history
            backend::top();
        } else {
            backend::search(&self.app.query);
        }
    }

    /// Usage recording first, then exactly one command line to the core.
    pub(super) fn submit(&mut self, now: Instant) {
        if self.app.preedit_active {
            return;
        }

        let Some(launch) = self.app.selected_row() else {
            return;
        };

        self.record_row_for(&launch, &launch.effective);

        // Enter runs the remembered default action when the row has one, but
        // usage above stays keyed to the row's own command.
        backend::command(&launch.effective);

        self.schedule_dismiss(now);
    }

    /// Record a row in usage history, unless the command about to run is a
    /// clipboard write: a copy is no re-launchable target and stays out of history.
    fn record_row_for(&self, launch: &app::Launch, runs: &Action) {
        if matches!(runs, Action::Copy { .. }) {
            return;
        }
        let usage = serde_json::json!({
            "title": launch.title,
            "summary": launch.summary,
            "on_click": launch.target,
            "icon": launch.icon,
            "ephemeral": launch.ephemeral,
        });
        backend::select(&usage);
    }

    /// The surface outlives a launch by `EXIT_DELAY_MS`. Without it a non-resident
    /// run can exit before the queued verb reaches the writer thread.
    fn schedule_dismiss(&mut self, now: Instant) {
        let at = now + Duration::from_millis(app::EXIT_DELAY_MS);
        self.app.dismiss_at = Some(at);
        let _ = self.loop_handle.insert_source(
            Timer::from_duration(Duration::from_millis(app::EXIT_DELAY_MS)),
            |deadline, _, state: &mut Shell| {
                if state.app.dismiss_at.is_some_and(|at| deadline >= at) {
                    state.dismiss(deadline);
                }
                TimeoutAction::Drop
            },
        );
    }

    pub(super) fn open_panel(&mut self, now: Instant) {
        if self.app.preedit_active {
            return;
        }
        if self.app.open_actions() {
            self.app.retarget_height(now);
            self.app.resync_hover();
            // The panel is taller than the row list was: the band below the
            // current card edge belongs to the backdrop during the reflow.
            self.needs_full = true;
            self.redraw();
        }
    }

    pub(super) fn close_panel(&mut self, now: Instant) {
        // The user closed it: a panel action's pending reply must not bring it back.
        self.app.cancel_panel_resume();
        if self.app.menu.take().is_some() {
            self.app.retarget_height(now);
            self.app.resync_hover();
            self.needs_full = true;
            self.redraw();
        }
    }

    pub(super) fn run_action(&mut self, now: Instant) {
        let Some(action) = self.app.selected_action() else {
            return;
        };
        self.execute_action(&action, now);
    }

    /// Alt+Enter in the panel: remember the highlighted action as its plugin's default;
    /// clear it if it already is, or is the row's command; a plugin-less one is ignored.
    pub(super) fn toggle_default(&mut self) {
        let Some(action) = self.app.selected_action() else {
            return;
        };
        let Some((plugin, action_id)) = self.app.selected_default() else {
            return;
        };
        backend::default(plugin, action_id);
        self.keep_panel(&action);
    }

    /// One action-panel command: pin/unpin and the default re-search keep the panel on
    /// the changed entry; the rest launch and dismiss; `forget` drops its row.
    fn execute_action(&mut self, action: &ActionItem, now: Instant) {
        match &action.action {
            PanelAction::Execute { command } => {
                if let Some(launch) = self.app.selected_row() {
                    self.record_row_for(&launch, command);
                }
                backend::command(command);
                self.schedule_dismiss(now);
            }
            PanelAction::Pin { scope } => {
                if let Some(command) = self.app.menu_parent_command() {
                    backend::pin(scope, &command);
                }
                self.keep_panel(action);
            }
            PanelAction::Unpin { scope, on_click } => {
                backend::unpin(scope, on_click);
                self.keep_panel(action);
            }
            PanelAction::Forget { on_click } => {
                backend::forget_row(on_click);
                self.close_panel(now);
            }
        }
    }

    /// Re-send the query with the panel held open on `action`, so the reply
    /// shows the change where the user made it.
    fn keep_panel(&mut self, action: &ActionItem) {
        self.app.keep_panel(action);
        self.resend_query();
    }

    /// Ctrl+C/X: hand the selected text to the core's `copy` command, which
    /// writes it with `wl-copy`. Whether there was a selection to copy.
    pub(super) fn copy_selection(&self) -> bool {
        let Some((start, end)) = self.app.selection() else {
            return false;
        };

        let text = self.app.query[start..end].to_string();
        backend::command(&Action::Copy { text });
        true
    }
}
