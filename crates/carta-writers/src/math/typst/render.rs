//! Lowering of structural math bodies (delimiters, matrices, grids, fractions, accents) to Typst markup.

use super::super::parse::{Atom, Body, Delim, GridKind, MatrixDelim, ModKind, StackSide};
use super::super::symbols;
use super::lookup::glyph_typst;
use super::{is_atomic_script, is_lone_ascii_symbol, lower, lower_loose, wrap_script};

pub(super) fn char_str(c: char) -> String {
    // escaped so Typst treats them as literal symbols that attach without spacing
    match c {
        // the slash is escaped so it reads as division, not the fraction operator
        '(' | ')' | '[' | ']' | '|' | ',' | ';' | '/' => format!("\\{c}"),
        '\'' => "'".to_string(),
        // An active tie renders as a medium space.
        '~' => "med".to_string(),
        // a raw Unicode math glyph maps to its named Typst token, or renders verbatim if none
        c => glyph_typst(c).map_or_else(|| c.to_string(), ToString::to_string),
    }
}

/// Render a stack (`\overset`/`\underset`/`\stackrel`) as a script of the mark on the base.
#[allow(clippy::similar_names)]
pub(super) fn stack_str(
    display: bool,
    side: StackSide,
    mark: &[Atom],
    base: &[Atom],
) -> Option<String> {
    let base_str = lower(display, base)?;
    let mark_lowered = lower(display, mark)?;
    // single-token marks (dotted ones like `tilde.op` included) stand alone; anything else is
    // parenthesised so it attaches as one unit
    let bare =
        mark.len() == 1 && is_atomic_script(&mark_lowered) && !is_lone_ascii_symbol(&mark_lowered);
    let mark_str = if bare {
        mark_lowered
    } else {
        format!("({mark_lowered})")
    };
    let op = match side {
        StackSide::Over => '^',
        StackSide::Under => '_',
    };
    Some(format!("{base_str}{op}{mark_str}"))
}

/// Render an extensible arrow as the arrow glyph with its labels as scripts: the `[below]` label is a
/// subscript and the `{above}` label a superscript.
pub(super) fn ext_arrow_str(
    display: bool,
    arrow: &str,
    below: Option<&[Atom]>,
    above: &[Atom],
) -> Option<String> {
    let sub = match below {
        Some(below) => format!("_{}", wrap_script(below, &lower(display, below)?)),
        None => String::new(),
    };
    let sup = format!("^{}", wrap_script(above, &lower(display, above)?));
    Some(format!("{arrow}{sub}{sup}"))
}

/// Render an aligned/cases/substack grid. `cases` becomes Typst's `cases(..)` with every cell a
/// comma-separated argument; `aligned` and `substack` join cells with ` & ` and rows with a trailing
/// `\` and a line break (`substack` has a single cell per row).
pub(super) fn grid_str(display: bool, kind: GridKind, rows: &[Vec<Vec<Atom>>]) -> Option<String> {
    match kind {
        GridKind::Cases => {
            // all-single-cell rows render as a bare `{` plus alignment-style rows; any multi-cell
            // row keeps `cases(..)` so each cell becomes an argument
            if rows.iter().all(|row| row.len() == 1) {
                let mut row_strs = Vec::new();
                for row in rows {
                    let cell = row.first()?;
                    row_strs.push(lower(display, cell)?);
                }
                return Some(format!("{{{}", row_strs.join("\\\n")));
            }
            let mut row_strs = Vec::new();
            for row in rows {
                let mut cells = Vec::new();
                for cell in row {
                    cells.push(lower(display, cell)?);
                }
                row_strs.push(cells.join(" & "));
            }
            Some(format!("cases(delim: \"{{\", {})", row_strs.join(", ")))
        }
        GridKind::Aligned
        | GridKind::Array
        | GridKind::Substack
        | GridKind::Gathered
        | GridKind::Eqnarray
        | GridKind::Flalign => {
            let mut row_strs = Vec::new();
            for row in rows {
                let mut cells = Vec::new();
                for cell in row {
                    cells.push(lower(display, cell)?);
                }
                row_strs.push(cells.join(" & "));
            }
            Some(row_strs.join("\\\n"))
        }
    }
}

pub(super) fn matrix_str(
    display: bool,
    delim: MatrixDelim,
    rows: &[Vec<Vec<Atom>>],
) -> Option<String> {
    // an undelimited matrix renders as a bare alignment; a delimited one wraps in `mat`
    let delim = match delim {
        MatrixDelim::Paren => "(",
        MatrixDelim::Bracket => "[",
        MatrixDelim::Brace => "{",
        MatrixDelim::Bar | MatrixDelim::DoubleBar => "||",
        MatrixDelim::None => return bare_matrix_str(display, rows),
    };
    delimited_matrix_str(display, delim, rows)
}

