//! Delimited groups, matrix and alignment environments, and their cell grids.

use super::scripts::push_prime;
use super::text::{parse_verbatim_group, text_pieces_to_string};
use super::{
    Atom, Body, ColumnAlign, Delim, GridKind, MAX_DEPTH, MatrixDelim, TextMode, Token,
    parse_atom_into, parse_atoms, parse_script, read_label_arg, skip_spaces,
};

/// The per-column justifications an `array` column specification declares, in column order. Each of
/// `l`, `c`, `r` (and the paragraph column types `p`, `m`, `b`, which set flush-left) contributes one
/// column; rules, inter-column material, and their braced arguments are skipped.
fn parse_column_aligns(spec: &str) -> Vec<ColumnAlign> {
    let mut aligns = Vec::new();
    let mut brace_depth = 0i32;
    for c in spec.chars() {
        match c {
            '{' => brace_depth += 1,
            '}' => brace_depth -= 1,
            _ if brace_depth > 0 => {}
            'l' | 'p' | 'm' | 'b' => aligns.push(ColumnAlign::Left),
            'c' => aligns.push(ColumnAlign::Center),
            'r' => aligns.push(ColumnAlign::Right),
            _ => {}
        }
    }
    aligns
}

/// Parse a `\left<delim> … \right<delim>` group: read the opening delimiter, the enclosed run up to
/// the matching `\right`, and the closing delimiter.
pub(super) fn parse_delimited(tokens: &[Token], pos: &mut usize, depth: usize) -> Option<Atom> {
    if depth > MAX_DEPTH {
        return None;
    }
    let open = parse_delimiter(tokens, pos)?;
    let mut inner: Vec<Atom> = Vec::new();
    loop {
        match tokens.get(*pos) {
            Some(Token::Command(c)) if c == "right" => {
                *pos += 1;
                let close = parse_delimiter(tokens, pos)?;
                return Some(Atom::new(Body::Delimited(open, close, inner)));
            }
            Some(Token::Command(c)) if c == "middle" => {
                *pos += 1;
                let middle = parse_middle_delimiter(tokens, pos)?;
                let (delim, open_side) = match middle {
                    Some((d, side)) => (Some(d), side),
                    None => (None, true),
                };
                inner.push(Atom::new(Body::Middle(delim, open_side)));
            }
            Some(Token::GroupClose) | None => return None,
            Some(Token::Space) => {
                *pos += 1;
            }
            Some(Token::Sub | Token::Sup) => {
                let is_sup = matches!(tokens.get(*pos), Some(Token::Sup));
                *pos += 1;
                let script = parse_script(tokens, pos, depth + 1)?;
                let last = inner.last_mut()?;
                if is_sup {
                    if last.sup.is_some() {
                        return None;
                    }
                    last.sup = Some(script);
                } else {
                    if last.sub.is_some() {
                        return None;
                    }
                    last.sub = Some(script);
                }
            }
            Some(Token::GroupOpen) => {
                *pos += 1;
                let group = parse_atoms(tokens, pos, depth + 1, true)?;
                if group.is_empty() {
                    inner.push(Atom::new(Body::EmptyGroup));
                } else {
                    inner.extend(group);
                }
            }
            Some(Token::Char('\'')) => {
                *pos += 1;
                let last = inner.last_mut()?;
                push_prime(last);
            }
            Some(_) => {
                parse_atom_into(tokens, pos, depth + 1, &mut inner)?;
            }
        }
    }
}

/// Read one delimiter token following `\left`/`\right`. The outer `Option` is the parse result and
/// the inner one distinguishes an absent delimiter (`.`, the inner `None`) from a present one.
#[allow(clippy::option_option)]
fn parse_delimiter(tokens: &[Token], pos: &mut usize) -> Option<Option<Delim>> {
    skip_spaces(tokens, pos);
    let delim = match tokens.get(*pos)? {
        Token::Char('.') => None,
        Token::Char('(' | ')') => Some(Delim::Paren),
        Token::Char('[' | ']') => Some(Delim::Bracket),
        Token::Char('|') => Some(Delim::Bar),
        Token::Char('<' | '>') => Some(Delim::Angle),
        Token::Command(c) => match c.as_str() {
            "{" | "lbrace" | "}" | "rbrace" => Some(Delim::Brace),
            "Vert" => Some(Delim::DoubleBar),
            "|" | "lVert" | "rVert" => Some(Delim::BarVert),
            "lvert" | "rvert" => Some(Delim::Bar),
            "langle" | "rangle" => Some(Delim::Angle),
            "lfloor" | "rfloor" => Some(Delim::Floor),
            "lceil" | "rceil" => Some(Delim::Ceil),
            "ulcorner" => Some(Delim::CornerUpperLeft),
            "urcorner" => Some(Delim::CornerUpperRight),
            _ => return None,
        },
        _ => return None,
    };
    *pos += 1;
    Some(delim)
}

