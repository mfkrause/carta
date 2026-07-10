#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Shared carta core: the conversion traits, their option types, and the common error type.
//!
//! [`Reader`] turns input text into a [`Document`]; [`Writer`] turns a [`Document`] back into
//! output text. Readers and writers depend only on the AST contract and this crate, so input and
//! output formats stay independent.

use std::fmt;
use std::io;
use std::sync::Arc;

use carta_ast::{Block, Document, Inline};

#[cfg(feature = "container")]
#[cfg_attr(docsrs, doc(cfg(feature = "container")))]
pub mod container;
pub mod extensions;
pub mod media;
pub mod sections;
#[cfg(feature = "template")]
#[cfg_attr(docsrs, doc(cfg(feature = "template")))]
pub mod template;
pub mod walk;

pub use extensions::{Extension, Extensions, presets};
pub use media::{MediaBag, MediaItem};

/// The error type returned across the conversion pipeline.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// JSON input or output could not be (de)serialized.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// An I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// Input handed to a text reader was not valid UTF-8.
    #[error("input is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    /// A text-only API was asked for a format whose output is binary; use the byte-capable API.
    #[error("format '{0}' converts binary data; use the byte-capable API (convert)")]
    BinaryFormat(String),
    /// The named format is not recognized.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    /// The named format is recognized but not compiled into this build.
    #[error("format '{0}' is recognized but not enabled in this build")]
    FormatNotEnabled(String),
    /// A `+`/`-` toggle named an extension that is not modeled.
    #[error("unknown extension: {0}")]
    UnknownExtension(String),
    /// A modeled extension does not apply to the given format.
    #[error(
        "The extension '{extension}' is not supported for {format}.\nUse --list-extensions={format} to list supported extensions."
    )]
    UnsupportedExtension {
        /// The extension the format does not support.
        extension: String,
        /// The format that does not support the extension.
        format: String,
    },
    /// Document metadata could not be parsed.
    #[error("invalid document metadata: {0}")]
    InvalidMetadata(String),
    /// A standalone template failed to parse or render.
    #[error("template error: {0}")]
    Template(String),
    /// The document holds content the target format cannot represent.
    #[error("cannot represent this content in the target format: {0}")]
    Unrepresentable(String),
    /// Building or reading a container archive failed.
    #[error("container error: {0}")]
    Container(String),
    /// A document filter failed to run or returned an unusable result.
    #[error("filter error: {0}")]
    Filter(String),
    /// A syntax-highlighting style or definition could not be resolved.
    #[cfg(feature = "highlight")]
    #[cfg_attr(docsrs, doc(cfg(feature = "highlight")))]
    #[error("syntax highlighting error: {0}")]
    Highlight(String),
}

#[cfg(feature = "template")]
impl From<template::TemplateError> for Error {
    fn from(error: template::TemplateError) -> Self {
        Error::Template(error.to_string())
    }
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Options controlling a [`Reader`]. Extended (not resignatured) as real options land.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ReaderOptions {
    /// Format extensions to enable. Strict-CommonMark readers ignore this (the empty preset).
    pub extensions: Extensions,
    /// When set, an open paragraph is greedy: a following line that would otherwise open a block —
    /// a blockquote, heading, list, thematic break, fenced div, or footnote definition — is folded
    /// into the paragraph as a lazy continuation instead. Only a blank line, a fenced code block, or
    /// an HTML block ends the paragraph. Unset, every such line interrupts the paragraph.
    pub greedy_paragraphs: bool,
}

/// How math is presented by a format that offers a choice of renderers (the HTML family). The
/// method decides both the inline markup inside a `span.math` and which loader a standalone document
/// pulls in to typeset it: a MathJax (or plain) document carries the source TeX wrapped in `\(…\)` /
/// `\[…\]`, whereas a KaTeX document carries the bare TeX, which its in-browser loader reads from the
/// span directly.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MathMethod {
    /// No renderer: the `\(…\)` / `\[…\]` markup is left for the reader to typeset (or read as
    /// source). The default.
    #[default]
    Plain,
    /// MathJax, loaded from the given script URL. The markup keeps the `\(…\)` / `\[…\]` delimiters.
    MathJax(String),
    /// KaTeX, loaded from the given asset base URL (the directory holding `katex.min.js` and its
    /// stylesheet). The span carries bare TeX without delimiters.
    Katex(String),
}

