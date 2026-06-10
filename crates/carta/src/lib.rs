//! carta — a document converter library.
//!
//! The single public entry point: [`convert`] parses `input` in a source format and renders it to a
//! target format. Formats are selected at compile time via per-direction Cargo features
//! (`read-*`/`write-*`); [`supported_input_formats`]/[`supported_output_formats`] report what this
//! build contains. For lower-level use, the document model is re-exported as [`ast`], and
//! [`reader_for`]/[`writer_for`] hand back the [`Reader`]/[`Writer`] trait objects so callers can
//! inspect or transform the [`Document`] directly.

pub use carta_ast as ast;
pub use carta_ast::Document;
pub use carta_core::{
    Error, Extension, Extensions, Reader, ReaderOptions, Result, Writer, WriterOptions, presets,
};

mod format_spec;
mod registry;

pub use format_spec::parse_format_spec;
pub use registry::{reader_for, supported_input_formats, supported_output_formats, writer_for};

/// Converts `input` from format `from` to format `to`.
///
/// Each format may carry `+ext`/`-ext` toggles (e.g. `commonmark+strikeout-raw_html`); the selected
/// extensions are merged with any already present in the supplied options.
///
/// The returned string carries no trailing newline; callers that emit to a stream append their own
/// (the CLI appends exactly one).
///
/// # Errors
/// Propagates format-resolution errors ([`Error::UnsupportedFormat`], [`Error::FormatNotEnabled`],
/// [`Error::UnknownExtension`]) and any reader/writer error encountered during conversion.
pub fn convert(
    from: &str,
    to: &str,
    input: &str,
    reader_options: &ReaderOptions,
    writer_options: &WriterOptions,
) -> Result<String> {
    let (from_base, from_ext) = parse_format_spec(from)?;
    let (to_base, to_ext) = parse_format_spec(to)?;

    let reader = reader_for(&from_base)?;
    let writer = writer_for(&to_base)?;

    let mut reader_options = reader_options.clone();
    reader_options.extensions = from_ext.union(reader_options.extensions);
    let mut writer_options = writer_options.clone();
    writer_options.extensions = to_ext.union(writer_options.extensions);

    let document = reader.read(input, &reader_options)?;
    writer.write(&document, &writer_options)
}
