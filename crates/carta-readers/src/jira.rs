//! Reader for Jira wiki markup, the line-oriented "text formatting notation" used in Jira
//! issue fields and comments.
//!
//! Blocks are recognised by a line prefix (`hN.`, `bq.`, list markers, table pipes, `----`) or a
//! paired brace macro (`{code}`, `{noformat}`, `{quote}`, `{panel}`). Inline markup (text effects
//! with flanking delimiters, links, images, monospaced and coloured spans, anchors, symbols, and
//! emoticons) is applied to the text of each line; markup does not span a line boundary.

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