/// How a writer supplies a table of contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TocStyle {
    /// The contents are rendered as a nested list and placed in the `toc` template variable. The
    /// default.
    #[default]
    List,
    /// The format assembles its own contents from a directive in its template, so only a boolean
    /// `toc` flag is exposed and no list is generated.
    Native,
}

/// How a text writer lays out the lines of a paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// Reflow inline content, breaking lines to keep them within the fill column. A soft line break
    /// in the source is just inter-word space and is re-flowed like any other.
    #[default]
    Auto,
    /// Never break a paragraph: each one is a single line, with soft breaks rendered as spaces. Lines
    /// run as long as their content (only an explicit hard break starts a new line).
    None,
    /// Keep the source's own line breaks: a soft break stays a line break and content is not
    /// reflowed, but lines are not wrapped to a column either.
    Preserve,
}

/// Options for the EPUB container writer. Ignored by every other writer. The default is an empty
/// book: no cover, no embedded fonts, the built-in stylesheet only, and chapters split at the top
/// heading level.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct EpubOptions {
    /// A cover image as `(file name, bytes)`. Produces a dedicated cover page and marks the image
    /// as the publication cover.
    pub cover_image: Option<(String, Vec<u8>)>,

    /// Fonts to embed verbatim, each as `(file name, bytes)`. A stylesheet refers to them by name.
    pub fonts: Vec<(String, Vec<u8>)>,

    /// User stylesheet contents, linked from every page. When any are given they replace the
    /// built-in stylesheet entirely; several are linked in order. Empty leaves the built-in in place.
    pub stylesheets: Vec<String>,

    /// A Dublin Core metadata fragment (bare `<dc:*>` elements) merged into the package metadata.
    pub metadata_xml: Option<String>,

    /// The container directory holding all publication content. `None` uses the conventional
    /// `EPUB`; an empty string places the content at the archive root.
    pub subdirectory: Option<String>,

    /// The heading level at which the book is split into separate chapter files. `None` splits at
    /// the top level, so each level-one heading starts a new file.
    pub split_level: Option<usize>,

    /// Seconds since the Unix epoch fixing the publication's modification timestamp. `None` uses a
    /// fixed epoch so output stays byte-reproducible.
    pub source_date_epoch: Option<i64>,

    /// The process locale (the `LANG` environment variable) whose language tag stands in when the
    /// document names no `lang`. `None` falls back to `en-US`, keeping output independent of the
    /// environment.
    pub locale: Option<String>,
}

/// Options for the DOCX container writer. Ignored by every other writer. The default produces a
/// self-contained document from the built-in template, with reproducible property timestamps and a
/// language tag drawn from the document or the environment.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DocxOptions {
    /// A reference document, as raw `.docx` bytes, whose styling parts and document template are
    /// reused while the converted content replaces its body. `None` uses the built-in template.
    pub reference_doc: Option<Vec<u8>>,

    /// Seconds since the Unix epoch fixing the document's property timestamps. `None` uses a fixed
    /// epoch so output stays byte-reproducible.
    pub source_date_epoch: Option<i64>,

    /// The process locale (the `LANG` environment variable) whose language tag stands in when the
    /// document names no `lang`. `None` falls back to `en-US`, keeping output independent of the
    /// environment.
    pub locale: Option<String>,
}

/// Syntax-highlighting configuration for the writers that colorize code blocks (the HTML family,
/// LaTeX, and DOCX). The default leaves code blocks unhighlighted.
#[cfg(feature = "highlight")]
#[cfg_attr(docsrs, doc(cfg(feature = "highlight")))]
#[derive(Debug, Clone, Default)]
pub struct HighlightOptions {
    /// The tokenizer catalog. `None` leaves code blocks as a plain `<pre><code>`, with no color
    /// spans and no line-number scaffolding.
    pub highlighter: Option<std::sync::Arc<carta_highlight::Highlighter>>,

    /// The active color theme, consulted by the writers that inline colors (LaTeX, DOCX) and to
    /// build the HTML family's stylesheet. `None` when highlighting is off.
    pub theme: Option<carta_highlight::Theme>,

    /// Present code blocks in the target format's own listing construct rather than colorizing them.
    /// No tokenizer runs; a format that offers a dedicated listing environment (LaTeX's `lstlisting`)
    /// uses it, while formats whose plain form already carries the language class (the HTML family,
    /// DOCX) render code exactly as they do with highlighting off. Ignored when a `highlighter` is set.
    pub idiomatic: bool,
}

