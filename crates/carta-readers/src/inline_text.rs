//! Small inline-sequence helpers shared by the line-oriented text readers.

use carta_ast::Inline;

/// Split flat text into inline words separated by single spaces, collapsing every run of whitespace.
#[cfg(any(feature = "man", feature = "rtf"))]
pub(crate) fn words_to_inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(Inline::Space);
        }
        out.push(Inline::Str(word.into()));
    }
    out
}

/// Drop leading and trailing whitespace inlines (spaces and soft breaks) from an inline sequence.
#[cfg(any(feature = "dokuwiki", feature = "rst", feature = "man"))]
pub(crate) fn trim_inline_ends(inlines: &mut Vec<Inline>) {
    while matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.remove(0);
    }
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
}
