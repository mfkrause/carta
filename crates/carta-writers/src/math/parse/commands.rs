//! Command parsing: control sequences, fixed-size delimiters, and their required/optional arguments.

use super::super::symbols;
use super::environments::{parse_delimited, parse_environment, parse_grid_rows_braced};
use super::text::{is_text_command, parse_verbatim_group, text_pieces_to_string};
use super::{
    Atom, BinomKind, Body, BraceKind, FracStyle, GridKind, MAX_DEPTH, ModKind, StackSide, TextMode,
    Token, parse_atom, parse_atom_into, parse_atoms, skip_spaces,
};

/// A fixed-size delimiter wrapper (`\big`, `\Big`, `\bigg`, `\Bigg`, with optional `l`/`r` variant)
/// sizes the single delimiter that follows. The size is presentational, so the wrapper is stripped
/// and the bare delimiter rendered. The relation-sizing `m`-suffix variants (`\bigm`, `\Bigm`, …) are
/// excluded: they size a relation rather than a delimiter, so the whole expression falls back to
/// verbatim.
fn big_delim_scale(name: &str) -> Option<u16> {
    let scale = match name {
        "big" | "bigl" | "bigr" => 120,
        "Big" | "Bigl" | "Bigr" => 180,
        "bigg" | "biggl" | "biggr" => 240,
        "Bigg" | "Biggl" | "Biggr" => 300,
        _ => return None,
    };
    Some(scale)
}

/// The nucleus a fixed-size wrapper (`\big…`) sizes, given the single follower token, or `None` when
/// the follower is not a token the wrapper accepts (a letter, digit, TeX-active character, or any
/// command outside the delimiter set), which leaves the wrapper unhandled and the whole expression
/// verbatim.
///
/// The wrapper sizes far more than the stretchy delimiters a `\left … \right` pair takes: any single
/// ordinary, relation, or punctuation character is also accepted and sized as a literal glyph (`\big+`
/// is a sized `+`, `\big<` a sized `<`). The null delimiter `.` is a sized literal period. The result
/// always carries ordinary class, so a sized glyph binds tightly to its neighbours regardless of the
/// character's usual class. Primes and the `:=` digraph span more than one token and are handled by
/// the caller; this single-token set excludes them.
fn big_follower(tok: &Token) -> Option<Body> {
    match tok {
        Token::Char(c) => match c {
            '(' | ')' | '[' | ']' | '|' | '<' | '>' | '/' | '.' | '!' | '*' | '+' | ',' | '-'
            | ':' | ';' | '=' | '?' | '@' | '~' => Some(Body::Char(*c)),
            _ => None,
        },
        Token::Command(c) => matches!(
            c.as_str(),
            "{" | "}"
                | "lbrace"
                | "rbrace"
                | "|"
                | "vert"
                | "Vert"
                | "lVert"
                | "rVert"
                | "lvert"
                | "rvert"
                | "langle"
                | "rangle"
                | "lfloor"
                | "rfloor"
                | "lceil"
                | "rceil"
                | "lbrack"
                | "rbrack"
        )
        .then(|| Body::Command(c.clone())),
        _ => None,
    }
}

/// Parse a fixed-size delimiter (`\big(`, `\Bigl[`, …): the wrapper sizes the single delimiter that
/// follows. See [`big_follower`] for the accepted single-token set; an unaccepted follower leaves the
/// wrapper unhandled so the whole expression falls back to verbatim.
///
/// Two followers span more than one token and are taken as a unit: a run of consecutive primes
/// (`\big''` is one sized double-prime, not a prime carrying a prime script) and the `:=` digraph
/// (`\big:=` is one sized relation, not a sized `:` followed by a loose `=`).
fn parse_big_delim(scale: u16, tokens: &[Token], pos: &mut usize, _depth: usize) -> Option<Atom> {
    skip_spaces(tokens, pos);
    let nucleus = match tokens.get(*pos)? {
        Token::Char('\'') => {
            let mut count: u8 = 0;
            while let Some(Token::Char('\'')) = tokens.get(*pos) {
                count = count.checked_add(1)?;
                *pos += 1;
            }
            Body::Prime(count)
        }
        Token::Char(':') if matches!(tokens.get(*pos + 1), Some(Token::Char('='))) => {
            *pos += 2;
            Body::ColonEq
        }
        tok => {
            let body = big_follower(tok)?;
            *pos += 1;
            body
        }
    };
    Some(Atom::new(Body::Big(scale, Box::new(Atom::new(nucleus)))))
}

