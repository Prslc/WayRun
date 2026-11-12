use std::time::Instant;

use tiny_skia::Pixmap;

use crate::icons::IconCache;
use crate::markdown::MarkdownCache;
use crate::text::TextEngine;
use crate::view::Frame;

use self::canvas::Canvas;

pub use self::bench::bench;
pub use self::field::{caret_rect, draw_caret};
pub use self::frame::draw;
use self::row::{RowBody, RowWindow, draw_row};

/// What a component draws with: the canvas, the pixmap, the caches and the frame
/// snapshot, so a component reads no state of its own.
pub(super) struct DrawCtx<'a, 'f> {
    pub canvas: Canvas,
    pub pixmap: &'a mut Pixmap,
    pub text: &'a mut TextEngine,
    pub icons: &'a mut IconCache,
    pub markdown: &'a mut MarkdownCache,
    pub now: Instant,
    pub frame: &'a Frame<'f>,
}

mod bench;
mod canvas;
mod card;
mod field;
mod footer;
mod frame;
mod list;
mod markdown;
mod panel;
mod preview;
mod row;
