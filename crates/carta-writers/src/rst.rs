//! reStructuredText writer: renders the document model to RST source text.
//!
//! Block structure is conveyed through directives and a three-space indent; inline emphasis maps to
//! `*`/`**`, inline code to double backticks, and roles such as `:sup:` and `:math:`. Footnotes and
//! image substitutions are collected while rendering and emitted as definition sections after the
//! body. Output carries no trailing newline; the caller appends one. Content is wrapped at a fill
//! column of 72.

use carta_ast::{
    Attr, Block, Caption, ColWidth, Document, Format, Inline, ListAttributes, ListNumberDelim,
    ListNumberStyle, MathType, MetaValue, QuoteType, Row, Table, Target, Text,
    single_block_inlines, slug, to_plain_text,
};
use carta_core::{Extension, Result, TocStyle, WrapMode, Writer, WriterOptions};

use crate::common::{
    Dimension, FILL_COLUMN, Piece, ascii_punctuation, attribute_value, block_inlines, body_rows,
    clean_prefix_len, display_width, fill, fill_cell, fill_hang, format_length_dimension,
    format_percent_dimension, indent_block, is_known_scheme, is_uri_scheme, label_matches_url,
    offset_as_i32, ordered_marker, pad_marker, parse_dimension, quote_marks,
};
use crate::grid;
use crate::grid::MAX_MEASURED_TABLE_NESTING;

/// Width of the transition emitted for a horizontal rule.
const RULE_WIDTH: usize = 14;

/// Renders a document to reStructuredText.
#[derive(Debug, Default, Clone, Copy)]
pub struct RstWriter;

impl Writer for RstWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let width = options.columns.unwrap_or(FILL_COLUMN);
        let mut state = State {
            wrap: options.wrap,
            width,
            smart: options.extensions.contains(Extension::Smart),
            ..State::default()
        };
        let body = state.blocks_to_string(&document.blocks, width, true);
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

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.rst"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }

    fn toc_style(&self) -> TocStyle {
        TocStyle::Native
    }

    fn numbers_sections_natively(&self) -> bool {
        true
    }

    fn title_block(&self, document: &Document, _options: &WriterOptions) -> Result<Option<String>> {
        let mut parts = Vec::new();
        if let Some(line) = title_line(document.meta.get("title")) {
            let bar = "=".repeat(display_width(&line));
            parts.push(format!("{bar}\n{line}\n{bar}"));
        }
        if let Some(line) = title_line(document.meta.get("subtitle")) {
            let bar = "-".repeat(display_width(&line));
            parts.push(format!("{bar}\n{line}\n{bar}"));
        }
        Ok((!parts.is_empty()).then(|| parts.join("\n")))
    }
}

/// Render a metadata value to a single reStructuredText title line, or `None` when it carries no
/// text. The line feeds an over/underlined title, whose rule length matches its display width.
fn title_line(value: Option<&MetaValue>) -> Option<String> {
    let inlines = match value? {
        MetaValue::MetaInlines(inlines) => inlines.clone(),
        MetaValue::MetaString(text) => vec![Inline::Str(text.clone())],
        MetaValue::MetaBlocks(blocks) => single_block_inlines(blocks).to_vec(),
        _ => return None,
    };
    let mut state = State::default();
    let line = flatten(state.tokens(&inlines)).trim().to_owned();
    (!line.is_empty()).then_some(line)
}

/// Collects the deferred constructs accumulated during rendering: footnote definitions and image
/// substitution definitions, both emitted as their own sections after the document body. The counter
/// names images that carry no alt text.
#[derive(Debug)]
struct State {
    footnotes: Vec<String>,
    substitutions: Vec<String>,
    fallback_count: usize,
    /// Substitution names already assigned, in assignment order. A repeated label falls back to a
    /// generated `image`-plus-counter name so each reference resolves to its own definition.
    used_names: Vec<String>,
    wrap: WrapMode,
    /// The fill column the document body lays out to.
    width: usize,
    /// Set while laying out the content of a table cell, whose field reflows to its column width
    /// even when the document is not auto-wrapped.
    in_cell: bool,
    /// Whether `smart` punctuation is rendered: quotes become straight ASCII and Unicode dashes and
    /// the ellipsis collapse to their ASCII forms, rather than passing through as literal Unicode.
    smart: bool,
    /// How many tables the current render is nested inside, counting the one being rendered.
    table_depth: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            footnotes: Vec::new(),
            substitutions: Vec::new(),
            fallback_count: 0,
            used_names: Vec::new(),
            wrap: WrapMode::default(),
            width: FILL_COLUMN,
            in_cell: false,
            smart: false,
            table_depth: 0,
        }
    }
}

/// An inline-rendering unit: an unbreakable text run carrying whether each of its edges is RST markup
/// (so that edge may need a `\ ` separator from an abutting run), a breakable space, a soft line break
/// from the source, or a forced line break. A word that only opens markup (e.g. `*one`) has a markup
/// leading edge but a plain trailing edge, so the two edges are tracked separately.
#[derive(Debug, Clone)]
enum Token {
    Word {
        text: String,
        lead_complex: bool,
        trail_complex: bool,
        lead: char,
    },
    /// A zero-width boundary that prints nothing but, like markup, needs a `\ ` separator when it
    /// meets adjacent markup (a raw inline whose target format is not being emitted).
    Marker,
    Space,
    /// A breakable space originating from a soft line break in the source, distinct from a plain
    /// space so the fill engine can preserve the break when asked to.
    Soft,
    Hard,
}

/// Build a markup or plain word whose separator-boundary character is the first character of its
/// rendered text. Escaped plain text uses [`plain_word`] instead, since escaping can prepend a
/// backslash that is not the character RST would actually see.
fn word(text: String, complex: bool) -> Token {
    edge_word(text, complex, complex)
}

/// Build a word whose leading and trailing edges may carry markup independently. A boundary word that
/// only opens markup marks its leading edge complex and its trailing edge plain, and vice versa, so an
/// interior word abutting it on the plain side is not parted by a spurious `\ ` separator.
fn edge_word(text: String, lead_complex: bool, trail_complex: bool) -> Token {
    let lead = text.chars().next().unwrap_or('\0');
    Token::Word {
        text,
        lead_complex,
        trail_complex,
        lead,
    }
}

fn is_word_token(token: &Token) -> bool {
    matches!(token, Token::Word { .. })
}

/// Whether an inline sequence yields any visible output. An empty string and a breaking space
/// render to nothing, as does a formatting wrapper around blank content; anything else (a
/// non-empty string, a hard break, code, math, media, or a nested link) is content. A link or
/// image whose target and title are empty is dropped entirely when its label is blank, since there
/// is then nothing to anchor a reference to.
fn renders_visible(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::Str(text) => !text.is_empty(),
        Inline::Space | Inline::SoftBreak => false,
        Inline::Emph(children)
        | Inline::Underline(children)
        | Inline::Strong(children)
        | Inline::SmallCaps(children)
        | Inline::Span(_, children)
        | Inline::Cite(_, children) => renders_visible(children),
        _ => true,
    })
}