/// Read the delimiter following a `\middle`, with its side. The outer `Option` is the parse result;
/// the inner one distinguishes an absent delimiter (`.`) from a present `(Delim, is_open_side)`. A
/// one-sided delimiter carries the side of the glyph that was written; a symmetric bar takes the
/// opening side by convention (its name has no side suffix). A non-delimiter (`/`, `\backslash`)
/// has no stretchy middle form, so the whole group falls back to verbatim.
#[allow(clippy::option_option)]
fn parse_middle_delimiter(tokens: &[Token], pos: &mut usize) -> Option<Option<(Delim, bool)>> {
    skip_spaces(tokens, pos);
    let middle = match tokens.get(*pos)? {
        Token::Char('.') => None,
        Token::Char('(') => Some((Delim::Paren, true)),
        Token::Char(')') => Some((Delim::Paren, false)),
        Token::Char('[') => Some((Delim::Bracket, true)),
        Token::Char(']') => Some((Delim::Bracket, false)),
        Token::Char('|') => Some((Delim::Bar, true)),
        Token::Char('<') => Some((Delim::Angle, true)),
        Token::Char('>') => Some((Delim::Angle, false)),
        Token::Command(c) => match c.as_str() {
            "{" | "lbrace" => Some((Delim::Brace, true)),
            "}" | "rbrace" => Some((Delim::Brace, false)),
            "Vert" => Some((Delim::DoubleBar, true)),
            "|" | "lVert" | "rVert" => Some((Delim::BarVert, true)),
            "vert" | "lvert" => Some((Delim::Bar, true)),
            "rvert" => Some((Delim::Bar, false)),
            "langle" => Some((Delim::Angle, true)),
            "rangle" => Some((Delim::Angle, false)),
            "lfloor" => Some((Delim::Floor, true)),
            "rfloor" => Some((Delim::Floor, false)),
            "lceil" => Some((Delim::Ceil, true)),
            "rceil" => Some((Delim::Ceil, false)),
            "ulcorner" => Some((Delim::CornerUpperLeft, true)),
            "urcorner" => Some((Delim::CornerUpperRight, false)),
            _ => return None,
        },
        _ => return None,
    };
    *pos += 1;
    Some(middle)
}

/// Parse a `\begin{env} … \end{env}` matrix environment into a grid of cells.
pub(super) fn parse_environment(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
) -> Option<Vec<Atom>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let env = text_pieces_to_string(&parse_verbatim_group(tokens, pos, TextMode::Math)?);
    // mathtools starred environments render as the unstarred form; the optional `[align]` argument
    // becomes literal leading content of the first cell. Base name drives the lookup, full name matches `\end`.
    let starred_grid = matches!(
        env.as_str(),
        "matrix*"
            | "pmatrix*"
            | "bmatrix*"
            | "Bmatrix*"
            | "vmatrix*"
            | "Vmatrix*"
            | "smallmatrix*"
            | "cases*"
    );
    let base = if starred_grid {
        env.strip_suffix('*').unwrap_or(env.as_str())
    } else {
        env.as_str()
    };
    let matrix_delim = match base {
        "matrix" => Some(MatrixDelim::None),
        "pmatrix" => Some(MatrixDelim::Paren),
        "bmatrix" => Some(MatrixDelim::Bracket),
        "Bmatrix" => Some(MatrixDelim::Brace),
        "vmatrix" => Some(MatrixDelim::Bar),
        "Vmatrix" => Some(MatrixDelim::DoubleBar),
        _ => None,
    };
    let grid_kind = match base {
        // Multi-line environments are alignment grids (rows on `\\`, columns on `&`); single-line
        // `equation` is handled below.
        "aligned" | "align" | "aligned*" | "align*" | "split" | "alignat" | "alignat*"
        | "alignedat" | "alignedat*" => Some(GridKind::Aligned),
        "gathered" | "gather" | "gather*" | "smallmatrix" | "multline" | "multline*"
        | "multlined" | "multlined*" => Some(GridKind::Gathered),
        "eqnarray" | "eqnarray*" => Some(GridKind::Eqnarray),
        "flalign" | "flalign*" | "flaligned" | "flaligned*" => Some(GridKind::Flalign),
        "array" => Some(GridKind::Array),
        "cases" => Some(GridKind::Cases),
        _ => None,
    };
    // Single-line equation wraps one expression: more than one grid cell falls back to verbatim.
    let single_line = matches!(env.as_str(), "equation" | "equation*");
    if matrix_delim.is_none() && grid_kind.is_none() && !single_line {
        return None;
    }
    // `array` carries a `{cols}` spec group, `alignat`/`alignedat` a mandatory `{N}` count group;
    // a missing group is malformed and falls back to verbatim.
    let array_aligns = if env == "array" {
        let spec = text_pieces_to_string(&parse_verbatim_group(tokens, pos, TextMode::Math)?);
        parse_column_aligns(&spec)
    } else {
        Vec::new()
    };
    if matches!(
        env.as_str(),
        "alignat" | "alignat*" | "alignedat" | "alignedat*"
    ) {
        parse_verbatim_group(tokens, pos, TextMode::Math)?;
    }
    // A starred grid's `[align]` argument becomes verbatim leading content of the first cell.
    let leading = if starred_grid {
        optional_bracket_literal(tokens, pos)
    } else {
        Vec::new()
    };
    let mut rows = parse_grid_rows(tokens, pos, depth, &env)?;
    prepend_first_cell(&mut rows, leading);
    if single_line {
        return single_cell(rows);
    }
    if let Some(delim) = matrix_delim {
        return Some(vec![Atom::new(Body::Matrix(delim, rows))]);
    }
    let kind = grid_kind?;
    Some(vec![Atom::new(Body::Grid(kind, array_aligns, rows))])
}

