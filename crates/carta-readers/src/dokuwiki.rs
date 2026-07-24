//! Reader for the `DokuWiki` markup language.
//!
//! The grammar is line-oriented at the block level and recursive-descent at the inline level. A
//! block is recognised by its first line: a heading (`=` runs), a code or raw passthrough region
//! (`<code>`, `<file>`, `<HTML>`, `<PHP>`), a table (rows opening with `|` or `^`), a list (`*` for
//! bullets, `-` for ordered, indented at least two columns), an indented code block, a thematic
//! break, a blockquote (`>` runs), or, failing all of those, a paragraph. Inline content is scanned
//! left to right with a small pending-text buffer: emphasis (`//`), strong (`**`), underline
//! (`__`), monospace (`''`), the `<sub>`/`<sup>`/`<del>` spans, links (`[[…]]`), media (`{{…}}`),
//! footnotes (`((…))`), bare URLs, and angle-bracket email addresses each form their own node.
//!
//! When the `Smart` extension is enabled, straight quotes fold into curly
//! [`Inline::Quoted`](carta_ast::Inline::Quoted) runs,
//! `--`/`---` fold into en/em dashes, and `...` folds into an ellipsis.

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
#[derive(Debug, Clone, Copy)]
enum Closer {
    /// A curly-quote run, closed by the matching straight quote character.
    Quote(char),
    /// A two-character emphasis run (`**`, `//`, `__`), closed by a repeat of the given character.
    Delim(char),
    /// A `''…''` monospace run, closed by `''`.
    Mono,
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
        let mut blocks = parse_blocks(&lines, &mut index, ctx, 0);
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

/// The deepest level of inline or block nesting that recursive parsing will follow. Beyond it,
/// would-be delimiters are taken literally, bounding stack use on adversarial input.
const MAX_DEPTH: usize = 32;

#[cfg(test)]
mod tests;
