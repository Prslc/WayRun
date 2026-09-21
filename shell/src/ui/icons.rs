use std::collections::HashMap;

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, PixmapRef, PremultipliedColorU8, Transform};

pub struct IconCache {
    /// path → (box size → premultiplied RGBA). The key is a `String` so a
    /// per-frame lookup borrows a `&str`; `None` marks an unreadable file.
    entries: HashMap<String, HashMap<u32, Option<Pixmap>>>,
    /// The same bitmap recoloured to the theme foreground, keyed by colour too,
    /// so a live theme change does not show the old tint.
    tinted: HashMap<(String, u32, [u8; 3]), Option<Pixmap>>,
}

impl IconCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            tinted: HashMap::new(),
        }
    }

    /// Drop every decoded icon: the pixmaps are the cache's bulk, and a hidden
    /// resident launcher has no business holding them.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.tinted.clear();
    }

    /// Rasterise `path` now, so a later draw cannot land the cost on an animated
    /// frame; called when a payload arrives, while the launcher is still hidden.
    pub fn warm(&mut self, path: &str, size: u32) {
        self.entries
            .entry(path.to_string())
            .or_default()
            .entry(size)
            .or_insert_with(|| render(path, size));
    }

    /// Like [`IconCache::warm`], for a glyph drawn through
    /// [`IconCache::draw_tinted`].
    pub fn warm_tinted(&mut self, path: &str, size: u32, color: [u8; 3]) {
        self.tinted
            .entry((path.to_string(), size, color))
            .or_insert_with(|| render_glyph(path, size, color));
    }

    /// Draw `path` tinted to `color` when it is a monochrome silhouette; a
    /// coloured icon is left unchanged. The panel's dark glyphs need this.
    pub fn draw_tinted(
        &mut self,
        target: &mut Pixmap,
        path: &str,
        pos: (f32, f32),
        size: u32,
        opacity: f32,
        color: [u8; 3],
    ) {
        let icon = self
            .tinted
            .entry((path.to_string(), size, color))
            .or_insert_with(|| render_glyph(path, size, color));
        let Some(icon) = icon else { return };
        target.draw_pixmap(
            pos.0.round() as i32,
            pos.1.round() as i32,
            icon.as_ref(),
            &PixmapPaint {
                opacity: opacity.clamp(0.0, 1.0),
                ..PixmapPaint::default()
            },
            Transform::identity(),
            None,
        );
    }

    /// Draw the icon contained in a `size`×`size` box at `(x, y)`, at `opacity`;
    /// `size` is in the target's pixels.
    pub fn draw(
        &mut self,
        target: &mut Pixmap,
        path: &str,
        x: f32,
        y: f32,
        size: u32,
        opacity: f32,
    ) {
        // `get_mut` on the outer map borrows the path as `&str`, so a cache hit
        // (every row, every frame) allocates nothing.
        if !self.entries.contains_key(path) {
            self.entries.insert(path.to_string(), HashMap::new());
        }
        let icon = self
            .entries
            .get_mut(path)
            .expect("just inserted")
            .entry(size)
            .or_insert_with(|| render(path, size));

        let Some(icon) = icon else { return };
        target.draw_pixmap(
            x.round() as i32,
            y.round() as i32,
            icon.as_ref(),
            &PixmapPaint {
                opacity: opacity.clamp(0.0, 1.0),
                ..PixmapPaint::default()
            },
            Transform::identity(),
            None,
        );
    }
}

