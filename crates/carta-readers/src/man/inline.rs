//! Font handling, inline tokenizing, and escape-sequence scanning for `man` text.

use carta_ast::Inline;

use crate::inline_text::trim_inline_ends;

use super::requests::split_args;
use super::{MAX_STRING_DEPTH, Strings};

/// The active typeface for a run of text. `\f(BI` and the `.BI`/`.IB` macros render bold-italic as
/// emphasis wrapping strong. The constant-width faces (`\f(CW`, `\fC`, `.CW`) render as inline code,
/// with a bold or italic constant-width face wrapping that code in the corresponding markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Font {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono,
    MonoBold,
    MonoItalic,
}

impl Font {
    /// Wraps already-built inline content in the markup for this font; roman content is unwrapped.
    /// A constant-width face collapses its content to a single inline-code span.
    pub(super) fn wrap(self, inlines: Vec<Inline>) -> Vec<Inline> {
        if inlines.is_empty() {
            return Vec::new();
        }
        self.wrap_forced(inlines)
    }

    /// Wraps the inlines in this font's markup unconditionally, even when they are empty. A
    /// single-font macro called with an explicit argument keeps its styled wrapper around empty
    /// content, whereas a font run that produces nothing collapses (see [`wrap`]).
    pub(super) fn wrap_forced(self, inlines: Vec<Inline>) -> Vec<Inline> {
        match self {
            Font::Regular => inlines,
            Font::Bold => vec![Inline::Strong(inlines)],
            Font::Italic => vec![Inline::Emph(inlines)],
            Font::BoldItalic => vec![Inline::Emph(vec![Inline::Strong(inlines)])],
            Font::Mono => vec![code_inline(&inlines)],
            Font::MonoBold => vec![Inline::Strong(vec![code_inline(&inlines)])],
            Font::MonoItalic => vec![Inline::Emph(vec![code_inline(&inlines)])],
        }
    }
}

/// Collapses a run of inline content into a single inline-code span, recovering its literal text.
fn code_inline(inlines: &[Inline]) -> Inline {
    let mut text = String::new();
    collect_code_text(inlines, &mut text);
    Inline::Code(Box::default(), text.into())
}

fn collect_code_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Str(s) => out.push_str(s),
            Inline::Space => out.push(' '),
            Inline::Strong(xs) | Inline::Emph(xs) => collect_code_text(xs, out),
            _ => {}
        }
    }
}

/// The font a single-font macro selects: `.B` is bold, every other (`.I`) is italic.
pub(super) fn single_font(name: &str) -> Font {
    if name == "B" {
        Font::Bold
    } else {
        Font::Italic
    }
}

/// The two alternating fonts of an alternating font macro, applied to arguments in turn.
pub(super) fn fonts_for(name: &str) -> [Font; 2] {
    match name {
        "BR" => [Font::Bold, Font::Regular],
        "RB" => [Font::Regular, Font::Bold],
        "BI" => [Font::Bold, Font::Italic],
        "IB" => [Font::Italic, Font::Bold],
        "RI" => [Font::Regular, Font::Italic],
        _ => [Font::Italic, Font::Regular],
    }
}

/// Renders a single-font macro (`.B`/`.I`): the whole argument is read as roman text and then
/// wrapped once in the macro's font, so an inner `\f` font change nests inside that font rather than
/// replacing it.
pub(super) fn font_macro(font: Font, text: &str, strings: &Strings) -> Vec<Inline> {
    font.wrap(tokenize(text, Font::Regular, strings))
}

/// Renders an alternating font macro: each argument takes the next font in the cycle, is read as
/// roman text, and is wrapped in that font; the rendered arguments abut with no separating space.
pub(super) fn alternating(rest: &str, fonts: [Font; 2], strings: &Strings) -> Vec<Inline> {
    let mut out = Vec::new();
    for (index, arg) in split_args(rest).into_iter().enumerate() {
        let font = fonts.get(index % 2).copied().unwrap_or(Font::Regular);
        out.extend(font.wrap(tokenize(&arg, Font::Regular, strings)));
    }
    out
}

