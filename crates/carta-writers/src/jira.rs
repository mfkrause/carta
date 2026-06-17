//! Jira writer: renders the document model to Jira wiki markup.
//!
//! Inline content is not wrapped — a soft break renders as a single space and block structure is
//! conveyed through Jira's line-oriented markup. Output carries no trailing newline; the caller
//! appends one. This format has no public specification, so its rules are stated directly here.

use std::fmt::Write as _;

use carta_ast::{
    Attr, Block, Document, Format, Inline, MathType, QuoteType, Row, Table, Target, to_plain_text,
};
use carta_core::{Result, Writer, WriterOptions};

use crate::common::{self, GridSlot, RawTrim, RowSpanGrid};

/// Renders a document to Jira wiki markup (no trailing newline).
#[derive(Debug, Default, Clone, Copy)]
pub struct JiraWriter;

impl Writer for JiraWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let mut state = State::default();
        let body = state.blocks(&document.blocks);
        Ok(state.finish(body))
    }
}

/// Collects footnote bodies as they are encountered so they can be emitted as a numbered section at
/// the end of the document.
#[derive(Debug, Default)]
struct State {
    notes: Vec<Note>,
}

/// A collected footnote: its rendered body and whether that body's final block ends with a single
/// trailing newline (code block, header, blockquote, rule) rather than a paragraph break.
#[derive(Debug)]
struct Note {
    body: String,
    ends_single_newline: bool,
}

impl State {
    /// Append the collected footnote section to the rendered body. Notes are numbered in encounter
    /// order, each introduced by `\[N]`, and separated from the body and from one another by a blank
    /// line plus a leading blank line.
    fn finish(&self, body: String) -> String {
        if self.notes.is_empty() {
            return body;
        }
        let mut out = body;
        for (index, note) in self.notes.iter().enumerate() {
            let _ = write!(out, "\n\n\n\\[{}] {}", index + 1, note.body);
        }
        if !self
            .notes
            .last()
            .is_some_and(|note| note.ends_single_newline)
        {
            out.push('\n');
        }
        out
    }

    /// Render a top-level block sequence.
    fn blocks(&mut self, blocks: &[Block]) -> String {
        let rendered: Vec<(&Block, String)> = blocks
            .iter()
            .map(|block| (block, self.block(block)))
            .collect();
        join_blocks(&rendered)
    }

