use std::time::{Duration, Instant};

use crate::config::{AppearanceConfig, Mode};
use crate::ui::geom::Layout;
use crate::ui::theme::{Surfaces, Theme};
use wayrun_core::wire::{Action, ActionItem, PanelAction, ResultItem};

/// Launch dismissals wait this long before the surface goes away.
pub const EXIT_DELAY_MS: u64 = 150;
/// The field's caret blink interval.
pub const CARET_BLINK_MS: u64 = 500;

/// What the pointer is over: a list row's tint, an action-panel row's, or the
/// ✕ button's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hover {
    Row(usize),
    Action(usize),
    Clear,
}

/// The window a list moves through: the highlighted index and the top row of the
/// fixed `max_rows` window. The result list and the action panel each own one
/// and move theirs by the same rules.
#[derive(Default)]
pub struct Cursor {
    pub selected: usize,
    pub first: usize,
}

impl Cursor {
    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn down(&mut self, len: usize) {
        if self.selected + 1 < len {
            self.selected += 1;
        }
    }

    pub fn page_up(&mut self, page: usize) {
        self.selected = self.selected.saturating_sub(page);
    }

    pub fn page_down(&mut self, len: usize, page: usize) {
        self.selected = (self.selected + page).min(len.saturating_sub(1));
    }

    /// Move by whole rows (the wheel).
    pub fn scroll(&mut self, rows: i32, len: usize) {
        let last = len.saturating_sub(1) as i32;
        self.selected = (self.selected as i32 + rows).clamp(0, last) as usize;
    }

    pub fn contain(&mut self, len: usize, layout: Layout) {
        self.first = layout.contain(self.selected, self.first, len);
    }
}

/// The action panel (Wox-style) opened with Shift+Enter over the selected row. It
/// replaces the result list; `parent` is the row it belongs to.
pub struct Menu {
    pub parent: usize,
    pub actions: Vec<ActionItem>,
    pub cursor: Cursor,
}

impl Menu {
    /// The action the panel would run.
    pub fn selected_action(&self) -> Option<&ActionItem> {
        self.actions.get(self.cursor.selected)
    }
}

/// A panel entry, named so a re-emit can find it again: an action id survives
/// verbatim, the row's own command is the id-less entry the core owns, and the
/// pin entry changes its label (`Pin to top` becomes `Unpin`) as it lands.
enum PanelKey {
    Id(String),
    Primary,
    Pin,
}

impl PanelKey {
    fn of(action: &ActionItem) -> Option<Self> {
        match &action.action {
            PanelAction::Pin { .. } | PanelAction::Unpin { .. } => Some(PanelKey::Pin),
            PanelAction::Execute { .. } => Some(match action.id.clone() {
                Some(id) => PanelKey::Id(id),
                None => PanelKey::Primary,
            }),
            // Removal takes the row the panel is about, so there is nothing to
            // come back to.
            PanelAction::Forget { .. } => None,
        }
    }

    fn matches(&self, action: &ActionItem) -> bool {
        match self {
            PanelKey::Id(id) => action.id.as_deref() == Some(id.as_str()),
            // The row's own command is the panel's leading entry, owned by a
            // plugin or not: a host's row has no plugin to hang it on.
            PanelKey::Primary => {
                action.id.is_none() && matches!(action.action, PanelAction::Execute { .. })
            }
            PanelKey::Pin => matches!(
                action.action,
                PanelAction::Pin { .. } | PanelAction::Unpin { .. }
            ),
        }
    }
}

/// Whether a panel entry runs the row's own command: the core's leading "Open"
/// entry and nothing else.
pub fn is_row_command(row: &ResultItem, action: &ActionItem) -> bool {
    matches!(
        &action.action,
        PanelAction::Execute { command } if row.on_click.as_ref() == Some(command)
    )
}

/// The entry the panel dots: the remembered default, else the row's own command,
/// so exactly one entry shows what the row's `Enter` runs.
pub fn marked_entry(row: &ResultItem) -> Option<usize> {
    row.actions
        .iter()
        .position(|action| action.default)
        .or_else(|| {
            row.actions
                .iter()
                .position(|action| is_row_command(row, action))
        })
}

/// The action a row's `Enter` runs instead of the row's own command, which only
/// a remembered default changes.
pub fn effective_action(row: &ResultItem) -> Option<&ActionItem> {
    let action = row.actions.iter().find(|action| action.default)?;
    let PanelAction::Execute { command } = &action.action else {
        return None;
    };
    (row.on_click.as_ref() != Some(command)).then_some(action)
}

/// A blank icon spec means "no icon", so a re-send compares equal.
fn normalize_icons(items: Vec<ResultItem>) -> Vec<ResultItem> {
    items
        .into_iter()
        .map(|mut item| {
            item.icon = item.icon.filter(|spec| !spec.is_empty());
            item
        })
        .collect()
}

/// The selected row's fields that `select` and the launch command need.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub title: String,
    pub summary: Option<String>,
    pub icon: Option<String>,
    /// The row's own command, recorded in usage so history and forget stay
    /// keyed to it.
    pub target: Action,
    /// The command Enter runs: the remembered default action when the row has
    /// one, else `target`.
    pub effective: Action,
    pub ephemeral: bool,
}

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
    /// The open action panel, if any. It takes over the list and the keyboard.
    pub menu: Option<Menu>,
    /// The panel's row and entry, held across the re-emit a panel action sends,
    /// so the panel comes back where the user left it instead of dropping.
    panel_resume: Option<(Action, PanelKey)>,
    /// The core's system theme (the DMS palette, or the fallback).
    pub system_theme: Theme,
    /// Which system palette is active, so `[colors.dark]`/`[colors.light]`
    /// pick the matching table.
    pub mode: Mode,
    /// The `theme.toml` appearance config.
    pub appearance: AppearanceConfig,
    /// The effective theme: `system_theme` with `appearance.colors` on top.
    pub theme: Theme,
    /// The per-surface colours derived from `theme` and the colour overrides.
    pub surfaces: Surfaces,
    /// The layer surface's logical size.
    pub surface: (u32, u32),
    /// The integer buffer scale `wl_surface` reports, used when the compositor
    /// offers no fractional scale; it is the ceiling of the exact ratio.
    pub scale: i32,
    /// The exact ratio from `wp_fractional_scale_v1`: the buffer is
    /// `surface × this` and a viewport maps it back, so `scale` is not used.
    pub fractional: Option<f32>,
    /// Whether the entrance animation has started on the first drawn frame; the
    /// layer surface configures a round trip after `shown`.
    pub entrance_started: bool,
    /// `WAYRUN_REDUCED_MOTION`: ORed with the config's `motion.reduced`.
    reduced_env: bool,
    pub reduce_motion: bool,
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
    shown_at: Instant,
    card_from: f32,
    card_to: f32,
    card_at: Instant,
    pub dismiss_at: Option<Instant>,
    /// The bottom edge of the card drawn last. A settle repaint covers only the
    /// card, so a shrink must restore the dim over the old edge too.
    pub last_card_bottom: f32,
}

