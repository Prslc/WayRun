mod animation;
mod appearance;
mod complete;
mod cursor;
mod edit;
mod menu;
mod row;
#[cfg(test)]
mod test_support;

use std::env;
use std::time::{Duration, Instant};

use wayrun_core::wire::{Action, Preview, ResultItem};
use wayrun_ui::geom::{Layout, PreviewKind};
use wayrun_ui::present::{Presentation, RowMotion};
use wayrun_ui::view::{Body, FieldView, FooterView, Frame, ListView, PanelView, PreviewView};

use self::menu::PanelKey;

pub use self::cursor::{Cursor, Hover, whole_rows};
pub use self::menu::{Menu, effective_action, marked_entry};

/// Launch dismissals wait this long before the surface goes away.
pub const EXIT_DELAY_MS: u64 = 150;
/// The field's caret blink interval.
pub const CARET_BLINK_MS: u64 = 500;
/// How long a search may run before the footer says it is still working.
pub const SEARCH_LOADING_DELAY: Duration = Duration::from_millis(150);
pub struct State {
    pub query: String,
    /// Caret, as a byte index into `query`, always on a char boundary.
    pub caret: usize,
    /// The fixed end of a keyboard selection; `caret` is the moving end. A
    /// selection exists while the two differ (Ctrl+A, Shift+arrows).
    pub anchor: Option<usize>,
    /// The live IME preedit, drawn at the caret (never inserted into `query`).
    pub preedit: Option<String>,
    pub rows: Vec<ResultItem>,
    pub cursor: Cursor,
    /// A row's layout motion: a new row enters, a reordered one slides to its
    /// place. A row that stayed put is `None` and does not animate.
    pub row_motion: Vec<Option<RowMotion>>,
    /// The open action panel, if any. It takes over the list and the keyboard.
    pub menu: Option<Menu>,
    /// The panel's row and entry, held across the re-emit a panel action sends,
    /// so the panel comes back where the user left it instead of dropping.
    panel_resume: Option<(Action, PanelKey)>,
    /// The appearance and motion state a frame is drawn from.
    pub presentation: Presentation,
    /// The layer surface's logical size.
    pub surface: (u32, u32),
    /// The integer buffer scale `wl_surface` reports, used when the compositor
    /// offers no fractional scale; it is the ceiling of the exact ratio.
    pub scale: i32,
    /// The exact ratio from `wp_fractional_scale_v1`: the buffer is
    /// `surface × this` and a viewport maps it back, so `scale` is not used.
    pub fractional: Option<f32>,
    /// The bottom edge of the card drawn last. A settle repaint covers only the
    /// card, so a shrink must restore the dim over the old edge too.
    pub last_card_bottom: f32,
    /// What preview the last frame drew, so a kind change can retarget the
    /// height spring when the list is short.
    preview_drawn: Option<PreviewKind>,
    /// `WAYRUN_TIMING=1`: the per-frame draw marks and the frame-pacing logs.
    pub timing: bool,
    /// What the pointer is over, if anything.
    pub hovered: Option<Hover>,
    /// The pointer's last position over the surface, so a moving list can
    /// re-derive the hover.
    pointer: Option<(f32, f32)>,
    pub pointer_on_card: bool,
    /// fcitx can deliver Enter while a preedit is live; Enter over composition
    /// must not launch a row.
    pub preedit_active: bool,
    pub caret_visible: bool,
    pub caret_at: Instant,
    /// When the pending wait began; keystrokes inside one wait do not move it,
    /// and the footer says "searching" once it passes `SEARCH_LOADING_DELAY`.
    pub search_started: Option<Instant>,
    pub dismiss_at: Option<Instant>,
}

impl State {
    pub fn new() -> Self {
        let now = Instant::now();
        let reduced_env = env::var_os("WAYRUN_REDUCED_MOTION").is_some();
        let timing = env::var_os("WAYRUN_TIMING").is_some();
        Self {
            query: String::new(),
            caret: 0,
            anchor: None,
            preedit: None,
            rows: Vec::new(),
            cursor: Cursor::default(),
            row_motion: Vec::new(),
            menu: None,
            panel_resume: None,
            presentation: Presentation::new(reduced_env),
            surface: (1920, 1080),
            scale: 1,
            fractional: None,
            last_card_bottom: 0.0,
            preview_drawn: None,
            timing,
            hovered: None,
            pointer: None,
            pointer_on_card: false,
            preedit_active: false,
            caret_visible: true,
            caret_at: now,
            search_started: None,
            dismiss_at: None,
        }
    }

    /// The geometry input and output share: config overrides with defaults.
    pub fn layout(&self) -> Layout {
        self.presentation.appearance.layout
    }

