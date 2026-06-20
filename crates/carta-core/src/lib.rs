//! Shared carta core: the conversion traits, their option types, and the common error type.
//!
//! [`Reader`] turns input text into a [`Document`]; [`Writer`] turns a [`Document`] back into
//! output text. Readers and writers depend only on the AST contract and this crate, so input and
//! output formats stay independent.

use std::io;

use carta_ast::{Block, Document, Inline};

pub mod extensions;
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
    #[error("invalid document metadata: {0}")]
    InvalidMetadata(String),
    #[error("template error: {0}")]
    Template(String),
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

/// Options controlling a [`Writer`]. Extended (not resignatured) as real options land.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WriterOptions {
    /// Format extensions to enable.
    pub extensions: Extensions,
}

/// Parses input text in some source format into the document model.
pub trait Reader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document>;
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
}
