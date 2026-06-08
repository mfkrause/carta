//! reStructuredText writer: renders the document model to RST source text.
//!
//! Block structure is conveyed through directives and a three-space indent; inline emphasis maps to
//! `*`/`**`, inline code to double backticks, and roles such as `:sup:` and `:math:`. Footnotes and
//! image substitutions are collected while rendering and emitted as definition sections after the
//! body. Output carries no trailing newline; the caller appends one. Content is wrapped at a fill
//! column of 72.

use oxidoc_ast::{
    Attr, Block, Caption, Document, Format, Inline, ListAttributes, MathType, Target, slug,
    to_plain_text,
};
use oxidoc_core::{Result, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, Piece, attribute_value, display_width, fill, indent_block, is_uri_scheme,
    offset_as_i32, ordered_marker, quote_marks,
};

/// Width of the transition emitted for a horizontal rule.
const RULE_WIDTH: usize = 14;

/// Renders a document to reStructuredText.
#[derive(Debug, Default, Clone, Copy)]
pub struct RstWriter;

impl Writer for RstWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let mut state = State::default();
        let body = state.blocks_to_string(&document.blocks, FILL_COLUMN, true);
        let mut sections = Vec::new();
        if !body.is_empty() {
            sections.push(body);
        }
        let notes: Vec<String> = state
            .footnotes
            .iter()
            .filter(|entry| !entry.is_empty())
            .cloned()
            .collect();
        if !notes.is_empty() {
            sections.push(notes.join("\n\n"));
        }
        if !state.substitutions.is_empty() {
            sections.push(state.substitutions.join("\n"));
        }
        Ok(sections.join("\n\n"))
    }
}

/// Collects the deferred constructs accumulated during rendering: footnote definitions and image
/// substitution definitions, both emitted as their own sections after the document body. The counter
/// names images that carry no alt text.
#[derive(Debug, Default)]
struct State {
    footnotes: Vec<String>,
    substitutions: Vec<String>,
    fallback_count: usize,
}

/// An inline-rendering unit: an unbreakable text run carrying whether it is RST markup (so its
/// boundaries may need a `\ ` separator), a breakable space, or a forced line break.
#[derive(Debug, Clone)]
enum Token {
    Word {
        text: String,
        complex: bool,
        lead: char,
    },
    /// A zero-width boundary that prints nothing but, like markup, needs a `\ ` separator when it
    /// meets adjacent markup (a raw inline whose target format is not being emitted).
    Marker,
    Space,
    Hard,
}

/// Build a markup or plain word whose separator-boundary character is the first character of its
/// rendered text. Escaped plain text uses [`plain_word`] instead, since escaping can prepend a
/// backslash that is not the character RST would actually see.
fn word(text: String, complex: bool) -> Token {
    let lead = text.chars().next().unwrap_or('\0');
    Token::Word {
        text,
        complex,
        lead,
    }
}

impl State {
    /// Render a block sequence into the document's default layout. Consecutive blocks are separated
    /// by a blank line, except that a [`Block::Plain`] is followed by a single newline when the next
    /// block can sit directly beneath it (see [`tight_after_plain`]). Blocks that render empty are
    /// dropped.
    fn blocks_to_string(&mut self, blocks: &[Block], width: usize, top: bool) -> String {
        let mut out = String::new();
        let mut previous: Option<&Block> = None;
        for block in blocks {
            let text = self.block(block, width, top);
            if text.is_empty() {
                continue;
            }
            if let Some(prev) = previous {
                out.push_str(block_separator(prev, block));
            }
            out.push_str(&text);
            previous = Some(block);
        }
        out
    }