/// Renders a `.OP` command-option synopsis: the option name (the first argument) is set bold and an
/// optional argument (the rest) roman, the whole bracketed as optional: `[ -name argument ]`.
pub(super) fn option_synopsis(rest: &str, strings: &Strings) -> Vec<Inline> {
    let args = split_args(rest);
    let mut out = vec![Inline::Str("[".into())];
    if let Some(name) = args.first() {
        out.push(Inline::Space);
        out.extend(font_macro(Font::Bold, name, strings));
    }
    let argument = args.get(1..).unwrap_or(&[]).join(" ");
    if !argument.is_empty() {
        out.push(Inline::Space);
        out.extend(tokenize(&argument, Font::Regular, strings));
    }
    out.push(Inline::Space);
    out.push(Inline::Str("]".into()));
    out
}

/// A scanned character together with the font in effect, or an inter-word separator carrying the
/// literal whitespace character it stands for (so a verbatim region can preserve a tab).
enum Atom {
    Char(Font, char),
    Space(char),
}

/// Tokenizes a line of `man` text into inlines: words become [`Inline::Str`], runs of whitespace a
/// single [`Inline::Space`], and font runs wrap in the appropriate markup. Leading and trailing
/// spaces are dropped.
pub(super) fn tokenize(text: &str, start_font: Font, strings: &Strings) -> Vec<Inline> {
    let atoms = scan(text, start_font, strings);
    let mut result: Vec<Inline> = Vec::new();
    let mut run: Vec<Inline> = Vec::new();
    let mut run_font = Font::Regular;
    let mut word = String::new();
    let mut word_font = Font::Regular;
    let mut pending_space = false;

    let commit_word = |word: &mut String,
                       word_font: Font,
                       run: &mut Vec<Inline>,
                       run_font: &mut Font,
                       result: &mut Vec<Inline>,
                       pending_space: &mut bool| {
        if word.is_empty() {
            return;
        }
        let text = std::mem::take(word);
        if !run.is_empty() && word_font == *run_font {
            if *pending_space {
                run.push(Inline::Space);
            }
            run.push(Inline::Str(text.into()));
        } else {
            flush_run(run, *run_font, result);
            if *pending_space {
                push_space(result);
            }
            *run_font = word_font;
            run.push(Inline::Str(text.into()));
        }
        *pending_space = false;
    };

    for atom in atoms {
        match atom {
            Atom::Char(font, c) => {
                if !word.is_empty() && font != word_font {
                    commit_word(
                        &mut word,
                        word_font,
                        &mut run,
                        &mut run_font,
                        &mut result,
                        &mut pending_space,
                    );
                }
                if word.is_empty() {
                    word_font = font;
                }
                word.push(c);
            }
            Atom::Space(_) => {
                commit_word(
                    &mut word,
                    word_font,
                    &mut run,
                    &mut run_font,
                    &mut result,
                    &mut pending_space,
                );
                pending_space = true;
            }
        }
    }
    commit_word(
        &mut word,
        word_font,
        &mut run,
        &mut run_font,
        &mut result,
        &mut pending_space,
    );
    flush_run(&mut run, run_font, &mut result);
    trim_inline_ends(&mut result);
    result
}

fn flush_run(run: &mut Vec<Inline>, run_font: Font, result: &mut Vec<Inline>) {
    if !run.is_empty() {
        result.extend(run_font.wrap(std::mem::take(run)));
    }
}

/// Appends a single top-level space, coalescing with any space already present.
fn push_space(result: &mut Vec<Inline>) {
    if !result.is_empty() && !matches!(result.last(), Some(Inline::Space)) {
        result.push(Inline::Space);
    }
}

/// Reduces a line to plain text for a verbatim region: escapes and special characters resolve, font
/// markup is discarded, and literal spacing is preserved.
pub(super) fn flatten(text: &str, strings: &Strings) -> String {
    let mut out = String::new();
    for atom in scan(text, Font::Regular, strings) {
        match atom {
            Atom::Char(_, c) | Atom::Space(c) => out.push(c),
        }
    }
    out
}

/// Scans a line into atoms, resolving escape sequences and interpolating named strings.
fn scan(text: &str, start_font: Font, strings: &Strings) -> Vec<Atom> {
    let mut atoms = Vec::new();
    let mut font = start_font;
    let mut previous = start_font;
    scan_into(text, &mut font, &mut previous, &mut atoms, strings, 0);
    atoms
}

