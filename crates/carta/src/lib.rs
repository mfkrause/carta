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
    Error, Extension, Extensions, Reader, ReaderOptions, Result, WrapMode, Writer, WriterOptions,
    presets,
};

mod format_spec;
mod registry;
#[cfg(feature = "standalone")]
mod standalone;

pub use format_spec::parse_format_spec;
pub use registry::{
    input_format_names, output_format_names, reader_for, supported_input_formats,
    supported_output_formats, writer_for,
};

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
    // The markdown dialect treats paragraphs as greedy: most block openers need a preceding blank
    // line, so a bare following line continues the paragraph rather than starting a new block.
    reader_options.greedy_paragraphs |= from_base == "markdown";
    let mut writer_options = writer_options.clone();
    writer_options.extensions = to_ext.union(writer_options.extensions);

    #[cfg(feature = "standalone")]
    let document = {
        let mut document = reader.read(input, &reader_options)?;
        standalone::merge_metadata(&mut document, &writer_options);
        document
    };
    #[cfg(not(feature = "standalone"))]
    let document = reader.read(input, &reader_options)?;

    let body = writer.write(&document, &writer_options)?;

    #[cfg(feature = "standalone")]
    if (writer_options.standalone || writer_options.template.is_some())
        && let Some(wrapped) =
            standalone::render(writer.as_ref(), &document, &body, &writer_options, &to_base)?
    {
        return Ok(wrapped);
    }

    Ok(body)
}

/// Parses a metadata file into a metadata map, for use as `WriterOptions::metadata_defaults`.
///
/// Scalar values are parsed as inline Markdown (independent of the document's own input format), so a
/// `title: *Hi*` yields emphasized inlines. `json` selects the JSON parser; otherwise the content is
/// read as YAML (which also accepts single-line JSON).
///
/// # Errors
/// [`Error::InvalidMetadata`] if the content does not parse as the selected format.
#[cfg(feature = "metadata-file")]
pub fn parse_metadata_file(
    content: &str,
    json: bool,
) -> Result<std::collections::BTreeMap<String, ast::MetaValue>> {
    if json {
        carta_readers::metadata::parse_json(content)
    } else {
        carta_readers::metadata::parse_yaml(content)
    }
}

/// Lists every extension carta models, each paired with whether `format` enables it by default.
///
/// `format` is a format specifier (`"gfm"`, `"commonmark+strikeout"`, …); `None` reports the
/// default Markdown dialect's set. Entries are sorted by extension name.
///
/// # Errors
/// [`Error::UnsupportedFormat`] if the base name is recognized by neither a reader nor a writer, or
/// [`Error::UnknownExtension`] if a `+`/`-` toggle names an extension this build does not recognize.
pub fn format_extensions(format: Option<&str>) -> Result<Vec<(Extension, bool)>> {
    let (base, extensions) = parse_format_spec(format.unwrap_or("markdown"))?;
    if !registry::reader_recognizes(&base) && !registry::writer_recognizes(&base) {
        return Err(Error::UnsupportedFormat(base));
    }

    let mut entries: Vec<(Extension, bool)> = Extension::ALL
        .iter()
        .map(|&extension| (extension, extensions.contains(extension)))
        .collect();
    entries.sort_by_key(|(extension, _)| extension.name());
    Ok(entries)
}
