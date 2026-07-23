//! Slide-deck writer: renders the document model to an html5 presentation of nested `<section>`s.
//!
//! The block sequence is split into slides at a computed slide level (see `crate::slides`):
//! headers above it are sectioning markers, the header at it (or a bare content run, or a horizontal
//! rule) opens a frame, and the rest gathers into frame bodies. A frame is a `<section>` holding the
//! slide's title heading and its html5-rendered body. Sectioning markers shallower than the slide
//! level group the frames that follow them: the shallowest such level forms the horizontal axis,
//! wrapping each group in an outer `<section>`, while deeper markers and the frames themselves are
//! the vertical axis inside it. Footnotes from every frame collect into one trailing section. The
//! result is a body fragment with no surrounding document scaffolding and no trailing newline; this
//! format has no public specification.

use std::fmt::Write;

use carta_ast::{Block, Document, Inline};
use carta_core::{MetaVarStyle, Result, Writer, WriterOptions};

use crate::html::{SlideRenderer, fill_slides, fill_width, highlighting};
use crate::slides::{FrameTitle, MAX_LEVEL, Slide, segment, slide_level};

/// Renders a document to a nested-`<section>` slide deck.
#[derive(Debug, Default, Clone, Copy)]
pub struct RevealjsWriter;

impl Writer for RevealjsWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let level = slide_level(&document.blocks);
        let slides = segment(&document.blocks, level);
        let mut renderer = SlideRenderer::new(highlighting(options));
        let mut deck = Deck::new(level);
        if slides.is_empty() {
            deck.empty_frame();
        } else {
            // The shallowest sectioning marker forms the horizontal axis; with none, every slide is
            // a standalone frame and there is no axis to anchor outer sections.
            let top = slides
                .iter()
                .filter_map(section_level)
                .min()
                .unwrap_or(MAX_LEVEL);
            for (index, slide) in slides.iter().enumerate() {
                let has_children = matches!(section_level(slide), Some(lvl) if lvl == top)
                    && opens_outer(&slides, index, top);
                deck.push(&mut renderer, slide, top, has_children);
            }
            deck.close_outer();
        }
        let body = deck.finish();
        let footnotes = renderer.footnote_section();
        let assembled = match footnotes {
            Some(section) if !body.is_empty() => format!("{body}\n{section}"),
            Some(section) => section,
            None => body,
        };
        Ok(fill_slides(&assembled, options.wrap, fill_width(options)))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.revealjs"))
    }

    fn meta_var_style(&self) -> MetaVarStyle {
        MetaVarStyle::Web
    }

    fn render_meta_inlines(&self, inlines: &[Inline], options: &WriterOptions) -> Result<String> {
        crate::html::HtmlWriter.render_meta_inlines(inlines, options)
    }

    fn render_meta_blocks(&self, blocks: &[Block], options: &WriterOptions) -> Result<String> {
        crate::html::HtmlWriter.render_meta_blocks(blocks, options)
    }
}

/// The sectioning-marker level of a slide, or `None` for a frame.
fn section_level(slide: &Slide) -> Option<i32> {
    match slide {
        Slide::Section { level, .. } => Some(*level),
        Slide::Frame { .. } => None,
    }
}

/// Whether the top-level sectioning marker at `index` is followed by at least one nested slide
/// before the next top-level marker, the condition that wraps its group in an outer `<section>`.
fn opens_outer(slides: &[Slide], index: usize, top: i32) -> bool {
    matches!(
        slides.get(index + 1),
        Some(next) if section_level(next) != Some(top)
    )
}

/// Assembles the break-laden deck body. Tracks the stack of open sectioning levels so the whitespace
/// between slides reflects how many nested sections a level change conceptually closes.
struct Deck {
    out: String,
    level: i32,
    open_levels: Vec<i32>,
    outer_open: bool,
    first: bool,
}

