use std::sync::Arc;

use smithay_client_toolkit::dispatch2::Dispatch2;
use wayland_client::backend::ObjectData;
use wayland_client::globals::GlobalList;
use wayland_client::{Connection, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::{
    self, WpFractionalScaleManagerV1,
};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::{self, WpViewport};
use wayland_protocols::wp::viewporter::client::wp_viewporter::{self, WpViewporter};

use super::Shell;

/// Object data for the fractional-scale proxies.
#[derive(Debug)]
pub struct ScaleData;

/// Object data for the viewporter proxies.
#[derive(Debug)]
pub struct ViewportData;

/// Both globals, if the compositor offers both: a scaled buffer is only usable
/// when a viewport can map it back onto the surface's logical size.
pub fn bind(
    globals: &GlobalList,
    qh: &QueueHandle<Shell>,
) -> Option<(WpFractionalScaleManagerV1, WpViewporter)> {
    let manager = globals
        .bind::<WpFractionalScaleManagerV1, Shell, ScaleData>(qh, 1..=1, ScaleData)
        .ok()?;
    let viewporter = globals
        .bind::<WpViewporter, Shell, ViewportData>(qh, 1..=1, ViewportData)
        .ok()?;

    Some((manager, viewporter))
}

impl Dispatch2<WpFractionalScaleManagerV1, Shell> for ScaleData {
    fn event(
        &self,
        _state: &mut Shell,
        _manager: &WpFractionalScaleManagerV1,
        event: wp_fractional_scale_manager_v1::Event,
        _conn: &Connection,
        _qh: &QueueHandle<Shell>,
    ) {
        let _ = event;
    }

    fn event_created_child(_opcode: u16, qh: &QueueHandle<Shell>) -> Arc<dyn ObjectData> {
        qh.make_data::<WpFractionalScaleV1, ScaleData>(ScaleData)
    }
}

impl Dispatch2<WpFractionalScaleV1, Shell> for ScaleData {
    fn event(
        &self,
        state: &mut Shell,
        _scale: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _conn: &Connection,
        _qh: &QueueHandle<Shell>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.set_fractional_scale(scale);
        }
    }
}

impl Dispatch2<WpViewporter, Shell> for ViewportData {
    fn event(
        &self,
        _state: &mut Shell,
        _viewporter: &WpViewporter,
        event: wp_viewporter::Event,
        _conn: &Connection,
        _qh: &QueueHandle<Shell>,
    ) {
        let _ = event;
    }

    fn event_created_child(_opcode: u16, qh: &QueueHandle<Shell>) -> Arc<dyn ObjectData> {
        qh.make_data::<WpViewport, ViewportData>(ViewportData)
    }
}

impl Dispatch2<WpViewport, Shell> for ViewportData {
    fn event(
        &self,
        _state: &mut Shell,
        _viewport: &WpViewport,
        event: wp_viewport::Event,
        _conn: &Connection,
        _qh: &QueueHandle<Shell>,
    ) {
        let _ = event;
    }
}

impl Shell {
    /// A `wp_fractional_scale_v1` change: the buffers must be rebuilt at the new ratio.
    pub fn set_fractional_scale(&mut self, scale_120: u32) {
        if scale_120 == 0 {
            return;
        }

        let scale = scale_120 as f32 / 120.0;
        if self.app.fractional == Some(scale) {
            return;
        }
        self.app.fractional = Some(scale);
        self.redraw();
    }
}
