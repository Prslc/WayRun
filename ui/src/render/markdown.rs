use std::mem;
use std::rc::Rc;

use cosmic_text::Weight;

use crate::config::FontConfig;
use crate::markdown::{Block, ListItem, Span};
use crate::text::{Shaped, TextEngine, TextStyle, line_height, split_wide};
use crate::theme::tint;
use crate::view::Frame;

use crate::geom::Rect;

use super::DrawCtx;

#[derive(Clone, Copy)]
enum Tone {
    Text,
    Heading,
    Accent,
    Muted,
}

#[derive(Clone, Copy)]
struct Palette {
    text: [u8; 4],
    heading: [u8; 4],
    accent: [u8; 4],
    muted: [u8; 4],
    hover: [u8; 4],
    bar: [u8; 4],
    rule: [u8; 4],
}

impl Palette {
    fn of(frame: &Frame<'_>) -> Self {
        let fg = frame.theme.fg;
        Self {
            text: tint(fg, 0.85),
            heading: tint(fg, 1.0),
            accent: frame.surfaces.accent,
            muted: frame.surfaces.summary,
            hover: frame.surfaces.hover,
            bar: frame.surfaces.muted,
            rule: frame.surfaces.hairline,
        }
    }

    fn tone(&self, tone: Tone) -> [u8; 4] {
        match tone {
            Tone::Text => self.text,
            Tone::Heading => self.heading,
            Tone::Accent => self.accent,
            Tone::Muted => self.muted,
        }
    }
}

/// The layouter works in physical pixels, so shaping metrics apply directly;
/// the content rect is converted to them once, at the draw entry.
#[derive(Clone, Copy)]
struct Viewport {
    origin_x: f32,
    origin_y: f32,
    max_w: f32,
    max_h: f32,
    scale: f32,
}

struct Run {
    shaped: Rc<Shaped>,
    x: f32,
    top: f32,
    tone: Tone,
}

struct Line {
    top: f32,
    baseline: f32,
    runs: Vec<Run>,
}

struct Background {
    rect: Rect,
    color: [u8; 4],
    radius: f32,
}

#[derive(Default)]
struct Layout {
    lines: Vec<Line>,
    extras: Vec<Run>,
    backgrounds: Vec<Background>,
    strikes: Vec<Background>,
}

struct Pending {
    text: String,
    style: TextStyle,
    tone: Tone,
    x: f32,
    inline_code: bool,
    strike: bool,
}

struct Layouter<'a> {
    text: &'a mut TextEngine,
    font: FontConfig,
    palette: Palette,
    viewport: Viewport,
    base: f32,
    y: f32,
    started: bool,
    layout: Layout,
}

impl<'a> Layouter<'a> {
    fn new(
        text: &'a mut TextEngine,
        font: FontConfig,
        palette: Palette,
        viewport: Viewport,
    ) -> Self {
        let base = font.title() * viewport.scale;
        Self {
            text,
            font,
            palette,
            viewport,
            base,
            y: viewport.origin_y,
            started: false,
            layout: Layout::default(),
        }
    }

    fn blocks(&mut self, blocks: &[Block], indent: f32, tone: Tone) {
        for block in blocks {
            if self.y >= self.viewport.origin_y + self.viewport.max_h {
                return;
            }
            match block {
                Block::Paragraph(spans) => {
                    self.paragraph(spans, self.base, Weight::NORMAL, indent, tone);
                    self.y += 0.55 * self.base;
                }
                Block::Heading { level, spans } => {
                    if self.started {
                        self.y += 0.75 * self.base;
                    }
                    let size = self.font.heading(*level) * self.viewport.scale;
                    self.paragraph(spans, size, Weight::BOLD, indent, Tone::Heading);
                    self.y += 0.3 * self.base;
                }
                Block::Code(lines) => {
                    if self.started {
                        self.y += 0.55 * self.base;
                    }
                    self.code(lines, indent, tone);
                    self.y += 0.55 * self.base;
                }
                Block::List {
                    ordered,
                    start,
                    items,
                } => self.list(*ordered, *start, items, indent, tone),
                Block::Quote(inner) => self.quote(inner, indent),
                Block::Rule => {
                    if self.started {
                        self.y += 0.55 * self.base;
                    }
                    self.rule(indent);
                    self.y += 0.55 * self.base;
                }
            }
            self.started = true;
        }
    }

