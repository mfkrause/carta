//! Tokenizer: turns HTML source into a flat stream of start tags, end tags and text. Character
//! references are resolved here, except inside the script-like raw-text elements that pass through
//! verbatim.
//!
//! The tokenizer walks the source by byte offset. Every structural character of HTML syntax (`<`,
//! `>`, `/`, `=`, quotes, `&`, ASCII whitespace) is a single ASCII byte, and in UTF-8 no byte of a
//! multi-byte character equals an ASCII byte, so scanning byte-by-byte for these delimiters is
//! boundary-safe: every position where a delimiter is found, and every slice cut at such a
//! position, lies on a character boundary.

use crate::entities::{code_point, lookup_named};

#[derive(Debug)]
pub(super) enum Token {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    End(String),
    Text(String),
    /// An HTML comment, carrying its full literal form with the `<!--` and `-->` delimiters.
    Comment(String),
}

/// Elements whose content is read verbatim up to the matching end tag. Entities are still resolved
/// in the text-only group; the script-like group is passed through untouched.
fn raw_text_mode(name: &str) -> Option<bool> {
    match name {
        "script" | "style" => Some(false),
        "title" | "textarea" => Some(true),
        _ => None,
    }
}

pub(super) fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        if input.as_bytes().get(pos) == Some(&b'<')
            && let Some(next) = read_markup(input, pos, &mut tokens)
        {
            pos = next;
            continue;
        }
        pos = read_text(input, pos, &mut tokens);
    }
    tokens
}

/// Consume one `<…>` construct. Returns the position past it, or `None` when the `<` does not begin
/// a tag and should be treated as literal text.
fn read_markup(input: &str, pos: usize, tokens: &mut Vec<Token>) -> Option<usize> {
    match input.as_bytes().get(pos + 1)? {
        b'!' => Some(read_declaration(input, pos, tokens)),
        b'?' => Some(skip_to_gt(input, pos + 1)),
        b'/' => read_end_tag(input, pos, tokens),
        b if b.is_ascii_alphabetic() => Some(read_start_tag(input, pos, tokens)),
        _ => None,
    }
}

/// Consume a `<! … >` construct. An HTML comment becomes a [`Token::Comment`] carrying its literal
/// form; a `<![CDATA[ … ]]>` section yields its content as verbatim text; a `<!doctype …>`
/// declaration is skipped.
fn read_declaration(input: &str, pos: usize, tokens: &mut Vec<Token>) -> usize {
    let bytes = input.as_bytes();
    if bytes.get(pos + 2) == Some(&b'-') && bytes.get(pos + 3) == Some(&b'-') {
        return read_comment(input, pos, tokens);
    }
    if input.get(pos + 2..pos + 9) == Some("[CDATA[") {
        return read_cdata(input, pos, tokens);
    }
    skip_to_gt(input, pos)
}

/// Consume `<![CDATA[ … ]]>`, emitting its content as a single text token. A CDATA section holds
/// character data, so its content carries through verbatim: markup delimiters stay literal and
/// character references are not resolved. An unterminated section runs to the end of input.
fn read_cdata(input: &str, pos: usize, tokens: &mut Vec<Token>) -> usize {
    let bytes = input.as_bytes();
    let start = pos + "<![CDATA[".len();
    let mut i = start;
    while i < bytes.len() {
        if bytes.get(i) == Some(&b']')
            && bytes.get(i + 1) == Some(&b']')
            && bytes.get(i + 2) == Some(&b'>')
        {
            push_cdata_text(input, start, i, tokens);
            return i + 3;
        }
        i += 1;
    }
    push_cdata_text(input, start, bytes.len(), tokens);
    bytes.len()
}

fn push_cdata_text(input: &str, start: usize, end: usize, tokens: &mut Vec<Token>) {
    let text = input.get(start..end).unwrap_or_default();
    if !text.is_empty() {
        tokens.push(Token::Text(text.to_string()));
    }
}

/// Consume `<!-- … -->`, emitting the whole literal (delimiters included) as a comment token. An
/// unterminated comment runs to the end of input.
fn read_comment(input: &str, pos: usize, tokens: &mut Vec<Token>) -> usize {
    let bytes = input.as_bytes();
    let mut i = pos + 4;
    while i < bytes.len() {
        if bytes.get(i) == Some(&b'-')
            && bytes.get(i + 1) == Some(&b'-')
            && bytes.get(i + 2) == Some(&b'>')
        {
            let end = i + 3;
            tokens.push(Token::Comment(
                input.get(pos..end).unwrap_or_default().to_string(),
            ));
            return end;
        }
        i += 1;
    }
    tokens.push(Token::Comment(
        input.get(pos..).unwrap_or_default().to_string(),
    ));
    bytes.len()
}

