//! oxidoc — a document converter library.
//!
//! The single public entry point: [`convert`] parses `input` in a source format and renders it to a
//! target format. Formats are selected at compile time via per-direction Cargo features
//! (`read-*`/`write-*`); [`supported_input_formats`]/[`supported_output_formats`] report what this
//! build contains. For lower-level use, the document model is re-exported as [`ast`], and
//! [`reader_for`]/[`writer_for`] hand back the [`Reader`]/[`Writer`] trait objects so callers can
//! inspect or transform the [`Document`] directly.

pub use oxidoc_ast as ast;
pub use oxidoc_ast::Document;
pub use oxidoc_core::{
    Error, Extension, Extensions, Reader, ReaderOptions, Result, Writer, WriterOptions, presets,
};

mod registry;

pub use registry::{reader_for, supported_input_formats, supported_output_formats, writer_for};

/// Converts `input` from format `from` to format `to`.
///
/// The returned string carries no trailing newline; callers that emit to a stream append their own
/// (the CLI appends exactly one, matching the reference tool).
///
/// # Errors
/// Propagates format-resolution errors ([`Error::UnsupportedFormat`], [`Error::FormatNotEnabled`])
/// and any reader/writer error encountered during conversion.
pub fn convert(
    from: &str,
    to: &str,
    input: &str,
    reader_options: &ReaderOptions,
    writer_options: &WriterOptions,
) -> Result<String> {
    let reader = reader_for(from)?;
    let writer = writer_for(to)?;
    let document = reader.read(input, reader_options)?;
    writer.write(&document, writer_options)
}
