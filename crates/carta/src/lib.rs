#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! carta — a document converter library.
//!
//! The public entry points: [`convert`] handles any format pair, taking raw bytes and returning an
//! [`Output`] that is text or bytes depending on the target format's wire shape; [`convert_text`] is
//! a shortcut for when both sides are text. Formats are selected
//! at compile time via per-direction Cargo features (`read-*`/`write-*`);
//! [`supported_input_formats`]/[`supported_output_formats`] report what this build contains. For
//! lower-level use, the document model is re-exported as [`ast`], and [`reader_for`]/[`writer_for`]
//! hand back the [`Reader`]/[`Writer`] trait objects so callers can inspect or transform the
//! [`Document`] directly.

pub use carta_ast as ast;
pub use carta_ast::Document;
pub use carta_core::{
    AnyReader, AnyWriter, BytesReader, BytesWriter, DocxOptions, EpubOptions, Error, Extension,
    Extensions, MathMethod, MediaBag, MediaItem, Output, Reader, ReaderOptions, Result, TocStyle,
    WrapMode, Writer, WriterOptions, media, presets, walk,
};

use std::sync::Arc;

mod format_spec;
mod registry;
#[cfg(feature = "standalone")]
mod standalone;

pub use format_spec::parse_format_spec;
pub use registry::{
    any_reader_for, any_writer_for, input_format_names, output_format_names, reader_for,
    supported_input_formats, supported_output_formats, writer_for,
};

/// The syntax-highlighting configuration attached to [`WriterOptions`], and the catalog the CLI
/// draws its language and style listings from.
#[cfg(feature = "highlight")]
pub use carta_core::HighlightOptions;
#[cfg(feature = "highlight")]
pub use carta_highlight::{Highlighter, Theme, builtin_style, languages, styles};

/// The post-render pass that inlines an HTML page's external resources as `data:` URIs and inline
/// `<style>`/`<script>` elements, together with the resolved-resource type it consumes. Drives the
/// CLI's self-contained HTML output.
#[cfg(feature = "write-html")]
pub use carta_writers::{Resource, inline_resources};

/// Converts text `input` from format `from` to text in format `to`.
///
/// A shortcut over [`convert`] for the common case where both formats are text-shaped. Each format
/// may carry `+ext`/`-ext` toggles (e.g. `commonmark+strikeout-raw_html`); the selected extensions
/// are merged with any already present in the supplied options.
///
/// The returned string carries no trailing newline; callers that emit to a stream append their own
/// (the CLI appends exactly one).
///
/// # Errors
/// [`Error::BinaryFormat`] if `to` names a byte-shaped format (its output cannot be represented as a
/// `String` — use [`convert`]). Otherwise propagates format-resolution errors
/// ([`Error::UnsupportedFormat`], [`Error::FormatNotEnabled`], [`Error::UnknownExtension`]) and any
/// reader/writer error encountered during conversion.
pub fn convert_text(
    from: &str,
    to: &str,
    input: &str,
    reader_options: &ReaderOptions,
    writer_options: &WriterOptions,
) -> Result<String> {
    match convert(from, to, input.as_bytes(), reader_options, writer_options)? {
        Output::Text(text) => Ok(text),
        Output::Bytes(_) => {
            let (base, _) = parse_format_spec(to)?;
            Err(Error::BinaryFormat(base))
        }
    }
}

/// Converts raw `input` bytes from format `from` to format `to`, returning text for a text target and
/// bytes for a byte-shaped one.
///
/// This handles any format pair; [`convert_text`] is a shortcut for when both sides are text. A text
/// reader decodes `input` as UTF-8 (yielding [`Error::InvalidUtf8`] on invalid bytes); a byte reader
/// takes the raw slice. Each format may carry `+ext`/`-ext` toggles, merged with the extensions
/// already in the supplied options.
///
/// # Errors
/// Propagates format-resolution errors ([`Error::UnsupportedFormat`], [`Error::FormatNotEnabled`],
/// [`Error::UnknownExtension`]) and any reader/writer error encountered during conversion.
///
/// ```
/// use carta::{convert, Output, ReaderOptions, WriterOptions};
///
/// let output = convert(
///     "commonmark",
///     "html",
///     b"# Hi\n",
///     &ReaderOptions::default(),
///     &WriterOptions::default(),
/// )
/// .unwrap();
/// assert_eq!(output, Output::Text("<h1>Hi</h1>".to_owned()));
/// ```
pub fn convert(
    from: &str,
    to: &str,
    input: &[u8],
    reader_options: &ReaderOptions,
    writer_options: &WriterOptions,
) -> Result<Output> {
    let (document, media) = read_document(from, input, reader_options)?;
    render_document(to, document, media, writer_options)
}