    fn block(&mut self, block: &Block, width: usize, top: bool) -> String {
        match block {
            Block::Plain(inlines) => fill(&to_pieces(self.tokens(inlines)), width),
            Block::Para(inlines) => self.para(inlines, width),
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines, top),
            Block::CodeBlock(attr, text) => code_block(attr, text),
            Block::RawBlock(format, text) => raw_block(format, text),
            Block::BlockQuote(blocks) => {
                let body = self.blocks_to_string(blocks, width.saturating_sub(3), false);
                if body.is_empty() {
                    String::new()
                } else {
                    indent_block(&body, "   ", "   ")
                }
            }
            Block::BulletList(items) => self.bullet_list(items, width),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::DefinitionList(items) => self.definition_list(items, width),
            Block::HorizontalRule => "-".repeat(RULE_WIDTH),
            Block::Table(_) => todo!("rst writer: render tables"),
            Block::Figure(attr, caption, blocks) => self.figure(attr, caption, blocks, width),
            Block::Div(attr, blocks) => self.div(attr, blocks, width),
            Block::LineBlock(lines) => {
                let rendered: Vec<String> = lines
                    .iter()
                    .map(|line| self.render_line(line, width))
                    .collect();
                rendered.join("\n")
            }
        }
    }

    /// Render a paragraph. A paragraph holding a forced line break becomes a line block; one holding
    /// display math is split around each formula into separate paragraphs and `.. math::` directives;
    /// otherwise its inlines are filled to `width`.
    fn para(&mut self, inlines: &[Inline], width: usize) -> String {
        let mut has_break = false;
        let mut has_display_math = false;
        for inline in inlines {
            match inline {
                Inline::LineBreak => has_break = true,
                Inline::Math(MathType::DisplayMath, _) => has_display_math = true,
                _ => {}
            }
        }
        if has_break {
            let lines = split_at(inlines, |inline| matches!(inline, Inline::LineBreak));
            let rendered: Vec<String> = lines
                .iter()
                .map(|line| self.render_line(line, width))
                .collect();
            return rendered.join("\n");
        }
        if has_display_math {
            return self.para_with_math(inlines, width);
        }
        fill(&to_pieces(self.tokens(inlines)), width)
    }

    fn para_with_math(&mut self, inlines: &[Inline], width: usize) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut start = 0;
        for (index, inline) in inlines.iter().enumerate() {
            if let Inline::Math(MathType::DisplayMath, tex) = inline {
                if let Some(segment) = inlines.get(start..index)
                    && !segment.is_empty()
                {
                    let text = fill(&to_pieces(self.tokens(segment)), width);
                    if !text.is_empty() {
                        parts.push(text);
                    }
                }
                parts.push(format!(".. math:: {}", tex.trim()));
                start = index + 1;
            }
        }
        if let Some(segment) = inlines.get(start..)
            && !segment.is_empty()
        {
            let text = fill(&to_pieces(self.tokens(segment)), width);
            if !text.is_empty() {
                parts.push(text);
            }
        }
        parts.join("\n\n")
    }

    /// Render one line-block line: its inlines filled to the body width, then prefixed with `| ` and
    /// continuation lines indented to match.
    fn render_line(&mut self, line: &[Inline], width: usize) -> String {
        let body = fill(&to_pieces(self.tokens(line)), width.saturating_sub(2));
        indent_block(&body, "| ", "  ")
    }

    fn header(&mut self, level: i32, attr: &Attr, inlines: &[Inline], top: bool) -> String {
        let line = flatten(self.tokens(inlines)).trim().to_owned();
        if !top {
            let mut rubric = format!(".. rubric:: {line}");
            if !attr.id.is_empty() {
                rubric.push_str("\n   :name: ");
                rubric.push_str(&attr.id);
            }
            return rubric;
        }
        if line.is_empty() {
            return String::new();
        }
        let underline = heading_char(level).to_string().repeat(display_width(&line));
        let header = format!("{line}\n{underline}");
        if attr.id.is_empty() || attr.id == auto_identifier(inlines) {
            header
        } else {
            format!(".. _{}:\n\n{header}", attr.id)
        }
    }

    fn item_body(&mut self, item: &[Block], width: usize) -> String {
        let body = self.blocks_to_string(item, width, false);
        if !body.is_empty() && item.first().is_some_and(marker_stands_alone) {
            format!("\n\n{body}")
        } else {
            lead_quote_fence(item, body)
        }
    }

    fn bullet_list(&mut self, items: &[Vec<Block>], width: usize) -> String {
        let mut units = Vec::new();
        for item in items {
            let simple = is_simple_item(item);
            let body = self.item_body(item, width.saturating_sub(2));
            units.push((simple, indent_block(&body, "- ", "  ")));
        }
        join_loose_items(units)
    }

    fn ordered_list(
        &mut self,
        attrs: &ListAttributes,
        items: &[Vec<Block>],
        width: usize,
    ) -> String {
        let markers: Vec<String> = (0..items.len())
            .map(|offset| {
                let number = attrs.start.saturating_add(offset_as_i32(offset));
                ordered_marker(number, &attrs.style, &attrs.delim)
            })
            .collect();
        let field = markers.iter().map(|m| m.chars().count()).max().unwrap_or(0) + 1;
        let rest = " ".repeat(field);
        let mut units = Vec::new();
        for (offset, item) in items.iter().enumerate() {
            let marker = markers.get(offset).cloned().unwrap_or_default();
            let first = format!("{marker:<field$}");
            let simple = is_simple_item(item);
            let body = self.item_body(item, width.saturating_sub(field));
            units.push((simple, indent_block(&body, &first, &rest)));
        }
        join_loose_items(units)
    }

    fn definition_list(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        let mut groups = Vec::new();
        for (term, definitions) in items {
            let term_line = self.flat(term);
            let mut def_units = Vec::new();
            for definition in definitions {
                let simple = matches!(definition.as_slice(), [Block::Plain(_)]);
                let body = self.blocks_to_string(definition, width.saturating_sub(3), false);
                let body = lead_quote_fence(definition, body);
                def_units.push((simple, indent_block(&body, "   ", "   ")));
            }
            let group_simple = definitions
                .iter()
                .all(|definition| matches!(definition.as_slice(), [Block::Plain(_)]));
            let group = format!("{term_line}\n{}", join_loose_items(def_units));
            groups.push((group_simple, group));
        }
        join_loose_items(groups)
    }

    fn figure(&mut self, attr: &Attr, caption: &Caption, blocks: &[Block], width: usize) -> String {
        let image = find_image(blocks);
        let url = image
            .map(|(_, _, target)| target.url.clone())
            .unwrap_or_default();
        let mut directive = format!(".. figure:: {url}");
        if !attr.id.is_empty() {
            directive.push_str("\n   name: ");
            directive.push_str(&attr.id);
        }
        if let Some((image_attr, alt, _)) = image {
            let alt_text = to_plain_text(alt);
            if !alt_text.is_empty() {
                directive.push_str("\n   :alt: ");
                directive.push_str(&alt_text);
            }
            for option in dimension_options(image_attr) {
                directive.push_str("\n   ");
                directive.push_str(&option);
            }
        }
        let caption_text = self.blocks_to_string(&caption.long, width.saturating_sub(3), false);
        if caption_text.is_empty() {
            directive
        } else {
            format!(
                "{directive}\n\n{}",
                indent_block(&caption_text, "   ", "   ")
            )
        }
    }

    fn div(&mut self, attr: &Attr, blocks: &[Block], width: usize) -> String {
        let body = self.blocks_to_string(blocks, width.saturating_sub(3), false);
        let body = lead_quote_fence(blocks, body);
        let indented = indent_block(&body, "   ", "   ");
        let mut directive = match attr.classes.first() {
            Some(class) if is_admonition(class) => format!(".. {class}::"),
            _ if attr.classes.is_empty() => ".. container::".to_owned(),
            _ => format!(".. container:: {}", attr.classes.join(" ")),
        };
        if !attr.id.is_empty() {
            directive.push_str("\n   :name: ");
            directive.push_str(&attr.id);
        }
        format!("{directive}\n\n{indented}")
    }

    /// Render inlines to a single flat line: spaces and forced breaks collapse to one space, with
    /// `\ ` separators inserted between adjacent markup boundaries. Used for content that must stay on
    /// one line (headers, definition terms, and the inside of inline markup).
    fn flat(&mut self, inlines: &[Inline]) -> String {
        self.flat_nested(inlines, false)
    }

    fn flat_nested(&mut self, inlines: &[Inline], in_emphasis: bool) -> String {
        flatten(self.tokens_nested(inlines, in_emphasis))
    }

    fn tokens(&mut self, inlines: &[Inline]) -> Vec<Token> {
        self.tokens_nested(inlines, false)
    }

    fn tokens_nested(&mut self, inlines: &[Inline], in_emphasis: bool) -> Vec<Token> {
        let mut out = Vec::new();
        for inline in inlines {
            self.token(inline, in_emphasis, &mut out);
        }
        out
    }

    /// Render one inline. `in_emphasis` is set when the surrounding context is already an emphasis,
    /// strong, or similar phrase markup: RST cannot nest such markup, so a nested member of that
    /// family contributes its content as plain text rather than reopening markers.
    fn token(&mut self, inline: &Inline, in_emphasis: bool, out: &mut Vec<Token>) {
        match inline {
            Inline::Str(text) => out.push(Token::Word {
                text: escape(text),
                complex: false,
                lead: text.chars().next().unwrap_or('\0'),
            }),
            Inline::Space | Inline::SoftBreak => out.push(Token::Space),
            Inline::LineBreak => out.push(Token::Hard),
            Inline::Emph(inlines) | Inline::Underline(inlines) => {
                self.phrase(inlines, in_emphasis, "*", "*", PhraseKind::Emph, out);
            }
            Inline::Strong(inlines) => {
                self.phrase(inlines, in_emphasis, "**", "**", PhraseKind::Strong, out);
            }
            Inline::Strikeout(inlines) => {
                self.phrase(
                    inlines,
                    in_emphasis,
                    "[STRIKEOUT:",
                    "]",
                    PhraseKind::Leaf,
                    out,
                );
            }
            Inline::Superscript(inlines) => {
                self.phrase(inlines, in_emphasis, ":sup:`", "`", PhraseKind::Leaf, out);
            }
            Inline::Subscript(inlines) => {
                self.phrase(inlines, in_emphasis, ":sub:`", "`", PhraseKind::Leaf, out);
            }
            Inline::SmallCaps(inlines) => {
                if in_emphasis {
                    for child in inlines {
                        self.token(child, true, out);
                    }
                } else {
                    out.push(word(self.flat_nested(inlines, false), true));
                }
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_marks(kind);
                out.push(word(
                    format!("{open}{}{close}", self.flat_nested(inlines, in_emphasis)),
                    false,
                ));
            }
            Inline::Cite(_, inlines) | Inline::Span(_, inlines) => {
                let inner = self.tokens_nested(inlines, in_emphasis);
                out.extend(inner);
            }
            Inline::Code(_, text) => {
                let trimmed = text.trim_matches(' ');
                let rendered = if trimmed.is_empty() {
                    "````".to_owned()
                } else if trimmed.contains('`') {
                    literal_role(trimmed)
                } else {
                    format!("``{trimmed}``")
                };
                out.push(word(rendered, true));
            }
            Inline::Math(_, tex) => out.push(word(format!(":math:`{tex}`"), true)),
            Inline::RawInline(format, text) => {
                if format.0.eq_ignore_ascii_case("rst") {
                    out.push(word(text.clone(), false));
                } else {
                    out.push(Token::Marker);
                }
            }
            Inline::Link(_, label, target) => self.link(label, target, out),
            Inline::Image(attr, alt, target) => {
                if in_emphasis {
                    for child in alt {
                        self.token(child, true, out);
                    }
                } else {
                    self.image(attr, alt, target, None, out);
                }
            }
            Inline::Note(blocks) => self.note(blocks, out),
        }
    }

    /// Render a phrase-markup inline (emphasis, strong, strikeout, super/subscript). Inside an
    /// existing phrase the markers are dropped and the content rendered inline. Otherwise the content
    /// is wrapped in the open/close markers; for emphasis and strong a child that cannot sit inside
    /// those markers — a link, or a strong span inside an emphasis — interrupts the run, closing the
    /// markers, rendering on its own, then reopening for the remainder.
    fn phrase(
        &mut self,
        inlines: &[Inline],
        in_emphasis: bool,
        open: &str,
        close: &str,
        kind: PhraseKind,
        out: &mut Vec<Token>,
    ) {
        if matches!(kind, PhraseKind::Leaf) {
            let body = self.flat_nested(inlines, true);
            let rendered = if in_emphasis {
                body
            } else {
                format!("{open}{body}{close}")
            };
            out.push(word(rendered, true));
            return;
        }
        // Inside an existing phrase the markers are dropped, but a child that breaks out still parts
        // the run, so the open/close fall away while the splitting logic stays in force.
        let (open, close) = if in_emphasis { ("", "") } else { (open, close) };
        let breakouts: Vec<usize> = inlines
            .iter()
            .enumerate()
            .filter(|(_, child)| breaks_out(child, kind))
            .map(|(index, _)| index)
            .collect();
        if breakouts.is_empty() {
            self.flush_phrase(inlines, open, close, out, false, false);
            return;
        }
        let mut run_start = 0;
        for (position, &index) in breakouts.iter().enumerate() {
            let segment = inlines.get(run_start..index).unwrap_or(&[]);
            self.flush_phrase(segment, open, close, out, position > 0, true);
            if let Some(child) = inlines.get(index) {
                self.token(child, in_emphasis, out);
            }
            run_start = index + 1;
        }
        let segment = inlines.get(run_start..).unwrap_or(&[]);
        self.flush_phrase(segment, open, close, out, true, false);
    }

    /// Wrap one uninterrupted run of phrase content in its markers, keeping any leading or trailing
    /// whitespace outside the markers (RST markup may not be padded by spaces on the inside). When the
    /// run abuts a broken-out child, a `\ ` null separator is placed just inside the marker on that
    /// side; an otherwise empty run between two such children collapses to a single `*\ *` placeholder.
    fn flush_phrase(
        &mut self,
        segment: &[Inline],
        open: &str,
        close: &str,
        out: &mut Vec<Token>,
        lead_break: bool,
        trail_break: bool,
    ) {
        let is_space = |inline: &Inline| {
            matches!(
                inline,
                Inline::Space | Inline::SoftBreak | Inline::LineBreak
            )
        };
        let Some(first) = segment.iter().position(|inline| !is_space(inline)) else {
            if segment.is_empty() && lead_break && trail_break {
                out.push(word(format!("{open}\\ {close}"), true));
            } else {
                for inline in segment {
                    self.token(inline, true, out);
                }
            }
            return;
        };
        let last = segment
            .iter()
            .rposition(|inline| !is_space(inline))
            .unwrap_or(first);
        if let Some(lead) = segment.get(..first) {
            for inline in lead {
                if lead_break && matches!(inline, Inline::SoftBreak | Inline::LineBreak) {
                    continue;
                }
                self.token(inline, true, out);
            }
        }
        if let Some(middle) = segment.get(first..=last) {
            let plain = to_plain_text(middle);
            let lead_sep = if lead_break
                && first == 0
                && plain.chars().next().is_some_and(|c| !is_safe_follower(c))
            {
                "\\ "
            } else {
                ""
            };
            let trail_sep = if trail_break
                && last + 1 == segment.len()
                && plain.chars().last().is_some_and(|c| !is_safe_preceder(c))
            {
                "\\ "
            } else {
                ""
            };
            let lines = split_at(middle, |inline| matches!(inline, Inline::LineBreak));
            let final_line = lines.len().saturating_sub(1);
            for (index, line) in lines.iter().enumerate() {
                let mut text = String::new();
                if index == 0 {
                    text.push_str(open);
                    text.push_str(lead_sep);
                }
                text.push_str(&self.flat_nested(line, true));
                if index == final_line {
                    text.push_str(trail_sep);
                    text.push_str(close);
                }
                out.push(word(text, true));
                if index != final_line {
                    out.push(Token::Hard);
                }
            }
        }
        if let Some(trail) = segment.get(last + 1..) {
            for inline in trail {
                if trail_break && matches!(inline, Inline::SoftBreak | Inline::LineBreak) {
                    continue;
                }
                self.token(inline, true, out);
            }
        }
    }

    fn link(&mut self, label: &[Inline], target: &Target, out: &mut Vec<Token>) {
        let plain = to_plain_text(label);
        if target.url.is_empty() && label.is_empty() {
            return;
        }
        if let [Inline::Image(attr, alt, image_target)] = label {
            self.image(attr, alt, image_target, Some(&target.url), out);
            return;
        }
        if target.url == format!("mailto:{plain}") {
            out.push(word(plain, true));
            return;
        }
        if plain == target.url && is_standalone_uri(&target.url) {
            out.push(word(target.url.clone(), true));
            return;
        }
        let text = self.flat_nested(label, true);
        out.push(word(format!("`{text} <{}>`__", target.url), true));
    }

    /// Render an image. A link nested in the alt text cannot live inside a substitution, so it
    /// interrupts the run: the alt splits around each link, each surrounding run becomes its own image
    /// substitution, and the link renders inline between the references.
    fn image(
        &mut self,
        attr: &Attr,
        alt: &[Inline],
        target: &Target,
        link: Option<&str>,
        out: &mut Vec<Token>,
    ) {
        let breakouts: Vec<usize> = alt
            .iter()
            .enumerate()
            .filter(|(_, child)| matches!(child, Inline::Link(..)))
            .map(|(index, _)| index)
            .collect();
        if breakouts.is_empty() {
            let name = self.substitution_name(to_plain_text(alt));
            self.register_image(attr, &name, target, link, out);
            return;
        }
        let mut run_start = 0;
        for (position, &index) in breakouts.iter().enumerate() {
            let segment = alt.get(run_start..index).unwrap_or(&[]);
            self.image_run(attr, segment, target, out, position > 0, true);
            if let Some(child) = alt.get(index) {
                self.token(child, false, out);
            }
            run_start = index + 1;
        }
        let segment = alt.get(run_start..).unwrap_or(&[]);
        self.image_run(attr, segment, target, out, true, false);
    }

    /// Emit one run of alt text sitting beside a broken-out link as its own image substitution. Spaces
    /// at the run's edge stay outside the reference; where the run abuts the link without a space, a
    /// `\ ` null separator is folded into the substitution name so the reference reads cleanly.
    fn image_run(
        &mut self,
        attr: &Attr,
        segment: &[Inline],
        target: &Target,
        out: &mut Vec<Token>,
        lead_break: bool,
        trail_break: bool,
    ) {
        let is_space = |inline: &Inline| {
            matches!(
                inline,
                Inline::Space | Inline::SoftBreak | Inline::LineBreak
            )
        };
        let Some(first) = segment.iter().position(|inline| !is_space(inline)) else {
            for inline in segment {
                self.token(inline, false, out);
            }
            return;
        };
        let last = segment
            .iter()
            .rposition(|inline| !is_space(inline))
            .unwrap_or(first);
        if let Some(lead) = segment.get(..first) {
            for inline in lead {
                self.token(inline, false, out);
            }
        }
        if let Some(middle) = segment.get(first..=last) {
            let plain = to_plain_text(middle);
            let lead_sep = if lead_break
                && first == 0
                && plain.chars().next().is_some_and(|c| !is_safe_follower(c))
            {
                "\\ "
            } else {
                ""
            };
            let trail_sep = if trail_break
                && last + 1 == segment.len()
                && plain.chars().last().is_some_and(|c| !is_safe_preceder(c))
            {
                "\\ "
            } else {
                ""
            };
            let name = format!("{lead_sep}{plain}{trail_sep}");
            self.register_image(attr, &name, target, None, out);
        }
        if let Some(trail) = segment.get(last + 1..) {
            for inline in trail {
                self.token(inline, false, out);
            }
        }
    }

    fn substitution_name(&mut self, plain: String) -> String {
        if plain.is_empty() {
            self.fallback_count += 1;
            format!("image{}", self.fallback_count)
        } else {
            plain
        }
    }

    fn register_image(
        &mut self,
        attr: &Attr,
        name: &str,
        target: &Target,
        link: Option<&str>,
        out: &mut Vec<Token>,
    ) {
        let mut definition = format!(".. |{name}| image:: {}", target.url);
        for option in dimension_options(attr) {
            definition.push_str("\n   ");
            definition.push_str(&option);
        }
        if let Some(url) = link {
            definition.push_str("\n   :target: ");
            definition.push_str(url);
        }
        self.substitutions.push(definition);
        out.push(word(format!("|{name}|"), true));
    }

    fn note(&mut self, blocks: &[Block], out: &mut Vec<Token>) {
        let index = self.footnotes.len();
        self.footnotes.push(String::new());
        let number = index + 1;
        let body = self.blocks_to_string(blocks, FILL_COLUMN.saturating_sub(3), false);
        let entry = if body.is_empty() {
            format!(".. [{number}]")
        } else {
            format!(".. [{number}]\n{}", indent_block(&body, "   ", "   "))
        };
        if let Some(slot) = self.footnotes.get_mut(index) {
            *slot = entry;
        }
        out.push(word(format!(" [{number}]_"), false));
    }
}