/// Render a grid of cells bracketed by `delim` (a Typst bracket string such as `"("`, `"["`, or
/// `"|"`). A parenthesised grid whose every row holds exactly one cell is a column vector, written
/// with Typst's `vec`; every other case is a `mat` carrying the explicit `delim` argument. Cells join
/// with commas and rows with `;`; a leading empty cell takes the explicit `#none` placeholder.
fn delimited_matrix_str(display: bool, delim: &str, rows: &[Vec<Vec<Atom>>]) -> Option<String> {
    if delim == "(" && rows.iter().all(|row| row.len() == 1) {
        let mut cells = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let cell = row.first()?;
            cells.push(matrix_cell_str(display, cell, i == 0)?);
        }
        return Some(format!("vec({})", cells.join(", ")));
    }
    let mut row_strs = Vec::new();
    for row in rows {
        let mut cells = Vec::new();
        for (i, cell) in row.iter().enumerate() {
            cells.push(matrix_cell_str(display, cell, i == 0)?);
        }
        row_strs.push(cells.join(", "));
    }
    Some(format!("mat(delim: \"{delim}\", {})", row_strs.join("; ")))
}

/// Lower a single matrix/vector cell. An empty cell in a leading position renders as Typst's explicit
/// `#none` placeholder, since a bare empty token there would form an invalid leading separator; any
/// other cell renders as its lowered content (empty or not).
fn matrix_cell_str(display: bool, cell: &[Atom], leading: bool) -> Option<String> {
    let lowered = lower(display, cell)?;
    if leading && lowered.is_empty() {
        return Some("#none".to_string());
    }
    Some(lowered)
}

/// The Typst function for a horizontal brace.
pub(super) fn brace_func(kind: super::super::parse::BraceKind) -> &'static str {
    match kind {
        super::super::parse::BraceKind::Over => "overbrace",
        super::super::parse::BraceKind::Under => "underbrace",
    }
}

/// Render a horizontal brace. The brace's matching script (a superscript over an over-brace, a
/// subscript under an under-brace) becomes the label argument, but only when the opposite script is
/// absent; when both scripts are present neither is a label and both render as ordinary Typst
/// scripts after the brace.
pub(super) fn brace_str(
    display: bool,
    kind: super::super::parse::BraceKind,
    inner: &[Atom],
    atom: &Atom,
) -> Option<String> {
    use super::super::parse::BraceKind;
    let content = lower(display, inner)?;
    let func = brace_func(kind);
    let superscript_labels = matches!(kind, BraceKind::Over) && atom.sub.is_none();
    let subscript_labels = matches!(kind, BraceKind::Under) && atom.sup.is_none();
    if superscript_labels && let Some(label) = atom.sup.as_deref() {
        return Some(format!("{func}({content}, {})", lower(display, label)?));
    }
    if subscript_labels && let Some(label) = atom.sub.as_deref() {
        return Some(format!("{func}({content}, {})", lower(display, label)?));
    }
    let mut out = format!("{func}({content})");
    if let Some(script) = atom.sub.as_deref() {
        out.push('_');
        out.push_str(&wrap_script(script, &lower(display, script)?));
    }
    if let Some(script) = atom.sup.as_deref() {
        out.push('^');
        out.push_str(&wrap_script(script, &lower(display, script)?));
    }
    Some(out)
}

/// Render an undelimited matrix as a bare alignment block: cells joined by ` & ` and rows joined by
/// a trailing `\` followed by a line break.
fn bare_matrix_str(display: bool, rows: &[Vec<Vec<Atom>>]) -> Option<String> {
    let mut row_strs = Vec::new();
    for row in rows {
        let mut cells = Vec::new();
        for cell in row {
            cells.push(lower(display, cell)?);
        }
        row_strs.push(cells.join(" & "));
    }
    Some(row_strs.join("\\\n"))
}

