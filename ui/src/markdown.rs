use std::mem;
use std::rc::Rc;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// A parsed preview: the blocks the renderer lays out in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Code(Vec<String>),
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
    },
    Quote(Vec<Block>),
    Rule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: InlineStyle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
}

/// The last parsed preview, keyed by its source; one entry, cleared on dismiss.
#[derive(Default)]
pub struct MarkdownCache {
    document: Option<(String, Rc<Document>)>,
}

impl MarkdownCache {
    pub fn get(&mut self, source: &str) -> Rc<Document> {
        if let Some((key, document)) = &self.document
            && key == source
        {
            return Rc::clone(document);
        }
        let document = Rc::new(parse(source));
        self.document = Some((source.to_string(), Rc::clone(&document)));
        document
    }

    pub fn clear(&mut self) {
        self.document = None;
    }
}

pub fn parse(text: &str) -> Document {
    let options = Options::ENABLE_STRIKETHROUGH;
    let mut builder = Builder {
        frames: vec![Frame::Blocks(Vec::new())],
        depth: 0,
    };
    for event in Parser::new_ext(text, options) {
        builder.event(event);
    }
    builder.finish()
}

/// Containers deeper than this are flattened, so a pathological document cannot
/// make the layout recurse without a bound.
const MAX_DEPTH: usize = 8;

struct Builder {
    frames: Vec<Frame>,
    depth: usize,
}

enum Frame {
    Blocks(Vec<Block>),
    Quote {
        blocks: Vec<Block>,
        clamped: bool,
    },
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
        item: Option<Vec<Block>>,
        clamped: bool,
    },
    Inline(Inline),
    Code(String),
    Html,
}

struct Inline {
    heading: Option<u8>,
    spans: Vec<Span>,
    bold: u32,
    italic: u32,
    strike: u32,
    link: u32,
}

