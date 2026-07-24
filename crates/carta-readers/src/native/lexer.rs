//! Lexer: splits native source text into the token stream the parser consumes.

use carta_core::Result;

use super::{Token, syntax_error};

pub(super) fn tokenize(input: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    let mut tokens = Vec::new();
    while let Some(&c) = chars.get(pos) {
        if c.is_whitespace() {
            pos += 1;
            continue;
        }
        match c {
            '(' => push_punct(&mut tokens, &mut pos, Token::LParen),
            ')' => push_punct(&mut tokens, &mut pos, Token::RParen),
            '[' => push_punct(&mut tokens, &mut pos, Token::LBracket),
            ']' => push_punct(&mut tokens, &mut pos, Token::RBracket),
            '{' => push_punct(&mut tokens, &mut pos, Token::LBrace),
            '}' => push_punct(&mut tokens, &mut pos, Token::RBrace),
            ',' => push_punct(&mut tokens, &mut pos, Token::Comma),
            '=' => push_punct(&mut tokens, &mut pos, Token::Equals),
            '"' => {
                let (text, next) = lex_string(&chars, pos)?;
                tokens.push(Token::Str(text));
                pos = next;
            }
            '-' => {
                let (number, next) = lex_number(&chars, pos)?;
                tokens.push(Token::Num(number));
                pos = next;
            }
            _ if c.is_ascii_digit() => {
                let (number, next) = lex_number(&chars, pos)?;
                tokens.push(Token::Num(number));
                pos = next;
            }
            _ if c.is_alphabetic() || c == '_' => {
                let (ident, next) = lex_ident(&chars, pos);
                tokens.push(Token::Ident(ident));
                pos = next;
            }
            _ => return Err(syntax_error(format!("unexpected character '{c}'"))),
        }
    }
    Ok(tokens)
}

fn push_punct(tokens: &mut Vec<Token>, pos: &mut usize, token: Token) {
    tokens.push(token);
    *pos += 1;
}

fn lex_ident(chars: &[char], start: usize) -> (String, usize) {
    let mut pos = start;
    let mut ident = String::new();
    while let Some(&c) = chars.get(pos) {
        if c.is_alphanumeric() || c == '_' || c == '\'' {
            ident.push(c);
            pos += 1;
        } else {
            break;
        }
    }
    (ident, pos)
}

fn lex_number(chars: &[char], start: usize) -> Result<(String, usize)> {
    let mut pos = start;
    let mut number = String::new();
    if chars.get(pos) == Some(&'-') {
        number.push('-');
        pos += 1;
    }
    let digits_start = pos;
    pos = consume_digits(chars, pos, &mut number);
    if pos == digits_start {
        return Err(syntax_error("expected a digit"));
    }
    if chars.get(pos) == Some(&'.') {
        number.push('.');
        pos += 1;
        pos = consume_digits(chars, pos, &mut number);
    }
    if matches!(chars.get(pos), Some('e' | 'E')) {
        if let Some(&exp) = chars.get(pos) {
            number.push(exp);
        }
        pos += 1;
        if matches!(chars.get(pos), Some('+' | '-')) {
            if let Some(&sign) = chars.get(pos) {
                number.push(sign);
            }
            pos += 1;
        }
        pos = consume_digits(chars, pos, &mut number);
    }
    Ok((number, pos))
}

fn consume_digits(chars: &[char], start: usize, out: &mut String) -> usize {
    let mut pos = start;
    while let Some(&c) = chars.get(pos) {
        if c.is_ascii_digit() {
            out.push(c);
            pos += 1;
        } else {
            break;
        }
    }
    pos
}

/// ASCII control-code mnemonics as emitted for non-printable characters (`\ESC`, `\SOH`, …),
/// longest first so maximal-munch matching prefers `SOH` over `SO`.
const CONTROL_MNEMONICS: &[(&str, u32)] = &[
    ("NUL", 0),
    ("SOH", 1),
    ("STX", 2),
    ("ETX", 3),
    ("EOT", 4),
    ("ENQ", 5),
    ("ACK", 6),
    ("BEL", 7),
    ("DLE", 16),
    ("DC1", 17),
    ("DC2", 18),
    ("DC3", 19),
    ("DC4", 20),
    ("NAK", 21),
    ("SYN", 22),
    ("ETB", 23),
    ("CAN", 24),
    ("SUB", 26),
    ("ESC", 27),
    ("DEL", 127),
    ("BS", 8),
    ("HT", 9),
    ("LF", 10),
    ("VT", 11),
    ("FF", 12),
    ("CR", 13),
    ("SO", 14),
    ("SI", 15),
    ("EM", 25),
    ("FS", 28),
    ("GS", 29),
    ("RS", 30),
    ("US", 31),
    ("SP", 32),
];

