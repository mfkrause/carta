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

// Markup escaping shared by the two XML math backends; live whenever either backend is reachable.
#[cfg_attr(not(any(feature = "docx", feature = "odt")), allow(dead_code))]
mod escape;
// The inline backend lowers the expression tree into a writer-agnostic inline list; the text writers
// render it, but a build with only a container writer (which lowers to MathML or OMML instead)
// compiles it without calling it.
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "markdown",
        feature = "gfm",
        feature = "plain",
        feature = "man",
        feature = "jira",
        feature = "rtf"
    )),
    allow(dead_code)
)]
mod inlines;
// The MathML backend lowers the same expression tree into Presentation MathML for the formula
// objects the `OpenDocument` writer embeds; builds without that writer compile it but never call it.
#[cfg_attr(not(feature = "odt"), allow(dead_code))]
mod mathml;
// The OMML backend lowers the same expression tree into Office Math markup for the word-processing
// writer; builds without that writer compile it but never call it.
#[cfg_attr(not(feature = "docx"), allow(dead_code))]
mod omml;
mod parse;
mod symbols;
mod typst;

use carta_ast::Inline;

/// Convert TeX math source to a writer-agnostic inline tree, or `None` when the expression contains
/// a construct that cannot be linearised into inlines.
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "markdown",
        feature = "gfm",
        feature = "plain",
        feature = "man",
        feature = "jira",
        feature = "rtf"
    )),
    allow(dead_code)
)]
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

/// Convert TeX math source to an Office Math (OMML) fragment: an `<m:oMath>` element for inline
/// math, or an `<m:oMathPara>` wrapper with centered justification for display math. Returns `None`
/// when the source cannot be parsed or holds a construct with no OMML rendering, so the caller emits
/// the verbatim source instead.
#[cfg_attr(not(feature = "docx"), allow(dead_code))]
pub(crate) fn to_omml(tex: &str, display: bool) -> Option<String> {
    omml::to_omml(tex, display)
}

/// Convert TeX math source to a Presentation MathML `<math>` element for an embedded formula object:
/// `display="inline"` for inline math, `display="block"` for display math. Returns `None` only when
/// the source cannot be parsed, so the caller emits the verbatim source instead.
#[cfg_attr(not(feature = "odt"), allow(dead_code))]
pub(crate) fn to_mathml(tex: &str, display: bool) -> Option<String> {
    mathml::to_mathml(tex, display)
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
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "markdown",
        feature = "gfm",
        feature = "plain",
        feature = "man",
        feature = "jira",
        feature = "rtf"
    )),
    allow(dead_code)
)]
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
