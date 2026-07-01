//! Shared carta core: the conversion traits, their option types, and the common error type.
//!
//! [`Reader`] turns input text into a [`Document`]; [`Writer`] turns a [`Document`] back into
//! output text. Readers and writers depend only on the AST contract and this crate, so input and
//! output formats stay independent.

use std::io;

use carta_ast::{Block, Document, Inline};

pub mod extensions;
pub mod sections;
#[cfg(feature = "template")]
pub mod template;

pub use extensions::{Extension, Extensions, presets};

/// The error type returned across the conversion pipeline.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("input is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("format '{0}' is recognized but not enabled in this build")]
    FormatNotEnabled(String),
    #[error("unknown extension: {0}")]
    UnknownExtension(String),
    #[error(
        "The extension '{extension}' is not supported for {format}.\nUse --list-extensions={format} to list supported extensions."
    )]
    UnsupportedExtension { extension: String, format: String },
    #[error("invalid document metadata: {0}")]
    InvalidMetadata(String),
    #[error("template error: {0}")]
    Template(String),
    #[error("cannot represent this content in the target format: {0}")]
    Unrepresentable(String),
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

/// Options controlling a [`Writer`]. Extended (not resignatured) as real options land.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WriterOptions {
    /// Format extensions to enable.
    pub extensions: Extensions,

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

    /// Emit a complete document by wrapping the rendered body in the target format's template,
    /// rather than a bare fragment.
    #[cfg(feature = "template")]
    pub standalone: bool,

    /// Template source overriding the format's built-in default. Its presence implies standalone
    /// output.
    #[cfg(feature = "template")]
    pub template: Option<String>,

    /// Directory used to resolve template partials (`$name()$`).
    #[cfg(feature = "template")]
    pub template_dir: Option<std::path::PathBuf>,

    /// Extension a partial (`$name()$`) inherits from the including template: the `--template`
    /// file's own extension, so the same partial name resolves to the same kind of file whatever
    /// the output format. An empty string means the template file had no extension (the partial is
    /// looked up bare). Absent for a built-in default, where the format name is used instead.
    #[cfg(feature = "template")]
    pub template_ext: Option<String>,

    /// Raw template variables, in order; a repeated key accumulates into a list. Inserted verbatim
    /// (unescaped) at the highest precedence when building the template context.
    #[cfg(feature = "template")]
    pub variables: Vec<(String, String)>,

    /// Metadata layered *above* the document's own (the `-M` layer): each key replaces the reader's
    /// value for that key when the context is built.
    #[cfg(feature = "template")]
    pub metadata: std::collections::BTreeMap<String, carta_ast::MetaValue>,

    /// Metadata layered *below* the document's own (the metadata-file layer): supplies defaults the
    /// reader's values and `-M` override.
    #[cfg(feature = "template")]
    pub metadata_defaults: std::collections::BTreeMap<String, carta_ast::MetaValue>,

    /// The source name an HTML-family standalone document falls back to for its `pagetitle` when no
    /// `title` metadata is present: an input file's stem, or `-` for standard input. `None` outside
    /// the command line, where there is no source name and the fallback is empty.
    #[cfg(feature = "template")]
    pub source_name: Option<String>,
}

/// Parses input text in some source format into the document model.
pub trait Reader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document>;
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