#[allow(clippy::too_many_lines)]
pub(super) fn parse_command(
    name: &str,
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    tail: &mut Option<String>,
) -> Option<Atom> {
    if let Some(scale) = big_delim_scale(name) {
        return parse_big_delim(scale, tokens, pos, depth);
    }
    if is_accent_command(name) {
        let arg = parse_required_group(tokens, pos, depth, tail)?;
        return Some(Atom::new(Body::Accent(name.to_string(), arg)));
    }
    if is_text_command(name) {
        // `\operatorname*` keeps its star so the display backend can center a subscript beneath it.
        let mut wrapper = name;
        if name == "operatorname" && matches!(tokens.get(*pos), Some(Token::Char('*'))) {
            *pos += 1;
            wrapper = "operatorname*";
        }
        let mode = if name == "operatorname" {
            TextMode::Math
        } else {
            TextMode::Wrapper
        };
        let text = parse_verbatim_group(tokens, pos, mode)?;
        return Some(Atom::new(Body::Text(wrapper.to_string(), text)));
    }
    if is_styled_command(name) {
        let arg = parse_required_group(tokens, pos, depth, tail)?;
        return Some(Atom::new(Body::Styled(name.to_string(), arg)));
    }
    match name {
        "frac" | "dfrac" | "tfrac" => {
            let num = parse_required_group(tokens, pos, depth, tail)?;
            let den = parse_required_group(tokens, pos, depth, tail)?;
            let style = if name == "tfrac" {
                FracStyle::Linear
            } else {
                FracStyle::Bar
            };
            Some(Atom::new(Body::Frac(style, num, den)))
        }
        "binom" => {
            let top = parse_required_group(tokens, pos, depth, tail)?;
            let bottom = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Binom(BinomKind::Paren, top, bottom)))
        }
        "genfrac" => parse_genfrac(tokens, pos, depth, tail),
        "xrightarrow" | "xleftarrow" => parse_extensible_arrow(name, tokens, pos, depth, tail),
        "overbrace" => {
            let arg = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Brace(BraceKind::Over, arg)))
        }
        "underbrace" => {
            let arg = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Brace(BraceKind::Under, arg)))
        }
        "sqrt" => {
            let index = parse_optional_bracket(tokens, pos, depth);
            // A bare `\sqrt` is the lone radical sign `√`; an index without a radicand is malformed.
            match parse_required_group(tokens, pos, depth, tail) {
                Some(radicand) => Some(Atom::new(Body::Sqrt(index, radicand))),
                None if index.is_none() => Some(Atom::new(Body::Char('\u{221A}'))),
                None => None,
            }
        }
        // A bare radical sign takes the single following atom as its radicand.
        "surd" => {
            let radicand = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Sqrt(None, radicand)))
        }
        // A nested environment wraps its content in one transparent group; at run top it is spliced.
        "begin" => {
            let spliced = parse_environment(tokens, pos, depth)?;
            Some(Atom::new(Body::Group(spliced)))
        }
        "overset" | "stackrel" => {
            let mark = parse_required_group(tokens, pos, depth, tail)?;
            let base = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Stack(StackSide::Over, mark, base)))
        }
        "underset" => {
            let mark = parse_required_group(tokens, pos, depth, tail)?;
            let base = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Stack(StackSide::Under, mark, base)))
        }
        "substack" => {
            skip_spaces(tokens, pos);
            if !matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
                return None;
            }
            *pos += 1;
            let rows = parse_grid_rows_braced(tokens, pos, depth)?;
            Some(Atom::new(Body::Grid(GridKind::Substack, Vec::new(), rows)))
        }
        "left" => parse_delimited(tokens, pos, depth),
        // `\bmod` is infix: with no following operand it is invalid and falls back to verbatim.
        "bmod" => {
            let mut probe = *pos;
            while matches!(tokens.get(probe), Some(Token::Space)) {
                probe += 1;
            }
            if tokens
                .get(probe)
                .is_none_or(|t| matches!(t, Token::GroupClose))
            {
                return None;
            }
            Some(Atom::new(Body::Mod(ModKind::Bmod, None)))
        }
        "pmod" => {
            let arg = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Mod(ModKind::Pmod, Some(arg))))
        }
        // `\mod` leads its operand (kept a separate atom); invalid with none.
        "mod" => {
            skip_spaces(tokens, pos);
            if tokens.get(*pos).is_none_or(|t| {
                matches!(
                    t,
                    Token::Sub | Token::Sup | Token::GroupClose | Token::Space
                )
            }) {
                return None;
            }
            Some(Atom::new(Body::Mod(ModKind::Mod, None)))
        }
        "pod" => {
            let arg = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Mod(ModKind::Pod, Some(arg))))
        }
        // `\not` strikes the following relation: a command name or a single character like `=`.
        "not" => {
            skip_spaces(tokens, pos);
            // A braced base negates the whole group; a bare command or character negates one token.
            if matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
                *pos += 1;
                let inner = parse_atoms(tokens, pos, depth + 1, true)?;
                return Some(Atom::new(Body::NegatedGroup(inner)));
            }
            // A literal character always strikes; a command only when it composes into a struck
            // form, so `\not\|` and the like are left verbatim.
            let base = match tokens.get(*pos)? {
                Token::Command(c) if symbols::command_negatable(c) => c.clone(),
                Token::Char(c) => c.to_string(),
                _ => return None,
            };
            *pos += 1;
            Some(Atom::new(Body::Negated(base)))
        }
        _ => Some(Atom::new(Body::Command(name.to_string()))),
    }
}

