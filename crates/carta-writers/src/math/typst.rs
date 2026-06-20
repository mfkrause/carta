//! Backend B: lower a parsed math tree to Typst math markup (the inner content, with no surrounding
//! `$` delimiters). Typst has native math, so this translation is far more total than the inline
//! lowering: it returns `None` only for genuinely untranslatable commands.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use super::parse::{
    Atom, BinomKind, Body, Delim, GridKind, MatrixDelim, ModKind, ScriptKind, StackSide, TextPiece,
};
use super::symbols::{self, Alphabet};

/// Codepoint-to-Typst-name map for raw Unicode math glyphs written directly in the source. It is
/// the inverse of the forward symbol/Greek tables: each command's Unicode rendering (from
/// [`symbols::symbol`]/[`symbols::greek`]) maps to that command's Typst name (from [`SYMBOL_TYPST`]/
/// [`GREEK_TYPST`]). A glyph whose Typst name is itself the same raw glyph carries no real token, so
/// it is skipped — the backend then emits it verbatim, which is correct. The double-struck capitals
/// (`ℝ` → `RR`, …) map through their styled-alphabet form. Built once from an ordered iteration into
/// a `BTreeMap`, so the result is deterministic.
static GLYPH_TYPST: LazyLock<BTreeMap<char, &'static str>> = LazyLock::new(build_glyph_typst);

fn build_glyph_typst() -> BTreeMap<char, &'static str> {
    let mut map = BTreeMap::new();
    let mut insert = |glyph: &str, typst: &'static str| {
        // Only single-codepoint glyphs that map to a distinct Typst token are reversible; a Typst
        // name equal to the glyph itself (a passthrough) would be a no-op and is dropped.
        let mut chars = glyph.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && glyph != typst
        {
            map.entry(c).or_insert(typst);
        }
    };
    for (name, typst) in SYMBOL_TYPST {
        if let Some(sym) = symbols::symbol(name) {
            insert(sym.text, typst);
        }
    }
    for (name, typst) in GREEK_TYPST {
        if let Some((glyph, _)) = symbols::greek(name) {
            insert(glyph, typst);
        }
    }
    // The double-struck capitals reverse to their doubled-letter Typst form (`ℝ` → `RR`). Only the
    // uppercase block is named this way; the lowercase and digit double-struck glyphs stay verbatim.
    for (letter, name) in (b'A'..=b'Z').zip(DOUBLE_STRUCK_NAMES) {
        let upper = letter as char;
        if let Some(glyph) = symbols::styled_letter(Alphabet::DoubleStruck, upper)
            && let Some(c) = glyph.chars().next()
            && glyph.chars().nth(1).is_none()
        {
            map.entry(c).or_insert(name);
        }
    }
    map
}

/// The Typst name for each double-struck capital `A`…`Z`: the letter doubled.
const DOUBLE_STRUCK_NAMES: [&str; 26] = [
    "AA", "BB", "CC", "DD", "EE", "FF", "GG", "HH", "II", "JJ", "KK", "LL", "MM", "NN", "OO", "PP",
    "QQ", "RR", "SS", "TT", "UU", "VV", "WW", "XX", "YY", "ZZ",
];

/// The Typst name for a raw Unicode math glyph written directly in the source, or `None` when the
/// glyph carries no dedicated Typst token (it is then emitted verbatim).
fn glyph_typst(c: char) -> Option<&'static str> {
    GLYPH_TYPST.get(&c).copied()
}

/// How adjacent pieces bind when joined. At the top level escaped punctuation (`\(`, `\,`, …) binds
/// tightly to its neighbours with no space; inside a `\left … \right` group that same punctuation is
/// set off with spaces, so the content of such a group is lowered loosely.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spacing {
    Tight,
    Loose,
}

/// Lower a list of atoms to a Typst math string, joining adjacent atoms with a single space where
/// Typst requires one to keep tokens distinct.
pub(super) fn lower(display: bool, atoms: &[Atom]) -> Option<String> {
    lower_with(display, atoms, Spacing::Tight)
}

/// Lower an atom list to Typst markup together with the first equation `\label` it carries (searched
/// depth-first through grid cells), formatted as its trailing Typst reference token. The label is
/// dropped from the body itself; the caller appends it after the closing `$`. With `display` set,
/// primes on limit operators stack as superscripts the way display math sets them.
pub(super) fn lower_labeled(atoms: &[Atom], display: bool) -> Option<(String, Option<String>)> {
    let body = lower(display, atoms)?;
    let label = first_label(atoms).map(super::parse::format_label);
    Some((body, label))
}

/// The verbatim name of the first equation `\label` in an atom list, searched depth-first so a label
/// inside a grid cell is found, or `None` when the expression carries none.
fn first_label(atoms: &[Atom]) -> Option<&str> {
    for atom in atoms {
        if let Body::Label(name) = &atom.body {
            return Some(name);
        }
        if let Some(name) = label_in_body(&atom.body) {
            return Some(name);
        }
    }
    None
}

/// Search a body's nested atom lists (the cells of a grid or matrix) for an equation `\label`.
fn label_in_body(body: &Body) -> Option<&str> {
    let (Body::Grid(_, rows) | Body::Matrix(_, rows)) = body else {
        return None;
    };
    rows.iter().flatten().find_map(|cell| first_label(cell))
}

/// Lower a list of atoms, setting escaped punctuation off with spaces. Used for the content of a
/// `\left … \right` group, where Typst spaces such punctuation away from its neighbours.
fn lower_loose(display: bool, atoms: &[Atom]) -> Option<String> {
    lower_with(display, atoms, Spacing::Loose)
}

fn lower_with(display: bool, atoms: &[Atom], spacing: Spacing) -> Option<String> {
    let mut pieces = Vec::new();
    for atom in atoms {
        // A captured equation `\label` carries no body glyph and must not occupy a spacing slot — it
        // is lifted to a trailing reference label by the entry point — so it is skipped entirely here.
        if matches!(atom.body, Body::Label(_)) {
            continue;
        }
        pieces.push(atom_str(display, atom)?);
    }
    Some(join(&pieces, spacing))
}

/// Join rendered atom pieces. Typst separates almost every adjacent pair with a space to keep them
/// as distinct atoms; the exceptions are runs of digits forming one number, attached primes, and
/// parentheses.
fn join(pieces: &[String], spacing: Spacing) -> String {
    let mut out = String::new();
    for (i, piece) in pieces.iter().enumerate() {
        if i > 0
            && let Some(prev) = pieces.get(i - 1)
            && needs_space(prev, piece, spacing)
        {
            out.push(' ');
        }
        out.push_str(piece);
    }
    out
}

/// Whether a space is needed between two adjacent rendered pieces.
fn needs_space(left: &str, right: &str, spacing: Spacing) -> bool {
    // An empty piece is a bare spacing slot (a standalone `{}`): it keeps a space on the side that
    // faces real content so the slot stays visible.
    let (Some(_), Some(r)) = (left.chars().next_back(), right.chars().next()) else {
        return true;
    };
    // Primes attach to their base.
    if r == '\'' {
        return false;
    }
    // Escaped delimiters and punctuation (`\(`, `\)`, `\,`, …) attach to their neighbours at the top
    // level; inside a delimited group they are set off with spaces instead.
    if spacing == Spacing::Tight && (ends_tight(left) || starts_tight(right)) {
        return false;
    }
    true
}

/// Characters that, when backslash-escaped, render as a literal symbol that binds tightly to its
/// neighbours with no surrounding space: delimiters, punctuation, and escaped operators.
fn is_tight_escape(c: char) -> bool {
    matches!(
        c,
        '(' | ')' | '[' | ']' | '|' | ',' | ';' | '/' | '\\' | '$' | '#' | '_' | '{' | '}'
    )
}

/// Whether a piece is a bare escaped tight symbol (no surrounding space wanted). The piece must be
/// exactly the two-character escape: a larger piece that merely ends or starts with an escaped
/// delimiter (e.g. a `\pod{..}` group rendered `med\(..\)`) is a compound atom and spaces normally.
fn is_bare_tight(piece: &str) -> bool {
    let mut chars = piece.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some('\\'), Some(c), None) if is_tight_escape(c)
    )
}

