//! Reader for the Rich Text Format (RTF), the word-processor interchange language.
//!
//! An RTF document is a tree of brace-delimited groups holding three things: control words
//! (`\word`, optionally with a numeric argument), control symbols (`\` before a single non-letter),
//! and literal text. A group scopes formatting: entering one saves the character state, leaving one
//! restores it.
//!
//! Character control words toggle formatting (`\b`, `\i`, `\ul`, `\strike`, `\super`, `\sub`,
//! `\scaps`, `\caps`); text between them is wrapped in the corresponding inline nodes, nested in a
//! fixed order and coalesced so a run that stays bold across an italic span keeps one enclosing
//! bold. Paragraph breaks (`\par`) close paragraphs; `\outlinelevelN` turns one into a heading;
//! `\line` is a hard break. Encoded characters arrive as `\'xx` (a byte in the ANSI code page) or
//! `\uN` (a Unicode scalar with a following fallback the reader skips). Structural groups are
//! recognized by their leading destination word: `\info` fills document metadata, `\pict` decodes an
//! embedded image into the media bag, `\field` unpacks a hyperlink, `\footnote` becomes a note, and
//! `\*\bkmkstart`/`\*\bkmkend` bracket a bookmark span. Font, color, and style tables, and any
//! group flagged ignorable with `\*`, are skipped. A run of `\trowd`/`\cell`/`\row` rows assembles
//! a table.

mod chars;
mod destinations;
mod emitter;
mod inlines;
mod lexer;
mod parser;
#[cfg(test)]
mod tests;

use carta_ast::Document;
use carta_core::{BytesReader, MediaBag, ReaderOptions, Result};

use self::lexer::{decode_input, tokenize};
use self::parser::Parser;

/// Parses a Rich Text Format document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct RtfReader;

impl BytesReader for RtfReader {
    fn read(&self, input: &[u8], options: &ReaderOptions) -> Result<Document> {
        Ok(self.read_media(input, options)?.0)
    }

    fn read_media(&self, input: &[u8], _options: &ReaderOptions) -> Result<(Document, MediaBag)> {
        let text = decode_input(input);
        let tokens = tokenize(&text);
        let mut parser = Parser::new(tokens);
        parser.run();
        let (meta, blocks, media) = parser.finish();
        Ok((
            Document {
                meta,
                blocks,
                ..Document::default()
            },
            media,
        ))
    }
}
