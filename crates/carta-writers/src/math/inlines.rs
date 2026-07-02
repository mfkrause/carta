//! Backend A: lower a parsed math tree to a writer-agnostic [`Inline`] list.
//!
//! Variables render in italics, sub/superscripts become [`Inline::Subscript`]/[`Inline::Superscript`],
//! named functions and symbols render upright, and inter-token spacing follows TeX's atom-class
//! rules using the appropriate fixed-width unicode spaces. Any construct that cannot be laid out on
//! a single line (fractions, radicals, stacked operator limits, …) makes the whole conversion
//! return `None`, so the caller can fall back to verbatim source.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use carta_ast::Inline;

use super::parse::{Atom, Body, Delim, ModKind, ScriptKind, TextPiece};
use super::symbols::{self, Alphabet, Class, is_limit_glyph};
use super::typst::SYMBOL_TYPST;

/// Space inserted around an active binary operator (four-per-em space).
const BIN_SPACE: &str = "\u{2005}";
/// Space inserted around an active relation (three-per-em space).
const REL_SPACE: &str = "\u{2004}";
/// Thin space after punctuation and trailing a bare named function (six-per-em space).
const THIN_SPACE: &str = "\u{2006}";

/// Lower a list of atoms to inlines. Returns `None` if any atom is unconvertible.
pub(super) fn lower(atoms: &[Atom]) -> Option<Vec<Inline>> {
    lower_styled(atoms, Style::Plain)
}

/// A per-atom style applied while lowering, preserving atom-class spacing between atoms and
/// recursing into each atom's scripts so a scripted atom is styled in both base and script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Plain,
    /// Each atom is set upright and wrapped in `Strong`, the way `\mathbf`/`\boldsymbol` bold every
    /// atom of their argument.
    Bold,
    /// Each atom is wrapped in `Emph`, the way `\mathit` slants every atom of its argument.
    Italic,
    /// Each atom is set bold and italic, wrapped `Strong(Emph(..))`.
    BoldItalic,
    /// Each atom is set upright with no extra wrapper, keeping full math spacing: `\mathrm`/`\mathsf`.
    Upright,
    /// Each character is set upright and rendered as its own code span: `\mathtt`.
    Monospace,
    /// Each character is mapped to its styled-alphabet codepoint; a character with no styled variant
    /// is rendered best-effort and kept with its ordinary math spacing: `\mathbb`/`\mathcal`/
    /// `\mathscr`/`\mathfrak`.
    Substitute(Alphabet),
    /// As [`Style::Substitute`], but each styled character is additionally wrapped in `Strong`:
    /// `\mathbfcal`/`\mathbffrak`/`\mathbfscr`.
    SubstituteBold(Alphabet),
}

impl Style {
    /// Whether this style sets its atoms upright, stripping the italics a variable would carry.
    fn is_upright(self) -> bool {
        matches!(
            self,
            Style::Bold
                | Style::Upright
                | Style::Monospace
                | Style::Substitute(_)
                | Style::SubstituteBold(_)
        )
    }
}

/// Lower a list of atoms to inlines, applying `style` to each atom while preserving the atom-class
/// spacing between them. Returns `None` if any atom is unconvertible.
fn lower_styled(atoms: &[Atom], style: Style) -> Option<Vec<Inline>> {
    let rendered = render_atoms(atoms, style)?;
    let effective = effective_classes(&rendered);
    let mut out = Vec::new();
    for (i, item) in rendered.iter().enumerate() {
        let class = effective.get(i).copied().unwrap_or(item.class);
        let left = i.checked_sub(1).and_then(|p| effective.get(p)).copied();
        let right = effective.get(i + 1).copied();
        if let Some(pre) = leading_space(class, left, right) {
            push_str(&mut out, pre);
        }
        out.extend(item.inlines.iter().cloned());
        if let Some(post) = trailing_space(class, left, right) {
            push_str(&mut out, post);
        }
    }
    Some(merge_adjacent_str(out))
}

/// The effective spacing class of each rendered atom after the retyping passes.
///
/// TeX suppresses the surrounding space of a binary operator that has no left operand: an operator
/// written first, or right after another operator, a relation, an opening delimiter, punctuation, or
/// a large operator, becomes ordinary for spacing. The retyping happens once — the now-ordinary atom
/// is a valid operand, so the next binary operator keeps its spacing. A large operator additionally
/// absorbs the thin space that a punctuation atom immediately after it would carry. A binary operator
/// with no valid right operand — one immediately followed by a relation, a closing delimiter, or
/// punctuation — likewise becomes ordinary, so it binds to its left operand instead of being spaced.
fn effective_classes(rendered: &[Rendered]) -> Vec<Class> {
    let mut effective: Vec<Class> = Vec::with_capacity(rendered.len());
    for (i, item) in rendered.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| effective.get(p)).copied();
        // A manual space written immediately after an operator or relation supplies that gap itself,
        // so the operator drops its own automatic spacing (on both sides) and becomes ordinary; this
        // avoids doubling a manual and an automatic space where they meet.
        let next_manual = rendered.get(i + 1).is_some_and(|n| n.is_manual_space);
        let class = match item.class {
            Class::Bin | Class::Rel | Class::Punct if next_manual => Class::Ord,
            Class::Bin if prev.is_none_or(retypes_following_operator) => Class::Ord,
            Class::Punct if prev == Some(Class::Op) => Class::Ord,
            other => other,
        };
        effective.push(class);
    }
    // A binary operator followed by a relation, closing delimiter, or punctuation has no right operand
    // to bind, so it retypes to ordinary. The look-ahead uses the class settled by the forward pass.
    for i in 0..effective.len() {
        let unbinds = effective.get(i) == Some(&Class::Bin)
            && effective
                .get(i + 1)
                .is_some_and(|next| matches!(next, Class::Rel | Class::Close | Class::Punct));
        if unbinds && let Some(slot) = effective.get_mut(i) {
            *slot = Class::Ord;
        }
    }
    effective
}

