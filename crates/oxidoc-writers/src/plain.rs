//! Plain-text writer: renders the document model to unformatted text.
//!
//! Output uses a fill column of 72, strips inline markup (emphasis, links, and inline code render as
//! their textual content), and conveys block structure through indentation alone. It carries no
//! trailing newline; the caller appends one. This format has no public specification.

use oxidoc_ast::{Block, Document, Format, Inline, ListAttributes};
use oxidoc_core::{Result, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, NotesHost, Piece, append_notes, fill, fill_offset, indent_block, is_loose,
    item_separator, join_loose, offset_as_i32, ordered_marker, quote_marks,
};

/// Renders a document to plain text.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainWriter;

impl Writer for PlainWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let mut state = State::default();
        let body = state.blocks_to_string(&document.blocks, FILL_COLUMN);
        Ok(append_notes(body, &state.footnotes))
    }
}

/// Carries the footnote bodies accumulated while rendering, so notes can be collected inline and
/// emitted as a section at the end of the document.
#[derive(Debug, Default)]
struct State {
    footnotes: Vec<String>,
}

impl State {
    /// Render a block sequence with a blank line between blocks, dropping those that produce no
    /// output. This is the default layout (document body, block quotes, divs, figures, loose list
    /// items, loose definitions). See [`join_loose`] for the [`Block::Plain`] spacing quirk.
    fn blocks_to_string(&mut self, blocks: &[Block], width: usize) -> String {
        let rendered = blocks
            .iter()
            .map(|block| (matches!(block, Block::Plain(_)), self.block(block, width)))
            .collect();
        join_loose(rendered)
    }

    /// Render a block sequence with a single newline between blocks: the compact layout used inside a
    /// tight list's items and tight definitions.
    fn blocks_tight(&mut self, blocks: &[Block], width: usize) -> String {
        let parts: Vec<String> = blocks
            .iter()
            .map(|block| self.block(block, width))
            .filter(|rendered| !rendered.is_empty())
            .collect();
        parts.join("\n")
    }

    /// Render a block sequence at the given layout density.
    fn blocks_at(&mut self, blocks: &[Block], width: usize, loose: bool) -> String {
        if loose {
            self.blocks_to_string(blocks, width)
        } else {
            self.blocks_tight(blocks, width)
        }
    }

