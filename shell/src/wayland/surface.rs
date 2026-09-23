use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::compositor::{CompositorHandler, FrameCallbackData, Region};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use tiny_skia::Pixmap;
use wayland_client::protocol::{wl_output, wl_shm, wl_surface};
use wayland_client::{Connection, QueueHandle};

use crate::session::{backend, ipc};
use crate::ui::render;

use super::{Damage, Shell, scale};

const NAMESPACE: &str = "WayRun";

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
        // An empty query means the usage-ranked history, so a show never
        // displays the previous query's payload.
        backend::top();
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
        self.pool = None;
        self.pixmap = None;
        self.caret_patch = None;
        self.buffers = [None, None];
        self.buffer_stale = [Damage::EMPTY, Damage::EMPTY];
        self.buffer_next = 0;
        self.pending_damage = Damage::EMPTY;
        self.buffer_physical = (0, 0);
        // Free the decoded icons: the resident shell sits hidden between shows
        // and these grow with every distinct payload.
        self.icons.clear();
        self.text.clear_cache();
        self.app.hidden();
        ipc::VISIBLE.store(false, Ordering::Relaxed);
        // glibc keeps freed large allocations in its arena, so a hidden resident
        // shell would hold the render buffers it just dropped; return the pages.
        trim_allocator();

        if self.resident {
            return;
        }

        let _ = now;
        self.exit = true;
    }

    /// The buffer size in pixels: the logical size times the effective scale,
    /// which is the fractional ratio when the compositor offered one.
    fn physical_size(&self) -> (u32, u32) {
        let scale = self.app.scale_factor();
        let (width, height) = self.app.surface;
        (
            ((width as f32) * scale).round() as u32,
            ((height as f32) * scale).round() as u32,
        )
    }

    /// Choose the shared-memory format once, from what `wl_shm` advertises.
    fn choose_format(&mut self) {
        if self.format_chosen {
            return;
        }
        self.format_chosen = true;
        let formats = self.shm.formats().to_vec();
        // `ABGR8888` is byte-for-byte the premultiplied RGBA that tiny-skia
        // produces; the mandatory `ARGB8888` needs a per-pixel R/B swap.
        self.shm_format = if formats.contains(&wl_shm::Format::Abgr8888) {
            wl_shm::Format::Abgr8888
        } else {
            wl_shm::Format::Argb8888
        };
        eprintln!(
            "wayrun: wl_shm formats {formats:?} -> {:?}",
            self.shm_format
        );
    }

    /// The retained pixmap, the shm pool and the two persistent buffers for a
    /// physical size; a size change invalidates both buffers and the frame.
    fn ensure_buffers(&mut self, physical_w: u32, physical_h: u32) -> bool {
        if physical_w == 0 || physical_h == 0 {
            return false;
        }
        self.choose_format();

        let sized = |pixmap: &Option<Pixmap>| {
            pixmap
                .as_ref()
                .is_some_and(|pixmap| (pixmap.width(), pixmap.height()) == (physical_w, physical_h))
        };
        if !sized(&self.pixmap) {
            self.pixmap = Pixmap::new(physical_w, physical_h);
            self.caret_patch = None;
            // Room for two full-output buffers; each persistent buffer takes a
            // slot, and the pool grows on demand rather than being a hard cap.
            self.pool =
                SlotPool::new(physical_w as usize * physical_h as usize * 4 * 2, &self.shm).ok();
            // A fresh pixmap holds nothing; the whole frame has to be laid down.
            self.needs_full = true;
        }

        if self.buffer_physical != (physical_w, physical_h)
            || self.buffers.iter().any(Option::is_none)
        {
            self.buffers = [None, None];
            self.buffer_physical = (physical_w, physical_h);
            self.buffer_stale = [Damage::FULL, Damage::FULL];
            self.buffer_next = 0;
            self.pending_damage = Damage::FULL;
            self.needs_full = true;
            let len = physical_w as usize * physical_h as usize * 4;
            for slot in 0..self.buffers.len() {
                self.buffers[slot] = self.new_buffer(len, physical_w, physical_h);
            }
        }

        self.pixmap.is_some() && self.pool.is_some() && self.buffers.iter().all(Option::is_some)
    }

    /// One persistent buffer over a fresh slot.
    fn new_buffer(&mut self, len: usize, physical_w: u32, physical_h: u32) -> Option<Buffer> {
        let format = self.shm_format;
        let pool = self.pool.as_mut()?;
        let slot = pool.new_slot(len).ok()?;
        pool.create_buffer_in(
            &slot,
            physical_w as i32,
            physical_h as i32,
            (physical_w * 4) as i32,
            format,
        )
        .ok()
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

    /// Copy the damaged region into a free shm buffer and commit; the rest of the
    /// buffer keeps its previous upload, so only the damage recomposites.
    fn blit(&mut self) -> bool {
        if !self.configured {
            return false;
        }

        let (physical_w, physical_h) = self.physical_size();
        let (width, height) = self.app.surface;
        let surface = {
            let Some(layer) = self.layer.as_ref() else {
                return false;
            };
            layer.wl_surface().clone()
        };

        // A buffer the compositor has released; if neither is free, retry.
        let Some(index) = self.free_buffer_index() else {
            self.schedule_retry();
            return false;
        };

        let damage = self.buffer_stale[index]
            .union(self.pending_damage)
            .clamped(physical_w, physical_h);
        let Some(damage) = damage else {
            // This buffer already matches the pixmap.
            return true;
        };

        let format = self.shm_format;
        {
            let Some(pixmap) = self.pixmap.as_ref() else {
                return false;
            };
            let Some(pool) = self.pool.as_mut() else {
                return false;
            };
            let Some(buffer) = self.buffers[index].as_ref() else {
                return false;
            };
            let Some(canvas) = buffer.canvas(pool) else {
                return false;
            };

            if damage.full {
                copy_into(pixmap.data(), canvas, format);
            } else {
                copy_rect(
                    pixmap.data(),
                    canvas,
                    physical_w,
                    (damage.x, damage.y, damage.w, damage.h),
                    format,
                );
            }
        }

        // With a fractional ratio the buffer is `logical × ratio` and the viewport
        // maps it back (surface scale 1); without one the integer scale applies.
        if self.app.uses_viewport() {
            surface.set_buffer_scale(1);
            if self.viewport_destination != Some((width, height)) {
                if let Some(viewport) = self.viewport.as_ref() {
                    viewport.set_destination(width as i32, height as i32);
                }
                self.viewport_destination = Some((width, height));
            }
        } else {
            surface.set_buffer_scale(self.app.scale.max(1));
        }
        surface.damage_buffer(damage.x, damage.y, damage.w, damage.h);

        // The frame callback is requested before the commit it paces — a request
        // after it waits for the next — so the next `redraw` coalesces into it.
        let frame_surface = surface.clone();
        surface.frame(&self.qh, FrameCallbackData(frame_surface));
        self.frame_pending = true;

        let Some(buffer) = self.buffers[index].as_ref() else {
            return false;
        };
        if buffer.attach_to(&surface).is_err() {
            self.schedule_retry();
            return false;
        }
        let Some(layer) = self.layer.as_ref() else {
            return false;
        };
        layer.commit();

        // This buffer now matches the pixmap; the other is missing this frame's
        // changes and catches up the next time it is written.
        self.buffer_stale[index] = Damage::EMPTY;
        let other = (index + 1) % self.buffers.len();
        self.buffer_stale[other].add(self.pending_damage);
        self.buffer_next = other;
        self.pending_damage = Damage::EMPTY;

        if self.app.timing {
            let now = Instant::now();
            match self.last_present.take() {
                Some(previous) => eprintln!(
                    "wayrun: frame +{:?} (draw+blit)",
                    now.duration_since(previous)
                ),
                None => eprintln!("wayrun: frame (first)"),
            }
            self.last_present = Some(now);
        }

        // `WAYRUN_SNAPSHOT=<path>` writes the buffer as drawn, once per show: the
        // only way to tell what the shell produced from what the compositor did.
        if !self.first_frame_logged
            && let Some(path) = std::env::var_os("WAYRUN_SNAPSHOT")
        {
            let pixmap = self.pixmap.as_ref();
            if let Some(pixmap) = pixmap {
                let _ = pixmap.save_png(&path);
                eprintln!(
                    "wayrun: snapshot {}x{} at scale {} -> {}",
                    pixmap.width(),
                    pixmap.height(),
                    self.app.scale_factor(),
                    path.to_string_lossy()
                );
            }
        }

        if !self.first_frame_logged {
            self.first_frame_logged = true;
            if self.app.timing
                && let Some(at) = self.open_at
            {
                eprintln!(
                    "wayrun: first frame {}us ({}x{} at scale {})",
                    at.elapsed().as_micros(),
                    physical_w,
                    physical_h,
                    self.app.scale_factor()
                );
            }
        }

        self.sync_ime_cursor();
        true
    }

    /// The next buffer the compositor has released, starting from
    /// [`Shell::buffer_next`].
    fn free_buffer_index(&self) -> Option<usize> {
        let start = self.buffer_next;
        for offset in 0..self.buffers.len() {
            let index = (start + offset) % self.buffers.len();
            if let Some(buffer) = self.buffers[index].as_ref()
                && !buffer.slot().has_active_buffers()
            {
                return Some(index);
            }
        }
        None
    }

    /// Retry a deferred present shortly, for when both buffers are still held; the
    /// frame callback usually wakes `pump`, but a release can arrive alone.
    fn schedule_retry(&mut self) {
        if self.retry_armed {
            return;
        }
        self.retry_armed = true;
        let _ = self.loop_handle.insert_source(
            Timer::from_duration(Duration::from_millis(4)),
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

    /// The frame without the caret, saved in a rect just around it. A blink
    /// restores this patch and redraws the caret, so no second full frame is held.
    fn capture_caret_patch(&mut self) {
        let (x, y, w, h) = render::caret_rect(&self.app, &mut self.text);
        let scale = self.app.scale_factor();
        let Some(pixmap) = self.pixmap.as_ref() else {
            self.caret_patch = None;
            return;
        };

        // The caret is drawn with AA at fractional scales, so widen the patch by
        // two physical pixels on each side and clamp it into the pixmap.
        let left = (x as f32 * scale - 2.0).max(0.0).floor() as u32;
        let top = (y as f32 * scale - 2.0).max(0.0).floor() as u32;
        let right = ((x + w) as f32 * scale + 2.0)
            .min(pixmap.width() as f32)
            .ceil() as u32;
        let bottom = ((y + h) as f32 * scale + 2.0)
            .min(pixmap.height() as f32)
            .ceil() as u32;
        self.caret_patch = CaretPatch::capture(
            pixmap,
            left,
            top,
            right.saturating_sub(left),
            bottom.saturating_sub(top),
        );
    }

    /// Put the caret-free pixels back, so the caret can be drawn (or not) again.
    fn restore_caret_patch(&mut self) {
        let (Some(pixmap), Some(patch)) = (self.pixmap.as_mut(), self.caret_patch.as_ref()) else {
            return;
        };
        patch.restore(pixmap);
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

/// The pixels under the caret from a frame drawn without it; a blink restores the
/// patch and redraws the caret, so no second full pixmap is held.
#[derive(Debug)]
pub(super) struct CaretPatch {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    pixels: Vec<u8>,
}

impl CaretPatch {
    /// `w`/`h` of zero, or any overflow of the pixmap, gives no patch.
    fn capture(pixmap: &Pixmap, x: u32, y: u32, w: u32, h: u32) -> Option<Self> {
        if w == 0 || h == 0 {
            return None;
        }
        if x as usize + w as usize > pixmap.width() as usize
            || y as usize + h as usize > pixmap.height() as usize
        {
            return None;
        }

        let stride = pixmap.width() as usize * 4;
        let row = w as usize * 4;
        let mut pixels = Vec::with_capacity(row * h as usize);
        for line in 0..h {
            let start = (y + line) as usize * stride + x as usize * 4;
            pixels.extend_from_slice(&pixmap.data()[start..start + row]);
        }
        Some(Self { x, y, w, h, pixels })
    }

    fn restore(&self, pixmap: &mut Pixmap) {
        let stride = pixmap.width() as usize * 4;
        let row = self.w as usize * 4;
        let data = pixmap.data_mut();
        for line in 0..self.h {
            let start = (self.y + line) as usize * stride + self.x as usize * 4;
            let source = line as usize * row;
            data[start..start + row].copy_from_slice(&self.pixels[source..source + row]);
        }
    }
}

/// Copy `source` into `destination` swapping R and B per pixel: the difference
/// between `ARGB8888` and the premultiplied `ABGR8888` tiny-skia produces.
fn swap_red_blue(destination: &mut [u8], source: &[u8]) {
    let (destination, _) = destination.as_chunks_mut::<4>();
    let (source, _) = source.as_chunks::<4>();
    for (target, pixel) in destination.iter_mut().zip(source) {
        target[0] = pixel[2];
        target[1] = pixel[1];
        target[2] = pixel[0];
        target[3] = pixel[3];
    }
}

/// The pixmap is premultiplied RGBA; `ABGR8888` is that byte for byte, while
/// `ARGB8888` is the same word with R and B swapped.
fn copy_into(source: &[u8], destination: &mut [u8], format: wl_shm::Format) {
    // `sctk` rounds a slot's length up to 64 bytes, so the canvas can be longer
    // than the pixmap; copying the whole slice would panic.
    let Some(canvas) = destination.get_mut(..source.len()) else {
        return;
    };

    if format == wl_shm::Format::Abgr8888 {
        canvas.copy_from_slice(source);
        return;
    }

    swap_red_blue(canvas, source);
}

/// Copy `rect` (physical pixels, already clamped to the buffer) between two
/// full buffers. `ARGB8888` is `ABGR8888` with R and B swapped per pixel.
fn copy_rect(
    source: &[u8],
    destination: &mut [u8],
    width: u32,
    rect: (i32, i32, i32, i32),
    format: wl_shm::Format,
) {
    let (x, y, w, h) = rect;
    if w <= 0 || h <= 0 {
        return;
    }

    let stride = width as usize * 4;
    let row = w as usize * 4;
    let first = y.max(0) as usize;
    for line in first..(y + h) as usize {
        let start = line * stride + x.max(0) as usize * 4;
        let Some(source) = source.get(start..start + row) else {
            break;
        };
        let Some(destination) = destination.get_mut(start..start + row) else {
            break;
        };

        if format == wl_shm::Format::Abgr8888 {
            destination.copy_from_slice(source);
            continue;
        }
        swap_red_blue(destination, source);
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

/// Release the allocator's free pages back to the kernel. Only glibc's
/// `malloc_trim` does this; other targets leave it to their allocator.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim_allocator() {
    // SAFETY: `malloc_trim` is a plain libc allocator call with no
    // preconditions and no memory effects beyond returning free pages.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim_allocator() {}

#[cfg(test)]
mod tests {
    use super::{CaretPatch, copy_into, copy_rect};
    use tiny_skia::Pixmap;
    use wayland_client::protocol::wl_shm;

    #[test]
    fn a_longer_shm_canvas_is_not_a_panic() {
        // `sctk` rounds a slot's length up to 64 bytes, so the canvas can be
        // longer than the pixmap when its byte length is not a multiple of 64.
        let source: Vec<u8> = (0..16).collect();
        let mut destination = vec![0u8; 16 + 48];

        copy_into(&source, &mut destination, wl_shm::Format::Abgr8888);
        assert_eq!(&destination[..16], &source[..]);
        assert!(destination[16..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn argb8888_swaps_red_and_blue() {
        let source = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let mut destination = vec![0u8; 8];

        copy_into(&source, &mut destination, wl_shm::Format::Argb8888);
        assert_eq!(destination, vec![3, 2, 1, 4, 7, 6, 5, 8]);
    }

    #[test]
    fn a_short_canvas_writes_nothing_rather_than_panicking() {
        let source = vec![1u8; 16];
        let mut destination = vec![0u8; 4];

        copy_into(&source, &mut destination, wl_shm::Format::Abgr8888);
        assert_eq!(destination, vec![0u8; 4]);
    }

    #[test]
    fn a_region_copy_touches_only_its_rows() {
        // A 4x3 buffer of distinct bytes; copy one 2-pixel row.
        let source: Vec<u8> = (0..48).collect();
        let mut destination = vec![0u8; 48];

        copy_rect(
            &source,
            &mut destination,
            4,
            (1, 1, 2, 1),
            wl_shm::Format::Abgr8888,
        );

        let stride = 16;
        assert_eq!(
            &destination[stride + 4..stride + 12],
            &source[stride + 4..stride + 12]
        );
        assert!(destination[..stride].iter().all(|byte| *byte == 0));
        assert!(destination[2 * stride..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_region_copy_swaps_red_and_blue_for_argb() {
        // A 2x1 buffer; copy only the second pixel.
        let source = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let mut destination = vec![0u8; 8];

        copy_rect(
            &source,
            &mut destination,
            2,
            (1, 0, 1, 1),
            wl_shm::Format::Argb8888,
        );

        assert_eq!(&destination[4..8], &[7, 6, 5, 8]);
        assert_eq!(&destination[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn a_caret_patch_restores_only_its_own_rect() {
        let mut pixmap = Pixmap::new(4, 4).unwrap();
        for (i, byte) in pixmap.data_mut().iter_mut().enumerate() {
            *byte = i as u8;
        }
        let before = pixmap.data().to_vec();

        let patch = CaretPatch::capture(&pixmap, 1, 1, 2, 1).unwrap();
        // Paint over the patch and elsewhere: only the patch's rect comes back.
        for byte in &mut pixmap.data_mut()[0..8] {
            *byte = 255;
        }
        for byte in &mut pixmap.data_mut()[32..44] {
            *byte = 255;
        }
        patch.restore(&mut pixmap);

        let stride = 4 * 4;
        let row = 1;
        assert_eq!(
            &pixmap.data()[row * stride + 4..row * stride + 12],
            &before[row * stride + 4..row * stride + 12]
        );
        assert_eq!(&pixmap.data()[0..8], &[255u8; 8]);
        assert_eq!(&pixmap.data()[32..44], &[255u8; 12]);
    }

    #[test]
    fn a_caret_patch_outside_the_pixmap_is_dropped() {
        let pixmap = Pixmap::new(4, 4).unwrap();
        assert!(CaretPatch::capture(&pixmap, 3, 0, 2, 1).is_none());
        assert!(CaretPatch::capture(&pixmap, 0, 3, 1, 2).is_none());
        assert!(CaretPatch::capture(&pixmap, 0, 0, 0, 1).is_none());
    }
}