/// Scans `text` into `atoms`, carrying the running font across the call so an interpolated `\*`
/// string can change the font for the remainder of the line. Font escapes (`\f…`) update the font;
/// an inline comment (`\"`/`\#`) ends the line; a `\*` string is expanded by re-scanning its value,
/// bounded by [`MAX_STRING_DEPTH`] so a self-referential definition cannot loop forever.
// Escape arms are listed separately by groff semantics even where two reduce to the same body.
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn scan_into(
    text: &str,
    font: &mut Font,
    previous: &mut Font,
    atoms: &mut Vec<Atom>,
    strings: &Strings,
    depth: usize,
) {
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ' ' || c == '\t' {
            atoms.push(Atom::Space(c));
            continue;
        }
        if c != '\\' {
            atoms.push(Atom::Char(*font, c));
            continue;
        }
        let Some(&escape) = chars.peek() else {
            break;
        };
        match escape {
            'f' => {
                chars.next();
                let name = read_escape_name(&mut chars);
                apply_font(&name, font, previous);
            }
            '"' | '#' => break,
            '-' => {
                chars.next();
                atoms.push(Atom::Char(*font, '-'));
            }
            'e' | '\\' => {
                chars.next();
                atoms.push(Atom::Char(*font, '\\'));
            }
            '.' => {
                chars.next();
                atoms.push(Atom::Char(*font, '.'));
            }
            // Both separate words; the tab keeps its character so verbatim regions preserve it.
            ' ' => {
                chars.next();
                atoms.push(Atom::Space(' '));
            }
            't' => {
                chars.next();
                atoms.push(Atom::Space('\t'));
            }
            '~' => {
                chars.next();
                atoms.push(Atom::Char(*font, '\u{00a0}'));
            }
            '0' => {
                chars.next();
                atoms.push(Atom::Char(*font, '\u{2007}'));
            }
            '^' => {
                chars.next();
                atoms.push(Atom::Char(*font, '\u{200a}'));
            }
            '|' => {
                chars.next();
                atoms.push(Atom::Char(*font, '\u{2006}'));
            }
            // Emit nothing: `\c`, the zero-width `\&` family, and the motions `\u`/`\d` (no argument).
            '&' | ')' | ',' | '/' | ':' | '!' | '%' | '{' | '}' | 'c' | 'u' | 'd' => {
                chars.next();
            }
            '(' => {
                chars.next();
                let name: String = (&mut chars).take(2).collect();
                push_chars(atoms, *font, special_char(&name));
            }
            '[' => {
                chars.next();
                let name = read_delimited(&mut chars, ']');
                push_chars(atoms, *font, bracket_char(&name));
            }
            '*' => {
                chars.next();
                let name = read_escape_name(&mut chars);
                if depth < MAX_STRING_DEPTH
                    && let Some(value) = strings.get(&name)
                {
                    scan_into(value, font, previous, atoms, strings, depth + 1);
                }
            }
            's' => {
                chars.next();
                skip_size(&mut chars);
            }
            // `\n` reads a number-register name and `\k` a position-register name; both are discarded.
            'n' | 'k' => {
                chars.next();
                let _ = read_escape_name(&mut chars);
            }
            // `\z` outputs the next glyph with no width; the glyph is dropped here.
            'z' => {
                chars.next();
                chars.next();
            }
            // Name-taking escapes (one char, `(xx`, or `[name]`) emitting no text: `\m`/`\M`, `\F`,
            // `\g`, `\V`, `\Y`, `\$N`.
            'm' | 'M' | 'F' | 'g' | 'V' | 'Y' | '$' => {
                chars.next();
                let _ = read_escape_name(&mut chars);
            }
            // `\p` (break the output line) and `\a` (leader) both produce no text.
            'p' | 'a' => {
                chars.next();
            }
            // `\C'name'` names a glyph with an explicit delimiter, like `\[name]`.
            'C' => {
                chars.next();
                let name = match chars.next() {
                    Some(delim) => read_delimited(&mut chars, delim),
                    None => String::new(),
                };
                push_chars(atoms, *font, bracket_char(&name));
            }
            'h' | 'v' | 'w' | 'o' | 'b' | 'l' | 'L' | 'D' | 'N' | 'R' | 'A' | 'Z' | 'X' | 'B' => {
                chars.next();
                skip_delimited_arg(&mut chars);
            }
            other => {
                chars.next();
                atoms.push(Atom::Char(*font, other));
            }
        }
    }
}

