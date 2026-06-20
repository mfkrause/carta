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
            Inline::Code(_, text) => {
                let text = normalize_whitespace(text);
                format!("{{{{{}}}}}", escape_text_with(&text, None, None))
            }
            Inline::Space | Inline::SoftBreak => " ".to_owned(),
            Inline::LineBreak => "\n".to_owned(),
            Inline::Math(kind, text) => self.math(kind, text, prev, next),
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

    /// Render math. A convertible expression lowers to the writer-agnostic inline tree (italic
    /// variables, unicode sub/superscripts, symbols and Greek letters), which this writer's own
    /// inline renderer turns into Jira markup with the right brace guards. An expression with no
    /// single-line form is emitted verbatim, wrapped in the math delimiters of its kind (`$…$` for
    /// inline, `$$…$$` for display) and routed through the text path so its braces and markers are
    /// escaped. An expression that is empty or only whitespace contributes nothing. Display math is
    /// set on its own line, framed by a newline on each side.
    fn math(
        &mut self,
        kind: &MathType,
        text: &str,
        prev: Option<char>,
        next: Option<char>,
    ) -> String {
        let display = matches!(kind, MathType::DisplayMath);
        // Display content stands alone on its line, so its edges abut the framing newlines rather
        // than the surrounding stream; inline content threads its real neighbors so the edge guards
        // reflect what flanks it.
        let (left, right) = if display { (None, None) } else { (prev, next) };
        let content = match crate::math::to_inlines(text) {
            Some(inlines) => self.inlines_bounded(&inlines, left, right),
            None if text.trim().is_empty() => String::new(),
            None => {
                let delimiter = if display { "$$" } else { "$" };
                let fallback = Inline::Str(format!("{delimiter}{text}{delimiter}"));
                self.inline(&fallback, left, right)
            }
        };
        if display {
            format!("\n{content}\n")
        } else {
            content
        }
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

/// The span markers whose bare form opens or closes Jira inline markup. An open parenthesis is also
/// markup-significant but is escaped by its own rule, so it is not listed here.
const SPAN_MARKERS: &[char] = &['*', '_', '+', '-', '^', '~', '!', '|', '[', ']', '&'];

/// The punctuation Jira does not treat as word content: the span markers plus the brace, colon,
/// semicolon, and question-mark characters that also delimit markup. A character in this set sits
/// on neither side of the content/non-content boundary the escape and bracketing tests key off.
const NEUTRAL_PUNCT: &[char] = &[
    '*', '_', '+', '-', '^', '~', '!', '|', '[', ']', '(', '&', '{', '}', ':', ';', '?',
];

/// Whether an emphasis span needs braced markers because the character before it is word-like.
/// A leading marker is recognized bare only after a markup boundary; it must be braced after a word
/// character or the neutral punctuation that Jira does not treat as a boundary. Only the plain space
/// and newline that an inter-element break renders to count as a boundary — every other space
/// (non-breaking, en/em, the typographic spaces math spacing emits) is word-like to Jira, so a
/// marker resting against one is braced.
fn bracket_before(prev: Option<char>) -> bool {
    match prev {
        None => false,
        Some(ch) if is_word_boundary(ch) => false,
        Some(ch) => !NEUTRAL_PUNCT.contains(&ch),
    }
}

/// Whether a character is one of the two whitespace characters Jira treats as a markup boundary: the
/// plain space and the newline that inter-element breaks render to. All other Unicode whitespace is
/// word-like for the leading-marker test.
fn is_word_boundary(ch: char) -> bool {
    ch == ' ' || ch == '\n'
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
        Inline::Math(kind, text) => math_leading_char(kind, text),
        Inline::RawInline(..) => None,
    }
}

