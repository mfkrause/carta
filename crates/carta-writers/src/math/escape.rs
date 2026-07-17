//! Markup escaping shared by the math markup backends.
//!
//! Both the Office Math and Presentation MathML lowerings emit element trees over the same small set
//! of well-formed inputs, so they escape character data and attribute values the same way: the
//! markup-significant characters are replaced with their entity references and characters XML
//! forbids are dropped, so the output stays well-formed whatever the caller supplies.

use carta_core::container::xml::is_xml_char;

/// Escapes character data: the ampersand and angle brackets become entity references; characters XML
/// forbids are dropped and every other character passes through.
pub(super) fn escape_text(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other if is_xml_char(other) => out.push(other),
            _ => {}
        }
    }
}

/// Escapes a double-quoted attribute value: the ampersand, the opening angle bracket, and the quote
/// become entity references; characters XML forbids are dropped and every other character passes
/// through.
pub(super) fn escape_attribute(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            other if is_xml_char(other) => out.push(other),
            _ => {}
        }
    }
}