/// Whether a list item renders to a single line: either it has no content, or its one block is a
/// [`Block::Plain`]. Any other shape spans multiple lines and makes its list loose.
fn is_simple_item(item: &[Block]) -> bool {
    item.is_empty() || matches!(item, [Block::Plain(_)])
}

/// Join already-rendered list items or definition groups. A list is tight only when every unit is a
/// single line; one multi-line unit makes the whole list loose, separating all units with a blank
/// line rather than a single newline. Empty units are dropped.
fn join_loose_items(units: Vec<(bool, String)>) -> String {
    let separator = if units.iter().all(|(simple, _)| *simple) {
        "\n"
    } else {
        "\n\n"
    };
    let mut out = String::new();
    let mut first = true;
    for (_, text) in units {
        if text.is_empty() {
            continue;
        }
        if !first {
            out.push_str(separator);
        }
        out.push_str(&text);
        first = false;
    }
    out
}

/// The text placed between two consecutive rendered blocks. A block quote that follows a block whose
/// body is indented or directive-introduced is fenced off with an empty `..` comment, so the quote's
/// indentation is not read as a continuation of the block above. A [`Block::Plain`] sits one newline
/// above a block that can follow it tightly; everything else is separated by a blank line.
fn block_separator(previous: &Block, current: &Block) -> &'static str {
    if matches!(current, Block::BlockQuote(_)) && needs_quote_fence(previous) {
        return "\n\n..\n\n";
    }
    if matches!(previous, Block::Plain(_)) && tight_after_plain(current) {
        return "\n";
    }
    "\n\n"
}

