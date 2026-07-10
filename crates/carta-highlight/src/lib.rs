//! A syntax highlighting engine: it tokenizes source code against a catalog of bundled grammars and
//! resolves the color styles that renderers paint those tokens with.
//!
//! The entry point is [`Highlighter`], which owns the grammar [`Registry`] and the caches that make
//! repeated tokenization cheap. Give it a language name and source text and it returns one
//! [`SourceLine`] per line, each a sequence of classified [`Token`]s. Color [`Theme`]s and the list
//! of bundled languages and styles are exposed alongside for the CLI and the writers.

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
