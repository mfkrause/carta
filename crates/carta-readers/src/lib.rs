//! Input-format readers. Each module parses a source format's text into the document model
//! ([`carta_ast::Document`]) via the [`carta_core::Reader`] trait.

#[cfg(any(feature = "commonmark", feature = "mediawiki"))]
mod emoji;
#[cfg(any(feature = "commonmark", feature = "html"))]
mod entities;
#[cfg(any(feature = "commonmark", feature = "man", feature = "rst"))]
mod heading_ids;
#[cfg(any(feature = "commonmark", feature = "html"))]
mod inline_scan;
#[cfg(any(feature = "dokuwiki", feature = "rst", feature = "man"))]
mod inline_text;

#[cfg(feature = "commonmark")]
pub mod commonmark;
#[cfg(feature = "csv")]
pub mod csv;
#[cfg(feature = "dokuwiki")]
pub mod dokuwiki;
#[cfg(feature = "html")]
pub mod html;
#[cfg(feature = "ipynb")]
pub mod ipynb;
#[cfg(feature = "jira")]
pub mod jira;
#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "man")]
pub mod man;
#[cfg(feature = "mediawiki")]
pub mod mediawiki;
#[cfg(feature = "commonmark")]
pub mod metadata;
#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "opml")]
pub mod opml;
#[cfg(feature = "rst")]
pub mod rst;
#[cfg(feature = "tsv")]
pub mod tsv;

#[cfg(feature = "commonmark")]
pub use commonmark::CommonmarkReader;
#[cfg(feature = "csv")]
pub use csv::CsvReader;
#[cfg(feature = "dokuwiki")]
pub use dokuwiki::DokuwikiReader;
#[cfg(feature = "html")]
pub use html::HtmlReader;
#[cfg(feature = "ipynb")]
pub use ipynb::IpynbReader;
#[cfg(feature = "jira")]
pub use jira::JiraReader;
#[cfg(feature = "json")]
pub use json::JsonReader;
#[cfg(feature = "man")]
pub use man::ManReader;
#[cfg(feature = "mediawiki")]
pub use mediawiki::MediawikiReader;
#[cfg(feature = "native")]
pub use native::NativeReader;
#[cfg(feature = "opml")]
pub use opml::OpmlReader;
#[cfg(feature = "rst")]
pub use rst::RstReader;
#[cfg(feature = "tsv")]
pub use tsv::TsvReader;