/// Whether an atom of this class strips the operator spacing of a binary operator written directly
/// after it.
fn retypes_following_operator(class: Class) -> bool {
    matches!(
        class,
        Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct
    )
}

/// A rendered atom: its inlines plus the math class that governs neighbouring spacing, and whether
/// the atom is a manual spacing command (`\,`, `\quad`, the tie `~`, …) — a manual space suppresses
/// the automatic spacing of an operator or relation written immediately before it.
struct Rendered {
    inlines: Vec<Inline>,
    class: Class,
    is_manual_space: bool,
}

fn render_atoms(atoms: &[Atom], style: Style) -> Option<Vec<Rendered>> {
    let mut out = Vec::new();
    for atom in atoms {
        out.push(render_atom(atom, style)?);
    }
    Some(out)
}

/// Apply the atom-level wrapper a style adds around an already-rendered atom (base plus its styled
/// scripts). The substituting and upright styles change each glyph at the leaf and add no wrapper
/// here; only the emphasis styles wrap the whole atom.
fn wrap_styled_atom(inlines: Vec<Inline>, style: Style) -> Vec<Inline> {
    match style {
        // Bold sets letters upright-bold, so the italic wrapper a variable would carry is removed
        // before the strong wrapper is added.
        Style::Bold => vec![Inline::Strong(strip_italic(inlines))],
        // Italic slants every atom, including digits and symbols not italicised by default; an
        // already-italic letter is unwrapped first so it is not doubly emphasised.
        Style::Italic => vec![Inline::Emph(strip_italic(inlines))],
        // Bold-italic strikes the variable italics first, then nests an emphasis inside a strong
        // wrapper so each atom is both bold and slanted.
        Style::BoldItalic => vec![Inline::Strong(vec![Inline::Emph(strip_italic(inlines))])],
        Style::Plain
        | Style::Upright
        | Style::Monospace
        | Style::Substitute(_)
        | Style::SubstituteBold(_) => inlines,
    }
}

/// Strip the variable italics from a run of inlines: `Emph[Str x]` becomes `Str x`. Bold styling
/// renders letters upright-bold, so the italic wrapper a variable would carry is removed first.
fn strip_italic(inlines: Vec<Inline>) -> Vec<Inline> {
    inlines
        .into_iter()
        .flat_map(|i| match i {
            Inline::Emph(inner) => strip_italic(inner),
            other => vec![other],
        })
        .collect()
}

#[allow(clippy::similar_names)]
fn render_atom(atom: &Atom, style: Style) -> Option<Rendered> {
    let (inlines, class, is_limit_op, intrinsic_thin) = render_nucleus(&atom.body, style)?;

    // A bare manual-spacing nucleus suppresses the automatic spacing of an operator before it.
    let is_manual_space = atom.sub.is_none()
        && atom.sup.is_none()
        && atom.siblings.is_empty()
        && is_manual_space_body(&atom.body);

    // Sequence the scripts into render runs: the primary subscript/superscript first, then every
    // sibling run, each reordered so the subscript renders before the superscript.
    let runs = atom.script_runs();
    let has_sub = runs
        .iter()
        .flat_map(|r| &r.scripts)
        .any(|s| s.kind == ScriptKind::Sub);
    let has_sup = runs
        .iter()
        .flat_map(|r| &r.scripts)
        .any(|s| s.kind == ScriptKind::Sup);
    let has_scripts = runs.iter().any(|r| !r.scripts.is_empty());

    // Stacked limits cannot be linearised. `\limits` forces stacking whenever a script is present;
    // `\nolimits` forces the scripts beside the operator; with no override a limit-class operator
    // stacks only when it carries both a sub- and a superscript.
    let stacks = match atom.limits {
        Some(true) => has_scripts,
        Some(false) => false,
        None => is_limit_op && has_sub && has_sup,
    };
    if stacks {
        return None;
    }

    // The style wraps the base nucleus, then the scripts — already styled by recursing the style into
    // them — are appended outside that wrapper, so a scripted atom is styled in base and script
    // independently rather than as one wrapped unit. The substituting and upright styles changed each
    // glyph at the leaf and add no wrapper here.
    let mut inlines = wrap_styled_atom(inlines, style);

    if has_scripts {
        for run in &runs {
            for script in &run.scripts {
                let rendered = render_script(script.atoms, style)?;
                match script.kind {
                    ScriptKind::Sub => inlines.push(Inline::Subscript(rendered)),
                    ScriptKind::Sup => inlines.push(Inline::Superscript(rendered)),
                }
            }
        }
    } else if intrinsic_thin {
        // A bare named function (no scripts) is followed by a thin space.
        push_str(&mut inlines, THIN_SPACE);
    }

    // A large operator that carries scripts no longer absorbs the space of a following operator: its
    // scripts make it an ordinary operand for the spacing pass.
    let class = if class == Class::Op && has_scripts {
        Class::Ord
    } else {
        class
    };

    Some(Rendered {
        inlines,
        class,
        is_manual_space,
    })
}

/// Whether a nucleus is a manual spacing command (`\,`, `\;`, `\quad`, `\medspace`, …) or the active
/// tie `~`, which all render as a fixed-width space.
fn is_manual_space_body(body: &Body) -> bool {
    match body {
        Body::Char('~') => true,
        Body::Command(name) => symbols::spacing(name).is_some(),
        _ => false,
    }
}

