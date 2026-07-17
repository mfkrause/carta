//! Markup escaping shared by the math markup backends.
//!
//! Both the Office Math and Presentation MathML lowerings emit element trees over the same small set
//! of well-formed inputs, so they escape character data and attribute values the same way: the
//! markup-significant characters are replaced with their entity references and every other character
//! passes through unchanged.

/// Escapes character data: the ampersand and angle brackets become entity references; all other
/// characters pass through.
pub(super) fn escape_text(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

/// Escapes a double-quoted attribute value: the ampersand, the opening angle bracket, and the quote
/// become entity references; all other characters pass through.
pub(super) fn escape_attribute(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}