/// Read an optional `[…]` bracket group immediately following the environment opener as a run of
/// literal atoms (the `[`, its contents, and the `]`, each an ordinary character). Returns an empty
/// run when no bracket follows, leaving the position unchanged. An unterminated `[` is also left in
/// place and yields nothing, so the bracket falls through to the first cell unaltered.
fn optional_bracket_literal(tokens: &[Token], pos: &mut usize) -> Vec<Atom> {
    if !matches!(tokens.get(*pos), Some(Token::Char('['))) {
        return Vec::new();
    }
    let mut probe = *pos + 1;
    let mut literal = vec![Atom::new(Body::Char('['))];
    while let Some(tok) = tokens.get(probe) {
        match tok {
            Token::Char(']') => {
                literal.push(Atom::new(Body::Char(']')));
                *pos = probe + 1;
                return literal;
            }
            Token::Char(c) => literal.push(Atom::new(Body::Char(*c))),
            Token::Number(digits) => literal.push(Atom::new(Body::Number(digits.clone()))),
            Token::Space => literal.push(Atom::new(Body::Char(' '))),
            _ => return Vec::new(),
        }
        probe += 1;
    }
    Vec::new()
}

/// Prepend a run of literal atoms to the first cell of the first row of a grid. A leading atom run
/// captured from a starred grid's `[align]` argument is glued ahead of the first cell's content.
fn prepend_first_cell(rows: &mut [Vec<Vec<Atom>>], leading: Vec<Atom>) {
    if leading.is_empty() {
        return;
    }
    if let Some(cell) = rows.first_mut().and_then(|row| row.first_mut()) {
        cell.splice(0..0, leading);
    }
}

/// The single cell of a grid that must hold exactly one row of one cell, or `None` if it has an
/// alignment column or a row break. The single-line equation environments accept only such a grid.
fn single_cell(rows: Vec<Vec<Vec<Atom>>>) -> Option<Vec<Atom>> {
    let [row] = <[_; 1]>::try_from(rows).ok()?;
    let [cell] = <[_; 1]>::try_from(row).ok()?;
    Some(cell)
}

/// Read the `&`/`\\`-separated grid of cells of an environment up to its matching `\end{env}`.
fn parse_grid_rows(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    env: &str,
) -> Option<Vec<Vec<Vec<Atom>>>> {
    let mut rows: Vec<Vec<Vec<Atom>>> = Vec::new();
    let mut row: Vec<Vec<Atom>> = Vec::new();
    loop {
        let (cell, sep) = parse_matrix_cell(tokens, pos, depth + 1)?;
        match sep {
            CellEnd::Column => row.push(cell),
            CellEnd::Row => {
                row.push(cell);
                rows.push(std::mem::take(&mut row));
            }
            CellEnd::Environment => {
                let closing =
                    text_pieces_to_string(&parse_verbatim_group(tokens, pos, TextMode::Math)?);
                if closing != env {
                    return None;
                }
                row.push(cell);
                rows.push(row);
                return Some(rows);
            }
        }
    }
}