/// Lower a script group's atoms to inlines under `style`, so a styled base carries the same style
/// into its sub/superscripts. A group that is a single bare prime atom (a trailing prime run with no
/// scripts of its own) renders as the flat prime glyph string; a scripted or otherwise structured
/// prime atom lowers normally so its nesting is preserved.
fn render_script(group: &[Atom], style: Style) -> Option<Vec<Inline>> {
    if let [
        atom @ Atom {
            body: Body::Prime(count),
            ..
        },
    ] = group
        && atom.sub.is_none()
        && atom.sup.is_none()
        && atom.siblings.is_empty()
    {
        return Some(vec![Inline::Str(prime_glyph(*count).into())]);
    }
    let mut inlines = lower_styled(group, style)?;
    // A script whose sole content is a bare named function drops that function's trailing thin space:
    // standing alone as the whole script it has no operand to be set off from. A function alongside
    // other atoms keeps the space, as does a manually written space (`\,`).
    if let [single] = group
        && ends_with_intrinsic_thin_space(single)
    {
        trim_trailing_thin_space(&mut inlines);
    }
    Some(inlines)
}

/// Whether an atom emits a trailing intrinsic thin space: a bare named function or `\operatorname`
/// with no scripts of its own (a script suppresses the trailing space, as does any added sibling).
fn ends_with_intrinsic_thin_space(atom: &Atom) -> bool {
    atom.sub.is_none()
        && atom.sup.is_none()
        && atom.siblings.is_empty()
        && match &atom.body {
            Body::Command(name) => symbols::named_function(name).is_some(),
            Body::Text(name, _) => name == "operatorname",
            _ => false,
        }
}

/// Remove a single trailing thin space from a lowered run, dropping the last `Str` entirely if the
/// thin space was its only character.
fn trim_trailing_thin_space(inlines: &mut Vec<Inline>) {
    if let Some(Inline::Str(last)) = inlines.last_mut()
        && let Some(trimmed) = last.strip_suffix(THIN_SPACE)
    {
        if trimmed.is_empty() {
            inlines.pop();
        } else {
            last.truncate(trimmed.len());
        }
    }
}

/// Render an atom's nucleus under `style`. Returns its inlines, math class, whether it is a limit
/// operator, and whether it carries an intrinsic trailing thin space (bare named functions). The
/// style substitutes or wraps each leaf glyph (styled-alphabet codepoint, upright, monospace),
/// recursing through transparent groups; constructs with no glyph of their own ignore it.
#[allow(clippy::match_same_arms)]
fn render_nucleus(body: &Body, style: Style) -> Option<(Vec<Inline>, Class, bool, bool)> {
    match body {
        // An empty nucleus contributes no glyph; only its scripts render. It carries ordinary class
        // so a neighbouring binary operator still spaces against it. A captured equation `\label`
        // likewise has no linear glyph — labels surface only in Typst output — so it lowers the same.
        Body::Empty | Body::EmptyGroup | Body::Label(_) => {
            Some((Vec::new(), Class::Ord, false, false))
        }
        // A prime mark that surfaces as a bare nucleus renders as the prime glyph. A run of five or
        // more marks sets the first quadruple-prime as the glyph and lifts the remaining marks into a
        // superscript, so a long bare run stacks rather than spilling across the baseline.
        Body::Prime(count) => Some((prime_nucleus(*count), Class::Ord, false, false)),
        // A bare (unescaped) TeX-active character — the parameter `#`, the alignment tab `&`, or the
        // comment `%` — has no ordinary-symbol meaning in inline math, so the whole expression falls
        // back to verbatim. A `&` that is an alignment separator is consumed by the grid parser and
        // never reaches here; the escaped forms `\#`/`\&`/`\%` arrive as commands and still convert.
        Body::Char('#' | '&' | '%') => None,
        Body::Char(c) => render_char(*c, style),
        // The `:=` digraph prints as the two literal characters and takes relation spacing as a unit.
        Body::ColonEq => Some((vec![Inline::Str(":=".into())], Class::Rel, false, false)),
        Body::Number(digits) => render_number(digits, style),
        Body::Command(name) => render_command(name, style),
        Body::Group(inner) => {
            let inlines = lower_styled(inner, style)?;
            // A group is transparent for spacing: take the class of its first/only element is hard
            // to attribute, so groups render as ordinary atoms.
            Some((inlines, Class::Ord, false, false))
        }
        Body::Accent(name, base) => render_accent(name, base),
        Body::Styled(name, arg) => render_styled(name, arg),
        Body::Text(name, content) => render_text(name, content),
        // A fixed-size wrapper renders its bare glyph; the size is presentational and does not
        // survive linear output. The sized glyph always takes ordinary class, so it binds tightly to
        // its neighbours whatever the glyph's usual class would be.
        Body::Big(_, delim) => {
            let (inlines, _, _, _) = render_nucleus(&delim.body, Style::Plain)?;
            Some((inlines, Class::Ord, false, false))
        }
        Body::Delimited(open, close, content) => render_delimited(*open, *close, content),
        // A `\middle<delim>` divider inside a delimited group: its plain glyph, with no space of its
        // own. An absent delimiter (`.`) contributes nothing.
        Body::Middle(delim, open_side) => {
            let side = if *open_side {
                DelimSide::Open
            } else {
                DelimSide::Close
            };
            let glyph = delim.map_or("", |d| delim_glyph(d, side));
            Some((
                vec![Inline::Str(glyph.to_string().into())],
                Class::Ord,
                false,
                false,
            ))
        }
        Body::Mod(kind, arg) => render_mod(*kind, arg.as_deref()),
        Body::Negated(base) => render_negated(base),
        // Two-dimensional constructs never linearise. A `\not` over a braced group has no linear
        // overlay, so it falls back to verbatim alongside them.
        Body::NegatedGroup(_)
        | Body::Frac(_, _)
        | Body::Sqrt(_, _)
        | Body::Binom(_, _, _)
        | Body::Matrix(_, _)
        | Body::Brace(_, _)
        | Body::Stack(_, _, _)
        | Body::Grid(_, _)
        | Body::ExtArrow(_, _, _) => None,
    }
}

