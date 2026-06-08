//! Input-format readers. Each module parses a source format's text into the document model
//! ([`oxidoc_ast::Document`]) via the [`oxidoc_core::Reader`] trait.

#[cfg(any(feature = "commonmark", feature = "html"))]
mod entities;

#[cfg(feature = "commonmark")]
pub mod commonmark;
#[cfg(feature = "html")]
pub mod html;
#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "native")]
pub mod native;

#[cfg(feature = "commonmark")]
pub use commonmark::CommonmarkReader;
#[cfg(feature = "html")]
pub use html::HtmlReader;
#[cfg(feature = "json")]
pub use json::JsonReader;
#[cfg(feature = "native")]
pub use native::NativeReader;
