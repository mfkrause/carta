//! Shared oxidoc core: the common error type and `Result` alias used across readers and writers.
//!
//! Conversion options and text/attribute helpers land alongside later slices; today this crate
//! holds only the error surface that the conversion pipeline propagates.

use std::io;

/// The error type returned across the conversion pipeline.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
}

/// A `Result` whose error is [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
