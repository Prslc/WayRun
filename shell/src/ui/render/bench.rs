use std::time::Instant;

use tiny_skia::{Color, FillRule, Pixmap, Transform};

use super::canvas::{Canvas, Rect, round_rect};

/// A micro-benchmark for the software rasteriser, run by `wayrun bench`.
pub fn bench() {
    let mut pixmap = Pixmap::new(1920, 1080).unwrap();
    let repeats = 20;

    let at = Instant::now();
    for _ in 0..repeats {
        pixmap.fill(Color::from_rgba8(0, 0, 0, crate::ui::theme::DEFAULT_DIM[3]));
    }
    println!("pixmap.fill (2.07M px): {:?}/frame", at.elapsed() / repeats);

    let square = Rect {
        x: 595.0,
        y: 302.0,
        w: 730.0,
        h: 460.0,
    };
    let square_path = round_rect(square, 0.0).unwrap();
    let paint = Canvas::paint([36, 40, 59, 184]);
    let at = Instant::now();
    for _ in 0..repeats {
        pixmap.fill_path(
            &square_path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    println!(
        "fill_path square  {}x{}: {:?}/frame",
        square.w,
        square.h,
        at.elapsed() / repeats
    );

    let round_path = round_rect(square, 16.0).unwrap();
    let at = Instant::now();
    for _ in 0..repeats {
        pixmap.fill_path(
            &round_path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    println!(
        "fill_path rounded {}x{}: {:?}/frame",
        square.w,
        square.h,
        at.elapsed() / repeats
    );
}