fn push_chars(atoms: &mut Vec<Atom>, font: Font, mapped: Option<char>) {
    atoms.push(Atom::Char(font, mapped.unwrap_or('\u{fffd}')));
}

/// Reads an escape name after `\f`, `\*` or `\n`: one character, a two-character `(xx` name, or a
/// `[name]` group.
fn read_escape_name(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    match chars.peek() {
        Some('(') => {
            chars.next();
            chars.take(2).collect()
        }
        Some('[') => {
            chars.next();
            read_delimited(chars, ']')
        }
        Some(_) => chars.next().map(String::from).unwrap_or_default(),
        None => String::new(),
    }
}

pub(super) fn read_delimited(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    close: char,
) -> String {
    let mut name = String::new();
    for c in chars.by_ref() {
        if c == close {
            break;
        }
        name.push(c);
    }
    name
}

/// Skips an argument delimited by a repeated character, as in `\h'amount'`.
fn skip_delimited_arg(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    let Some(delim) = chars.next() else {
        return;
    };
    for c in chars.by_ref() {
        if c == delim {
            break;
        }
    }
}

/// Skips a `\s` size argument: an optional sign and one or two digits, or a delimited or grouped
/// form.
fn skip_size(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.peek() {
        Some('(') => {
            chars.next();
            chars.next();
            chars.next();
        }
        Some('[') => {
            chars.next();
            read_delimited(chars, ']');
        }
        Some('\'') => {
            chars.next();
            read_delimited(chars, '\'');
        }
        _ => {
            if matches!(chars.peek(), Some('+' | '-')) {
                chars.next();
            }
            for _ in 0..2 {
                if matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
                    chars.next();
                } else {
                    break;
                }
            }
        }
    }
}

/// Applies a `\f` font name to the running font, remembering the previous font so `P` (or an empty
/// name) can return to it.
// The named roman fonts are spelled out; any unrecognized name also falls back to roman.
#[allow(clippy::match_same_arms)]
fn apply_font(name: &str, font: &mut Font, previous: &mut Font) {
    let next = match name {
        "B" => Font::Bold,
        "I" => Font::Italic,
        "BI" | "IB" => Font::BoldItalic,
        "C" | "CW" | "CR" => Font::Mono,
        "CB" => Font::MonoBold,
        "CI" => Font::MonoItalic,
        "R" => Font::Regular,
        "P" | "" => {
            std::mem::swap(font, previous);
            return;
        }
        _ => Font::Regular,
    };
    *previous = *font;
    *font = next;
}

/// Resolves a `\[name]` escape: a `uXXXX` Unicode escape or a special-character name.
fn bracket_char(name: &str) -> Option<char> {
    if let Some(hex) = name.strip_prefix('u') {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
    }
    special_char(name)
}

