//! Read tab-separated values into tables.

use carta_ast::Document;
use carta_core::{Reader, ReaderOptions, Result};

use crate::csv::{build_document, parse_records};

/// Parses tab-separated values into a single table.
#[derive(Debug, Default, Clone, Copy)]
pub struct TsvReader;

impl Reader for TsvReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        Ok(build_document(parse_records(input, '\t', false)))
    }
}
