//! Tokenizer: turns HTML source into a flat stream of start tags, end tags and text. Character
//! references are resolved here, except inside the script-like raw-text elements that pass through
//! verbatim.

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

pub(super) fn tokenize(chars: &[char]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut pos = 0;
    while pos < chars.len() {
        if chars.get(pos) == Some(&'<')
            && let Some(next) = read_markup(chars, pos, &mut tokens)
        {
            pos = next;
            continue;
        }
        pos = read_text(chars, pos, &mut tokens);
    }
    tokens
}

/// Consume one `<…>` construct. Returns the position past it, or `None` when the `<` does not begin
/// a tag and should be treated as literal text.
fn read_markup(chars: &[char], pos: usize, tokens: &mut Vec<Token>) -> Option<usize> {
    match chars.get(pos + 1)? {
        '!' => Some(skip_declaration(chars, pos)),
        '?' => Some(skip_to_gt(chars, pos + 1)),
        '/' => read_end_tag(chars, pos, tokens),
        c if c.is_ascii_alphabetic() => Some(read_start_tag(chars, pos, tokens)),
        _ => None,
    }
}

/// Skip `<!-- … -->`, `<![CDATA[ … ]]>` or a `<!doctype …>` declaration.
fn skip_declaration(chars: &[char], pos: usize) -> usize {
    if chars.get(pos + 2) == Some(&'-') && chars.get(pos + 3) == Some(&'-') {
        let mut i = pos + 4;
        while i < chars.len() {
            if chars.get(i) == Some(&'-')
                && chars.get(i + 1) == Some(&'-')
                && chars.get(i + 2) == Some(&'>')
            {
                return i + 3;
            }
            i += 1;
        }
        return chars.len();
    }
    skip_to_gt(chars, pos)
}

fn skip_to_gt(chars: &[char], pos: usize) -> usize {
    let mut i = pos + 1;
    while i < chars.len() {
        if chars.get(i) == Some(&'>') {
            return i + 1;
        }
        i += 1;
    }
    chars.len()
}

fn read_end_tag(chars: &[char], pos: usize, tokens: &mut Vec<Token>) -> Option<usize> {
    let start = pos + 2;
    if !chars.get(start).copied()?.is_ascii_alphabetic() {
        return None;
    }
    let mut i = start;
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_whitespace() || c == '>' {
            break;
        }
        i += 1;
    }
    let name: String = collect_lower(chars, start, i);
    tokens.push(Token::End(name));
    Some(skip_to_gt(chars, i - 1))
}

