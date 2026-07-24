//! Free helpers shared by the tokenizer: context lookup, token normalization, and matcher primitives.

use std::rc::Rc;

use fancy_regex::{Regex, RegexBuilder};

use crate::grammar::Grammar;
use crate::token::{SourceLine, Token, TokenKind};

use super::RegexKey;

/// The backtracking budget granted to each regular-expression match. On exhaustion the match fails,
/// keeping tokenization bounded on adversarial input.
const REGEX_BACKTRACK_LIMIT: usize = 1_000_000;

pub(super) fn context_index(grammar: &Grammar, name: &str) -> Option<usize> {
    grammar.contexts.iter().position(|c| c.name == name)
}

pub(super) fn kind_for(grammar: &Grammar, attr_name: &str) -> TokenKind {
    grammar
        .item_styles
        .get(attr_name)
        .copied()
        .unwrap_or(TokenKind::Normal)
}

pub(super) fn plain_line(line: &str) -> SourceLine {
    if line.is_empty() {
        Vec::new()
    } else {
        vec![Token::new(TokenKind::Normal, line.to_string())]
    }
}

/// Drop empty tokens and merge runs of the same kind, matching how a line is finally rendered.
pub(super) fn normalize(tokens: Vec<Token>) -> SourceLine {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if token.text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.kind == token.kind => last.text.push_str(&token.text),
            _ => out.push(token),
        }
    }
    out
}

pub(super) fn split_lines(code: &str) -> Vec<&str> {
    if code.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = code.split('\n').collect();
    if code.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub(super) fn is_word_boundary(c: char, d: char) -> bool {
    is_word_char(c) != is_word_char(d)
}

pub(super) fn substitute<F>(template: &str, lookup: F) -> String
where
    F: Fn(usize) -> Option<String>,
{
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%'
            && let Some(d) = chars.peek().copied().filter(char::is_ascii_digit)
        {
            chars.next();
            let idx = (d as usize) - ('0' as usize);
            out.push_str(&lookup(idx).unwrap_or_default());
            continue;
        }
        out.push(c);
    }
    out
}

pub(super) fn escape_regex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub(super) fn build_regex(key: &RegexKey) -> Option<Rc<Regex>> {
    let mut pattern = String::new();
    if key.insensitive {
        pattern.push_str("(?i)");
    }
    if key.minimal {
        pattern.push_str("(?U)");
    }
    pattern.push_str("\\A(?:");
    pattern.push_str(&key.pattern);
    pattern.push(')');
    RegexBuilder::new(&pattern)
        .backtrack_limit(REGEX_BACKTRACK_LIMIT)
        .build()
        .ok()
        .map(Rc::new)
}

fn digits_len(s: &str, valid: impl Fn(char) -> bool) -> usize {
    s.chars()
        .take_while(|c| valid(*c))
        .map(char::len_utf8)
        .sum()
}

pub(super) fn match_decimal(s: &str) -> Option<usize> {
    let mut len = 0;
    if s.starts_with('-') {
        len += 1;
    }
    let digits = digits_len(&s[len..], |c| c.is_ascii_digit());
    if digits == 0 {
        return None;
    }
    Some(len + digits)
}

pub(super) fn match_hex(s: &str) -> Option<usize> {
    let mut len = 0;
    if s.starts_with('-') {
        len += 1;
    }
    let rest = &s[len..];
    let mut chars = rest.char_indices();
    if chars.next()?.1 != '0' {
        return None;
    }
    match chars.next()?.1 {
        'x' | 'X' => {}
        _ => return None,
    }
    let digits = digits_len(&rest[2..], |c| c.is_ascii_hexdigit());
    if digits == 0 {
        return None;
    }
    Some(len + 2 + digits)
}

pub(super) fn match_octal(s: &str) -> Option<usize> {
    let mut len = 0;
    if s.starts_with('-') {
        len += 1;
    }
    let rest = s.get(len..).unwrap_or("");
    if !rest.starts_with('0') {
        return None;
    }
    let digits = digits_len(rest.get(1..).unwrap_or(""), |c| ('0'..='7').contains(&c));
    if digits == 0 {
        return None;
    }
    Some(len + 1 + digits)
}

// The float shapes are clearer enumerated as independent cases than as a single minimized predicate.
#[allow(clippy::nonminimal_bool)]
pub(super) fn match_float(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    if bytes.first() == Some(&b'+') || bytes.first() == Some(&b'-') {
        i += 1;
    }
    let before = {
        let n = digits_len(&s[i..], |c| c.is_ascii_digit());
        i += n;
        n > 0
    };
    let dot = if bytes.get(i) == Some(&b'.') {
        i += 1;
        true
    } else {
        false
    };
    let after = {
        let n = digits_len(&s[i..], |c| c.is_ascii_digit());
        i += n;
        n > 0
    };
    let exponent = {
        if matches!(bytes.get(i), Some(&b'e' | &b'E')) {
            let mut j = i + 1;
            if matches!(bytes.get(j), Some(&b'+' | &b'-')) {
                j += 1;
            }
            let n = digits_len(&s[j..], |c| c.is_ascii_digit());
            if n > 0 {
                i = j + n;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if matches!(s[i..].chars().next(), Some('.')) {
        return None;
    }
    let valid = (before && !dot && exponent)
        || (before && dot && (after || !exponent))
        || (!before && dot && after);
    valid.then_some(i)
}

pub(super) fn match_c_string_char(s: &str) -> Option<usize> {
    let mut chars = s.char_indices();
    if chars.next()?.1 != '\\' {
        return None;
    }
    let (_, next) = chars.next()?;
    match next {
        'x' | 'X' => {
            let digits = digits_len(&s[2..], |c| c.is_ascii_hexdigit());
            if digits == 0 {
                return None;
            }
            Some(2 + digits)
        }
        '0' => {
            let digits = digits_len(&s[2..], |c| ('0'..='7').contains(&c));
            Some(2 + digits)
        }
        c if "abefnrtv\"'?\\".contains(c) => Some(1 + c.len_utf8()),
        _ => None,
    }
}

pub(super) fn match_c_char(s: &str) -> Option<usize> {
    let mut i = 0;
    if s.get(i..).and_then(|r| r.chars().next()) != Some('\'') {
        return None;
    }
    i += 1;
    let rest = s.get(i..)?;
    let inner = if let Some(len) = match_c_string_char(rest) {
        len
    } else {
        let c = rest.chars().next()?;
        if c == '\'' || c == '\\' {
            return None;
        }
        c.len_utf8()
    };
    i += inner;
    if s.get(i..).and_then(|r| r.chars().next()) != Some('\'') {
        return None;
    }
    Some(i + 1)
}