fn lex_string(chars: &[char], start: usize) -> Result<(String, usize)> {
    let mut pos = start + 1;
    let mut text = String::new();
    loop {
        match chars.get(pos) {
            None => return Err(syntax_error("unterminated string literal")),
            Some('"') => return Ok((text, pos + 1)),
            Some('\\') => pos = lex_escape(chars, pos, &mut text)?,
            Some(&c) => {
                text.push(c);
                pos += 1;
            }
        }
    }
}

/// Decodes one escape sequence starting at the backslash at `pos`, appending its character (if
/// any) to `text`, and returns the index just past the sequence.
fn lex_escape(chars: &[char], pos: usize, text: &mut String) -> Result<usize> {
    let escaped = chars
        .get(pos + 1)
        .copied()
        .ok_or_else(|| syntax_error("dangling escape at end of string"))?;
    match escaped {
        'n' => Ok(push_char(text, '\n', pos + 2)),
        't' => Ok(push_char(text, '\t', pos + 2)),
        'r' => Ok(push_char(text, '\r', pos + 2)),
        'f' => Ok(push_char(text, '\u{0C}', pos + 2)),
        'v' => Ok(push_char(text, '\u{0B}', pos + 2)),
        'a' => Ok(push_char(text, '\u{07}', pos + 2)),
        'b' => Ok(push_char(text, '\u{08}', pos + 2)),
        '\\' => Ok(push_char(text, '\\', pos + 2)),
        '"' => Ok(push_char(text, '"', pos + 2)),
        '\'' => Ok(push_char(text, '\'', pos + 2)),
        '&' => Ok(pos + 2),
        '^' => {
            let control = chars
                .get(pos + 2)
                .copied()
                .ok_or_else(|| syntax_error("dangling control escape"))?;
            let code = (control as u32)
                .checked_sub(64)
                .ok_or_else(|| syntax_error("invalid control escape"))?;
            Ok(push_char(text, code_to_char(code)?, pos + 3))
        }
        'x' => lex_radix_escape(chars, pos + 2, 16, text),
        'o' => lex_radix_escape(chars, pos + 2, 8, text),
        d if d.is_ascii_digit() => lex_decimal_escape(chars, pos + 1, text),
        w if w.is_whitespace() => lex_gap(chars, pos + 1),
        u if u.is_ascii_uppercase() => lex_mnemonic_escape(chars, pos + 1, text),
        other => Err(syntax_error(format!("unknown string escape '\\{other}'"))),
    }
}

fn push_char(text: &mut String, c: char, next: usize) -> usize {
    text.push(c);
    next
}

fn code_to_char(code: u32) -> Result<char> {
    char::from_u32(code).ok_or_else(|| syntax_error(format!("invalid character code {code}")))
}

fn lex_decimal_escape(chars: &[char], start: usize, text: &mut String) -> Result<usize> {
    let mut pos = start;
    let mut code: u32 = 0;
    while let Some(&c) = chars.get(pos) {
        if let Some(digit) = c.to_digit(10) {
            code = code
                .checked_mul(10)
                .and_then(|value| value.checked_add(digit))
                .ok_or_else(|| syntax_error("character code out of range"))?;
            pos += 1;
        } else {
            break;
        }
    }
    Ok(push_char(text, code_to_char(code)?, pos))
}

fn lex_radix_escape(chars: &[char], start: usize, radix: u32, text: &mut String) -> Result<usize> {
    let mut pos = start;
    let mut code: u32 = 0;
    let mut seen = false;
    while let Some(&c) = chars.get(pos) {
        if let Some(digit) = c.to_digit(radix) {
            code = code
                .checked_mul(radix)
                .and_then(|value| value.checked_add(digit))
                .ok_or_else(|| syntax_error("character code out of range"))?;
            seen = true;
            pos += 1;
        } else {
            break;
        }
    }
    if !seen {
        return Err(syntax_error("empty numeric escape"));
    }
    Ok(push_char(text, code_to_char(code)?, pos))
}

fn lex_mnemonic_escape(chars: &[char], start: usize, text: &mut String) -> Result<usize> {
    for &(name, code) in CONTROL_MNEMONICS {
        if mnemonic_matches(chars, start, name) {
            return Ok(push_char(text, code_to_char(code)?, start + name.len()));
        }
    }
    Err(syntax_error("unknown control-code escape"))
}

fn mnemonic_matches(chars: &[char], start: usize, name: &str) -> bool {
    name.chars()
        .enumerate()
        .all(|(offset, expected)| chars.get(start + offset) == Some(&expected))
}

/// A string gap (`\<whitespace>\`) carries no character; skip the whitespace run and its closing
/// backslash.
fn lex_gap(chars: &[char], start: usize) -> Result<usize> {
    let mut pos = start;
    while let Some(&c) = chars.get(pos) {
        if c.is_whitespace() {
            pos += 1;
        } else {
            break;
        }
    }
    if chars.get(pos) == Some(&'\\') {
        Ok(pos + 1)
    } else {
        Err(syntax_error("unterminated string gap"))
    }
}
