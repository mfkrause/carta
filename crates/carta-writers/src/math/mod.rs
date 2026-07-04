//! TeX math conversion shared by every writer.
//!
//! A single tokenizer/parser ([`parse`]) turns TeX math source into one small expression tree; two
//! backends lower that tree:
//!
//! - [`to_inlines`] produces a writer-agnostic [`carta_ast::Inline`] list (variables italicised,
//!   sub/superscripts, unicode symbols and Greek letters, styled alphabets, accents, atom-class
//!   spacing). Each writer renders that list with its own inline machinery, so the conversion logic
//!   lives in exactly one place. It returns `None` for any construct that cannot be laid out on a
//!   single line (fractions, radicals, stacked operator limits, environments, unknown commands), so
//!   the caller can emit the source verbatim.
//! - [`to_typst`] produces Typst math markup (the inner content, no surrounding `$`). Typst has
//!   native math, so this translation succeeds for almost all well-formed input.
//!
//! Both entry points are panic-free and bounded against pathological nesting.

mod inlines;
// The OMML backend lowers the same expression tree into Office Math markup for the word-processing
// writer. That writer is not yet wired to this entry point, so it is unused in current builds.
#[allow(dead_code)]
mod omml;
mod parse;
mod symbols;
mod typst;

use carta_ast::Inline;

/// Convert TeX math source to a writer-agnostic inline tree, or `None` when the expression contains
/// a construct that cannot be linearised into inlines.
pub(crate) fn to_inlines(tex: &str) -> Option<Vec<Inline>> {
    let atoms = parse::parse(tex)?;
    if is_bare_binary_operator(&atoms) {
        return None;
    }
    if ends_with_control_space(&atoms) {
        return None;
    }
    // Whitespace-only or empty math parses to no atoms and lowers to no inlines: that is a
    // successful (empty) conversion, not a fallback to verbatim source.
    inlines::lower(&atoms)
}

/// Convert TeX math source to Typst math markup (the inner content, no surrounding `$`), or `None`
/// when the expression cannot be translated. The body alone; [`to_typst_labeled`] also carries the
/// equation label and is what writers call.
#[cfg(test)]
pub(crate) fn to_typst(tex: &str) -> Option<String> {
    Some(to_typst_labeled(tex, false)?.body)
}

/// As [`to_typst`], but lowering in display context so a prime on a limit operator stacks as a
/// superscript (`\sum'` → `sum^(')`) the way display math sets it.
#[cfg(test)]
pub(crate) fn to_typst_display(tex: &str) -> Option<String> {
    Some(to_typst_labeled(tex, true)?.body)
}

/// Convert TeX math source to Typst math markup (the inner content, no surrounding `$`) and the
/// first equation `\label` it carries, formatted as the trailing Typst reference token to set after
/// the closing `$`. Returns `None` when the expression cannot be translated. With `display` set, the
/// markup is lowered for display context: a prime on a limit operator stacks as a superscript
/// (`\sum'` → `sum^(')`) the way display math sets primes above the operator.
// Only the Typst writer lowers math to Typst markup; builds without it still compile this entry
// point (and the chain it drives) but never call it.
#[cfg_attr(not(feature = "typst"), allow(dead_code))]
pub(crate) fn to_typst_labeled(tex: &str, display: bool) -> Option<TypstMath> {
    let atoms = parse::parse(tex)?;
    if is_bare_binary_operator(&atoms) {
        return None;
    }
    // Whitespace-only or empty math parses to no atoms and lowers to the empty string: that is a
    // successful (empty) conversion, which the writer renders as bare `$$`.
    let (body, label) = typst::lower_labeled(&atoms, display)?;
    Some(TypstMath { body, label })
}

/// Typst math markup: the inner content (no surrounding `$`) and the trailing reference label to set
/// after the closing `$`, if the expression carried an equation `\label`.
#[cfg_attr(not(feature = "typst"), allow(dead_code))]
pub(crate) struct TypstMath {
    pub body: String,
    pub label: Option<String>,
}

/// Whether the final atom is an undimensioned control-space (`\ `) with no operand following it. A
/// trailing control-space has nothing to set its space against, so the expression is emitted verbatim
/// rather than ending on a dangling space. A control-space that precedes further content (`\ x`,
/// `a\ b`) keeps its space and is unaffected; the dimensioned `\hspace`/`\mspace` forms are distinct.
fn ends_with_control_space(atoms: &[parse::Atom]) -> bool {
    matches!(
        atoms.last(),
        Some(parse::Atom {
            body: parse::Body::Command(name),
            sub: None,
            sup: None,
            siblings,
            limits: None,
        }) if name == " " && siblings.is_empty()
    )
}

/// Whether the whole expression is a single binary modulo operator with no operands, e.g. a lone
/// `\bmod`. Such an operator has nothing to bind to and is emitted verbatim rather than rendered
/// with its surrounding spaces.
fn is_bare_binary_operator(atoms: &[parse::Atom]) -> bool {
    matches!(
        atoms,
        [
            parse::Atom {
                body: parse::Body::Mod(parse::ModKind::Bmod, None),
                sub: None,
                sup: None,
                siblings,
                limits: None,
            }
        ] if siblings.is_empty()
    )
}

#[cfg(test)]
mod tests;
