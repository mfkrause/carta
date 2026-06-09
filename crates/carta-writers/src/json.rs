//! JSON interchange writer: a thin [`Writer`] adapter over the AST's own serde codec.

use carta_ast::Document;
use carta_core::{Result, Writer, WriterOptions};

/// Renders a document as compact interchange JSON (no trailing newline).
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonWriter;

impl Writer for JsonWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        Ok(carta_ast::to_json(document)?)
    }
}
