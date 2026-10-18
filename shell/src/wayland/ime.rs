use std::sync::Arc;

use smithay_client_toolkit::dispatch2::Dispatch2;
use wayland_client::backend::ObjectData;
use wayland_client::globals::{BindError, GlobalList};
use wayland_client::{Connection, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::{
    self, ZwpTextInputManagerV3,
};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    self, ContentHint, ContentPurpose, ZwpTextInputV3,
};

use crate::ui::render;

use super::Shell;

/// Object data shared by both proxies: every event is routed to the state.
#[derive(Debug)]
pub struct TextInputData;

/// Bind `zwp_text_input_manager_v3`, if the compositor offers it.
pub fn bind(
    globals: &GlobalList,
    qh: &QueueHandle<Shell>,
) -> Result<ZwpTextInputManagerV3, BindError> {
    globals.bind(qh, 1..=1, TextInputData)
}

impl Dispatch2<ZwpTextInputManagerV3, Shell> for TextInputData {
    fn event(
        &self,
        _state: &mut Shell,
        _manager: &ZwpTextInputManagerV3,
        event: zwp_text_input_manager_v3::Event,
        _conn: &Connection,
        _qh: &QueueHandle<Shell>,
    ) {
        // The manager has no events; the match keeps the arm exhaustive if the
        // protocol ever grows one.
        let _ = event;
    }

    /// `get_text_input` creates the per-seat proxy: it must be dispatched as
    /// this same object data type.
    fn event_created_child(_opcode: u16, qh: &QueueHandle<Shell>) -> Arc<dyn ObjectData> {
        qh.make_data::<ZwpTextInputV3, TextInputData>(TextInputData)
    }
}

impl Dispatch2<ZwpTextInputV3, Shell> for TextInputData {
    fn event(
        &self,
        state: &mut Shell,
        _input: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _conn: &Connection,
        _qh: &QueueHandle<Shell>,
    ) {
        state.ime_event(event);
    }
}

/// The staged `zwp_text_input_v3` state. The protocol is double-buffered, so the
/// events only apply once `done` arrives.
#[derive(Debug, Default)]
pub struct Pending {
    /// `Some(None)` is an explicit clear.
    preedit: Option<Option<String>>,
    commit: Option<String>,
    delete: Option<(u32, u32)>,
}

/// What `done` should do with the drawn preedit.
#[derive(Debug, PartialEq, Eq)]
pub enum PreeditAction {
    /// The IME said nothing about a preedit, so the composition is over and what
    /// is drawn must go.
    Clear,
    /// The IME staged a preedit: `None` is an explicit clear.
    Set(Option<String>),
}

impl Pending {
    pub fn stage_preedit(&mut self, text: Option<String>) {
        self.preedit = Some(text.filter(|text| !text.is_empty()));
    }

    pub fn stage_commit(&mut self, text: Option<String>) {
        self.commit = text.filter(|text| !text.is_empty());
    }

    pub fn stage_delete(&mut self, before: u32, after: u32) {
        self.delete = Some((before, after));
    }

    /// Everything staged since the last `done`, plus what the drawn preedit
    /// should become. Clears the staged state.
    pub fn resolve(&mut self) -> (Option<(u32, u32)>, Option<String>, PreeditAction) {
        let delete = self.delete.take();
        let commit = self.commit.take();
        let preedit = match self.preedit.take() {
            Some(preedit) => PreeditAction::Set(preedit),
            None => PreeditAction::Clear,
        };

        (delete, commit, preedit)
    }

    /// The input left this surface: nothing staged is still meaningful.
    pub fn clear(&mut self) {
        self.preedit = None;
        self.commit = None;
        self.delete = None;
    }
}

/// The content type the launcher asks for: a single normal line.
pub fn content_type() -> (ContentHint, ContentPurpose) {
    (ContentHint::None, ContentPurpose::Normal)
}

impl Shell {
    pub(super) fn enable_ime(&mut self) {
        let Some(ime) = self.ime.as_ref() else { return };
        let (hint, purpose) = content_type();
        ime.enable();
        ime.set_content_type(hint, purpose);
        ime.commit();
        self.ime_cursor_sent = None;
    }

    pub(super) fn disable_ime(&mut self) {
        if let Some(ime) = self.ime.as_ref() {
            ime.disable();
            ime.commit();
        }
        self.ime_cursor_sent = None;
    }