/// Fuse a matched `\left … \right` pair around a single bare grid into one `mat`/`vec`, or `None`
/// when no fusion applies. The pair must open and close with the same bracket from the fusing set
/// (paren, square bracket, curly brace, single bar, double bar; angle, floor, ceil, and the null
/// `.` delimiter do not fuse), and its only content must be one unscripted grid that carries no
/// delimiter of its own: a bare `matrix` or an `aligned`/`array`-family grid. A `cases` block keeps
/// its own braces, and an already-bracketed matrix (`pmatrix` …) keeps its own delimiter, so neither
/// fuses. The pair's delimiter then drives the grid's `mat`/`vec` rendering.
fn fused_grid_str(
    display: bool,
    open: Option<Delim>,
    close: Option<Delim>,
    content: &[Atom],
) -> Option<String> {
    let open = open?;
    if Some(open) != close {
        return None;
    }
    let delim = fusing_delim_str(open)?;
    let atom = sole_unscripted_atom(content)?;
    // peel the transparent group a `\begin{…}` splice adds around the grid
    let atom = match &atom.body {
        Body::Group(inner) => sole_unscripted_atom(inner)?,
        _ => atom,
    };
    let (Body::Matrix(MatrixDelim::None, rows)
    | Body::Grid(
        GridKind::Aligned
        | GridKind::Array
        | GridKind::Gathered
        | GridKind::Eqnarray
        | GridKind::Flalign,
        _,
        rows,
    )) = &atom.body
    else {
        return None;
    };
    delimited_matrix_str(display, delim, rows)
}

/// Whether a group holds exactly one unscripted grid or matrix atom, the shape a `\begin{…}`
/// environment splice takes. Such a group is transparent: its grid renders as a self-contained block
/// with no added brackets.
pub(super) fn is_environment_group(inner: &[Atom]) -> bool {
    sole_unscripted_atom(inner)
        .is_some_and(|atom| matches!(atom.body, Body::Matrix(..) | Body::Grid(..)))
}

/// The single atom of a run that carries no scripts, or `None` when the run is empty, holds more than
/// one atom, or the lone atom carries a sub/superscript.
fn sole_unscripted_atom(atoms: &[Atom]) -> Option<&Atom> {
    let [atom] = atoms else { return None };
    if atom.sub.is_some() || atom.sup.is_some() || !atom.siblings.is_empty() {
        return None;
    }
    Some(atom)
}

/// The Typst bracket string a `\left`/`\right` delimiter contributes when fusing with a bare grid, or
/// `None` for a delimiter outside the fusing set. A single bar fuses as `|` and a double bar as `||`,
/// distinct from the bar-typed matrix environments which always render `||`.
fn fusing_delim_str(delim: Delim) -> Option<&'static str> {
    Some(match delim {
        Delim::Paren => "(",
        Delim::Bracket => "[",
        Delim::Brace => "{",
        Delim::Bar => "|",
        Delim::BarVert | Delim::DoubleBar => "||",
        Delim::Angle
        | Delim::Floor
        | Delim::Ceil
        | Delim::CornerUpperLeft
        | Delim::CornerUpperRight => return None,
    })
}

/// The raw glyph a delimiter prints inside an `lr(..)` wrapper around a mismatched pair, or `None`
/// for a delimiter outside the pairing set (angle, floor, ceil, double bar). The bar prints as a
/// bare `|` rather than the escaped `\|` it takes when standing alone.
fn lr_pair_glyph(delim: Delim, side: DelimSide) -> Option<&'static str> {
    Some(match (delim, side) {
        (Delim::Paren, DelimSide::Open) => "(",
        (Delim::Paren, DelimSide::Close) => ")",
        (Delim::Bracket, DelimSide::Open) => "[",
        (Delim::Bracket, DelimSide::Close) => "]",
        (Delim::Brace, DelimSide::Open) => "{",
        (Delim::Brace, DelimSide::Close) => "}",
        (Delim::Bar, _) => "|",
        _ => return None,
    })
}

pub(super) fn delimited_str(
    display: bool,
    open: Option<Delim>,
    close: Option<Delim>,
    content: &[Atom],
) -> Option<String> {
    // a grid sole inside a matched `\left … \right` pair fuses into one `mat`/`vec`, the pair's
    // delimiter supplying the bracket
    if let Some(fused) = fused_grid_str(display, open, close, content) {
        return Some(fused);
    }
    let inner = lower_loose(display, content)?;
    // mismatched sides cannot auto-pair: escape lone parens/brackets so Typst prints them verbatim
    // (distinct paired delimiters are pinned in `lr(..)` below; this covers the remaining mismatches)
    let unpaired = open != close;
    let o = open.map_or("", |d| one_sided_paren(d, DelimSide::Open, unpaired));
    let c = close.map_or("", |d| one_sided_paren(d, DelimSide::Close, unpaired));
    if open == Some(Delim::Bar) && close == Some(Delim::Bar) {
        // Inside `lr(..)` the stretchy bars are bare, not escaped.
        return Some(format!("lr(|{inner}|)"));
    }
    if open == Some(Delim::DoubleBar) && close == Some(Delim::DoubleBar) {
        // A balanced double bar stretches to its content as the named double-line glyph.
        return Some(format!("lr(bar.v.double {inner} bar.v.double)"));
    }
    // two distinct paired delimiters are pinned with `lr(..)`, raw glyphs; mismatches involving
    // angle/floor/ceil/double-bar/`.` fall through to the direct-glyph path below
    if open != close
        && let (Some(o), Some(c)) = (open, close)
        && let (Some(og), Some(cg)) = (
            lr_pair_glyph(o, DelimSide::Open),
            lr_pair_glyph(c, DelimSide::Close),
        )
    {
        return Some(format!("lr({og}{inner}{cg})"));
    }
    if has_colliding_middle(open, close, content) {
        return Some(format!("lr({o}{inner}{c})"));
    }
    // Each delimiter is a single glyph that attaches directly to the content with no space.
    Some(format!("{o}{inner}{c}"))
}

