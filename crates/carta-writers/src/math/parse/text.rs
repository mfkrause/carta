//! Text-mode wrappers: verbatim groups, accents, ligatures, and their piece sequences.

use super::{TextMode, TextPiece, TextSpace, Token, parse_atoms, skip_spaces};

/// Text-mode commands: their argument is verbatim text, not math.
pub(super) fn is_text_command(name: &str) -> bool {
    matches!(
        name,
        "text" | "textrm" | "textbf" | "textit" | "texttt" | "textsf" | "operatorname" | "mbox"
    )
}

/// Flatten a text-piece sequence to a plain string, rendering each spacing as its codepoint. Used
/// where a text-mode group is read only for a delimiter character, never re-emitted as formatted text.
pub(super) fn text_pieces_to_string(pieces: &[TextPiece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            TextPiece::Run(run) => out.push_str(run),
            TextPiece::Space(space) => out.push(space.codepoint()),
            // A `$…$` cannot occur in the math-mode groups this flattening serves.
            TextPiece::Math(_) => {}
        }
    }
    out
}

/// Finalize a completed text-wrapper run, applying TeX input-mode ligatures when the run is genuine
/// text (a `\text`/`\textbf`/… wrapper). An operator-name run is upright math text, which does not
/// run input ligatures, so it is returned unchanged.
fn finish_text_run(run: String, mode: TextMode) -> String {
    if mode == TextMode::Math {
        run
    } else {
        apply_text_ligatures(&run)
    }
}

