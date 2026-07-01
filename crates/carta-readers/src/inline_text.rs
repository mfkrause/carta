//! Small inline-sequence helpers shared by the line-oriented text readers.

use carta_ast::Inline;

/// Drop leading and trailing whitespace inlines (spaces and soft breaks) from an inline sequence.
pub(crate) fn trim_inline_ends(inlines: &mut Vec<Inline>) {
    while matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.remove(0);
    }
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
}
