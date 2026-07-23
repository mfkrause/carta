//! Footnote plumbing for the text writers: the marker-numbering host trait and the trailing-section
//! assembler.

use super::{FILL_COLUMN, join_loose};
use carta_ast::{Block, Inline};

/// A text writer that gathers footnotes inline and emits them as a trailing section. Each note is
/// referenced by a numbered `[n]` marker; its body is rendered offset so the marker shifts only the
/// first line's wrap point. The format supplies how a block and a marker-offset leading paragraph
/// render; the marker numbering and slot bookkeeping are shared here.
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "gfm",
        feature = "markdown",
        feature = "plain"
    )),
    allow(dead_code)
)]
pub(crate) trait NotesHost {
    /// The accumulated note bodies, indexed by note number minus one.
    fn notes(&mut self) -> &mut Vec<String>;

    /// Render a block at the given fill width.
    fn render_block(&mut self, block: &Block, width: usize) -> String;

    /// Render a leading paragraph's text with its first line beginning `initial` columns in.
    fn render_offset_paragraph(
        &mut self,
        inlines: &[Inline],
        width: usize,
        initial: usize,
    ) -> String;

    /// The fill width a note's body lays out to: the document's configured column width.
    fn base_width(&self) -> usize {
        FILL_COLUMN
    }

    /// Record a footnote: reserve its slot before rendering (so nested notes number after it), fill
    /// the slot with the assembled body, and return the inline `[n]` marker.
    fn record_note(&mut self, blocks: &[Block]) -> String {
        self.numbered_note(blocks)
    }

    /// Record a footnote in the generic numbered form — a `[n]` reference marker and a matching
    /// `[n] body` definition whose first line is offset by the marker width. This is the layout a
    /// markdown dialect without the `footnotes` extension falls back to, so it stays reachable even
    /// when [`record_note`](Self::record_note) is overridden with a richer footnote syntax.
    fn numbered_note(&mut self, blocks: &[Block]) -> String {
        let index = self.notes().len();
        self.notes().push(String::new());
        let marker = format!("[{}]", index + 1);
        let field = marker.chars().count() + 1;
        let body = self.offset_note_body(blocks, field);
        // The body shares the marker's line only when it opens with a paragraph; a leading block of
        // any other kind (a code block, a list) begins on the line below the marker.
        let starts_inline = matches!(blocks.first(), Some(Block::Plain(_) | Block::Para(_)));
        let rendered = if body.is_empty() {
            marker.clone()
        } else if starts_inline {
            format!("{marker} {body}")
        } else {
            format!("{marker}\n{body}")
        };
        if let Some(slot) = self.notes().get_mut(index) {
            *slot = rendered;
        }
        marker
    }

    /// Render a footnote's body: the first block's opening line is offset by the marker width, every
    /// later block and continuation line sits at the margin.
    #[cfg_attr(not(any(feature = "gfm", feature = "markdown")), allow(dead_code))]
    fn note_body(&mut self, blocks: &[Block], initial: usize) -> String {
        self.offset_note_body(blocks, initial)
    }

    /// The marker-offset note body: the first block's opening line begins `initial` columns in and
    /// every later block sits at the margin. Kept separate from [`note_body`](Self::note_body) so an
    /// overriding writer can still reach the generic layout from [`numbered_note`](Self::numbered_note).
    fn offset_note_body(&mut self, blocks: &[Block], initial: usize) -> String {
        let width = self.base_width();
        let rendered = blocks
            .iter()
            .enumerate()
            .map(|(position, block)| {
                let is_plain = matches!(block, Block::Plain(_));
                let text = if position == 0 {
                    self.note_block_offset(block, width, initial)
                } else {
                    self.render_block(block, width)
                };
                (is_plain, text)
            })
            .collect();
        join_loose(rendered)
    }

    /// Render a block whose first line begins `initial` columns in. Only a leading paragraph wraps,
    /// so the offset is meaningful for it alone; other block kinds render at the margin.
    fn note_block_offset(&mut self, block: &Block, width: usize, initial: usize) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                self.render_offset_paragraph(inlines, width, initial)
            }
            other => self.render_block(other, width),
        }
    }
}

/// Append a gathered footnote section to a rendered body, separated by a blank line, and trim the
/// trailing newlines. With no notes this just trims the body.
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "gfm",
        feature = "markdown",
        feature = "org",
        feature = "plain"
    )),
    allow(dead_code)
)]
pub(crate) fn append_notes(body: String, notes: &[String]) -> String {
    let mut out = body;
    if !notes.is_empty() {
        let section = notes.join("\n\n");
        out = if out.is_empty() {
            section
        } else {
            format!("{out}\n\n{section}")
        };
    }
    out.trim_end_matches('\n').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_notes_sections() {
        assert_eq!(append_notes("body\n".to_owned(), &[]), "body");
        assert_eq!(
            append_notes("body".to_owned(), &["[1] note".to_owned()]),
            "body\n\n[1] note"
        );
        assert_eq!(
            append_notes(String::new(), &["[1] note".to_owned()]),
            "[1] note"
        );
    }
}