/// Options controlling a [`Writer`]. Extended (not resignatured) as real options land.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WriterOptions {
    /// Format extensions to enable.
    pub extensions: Extensions,

    /// The embedded resources the document references by name but does not carry inline. A writer
    /// that re-embeds resource bytes — a notebook re-encoding its image outputs — reads them from
    /// here; most writers ignore it. Shared cheaply, so cloning the options does not copy the bytes.
    pub media: Arc<MediaBag>,

    /// Options for the EPUB container writer; ignored by every other writer.
    pub epub: EpubOptions,

    /// Options for the DOCX container writer; ignored by every other writer.
    pub docx: DocxOptions,

    /// How paragraphs are laid out: reflowed to the fill column, never wrapped, or with the source's
    /// own line breaks preserved.
    pub wrap: WrapMode,

    /// The fill column a wrapping writer reflows to under [`WrapMode::Auto`]. `None` uses the
    /// writer's built-in default width.
    pub columns: Option<usize>,

    /// Splice a hierarchical section number into each heading. A format that numbers headings with a
    /// typesetting counter applies it through its template instead (see
    /// [`Writer::numbers_sections_natively`]).
    pub number_sections: bool,

    /// Emit a table of contents in a standalone document.
    pub toc: bool,

    /// The deepest heading level the table of contents includes. `None` uses the conventional depth
    /// of three.
    pub toc_depth: Option<usize>,

    /// How math is presented by a format offering a choice of renderers (the HTML family).
    pub math_method: MathMethod,

    /// Syntax-highlighting configuration for code blocks; the default leaves code unhighlighted.
    #[cfg(feature = "highlight")]
    #[cfg_attr(docsrs, doc(cfg(feature = "highlight")))]
    pub highlight: HighlightOptions,

    /// Emit a complete document by wrapping the rendered body in the target format's template,
    /// rather than a bare fragment.
    #[cfg(feature = "template")]
    #[cfg_attr(docsrs, doc(cfg(feature = "template")))]
    pub standalone: bool,

    /// Template source overriding the format's built-in default. Its presence implies standalone
    /// output.
    #[cfg(feature = "template")]
    #[cfg_attr(docsrs, doc(cfg(feature = "template")))]
    pub template: Option<String>,

    /// Directory used to resolve template partials (`$name()$`).
    #[cfg(feature = "template")]
    #[cfg_attr(docsrs, doc(cfg(feature = "template")))]
    pub template_dir: Option<std::path::PathBuf>,

    /// A shared directory of partials (`$name()$`) consulted when a partial is not found beside the
    /// including template — the data directory's `templates/`. `None` when no data directory applies.
    #[cfg(feature = "template")]
    #[cfg_attr(docsrs, doc(cfg(feature = "template")))]
    pub template_datadir: Option<std::path::PathBuf>,

    /// Extension a partial (`$name()$`) inherits from the including template: the `--template`
    /// file's own extension, so the same partial name resolves to the same kind of file whatever
    /// the output format. An empty string means the template file had no extension (the partial is
    /// looked up bare). Absent for a built-in default, where the format name is used instead.
    #[cfg(feature = "template")]
    #[cfg_attr(docsrs, doc(cfg(feature = "template")))]
    pub template_ext: Option<String>,

    /// Raw template variables, in order; a repeated key accumulates into a list. Inserted verbatim
    /// (unescaped) at the highest precedence when building the template context.
    #[cfg(feature = "template")]
    #[cfg_attr(docsrs, doc(cfg(feature = "template")))]
    pub variables: Vec<(String, String)>,

    /// Metadata layered *above* the document's own (the `-M` layer): each key replaces the reader's
    /// value for that key when the context is built.
    #[cfg(feature = "template")]
    #[cfg_attr(docsrs, doc(cfg(feature = "template")))]
    pub metadata: std::collections::BTreeMap<String, carta_ast::MetaValue>,

    /// Metadata layered *below* the document's own (the metadata-file layer): supplies defaults the
    /// reader's values and `-M` override.
    #[cfg(feature = "template")]
    #[cfg_attr(docsrs, doc(cfg(feature = "template")))]
    pub metadata_defaults: std::collections::BTreeMap<String, carta_ast::MetaValue>,

    /// The source name a standalone document falls back to when no `title` metadata is present: an
    /// input file's stem, or `-` for standard input. `None` outside the command line, where there is
    /// no source name and the fallback is empty. Consumed by the HTML family (for its `pagetitle`)
    /// and by the container writer (for the navigation document's title).
    #[cfg(any(feature = "template", feature = "container"))]
    #[cfg_attr(docsrs, doc(cfg(any(feature = "template", feature = "container"))))]
    pub source_name: Option<String>,
}

