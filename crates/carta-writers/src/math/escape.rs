//! Markup escaping shared by the math markup backends.
//!
//! Both the Office Math and Presentation MathML lowerings emit element trees over the same small set
//! of well-formed inputs, so they escape character data and attribute values the same way: the
//! markup-significant characters are replaced with their entity references and characters XML
//! forbids are dropped, so the output stays well-formed whatever the caller supplies.

/// Whether `ch` may appear in an XML 1.0 document. Tab, newline, and carriage return are the only C0
/// controls permitted; every other control below `U+0020`, the surrogate range, and the two
/// `U+FFFE`/`U+FFFF` noncharacters are forbidden and cannot be represented even as a character
/// reference, so the escapers drop them.
fn is_xml_char(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\r')
        || ('\u{20}'..='\u{d7ff}').contains(&ch)
        || ('\u{e000}'..='\u{fffd}').contains(&ch)
        || ch >= '\u{10000}'
}

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

#[cfg(test)]
mod tests {
    use super::{escape_attribute, escape_text, is_xml_char};

    #[test]
    fn text_replaces_the_markup_characters_and_drops_forbidden_ones() {
        let mut out = String::new();
        escape_text("a & b < c > d\u{0}e\u{c}f", &mut out);
        assert_eq!(out, "a &amp; b &lt; c &gt; def");
    }

    #[test]
    fn attribute_replaces_the_quote_and_keeps_the_bracket_close() {
        let mut out = String::new();
        escape_attribute("say \"hi\" & <x> \u{1}ok", &mut out);
        assert_eq!(out, "say &quot;hi&quot; &amp; &lt;x> ok");
    }

    #[test]
    fn xml_char_admits_prose_and_the_permitted_controls_but_not_the_rest() {
        for permitted in ['\t', '\n', '\r', ' ', 'z', 'é', '\u{10000}'] {
            assert!(is_xml_char(permitted), "{permitted:?} should be admitted");
        }
        for forbidden in ['\u{0}', '\u{1}', '\u{8}', '\u{c}', '\u{fffe}', '\u{ffff}'] {
            assert!(!is_xml_char(forbidden), "{forbidden:?} should be dropped");
        }
    }
}
