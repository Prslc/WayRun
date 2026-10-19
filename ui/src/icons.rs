mod builtin;
mod raster;
mod render;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use std::thread;

use tiny_skia::{FillRule, Mask, PathBuilder, Pixmap, PixmapPaint, Transform};
use wayrun_core::wire::IconShape;

use self::render::{intrinsic_size, render, render_glyph};

/// One decode a frame asked for: what the worker renders and the key it lands
/// under, so a repeat ask is told apart from a first one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconKey {
    Plain {
        path: String,
        size: u32,
        shape: IconShape,
    },
    Tinted {
        path: String,
        size: u32,
        color: [u8; 3],
        shape: IconShape,
    },
}

impl IconKey {
    fn path(&self) -> &str {
        match self {
            IconKey::Plain { path, .. } | IconKey::Tinted { path, .. } => path,
        }
    }
}

/// One icon draw: the file, the box it fits and how it lands.
pub struct IconDraw<'a> {
    pub path: &'a str,
    pub x: f32,
    pub y: f32,
    pub size: u32,
    pub opacity: f32,
    pub shape: IconShape,
}

pub struct IconCache {
    /// path → (box size and shape → premultiplied RGBA). The key is a `String`
    /// so a per-frame lookup borrows a `&str`; `None` marks an unreadable file.
    entries: HashMap<String, HashMap<(u32, IconShape), Option<Pixmap>>>,
    /// The same bitmap recoloured to the theme foreground, keyed by colour too,
    /// so a live theme change does not show the old tint.
    #[allow(clippy::type_complexity)] // two levels, so a lookup borrows the path
    tinted: HashMap<String, HashMap<(u32, [u8; 3], IconShape), Option<Pixmap>>>,
    /// Keys asked for but not yet delivered, so a frame never queues a decode
    /// the worker is already on.
    pending: HashSet<IconKey>,
    /// Intrinsic image sizes for the preview pane, read from the file header
    /// once so a preview can keep its aspect; SVG falls back to a square.
    aspects: HashMap<String, Option<(u32, u32)>>,
    /// Bumped by [`IconCache::clear`], so a result a dismissed show asked for
    /// cannot land in the next one.
    generation: u64,
    /// The decode worker's queue.
    jobs: Sender<(u64, IconKey)>,
}

impl IconCache {
    pub fn new(jobs: Sender<(u64, IconKey)>) -> Self {
        Self {
            entries: HashMap::new(),
            tinted: HashMap::new(),
            pending: HashSet::new(),
            aspects: HashMap::new(),
            generation: 0,
            jobs,
        }
    }

    /// Drop every decoded icon and orphan the results the worker still owes, so
    /// a hidden resident launcher holds neither.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.tinted.clear();
        self.pending.clear();
        self.aspects.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// The image's intrinsic size, read once so a preview can keep its aspect;
    /// SVG is parsed, and a file neither parser answers stays square.
    pub fn aspect(&mut self, path: &str) -> Option<(u32, u32)> {
        if let Some(cached) = self.aspects.get(path) {
            return *cached;
        }
        let size = intrinsic_size(path).filter(|(width, height)| *width > 0 && *height > 0);
        self.aspects.insert(path.to_string(), size);
        size
    }

    /// Drop the recoloured bitmaps: a tint is keyed by the colour it was mixed
    /// from, so a theme change leaves every entry unreachable.
    pub fn drop_tinted(&mut self) {
        self.tinted.clear();
    }

    /// Ask the worker for `path`, unless it is cached or already asked for; a
    /// payload that arrives while the launcher is hidden lands before it shows.
    pub fn warm(&mut self, path: &str, size: u32, shape: IconShape) {
        if self
            .entries
            .get(path)
            .is_none_or(|by_key| !by_key.contains_key(&(size, shape)))
        {
            self.request(IconKey::Plain {
                path: path.to_string(),
                size,
                shape,
            });
        }
    }

    /// Like [`IconCache::warm`], for a glyph drawn through [`IconCache::draw_tinted`].
    pub fn warm_tinted(&mut self, path: &str, size: u32, color: [u8; 3], shape: IconShape) {
        if self
            .tinted
            .get(path)
            .is_none_or(|by_key| !by_key.contains_key(&(size, color, shape)))
        {
            self.request(IconKey::Tinted {
                path: path.to_string(),
                size,
                color,
                shape,
            });
        }
    }