fn skip_to_gt(input: &str, pos: usize) -> usize {
    let bytes = input.as_bytes();
    let mut i = pos + 1;
    while i < bytes.len() {
        if bytes.get(i) == Some(&b'>') {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

fn read_end_tag(input: &str, pos: usize, tokens: &mut Vec<Token>) -> Option<usize> {
    let bytes = input.as_bytes();
    let start = pos + 2;
    if !bytes.get(start)?.is_ascii_alphabetic() {
        return None;
    }
    let mut i = start;
    while let Some(&b) = bytes.get(i) {
        if b.is_ascii_whitespace() || b == b'>' {
            break;
        }
        i += 1;
    }
    let name = collect_lower(input, start, i);
    tokens.push(Token::End(name));
    Some(skip_to_gt(input, i - 1))
}

fn read_start_tag(input: &str, pos: usize, tokens: &mut Vec<Token>) -> usize {
    let bytes = input.as_bytes();
    let start = pos + 1;
    let mut i = start;
    while let Some(&b) = bytes.get(i) {
        if b.is_ascii_whitespace() || b == b'>' || b == b'/' {
            break;
        }
        i += 1;
    }
    let name = collect_lower(input, start, i);
    let (attrs, next, self_closing) = read_attributes(input, i);

    if let Some(decode) = raw_text_mode(&name) {
        let (text, after) = read_raw_text(input, next, &name, decode);
        tokens.push(Token::Start {
            name: name.clone(),
            attrs,
            self_closing: false,
        });
        if !text.is_empty() {
            tokens.push(Token::Text(text));
        }
        tokens.push(Token::End(name));
        return after;
    }

    tokens.push(Token::Start {
        name,
        attrs,
        self_closing,
    });
    next
}

/// Parse a start tag's attribute list. `pos` points just after the tag name; the result position is
/// just past the closing `>`.
fn read_attributes(input: &str, pos: usize) -> (Vec<(String, String)>, usize, bool) {
    let bytes = input.as_bytes();
    let mut attrs = Vec::new();
    let mut i = pos;
    let mut self_closing = false;
    loop {
        i = skip_whitespace(input, i);
        match bytes.get(i) {
            None => break,
            Some(b'>') => {
                i += 1;
                break;
            }
            Some(b'/') => {
                if bytes.get(i + 1) == Some(&b'>') {
                    self_closing = true;
                    i += 2;
                    break;
                }
                i += 1;
            }
            Some(_) => {
                let (name, after_name) = read_attr_name(input, i);
                i = skip_whitespace(input, after_name);
                if bytes.get(i) == Some(&b'=') {
                    i = skip_whitespace(input, i + 1);
                    let (value, after_value) = read_attr_value(input, i);
                    i = after_value;
                    if !name.is_empty() {
                        attrs.push((name, value));
                    }
                } else if !name.is_empty() {
                    attrs.push((name, String::new()));
                }
            }
        }
    }
    (attrs, i, self_closing)
}

fn read_attr_name(input: &str, pos: usize) -> (String, usize) {
    let bytes = input.as_bytes();
    let mut i = pos;
    while let Some(&b) = bytes.get(i) {
        if b.is_ascii_whitespace() || b == b'=' || b == b'>' || b == b'/' {
            break;
        }
        i += 1;
    }
    (collect_lower(input, pos, i), i)
}

fn read_attr_value(input: &str, pos: usize) -> (String, usize) {
    let bytes = input.as_bytes();
    if let Some(&quote @ (b'"' | b'\'')) = bytes.get(pos) {
        let mut i = pos + 1;
        let value_start = i;
        while let Some(&b) = bytes.get(i) {
            if b == quote {
                break;
            }
            i += 1;
        }
        let raw = input.get(value_start..i).unwrap_or_default();
        (decode_entities(raw), i + 1)
    } else {
        let mut i = pos;
        while let Some(&b) = bytes.get(i) {
            if b.is_ascii_whitespace() || b == b'>' {
                break;
            }
            i += 1;
        }
        let raw = input.get(pos..i).unwrap_or_default();
        (decode_entities(raw), i)
    }
}

fn read_raw_text(input: &str, pos: usize, name: &str, decode: bool) -> (String, usize) {
    let bytes = input.as_bytes();
    let mut i = pos;
    while i < bytes.len() {
        if bytes.get(i) == Some(&b'<')
            && bytes.get(i + 1) == Some(&b'/')
            && matches_name(input, i + 2, name)
        {
            let raw = input.get(pos..i).unwrap_or_default();
            let text = if decode {
                decode_entities(raw)
            } else {
                raw.to_string()
            };
            return (text, skip_to_gt(input, i + 1));
        }
        i += 1;
    }
    let raw = input.get(pos..).unwrap_or_default();
    let text = if decode {
        decode_entities(raw)
    } else {
        raw.to_string()
    };
    (text, bytes.len())
}

fn read_text(input: &str, pos: usize, tokens: &mut Vec<Token>) -> usize {
    let bytes = input.as_bytes();
    let start = pos;
    let mut search = pos;
    let i = loop {
        let Some(offset) = memchr::memchr(b'<', bytes.get(search..).unwrap_or_default()) else {
            break bytes.len();
        };
        let at = search + offset;
        if begins_markup(input, at) {
            break at;
        }
        search = at + 1;
    };
    // A lone `<` that opens no markup is consumed as literal text; it is a single byte, so the
    // one-byte step stays on a character boundary.
    let next = if i == start { start + 1 } else { i };
    let raw = input.get(start..next).unwrap_or_default();
    tokens.push(Token::Text(decode_entities(raw)));
    next
}

fn begins_markup(input: &str, pos: usize) -> bool {
    match input.as_bytes().get(pos + 1) {
        Some(b'!' | b'?' | b'/') => true,
        Some(b) => b.is_ascii_alphabetic(),
        None => false,
    }
}

/// Whether the raw-text closer name (always ASCII) appears at `pos`, followed by a tag-name
/// terminator.
fn matches_name(input: &str, pos: usize, name: &str) -> bool {
    let bytes = input.as_bytes();
    let Some(candidate) = bytes.get(pos..pos + name.len()) else {
        return false;
    };
    if !candidate.eq_ignore_ascii_case(name.as_bytes()) {
        return false;
    }
    match bytes.get(pos + name.len()) {
        None => true,
        Some(&b) => b.is_ascii_whitespace() || b == b'>' || b == b'/',
    }
}

fn skip_whitespace(input: &str, pos: usize) -> usize {
    let bytes = input.as_bytes();
    let mut i = pos;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// Lowercase a tag or attribute name. Names are ASCII in practice, so the common path lowers the
/// bytes in place; non-ASCII names lower character by character.
fn collect_lower(input: &str, start: usize, end: usize) -> String {
    let raw = input.get(start..end).unwrap_or_default();
    if raw.is_ascii() {
        let mut lowered = raw.to_string();
        lowered.make_ascii_lowercase();
        lowered
    } else {
        raw.chars().flat_map(char::to_lowercase).collect()
    }
}

/// A resolved character reference: either a named entity's static expansion or a numeric code
/// point, so decoding never allocates per reference.
enum CharRef {
    Named(&'static str),
    Code(char),
}

/// Decode character references in `text` in a single pass, copying the verbatim runs between them.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut run_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes.get(i) == Some(&b'&')
            && let Some((decoded, next)) = scan_char_ref(text, i)
        {
            out.push_str(text.get(run_start..i).unwrap_or_default());
            match decoded {
                CharRef::Named(expansion) => out.push_str(expansion),
                CharRef::Code(c) => out.push(c),
            }
            run_start = next;
            i = next;
            continue;
        }
        i += 1;
    }
    out.push_str(text.get(run_start..).unwrap_or_default());
    out
}

/// Resolve a character reference beginning at `start` (a `&`). Named references decode without a
/// trailing `;` only when the whole alphanumeric run names a known entity.
fn scan_char_ref(text: &str, start: usize) -> Option<(CharRef, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(start + 1) == Some(&b'#') {
        return scan_numeric_ref(text, start);
    }
    let mut i = start + 1;
    while bytes.get(i).is_some_and(u8::is_ascii_alphanumeric) {
        i += 1;
    }
    if i == start + 1 {
        return None;
    }
    let expansion = lookup_named(text.get(start + 1..i)?)?;
    let end = if bytes.get(i) == Some(&b';') {
        i + 1
    } else {
        i
    };
    Some((CharRef::Named(expansion), end))
}

fn scan_numeric_ref(text: &str, start: usize) -> Option<(CharRef, usize)> {
    let bytes = text.as_bytes();
    let hex = matches!(bytes.get(start + 2), Some(b'x' | b'X'));
    let digits_start = if hex { start + 3 } else { start + 2 };
    let is_digit = |b: &u8| {
        if hex {
            b.is_ascii_hexdigit()
        } else {
            b.is_ascii_digit()
        }
    };
    let mut i = digits_start;
    while bytes.get(i).is_some_and(is_digit) {
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    let radix = if hex { 16 } else { 10 };
    let code = u32::from_str_radix(text.get(digits_start..i)?, radix).ok()?;
    let end = if bytes.get(i) == Some(&b';') {
        i + 1
    } else {
        i
    };
    Some((CharRef::Code(code_point(code)), end))
}

#[cfg(test)]
mod tests {
    use super::{Token, decode_entities, tokenize};

    fn text_tokens(input: &str) -> Vec<String> {
        tokenize(input)
            .into_iter()
            .filter_map(|token| match token {
                Token::Text(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn multibyte_text_inside_a_paragraph() {
        let tokens = tokenize("<p>\u{201c}quoted\u{201d}</p>");
        assert!(matches!(
            tokens.as_slice(),
            [
                Token::Start { name, .. },
                Token::Text(text),
                Token::End(end),
            ] if name == "p" && text == "\u{201c}quoted\u{201d}" && end == "p"
        ));
    }

    #[test]
    fn attribute_value_with_emoji() {
        let tokens = tokenize("<span title=\"\u{1f600} face\">x</span>");
        let Some(Token::Start { name, attrs, .. }) = tokens.first() else {
            panic!("expected start tag");
        };
        assert_eq!(name, "span");
        assert_eq!(
            attrs.as_slice(),
            [("title".to_string(), "\u{1f600} face".to_string())]
        );
    }

    #[test]
    fn raw_text_with_multibyte_and_closer_lookalikes() {
        let tokens = tokenize("<script>var s = \"\u{00e9}</scripx\" + x;</script>");
        assert!(matches!(
            tokens.as_slice(),
            [
                Token::Start { name, .. },
                Token::Text(text),
                Token::End(_),
            ] if name == "script" && text == "var s = \"\u{00e9}</scripx\" + x;"
        ));
    }

    #[test]
    fn unterminated_tag_at_end_of_input_after_multibyte_text() {
        let tokens = tokenize("\u{00e9}\u{00e9}<em");
        assert!(matches!(
            tokens.as_slice(),
            [Token::Text(text), Token::Start { name, .. }]
                if text == "\u{00e9}\u{00e9}" && name == "em"
        ));
    }

    #[test]
    fn lone_open_angle_before_multibyte_text_stays_literal() {
        assert_eq!(text_tokens("a < \u{00e9} b"), ["a < \u{00e9} b"]);
    }

    #[test]
    fn end_tag_name_with_multibyte_garbage_recovers() {
        assert_eq!(text_tokens("<p>x</p\u{00e9}garbage>y</p>"), ["x", "y"]);
    }

    #[test]
    fn entity_at_start_of_input() {
        assert_eq!(decode_entities("&amp;x"), "&x");
    }

    #[test]
    fn entity_at_end_of_input() {
        assert_eq!(decode_entities("x&amp;"), "x&");
    }

    #[test]
    fn named_entity_without_semicolon_at_end_of_input() {
        assert_eq!(decode_entities("x&amp"), "x&");
    }

    #[test]
    fn truncated_entity_stays_literal() {
        assert_eq!(decode_entities("&am"), "&am");
    }

    #[test]
    fn isolated_ampersands_stay_literal() {
        assert_eq!(decode_entities("a & b &; c"), "a & b &; c");
    }

    #[test]
    fn multibyte_text_around_an_entity() {
        assert_eq!(
            decode_entities("\u{00e9}&amp;\u{00e9}"),
            "\u{00e9}&\u{00e9}"
        );
    }

    #[test]
    fn entity_with_multibyte_expansion() {
        assert_eq!(decode_entities("a&hellip;b"), "a\u{2026}b");
    }

    #[test]
    fn numeric_and_hex_references_decode() {
        assert_eq!(decode_entities("&#233;&#xE9;&#xe9"), "\u{e9}\u{e9}\u{e9}");
    }

    #[test]
    fn numeric_reference_without_digits_stays_literal() {
        assert_eq!(decode_entities("&#;&#x;"), "&#;&#x;");
    }

    #[test]
    fn non_ascii_attribute_name_lowercases_by_character() {
        let tokens = tokenize("<p \u{00c9}X=1>t</p>");
        let Some(Token::Start { attrs, .. }) = tokens.first() else {
            panic!("expected start tag");
        };
        assert_eq!(
            attrs.as_slice(),
            [("\u{00e9}x".to_string(), "1".to_string())]
        );
    }
}
