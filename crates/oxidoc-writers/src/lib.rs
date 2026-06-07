//! Output-format writers. Each module renders the document model ([`oxidoc_ast::Document`]) into a
//! target format's text via the [`oxidoc_core::Writer`] trait.

#[cfg(any(feature = "html", feature = "plain", feature = "latex"))]
mod common;

#[cfg(feature = "html")]
pub mod html;
#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "latex")]
pub mod latex;
#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "plain")]
pub mod plain;

#[cfg(feature = "html")]
pub use html::HtmlWriter;
#[cfg(feature = "json")]
pub use json::JsonWriter;
#[cfg(feature = "latex")]
pub use latex::LatexWriter;
#[cfg(feature = "native")]
pub use native::NativeWriter;
#[cfg(feature = "plain")]
pub use plain::PlainWriter;