    fn paragraph(&mut self, spans: &[Span], size: f32, weight: Weight, indent: f32, tone: Tone) {
        let tokens = tokens(spans, weight, tone);
        let max_line = (self.viewport.max_w - indent).max(1.0);
        let space = self
            .text
            .shape_styled(" ", size, TextStyle::normal(weight))
            .width;
        let mut pending: Vec<Pending> = Vec::new();
        let mut line_w = 0.0_f32;

        for token in tokens {
            if self.y >= self.viewport.origin_y + self.viewport.max_h {
                break;
            }
            if token.hard_break && !pending.is_empty() {
                self.flush(&mut pending, size);
                line_w = 0.0;
            }
            let shaped = self.text.shape_styled(&token.text, size, token.style).width;
            let gap = if pending.is_empty() || !token.space_before {
                0.0
            } else {
                space
            };
            if !pending.is_empty() && line_w + gap + shaped > max_line {
                self.flush(&mut pending, size);
                line_w = 0.0;
            }
            if shaped > max_line {
                if !pending.is_empty() {
                    self.flush(&mut pending, size);
                }
                let line_h = line_height(size);
                let remaining = self.viewport.origin_y + self.viewport.max_h - self.y;
                let budget = ((remaining / line_h).ceil() as usize).max(1);
                for chunk in split_wide(self.text, &token.text, size, token.style, max_line, budget)
                {
                    pending.push(Pending {
                        text: chunk,
                        style: token.style,
                        tone: token.tone,
                        x: self.viewport.origin_x + indent,
                        inline_code: token.inline_code,
                        strike: token.strike,
                    });
                    self.flush(&mut pending, size);
                    if self.y >= self.viewport.origin_y + self.viewport.max_h {
                        break;
                    }
                }
                line_w = 0.0;
                continue;
            }
            let gap = if pending.is_empty() {
                0.0
            } else if token.space_before {
                space
            } else {
                0.0
            };
            pending.push(Pending {
                text: token.text,
                style: token.style,
                tone: token.tone,
                x: self.viewport.origin_x + indent + line_w + gap,
                inline_code: token.inline_code,
                strike: token.strike,
            });
            line_w += gap + shaped;
        }
        self.flush(&mut pending, size);
    }

    fn flush(&mut self, pending: &mut Vec<Pending>, size: f32) {
        if pending.is_empty() {
            return;
        }
        let top = self.y;
        let mut baseline = 0.0_f32;
        let mut height = 0.0_f32;
        let mut shaped = Vec::with_capacity(pending.len());
        for item in pending.drain(..) {
            let run = self.text.shape_styled(&item.text, size, item.style);
            baseline = baseline.max(run.baseline);
            height = height.max(run.height);
            shaped.push((item, run));
        }

        let mut runs = Vec::with_capacity(shaped.len());
        for (item, run) in shaped {
            if item.inline_code {
                let pad = self.viewport.scale;
                self.backgrounds(
                    item.x - 2.0 * self.viewport.scale,
                    top + pad,
                    run.width + 4.0 * self.viewport.scale,
                    (height - 2.0 * pad).max(1.0),
                    self.palette.hover,
                    3.0,
                );
            }
            if item.strike {
                self.strike(
                    item.x,
                    top + baseline - 0.32 * size,
                    run.width,
                    1.0 * self.viewport.scale,
                    self.palette.tone(item.tone),
                );
            }
            runs.push(Run {
                x: item.x,
                top: top + (baseline - run.baseline),
                shaped: run,
                tone: item.tone,
            });
        }

        self.layout.lines.push(Line {
            top,
            baseline,
            runs,
        });
        self.y += height;
    }