/// Maps a special-character name (`\(xx`, `\[name]`) to its character; unknown names yield `None`,
/// which the caller renders as the replacement character.
// One name per arm keeps the glyph table legible even where distinct names share a character.
#[allow(clippy::match_same_arms, clippy::too_many_lines)]
fn special_char(name: &str) -> Option<char> {
    let c = match name {
        // Dashes, hyphens, and quotation.
        "hy" => '\u{2010}',
        "en" => '\u{2013}',
        "em" => '\u{2014}',
        "lq" => '\u{201c}',
        "rq" => '\u{201d}',
        "oq" => '\u{2018}',
        "cq" => '\u{2019}',
        "aq" => '\'',
        "dq" => '"',
        "Bq" => '\u{201e}',
        "bq" => '\u{201a}',
        "Fo" => '\u{00ab}',
        "Fc" => '\u{00bb}',
        "fo" => '\u{2039}',
        "fc" => '\u{203a}',
        "ga" => '`',
        "aa" => '\u{00b4}',
        "ha" => '^',
        "ti" => '~',
        "ul" => '_',
        "ru" => '_',
        "rs" => '\\',
        "sl" => '/',
        // Bullets, marks, and shapes.
        "bu" => '\u{00b7}',
        "ci" => '\u{25cb}',
        "sq" => '\u{25a1}',
        "lz" => '\u{25ca}',
        "dg" => '\u{2020}',
        "dd" => '\u{2021}',
        "ps" => '\u{00b6}',
        "sc" => '\u{00a7}',
        "lh" => '\u{261c}',
        "rh" => '\u{261e}',
        "co" => '\u{00a9}',
        "rg" => '\u{00ae}',
        "tm" => '\u{2122}',
        "fm" => '\u{2032}',
        "sd" => '\u{2033}',
        "de" => '\u{00b0}',
        "mc" => '\u{00b5}',
        "%0" => '\u{2030}',
        // Punctuation and bars.
        "at" => '@',
        "sh" => '#',
        "or" => '|',
        "ba" => '|',
        "br" => '\u{2502}',
        "bb" => '\u{00a6}',
        "rn" => '\u{203e}',
        "ct" => '\u{00a2}',
        // Currency.
        "Do" => '$',
        "Eu" | "eu" => '\u{20ac}',
        "Po" => '\u{00a3}',
        "Ye" => '\u{00a5}',
        "Cs" => '\u{00a4}',
        // Fractions and ligatures.
        "12" => '\u{00bd}',
        "14" => '\u{00bc}',
        "34" => '\u{00be}',
        "ff" => '\u{fb00}',
        "fi" => '\u{fb01}',
        "fl" => '\u{fb02}',
        "Fi" => '\u{fb03}',
        "Fl" => '\u{fb04}',
        // Accented letters and accents.
        "oA" => '\u{00c5}',
        "oa" => '\u{00e5}',
        "/L" => '\u{0141}',
        "/l" => '\u{0142}',
        "/O" => '\u{00d8}',
        "/o" => '\u{00f8}',
        "a-" => '\u{00af}',
        "a." => '\u{02d9}',
        "ad" => '\u{00a8}',
        "ah" => '\u{02c7}',
        "a^" => '^',
        // Diaeresis.
        ":a" => '\u{00e4}',
        ":e" => '\u{00eb}',
        ":i" => '\u{00ef}',
        ":o" => '\u{00f6}',
        ":u" => '\u{00fc}',
        ":y" => '\u{00ff}',
        ":A" => '\u{00c4}',
        ":E" => '\u{00cb}',
        ":I" => '\u{00cf}',
        ":O" => '\u{00d6}',
        ":U" => '\u{00dc}',
        ":Y" => '\u{0178}',
        // Acute accent.
        "'a" => '\u{00e1}',
        "'c" => '\u{0107}',
        "'e" => '\u{00e9}',
        "'i" => '\u{00ed}',
        "'o" => '\u{00f3}',
        "'u" => '\u{00fa}',
        "'y" => '\u{00fd}',
        "'A" => '\u{00c1}',
        "'C" => '\u{0106}',
        "'E" => '\u{00c9}',
        "'I" => '\u{00cd}',
        "'O" => '\u{00d3}',
        "'U" => '\u{00da}',
        "'Y" => '\u{00dd}',
        // Grave accent.
        "`a" => '\u{00e0}',
        "`e" => '\u{00e8}',
        "`i" => '\u{00ec}',
        "`o" => '\u{00f2}',
        "`u" => '\u{00f9}',
        "`A" => '\u{00c0}',
        "`E" => '\u{00c8}',
        "`I" => '\u{00cc}',
        "`O" => '\u{00d2}',
        "`U" => '\u{00d9}',
        // Circumflex.
        "^a" => '\u{00e2}',
        "^e" => '\u{00ea}',
        "^i" => '\u{00ee}',
        "^o" => '\u{00f4}',
        "^u" => '\u{00fb}',
        "^A" => '\u{00c2}',
        "^E" => '\u{00ca}',
        "^I" => '\u{00ce}',
        "^O" => '\u{00d4}',
        "^U" => '\u{00db}',
        // Tilde.
        "~a" => '\u{00e3}',
        "~n" => '\u{00f1}',
        "~o" => '\u{00f5}',
        "~A" => '\u{00c3}',
        "~N" => '\u{00d1}',
        "~O" => '\u{00d5}',
        // Cedilla.
        ",c" => '\u{00e7}',
        ",C" => '\u{00c7}',
        // Other Latin letters and ligatures.
        "ss" => '\u{00df}',
        "ae" => '\u{00e6}',
        "AE" => '\u{00c6}',
        "oe" => '\u{0153}',
        "OE" => '\u{0152}',
        "-D" => '\u{00d0}',
        "Sd" => '\u{00f0}',
        "TP" => '\u{00de}',
        "Tp" => '\u{00fe}',
        // Mathematical operators and relations.
        "pl" => '+',
        "mi" => '\u{2212}',
        "mu" => '\u{00d7}',
        "di" => '\u{00f7}',
        "+-" => '\u{00b1}',
        "**" => '\u{2217}',
        "c*" => '\u{2297}',
        "c+" => '\u{2295}',
        "<=" => '\u{2264}',
        ">=" => '\u{2265}',
        "!=" => '\u{2260}',
        "==" => '\u{2261}',
        "->" => '\u{2192}',
        "<-" => '\u{2190}',
        "eq" => '=',
        "no" => '\u{00ac}',
        "sr" => '\u{221a}',
        "is" => '\u{222b}',
        "pd" => '\u{2202}',
        "gr" => '\u{2207}',
        "fa" => '\u{2200}',
        "te" => '\u{2203}',
        "if" => '\u{221e}',
        "pt" => '\u{221d}',
        "es" => '\u{2205}',
        "ca" => '\u{2229}',
        "cu" => '\u{222a}',
        "sb" => '\u{2282}',
        "sp" => '\u{2283}',
        "ib" => '\u{2286}',
        "ip" => '\u{2287}',
        "mo" => '\u{2208}',
        "nm" => '\u{2209}',
        "pp" => '\u{22a5}',
        "3d" => '\u{2234}',
        "Ah" => '\u{2135}',
        "Im" => '\u{2111}',
        "Re" => '\u{211c}',
        "wp" => '\u{2118}',
        // Angle brackets and extensible bars.
        "la" => '\u{27e8}',
        "ra" => '\u{27e9}',
        "va" => '\u{2195}',
        "an" => '\u{23af}',
        // Greek lowercase.
        "*a" => '\u{03b1}',
        "*b" => '\u{03b2}',
        "*g" => '\u{03b3}',
        "*d" => '\u{03b4}',
        "*e" => '\u{03b5}',
        "*z" => '\u{03b6}',
        "*y" => '\u{03b7}',
        "*h" => '\u{03b8}',
        "*i" => '\u{03b9}',
        "*k" => '\u{03ba}',
        "*l" => '\u{03bb}',
        "*m" => '\u{03bc}',
        "*n" => '\u{03bd}',
        "*c" => '\u{03be}',
        "*o" => '\u{03bf}',
        "*p" => '\u{03c0}',
        "*r" => '\u{03c1}',
        "ts" => '\u{03c2}',
        "*s" => '\u{03c3}',
        "*t" => '\u{03c4}',
        "*u" => '\u{03c5}',
        "*f" => '\u{03c6}',
        "*x" => '\u{03c7}',
        "*q" => '\u{03c8}',
        "*w" => '\u{03c9}',
        // Greek uppercase.
        "*A" => '\u{0391}',
        "*B" => '\u{0392}',
        "*G" => '\u{0393}',
        "*D" => '\u{0394}',
        "*E" => '\u{0395}',
        "*Z" => '\u{0396}',
        "*Y" => '\u{0397}',
        "*H" => '\u{0398}',
        "*I" => '\u{0399}',
        "*K" => '\u{039a}',
        "*L" => '\u{039b}',
        "*M" => '\u{039c}',
        "*N" => '\u{039d}',
        "*C" => '\u{039e}',
        "*O" => '\u{039f}',
        "*P" => '\u{03a0}',
        "*R" => '\u{03a1}',
        "*S" => '\u{03a3}',
        "*T" => '\u{03a4}',
        "*U" => '\u{03a5}',
        "*F" => '\u{03a6}',
        "*X" => '\u{03a7}',
        "*Q" => '\u{03a8}',
        "*W" => '\u{03a9}',
        _ => return None,
    };
    Some(c)
}