/// Render a `\not`-negated base. The strike-through depends on the base's atom class:
///
/// - over a relation (`=`, `\leq`, `\to`, …) the base carries a precomposed negated glyph or a
///   combining long solidus and keeps relation spacing;
/// - over a letter (Latin or Greek), a digit, or a delimiter glyph (`(`, `)`, `[`, `]`, `|`) the
///   base renders in its normal math styling — variable italics for a letter or delimiter, upright
///   for a digit — with a trailing combining long solidus (U+0338), and binds as an ordinary atom;
/// - over a binary or large operator, an ordinary symbol, or punctuation the strike has nothing
///   meaningful to overlay, so the whole expression falls back to verbatim.
fn render_negated(base: &str) -> Option<(Vec<Inline>, Class, bool, bool)> {
    if symbols::is_unnegatable(base) {
        return None;
    }
    if let Some(glyph) = symbols::negated_relation(base) {
        return Some((
            vec![Inline::Str(glyph.to_string().into())],
            Class::Rel,
            false,
            false,
        ));
    }
    match negated_base(base)? {
        NegatedBase::Relation(glyph) => {
            let mut struck = glyph;
            struck.push('\u{0338}');
            Some((vec![Inline::Str(struck.into())], Class::Rel, false, false))
        }
        NegatedBase::Italic(glyph) => {
            let mut struck = glyph;
            struck.push('\u{0338}');
            Some((vec![italic(struck)], Class::Ord, false, false))
        }
        NegatedBase::Upright(glyph) => {
            let mut struck = glyph;
            struck.push('\u{0338}');
            Some((vec![Inline::Str(struck.into())], Class::Ord, false, false))
        }
    }
}

/// How a `\not` base is struck through, with its plain (unstruck) glyph.
enum NegatedBase {
    /// A relation base: the combining solidus sits on it and it keeps relation spacing.
    Relation(String),
    /// A letter, Greek letter, or delimiter base: the struck glyph is set in variable italics and
    /// binds as an ordinary atom.
    Italic(String),
    /// A digit base: the struck glyph is set upright and binds as an ordinary atom.
    Upright(String),
}

/// Classify a `\not` base and resolve its plain glyph, or `None` when the base carries no meaningful
/// strike-through (a binary or large operator, an ordinary symbol, or punctuation).
fn negated_base(base: &str) -> Option<NegatedBase> {
    if let Some(c) = single_char(base) {
        if c.is_ascii_digit() {
            return Some(NegatedBase::Upright(c.to_string()));
        }
        if c.is_alphabetic() {
            return Some(NegatedBase::Italic(c.to_string()));
        }
        let (text, class, _) = char_glyph(c);
        return match class {
            Class::Rel => Some(NegatedBase::Relation(text)),
            // The delimiter glyphs `(`, `)`, `[`, `]`, and the bar `|` (an ordinary-class bar) carry
            // the strike as an italicised ordinary atom.
            Class::Open | Class::Close => Some(NegatedBase::Italic(text)),
            Class::Ord if c == '|' => Some(NegatedBase::Italic(text)),
            _ => None,
        };
    }
    if let Some((letter, _)) = symbols::greek(base) {
        return Some(NegatedBase::Italic(letter.to_string()));
    }
    let sym = symbols::symbol(base)?;
    if sym.class == Class::Rel {
        return Some(NegatedBase::Relation(sym.text.to_string()));
    }
    // A letterlike symbol set in variable italics (`\ell`, `\imath`, `\aleph`, …) takes the strike;
    // an upright letterlike glyph (`\hbar`, `\Re`, …) or any operator base does not.
    if sym.italic {
        return Some(NegatedBase::Italic(sym.text.to_string()));
    }
    None
}

/// The single character of a one-character string, if it is exactly one.
fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        Some(first)
    } else {
        None
    }
}

/// Render a modulo operator. The `mod` forms carry the operator word `mod` flanked by fixed spaces;
/// the parenthesised forms (`\pmod`, `\pod`) wrap their argument in parentheses, and `\pod` omits the
/// word. `\mod` leads its operand with an en-quad space; the others lead with a non-breaking space.
fn render_mod(kind: ModKind, arg: Option<&[Atom]>) -> Option<(Vec<Inline>, Class, bool, bool)> {
    let prefix = match kind {
        ModKind::Bmod => "\u{00A0}mod\u{2006}\u{00A0}",
        ModKind::Pmod => "\u{00A0}(mod\u{2006}\u{00A0}",
        ModKind::Mod => "\u{2000}mod\u{2006}\u{00A0}",
        ModKind::Pod => "\u{00A0}(",
    };
    let mut inlines = vec![Inline::Str(prefix.to_string().into())];
    if let Some(arg) = arg {
        inlines.extend(lower(arg)?);
        inlines.push(Inline::Str(")".into()));
    }
    Some((merge_adjacent_str(inlines), Class::Ord, false, false))
}

/// Render a `\left … \right` group as its opening glyph, the lowered content, and its closing glyph.
/// An absent delimiter (`.`) contributes no glyph. The whole group is an ordinary atom.
fn render_delimited(
    open: Option<Delim>,
    close: Option<Delim>,
    content: &[Atom],
) -> Option<(Vec<Inline>, Class, bool, bool)> {
    let mut inlines = Vec::new();
    if let Some(d) = open {
        push_str(&mut inlines, delim_glyph(d, DelimSide::Open));
    }
    inlines.extend(lower(content)?);
    if let Some(d) = close {
        push_str(&mut inlines, delim_glyph(d, DelimSide::Close));
    }
    Some((merge_adjacent_str(inlines), Class::Ord, false, false))
}

#[derive(Clone, Copy)]
enum DelimSide {
    Open,
    Close,
}