/// Whether a piece is a scripted escaped tight delimiter: a bare two-character escape (`\)`, `\]`, …)
/// carrying its scripts or primes (`\)^2`, `\)_n`, `\)'`). The escaped delimiter is the base of the
/// script, so the whole piece is one tight atom that binds to its neighbours on both sides without a
/// space, just as the bare delimiter would.
fn is_scripted_tight(piece: &str) -> bool {
    let mut chars = piece.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some('\\'), Some(c), Some('^' | '_' | '\'')) if is_tight_escape(c)
    )
}

/// Whether a piece ends with a bare escaped tight symbol (no trailing space wanted). A scripted
/// escaped delimiter is one tight atom and so binds tightly on its trailing side as well.
fn ends_tight(piece: &str) -> bool {
    is_bare_tight(piece) || is_scripted_tight(piece)
}

/// Whether a piece starts with a bare escaped tight symbol (no leading space wanted). A scripted
/// escaped delimiter binds tightly on its leading side: the escaped glyph is the script's base.
fn starts_tight(piece: &str) -> bool {
    is_bare_tight(piece) || is_scripted_tight(piece)
}

fn is_number_char(c: char) -> bool {
    c.is_ascii_digit() || c == '.'
}

#[allow(clippy::similar_names)]
fn atom_str(display: bool, atom: &Atom) -> Option<String> {
    // A horizontal brace consumes its matching script (a superscript for an over-brace, a subscript
    // for an under-brace) as the brace's label argument rather than as a Typst script.
    if let Body::Brace(kind, inner) = &atom.body {
        return brace_str(display, *kind, inner, atom);
    }
    let has_scripts = atom.sub.is_some() || atom.sup.is_some() || !atom.siblings.is_empty();
    let mut out = match &atom.body {
        // A synthesized empty nucleus (a leading `_`/`^`) renders as Typst's empty content.
        Body::Empty => "\"\"".to_string(),
        // A bare prime nucleus renders as literal prime marks.
        Body::Prime(count) => "'".repeat(*count as usize),
        // An explicit empty group renders as a zero-width space when it carries scripts, and as
        // nothing (a bare spacing slot) when it stands alone.
        Body::EmptyGroup if has_scripts => "zws".to_string(),
        Body::EmptyGroup => String::new(),
        body => nucleus_str(display, body)?,
    };
    // On a limit operator in display context a prime sets as a stacked superscript above the operator
    // (`\sum'` → `sum^(')`) rather than as a literal `'` beside it. The prime then renders through the
    // ordinary superscript path below — kept in its natural slot after any subscript — instead of
    // being pulled ahead as a literal mark.
    let stack_prime = display && is_limit_op_body(&atom.body);
    // Typst attaches at most one subscript and one superscript per base. The scripts are sequenced
    // into render runs (each reordered subscript-before-superscript); within a run a slot reuse, and
    // at the start of a flagged restart run, emits a fresh empty base (`""`). A superscript that is a
    // prime attaches as a literal `'` on the current base rather than as a `^` script, and a prime in
    // the primary superscript slot sets ahead of the subscript, the way TeX sets primes first.
    let primary_sup_is_prime = !stack_prime && atom.sup.as_deref().and_then(prime_script).is_some();
    if primary_sup_is_prime && let Some(count) = atom.sup.as_deref().and_then(prime_script) {
        for _ in 0..count {
            out.push('\'');
        }
    }
    for run in &atom.script_runs() {
        if run.restart {
            out.push_str(" \"\"");
        }
        let mut sub_used = false;
        let mut sup_used = false;
        for script in &run.scripts {
            // The primary prime superscript was already emitted ahead of the subscript above; skip it
            // here so it is not rendered twice.
            if primary_sup_is_prime
                && script.kind == ScriptKind::Sup
                && !run.restart
                && prime_script(script.atoms).is_some()
            {
                sup_used = true;
                continue;
            }
            push_typst_script(
                display,
                &mut out,
                script.kind,
                script.atoms,
                &mut sub_used,
                &mut sup_used,
                stack_prime,
            )?;
        }
    }
    Some(out)
}

/// Whether a base body is a large operator that stacks its scripts above and below in display math
/// (`\sum`, `\prod`, the big set/logic operators, and the named limit functions `\lim`, `\max`, …,
/// including their direct-glyph spellings). On such a base in display context a prime sets as a
/// stacked superscript (`\sum'` → `sum^(')`) rather than a literal `'` set beside the operator; the
/// side-script operators (`\int`, `\oint`, `\bigoplus`, …) are excluded and keep the literal prime.
fn is_limit_op_body(body: &Body) -> bool {
    match body {
        Body::Command(name) => {
            symbols::is_limit_operator(name)
                || matches!(symbols::named_function(name), Some((_, true)))
        }
        Body::Char(c) => symbols::is_limit_glyph(*c),
        _ => false,
    }
}

/// The prime count of a script group that is exactly a single prime atom (a literal `'` run or a
/// `\prime` command), if it is one. A `\prime` superscript collapses to a single postfix prime mark.
fn prime_script(group: &[Atom]) -> Option<u8> {
    match group {
        [
            Atom {
                body: Body::Prime(count),
                sub: None,
                sup: None,
                siblings,
                limits: None,
            },
        ] if siblings.is_empty() => Some(*count),
        [
            Atom {
                body: Body::Command(name),
                sub: None,
                sup: None,
                siblings,
                limits: None,
            },
        ] if siblings.is_empty() && name == "prime" => Some(1),
        _ => None,
    }
}

/// Append one Typst script of `kind` to `out`. A kind already filled on the current base forces a
/// fresh empty base (`""`) so Typst does not see two subscripts or two superscripts on one atom.
/// With `stack_prime` set (a limit operator in display context), a prime superscript sets as a
/// stacked `^(')` script rather than a literal `'` mark beside the base.
#[allow(clippy::similar_names, clippy::too_many_arguments)]
fn push_typst_script(
    display: bool,
    out: &mut String,
    kind: ScriptKind,
    group: &[Atom],
    sub_used: &mut bool,
    sup_used: &mut bool,
    stack_prime: bool,
) -> Option<()> {
    // A superscript that is exactly a prime mark attaches as a literal `'` on the current base
    // rather than as a `^` script, and does not consume the superscript slot — except on a limit
    // operator in display context, where it sets as a stacked `^(')` script through the path below.
    if !stack_prime
        && kind == ScriptKind::Sup
        && let Some(count) = prime_script(group)
    {
        for _ in 0..count {
            out.push('\'');
        }
        *sup_used = true;
        return Some(());
    }
    let already_used = match kind {
        ScriptKind::Sub => *sub_used,
        ScriptKind::Sup => *sup_used,
    };
    if already_used {
        out.push_str(" \"\"");
        *sub_used = false;
        *sup_used = false;
    }
    match kind {
        ScriptKind::Sub => *sub_used = true,
        ScriptKind::Sup => *sup_used = true,
    }
    out.push(match kind {
        ScriptKind::Sub => '_',
        ScriptKind::Sup => '^',
    });
    out.push_str(&wrap_script(group, &lower(display, group)?));
    Some(())
}

/// Wrap a script's content in parentheses unless it is a single bare token. A single atom whose
/// rendering is a bare run needs none; a digit run (`10`) is one number and needs none; any other
/// multi-atom script is compound and is parenthesised even when its rendering has no space (e.g.
/// `i,j` renders `i\,j`). A script that reduces to a single literal ASCII symbol glyph is
/// parenthesised so it reads as the script content rather than as adjacent markup.
fn wrap_script(atoms: &[Atom], inner: &str) -> String {
    let bare = (atoms.len() <= 1 && is_atomic_script(inner) && !is_lone_ascii_symbol(inner))
        || is_number_run(inner);
    if bare {
        inner.to_string()
    } else {
        format!("({inner})")
    }
}

/// Whether a rendered script is a single literal ASCII symbol glyph: one ASCII punctuation character
/// (other than the decimal point), or a backslash-escaped one (`\#`, `\,`, `\|`, …). A multi-letter
/// identifier (`alpha`, `sum`, `arrow.r`), a digit, a letter, the decimal point, or a non-ASCII
/// glyph is not a lone ASCII symbol and stays bare.
#[allow(clippy::match_same_arms)]
fn is_lone_ascii_symbol(s: &str) -> bool {
    let symbol = |c: char| c.is_ascii_punctuation() && c != '.';
    let mut chars = s.chars();
    match (chars.next(), chars.next(), chars.next()) {
        // A single bare symbol, or a backslash-escaped one (the symbol is the second character).
        (Some(c), None, None) => symbol(c),
        (Some('\\'), Some(c), None) => symbol(c),
        _ => false,
    }
}

