//! Input-format readers. Each module parses a source format's text into the document model
//! ([`oxidoc_ast::Document`]) via the [`oxidoc_core::Reader`] trait.

#[cfg(feature = "commonmark")]
pub mod commonmark;
#[cfg(feature = "json")]
pub mod json;

#[cfg(feature = "commonmark")]
pub use commonmark::CommonmarkReader;
#[cfg(feature = "json")]
pub use json::JsonReader;
