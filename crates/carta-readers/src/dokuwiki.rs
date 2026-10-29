//! Read `DokuWiki` markup into the document model.

use carta_ast::Document;
use carta_core::{Extension, Reader, ReaderOptions, Result};

use crate::heading_ids::{IdRegistry, IdScheme};

mod blocks;
mod helpers;
mod inline;
mod links;
mod lists;
mod tables;

use blocks::{assign_heading_ids, parse_blocks, strip_wide_line_breaks};
use helpers::normalize_newlines;

/// The inline-syntax toggles that the scanner threads through every level of parsing.
#[derive(Debug, Clone, Copy)]
struct Ctx {
    /// Straight quotes, dashes, and ellipses fold into their typographic forms.
    smart: bool,
    /// `$…$` and `$$…$$` spans are read as inline and display math.
    math: bool,
}

/// What ends an inline scan started by an enclosing construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Closer {
    /// A curly-quote run, closed by the matching straight quote character.
    Quote(char),
    /// A two-character emphasis run (`**`, `//`, `__`), closed by a repeat of the given character.
    Delim(char),
    /// A `''…''` monospace run, closed by `''`.
    Mono,
    /// A footnote, closed by `))`.
    Note,
}

/// Parses `DokuWiki` markup into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct DokuwikiReader;

impl Reader for DokuwikiReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let ctx = Ctx {
            smart: options.extensions.contains(Extension::Smart),
            math: options.extensions.contains(Extension::TexMathDollars),
        };
        let text = normalize_newlines(input);
        let lines: Vec<&str> = text.split('\n').collect();
        let mut index = 0;
        let mut blocks = parse_blocks(&lines, &mut index, ctx);
        if options.extensions.contains(Extension::EastAsianLineBreaks) {
            strip_wide_line_breaks(&mut blocks);
        }
        // Only `auto_identifiers` enables derivation; gfm/ASCII-fold just select the algorithm.
        if options.extensions.contains(Extension::AutoIdentifiers)
            && let Some(scheme) = IdScheme::select(options.extensions, false)
        {
            let ascii = options.extensions.contains(Extension::AsciiIdentifiers);
            let mut registry = IdRegistry::default();
            assign_heading_ids(&mut blocks, scheme, ascii, &mut registry);
        }
        Ok(Document {
            blocks,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests;
