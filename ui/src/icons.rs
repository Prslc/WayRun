mod builtin;
mod render;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;

use tiny_skia::{Pixmap, PixmapPaint, Transform};

use self::render::{render, render_glyph};

/// One decode a frame asked for: what the worker renders and the key it lands
/// under, so a repeat ask is told apart from a first one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconKey {
    Plain {
        path: String,
        size: u32,
    },
    Tinted {
        path: String,
        size: u32,
        color: [u8; 3],
    },
}

pub struct IconCache {
    /// path → (box size → premultiplied RGBA). The key is a `String` so a
    /// per-frame lookup borrows a `&str`; `None` marks an unreadable file.
    entries: HashMap<String, HashMap<u32, Option<Pixmap>>>,
    /// The same bitmap recoloured to the theme foreground, keyed by colour too,
    /// so a live theme change does not show the old tint.
    #[allow(clippy::type_complexity)] // two levels, so a lookup borrows the path
    tinted: HashMap<String, HashMap<(u32, [u8; 3]), Option<Pixmap>>>,
    /// Keys asked for but not yet delivered, so a frame never queues a decode
    /// the worker is already on.
    pending: HashSet<IconKey>,
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
        self.generation = self.generation.wrapping_add(1);
    }

    /// Drop the recoloured bitmaps: a tint is keyed by the colour it was mixed
    /// from, so a theme change leaves every entry unreachable.
    pub fn drop_tinted(&mut self) {
        self.tinted.clear();
    }

    /// Ask the worker for `path`, unless it is cached or already asked for; a
    /// payload that arrives while the launcher is hidden lands before it shows.
    pub fn warm(&mut self, path: &str, size: u32) {
        if self
            .entries
            .get(path)
            .is_none_or(|by_size| !by_size.contains_key(&size))
        {
            self.request(IconKey::Plain {
                path: path.to_string(),
                size,
            });
        }
    }

    /// Like [`IconCache::warm`], for a glyph drawn through [`IconCache::draw_tinted`].
    pub fn warm_tinted(&mut self, path: &str, size: u32, color: [u8; 3]) {
        if self
            .tinted
            .get(path)
            .is_none_or(|by_color| !by_color.contains_key(&(size, color)))
        {
            self.request(IconKey::Tinted {
                path: path.to_string(),
                size,
                color,
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
            IconKey::Plain { path, size } => {
                self.entries.entry(path).or_default().insert(size, icon);
            }
            IconKey::Tinted { path, size, color } => {
                self.tinted
                    .entry(path)
                    .or_default()
                    .insert((size, color), icon);
            }
        }
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
        let key = (size, color);
        if self
            .tinted
            .get(path)
            .is_none_or(|by_color| !by_color.contains_key(&key))
        {
            self.request(IconKey::Tinted {
                path: path.to_string(),
                size,
                color,
            });
            return;
        }
        // the key is known here, so a `None` inside is a cached unreadable file
        let Some(Some(icon)) = self
            .tinted
            .get(path)
            .and_then(|by_color| by_color.get(&key))
        else {
            return;
        };

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

    /// Draw the icon contained in a `size`×`size` box at `(x, y)`, at `opacity`.
    /// A missing file is asked for, never rasterised here: no frame decodes.
    pub fn draw(
        &mut self,
        target: &mut Pixmap,
        path: &str,
        x: f32,
        y: f32,
        size: u32,
        opacity: f32,
    ) {
        // `get` borrows the path as `&str`, so a cache hit allocates nothing.
        if self
            .entries
            .get(path)
            .is_none_or(|by_size| !by_size.contains_key(&size))
        {
            self.request(IconKey::Plain {
                path: path.to_string(),
                size,
            });
            return;
        }
        // the key is known here, so a `None` inside is a cached unreadable file
        let Some(Some(icon)) = self
            .entries
            .get(path)
            .and_then(|by_size| by_size.get(&size))
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
    let (jobs, queue) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok((generation, key)) = queue.recv() {
            let icon = match &key {
                IconKey::Plain { path, size } => render(path, *size),
                IconKey::Tinted { path, size, color } => render_glyph(path, *size, *color),
            };
            if !done((generation, key, icon)) {
                break;
            }
        }
    });
    jobs
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

    /// The cache plus the queue the worker would drain.
    fn test_cache() -> (IconCache, std::sync::mpsc::Receiver<(u64, IconKey)>) {
        let (jobs, queued) = std::sync::mpsc::channel();
        (IconCache::new(jobs), queued)
    }

    /// Store what the worker would deliver for a plain icon.
    fn deliver(cache: &mut IconCache, path: &str, size: u32) {
        let key = IconKey::Plain {
            path: path.to_string(),
            size,
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

        let (mut cache, _queued) = test_cache();
        let mut target = Pixmap::new(34, 34).unwrap();
        deliver(&mut cache, path.to_str().unwrap(), 30);
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

        let (mut cache, _queued) = test_cache();
        let mut target = Pixmap::new(34, 34).unwrap();
        deliver_tinted(&mut cache, path.to_str().unwrap(), 30, [241, 223, 218]);
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

        let (mut cache, _queued) = test_cache();
        let mut target = Pixmap::new(34, 34).unwrap();
        deliver_tinted(&mut cache, path.to_str().unwrap(), 30, [241, 223, 218]);
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
        let (mut cache, queued) = test_cache();
        let mut target = Pixmap::new(30, 30).unwrap();
        deliver(&mut cache, "/nonexistent/icon.png", 30);

        cache.draw(&mut target, "/nonexistent/icon.png", 0.0, 0.0, 30, 1.0);
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

        cache.draw(&mut target, &path, 2.0, 2.0, 30, 1.0);
        assert!(
            target.pixels().iter().all(|p| p.alpha() == 0),
            "nothing yet"
        );
        let (generation, key) = queued.try_recv().expect("the frame asks the worker");
        assert_eq!(
            key,
            IconKey::Plain {
                path: path.clone(),
                size: 30
            }
        );

        // the same miss must not queue the decode twice
        cache.draw(&mut target, &path, 2.0, 2.0, 30, 1.0);
        assert!(queued.try_recv().is_err());

        cache.insert(generation, key, render(&path, 30));
        cache.draw(&mut target, &path, 2.0, 2.0, 30, 1.0);
        assert_eq!(target.pixel(17, 17).unwrap().alpha(), 255);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_result_from_a_cleared_generation_is_dropped() {
        let path = red_png("wayrun-icon-cleared.png");
        let path = path.to_str().unwrap().to_string();
        let (mut cache, queued) = test_cache();
        let mut target = Pixmap::new(34, 34).unwrap();

        cache.draw(&mut target, &path, 2.0, 2.0, 30, 1.0);
        let (generation, key) = queued.try_recv().unwrap();
        cache.clear();
        cache.insert(generation, key, render(&path, 30));

        cache.draw(&mut target, &path, 2.0, 2.0, 30, 1.0);
        assert!(
            target.pixels().iter().all(|p| p.alpha() == 0),
            "a dismissed show's icon stays out"
        );
        assert!(queued.try_recv().is_ok(), "the cleared miss asks again");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn every_bundled_glyph_renders() {
        let (mut cache, _queued) = test_cache();
        let mut checked = 0;
        for (name, _) in builtin::GLYPHS {
            let path = format!("builtin:{name}");
            let mut target = Pixmap::new(64, 64).unwrap();
            deliver(&mut cache, &path, 64);
            cache.draw(&mut target, &path, 0.0, 0.0, 64, 1.0);
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