    fn block(&mut self, block: &Block) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => self.inlines(inlines),
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines),
            Block::CodeBlock(attr, text) => {
                let block = code_block(attr, text);
                if attr.id.is_empty() {
                    block
                } else {
                    format!("{{anchor:{}}}\n\n{block}", attr.id)
                }
            }
            Block::RawBlock(format, text) => raw_passthrough(format, text),
            Block::BlockQuote(blocks) => self.block_quote(blocks),
            Block::BulletList(items) => self.list('*', items),
            Block::OrderedList(_, items) => self.list('#', items),
            Block::DefinitionList(items) => self.definition_list(items),
            Block::HorizontalRule => "----".to_owned(),
            Block::Table(table) => self.table(table),
            Block::Figure(_, _, blocks) => self.blocks(blocks),
            Block::Div(attr, blocks) => {
                let body = self.blocks(blocks);
                if attr.id.is_empty() {
                    body
                } else {
                    format!("{{anchor:{}}}{body}", attr.id)
                }
            }
            Block::LineBlock(lines) => self.line_block(lines),
        }
    }

    fn header(&mut self, level: i32, attr: &Attr, inlines: &[Inline]) -> String {
        let text = self.inlines(inlines);
        format!("h{level}. {{anchor:{}}}{text}", attr.id)
    }

    fn block_quote(&mut self, blocks: &[Block]) -> String {
        if let [Block::Para(inlines) | Block::Plain(inlines)] = blocks {
            return format!("bq. {}", self.inlines(inlines));
        }
        let body = self.blocks(blocks);
        let trailing = match blocks.last() {
            Some(block) if ends_single_newline(block) => "",
            _ => "\n",
        };
        format!("{{quote}}\n{body}{trailing}{{quote}}")
    }

    fn list(&mut self, marker: char, items: &[Vec<Block>]) -> String {
        self.list_levels(marker, items, "")
    }

    /// Render a list in the prefix notation. `parent` is the accumulated marker run of the enclosing
    /// levels; this level appends its own marker to it on every line.
    fn list_levels(&mut self, marker: char, items: &[Vec<Block>], parent: &str) -> String {
        let prefix = format!("{parent}{marker}");
        let mut lines: Vec<String> = Vec::new();
        for item in items {
            // An item carries its marker on its first text line; an item whose first block is a
            // sublist has no such line, so the marker is carried into the sublist's first line.
            let mut item_has_marker = false;
            for inner in item {
                match inner {
                    Block::Plain(inlines) | Block::Para(inlines) => {
                        let text = self.inlines(inlines);
                        if item_has_marker {
                            lines.push(text);
                        } else {
                            lines.push(format!("{prefix} {text}"));
                            item_has_marker = true;
                        }
                    }
                    Block::BulletList(sub) => {
                        lines.push(self.list_levels('*', sub, &prefix));
                        item_has_marker = true;
                    }
                    Block::OrderedList(_, sub) => {
                        lines.push(self.list_levels('#', sub, &prefix));
                        item_has_marker = true;
                    }
                    other => lines.push(self.block(other)),
                }
            }
        }
        lines.join("\n")
    }

    fn definition_list(&mut self, items: &[(Vec<Inline>, Vec<Vec<Block>>)]) -> String {
        let mut lines: Vec<String> = Vec::new();
        for (term, definitions) in items {
            lines.push(format!("* *{}*", self.inlines(term)));
            for definition in definitions {
                lines.push(self.cell_blocks(definition));
            }
        }
        lines.join("\n")
    }

    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        let rendered: Vec<String> = lines.iter().map(|line| self.inlines(line)).collect();
        rendered.join("\n")
    }

    fn table(&mut self, table: &Table) -> String {
        let mut rows: Vec<String> = Vec::new();
        let mut grid = RowSpanGrid::new(table.col_specs.len());
        for row in &table.head.rows {
            rows.push(self.table_row(row, true, &mut grid));
        }
        for body in &table.bodies {
            for row in body.head.iter().chain(body.body.iter()) {
                rows.push(self.table_row(row, false, &mut grid));
            }
        }
        for row in &table.foot.rows {
            rows.push(self.table_row(row, false, &mut grid));
        }
        rows.join("\n")
    }

    /// Render one table row. A cell that spans multiple columns is followed by that many blank cells,
    /// and a column still covered by a row span opened above contributes a blank cell here, so every
    /// row presents the same column count.
    fn table_row(&mut self, row: &Row, header: bool, grid: &mut RowSpanGrid) -> String {
        let delimiter = if header { "||" } else { "|" };
        let mut out = String::from(delimiter);
        let blank = format!("  {delimiter}");
        for slot in grid.place_slots(&row.cells) {
            match slot {
                GridSlot::Cell(_, cell) => {
                    let content = self.cell_blocks(&cell.content);
                    let _ = write!(out, " {content} {delimiter}");
                }
                GridSlot::Covered => out.push_str(&blank),
            }
        }
        out
    }

    /// Render a cell's (or definition's) blocks: inline content with paragraph breaks flattened to a
    /// single newline, since Jira cells carry no inter-block spacing.
    fn cell_blocks(&mut self, blocks: &[Block]) -> String {
        let rendered: Vec<String> = blocks.iter().map(|block| self.block(block)).collect();
        rendered.join("\n")
    }

    fn inlines(&mut self, inlines: &[Inline]) -> String {
        self.inlines_bounded(inlines, None, None)
    }

    /// Render an inline sequence, threading neighbor characters so that each child's edge-sensitive
    /// rendering reflects what surrounds it. `prev` seeds the character before the first child and
    /// `after` is the character following the last child.
    fn inlines_bounded(
        &mut self,
        inlines: &[Inline],
        prev: Option<char>,
        after: Option<char>,
    ) -> String {
        let mut out = String::new();
        for (index, inline) in inlines.iter().enumerate() {
            let before = out
                .chars()
                .next_back()
                .or(if index == 0 { prev } else { None });
            let next = match inlines.get(index + 1) {
                Some(following) => leading_char(following),
                None => after,
            };
            out.push_str(&self.inline(inline, before, next));
        }
        out
    }

    fn inline(&mut self, inline: &Inline, prev: Option<char>, next: Option<char>) -> String {
        match inline {
            Inline::Str(text) => {
                let text = normalize_whitespace(text);
                escape_text_with(&text, prev, next)
            }
            Inline::Emph(inlines) => self.emphasized('_', inlines, prev, next),
            Inline::Strong(inlines) | Inline::SmallCaps(inlines) => {
                self.emphasized('*', inlines, prev, next)
            }
            Inline::Underline(inlines) => self.emphasized('+', inlines, prev, next),
            Inline::Strikeout(inlines) => self.emphasized('-', inlines, prev, next),
            Inline::Superscript(inlines) => self.emphasized('^', inlines, prev, next),
            Inline::Subscript(inlines) => self.emphasized('~', inlines, prev, next),
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_marks(kind);
                format!(
                    "{open}{}{close}",
                    self.inlines_bounded(inlines, Some(open), Some(close))
                )
            }
            Inline::Cite(_, inlines) => self.inlines_bounded(inlines, prev, next),
            Inline::Code(_, text) => format!("{{{{{}}}}}", escape_monospaced(text)),
            Inline::Space | Inline::SoftBreak => " ".to_owned(),
            Inline::LineBreak => "\n".to_owned(),
            Inline::Math(kind, text) => self.math(kind, text),
            Inline::RawInline(format, text) => raw_passthrough(format, text),
            Inline::Link(_, inlines, target) => self.link(inlines, target),
            Inline::Image(attr, inlines, target) => self.image(attr, inlines, target),
            Inline::Span(attr, inlines) => {
                let body = self.inlines_bounded(inlines, prev, next);
                if attr.id.is_empty() {
                    body
                } else {
                    format!("{{anchor:{}}}{body}", attr.id)
                }
            }
            Inline::Note(blocks) => self.note(blocks),
        }
    }

    /// Render an emphasis span with marker `marker`. Jira recognizes a bare `m…m` only when the span
    /// is flanked by markup boundaries; when it abuts surrounding word text the markers are wrapped in
    /// braces (`{m}…{m}`) so they are still parsed as markup.
    fn emphasized(
        &mut self,
        marker: char,
        inlines: &[Inline],
        prev: Option<char>,
        next: Option<char>,
    ) -> String {
        let body = self.inlines_bounded(inlines, Some(marker), Some(marker));
        if bracket_before(prev) || bracket_after(next) {
            format!("{{{marker}}}{body}{{{marker}}}")
        } else {
            format!("{marker}{body}{marker}")
        }
    }

    fn math(&mut self, _kind: &MathType, _text: &str) -> String {
        todo!("Jira writer: render Math by converting TeX to Jira markup with a TeX fallback")
    }

    fn link(&mut self, inlines: &[Inline], target: &Target) -> String {
        let label = self.inlines(inlines);
        if label.is_empty() || to_plain_text(inlines) == target.url {
            format!("[{}]", target.url)
        } else {
            format!("[{label}|{}]", target.url)
        }
    }

    fn image(&mut self, attr: &Attr, inlines: &[Inline], target: &Target) -> String {
        let mut params: Vec<String> = Vec::new();
        if !target.title.is_empty() {
            params.push(format!("title={}", target.title));
        }
        let alt = self.inlines(inlines);
        if !alt.is_empty() {
            params.push(format!("alt={alt}"));
        }
        for (key, value) in &attr.attributes {
            params.push(format!("{key}={value}"));
        }
        if params.is_empty() {
            format!("!{}!", target.url)
        } else {
            format!("!{}|{}!", target.url, params.join(", "))
        }
    }

    fn note(&mut self, blocks: &[Block]) -> String {
        let body = self.blocks(blocks);
        let ends_single_newline = blocks.last().is_some_and(ends_single_newline);
        self.notes.push(Note {
            body,
            ends_single_newline,
        });
        format!("[{}]", self.notes.len())
    }
}

