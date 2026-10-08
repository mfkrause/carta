//! Read Jira wiki markup into the document model.

use carta_ast::Document;
use carta_core::{Reader, ReaderOptions, Result};

mod block;
mod inline;
mod links;
mod shared;
#[cfg(test)]
mod tests;

use self::block::parse_blocks_from_str;

/// Parses Jira wiki markup into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct JiraReader;

impl Reader for JiraReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        Ok(Document {
            blocks: parse_blocks_from_str(input),
            ..Document::default()
        })
    }
}