/// Whether an interior `\middle` reuses the same auto-pairing bracket kind (paren, bracket, brace)
/// as the group's open or close delimiter. Typst auto-matches those bracket glyphs, so a duplicate
/// one written as a divider would pair with the wrong glyph; the group must then be pinned with an
/// explicit `lr(..)`.
fn has_colliding_middle(open: Option<Delim>, close: Option<Delim>, content: &[Atom]) -> bool {
    let outer_brackets: Vec<Delim> = [open, close]
        .into_iter()
        .flatten()
        .filter(|d| is_auto_pair(*d))
        .collect();
    if outer_brackets.is_empty() {
        return false;
    }
    content.iter().any(|atom| {
        matches!(&atom.body, Body::Middle(Some(d), _) if is_auto_pair(*d) && outer_brackets.contains(d))
    })
}

/// Whether a delimiter is one Typst auto-pairs (a paren, square bracket, or curly brace).
fn is_auto_pair(delim: Delim) -> bool {
    matches!(delim, Delim::Paren | Delim::Bracket | Delim::Brace)
}

/// Render a `\middle<delim>` divider as a stretchy `mid(<name>)` call. An absent delimiter (`.`)
/// yields an empty `mid()`. The name is the delimiter's Typst symbol; a one-sided delimiter takes
/// its left or right form from the side that was written.
pub(super) fn middle_str(delim: Option<Delim>, open_side: bool) -> String {
    let name = delim.map_or("", |d| middle_delim_name(d, open_side));
    format!("mid({name})")
}

/// The Typst symbol name a `\middle` delimiter carries, by side. These are the named delimiter
/// glyphs (`paren.l`, `bracket.r`, `bar.v`, …), distinct from the literal characters a `\left`/
/// `\right` delimiter prints.
fn middle_delim_name(delim: Delim, open_side: bool) -> &'static str {
    match (delim, open_side) {
        (Delim::Paren, true) => "paren.l",
        (Delim::Paren, false) => "paren.r",
        (Delim::Bracket, true) => "bracket.l",
        (Delim::Bracket, false) => "bracket.r",
        (Delim::Brace, true) => "brace.l",
        (Delim::Brace, false) => "brace.r",
        (Delim::Bar, _) => "bar.v",
        (Delim::BarVert, _) => "parallel",
        (Delim::DoubleBar, _) => "bar.v.double",
        (Delim::Angle, true) => "chevron.l",
        (Delim::Angle, false) => "chevron.r",
        (Delim::Floor, true) => "floor.l",
        (Delim::Floor, false) => "floor.r",
        (Delim::Ceil, true) => "ceil.l",
        (Delim::Ceil, false) => "ceil.r",
        (Delim::CornerUpperLeft, _) => "corner.l.t",
        (Delim::CornerUpperRight, _) => "corner.r.t",
    }
}

#[derive(Clone, Copy)]
enum DelimSide {
    Open,
    Close,
}

/// Render a modulo operator in Typst. The `mod` forms set the operator word `mod` off with spaces;
/// the parenthesised forms wrap their argument in escaped parentheses, and `\pod` omits the word.
pub(super) fn mod_str(display: bool, kind: ModKind, arg: Option<&[Atom]>) -> Option<String> {
    match kind {
        ModKind::Bmod => Some("med mod med".to_string()),
        ModKind::Mod => Some("#h(0em) mod med".to_string()),
        ModKind::Pmod => {
            let inner = lower(display, arg?)?;
            Some(format!("med\\(mod med {inner}\\)"))
        }
        ModKind::Pod => {
            let inner = lower(display, arg?)?;
            Some(format!("med\\({inner}\\)"))
        }
    }
}