/// Join already-rendered blocks. The preceding block decides the gap: a header, code block,
/// blockquote, or horizontal rule joins to the next block with a single newline; everything else is
/// separated by a blank line.
fn join_blocks(rendered: &[(&Block, String)]) -> String {
    let mut out = String::new();
    for (index, (_, text)) in rendered.iter().enumerate() {
        if index > 0 {
            match rendered.get(index - 1) {
                Some((prev, _)) if ends_single_newline(prev) => out.push('\n'),
                _ => out.push_str("\n\n"),
            }
        }
        out.push_str(text);
    }
    out
}

/// Whether a block, when followed by another, is separated from it by a single newline rather than a
/// blank line.
fn ends_single_newline(block: &Block) -> bool {
    matches!(
        block,
        Block::Header(..) | Block::CodeBlock(..) | Block::BlockQuote(_) | Block::HorizontalRule
    )
}

/// The span markers whose bare form opens or closes Jira inline markup.
const SPAN_MARKERS: &[char] = &['*', '_', '+', '-', '^', '~', '!', '|', '[', ']', '(', '&'];

/// The punctuation Jira does not treat as word content: the span markers plus the brace, colon,
/// semicolon, and question-mark characters that also delimit markup. A character in this set sits
/// on neither side of the content/non-content boundary the escape and bracketing tests key off.
const NEUTRAL_PUNCT: &[char] = &[
    '*', '_', '+', '-', '^', '~', '!', '|', '[', ']', '(', '&', '{', '}', ':', ';', '?',
];

/// Whether an emphasis span needs braced markers because the character before it is word-like.
/// A leading marker is recognized bare only after a markup boundary; it must be braced after a word
/// character or the neutral punctuation that Jira does not treat as a boundary.
fn bracket_before(prev: Option<char>) -> bool {
    match prev {
        None => false,
        Some(ch) if ch.is_whitespace() => false,
        Some(ch) => !NEUTRAL_PUNCT.contains(&ch),
    }
}