/// Prefix an empty `..` comment when an indented container's content opens with a block quote, whose
/// own indentation would otherwise merge into the container's body. Returns `body` unchanged when it
/// is empty or does not begin with a quote.
fn lead_quote_fence(blocks: &[Block], body: String) -> String {
    if !body.is_empty() && matches!(blocks.first(), Some(Block::BlockQuote(_))) {
        format!("..\n\n{body}")
    } else {
        body
    }
}

/// Whether a block quote placed directly below this block would be misread as a continuation of it,
/// requiring an empty `..` comment between them. True for blocks whose rendering ends in indented or
/// directive-introduced content: quotes, literal and directive blocks, lists, and figures.
fn needs_quote_fence(previous: &Block) -> bool {
    match previous {
        Block::BlockQuote(_)
        | Block::CodeBlock(..)
        | Block::BulletList(_)
        | Block::OrderedList(..)
        | Block::DefinitionList(_)
        | Block::Div(..)
        | Block::Figure(..) => true,
        Block::RawBlock(format, _) => !format.0.eq_ignore_ascii_case("rst"),
        _ => false,
    }
}

/// Whether a block may sit one newline below a preceding [`Block::Plain`] rather than across a blank
/// line. Lists, rules, and directive-introduced blocks require the blank line; flowing text, literal
/// blocks, quotes, headers, line blocks, and figures do not.
fn tight_after_plain(block: &Block) -> bool {
    match block {
        Block::Para(inlines) => !inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Math(MathType::DisplayMath, _))),
        Block::Plain(_)
        | Block::BlockQuote(_)
        | Block::Header(..)
        | Block::LineBlock(_)
        | Block::Figure(..) => true,
        Block::CodeBlock(attr, _) => code_language(attr).is_none(),
        _ => false,
    }
}

