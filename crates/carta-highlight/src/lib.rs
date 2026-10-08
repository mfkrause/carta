//! Syntax highlighting with language grammars and color themes.
//!
//! [`Highlighter`] returns classified [`Token`]s grouped into [`SourceLine`]s.
//! [`Registry`] resolves grammars; [`Theme`] supplies colors and font styles.

mod firstbytes;
mod grammar;
mod highlighter;
mod parse;
mod registry;
mod style;
mod token;

pub use highlighter::Highlighter;
pub use parse::ParseError;
pub use registry::{Registry, builtin_style, style_names};
pub use style::{Error as StyleError, Theme, TokenStyle};
pub use token::{SourceLine, Token, TokenKind};

/// The names of the languages offered in listings, in published order.
#[must_use]
pub fn languages() -> Vec<String> {
    Registry::new().languages()
}

/// The names of the built-in color themes, in published order.
#[must_use]
pub fn styles() -> Vec<String> {
    style_names()
}