impl Deck {
    fn new(level: i32) -> Self {
        Self {
            out: String::new(),
            level,
            open_levels: Vec::new(),
            outer_open: false,
            first: true,
        }
    }

    fn empty_frame(&mut self) {
        let _ = write!(
            self.out,
            "<section class=\"slide level{}\">\n\n</section>",
            self.level
        );
    }

    fn push(&mut self, renderer: &mut SlideRenderer, slide: &Slide, top: i32, has_children: bool) {
        match slide {
            Slide::Section { level, attr, title } if *level == top => {
                if self.outer_open {
                    self.append_outer_close(top);
                }
                let popped = self.pop_to(top);
                if !self.first {
                    self.separate(popped + 1);
                }
                self.open_levels.push(top);
                let marker = Self::section_marker(renderer, *level, attr, title);
                if has_children {
                    self.out.push_str("<section>\n");
                    self.out.push_str(&marker);
                    self.outer_open = true;
                } else {
                    self.out.push_str(&marker);
                }
                self.first = false;
            }
            Slide::Section { level, attr, title } => {
                let popped = self.pop_to(*level);
                if !self.first {
                    self.separate(popped + 1);
                }
                let marker = Self::section_marker(renderer, *level, attr, title);
                self.out.push_str(&marker);
                self.open_levels.push(*level);
                self.first = false;
            }
            Slide::Frame { title, body } => {
                if !self.first {
                    self.out.push('\n');
                }
                self.out
                    .push_str(&self.frame(renderer, title.as_ref(), body));
                self.first = false;
            }
        }
    }

    /// Pop every open level at or below `level`, returning the count removed.
    fn pop_to(&mut self, level: i32) -> usize {
        let mut popped = 0;
        while self.open_levels.last().is_some_and(|open| *open >= level) {
            self.open_levels.pop();
            popped += 1;
        }
        popped
    }

    fn separate(&mut self, newlines: usize) {
        for _ in 0..newlines {
            self.out.push('\n');
        }
    }

    fn close_outer(&mut self) {
        if self.outer_open {
            let top = self.open_levels.first().copied().unwrap_or(self.level);
            self.append_outer_close(top);
        }
    }

    /// Close the open outer `<section>`, prefixing it with one newline per open level deeper than the
    /// horizontal axis, then clear the level stack.
    fn append_outer_close(&mut self, top: i32) {
        let above = self.open_levels.iter().filter(|open| **open > top).count();
        self.separate(above);
        self.out.push_str("</section>");
        self.outer_open = false;
        self.open_levels.clear();
    }

    fn section_marker(
        renderer: &mut SlideRenderer,
        level: i32,
        attr: &carta_ast::Attr,
        title: &[Inline],
    ) -> String {
        let open =
            SlideRenderer::section_open(attr, &["title-slide", "slide", &level_class(level)]);
        let heading = renderer.title(level, attr, title);
        format!("{open}\n{heading}\n\n</section>")
    }

    /// A frame holds an optional title heading and a body slot. An empty body block list collapses
    /// the slot (the heading or opener abuts the close); a non-empty body keeps the slot even when
    /// its blocks render to nothing, leaving a blank line where dropped content stood.
    fn frame(
        &self,
        renderer: &mut SlideRenderer,
        title: Option<&FrameTitle>,
        body: &[Block],
    ) -> String {
        let rendered_body = renderer.body(body);
        if let Some(title) = title {
            let open =
                SlideRenderer::section_open(title.attr, &["slide", &level_class(self.level)]);
            let heading = renderer.title(self.level, title.attr, title.inlines);
            if body.is_empty() {
                format!("{open}\n{heading}\n</section>")
            } else {
                format!("{open}\n{heading}\n{rendered_body}\n</section>")
            }
        } else {
            let open = format!("<section class=\"slide level{}\">", self.level);
            if body.is_empty() {
                format!("{open}\n\n</section>")
            } else {
                format!("{open}\n\n{rendered_body}\n</section>")
            }
        }
    }