/// Whether a list item whose first block is this one must place its marker on a line of its own,
/// pushing the content down past a blank line. True for blocks that introduce their content through a
/// directive or nested marker — rules, raw passthrough, nested lists, containers, and highlighted
/// literal blocks — none of which can share the marker's line.
fn marker_stands_alone(block: &Block) -> bool {
    match block {
        Block::HorizontalRule
        | Block::BulletList(_)
        | Block::OrderedList(..)
        | Block::DefinitionList(_)
        | Block::Div(..) => true,
        Block::CodeBlock(attr, _) => code_language(attr).is_some(),
        Block::RawBlock(format, _) => !format.0.eq_ignore_ascii_case("rst"),
        _ => false,
    }
}

/// Build the piece stream for the fill engine from inline tokens, inserting a `\ ` separator between
/// adjacent markup boundaries that RST would not otherwise recognize.
fn to_pieces(tokens: Vec<Token>) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut pending: Option<(bool, char)> = None;
    for token in tokens {
        match token {
            Token::Word {
                text,
                complex,
                lead,
            } => {
                let Some(last) = text.chars().last() else {
                    continue;
                };
                if let Some((previous_complex, previous_last)) = pending
                    && separator_needed(previous_complex, previous_last, complex, lead)
                {
                    out.push(Piece::Text("\\ ".to_owned()));
                }
                out.push(Piece::Text(text));
                pending = Some((complex, last));
            }
            Token::Marker => {
                if pending.is_some_and(|(previous_complex, _)| previous_complex) {
                    out.push(Piece::Text("\\ ".to_owned()));
                }
                pending = Some((false, MARKER_BOUNDARY));
            }
            Token::Space => {
                out.push(Piece::Space);
                pending = None;
            }
            Token::Hard => {
                out.push(Piece::Hard);
                pending = None;
            }
        }
    }
    out
}

