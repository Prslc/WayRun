mod buffer;
mod caret;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::compositor::{CompositorHandler, Region};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, QueueHandle};
use wayrun_core::trim_allocator;

use crate::session::{backend, ipc};
use crate::ui::render;

use super::{Damage, Shell, scale};

pub(super) use caret::CaretPatch;

const NAMESPACE: &str = "WayRun";

/// How soon to retry a present when no buffer is free yet.
const RETRY_DELAY: Duration = Duration::from_millis(4);

impl Shell {
    pub fn open(&mut self, now: Instant) {
        if self.layer.is_some() {
            return;
        }

        let surface = self.compositor_state.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Overlay,
            Some(NAMESPACE),
            None,
        );
        layer.set_anchor(Anchor::all());
        // `-1`: the surface ignores other surfaces' exclusive zones, so the
        // configure is the full output.
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        // A size of 0 with every anchor lets the compositor decide: the output.
        layer.set_size(0, 0);

        self.effect = self
            .effect_state
            .get_background_effect(layer.wl_surface(), &self.qh)
            .ok();
        // Both objects must exist before the first commit: the fractional scale
        // arrives as an event on the first one, and the viewport maps it back.
        self.fractional = self.fractional_manager.as_ref().map(|manager| {
            manager.get_fractional_scale(layer.wl_surface(), &self.qh, scale::ScaleData)
        });
        self.viewport = self.viewporter.as_ref().map(|viewporter| {
            viewporter.get_viewport(layer.wl_surface(), &self.qh, scale::ViewportData)
        });
        self.viewport_destination = None;
        self.layer = Some(layer);
        // A configure queued for the previous surface must not count as this
        // one's: present/blit would attach a buffer before the first configure.
        self.configured = false;
        self.blur_sent = None;
        self.ime_cursor_sent = None;
        self.first_frame_logged = false;
        // The previous surface may have left a callback outstanding; nothing
        // will answer it, so the new show starts unpaced.
        self.frame_pending = false;
        self.needs_present = false;
        self.needs_full = true;
        // A paste worker from the previous show must not land on this one.
        self.paste_generation = self.paste_generation.wrapping_add(1);

        self.app.shown(now);
        ipc::VISIBLE.store(true, Ordering::Relaxed);
        self.open_at = Some(now);
        self.last_present = None;
        // Allocate the frame buffers here rather than on the first present, so
        // the allocation does not land on the frame that starts the entrance.
        let (physical_w, physical_h) = self.physical_size();
        self.ensure_buffers(physical_w, physical_h);
        if self.app.timing {
            eprintln!("wayrun: open at +{:?}", self.started.elapsed());
        }

