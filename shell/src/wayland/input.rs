use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};

use smithay_client_toolkit::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use wayland_client::protocol::{wl_keyboard, wl_pointer, wl_seat, wl_surface};
use wayland_client::{Connection, QueueHandle};

use crate::app;
use crate::session::{backend, clipboard};
use crate::ui::geom;
use wayrun_core::wire::{Action, ActionItem, PanelAction};

use super::{Shell, ime};

/// The wheel's pixels-per-row for the pixel half of a notch.
const WHEEL_ROW_PX: f32 = geom::ROW_H;

impl Shell {
    fn key(&mut self, event: &KeyEvent, now: Instant) {
        let control = self.modifiers.ctrl;
        // Shift extends a selection instead of collapsing it, and opens the
        // action panel on Enter.
        let shift = self.modifiers.shift;

        if control {
            match event.keysym {
                Keysym::v => {
                    // The read is blocking, so it happens on a worker thread and
                    // comes back through `on_paste`.
                    clipboard::read(self.paste_tx.clone(), self.paste_generation);
                    return;
                }
                Keysym::a => {
                    self.app.select_all();
                    self.redraw();
                    return;
                }
                // The shell owns no clipboard: Ctrl+C/X hand the selection to the
                // core's `copy` verb, which writes it with `wl-copy`.
                Keysym::c => {
                    self.copy_selection();
                    return;
                }
                Keysym::x => {
                    if self.copy_selection() {
                        self.app.delete_selection();
                        self.redraw();
                        self.query_changed();
                    }
                    return;
                }
                _ => {}
            }
        }

        // The action panel owns the keyboard while open; a key it does not use
        // dismisses it and falls through to the field.
        if self.app.menu.is_some() {
            match event.keysym {
                Keysym::Escape => {
                    self.close_panel(now);
                    return;
                }
                Keysym::Return | Keysym::KP_Enter if shift => {
                    self.close_panel(now);
                    return;
                }
                Keysym::Return | Keysym::KP_Enter => {
                    self.run_action(now);
                    return;
                }
                Keysym::Up => {
                    self.app.menu_up();
                    self.redraw();
                    return;
                }
                Keysym::Down => {
                    self.app.menu_down();
                    self.redraw();
                    return;
                }
                Keysym::Page_Up => {
                    self.app.menu_page_up();
                    self.redraw();
                    return;
                }
                Keysym::Page_Down => {
                    self.app.menu_page_down();
                    self.redraw();
                    return;
                }
                // A modifier on its own is not a command. Closing here would let
                // the Shift of a Shift+Enter undo the close it is part of.
                _ if event.keysym.is_modifier_key() => return,
                _ => self.close_panel(now),
            }
        }

        // `redraw` is "something visible changed"; `edited` is narrower, and only
        // a query change may send a search, so navigation never re-searches.
        let mut redraw = true;
        let mut edited = false;
        match event.keysym {
            Keysym::Escape => {
                self.dismiss(now);
                return;
            }
            Keysym::Return | Keysym::KP_Enter => {
                if shift {
                    self.open_panel(now);
                } else {
                    self.submit(now);
                }
                return;
            }
            Keysym::BackSpace => {
                let before = self.app.query.len();
                self.app.backspace();
                edited = self.app.query.len() != before;
            }
            Keysym::Delete => {
                let before = self.app.query.len();
                self.app.delete();
                edited = self.app.query.len() != before;
            }
            Keysym::Home if shift => self.app.extend_home(),
            Keysym::Home => self.app.home(),
            Keysym::End if shift => self.app.extend_end(),
            Keysym::End => self.app.end(),
            Keysym::Left if shift => self.app.extend_left(),
            Keysym::Left => self.app.left(),
            Keysym::Right if shift => self.app.extend_right(),
            Keysym::Right => self.app.right(),
            Keysym::Up => self.app.up(),
            Keysym::Down => self.app.down(),
            Keysym::Page_Up => self.app.page_up(),
            Keysym::Page_Down => self.app.page_down(),
            Keysym::space => {
                self.app.insert(" ");
                edited = true;
            }
            _ => {
                redraw = false;
                if !control
                    && !self.modifiers.alt
                    && let Some(text) = event.utf8.as_deref()
                    && !text.chars().any(char::is_control)
                {
                    self.app.insert(text);
                    redraw = true;
                    edited = true;
                }
            }
        }

        if redraw {
            // A key press restarts the caret's flash.
            self.app.caret_visible = true;
            self.app.caret_at = now;
            self.redraw();
            if edited {
                self.query_changed();
            }
        }
    }

