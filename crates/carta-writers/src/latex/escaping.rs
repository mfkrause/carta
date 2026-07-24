//! LaTeX escaping for literal text, URLs, and cross-reference labels.

use crate::common::clean_prefix_len;

use super::to_label;

/// Selects the escaping policy for a run of literal text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EscapeMode {
    /// Running prose.
    Text,
    /// Inside a `\texttt{…}` group, where spaces and a few extra glyphs gain escapes.
    Code,
}

pub(super) fn is_latex_format(format: &str) -> bool {
    format.eq_ignore_ascii_case("latex") || format.eq_ignore_ascii_case("tex")
}

/// Escape a run of literal text for the given context.
pub(super) fn escape(text: &str, mode: EscapeMode) -> String {
    escape_smart(text, mode, true)
}

/// Escape text for LaTeX. With `smart`, Unicode smart punctuation (curly quotes, en/em dashes, the
/// ellipsis) renders as its TeX ligature; otherwise it passes through as the literal Unicode
/// character. The non-breaking space and the `--` ligature guard are structural and are emitted
/// regardless of `smart`.
pub(super) fn escape_smart(text: &str, mode: EscapeMode, smart: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let code = mode == EscapeMode::Code;
    let is_trigger = |byte: u8| {
        matches!(
            byte,
            b'&' | b'%'
                | b'#'
                | b'_'
                | b'$'
                | b'{'
                | b'}'
                | b'^'
                | b'['
                | b']'
                | b'~'
                | b'\\'
                | b'<'
                | b'>'
                | b'|'
                | b'\''
                | b'-'
        ) || byte >= 0x80
            || (code && matches!(byte, b' ' | b'`'))
    };
    let mut rest = text;
    loop {
        let clean = clean_prefix_len(rest, is_trigger);
        let Some((head, tail)) = rest.split_at_checked(clean) else {
            out.push_str(rest);
            break;
        };
        out.push_str(head);
        let mut chars = tail.chars();
        let Some(ch) = chars.next() else { break };
        let next = chars.clone().next();
        match ch {
            '&' | '%' | '#' | '_' | '$' | '{' | '}' => {
                out.push('\\');
                out.push(ch);
            }
            '^' => out.push_str("\\^{}"),
            '[' => out.push_str("{[}"),
            ']' => out.push_str("{]}"),
            '~' => push_control_word(&mut out, "\\textasciitilde", next, mode),
            '\\' => push_control_word(&mut out, "\\textbackslash", next, mode),
            '<' => push_control_word(&mut out, "\\textless", next, mode),
            '>' => push_control_word(&mut out, "\\textgreater", next, mode),
            '|' => push_control_word(&mut out, "\\textbar", next, mode),
            '\'' => push_control_word(&mut out, "\\textquotesingle", next, mode),
            '-' if next == Some('-') => out.push_str("-\\/"),
            // A literal hyphen abutting a smart dash would merge into a longer ligature.
            '-' if mode == EscapeMode::Text
                && smart
                && matches!(next, Some('\u{2013}' | '\u{2014}')) =>
            {
                out.push_str("-\\/");
            }
            ' ' if mode == EscapeMode::Code => out.push_str("\\ "),
            '`' if mode == EscapeMode::Code => out.push_str("\\textasciigrave{}"),
            '\u{a0}' => out.push('~'),
            '\u{2026}' if mode == EscapeMode::Text && smart => {
                push_control_word(&mut out, "\\ldots", next, mode);
            }
            '\u{2013}' if mode == EscapeMode::Text && smart => out.push_str("--"),
            '\u{2014}' if mode == EscapeMode::Text && smart => out.push_str("---"),
            '\u{2018}' if mode == EscapeMode::Text && smart => {
                out.push('`');
                guard_quote_ligature(&mut out, next);
            }
            '\u{2019}' if mode == EscapeMode::Text && smart => {
                out.push('\'');
                guard_quote_ligature(&mut out, next);
            }
            '\u{201C}' if mode == EscapeMode::Text && smart => {
                out.push_str("``");
                guard_quote_ligature(&mut out, next);
            }
            '\u{201D}' if mode == EscapeMode::Text && smart => {
                out.push_str("''");
                guard_quote_ligature(&mut out, next);
            }
            other => out.push(other),
        }
        rest = chars.as_str();
    }
    out
}

/// Insert a thin-space ligature guard after a smart-quote glyph when the next character also opens
/// with a quote glyph (another smart quote, or a literal backtick). Without it, adjacent quotes such
/// as the two apostrophes of `’’` would fuse into a single closing double quote.
fn guard_quote_ligature(out: &mut String, next: Option<char>) {
    if matches!(
        next,
        Some('\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}' | '`')
    ) {
        out.push_str("\\,");
    }
}

/// Emit a control-word command and the separator that stops it from absorbing the following
/// character. In code context the command always closes with an empty group; in text context the
/// separator depends on what follows: a space before a letter, an empty group before whitespace or
/// the end of the run, and nothing before other glyphs (which already terminate the command).
fn push_control_word(out: &mut String, command: &str, next: Option<char>, mode: EscapeMode) {
    out.push_str(command);
    match mode {
        EscapeMode::Code => out.push_str("{}"),
        EscapeMode::Text => match next {
            Some(following) if following.is_alphabetic() => out.push(' '),
            Some(following) if following.is_whitespace() => out.push_str("{}"),
            None => out.push_str("{}"),
            Some(_) => {}
        },
    }
}

/// Escape a URL for `\href`/`\url`/`\includegraphics`: percent-encode the bytes LaTeX cannot carry
/// in a URL argument, map a backslash to a forward slash, and escape the surviving `#` and `%`.
pub(super) fn escape_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        match ch {
            '\\' => out.push('/'),
            '#' => out.push_str("\\#"),
            '%' => out.push_str("\\%"),
            ' ' | '"' | '<' | '>' | '[' | ']' | '^' | '`' | '{' | '|' | '}' => {
                percent_encode(ch, &mut out);
            }
            other if !other.is_ascii() || (other as u32) < 0x20 => percent_encode(other, &mut out),
            other => out.push(other),
        }
    }
    out
}

/// The label naming an internal cross-reference, derived from a link's fragment. The fragment is
/// first escaped as a URL, then reduced to a single `\hyperref`-safe token by [`to_label`].
pub(super) fn cross_reference_label(reference: &str) -> String {
    let mut escaped = String::with_capacity(reference.len());
    for ch in reference.chars() {
        match ch {
            '\\' => escaped.push('/'),
            '#' => escaped.push_str("\\#"),
            '%' => escaped.push_str("\\%"),
            '[' | ']' | '^' | '`' | '{' | '|' | '}' => percent_encode(ch, &mut escaped),
            other => escaped.push(other),
        }
    }
    to_label(&escaped)
}

fn percent_encode(ch: char, out: &mut String) {
    let mut buffer = [0u8; 4];
    for byte in ch.encode_utf8(&mut buffer).bytes() {
        out.push_str("\\%");
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0x0f));
    }
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'A' + value - 10) as char,
    }
}