    fn list(&mut self, ordered: bool, start: u64, items: &[ListItem], indent: f32, tone: Tone) {
        if self.started {
            self.y += 0.55 * self.base;
        }
        let content = indent + 1.5 * self.base;
        for (index, item) in items.iter().enumerate() {
            if self.y >= self.viewport.origin_y + self.viewport.max_h {
                return;
            }
            let marker = if ordered {
                format!("{}.", start + index as u64)
            } else {
                "•".to_string()
            };
            let item_top = self.y;
            self.blocks(&item.blocks, content, tone);
            if let Some(line) = self
                .layout
                .lines
                .iter()
                .find(|line| line.top >= item_top - 0.01)
            {
                let shaped =
                    self.text
                        .shape_styled(&marker, self.base, TextStyle::normal(Weight::NORMAL));
                let top = line.top + line.baseline - shaped.baseline;
                self.layout.extras.push(Run {
                    shaped,
                    x: self.viewport.origin_x + indent,
                    top,
                    tone: Tone::Muted,
                });
            }
            if index + 1 < items.len() {
                self.y += 0.2 * self.base;
            }
        }
    }

    fn quote(&mut self, inner: &[Block], indent: f32) {
        if self.started {
            self.y += 0.55 * self.base;
        }
        let start = self.y;
        let bar_x = self.viewport.origin_x + indent;
        self.blocks(inner, indent + 0.8 * self.base, Tone::Muted);
        let height = (self.y - start).max(1.0);
        self.backgrounds(
            bar_x,
            start,
            2.0 * self.viewport.scale,
            height,
            self.palette.bar,
            0.0,
        );
    }

    fn code(&mut self, lines: &[String], indent: f32, tone: Tone) {
        let pad = 0.45 * self.base;
        let pad_x = 6.0 * self.viewport.scale;
        let start = self.y;
        self.y += pad;
        let style = TextStyle {
            weight: Weight::NORMAL,
            italic: false,
            mono: true,
        };
        for line in lines {
            if self.y >= self.viewport.origin_y + self.viewport.max_h {
                break;
            }
            let shaped = self.text.shape_styled(line, self.base, style);
            let top = self.y;
            let height = shaped.height;
            self.layout.lines.push(Line {
                top,
                baseline: shaped.baseline,
                runs: vec![Run {
                    shaped,
                    x: self.viewport.origin_x + indent + pad_x,
                    top,
                    tone,
                }],
            });
            self.y += height;
        }
        let end = self.y + pad;
        self.backgrounds(
            self.viewport.origin_x + indent,
            start,
            (self.viewport.max_w - indent).max(1.0),
            end - start,
            self.palette.hover,
            4.0,
        );
        self.y = end;
    }

    fn rule(&mut self, indent: f32) {
        self.backgrounds(
            self.viewport.origin_x + indent,
            self.y,
            (self.viewport.max_w - indent).max(1.0),
            1.0 * self.viewport.scale,
            self.palette.rule,
            0.0,
        );
        self.y += 1.0 * self.viewport.scale;
    }

    fn backgrounds(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4], radius: f32) {
        self.layout.backgrounds.push(Background {
            rect: Rect { x, y, w, h },
            color,
            radius,
        });
    }

    fn strike(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        self.layout.strikes.push(Background {
            rect: Rect { x, y, w, h },
            color,
            radius: 0.0,
        });
    }

    fn finish(self) -> Layout {
        self.layout
    }
}

struct Token {
    text: String,
    style: TextStyle,
    tone: Tone,
    inline_code: bool,
    strike: bool,
    space_before: bool,
    hard_break: bool,
}