/// The glyph for a stretchable delimiter on the given side.
fn delim_glyph(delim: Delim, side: DelimSide) -> &'static str {
    match (delim, side) {
        (Delim::Paren, DelimSide::Open) => "(",
        (Delim::Paren, DelimSide::Close) => ")",
        (Delim::Bracket, DelimSide::Open) => "[",
        (Delim::Bracket, DelimSide::Close) => "]",
        (Delim::Brace, DelimSide::Open) => "{",
        (Delim::Brace, DelimSide::Close) => "}",
        (Delim::Bar, _) => "|",
        (Delim::BarVert, _) => "\u{2225}",
        (Delim::DoubleBar, _) => "\u{2016}",
        (Delim::Angle, DelimSide::Open) => "\u{27E8}",
        (Delim::Angle, DelimSide::Close) => "\u{27E9}",
        (Delim::Floor, DelimSide::Open) => "\u{230A}",
        (Delim::Floor, DelimSide::Close) => "\u{230B}",
        (Delim::Ceil, DelimSide::Open) => "\u{2308}",
        (Delim::Ceil, DelimSide::Close) => "\u{2309}",
        (Delim::CornerUpperLeft, _) => "\u{231C}",
        (Delim::CornerUpperRight, _) => "\u{231D}",
    }
}

fn render_text(name: &str, content: &[TextPiece]) -> Option<(Vec<Inline>, Class, bool, bool)> {
    // The wrapper's formatting applies to each literal run independently, so a spacing inside the
    // wrapper produces a separately-wrapped run on either side; the spacing itself is a bare glyph.
    let intrinsic_thin = name == "operatorname";
    let mut inlines = Vec::new();
    for piece in content {
        match piece {
            TextPiece::Run(run) => inlines.extend(wrap_text_run(name, run)?),
            TextPiece::Space(space) => {
                inlines.push(Inline::Str(space.codepoint().to_string().into()));
            }
            // A `$…$` segment is math, rendered unaffected by the wrapper's own formatting.
            TextPiece::Math(atoms) => inlines.extend(lower(atoms)?),
        }
    }
    // An empty wrapper contributes no glyph at all, the way a bare `{}` does: it must not emit an
    // empty styled inline (an empty `Strong`/`Emph`/code span would print as stray markup). A named
    // operator keeps its trailing thin space even when empty.
    Some((inlines, Class::Ord, false, intrinsic_thin))
}

/// Wrap one literal run of text in the wrapper's formatting.
fn wrap_text_run(name: &str, run: &str) -> Option<Vec<Inline>> {
    let text = Inline::Str(run.to_string().into());
    let wrapped = match name {
        "text" | "textrm" | "textsf" | "mbox" | "operatorname" => vec![text],
        "textbf" => vec![Inline::Strong(vec![text])],
        "textit" => vec![Inline::Emph(vec![text])],
        // Typewriter text renders as a code span over the run.
        "texttt" => vec![Inline::Code(Box::default(), run.to_string().into())],
        _ => return None,
    };
    Some(wrapped)
}

/// Spacing class of a raw Unicode math glyph written directly in the source. It inverts the symbol
/// table: each command's glyph (from [`symbols::symbol`]) carries that command's spacing class. A
/// large operator written as a bare glyph takes no surrounding space, so [`Class::Op`] maps to
/// [`Class::Ord`]; a bare delimiter glyph likewise carries no space of its own, so the open/close
/// classes map to [`Class::Ord`]. Built once into a `BTreeMap` from an ordered iteration, so the
/// result is deterministic; on a glyph reachable from several commands the first in iteration order
/// wins.
static GLYPH_CLASS: LazyLock<BTreeMap<char, Class>> = LazyLock::new(build_glyph_class);

fn build_glyph_class() -> BTreeMap<char, Class> {
    let mut map = BTreeMap::new();
    for (name, _) in SYMBOL_TYPST {
        if let Some(sym) = symbols::symbol(name) {
            let class = match sym.class {
                Class::Op | Class::Open | Class::Close => Class::Ord,
                other => other,
            };
            let mut chars = sym.text.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                map.entry(c).or_insert(class);
            }
        }
    }
    map
}

/// The spacing class a bare glyph carries, when the symbol table assigns it one.
fn glyph_class(c: char) -> Option<Class> {
    GLYPH_CLASS.get(&c).copied()
}

#[allow(clippy::unnecessary_wraps)]
fn render_char(c: char, style: Style) -> Option<(Vec<Inline>, Class, bool, bool)> {
    // A letter or digit under a substituting style maps to its styled glyph — a dedicated codepoint
    // or, when no styled variant exists, a best-effort fall-through of its plain glyph. A
    // fall-through letter/digit is an ordinary atom.
    if (c.is_alphabetic() || c.is_ascii_digit())
        && matches!(style, Style::Substitute(_) | Style::SubstituteBold(_))
        && let Some(styled) = style_glyph(c, style)
    {
        return Some((styled, Class::Ord, false, false));
    }
    let (text, class, is_limit) = char_glyph(c);
    // Monospace renders every glyph as its own code span, preserving its math class so an operator
    // or punctuation glyph keeps its spacing; the substituting styles already mapped a letter/digit
    // above and fall through here for a symbol, which keeps its plain glyph and class.
    let inlines = match style {
        Style::Monospace => vec![Inline::Code(Box::default(), text.into())],
        _ if c.is_alphabetic() && !style.is_upright() => vec![italic(text)],
        _ => vec![Inline::Str(text.into())],
    };
    Some((inlines, class, is_limit, false))
}

