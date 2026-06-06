//! Input-format readers. Each module parses a source format's text into the document model
//! ([`oxidoc_ast::Document`]) via the [`oxidoc_core::Reader`] trait.

pub mod commonmark;
pub mod json;

pub use commonmark::CommonmarkReader;
pub use json::JsonReader;
