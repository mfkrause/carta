//! Character and token escaping, image, math, and passthrough helpers for the Typst writer.

use std::fmt::Write as _;

use carta_ast::{Attr, Format, Inline, MathType, QuoteType, Target, to_plain_text};
use carta_core::media::decode_data_uri;

use crate::common::{
    Dimension, attribute_value, clean_prefix_len, dimension_inches, format_length_dimension,
    format_percent_dimension, parse_dimension,
};

/// Render an `image(..)` call with its source, dimensions, and alt text.
pub(super) fn image_call(attr: &Attr, alt: &[Inline], target: &Target) -> String {
    let mut args = vec![image_source(&target.url)];
    if let Some(height) = attribute_value(attr, "height").and_then(dimension) {
        args.push(format!("height: {height}"));
    }
    if let Some(width) = attribute_value(attr, "width").and_then(dimension) {
        args.push(format!("width: {width}"));
    }
    let alt_text = to_plain_text(alt);
    if !alt_text.is_empty() {
        args.push(format!("alt: \"{}\"", escape_string(&alt_text)));
    }
    format!("image({})", args.join(", "))
}

fn image_source(url: &str) -> String {
    let Some((bytes, mime)) = decode_data_uri(url) else {
        return format!("\"{}\"", escape_string(url));
    };
    if mime == "image/svg+xml"
        && let Ok(svg) = std::str::from_utf8(&bytes)
    {
        return format!("bytes(\"{}\")", escape_string(svg));
    }
    let mut source = String::from("bytes((");
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            source.push(',');
        }
        let _ = write!(source, "{byte}");
    }
    if bytes.len() == 1 {
        source.push(',');
    }
    source.push_str("))");
    source
}

/// A Typst length argument from an attribute value, or `None` when the value is not a dimension. A
/// percentage carries a `.0` for a whole number, a bare number or pixel count converts to inches,
/// and a length keeps its unit with the number in its shortest form.
fn dimension(value: &str) -> Option<String> {
    let parsed = parse_dimension(value)?;
    Some(match parsed {
        Dimension::Percent(percent) => format_percent_dimension(percent),
        Dimension::Length(magnitude, unit) => format_length_dimension(magnitude, unit),
        Dimension::Pixels(_) => format_length_dimension(dimension_inches(&parsed)?, "in"),
    })
}

/// Format a column-width percentage to at most two decimal places.
pub(super) fn trim_percent(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
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