/// Contain-fit `path` into a `size`×`size` box, transparent around it.
fn render(path: &str, size: u32) -> Option<Pixmap> {
    let data = std::fs::read(path).ok()?;
    if path.to_ascii_lowercase().ends_with(".svg") {
        render_svg(&data, size)
    } else {
        // Anything that is not an SVG is decoded as a PNG: that is what the
        // theme's raster icons and every plugin directory hold.
        let source = Pixmap::decode_png(&data).ok()?;
        let mut target = Pixmap::new(size, size)?;
        resample(source.as_ref(), &mut target);
        Some(target)
    }
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

/// Render one panel action glyph. Icon families pad their artwork differently,
/// so crop to the ink and scale that to a common box, then tint: actions from a
/// 24px action set and a 32px symbolic set end up the same optical size.
fn render_glyph(path: &str, size: u32, color: [u8; 3]) -> Option<Pixmap> {
    if path.to_ascii_lowercase().ends_with(".svg") {
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

/// The SVG path of [`render_glyph`]: probe the ink once, then rasterise the
/// vector a second time with the ink fitted to the box, so the glyph is drawn
/// crisp at the final size instead of being scaled after rasterising.
fn render_svg_glyph(path: &str, size: u32, color: [u8; 3]) -> Option<Pixmap> {
    let data = std::fs::read(path).ok()?;
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

/// Bilinear resize into the target box. Premultiplied channels interpolate
/// correctly, so a transparent edge cannot bleed its colour.
fn resample(source: PixmapRef, target: &mut Pixmap) {
    let (sw, sh) = (source.width() as f32, source.height() as f32);
    let (tw, th) = (target.width() as f32, target.height() as f32);
    let scale = (tw / sw).min(th / sh);
    let (offset_x, offset_y) = ((tw - sw * scale) / 2.0, (th - sh * scale) / 2.0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Color;

    /// A PNG on disk is the shape `render` accepts, so the test writes one: an
    /// opaque red square.
    fn red_png(name: &str) -> std::path::PathBuf {
        let mut source = Pixmap::new(4, 4).unwrap();
        source.fill(Color::from_rgba8(255, 0, 0, 255));
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, source.encode_png().unwrap()).unwrap();
        path
    }

    #[test]
    fn a_png_icon_is_decoded_and_drawn_inside_its_box() {
        let path = red_png("wayrun-icon-test.png");
        let mut cache = IconCache::new();
        let mut target = Pixmap::new(34, 34).unwrap();

        cache.draw(&mut target, path.to_str().unwrap(), 2.0, 2.0, 30, 1.0);

        let centre = target.pixel(17, 17).unwrap();
        assert_eq!(centre.alpha(), 255, "the square is drawn");
        assert_eq!((centre.red(), centre.green(), centre.blue()), (255, 0, 0));

        // the box starts at (2,2) and is 30 wide: nothing outside it
        assert_eq!(target.pixel(0, 0).unwrap().alpha(), 0, "outside, top-left");
        assert_eq!(
            target.pixel(33, 33).unwrap().alpha(),
            0,
            "outside, bottom-right"
        );
        assert!(target.pixel(15, 15).unwrap().alpha() > 0, "inside the box");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_svg_icon_is_rendered_without_the_text_feature() {
        // `resvg` is built without `text`/`system-fonts`; path-only SVGs, which
        // is what every theme and plugin icon is, must still render.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#ff0000"/></svg>"##;
        let path = std::env::temp_dir().join("wayrun-icon-test.svg");
        std::fs::write(&path, svg).unwrap();

        let mut cache = IconCache::new();
        let mut target = Pixmap::new(34, 34).unwrap();
        cache.draw(&mut target, path.to_str().unwrap(), 2.0, 2.0, 30, 1.0);

        let centre = target.pixel(17, 17).unwrap();
        assert_eq!(centre.alpha(), 255, "the square is drawn");
        assert_eq!((centre.red(), centre.green(), centre.blue()), (255, 0, 0));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_greyscale_glyph_is_tinted_to_the_theme_colour() {
        // a `#444` silhouette, the shape a Papirus action icon has on this box
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#444444"/></svg>"##;
        let path = std::env::temp_dir().join("wayrun-icon-tint.svg");
        std::fs::write(&path, svg).unwrap();

        let mut cache = IconCache::new();
        let mut target = Pixmap::new(34, 34).unwrap();
        cache.draw_tinted(
            &mut target,
            path.to_str().unwrap(),
            (2.0, 2.0),
            30,
            1.0,
            [241, 223, 218],
        );

        let centre = target.pixel(17, 17).unwrap();
        assert_eq!(centre.alpha(), 255);
        assert_eq!(
            (centre.red(), centre.green(), centre.blue()),
            (241, 223, 218)
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_coloured_glyph_keeps_its_own_colour() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#1e88e5"/></svg>"##;
        let path = std::env::temp_dir().join("wayrun-icon-notint.svg");
        std::fs::write(&path, svg).unwrap();

        let mut cache = IconCache::new();
        let mut target = Pixmap::new(34, 34).unwrap();
        cache.draw_tinted(
            &mut target,
            path.to_str().unwrap(),
            (2.0, 2.0),
            30,
            1.0,
            [241, 223, 218],
        );

        let centre = target.pixel(17, 17).unwrap();
        assert_eq!(
            (centre.red(), centre.green(), centre.blue()),
            (0x1e, 0x88, 0xe5)
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_unreadable_icon_draws_nothing_rather_than_failing() {
        let mut cache = IconCache::new();
        let mut target = Pixmap::new(30, 30).unwrap();
        cache.draw(&mut target, "/nonexistent/icon.png", 0.0, 0.0, 30, 1.0);
        assert!(target.pixels().iter().all(|p| p.alpha() == 0));
    }

    #[test]
    fn every_bundled_glyph_renders() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/assets/icons");
        let mut cache = IconCache::new();
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("svg") {
                continue;
            }
            let mut target = Pixmap::new(64, 64).unwrap();
            cache.draw(&mut target, path.to_str().unwrap(), 0.0, 0.0, 64, 1.0);
            assert!(
                target.pixels().iter().any(|p| p.alpha() > 0),
                "{} rendered empty",
                path.display()
            );
            checked += 1;
        }
        assert!(
            checked >= 20,
            "expected the bundled glyphs, found {checked}"
        );
    }
}