    /// Store a decoded icon the worker delivered; a result asked for before a
    /// [`IconCache::clear`] is dropped instead of painted into the next show.
    pub fn insert(&mut self, generation: u64, key: IconKey, icon: Option<Pixmap>) {
        if generation != self.generation {
            return;
        }
        self.pending.remove(&key);
        match key {
            IconKey::Plain { path, size, shape } => {
                self.entries
                    .entry(path)
                    .or_default()
                    .insert((size, shape), icon);
            }
            IconKey::Tinted {
                path,
                size,
                color,
                shape,
            } => {
                self.tinted
                    .entry(path)
                    .or_default()
                    .insert((size, color, shape), icon);
            }
        }
    }

    /// Drop one path's decoded entries and queued request, so a file that just
    /// appeared is loaded on the next frame.
    pub fn forget(&mut self, path: &str) {
        self.entries.remove(path);
        self.tinted.remove(path);
        self.aspects.remove(path);
        self.pending.retain(|key| key.path() != path);
    }

    /// Draw `path` tinted to `color` when it is a monochrome silhouette; a
    /// coloured icon is left unchanged. The panel's dark glyphs need this.
    pub fn draw_tinted(&mut self, target: &mut Pixmap, draw: IconDraw<'_>, color: [u8; 3]) {
        let IconDraw {
            path,
            x,
            y,
            size,
            opacity,
            shape,
        } = draw;
        let key = (size, color, shape);
        if self
            .tinted
            .get(path)
            .is_none_or(|by_key| !by_key.contains_key(&key))
        {
            self.request(IconKey::Tinted {
                path: path.to_string(),
                size,
                color,
                shape,
            });
            return;
        }
        // the key is known here, so a `None` inside is a cached unreadable file
        let Some(Some(icon)) = self.tinted.get(path).and_then(|by_key| by_key.get(&key)) else {
            return;
        };

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

    /// Draw the icon contained in a `size`×`size` box at its position, at
    /// `opacity`. A missing file is asked for, never rasterised here.
    pub fn draw(&mut self, target: &mut Pixmap, draw: IconDraw<'_>) {
        let IconDraw {
            path,
            x,
            y,
            size,
            opacity,
            shape,
        } = draw;
        // `get` borrows the path as `&str`, so a cache hit allocates nothing.
        if self
            .entries
            .get(path)
            .is_none_or(|by_key| !by_key.contains_key(&(size, shape)))
        {
            self.request(IconKey::Plain {
                path: path.to_string(),
                size,
                shape,
            });
            return;
        }
        // the key is known here, so a `None` inside is a cached unreadable file
        let Some(Some(icon)) = self
            .entries
            .get(path)
            .and_then(|by_key| by_key.get(&(size, shape)))
        else {
            return;
        };

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

    /// Queue `key` unless the worker is already on it.
    fn request(&mut self, key: IconKey) {
        if self.pending.insert(key.clone()) {
            let _ = self.jobs.send((self.generation, key));
        }
    }
}

/// Decode requests on one thread so the UI thread never rasterises; every result
/// comes back through `done` under the generation it was asked for.
pub fn spawn_worker<F>(done: F) -> Sender<(u64, IconKey)>
where
    F: Fn((u64, IconKey, Option<Pixmap>)) -> bool + Send + 'static,
{
    let (jobs, queue) = mpsc::channel();
    thread::spawn(move || {
        while let Ok((generation, key)) = queue.recv() {
            let shape = match &key {
                IconKey::Plain { shape, .. } | IconKey::Tinted { shape, .. } => *shape,
            };
            let icon = match &key {
                IconKey::Plain { path, size, .. } => render(path, *size),
                IconKey::Tinted {
                    path, size, color, ..
                } => render_glyph(path, *size, *color),
            }
            .map(|mut icon| {
                clip(&mut icon, shape);
                icon
            });
            if !done((generation, key, icon)) {
                break;
            }
        }
    });
    jobs
}

/// Clip a decoded icon to its box shape; the mask runs on the decode thread, so
/// a frame never scans pixels.
fn clip(icon: &mut Pixmap, shape: IconShape) {
    match shape {
        IconShape::Square => return,
        IconShape::Circle => {}
    }
    let width = icon.width() as f32;
    let height = icon.height() as f32;
    let Some(path) = PathBuilder::from_circle(width / 2.0, height / 2.0, width.min(height) / 2.0)
    else {
        return;
    };
    let Some(mut mask) = Mask::new(icon.width(), icon.height()) else {
        return;
    };
    mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
    icon.apply_mask(&mask);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use tiny_skia::Color;

    /// A PNG on disk is the shape `render` accepts, so the test writes one: an
    /// opaque red square.
    fn red_png(name: &str) -> PathBuf {
        let mut source = Pixmap::new(4, 4).unwrap();
        source.fill(Color::from_rgba8(255, 0, 0, 255));
        let path = env::temp_dir().join(name);
        fs::write(&path, source.encode_png().unwrap()).unwrap();
        path
    }

    /// The cache plus the queue the worker would drain.
    fn test_cache() -> (IconCache, mpsc::Receiver<(u64, IconKey)>) {
        let (jobs, queued) = mpsc::channel();
        (IconCache::new(jobs), queued)
    }

    /// A plain draw at full opacity: what a settled frame asks for.
    fn plain(path: &str, x: f32, y: f32, size: u32) -> IconDraw<'_> {
        IconDraw {
            path,
            x,
            y,
            size,
            opacity: 1.0,
            shape: IconShape::Square,
        }
    }

    /// Store what the worker would deliver for a plain icon.
    fn deliver(cache: &mut IconCache, path: &str, size: u32) {
        let key = IconKey::Plain {
            path: path.to_string(),
            size,
            shape: IconShape::Square,
        };
        let generation = cache.generation;
        cache.insert(generation, key, render(path, size));
    }

    /// Store what the worker would deliver for a tinted glyph.
    fn deliver_tinted(cache: &mut IconCache, path: &str, size: u32, color: [u8; 3]) {
        let key = IconKey::Tinted {
            path: path.to_string(),
            size,
            color,
            shape: IconShape::Square,
        };
        let generation = cache.generation;
        cache.insert(generation, key, render_glyph(path, size, color));
    }

    #[test]
    fn a_png_icon_is_decoded_and_drawn_inside_its_box() {
        let path = red_png("wayrun-icon-test.png");
        let (mut cache, _queued) = test_cache();
        let mut target = Pixmap::new(34, 34).unwrap();

        deliver(&mut cache, path.to_str().unwrap(), 30);
        cache.draw(&mut target, plain(path.to_str().unwrap(), 2.0, 2.0, 30));

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

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_circle_icon_clips_the_box_corners() {
        let path = red_png("wayrun-icon-circle.png");
        let mut icon = render(path.to_str().unwrap(), 30).unwrap();
        clip(&mut icon, IconShape::Circle);

        assert_eq!(icon.pixel(15, 15).unwrap().alpha(), 255, "the centre stays");
        assert_eq!(icon.pixel(0, 0).unwrap().alpha(), 0, "a corner is clipped");
        assert!(icon.pixel(15, 2).unwrap().alpha() > 0, "the top edge stays");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn an_svg_icon_is_rendered_without_the_text_feature() {
        // `resvg` is built without `text`/`system-fonts`; path-only SVGs, which
        // is what every theme and plugin icon is, must still render.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#ff0000"/></svg>"##;
        let path = env::temp_dir().join("wayrun-icon-test.svg");
        fs::write(&path, svg).unwrap();

        let (mut cache, _queued) = test_cache();
        let mut target = Pixmap::new(34, 34).unwrap();
        deliver(&mut cache, path.to_str().unwrap(), 30);
        cache.draw(&mut target, plain(path.to_str().unwrap(), 2.0, 2.0, 30));

        let centre = target.pixel(17, 17).unwrap();
        assert_eq!(centre.alpha(), 255, "the square is drawn");
        assert_eq!((centre.red(), centre.green(), centre.blue()), (255, 0, 0));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_greyscale_glyph_is_tinted_to_the_theme_colour() {
        // a `#444` silhouette, the shape a Papirus action icon has on this box
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#444444"/></svg>"##;
        let path = env::temp_dir().join("wayrun-icon-tint.svg");
        fs::write(&path, svg).unwrap();

        let (mut cache, _queued) = test_cache();
        let mut target = Pixmap::new(34, 34).unwrap();
        deliver_tinted(&mut cache, path.to_str().unwrap(), 30, [241, 223, 218]);
        cache.draw_tinted(
            &mut target,
            plain(path.to_str().unwrap(), 2.0, 2.0, 30),
            [241, 223, 218],
        );

        let centre = target.pixel(17, 17).unwrap();
        assert_eq!(centre.alpha(), 255);
        assert_eq!(
            (centre.red(), centre.green(), centre.blue()),
            (241, 223, 218)
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_coloured_glyph_keeps_its_own_colour() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#1e88e5"/></svg>"##;
        let path = env::temp_dir().join("wayrun-icon-notint.svg");
        fs::write(&path, svg).unwrap();

        let (mut cache, _queued) = test_cache();
        let mut target = Pixmap::new(34, 34).unwrap();
        deliver_tinted(&mut cache, path.to_str().unwrap(), 30, [241, 223, 218]);
        cache.draw_tinted(
            &mut target,
            plain(path.to_str().unwrap(), 2.0, 2.0, 30),
            [241, 223, 218],
        );

        let centre = target.pixel(17, 17).unwrap();
        assert_eq!(
            (centre.red(), centre.green(), centre.blue()),
            (0x1e, 0x88, 0xe5)
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn an_unreadable_icon_draws_nothing_rather_than_failing() {
        let (mut cache, queued) = test_cache();
        let mut target = Pixmap::new(30, 30).unwrap();
        deliver(&mut cache, "/nonexistent/icon.png", 30);

        cache.draw(&mut target, plain("/nonexistent/icon.png", 0.0, 0.0, 30));
        assert!(target.pixels().iter().all(|p| p.alpha() == 0));
        assert!(
            queued.try_recv().is_err(),
            "a cached miss is never asked for again"
        );
    }

    #[test]
    fn a_missing_icon_is_asked_for_once_and_drawn_when_it_arrives() {
        let path = red_png("wayrun-icon-late.png");
        let path = path.to_str().unwrap().to_string();
        let (mut cache, queued) = test_cache();
        let mut target = Pixmap::new(34, 34).unwrap();

        cache.draw(&mut target, plain(&path, 2.0, 2.0, 30));
        assert!(
            target.pixels().iter().all(|p| p.alpha() == 0),
            "nothing yet"
        );
        let (generation, key) = queued.try_recv().expect("the frame asks the worker");
        assert_eq!(
            key,
            IconKey::Plain {
                path: path.clone(),
                size: 30,
                shape: IconShape::Square,
            }
        );

        // the same miss must not queue the decode twice
        cache.draw(&mut target, plain(&path, 2.0, 2.0, 30));
        assert!(queued.try_recv().is_err());

        cache.insert(generation, key, render(&path, 30));
        cache.draw(&mut target, plain(&path, 2.0, 2.0, 30));
        assert_eq!(target.pixel(17, 17).unwrap().alpha(), 255);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_late_icon_forgets_the_cached_miss() {
        let path = "/nonexistent/icon.png";
        let (mut cache, queued) = test_cache();
        let mut target = Pixmap::new(30, 30).unwrap();

        cache.draw(&mut target, plain(path, 0.0, 0.0, 30));
        let (generation, key) = queued.try_recv().expect("the frame asks the worker");
        cache.insert(generation, key, None);
        cache.draw(&mut target, plain(path, 0.0, 0.0, 30));
        assert!(
            queued.try_recv().is_err(),
            "a cached miss is not asked for again"
        );

        cache.forget(path);
        cache.draw(&mut target, plain(path, 0.0, 0.0, 30));
        assert!(
            queued.try_recv().is_ok(),
            "the file that just appeared is asked for"
        );
    }

    #[test]
    fn a_result_from_a_cleared_generation_is_dropped() {
        let path = red_png("wayrun-icon-cleared.png");
        let path = path.to_str().unwrap().to_string();
        let (mut cache, queued) = test_cache();
        let mut target = Pixmap::new(34, 34).unwrap();

        cache.draw(&mut target, plain(&path, 2.0, 2.0, 30));
        let (generation, key) = queued.try_recv().unwrap();
        cache.clear();
        cache.insert(generation, key, render(&path, 30));

        cache.draw(&mut target, plain(&path, 2.0, 2.0, 30));
        assert!(
            target.pixels().iter().all(|p| p.alpha() == 0),
            "a dismissed show's icon stays out"
        );
        assert!(queued.try_recv().is_ok(), "the cleared miss asks again");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn every_bundled_glyph_renders() {
        let (mut cache, _queued) = test_cache();
        let mut checked = 0;
        for (name, _) in builtin::GLYPHS {
            let path = format!("builtin:{name}");
            let mut target = Pixmap::new(64, 64).unwrap();
            deliver(&mut cache, &path, 64);
            cache.draw(&mut target, plain(&path, 0.0, 0.0, 64));
            assert!(
                target.pixels().iter().any(|p| p.alpha() > 0),
                "{path} rendered empty"
            );
            checked += 1;
        }
        assert_eq!(
            checked,
            wayrun_core::wire::BUILTIN_GLYPHS.len(),
            "the compiled table and the wire vocabulary agree"
        );
    }
}