/// Apply TeX's text-mode input ligatures to a literal character run. The TeX tokenizer folds straight
/// quote and dash runs into their curly/dash punctuation as the input is read, independent of any
/// smart-typography toggle:
///
/// - a run of backticks pairs into left double quotes `“` with a trailing lone backtick as a left
///   single quote `‘`; a run of apostrophes pairs into right double quotes `”` with a trailing lone
///   apostrophe as a right single quote `’`;
/// - a run of hyphens is consumed greedily, three at a time as an em dash `—`, then two as an en
///   dash `–`, then one as a literal hyphen.
///
/// Other characters pass through unchanged, so a run with no quote or dash is returned as written.
fn apply_text_ligatures(run: &str) -> String {
    let chars: Vec<char> = run.chars().collect();
    let mut out = String::with_capacity(run.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '`' | '\'' => {
                let (open, single) = if c == '`' {
                    ('\u{201C}', '\u{2018}')
                } else {
                    ('\u{201D}', '\u{2019}')
                };
                let mut run_len = 1;
                while chars.get(i + run_len) == Some(&c) {
                    run_len += 1;
                }
                for _ in 0..run_len / 2 {
                    out.push(open);
                }
                if run_len % 2 == 1 {
                    out.push(single);
                }
                i += run_len;
            }
            '-' => {
                let mut run_len = 1;
                while chars.get(i + run_len) == Some(&'-') {
                    run_len += 1;
                }
                let mut remaining = run_len;
                while remaining >= 3 {
                    out.push('\u{2014}');
                    remaining -= 3;
                }
                if remaining == 2 {
                    out.push('\u{2013}');
                } else if remaining == 1 {
                    out.push('-');
                }
                i += run_len;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    out
}

/// The literal character a backslash-escape inside text mode unescapes to (`\%`→`%`, `\_`→`_`, …),
/// or `None` if the command is not a recognized text-mode escape.
fn text_escape_char(name: &str) -> Option<char> {
    let c = match name {
        "%" => '%',
        "&" => '&',
        "_" => '_',
        "$" => '$',
        "{" => '{',
        "}" => '}',
        "#" => '#',
        " " => ' ',
        _ => return None,
    };
    Some(c)
}

/// The spacing a backslash-escape inside text mode introduces (`\,`, `\;`, `\:`, `\!`), or `None`.
fn text_space(name: &str) -> Option<TextSpace> {
    let s = match name {
        "," => TextSpace::Thin,
        ";" => TextSpace::Thick,
        ":" => TextSpace::Medium,
        "!" => TextSpace::NegThin,
        _ => return None,
    };
    Some(s)
}

/// Whether `name` is a text-mode accent command that places a diacritic over the following character.
/// The accents recognized are those that compose with a Latin base to a single Unicode letter (acute,
/// grave, circumflex, diaeresis, tilde, macron, dot-above, breve, caron, cedilla).
fn is_text_accent_command(name: &str) -> bool {
    matches!(
        name,
        "'" | "`" | "^" | "\"" | "~" | "=" | "." | "u" | "v" | "c"
    )
}

/// The text-mode accent command applied to a base character, resolving to the composed Latin letter.
/// A base with no composed form for the accent returns the bare base character; an accent/base pair
/// outside the recognized set returns `None`, leaving the wrapper to fall back to verbatim.
// One cohesive accent-to-letter lookup table; splitting into helpers would only scatter it.
#[allow(clippy::too_many_lines)]
fn text_accent(name: &str, base: char) -> Option<&'static str> {
    if !is_text_accent_command(name) {
        return None;
    }
    let table: &[(char, &str)] = match name {
        "'" => &[
            ('a', "\u{e1}"),
            ('c', "\u{107}"),
            ('e', "\u{e9}"),
            ('i', "\u{ed}"),
            ('l', "\u{13a}"),
            ('n', "\u{144}"),
            ('o', "\u{f3}"),
            ('r', "\u{155}"),
            ('s', "\u{15b}"),
            ('u', "\u{fa}"),
            ('y', "\u{fd}"),
            ('z', "\u{17a}"),
            ('A', "\u{c1}"),
            ('C', "\u{106}"),
            ('E', "\u{c9}"),
            ('I', "\u{cd}"),
            ('L', "\u{139}"),
            ('N', "\u{143}"),
            ('O', "\u{d3}"),
            ('R', "\u{154}"),
            ('S', "\u{15a}"),
            ('U', "\u{da}"),
            ('Y', "\u{dd}"),
            ('Z', "\u{179}"),
        ],
        "`" => &[
            ('a', "\u{e0}"),
            ('e', "\u{e8}"),
            ('i', "\u{ec}"),
            ('o', "\u{f2}"),
            ('u', "\u{f9}"),
            ('A', "\u{c0}"),
            ('E', "\u{c8}"),
            ('I', "\u{cc}"),
            ('O', "\u{d2}"),
            ('U', "\u{d9}"),
        ],
        "^" => &[
            ('a', "\u{e2}"),
            ('c', "\u{109}"),
            ('e', "\u{ea}"),
            ('g', "\u{11d}"),
            ('h', "\u{125}"),
            ('i', "\u{ee}"),
            ('j', "\u{135}"),
            ('o', "\u{f4}"),
            ('s', "\u{15d}"),
            ('u', "\u{fb}"),
            ('w', "\u{175}"),
            ('y', "\u{177}"),
            ('A', "\u{c2}"),
            ('C', "\u{108}"),
            ('E', "\u{ca}"),
            ('G', "\u{11c}"),
            ('H', "\u{124}"),
            ('I', "\u{ce}"),
            ('J', "\u{134}"),
            ('O', "\u{d4}"),
            ('S', "\u{15c}"),
            ('U', "\u{db}"),
            ('W', "\u{174}"),
            ('Y', "\u{176}"),
        ],
        "\"" => &[
            ('a', "\u{e4}"),
            ('e', "\u{eb}"),
            ('i', "\u{ef}"),
            ('o', "\u{f6}"),
            ('u', "\u{fc}"),
            ('A', "\u{c4}"),
            ('E', "\u{cb}"),
            ('I', "\u{cf}"),
            ('O', "\u{d6}"),
            ('U', "\u{dc}"),
        ],
        "~" => &[
            ('a', "\u{e3}"),
            ('i', "\u{129}"),
            ('n', "\u{f1}"),
            ('o', "\u{f5}"),
            ('u', "\u{169}"),
            ('A', "\u{c3}"),
            ('I', "\u{128}"),
            ('N', "\u{d1}"),
            ('O', "\u{d5}"),
            ('U', "\u{168}"),
        ],
        "=" => &[
            ('a', "\u{101}"),
            ('e', "\u{113}"),
            ('i', "\u{12b}"),
            ('o', "\u{14d}"),
            ('u', "\u{16b}"),
            ('A', "\u{100}"),
            ('E', "\u{112}"),
            ('I', "\u{12a}"),
            ('O', "\u{14c}"),
            ('U', "\u{16a}"),
        ],
        "." => &[
            ('c', "\u{10b}"),
            ('e', "\u{117}"),
            ('g', "\u{121}"),
            ('z', "\u{17c}"),
            ('C', "\u{10a}"),
            ('E', "\u{116}"),
            ('G', "\u{120}"),
            ('I', "\u{130}"),
            ('Z', "\u{17b}"),
        ],
        "u" => &[
            ('a', "\u{103}"),
            ('e', "\u{115}"),
            ('g', "\u{11f}"),
            ('i', "\u{12d}"),
            ('o', "\u{14f}"),
            ('u', "\u{16d}"),
            ('A', "\u{102}"),
            ('E', "\u{114}"),
            ('G', "\u{11e}"),
            ('I', "\u{12c}"),
            ('O', "\u{14e}"),
            ('U', "\u{16c}"),
        ],
        "v" => &[
            ('a', "\u{1ce}"),
            ('c', "\u{10d}"),
            ('d', "\u{10f}"),
            ('e', "\u{11b}"),
            ('g', "\u{1e7}"),
            ('h', "\u{21f}"),
            ('i', "\u{1d0}"),
            ('j', "\u{1f0}"),
            ('k', "\u{1e9}"),
            ('l', "\u{13e}"),
            ('n', "\u{148}"),
            ('o', "\u{1d2}"),
            ('r', "\u{159}"),
            ('s', "\u{161}"),
            ('t', "\u{165}"),
            ('u', "\u{1d4}"),
            ('z', "\u{17e}"),
            ('A', "\u{1cd}"),
            ('C', "\u{10c}"),
            ('D', "\u{10e}"),
            ('E', "\u{11a}"),
            ('G', "\u{1e6}"),
            ('H', "\u{21e}"),
            ('I', "\u{1cf}"),
            ('K', "\u{1e8}"),
            ('L', "\u{13d}"),
            ('N', "\u{147}"),
            ('O', "\u{1d1}"),
            ('R', "\u{158}"),
            ('S', "\u{160}"),
            ('T', "\u{164}"),
            ('U', "\u{1d3}"),
            ('Z', "\u{17d}"),
        ],
        "c" => &[
            ('c', "\u{e7}"),
            ('e', "\u{229}"),
            ('h', "\u{1e29}"),
            ('o', "o\u{327}"),
            ('s', "\u{15f}"),
            ('t', "\u{163}"),
            ('C', "\u{c7}"),
            ('E', "\u{228}"),
            ('H', "\u{1e28}"),
            ('O', "O\u{327}"),
            ('S', "\u{15e}"),
            ('T', "\u{162}"),
        ],
        _ => return None,
    };
    for &(b, composed) in table {
        if b == base {
            return Some(composed);
        }
    }
    BARE_BASE.get(&base).copied()
}

/// A single-character string for each ASCII letter that an accent may fall back to bare. Holding the
/// `&'static str` views here lets `text_accent` return a `'static` slice for the no-composition case.
static BARE_BASE: std::sync::LazyLock<std::collections::BTreeMap<char, &'static str>> =
    std::sync::LazyLock::new(|| {
        const LETTERS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        LETTERS
            .char_indices()
            .filter_map(|(i, c)| Some((c, LETTERS.get(i..=i)?)))
            .collect()
    });

/// A foreign letter or ligature command resolved in a text wrapper (`\oe`→œ, `\ss`→ß, …), or `None`.
/// Only the commands that render to a single precomposed glyph are recognized; the rest fall back to
/// verbatim.
fn foreign_letter(name: &str) -> Option<char> {
    let c = match name {
        "oe" => '\u{153}',
        "OE" => '\u{152}',
        "ae" => '\u{e6}',
        "AE" => '\u{c6}',
        "o" => '\u{f8}',
        "O" => '\u{d8}',
        "ss" => '\u{df}',
        "l" => '\u{142}',
        "L" => '\u{141}',
        "aa" => '\u{e5}',
        "AA" => '\u{c5}',
        _ => return None,
    };
    Some(c)
}

/// A text-symbol command resolved in a text wrapper to its literal glyph, or `None`. Only the
/// commands that lower to an ordinary ASCII glyph are recognized.
fn text_symbol(name: &str) -> Option<char> {
    let c = match name {
        "textbackslash" => '\\',
        "textasciitilde" => '~',
        "textasciicircum" => '^',
        _ => return None,
    };
    Some(c)
}

/// The base character an accent command applies to, read from the tokens at `from`: a single bare
/// character, or a brace-wrapped single character, optionally preceded by one space. Returns the base
/// and the index just past it, or `None` if the following token is not a lone character.
fn accent_base(tokens: &[Token], from: usize) -> Option<(char, usize)> {
    let mut i = from;
    if matches!(tokens.get(i), Some(Token::Space)) {
        i += 1;
    }
    match tokens.get(i)? {
        Token::Char(c) => Some((*c, i + 1)),
        Token::GroupOpen => {
            let inner = tokens.get(i + 1)?;
            let base = match inner {
                Token::Char(c) => *c,
                _ => return None,
            };
            if matches!(tokens.get(i + 2), Some(Token::GroupClose)) {
                Some((base, i + 3))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The literal source of a dotless-letter control word (`\i`, `\j`) following an accent, read from the
/// tokens at `from` (after an optional leading space), with the index just past it. These bases have no
/// composed accented form, so the accent is dropped and the control word's source is kept verbatim.
fn accent_dotless_base(tokens: &[Token], from: usize) -> Option<(&'static str, usize)> {
    let mut i = from;
    if matches!(tokens.get(i), Some(Token::Space)) {
        i += 1;
    }
    match tokens.get(i)? {
        Token::Command(name) if name == "i" => Some(("\\i", i + 1)),
        Token::Command(name) if name == "j" => Some(("\\j", i + 1)),
        _ => None,
    }
}

/// Read a `{...}` group as text-mode content. Recognized backslash-escapes unescape to their literal
/// character; `~` becomes a non-breaking space; the spacing escapes (`\,`, `\;`, `\:`, `\!`) become
/// spacing pieces that break the run (unless `fold_spacing`, in which case they collapse into the run
/// as their spacing codepoint). Returns `None` if the group contains a math-mode switch, an
/// unrecognized control sequence, or a nested script/group, which we do not attempt to render.
// The `$` arm must precede the general-character arm, so it cannot fold into the unhandled-token arm.
#[allow(clippy::match_same_arms, clippy::too_many_lines)]
pub(super) fn parse_verbatim_group(
    tokens: &[Token],
    pos: &mut usize,
    mode: TextMode,
) -> Option<Vec<TextPiece>> {
    skip_spaces(tokens, pos);
    if !matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
        return None;
    }
    let math = mode == TextMode::Math;
    let mut probe = *pos + 1;
    let mut pieces: Vec<TextPiece> = Vec::new();
    let mut run = String::new();
    while let Some(tok) = tokens.get(probe) {
        let following_space = matches!(tokens.get(probe + 1), Some(Token::Space));
        match tok {
            Token::GroupClose => {
                *pos = probe + 1;
                if !run.is_empty() {
                    pieces.push(TextPiece::Run(finish_text_run(run, mode)));
                }
                return Some(pieces);
            }
            // `$…$` in a text wrapper re-enters math mode; in an operator-name group (already math)
            // a `$` has no meaning and is unhandled.
            Token::Char('$') if math => return None,
            Token::Char('$') => {
                let inner_start = probe + 1;
                let inner_end = (inner_start..tokens.len())
                    .find(|&i| matches!(tokens.get(i), Some(Token::Char('$'))))?;
                let inner = tokens.get(inner_start..inner_end)?;
                let mut inner_pos = 0;
                let atoms = parse_atoms(inner, &mut inner_pos, 0, false)?;
                if inner_pos != inner.len() {
                    return None;
                }
                if !run.is_empty() {
                    pieces.push(TextPiece::Run(finish_text_run(
                        std::mem::take(&mut run),
                        mode,
                    )));
                }
                pieces.push(TextPiece::Math(atoms));
                probe = inner_end;
            }
            // `~` is a non-breaking space; in math mode it also swallows a following space like a control word.
            Token::Char('~') => {
                run.push('\u{00A0}');
                if math && following_space {
                    probe += 1;
                }
            }
            // Bare `"` or backtick is unparsable in math mode (verbatim fallback); in a text
            // wrapper both are ordinary literals.
            Token::Char('"' | '`') if math => return None,
            Token::Char(c) => run.push(*c),
            Token::Number(digits) => run.push_str(digits),
            // Math-mode inter-token space is not significant; a text wrapper keeps it literal.
            Token::Space => {
                if !math {
                    run.push(' ');
                }
            }
            // Subscript/superscript markers are literal characters in text mode.
            Token::Sub => run.push('_'),
            Token::Sup => run.push('^'),
            // `\(…\)`/`\[…\]` in a text wrapper re-enter math mode like `$…$`; the markers mean
            // nothing in an operator-name group (already math).
            Token::Command(name) if !math && (name == "(" || name == "[") => {
                let close = if name == "(" { ")" } else { "]" };
                let inner_start = probe + 1;
                let inner_end = (inner_start..tokens.len())
                    .find(|&i| matches!(tokens.get(i), Some(Token::Command(c)) if c == close))?;
                let inner = tokens.get(inner_start..inner_end)?;
                let mut inner_pos = 0;
                let atoms = parse_atoms(inner, &mut inner_pos, 0, false)?;
                if inner_pos != inner.len() {
                    return None;
                }
                if !run.is_empty() {
                    pieces.push(TextPiece::Run(finish_text_run(
                        std::mem::take(&mut run),
                        mode,
                    )));
                }
                pieces.push(TextPiece::Math(atoms));
                probe = inner_end;
            }
            Token::Command(name) => {
                probe = apply_text_command(
                    TextCommand {
                        name,
                        mode,
                        math,
                        following_space,
                    },
                    tokens,
                    probe,
                    &mut run,
                    &mut pieces,
                )?;
            }
            // In a text wrapper an inner group is transparent (ligatures kept per group); in math
            // mode a nested group is unhandled.
            Token::GroupOpen if !math => {
                if !run.is_empty() {
                    pieces.push(TextPiece::Run(finish_text_run(
                        std::mem::take(&mut run),
                        mode,
                    )));
                }
                let mut nested = probe;
                let inner = parse_verbatim_group(tokens, &mut nested, mode)?;
                if inner.is_empty() {
                    // An empty inner group is still a distinct (empty) text segment.
                    pieces.push(TextPiece::Run(String::new()));
                } else {
                    pieces.extend(inner);
                }
                probe = nested - 1;
            }
            Token::GroupOpen => return None,
        }
        probe += 1;
    }
    None
}

/// The context a control word inside a verbatim group is resolved in.
#[derive(Clone, Copy)]
struct TextCommand<'a> {
    name: &'a str,
    mode: TextMode,
    /// Whether the wrapper is math mode (`\operatorname`) rather than a text wrapper.
    math: bool,
    /// Whether a space immediately follows, which a control word swallows as in TeX.
    following_space: bool,
}

/// Resolve a control word inside a verbatim group, appending its rendering to `run` (or flushing a
/// space piece to `pieces`), and return the probe position the scan should continue from; the
/// caller's loop advances one further past it. Returns `None` for a command the wrapper does not
/// resolve, leaving the whole expression to fall back to verbatim.
fn apply_text_command(
    ctx: TextCommand<'_>,
    tokens: &[Token],
    probe: usize,
    run: &mut String,
    pieces: &mut Vec<TextPiece>,
) -> Option<usize> {
    let TextCommand {
        name,
        mode,
        math,
        following_space,
    } = ctx;
    // A control symbol or word that resolves to a single glyph swallows one following run of spaces.
    let swallow = |probe: usize| if following_space { probe + 1 } else { probe };
    if let Some(c) = text_escape_char(name) {
        // The escaped space is a non-breaking space in math mode, an ordinary one in text.
        let pushed = if c == ' ' && math { '\u{00A0}' } else { c };
        run.push(pushed);
        Some(swallow(probe))
    } else if name == "ldots" {
        // `\ldots` is the only ellipsis command resolved here; others leave the expression verbatim.
        run.push('\u{2026}');
        Some(swallow(probe))
    } else if let Some(space) = text_space(name) {
        if math {
            run.push(space.codepoint());
            Some(swallow(probe))
        } else {
            if !run.is_empty() {
                pieces.push(TextPiece::Run(finish_text_run(std::mem::take(run), mode)));
            }
            pieces.push(TextPiece::Space(space));
            Some(probe)
        }
    } else if !math && is_text_accent_command(name) {
        // An accent over `\i`/`\j` has no composed form: drop the accent, emit the word literally.
        if let Some((literal, next)) = accent_dotless_base(tokens, probe + 1) {
            run.push_str(literal);
            return Some(next - 1);
        }
        // An accent composes with its base into one letter; an unparsable base leaves the wrapper verbatim.
        let (base, next) = accent_base(tokens, probe + 1)?;
        run.push_str(text_accent(name, base)?);
        Some(next - 1)
    } else if !math && let Some(c) = foreign_letter(name) {
        run.push(c);
        Some(swallow(probe))
    } else if !math && let Some(c) = text_symbol(name) {
        run.push(c);
        Some(swallow(probe))
    } else if !math && name == "quad" {
        // A text-mode `\quad` folds into the run as a single en quad.
        run.push('\u{2000}');
        Some(swallow(probe))
    } else {
        None
    }
}