impl Builder {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match self.frames.last_mut() {
                Some(Frame::Code(buffer)) => buffer.push_str(&text),
                Some(Frame::Html) => {}
                _ => {
                    self.inline(|inline| inline.push(&text, false));
                }
            },
            Event::Code(text) => self.inline(|inline| inline.push(&text, true)),
            Event::SoftBreak => self.inline(|inline| inline.push(" ", false)),
            Event::HardBreak => self.inline(|inline| inline.push("\n", false)),
            Event::Rule => self.push_block(Block::Rule),
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::FootnoteReference(_)
            | Event::TaskListMarker(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.push_inline(None),
            Tag::Heading { level, .. } => self.push_inline(Some(level as u8)),
            Tag::BlockQuote(_) => {
                self.close_inline();
                let clamped = self.depth >= MAX_DEPTH;
                self.depth += 1;
                self.frames.push(Frame::Quote {
                    blocks: Vec::new(),
                    clamped,
                });
            }
            Tag::CodeBlock(_) => {
                self.close_inline();
                self.frames.push(Frame::Code(String::new()));
            }
            Tag::List(start) => {
                self.close_inline();
                let clamped = self.depth >= MAX_DEPTH;
                self.depth += 1;
                self.frames.push(Frame::List {
                    ordered: start.is_some(),
                    start: start.unwrap_or(1),
                    items: Vec::new(),
                    item: None,
                    clamped,
                });
            }
            Tag::Item => {
                self.close_inline();
                if let Some(Frame::List { item, .. }) = self.frames.last_mut() {
                    *item = Some(Vec::new());
                }
            }
            Tag::HtmlBlock => {
                self.close_inline();
                self.frames.push(Frame::Html);
            }
            Tag::Strong => self.inline(|inline| inline.bold += 1),
            Tag::Emphasis => self.inline(|inline| inline.italic += 1),
            Tag::Strikethrough => self.inline(|inline| inline.strike += 1),
            Tag::Link { .. } => self.inline(|inline| inline.link += 1),
            Tag::Image { .. } => {}
            Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_)
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.close_inline(),
            TagEnd::Heading(_) => self.close_inline(),
            TagEnd::BlockQuote(_) => {
                self.close_inline();
                let Some(Frame::Quote { blocks, clamped }) = self.pop_to_quote() else {
                    return;
                };
                self.depth = self.depth.saturating_sub(1);
                if clamped {
                    self.extend_blocks(blocks);
                } else {
                    self.push_block(Block::Quote(blocks));
                }
            }
            TagEnd::CodeBlock => {
                let Some(Frame::Code(buffer)) = self.pop_to_code() else {
                    return;
                };
                let lines = code_lines(&buffer);
                if !lines.is_empty() {
                    self.push_block(Block::Code(lines));
                }
            }
            TagEnd::List(_) => {
                self.close_inline();
                let Some(Frame::List {
                    ordered,
                    start,
                    mut items,
                    item,
                    clamped,
                }) = self.pop_to_list()
                else {
                    return;
                };
                self.depth = self.depth.saturating_sub(1);
                if let Some(blocks) = item
                    && !blocks.is_empty()
                {
                    items.push(ListItem { blocks });
                }
                if clamped {
                    self.extend_blocks(items.into_iter().flat_map(|item| item.blocks));
                } else if !items.is_empty() {
                    self.push_block(Block::List {
                        ordered,
                        start,
                        items,
                    });
                }
            }
            TagEnd::Item => {
                self.close_inline();
                let Some(Frame::List { items, item, .. }) = self.frames.last_mut() else {
                    return;
                };
                if let Some(blocks) = item.take()
                    && !blocks.is_empty()
                {
                    items.push(ListItem { blocks });
                }
            }
            TagEnd::HtmlBlock => {
                if matches!(self.frames.last(), Some(Frame::Html)) {
                    self.frames.pop();
                }
            }
            TagEnd::Strong => self.style(|inline| inline.bold = inline.bold.saturating_sub(1)),
            TagEnd::Emphasis => {
                self.style(|inline| inline.italic = inline.italic.saturating_sub(1))
            }
            TagEnd::Strikethrough => {
                self.style(|inline| inline.strike = inline.strike.saturating_sub(1));
            }
            TagEnd::Link => self.style(|inline| inline.link = inline.link.saturating_sub(1)),
            TagEnd::Image => {}
            TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_)
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }

    fn finish(mut self) -> Document {
        self.close_inline();
        let blocks = match self.frames.first_mut() {
            Some(Frame::Blocks(blocks)) => mem::take(blocks),
            _ => Vec::new(),
        };
        Document { blocks }
    }

    fn push_inline(&mut self, heading: Option<u8>) {
        self.close_inline();
        self.frames.push(Frame::Inline(Inline {
            heading,
            spans: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            link: 0,
        }));
    }

    /// Run `f` on the open inline frame, opening an implicit paragraph for a
    /// tight list item's bare text.
    fn inline(&mut self, f: impl FnOnce(&mut Inline)) {
        if !matches!(self.frames.last(), Some(Frame::Inline(_))) {
            self.push_inline(None);
        }
        if let Some(Frame::Inline(inline)) = self.frames.last_mut() {
            f(inline);
        }
    }

    fn style(&mut self, f: impl FnOnce(&mut Inline)) {
        if let Some(Frame::Inline(inline)) = self.frames.last_mut() {
            f(inline);
        }
    }

    fn close_inline(&mut self) {
        let Some(Frame::Inline(inline)) = self.frames.last() else {
            return;
        };
        if inline.spans.iter().all(|span| span.text.trim().is_empty()) {
            self.frames.pop();
            return;
        }
        let spans = inline.spans.clone();
        let heading = inline.heading;
        self.frames.pop();
        let block = match heading {
            Some(level) => Block::Heading { level, spans },
            None => Block::Paragraph(spans),
        };
        if let Some(target) = self.target() {
            target.push(block);
        }
    }

    fn push_block(&mut self, block: Block) {
        self.close_inline();
        if let Some(target) = self.target() {
            target.push(block);
        }
    }

    fn extend_blocks(&mut self, blocks: impl IntoIterator<Item = Block>) {
        self.close_inline();
        if let Some(target) = self.target() {
            target.extend(blocks);
        }
    }

    fn target(&mut self) -> Option<&mut Vec<Block>> {
        match self.frames.last_mut()? {
            Frame::Blocks(blocks) => Some(blocks),
            Frame::Quote { blocks, .. } => Some(blocks),
            Frame::List { item, .. } => item.as_mut(),
            _ => None,
        }
    }

    fn pop_to_quote(&mut self) -> Option<Frame> {
        pop_if(&mut self.frames, |frame| {
            matches!(frame, Frame::Quote { .. })
        })
    }

    fn pop_to_code(&mut self) -> Option<Frame> {
        pop_if(&mut self.frames, |frame| matches!(frame, Frame::Code(_)))
    }

    fn pop_to_list(&mut self) -> Option<Frame> {
        pop_if(&mut self.frames, |frame| {
            matches!(frame, Frame::List { .. })
        })
    }
}