/// Whether a `\not` at `pos` (a command token) is immediately followed by a numeric literal, after
/// any intervening spaces. Such a `\not` strikes only the number's first digit.
pub(super) fn not_over_number(tokens: &[Token], pos: usize) -> bool {
    let mut probe = pos + 1;
    while matches!(tokens.get(probe), Some(Token::Space)) {
        probe += 1;
    }
    matches!(tokens.get(probe), Some(Token::Number(_)))
}

/// Commands that place a mark over a single base group. The set is broader than what inline output
/// can render: a few (e.g. `\ddot`, `\overline`) only translate to Typst and force a verbatim
/// fallback in inline output.
fn is_accent_command(name: &str) -> bool {
    matches!(
        name,
        "bar"
            | "hat"
            | "widehat"
            | "tilde"
            | "widetilde"
            | "vec"
            | "overrightarrow"
            | "overleftarrow"
            | "dot"
            | "ddot"
            | "check"
            | "breve"
            | "acute"
            | "grave"
            | "mathring"
            | "overline"
            | "underline"
            | "dddot"
            | "ddddot"
            | "overleftrightarrow"
            | "underleftarrow"
            | "underrightarrow"
    )
}

/// A style switch that only sets the typesetting size and carries no glyph.
pub(super) fn is_style_switch(name: &str) -> bool {
    matches!(
        name,
        "displaystyle" | "textstyle" | "scriptstyle" | "scriptscriptstyle"
    )
}

/// The bracket kind of an infix binomial operator (`\choose`, `\brace`, `\brack`), or `None` for any
/// other command.
pub(super) fn binom_kind(name: &str) -> Option<BinomKind> {
    match name {
        "choose" => Some(BinomKind::Paren),
        "brace" => Some(BinomKind::Brace),
        "brack" => Some(BinomKind::Brack),
        _ => None,
    }
}

/// The bracket kind of a `\genfrac` from its opening-delimiter argument, or `None` when the delimiter
/// is absent or unrecognised (the construct then falls back to verbatim).
fn genfrac_kind(left: &str) -> Option<BinomKind> {
    match left.trim() {
        "(" => Some(BinomKind::Paren),
        "[" => Some(BinomKind::Brack),
        "{" | "\\{" => Some(BinomKind::Brace),
        _ => None,
    }
}

/// Parse `\genfrac{ldelim}{rdelim}{thickness}{style}{num}{den}`: the opening delimiter selects the
/// surrounding bracket; the right-delimiter, rule-thickness and style groups are consumed. With no
/// opening delimiter the construct cannot be laid out and falls back to verbatim.
fn parse_genfrac(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    tail: &mut Option<String>,
) -> Option<Atom> {
    let left = parse_verbatim_group(tokens, pos, TextMode::Math)?;
    let kind = genfrac_kind(&text_pieces_to_string(&left))?;
    for _ in 0..3 {
        parse_required_group(tokens, pos, depth, tail)?;
    }
    let top = parse_required_group(tokens, pos, depth, tail)?;
    let bottom = parse_required_group(tokens, pos, depth, tail)?;
    Some(Atom::new(Body::Binom(kind, top, bottom)))
}

/// Parse an extensible arrow (`\xrightarrow`/`\xleftarrow`): an optional `[below]` label and a
/// required `{above}` label, set as scripts on the arrow glyph.
fn parse_extensible_arrow(
    name: &str,
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    tail: &mut Option<String>,
) -> Option<Atom> {
    let arrow = if name == "xrightarrow" {
        "arrow.r"
    } else {
        "arrow.l"
    };
    let below = parse_optional_bracket(tokens, pos, depth);
    let above = parse_required_group(tokens, pos, depth, tail)?;
    Some(Atom::new(Body::ExtArrow(arrow, below, above)))
}