/// The plain glyph string, math class, and limit flag for a bare character written in math source.
fn char_glyph(c: char) -> (String, Class, bool) {
    if c.is_alphabetic() || c.is_ascii_digit() {
        return (c.to_string(), Class::Ord, false);
    }
    let (text, class) = match c {
        '+' => ("+".to_string(), Class::Bin),
        '-' => ("\u{2212}".to_string(), Class::Bin),
        '*' => ("*".to_string(), Class::Bin),
        '/' => ("/".to_string(), Class::Ord),
        '=' => ("=".to_string(), Class::Rel),
        '<' => ("<".to_string(), Class::Rel),
        '>' => (">".to_string(), Class::Rel),
        ':' => (":".to_string(), Class::Rel),
        ',' => (",".to_string(), Class::Punct),
        ';' => (";".to_string(), Class::Punct),
        '(' | '[' => (c.to_string(), Class::Open),
        ')' | ']' => (c.to_string(), Class::Close),
        // A bare vertical bar opens a delimited group, so a sign following it is unary (`|-x|`
        // sets `-` tight) just as after `(` or `[`. The command form `\vert` stays an ordinary
        // atom and keeps its surrounding spacing.
        '|' => ("|".to_string(), Class::Open),
        // An active tie renders as a non-breaking space.
        '~' => ("\u{00A0}".to_string(), Class::Ord),
        // A bare glyph carrying a stacked-limit class falls back to verbatim when it would carry both
        // a sub- and a superscript; that decision is made by the caller from the limit flag.
        _ if is_limit_glyph(c) => (c.to_string(), Class::Op),
        // A bare operator, relation, or punctuation glyph carries its symbol-table spacing class;
        // anything the table does not classify (primes, stray punctuation) is an ordinary atom.
        _ => (c.to_string(), glyph_class(c).unwrap_or(Class::Ord)),
    };
    (text, class, is_limit_glyph(c))
}

/// Render a numeric literal under `style`. A digit run is a single nucleus: each digit is mapped to
/// its styled glyph, then the run is emitted under a single wrapper (one code span for monospace,
/// one `Strong` for a bold substituting alphabet, a plain concatenated string otherwise).
#[allow(clippy::unnecessary_wraps)]
fn render_number(digits: &str, style: Style) -> Option<(Vec<Inline>, Class, bool, bool)> {
    let inlines = match style {
        Style::Monospace => vec![Inline::Code(Box::default(), digits.to_string().into())],
        Style::Substitute(alphabet) => vec![Inline::Str(substitute_run(digits, alphabet).into())],
        Style::SubstituteBold(alphabet) => {
            vec![Inline::Strong(vec![Inline::Str(
                substitute_run(digits, alphabet).into(),
            )])]
        }
        _ => vec![Inline::Str(digits.to_string().into())],
    };
    Some((inlines, Class::Ord, false, false))
}

/// Map a single letter or digit to its styled inline run for a substituting/monospace style, or
/// `None` when the style adds no per-glyph styling. A styled-alphabet codepoint is used when one
/// exists; otherwise the character is kept as its plain glyph (a fall-through), still under the
/// style's wrapper. Returns the fully-wrapped inline run for that one character.
fn style_glyph(c: char, style: Style) -> Option<Vec<Inline>> {
    match style {
        Style::Substitute(alphabet) => Some(vec![Inline::Str(substitute_char(c, alphabet).into())]),
        Style::SubstituteBold(alphabet) => Some(vec![Inline::Strong(vec![Inline::Str(
            substitute_char(c, alphabet).into(),
        )])]),
        // Monospace renders the character as its own code span, whatever the character.
        Style::Monospace => Some(vec![Inline::Code(Box::default(), c.to_string().into())]),
        _ => None,
    }
}

/// The styled glyph string for a run of characters in a substituting alphabet, each character mapped
/// independently and concatenated.
fn substitute_run(run: &str, alphabet: Alphabet) -> String {
    run.chars().map(|c| substitute_char(c, alphabet)).collect()
}

