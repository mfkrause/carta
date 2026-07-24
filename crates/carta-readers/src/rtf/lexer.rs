//! Lexer that splits an RTF stream into its brace, control-word, and text tokens.

use std::borrow::Cow;

/// Decodes the input bytes to text. The stream is UTF-8 when it parses as such; otherwise each byte
/// is taken as its own code point (an 8-bit Latin-1 reading), the layer the wire form falls back to
/// so a document carrying raw high bytes still reads rather than being rejected. A `\'xx` escape is
/// unaffected either way, since it is spelled in ASCII and resolved through the code page later.
pub(super) fn decode_input(input: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(input) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => Cow::Owned(input.iter().map(|&byte| byte as char).collect()),
    }
}

/// One lexical unit of an RTF stream.
#[derive(Debug, Clone)]
pub(super) enum Token {
    /// `{`: opens a group.
    GroupStart,
    /// `}`: closes a group.
    GroupEnd,
    /// `\word` with an optional trailing numeric argument.
    Control(String, Option<i32>),
    /// `\` before a single non-letter character (e.g. `\~`, `\-`, `\\`).
    Symbol(char),
    /// `\'xx`: a raw byte in the document's code page.
    Hex(u8),
    /// The raw bytes introduced by a `\binN` control word: exactly `N` bytes of embedded binary,
    /// carried opaque so their values never re-enter lexing as braces, backslashes, or text.
    Binary(Vec<u8>),
    /// A literal text character.
    Char(char),
    /// A literal space; a run of them collapses when emitted.
    Space,
}

/// Splits an RTF source string into its token stream. Carriage returns, line feeds, and literal
/// tabs are structural whitespace in the wire form and carry no content, so they are dropped.
pub(super) fn tokenize(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '{' => {
                tokens.push(Token::GroupStart);
                i += 1;
            }
            '}' => {
                tokens.push(Token::GroupEnd);
                i += 1;
            }
            '\\' => {
                i = lex_backslash(&chars, i, &mut tokens);
            }
            '\r' | '\n' | '\t' => i += 1,
            ' ' => {
                tokens.push(Token::Space);
                i += 1;
            }
            _ => {
                tokens.push(Token::Char(c));
                i += 1;
            }
        }
    }
    tokens
}

/// Whether `c` is the delimiter that ends a control word and is consumed with it. A space always
/// qualifies; any other single printable punctuation mark does too, except the three characters that
/// open their own token (`{` and `}` begin and end groups, `\` starts the next control sequence) and
/// letters or digits, which would extend the word or its numeric parameter instead.
fn is_control_delimiter(c: char) -> bool {
    c == ' '
        || (c.is_ascii_graphic() && !c.is_ascii_alphanumeric() && !matches!(c, '{' | '}' | '\\'))
}

/// Lexes one backslash-introduced token starting at `start` (the backslash). Returns the index just
/// past what was consumed.
fn lex_backslash(chars: &[char], start: usize, tokens: &mut Vec<Token>) -> usize {
    let mut i = start + 1;
    match chars.get(i) {
        None => i,
        Some(&n) if n.is_ascii_alphabetic() => {
            let word_start = i;
            while matches!(chars.get(i), Some(c) if c.is_ascii_alphabetic()) {
                i += 1;
            }
            let word: String = chars.get(word_start..i).unwrap_or(&[]).iter().collect();
            let negative = matches!(chars.get(i), Some('-'));
            let digits_start = if negative { i + 1 } else { i };
            let mut j = digits_start;
            while matches!(chars.get(j), Some(c) if c.is_ascii_digit()) {
                j += 1;
            }
            let param = if j > digits_start {
                let digits: String = chars.get(digits_start..j).unwrap_or(&[]).iter().collect();
                i = j;
                digits.parse::<i64>().ok().map(|value| {
                    let signed = if negative { -value } else { value };
                    let clamped = signed.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
                    i32::try_from(clamped).unwrap_or(0)
                })
            } else {
                None
            };
            // A delimiter is absorbed with the word unless it begins another token.
            if matches!(chars.get(i), Some(&c) if is_control_delimiter(c)) {
                i += 1;
            }
            // `\binN`'s N raw bytes are captured at the lexer so a `{`, `}`, or `\` among them
            // is data, not structure, and cannot desync brace nesting.
            if word == "bin"
                && let Some(count) = param.and_then(|value| usize::try_from(value).ok())
                && count > 0
            {
                let end = i.saturating_add(count).min(chars.len());
                let bytes = chars
                    .get(i..end)
                    .unwrap_or(&[])
                    .iter()
                    .map(|&c| u8::try_from(u32::from(c) & 0xFF).unwrap_or(0))
                    .collect();
                tokens.push(Token::Binary(bytes));
                return end;
            }
            tokens.push(Token::Control(word, param));
            i
        }
        Some(&'\'') => {
            i += 1;
            let hi = chars.get(i).and_then(|c| c.to_digit(16));
            let lo = chars.get(i + 1).and_then(|c| c.to_digit(16));
            match (hi, lo) {
                (Some(hi), Some(lo)) => {
                    tokens.push(Token::Hex(u8::try_from((hi << 4) | lo).unwrap_or(0)));
                    i + 2
                }
                _ => i,
            }
        }
        Some(&symbol) => {
            tokens.push(Token::Symbol(symbol));
            i + 1
        }
    }
}
