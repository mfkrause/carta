//! JSON interchange entry points for [`Document`].
//!
//! Output is compact and carries no trailing newline; callers that need a terminating newline append
//! one themselves.

use std::io::Write;

use crate::ast::Document;

/// Parse an interchange JSON document from raw bytes.
pub fn from_json(bytes: &[u8]) -> serde_json::Result<Document> {
    serde_json::from_slice(bytes)
}

/// Serialize a document to a compact JSON string.
pub fn to_json(document: &Document) -> serde_json::Result<String> {
    serde_json::to_string(document)
}

/// Serialize a document as compact JSON to a writer.
pub fn to_json_writer<W: Write>(document: &Document, writer: W) -> serde_json::Result<()> {
    serde_json::to_writer(writer, document)
}
