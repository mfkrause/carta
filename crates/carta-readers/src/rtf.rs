//! Read Rich Text Format groups, formatting, tables, metadata, notes, and images.

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