/// Parses input text in some source format into the document model.
pub trait Reader {
    /// Parses `input` text into a document.
    ///
    /// # Errors
    /// Propagates any error from parsing the input.
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document>;

    /// Reads `input` into a document together with the embedded resources it references. The default
    /// carries no resources; a container format — a notebook with image outputs — overrides this to
    /// decode those bytes into the returned [`MediaBag`], and implements [`read`](Reader::read) by
    /// discarding the bag.
    ///
    /// # Errors
    /// Propagates any error from parsing the input.
    fn read_media(&self, input: &str, options: &ReaderOptions) -> Result<(Document, MediaBag)> {
        Ok((self.read(input, options)?, MediaBag::new()))
    }
}

/// Which plain-text identity variables a writer's standalone template draws on. The document's
/// title, authors, and date are exposed as markup-free, target-escaped text for places that cannot
/// carry markup — a web document head or a PDF document's properties. See [`Writer::meta_var_style`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetaVarStyle {
    /// The format exposes none of these variables.
    #[default]
    None,
    /// A web document head: `pagetitle` (the title, falling back to the source name), `date-meta`
    /// (the date), and `author-meta` (the authors, one list entry each).
    Web,
    /// A PDF document's properties: `title-meta` (the title) and `author-meta` (the authors joined
    /// into one string with `; `).
    Pdf,
}

/// Renders the document model into some target format's text.
///
/// The returned string carries no trailing newline; the CLI appends exactly one.
pub trait Writer {
    /// Renders `document` into this format's text.
    ///
    /// # Errors
    /// Propagates any error from rendering the document.
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String>;

    /// Render an inline sequence in this format, for interpolating inline metadata (a `title`, an
    /// `author`) into a template variable. Wrapping the inlines in a [`Block::Plain`] yields them
    /// with no paragraph chrome across formats; a writer whose `Plain` diverges overrides this.
    ///
    /// # Errors
    /// Propagates any error from [`Writer::write`].
    fn render_meta_inlines(&self, inlines: &[Inline], options: &WriterOptions) -> Result<String> {
        let document = Document {
            blocks: vec![Block::Plain(inlines.to_vec())],
            ..Document::default()
        };
        Ok(self
            .write(&document, options)?
            .trim_end_matches('\n')
            .to_string())
    }

    /// Render a block sequence in this format, for interpolating block metadata (an `abstract`
    /// authored as Markdown blocks) into a template variable.
    ///
    /// # Errors
    /// Propagates any error from [`Writer::write`].
    fn render_meta_blocks(&self, blocks: &[Block], options: &WriterOptions) -> Result<String> {
        let document = Document {
            blocks: blocks.to_vec(),
            ..Document::default()
        };
        Ok(self
            .write(&document, options)?
            .trim_end_matches('\n')
            .to_string())
    }