    fn block(&mut self, block: &Block, width: usize) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                let pieces = self.pieces(inlines);
                fill(&pieces, width)
            }
            Block::Header(_, _, inlines) => {
                let pieces = self.pieces(inlines);
                header_text(&pieces)
            }
            Block::CodeBlock(_, text) => {
                indent_block(text.strip_suffix('\n').unwrap_or(text), "    ", "    ")
            }
            Block::RawBlock(format, text) => {
                if is_plain_format(format) {
                    text.strip_suffix('\n').unwrap_or(text).to_owned()
                } else {
                    String::new()
                }
            }
            Block::BlockQuote(blocks) => {
                let body = self.blocks_to_string(blocks, width.saturating_sub(2));
                indent_block(&body, "  ", "  ")
            }
            Block::BulletList(items) => self.bullet_list(items, width),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::DefinitionList(items) => self.definition_list(items, width),
            Block::HorizontalRule => "-".repeat(FILL_COLUMN),
            Block::Table(_) => todo!("plain writer: render tables"),
            Block::Figure(_, _, blocks) | Block::Div(_, blocks) => {
                self.blocks_to_string(blocks, width)
            }
            Block::LineBlock(lines) => self.line_block(lines),
        }
    }

    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| {
                let pieces = self.pieces(line);
                pieces_to_string(&pieces)
            })
            .collect();
        rendered.join("\n")
    }

    fn bullet_list(&mut self, items: &[Vec<Block>], width: usize) -> String {
        let loose = is_loose(items);
        let body_width = width.saturating_sub(2);
        let rendered: Vec<String> = items
            .iter()
            .map(|item| {
                let body = self.blocks_at(item, body_width, loose);
                indent_block(&body, "- ", "  ")
            })
            .collect();
        rendered.join(item_separator(loose))
    }

    fn ordered_list(
        &mut self,
        attrs: &ListAttributes,
        items: &[Vec<Block>],
        width: usize,
    ) -> String {
        let loose = is_loose(items);
        let rendered: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(offset, item)| {
                let number = attrs.start.saturating_add(offset_as_i32(offset));
                let marker = ordered_marker(number, &attrs.style, &attrs.delim);
                let field = (marker.chars().count() + 1).max(4);
                let body = self.blocks_at(item, width.saturating_sub(field), loose);
                let first = format!("{marker:<field$}");
                let rest = " ".repeat(field);
                indent_block(&body, &first, &rest)
            })
            .collect();
        rendered.join(item_separator(loose))
    }

    fn definition_list(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        let groups: Vec<String> = items
            .iter()
            .map(|(term, definitions)| {
                let term_pieces = self.pieces(term);
                let mut group = fill(&term_pieces, width);
                for definition in definitions {
                    let loose = is_loose_definition(definition);
                    let body = self.blocks_at(definition, width.saturating_sub(2), loose);
                    let indented = indent_block(&body, "  ", "  ");
                    group.push_str(if loose { "\n\n" } else { "\n" });
                    group.push_str(&indented);
                }
                group
            })
            .collect();
        groups.join("\n\n")
    }

    fn pieces(&mut self, inlines: &[Inline]) -> Vec<Piece> {
        let mut out = Vec::new();
        self.extend_pieces(inlines, &mut out);
        out
    }

    /// Append the inline sequence's pieces to `out`. A `Str` ending in `!` immediately before a link
    /// or span is escaped so it is not re-read as the image marker.
    fn extend_pieces(&mut self, inlines: &[Inline], out: &mut Vec<Piece>) {
        for (position, inline) in inlines.iter().enumerate() {
            if let Inline::Str(text) = inline
                && let Some(prefix) = text.strip_suffix('!')
                && matches!(
                    inlines.get(position + 1),
                    Some(Inline::Link(..) | Inline::Span(..))
                )
            {
                out.push(Piece::Text(format!("{prefix}\\!")));
                continue;
            }
            self.inline(inline, out);
        }
    }

    fn inline(&mut self, inline: &Inline, out: &mut Vec<Piece>) {
        match inline {
            Inline::Str(text) | Inline::Code(_, text) => out.push(Piece::Text(text.clone())),
            Inline::Emph(inlines)
            | Inline::Strong(inlines)
            | Inline::Underline(inlines)
            | Inline::Cite(_, inlines)
            | Inline::Link(_, inlines, _)
            | Inline::Span(_, inlines) => self.extend_pieces(inlines, out),
            Inline::Strikeout(inlines) => {
                out.push(Piece::Text("~~".to_owned()));
                self.extend_pieces(inlines, out);
                out.push(Piece::Text("~~".to_owned()));
            }
            Inline::Superscript(inlines) => {
                let inner = pieces_to_string(&self.pieces(inlines));
                out.push(Piece::Text(to_superscript(&inner)));
            }
            Inline::Subscript(inlines) => {
                let inner = pieces_to_string(&self.pieces(inlines));
                out.push(Piece::Text(to_subscript(
                    &inner,
                    forces_superscript(inlines),
                )));
            }
            Inline::SmallCaps(inlines) => {
                let start = out.len();
                self.extend_pieces(inlines, out);
                uppercase_pieces(out, start);
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_marks(kind);
                out.push(Piece::Text(open.to_string()));
                self.extend_pieces(inlines, out);
                out.push(Piece::Text(close.to_string()));
            }
            Inline::Space | Inline::SoftBreak => out.push(Piece::Space),
            Inline::LineBreak => out.push(Piece::Hard),
            Inline::Math(_, _) => todo!("plain writer: render math"),
            Inline::RawInline(format, text) => {
                if is_plain_format(format) {
                    out.push(Piece::Text(text.clone()));
                }
            }
            Inline::Image(_, inlines, _) => {
                out.push(Piece::Text("[".to_owned()));
                self.extend_pieces(inlines, out);
                out.push(Piece::Text("]".to_owned()));
            }
            Inline::Note(blocks) => {
                let marker = self.record_note(blocks);
                out.push(Piece::Text(marker));
            }
        }
    }
}

impl NotesHost for State {
    fn notes(&mut self) -> &mut Vec<String> {
        &mut self.footnotes
    }

    fn render_block(&mut self, block: &Block, width: usize) -> String {
        self.block(block, width)
    }

    fn render_offset_paragraph(
        &mut self,
        inlines: &[Inline],
        width: usize,
        initial: usize,
    ) -> String {
        let pieces = self.pieces(inlines);
        fill_offset(&pieces, width, initial)
    }
}

