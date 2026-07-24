//! Character and token escaping, image, math, and passthrough helpers for the Typst writer.

use carta_ast::{Attr, Format, Inline, MathType, QuoteType, Target, to_plain_text};

use crate::common::{attribute_value, clean_prefix_len};

/// Render an `image(..)` call: the path, then any `height`/`width` from the attributes, then the alt
/// text.
pub(super) fn image_call(attr: &Attr, alt: &[Inline], target: &Target) -> String {
    let mut args = vec![format!("\"{}\"", escape_string(&target.url))];
    if let Some(height) = attribute_value(attr, "height") {
        args.push(format!("height: {}", dimension(height)));
    }
    if let Some(width) = attribute_value(attr, "width") {
        args.push(format!("width: {}", dimension(width)));
    }
    let alt_text = to_plain_text(alt);
    if !alt_text.is_empty() {
        args.push(format!("alt: \"{}\"", escape_string(&alt_text)));
    }
    format!("image({})", args.join(", "))
}

/// A Typst length argument from an attribute value: a percentage carries a `.0` for a whole number; a
/// bare number or pixel count converts to inches at 96 pixels per inch; any other unit passes
/// through.
fn dimension(value: &str) -> String {
    if let Some(percent) = value.strip_suffix('%') {
        if percent.contains('.') {
            return format!("{percent}%");
        }
        return format!("{percent}.0%");
    }
    let pixels = value.strip_suffix("px").unwrap_or(value);
    if let Ok(number) = pixels.parse::<f64>() {
        format!("{}in", trim_number(number / 96.0))
    } else {
        value.to_owned()
    }
}

/// Format a number to at most five decimal places, dropping trailing zeros and a bare decimal point.
fn trim_number(value: f64) -> String {
    trim_decimals(&format!("{value:.5}"))
}

/// Format a column-width percentage to at most two decimal places.
pub(super) fn trim_percent(value: f64) -> String {
    trim_decimals(&format!("{value:.2}"))
}

fn trim_decimals(text: &str) -> String {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text.to_owned()
    }
}

/// Render a math expression as Typst. The source is translated to Typst's native math markup when
/// possible; an expression with no Typst equivalent is emitted verbatim, with its TeX delimiters
/// reconstructed and the whole run escaped as ordinary markup text.
pub(super) fn math(kind: &MathType, text: &str, smart: bool) -> String {
    let display = matches!(kind, MathType::DisplayMath);
    let Some(math) = crate::math::to_typst_labeled(text, display) else {
        let verbatim = match kind {
            MathType::InlineMath => format!("${text}$"),
            MathType::DisplayMath => format!("$${text}$$"),
        };
        return escape_text(&verbatim, false, true, smart);
    };
    let crate::math::TypstMath { body, label } = math;
    // An equation `\label` is set as a Typst reference label immediately after the closing `$`.
    let label = label.as_deref().unwrap_or("");
    match kind {
        MathType::InlineMath => format!("${body}${label}"),
        MathType::DisplayMath => format!("$ {body} ${label}"),
    }
}

/// Inline code: backtick-delimited raw markup, falling back to `#raw(..)` when the content contains a
/// backtick that the delimiter could not contain.
pub(super) fn inline_code(text: &str) -> String {
    if text.contains('`') {
        format!("#raw(\"{}\")", escape_string(text))
    } else {
        format!("`{text}`")
    }
}

/// Emit a raw-passthrough inline verbatim when its format is Typst; drop it otherwise.
pub(super) fn raw_inline_passthrough(format: &Format, text: &str) -> String {
    if format.0 == "typst" {
        text.to_owned()
    } else {
        String::new()
    }
}

/// Emit a raw-passthrough block verbatim when its format is Typst; drop it otherwise.
pub(super) fn raw_passthrough(format: &Format, text: &str) -> String {
    if format.0 == "typst" {
        text.trim_end_matches('\n').to_owned()
    } else {
        String::new()
    }
}

pub(super) fn quote_marks(kind: &QuoteType, smart: bool) -> (char, char) {
    match (kind, smart) {
        (QuoteType::SingleQuote, true) => ('\'', '\''),
        (QuoteType::DoubleQuote, true) => ('"', '"'),
        (QuoteType::SingleQuote, false) => ('\u{2018}', '\u{2019}'),
        (QuoteType::DoubleQuote, false) => ('\u{201C}', '\u{201D}'),
    }
}

/// Escape a literal-text token for markup mode.
///
/// Always-escaped characters are markup-significant anywhere. The remaining cases key off position:
/// `.` and `;` are escaped when they open a token that continues with more text; `(` is escaped when
/// it opens a token that is not preceded by a space; a `-` or `/` directly following one of its own
/// kind is escaped. En/em dashes are spelled `--`/`---`. The leading `- + = /` line markers are left
/// for the fill pass, which escapes them only at a physical line start.
pub(super) fn escape_text(text: &str, after_space: bool, first_text: bool, smart: bool) -> String {
    let is_trigger = |byte: u8| {
        matches!(
            byte,
            b'*' | b'_'
                | b'`'
                | b'\\'
                | b'#'
                | b'$'
                | b'@'
                | b'<'
                | b'>'
                | b'~'
                | b'['
                | b']'
                | b'"'
                | b'\''
                | b'.'
                | b';'
                | b'('
                | b'-'
                | b'/'
        ) || byte >= 0x80
    };
    let mut out = String::with_capacity(text.len());
    let mut previous: Option<char> = None;
    let mut first = true;
    let mut rest = text;
    loop {
        let clean = clean_prefix_len(rest, is_trigger);
        let Some((head, tail)) = rest.split_at_checked(clean) else {
            out.push_str(rest);
            break;
        };
        if !head.is_empty() {
            out.push_str(head);
            previous = head.chars().next_back();
            first = false;
        }
        let mut chars = tail.chars();
        let Some(ch) = chars.next() else { break };
        let has_more = chars.clone().next().is_some();
        let escape = match ch {
            '*' | '_' | '`' | '\\' | '#' | '$' | '@' | '<' | '>' | '~' | '[' | ']' | '"' | '\'' => {
                true
            }
            '.' | ';' => first && has_more,
            '(' => first && has_more && (!after_space || first_text),
            '-' | '/' => previous == Some(ch),
            _ => false,
        };
        // The non-breaking space is Typst's structural `~` shortcut, independent of smart punctuation.
        if ch == '\u{00A0}' {
            out.push('~');
        } else if smart && let Some(replacement) = smart_replacement(ch) {
            out.push_str(replacement);
        } else {
            if escape {
                out.push('\\');
            }
            out.push(ch);
        }
        previous = Some(ch);
        first = false;
        rest = chars.as_str();
    }
    out
}

/// The literal spelling Typst markup uses for a punctuation character that has a typographic form:
/// dashes become their hyphen runs and smart quotes their straight equivalents.
fn smart_replacement(ch: char) -> Option<&'static str> {
    match ch {
        '\u{2013}' => Some("--"),
        '\u{2014}' => Some("---"),
        '\u{2018}' | '\u{2019}' => Some("'"),
        '\u{201C}' | '\u{201D}' => Some("\""),
        _ => None,
    }
}

/// Escape a string for a double-quoted Typst string literal: backslash and double-quote only.
pub(super) fn escape_string(text: &str) -> String {
    let is_trigger = |byte: u8| matches!(byte, b'\\' | b'"');
    let mut out = String::with_capacity(text.len());
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
        out.push('\\');
        out.push(ch);
        rest = chars.as_str();
    }
    out
}
