use super::to_typst_labeled;
use carta_ast::Inline;

mod inline_lowering;
mod malformed_and_structures;
mod scripts_and_text;
mod symbols_and_delimiters;
mod text_grids_labels;
mod typst_lowering;

/// The Typst body and formatted trailing label for an expression, for the equation-label tests.
fn typst_labeled(tex: &str) -> Option<(String, Option<String>)> {
    to_typst_labeled(tex, false).map(|m| (m.body, m.label))
}

fn str_inline(s: &str) -> Inline {
    Inline::Str(s.to_string().into())
}

fn emph(inner: Vec<Inline>) -> Inline {
    Inline::Emph(inner)
}

/// A single italic letter: the common shape for a math variable.
fn var(s: &str) -> Inline {
    emph(vec![str_inline(s)])
}
