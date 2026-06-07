//! reStructuredText writer: renders the document model to RST source text.
//!
//! Block structure is conveyed through directives and a three-space indent; inline emphasis maps to
//! `*`/`**`, inline code to double backticks, and roles such as `:sup:` and `:math:`. Footnotes and
//! image substitutions are collected while rendering and emitted as definition sections after the
//! body. Output carries no trailing newline; the caller appends one. Content is wrapped at a fill
//! column of 72.

use oxidoc_ast::{
    Attr, Block, Caption, Document, Format, Inline, ListAttributes, MathType, Target, to_plain_text,
};
use oxidoc_core::{Result, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, Piece, attribute_value, display_width, fill, indent_block, offset_as_i32,
    ordered_marker, quote_marks,
};

/// Width of the transition emitted for a horizontal rule.
const RULE_WIDTH: usize = 14;

/// Renders a document to reStructuredText.
#[derive(Debug, Default, Clone, Copy)]
pub struct RstWriter;

impl Writer for RstWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let mut state = State::default();
        let body = state.blocks_to_string(&document.blocks, FILL_COLUMN);
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
    Word { text: String, complex: bool },
    Space,
    Hard,
}

impl State {
    /// Render a block sequence into the document's default layout. Consecutive blocks are separated
    /// by a blank line, except that a [`Block::Plain`] is followed by a single newline when the next
    /// block can sit directly beneath it (see [`tight_after_plain`]). Blocks that render empty are
    /// dropped.
    fn blocks_to_string(&mut self, blocks: &[Block], width: usize) -> String {
        let mut out = String::new();
        let mut previous_plain: Option<bool> = None;
        for block in blocks {
            let text = self.block(block, width);
            if text.is_empty() {
                continue;
            }
            if let Some(was_plain) = previous_plain {
                let separator = if was_plain && tight_after_plain(block) {
                    "\n"
                } else {
                    "\n\n"
                };
                out.push_str(separator);
            }
            out.push_str(&text);
            previous_plain = Some(matches!(block, Block::Plain(_)));
        }
        out
    }