/// Flatten inline tokens to a single line: spaces and forced breaks become one space, with the same
/// `\ ` separators [`to_pieces`] inserts between adjacent markup boundaries.
fn flatten(tokens: Vec<Token>) -> String {
    let mut out = String::new();
    for piece in to_pieces(tokens) {
        match piece {
            Piece::Text(text) => out.push_str(&text),
            Piece::Space | Piece::Hard => out.push(' '),
        }
    }
    out
}

/// The boundary character a [`Token::Marker`] presents to a following run: a value that is neither a
/// safe follower nor a safe preceder, so adjacent markup is always separated from it.
const MARKER_BOUNDARY: char = '\0';

/// Whether a `\ ` separator is needed between two adjacent inline runs: a markup run meeting a
/// character that cannot legally follow it, or one preceded by a character that cannot legally
/// precede it.
fn separator_needed(
    previous_complex: bool,
    previous_last: char,
    current_complex: bool,
    current_first: char,
) -> bool {
    (previous_complex && !is_safe_follower(current_first))
        || (current_complex && !is_safe_preceder(previous_last))
}

/// Characters that may directly follow an inline-markup end without a separator.
fn is_safe_follower(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '\''
            | '"'
            | ')'
            | ']'
            | '}'
            | '>'
            | '-'
            | '.'
            | ','
            | ':'
            | ';'
            | '!'
            | '?'
            | '\\'
            | '/'
    )
}

/// Characters that may directly precede an inline-markup start without a separator.
fn is_safe_preceder(ch: char) -> bool {
    matches!(
        ch,
        ' ' | ':' | '/' | '-' | '"' | '\'' | '<' | '(' | '[' | '{'
    )
}

/// Characters that may directly precede an inline-markup start-string.
const OPENERS: &[char] = &['-', ':', '/', '\'', '"', '<', '(', '[', '{'];

/// Characters that may directly follow an inline-markup end-string.
const CLOSERS: &[char] = &[
    '-', '.', ',', ':', ';', '!', '?', '\'', '"', ')', ']', '}', '>',
];