impl State {
    pub fn new() -> Self {
        let now = Instant::now();
        let appearance = AppearanceConfig::default();
        let reduced_env = std::env::var_os("WAYRUN_REDUCED_MOTION").is_some();
        let reduce_motion = reduced_env || appearance.reduced;
        let theme = Theme::default();
        let mode = Mode::default();
        let colors = appearance.colors.for_mode(mode);
        let surfaces = Surfaces::resolve(theme, &colors);
        let card = appearance.layout.content_h(0);
        Self {
            query: String::new(),
            caret: 0,
            anchor: None,
            preedit: None,
            rows: Vec::new(),
            cursor: Cursor::default(),
            menu: None,
            panel_resume: None,
            system_theme: theme,
            mode,
            appearance,
            theme,
            surfaces,
            surface: (1920, 1080),
            scale: 1,
            fractional: None,
            entrance_started: false,
            reduced_env,
            reduce_motion,
            hovered: None,
            pointer: None,
            pointer_on_card: false,
            preedit_active: false,
            caret_visible: true,
            caret_at: now,
            shown_at: now,
            card_from: card,
            card_to: card,
            card_at: now,
            dismiss_at: None,
            last_card_bottom: 0.0,
        }
    }

    /// The `theme.toml` config changed. Recompute the theme and the derived
    /// geometry; the caller repaints the whole frame.
    pub fn apply_appearance(&mut self, config: AppearanceConfig, now: Instant) {
        self.reduce_motion = self.reduced_env || config.reduced;
        self.appearance = config;
        self.refresh_theme();
        self.card_from = self.content_height();
        self.card_to = self.card_from;
        self.card_at = now;
    }

    /// The core's system theme changed, with the mode whose palette it holds.
    pub fn set_system_theme(&mut self, system: Theme, mode: Mode) {
        self.system_theme = system;
        self.mode = mode;
        self.refresh_theme();
    }

