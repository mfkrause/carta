//! Helpers shared by the writers that colorize code blocks (the HTML family and LaTeX): recognizing
//! the line-numbering class, reading the starting line number, and splitting an unclassified block
//! into lines. Token escaping and per-token markup are format-specific and stay with each writer.

use carta_ast::{Attr, Text};
use carta_highlight::{SourceLine, Token, TokenKind};

/// Whether a class requests per-line numbering on a code block.
pub(crate) fn is_number_lines_class(class: &Text) -> bool {
    matches!(class.as_str(), "numberLines" | "number-lines")
}

/// The first line's number: the `startFrom` key parsed as an integer, or 1 when absent or unparsable.
pub(crate) fn start_line(attr: &Attr) -> i64 {
    attr.attributes
        .iter()
        .find(|(key, _)| key.as_str() == "startFrom")
        .and_then(|(_, value)| value.as_str().parse::<i64>().ok())
        .unwrap_or(1)
}

/// Split a code block's text into lines the way the tokenizer does, treating each as a single
/// unclassified run. Used when a block gets the structured scaffolding but names no known language,
/// so every line is one plain token without any color.
pub(crate) fn plain_source_lines(text: &str) -> Vec<SourceLine> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
        .into_iter()
        .map(|line| {
            if line.is_empty() {
                Vec::new()
            } else {
                vec![Token::new(TokenKind::Normal, line)]
            }
        })
        .collect()
}