fn tokens(spans: &[Span], weight: Weight, tone: Tone) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut space = false;
    let mut hard = false;
    for span in spans {
        let style = TextStyle {
            weight: if span.style.bold {
                Weight::BOLD
            } else {
                weight
            },
            italic: span.style.italic,
            mono: span.style.code,
        };
        let span_tone = if span.style.link { Tone::Accent } else { tone };
        if span.style.code {
            for (index, part) in span.text.split('\n').enumerate() {
                if index > 0 {
                    hard = true;
                }
                if part.is_empty() {
                    continue;
                }
                tokens.push(Token {
                    text: part.to_string(),
                    style,
                    tone: span_tone,
                    inline_code: true,
                    strike: span.style.strike,
                    space_before: space,
                    hard_break: hard,
                });
                space = false;
                hard = false;
            }
            continue;
        }
        let mut word = String::new();
        for ch in span.text.chars() {
            if ch == '\n' {
                push_word(
                    &mut tokens,
                    &mut word,
                    style,
                    span_tone,
                    span.style.strike,
                    &mut space,
                    &mut hard,
                );
                hard = true;
            } else if ch.is_whitespace() {
                push_word(
                    &mut tokens,
                    &mut word,
                    style,
                    span_tone,
                    span.style.strike,
                    &mut space,
                    &mut hard,
                );
                space = true;
            } else {
                word.push(ch);
            }
        }
        push_word(
            &mut tokens,
            &mut word,
            style,
            span_tone,
            span.style.strike,
            &mut space,
            &mut hard,
        );
    }
    tokens
}

fn push_word(
    tokens: &mut Vec<Token>,
    word: &mut String,
    style: TextStyle,
    tone: Tone,
    strike: bool,
    space: &mut bool,
    hard: &mut bool,
) {
    if word.is_empty() {
        return;
    }
    tokens.push(Token {
        text: mem::take(word),
        style,
        tone,
        inline_code: false,
        strike,
        space_before: *space,
        hard_break: *hard,
    });
    *space = false;
    *hard = false;
}

pub(super) fn draw_markdown(ctx: &mut DrawCtx, content: Rect, body: &str) {
    if content.w <= 0.0 || content.h <= 0.0 {
        return;
    }
    let frame = ctx.frame;
    let canvas = ctx.canvas;
    let document = ctx.markdown.get(body);
    let scale = canvas.scale;
    let max_w = canvas.px(content.w).max(1.0);
    let max_h = canvas.px(content.h).max(1.0);
    let origin_x = canvas.px(content.x);
    let origin_y = canvas.px(content.y);
    let clip_content = Rect {
        x: origin_x,
        y: origin_y,
        w: max_w,
        h: max_h,
    };
    let palette = Palette::of(frame);
    let viewport = Viewport {
        origin_x,
        origin_y,
        max_w,
        max_h,
        scale,
    };
    let mut layouter = Layouter::new(ctx.text, frame.font.clone(), palette, viewport);
    layouter.blocks(&document.blocks, 0.0, Tone::Text);
    let layout = layouter.finish();

    for background in &layout.backgrounds {
        if let Some(rect) = clip_rect(background.rect, clip_content) {
            canvas.fill_round_px(
                ctx.pixmap,
                rect,
                background.radius,
                frame.fade_rgba(background.color),
            );
        }
    }

    let clip = [origin_x, origin_y, max_w, max_h];
    for line in &layout.lines {
        for run in &line.runs {
            ctx.text.draw(
                ctx.pixmap,
                &run.shaped,
                frame.fade_rgba(palette.tone(run.tone)),
                run.x,
                run.top,
                Some(clip),
            );
        }
    }
    for run in &layout.extras {
        ctx.text.draw(
            ctx.pixmap,
            &run.shaped,
            frame.fade_rgba(palette.tone(run.tone)),
            run.x,
            run.top,
            Some(clip),
        );
    }
    for strike in &layout.strikes {
        if let Some(rect) = clip_rect(strike.rect, clip_content) {
            canvas.fill_round_px(ctx.pixmap, rect, 0.0, frame.fade_rgba(strike.color));
        }
    }
}