        // A layer surface must be committed once with no buffer attached before
        // the compositor configures it.
        if let Some(layer) = self.layer.as_ref() {
            layer.commit();
        }
        self.enable_ime();
        self.arm_timer();
    }

    pub(super) fn dismiss(&mut self, now: Instant) {
        if self.layer.is_none() {
            return;
        }

        self.disable_ime();
        // The staged text-input state belongs to the surface going away: a queued
        // `done` must not land on the next show's empty query.
        self.pending.clear();
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        if let Some(fractional) = self.fractional.take() {
            fractional.destroy();
        }
        self.viewport_destination = None;
        // `app.fractional` is retained (an output property), so the next `open`
        // allocates at the exact ratio. Dropping the layer surface destroys it.
        self.layer = None;
        self.configured = false;
        self.frame_pending = false;
        self.needs_present = false;
        // `destroy`, not a plain drop: wayland-rs removes an object only when its
        // destructor is sent, so dropping the proxy would leak one per toggle.
        if let Some(effect) = self.effect.take() {
            effect.destroy();
        }
        // The pool and its buffer stay: every show after the first starts warm.
        // The frame pixmap and the scratch buffer belong to one show.
        self.pixmap = None;
        self.caret_patch = None;
        self.buffers[1] = None;
        self.scratch_pool = None;
        self.buffer_stale[1] = Damage::FULL;
        // Free the decoded icons: the resident shell sits hidden between shows
        // and these grow with every distinct payload.
        self.icons.clear();
        self.text.clear_cache();
        // The dismissal cancels the core's pending search and reaps the warm hosts.
        backend::dismiss();
        self.app.hidden();
        ipc::VISIBLE.store(false, Ordering::Relaxed);

        if self.resident {
            // glibc keeps freed large allocations in its arenas; return the pages.
            trim_allocator();
            return;
        }

        let _ = now;
        self.exit = true;
    }

    pub(super) fn present(&mut self, now: Instant) {
        if self.layer.is_none() || !self.configured {
            return;
        }

        let (physical_w, physical_h) = self.physical_size();
        if physical_w == 0 || physical_h == 0 {
            return;
        }

        if !self.ensure_buffers(physical_w, physical_h) {
            return;
        }
        // Rasterise only when a buffer can take the frame: a deferred blit would
        // otherwise draw the whole thing again on the retry.
        if self.free_buffer_index().is_none() {
            self.schedule_retry();
            return;
        }
        self.needs_present = false;
        // The fade begins on the first frame that is actually drawn.
        self.app.start_entrance(now);
        // A fresh pixmap, a moving dim (entrance) or a reflowing card needs the
        // whole surface; a settled redraw only repaints the card rectangle.
        let full = self.needs_full || self.app.animating(now);
        self.needs_full = false;
        {
            let Some(pixmap) = self.pixmap.as_mut() else {
                return;
            };
            render::draw(
                pixmap,
                &self.app,
                &mut self.text,
                &mut self.icons,
                now,
                full,
            );
        }

        // What the frame just changed, in physical pixels: a settled repaint is
        // the card rect, spanned by the previous and current bottoms.
        let surface_size = self.app.surface;
        let scale = self.app.scale_factor();
        let layout = self.app.appearance.layout;
        let top = layout.card_top(surface_size);
        let new_bottom = top + self.app.card_height(now);
        let damage = if full {
            Damage::FULL
        } else {
            let base_bottom = new_bottom.max(self.app.last_card_bottom);
            Damage::rect(
                (layout.card_x(surface_size) * scale).floor() as i32 - 1,
                (top * scale).floor() as i32 - 1,
                (layout.card_w(surface_size) * scale).ceil() as i32 + 2,
                ((base_bottom - top) * scale).ceil() as i32 + 3,
            )
        };
        self.pending_damage.add(damage);
        self.app.last_card_bottom = new_bottom;

        // Capture the caret-free pixels before drawing the caret, so a blink
        // can restore them without a second frame pixmap.
        self.capture_caret_patch();
        if let Some(pixmap) = self.pixmap.as_mut() {
            render::draw_caret(pixmap, &self.app, &mut self.text, now);
        }

        // The blur region is double-buffered state: it must be set before the
        // commit that should carry it, or the first frame goes unblurred.
        self.sync_blur();

        if !self.blit() {
            self.needs_present = true;
        }
    }

    /// Retry a deferred present shortly, for when no buffer is free yet; the
    /// frame callback usually wakes `pump`, but a release can arrive alone.
    fn schedule_retry(&mut self) {
        if self.retry_armed {
            return;
        }
        self.retry_armed = true;
        let _ = self.loop_handle.insert_source(
            Timer::from_duration(RETRY_DELAY),
            |_, _, state: &mut Shell| {
                state.retry_armed = false;
                state.pump();
                TimeoutAction::Drop
            },
        );
    }

    /// A blink: repaint the caret over the restored patch and commit, without
    /// re-rendering the surface.
    pub(super) fn present_caret(&mut self, now: Instant) {
        self.restore_caret_patch();
        if let Some(pixmap) = self.pixmap.as_mut() {
            render::draw_caret(pixmap, &self.app, &mut self.text, now);
        }
        if let Some(patch) = self.caret_patch.as_ref() {
            self.pending_damage.add(Damage::rect(
                patch.x as i32,
                patch.y as i32,
                patch.w as i32,
                patch.h as i32,
            ));
        }
        if !self.blit() {
            self.needs_present = true;
        }
    }

    /// The blur region covers the card's rounded rect, deduped on the
    /// 4px-quantised rect: a present must not re-apply the stored region.
    fn sync_blur(&mut self) {
        let (Some(layer), Some(effect)) = (self.layer.as_ref(), self.effect.as_ref()) else {
            return;
        };
        let _ = layer;

        if !self.app.appearance.blur {
            // `None` clears a region set before the config disabled blur.
            if self.blur_sent.take().is_some() {
                effect.set_blur_region(None);
            }
            return;
        }

        let height = self.app.card_height(Instant::now());
        let rects = self
            .app
            .appearance
            .layout
            .blur_rects(self.app.surface, height);
        let key = rects
            .first()
            .map(|rect| (rect.x, rect.y, rect.width, rect.height));
        if key.is_none() || self.blur_sent == key {
            return;
        }

        let Ok(region) = Region::new(&self.compositor_state) else {
            return;
        };
        for rect in &rects {
            region.add(rect.x, rect.y, rect.width, rect.height);
        }
        effect.set_blur_region(Some(region.wl_region()));
        self.blur_sent = key;
    }
}

impl CompositorHandler for Shell {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if self
            .layer
            .as_ref()
            .is_none_or(|layer| layer.wl_surface() != surface)
        {
            return;
        }

        self.app.scale = factor.max(1);
        self.needs_full = true;
        self.redraw();
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        self.frame_pending = false;
        self.pump();
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for Shell {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.dismiss(Instant::now());
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // A configure for a surface that is gone (or a superseded one) must not
        // flip `configured` and let the next show present before its own.
        if self
            .layer
            .as_ref()
            .is_none_or(|current| current.wl_surface() != layer.wl_surface())
        {
            return;
        }

        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }

        if self.app.timing {
            eprintln!(
                "wayrun: configure {width}x{height} at +{:?}",
                self.started.elapsed()
            );
        }
        self.app.surface = (width, height);
        self.configured = true;
        self.blur_sent = None;
        self.needs_full = true;

        self.present(Instant::now());
        let _ = self.conn.flush();
    }
}