    fn finish(self) -> String {
        self.out
    }
}

fn level_class(level: i32) -> String {
    format!("level{level}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::Attr;

    fn render(blocks: Vec<Block>) -> String {
        RevealjsWriter
            .write(
                &Document {
                    blocks,
                    ..Document::default()
                },
                &WriterOptions::default(),
            )
            .unwrap()
    }

    fn header(level: i32, id: &str, title: &str) -> Block {
        Block::Header(
            level,
            Box::new(Attr {
                id: id.to_owned().into(),
                ..Attr::default()
            }),
            vec![Inline::Str(title.to_owned().into())],
        )
    }

    fn para(text: &str) -> Block {
        Block::Para(vec![Inline::Str(text.to_owned().into())])
    }

    #[test]
    fn empty_document_is_one_empty_frame() {
        assert_eq!(
            render(vec![]),
            "<section class=\"slide level6\">\n\n</section>"
        );
    }

    #[test]
    fn bare_content_is_a_titleless_frame() {
        assert_eq!(
            render(vec![para("hello")]),
            "<section class=\"slide level6\">\n\n<p>hello</p>\n</section>"
        );
    }

    #[test]
    fn horizontal_rule_splits_frames() {
        let out = render(vec![para("a"), Block::HorizontalRule, para("b")]);
        assert_eq!(
            out,
            "<section class=\"slide level6\">\n\n<p>a</p>\n</section>\n\
             <section class=\"slide level6\">\n\n<p>b</p>\n</section>"
        );
    }

    #[test]
    fn shallow_headers_nest_a_frame() {
        let out = render(vec![header(1, "one", "One"), header(6, "six", "Six")]);
        assert_eq!(
            out,
            "<section>\n\
             <section id=\"one\" class=\"title-slide slide level1\">\n<h1>One</h1>\n\n</section>\n\
             <section id=\"six\" class=\"slide level6\">\n<h6>Six</h6>\n</section></section>"
        );
    }

    #[test]
    fn two_bare_sections_are_separated_by_a_blank_line() {
        let out = render(vec![header(1, "one", "One"), header(1, "two", "Two")]);
        assert_eq!(
            out,
            "<section id=\"one\" class=\"title-slide slide level1\">\n<h1>One</h1>\n\n</section>\n\n\
             <section id=\"two\" class=\"title-slide slide level1\">\n<h1>Two</h1>\n\n</section>"
        );
    }

    #[test]
    fn vertical_stack_under_a_section() {
        let out = render(vec![
            header(1, "one", "One"),
            header(2, "a", "A"),
            para("bodyA"),
            header(2, "b", "B"),
            para("bodyB"),
        ]);
        assert!(out.starts_with("<section>\n<section id=\"one\""));
        assert!(out.contains(
            "<section id=\"a\" class=\"slide level2\">\n<h2>A</h2>\n<p>bodyA</p>\n</section>"
        ));
        assert!(out.ends_with("<p>bodyB</p>\n</section></section>"));
    }

    #[test]
    fn deeper_headers_inside_a_frame_stay_inline() {
        let out = render(vec![
            header(1, "one", "One"),
            para("intro"),
            header(2, "a", "A"),
            para("bodyA"),
        ]);
        assert!(out.contains("<h2 id=\"a\">A</h2>"));
        assert!(out.starts_with("<section id=\"one\" class=\"slide level1\">"));
    }

    #[test]
    fn footnotes_collect_into_a_trailing_section() {
        let out = render(vec![Block::Para(vec![
            Inline::Str("x".to_owned().into()),
            Inline::Note(vec![para("note")]),
        ])]);
        assert!(out.contains("href=\"#/fn1\""));
        assert!(out.contains("<section id=\"footnotes\""));
        assert!(out.contains("href=\"#/fnref1\""));
    }
}