    fn refresh_theme(&mut self) {
        let colors = self.appearance.colors.for_mode(self.mode);
        self.theme = Theme::overlay(self.system_theme, &colors);
        self.surfaces = Surfaces::resolve(self.theme, &colors);
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

    /// The card's resting height (panel when open, else the list). The footer,
    /// blur region and reflow all read it, so they agree with the drawn frame.
    pub fn content_height(&self) -> f32 {
        match &self.menu {
            Some(menu) => self.appearance.layout.panel_h(menu.actions.len()),
            None => self.appearance.layout.content_h(self.rows.len()),
        }
    }

    pub fn card_height(&self, now: Instant) -> f32 {
        let to = self.card_to;
        if self.reduce_motion {
            return to;
        }

        let elapsed = now.saturating_duration_since(self.card_at).as_secs_f32() * 1000.0;
        let t = (elapsed / self.appearance.reflow_ms as f32).clamp(0.0, 1.0);
        self.card_from + (to - self.card_from) * ease_out_cubic(t)
    }

    /// The backdrop dim's current alpha (0 → the surface's alpha), not a
    /// progress fraction.
    pub fn dim_alpha(&self, now: Instant) -> f32 {
        (self.surfaces.dim[3] as f32 / 255.0) * self.entrance(now)
    }

    /// The backdrop dim's colour, its alpha handled by [`Self::dim_alpha`].
    pub fn dim_color(&self) -> [u8; 3] {
        [
            self.surfaces.dim[0],
            self.surfaces.dim[1],
            self.surfaces.dim[2],
        ]
    }

    /// The muted text alpha, shared by the `fg`-tinted hints and the `primary`
    /// `⏎` markers, which take the same alpha over a different role.
    pub fn muted_alpha(&self) -> f32 {
        self.surfaces.muted[3] as f32 / 255.0
    }

    /// The card's fill alpha for the entrance's opacity fade.
    pub fn entrance(&self, now: Instant) -> f32 {
        if self.reduce_motion {
            // Reduced motion starts at the end state: returning 0 here leaves
            // the dim and every faded colour invisible, not "no animation".
            return 1.0;
        }
        if !self.entrance_started {
            return 0.0;
        }

        let elapsed = now.saturating_duration_since(self.shown_at).as_secs_f32() * 1000.0;
        ease_out_quint((elapsed / self.appearance.entrance_ms as f32).clamp(0.0, 1.0))
    }

    /// A card-content colour at `alpha`, scaled by the entrance fade: the whole
    /// card arrives as one unit and the first frame really is invisible.
    pub fn fade(&self, color: [u8; 3], alpha: f32, now: Instant) -> [u8; 4] {
        [
            color[0],
            color[1],
            color[2],
            ((alpha * self.entrance(now)).clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }

    /// A resolved surface colour, whose own inline alpha rides the entrance.
    pub fn fade_rgba(&self, color: [u8; 4], now: Instant) -> [u8; 4] {
        self.fade([color[0], color[1], color[2]], color[3] as f32 / 255.0, now)
    }

    pub fn animating(&self, now: Instant) -> bool {
        if self.reduce_motion || !self.entrance_started {
            return false;
        }

        let entrance = now.saturating_duration_since(self.shown_at)
            < Duration::from_millis(self.appearance.entrance_ms);
        let reflow = now.saturating_duration_since(self.card_at)
            < Duration::from_millis(self.appearance.reflow_ms);
        entrance || reflow
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
        self.shown_at = now;
        self.card_at = now;
        self.card_from = self.content_height();
        self.card_to = self.card_from;
        self.contain();
        self.caret_visible = true;
        self.caret_at = now;
        self.entrance_started = false;
    }

    /// The first frame of a show: this is where the entrance fade begins.
    pub fn start_entrance(&mut self, now: Instant) {
        if self.entrance_started {
            return;
        }
        self.entrance_started = true;
        self.shown_at = now;
        self.card_at = now;
        self.card_from = self.content_height();
        self.card_to = self.card_from;
    }

    pub fn hidden(&mut self) {
        self.dismiss_at = None;
        self.menu = None;
        // A reply that arrives while hidden must not resurrect the panel.
        self.panel_resume = None;
        // The surface is gone, so nothing is composing on it any more.
        self.preedit = None;
        self.preedit_active = false;
    }

    /// Keep the selection inside the `max_rows` window, minimally.
    pub fn contain(&mut self) {
        self.cursor.contain(self.rows.len(), self.appearance.layout);
        self.resync_hover();
    }

    /// Open the selected row's action panel; whether it had one to open.
    pub fn open_actions(&mut self) -> bool {
        let Some(row) = self.rows.get(self.cursor.selected) else {
            return false;
        };
        if row.actions.is_empty() {
            return false;
        }
        self.menu = Some(Menu {
            parent: self.cursor.selected,
            actions: row.actions.clone(),
            cursor: Cursor::default(),
        });
        true
    }

    pub fn close_actions(&mut self) {
        self.menu = None;
    }

    /// Remember the panel's row and highlighted entry, so the re-emit the action
    /// is about to trigger brings the panel back instead of dropping it.
    pub fn keep_panel(&mut self, action: &ActionItem) {
        let Some(key) = PanelKey::of(action) else {
            return;
        };
        let Some(parent) = self.menu.as_ref().map(|menu| menu.parent) else {
            return;
        };
        let Some(on_click) = self.rows.get(parent).and_then(|row| row.on_click.clone()) else {
            return;
        };
        self.panel_resume = Some((on_click, key));
    }

    /// Drop a pending resume: an edit or a dismissal means the panel must not
    /// come back when the reply it asked for finally lands.
    pub fn cancel_panel_resume(&mut self) {
        self.panel_resume = None;
    }

    /// Put the panel back on the row `row_key` names with `key` highlighted. A
    /// row or entry that did not survive the re-emit leaves the panel closed.
    fn resume_panel(&mut self, row_key: &Action, key: &PanelKey) {
        let Some(index) = self
            .rows
            .iter()
            .position(|row| row.on_click.as_ref() == Some(row_key))
        else {
            return;
        };
        self.cursor.selected = index;
        let Some(row) = self.rows.get(index) else {
            return;
        };
        if row.actions.is_empty() {
            return;
        }
        let actions = row.actions.clone();
        let selected = actions
            .iter()
            .position(|action| key.matches(action))
            .unwrap_or(0);
        self.menu = Some(Menu {
            parent: index,
            actions,
            cursor: Cursor { selected, first: 0 },
        });
        self.menu_contain();
    }

    /// The action Enter would run in the open panel.
    pub fn selected_action(&self) -> Option<ActionItem> {
        self.menu.as_ref().and_then(Menu::selected_action).cloned()
    }

    /// What `Alt+Enter` on the panel's selection would remember: the owning
    /// plugin and the action id, `None` id clearing the scope. The row's own
    /// command is the implicit default, so picking it drops a remembered one.
    /// `None` when the selection cannot be a default.
    pub fn selected_default(&self) -> Option<(&str, Option<&str>)> {
        let menu = self.menu.as_ref()?;
        let action = menu.selected_action()?;
        let plugin = action.plugin.as_deref();
        if action.default {
            return Some((plugin?, None));
        }
        if let Some(id) = action.id.as_deref() {
            return Some((plugin?, Some(id)));
        }
        let row = self.rows.get(menu.parent)?;
        if !is_row_command(row, action) {
            return None;
        }
        // The row's own command is the implicit default, so picking it drops what
        // the row remembered — whichever plugin's scope remembered it.
        row.actions
            .iter()
            .find(|action| action.default)
            .and_then(|action| action.plugin.as_deref())
            .or(plugin)
            .map(|scope| (scope, None))
    }

    /// The title of the row the panel belongs to, for its header.
    pub fn menu_parent_title(&self) -> Option<&str> {
        let menu = self.menu.as_ref()?;
        self.rows.get(menu.parent).map(|row| row.title.as_str())
    }

    pub fn menu_up(&mut self) {
        if let Some(menu) = &mut self.menu {
            menu.cursor.up();
        }
        self.menu_contain();
    }

    pub fn menu_down(&mut self) {
        if let Some(menu) = &mut self.menu {
            menu.cursor.down(menu.actions.len());
        }
        self.menu_contain();
    }

    /// The wheel moves the panel's selection, like the arrows.
    pub fn menu_scroll(&mut self, rows: i32) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        if menu.actions.is_empty() || rows == 0 {
            return;
        }
        menu.cursor.scroll(rows, menu.actions.len());
        self.menu_contain();
    }

    pub fn menu_page_up(&mut self) {
        let page = self.appearance.layout.max_rows;
        if let Some(menu) = &mut self.menu {
            menu.cursor.page_up(page);
        }
        self.menu_contain();
    }

    pub fn menu_page_down(&mut self) {
        let page = self.appearance.layout.max_rows;
        if let Some(menu) = &mut self.menu {
            menu.cursor.page_down(menu.actions.len(), page);
        }
        self.menu_contain();
    }

    fn menu_contain(&mut self) {
        let layout = self.appearance.layout;
        if let Some(menu) = &mut self.menu {
            menu.cursor.contain(menu.actions.len(), layout);
        }
        self.resync_hover();
    }

    /// The pointer position and the hover it implies; returns whether the hover
    /// changed. The ✕ button sits outside the list and wins over a row.
    pub fn hover_at(&mut self, x: f32, y: f32) -> bool {
        self.pointer = Some((x, y));
        let hover = if self.clear_hit(x, y) {
            Some(Hover::Clear)
        } else if let Some(menu) = &self.menu {
            self.appearance
                .layout
                .action_at(self.surface, menu.cursor.first, menu.actions.len(), x, y)
                .map(Hover::Action)
        } else {
            self.appearance
                .layout
                .row_at(self.surface, self.cursor.first, self.rows.len(), x, y)
                .map(Hover::Row)
        };

        if hover == self.hovered {
            return false;
        }
        self.hovered = hover;
        true
    }

    /// The pointer left the surface.
    pub fn pointer_left(&mut self) {
        self.pointer = None;
        self.hovered = None;
    }

    /// Rows that move under a stationary pointer are different rows and fire no
    /// enter/exit, so re-derive the hover; `Hover::Clear` is left alone.
    fn resync_hover(&mut self) {
        let hit = self.pointer.and_then(|(x, y)| {
            if let Some(menu) = &self.menu {
                self.appearance
                    .layout
                    .action_at(self.surface, menu.cursor.first, menu.actions.len(), x, y)
                    .map(Hover::Action)
            } else {
                self.appearance
                    .layout
                    .row_at(self.surface, self.cursor.first, self.rows.len(), x, y)
                    .map(Hover::Row)
            }
        });

        match hit {
            Some(hit) => self.hovered = Some(hit),
            None if matches!(self.hovered, Some(Hover::Row(_) | Hover::Action(_))) => {
                self.hovered = None
            }
            None => {}
        }
    }

    /// The selected row's launch fields.
    pub fn selected_row(&self) -> Option<Launch> {
        let row = self.rows.get(self.cursor.selected)?;
        let target = row.on_click.clone()?;
        let effective = row
            .actions
            .iter()
            .find(|action| action.default)
            .and_then(|action| match &action.action {
                PanelAction::Execute { command } => Some(command.clone()),
                _ => None,
            })
            .unwrap_or_else(|| target.clone());
        Some(Launch {
            title: row.title.clone(),
            summary: row.summary.clone(),
            icon: row.icon.clone(),
            target,
            effective,
            ephemeral: row.ephemeral,
        })
    }

    pub fn retarget_height(&mut self, now: Instant) {
        let target = self.content_height();
        if target != self.card_to {
            self.card_from = self.card_height(now);
            self.card_to = target;
            self.card_at = now;
        }
    }

    /// The selected byte range, `None` when the caret is a bare point.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let (start, end) = if anchor < self.caret {
            (anchor, self.caret)
        } else {
            (self.caret, anchor)
        };
        (start != end).then_some((start, end))
    }