    /// Tell the IME where the caret is, using the renderer's own caret geometry so
    /// a scrolled field cannot strand it; deduped like the blur region.
    pub(super) fn sync_ime_cursor(&mut self) {
        let Some(ime) = self.ime.as_ref() else { return };
        if self.layer.is_none() {
            return;
        }

        let (x, y, width, height) =
            render::caret_rect(&self.app, &mut self.text, std::time::Instant::now());
        // 4px-quantised, like the blur region: a present must not commit a
        // text-input state change of its own.
        let key = (x / 4, y / 4, width, height);
        if self.ime_cursor_sent == Some(key) {
            return;
        }

        ime.set_cursor_rectangle(x, y, width, height);
        ime.commit();
        self.ime_cursor_sent = Some(key);
    }

    pub fn ime_event(&mut self, event: zwp_text_input_v3::Event) {
        if self.ime_log {
            eprintln!("wayrun: ime event {event:?}");
        }

        match event {
            zwp_text_input_v3::Event::Enter { .. } => self.enable_ime(),
            zwp_text_input_v3::Event::Leave { .. } => {
                self.pending.clear();
                self.app.preedit = None;
                self.app.preedit_active = false;
                self.redraw();
            }
            zwp_text_input_v3::Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                let _ = (cursor_begin, cursor_end);
                self.pending.stage_preedit(text);
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                self.pending.stage_commit(text);
            }
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                self.pending.stage_delete(before_length, after_length);
            }
            zwp_text_input_v3::Event::Done { .. } => self.ime_done(),
            _ => {}
        }
    }

    /// `done` is the only point at which the staged state may be applied.
    fn ime_done(&mut self) {
        let (delete, commit, preedit) = self.pending.resolve();
        let edited = self.apply_staged(delete, commit);

        match preedit {
            PreeditAction::Set(preedit) => {
                self.app.preedit_active = preedit.is_some();
                self.app.preedit = preedit;
            }
            PreeditAction::Clear => {
                self.app.preedit = None;
                self.app.preedit_active = false;
            }
        }

        // A commit is a query change like any keystroke, or the composition draws
        // characters but never searches; the preedit stays out of `query`.
        if edited {
            self.query_changed();
        }

        if self.ime_log {
            eprintln!(
                "wayrun: ime done -> query={:?} caret={} preedit={:?}",
                self.app.query, self.app.caret, self.app.preedit
            );
        }
        self.redraw();
    }

    /// Apply what `done` staged for the query; whether the query changed.
    fn apply_staged(&mut self, delete: Option<(u32, u32)>, commit: Option<String>) -> bool {
        let query = self.app.query.clone();

        if let Some((before, after)) = delete {
            self.app.delete_surrounding(before, after);
        }
        if let Some(commit) = commit {
            self.app.insert(&commit);
        }

        self.app.query != query
    }
}

#[cfg(test)]
mod tests {
    use super::{Pending, PreeditAction};

    #[test]
    fn a_bare_done_ends_the_composition() {
        let mut pending = Pending::default();
        assert_eq!(
            pending.resolve(),
            (None, None, PreeditAction::Clear),
            "a done with nothing staged clears the drawn preedit"
        );
    }

    #[test]
    fn an_explicit_empty_preedit_is_a_clear_too() {
        let mut pending = Pending::default();
        pending.stage_preedit(Some(String::new()));
        assert_eq!(pending.resolve().2, PreeditAction::Set(None));
        pending.stage_preedit(None);
        assert_eq!(pending.resolve().2, PreeditAction::Set(None));
    }

    #[test]
    fn a_staged_preedit_replaces_what_is_drawn() {
        let mut pending = Pending::default();
        pending.stage_preedit(Some("你好".into()));
        assert_eq!(pending.resolve().2, PreeditAction::Set(Some("你好".into())));
    }

    #[test]
    fn a_commit_ends_the_composition_as_well() {
        let mut pending = Pending::default();
        pending.stage_commit(Some("你好".into()));
        let (delete, commit, preedit) = pending.resolve();
        assert_eq!(delete, None);
        assert_eq!(commit.as_deref(), Some("你好"));
        assert_eq!(preedit, PreeditAction::Clear);
    }

    #[test]
    fn deleting_surrounding_text_and_a_preedit_stage_together() {
        let mut pending = Pending::default();
        pending.stage_delete(1, 2);
        pending.stage_preedit(Some("a".into()));
        let (delete, commit, preedit) = pending.resolve();
        assert_eq!(delete, Some((1, 2)));
        assert_eq!(commit, None);
        assert_eq!(preedit, PreeditAction::Set(Some("a".into())));
        // and everything is consumed, so a second resolve is a clear
        assert_eq!(pending.resolve().2, PreeditAction::Clear);
    }

    #[test]
    fn leaving_the_surface_drops_what_was_staged() {
        let mut pending = Pending::default();
        pending.stage_preedit(Some("你".into()));
        pending.clear();
        assert_eq!(pending.resolve().2, PreeditAction::Clear);
    }
}