/// Escape the characters of a text run that RST would otherwise read as markup. A backslash is always
/// doubled. A `*`, backtick, or `|` is escaped where it could open or close inline markup given its
/// neighbors. A `_` is a reference marker: it is escaped everywhere except where it is buried directly
/// before an alphanumeric and is not itself opening at a word boundary.
fn escape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    for (index, &ch) in chars.iter().enumerate() {
        let prev = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
        let next = chars.get(index + 1).copied();
        match ch {
            '\\' => out.push_str("\\\\"),
            '*' | '`' | '|' => {
                if flanking_escape(prev, next) {
                    out.push('\\');
                }
                out.push(ch);
            }
            '_' => {
                if underscore_escape(prev, next) {
                    out.push('\\');
                }
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Whether a `*`, backtick, or `|` could be read as an inline-markup delimiter: a start-string sits at
/// a boundary or opener and is not followed by whitespace; an end-string follows non-whitespace and is
/// not followed by other text. A run boundary counts as both an opener and a closer.
fn flanking_escape(prev: Option<char>, next: Option<char>) -> bool {
    let could_start = prev.is_none_or(|c| c.is_whitespace() || OPENERS.contains(&c))
        && next.is_none_or(|c| !c.is_whitespace());
    let could_end = prev.is_some_and(|c| !c.is_whitespace())
        && next.is_none_or(|c| c.is_whitespace() || CLOSERS.contains(&c));
    could_start || could_end
}

/// Whether a `_` needs escaping: only a `_` buried directly before an alphanumeric, with a preceding
/// non-whitespace, non-opener character, is safe to leave bare.
fn underscore_escape(prev: Option<char>, next: Option<char>) -> bool {
    let buried = next.is_some_and(char::is_alphanumeric)
        && prev.is_some_and(|c| !c.is_whitespace() && !OPENERS.contains(&c));
    !buried
}

/// Whether a class name maps to a standard RST admonition directive.
fn is_admonition(name: &str) -> bool {
    matches!(
        name,
        "attention"
            | "caution"
            | "danger"
            | "error"
            | "hint"
            | "important"
            | "note"
            | "tip"
            | "warning"
            | "admonition"
    )
}

/// The language argument of a code block: its first class that is not the line-numbering flag.
fn code_language(attr: &Attr) -> Option<&str> {
    attr.classes
        .iter()
        .map(String::as_str)
        .find(|class| *class != "numberLines")
}

fn code_block(attr: &Attr, text: &str) -> String {
    let head = match code_language(attr) {
        Some(language) => {
            let mut head = format!(".. code:: {language}");
            if attr.classes.iter().any(|class| class == "numberLines") {
                head.push_str("\n   :number-lines:");
            }
            head
        }
        None => "::".to_owned(),
    };
    literal_directive(&head, text)
}

/// Render a raw block: content whose format is `rst` passes through verbatim; any other format is
/// wrapped in a `.. raw::` directive naming that format.
fn raw_block(format: &Format, text: &str) -> String {
    if format.0.eq_ignore_ascii_case("rst") {
        text.strip_suffix('\n').unwrap_or(text).to_owned()
    } else {
        literal_directive(&format!(".. raw:: {}", format.0), text)
    }
}

/// Render a directive whose body is verbatim text indented three columns under a blank line. Each
/// line of content is indented, blank lines stay empty, and a final line break in the source is
/// dropped; the body keeps any blank lines that remain beyond the two a trailing run would otherwise
/// supply. Content that is empty or only blank lines collapses onto the directive head.
fn literal_directive(head: &str, text: &str) -> String {
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("   {line}")
            }
        })
        .collect();
    let trailing = lines
        .iter()
        .rev()
        .take_while(|line| line.is_empty())
        .count();
    let kept = lines.len().saturating_sub(trailing);
    let Some(body) = lines.get(..kept) else {
        return head.to_owned();
    };
    if body.is_empty() {
        return format!("{head}{}", "\n".repeat(trailing.saturating_sub(1)));
    }
    format!(
        "{head}\n\n{}{}",
        body.join("\n"),
        "\n".repeat(trailing.saturating_sub(2))
    )
}

/// Find the first image among a figure's blocks, returning its attributes, alt inlines, and target.
fn find_image(blocks: &[Block]) -> Option<(&Attr, &Vec<Inline>, &Target)> {
    for block in blocks {
        let (Block::Plain(inlines) | Block::Para(inlines)) = block else {
            continue;
        };
        for inline in inlines {
            if let Inline::Image(attr, alt, target) = inline {
                return Some((attr, alt, target));
            }
        }
    }
    None
}

/// The `:width:`/`:height:` directive options built from an attribute's `width`/`height` entries.
fn dimension_options(attr: &Attr) -> Vec<String> {
    let mut options = Vec::new();
    if let Some(value) = attribute_value(attr, "width")
        && let Some(shown) = show_dimension(value)
    {
        options.push(format!(":width: {shown}"));
    }
    if let Some(value) = attribute_value(attr, "height")
        && let Some(shown) = show_dimension(value)
    {
        options.push(format!(":height: {shown}"));
    }
    options
}

/// Normalize a dimension value: a percentage keeps its number (with at least one decimal); a unitless
/// number is floored to whole pixels; a recognized absolute unit passes through; anything else is
/// dropped.
fn show_dimension(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(number) = value.strip_suffix('%') {
        let parsed: f64 = number.trim().parse().ok()?;
        return Some(format!("{}%", show_double(parsed)));
    }
    let boundary = value
        .char_indices()
        .find(|(_, ch)| !(ch.is_ascii_digit() || *ch == '.'))
        .map_or(value.len(), |(index, _)| index);
    let (number, unit) = value.split_at(boundary);
    let parsed: f64 = number.parse().ok()?;
    match unit {
        "" => Some(format!("{}px", parsed.floor())),
        "px" | "pt" | "em" | "ex" | "cm" | "mm" | "in" | "pc" => Some(format!("{number}{unit}")),
        _ => None,
    }
}

/// Format a number with at least one fractional digit (a whole value gains a trailing `.0`).
fn show_double(value: f64) -> String {
    let shown = format!("{value}");
    if shown.contains('.') || shown.contains('e') || shown.contains('E') {
        shown
    } else {
        format!("{shown}.0")
    }
}