    /// Ctrl+A.
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.query.len();
    }

    /// Remove the selected range, if any; whether it removed something. The
    /// anchor is always cleared: this is also what collapses a selection.
    pub fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            self.anchor = None;
            return false;
        };

        self.query.replace_range(start..end, "");
        self.caret = start;
        self.anchor = None;
        true
    }

    /// Typing over a selection replaces it, as it does in any text field.
    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        self.caret = self.caret.min(self.query.len());
        self.query.insert_str(self.caret, text);
        self.caret += text.len();
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if let Some(at) = prev_boundary(&self.query, self.caret) {
            self.query.remove(at);
            self.caret = at;
        }
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.caret < self.query.len() {
            self.query.remove(self.caret);
        }
    }

    /// `delete_surrounding_text` lengths are UTF-8 *bytes*: remove characters
    /// while they fit, stop at the ends, so a bogus length cannot spin.
    pub fn delete_surrounding(&mut self, before: u32, after: u32) {
        // the IME is about to replace whatever is selected
        self.delete_selection();
        self.caret = self.caret.min(self.query.len());

        let mut budget = before as usize;
        while budget > 0 {
            let Some(at) = prev_boundary(&self.query, self.caret) else {
                break;
            };
            let len = self.caret - at;
            if len > budget {
                break;
            }
            budget -= len;
            self.backspace();
        }

        let mut budget = after as usize;
        while budget > 0 {
            let Some(len) = next_char_len(&self.query, self.caret) else {
                break;
            };
            if len > budget {
                break;
            }
            budget -= len;
            self.delete();
        }
    }

    /// Home/End are deliberately not intercepted as widget shortcuts: they
    /// belong to the caret.
    pub fn home(&mut self) {
        self.anchor = None;
        self.caret = 0;
    }

    pub fn end(&mut self) {
        self.anchor = None;
        self.caret = self.query.len();
    }

    pub fn left(&mut self) {
        // with a selection, a bare arrow collapses to the end it moves towards
        if let Some((start, _)) = self.selection() {
            self.anchor = None;
            self.caret = start;
            return;
        }
        self.anchor = None;
        if let Some(at) = prev_boundary(&self.query, self.caret) {
            self.caret = at;
        }
    }

    pub fn right(&mut self) {
        if let Some((_, end)) = self.selection() {
            self.anchor = None;
            self.caret = end;
            return;
        }
        self.anchor = None;
        if let Some(len) = next_char_len(&self.query, self.caret) {
            self.caret += len;
        }
    }

    /// Shift+Left/Right/Home/End extend from wherever the caret sat when the
    /// shift was first held.
    pub fn extend_left(&mut self) {
        self.anchor.get_or_insert(self.caret);
        if let Some(at) = prev_boundary(&self.query, self.caret) {
            self.caret = at;
        }
    }

    pub fn extend_right(&mut self) {
        self.anchor.get_or_insert(self.caret);
        if let Some(len) = next_char_len(&self.query, self.caret) {
            self.caret += len;
        }
    }

    pub fn extend_home(&mut self) {
        self.anchor.get_or_insert(self.caret);
        self.caret = 0;
    }

    pub fn extend_end(&mut self) {
        self.anchor.get_or_insert(self.caret);
        self.caret = self.query.len();
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.caret = 0;
        self.anchor = None;
    }

    /// Whether a surface-local point is on the ✕ button, whose circle comes from
    /// `Layout::clear_circle` so the hit test cannot drift from what is drawn.
    pub fn clear_hit(&self, x: f32, y: f32) -> bool {
        if self.query.is_empty() {
            return false;
        }

        let (cx, cy, r) = self.appearance.layout.clear_circle(self.surface);
        (x - cx).abs() <= r && (y - cy).abs() <= r
    }

    /// The text before the caret, which is what the IME wants as surrounding
    /// text (only its byte length is reported, since nothing tracks it).
    pub fn before_caret(&self) -> &str {
        &self.query[..self.caret]
    }

    /// Drop a row the core confirmed it forgot, looked up by its command key
    /// because the payload may have been replaced while the reply was in flight.
    pub fn remove_row(&mut self, key: &str, now: Instant) -> bool {
        let Some(index) = self.rows.iter().position(|row| {
            row.on_click
                .as_ref()
                .is_some_and(|command| command.key() == key)
        }) else {
            return false;
        };

        self.rows.remove(index);
        self.cursor.selected = self.cursor.selected.min(self.rows.len().saturating_sub(1));
        // The panel was about the list that just changed under it.
        self.menu = None;
        self.contain();
        self.retarget_height(now);
        true
    }

    pub fn apply_results(&mut self, items: Vec<ResultItem>, now: Instant) {
        let items = normalize_icons(items);
        // A panel action holds its place across the reply it asked for.
        let resume = self.panel_resume.take();
        // An identical re-send is dropped so it cannot reset the selection.
        if self.rows == items && !self.rows.is_empty() {
            return;
        }

        self.rows = items;

        // A fresh payload starts at the top row, and any open panel is stale; a
        // local removal and an identical re-send keep the cursor.
        self.cursor.selected = 0;
        self.menu = None;
        if let Some((row_key, key)) = resume {
            self.resume_panel(&row_key, &key);
        }
        self.contain();
        self.retarget_height(now);
    }

    /// Move the selection by whole rows (the wheel): the highlighted row is what
    /// Enter launches, so the selection moves, not just the window.
    pub fn scroll(&mut self, rows: i32) {
        if self.rows.is_empty() || rows == 0 {
            return;
        }
        self.cursor.scroll(rows, self.rows.len());
        self.contain();
    }

    pub fn up(&mut self) {
        self.cursor.up();
        self.contain();
    }

    pub fn down(&mut self) {
        self.cursor.down(self.rows.len());
        self.contain();
    }

    pub fn page_up(&mut self) {
        self.cursor.page_up(self.appearance.layout.max_rows);
        self.contain();
    }

    pub fn page_down(&mut self) {
        self.cursor
            .page_down(self.rows.len(), self.appearance.layout.max_rows);
        self.contain();
    }

    /// The active keyword prefix (`b`, `h`, `f`, …): `^([a-zA-Z]{1,3})\s`.
    pub fn keyword_prefix(&self) -> Option<&str> {
        let end = self
            .query
            .as_bytes()
            .iter()
            .take(4)
            .position(|byte| *byte == b' ')?;
        if end == 0
            || !self.query.as_bytes()[..end]
                .iter()
                .all(u8::is_ascii_alphabetic)
        {
            return None;
        }

        Some(&self.query[..end])
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_out_quint(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(5)
}

/// The byte index where the character before `at` starts; `at` must be a char
/// boundary, and the start of the text has nothing before it.
fn prev_boundary(text: &str, at: usize) -> Option<usize> {
    text[..at]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
}

/// The UTF-8 length of the character at `at`, or `None` at the end.
fn next_char_len(text: &str, at: usize) -> Option<usize> {
    text[at..].chars().next().map(char::len_utf8)
}

/// Whole rows from a fractional wheel delta, carrying the remainder. One notch
/// arrives as a line and its pixel half, so pixels must not add a second row.
pub fn whole_rows(accum: &mut f32, delta: f32) -> i32 {
    // A reversal starts a new gesture: without this the previous direction's
    // slack would swallow the first notch back.
    if accum.signum() * delta.signum() < 0.0 {
        *accum = 0.0;
    }

    *accum += delta;
    let whole = accum.trunc();
    *accum -= whole;
    whole as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::geom::{self, Layout};
    use wayrun_core::wire::{PanelAction, ResultItem};

    fn run(cmd: &str) -> Action {
        Action::Run {
            cmd: cmd.to_string(),
        }
    }

    fn item(
        title: &str,
        summary: Option<&str>,
        on_click: Option<Action>,
        icon: Option<&str>,
    ) -> ResultItem {
        ResultItem {
            title: title.into(),
            summary: summary.map(str::to_string),
            on_click,
            icon: icon.map(str::to_string),
            ephemeral: false,
            actions: Vec::new(),
            badge: None,
        }
    }

    fn state() -> State {
        let mut state = State::new();
        state.surface = (1920, 1080);
        state
    }

    #[test]
    fn boundaries_are_char_steps_not_bytes() {
        let text = "aé中";
        assert_eq!(prev_boundary(text, text.len()), Some(3));
        assert_eq!(prev_boundary(text, 3), Some(1));
        assert_eq!(prev_boundary(text, 1), Some(0));
        assert_eq!(prev_boundary(text, 0), None);

        assert_eq!(next_char_len(text, 0), Some(1));
        assert_eq!(next_char_len(text, 1), Some(2));
        assert_eq!(next_char_len(text, 3), Some(3));
        assert_eq!(next_char_len(text, text.len()), None);
    }

    #[test]
    fn contain_moves_only_as_far_as_the_selection_needs() {
        let l = Layout::default();
        let max = l.max_rows;
        assert_eq!(l.contain(0, 0, 20), 0);
        assert_eq!(l.contain(max - 1, 0, 20), 0);
        // stepping past the window scrolls by exactly one row
        assert_eq!(l.contain(max, 0, 20), 1);
        assert_eq!(l.contain(max + 1, 1, 20), 2);
        // moving up *inside* the window keeps it put
        assert_eq!(l.contain(3, 1, 20), 1);
        // leaving it upwards follows the selection
        assert_eq!(l.contain(0, 3, 20), 0);
        // the window never runs past the last row
        assert_eq!(l.contain(19, 0, 20), 15);
        assert_eq!(l.contain(19, 17, 20), 15);
        // a list shorter than the window cannot scroll
        assert_eq!(l.contain(0, 5, 3), 0);
        assert_eq!(l.contain(2, 5, 3), 0);
    }

    #[test]
    fn an_appearance_change_rebuilds_the_theme_and_geometry() {
        let mut state = state();
        let now = std::time::Instant::now();

        let mut config = AppearanceConfig::default();
        config.colors.shared.fg = Some([0x10, 0x20, 0x30]);
        config.layout.max_rows = 2;
        state.set_system_theme(
            Theme {
                primary: [1, 2, 3],
                fg: [4, 5, 6],
                container: [7, 8, 9],
            },
            Mode::Dark,
        );
        state.apply_appearance(config, now);

        assert_eq!(state.theme.primary, [1, 2, 3]);
        assert_eq!(state.theme.fg, [0x10, 0x20, 0x30]);
        assert_eq!(state.theme.container, [7, 8, 9]);
        assert_eq!(state.surfaces.dim, crate::ui::theme::DEFAULT_DIM);
        assert_eq!(state.card_to, state.appearance.layout.content_h(0));
    }

    #[test]
    fn the_env_reduced_motion_flag_survives_a_config_reload() {
        let mut state = state();
        state.reduced_env = true;
        state.reduce_motion = true;
        let config = AppearanceConfig {
            reduced: false,
            ..AppearanceConfig::default()
        };
        state.apply_appearance(config, std::time::Instant::now());
        assert!(state.reduce_motion);
    }

    #[test]
    fn a_wheel_notch_moves_exactly_one_row() {
        // the line delta and its pixel twin both arrive for one notch
        let mut accum = 0.0;
        assert_eq!(whole_rows(&mut accum, 1.0), 1);
        assert_eq!(whole_rows(&mut accum, -2.0 / 64.0), 0);
        // a trackpad accumulates pixels into whole rows
        let mut accum = 0.0;
        assert_eq!(whole_rows(&mut accum, 0.5), 0);
        assert_eq!(whole_rows(&mut accum, 0.5), 1);
        // a reversal drops the previous gesture's slack
        let mut accum = -1.0;
        assert_eq!(whole_rows(&mut accum, 0.5), 0);
        assert_eq!(whole_rows(&mut accum, 1.0), 1);
    }

    #[test]
    fn the_result_dedupe_matches_the_payload_it_was_built_from() {
        let mut state = state();
        let items = vec![
            item("Files", None, None, None),
            item(
                "Firefox",
                Some("Browser"),
                Some(Action::Launch {
                    desktop_id: "firefox.desktop".to_string(),
                }),
                Some("/i.svg"),
            ),
        ];
        let now = std::time::Instant::now();
        state.apply_results(items.clone(), now);

        state.cursor.selected = 1;
        // an identical re-send must not reset the selection
        state.apply_results(items.clone(), now);
        assert_eq!(state.cursor.selected, 1);

        let changed = vec![
            item("Files", None, None, None),
            item(
                "Firefox",
                Some("Browser"),
                Some(run("firefox")),
                Some("/i.svg"),
            ),
        ];
        state.apply_results(changed, now);
        // a genuinely new payload starts from the top row: what the cursor
        // pointed at has changed
        assert_eq!(state.cursor.selected, 0);
        assert_eq!(state.rows[1].on_click.as_ref(), Some(&run("firefox")));
    }

    #[test]
    fn a_moving_list_keeps_the_hover_on_the_row_under_the_pointer() {
        let mut state = state();
        let items: Vec<ResultItem> = (0..20)
            .map(|i| item(&format!("row {i}"), None, Some(run("x")), None))
            .collect();
        state.apply_results(items, std::time::Instant::now());

        // the pointer sits on the second visible row
        let layout = state.appearance.layout;
        let x = layout.card_x(state.surface) + 10.0;
        let y = layout.rows_top(state.surface) + geom::ROW_H + 1.0;
        assert!(state.hover_at(x, y));
        assert_eq!(state.hovered, Some(Hover::Row(1)));

        // a page turn draws different rows under the same pointer: the hover has
        // to follow the row now under it, which is one further down the list
        state.cursor.selected = layout.max_rows;
        state.contain();
        assert_eq!(state.cursor.first, 1);
        assert_eq!(state.hovered, Some(Hover::Row(2)));

        // and the ✕ button's hover is not a row, so a page turn leaves it alone
        state.query = "q".into();
        let clear = (
            layout.card_x(state.surface) + layout.card_w(state.surface) - geom::PAD - 8.0 - 13.0,
            layout.card_top(state.surface) + geom::PAD + geom::SEARCH_H / 2.0,
        );
        assert!(state.hover_at(clear.0, clear.1));
        assert_eq!(state.hovered, Some(Hover::Clear));
        state.contain();
        assert_eq!(state.hovered, Some(Hover::Clear));

        state.pointer_left();
        assert_eq!(state.hovered, None);
    }

    #[test]
    fn editing_is_utf8_safe() {
        let mut state = state();
        state.insert("中文");
        state.insert("ab");
        assert_eq!(state.query, "中文ab");
        assert_eq!(state.caret, "中文ab".len());

        // backspace removes a whole character, not a byte
        state.backspace();
        state.backspace();
        assert_eq!(state.query, "中文");

        state.home();
        assert_eq!(state.caret, 0);
        state.delete();
        assert_eq!(state.query, "文");
        state.end();
        assert_eq!(state.caret, "文".len());

        // the caret never leaves the string
        state.left();
        state.left();
        assert_eq!(state.caret, 0);
        state.right();
        state.right();
        assert_eq!(state.caret, "文".len());

        state.clear_query();
        assert!(state.query.is_empty() && state.caret == 0);
    }

    #[test]
    fn deleting_surrounding_text_counts_bytes_not_characters() {
        let mut state = state();

        // one CJK character is three bytes: a three-byte request before the
        // caret removes exactly that character
        state.insert("中文");
        state.delete_surrounding(3, 0);
        assert_eq!(state.query, "中");
        assert_eq!(state.caret, "中".len());

        // ASCII is one byte a character
        state.insert("ab");
        state.delete_surrounding(1, 0);
        assert_eq!(state.query, "中a");
        state.delete_surrounding(1, 0);
        assert_eq!(state.query, "中");

        // a character that does not fit the budget is left alone: the request is
        // never exceeded, so no user text disappears for a one-byte ask
        state.delete_surrounding(1, 0);
        assert_eq!(state.query, "中");

        // forward deletion counts bytes the same way
        state.caret = 0;
        state.delete_surrounding(0, 3);
        assert_eq!(state.query, "");

        // a bogus length is clamped by the ends of the query, not looped
        state.insert("x");
        state.delete_surrounding(u32::MAX, u32::MAX);
        assert_eq!(state.query, "");
        assert_eq!(state.caret, 0);
    }

    #[test]
    fn ctrl_a_selects_the_query_and_editing_replaces_it() {
        let mut state = state();
        state.insert("firefox");
        state.select_all();
        assert_eq!(state.selection(), Some((0, 7)));

        // typing over a selection replaces it
        state.insert("kit");
        assert_eq!(state.query, "kit");
        assert_eq!(state.caret, 3);
        assert_eq!(state.selection(), None);

        // backspace over a selection removes all of it, not one character
        state.insert("ty");
        state.select_all();
        state.backspace();
        assert!(state.query.is_empty(), "{:?}", state.query);

        // a bare arrow collapses the selection instead of moving the caret
        state.insert("abcdef");
        state.select_all();
        state.left();
        assert_eq!((state.caret, state.selection()), (0, None));
        state.select_all();
        state.right();
        assert_eq!((state.caret, state.selection()), (6, None));
    }

    #[test]
    fn shift_arrows_extend_a_selection_from_the_anchor() {
        let mut state = state();
        state.insert("abc");
        state.home();

        state.extend_right();
        state.extend_right();
        assert_eq!(state.selection(), Some((0, 2)));
        // walking back to the anchor leaves no selection at all
        state.extend_left();
        assert_eq!(state.selection(), Some((0, 1)));
        state.extend_left();
        assert_eq!(state.selection(), None);

        // anchored in the middle, extending the other way flips the ends
        state.clear_query();
        state.insert("abc");
        state.home();
        state.right();
        state.right();
        state.extend_home();
        assert_eq!(state.selection(), Some((0, 2)));
        state.extend_right();
        assert_eq!(state.selection(), Some((1, 2)));
        state.extend_right();
        assert_eq!(state.selection(), None, "back at the anchor");
    }

    #[test]
    fn a_row_is_only_removed_when_the_core_confirms_the_forget() {
        let mut state = state();
        let items = vec![
            item(
                "Files",
                None,
                Some(Action::Launch {
                    desktop_id: "files.desktop".to_string(),
                }),
                None,
            ),
            item(
                "Firefox",
                None,
                Some(Action::Launch {
                    desktop_id: "firefox.desktop".to_string(),
                }),
                None,
            ),
        ];
        let now = std::time::Instant::now();
        state.apply_results(items, now);
        state.cursor.selected = 1;
        assert_eq!(
            state.rows[1].on_click.as_ref(),
            Some(&Action::Launch {
                desktop_id: "firefox.desktop".to_string()
            })
        );

        // "nothing was dropped" (a provider that implements no forget): the row
        // stays exactly where it is
        let other = Action::Launch {
            desktop_id: "other.desktop".to_string(),
        }
        .key();
        assert!(!state.remove_row(&other, now));
        assert_eq!(state.rows.len(), 2);

        // a confirmed forget takes that row out and keeps the selection valid
        let firefox = Action::Launch {
            desktop_id: "firefox.desktop".to_string(),
        }
        .key();
        assert!(state.remove_row(&firefox, now));
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].title, "Files");
        assert_eq!(state.cursor.selected, 0);
    }

    #[test]
    fn the_fractional_scale_overrides_the_integer_one() {
        let mut state = state();
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
    fn a_keyword_prefix_is_one_to_three_letters_and_a_space() {
        let mut state = state();
        state.query = "b firefox".into();
        assert_eq!(state.keyword_prefix(), Some("b"));
        state.query = "tr 你好".into();
        assert_eq!(state.keyword_prefix(), Some("tr"));
        state.query = "abcd ".into();
        assert_eq!(state.keyword_prefix(), None);
        state.query = "1 x".into();
        assert_eq!(state.keyword_prefix(), None);
        state.query = "b".into();
        assert_eq!(state.keyword_prefix(), None);
    }

    #[test]
    fn nothing_is_launched_without_a_target() {
        let mut state = state();
        let items = vec![item("Files", None, None, None)];
        state.apply_results(items, std::time::Instant::now());
        assert_eq!(state.selected_row(), None);
    }

    #[test]
    fn the_rows_own_command_clears_a_remembered_default() {
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        let execute =
            |title: &str, command: Action, id: Option<&str>, plugin: Option<&str>| ActionItem {
                title: title.to_string(),
                action: PanelAction::Execute { command },
                icon: None,
                id: id.map(str::to_string),
                plugin: plugin.map(str::to_string),
                default: false,
            };
        let mut row = item(
            "a.txt",
            None,
            Some(Action::Open {
                uri: uri.to_string(),
            }),
            None,
        );
        let mut terminal = execute(
            "Open in terminal",
            Action::Terminal {
                uri: uri.to_string(),
            },
            Some("terminal"),
            Some("file-search"),
        );
        terminal.default = true;
        row.actions = vec![
            execute(
                "Open",
                Action::Open {
                    uri: uri.to_string(),
                },
                None,
                Some("file-search"),
            ),
            execute(
                "Copy path",
                Action::Run {
                    cmd: "true".to_string(),
                },
                Some("copy_path"),
                Some("file-search"),
            ),
            terminal,
            ActionItem {
                title: "Pin to top".to_string(),
                action: PanelAction::Pin {
                    scope: String::new(),
                    item: Box::new(item("a.txt", None, None, None)),
                },
                icon: None,
                id: None,
                plugin: None,
                default: false,
            },
        ];
        state.apply_results(vec![row], std::time::Instant::now());
        assert!(state.open_actions());

        fn remember(state: &mut State, selected: usize) -> Option<(String, Option<String>)> {
            state.menu.as_mut().unwrap().cursor.selected = selected;
            state
                .selected_default()
                .map(|(plugin, id)| (plugin.to_string(), id.map(str::to_string)))
        }
        // the row's own command is the implicit default: picking it clears the
        // scope, so a row can always go back to opening normally
        assert_eq!(
            remember(&mut state, 0),
            Some(("file-search".to_string(), None))
        );
        // a plugin action is remembered by id, and re-picking the remembered one
        // clears it
        assert_eq!(
            remember(&mut state, 1),
            Some(("file-search".to_string(), Some("copy_path".to_string())))
        );
        assert_eq!(
            remember(&mut state, 2),
            Some(("file-search".to_string(), None))
        );
        // a launcher-level action belongs to no plugin
        assert_eq!(remember(&mut state, 3), None);
    }

    #[test]
    fn enter_uses_the_default_action_but_records_the_row_command() {
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        let mut row = item(
            "a.txt",
            None,
            Some(Action::Open {
                uri: uri.to_string(),
            }),
            None,
        );
        row.actions = vec![ActionItem {
            title: "Open in terminal".to_string(),
            action: PanelAction::Execute {
                command: Action::Terminal {
                    uri: uri.to_string(),
                },
            },
            icon: None,
            id: Some("terminal".to_string()),
            plugin: Some("file-search".to_string()),
            default: true,
        }];
        state.apply_results(vec![row], std::time::Instant::now());

        let launch = state.selected_row().unwrap();
        // what Enter runs
        assert_eq!(
            launch.effective,
            Action::Terminal {
                uri: uri.to_string()
            }
        );
        // what usage records, so history and forget stay keyed to the row
        assert_eq!(
            launch.target,
            Action::Open {
                uri: uri.to_string()
            }
        );
    }

    #[test]
    fn an_ephemeral_row_is_forwarded_when_selected() {
        let mut state = state();
        let mut row = item(
            "repo",
            None,
            Some(Action::Open {
                uri: "https://x".to_string(),
            }),
            None,
        );
        row.ephemeral = true;
        state.apply_results(vec![row], std::time::Instant::now());
        assert!(state.selected_row().unwrap().ephemeral);
    }

    fn with_actions(mut row: ResultItem, titles: &[&str]) -> ResultItem {
        row.actions = titles
            .iter()
            .map(|title| ActionItem {
                title: title.to_string(),
                action: PanelAction::Execute {
                    command: run(title),
                },
                icon: None,
                id: None,
                plugin: None,
                default: false,
            })
            .collect();
        row
    }

    #[test]
    fn the_panel_opens_over_the_selected_row_and_runs_its_action() {
        let mut state = state();
        let now = std::time::Instant::now();
        state.apply_results(
            vec![
                item(
                    "Files",
                    None,
                    Some(Action::Open {
                        uri: "file:///tmp".to_string(),
                    }),
                    None,
                ),
                item(
                    "Firefox",
                    None,
                    Some(Action::Launch {
                        desktop_id: "firefox.desktop".to_string(),
                    }),
                    None,
                ),
            ],
            now,
        );
        state.cursor.selected = 1;
        state.rows[1].actions = vec![ActionItem {
            title: "Pin to top".to_string(),
            action: PanelAction::Pin {
                scope: String::new(),
                item: Box::new(item("Firefox", None, None, None)),
            },
            icon: None,
            id: None,
            plugin: None,
            default: false,
        }];

        assert!(state.open_actions());
        assert_eq!(state.menu_parent_title(), Some("Firefox"));
        assert_eq!(state.selected_action().unwrap().title, "Pin to top");
        // the panel is taller than the one-row list it replaced
        assert_eq!(state.content_height(), state.appearance.layout.panel_h(1));

        state.close_actions();
        assert!(state.menu.is_none());
        assert_eq!(state.content_height(), state.appearance.layout.content_h(2));
    }

    #[test]
    fn a_pinned_row_keeps_its_badge_and_offers_unpin() {
        let mut state = state();
        let mut row = item(
            "YouTube",
            None,
            Some(Action::Open {
                uri: "https://youtube.com/".to_string(),
            }),
            None,
        );
        row.badge = Some("/usr/share/icons/Papirus/24x24/actions/pin.svg".to_string());
        row.actions = vec![ActionItem {
            title: "Unpin".to_string(),
            action: PanelAction::Unpin {
                scope: "b youtube".to_string(),
                on_click: Action::Open {
                    uri: "https://youtube.com/".to_string(),
                },
            },
            icon: None,
            id: None,
            plugin: None,
            default: false,
        }];
        state.apply_results(vec![row], std::time::Instant::now());

        assert!(state.rows[0].badge.is_some());
        assert!(state.open_actions());
        assert_eq!(state.selected_action().unwrap().title, "Unpin");
    }

    #[test]
    fn a_row_without_actions_has_no_panel() {
        let mut state = state();
        state.apply_results(
            vec![item(
                "Files",
                None,
                Some(Action::Open {
                    uri: "file:///tmp".to_string(),
                }),
                None,
            )],
            std::time::Instant::now(),
        );
        assert!(!state.open_actions());
        assert!(state.menu.is_none());
    }

    #[test]
    fn a_new_payload_closes_the_panel() {
        let mut state = state();
        let now = std::time::Instant::now();
        state.apply_results(
            vec![with_actions(
                item("a", None, Some(run("a")), None),
                &["Pin to top"],
            )],
            now,
        );
        assert!(state.open_actions());

        state.apply_results(vec![item("b", None, Some(run("b")), None)], now);
        assert!(state.menu.is_none(), "a new list invalidates the panel");
    }

    /// A file row: the row's own command, with the panel entries a plugin and
    /// the launcher put on it.
    fn file_row(uri: &str, actions: Vec<ActionItem>) -> ResultItem {
        let mut row = item(
            "a.txt",
            None,
            Some(Action::Open {
                uri: uri.to_string(),
            }),
            None,
        );
        row.actions = actions;
        row
    }

    fn plugin_action(title: &str, command: Action, id: &str, default: bool) -> ActionItem {
        ActionItem {
            title: title.to_string(),
            action: PanelAction::Execute { command },
            icon: None,
            id: Some(id.to_string()),
            plugin: Some("file-search".to_string()),
            default,
        }
    }

    fn pin_entry(uri: &str, unpin: bool) -> ActionItem {
        let on_click = Action::Open {
            uri: uri.to_string(),
        };
        let action = if unpin {
            PanelAction::Unpin {
                scope: "f a".to_string(),
                on_click,
            }
        } else {
            PanelAction::Pin {
                scope: "f a".to_string(),
                item: Box::new(file_row(uri, Vec::new())),
            }
        };
        ActionItem {
            title: if unpin { "Unpin" } else { "Pin to top" }.to_string(),
            action,
            icon: None,
            id: None,
            plugin: None,
            default: false,
        }
    }

    #[test]
    fn the_panel_dots_the_entry_enter_runs() {
        let uri = "file:///tmp/a.txt";
        let open = ActionItem {
            title: "Open".to_string(),
            action: PanelAction::Execute {
                command: Action::Open {
                    uri: uri.to_string(),
                },
            },
            icon: None,
            id: None,
            plugin: Some("file-search".to_string()),
            default: false,
        };
        let reveal = plugin_action(
            "Reveal in file manager",
            Action::Reveal {
                uri: uri.to_string(),
            },
            "reveal",
            false,
        );
        let terminal = |default| {
            plugin_action(
                "Open in terminal",
                Action::Terminal {
                    uri: uri.to_string(),
                },
                "terminal",
                default,
            )
        };

        let mut row = file_row(uri, vec![open.clone(), reveal.clone(), terminal(false)]);
        assert_eq!(
            marked_entry(&row),
            Some(0),
            "no remembered default: the row's own command is what Enter runs"
        );

        row.actions = vec![open, reveal, terminal(true)];
        assert_eq!(
            marked_entry(&row),
            Some(2),
            "the remembered action carries it"
        );
    }

    #[test]
    fn picking_the_row_command_clears_the_scope_that_remembered_it() {
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        let mut row = item(
            "a.txt",
            None,
            Some(Action::Open {
                uri: uri.to_string(),
            }),
            None,
        );
        // A host row: its actions carry the host's scope, and the leading Open
        // entry carries none of its own.
        row.actions = vec![
            ActionItem {
                title: "Open".to_string(),
                action: PanelAction::Execute {
                    command: Action::Open {
                        uri: uri.to_string(),
                    },
                },
                icon: None,
                id: None,
                plugin: None,
                default: false,
            },
            ActionItem {
                title: "Mark done".to_string(),
                action: PanelAction::Execute {
                    command: Action::Run {
                        cmd: "done".to_string(),
                    },
                },
                icon: None,
                id: Some("done".to_string()),
                plugin: Some("todo".to_string()),
                default: true,
            },
        ];
        state.apply_results(vec![row], std::time::Instant::now());
        assert!(state.open_actions());

        assert_eq!(
            state.selected_default(),
            Some(("todo", None)),
            "the scope that remembered the action is the one to clear"
        );
    }

    #[test]
    fn a_default_gesture_keeps_the_panel_on_the_entry_it_marked() {
        let now = std::time::Instant::now();
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        let reveal = |default| {
            plugin_action(
                "Reveal in file manager",
                Action::Reveal {
                    uri: uri.to_string(),
                },
                "reveal",
                default,
            )
        };
        let terminal = |default| {
            plugin_action(
                "Open in terminal",
                Action::Terminal {
                    uri: uri.to_string(),
                },
                "terminal",
                default,
            )
        };
        state.apply_results(
            vec![file_row(uri, vec![reveal(false), terminal(false)])],
            now,
        );
        assert!(state.open_actions());
        state.menu.as_mut().unwrap().cursor.selected = 1;

        // Alt+Enter on "Open in terminal": the reply carries the dot and the
        // panel must land back on that entry, not on the top of the list.
        let chosen = state.selected_action().unwrap();
        state.keep_panel(&chosen);
        state.apply_results(
            vec![file_row(uri, vec![reveal(false), terminal(true)])],
            now,
        );

        let menu = state.menu.as_ref().expect("the panel comes back");
        assert_eq!(menu.cursor.selected, 1);
        assert!(
            menu.actions[1].default,
            "the highlighted entry is the marked one"
        );
        assert_eq!(state.cursor.selected, 0, "the panel's row is the selection");
    }

    #[test]
    fn a_pin_gesture_keeps_the_panel_on_the_entry_it_flipped() {
        let now = std::time::Instant::now();
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        state.apply_results(vec![file_row(uri, vec![pin_entry(uri, false)])], now);
        assert!(state.open_actions());

        let chosen = state.selected_action().unwrap();
        state.keep_panel(&chosen);
        state.apply_results(vec![file_row(uri, vec![pin_entry(uri, true)])], now);

        let menu = state.menu.as_ref().expect("the panel comes back");
        assert_eq!(menu.actions[menu.cursor.selected].title, "Unpin");
    }

    #[test]
    fn a_panel_gesture_on_a_vanished_row_leaves_the_panel_closed() {
        let now = std::time::Instant::now();
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        state.apply_results(vec![file_row(uri, vec![pin_entry(uri, false)])], now);
        assert!(state.open_actions());

        let chosen = state.selected_action().unwrap();
        state.keep_panel(&chosen);
        state.apply_results(vec![item("b", None, Some(run("b")), None)], now);
        assert!(state.menu.is_none(), "the panel's row is gone");
    }

    #[test]
    fn a_resume_dropped_before_the_reply_lands_leaves_the_panel_closed() {
        let now = std::time::Instant::now();
        let mut state = state();
        let uri = "file:///tmp/a.txt";
        let row = file_row(uri, vec![pin_entry(uri, false)]);
        state.apply_results(vec![row.clone()], now);
        assert!(state.open_actions());

        // the row is still there and the payload changed, so only the dropped
        // resume keeps the panel closed — a dismissal or an edit must not let a
        // slow reply resurrect it
        let chosen = state.selected_action().unwrap();
        state.keep_panel(&chosen);
        state.hidden();
        state.apply_results(vec![file_row(uri, vec![pin_entry(uri, true)])], now);
        assert!(state.menu.is_none());

        state.apply_results(vec![row], now);
        assert!(state.open_actions());
        let chosen = state.selected_action().unwrap();
        state.keep_panel(&chosen);
        state.cancel_panel_resume();
        state.apply_results(vec![file_row(uri, vec![pin_entry(uri, true)])], now);
        assert!(state.menu.is_none(), "an edit cancels the pending resume");
    }

    #[test]
    fn the_panel_window_follows_the_selection_past_max_rows() {
        let mut state = state();
        let now = std::time::Instant::now();
        let actions: Vec<&str> = (0..10).map(|_| "act").collect();
        state.apply_results(
            vec![with_actions(
                item("a", None, Some(run("a")), None),
                &actions,
            )],
            now,
        );
        assert!(state.open_actions());

        let max = state.appearance.layout.max_rows;
        state.menu_down();
        state.menu_down();
        state.menu_down();
        state.menu_down();
        state.menu_down();
        assert_eq!(state.menu.as_ref().unwrap().cursor.selected, 5);
        assert_eq!(state.menu.as_ref().unwrap().cursor.first, 5 - max + 1);

        state.menu_page_up();
        assert_eq!(state.menu.as_ref().unwrap().cursor.selected, 5 - max);
    }
}
