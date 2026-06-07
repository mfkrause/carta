//! Shared oxidoc core: the conversion traits, their option types, and the common error type.
//!
//! [`Reader`] turns input text into a [`Document`]; [`Writer`] turns a [`Document`] back into
//! output text. Readers and writers depend only on the AST contract and this crate, so input and
//! output formats stay independent (see `docs/PORTING.md` §3).

use std::io;

use oxidoc_ast::Document;

pub mod extensions;

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
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Options controlling a [`Reader`]. Extended (not resignatured) as real options land.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ReaderOptions {
    /// Format extensions to enable. Strict-CommonMark readers ignore this (the empty preset).
    pub extensions: Extensions,
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
}
