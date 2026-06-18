//! Tab-separated value reader.
//!
//! Shares the table assembly and field tokenizer of the comma-separated reader but splits records
//! on tabs with no quoting: every character is literal and a line break always ends a record.

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