    /// The selected row's preview kind when the list owns the card, else none.
    pub fn preview_kind(&self) -> Option<PreviewKind> {
        if self.menu.is_some() || !self.presentation.appearance.preview {
            return None;
        }
        match self.rows.get(self.cursor.selected)?.preview.as_ref()? {
            Preview::Image { .. } => Some(PreviewKind::Image),
            Preview::Text { .. } => Some(PreviewKind::Text),
        }
    }

    pub fn split(&self) -> bool {
        self.preview_kind().is_some()
    }

    fn selected_preview(&self) -> Option<&Preview> {
        self.rows.get(self.cursor.selected)?.preview.as_ref()
    }

    /// A short list gains height when the preview appears or changes kind, so
    /// the spring is retargeted then.
    pub fn sync_split_height(&mut self, now: Instant) {
        let kind = self.preview_kind();
        if kind != self.preview_drawn {
            self.preview_drawn = kind;
            self.retarget_height(now);
        }
    }

    /// The buffer scale to render at: the compositor's fractional ratio when
    /// there is one, else the integer scale.
    pub fn scale_factor(&self) -> f32 {
        self.fractional.unwrap_or(self.scale.max(1) as f32)
    }

    /// Whether the buffers are mapped through a `wp_viewport`: their size is
    /// `surface × scale_factor` and the surface's own buffer scale stays 1.
    pub fn uses_viewport(&self) -> bool {
        self.fractional.is_some()
    }

    /// The card's resting height (panel when open, the field alone while the
    /// query is untouched, else the list). The footer, blur region and reflow read it.
    pub fn content_height(&self) -> f32 {
        let layout = self.layout();
        match &self.menu {
            Some(menu) => layout.panel_h(menu.actions.len()),
            None if self.rows.is_empty() && self.query.is_empty() => layout.empty_h(),
            None => match self.preview_kind() {
                Some(kind) => layout.content_h_for(layout.split_h(self.rows.len(), kind)),
                None => layout.content_h(self.rows.len()),
            },
        }
    }

    /// Note a sent search: the first of a wait starts the loading clock, and a
    /// later keystroke leaves it alone. Returns whether the clock started.
    pub fn search_sent(&mut self, now: Instant) -> bool {
        if self.search_started.is_some() {
            return false;
        }
        self.search_started = Some(now);
        true
    }

    pub fn card_height(&self) -> f32 {
        self.presentation.card_height()
    }

    /// The frame's complete input: content borrowed from the state, appearance
    /// and motion resolved, so the renderer never reads the state machine.
    pub fn frame(&self, now: Instant) -> Frame<'_> {
        let field = FieldView {
            query: &self.query,
            caret: self.caret,
            selection: self.selection(),
            preedit: self.preedit.as_deref(),
            caret_visible: self.caret_visible,
        };
        let (hovered_row, hovered_action) = match self.hovered {
            Some(Hover::Row(index)) => (Some(index), None),
            Some(Hover::Action(index)) => (None, Some(index)),
            None => (None, None),
        };
        let preview = if self.split() {
            self.selected_preview().map(|preview| match preview {
                Preview::Image { path } => PreviewView::Image { path },
                Preview::Text { text } => PreviewView::Text { text },
            })
        } else {
            None
        };
        let body = match &self.menu {
            Some(menu) => Body::Panel(PanelView {
                parent_title: self.menu_parent_title(),
                actions: &menu.actions,
                first: menu.cursor.first,
                selected: menu.cursor.selected,
                hovered: hovered_action,
                marked: self.rows.get(menu.parent).and_then(marked_entry),
            }),
            None => Body::List(ListView {
                rows: &self.rows,
                motion: &self.row_motion,
                first: self.cursor.first,
                selected: self.cursor.selected,
                hovered: hovered_row,
                preview,
            }),
        };

        let selected_row = self.rows.get(self.cursor.selected);
        let panel = self.menu.is_some();
        let loading = self
            .search_started
            .is_some_and(|started| now.saturating_duration_since(started) >= SEARCH_LOADING_DELAY);
        let footer = FooterView {
            rows: self.rows.len(),
            action_count: self.menu.as_ref().map_or(0, |menu| menu.actions.len()),
            query_empty: self.query.is_empty(),
            panel,
            loading,
            has_actions: selected_row.is_some_and(|row| !row.actions.is_empty()),
            can_default: self.selected_default().is_some(),
            is_default: self
                .menu
                .as_ref()
                .and_then(Menu::selected_action)
                .is_some_and(|action| action.default),
            effective_label: if panel {
                None
            } else {
                selected_row
                    .and_then(effective_action)
                    .map(|action| action.title.as_str())
            },
        };

