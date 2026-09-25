use tiny_skia::{
    BlendMode, Color, FillRule, Paint, Path, PathBuilder, Pixmap, Shader, Stroke, Transform,
};

use crate::ui::geom;

/// The search field's inner insets (the card's `PAD`, magnifier 22, spacing 12
/// and input 8); the IME needs it to place the caret rectangle.
pub const TEXT_INSET: f32 = geom::PAD + 22.0 + 12.0 + 8.0;

/// The clear button and the return hint use the covered `×`/`⏎` forms rather
/// than `✕`/`↵`, which the shaping family may lack and then fall back widely.
pub const CLEAR_GLYPH: &str = "×";
pub const ENTER_GLYPH: &str = "⏎";

/// Everything logical→physical scaling goes through here.
pub struct Canvas {
    pub scale: f32,
}

impl Canvas {
    pub fn px(&self, value: f32) -> f32 {
        value * self.scale
    }

    pub fn color(rgba: [u8; 4]) -> Color {
        Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3])
    }

    pub fn paint(rgba: [u8; 4]) -> Paint<'static> {
        Paint {
            shader: Shader::SolidColor(Self::color(rgba)),
            anti_alias: true,
            ..Paint::default()
        }
    }

    pub fn fill_all(&self, pixmap: &mut Pixmap, rgba: [u8; 4]) {
        if rgba[3] == 0 {
            pixmap.fill(Color::TRANSPARENT);
        } else {
            pixmap.fill(Self::color(rgba));
        }
    }

    /// Overwrite `rect` with the backdrop dim. `BlendMode::Source` replaces, so
    /// a region repaint can put the dim back without applying it twice.
    pub fn restore_dim(&self, pixmap: &mut Pixmap, rect: Rect, dim: f32, rgb: [u8; 3]) {
        let Some(path) = round_rect(rect.scaled(self.scale), 0.0) else {
            return;
        };

        let mut paint = Self::paint([rgb[0], rgb[1], rgb[2], (dim * 255.0).round() as u8]);
        paint.blend_mode = BlendMode::Source;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    /// The band below `y` is backdrop. A growing payload lays rows out at final
    /// size while the card still animates, so they paint below its edge.
    pub fn restore_dim_below(
        &self,
        pixmap: &mut Pixmap,
        y: f32,
        surface: (u32, u32),
        dim: f32,
        rgb: [u8; 3],
    ) {
        self.restore_dim(
            pixmap,
            Rect {
                x: 0.0,
                y,
                w: surface.0 as f32,
                h: surface.1 as f32 - y,
            },
            dim,
            rgb,
        );
    }

    pub fn fill_path(&self, pixmap: &mut Pixmap, path: &Path, rgba: [u8; 4]) {
        if rgba[3] == 0 {
            return;
        }
        pixmap.fill_path(
            path,
            &Self::paint(rgba),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    pub fn stroke_round(
        &self,
        pixmap: &mut Pixmap,
        rect: Rect,
        radius: f32,
        width: f32,
        rgba: [u8; 4],
    ) {
        let Some(path) = round_rect(rect.scaled(self.scale), self.px(radius)) else {
            return;
        };
        let stroke = Stroke {
            width: self.px(width),
            ..Stroke::default()
        };
        pixmap.stroke_path(
            &path,
            &Self::paint(rgba),
            &stroke,
            Transform::identity(),
            None,
        );
    }

    /// A rectangle with square corners (the caret, the preedit quad).
    pub fn fill_rect(&self, pixmap: &mut Pixmap, rect: Rect, rgba: [u8; 4]) {
        self.fill_round(pixmap, rect, 0.0, rgba);
    }

    pub fn fill_round(&self, pixmap: &mut Pixmap, rect: Rect, radius: f32, rgba: [u8; 4]) {
        if let Some(path) = round_rect(rect.scaled(self.scale), self.px(radius)) {
            self.fill_path(pixmap, &path, rgba);
        }
    }

    pub fn stroke_line(
        &self,
        pixmap: &mut Pixmap,
        from: (f32, f32),
        to: (f32, f32),
        width: f32,
        rgba: [u8; 4],
    ) {
        let mut builder = PathBuilder::new();
        builder.move_to(self.px(from.0), self.px(from.1));
        builder.line_to(self.px(to.0), self.px(to.1));
        let Some(path) = builder.finish() else { return };

        let stroke = Stroke {
            width: self.px(width),
            ..Stroke::default()
        };
        pixmap.stroke_path(
            &path,
            &Self::paint(rgba),
            &stroke,
            Transform::identity(),
            None,
        );
    }

    pub fn stroke_circle(
        &self,
        pixmap: &mut Pixmap,
        center: (f32, f32),
        radius: f32,
        width: f32,
        rgba: [u8; 4],
    ) {
        let Some(path) =
            PathBuilder::from_circle(self.px(center.0), self.px(center.1), self.px(radius))
        else {
            return;
        };

        let stroke = Stroke {
            width: self.px(width),
            ..Stroke::default()
        };
        pixmap.stroke_path(
            &path,
            &Self::paint(rgba),
            &stroke,
            Transform::identity(),
            None,
        );
    }
}

/// A logical rectangle.
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    pub fn center_y(self) -> f32 {
        self.y + self.h / 2.0
    }

    /// The search field inside a card at `(x, y)` of width `w`, inset by `PAD`
    /// and `SEARCH_H` tall; the IME's `caret_box` shares it so they cannot drift.
    pub fn field_at(x: f32, y: f32, w: f32) -> Self {
        Self {
            x: x + geom::PAD,
            y: y + geom::PAD,
            w: w - 2.0 * geom::PAD,
            h: geom::SEARCH_H,
        }
    }

    /// The same rectangle in the target's pixels: `round_rect` works in pixmap
    /// space, so every rect passed to it must be scaled, not just radii.
    pub fn scaled(self, scale: f32) -> Self {
        Self {
            x: self.x * scale,
            y: self.y * scale,
            w: self.w * scale,
            h: self.h * scale,
        }
    }
}

/// A rounded rectangle as four quadratic-cornered cubic arcs (kappa).
pub fn round_rect(rect: Rect, radius: f32) -> Option<Path> {
    let r = radius.min(rect.w / 2.0).min(rect.h / 2.0).max(0.0);
    // The standard circular-arc kappa for a 90° cubic approximation.
    let k = r * 0.552_284_8;
    let (x, y, right, bottom) = (rect.x, rect.y, rect.right(), rect.bottom());

    let mut builder = PathBuilder::new();
    builder.move_to(x + r, y);
    builder.line_to(right - r, y);
    builder.cubic_to(right - r + k, y, right, y + r - k, right, y + r);
    builder.line_to(right, bottom - r);
    builder.cubic_to(
        right,
        bottom - r + k,
        right - r + k,
        bottom,
        right - r,
        bottom,
    );
    builder.line_to(x + r, bottom);
    builder.cubic_to(x + r - k, bottom, x, bottom - r + k, x, bottom - r);
    builder.line_to(x, y + r);
    builder.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    builder.close();
    builder.finish()
}

#[cfg(test)]
pub(super) fn pixmap() -> Pixmap {
    Pixmap::new(64, 64).unwrap()
}

/// The 8-bit alpha the dim writes, derived from the shipped surface colour.
#[cfg(test)]
pub(super) fn dim_u8() -> u8 {
    crate::ui::theme::DEFAULT_DIM[3]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reflow_puts_the_band_below_the_card_back_to_the_backdrop() {
        let canvas = Canvas { scale: 1.0 };
        let mut pixmap = pixmap();

        // the "content" the card is still growing over
        canvas.fill_all(&mut pixmap, [255, 255, 255, 255]);
        assert_eq!(pixmap.pixel(32, 40).unwrap().alpha(), 255);

        // 0.30 dim over transparent, the same value the backdrop has
        canvas.restore_dim_below(
            &mut pixmap,
            32.0,
            (64, 64),
            dim_u8() as f32 / 255.0,
            [0, 0, 0],
        );

        let below = pixmap.pixel(32, 40).unwrap();
        assert_eq!(
            below.alpha(),
            dim_u8(),
            "the band is the dim, not the content"
        );
        assert_eq!((below.red(), below.green(), below.blue()), (0, 0, 0));
        // and everything above the band is untouched
        assert_eq!(pixmap.pixel(32, 31).unwrap().alpha(), 255);
    }

    #[test]
    fn geometry_is_scaled_into_the_buffer_not_only_radii_and_strokes() {
        let canvas = Canvas { scale: 2.0 };
        let mut pixmap = Pixmap::new(64, 64).unwrap();
        canvas.fill_all(&mut pixmap, [0, 0, 0, 0]);

        // a 10x10 logical rect at the origin covers the first 20x20 buffer pixels
        canvas.fill_round(
            &mut pixmap,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            0.0,
            [255, 255, 255, 255],
        );
        assert_eq!(pixmap.pixel(19, 19).unwrap().alpha(), 255, "inside");
        assert_eq!(pixmap.pixel(21, 21).unwrap().alpha(), 0, "outside");

        // and a rect at logical (10, 10) starts at buffer (20, 20)
        canvas.fill_round(
            &mut pixmap,
            Rect {
                x: 10.0,
                y: 10.0,
                w: 5.0,
                h: 5.0,
            },
            0.0,
            [255, 255, 255, 255],
        );
        assert_eq!(pixmap.pixel(20, 20).unwrap().alpha(), 255, "second rect");
        assert_eq!(pixmap.pixel(9, 9).unwrap().alpha(), 255, "first rect stays");
    }

    #[test]
    fn the_reflow_band_is_scaled_like_everything_else() {
        let canvas = Canvas { scale: 2.0 };
        let mut pixmap = Pixmap::new(64, 64).unwrap();
        canvas.fill_all(&mut pixmap, [255, 255, 255, 255]);

        // the band starts at logical 10, i.e. buffer row 20
        canvas.restore_dim_below(
            &mut pixmap,
            10.0,
            (64, 64),
            dim_u8() as f32 / 255.0,
            [0, 0, 0],
        );
        assert_eq!(pixmap.pixel(32, 19).unwrap().alpha(), 255, "above the band");
        assert_eq!(
            pixmap.pixel(32, 20).unwrap().alpha(),
            dim_u8(),
            "first band row"
        );
        assert_eq!(
            pixmap.pixel(32, 63).unwrap().alpha(),
            dim_u8(),
            "last band row"
        );
    }

    #[test]
    fn scale_one_leaves_the_geometry_alone() {
        let canvas = Canvas { scale: 1.0 };
        let mut pixmap = Pixmap::new(64, 64).unwrap();
        canvas.fill_all(&mut pixmap, [0, 0, 0, 0]);
        let rect = Rect {
            x: 10.0,
            y: 10.0,
            w: 5.0,
            h: 5.0,
        };
        assert_eq!(rect.scaled(1.0).x, 10.0, "scaled(1.0) is the identity");
        assert_eq!(rect.scaled(1.0).w, 5.0);
        canvas.fill_round(&mut pixmap, rect, 0.0, [255, 255, 255, 255]);
        assert_eq!(
            pixmap.pixel(10, 10).unwrap().alpha(),
            255,
            "starts at 10,10"
        );
        assert_eq!(
            pixmap.pixel(15, 15).unwrap().alpha(),
            0,
            "ends before 15,15"
        );
    }

    #[test]
    fn the_reflow_band_starts_at_the_card_edge() {
        let canvas = Canvas { scale: 1.0 };
        let mut pixmap = pixmap();
        canvas.fill_all(&mut pixmap, [255, 255, 255, 255]);
        canvas.restore_dim_below(
            &mut pixmap,
            0.0,
            (64, 64),
            dim_u8() as f32 / 255.0,
            [0, 0, 0],
        );
        // every row is the dim, including the first one
        for y in [0, 1, 63] {
            assert_eq!(pixmap.pixel(32, y).unwrap().alpha(), dim_u8(), "y={y}");
        }
    }
}
