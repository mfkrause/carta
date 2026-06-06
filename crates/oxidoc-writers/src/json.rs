//! JSON interchange writer: a thin [`Writer`] adapter over the AST's own serde codec.

use oxidoc_ast::Document;
use oxidoc_core::{Result, Writer, WriterOptions};

/// Renders a document as compact interchange JSON (no trailing newline).
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonWriter;

impl Writer for JsonWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        Ok(oxidoc_ast::to_json(document)?)
    }
}