impl State {
    /// Render a block sequence into the document's default layout. Consecutive blocks are separated
    /// by a blank line, except that a [`Block::Plain`] is followed by a single newline when the next
    /// block can sit directly beneath it (see [`tight_after_plain`]). Blocks that render empty are
    /// dropped.
    fn blocks_to_string(&mut self, blocks: &[Block], width: usize, top: bool) -> String {
        self.blocks_laid(blocks, width, top, false)
    }

    /// Render a block sequence as [`Self::blocks_to_string`], but when `hang` is set the first
    /// non-empty block keeps a space that opens it, so a list item's text keeps the gap the source
    /// put after the marker rather than collapsing it against the marker.
    fn blocks_laid(&mut self, blocks: &[Block], width: usize, top: bool, hang: bool) -> String {
        let mut out = String::new();
        let mut previous: Option<&Block> = None;
        let mut first = true;
        for block in blocks {
            let text = self.block_laid(block, width, top, hang && first);
            if text.is_empty() {
                continue;
            }
            if let Some(prev) = previous {
                out.push_str(block_separator(prev, block));
            }
            out.push_str(&text);
            previous = Some(block);
            first = false;
        }
        out
    }

    /// Fill inline content to `width` under the active wrap mode. Inside a table cell the field
    /// reflows to its column width even when the document is not auto-wrapped. With `hang`, a space
    /// that opens the content is kept rather than dropped.
    fn lay(&mut self, inlines: &[Inline], width: usize, hang: bool) -> String {
        let pieces = to_pieces(self.tokens(inlines));
        if self.in_cell {
            fill_cell(&pieces, width, self.wrap)
        } else if hang {
            fill_hang(&pieces, width, self.wrap)
        } else {
            fill(&pieces, width, self.wrap)
        }
    }