/// Render a `\not`-negated base in Typst, branching on the base's atom class. A relation uses its
/// dedicated negated token or carries a combining long solidus on its bare glyph; a letter, Greek
/// letter, or delimiter base sets the struck glyph as an upright string literal; a digit sets it
/// bare. Any operator, ordinary symbol, or punctuation base returns `None` (verbatim fallback).
pub(super) fn negated_str(base: &str) -> Option<String> {
    if symbols::is_unnegatable(base) {
        return None;
    }
    if let Some(token) = super::super::symbols::negated_relation_typst(base) {
        return Some(token.to_string());
    }
    match negated_base_typst(base)? {
        NegatedToken::Relation(glyph) | NegatedToken::Bare(glyph) => {
            Some(format!("{glyph}\u{0338}"))
        }
        NegatedToken::Quoted(glyph) => Some(format!("\"{glyph}\u{0338}\"")),
    }
}

/// How a `\not` base lowers to Typst, with its plain (unstruck) glyph.
enum NegatedToken {
    /// A relation base: a bare glyph carrying the combining solidus.
    Relation(String),
    /// A letter, Greek letter, or delimiter base: an upright string literal of the struck glyph.
    Quoted(String),
    /// A digit base: the struck glyph written bare.
    Bare(String),
}

/// Classify a `\not` base for Typst and resolve its plain glyph, or `None` when the base carries no
/// meaningful strike-through.
fn negated_base_typst(base: &str) -> Option<NegatedToken> {
    let mut chars = base.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        if first.is_ascii_digit() {
            return Some(NegatedToken::Bare(first.to_string()));
        }
        if first.is_alphabetic() {
            return Some(NegatedToken::Quoted(first.to_string()));
        }
        return match first {
            '=' | '<' | '>' | ':' => Some(NegatedToken::Relation(first.to_string())),
            '(' | ')' | '[' | ']' | '|' => Some(NegatedToken::Quoted(first.to_string())),
            _ => None,
        };
    }
    if let Some((glyph, _)) = symbols::greek(base) {
        return Some(NegatedToken::Quoted(glyph.to_string()));
    }
    let sym = super::super::symbols::symbol(base)?;
    if sym.class == symbols::Class::Rel {
        return Some(NegatedToken::Relation(sym.text.to_string()));
    }
    if sym.italic {
        return Some(NegatedToken::Quoted(sym.text.to_string()));
    }
    None
}

/// The Typst markup for a delimiter glyph, escaping a paren or bracket that stands unpaired. A
/// matching same-kind pair (`unpaired` false) keeps the bare glyph so Typst stretches and matches it;
/// a glyph written opposite a null or unrelated delimiter is escaped so Typst prints it as a literal
/// rather than auto-pairing it with the wrong neighbour.
fn one_sided_paren(delim: Delim, side: DelimSide, unpaired: bool) -> &'static str {
    if unpaired {
        match (delim, side) {
            (Delim::Paren, DelimSide::Open) => return "\\(",
            (Delim::Paren, DelimSide::Close) => return "\\)",
            (Delim::Bracket, DelimSide::Open) => return "\\[",
            (Delim::Bracket, DelimSide::Close) => return "\\]",
            _ => {}
        }
    }
    typst_delim(delim, side)
}

fn typst_delim(delim: Delim, side: DelimSide) -> &'static str {
    match (delim, side) {
        (Delim::Paren, DelimSide::Open) => "(",
        (Delim::Paren, DelimSide::Close) => ")",
        (Delim::Bracket, DelimSide::Open) => "[",
        (Delim::Bracket, DelimSide::Close) => "]",
        (Delim::Brace, DelimSide::Open) => "{",
        (Delim::Brace, DelimSide::Close) => "}",
        (Delim::Bar, _) => "\\|",
        (Delim::BarVert | Delim::DoubleBar, _) => "\u{2225}",
        // balanced double bars render via `lr(bar.v.double …)`; this glyph covers unbalanced sides
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

pub(super) fn frac_str(display: bool, num: &[Atom], den: &[Atom]) -> Option<String> {
    let n = lower(display, num)?;
    let d = lower(display, den)?;
    // inline slash only when both operands are single atoms; multi-atom operands keep `frac(.., ..)`
    // so grouping stays unambiguous
    if is_inline_frac_operand(num) && is_inline_frac_operand(den) {
        Some(format!("{n} / {d}"))
    } else {
        Some(format!("frac({n}, {d})"))
    }
}

/// Whether a fraction operand is a single atom and so renders with an inline slash. A single
/// nucleus (variable, symbol, group, accent, or nested fraction), with or without scripts, qualifies.
fn is_inline_frac_operand(atoms: &[Atom]) -> bool {
    atoms.len() == 1
}