/// The styled glyph string for a single character in a substituting alphabet. A character with a
/// dedicated styled codepoint uses it; the double-struck alphabet additionally maps a few letterlike
/// glyphs (the lower/upper gamma and pi, the n-ary sum) to their double-struck forms; otherwise the
/// character falls through as its plain glyph.
fn substitute_char(c: char, alphabet: Alphabet) -> String {
    symbols::styled_letter(alphabet, c)
        .or_else(|| {
            if alphabet == Alphabet::DoubleStruck {
                symbols::double_struck_special(c).map(|s| s.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| c.to_string())
}

fn render_command(name: &str, style: Style) -> Option<(Vec<Inline>, Class, bool, bool)> {
    if let Some(s) = symbols::spacing(name) {
        return Some((
            vec![Inline::Str(s.to_string().into())],
            Class::Ord,
            false,
            false,
        ));
    }
    if let Some((letter, is_var)) = symbols::greek(name) {
        return Some((
            style_single_glyph(letter, is_var, style),
            Class::Ord,
            false,
            false,
        ));
    }
    if let Some((text, is_limit)) = symbols::named_function(name) {
        let inlines = style_text_glyph(text, style);
        return Some((inlines, Class::Ord, is_limit, true));
    }
    if let Some(sym) = symbols::symbol(name) {
        let inlines = style_single_glyph(sym.text, sym.italic, style);
        let is_limit = symbols::is_limit_operator(name);
        return Some((inlines, sym.class, is_limit, false));
    }
    // An unknown control sequence cannot be rendered; fall back to verbatim.
    None
}

/// Style a command's resolved glyph string under `style`. A single-character glyph routes through
/// the substituting/monospace per-glyph path (so `\sum` under `\mathbb` reaches its double-struck
/// special, and any glyph under `\mathtt` becomes a code span); a multi-character glyph (a named
/// function word) is styled as a text run. `italic` is whether the glyph slants by default.
fn style_single_glyph(text: &str, italic_default: bool, style: Style) -> Vec<Inline> {
    if let Some(c) = single_char(text)
        && let Some(styled) = style_glyph(c, style)
    {
        return styled;
    }
    if italic_default && !style.is_upright() {
        italic_run(text)
    } else {
        vec![Inline::Str(text.to_string().into())]
    }
}

/// Style a multi-character glyph word (a named function) under `style`. Monospace renders the whole
/// word as a single code span; a substituting style maps each letter independently and concatenates;
/// every other style keeps the run whole and upright.
fn style_text_glyph(text: &str, style: Style) -> Vec<Inline> {
    match style {
        Style::Monospace => vec![Inline::Code(Box::default(), text.to_string().into())],
        Style::Substitute(alphabet) => vec![Inline::Str(substitute_run(text, alphabet).into())],
        Style::SubstituteBold(alphabet) => {
            vec![Inline::Strong(vec![Inline::Str(
                substitute_run(text, alphabet).into(),
            )])]
        }
        _ => vec![Inline::Str(text.to_string().into())],
    }
}

fn italic_run(text: &str) -> Vec<Inline> {
    vec![italic(text.to_string())]
}

fn render_accent(name: &str, base: &[Atom]) -> Option<(Vec<Inline>, Class, bool, bool)> {
    let mark = symbols::accent(name)?;
    // A combining mark sits on a single letter-class glyph: a Latin letter, a Greek letter, or a
    // named single-glyph letter (`\imath`, `\ell`, `\aleph`, …). Over a digit, operator, relation,
    // delimiter, large operator, or a styled or multi-atom base the mark has nothing it can attach to,
    // so the whole expression falls back to verbatim.
    if !is_letter_class_base(base) {
        return None;
    }
    let base_inlines = lower(base)?;
    let combined = append_combining(&base_inlines, mark)?;
    Some((combined, Class::Ord, false, false))
}

/// Whether an accent's base is a single letter-class atom — a Latin letter, a Greek letter, or a
/// named single-glyph letter that renders as an italic variable — and so can carry a combining mark.
/// A scripted, styled, multi-atom, digit, or symbol (non-letter) base does not qualify.
fn is_letter_class_base(base: &[Atom]) -> bool {
    let [atom] = base else { return false };
    if atom.sub.is_some() || atom.sup.is_some() || !atom.siblings.is_empty() {
        return false;
    }
    match &atom.body {
        // Any single Unicode letter — Latin, Greek, Cyrillic, CJK — can carry a combining mark.
        Body::Char(c) => c.is_alphabetic(),
        Body::Command(name) => {
            // Greek letters and any symbol the table renders as an italic variable are letter-class;
            // an upright symbol (`\hbar`, `\partial`, `\Re`, …) is not.
            symbols::greek(name).is_some() || symbols::symbol(name).is_some_and(|sym| sym.italic)
        }
        // A transparent single-atom group inherits its content's class.
        Body::Group(inner) => is_letter_class_base(inner),
        _ => false,
    }
}

/// Append a combining mark to the single text glyph produced by an accent's base.
fn append_combining(inlines: &[Inline], mark: char) -> Option<Vec<Inline>> {
    match inlines {
        [Inline::Emph(inner)] => {
            let appended = append_combining(inner, mark)?;
            Some(vec![Inline::Emph(appended)])
        }
        [Inline::Str(text)] => {
            let mut s = text.clone();
            s.push(mark);
            Some(vec![Inline::Str(s)])
        }
        _ => None,
    }
}

#[allow(clippy::match_same_arms)]
fn render_styled(name: &str, arg: &[Atom]) -> Option<(Vec<Inline>, Class, bool, bool)> {
    match name {
        "mathbb" | "mathds" => styled(arg, Style::Substitute(Alphabet::DoubleStruck)),
        "mathcal" | "mathscr" => styled(arg, Style::Substitute(Alphabet::Script)),
        "mathfrak" => styled(arg, Style::Substitute(Alphabet::Fraktur)),
        // The bold script and fraktur alphabets style each letter to its dedicated bold codepoint
        // and wrap it in `Strong`, one letter at a time.
        "mathbfcal" | "mathbfscr" => styled(arg, Style::SubstituteBold(Alphabet::BoldScript)),
        "mathbffrak" => styled(arg, Style::SubstituteBold(Alphabet::BoldFraktur)),
        // Bold-italic and bold-sans-italic both wrap each atom in `Strong(Emph(..))`; the sans
        // component has no inline form of its own. Sans-italic is plain italic.
        "mathbfit" | "mathbfsfit" => styled(arg, Style::BoldItalic),
        "mathsfit" => styled(arg, Style::Italic),
        // Each atom of the argument is styled individually, preserving the atom-class spacing
        // between them and recursing the style into any scripts.
        // `\symbf` and the upright-bold variants set letters bold without an upright wrapper of their
        // own in inline output, the same shape as `\mathbf`.
        "mathbf" | "boldsymbol" | "bm" | "pmb" | "symbf" | "mathbfup" | "mathbfsfup" => {
            styled(arg, Style::Bold)
        }
        "mathit" => styled(arg, Style::Italic),
        // The math-upright styles keep full math spacing (binary ops and relations are spaced, a
        // comma keeps its trailing space) while setting letters upright. The explicitly-upright
        // spellings (`\mathup`, `\mathsfup`) join the serif/sans uprights.
        "mathrm" | "mathsf" | "mathup" | "mathsfup" => styled(arg, Style::Upright),
        // Monospace renders each character as its own code span.
        "mathtt" => styled(arg, Style::Monospace),
        // A math-class wrapper re-classes its single-atom argument and sets it upright, the way a
        // one-symbol ordinary atom is typeset. A multi-atom argument is left to verbatim fallback.
        "mathord" => math_class_wrapper(arg, Class::Ord),
        "mathrel" => math_class_wrapper(arg, Class::Rel),
        "mathbin" => math_class_wrapper(arg, Class::Bin),
        "mathpunct" => math_class_wrapper(arg, Class::Punct),
        "mathopen" => math_class_wrapper(arg, Class::Open),
        "mathclose" => math_class_wrapper(arg, Class::Close),
        // A `\mathop` argument is set like a named operator (the operator class re-types a following
        // binary sign so it is not spaced). A multi-atom run also takes operator spacing of its own;
        // a single atom does not.
        "mathop" if arg.len() == 1 => {
            Some((flatten_emph(upright_inner(arg)?), Class::Op, false, false))
        }
        "mathop" => Some((flatten_emph(upright_inner(arg)?), Class::Op, true, true)),
        _ => None,
    }
}

/// Lower a style wrapper's argument under `style`, threading the style through each atom and into
/// any scripts. The whole wrapper is an ordinary atom for spacing.
fn styled(arg: &[Atom], style: Style) -> Option<(Vec<Inline>, Class, bool, bool)> {
    let inner = lower_styled(arg, style)?;
    Some((inner, Class::Ord, false, false))
}

/// Render a math-class wrapper (`\mathord`, `\mathbin`, …) carrying the wrapper's spacing class. A
/// single-symbol argument is set upright, the way a one-symbol ordinary atom is typeset; a multi-atom
/// argument keeps its ordinary (italic) rendering.
fn math_class_wrapper(arg: &[Atom], class: Class) -> Option<(Vec<Inline>, Class, bool, bool)> {
    let inlines = if arg.len() == 1 {
        flatten_emph(upright_inner(arg)?)
    } else {
        lower(arg)?
    };
    Some((inlines, class, false, false))
}

fn upright_inner(arg: &[Atom]) -> Option<Vec<Inline>> {
    let mut out = Vec::new();
    for atom in arg {
        if atom.sub.is_some() || atom.sup.is_some() {
            return None;
        }
        match &atom.body {
            Body::Char(c) => out.push(Inline::Str(c.to_string().into())),
            Body::Number(digits) => out.push(Inline::Str(digits.clone().into())),
            Body::Command(name) => {
                if let Some((letter, _)) = symbols::greek(name) {
                    out.push(Inline::Str(letter.to_string().into()));
                } else if let Some(sym) = symbols::symbol(name) {
                    out.push(Inline::Str(sym.text.to_string().into()));
                } else {
                    return None;
                }
            }
            Body::Group(inner) => out.extend(upright_inner(inner)?),
            _ => return None,
        }
    }
    Some(out)
}

/// Collapse a single `Emph`-wrapped run into a bare `Str` run (used where a style is upright).
fn flatten_emph(inlines: Vec<Inline>) -> Vec<Inline> {
    inlines
        .into_iter()
        .map(|i| match i {
            Inline::Emph(inner) => {
                if let [Inline::Str(s)] = inner.as_slice() {
                    Inline::Str(s.clone())
                } else {
                    Inline::Emph(inner)
                }
            }
            other => other,
        })
        .collect()
}

fn italic(text: String) -> Inline {
    Inline::Emph(vec![Inline::Str(text.into())])
}

/// The flat prime glyph string for a run of `count` apostrophes. A run of one to four marks is a
/// single, double, triple, or quadruple prime; a longer run decomposes greedily into `⌊count/4⌋`
/// quadruple-prime glyphs followed by one remainder glyph for the `count mod 4` marks left over.
fn prime_glyph(count: u8) -> String {
    let quads = (count / 4) as usize;
    let mut out = "\u{2057}".repeat(quads);
    out.push_str(match count % 4 {
        1 => "\u{2032}",
        2 => "\u{2033}",
        3 => "\u{2034}",
        // A run that is an exact multiple of four (including the empty run) needs no remainder glyph;
        // a count of zero is a degenerate single prime with no quads.
        _ if count == 0 => "\u{2032}",
        _ => "",
    });
    out
}

/// The inline run for a prime mark that surfaces as a bare nucleus. Up to four marks set the flat
/// prime glyph directly; a longer run sets the first quadruple-prime as the nucleus glyph and lifts
/// the remaining `count - 4` marks into a superscript, which itself decomposes the same way.
fn prime_nucleus(count: u8) -> Vec<Inline> {
    if count <= 4 {
        return vec![Inline::Str(prime_glyph(count).into())];
    }
    vec![
        Inline::Str("\u{2057}".into()),
        Inline::Superscript(vec![Inline::Str(prime_glyph(count - 4).into())]),
    ]
}

fn push_str(out: &mut Vec<Inline>, text: &str) {
    if !text.is_empty() {
        out.push(Inline::Str(text.to_string().into()));
    }
}

/// The space emitted immediately before an atom of class `class`, given its neighbours' classes.
/// The class has already been through the effective-class retyping pass, so a binary operator that
/// reaches here always has a left operand.
fn leading_space(class: Class, _left: Option<Class>, right: Option<Class>) -> Option<&'static str> {
    match class {
        Class::Rel if right.is_some() => Some(REL_SPACE),
        Class::Bin if right.is_some() => Some(BIN_SPACE),
        _ => None,
    }
}

/// The space emitted immediately after an atom of class `class`, given its neighbours' classes.
fn trailing_space(class: Class, left: Option<Class>, right: Option<Class>) -> Option<&'static str> {
    match class {
        Class::Rel if right.is_some() => Some(REL_SPACE),
        Class::Bin if right.is_some() => Some(BIN_SPACE),
        Class::Punct
            if right.is_some_and(|r| r != Class::Punct)
                && !matches!(left, Some(Class::Open | Class::Punct)) =>
        {
            Some(THIN_SPACE)
        }
        _ => None,
    }
}

/// Merge consecutive `Str` inlines so the output groups runs the way a hand-written tree would.
/// A run ending in a literal backslash is kept separate from what follows: writers that form escape
/// sequences pair a backslash with its successor, so a glyph like `\` (set-minus) must not absorb a
/// trailing space into the same run.
fn merge_adjacent_str(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for inline in inlines {
        if let Inline::Str(s) = &inline
            && let Some(Inline::Str(prev)) = out.last_mut()
            && !prev.ends_with('\\')
        {
            prev.push_str(s);
            continue;
        }
        out.push(inline);
    }
    out
}