/// Whether a rendered script is a plain run of digits (and decimal points) forming one number.
fn is_number_run(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_number_char)
}

/// Whether a script body needs no parentheses: a bare run with no space, no further scripting, no
/// attached prime (a primed base such as `2'` is parenthesised so the prime stays inside), and no
/// function-call shape. A rendering containing `(` or `"` is a Typst function call (`sqrt(2)`,
/// `hat(a)`, `upright("map")`) whose argument list would otherwise bind only its first token to the
/// script, so it is parenthesised.
fn is_atomic_script(s: &str) -> bool {
    !s.is_empty()
        && !s.chars().any(char::is_whitespace)
        && !s.contains('^')
        && !s.contains('_')
        && !s.contains('\'')
        && !s.contains('(')
        && !s.contains('"')
}

/// Whether a rendered string is a single Typst atom (no spaces, balanced enough to stand alone).
fn is_single_token(s: &str) -> bool {
    !s.contains(' ') && !s.is_empty()
}

#[allow(clippy::match_same_arms)]
fn nucleus_str(display: bool, body: &Body) -> Option<String> {
    match body {
        // The empty nuclei are rendered by `atom_str`, which sees the surrounding scripts.
        Body::Empty | Body::EmptyGroup => None,
        Body::Prime(count) => Some("'".repeat(*count as usize)),
        // A bare (unescaped) TeX-active character — `#`, `&`, or `%` — has no ordinary-symbol meaning
        // in math, so the expression falls back to verbatim. An alignment-separator `&` is consumed by
        // the grid parser and never reaches here; the escaped forms still convert.
        Body::Char('#' | '&' | '%') => None,
        Body::Char(c) => Some(char_str(*c)),
        // The `:=` digraph prints as the two literal characters joined as one piece, so they stay
        // tight to each other while the piece spaces normally against its neighbours.
        Body::ColonEq => Some(":=".to_string()),
        Body::Number(digits) => Some(digits.clone()),
        Body::Command(name) => command_str(name),
        Body::Group(inner) => {
            let s = lower(display, inner)?;
            // A `\begin{…}` environment parses as a transparent single-atom group around its grid; the
            // grid is already a self-contained `mat`/`vec`/bare block, so the group adds no brackets.
            // An ordinary group with multiple tokens stays grouped with parentheses; a single token
            // does not need them.
            if is_environment_group(inner) || is_single_token(&s) {
                Some(s)
            } else {
                Some(format!("({s})"))
            }
        }
        Body::Accent(name, base) => {
            let inner = lower(display, base)?;
            // `\overline`/`\underline` always use their named function. Other accents use their
            // named function over a single atom, but the generic `accent(content, mark)` form when
            // the base spans several atoms.
            if matches!(name.as_str(), "overline" | "underline") {
                let func = accent_func(name)?;
                return Some(format!("{func}({inner})"));
            }
            // An accent with a named Typst function uses it over a single atom; without one, or over
            // a multi-atom base, the generic `accent(content, mark)` form applies.
            if is_single_token(&inner)
                && let Some(func) = accent_func(name)
            {
                Some(format!("{func}({inner})"))
            } else {
                let mark = accent_mark(name)?;
                Some(format!("accent({inner}, {mark})"))
            }
        }
        Body::Styled(name, arg) => styled_str(display, name, arg),
        Body::Text(name, content) => text_str(display, name, content),
        Body::Binom(kind, top, bottom) => {
            let t = lower(display, top)?;
            let b = lower(display, bottom)?;
            match kind {
                BinomKind::Paren => Some(format!("binom({t}, {b})")),
                BinomKind::Brace => Some(format!("{{{t} / {b}}}")),
                BinomKind::Brack => Some(format!("[{t} / {b}]")),
            }
        }
        // A fixed-size delimiter scales its bare glyph by the wrapper's percentage. A run of five or
        // more sized primes scales only the first quadruple-prime; the remaining marks set after the
        // box as literal primes, the way the inline nucleus lifts them into a superscript.
        Body::Big(scale, delim) => {
            if let Body::Prime(count) = delim.body
                && count > 4
            {
                let tail = "'".repeat((count - 4) as usize);
                return Some(format!("#scale(x: {scale}%, y: {scale}%)['''']{tail}"));
            }
            let inner = nucleus_str(display, &delim.body)?;
            Some(format!("#scale(x: {scale}%, y: {scale}%)[{inner}]"))
        }
        Body::Stack(side, mark, base) => stack_str(display, *side, mark, base),
        Body::Grid(kind, rows) => grid_str(display, *kind, rows),
        Body::ExtArrow(arrow, below, above) => {
            ext_arrow_str(display, arrow, below.as_deref(), above)
        }
        Body::Matrix(delim, rows) => matrix_str(display, *delim, rows),
        Body::Delimited(open, close, content) => delimited_str(display, *open, *close, content),
        Body::Middle(delim, open_side) => Some(middle_str(*delim, *open_side)),
        Body::Mod(kind, arg) => mod_str(display, *kind, arg.as_deref()),
        Body::Negated(base) => negated_str(base),
        // A `\not` over a braced group overlays the combining long solidus on the lowered content
        // through Typst's generic accent form.
        Body::NegatedGroup(inner) => {
            let content = lower(display, inner)?;
            Some(format!("accent({content}, \u{0338})"))
        }
        // A label-less brace reached without a surrounding atom (e.g. inside a script). The labelled
        // form is produced by `atom_str`, which can see the brace's matching script.
        Body::Brace(kind, inner) => {
            let content = lower(display, inner)?;
            Some(format!("{}({content})", brace_func(*kind)))
        }
        Body::Frac(num, den) => frac_str(display, num, den),
        Body::Sqrt(index, radicand) => {
            let inner = lower(display, radicand)?;
            match index {
                Some(idx) => {
                    let i = lower(display, idx)?;
                    Some(format!("root({i}, {inner})"))
                }
                None => Some(format!("sqrt({inner})")),
            }
        }
        // A captured equation `\label` has no glyph in the body: it is lifted to a trailing reference
        // label after the closing `$` by the caller, so here it contributes the empty string.
        Body::Label(_) => Some(String::new()),
    }
}

fn char_str(c: char) -> String {
    // Delimiters and punctuation are backslash-escaped so Typst treats them as literal symbols and
    // attaches them to their neighbours without spacing.
    match c {
        // A literal slash is escaped so Typst reads it as a division glyph rather than as its
        // fraction operator.
        '(' | ')' | '[' | ']' | '|' | ',' | ';' | '/' => format!("\\{c}"),
        '\'' => "'".to_string(),
        // An active tie renders as a medium space.
        '~' => "med".to_string(),
        // A raw Unicode math glyph (`α`, `∑`, `≤`, `ℝ`, …) written directly in the source maps to
        // its named Typst token; a glyph with no token renders verbatim.
        c => glyph_typst(c).map_or_else(|| c.to_string(), ToString::to_string),
    }
}