fn pop_if(frames: &mut Vec<Frame>, matches: impl Fn(&Frame) -> bool) -> Option<Frame> {
    if frames.last().is_some_and(matches) {
        frames.pop()
    } else {
        None
    }
}

impl Inline {
    fn push(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        let style = InlineStyle {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code,
            link: self.link > 0,
        };
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.spans.push(Span {
                text: text.to_string(),
                style,
            }),
        }
    }
}

fn code_lines(buffer: &str) -> Vec<String> {
    let mut lines: Vec<String> = buffer
        .lines()
        .map(|line| {
            line.strip_suffix('\r')
                .unwrap_or(line)
                .replace('\t', "    ")
        })
        .collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_blocks(source: &str) -> Vec<Block> {
        parse(source).blocks
    }

    fn text(blocks: &[Block]) -> &str {
        match &blocks[0] {
            Block::Paragraph(spans) | Block::Heading { spans, .. } => &spans[0].text,
            other => panic!("expected inline text, got {other:?}"),
        }
    }

    fn paragraph_span(blocks: &[Block], index: usize) -> &Span {
        match &blocks[0] {
            Block::Paragraph(spans) => &spans[index],
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn atx_and_setext_headings_share_one_block() {
        assert_eq!(
            parse_blocks("# Title"),
            vec![Block::Heading {
                level: 1,
                spans: vec![Span {
                    text: "Title".into(),
                    style: InlineStyle::default(),
                }],
            }]
        );

        let blocks = parse_blocks("Title\n=====");
        assert!(matches!(
            &blocks[0],
            Block::Heading { level: 1, .. } if text(&blocks) == "Title"
        ));
    }

    #[test]
    fn inline_emphasis_and_code_carry_their_style() {
        let blocks = parse_blocks("a **b** *c* ~~d~~ `e`");
        assert!(paragraph_span(&blocks, 1).style.bold);
        assert!(paragraph_span(&blocks, 3).style.italic);
        assert!(paragraph_span(&blocks, 5).style.strike);
        assert!(paragraph_span(&blocks, 7).style.code);
        assert_eq!(paragraph_span(&blocks, 7).text, "e");
    }

    #[test]
    fn a_fenced_code_block_keeps_its_lines() {
        let blocks = parse_blocks("```rust\nfn main() {}\n\nx\n```");
        assert_eq!(
            blocks,
            vec![Block::Code(vec![
                "fn main() {}".into(),
                String::new(),
                "x".into()
            ])]
        );
    }

    #[test]
    fn lists_carry_order_start_and_nesting() {
        let blocks = parse_blocks("3. a\n4. b\n   - c");
        let Block::List {
            ordered,
            start,
            items,
        } = &blocks[0]
        else {
            panic!("expected a list, got {blocks:?}");
        };
        assert!(*ordered);
        assert_eq!(*start, 3);
        assert_eq!(items.len(), 2);
        assert!(matches!(
            &items[1].blocks[1],
            Block::List {
                ordered: false,
                items,
                ..
            } if items.len() == 1
        ));
    }

    #[test]
    fn a_quote_wraps_its_blocks() {
        let blocks = parse_blocks("> quoted");
        let Block::Quote(inner) = &blocks[0] else {
            panic!("expected a quote, got {blocks:?}");
        };
        assert_eq!(text(inner), "quoted");
    }

    #[test]
    fn rules_links_and_images_degrade_as_documented() {
        let blocks = parse_blocks("***");
        assert_eq!(blocks, vec![Block::Rule]);

        let blocks = parse_blocks("[label](https://example.com)");
        assert_eq!(paragraph_span(&blocks, 0).text, "label");
        assert!(paragraph_span(&blocks, 0).style.link);
        assert!(!paragraph_span(&blocks, 0).text.contains("example"));

        let blocks = parse_blocks("![alt](/x.png)");
        assert_eq!(paragraph_span(&blocks, 0).text, "alt");
    }

    #[test]
    fn html_is_dropped_and_unsupported_constructs_stay_literal() {
        let blocks = parse_blocks("<b>kept</b>");
        assert_eq!(paragraph_span(&blocks, 0).text, "kept");

        let blocks = parse_blocks("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(matches!(&blocks[0], Block::Paragraph(_)));
        let literal = match &blocks[0] {
            Block::Paragraph(spans) => spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            other => panic!("expected a paragraph, got {other:?}"),
        };
        assert!(literal.contains('|'));
    }

    #[test]
    fn a_tight_list_item_still_becomes_a_paragraph() {
        let blocks = parse_blocks("- one\n- two");
        let Block::List { items, .. } = &blocks[0] else {
            panic!("expected a list, got {blocks:?}");
        };
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0].blocks[0], Block::Paragraph(_)));
    }

    #[test]
    fn a_tight_list_item_keeps_its_leading_style() {
        let blocks = parse_blocks("- **bold** and *italic*");
        let Block::List { items, .. } = &blocks[0] else {
            panic!("expected a list, got {blocks:?}");
        };
        let Block::Paragraph(spans) = &items[0].blocks[0] else {
            panic!("expected a paragraph, got {:?}", items[0].blocks);
        };
        assert!(
            spans
                .iter()
                .any(|span| span.style.bold && span.text == "bold")
        );
        assert!(spans.iter().any(|span| span.style.italic));

        let blocks = parse_blocks("- [label](https://example.com)");
        let Block::List { items, .. } = &blocks[0] else {
            panic!("expected a list, got {blocks:?}");
        };
        let Block::Paragraph(spans) = &items[0].blocks[0] else {
            panic!("expected a paragraph, got {:?}", items[0].blocks);
        };
        assert!(
            spans
                .iter()
                .any(|span| span.style.link && span.text == "label")
        );
    }

    #[test]
    fn the_cache_hits_one_source_and_clears() {
        let mut cache = MarkdownCache::default();
        let first = cache.get("# a");
        let again = cache.get("# a");
        assert!(Rc::ptr_eq(&first, &again));

        let other = cache.get("# b");
        assert!(!Rc::ptr_eq(&first, &other));

        cache.clear();
        let after = cache.get("# b");
        assert!(!Rc::ptr_eq(&other, &after));
    }

    #[test]
    fn deep_nesting_is_flattened() {
        let source = ">".repeat(MAX_DEPTH + 2) + " deep";
        let blocks = parse_blocks(&source);
        let mut depth = 0;
        let mut current = &blocks;
        while let Some(Block::Quote(inner)) = current.first() {
            depth += 1;
            current = inner;
        }
        assert!(depth <= MAX_DEPTH);
        assert_eq!(text(current), "deep");
    }
}
