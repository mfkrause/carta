//! Output-format writers. Each module renders the document model ([`oxidoc_ast::Document`]) into a
//! target format's text via the [`oxidoc_core::Writer`] trait.

#[cfg(feature = "html")]
pub mod html;
#[cfg(feature = "json")]
pub mod json;

#[cfg(feature = "html")]
pub use html::HtmlWriter;
#[cfg(feature = "json")]
pub use json::JsonWriter;
