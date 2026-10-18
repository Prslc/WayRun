use std::borrow::Cow;

use tiny_skia::{
    FilterQuality, IntSize, Pixmap, PixmapPaint, PixmapRef, PremultipliedColorU8, Transform,
};

use super::builtin;

/// Contain-fit `path` into a `size`×`size` box, transparent around it.
pub(super) fn render(path: &str, size: u32) -> Option<Pixmap> {
    let data = load(path)?;
    if is_svg(path) {
        render_svg(&data, size)
    } else {
        // PNG is tiny-skia's fast path; the `image` decoders cover the rest of
        // the common raster formats a file preview can point at.
        let source = Pixmap::decode_png(&data)
            .ok()
            .or_else(|| decode_image(&data))?;
        let mut target = Pixmap::new(size, size)?;
        resample(source.as_ref(), &mut target);
        Some(target)
    }
}

/// Decode JPEG/WebP/GIF/BMP through `image` and premultiply into tiny-skia's
/// representation; a GIF and an animated WebP contribute their first frame.
fn decode_image(data: &[u8]) -> Option<Pixmap> {
    let image = image::load_from_memory(data).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    let mut rgba = image.into_raw();
    let (pixels, _) = rgba.as_chunks_mut::<4>();
    for pixel in pixels {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    Pixmap::from_vec(rgba, IntSize::from_wh(width, height)?)
}

/// The bytes behind an icon spec: a compiled `builtin:` glyph, or the file's.
fn load(path: &str) -> Option<Cow<'static, [u8]>> {
    if let Some(name) = path.strip_prefix("builtin:") {
        return Some(Cow::Borrowed(builtin::glyph(name)));
    }
    Some(Cow::Owned(std::fs::read(path).ok()?))
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

/// Area-average a large source down, bilinear for upscales and near 1:1; a
/// premultiplied channel interpolates without a transparent edge bleeding.
fn resample(source: PixmapRef, target: &mut Pixmap) {
    let (sw, sh) = (source.width() as f32, source.height() as f32);
    let (tw, th) = (target.width() as f32, target.height() as f32);
    let scale = (tw / sw).min(th / sh);
    let (offset_x, offset_y) = ((tw - sw * scale) / 2.0, (th - sh * scale) / 2.0);

    if scale < 1.0 {
        resample_area(source, target, scale, offset_x, offset_y);
        return;
    }

    let source_pixels = source.pixels();
    let width = source.width() as i32;
    let height = source.height() as i32;
    let target_width = target.width();

    for y in 0..target.height() {
        for x in 0..target_width {
            let fx = (x as f32 + 0.5 - offset_x) / scale - 0.5;
            let fy = (y as f32 + 0.5 - offset_y) / scale - 0.5;
            let (x0, y0) = (fx.floor() as i32, fy.floor() as i32);
            let (tx, ty) = (fx - fx.floor(), fy - fy.floor());

            let mut channels = [0.0_f32; 4];
            for (dx, dy, weight) in [
                (0, 0, (1.0 - tx) * (1.0 - ty)),
                (1, 0, tx * (1.0 - ty)),
                (0, 1, (1.0 - tx) * ty),
                (1, 1, tx * ty),
            ] {
                let (px, py) = (x0 + dx, y0 + dy);
                if px < 0 || py < 0 || px >= width || py >= height || weight == 0.0 {
                    continue;
                }
                let pixel = source_pixels[(py * width + px) as usize];
                channels[0] += f32::from(pixel.red()) * weight;
                channels[1] += f32::from(pixel.green()) * weight;
                channels[2] += f32::from(pixel.blue()) * weight;
                channels[3] += f32::from(pixel.alpha()) * weight;
            }

            // Premultiplied: a colour channel cannot exceed alpha, or
            // `from_rgba` refuses the pixel outright.
            let alpha = channels[3].round().clamp(0.0, 255.0) as u8;
            let channel = |value: f32| value.round().clamp(0.0, f32::from(alpha)) as u8;
            if let Some(pixel) = PremultipliedColorU8::from_rgba(
                channel(channels[0]),
                channel(channels[1]),
                channel(channels[2]),
                alpha,
            ) {
                target.pixels_mut()[(y * target_width + x) as usize] = pixel;
            }
        }
    }
}

/// Average every source pixel whose centre falls in the target pixel's
/// footprint; a four-tap sample of a large downscale reads as mush.
fn resample_area(source: PixmapRef, target: &mut Pixmap, scale: f32, offset_x: f32, offset_y: f32) {
    let source_pixels = source.pixels();
    let width = source.width() as i32;
    let height = source.height() as i32;
    let target_width = target.width();
    let half = 0.5 / scale;

    for y in 0..target.height() {
        let cy = (y as f32 + 0.5 - offset_y) / scale;
        let y0 = ((cy - half - 0.5).ceil() as i32).max(0);
        let y1 = ((cy + half - 0.5).floor() as i32 + 1).min(height);
        if y1 <= y0 {
            continue;
        }
        for x in 0..target_width {
            let cx = (x as f32 + 0.5 - offset_x) / scale;
            let x0 = ((cx - half - 0.5).ceil() as i32).max(0);
            let x1 = ((cx + half - 0.5).floor() as i32 + 1).min(width);
            if x1 <= x0 {
                continue;
            }

            let count = u64::from((x1 - x0) as u32) * u64::from((y1 - y0) as u32);
            let mut channels = [0u64; 4];
            for py in y0..y1 {
                for px in x0..x1 {
                    let pixel = source_pixels[(py * width + px) as usize];
                    channels[0] += u64::from(pixel.red());
                    channels[1] += u64::from(pixel.green());
                    channels[2] += u64::from(pixel.blue());
                    channels[3] += u64::from(pixel.alpha());
                }
            }

            let round = |sum: u64| ((sum + count / 2) / count).min(255) as u8;
            let alpha = round(channels[3]);
            let channel = |sum: u64| round(sum).min(alpha);
            if let Some(pixel) = PremultipliedColorU8::from_rgba(
                channel(channels[0]),
                channel(channels[1]),
                channel(channels[2]),
                alpha,
            ) {
                target.pixels_mut()[(y * target_width + x) as usize] = pixel;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use image::ImageEncoder;

    use super::*;

    #[test]
    fn a_downscale_averages_the_covered_source_area() {
        let mut source = Pixmap::new(4, 2).unwrap();
        for y in 0..2 {
            for x in 0..4 {
                let (red, blue) = if x < 2 { (255, 0) } else { (0, 255) };
                source.pixels_mut()[y * 4 + x] =
                    PremultipliedColorU8::from_rgba(red, 0, blue, 255).unwrap();
            }
        }
        let mut target = Pixmap::new(1, 1).unwrap();
        resample(source.as_ref(), &mut target);

        let pixel = target.pixel(0, 0).unwrap();
        assert_eq!(pixel.alpha(), 255);
        assert!(
            pixel.red() > 100 && pixel.blue() > 100,
            "both halves are represented: {:?}",
            (pixel.red(), pixel.blue())
        );
    }

    #[test]
    fn a_jpeg_decodes_through_the_image_crate() {
        let mut data = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut data)
            .write_image(&[220, 40, 40], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let path = std::env::temp_dir().join(format!("wayrun-preview-{}.jpg", std::process::id()));
        std::fs::write(&path, data).unwrap();

        let pixmap = render(path.to_str().unwrap(), 8).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (8, 8));
        let pixel = pixmap.pixel(4, 4).unwrap();
        assert!(pixel.alpha() > 0);
        assert!(pixel.red() > pixel.blue());

        let _ = std::fs::remove_file(path);
    }
}