fn is_styled_command(name: &str) -> bool {
    matches!(
        name,
        "mathbb"
            | "mathcal"
            | "mathscr"
            | "mathfrak"
            | "mathbf"
            | "boldsymbol"
            | "bm"
            | "mathit"
            | "mathrm"
            | "mathsf"
            | "mathtt"
            | "pmb"
            // Composed styled alphabets (bold-italic, sans-italic, bold script/fraktur).
            | "mathbfit"
            | "mathsfit"
            | "mathbfsfit"
            | "mathbfcal"
            | "mathbfscr"
            | "mathbffrak"
            // Alternative alphabet-wrapper spellings; of the `\sym…` family only `\symbf` changes glyphs.
            | "mathds"
            | "symbf"
            | "mathup"
            | "mathsfup"
            | "mathbfup"
            | "mathbfsfup"
            // Math-class wrappers re-class their argument; the class is invisible in linear output.
            | "mathord"
            | "mathrel"
            | "mathop"
            | "mathbin"
            | "mathopen"
            | "mathclose"
            | "mathpunct"
            // Presentation wrappers translate only to Typst; linear output falls back to verbatim.
            | "phantom"
            | "cancel"
            | "xcancel"
            | "bcancel"
            | "boxed"
            | "overparen"
            | "underparen"
    )
}

/// Parse a required `{...}` group, advancing past optional leading spaces. An unbraced argument is a
/// single token: a multi-digit number gives up only its first digit and leaves the rest in `tail` for
/// the command's next argument or the enclosing run to place.
pub(super) fn parse_required_group(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    tail: &mut Option<String>,
) -> Option<Vec<Atom>> {
    if depth > MAX_DEPTH {
        return None;
    }
    // A prior argument took one digit of an unbraced number and left the rest; take the next (`\frac12`).
    if let Some(rest) = tail.take() {
        let mut chars = rest.chars();
        let first = chars.next()?;
        let remainder: String = chars.collect();
        if !remainder.is_empty() {
            *tail = Some(remainder);
        }
        return Some(vec![Atom::new(Body::Number(first.to_string()))]);
    }
    skip_spaces(tokens, pos);
    match tokens.get(*pos)? {
        Token::GroupOpen => {
            *pos += 1;
            parse_atoms(tokens, pos, depth + 1, true)
        }
        // An unbraced number yields only its first digit; the rest stays in `tail` (`\sqrt12`).
        Token::Number(digits) => {
            let mut chars = digits.chars();
            let first = chars.next()?;
            let remainder: String = chars.collect();
            *pos += 1;
            if !remainder.is_empty() {
                *tail = Some(remainder);
            }
            Some(vec![Atom::new(Body::Number(first.to_string()))])
        }
        // A single token also serves as the argument (e.g. `\bar x`).
        Token::Char(_) | Token::Command(_) => {
            let mut group = Vec::new();
            parse_atom_into(tokens, pos, depth + 1, &mut group)?;
            Some(group)
        }
        _ => None,
    }
}

/// Parse an optional `[...]` bracket argument (used by `\sqrt[n]{..}`); the bracketed content is
/// read as raw atoms.
fn parse_optional_bracket(tokens: &[Token], pos: &mut usize, depth: usize) -> Option<Vec<Atom>> {
    skip_spaces(tokens, pos);
    if !matches!(tokens.get(*pos), Some(Token::Char('['))) {
        return None;
    }
    let mut probe = *pos + 1;
    let mut inner: Vec<Atom> = Vec::new();
    while let Some(tok) = tokens.get(probe) {
        if matches!(tok, Token::Char(']')) {
            *pos = probe + 1;
            return Some(inner);
        }
        let mut local = probe;
        // A bracketed argument keeps numbers whole (`\sqrt[12]{x}`); leftover digits stay inside the bracket.
        let mut bracket_tail = None;
        let atom = parse_atom(tokens, &mut local, depth + 1, &mut bracket_tail)?;
        if local == probe {
            return None;
        }
        probe = local;
        inner.push(atom);
        if let Some(rest) = bracket_tail {
            inner.push(Atom::new(Body::Number(rest)));
        }
    }
    None
}

/// Consume a balanced `{…}` group without interpreting its contents, advancing past leading spaces.
/// Returns `None` if no group is present or it is unbalanced.
pub(super) fn skip_balanced_group(tokens: &[Token], pos: &mut usize) -> Option<()> {
    skip_spaces(tokens, pos);
    if !matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
        return None;
    }
    *pos += 1;
    let mut depth = 1u32;
    while let Some(tok) = tokens.get(*pos) {
        *pos += 1;
        match tok {
            Token::GroupOpen => depth += 1,
            Token::GroupClose => {
                depth -= 1;
                if depth == 0 {
                    return Some(());
                }
            }
            _ => {}
        }
    }
    None
}
