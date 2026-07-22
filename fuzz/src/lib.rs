//! Shared scaffolding for the writer fuzz targets.
//!
//! Each writer target synthesizes a `Document` via `Arbitrary` and drives one format's writers
//! through these checks: rendering must never panic, and rendering the same document twice must
//! produce identical output (writers guarantee byte-reproducible output).

use carta_ast::Document;
use carta_core::{BytesWriter, Writer, WriterOptions};

/// Render `document` twice with a text writer; panics and divergent output are bugs. A rendering
/// error is acceptable (some documents are unrepresentable in some formats), but the second call
/// must then error as well.
pub fn check_text_writer(writer: &impl Writer, document: &Document) {
    let options = WriterOptions::default();
    let first = writer.write(document, &options);
    let second = writer.write(document, &options);
    assert_eq!(
        first.as_ref().ok(),
        second.as_ref().ok(),
        "writer output must be reproducible"
    );
}

/// Render `document` twice with a byte-shaped writer; the byte-shaped counterpart of
/// [`check_text_writer`].
pub fn check_bytes_writer(writer: &impl BytesWriter, document: &Document) {
    let options = WriterOptions::default();
    let first = writer.write(document, &options);
    let second = writer.write(document, &options);
    assert_eq!(
        first.as_ref().ok(),
        second.as_ref().ok(),
        "writer output must be reproducible"
    );
}