fn read_start_tag(chars: &[char], pos: usize, tokens: &mut Vec<Token>) -> usize {
    let start = pos + 1;
    let mut i = start;
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_whitespace() || c == '>' || c == '/' {
            break;
        }
        i += 1;
    }
    let name = collect_lower(chars, start, i);
    let (attrs, next, self_closing) = read_attributes(chars, i);

    if let Some(decode) = raw_text_mode(&name) {
        let (text, after) = read_raw_text(chars, next, &name, decode);
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
fn read_attributes(chars: &[char], pos: usize) -> (Vec<(String, String)>, usize, bool) {
    let mut attrs = Vec::new();
    let mut i = pos;
    let mut self_closing = false;
    loop {
        i = skip_whitespace(chars, i);
        match chars.get(i) {
            None => break,
            Some('>') => {
                i += 1;
                break;
            }
            Some('/') => {
                if chars.get(i + 1) == Some(&'>') {
                    self_closing = true;
                    i += 2;
                    break;
                }
                i += 1;
            }
            Some(_) => {
                let (name, after_name) = read_attr_name(chars, i);
                i = skip_whitespace(chars, after_name);
                if chars.get(i) == Some(&'=') {
                    i = skip_whitespace(chars, i + 1);
                    let (value, after_value) = read_attr_value(chars, i);
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

fn read_attr_name(chars: &[char], pos: usize) -> (String, usize) {
    let mut i = pos;
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_whitespace() || c == '=' || c == '>' || c == '/' {
            break;
        }
        i += 1;
    }
    (collect_lower(chars, pos, i), i)
}

fn read_attr_value(chars: &[char], pos: usize) -> (String, usize) {
    if let Some(&quote @ ('"' | '\'')) = chars.get(pos) {
        let mut i = pos + 1;
        let value_start = i;
        while let Some(&c) = chars.get(i) {
            if c == quote {
                break;
            }
            i += 1;
        }
        let raw: String = slice(chars, value_start, i);
        (decode_entities(raw), i + 1)
    } else {
        let mut i = pos;
        while let Some(&c) = chars.get(i) {
            if c.is_ascii_whitespace() || c == '>' {
                break;
            }
            i += 1;
        }
        let raw: String = slice(chars, pos, i);
        (decode_entities(raw), i)
    }
}

fn read_raw_text(chars: &[char], pos: usize, name: &str, decode: bool) -> (String, usize) {
    let mut i = pos;
    while i < chars.len() {
        if chars.get(i) == Some(&'<')
            && chars.get(i + 1) == Some(&'/')
            && matches_name(chars, i + 2, name)
        {
            let raw: String = slice(chars, pos, i);
            let text = if decode { decode_entities(raw) } else { raw };
            return (text, skip_to_gt(chars, i + 1));
        }
        i += 1;
    }
    let raw: String = slice(chars, pos, chars.len());
    let text = if decode { decode_entities(raw) } else { raw };
    (text, chars.len())
}

fn read_text(chars: &[char], pos: usize, tokens: &mut Vec<Token>) -> usize {
    let start = pos;
    let mut i = pos;
    while let Some(&c) = chars.get(i) {
        if c == '<' && begins_markup(chars, i) {
            break;
        }
        i += 1;
    }
    let next = if i == start { start + 1 } else { i };
    let raw: String = slice(chars, start, next);
    tokens.push(Token::Text(decode_entities(raw)));
    next
}

fn begins_markup(chars: &[char], pos: usize) -> bool {
    match chars.get(pos + 1) {
        Some('!' | '?' | '/') => true,
        Some(c) => c.is_ascii_alphabetic(),
        None => false,
    }
}

fn matches_name(chars: &[char], pos: usize, name: &str) -> bool {
    let candidate = name.chars().enumerate().all(|(offset, expected)| {
        chars
            .get(pos + offset)
            .is_some_and(|&c| c.eq_ignore_ascii_case(&expected))
    });
    if !candidate {
        return false;
    }
    match chars.get(pos + name.chars().count()) {
        None => true,
        Some(&c) => c.is_ascii_whitespace() || c == '>' || c == '/',
    }
}

fn skip_whitespace(chars: &[char], pos: usize) -> usize {
    let mut i = pos;
    while chars.get(i).is_some_and(char::is_ascii_whitespace) {
        i += 1;
    }
    i
}

fn slice(chars: &[char], start: usize, end: usize) -> String {
    chars
        .get(start..end)
        .map(|s| s.iter().collect())
        .unwrap_or_default()
}

fn collect_lower(chars: &[char], start: usize, end: usize) -> String {
    chars
        .get(start..end)
        .map(|s| s.iter().flat_map(|c| c.to_lowercase()).collect())
        .unwrap_or_default()
}

fn decode_entities(text: String) -> String {
    if !text.contains('&') {
        return text;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c == '&'
            && let Some((decoded, next)) = scan_char_ref(&chars, i)
        {
            out.push_str(&decoded);
            i = next;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Resolve a character reference beginning at `start` (a `&`). Named references decode without a
/// trailing `;` only when the whole alphanumeric run names a known entity.
fn scan_char_ref(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start + 1) == Some(&'#') {
        return scan_numeric_ref(chars, start);
    }
    let mut i = start + 1;
    while chars.get(i).is_some_and(char::is_ascii_alphanumeric) {
        i += 1;
    }
    if i == start + 1 {
        return None;
    }
    let name: String = slice(chars, start + 1, i);
    if chars.get(i) == Some(&';') {
        let decoded = lookup_named(&name)?;
        return Some((decoded.to_string(), i + 1));
    }
    let decoded = lookup_named(&name)?;
    Some((decoded.to_string(), i))
}

fn scan_numeric_ref(chars: &[char], start: usize) -> Option<(String, usize)> {
    let hex = matches!(chars.get(start + 2), Some('x' | 'X'));
    let digits_start = if hex { start + 3 } else { start + 2 };
    let mut i = digits_start;
    let radix = if hex { 16 } else { 10 };
    while chars.get(i).is_some_and(|c| c.is_digit(radix)) {
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    let digits: String = slice(chars, digits_start, i);
    let code = u32::from_str_radix(&digits, radix).ok()?;
    let end = if chars.get(i) == Some(&';') { i + 1 } else { i };
    Some((code_point(code).to_string(), end))
}