/// The first character a math node contributes, used to test the flanking of a preceding emphasis
/// span. Display math opens with the framing newline; inline math opens with its converted content's
/// first character, or the `$` delimiter when the expression falls back to verbatim source.
fn math_leading_char(kind: &MathType, text: &str) -> Option<char> {
    if matches!(kind, MathType::DisplayMath) {
        return Some('\n');
    }
    match crate::math::to_inlines(text) {
        Some(inlines) => inlines.iter().find_map(leading_char),
        None if text.trim().is_empty() => None,
        None => Some('$'),
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
/// `{macro}` syntax. A span marker (`* _ + - ^ ~ ! | [ ] &`) is escaped only where it could be
/// parsed as opening or closing markup: at the string edge, or at a transition between content and a
/// non-content position (whitespace or another marker) — a marker resting entirely within content or
/// entirely within non-content is left alone. `?` is escaped only when it opens the citation digraph
/// `??`. An open parenthesis is escaped wherever Jira would read it as the start of an emoticon or
/// macro, and the colon/semicolon that open a text emoticon (`:)`, `:(`, `;)`, …) are escaped to keep
/// Jira from rendering an icon in their place. A backslash is kept literal when it sits between two
/// like neighbors (both word content, or both spaces) and entity-escaped to `&bsol;` otherwise, since
/// elsewhere Jira would consume it as an escape. The `prev`/`after` neighbors are supplied by the
/// caller so the tests reflect the surrounding inline stream, not just this string's own ends.
fn escape_text_with(text: &str, prev: Option<char>, after: Option<char>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    // The character at an absolute offset within this string, falling back to the supplied
    // neighbor when the offset runs off the trailing end.
    let at = |offset: usize| -> Option<char> { chars.get(offset).copied().or(after) };
    let mut offset = 0;
    while let Some(&ch) = chars.get(offset) {
        // The character one position before, falling back to the caller's preceding neighbor.
        let before = match offset.checked_sub(1) {
            Some(earlier) => chars.get(earlier).copied(),
            None => prev,
        };
        let next = at(offset + 1);
        if ch == '\\' {
            // A backslash stays literal between two like neighbors (both content, or both spaces) and
            // is otherwise consumed, so it is emitted as the `&bsol;` entity. A neighbor that is
            // itself escaped into literal text as part of an emoticon reads as ordinary content here.
            let effective_before = if neutralized_before(&chars, offset) {
                Some('a')
            } else {
                before
            };
            let effective_next = if neutralized_after(&chars, offset, after) {
                Some('a')
            } else {
                next
            };
            if backslash_is_literal(effective_before, effective_next) {
                out.push('\\');
            } else {
                out.push_str("&bsol;");
            }
            offset += 1;
            continue;
        }
        if ch == '(' {
            // A recognized icon body delimited by `)` is emitted with its inner characters bare, the
            // opening parenthesis escaped so Jira renders the literal text rather than the icon.
            if let Some(body_len) = emoticon_icon(&chars, offset, after) {
                out.push_str("\\(");
                let body_end = offset + 1 + body_len;
                for &body_char in chars.get(offset + 1..body_end).unwrap_or_default() {
                    out.push(body_char);
                }
                out.push(')');
                offset = body_end + 1;
                continue;
            }
            // The char before is read as content when it is itself escaped into literal text as the
            // body of a preceding emoticon, so it does not make this parenthesis markup-significant.
            let effective_before = if neutralized_before(&chars, offset) {
                Some('a')
            } else {
                before
            };
            if needs_paren_escape_in(&chars, offset, effective_before, next, after) {
                out.push('\\');
            }
            out.push('(');
            offset += 1;
            continue;
        }
        if (ch == ':' || ch == ';') && opens_emoticon(ch, next, at(offset + 2), before) {
            // The leading punctuation of a text emoticon is escaped; its second character, even a
            // parenthesis, is left bare since the emoticon as a whole is now neutralized.
            out.push('\\');
            out.push(ch);
            if let Some(&body) = chars.get(offset + 1) {
                out.push(body);
            }
            offset += 2;
            continue;
        }
        if is_marker(ch) {
            // A `:`/`;`/`(` that opens a text emoticon is escaped into literal text, so for the run
            // tests it reads as ordinary content rather than as a markup boundary. Replace such a
            // neighbor with a content sentinel before the test.
            let effective_before = if neutralized_before(&chars, offset) {
                Some('a')
            } else {
                before
            };
            let effective_next = if neutralized_after(&chars, offset, after) {
                Some('a')
            } else {
                next
            };
            if needs_escape(ch, effective_before, effective_next) {
                out.push('\\');
            }
            out.push(ch);
            offset += 1;
            continue;
        }
        if needs_escape(ch, before, next) {
            out.push('\\');
        }
        out.push(ch);
        offset += 1;
    }
    out
}

/// Whether the character just after the marker at `marker` is the leading punctuation of a text
/// emoticon (or the open parenthesis of an icon), which the writer escapes into literal text — so it
/// reads as ordinary content beside the marker rather than as a markup boundary.
fn neutralized_after(chars: &[char], marker: usize, after: Option<char>) -> bool {
    let pos = marker + 1;
    match chars.get(pos) {
        Some(&(':' | ';')) => opens_emoticon_at(chars, pos),
        Some(&'(') => emoticon_icon(chars, pos, after).is_some(),
        _ => false,
    }
}

/// Whether the character just before the marker at `marker` is part of a text emoticon the writer
/// escapes into literal text: the leading `:`/`;`, the parenthesis body that follows one, or the open
/// parenthesis of an icon. Such a neighbor reads as ordinary content beside the marker.
fn neutralized_before(chars: &[char], marker: usize) -> bool {
    let Some(pos) = marker.checked_sub(1) else {
        return false;
    };
    match chars.get(pos) {
        Some(&(':' | ';')) => opens_emoticon_at(chars, pos),
        Some(&'(') => {
            // The parenthesis body of a `:(`/`;(` emoticon, or the open parenthesis of an icon.
            let opens_smiley = pos
                .checked_sub(1)
                .is_some_and(|lead| smiley_follows(chars, lead));
            opens_smiley || emoticon_icon(chars, pos, None).is_some()
        }
        _ => false,
    }
}

/// Whether a text emoticon begins at `pos`, reading the lead, body, and the characters that bound it
/// from the surrounding text.
fn opens_emoticon_at(chars: &[char], pos: usize) -> bool {
    let Some(&lead) = chars.get(pos) else {
        return false;
    };
    let body = chars.get(pos + 1).copied();
    let trailing = chars.get(pos + 2).copied();
    let before = pos.checked_sub(1).and_then(|i| chars.get(i).copied());
    opens_emoticon(lead, body, trailing, before)
}

/// The single- and multi-character bodies Jira renders as an icon when wrapped in parentheses.
const EMOTICON_ICONS: &[&str] = &[
    "x", "y", "i", "n", "/", "!", "?", "on", "off", "*", "*r", "*g", "*b", "*y", "+", "-", "flag",
    "flagoff",
];

/// If an open parenthesis at `paren` begins a recognized icon — one of [`EMOTICON_ICONS`] followed by
/// a closing parenthesis and a word boundary — return the body's length in characters. The trailing
/// boundary keeps a word-continued sequence like `foo(x)bar` from being read as an icon.
fn emoticon_icon(chars: &[char], paren: usize, after: Option<char>) -> Option<usize> {
    let body_start = paren + 1;
    for icon in EMOTICON_ICONS {
        let body_len = icon.chars().count();
        let close = body_start + body_len;
        let Some(body) = chars.get(body_start..close) else {
            continue;
        };
        if body.iter().copied().eq(icon.chars()) && chars.get(close) == Some(&')') {
            let trailing = chars.get(close + 1).copied().or(after);
            if emoticon_boundary(trailing) {
                return Some(body_len);
            }
        }
    }
    None
}

/// The characters that make an open parenthesis read as the start of an emoticon or macro to Jira: the
/// span markers and other markup punctuation. With this character (or a markup boundary) on either
/// side, a bare `(` would begin an icon, so it is escaped.
const PAREN_SIGNIFICANT: &[char] = &[
    '!', '&', '(', '*', '+', '-', ':', ';', '?', '[', '\\', ']', '^', '_', '{', '|', '}', '~',
];

/// Whether an open parenthesis must be escaped: it is, unless both neighbors are ordinary content. A
/// markup boundary (string edge or a plain space) or one of the markup-significant punctuation
/// characters on either side makes Jira read the `(` as opening an emoticon or macro.
fn needs_paren_escape(prev: Option<char>, next: Option<char>) -> bool {
    paren_boundary(prev) || paren_boundary(next)
}

/// The escape decision for an open parenthesis with access to the surrounding text. A `(` set off by
/// a plain space on each side is a lone parenthetical in running prose, not the start of an icon, so
/// it is left bare — unless it rests against the trailing edge of the document, where it would still
/// open a macro. `after` is the character the surrounding inline stream contributes once this string
/// is exhausted, so a following space inline keeps the parenthesis in running prose even when nothing
/// but spaces remain in this string. Every other position falls back to the neighbor-only rule.
fn needs_paren_escape_in(
    chars: &[char],
    paren: usize,
    before: Option<char>,
    next: Option<char>,
    after: Option<char>,
) -> bool {
    if before == Some(' ') && next == Some(' ') {
        // A space-flanked parenthetical opens a macro only at the trailing edge: bare while real
        // content (in this string or the inline stream beyond it) still follows the trailing spaces.
        let has_in_string_content = chars
            .get(paren + 1..)
            .unwrap_or_default()
            .iter()
            .any(|&candidate| candidate != ' ');
        if has_in_string_content {
            return false;
        }
        // This string holds only trailing spaces past the parenthesis; the inline stream beyond it
        // decides. A following space inline keeps it in prose; the document's end escapes it.
        return after != Some(' ');
    }
    // A parenthesis whose only markup-significance is the smiley punctuation right after it — within
    // ordinary content — is left bare; the following emoticon's own escape already neutralizes it.
    if !paren_boundary(before) && smiley_follows(chars, paren + 1) {
        return false;
    }
    needs_paren_escape(before, next)
}

fn paren_boundary(neighbor: Option<char>) -> bool {
    match neighbor {
        None | Some(' ') => true,
        Some(ch) => PAREN_SIGNIFICANT.contains(&ch),
    }
}

/// Whether a backslash should be emitted literally rather than as the `&bsol;` entity. Jira keeps a
/// backslash verbatim only when both of its neighbors share the same plain category — both ordinary
/// content, or both spaces. Against a string edge, another backslash, or markup punctuation it would
/// be consumed as an escape, so it is rendered as the entity there.
fn backslash_is_literal(prev: Option<char>, next: Option<char>) -> bool {
    let category = backslash_category(prev);
    matches!(
        category,
        BackslashCategory::Content | BackslashCategory::Space
    ) && category == backslash_category(next)
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum BackslashCategory {
    Content,
    Space,
    Other,
}

fn backslash_category(neighbor: Option<char>) -> BackslashCategory {
    match neighbor {
        Some(' ') => BackslashCategory::Space,
        Some(')') => BackslashCategory::Content,
        Some('\\') | None => BackslashCategory::Other,
        Some(ch) if NEUTRAL_PUNCT.contains(&ch) => BackslashCategory::Other,
        Some(_) => BackslashCategory::Content,
    }
}

/// The body of a text emoticon recognized by Jira, paired with the leading punctuation it follows.
/// The `:` family (`:)`, `:(`, `:P`, `:D`) and the `;)` wink are recognized as long as a markup
/// boundary follows them; the `;P` and `;D` winks additionally require a boundary before. A
/// word-character or `)` adjacent where a boundary is required keeps Jira from reading the emoticon.
fn opens_emoticon(
    lead: char,
    body: Option<char>,
    trailing: Option<char>,
    before: Option<char>,
) -> bool {
    let Some(body) = body else { return false };
    match (lead, body) {
        (':', ')' | '(' | 'P' | 'D') | (';', ')') => emoticon_boundary(trailing),
        (';', 'P' | 'D') => emoticon_boundary_strict(before) && emoticon_boundary_strict(trailing),
        _ => false,
    }
}

/// Whether a colon/semicolon text emoticon of the boundary-after family (`:)`, `:(`, `:P`, `:D`,
/// `;)`) begins at `index` and reads as an emoticon. A parenthesis sitting just before such an
/// emoticon carries no markup meaning of its own.
fn smiley_follows(chars: &[char], index: usize) -> bool {
    let Some(&lead) = chars.get(index) else {
        return false;
    };
    if lead != ':' && lead != ';' {
        return false;
    }
    let body = chars.get(index + 1).copied();
    let trailing = chars.get(index + 2).copied();
    matches!(
        (lead, body),
        (':', Some(')' | '(' | 'P' | 'D')) | (';', Some(')')),
    ) && emoticon_boundary(trailing)
}

/// Whether a position ends a text emoticon: a non-word character or the string edge. A closing
/// parenthesis counts as a boundary here, so `:))` still reads its leading `:)` as an emoticon.
fn emoticon_boundary(neighbor: Option<char>) -> bool {
    !neighbor.is_some_and(char::is_alphanumeric)
}

/// A stricter emoticon boundary used for the `;P` and `;D` winks: only the string edge, a plain
/// space, or one of the markup-significant punctuation characters bounds them. A word character, a
/// closing parenthesis, or ordinary punctuation is word-like here and suppresses the wink.
fn emoticon_boundary_strict(neighbor: Option<char>) -> bool {
    match neighbor {
        None | Some(' ' | '\\') => true,
        Some(ch) => NEUTRAL_PUNCT.contains(&ch),
    }
}

fn needs_escape(ch: char, prev: Option<char>, next: Option<char>) -> bool {
    match ch {
        '{' | '}' => true,
        '?' => next == Some('?'),
        // A marker is markup-significant at the string edge, at a content/non-content transition,
        // or whenever it abuts another markup-significant character — that last case escapes every
        // member of a run of two or more adjoining markers, not only the run's outer edges.
        _ if is_marker(ch) => {
            prev.is_none()
                || next.is_none()
                || is_content(prev) != is_content(next)
                || is_neutral(prev)
                || is_neutral(next)
        }
        _ => false,
    }
}

/// Whether a neighbor is one of the markup-significant punctuation characters: present, and not
/// content or whitespace. A marker resting against such a neighbor sits inside a marker run and is
/// escaped even when no content/non-content transition crosses it. A backslash counts too: a
/// backslash abutting a marker always renders as the `&bsol;` entity, whose leading `&` is itself a
/// boundary.
fn is_neutral(neighbor: Option<char>) -> bool {
    neighbor.is_some_and(|ch| ch == '\\' || NEUTRAL_PUNCT.contains(&ch))
}

/// Whether a character is a span marker subject to edge/transition escaping.
fn is_marker(ch: char) -> bool {
    SPAN_MARKERS.contains(&ch)
}

/// Whether a neighbor counts as content for the transition test: not whitespace, not a backslash,
/// and not one of the Jira-significant punctuation characters that delimit markup. A backslash
/// abutting a marker renders as the `&bsol;` entity, so it is a boundary, not content.
fn is_content(neighbor: Option<char>) -> bool {
    match neighbor {
        None => false,
        Some(ch) => ch != '\\' && !ch.is_whitespace() && !NEUTRAL_PUNCT.contains(&ch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::Document;

    fn render(blocks: Vec<Block>) -> String {
        let document = Document {
            blocks,
            ..Document::default()
        };
        JiraWriter
            .write(&document, &WriterOptions::default())
            .expect("jira writer is infallible over these inputs")
    }

    fn para(inlines: Vec<Inline>) -> Block {
        Block::Para(inlines)
    }

    fn math(kind: MathType, tex: &str) -> Inline {
        Inline::Math(kind, tex.to_owned())
    }

    fn inline(tex: &str) -> String {
        render(vec![para(vec![math(MathType::InlineMath, tex)])])
    }

    fn display(tex: &str) -> String {
        render(vec![para(vec![math(MathType::DisplayMath, tex)])])
    }

    fn str_inline(text: &str) -> Inline {
        Inline::Str(text.to_owned())
    }

    #[test]
    fn superscript_lowers_to_caret_markup() {
        // A variable is italicised (`_a_`) and a superscript becomes a `^…^` run.
        assert_eq!(inline("a^2"), "_a_^2^");
    }

    #[test]
    fn subscript_lowers_to_tilde_markup() {
        assert_eq!(inline("a_n"), "_a_~_n_~");
    }

    #[test]
    fn binary_operator_and_relation_carry_typographic_spacing() {
        // The spacing emitted around an operator (`+`) and a relation (`=`) is a word-like space, so
        // the variables that follow it are guarded with braced emphasis markers.
        assert_eq!(
            inline("a^2 + b^2 = c^2"),
            "_a_^2^\u{2005}+\u{2005}{_}b{_}^2^\u{2004}=\u{2004}{_}c{_}^2^"
        );
    }

    #[test]
    fn greek_letters_render_as_unicode() {
        assert_eq!(inline("\\alpha+\\beta"), "_α_\u{2005}+\u{2005}{_}β{_}");
    }

    #[test]
    fn blackboard_alphabet_renders_as_unicode_symbol() {
        assert_eq!(inline("\\mathbb{R}"), "ℝ");
    }

    #[test]
    fn bold_alphabet_lowers_to_strong_markup() {
        assert_eq!(inline("\\mathbf{v}"), "*v*");
    }

    #[test]
    fn accent_renders_as_combining_mark_inside_emphasis() {
        assert_eq!(inline("\\bar{x}"), "_x\u{304}_");
    }

    #[test]
    fn integral_lowers_to_symbol_with_scripts_and_thin_space() {
        // The integral sign carries its limits as sub/superscript markup and the thin space (`\,`)
        // is a word-like space, so the differential's variable is brace-guarded.
        assert_eq!(
            inline("\\int_0^1 x \\, dx"),
            "∫{~}0{~}^1^_x_\u{2006}{_}d{_}_x_"
        );
    }

    #[test]
    fn inline_math_threads_surrounding_text_into_edge_guards() {
        // Flanked by word text, the leading and trailing emphasis runs are brace-guarded so Jira
        // still parses them as markup.
        let out = render(vec![para(vec![
            str_inline("x"),
            math(MathType::InlineMath, "a^2"),
            str_inline("y"),
        ])]);
        assert_eq!(out, "x{_}a{_}{^}2{^}y");
    }

    #[test]
    fn display_math_stands_on_its_own_line() {
        // Display math is framed by a newline on each side; the document writer's trailing newline
        // closes the block.
        assert_eq!(display("a^2"), "\n_a_^2^\n");
    }

    #[test]
    fn display_math_breaks_surrounding_inline_content() {
        let out = render(vec![para(vec![
            str_inline("before"),
            Inline::Space,
            math(MathType::DisplayMath, "a^2"),
            Inline::Space,
            str_inline("after"),
        ])]);
        assert_eq!(out, "before \n_a_^2^\n after");
    }

    #[test]
    fn inline_fallback_wraps_source_in_single_dollars() {
        // A construct with no single-line form is emitted verbatim in inline delimiters. Its braces
        // are escaped by the text path; the backslash sits between ordinary content and stays literal.
        assert_eq!(inline("\\frac{1}{2}"), "$\\frac\\{1\\}\\{2\\}$");
    }

    #[test]
    fn display_fallback_wraps_source_in_double_dollars_on_its_own_line() {
        assert_eq!(display("\\sqrt{x}"), "\n$$\\sqrt\\{x\\}$$\n");
    }

    #[test]
    fn empty_inline_math_contributes_nothing() {
        let out = render(vec![para(vec![
            str_inline("x"),
            math(MathType::InlineMath, "  "),
            str_inline("y"),
        ])]);
        assert_eq!(out, "xy");
    }

    #[test]
    fn empty_display_math_emits_an_empty_framed_line() {
        let out = render(vec![para(vec![
            str_inline("x"),
            math(MathType::DisplayMath, ""),
            str_inline("y"),
        ])]);
        assert_eq!(out, "x\n\ny");
    }

    #[test]
    fn preceding_emphasis_guards_against_convertible_math_starting_with_a_letter() {
        // Math that opens with an alphanumeric symbol (`ℝ`) forces the emphasis before it to brace
        // its closing marker.
        let out = render(vec![para(vec![
            Inline::Emph(vec![str_inline("word")]),
            math(MathType::InlineMath, "\\mathbb{R}"),
        ])]);
        assert_eq!(out, "{_}word{_}ℝ");
    }

    #[test]
    fn only_plain_space_and_newline_are_word_boundaries() {
        assert!(is_word_boundary(' '));
        assert!(is_word_boundary('\n'));
        // Typographic and non-breaking spaces are word-like, so a marker resting on one is braced.
        assert!(!is_word_boundary('\u{00a0}'));
        assert!(!is_word_boundary('\u{2004}'));
        assert!(!is_word_boundary('\u{2005}'));
        assert!(!is_word_boundary('\u{2006}'));
        assert!(!is_word_boundary('\t'));
    }

    #[test]
    fn deep_nesting_falls_back_without_panicking() {
        // Pathological brace nesting exceeds the conversion depth limit and must degrade to the
        // verbatim fallback rather than overflow the stack.
        let tex = format!("{}x{}", "{".repeat(5000), "}".repeat(5000));
        let out = inline(&tex);
        assert!(out.starts_with('$') && out.ends_with('$'));
    }

    /// Render a single plain-text token as the writer would, with no surrounding inlines.
    fn text(value: &str) -> String {
        render(vec![para(vec![str_inline(value)])])
    }

    #[test]
    fn lone_backslash_between_words_stays_literal() {
        // A backslash flanked by ordinary content is kept verbatim; Jira does not consume it there.
        assert_eq!(text("a\\b"), "a\\b");
        assert_eq!(text("path\\to\\file"), "path\\to\\file");
    }

    #[test]
    fn backslash_at_an_edge_becomes_the_entity() {
        // Against the string edge a backslash would be read as an escape, so it is emitted as the
        // `&bsol;` entity instead.
        assert_eq!(text("\\start"), "&bsol;start");
        assert_eq!(text("end\\"), "end&bsol;");
    }

    #[test]
    fn backslash_between_spaces_stays_literal() {
        // Both neighbors are plain spaces, the same category, so the backslash is kept literal.
        assert_eq!(text("a \\ b"), "a \\ b");
    }

    #[test]
    fn consecutive_backslashes_become_entities() {
        // Each backslash neighbors another backslash, a differing category, so both are entities.
        assert_eq!(text("x\\\\y"), "x&bsol;&bsol;y");
    }

    #[test]
    fn backslash_before_a_marker_becomes_the_entity() {
        // A marker is non-content, a category that differs from the word before it, so the backslash
        // between them is rendered as the entity. That entity's leading `&` is itself a boundary, so
        // the marker right after it is markup-significant and is escaped.
        assert_eq!(text("a\\*b"), "a&bsol;\\*b");
        // At the string edge the backslash is likewise the entity, and the marker after it escaped.
        assert_eq!(text("\\*end"), "&bsol;\\*end");
    }

    #[test]
    fn open_paren_inside_a_word_stays_bare() {
        // Flanked by ordinary content on both sides, an open parenthesis carries no markup meaning.
        assert_eq!(text("a(b)c"), "a(b)c");
        assert_eq!(text("a (b) c"), "a \\(b) c");
    }

    #[test]
    fn open_paren_before_an_emoticon_body_is_escaped() {
        // `(x)`, `(y)`, and the macro-like bodies would render as an icon, so the opening `(` is
        // escaped while its matching `)` is left alone.
        assert_eq!(text("(x)"), "\\(x)");
        assert_eq!(text("(y)"), "\\(y)");
        assert_eq!(text("f(x)"), "f\\(x)");
        assert_eq!(text("(!)"), "\\(!)");
    }

    #[test]
    fn space_flanked_open_paren_escapes_only_at_the_trailing_edge() {
        // A parenthesis with a plain space on each side is a lone parenthetical in prose and stays
        // bare while content follows; with only spaces after it, it rests on the trailing edge and
        // is escaped.
        assert_eq!(text("a ( b"), "a ( b");
        assert_eq!(text("a ( ( b"), "a ( ( b");
        assert_eq!(text("a ( )"), "a ( )");
        assert_eq!(text("a ( "), "a \\(");
    }

    #[test]
    fn space_flanked_open_paren_consults_the_inline_stream_for_the_trailing_edge() {
        // A bare `(` token flanked by `Space` inlines is a lone parenthetical: the trailing-edge test
        // must consult the surrounding stream, not just this token's own (empty) tail. While a space
        // inline follows it stays in prose and bare; only the document's end escapes it.
        let prose = render(vec![para(vec![
            str_inline("a"),
            Inline::Space,
            str_inline("("),
            Inline::Space,
            str_inline("b"),
        ])]);
        assert_eq!(prose, "a ( b");
        let trailing_space = render(vec![para(vec![
            str_inline("a"),
            Inline::Space,
            str_inline("("),
            Inline::Space,
        ])]);
        assert_eq!(trailing_space, "a ( ");
        let document_edge = render(vec![para(vec![
            str_inline("a"),
            Inline::Space,
            str_inline("("),
        ])]);
        assert_eq!(document_edge, "a \\(");
        // A following markup inline (no space between) makes the `(` rest against a boundary, so it
        // is escaped by the neighbor rule.
        let before_markup = render(vec![para(vec![
            str_inline("a"),
            Inline::Space,
            str_inline("("),
            Inline::Emph(vec![str_inline("b")]),
        ])]);
        assert_eq!(before_markup, "a \\(_b_");
    }

    #[test]
    fn open_paren_at_an_edge_is_escaped() {
        assert_eq!(text("(start"), "\\(start");
        assert_eq!(text("("), "\\(");
    }

    #[test]
    fn open_paren_does_not_over_escape_a_following_marker() {
        // The `!` after the escaped `(` is content here and must not itself be escaped.
        assert_eq!(text("z(!)"), "z\\(!)");
    }

    #[test]
    fn text_emoticons_escape_their_leading_punctuation() {
        // A recognized text emoticon at a boundary has its leading `:`/`;` escaped so Jira renders
        // the literal characters instead of an icon.
        assert_eq!(text(":)"), "\\:)");
        assert_eq!(text(":("), "\\:(");
        assert_eq!(text(":P"), "\\:P");
        assert_eq!(text(":D"), "\\:D");
        assert_eq!(text(";)"), "\\;)");
        assert_eq!(text(";P"), "\\;P");
        assert_eq!(text(";D"), "\\;D");
    }

    #[test]
    fn colon_emoticon_escapes_when_followed_by_a_word() {
        // The `:`-family reads as an emoticon whenever a markup boundary follows, even mid-word.
        assert_eq!(text("a:)"), "a\\:)");
        // A following word character keeps the sequence from being an emoticon.
        assert_eq!(text(":)x"), ":)x");
    }

    #[test]
    fn wink_letter_emoticon_needs_a_boundary_on_both_sides() {
        // `;P` and `;D` only read as emoticons with a boundary before and after; a neighboring word
        // character or `)` suppresses the escape.
        assert_eq!(text("a;P"), "a;P");
        assert_eq!(text(";P)"), ";P)");
        assert_eq!(text(" ;P "), " \\;P");
    }

    #[test]
    fn colon_open_paren_emoticon_escapes_the_colon_only() {
        // `:(` at a boundary is a sad-face emoticon: the `:` is escaped and the parenthesis is bare.
        assert_eq!(text(":("), "\\:(");
        // Followed by a word the sequence is not an emoticon, so the `:` stays bare and the `(`,
        // resting against the significant `:`, is escaped by its own rule.
        assert_eq!(text(":(x"), ":\\(x");
    }

    #[test]
    fn slash_adjacent_open_paren_stays_bare() {
        // A slash is ordinary content for the parenthesis rule, so a `(` glued between content and a
        // slash is not escaped.
        assert_eq!(text("a(/"), "a(/");
        assert_eq!(text("/(x"), "/(x");
    }

    #[test]
    fn run_of_markers_escapes_every_member() {
        // Two or more adjoining markers each open or close markup, so all of them are escaped — not
        // only the run's outer edges.
        assert_eq!(text("a -- b"), "a \\-\\- b");
        assert_eq!(text("a---b"), "a\\-\\-\\-b");
        assert_eq!(text("__x"), "\\_\\_x");
        assert_eq!(text("++plus++"), "\\+\\+plus\\+\\+");
        assert_eq!(text("a !! b"), "a \\!\\! b");
    }

    #[test]
    fn interior_marker_of_a_long_run_is_escaped() {
        // The middle marker of a three-marker run has a marker on each side, so the run rule escapes
        // it even though no content/non-content transition crosses it.
        assert_eq!(text("*_*"), "\\*\\_\\*");
        assert_eq!(text("-+-"), "\\-\\+\\-");
        assert_eq!(text("a*-+b"), "a\\*\\-\\+b");
    }

    #[test]
    fn marker_against_punctuation_is_escaped() {
        // The colon, semicolon, and brace are markup-significant punctuation, so a marker resting
        // against one is escaped; a closing parenthesis is ordinary content and leaves it bare.
        assert_eq!(text("a:*:b"), "a:\\*:b");
        assert_eq!(text("a)*)b"), "a)*)b");
    }

    #[test]
    fn marker_against_an_entity_backslash_is_escaped() {
        // A backslash abutting a marker always renders as the `&bsol;` entity, whose `&` is a markup
        // boundary, so the marker is escaped on that side.
        assert_eq!(text("a*\\"), "a\\*&bsol;");
        assert_eq!(text("x*\\y"), "x\\*&bsol;y");
        assert_eq!(text("\\*a"), "&bsol;\\*a");
    }

    #[test]
    fn marker_against_a_neutralized_emoticon_stays_bare() {
        // The leading `:`/`;` of a text emoticon (and the open parenthesis of an icon) is escaped
        // into literal text, so a marker resting against it reads as content-flanked and stays bare.
        assert_eq!(text("a_:)"), "a_\\:)");
        assert_eq!(text(":(*<>:"), "\\:(*<>:");
        assert_eq!(text("a_(x)"), "a_\\(x)");
        // When the colon does not open an emoticon it is markup-significant and the marker is escaped.
        assert_eq!(text("a_:x"), "a\\_:x");
    }

    #[test]
    fn lone_marker_keeps_the_boundary_rule() {
        // A single marker surrounded by ordinary content carries no markup meaning and stays bare.
        assert_eq!(text("a*b"), "a*b");
        assert_eq!(text("a - b"), "a - b");
        assert_eq!(text("a_b"), "a_b");
    }

    /// Render a single inline-code token as the writer would, with no surrounding inlines.
    fn code(value: &str) -> String {
        render(vec![para(vec![Inline::Code(
            Attr::default(),
            value.to_owned(),
        )])])
    }

    #[test]
    fn inline_code_escapes_its_boundary_markers() {
        // Inside a monospaced span the leading and trailing markup characters are still escaped, so
        // Jira keeps the literal text rather than reading the run as markup.
        assert_eq!(code("(x)"), "{{\\(x)}}");
        assert_eq!(code("[y]"), "{{\\[y\\]}}");
        assert_eq!(code("*x*"), "{{\\*x\\*}}");
        assert_eq!(code("|x|"), "{{\\|x\\|}}");
    }

    #[test]
    fn inline_code_leaves_interior_content_markers_bare() {
        // A marker resting between two content characters is ordinary text inside code, so it stays
        // bare; an abutting run is escaped throughout.
        assert_eq!(code("a*b"), "{{a*b}}");
        assert_eq!(code("a|b"), "{{a|b}}");
        assert_eq!(code("a--b"), "{{a\\-\\-b}}");
    }

    #[test]
    fn inline_code_handles_backslashes_like_running_text() {
        // A backslash flanked by content stays literal; one against the edge becomes the `&bsol;`
        // entity, exactly as in ordinary text.
        assert_eq!(code("a\\b"), "{{a\\b}}");
        assert_eq!(code("a\\"), "{{a&bsol;}}");
    }
}
