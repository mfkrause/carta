//! JSON interchange entry points for [`Document`].
//!
//! Output is compact and carries no trailing newline; callers that need a terminating newline append
//! one themselves. Errors surface as [`serde_json::Error`].

mod de;
mod ser;

use std::io::Write;

use serde::ser::Error as _;

use crate::ast::Document;

/// Parse an interchange JSON document from raw bytes.
pub fn from_json(bytes: &[u8]) -> serde_json::Result<Document> {
    de::from_json_bytes(bytes)
}

/// Serialize a document to a compact JSON string.
pub fn to_json(document: &Document) -> serde_json::Result<String> {
    Ok(ser::write_document_string(document))
}

/// Serialize a document as compact JSON to a writer.
pub fn to_json_writer<W: Write>(document: &Document, mut writer: W) -> serde_json::Result<()> {
    let json = ser::write_document_string(document);
    writer
        .write_all(json.as_bytes())
        .map_err(serde_json::Error::custom)
}