fn clip_rect(rect: Rect, pane: Rect) -> Option<Rect> {
    let x = rect.x.max(pane.x);
    let y = rect.y.max(pane.y);
    let right = rect.right().min(pane.right());
    let bottom = rect.bottom().min(pane.bottom());
    (right > x && bottom > y).then_some(Rect {
        x,
        y,
        w: right - x,
        h: bottom - y,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse;

    fn palette() -> Palette {
        Palette {
            text: [255, 255, 255, 255],
            heading: [255, 255, 255, 255],
            accent: [255, 255, 255, 255],
            muted: [200, 200, 200, 255],
            hover: [40, 40, 40, 255],
            bar: [120, 120, 120, 255],
            rule: [80, 80, 80, 255],
        }
    }

    fn layout(text: &mut TextEngine, max_w: f32, source: &str) -> Layout {
        let viewport = Viewport {
            origin_x: 0.0,
            origin_y: 0.0,
            max_w,
            max_h: 1000.0,
            scale: 1.0,
        };
        let mut layouter = Layouter::new(text, FontConfig::default(), palette(), viewport);
        layouter.blocks(&parse(source).blocks, 0.0, Tone::Text);
        layouter.finish()
    }

    #[test]
    fn a_long_line_wraps_inside_the_column() {
        let mut text = TextEngine::with_family("Source Han Sans CN".to_string());
        let layout = layout(&mut text, 80.0, "one two three four five six seven eight");
        assert!(layout.lines.len() > 1);
        for line in &layout.lines {
            for run in &line.runs {
                assert!(
                    run.x + run.shaped.width <= 81.0,
                    "{} > 80",
                    run.x + run.shaped.width
                );
            }
        }
    }

    #[test]
    fn an_overlong_cjk_run_is_split_by_character() {
        let mut text = TextEngine::with_family("Source Han Sans CN".to_string());
        let layout = layout(&mut text, 40.0, "中文中文中文中文中文中文中文中文");
        assert!(layout.lines.len() > 1);
        for line in &layout.lines {
            for run in &line.runs {
                assert!(run.x + run.shaped.width <= 41.0);
            }
        }
    }

    #[test]
    fn a_code_line_is_not_wrapped() {
        let mut text = TextEngine::with_family("Source Han Sans CN".to_string());
        let source = "```\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n```";
        let layout = layout(&mut text, 40.0, source);
        assert_eq!(layout.lines.len(), 1);
        assert!(layout.lines[0].runs[0].shaped.width > 40.0);
        assert_eq!(layout.backgrounds.len(), 1, "the code block's fill");
    }

    #[test]
    fn inline_code_gets_a_background_and_a_heading_its_size() {
        let mut text = TextEngine::with_family("Source Han Sans CN".to_string());
        let layout = layout(&mut text, 400.0, "# Heading\n\nbody `code`");
        let heading = layout.lines[0].runs[0].shaped.height;
        let body = layout.lines[1].runs[0].shaped.height;
        assert!(heading > body);
        assert!(!layout.backgrounds.is_empty(), "the inline code's fill");
    }

    #[test]
    fn a_hard_break_starts_a_new_line() {
        let mut text = TextEngine::with_family("Source Han Sans CN".to_string());
        let layout = layout(&mut text, 400.0, "first\\\nsecond");
        assert_eq!(layout.lines.len(), 2);
        assert!(layout.lines[0].runs[0].shaped.width > 0.0);
        assert!(layout.lines[1].runs[0].shaped.width > 0.0);
    }

    #[test]
    fn list_items_get_a_marker_run() {
        let mut text = TextEngine::with_family("Source Han Sans CN".to_string());
        let layout = layout(&mut text, 400.0, "- one\n- two");
        assert_eq!(layout.extras.len(), 2);
        assert_eq!(layout.extras[0].shaped.width, layout.extras[1].shaped.width);
    }
}