    /// This format's own standalone template, or `None` when standalone output is identical to the
    /// fragment (no wrapping document exists for the format).
    fn default_template(&self) -> Option<&'static str> {
        None
    }

    /// A standalone document this format assembles structurally, embedding the metadata and block
    /// list in one value rather than wrapping a text body in a template — the data form is the
    /// canonical example. Returned in place of template rendering. `None` (the default) when the
    /// format wraps its body with a text template instead.
    ///
    /// # Errors
    /// Propagates any error from rendering the document.
    fn standalone_document(
        &self,
        document: &Document,
        options: &WriterOptions,
    ) -> Result<Option<String>> {
        let _ = (document, options);
        Ok(None)
    }

    /// Which plain-text identity variables this writer's standalone template draws on — the title,
    /// authors, and date as markup-free text. The default is [`MetaVarStyle::None`]; an HTML-family
    /// writer returns [`MetaVarStyle::Web`] and a LaTeX-family writer [`MetaVarStyle::Pdf`].
    fn meta_var_style(&self) -> MetaVarStyle {
        MetaVarStyle::None
    }

    /// Whether block-shaped metadata is flattened to its inline content when built into the template
    /// context. A writer that places title, author, and date into single-line header fields — a man
    /// page's `.TH` line cannot carry paragraph structure — sets this so a lone-paragraph value
    /// contributes its inline text and any other block shape contributes nothing. The default `false`
    /// renders block metadata as blocks.
    fn flatten_block_metadata(&self) -> bool {
        false
    }

    /// A title presentation the template language cannot express from individual variables — an
    /// underlined title for reStructuredText, say, whose rule length depends on the rendered title
    /// width. Exposed to the template as the `titleblock` variable. `None` (the default) when the
    /// format builds its title presentation from individual variables instead.
    ///
    /// # Errors
    /// Propagates any error from rendering the metadata.
    fn title_block(&self, document: &Document, options: &WriterOptions) -> Result<Option<String>> {
        let _ = (document, options);
        Ok(None)
    }

    /// Whether this writer lays the document out as newline-terminated lines, so a non-empty `body`
    /// template variable ends with a newline. Writers that build their markup as one string ending
    /// at its final glyph (HTML, LaTeX, and the like) leave the default `false`.
    fn body_ends_with_newline(&self) -> bool {
        false
    }

    /// How this writer supplies a table of contents. The default renders a nested list into the
    /// `toc` variable; a format whose template assembles its own contents from a directive overrides
    /// to [`TocStyle::Native`].
    fn toc_style(&self) -> TocStyle {
        TocStyle::List
    }

    /// Whether a list-style table of contents attaches a back-reference anchor — an `id` on each
    /// entry's link — so the entries can be linked to. The default includes them; a format that
    /// cannot represent an inline identifier (so an attributed link would degrade to raw markup)
    /// overrides to `false`. Honored only when [`toc_style`](Writer::toc_style) is [`TocStyle::List`].
    fn toc_link_anchors(&self) -> bool {
        true
    }

    /// Whether this format numbers sections with its own typesetting counter rather than carrying the
    /// number in the heading text. The default splices a `header-section-number` span into each
    /// heading; a format with a native counter (the typesetting formats) overrides to `true` and is
    /// driven by a `numbersections` template flag instead.
    fn numbers_sections_natively(&self) -> bool {
        false
    }

    /// Whether this writer carries section numbers in the heading text, so the number is spliced into
    /// each heading before rendering (and contents entries inherit it). The default leaves headings
    /// untouched; a format that renders the number inline (HTML) overrides to `true`. A format with a
    /// native counter relies on [`numbers_sections_natively`](Writer::numbers_sections_natively)
    /// instead and leaves this `false`.
    fn numbers_sections_in_body(&self) -> bool {
        false
    }
}

/// Parses input bytes in some source format into the document model. The byte-shaped counterpart of
/// [`Reader`], for formats whose wire form is not text — zip containers and the like.
pub trait BytesReader {
    /// Parses `input` bytes into a document.
    ///
    /// # Errors
    /// Propagates any error from parsing the input.
    fn read(&self, input: &[u8], options: &ReaderOptions) -> Result<Document>;

    /// Reads `input` into a document together with the embedded resources it references. The
    /// byte-shaped counterpart of [`Reader::read_media`]; the default carries no resources.
    ///
    /// # Errors
    /// Propagates any error from parsing the input.
    fn read_media(&self, input: &[u8], options: &ReaderOptions) -> Result<(Document, MediaBag)> {
        Ok((self.read(input, options)?, MediaBag::new()))
    }
}

/// Renders the document model into some target format's bytes. The byte-shaped counterpart of
/// [`Writer`], for formats whose output is not text — zip containers and the like.
///
/// This trait carries no decoration hooks (templates, table of contents, metadata rendering): a
/// container writer produces a complete document by construction. Hooks are added when a real format
/// needs them.
pub trait BytesWriter {
    /// Renders `document` into this format's bytes.
    ///
    /// # Errors
    /// Propagates any error from rendering the document.
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<Vec<u8>>;
}

/// The output of a conversion: text from a text writer, bytes from a byte-shaped writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// Text produced by a text-shaped writer.
    Text(String),
    /// Bytes produced by a byte-shaped writer.
    Bytes(Vec<u8>),
}

/// A resolved reader, either text-shaped ([`Reader`]) or byte-shaped ([`BytesReader`]).
pub enum AnyReader {
    /// A text-shaped reader; input is decoded as UTF-8 before parsing.
    Text(Box<dyn Reader>),
    /// A byte-shaped reader; input is parsed from raw bytes.
    Bytes(Box<dyn BytesReader>),
}

impl fmt::Debug for AnyReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let variant = match self {
            AnyReader::Text(_) => "Text",
            AnyReader::Bytes(_) => "Bytes",
        };
        f.debug_tuple(variant).finish()
    }
}

