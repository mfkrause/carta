//! A small string-template engine: variables, conditionals, loops, pipes, and partials over a
//! [`Value`] context. The language uses `$`-delimited directives; see [`parse`] for the grammar and
//! the surrounding module docs for whitespace handling.
//!
//! The engine is format-agnostic and does no I/O: partial inclusion is delegated to a caller-supplied
//! resolver, so the same parsed [`Template`] renders identically whether partials come from disk, an
//! embedded set, or a test fixture.

mod node;
mod parse;
mod pipe;
mod render;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;

pub use node::Template;

/// A value a template can interpolate. Maps are ordered, so iteration and `pairs` are deterministic
/// and key-sorted.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A string, inserted as-is.
    Str(String),
    /// A sequence; iterated by `$for$`, concatenated (no separator) when interpolated directly.
    List(Vec<Value>),
    /// A keyed record; fields reached with `$x.field$`, enumerated with the `pairs` pipe.
    Map(BTreeMap<String, Value>),
    /// A boolean; renders bare as `true`/`false`, and is the one non-empty value that is still falsy
    /// (when `false`) in a conditional.
    Bool(bool),
}

impl Value {
    /// Build a map value from string key/value pairs.
    #[must_use]
    pub fn map(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
        Value::Map(entries.into_iter().collect())
    }
}

/// A template that could not be processed: either a parse failure (an unterminated directive, an
/// unmatched `$if$`/`$for$`, an unknown pipe, …) or a render failure (a referenced partial that
/// cannot be resolved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError {
    message: String,
}

impl TemplateError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TemplateError {}