/// Whether a raw node targets this writer and should pass its content through verbatim. Raw content
/// whose format matches `plain` (case-insensitively) is emitted; everything else is dropped.
fn is_plain_format(format: &Format) -> bool {
    format.0.eq_ignore_ascii_case("plain")
}

/// Whether a single definition lays out with blank lines. A definition is rendered
/// compactly only when its first block is a [`Block::Plain`]; an empty definition or one that opens
/// with block-level content (a paragraph, list, quote, code block, …) gets blank-line spacing.
fn is_loose_definition(blocks: &[Block]) -> bool {
    !matches!(blocks.first(), Some(Block::Plain(_)))
}

/// Uppercase the text of every piece from `start` onward, in place (small-caps rendering).
fn uppercase_pieces(pieces: &mut [Piece], start: usize) {
    for piece in pieces.iter_mut().skip(start) {
        if let Piece::Text(text) = piece {
            *text = text.to_uppercase();
        }
    }
}

/// Flatten inline pieces to a single string without line filling: breakable spaces become one
/// space, while forced breaks become `hard`. Used where content is not wrapped
/// (line-block lines and the inner text of sub/superscripts use a newline; see [`header_text`]).
fn join_pieces(pieces: &[Piece], hard: char) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Space => out.push(' '),
            Piece::Hard => out.push(hard),
        }
    }
    out
}

fn pieces_to_string(pieces: &[Piece]) -> String {
    join_pieces(pieces, '\n')
}

/// Flatten a header's inline pieces to a single line: a forced break renders as a space, keeping a
/// header on one line.
fn header_text(pieces: &[Piece]) -> String {
    join_pieces(pieces, ' ')
}

#[derive(Debug, Clone, Copy)]
enum Script {
    Super,
    Sub,
}

/// Render text as superscript: when every character has a Unicode superscript equivalent (digits and
/// a small set of symbols, with spaces preserved) emit the mapped characters; otherwise fall back to
/// the parenthesized form (`^(…)`).
fn to_superscript(text: &str) -> String {
    let mapped: Option<String> = text
        .chars()
        .map(|ch| script_char(ch, Script::Super))
        .collect();
    mapped.unwrap_or_else(|| format!("^({text})"))
}

/// Render subscript text: when the content carries any
/// formatted inline (anything other than plain text or spaces) and is otherwise convertible, the
/// characters are mapped to their *superscript* equivalents rather than subscript ones. A
/// non-convertible run still falls back to the subscript parenthesized form.
fn to_subscript(text: &str, force_superscript: bool) -> String {
    let kind = if force_superscript {
        Script::Super
    } else {
        Script::Sub
    };
    let mapped: Option<String> = text.chars().map(|ch| script_char(ch, kind)).collect();
    mapped.unwrap_or_else(|| format!("_({text})"))
}

/// Whether a script's content holds an inline that is neither plain text nor a space. Such content
/// triggers the subscript-to-superscript fallback in [`to_subscript`].
fn forces_superscript(inlines: &[Inline]) -> bool {
    inlines
        .iter()
        .any(|inline| !matches!(inline, Inline::Str(_) | Inline::Space))
}

fn script_char(ch: char, kind: Script) -> Option<char> {
    if ch == ' ' {
        return Some(' ');
    }
    let mapped = match kind {
        Script::Super => match ch {
            '0' => '\u{2070}',
            '1' => '\u{00b9}',
            '2' => '\u{00b2}',
            '3' => '\u{00b3}',
            '4' => '\u{2074}',
            '5' => '\u{2075}',
            '6' => '\u{2076}',
            '7' => '\u{2077}',
            '8' => '\u{2078}',
            '9' => '\u{2079}',
            '+' => '\u{207a}',
            '-' => '\u{207b}',
            '=' => '\u{207c}',
            '(' => '\u{207d}',
            ')' => '\u{207e}',
            _ => return None,
        },
        Script::Sub => match ch {
            '0' => '\u{2080}',
            '1' => '\u{2081}',
            '2' => '\u{2082}',
            '3' => '\u{2083}',
            '4' => '\u{2084}',
            '5' => '\u{2085}',
            '6' => '\u{2086}',
            '7' => '\u{2087}',
            '8' => '\u{2088}',
            '9' => '\u{2089}',
            '+' => '\u{208a}',
            '-' => '\u{208b}',
            '=' => '\u{208c}',
            '(' => '\u{208d}',
            ')' => '\u{208e}',
            _ => return None,
        },
    };
    Some(mapped)
}