/// Render inline code that contains a backtick as a `:literal:` role. A backtick is backslash-escaped
/// when exactly one of its neighbors is whitespace or a content edge, the positions where it would
/// otherwise merge with the role's own delimiters.
fn literal_role(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut body = String::new();
    for (index, &ch) in chars.iter().enumerate() {
        if ch == '\\' {
            body.push_str("\\\\");
            continue;
        }
        if ch == '`' {
            let before_space = index == 0
                || chars
                    .get(index.wrapping_sub(1))
                    .is_some_and(|c| c.is_whitespace());
            let after_space = chars.get(index + 1).is_none_or(|c| c.is_whitespace());
            if before_space != after_space {
                body.push('\\');
            }
        }
        body.push(ch);
    }
    format!(":literal:`{body}`")
}

/// Whether a link whose visible text equals its address may be written as a bare standalone URI,
/// which RST recognizes only for an address carrying a registered scheme and built solely from URI
/// characters.
fn is_standalone_uri(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    is_uri_scheme(scheme)
        && KNOWN_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str())
        && url.chars().all(is_uri_char)
}

/// Whether a character may appear in a standalone URI reference.
fn is_uri_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "-._~:/?#@!$&'()*+,;=%".contains(ch)
}

/// Registered URI schemes recognized for bare standalone-hyperlink rendering (the IANA scheme
/// registry).
const KNOWN_SCHEMES: &[&str] = &[
    "aaa",
    "aaas",
    "about",
    "acap",
    "acct",
    "cap",
    "cid",
    "coap",
    "coaps",
    "crid",
    "data",
    "dav",
    "dict",
    "dns",
    "example",
    "file",
    "ftp",
    "geo",
    "go",
    "gopher",
    "h323",
    "http",
    "https",
    "iax",
    "icap",
    "im",
    "imap",
    "info",
    "ipp",
    "ipps",
    "iris",
    "iris.beep",
    "iris.lwz",
    "iris.xpc",
    "iris.xpcs",
    "jabber",
    "ldap",
    "mailto",
    "mid",
    "msrp",
    "msrps",
    "mtqp",
    "mupdate",
    "news",
    "nfs",
    "ni",
    "nih",
    "nntp",
    "opaquelocktoken",
    "pkcs11",
    "pop",
    "pres",
    "reload",
    "rtsp",
    "rtsps",
    "rtspu",
    "service",
    "session",
    "shttp",
    "sieve",
    "sip",
    "sips",
    "sms",
    "snmp",
    "soap.beep",
    "soap.beeps",
    "stun",
    "stuns",
    "tag",
    "tel",
    "telnet",
    "tftp",
    "thismessage",
    "tip",
    "tn3270",
    "turn",
    "turns",
    "tv",
    "urn",
    "vemmi",
    "ws",
    "wss",
    "xcon",
    "xcon-userid",
    "xmlrpc.beep",
    "xmlrpc.beeps",
    "xmpp",
    "z39.50r",
    "z39.50s",
    "admin",
    "app",
    "bitcoin",
    "bzr",
    "chrome",
    "cvs",
    "doi",
    "facetime",
    "feed",
    "finger",
    "fish",
    "git",
    "gizmoproject",
    "gtalk",
    "irc",
    "ircs",
    "irc6",
    "itms",
    "jar",
    "ldaps",
    "magnet",
    "maps",
    "market",
    "message",
    "mms",
    "mvn",
    "notes",
    "psyc",
    "rmi",
    "rsync",
    "secondlife",
    "sftp",
    "skype",
    "smb",
    "soldat",
    "spotify",
    "ssh",
    "steam",
    "svn",
    "teamspeak",
    "things",
    "udp",
    "unreal",
    "ut2004",
    "ventrilo",
    "view-source",
    "webcal",
    "wtai",
    "wyciwyg",
    "xfire",
    "xri",
    "ymsgr",
];

/// The phrase-markup family being rendered: leaf markup that admits no break-out (strikeout,
/// super/subscript), emphasis, or strong.
#[derive(Debug, Clone, Copy)]
enum PhraseKind {
    Leaf,
    Emph,
    Strong,
}

/// Whether an inline must be lifted out of surrounding emphasis or strong markup: a link always, and
/// a strong span when it sits inside emphasis.
fn breaks_out(inline: &Inline, kind: PhraseKind) -> bool {
    matches!(inline, Inline::Link(..))
        || (matches!(kind, PhraseKind::Emph) && matches!(inline, Inline::Strong(_)))
}

/// Derive a header's implicit identifier from its inline text: keep alphanumerics, spaces, and
/// `_`/`-`/`.`; lowercase; collapse whitespace runs into single hyphens; drop everything up to the
/// first letter; fall back to `section` when nothing remains. An explicit identifier equal to this is
/// redundant and need not be emitted as a reference target.
fn auto_identifier(inlines: &[Inline]) -> String {
    let identifier = slug(&to_plain_text(inlines));
    if identifier.is_empty() {
        "section".to_owned()
    } else {
        identifier
    }
}

/// The underline glyph for a heading level.
fn heading_char(level: i32) -> char {
    match level {
        1 => '=',
        2 => '-',
        3 => '~',
        4 => '^',
        5 => '\'',
        _ => ' ',
    }
}

/// Split an inline sequence at each element matching `is_separator`, dropping the separators.
fn split_at(inlines: &[Inline], is_separator: impl Fn(&Inline) -> bool) -> Vec<&[Inline]> {
    let mut segments = Vec::new();
    let mut start = 0;
    for (index, inline) in inlines.iter().enumerate() {
        if is_separator(inline) {
            segments.push(inlines.get(start..index).unwrap_or(&[]));
            start = index + 1;
        }
    }
    segments.push(inlines.get(start..).unwrap_or(&[]));
    segments
}