    /// Every query change is one search line (the core debounces); every editing
    /// path — typing, paste, IME, ✕ — goes through here.
    pub(super) fn query_changed(&self) {
        backend::send(&self.app.query);
    }

    /// Usage recording first, then exactly one command line to the core.
    fn submit(&mut self, now: Instant) {
        if self.app.preedit_active {
            return;
        }

        let Some(launch) = self.app.selected_row() else {
            return;
        };

        let usage = serde_json::json!({
            "title": launch.title,
            "summary": launch.summary.unwrap_or_default(),
            "on_click": launch.target,
            "icon": launch.icon.unwrap_or_default(),
            "ephemeral": launch.ephemeral,
        });

        backend::select(&usage);

        backend::command(&launch.target);

        self.schedule_dismiss(now);
    }

    /// The surface outlives a launch by 150ms. Without it a non-resident run can
    /// exit before the queued verb reaches the writer thread.
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

    fn open_panel(&mut self, now: Instant) {
        if self.app.preedit_active {
            return;
        }
        if self.app.open_actions() {
            self.app.retarget_height(now);
            // The panel is taller than the row list was: the band below the
            // current card edge belongs to the backdrop during the reflow.
            self.needs_full = true;
            self.redraw();
        }
    }

    fn close_panel(&mut self, now: Instant) {
        if self.app.menu.take().is_some() {
            self.app.retarget_height(now);
            self.needs_full = true;
            self.redraw();
        }
    }

    fn run_action(&mut self, now: Instant) {
        let Some(action) = self.app.selected_action() else {
            return;
        };
        self.execute_action(&action, now);
    }

    /// One action-panel command. Pin/unpin re-search so the launcher stays open;
    /// the rest launch and dismiss, except `forget`, which drops the row in place.
    fn execute_action(&mut self, action: &ActionItem, now: Instant) {
        match &action.action {
            PanelAction::Execute { command } => {
                backend::command(command);
                self.schedule_dismiss(now);
            }
            PanelAction::Pin { scope, item } => {
                backend::pin(scope, item);
                self.close_panel(now);
                self.query_changed();
            }
            PanelAction::Unpin { scope, on_click } => {
                backend::unpin(scope, on_click);
                self.close_panel(now);
                self.query_changed();
            }
            PanelAction::Forget { on_click } => {
                backend::forget_row(on_click);
                self.close_panel(now);
            }
        }
    }

    /// Ctrl+C/X: hand the selected text to the core's `copy` command, which
    /// writes it with `wl-copy`. Whether there was a selection to copy.
    fn copy_selection(&self) -> bool {
        let Some((start, end)) = self.app.selection() else {
            return false;
        };

        let text = self.app.query[start..end].to_string();
        backend::command(&Action::Copy { text });
        true
    }
}

