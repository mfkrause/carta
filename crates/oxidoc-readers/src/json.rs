//! JSON interchange reader: a thin [`Reader`] adapter over the AST's own serde codec.

use oxidoc_ast::Document;
use oxidoc_core::{Reader, ReaderOptions, Result};

/// Parses an interchange JSON document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonReader;

impl Reader for JsonReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        Ok(oxidoc_ast::from_json(input.as_bytes())?)
    }
}