/// Whether an emphasis span needs braced markers because the character after it is alphanumeric, the
/// only case in which a trailing bare marker would not be recognized.
fn bracket_after(next: Option<char>) -> bool {
    next.is_some_and(char::is_alphanumeric)
}

/// The first character an inline contributes to the output, used to test the flanking of a preceding
/// emphasis span. Container inlines defer to their first child; markers and structural inlines return
/// their own opening character.
fn leading_char(inline: &Inline) -> Option<char> {
    match inline {
        Inline::Str(text) => normalize_whitespace(text).chars().next(),
        Inline::Space | Inline::SoftBreak => Some(' '),
        Inline::LineBreak => Some('\n'),
        Inline::Emph(_) => Some('_'),
        Inline::Strong(_) | Inline::SmallCaps(_) => Some('*'),
        Inline::Underline(_) => Some('+'),
        Inline::Strikeout(_) => Some('-'),
        Inline::Superscript(_) => Some('^'),
        Inline::Subscript(_) => Some('~'),
        Inline::Code(_, _) => Some('{'),
        Inline::Link(..) | Inline::Note(_) => Some('['),
        Inline::Image(..) => Some('!'),
        Inline::Quoted(kind, _) => match kind {
            QuoteType::SingleQuote => Some('\''),
            QuoteType::DoubleQuote => Some('"'),
        },
        Inline::Cite(_, inlines) | Inline::Span(_, inlines) => {
            inlines.iter().find_map(leading_char)
        }
        Inline::Math(..) | Inline::RawInline(..) => None,
    }
}

fn quote_marks(kind: &QuoteType) -> (char, char) {
    match kind {
        QuoteType::SingleQuote => ('\'', '\''),
        QuoteType::DoubleQuote => ('"', '"'),
    }
}

/// Render a code block: a `{code:lang}` block when the first class names a language, else a verbatim
/// `{noformat}` block. The body is emitted verbatim.
fn code_block(attr: &Attr, text: &str) -> String {
    if let Some(language) = attr.classes.first() {
        format!("{{code:{language}}}\n{text}\n{{code}}")
    } else {
        format!("{{noformat}}\n{text}{{noformat}}")
    }
}

/// Emit a raw-passthrough payload verbatim when its format is Jira markup; otherwise drop it.
fn raw_passthrough(format: &Format, text: &str) -> String {
    common::raw_passthrough(format, text, "jira", RawTrim::Keep)
}

/// Normalize the spacing of literal text: each run of ASCII spaces collapses to one, and a single
/// trailing space is dropped. Inter-element spacing is carried by `Space`/`SoftBreak` inlines, so a
/// space left at the end of a text token is redundant. Other whitespace (tabs, non-breaking spaces)
/// is preserved verbatim.
fn normalize_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        if ch == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Escape Jira markup characters in text. Braces are always escaped, since they begin Jira's
/// `{macro}` syntax. A span marker (`* _ + - ^ ~ ! | [ ] ( &`) is escaped only where it could be
/// parsed as opening or closing markup: at the string edge, or at a transition between content and a
/// non-content position (whitespace or another marker) — a marker resting entirely within content or
/// entirely within non-content is left alone. `?` is escaped only when it opens the citation digraph
/// `??`. The `prev`/`next` neighbors are supplied by the caller so the test reflects the surrounding
/// inline stream, not just this string's own ends.
fn escape_text_with(text: &str, mut prev: Option<char>, after: Option<char>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let next = chars.peek().copied().or(after);
        if ch == '\\' {
            out.push_str("&bsol;");
        } else {
            if needs_escape(ch, prev, next) {
                out.push('\\');
            }
            out.push(ch);
        }
        prev = Some(ch);
    }
    out
}

/// Escape the contents of a monospaced (`{{…}}`) span: only the brace characters that would close
/// the span or open a macro are neutralized; everything else, including markup characters and
/// backslashes, is verbatim.
fn escape_monospaced(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '{' || ch == '}' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn needs_escape(ch: char, prev: Option<char>, next: Option<char>) -> bool {
    match ch {
        '{' | '}' => true,
        '?' => next == Some('?'),
        _ if is_marker(ch) => {
            prev.is_none() || next.is_none() || is_content(prev) != is_content(next)
        }
        _ => false,
    }
}

/// Whether a character is a span marker subject to edge/transition escaping.
fn is_marker(ch: char) -> bool {
    SPAN_MARKERS.contains(&ch)
}

/// Whether a neighbor counts as content for the transition test: not whitespace and not one of the
/// Jira-significant punctuation characters that delimit markup.
fn is_content(neighbor: Option<char>) -> bool {
    match neighbor {
        None => false,
        Some(ch) => !ch.is_whitespace() && !NEUTRAL_PUNCT.contains(&ch),
    }
}