impl SeatHandler for Shell {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
            // The text input is per seat, and it must exist before the layer
            // surface asks to be `enable()`d.
            if self.ime.is_none()
                && let Some(manager) = self.ime_manager.as_ref()
            {
                self.ime = Some(manager.get_text_input(&seat, qh, ime::TextInputData));
                // A non-resident first show opens before the seat capability is
                // dispatched, so enable the IME here after `open` missed it.
                if self.layer.is_some() {
                    self.enable_ime();
                }
            }
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            keyboard.release();
        }
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for Shell {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        self.keyboard_focus = true;
        self.app.caret_visible = true;
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        self.keyboard_focus = false;
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.key(&event, Instant::now());
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn repeat_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        // The compositor's own key repeat drives the field.
        self.key(&event, Instant::now());
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _raw_modifiers: smithay_client_toolkit::seat::keyboard::RawModifiers,
        _layout: u32,
    ) {
        self.modifiers = modifiers;
    }
}

impl PointerHandler for Shell {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let now = Instant::now();
        let mut redraw = false;

        for event in events {
            let inside_card = {
                let layout = self.app.appearance.layout;
                let (x, y) = (event.position.0 as f32, event.position.1 as f32);
                x >= layout.card_x(self.app.surface)
                    && x <= layout.card_x(self.app.surface) + layout.card_w(self.app.surface)
                    && y >= layout.card_top(self.app.surface)
                    && y <= layout.card_top(self.app.surface) + self.app.card_height(now)
            };

            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    if self
                        .app
                        .hover_at(event.position.0 as f32, event.position.1 as f32)
                    {
                        redraw = true;
                    }
                    self.app.pointer_on_card = inside_card;
                }
                PointerEventKind::Leave { .. } => {
                    self.app.cursor_left();
                    self.app.pointer_on_card = false;
                    redraw = true;
                }
                PointerEventKind::Press { button, .. } => {
                    if button != 0x110 {
                        // only BTN_LEFT is a click
                        continue;
                    }
                    if self
                        .app
                        .clear_hit(event.position.0 as f32, event.position.1 as f32)
                    {
                        self.app.clear_query();
                        self.app.close_actions();
                        self.redraw();
                        self.query_changed();
                        return;
                    }

                    if self.app.menu.is_some() {
                        let (first, count) = self
                            .app
                            .menu
                            .as_ref()
                            .map(|menu| (menu.first, menu.actions.len()))
                            .unwrap_or_default();
                        if let Some(index) = self.app.appearance.layout.action_at(
                            self.app.surface,
                            first,
                            count,
                            event.position.0 as f32,
                            event.position.1 as f32,
                        ) {
                            if let Some(menu) = &mut self.app.menu {
                                menu.selected = index;
                            }
                            self.run_action(now);
                            return;
                        }
                        if inside_card {
                            // the panel swallows a press that is not on an action
                            return;
                        }
                        self.dismiss(now);
                        return;
                    }

                    let row = self.app.appearance.layout.row_at(
                        self.app.surface,
                        self.app.first,
                        self.app.rows.len(),
                        event.position.0 as f32,
                        event.position.1 as f32,
                    );
                    match row {
                        Some(row) => {
                            self.app.selected = row;
                            self.app.contain();
                            self.submit(now);
                            return;
                        }
                        None if inside_card => {
                            // a press inside the card is swallowed: it must not
                            // reach the backdrop's dismiss
                        }
                        None => {
                            self.dismiss(now);
                            return;
                        }
                    }
                }
                PointerEventKind::Axis { vertical, .. } => {
                    if !self.app.pointer_on_card {
                        continue;
                    }
                    // niri sends both halves of a notch: prefer the
                    // high-resolution one and let the pixel remainder carry.
                    let rows = if vertical.value120 != 0 {
                        vertical.value120 as f32 / 120.0
                    } else if vertical.discrete != 0 {
                        vertical.discrete as f32
                    } else {
                        vertical.absolute as f32 / WHEEL_ROW_PX
                    };
                    let whole = app::whole_rows(&mut self.wheel_accum, rows);
                    if whole != 0 {
                        if self.app.menu.is_some() {
                            self.app.menu_scroll(whole);
                        } else {
                            self.app.scroll(whole);
                        }
                        redraw = true;
                    }
                }
                _ => {}
            }
        }

        if redraw {
            self.redraw();
        }
    }
}