/// What terminated a matrix cell.
enum CellEnd {
    Column,
    Row,
    Environment,
}

/// Read the `\\`-separated rows of a braced grid (`\substack{a \\ b}`) up to its closing `}`. Each
/// row is a single cell, since `\substack` has no column separator.
pub(super) fn parse_grid_rows_braced(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
) -> Option<Vec<Vec<Vec<Atom>>>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut rows: Vec<Vec<Vec<Atom>>> = Vec::new();
    let mut atoms: Vec<Atom> = Vec::new();
    while let Some(tok) = tokens.get(*pos) {
        match tok {
            Token::GroupClose => {
                *pos += 1;
                rows.push(vec![std::mem::take(&mut atoms)]);
                return Some(rows);
            }
            Token::Command(c) if c == "\\" => {
                *pos += 1;
                skip_optional_break_dim(tokens, pos);
                rows.push(vec![std::mem::take(&mut atoms)]);
            }
            Token::Space => {
                *pos += 1;
            }
            _ => parse_atom_into(tokens, pos, depth + 1, &mut atoms)?,
        }
    }
    None
}

/// Parse a single matrix cell: a run of atoms up to the next `&`, `\\`, or `\end`.
fn parse_matrix_cell(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
) -> Option<(Vec<Atom>, CellEnd)> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut atoms: Vec<Atom> = Vec::new();
    while let Some(tok) = tokens.get(*pos) {
        match tok {
            Token::Char('&') => {
                *pos += 1;
                return Some((atoms, CellEnd::Column));
            }
            Token::Command(c) if c == "\\" => {
                *pos += 1;
                skip_optional_break_dim(tokens, pos);
                return Some((atoms, CellEnd::Row));
            }
            Token::Command(c) if c == "end" => {
                *pos += 1;
                return Some((atoms, CellEnd::Environment));
            }
            // An environment nested directly in a cell splices, so an inner grid reads as part of
            // the surrounding alignment rather than a parenthesised operand.
            Token::Command(c) if c == "begin" => {
                *pos += 1;
                let spliced = parse_environment(tokens, pos, depth)?;
                atoms.extend(spliced);
            }
            Token::Space => {
                *pos += 1;
            }
            Token::Sub | Token::Sup => {
                let is_sup = matches!(tok, Token::Sup);
                *pos += 1;
                let script = parse_script(tokens, pos, depth + 1)?;
                let last = atoms.last_mut()?;
                if is_sup {
                    if last.sup.is_some() {
                        return None;
                    }
                    last.sup = Some(script);
                } else {
                    if last.sub.is_some() {
                        return None;
                    }
                    last.sub = Some(script);
                }
            }
            Token::GroupOpen => {
                *pos += 1;
                let inner = parse_atoms(tokens, pos, depth + 1, true)?;
                atoms.extend(inner);
            }
            Token::Char('\'') => {
                *pos += 1;
                let last = atoms.last_mut()?;
                push_prime(last);
            }
            // In-cell numbering annotations have no glyph: `\nonumber` drops, `\tag` discards its
            // argument, `\label` becomes a `Label` atom; a nested group leaves the expression unhandled.
            Token::Command(c) if c == "nonumber" => {
                *pos += 1;
            }
            Token::Command(c) if c == "tag" => {
                *pos += 1;
                read_label_arg(tokens, pos)?;
            }
            Token::Command(c) if c == "label" => {
                *pos += 1;
                if let Some(name) = read_label_arg(tokens, pos)? {
                    atoms.push(Atom::new(Body::Label(name)));
                }
            }
            // A rule between rows has no glyph and does not affect the cells; consume and drop.
            Token::Command(c) if c == "hline" || c == "hdashline" => {
                *pos += 1;
            }
            _ => {
                parse_atom_into(tokens, pos, depth, &mut atoms)?;
            }
        }
    }
    None
}

/// Consume and discard a row break's optional `[<dim>]` extra-space argument. The bracket binds only
/// when it immediately follows the `\\` with no intervening space; a `[` after a space is ordinary
/// content (the start of the next row) and is left untouched.
fn skip_optional_break_dim(tokens: &[Token], pos: &mut usize) {
    if !matches!(tokens.get(*pos), Some(Token::Char('['))) {
        return;
    }
    let mut probe = *pos + 1;
    while let Some(tok) = tokens.get(probe) {
        if matches!(tok, Token::Char(']')) {
            *pos = probe + 1;
            return;
        }
        probe += 1;
    }
}