    fn block_laid(&mut self, block: &Block, width: usize, top: bool, hang: bool) -> String {
        match block {
            Block::Plain(inlines) => self.lay(inlines, width, hang),
            Block::Para(inlines) => self.para(inlines, width, hang),
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
            Block::Table(table) => self.table(table, width),
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
    /// otherwise its inlines are filled to `width`. With `hang`, a space that opens the paragraph is
    /// kept (see [`Self::lay`]).
    fn para(&mut self, inlines: &[Inline], width: usize, hang: bool) -> String {
        if inlines
            .iter()
            .any(|inline| matches!(inline, Inline::LineBreak))
        {
            let lines = split_at(inlines, |inline| matches!(inline, Inline::LineBreak));
            let rendered: Vec<String> = lines
                .iter()
                .map(|line| self.render_line(line, width))
                .collect();
            return rendered.join("\n");
        }
        if contains_display_math(inlines) {
            let flattened = unwrap_transparent(inlines);
            return self.para_with_math(&flattened, width);
        }
        self.lay(inlines, width, hang)
    }

    /// Render a paragraph that carries display math, splitting it into `.. math::` directives around
    /// the surrounding inline runs. A run adjacent to a directive keeps an escaped-space marker on the
    /// touching side so the inline flow survives the block break; two abutting directives are parted by
    /// a standalone escaped space.
    fn para_with_math(&mut self, inlines: &[Inline], width: usize) -> String {
        enum Piece {
            Math(String),
            Text(String),
        }
        const MARKER: &str = "\\ ";
        let mut pieces: Vec<Piece> = Vec::new();
        let mut start = 0;
        for (index, inline) in inlines.iter().enumerate() {
            if let Inline::Math(MathType::DisplayMath, tex) = inline {
                if let Some(segment) = inlines.get(start..index) {
                    let text = self.lay(segment, width, false);
                    if !text.is_empty() {
                        pieces.push(Piece::Text(text));
                    }
                }
                pieces.push(Piece::Math(tex.to_string()));
                start = index + 1;
            }
        }
        if let Some(segment) = inlines.get(start..) {
            let text = self.lay(segment, width, false);
            if !text.is_empty() {
                pieces.push(Piece::Text(text));
            }
        }

        let mut parts: Vec<String> = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            let prev_math = index
                .checked_sub(1)
                .is_some_and(|prev| matches!(pieces.get(prev), Some(Piece::Math(_))));
            let next_math = matches!(pieces.get(index + 1), Some(Piece::Math(_)));
            match piece {
                Piece::Math(tex) => {
                    if prev_math {
                        parts.push(MARKER.to_owned());
                    }
                    parts.push(math_directive(tex));
                }
                Piece::Text(text) => {
                    let mut line = String::new();
                    if prev_math {
                        line.push_str(MARKER);
                    }
                    line.push_str(text);
                    if next_math {
                        line.push_str(MARKER);
                    }
                    parts.push(line);
                }
            }
        }
        parts.join("\n\n")
    }

    /// Render one line-block line: its inlines filled to the body width, then prefixed with `| ` and
    /// continuation lines indented to match.
    fn render_line(&mut self, line: &[Inline], width: usize) -> String {
        let body = self.lay(line, width.saturating_sub(2), false);
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
        let body = self.blocks_laid(item, width, false, true);
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
        // Unstyled lists starting at 1 use the `#` auto-enumerator; an explicit style or later start needs literal numbers.
        let auto_enumerated = attrs.start == 1
            && matches!(attrs.style, ListNumberStyle::DefaultStyle)
            && matches!(attrs.delim, ListNumberDelim::DefaultDelim);
        let markers: Vec<String> = (0..items.len())
            .map(|offset| {
                if auto_enumerated {
                    "#.".to_string()
                } else {
                    let number = attrs.start.saturating_add(offset_as_i32(offset));
                    ordered_marker(number, attrs.style, attrs.delim)
                }
            })
            .collect();
        let field = markers.iter().map(|m| m.chars().count()).max().unwrap_or(0) + 1;
        let rest = " ".repeat(field);
        let mut units = Vec::new();
        for (offset, item) in items.iter().enumerate() {
            let marker = markers.get(offset).cloned().unwrap_or_default();
            let first = pad_marker(&marker, field);
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
            let term_line = self.term_line(term);
            let mut def_units = Vec::new();
            for definition in definitions {
                let simple = matches!(definition.as_slice(), [Block::Plain(_)]);
                let body = self.blocks_to_string(definition, width.saturating_sub(3), false);
                let body = lead_quote_fence(definition, body);
                let indented = if body.is_empty() {
                    String::new()
                } else {
                    indent_block(&body, "   ", "   ")
                };
                def_units.push((simple, indented));
            }
            let group_simple = definitions
                .iter()
                .all(|definition| matches!(definition.as_slice(), [Block::Plain(_)]));
            let bodies = join_loose_items(def_units);
            let group = if term_line.is_empty() {
                bodies
            } else if bodies.is_empty() {
                term_line
            } else {
                format!("{term_line}\n{bodies}")
            };
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
        if let Some((image_attr, alt, target)) = image {
            let alt_text = to_plain_text(alt);
            let alt_text = if alt_text.is_empty() {
                target.title.to_string()
            } else {
                alt_text
            };
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
        if is_bare_title(attr) {
            return String::new();
        }
        let body = self.blocks_to_string(blocks, width.saturating_sub(3), false);
        let body = lead_quote_fence(blocks, body);
        let mut directive = match attr.classes.first() {
            Some(class) if is_admonition(class) => format!(".. {class}::"),
            _ if attr.classes.is_empty() => ".. container::".to_owned(),
            _ => format!(".. container:: {}", attr.classes.join(" ")),
        };
        if !attr.id.is_empty() {
            directive.push_str("\n   :name: ");
            directive.push_str(&attr.id);
        }
        if body.is_empty() {
            return directive;
        }
        let indented = indent_block(&body, "   ", "   ");
        format!("{directive}\n\n{indented}")
    }

    /// Render inlines to a single flat line: spaces and forced breaks collapse to one space, with
    /// `\ ` separators inserted between adjacent markup boundaries. Used for content that must stay on
    /// one line (headers and the inside of inline markup).
    fn flat(&mut self, inlines: &[Inline]) -> String {
        self.flat_nested(inlines, false)
    }

    /// Render a definition-list term: like [`flat`](Self::flat), but a forced line break stays a real
    /// newline so a term that spans lines is kept split across them.
    fn term_line(&mut self, inlines: &[Inline]) -> String {
        let mut out = String::new();
        for piece in to_pieces(self.tokens_nested(inlines, false)) {
            match piece {
                Piece::Text(text) => out.push_str(&text),
                Piece::Space | Piece::Soft => out.push(' '),
                Piece::Hard => out.push('\n'),
            }
        }
        out
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
                text: escape(text, self.smart),
                lead_complex: false,
                trail_complex: false,
                lead: text.chars().next().unwrap_or('\0'),
            }),
            Inline::Space => out.push(Token::Space),
            Inline::SoftBreak => out.push(Token::Soft),
            Inline::LineBreak => out.push(Token::Hard),
            Inline::Emph(inlines) | Inline::Underline(inlines) => {
                self.phrase(inlines, in_emphasis, "*", "*", PhraseKind::Emph, out);
            }
            Inline::Strong(inlines) => {
                self.phrase(inlines, in_emphasis, "**", "**", PhraseKind::Strong, out);
            }
            Inline::Strikeout(inlines) => {
                let (open, close) = if in_emphasis {
                    ("", "")
                } else {
                    ("[STRIKEOUT:", "]")
                };
                self.wrapped(inlines, open, close, true, true, out);
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
                let (open, close) = if self.smart {
                    match kind {
                        QuoteType::SingleQuote => ('\'', '\''),
                        QuoteType::DoubleQuote => ('"', '"'),
                    }
                } else {
                    quote_marks(kind)
                };
                self.wrapped(
                    inlines,
                    &open.to_string(),
                    &close.to_string(),
                    in_emphasis,
                    false,
                    out,
                );
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
                    out.push(word(text.to_string(), false));
                } else if format.0.eq_ignore_ascii_case("latex")
                    || format.0.eq_ignore_ascii_case("tex")
                {
                    out.push(word(format!(":raw-latex:`{text}`"), true));
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
    /// those markers (a link, or a strong span inside an emphasis) interrupts the run, closing the
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
        // Inside an existing phrase the markers fall away, but breakout splitting stays in force.
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
        let Some(split) = split_run(segment, lead_break, trail_break) else {
            if segment.is_empty() && lead_break && trail_break {
                out.push(word(format!("{open}\\ {close}"), true));
            } else {
                for inline in segment {
                    self.token(inline, true, out);
                }
            }
            return;
        };
        for inline in split.lead {
            if lead_break && matches!(inline, Inline::SoftBreak | Inline::LineBreak) {
                continue;
            }
            self.token(inline, true, out);
        }
        // Markers fuse to the first and last words; the words between stay separately breakable so long phrases reflow.
        let opening = format!("{open}{}", split.lead_sep);
        let closing = format!("{}{close}", split.trail_sep);
        let body = self.tokens_nested(split.middle, true);
        wrap_run(body, &opening, &closing, true, out);
        for inline in split.trail {
            if trail_break && matches!(inline, Inline::SoftBreak | Inline::LineBreak) {
                continue;
            }
            self.token(inline, true, out);
        }
    }

    /// Wrap an inline run in fixed open/close delimiters, leaving the words between them separately
    /// breakable so a long span can reflow and a source line break inside it survives. `complex`
    /// marks the fused boundary words as markup (so neighbouring text is parted by a `\ ` separator);
    /// smart-quote glyphs are plain text and pass `false`.
    fn wrapped(
        &mut self,
        inlines: &[Inline],
        open: &str,
        close: &str,
        in_emphasis: bool,
        complex: bool,
        out: &mut Vec<Token>,
    ) {
        let body = self.tokens_nested(inlines, in_emphasis);
        wrap_run(body, open, close, complex, out);
    }

    fn link(&mut self, label: &[Inline], target: &Target, out: &mut Vec<Token>) {
        let plain = to_plain_text(label);
        if target.url.is_empty() && target.title.is_empty() && !renders_visible(label) {
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
        if label_matches_url(&plain, &target.url) && is_standalone_uri(&target.url) {
            out.push(word(target.url.to_string(), true));
            return;
        }
        // Render the label once, then classify: a separate probing render would compound exponentially down a chain of nested links.
        let breakouts: Vec<usize> = label
            .iter()
            .enumerate()
            .filter(|(_, child)| matches!(child, Inline::Link(..)))
            .map(|(index, _)| index)
            .collect();
        let mut rendered = Vec::new();
        if breakouts.is_empty() {
            self.link_run(label, target, &mut rendered);
        } else {
            let mut run_start = 0;
            for &index in &breakouts {
                let segment = label.get(run_start..index).unwrap_or(&[]);
                self.link_run(segment, target, &mut rendered);
                if let Some(child) = label.get(index) {
                    self.token(child, true, &mut rendered);
                }
                run_start = index + 1;
            }
            let segment = label.get(run_start..).unwrap_or(&[]);
            self.link_run(segment, target, &mut rendered);
        }
        // A wordless label cannot anchor a reference; collapse to an empty-label reference.
        if rendered.iter().any(is_word_token) {
            out.extend(rendered);
        } else {
            out.push(word(format!("` <{}>`__", target.url), true));
        }
    }

    /// Render one run of link label that holds no nested link, wrapping it as `` `text <url>`__ `` with
    /// the label words left breakable so the fill engine may wrap between them. An empty run renders
    /// nothing.
    fn link_run(&mut self, label: &[Inline], target: &Target, out: &mut Vec<Token>) {
        let label_tokens = self.tokens_nested(label, true);
        let Some(first) = label_tokens.iter().position(is_word_token) else {
            return;
        };
        let suffix = format!(" <{}>`__", target.url);
        let last = label_tokens
            .iter()
            .rposition(is_word_token)
            .unwrap_or(first);
        for (index, token) in label_tokens.into_iter().enumerate() {
            match token {
                Token::Word { text, .. } if index == first && index == last => {
                    out.push(edge_word(format!("`{text}{suffix}"), true, true));
                }
                Token::Word { text, .. } if index == first => {
                    out.push(edge_word(format!("`{text}"), true, false));
                }
                Token::Word { text, .. } if index == last => {
                    out.push(edge_word(format!("{text}{suffix}"), false, true));
                }
                other => out.push(other),
            }
        }
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
        if link.is_none()
            && target.url.is_empty()
            && target.title.is_empty()
            && !renders_visible(alt)
        {
            return;
        }
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
        let Some(split) = split_run(segment, lead_break, trail_break) else {
            for inline in segment {
                self.token(inline, false, out);
            }
            return;
        };
        for inline in split.lead {
            self.token(inline, false, out);
        }
        let candidate = format!("{}{}{}", split.lead_sep, split.plain, split.trail_sep);
        let name = self.substitution_name(candidate);
        self.register_image(attr, &name, target, None, out);
        for inline in split.trail {
            self.token(inline, false, out);
        }
    }

    /// The substitution name for an image labelled `plain`: its own label when that is non-empty and
    /// not already taken, otherwise a generated `image`-plus-counter name. The counter advances on
    /// every fallback, so an empty or repeated label always yields a fresh name.
    fn substitution_name(&mut self, plain: String) -> String {
        let name = if plain.is_empty() || self.used_names.contains(&plain) {
            self.fallback_count += 1;
            format!("image{}", self.fallback_count)
        } else {
            plain
        };
        self.used_names.push(name.clone());
        name
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
        let body = self.blocks_to_string(blocks, self.width.saturating_sub(3), false);
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

    /// Render a table. A non-empty caption becomes a `.. table::` directive followed by the table
    /// indented three columns; without one the table sits at the left margin. The simple form is
    /// chosen when the table is span-free, carries no explicit column widths, and fits the fill
    /// column; otherwise the bordered grid form is used.
    fn table(&mut self, table: &Table, width: usize) -> String {
        let columns = table.col_specs.len();
        self.table_depth += 1;
        let body = if columns == 0 {
            String::new()
        } else if self.table_depth > MAX_MEASURED_TABLE_NESTING {
            self.grid_table(table, columns)
        } else {
            match self.simple_layout(table, columns) {
                Some(widths) => self.simple_table(table, &widths),
                None => self.grid_table(table, columns),
            }
        };
        self.table_depth = self.table_depth.saturating_sub(1);
        match self.table_caption(&table.caption, width) {
            Some(caption) if body.is_empty() => caption,
            Some(caption) => format!("{caption}\n\n{}", indent_block(&body, "   ", "   ")),
            None => body,
        }
    }

    /// Decide whether the table renders in simple form, returning its per-column content widths when
    /// so. A single column, a row span, or an explicit column width forces the grid form; otherwise
    /// the simple form is used only when the laid-out width fits the fill column.
    fn simple_layout(&mut self, table: &Table, columns: usize) -> Option<Vec<usize>> {
        if columns <= 1 {
            return None;
        }
        let rows: Vec<&Row> = table
            .head
            .rows
            .iter()
            .chain(body_rows(table))
            .chain(table.foot.rows.iter())
            .collect();
        if rows.is_empty() {
            return None;
        }
        let has_rowspan = rows
            .iter()
            .any(|row| row.cells.iter().any(|cell| cell.row_span > 1));
        let has_explicit = table
            .col_specs
            .iter()
            .any(|spec| matches!(spec.width, ColWidth::ColWidth(fraction) if fraction > 0.0));
        if has_rowspan || has_explicit {
            return None;
        }
        let widths = self.simple_widths(&rows, columns);
        let total = widths.iter().sum::<usize>() + columns.saturating_sub(1);
        if total > self.width {
            None
        } else {
            Some(widths)
        }
    }

    /// The natural display width of each column: the widest single-column cell, with a spanning
    /// cell's whole content absorbed into the first column it covers.
    fn simple_widths(&mut self, rows: &[&Row], columns: usize) -> Vec<usize> {
        let mut widths = vec![0usize; columns];
        let snapshot = self.snapshot();
        for row in rows {
            let mut col = 0;
            for cell in &row.cells {
                if col >= columns {
                    break;
                }
                let span = grid::span_count(cell.col_span).min(columns - col);
                let lines = self.cell_lines(&cell.content, SIMPLE_WIDTH);
                let mut content = lines
                    .iter()
                    .map(|line| display_width(line))
                    .max()
                    .unwrap_or(0);
                // A trailing cell space is trimmed from the field but still holds a column of width.
                if cell_ends_with_space(&cell.content) {
                    content += 1;
                }
                if let Some(slot) = widths.get_mut(col) {
                    *slot = (*slot).max(content);
                }
                col += span;
            }
        }
        self.restore(snapshot);
        widths
    }

    /// A simple table: `=` rules above and below, plus one under a non-empty header. Cells render at
    /// their natural width with no wrapping; a column-spanning cell occupies its merged field, and a
    /// row holding one is followed by a `-` underline. Not indented.
    fn simple_table(&mut self, table: &Table, widths: &[usize]) -> String {
        let columns = widths.len();
        let head: Vec<&Row> = table.head.rows.iter().collect();
        let data: Vec<&Row> = body_rows(table)
            .into_iter()
            .chain(table.foot.rows.iter())
            .collect();
        let has_header = head
            .iter()
            .any(|row| row.cells.iter().any(|cell| !cell.content.is_empty()));
        let rule = equals_rule(widths);

        let mut lines: Vec<String> = vec![rule.clone()];
        for row in &head {
            let after_rule = lines.last() == Some(&rule);
            self.simple_row(row, widths, columns, after_rule, &mut lines);
        }
        if has_header {
            lines.push(rule.clone());
        }
        for row in &data {
            let after_rule = lines.last() == Some(&rule);
            self.simple_row(row, widths, columns, after_rule, &mut lines);
        }
        lines.push(rule);
        lines.join("\n")
    }

    /// Append one simple-table row: each cell's lines stacked across the column fields, the last
    /// column left unpadded so trailing space is dropped, then a `-` underline when the row carries a
    /// column span.
    fn simple_row(
        &mut self,
        row: &Row,
        widths: &[usize],
        columns: usize,
        after_rule: bool,
        lines: &mut Vec<String>,
    ) {
        let mut col_lines: Vec<Vec<String>> = vec![Vec::new(); columns];
        let mut placements: Vec<(usize, usize)> = Vec::new();
        let mut col = 0;
        for cell in &row.cells {
            if col >= columns {
                break;
            }
            let span = grid::span_count(cell.col_span).min(columns - col);
            if let Some(slot) = col_lines.get_mut(col) {
                *slot = self.cell_lines(&cell.content, SIMPLE_WIDTH);
            }
            placements.push((col, span));
            col += span;
        }
        // A row right after a `=` rule with an empty first cell would start with whitespace, which
        // the grammar reads as rule continuation; a lone backslash holds the column open.
        let row_has_content = col_lines.iter().any(|lines| !lines.is_empty());
        if after_rule
            && row_has_content
            && let Some(first) = col_lines.first_mut()
            && first.is_empty()
        {
            first.push("\\".to_owned());
        }
        let height = col_lines.iter().map(Vec::len).max().unwrap_or(0).max(1);
        for line in 0..height {
            lines.push(lay_simple_line(&col_lines, widths, columns, line));
        }
        if placements.iter().any(|&(_, span)| span > 1) {
            lines.push(colspan_underline(&placements, widths));
        }
    }

    /// A grid table: bordered cells whose widths come from explicit fractional specs or a
    /// content-proportional fit; the engine in [`crate::grid`] draws the borders without alignment
    /// colons. Not indented.
    fn grid_table(&mut self, table: &Table, columns: usize) -> String {
        let head: Vec<&Row> = table.head.rows.iter().collect();
        let body = body_rows(table);
        let foot: Vec<&Row> = table.foot.rows.iter().collect();
        let head_layout = grid::place_columns(&head, columns);
        let body_layout = grid::place_columns(&body, columns);
        let foot_layout = grid::place_columns(&foot, columns);

        let mut natural = vec![0usize; columns];
        let mut minword = vec![0usize; columns];
        if self.table_depth > MAX_MEASURED_TABLE_NESTING {
            // Measuring re-renders every cell, compounding per nesting level; past the cap,
            // columns take an even share of the fill width, keeping total work linear.
            let share = (self.width / columns.max(1)).max(1);
            natural.fill(share);
            minword.fill(1);
        } else {
            let snapshot = self.snapshot();
            for (rows, layout) in [
                (&head, &head_layout),
                (&body, &body_layout),
                (&foot, &foot_layout),
            ] {
                self.measure_grid(rows, layout, &mut natural, &mut minword);
            }
            self.restore(snapshot);
        }

        let colspans: Vec<(usize, usize)> = [&head_layout, &body_layout, &foot_layout]
            .into_iter()
            .flatten()
            .flatten()
            .copied()
            .filter(|&(_, span)| span > 1)
            .collect();
        let content = grid::grid_content_widths(
            &table.col_specs,
            &natural,
            &minword,
            &colspans,
            columns,
            self.width,
            self.wrap,
        );
        let col_widths: Vec<usize> = content.iter().map(|width| width + 2).collect();
        let head_grid = self.grid_rows(&head, &head_layout, &content);
        let body_grid = self.grid_rows(&body, &body_layout, &content);
        let foot_grid = self.grid_rows(&foot, &foot_layout, &content);

        grid::render(&grid::GridTable {
            col_widths,
            aligns: None,
            head: head_grid,
            body: body_grid,
            foot: foot_grid,
        })
    }

    /// Accumulate the natural and longest-word widths of every single-column cell into the
    /// per-column maxima, rendering each cell at an unconstrained width.
    fn measure_grid(
        &mut self,
        rows: &[&Row],
        layout: &[Vec<(usize, usize)>],
        natural: &mut [usize],
        minword: &mut [usize],
    ) {
        for (row_index, row) in rows.iter().enumerate() {
            for (cell_index, cell) in row.cells.iter().enumerate() {
                let Some(&(start, span)) = layout
                    .get(row_index)
                    .and_then(|placements| placements.get(cell_index))
                else {
                    continue;
                };
                let lines = self.cell_lines(&cell.content, SIMPLE_WIDTH);
                let (width, word) = measure_unbreakable(&lines);
                let share_natural = width.div_ceil(span.max(1));
                let share_word = word.div_ceil(span.max(1));
                for column in start..start + span {
                    if let Some(value) = natural.get_mut(column) {
                        *value = (*value).max(share_natural);
                    }
                    if let Some(value) = minword.get_mut(column) {
                        *value = (*value).max(share_word);
                    }
                }
            }
        }
    }

    /// Build the grid rows for one section, rendering each cell's content to lines at the width of
    /// the columns it spans.
    fn grid_rows(
        &mut self,
        rows: &[&Row],
        layout: &[Vec<(usize, usize)>],
        content: &[usize],
    ) -> Vec<grid::GridRow> {
        let mut result = Vec::with_capacity(rows.len());
        for (row_index, row) in rows.iter().enumerate() {
            let mut cells = Vec::with_capacity(row.cells.len());
            for (cell_index, cell) in row.cells.iter().enumerate() {
                let Some(&(start, span)) = layout
                    .get(row_index)
                    .and_then(|placements| placements.get(cell_index))
                else {
                    continue;
                };
                let width = grid::merged_width(content, start, span);
                let lines = self.cell_lines(&cell.content, width);
                cells.push(grid::GridCell {
                    lines,
                    row_span: grid::span_count(cell.row_span),
                    col_span: grid::span_count(cell.col_span),
                });
            }
            result.push(grid::GridRow { cells });
        }
        result
    }

    /// Render a cell's block content to lines at the given width.
    fn cell_lines(&mut self, content: &[Block], width: usize) -> Vec<String> {
        let was_in_cell = self.in_cell;
        self.in_cell = true;
        let text = self.blocks_to_string(content, width, false);
        self.in_cell = was_in_cell;
        if text.is_empty() {
            Vec::new()
        } else {
            text.split('\n').map(str::to_owned).collect()
        }
    }

    /// The `.. table::` directive carrying the caption, or `None` when the caption is empty. Each
    /// caption block contributes one line; the table's own attributes are not emitted.
    fn table_caption(&mut self, caption: &Caption, _width: usize) -> Option<String> {
        let parts: Vec<String> = caption
            .long
            .iter()
            .map(|block| self.flat(block_inlines(block)))
            .filter(|line| !line.is_empty())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(format!(".. table:: {}", parts.join("\n")))
        }
    }

    fn snapshot(&self) -> (usize, usize, usize, usize) {
        (
            self.footnotes.len(),
            self.substitutions.len(),
            self.fallback_count,
            self.used_names.len(),
        )
    }

    fn restore(&mut self, snapshot: (usize, usize, usize, usize)) {
        self.footnotes.truncate(snapshot.0);
        self.substitutions.truncate(snapshot.1);
        self.fallback_count = snapshot.2;
        self.used_names.truncate(snapshot.3);
    }
}

/// Width used to render a simple-table cell: large enough that its content never wraps, so a
/// column's width is the natural extent of its widest cell.
const SIMPLE_WIDTH: usize = 100_000;

/// Whether a cell's sole paragraph closes with a space inline, which a simple table keeps as part of
/// the column rather than trimming.
fn cell_ends_with_space(content: &[Block]) -> bool {
    matches!(
        content,
        [Block::Plain(inlines) | Block::Para(inlines)]
            if matches!(inlines.last(), Some(Inline::Space))
    )
}

/// A simple table's `=` rule: a run of `=` per column width, joined by single spaces.
fn equals_rule(widths: &[usize]) -> String {
    widths
        .iter()
        .map(|width| "=".repeat(*width))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The widest line and widest token across rendered lines, where a space escaped by a preceding
/// backslash holds its token together (RST writes a non-breaking space as `\ `, which must not be
/// counted as a column-shrinking break point).
fn measure_unbreakable(lines: &[String]) -> (usize, usize) {
    let mut natural = 0usize;
    let mut minword = 0usize;
    for line in lines {
        natural = natural.max(display_width(line));
        let mut token = String::new();
        let mut prev = '\0';
        for ch in line.chars() {
            if ch.is_whitespace() && prev != '\\' {
                minword = minword.max(display_width(&token));
                token.clear();
            } else {
                token.push(ch);
            }
            prev = ch;
        }
        minword = minword.max(display_width(&token));
    }
    (natural, minword)
}

/// Lay one line of a simple-table row across the column fields. Each column is padded to its width
/// except the last, which is left bare; the columns are joined by single spaces. A spanning cell's
/// content occupies its first column and the columns it covers contribute empty fields.
fn lay_simple_line(
    col_lines: &[Vec<String>],
    widths: &[usize],
    columns: usize,
    line: usize,
) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(columns);
    for col in 0..columns {
        let text = col_lines
            .get(col)
            .and_then(|cell| cell.get(line))
            .map_or("", String::as_str);
        if col + 1 == columns {
            parts.push(text.to_owned());
        } else {
            let width = widths.get(col).copied().unwrap_or(0);
            parts.push(pad_right(text, width));
        }
    }
    parts.join(" ")
}

/// The `-` underline placed beneath a simple-table row that carries a column span: a dash run per
/// cell sized to the merged field it occupies, joined by single spaces.
fn colspan_underline(placements: &[(usize, usize)], widths: &[usize]) -> String {
    placements
        .iter()
        .map(|&(start, span)| {
            let merged =
                widths.iter().skip(start).take(span).sum::<usize>() + span.saturating_sub(1);
            "-".repeat(merged)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pad `text` on the right with spaces to `width` display columns, leaving content wider than the
/// field untouched.
fn pad_right(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(display_width(text));
    format!("{text}{}", " ".repeat(pad))
}

/// Whether a list item renders to a single line: either it has no content, or its one block is a
/// [`Block::Plain`]. Any other shape spans multiple lines and makes its list loose.
fn is_simple_item(item: &[Block]) -> bool {
    item.is_empty() || matches!(item, [Block::Plain(_)])
}

/// Join already-rendered list items or definition groups. The gap before each unit depends on the
/// one above it: a single-line unit is followed on the next line, a multi-line unit is followed
/// across a blank line. Empty units are dropped and do not influence the gap around them.
fn join_loose_items(units: Vec<(bool, String)>) -> String {
    let mut out = String::new();
    let mut previous_simple: Option<bool> = None;
    for (simple, text) in units {
        if text.is_empty() {
            continue;
        }
        if let Some(above_is_simple) = previous_simple {
            out.push_str(if above_is_simple { "\n" } else { "\n\n" });
        }
        out.push_str(&text);
        previous_simple = Some(simple);
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
        | Block::Table(_)
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
/// directive or nested marker (rules, raw passthrough, nested lists, containers, and highlighted
/// literal blocks), none of which can share the marker's line.
fn marker_stands_alone(block: &Block) -> bool {
    match block {
        Block::HorizontalRule
        | Block::BulletList(_)
        | Block::OrderedList(..)
        | Block::DefinitionList(_)
        | Block::Table(_)
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
                lead_complex,
                trail_complex,
                lead,
            } => {
                let Some(last) = text.chars().last() else {
                    continue;
                };
                if let Some((previous_trail_complex, previous_last)) = pending
                    && separator_needed(previous_trail_complex, previous_last, lead_complex, lead)
                {
                    out.push(Piece::text("\\ "));
                }
                out.push(Piece::text(text));
                pending = Some((trail_complex, last));
            }
            Token::Marker => {
                if pending.is_some_and(|(previous_complex, _)| previous_complex) {
                    out.push(Piece::text("\\ "));
                }
                pending = Some((false, MARKER_BOUNDARY));
            }
            Token::Space => {
                out.push(Piece::Space);
                pending = None;
            }
            Token::Soft => {
                out.push(Piece::Soft);
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

/// Emit `body` wrapped in `opening`/`closing` delimiters while keeping its internal spaces breakable:
/// the opening fuses to the first word token and the closing to the last, so a long run reflows with
/// the delimiters anchored to their boundary words. `complex` marks the boundary words as markup so
/// the `\ ` null-separator rules apply around them. A body with no word token collapses to a single
/// flattened word carrying both delimiters.
fn wrap_run(body: Vec<Token>, opening: &str, closing: &str, complex: bool, out: &mut Vec<Token>) {
    let first = body.iter().position(is_word_token);
    let last = body.iter().rposition(is_word_token);
    match (first, last) {
        (Some(first), Some(last)) => {
            for (index, token) in body.into_iter().enumerate() {
                match token {
                    Token::Word { text, .. } if index == first && index == last => {
                        out.push(edge_word(
                            format!("{opening}{text}{closing}"),
                            complex,
                            complex,
                        ));
                    }
                    Token::Word { text, .. } if index == first => {
                        out.push(edge_word(format!("{opening}{text}"), complex, false));
                    }
                    Token::Word { text, .. } if index == last => {
                        out.push(edge_word(format!("{text}{closing}"), false, complex));
                    }
                    other => out.push(other),
                }
            }
        }
        _ => out.push(word(
            format!("{opening}{}{closing}", flatten(body)),
            complex,
        )),
    }
}

/// Flatten inline tokens to a single line: spaces and forced breaks become one space, with the same
/// `\ ` separators [`to_pieces`] inserts between adjacent markup boundaries.
fn flatten(tokens: Vec<Token>) -> String {
    let mut out = String::new();
    for piece in to_pieces(tokens) {
        match piece {
            Piece::Text(text) => out.push_str(&text),
            Piece::Space | Piece::Soft | Piece::Hard => out.push(' '),
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
    previous_trail_complex: bool,
    previous_last: char,
    current_lead_complex: bool,
    current_first: char,
) -> bool {
    (previous_trail_complex && !is_safe_follower(current_first))
        || (current_lead_complex && !is_safe_preceder(previous_last))
}

/// A phrase run partitioned around its non-space core, with the null-separator decision for each
/// edge that abuts a broken-out child.
struct RunSplit<'a> {
    lead: &'a [Inline],
    middle: &'a [Inline],
    trail: &'a [Inline],
    plain: String,
    lead_sep: &'static str,
    trail_sep: &'static str,
}

/// Partition a run into leading whitespace, its non-space core, and trailing whitespace, returning
/// `None` when the run holds no non-space content. `lead_sep`/`trail_sep` carry the `\ ` null
/// separator for an edge that abuts a broken-out child where the core character would otherwise read
/// as continuing markup.
fn split_run(segment: &[Inline], lead_break: bool, trail_break: bool) -> Option<RunSplit<'_>> {
    let is_space = |inline: &Inline| {
        matches!(
            inline,
            Inline::Space | Inline::SoftBreak | Inline::LineBreak
        )
    };
    let first = segment.iter().position(|inline| !is_space(inline))?;
    let last = segment
        .iter()
        .rposition(|inline| !is_space(inline))
        .unwrap_or(first);
    let middle = segment.get(first..=last).unwrap_or(&[]);
    let plain = to_plain_text(middle);
    let lead_sep =
        if lead_break && first == 0 && plain.chars().next().is_some_and(|c| !is_safe_follower(c)) {
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
    Some(RunSplit {
        lead: segment.get(..first).unwrap_or(&[]),
        middle,
        trail: segment.get(last + 1..).unwrap_or(&[]),
        plain,
        lead_sep,
        trail_sep,
    })
}

/// Characters that may directly precede an inline-markup start-string.
const OPENERS: &[char] = &['-', ':', '/', '\'', '"', '<', '(', '[', '{'];

/// Characters that may directly follow an inline-markup end-string.
const CLOSERS: &[char] = &[
    '-', '.', ',', ':', ';', '!', '?', '\'', '"', ')', ']', '}', '>',
];

/// Characters that may directly follow an inline-markup end without a separator: whitespace (any
/// space, including a non-breaking space), a backslash or slash, or any end-string closer.
fn is_safe_follower(ch: char) -> bool {
    ch.is_whitespace() || ch == '\\' || ch == '/' || CLOSERS.contains(&ch)
}

/// Characters that may directly precede an inline-markup start without a separator: whitespace (any
/// space, including a non-breaking space) or any start-string opener.
fn is_safe_preceder(ch: char) -> bool {
    ch.is_whitespace() || OPENERS.contains(&ch)
}

/// Escape the characters of a text run that RST would otherwise read as markup. A backslash is always
/// doubled. A `*`, backtick, or `|` is escaped where it could open or close inline markup given its
/// neighbors. A `_` is a reference marker: it is escaped everywhere except where it is buried directly
/// before an alphanumeric and is not itself opening at a word boundary.
fn escape(text: &str, smart: bool) -> String {
    let is_trigger =
        |byte: u8| matches!(byte, b'\\' | b'*' | b'`' | b'|' | b'_') || (smart && byte >= 0x80);
    let mut out = String::new();
    let mut prev: Option<char> = None;
    let mut rest = text;
    loop {
        let clean = clean_prefix_len(rest, is_trigger);
        let Some((head, tail)) = rest.split_at_checked(clean) else {
            out.push_str(rest);
            break;
        };
        out.push_str(head);
        prev = head.chars().next_back().or(prev);
        let mut chars = tail.chars();
        let Some(ch) = chars.next() else { break };
        let next = chars.clone().next();
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
            // With `smart`, Unicode punctuation collapses to its ASCII form.
            _ => match smart.then(|| ascii_punctuation(ch)).flatten() {
                Some(ascii) => out.push_str(ascii),
                None => out.push(ch),
            },
        }
        prev = Some(ch);
        rest = chars.as_str();
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

/// Render a display-math formula as a `.. math::` directive. A single-line formula sits on the
/// directive line; a formula spanning several lines moves to an indented body, each line indented by
/// three spaces with blank lines left empty and trailing blanks dropped.
fn math_directive(tex: &str) -> String {
    if !tex.contains('\n') {
        return format!(".. math:: {}", tex.trim());
    }
    let mut lines: Vec<String> = tex
        .split('\n')
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("   {line}")
            }
        })
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    format!(".. math::\n\n{}", lines.join("\n"))
}

/// Whether any inline is display math, looking through the transparent wrappers (spans and citations)
/// that the writer renders without their own markup.
fn contains_display_math(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::Math(MathType::DisplayMath, _) => true,
        Inline::Span(_, inner) | Inline::Cite(_, inner) => contains_display_math(inner),
        _ => false,
    })
}

/// Lift the children of transparent wrappers (spans and citations) to the top level so any display
/// math they enclose drives the paragraph's `.. math::` directives directly.
fn unwrap_transparent(inlines: &[Inline]) -> Vec<Inline> {
    let mut out = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Span(_, inner) | Inline::Cite(_, inner) => {
                out.extend(unwrap_transparent(inner));
            }
            other => out.push(other.clone()),
        }
    }
    out
}

/// Whether a `Div` is a bare title wrapper: its only attribute is the single class `title`. Such a
/// wrapper carries a title implied by its surrounding directive, so reStructuredText leaves it
/// unwritten.
fn is_bare_title(attr: &Attr) -> bool {
    attr.id.is_empty()
        && attr.attributes.is_empty()
        && attr.classes.len() == 1
        && attr.classes.first().map(Text::as_str) == Some("title")
}

/// The language argument of a code block: its first class that is not the line-numbering flag.
fn code_language(attr: &Attr) -> Option<&str> {
    attr.classes
        .iter()
        .map(Text::as_str)
        .find(|class| *class != "numberLines")
}

fn code_block(attr: &Attr, text: &str) -> String {
    let head = match code_language(attr) {
        Some(language) => {
            let mut head = format!(".. code:: {language}");
            if attr.classes.iter().any(|class| class == "numberLines") {
                head.push_str("\n   :number-lines:");
                if let Some(start) = start_from(attr) {
                    head.push(' ');
                    head.push_str(start);
                }
            }
            head
        }
        None => "::".to_owned(),
    };
    literal_directive(&head, text)
}

/// The line-numbering start value carried by a code block's `startFrom` attribute, if any.
fn start_from(attr: &Attr) -> Option<&str> {
    attr.attributes
        .iter()
        .find(|(key, _)| key == "startFrom")
        .map(|(_, value)| value.as_str())
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

/// Normalize a dimension value: a unitless or pixel value is floored to whole pixels; a percentage
/// keeps its number with at least one decimal; a recognized absolute unit renders with trailing
/// zeros trimmed; anything else is dropped.
fn show_dimension(value: &str) -> Option<String> {
    match parse_dimension(value)? {
        Dimension::Pixels(count) => Some(format!("{count}px")),
        Dimension::Percent(magnitude) => Some(format_percent_dimension(magnitude)),
        Dimension::Length(magnitude, unit) => Some(format_length_dimension(magnitude, unit)),
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
    is_uri_scheme(scheme) && is_known_scheme(scheme) && url.chars().all(is_uri_char)
}

/// Whether a character may appear in a standalone URI reference.
fn is_uri_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "-._~:/?#@!$&'()*+,;=%".contains(ch)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(simple: bool, text: &str) -> (bool, String) {
        (simple, text.to_string())
    }

    #[test]
    fn escape_flanking_tests_see_the_neighbor_before_a_trigger() {
        // space before makes a potential start-string, word char before whitespace a potential end-string, buried star neither
        assert_eq!(escape("text *star", false), "text \\*star");
        assert_eq!(escape("text* tail", false), "text\\* tail");
        assert_eq!(escape("a*b", false), "a*b");
    }

    #[test]
    fn escape_underscore_depends_on_both_neighbors() {
        assert_eq!(escape("snake_case", false), "snake_case");
        assert_eq!(escape("word_ end", false), "word\\_ end");
        assert_eq!(escape("tail_", false), "tail\\_");
    }

    #[test]
    fn escape_multibyte_neighbors_survive_the_verbatim_copy() {
        assert_eq!(escape("caf\u{e9}_x", false), "caf\u{e9}_x");
        assert_eq!(escape("\u{e9} *x", true), "\u{e9} \\*x");
    }

    #[test]
    fn all_single_line_units_join_tightly() {
        let joined = join_loose_items(vec![unit(true, "a"), unit(true, "b"), unit(true, "c")]);
        assert_eq!(joined, "a\nb\nc");
    }

    #[test]
    fn all_multi_line_units_join_loosely() {
        let joined = join_loose_items(vec![unit(false, "a"), unit(false, "b")]);
        assert_eq!(joined, "a\n\nb");
    }

    #[test]
    fn the_gap_below_a_unit_follows_that_unit_not_the_whole_list() {
        // a single-line unit joins tightly even when a later unit is multi-line; a multi-line unit forces a blank after it
        let joined = join_loose_items(vec![
            unit(true, "one"),
            unit(false, "two\n\n  - sub"),
            unit(true, "three"),
        ]);
        assert_eq!(joined, "one\ntwo\n\n  - sub\n\nthree");
    }

    #[test]
    fn empty_units_are_dropped_and_do_not_set_the_gap() {
        let joined = join_loose_items(vec![unit(false, ""), unit(true, "a"), unit(true, "b")]);
        assert_eq!(joined, "a\nb");
    }

    #[test]
    fn show_dimension_normalizes_lengths() {
        // whole lengths drop the trailing zero, percentages keep one decimal, unitless renders in px, unknown units dropped
        assert_eq!(show_dimension("1.0in"), Some("1in".to_owned()));
        assert_eq!(show_dimension("2in"), Some("2in".to_owned()));
        assert_eq!(show_dimension("0.5in"), Some("0.5in".to_owned()));
        assert_eq!(show_dimension("50%"), Some("50.0%".to_owned()));
        assert_eq!(show_dimension("200px"), Some("200px".to_owned()));
        assert_eq!(show_dimension("200"), Some("200px".to_owned()));
        assert_eq!(show_dimension("4ex"), None);
    }

    #[test]
    fn substitution_names_stay_unique() {
        let mut state = State::default();
        // repeats and empty labels fall back to a counter name so every reference resolves uniquely
        assert_eq!(state.substitution_name("image".to_owned()), "image");
        assert_eq!(state.substitution_name("image".to_owned()), "image1");
        assert_eq!(state.substitution_name("logo".to_owned()), "logo");
        assert_eq!(state.substitution_name(String::new()), "image2");
        assert_eq!(state.substitution_name("image".to_owned()), "image3");
    }

    #[test]
    fn image_run_names_dedupe_across_registrations() {
        // link-embedding alt text registers each run as a substitution; a later image sharing the name must fall back so no two definitions share a label
        let link = Inline::Link(
            Box::default(),
            vec![Inline::Str("L".into())],
            Box::new(Target {
                url: "http://x".into(),
                ..Default::default()
            }),
        );
        let with_link = Inline::Image(
            Box::default(),
            vec![Inline::Str("dup".into()), Inline::Space, link],
            Box::new(Target {
                url: "a.png".into(),
                ..Default::default()
            }),
        );
        let plain = Inline::Image(
            Box::default(),
            vec![Inline::Str("dup".into())],
            Box::new(Target {
                url: "b.png".into(),
                ..Default::default()
            }),
        );
        let doc = Document {
            blocks: vec![Block::Para(vec![with_link]), Block::Para(vec![plain])],
            ..Document::default()
        };
        let out = RstWriter.write(&doc, &WriterOptions::default()).unwrap();
        assert_eq!(out.matches(".. |dup| image::").count(), 1);
        assert!(out.contains(".. |image1| image:: b.png"));
    }

    #[test]
    fn deeply_nested_tables_render_without_compounding_measurement() {
        // without the nesting cap, measurement renders would compound exponentially in depth
        use carta_ast::{Alignment, Cell, ColSpec, TableBody};

        fn nested_table(content: Vec<Block>) -> Block {
            let cell = Cell {
                attr: Attr::default(),
                align: Alignment::AlignDefault,
                row_span: 1,
                col_span: 1,
                content,
            };
            let filler = Cell {
                content: vec![Block::Para(vec![Inline::Str("cell".into())])],
                ..cell.clone()
            };
            let spec = ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            };
            Block::Table(Box::new(Table {
                col_specs: vec![spec.clone(), spec],
                bodies: vec![TableBody {
                    body: vec![Row {
                        attr: Attr::default(),
                        cells: vec![cell, filler],
                    }],
                    ..TableBody::default()
                }],
                ..Table::default()
            }))
        }

        // deep enough that compounding measurement would take minutes; capped stays under a second
        let mut block = Block::Para(vec![Inline::Str("innermost".into())]);
        for _ in 0..16 {
            block = nested_table(vec![block]);
        }
        let doc = Document {
            blocks: vec![block],
            ..Document::default()
        };
        RstWriter
            .write(&doc, &WriterOptions::default())
            .expect("write");
    }

    #[test]
    fn deeply_nested_links_render_each_label_once() {
        // a probing render per ancestor would be exponential down a chain of links in labels
        let mut inline = Inline::Str("innermost".into());
        for level in 0..24 {
            inline = Inline::Link(
                Box::default(),
                vec![Inline::Str("label".into()), Inline::Space, inline],
                Box::new(Target {
                    url: format!("https://example.com/{level}").into(),
                    ..Target::default()
                }),
            );
        }
        let doc = Document {
            blocks: vec![Block::Para(vec![inline])],
            ..Document::default()
        };
        RstWriter
            .write(&doc, &WriterOptions::default())
            .expect("write");
    }
}