/// Render a stack (`\overset`/`\underset`/`\stackrel`) as a script of the mark on the base.
#[allow(clippy::similar_names)]
fn stack_str(display: bool, side: StackSide, mark: &[Atom], base: &[Atom]) -> Option<String> {
    let base_str = lower(display, base)?;
    let mark_lowered = lower(display, mark)?;
    // A single-token mark stands alone; anything else (an empty, escaped, function-call, or
    // multi-token mark) is parenthesised so it attaches as one unit. A mark that lowers to a single
    // dotted Typst token (`\sim` → `tilde.op`, `\to` → `arrow.r`) is one atom and needs no
    // parentheses, so the predicate tolerates the dot the way regular scripts do.
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
fn ext_arrow_str(
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
fn grid_str(display: bool, kind: GridKind, rows: &[Vec<Vec<Atom>>]) -> Option<String> {
    match kind {
        GridKind::Cases => {
            // A `cases` block whose every row is a single cell (no `&` alignment) renders as a bare
            // brace: an opening `{` followed by the rows joined the way an alignment grid joins them,
            // with a trailing `\` and a line break. A block with any multi-cell row keeps the
            // `cases(..)` function so each cell becomes a comma-separated argument.
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
        GridKind::Aligned | GridKind::Substack => {
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

fn matrix_str(display: bool, delim: MatrixDelim, rows: &[Vec<Vec<Atom>>]) -> Option<String> {
    // An undelimited matrix has no bracketing, so it renders as a bare alignment: cells separated by
    // ` & `, rows by a trailing `\` and a line break. A delimited environment wraps the grid in
    // Typst's `mat`.
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
            // The first cell of each row takes `#none` when empty; later cells stay bare so the
            // alignment keeps its column count.
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
fn brace_func(kind: super::parse::BraceKind) -> &'static str {
    match kind {
        super::parse::BraceKind::Over => "overbrace",
        super::parse::BraceKind::Under => "underbrace",
    }
}

/// Render a horizontal brace. The brace's matching script — a superscript over an over-brace, a
/// subscript under an under-brace — becomes the label argument, but only when the opposite script is
/// absent; when both scripts are present neither is a label and both render as ordinary Typst
/// scripts after the brace.
fn brace_str(
    display: bool,
    kind: super::parse::BraceKind,
    inner: &[Atom],
    atom: &Atom,
) -> Option<String> {
    use super::parse::BraceKind;
    let content = lower(display, inner)?;
    let func = brace_func(kind);
    // The matching script is the label only when the opposite script is absent. Otherwise both
    // scripts stay as ordinary Typst scripts on the brace.
    let superscript_labels = matches!(kind, BraceKind::Over) && atom.sub.is_none();
    let subscript_labels = matches!(kind, BraceKind::Under) && atom.sup.is_none();
    if superscript_labels && let Some(label) = atom.sup.as_deref() {
        return Some(format!("{func}({content}, {})", lower(display, label)?));
    }
    if subscript_labels && let Some(label) = atom.sub.as_deref() {
        return Some(format!("{func}({content}, {})", lower(display, label)?));
    }
    // No label applies: both scripts (when present) render as ordinary Typst scripts on the brace.
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

/// Render a `\left … \right` group. When both delimiters are present single bars, the group is
/// wrapped in `lr(..)` so Typst stretches them; otherwise each present delimiter prints its glyph
/// directly around the content and an absent (`.`) delimiter contributes nothing.
///
/// A bare paren/bracket/brace pair stretches under Typst's own matching, so it needs no wrapper —
/// unless an interior `\middle` divider reuses that same bracket kind. The duplicate auto-pairing
/// glyph would then mis-match, so the whole group is wrapped in an explicit `lr(..)` to pin the
/// stretch to the written delimiters.
/// Fuse a matched `\left … \right` pair around a single bare grid into one `mat`/`vec`, or `None`
/// when no fusion applies. The pair must open and close with the same bracket from the fusing set
/// (paren, square bracket, curly brace, single bar, double bar — angle, floor, ceil, and the null
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
    // A `\begin{…}` reached inside `\left … \right` parses as a transparent single-atom group around
    // the grid, so the group is peeled before the grid is matched.
    let atom = match &atom.body {
        Body::Group(inner) => sole_unscripted_atom(inner)?,
        _ => atom,
    };
    let (Body::Matrix(MatrixDelim::None, rows) | Body::Grid(GridKind::Aligned, rows)) = &atom.body
    else {
        return None;
    };
    delimited_matrix_str(display, delim, rows)
}

/// Whether a group holds exactly one unscripted grid or matrix atom — the shape a `\begin{…}`
/// environment splice takes. Such a group is transparent: its grid renders as a self-contained block
/// with no added brackets.
fn is_environment_group(inner: &[Atom]) -> bool {
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

fn delimited_str(
    display: bool,
    open: Option<Delim>,
    close: Option<Delim>,
    content: &[Atom],
) -> Option<String> {
    // A grid that is the sole content of a matched `\left … \right` pair fuses into the grid's own
    // `mat`/`vec` form, the pair's delimiter supplying the bracket — `\left( \begin{matrix} … \right)`
    // becomes one `mat(delim: "(", …)`, with no separate brackets around it.
    if let Some(fused) = fused_grid_str(display, open, close, content) {
        return Some(fused);
    }
    let inner = lower_loose(display, content)?;
    // A paren or bracket auto-pairs only with a matching same-kind partner. When the two sides differ
    // — one written as `.`, or as an unrelated delimiter — each paren/bracket stands alone as a literal
    // glyph and is escaped so Typst prints it verbatim rather than auto-pairing it with the wrong
    // neighbour. (Distinct paired delimiters are pinned in `lr(..)` just below; this escaping governs
    // the direct-glyph path for the remaining mismatches.)
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
    // A pair of distinct paired delimiters (a paren, bracket, brace, or single bar opposite a
    // different one of those) cannot auto-pair, so the group is pinned with `lr(..)` and each
    // delimiter prints as its raw glyph. Mismatches that involve an angle, floor, ceil, double bar,
    // or a null `.` side fall through to the direct-glyph path below.
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
fn middle_str(delim: Option<Delim>, open_side: bool) -> String {
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
fn mod_str(display: bool, kind: ModKind, arg: Option<&[Atom]>) -> Option<String> {
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
fn negated_str(base: &str) -> Option<String> {
    if symbols::is_unnegatable(base) {
        return None;
    }
    if let Some(token) = super::symbols::negated_relation_typst(base) {
        return Some(token.to_string());
    }
    match negated_base_typst(base)? {
        // A relation glyph and a bare digit both carry the combining solidus written directly; only a
        // letter/Greek/delimiter base is wrapped in an upright string literal.
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
    let sym = super::symbols::symbol(base)?;
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
        // A balanced `\Vert`/`\|` pair renders in `delimited_str` via `lr(bar.v.double …)`; the
        // glyph above is only reached for an unbalanced or single-sided double bar.
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

fn frac_str(display: bool, num: &[Atom], den: &[Atom]) -> Option<String> {
    let n = lower(display, num)?;
    let d = lower(display, den)?;
    // A fraction renders with an inline slash only when each operand is a single atom (one nucleus,
    // possibly scripted, possibly itself a slash-fraction). A multi-atom operand — a sum, a digit
    // run with an operator, or a delimited group — keeps the explicit `frac(.., ..)` so grouping
    // stays unambiguous.
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

#[allow(clippy::match_same_arms)]
fn styled_str(display: bool, name: &str, arg: &[Atom]) -> Option<String> {
    let inner = lower(display, arg)?;
    let s = match name {
        "mathbb" | "mathds" => format!("bb({inner})"),
        "mathcal" | "mathscr" => format!("cal({inner})"),
        "mathfrak" => format!("frak({inner})"),
        "mathbf" => format!("upright(bold({inner}))"),
        "boldsymbol" | "bm" | "symbf" | "mathbfup" => format!("bold({inner})"),
        "mathit" => format!("italic({inner})"),
        "mathsf" | "mathsfup" => format!("sans({inner})"),
        "mathbfsfup" => format!("bold(sans({inner}))"),
        "mathtt" => format!("mono({inner})"),
        // Composed styled alphabets nest the Typst style functions.
        "mathbfit" => format!("bold(italic({inner}))"),
        "mathsfit" => format!("italic(sans({inner}))"),
        "mathbfsfit" => format!("bold(italic(sans({inner})))"),
        "mathbfcal" | "mathbfscr" => format!("bold(cal({inner}))"),
        "mathbffrak" => format!("bold(frak({inner}))"),
        "mathrm" | "mathup" => format!("upright({inner})"),
        "pmb" => format!("bold({inner})"),
        // Math-class wrappers re-class their argument but add no glyph: the content renders directly.
        "mathord" | "mathrel" | "mathbin" | "mathopen" | "mathclose" | "mathpunct" => inner,
        // `\mathop` sets its argument as an operator: a multi-letter run becomes a known operator's
        // bare identifier or, failing that, a quoted operator name; anything else renders directly.
        "mathop" => match operator_name(arg) {
            Some(name) if super::symbols::named_function(&name).is_some() => name,
            Some(name) => format!("\"{name}\""),
            None => inner,
        },
        "phantom" => format!("#hide[{inner}]"),
        "cancel" => format!("cancel({inner})"),
        "xcancel" => format!("cancel({inner}, cross: #true)"),
        "bcancel" => format!("cancel({inner}, inverted: #true)"),
        "boxed" => format!("#box(stroke: black, inset: 3pt, [$ {inner} $])"),
        // A horizontal paren is set as a script over or under its content.
        "overparen" => format!("{inner}^paren.t"),
        "underparen" => format!("{inner}_paren.b"),
        _ => return None,
    };
    Some(s)
}

/// A run of plain letters joined into a single Typst identifier (e.g. `lim`), used for an operator
/// name. Returns `None` if any atom is not a bare letter, leaving the caller to fall back.
fn operator_name(atoms: &[Atom]) -> Option<String> {
    if atoms.len() < 2 {
        return None;
    }
    let mut out = String::new();
    for atom in atoms {
        if atom.sub.is_some() || atom.sup.is_some() || !atom.siblings.is_empty() {
            return None;
        }
        match &atom.body {
            Body::Char(c) if c.is_ascii_alphabetic() => out.push(*c),
            _ => return None,
        }
    }
    Some(out)
}

fn text_str(display: bool, name: &str, content: &[TextPiece]) -> Option<String> {
    // `\operatorname` folds any spacing into its single run, so it renders as one identifier or quoted
    // string over the whole content. Every other wrapper applies its formatting to each run, with the
    // spacing emitted as a Typst spacing token between them.
    if name == "operatorname" {
        let text = text_run_text(content);
        let s = if super::symbols::named_function(&text).is_some() {
            text
        } else {
            format!("\"{}\"", escape_typst_string(&text))
        };
        return Some(s);
    }
    let wrapper = match name {
        "text" | "textrm" | "mbox" => "upright",
        "textbf" => "bold",
        "textit" => "italic",
        "texttt" => "mono",
        "textsf" => "sans",
        _ => return None,
    };
    let mut parts: Vec<String> = Vec::new();
    for piece in content {
        match piece {
            TextPiece::Run(run) if run.is_empty() => {
                // An empty segment (an empty inner brace group) carries no formatted text but still
                // occupies a join position, so it contributes an empty token rather than a wrapper.
                parts.push(String::new());
            }
            TextPiece::Run(run) => {
                parts.push(format!("{wrapper}(\"{}\")", escape_typst_string(run)));
            }
            TextPiece::Space(space) => parts.push(space.typst_token().to_string()),
            // A `$…$` segment is math, rendered unaffected by the wrapper's own formatting.
            TextPiece::Math(atoms) => parts.push(lower(display, atoms)?),
        }
    }
    // An empty wrapper still renders as the empty quoted form, matching a bare `\text{}`.
    if parts.is_empty() {
        return Some(format!("{wrapper}(\"\")"));
    }
    Some(parts.join(" "))
}

/// The concatenated literal text of a run sequence, with each spacing rendered as its codepoint. Used
/// for `\operatorname`, whose spacing folds into the single identifier rather than splitting it.
fn text_run_text(content: &[TextPiece]) -> String {
    let mut out = String::new();
    for piece in content {
        match piece {
            TextPiece::Run(run) => out.push_str(run),
            TextPiece::Space(space) => out.push(space.codepoint()),
            // A `$…$` cannot occur in an operator-name group, which is already math mode.
            TextPiece::Math(_) => {}
        }
    }
    out
}

/// Escape a literal string for inclusion in a Typst quoted string: backslash and double-quote.
fn escape_typst_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn accent_func(name: &str) -> Option<&'static str> {
    let f = match name {
        "bar" => "macron",
        "hat" | "widehat" => "hat",
        "tilde" | "widetilde" => "tilde",
        "vec" | "overrightarrow" => "arrow",
        "overleftarrow" => "arrow.l",
        "dot" => "dot",
        "ddot" => "dot.double",
        "check" => "caron",
        "breve" => "breve",
        "acute" => "acute",
        "grave" => "grave",
        "mathring" => "circle",
        "overline" => "overline",
        "underline" => "underline",
        "overleftrightarrow" => "arrow.l.r",
        _ => return None,
    };
    Some(f)
}

/// The combining mark used by Typst's generic `accent(content, mark)` form for a multi-atom base.
fn accent_mark(name: &str) -> Option<char> {
    let m = match name {
        "bar" => '\u{203E}',
        "hat" | "widehat" => '\u{0302}',
        "tilde" | "widetilde" => '\u{0303}',
        "vec" | "overrightarrow" => '\u{20D7}',
        "overleftarrow" => '\u{20D6}',
        "dot" => '\u{0307}',
        "ddot" => '\u{0308}',
        "check" => '\u{030C}',
        "breve" => '\u{0306}',
        "acute" => '\u{0301}',
        "grave" => '\u{0300}',
        "mathring" => '\u{030A}',
        "dddot" => '\u{20DB}',
        "ddddot" => '\u{20DC}',
        "overleftrightarrow" => '\u{20E1}',
        "underleftarrow" => '\u{20EE}',
        "underrightarrow" => '\u{20EF}',
        _ => return None,
    };
    Some(m)
}

fn command_str(name: &str) -> Option<String> {
    if let Some(s) = spacing_str(name) {
        return Some(s.to_string());
    }
    if let Some(g) = greek_str(name) {
        return Some(g.to_string());
    }
    if let Some(f) = function_str(name) {
        return Some(f.to_string());
    }
    typst_symbol(name).map(ToString::to_string)
}

#[allow(clippy::match_same_arms)]
fn spacing_str(name: &str) -> Option<&'static str> {
    let s = match name {
        "," => "thin",
        ":" | ">" | " " => "med",
        ";" => "#h(0em)",
        "!" => "#h(-1em)",
        "medspace" => "space.med",
        "enspace" => "#h(0em)",
        "quad" => "quad",
        "qquad" => "#h(2em)",
        _ => return None,
    };
    Some(s)
}

fn greek_str(name: &str) -> Option<&'static str> {
    GREEK_TYPST
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
}

fn function_str(name: &str) -> Option<&'static str> {
    let f = match name {
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "cot" => "cot",
        "sec" => "sec",
        "csc" => "csc",
        "arcsin" => "arcsin",
        "arccos" => "arccos",
        "arctan" => "arctan",
        "sinh" => "sinh",
        "cosh" => "cosh",
        "tanh" => "tanh",
        "coth" => "coth",
        "log" => "log",
        "ln" => "ln",
        "lg" => "lg",
        "exp" => "exp",
        "deg" => "deg",
        "dim" => "dim",
        "hom" => "hom",
        "ker" => "ker",
        "arg" => "arg",
        "lim" => "lim",
        "limsup" => "limsup",
        "liminf" => "liminf",
        "max" => "max",
        "min" => "min",
        "sup" => "sup",
        "inf" => "inf",
        "det" => "det",
        "gcd" => "gcd",
        "Pr" => "Pr",
        _ => return None,
    };
    Some(f)
}

/// The Typst name for a TeX math symbol command.
fn typst_symbol(name: &str) -> Option<&'static str> {
    SYMBOL_TYPST
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
}

/// The Typst name for each TeX math symbol command, as an iterable table. Both the forward
/// lookup [`typst_symbol`] and the codepoint-to-Typst reverse map ([`super::symbols`]) read
/// from this single source, so the two directions can never drift apart.
pub(super) const SYMBOL_TYPST: &[(&str, &str)] = &[
    ("Aries", "\u{2648}"),
    ("Bbbk", "\u{1D55C}"),
    ("Box", "square.stroked"),
    ("Bumpeq", "\u{224E}"),
    ("Cap", "inter.double"),
    ("CheckedBox", "ballot.check"),
    ("Colon", "colon.double"),
    ("Cup", "union.double"),
    ("DD", "\u{2145}"),
    ("Diamond", "diamond.stroked"),
    ("Doteq", "eq.dots"),
    ("Downarrow", "arrow.b.double"),
    ("Finv", "\u{2132}"),
    ("Game", "\u{2141}"),
    ("Gemini", "\u{264A}"),
    ("Im", "Im"),
    ("Join", "\u{22C8}"),
    ("Leftarrow", "arrow.l.double"),
    ("Leftrightarrow", "arrow.l.r.double"),
    ("Leo", "\u{264C}"),
    ("Libra", "\u{264E}"),
    ("Lleftarrow", "arrow.l.triple"),
    ("Longleftarrow", "arrow.l.double"),
    ("Longleftrightarrow", "arrow.l.r.double"),
    ("Longrightarrow", "arrow.r.double"),
    ("Lsh", "\u{21B0}"),
    ("Pluto", "\u{2647}"),
    ("Re", "Re"),
    ("Rightarrow", "arrow.r.double"),
    ("Rrightarrow", "arrow.r.triple"),
    ("Rsh", "\u{21B1}"),
    ("Scorpio", "\u{264F}"),
    ("Square", "ballot"),
    ("Subset", "subset.double"),
    ("Sun", "sun"),
    ("Supset", "supset.double"),
    ("Taurus", "\u{2649}"),
    ("Uparrow", "arrow.t.double"),
    ("Updownarrow", "arrow.t.b.double"),
    ("Vdash", "forces"),
    ("Vert", "bar.v.double"),
    ("Vvdash", "\u{22AA}"),
    ("XBox", "ballot.cross"),
    ("aleph", "\u{2135}"),
    ("amalg", "product.co"),
    ("anchor", "\u{2693}"),
    ("angle", "angle"),
    ("approx", "approx"),
    ("approxeq", "approx.eq"),
    ("approxident", "tilde.triple"),
    ("ast", "*"),
    ("astrosun", "sun"),
    ("asymp", "asymp"),
    ("backcong", "tilde.rev.equiv"),
    ("backepsilon", "epsilon.alt.rev"),
    ("backprime", "prime.rev"),
    ("backsim", "tilde.rev"),
    ("backsimeq", "tilde.eq.rev"),
    ("backslash", "without"),
    ("barwedge", "\u{2305}"),
    ("because", "because"),
    ("beth", "\u{2136}"),
    ("between", "\u{226C}"),
    ("bigcap", "inter.big"),
    ("bigcirc", "circle.stroked"),
    ("bigcup", "union.big"),
    ("bigcupdot", "union.dot.big"),
    ("bigodot", "dot.o.big"),
    ("bigoplus", "xor.big"),
    ("bigotimes", "times.o.big"),
    ("bigsqcap", "inter.sq.big"),
    ("bigsqcup", "union.sq.big"),
    ("bigtimes", "times.big"),
    ("bigstar", "star.filled"),
    ("bigtriangledown", "triangle.stroked.b"),
    ("bigtriangleup", "triangle.stroked.t"),
    ("biguplus", "union.plus.big"),
    ("bigvee", "or.big"),
    ("bigwedge", "and.big"),
    ("biohazard", "\u{2623}"),
    ("blacklozenge", "lozenge.filled.medium"),
    ("blacksmiley", "\u{263B}"),
    ("blacksquare", "square.filled.medium"),
    ("blacktriangleleft", "triangle.filled.small.l"),
    ("blacktriangleright", "triangle.filled.small.r"),
    ("bot", "tack.t"),
    ("bowtie", "\u{22C8}"),
    ("boxdot", "dot.square"),
    ("boxminus", "minus.square"),
    ("boxplus", "plus.square"),
    ("boxtimes", "times.square"),
    ("bullet", "bullet"),
    ("bumpeq", "\u{224F}"),
    ("cap", "inter"),
    ("cdot", "dot.op"),
    ("cdotp", "dot.c"),
    ("cdots", "dots.h.c"),
    ("centerdot", "\u{2B1D}"),
    ("checkmark", "checkmark"),
    ("circeq", "\u{2257}"),
    ("circlearrowleft", "arrow.ccw"),
    ("circlearrowright", "arrow.cw"),
    ("circledR", "trademark.registered"),
    ("circledast", "convolve.o"),
    ("circledcirc", "compose.o"),
    ("circledequal", "eq.o"),
    ("circledparallel", "parallel.o"),
    ("circ", "compose"),
    ("clubsuit", "suit.club.filled"),
    ("varclubsuit", "suit.club.stroked"),
    ("varheartsuit", "suit.heart.filled"),
    ("vardiamondsuit", "suit.diamond.filled"),
    ("varspadesuit", "suit.spade.stroked"),
    ("rightmoon", "\u{263D}"),
    ("leftmoon", "\u{263E}"),
    ("sun", "\u{263C}"),
    ("colon", ":"),
    ("coloneqq", "colon.eq"),
    ("eqqcolon", "eq.colon"),
    ("Coloneqq", "colon.double.eq"),
    ("coloneq", "colon.eq"),
    ("Coloneq", "colon.double.eq"),
    ("eqcolon", "eq.colon"),
    ("notni", "in.rev.not"),
    ("leqq", "lt.equiv"),
    ("geqq", "gt.equiv"),
    ("lneqq", "lt.nequiv"),
    ("gneqq", "gt.nequiv"),
    ("gtreqqless", "\u{2A8C}"),
    ("lesseqqgtr", "\u{2A8B}"),
    ("complement", "complement"),
    ("cong", "tilde.equiv"),
    ("coprod", "product.co"),
    ("cup", "union"),
    ("curlyeqprec", "eq.prec"),
    ("curlyeqsucc", "eq.succ"),
    ("curlyvee", "or.curly"),
    ("curlywedge", "and.curly"),
    ("curvearrowleft", "arrow.ccw.half"),
    ("curvearrowright", "arrow.cw.half"),
    ("dag", "dagger"),
    ("dagger", "dagger"),
    ("daleth", "\u{2138}"),
    ("dashcolon", "dash.colon"),
    ("dashleftarrow", "arrow.l.dashed"),
    ("dashrightarrow", "arrow.r.dashed"),
    ("dashv", "tack.l"),
    ("ddag", "dagger.double"),
    ("dd", "\u{2146}"),
    ("ddagger", "dagger.double"),
    ("ddots", "dots.down"),
    ("diamond", "diamond.stroked.small"),
    ("diamondsuit", "suit.diamond.stroked"),
    ("digamma", "digamma"),
    ("div", "div"),
    ("divideontimes", "times.div"),
    ("doteq", "\u{2250}"),
    ("doteqdot", "eq.dots"),
    ("dotminus", "minus.dot"),
    ("dots", "dots.h"),
    ("dotsb", "dots.h.c"),
    ("dotsc", "dots.h"),
    ("dotsi", "dots.h.c"),
    ("dotsm", "dots.h.c"),
    ("dotso", "dots.h"),
    ("doublebarwedge", "\u{2306}"),
    ("downarrow", "arrow.b"),
    ("downdownarrows", "arrows.bb"),
    ("downharpoonleft", "harpoon.bl"),
    ("downharpoonright", "harpoon.br"),
    ("earth", "earth.alt"),
    ("eighthnote", "note.eighth.alt"),
    ("ee", "\u{2147}"),
    ("ell", "ell"),
    ("emptyset", "nothing"),
    ("eqcirc", "\u{2256}"),
    ("eqgtr", "eq.gt"),
    ("eqless", "eq.lt"),
    ("eqsim", "minus.tilde"),
    ("eqslantgtr", "\u{2A96}"),
    ("eqslantless", "\u{2A95}"),
    ("equiv", "equiv"),
    ("eth", "\u{F0}"),
    ("euro", "euro"),
    ("exists", "exists"),
    ("fallingdotseq", "eq.dots.down"),
    ("female", "venus"),
    ("flat", "flat"),
    ("forall", "forall"),
    ("frown", "frown"),
    ("frownie", "\u{2639}"),
    ("ge", "gt.eq"),
    ("geq", "gt.eq"),
    ("geqslant", "gt.eq"),
    ("gescc", "\u{2AA9}"),
    ("gets", "arrow.l"),
    ("gg", "gt.double"),
    ("ggg", "gt.triple"),
    ("gimel", "\u{2137}"),
    ("gnapprox", "gt.napprox"),
    ("gneq", "gt.neq"),
    ("gnsim", "gt.ntilde"),
    ("gtrdot", "gt.dot"),
    ("gtreqless", "gt.eq.lt"),
    ("gtrless", "gt.lt"),
    ("gtrsim", "gt.tilde"),
    ("hbar", "\u{210F}"),
    ("hdots", "dots.h"),
    ("heartsuit", "suit.heart.stroked"),
    ("hookleftarrow", "arrow.l.hook"),
    ("hookrightarrow", "arrow.r.hook"),
    ("hslash", "\u{210F}"),
    ("iff", "arrow.l.r.double"),
    ("iiint", "integral.triple"),
    ("iint", "integral.double"),
    ("ii", "\u{2148}"),
    ("imath", "dotless.i"),
    ("impliedby", "arrow.l.double.long"),
    ("implies", "arrow.r.double.long"),
    ("in", "in"),
    ("infty", "oo"),
    ("int", "integral"),
    ("intercal", "\u{22BA}"),
    ("jj", "\u{2149}"),
    ("jmath", "dotless.j"),
    ("jupiter", "jupiter"),
    ("lAngle", "chevron.l.double"),
    ("lBrace", "brace.l.stroked"),
    ("lVert", "parallel"),
    ("land", "and"),
    ("langle", "chevron.l"),
    ("lbrace", "{"),
    ("lbrack", "\\["),
    ("lceil", "ceil.l"),
    ("ldots", "dots.h"),
    ("le", "lt.eq"),
    ("leadsto", "\u{2933}"),
    ("leftarrow", "arrow.l"),
    ("leftharpoondown", "harpoon.lb"),
    ("leftharpoonup", "harpoon.lt"),
    ("leftleftarrows", "arrows.ll"),
    ("leftrightarrow", "arrow.l.r"),
    ("leftrightharpoons", "harpoons.ltrb"),
    ("leftrightsquigarrow", "arrow.l.r.wave"),
    ("leftthreetimes", "times.three.l"),
    ("leq", "lt.eq"),
    ("leqslant", "lt.eq"),
    ("lescc", "\u{2AA8}"),
    ("lessdot", "lt.dot"),
    ("lesseqgtr", "lt.eq.gt"),
    ("lessgtr", "lt.gt"),
    ("lesssim", "lt.tilde"),
    ("lfloor", "floor.l"),
    ("lhd", "lt.tri"),
    ("ll", "lt.double"),
    ("lll", "lt.triple"),
    ("lneq", "lt.neq"),
    ("lnapprox", "lt.napprox"),
    ("lnot", "not"),
    ("lnsim", "lt.ntilde"),
    ("longleftarrow", "arrow.l"),
    ("longleftrightarrow", "arrow.l.r"),
    ("longmapsto", "mapsto"),
    ("longrightarrow", "arrow.r"),
    ("lightning", "arrow.zigzag"),
    ("looparrowleft", "arrow.l.loop"),
    ("looparrowright", "arrow.r.loop"),
    ("lor", "or"),
    ("lozenge", "lozenge.stroked"),
    ("lparen", "\\("),
    ("ltimes", "times.l"),
    ("lvert", "\\|"),
    ("male", "mars"),
    ("maltese", "maltese"),
    ("mapsto", "mapsto"),
    ("mapsfrom", "arrow.l.bar"),
    ("Mapsto", "arrow.r.double.bar"),
    ("Mapsfrom", "arrow.l.double.bar"),
    ("longmapsfrom", "arrow.l.long.bar"),
    ("measuredangle", "angle.arc"),
    ("mercury", "mercury"),
    ("mho", "Omega.inv"),
    ("mid", "divides"),
    ("models", "tack.r.double"),
    ("mp", "minus.plus"),
    ("multimap", "multimap"),
    ("nLeftarrow", "arrow.l.double.not"),
    ("nLeftrightarrow", "arrow.l.r.double.not"),
    ("nRightarrow", "arrow.r.double.not"),
    ("nVDash", "\u{22AF}"),
    ("nVdash", "forces.not"),
    ("nabla", "nabla"),
    ("napprox", "approx.not"),
    ("nasymp", "asymp.not"),
    ("natural", "natural"),
    ("ncong", "tilde.equiv.not"),
    ("ne", "eq.not"),
    ("nearrow", "arrow.tr"),
    ("neg", "not"),
    ("neptune", "neptune"),
    ("neq", "eq.not"),
    ("nequiv", "equiv.not"),
    ("nexists", "exists.not"),
    ("ngeq", "gt.eq.not"),
    ("ngeqslant", "gt.eq.not"),
    ("ngtr", "gt.not"),
    ("ngtrsim", "gt.tilde.not"),
    ("ni", "in.rev"),
    ("nin", "in.not"),
    ("nleftarrow", "arrow.l.not"),
    ("nleftrightarrow", "arrow.l.r.not"),
    ("nleq", "lt.eq.not"),
    ("nleqslant", "lt.eq.not"),
    ("nlessgtr", "lt.gt.not"),
    ("nlesssim", "lt.tilde.not"),
    ("nless", "lt.not"),
    ("nmid", "divides.not"),
    ("notin", "in.not"),
    ("nparallel", "parallel.not"),
    ("nprec", "prec.not"),
    ("npreccurlyeq", "prec.curly.eq.not"),
    ("npreceq", "prec.curly.eq.not"),
    ("nrightarrow", "arrow.r.not"),
    ("nsim", "tilde.not"),
    ("nsime", "tilde.eq.not"),
    ("nsimeq", "tilde.eq.not"),
    ("nsubset", "subset.not"),
    ("nsubseteq", "subset.eq.not"),
    ("nsucc", "succ.not"),
    ("nsupset", "supset.not"),
    ("nsupseteq", "supset.eq.not"),
    ("nsucccurlyeq", "succ.curly.eq.not"),
    ("nsucceq", "succ.curly.eq.not"),
    ("ntriangleleft", "lt.tri.not"),
    ("ntrianglelefteq", "lt.tri.eq.not"),
    ("ntriangleright", "gt.tri.not"),
    ("ntrianglerighteq", "gt.tri.eq.not"),
    ("nvDash", "tack.r.double.not"),
    ("nvdash", "tack.r.not"),
    ("nwarrow", "arrow.tl"),
    ("odot", "dot.o"),
    ("oint", "integral.cont"),
    ("oiint", "integral.surf"),
    ("oiiint", "integral.vol"),
    ("sqint", "integral.square"),
    ("fint", "integral.slash"),
    ("varointclockwise", "integral.cont.cw"),
    ("ointctrclockwise", "integral.cont.ccw"),
    ("awint", "integral.ccw"),
    ("iiiint", "integral.quad"),
    ("varprod", "times.big"),
    ("leftarrowtail", "arrow.l.tail"),
    ("subseteqq", "\u{2AC5}"),
    ("supseteqq", "\u{2AC6}"),
    ("gtrapprox", "gt.approx"),
    ("lessapprox", "lt.approx"),
    ("ngtrless", "gt.lt.not"),
    ("twoheadleftarrow", "arrow.l.twohead"),
    ("leftrightarrows", "arrows.lr"),
    ("circleddash", "dash.o"),
    ("dotplus", "plus.dot"),
    ("blacktriangle", "triangle.filled.small.t"),
    ("blacktriangledown", "triangle.filled.small.b"),
    ("ulcorner", "corner.l.t"),
    ("urcorner", "corner.r.t"),
    ("llcorner", "corner.l.b"),
    ("lrcorner", "corner.r.b"),
    ("lmoustache", "mustache.l"),
    ("rmoustache", "mustache.r"),
    ("lgroup", "paren.l.flat"),
    ("rgroup", "paren.r.flat"),
    ("llbracket", "bracket.l.stroked"),
    ("rrbracket", "bracket.r.stroked"),
    ("llparenthesis", "paren.l.closed"),
    ("rrparenthesis", "paren.r.closed"),
    ("diameter", "diameter"),
    ("obar", "\u{233D}"),
    ("ogreaterthan", "gt.o"),
    ("olessthan", "lt.o"),
    ("ominus", "minus.o"),
    ("oplus", "xor"),
    ("oslash", "slash.o"),
    ("otimes", "times.o"),
    ("owns", "in.rev"),
    ("parallel", "parallel"),
    ("partial", "partial"),
    ("pencil", "\u{270E}"),
    ("perp", "perp"),
    ("pitchfork", "\u{22D4}"),
    ("pm", "plus.minus"),
    ("pounds", "pound"),
    ("prec", "prec"),
    ("precapprox", "prec.approx"),
    ("preccurlyeq", "prec.curly.eq"),
    ("preceq", "prec.curly.eq"),
    ("precnapprox", "prec.napprox"),
    ("precneqq", "prec.nequiv"),
    ("precnsim", "prec.ntilde"),
    ("precsim", "prec.tilde"),
    ("prime", "'"),
    ("prod", "product"),
    ("propto", "prop"),
    ("quarternote", "note.quarter.alt"),
    ("rAngle", "chevron.r.double"),
    ("rBrace", "brace.r.stroked"),
    ("rVert", "parallel"),
    ("rangle", "chevron.r"),
    ("rbrace", "}"),
    ("rbrack", "\\]"),
    ("radiation", "\u{2622}"),
    ("rceil", "ceil.r"),
    ("recycle", "\u{267B}"),
    ("rfloor", "floor.r"),
    ("rhd", "gt.tri"),
    ("restriction", "harpoon.tr"),
    ("rightarrow", "arrow.r"),
    ("rightarrowtail", "arrow.r.tail"),
    ("rightharpoondown", "harpoon.rb"),
    ("rightharpoonup", "harpoon.rt"),
    ("rightleftarrows", "arrows.rl"),
    ("rightleftharpoons", "harpoons.rtlb"),
    ("rightrightarrows", "arrows.rr"),
    ("rightsquigarrow", "arrow.r.squiggly"),
    ("rightthreetimes", "times.three.r"),
    ("risingdotseq", "eq.dots.up"),
    ("rparen", "\\)"),
    ("rtimes", "times.r"),
    ("rvert", "\\|"),
    ("saturn", "saturn"),
    ("searrow", "arrow.br"),
    ("setminus", "\\\\"),
    ("sharp", "sharp"),
    ("sim", "tilde.op"),
    ("simeq", "tilde.eq"),
    ("simneqq", "tilde.nequiv"),
    ("skull", "\u{2620}"),
    ("smallsetminus", "without"),
    ("smallsmile", "smile"),
    ("smallfrown", "frown"),
    ("smile", "smile"),
    ("smiley", "\u{263A}"),
    ("spadesuit", "suit.spade.filled"),
    ("sphericalangle", "angle.spheric"),
    ("sqcap", "inter.sq"),
    ("sqcup", "union.sq"),
    ("sqsubset", "subset.sq"),
    ("sqsubseteq", "subset.eq.sq"),
    ("sqsupset", "supset.sq"),
    ("sqsupseteq", "supset.eq.sq"),
    ("square", "square.stroked.tiny"),
    ("star", "star.op"),
    ("strictfi", "\u{297C}"),
    ("strictif", "\u{297D}"),
    ("subset", "subset"),
    ("subseteq", "subset.eq"),
    ("subsetneq", "subset.neq"),
    ("subsetneqq", "\u{2ACB}"),
    ("succ", "succ"),
    ("succapprox", "succ.approx"),
    ("succcurlyeq", "succ.curly.eq"),
    ("succeq", "succ.curly.eq"),
    ("succnapprox", "succ.napprox"),
    ("succneqq", "succ.nequiv"),
    ("succnsim", "succ.ntilde"),
    ("succsim", "succ.tilde"),
    ("sum", "sum"),
    ("supset", "supset"),
    ("supseteq", "supset.eq"),
    ("supsetneq", "supset.neq"),
    ("supsetneqq", "\u{2ACC}"),
    ("swarrow", "arrow.bl"),
    ("therefore", "therefore"),
    ("times", "times"),
    ("to", "arrow.r"),
    ("top", "top"),
    ("triangle", "triangle.stroked.t"),
    ("triangledown", "triangle.stroked.small.b"),
    ("triangleleft", "lt.tri"),
    ("trianglelefteq", "lt.tri.eq"),
    ("triangleq", "eq.delta"),
    ("triangleright", "gt.tri"),
    ("trianglerighteq", "gt.tri.eq"),
    ("twoheadrightarrow", "arrow.r.twohead"),
    ("twonotes", "note.eighth.beamed"),
    ("unlhd", "lt.tri.eq"),
    ("unrhd", "gt.tri.eq"),
    ("uparrow", "arrow.t"),
    ("updownarrow", "arrow.t.b"),
    ("upharpoonleft", "harpoon.tl"),
    ("upharpoonright", "harpoon.tr"),
    ("uplus", "union.plus"),
    ("upuparrows", "arrows.tt"),
    ("uranus", "uranus.alt"),
    ("vDash", "tack.r.double"),
    ("varkappa", "\u{1D718}"),
    ("varnothing", "diameter"),
    ("varpropto", "prop"),
    ("varsubsetneq", "subset.neq"),
    ("vartriangle", "triangle.stroked.small.t"),
    ("vartriangleleft", "lt.tri"),
    ("vartriangleright", "gt.tri"),
    ("vdash", "tack.r"),
    ("vdots", "dots.v"),
    ("vee", "or"),
    ("veebar", "\u{22BB}"),
    ("vert", "\\|"),
    ("wasylozenge", "\u{2311}"),
    ("wedge", "and"),
    ("wp", "\u{2118}"),
    ("wr", "wreath"),
    ("yen", "yuan"),
    ("yinyang", "\u{262F}"),
    // Escaped literal characters. Typst escapes the dollar, hash, and underscore so they
    // are not read as code/script syntax; the brace, bar, ampersand, and percent are literal.
    ("{", "{"),
    ("}", "}"),
    ("|", "parallel"),
    ("%", "%"),
    ("$", "\\$"),
    ("#", "\\#"),
    ("&", "&"),
    ("_", "\\_"),
];

/// The Typst name for each Greek-letter command, as an iterable table feeding both the forward
/// lookup [`greek_str`] and the codepoint reverse map.
pub(super) const GREEK_TYPST: &[(&str, &str)] = &[
    ("alpha", "alpha"),
    ("beta", "beta"),
    ("gamma", "gamma"),
    ("delta", "delta"),
    ("epsilon", "epsilon.alt"),
    ("varepsilon", "epsilon"),
    ("zeta", "zeta"),
    ("eta", "eta"),
    ("theta", "theta"),
    ("vartheta", "theta.alt"),
    ("iota", "iota"),
    ("kappa", "kappa"),
    ("lambda", "lambda"),
    ("mu", "mu"),
    ("nu", "nu"),
    ("xi", "xi"),
    ("omicron", "omicron"),
    ("pi", "pi"),
    ("varpi", "pi.alt"),
    ("rho", "rho"),
    ("varrho", "\u{1D71A}"),
    ("sigma", "sigma"),
    ("varsigma", "\u{1D70D}"),
    ("tau", "tau"),
    ("upsilon", "upsilon"),
    ("phi", "phi.alt"),
    ("varphi", "phi"),
    ("chi", "chi"),
    ("psi", "psi"),
    ("omega", "omega"),
    ("Gamma", "Gamma"),
    ("Delta", "Delta"),
    ("Theta", "Theta"),
    ("Lambda", "Lambda"),
    ("Xi", "Xi"),
    ("Pi", "Pi"),
    ("Sigma", "Sigma"),
    ("Upsilon", "Upsilon"),
    ("Phi", "Phi"),
    ("Psi", "Psi"),
    ("Omega", "Omega"),
    // Capital Greek letters whose glyph is a Latin lookalike are spelled by name.
    ("Alpha", "Alpha"),
    ("Beta", "Beta"),
    ("Epsilon", "Epsilon"),
    ("Zeta", "Zeta"),
    ("Eta", "Eta"),
    ("Iota", "Iota"),
    ("Kappa", "Kappa"),
    ("Mu", "Mu"),
    ("Nu", "Nu"),
    ("Omicron", "Omicron"),
    ("Rho", "Rho"),
    ("Tau", "Tau"),
    ("Chi", "Chi"),
    ("upalpha", "alpha"),
    ("upbeta", "beta"),
    ("upgamma", "gamma"),
    ("updelta", "delta"),
    ("upepsilon", "epsilon"),
    ("upzeta", "zeta"),
    ("upeta", "eta"),
    ("uptheta", "theta"),
    ("upiota", "iota"),
    ("upkappa", "kappa"),
    ("uplambda", "lambda"),
    ("upmu", "mu"),
    ("upnu", "nu"),
    ("upxi", "xi"),
    ("upomicron", "omicron"),
    ("uppi", "pi"),
    ("uprho", "rho"),
    ("upsigma", "sigma"),
    ("uptau", "tau"),
    ("upupsilon", "upsilon"),
    ("upphi", "phi.alt"),
    ("upchi", "chi"),
    ("uppsi", "psi"),
    ("upomega", "omega"),
];
