//! Output-format writers. Each module renders the document model ([`carta_ast::Document`]) into a
//! target format's text via the [`carta_core::Writer`] trait.

#[cfg(any(
    feature = "html",
    feature = "plain",
    feature = "latex",
    feature = "commonmark",
    feature = "markdown",
    feature = "gfm",
    feature = "rst",
    feature = "mediawiki",
    feature = "typst",
    feature = "asciidoc",
    feature = "man",
    feature = "dokuwiki",
    feature = "jira"
))]
mod common;

#[cfg(any(
    feature = "plain",
    feature = "rst",
    feature = "latex",
    feature = "markdown",
    feature = "gfm"
))]
mod grid;

#[cfg(any(
    feature = "commonmark",
    feature = "markdown",
    feature = "gfm",
    feature = "plain",
    feature = "man",
    feature = "jira",
    feature = "typst"
))]
mod math;

#[cfg(feature = "slides")]
mod slides;

#[cfg(feature = "asciidoc")]
pub mod asciidoc;
#[cfg(feature = "beamer")]
pub mod beamer;
#[cfg(feature = "commonmark")]
pub mod commonmark;
#[cfg(feature = "dokuwiki")]
pub mod dokuwiki;
#[cfg(feature = "html")]
pub mod html;
#[cfg(feature = "jira")]
pub mod jira;
#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "latex")]
pub mod latex;
#[cfg(feature = "man")]
pub mod man;
#[cfg(any(feature = "markdown", feature = "gfm"))]
pub mod markdown;
#[cfg(feature = "mediawiki")]
pub mod mediawiki;
#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "opml")]
pub mod opml;
#[cfg(feature = "plain")]
pub mod plain;
#[cfg(feature = "revealjs")]
pub mod revealjs;
#[cfg(feature = "rst")]
pub mod rst;
#[cfg(feature = "typst")]
pub mod typst;

#[cfg(feature = "asciidoc")]
pub use asciidoc::AsciidocWriter;
#[cfg(feature = "beamer")]
pub use beamer::BeamerWriter;
#[cfg(feature = "commonmark")]
pub use commonmark::CommonmarkWriter;
#[cfg(feature = "dokuwiki")]
pub use dokuwiki::DokuwikiWriter;
#[cfg(feature = "html")]
pub use html::{Html4Writer, HtmlWriter};
#[cfg(feature = "jira")]
pub use jira::JiraWriter;
#[cfg(feature = "json")]
pub use json::JsonWriter;
#[cfg(feature = "latex")]
pub use latex::LatexWriter;
#[cfg(feature = "man")]
pub use man::ManWriter;
#[cfg(feature = "gfm")]
pub use markdown::GfmWriter;
#[cfg(feature = "markdown")]
pub use markdown::{
    CommonmarkXWriter, MarkdownGithubWriter, MarkdownMmdWriter, MarkdownPhpextraWriter,
    MarkdownStrictWriter, MarkdownWriter,
};
#[cfg(feature = "mediawiki")]
pub use mediawiki::MediawikiWriter;
#[cfg(feature = "native")]
pub use native::NativeWriter;
#[cfg(feature = "opml")]
pub use opml::OpmlWriter;
#[cfg(feature = "plain")]
pub use plain::PlainWriter;
#[cfg(feature = "revealjs")]
pub use revealjs::RevealjsWriter;
#[cfg(feature = "rst")]
pub use rst::RstWriter;
#[cfg(feature = "typst")]
pub use typst::TypstWriter;