        Frame {
            surface: self.surface,
            scale: self.scale_factor(),
            timing: self.timing,
            layout: self.presentation.frame_layout(now),
            card_h: self.presentation.card_height(),
            content_h: self.content_height(),
            last_card_bottom: self.last_card_bottom,
            opacity: self.presentation.entrance(now),
            theme: self.presentation.theme,
            surfaces: self.presentation.surfaces,
            font: self.presentation.appearance.font,
            field,
            body,
            footer,
        }
    }

    pub fn shown(&mut self, now: Instant) {
        self.query.clear();
        self.caret = 0;
        self.anchor = None;
        self.preedit = None;
        self.preedit_active = false;
        self.cursor.selected = 0;
        self.menu = None;
        self.hovered = None;
        self.dismiss_at = None;
        self.search_started = None;
        self.presentation.begin_show(now);
        self.presentation.snap_to(self.content_height(), now);
        self.preview_drawn = None;
        self.contain();
        self.caret_visible = true;
        self.caret_at = now;
    }

    pub fn hidden(&mut self) {
        self.dismiss_at = None;
        self.search_started = None;
        self.menu = None;
        // A reply that arrives while hidden must not resurrect the panel.
        self.panel_resume = None;
        // The surface is gone, so nothing is composing on it any more.
        self.preedit = None;
        self.preedit_active = false;
        // A show starts from an empty list: an empty query fetches nothing.
        self.rows = Vec::new();
        self.row_motion.clear();
        self.preview_drawn = None;
    }

    /// Keep the selection inside the `max_rows` window, minimally.
    pub fn contain(&mut self) {
        self.cursor.contain(self.rows.len(), self.layout());
        self.resync_hover();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fractional_scale_overrides_the_integer_one() {
        let mut state = State::new();
        // what niri reports for a 1.25x output
        state.scale = 2;
        state.surface = (1536, 864);
        assert_eq!(state.scale_factor(), 2.0);
        assert!(!state.uses_viewport());

        state.fractional = Some(1.25);
        assert_eq!(state.scale_factor(), 1.25);
        assert!(state.uses_viewport());
        // the same logical surface asks for 1920 physical pixels through the
        // fractional ratio and 3072 through the integer scale
        assert_eq!((1536.0 * state.scale_factor()).round() as u32, 1920);
        assert_eq!((1536.0 * 2.0) as u32, 3072);
    }

    #[test]
    fn a_preview_row_switches_the_card_into_the_split_layout() {
        use crate::app::test_support::{item, state};
        use wayrun_core::wire::Preview;

        let mut state = state();
        let now = Instant::now();
        let mut picture = item(
            "picture",
            None,
            Some(Action::Run {
                cmd: "true".to_string(),
            }),
            None,
        );
        picture.preview = Some(Preview::Image {
            path: "/tmp/p.png".to_string(),
        });
        state.apply_results(vec![picture, item("plain", None, None, None)], now);

        assert!(state.split());
        let kind = state.preview_kind().unwrap();
        let split_height = state
            .layout()
            .content_h_for(state.layout().split_h(state.rows.len(), kind));
        assert_eq!(state.content_height(), split_height);
        {
            let frame = state.frame(now);
            match &frame.body {
                Body::List(list) => match &list.preview {
                    Some(PreviewView::Image { path }) => assert_eq!(*path, "/tmp/p.png"),
                    Some(PreviewView::Text { .. }) => panic!("expected an image preview"),
                    None => panic!("the selected row has a preview"),
                },
                Body::Panel(_) => panic!("the list owns the card"),
            }
        }

        state.cursor.selected = 1;
        assert!(!state.split());
        assert_eq!(state.content_height(), state.layout().content_h(2));
    }

    #[test]
    fn a_text_preview_uses_the_text_column() {
        use crate::app::test_support::{item, state};
        use wayrun_core::wire::Preview;

        let mut state = state();
        let now = Instant::now();
        let mut row = item("note", None, None, None);
        row.preview = Some(Preview::Text {
            text: "first line\nsecond line".to_string(),
        });
        state.apply_results(vec![row], now);

        assert_eq!(state.preview_kind(), Some(PreviewKind::Text));
        match &state.frame(now).body {
            Body::List(list) => match &list.preview {
                Some(PreviewView::Text { text }) => {
                    assert_eq!(*text, "first line\nsecond line")
                }
                _ => panic!("the selected row has a text preview"),
            },
            Body::Panel(_) => panic!("the list owns the card"),
        }
        assert_eq!(
            state.content_height(),
            state
                .layout()
                .content_h_for(state.layout().split_h(1, PreviewKind::Text))
        );
    }

    #[test]
    fn disabling_previews_keeps_the_list_full_width() {
        use crate::app::test_support::{item, state};
        use wayrun_core::wire::Preview;

        let mut state = state();
        state.presentation.appearance.preview = false;
        let now = Instant::now();
        let mut row = item("picture", None, None, None);
        row.preview = Some(Preview::Image {
            path: "/tmp/p.png".to_string(),
        });
        state.apply_results(vec![row], now);

        assert_eq!(state.preview_kind(), None);
        assert!(!state.split());
        assert_eq!(state.content_height(), state.layout().content_h(1));
    }
}