impl AnyReader {
    /// Reads `input` into a document. A text reader decodes the bytes as UTF-8 first; a byte reader
    /// takes the raw slice.
    ///
    /// # Errors
    /// [`Error::InvalidUtf8`] if a text reader is handed input that is not valid UTF-8, plus any error
    /// the underlying reader returns.
    pub fn read(&self, input: &[u8], options: &ReaderOptions) -> Result<Document> {
        match self {
            AnyReader::Text(reader) => reader.read(std::str::from_utf8(input)?, options),
            AnyReader::Bytes(reader) => reader.read(input, options),
        }
    }

    /// Reads `input` into a document together with the embedded resources it references. A text
    /// reader decodes the bytes as UTF-8 first; a byte reader takes the raw slice. A reader that
    /// carries no resources returns an empty [`MediaBag`].
    ///
    /// # Errors
    /// [`Error::InvalidUtf8`] if a text reader is handed input that is not valid UTF-8, plus any
    /// error the underlying reader returns.
    pub fn read_media(
        &self,
        input: &[u8],
        options: &ReaderOptions,
    ) -> Result<(Document, MediaBag)> {
        match self {
            AnyReader::Text(reader) => reader.read_media(std::str::from_utf8(input)?, options),
            AnyReader::Bytes(reader) => reader.read_media(input, options),
        }
    }
}

/// A resolved writer, either text-shaped ([`Writer`]) or byte-shaped ([`BytesWriter`]).
pub enum AnyWriter {
    /// A text-shaped writer; rendering produces a string.
    Text(Box<dyn Writer>),
    /// A byte-shaped writer; rendering produces raw bytes.
    Bytes(Box<dyn BytesWriter>),
}

impl fmt::Debug for AnyWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let variant = match self {
            AnyWriter::Text(_) => "Text",
            AnyWriter::Bytes(_) => "Bytes",
        };
        f.debug_tuple(variant).finish()
    }
}

impl AnyWriter {
    /// This format's own standalone template, or `None` when standalone output is identical to the
    /// fragment. A byte-shaped writer never has one.
    #[must_use]
    pub fn default_template(&self) -> Option<&'static str> {
        match self {
            AnyWriter::Text(writer) => writer.default_template(),
            AnyWriter::Bytes(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AnyReader, AnyWriter, BytesReader, BytesWriter, Error, Reader, ReaderOptions, Result,
        WriterOptions,
    };
    use carta_ast::Document;

    struct FixedBytesWriter;
    impl BytesWriter for FixedBytesWriter {
        fn write(&self, _document: &Document, _options: &WriterOptions) -> Result<Vec<u8>> {
            Ok(vec![0x00, 0xff, 0x9f])
        }
    }

    struct RawBytesReader;
    impl BytesReader for RawBytesReader {
        fn read(&self, input: &[u8], _options: &ReaderOptions) -> Result<Document> {
            assert_eq!(input, &[0xff, 0xfe]);
            Ok(Document::default())
        }
    }

    struct EmptyTextReader;
    impl Reader for EmptyTextReader {
        fn read(&self, _input: &str, _options: &ReaderOptions) -> Result<Document> {
            Ok(Document::default())
        }
    }

    #[test]
    fn bytes_writer_round_trips_bytes() {
        let writer = AnyWriter::Bytes(Box::new(FixedBytesWriter));
        assert!(writer.default_template().is_none());
        let AnyWriter::Bytes(inner) = &writer else {
            panic!("expected a byte writer");
        };
        let output = inner
            .write(&Document::default(), &WriterOptions::default())
            .unwrap();
        assert_eq!(output, vec![0x00, 0xff, 0x9f]);
    }

    #[test]
    fn text_reader_rejects_invalid_utf8() {
        let reader = AnyReader::Text(Box::new(EmptyTextReader));
        let error = reader
            .read(&[0xff, 0xfe], &ReaderOptions::default())
            .unwrap_err();
        assert!(matches!(error, Error::InvalidUtf8(_)), "{error:?}");
    }

    #[test]
    fn bytes_reader_accepts_invalid_utf8() {
        let reader = AnyReader::Bytes(Box::new(RawBytesReader));
        assert!(
            reader
                .read(&[0xff, 0xfe], &ReaderOptions::default())
                .is_ok()
        );
    }

    #[test]
    fn default_read_media_carries_no_resources() {
        let reader = AnyReader::Text(Box::new(EmptyTextReader));
        let (_, media) = reader
            .read_media(b"anything", &ReaderOptions::default())
            .expect("read succeeds");
        assert!(media.is_empty());
    }
}