/// Parses `input` in format `from` into the document model together with the embedded resources it
/// references (a notebook's image outputs; empty for a format that carries none).
///
/// The reading half of [`convert`], exposed so a caller can inspect or transform the [`Document`] —
/// and extract or rewrite its media — before rendering it with [`render_document`]. `from` may carry
/// `+ext`/`-ext` toggles, merged with the extensions already in `reader_options`.
///
/// # Errors
/// Propagates format-resolution errors ([`Error::UnsupportedFormat`], [`Error::FormatNotEnabled`],
/// [`Error::UnknownExtension`]) and any reader error, including [`Error::InvalidUtf8`] when a
/// text-shaped reader is handed input that is not valid UTF-8.
pub fn read_document(
    from: &str,
    input: &[u8],
    reader_options: &ReaderOptions,
) -> Result<(Document, MediaBag)> {
    let (from_base, from_ext) = format_spec::parse_reader_format_spec(from)?;
    let reader = any_reader_for(&from_base)?;

    let mut reader_options = reader_options.clone();
    reader_options.extensions = from_ext.union(reader_options.extensions);
    // The markdown dialect and its variants treat paragraphs as greedy: most block openers need a
    // preceding blank line, so a bare following line continues the paragraph rather than starting a
    // new block.
    reader_options.greedy_paragraphs |= from_base.starts_with("markdown");

    reader.read_media(input, &reader_options)
}

/// Renders `document` into format `to`, returning text for a text target and bytes for a byte-shaped
/// one. `media` supplies the embedded resources a re-embedding writer (a notebook re-encoding its
/// image outputs) draws on; pass an empty bag when the document references none.
///
/// The writing half of [`convert`]. `to` may carry `+ext`/`-ext` toggles, merged with the extensions
/// already in `writer_options`.
///
/// # Errors
/// Propagates format-resolution errors ([`Error::UnsupportedFormat`], [`Error::FormatNotEnabled`],
/// [`Error::UnknownExtension`]) and any writer error encountered during rendering.
pub fn render_document(
    to: &str,
    document: Document,
    media: MediaBag,
    writer_options: &WriterOptions,
) -> Result<Output> {
    let (to_base, to_ext) = parse_format_spec(to)?;
    let writer = any_writer_for(&to_base)?;

    let mut writer_options = writer_options.clone();
    writer_options.extensions = to_ext.union(writer_options.extensions);
    writer_options.media = Arc::new(media);

    #[cfg(feature = "standalone")]
    let document = {
        let mut document = document;
        standalone::merge_metadata(&mut document, &writer_options);
        document
    };

    // A byte-shaped writer owns its complete output: no template, standalone wrapping, or section
    // splicing decorates it.
    let writer = match writer {
        AnyWriter::Text(writer) => writer,
        AnyWriter::Bytes(writer) => {
            return writer.write(&document, &writer_options).map(Output::Bytes);
        }
    };

    // A pristine copy of the document is kept only when the contents builder will later consume it:
    // numbering splices section numbers into the heading inlines the builder reads, so it must see
    // the unnumbered tree to avoid double-numbering its entries. When no standalone wrapper runs the
    // pristine copy is never read again, so the body is numbered in place. A format with a
    // typesetting counter leaves the body untouched and is driven by a template flag instead.
    #[cfg(feature = "standalone")]
    let pristine_needed = writer_options.standalone || writer_options.template.is_some();
    #[cfg(not(feature = "standalone"))]
    let pristine_needed = false;

    let mut document = document;
    let body = if writer_options.number_sections && writer.numbers_sections_in_body() {
        if pristine_needed {
            let mut numbered = document.clone();
            carta_core::sections::number_sections(&mut numbered.blocks);
            writer.write(&numbered, &writer_options)?
        } else {
            carta_core::sections::number_sections(&mut document.blocks);
            return writer.write(&document, &writer_options).map(Output::Text);
        }
    } else {
        writer.write(&document, &writer_options)?
    };

    #[cfg(feature = "standalone")]
    if (writer_options.standalone || writer_options.template.is_some())
        && let Some(wrapped) =
            standalone::render(writer.as_ref(), &document, &body, &writer_options, &to_base)?
    {
        return Ok(Output::Text(wrapped));
    }

    Ok(Output::Text(body))
}

/// Folds the extra metadata layers carried in `writer_options` into `document.meta`: the
/// metadata-file defaults sit below the document's own values, and the `-M` layer above them.
///
/// [`render_document`] does this itself just before writing, so a direct conversion needs no separate
/// call. It is exposed for a caller that transforms the document between [`read_document`] and
/// [`render_document`] — running it through a filter — and wants the transform to observe the same
/// merged metadata the writer will. Such a caller merges here, then clears
/// [`WriterOptions::metadata`] and [`WriterOptions::metadata_defaults`] so rendering does not layer
/// them a second time.
#[cfg(feature = "standalone")]
#[cfg_attr(docsrs, doc(cfg(feature = "standalone")))]
pub fn merge_metadata(document: &mut Document, writer_options: &WriterOptions) {
    standalone::merge_metadata(document, writer_options);
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
#[cfg_attr(docsrs, doc(cfg(feature = "metadata-file")))]
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

    // A format that declares a fixed extension set lists only those; otherwise every modeled
    // extension is reported with its default state.
    let supported = format_spec::supported_extensions(&base);
    let mut entries: Vec<(Extension, bool)> = Extension::ALL
        .iter()
        .filter(|&&extension| supported.is_none_or(|set| set.contains(extension)))
        .map(|&extension| (extension, extensions.contains(extension)))
        .collect();
    entries.sort_by_key(|(extension, _)| extension.name());
    Ok(entries)
}
