use std::borrow::Cow;
use std::fs;

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, PremultipliedColorU8, Transform};

use super::{builtin, raster};

/// Contain-fit `path` into a `size`×`size` box: an SVG is rasterised, a raster decoded.
pub(super) fn render(path: &str, size: u32) -> Option<Pixmap> {
    if is_svg(path) {
        let data = load(path)?;
        return render_svg(&data, size);
    }
    raster::decode(path, size)
}

/// The intrinsic size a preview pane should fit to. SVG is parsed because
/// `image` cannot read it; a file neither parser answers declines.
pub(super) fn intrinsic_size(path: &str) -> Option<(u32, u32)> {
    if is_svg(path) {
        let data = load(path)?;
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let size = tree.size();
        return Some((size.width().round() as u32, size.height().round() as u32));
    }
    raster::dimensions(path)
}

/// The bytes behind an icon spec: a compiled `builtin:` glyph, or the file's.
fn load(path: &str) -> Option<Cow<'static, [u8]>> {
    if let Some(name) = path.strip_prefix("builtin:") {
        return Some(Cow::Borrowed(builtin::glyph(name)));
    }
    Some(Cow::Owned(fs::read(path).ok()?))
}

/// Whether a spec is an SVG; every compiled glyph is one.
fn is_svg(path: &str) -> bool {
    path.starts_with("builtin:") || path.to_ascii_lowercase().ends_with(".svg")
}

/// Recolour a monochrome silhouette to `color`, preserving alpha; a coloured icon
/// is left alone.
fn tint(pixmap: &mut Pixmap, color: [u8; 3]) {
    let greyscale = pixmap
        .pixels()
        .iter()
        .filter(|pixel| pixel.alpha() > 0)
        .all(|pixel| {
            pixel.red().abs_diff(pixel.green()) <= 8 && pixel.green().abs_diff(pixel.blue()) <= 8
        });
    if !greyscale {
        return;
    }

    for pixel in pixmap.pixels_mut() {
        let alpha = pixel.alpha();
        let scale = u32::from(alpha);
        // Premultiplied colour: `color * alpha / 255`, which never exceeds the
        // alpha, so `from_rgba` accepts it.
        let channel = |value: u8| ((u32::from(value) * scale + 127) / 255) as u8;
        *pixel = PremultipliedColorU8::from_rgba(
            channel(color[0]),
            channel(color[1]),
            channel(color[2]),
            alpha,
        )
        .expect("a premultiplied channel never exceeds its alpha");
    }
}

/// Render one panel action glyph: families pad their artwork differently, so crop
/// to the ink, scale it to a common box, then tint; any source size lands alike.
pub(super) fn render_glyph(path: &str, size: u32, color: [u8; 3]) -> Option<Pixmap> {
    if is_svg(path) {
        return render_svg_glyph(path, size, color);
    }

    // A raster glyph: crop to its ink, tint, then resample into the box.
    let source = render(path, size)?;
    let (x, y, w, h) = ink_bounds(&source)?;
    let mut cropped = Pixmap::new(w, h)?;
    cropped.draw_pixmap(
        -(x as i32),
        -(y as i32),
        source.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    tint(&mut cropped, color);

    let mut out = Pixmap::new(size, size)?;
    let side = size as f32;
    let box_side = side * GLYPH_BOX;
    let scale = (box_side / w as f32).min(box_side / h as f32);
    out.draw_pixmap(
        0,
        0,
        cropped.as_ref(),
        &PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        },
        Transform::from_scale(scale, scale).post_translate(
            (side - w as f32 * scale) / 2.0,
            (side - h as f32 * scale) / 2.0,
        ),
        None,
    );
    Some(out)
}

/// The SVG path of [`render_glyph`]: probe the ink once, then rasterise the vector
/// a second time with the ink fitted to the box, so it is crisp at the final size.
fn render_svg_glyph(path: &str, size: u32, color: [u8; 3]) -> Option<Pixmap> {
    let data = load(path)?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
    let source = tree.size();
    if source.width() <= 0.0 || source.height() <= 0.0 {
        return None;
    }

    let side = size as f32;
    let base_scale = (side / source.width()).min(side / source.height());
    let base = Transform::from_scale(base_scale, base_scale).post_translate(
        (side - source.width() * base_scale) / 2.0,
        (side - source.height() * base_scale) / 2.0,
    );

    let mut probe = Pixmap::new(size, size)?;
    resvg::render(&tree, base, &mut probe.as_mut());
    let (x, y, w, h) = ink_bounds(&probe)?;

    let box_side = side * GLYPH_BOX;
    let fit = (box_side / w as f32).min(box_side / h as f32);
    let ox = (side - w as f32 * fit) / 2.0;
    let oy = (side - h as f32 * fit) / 2.0;
    let fitted = Transform::from_row(
        base.sx * fit,
        base.kx * fit,
        base.ky * fit,
        base.sy * fit,
        fit * (base.tx - x as f32) + ox,
        fit * (base.ty - y as f32) + oy,
    );

    let mut out = Pixmap::new(size, size)?;
    resvg::render(&tree, fitted, &mut out.as_mut());
    tint(&mut out, color);
    Some(out)
}

/// The fraction of the box a panel glyph's ink fills, so icons from different
/// families look the same optical size.
const GLYPH_BOX: f32 = 0.8;

/// The bounding box `(x, y, w, h)` of a pixmap's non-transparent pixels.
fn ink_bounds(pixmap: &Pixmap) -> Option<(u32, u32, u32, u32)> {
    let (width, height) = (pixmap.width(), pixmap.height());
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (width, height, 0, 0);
    let mut found = false;
    for y in 0..height {
        for x in 0..width {
            if pixmap.pixel(x, y).is_some_and(|pixel| pixel.alpha() > 0) {
                found = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    found.then(|| (min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
}

/// Render the SVG into a `size`×`size` box, contained (aspect preserved).
fn render_svg(data: &[u8], size: u32) -> Option<Pixmap> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let source = tree.size();
    if source.width() <= 0.0 || source.height() <= 0.0 {
        return None;
    }

    let mut pixmap = Pixmap::new(size, size)?;
    let side = size as f32;
    let scale = (side / source.width()).min(side / source.height());
    let transform = Transform::from_scale(scale, scale).post_translate(
        (side - source.width() * scale) / 2.0,
        (side - source.height() * scale) / 2.0,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use image::ImageEncoder;
    use std::env;
    use std::process;

    use super::*;

    #[test]
    fn an_svg_intrinsic_size_comes_from_its_attributes() {
        let path = env::temp_dir().join("wayrun-aspect.svg");
        fs::write(
            &path,
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="90"><rect width="160" height="90"/></svg>"#,
        )
        .unwrap();
        assert_eq!(intrinsic_size(path.to_str().unwrap()), Some((160, 90)));

        fs::write(
            &path,
            br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 32"><rect width="64" height="32"/></svg>"#,
        )
        .unwrap();
        assert_eq!(intrinsic_size(path.to_str().unwrap()), Some((64, 32)));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_jpeg_decodes_through_the_scaled_decoder() {
        let mut data = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut data)
            .write_image(&[220, 40, 40], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let path = env::temp_dir().join(format!("wayrun-preview-{}.jpg", process::id()));
        fs::write(&path, data).unwrap();

        let pixmap = render(path.to_str().unwrap(), 8).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (8, 8));
        let pixel = pixmap.pixel(4, 4).unwrap();
        assert!(pixel.alpha() > 0);
        assert!(pixel.red() > pixel.blue());

        let _ = fs::remove_file(path);
    }
}