    fn block(&mut self, block: &Block, width: usize) -> String {
        match block {
            Block::Plain(inlines) => fill(&to_pieces(self.tokens(inlines)), width),
            Block::Para(inlines) => self.para(inlines, width),
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines),
            Block::CodeBlock(attr, text) => code_block(attr, text),
            Block::RawBlock(format, text) => raw_block(format, text),
            Block::BlockQuote(blocks) => {
                let body = self.blocks_to_string(blocks, width.saturating_sub(3));
                indent_block(&body, "   ", "   ")
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

    fn header(&mut self, level: i32, attr: &Attr, inlines: &[Inline]) -> String {
        let line = flatten(self.tokens(inlines)).trim().to_owned();
        let underline = heading_char(level).to_string().repeat(display_width(&line));
        let header = format!("{line}\n{underline}");
        if attr.id.is_empty() || attr.id == auto_identifier(inlines) {
            header
        } else {
            format!(".. _{}:\n\n{header}", attr.id)
        }
    }

    fn bullet_list(&mut self, items: &[Vec<Block>], width: usize) -> String {
        let mut units = Vec::new();
        for item in items {
            let simple = matches!(item.as_slice(), [Block::Plain(_)]);
            let body = self.blocks_to_string(item, width.saturating_sub(2));
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
            let simple = matches!(item.as_slice(), [Block::Plain(_)]);
            let body = self.blocks_to_string(item, width.saturating_sub(field));
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
                let body = self.blocks_to_string(definition, width.saturating_sub(3));
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
        let caption_text = self.blocks_to_string(&caption.long, width.saturating_sub(3));
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
        let body = self.blocks_to_string(blocks, width.saturating_sub(3));
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
                    out.push(Token::Word {
                        text: self.flat_nested(inlines, false),
                        complex: true,
                    });
                }
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_marks(kind);
                out.push(Token::Word {
                    text: format!("{open}{}{close}", self.flat_nested(inlines, in_emphasis)),
                    complex: false,
                });
            }
            Inline::Cite(_, inlines) | Inline::Span(_, inlines) => {
                let inner = self.tokens_nested(inlines, in_emphasis);
                out.extend(inner);
            }
            Inline::Code(_, text) => out.push(Token::Word {
                text: if text.contains('`') {
                    literal_role(text)
                } else {
                    format!("``{text}``")
                },
                complex: true,
            }),
            Inline::Math(_, tex) => out.push(Token::Word {
                text: format!(":math:`{tex}`"),
                complex: true,
            }),
            Inline::RawInline(format, text) => {
                if format.0.eq_ignore_ascii_case("rst") {
                    out.push(Token::Word {
                        text: text.clone(),
                        complex: false,
                    });
                }
            }
            Inline::Link(_, label, target) => self.link(label, target, out),
            Inline::Image(attr, alt, target) => {
                if in_emphasis {
                    for child in alt {
                        self.token(child, true, out);
                    }
                } else {
                    self.image(attr, alt, target, out);
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
        if in_emphasis {
            for child in inlines {
                self.token(child, true, out);
            }
            return;
        }
        if matches!(kind, PhraseKind::Leaf) {
            out.push(Token::Word {
                text: format!("{open}{}{close}", self.flat_nested(inlines, true)),
                complex: true,
            });
            return;
        }
        let mut run_start = 0;
        for (index, child) in inlines.iter().enumerate() {
            if breaks_out(child, kind) {
                if let Some(segment) = inlines.get(run_start..index) {
                    self.flush_phrase(segment, open, close, out);
                }
                self.token(child, false, out);
                run_start = index + 1;
            }
        }
        if let Some(segment) = inlines.get(run_start..) {
            self.flush_phrase(segment, open, close, out);
        }
    }

    /// Wrap one uninterrupted run of phrase content in its markers, keeping any leading or trailing
    /// whitespace outside the markers (RST markup may not be padded by spaces on the inside).
    fn flush_phrase(&mut self, segment: &[Inline], open: &str, close: &str, out: &mut Vec<Token>) {
        let is_space = |inline: &Inline| {
            matches!(
                inline,
                Inline::Space | Inline::SoftBreak | Inline::LineBreak
            )
        };
        let Some(first) = segment.iter().position(|inline| !is_space(inline)) else {
            for inline in segment {
                self.token(inline, true, out);
            }
            return;
        };
        let last = segment
            .iter()
            .rposition(|inline| !is_space(inline))
            .unwrap_or(first);
        if let Some(lead) = segment.get(..first) {
            for inline in lead {
                self.token(inline, true, out);
            }
        }
        if let Some(middle) = segment.get(first..=last) {
            out.push(Token::Word {
                text: format!("{open}{}{close}", self.flat_nested(middle, true)),
                complex: true,
            });
        }
        if let Some(trail) = segment.get(last + 1..) {
            for inline in trail {
                self.token(inline, true, out);
            }
        }
    }

    fn link(&mut self, label: &[Inline], target: &Target, out: &mut Vec<Token>) {
        if !target.url.is_empty() && to_plain_text(label) == target.url {
            out.push(Token::Word {
                text: target.url.clone(),
                complex: true,
            });
        } else {
            let text = self.flat(label);
            out.push(Token::Word {
                text: format!("`{text} <{}>`__", target.url),
                complex: true,
            });
        }
    }

    fn image(&mut self, attr: &Attr, alt: &[Inline], target: &Target, out: &mut Vec<Token>) {
        let plain = to_plain_text(alt);
        let name = if plain.is_empty() {
            self.fallback_count += 1;
            format!("image{}", self.fallback_count)
        } else {
            plain
        };
        let mut definition = format!(".. |{name}| image:: {}", target.url);
        for option in dimension_options(attr) {
            definition.push_str("\n   ");
            definition.push_str(&option);
        }
        self.substitutions.push(definition);
        out.push(Token::Word {
            text: format!("|{name}|"),
            complex: true,
        });
    }

    fn note(&mut self, blocks: &[Block], out: &mut Vec<Token>) {
        let index = self.footnotes.len();
        self.footnotes.push(String::new());
        let number = index + 1;
        let body = self.blocks_to_string(blocks, FILL_COLUMN.saturating_sub(3));
        let entry = if body.is_empty() {
            format!(".. [{number}]")
        } else {
            format!(".. [{number}]\n{}", indent_block(&body, "   ", "   "))
        };
        if let Some(slot) = self.footnotes.get_mut(index) {
            *slot = entry;
        }
        out.push(Token::Word {
            text: format!(" [{number}]_"),
            complex: false,
        });
    }
}

/// Join already-rendered list items or definition groups: a unit is followed by a blank line when it
/// is not a single simple paragraph, otherwise by a single newline. Empty units are dropped.
fn join_loose_items(units: Vec<(bool, String)>) -> String {
    let mut out = String::new();
    let mut previous_simple: Option<bool> = None;
    for (simple, text) in units {
        if text.is_empty() {
            continue;
        }
        if let Some(was_simple) = previous_simple {
            out.push_str(if was_simple { "\n" } else { "\n\n" });
        }
        out.push_str(&text);
        previous_simple = Some(simple);
    }
    out
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

/// Build the piece stream for the fill engine from inline tokens, inserting a `\ ` separator between
/// adjacent markup boundaries that RST would not otherwise recognize.
fn to_pieces(tokens: Vec<Token>) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut pending: Option<(bool, char)> = None;
    for token in tokens {
        match token {
            Token::Word { text, complex } => {
                let mut chars = text.chars();
                let Some(first) = chars.next() else {
                    continue;
                };
                let last = chars.last().unwrap_or(first);
                if let Some((previous_complex, previous_last)) = pending
                    && separator_needed(previous_complex, previous_last, complex, first)
                {
                    out.push(Piece::Text("\\ ".to_owned()));
                }
                out.push(Piece::Text(text));
                pending = Some((complex, last));
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

/// Character class used to decide per-character escaping inside a text run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Space,
    Word,
    Other,
    Boundary,
}

fn category(ch: char) -> Category {
    if ch.is_whitespace() {
        Category::Space
    } else if ch.is_alphanumeric() {
        Category::Word
    } else {
        Category::Other
    }
}

/// Escape the characters of a text run that RST would otherwise read as markup. A backslash is always
/// doubled; `*`, backtick, and `|` are escaped unless safely buried in or between words; a trailing-
/// style `_` is escaped unless it sits between two word characters.
fn escape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    for (index, &ch) in chars.iter().enumerate() {
        let before = if index == 0 {
            Category::Space
        } else {
            category(chars.get(index - 1).copied().unwrap_or(' '))
        };
        let after = match chars.get(index + 1) {
            Some(&next) => category(next),
            None => Category::Boundary,
        };
        match ch {
            '\\' => out.push_str("\\\\"),
            '*' | '`' | '|' => {
                if escape_symmetric(before, after) {
                    out.push('\\');
                }
                out.push(ch);
            }
            '_' => {
                if escape_underscore(before, after) {
                    out.push('\\');
                }
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// A start/end delimiter (`*`, backtick, `|`) is left unescaped only when both sides are spaces or
/// both sides are word characters.
fn escape_symmetric(before: Category, after: Category) -> bool {
    !((before == Category::Space && after == Category::Space)
        || (before == Category::Word && after == Category::Word))
}

/// A trailing-reference `_` is left unescaped only when it sits between two word characters.
fn escape_underscore(before: Category, after: Category) -> bool {
    !(before == Category::Word && after == Category::Word)
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
    let source = text.strip_suffix('\n').unwrap_or(text);
    let body = indent_block(source, "   ", "   ");
    match code_language(attr) {
        Some(language) => {
            let mut head = format!(".. code:: {language}");
            if attr.classes.iter().any(|class| class == "numberLines") {
                head.push_str("\n   :number-lines:");
            }
            format!("{head}\n\n{body}")
        }
        None => format!("::\n\n{body}"),
    }
}

/// Render a raw block: content whose format is `rst` passes through verbatim; any other format is
/// wrapped in a `.. raw::` directive naming that format.
fn raw_block(format: &Format, text: &str) -> String {
    let source = text.strip_suffix('\n').unwrap_or(text);
    if format.0.eq_ignore_ascii_case("rst") {
        source.to_owned()
    } else {
        format!(
            ".. raw:: {}\n\n{}",
            format.0,
            indent_block(source, "   ", "   ")
        )
    }
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
    let filtered: String = to_plain_text(inlines)
        .chars()
        .filter(|ch| ch.is_alphanumeric() || ch.is_whitespace() || matches!(ch, '_' | '-' | '.'))
        .flat_map(char::to_lowercase)
        .collect();
    let joined = filtered.split_whitespace().collect::<Vec<_>>().join("-");
    let slug: String = joined
        .chars()
        .skip_while(|ch| !ch.is_alphabetic())
        .collect();
    if slug.is_empty() {
        "section".to_owned()
    } else {
        slug
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
