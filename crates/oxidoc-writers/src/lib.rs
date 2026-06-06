//! Output-format writers. Each module renders the document model ([`oxidoc_ast::Document`]) into a
//! target format's text via the [`oxidoc_core::Writer`] trait.

pub mod html;
pub mod json;

pub use html::HtmlWriter;
pub use json::JsonWriter;
