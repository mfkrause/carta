//! Backend B: lower a parsed math tree to Typst math markup (the inner content, with no surrounding
//! `$` delimiters). Typst has native math, so this translation is far more total than the inline
//! lowering: it returns `None` only for genuinely untranslatable commands.

mod lookup;
mod render;
mod styled;

#[cfg(test)]
mod lookup_tests;

use super::parse::{Atom, BinomKind, Body, ScriptKind};
use super::symbols;

use lookup::command_str;
use render::{
    brace_func, brace_str, char_str, delimited_str, ext_arrow_str, frac_str, grid_str,
    is_environment_group, matrix_str, middle_str, mod_str, negated_str, stack_str,
};
use styled::{accent_func, accent_mark, styled_str, text_str};

pub(super) use lookup::SYMBOL_TYPST;
#[cfg(test)]
pub(super) use lookup::{GREEK_TYPST, GREEK_TYPST_MAP, SYMBOL_TYPST_MAP};

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
    let (Body::Grid(_, _, rows) | Body::Matrix(_, rows)) = body else {
        return None;
    };
    rows.iter().flatten().find_map(|cell| first_label(cell))
}

/// Lower a list of atoms, setting escaped punctuation off with spaces. Used for the content of a
/// `\left … \right` group, where Typst spaces such punctuation away from its neighbours.
pub(super) fn lower_loose(display: bool, atoms: &[Atom]) -> Option<String> {
    lower_with(display, atoms, Spacing::Loose)
}

fn lower_with(display: bool, atoms: &[Atom], spacing: Spacing) -> Option<String> {
    let mut pieces = Vec::new();
    for atom in atoms {
        // a `\label` has no glyph and is lifted out by the entry point; skip so it takes no spacing slot
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
    // an empty piece is a bare spacing slot (`{}`): keep a space so the slot stays visible
    let (Some(_), Some(r)) = (left.chars().next_back(), right.chars().next()) else {
        return true;
    };
    // Primes attach to their base.
    if r == '\'' {
        return false;
    }
    // escaped punctuation (`\(`, `\,`, …) binds tight at top level, spaced inside delimited groups
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

/// Whether a piece ends with a bare escaped tight symbol (no trailing space wanted).
fn ends_tight(piece: &str) -> bool {
    is_bare_tight(piece) || is_scripted_tight(piece)
}

/// Whether a piece starts with a bare escaped tight symbol (no leading space wanted).
fn starts_tight(piece: &str) -> bool {
    is_bare_tight(piece) || is_scripted_tight(piece)
}

fn is_number_char(c: char) -> bool {
    c.is_ascii_digit() || c == '.'
}

#[allow(clippy::similar_names)]
fn atom_str(display: bool, atom: &Atom) -> Option<String> {
    // a horizontal brace consumes its matching script as the label argument, not as a Typst script
    if let Body::Brace(kind, inner) = &atom.body {
        return brace_str(display, *kind, inner, atom);
    }
    let has_scripts = atom.sub.is_some() || atom.sup.is_some() || !atom.siblings.is_empty();
    let mut out = match &atom.body {
        // A synthesized empty nucleus (a leading `_`/`^`) renders as Typst's empty content.
        Body::Empty => "\"\"".to_string(),
        Body::Prime(count) => "'".repeat(*count as usize),
        // explicit empty group: zero-width space under scripts, bare spacing slot alone
        Body::EmptyGroup if has_scripts => "zws".to_string(),
        Body::EmptyGroup => String::new(),
        body => nucleus_str(display, body)?,
    };
    // display-mode limit operator: a prime stacks as a superscript (`\sum'` → `sum^(')`) via the
    // ordinary script path below, not pulled ahead as a literal mark
    let stack_prime = display && is_limit_op_body(&atom.body);
    // Typst allows one sub and one sup per base: slot reuse or a restart run emits a fresh `""` base;
    // a prime superscript attaches as a literal `'` set before the subscript, as TeX sets primes first
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
            // primary prime superscript was already emitted above; skip to avoid a double render
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
    // a prime superscript attaches as a literal `'` without consuming the slot, except on a
    // display-mode limit operator where it stacks as `^(')` through the path below
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
pub(super) fn wrap_script(atoms: &[Atom], inner: &str) -> String {
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
pub(super) fn is_lone_ascii_symbol(s: &str) -> bool {
    let symbol = |c: char| c.is_ascii_punctuation() && c != '.';
    let mut chars = s.chars();
    match (chars.next(), chars.next(), chars.next()) {
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
pub(super) fn is_atomic_script(s: &str) -> bool {
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
        // bare TeX-active `#`/`&`/`%` has no math meaning: verbatim fallback (alignment `&` never
        // reaches here; the escaped forms still convert)
        Body::Char('#' | '&' | '%') => None,
        Body::Char(c) => Some(char_str(*c)),
        // `:=` prints as one piece so the two characters stay tight
        Body::ColonEq => Some(":=".to_string()),
        Body::Number(digits) => Some(digits.clone()),
        Body::Command(name) => command_str(name),
        Body::Group(inner) => {
            let s = lower(display, inner)?;
            // an environment group splices its grid with no brackets; other multi-token groups
            // are parenthesised
            if is_environment_group(inner) || is_single_token(&s) {
                Some(s)
            } else {
                Some(format!("({s})"))
            }
        }
        Body::Accent(name, base) => {
            let inner = lower(display, base)?;
            // overline/underline always use their function; other accents fall back to the generic
            // `accent(content, mark)` form on multi-atom bases
            if matches!(name.as_str(), "overline" | "underline") {
                let func = accent_func(name)?;
                return Some(format!("{func}({inner})"));
            }
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
        // scale the bare glyph by percentage; a run of 5+ sized primes scales only the first
        // quadruple-prime, the rest set after the box as literal primes
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
        Body::Grid(kind, _, rows) => grid_str(display, *kind, rows),
        Body::ExtArrow(arrow, below, above) => {
            ext_arrow_str(display, arrow, below.as_deref(), above)
        }
        Body::Matrix(delim, rows) => matrix_str(display, *delim, rows),
        Body::Delimited(open, close, content) => delimited_str(display, *open, *close, content),
        Body::Middle(delim, open_side) => Some(middle_str(*delim, *open_side)),
        Body::Mod(kind, arg) => mod_str(display, *kind, arg.as_deref()),
        Body::Negated(base) => negated_str(base),
        // overlay the combining long solidus via the generic accent form
        Body::NegatedGroup(inner) => {
            let content = lower(display, inner)?;
            Some(format!("accent({content}, \u{0338})"))
        }
        // label-less brace (e.g. inside a script); the labelled form comes from `atom_str`
        Body::Brace(kind, inner) => {
            let content = lower(display, inner)?;
            Some(format!("{}({content})", brace_func(*kind)))
        }
        Body::Frac(_, num, den) => frac_str(display, num, den),
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
        // a `\label` has no glyph; the caller lifts it after the closing `$`
        Body::Label(_) => Some(String::new()),
    }
}
