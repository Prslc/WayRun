use tiny_skia::Pixmap;

use crate::ui::render;
use crate::wayland::Shell;

impl Shell {
    /// The frame without the caret, saved in a rect just around it. A blink
    /// restores this patch and redraws the caret, so no second full frame is held.
    pub(super) fn capture_caret_patch(&mut self) {
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
    pub(super) fn restore_caret_patch(&mut self) {
        let (Some(pixmap), Some(patch)) = (self.pixmap.as_mut(), self.caret_patch.as_ref()) else {
            return;
        };
        patch.restore(pixmap);
    }
}

/// The pixels under the caret from a frame drawn without it; a blink restores the
/// patch and redraws the caret, so no second full pixmap is held.
#[derive(Debug)]
pub(in crate::wayland) struct CaretPatch {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) w: u32,
    pub(super) h: u32,
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

#[cfg(test)]
mod tests {
    use super::CaretPatch;
    use tiny_skia::Pixmap;

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
