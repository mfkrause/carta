//! Tokenizer and user-macro expansion for TeX math source.

use std::collections::{BTreeMap, BTreeSet};

use super::{Token, skip_spaces};

/// Whether the character one position past the current peek is a digit. Used to decide whether a
/// leading `.` begins a decimal number (`.5`) or is an ordinary punctuation character.
fn peek_is_digit(chars: &std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let mut lookahead = chars.clone();
    lookahead.next();
    matches!(lookahead.next(), Some(d) if d.is_ascii_digit())
}

/// Consume one numeric literal from the front of `chars`: a run of digits with at most one interior
/// decimal point that is immediately followed by a digit. The cursor is left just past the number.
fn lex_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut number = String::new();
    let mut took_dot = false;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            number.push(c);
            chars.next();
        } else if c == '.' && !took_dot && peek_is_digit(chars) {
            took_dot = true;
            number.push(c);
            chars.next();
        } else {
            break;
        }
    }
    number
}

/// Tokenize TeX math source. Returns `None` only on a malformed control sequence we never accept.
pub(super) fn tokenize(src: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '\\' => {
                chars.next();
                match chars.peek().copied() {
                    Some(next) if next.is_ascii_alphabetic() => {
                        let mut name = String::new();
                        while let Some(&a) = chars.peek() {
                            if a.is_ascii_alphabetic() {
                                name.push(a);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        tokens.push(Token::Command(name));
                    }
                    Some(sym) => {
                        chars.next();
                        tokens.push(Token::Command(sym.to_string()));
                    }
                    None => tokens.push(Token::Char('\\')),
                }
            }
            '^' => {
                chars.next();
                tokens.push(Token::Sup);
            }
            '_' => {
                chars.next();
                tokens.push(Token::Sub);
            }
            '{' => {
                chars.next();
                tokens.push(Token::GroupOpen);
            }
            '}' => {
                chars.next();
                tokens.push(Token::GroupClose);
            }
            c if c.is_whitespace() => {
                chars.next();
                while let Some(&w) = chars.peek() {
                    if w.is_whitespace() {
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Space);
            }
            c if c.is_ascii_digit() || (c == '.' && peek_is_digit(&chars)) => {
                let number = lex_number(&mut chars);
                tokens.push(Token::Number(number));
            }
            _ => {
                chars.next();
                tokens.push(Token::Char(c));
            }
        }
    }
    tokens
}

/// Initial expansion allowances, extended in proportion to the source's token weight.
const MACRO_EXPANSION_BUDGET: usize = 4096;
const MACRO_TOKEN_BUDGET: usize = 65_536;

type Call = (String, Vec<Vec<Token>>);

struct Frame {
    tokens: std::vec::IntoIter<Token>,
    call: Option<Call>,
}

fn next_token(frames: &mut Vec<Frame>, active: &mut BTreeSet<Call>) -> Option<Token> {
    while let Some(frame) = frames.last_mut() {
        if let Some(token) = frame.tokens.next() {
            return Some(token);
        }
        if let Some(call) = frames.pop()?.call {
            active.remove(&call);
        }
    }
    None
}

fn token_weight(token: &Token) -> usize {
    match token {
        Token::Command(text) | Token::Number(text) => text.len().max(1),
        Token::Char(character) => character.len_utf8(),
        _ => 1,
    }
}

/// One `\newcommand`/`\renewcommand` definition: its mandatory-argument count and its replacement
/// body, in which each `#n` placeholder is recorded as a parameter reference.
struct Macro {
    params: usize,
    body: Vec<BodyPiece>,
}

/// One element of a macro's replacement body: either a literal token or a reference to the nth
/// argument the use supplies.
enum BodyPiece {
    Literal(Token),
    Param(usize),
}

/// Read a balanced brace group's inner tokens, assuming the cursor sits on its opening `{` and
/// advancing past the matching `}`. Returns `None` if the group never closes.
fn read_group(tokens: &[Token], pos: &mut usize) -> Option<Vec<Token>> {
    if !matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
        return None;
    }
    *pos += 1;
    let mut depth = 1usize;
    let mut inner = Vec::new();
    while let Some(tok) = tokens.get(*pos) {
        match tok {
            Token::GroupOpen => depth += 1,
            Token::GroupClose => {
                depth -= 1;
                if depth == 0 {
                    *pos += 1;
                    return Some(inner);
                }
            }
            _ => {}
        }
        inner.push(tok.clone());
        *pos += 1;
    }
    None
}

/// Compile a macro's raw body tokens into replacement pieces, turning each `#n` (with `n` a valid
/// parameter index) into a parameter reference and leaving every other token literal.
fn compile_macro_body(tokens: &[Token], params: usize) -> Vec<BodyPiece> {
    let mut body = Vec::new();
    let mut index = 0;
    while let Some(tok) = tokens.get(index) {
        if matches!(tok, Token::Char('#'))
            && let Some(Token::Number(digits)) = tokens.get(index + 1)
            && let Some(first) = digits.chars().next()
            && let Some(reference) = first.to_digit(10)
            && (1..=params).contains(&(reference as usize))
        {
            body.push(BodyPiece::Param(reference as usize));
            let rest: String = digits.chars().skip(1).collect();
            if !rest.is_empty() {
                body.push(BodyPiece::Literal(Token::Number(rest)));
            }
            index += 2;
            continue;
        }
        body.push(BodyPiece::Literal(tok.clone()));
        index += 1;
    }
    body
}

/// Parse one `\newcommand`/`\renewcommand` definition beginning at `start` (the control-word token).
/// On success returns the macro name, its compiled form, and the position just past the definition;
/// `None` for any shape outside the supported form (a braced or bare name, an optional `[N]`
/// argument count with no default, and a braced body), leaving the caller to treat the token
/// literally.
fn parse_macro_definition(tokens: &[Token], start: usize) -> Option<(String, Macro, usize)> {
    let mut pos = start + 1;
    skip_spaces(tokens, &mut pos);
    let name = match tokens.get(pos)? {
        Token::Command(name) => {
            pos += 1;
            name.clone()
        }
        Token::GroupOpen => {
            pos += 1;
            let Token::Command(name) = tokens.get(pos)? else {
                return None;
            };
            let name = name.clone();
            pos += 1;
            match tokens.get(pos)? {
                Token::GroupClose => pos += 1,
                _ => return None,
            }
            name
        }
        _ => return None,
    };
    skip_spaces(tokens, &mut pos);
    let mut params = 0usize;
    if matches!(tokens.get(pos), Some(Token::Char('['))) {
        pos += 1;
        let Token::Number(count) = tokens.get(pos)? else {
            return None;
        };
        let count = count.parse::<usize>().ok()?;
        if count > 9 {
            return None;
        }
        pos += 1;
        match tokens.get(pos)? {
            Token::Char(']') => pos += 1,
            _ => return None,
        }
        params = count;
        skip_spaces(tokens, &mut pos);
        if matches!(tokens.get(pos), Some(Token::Char('['))) {
            return None;
        }
    }
    let body_tokens = read_group(tokens, &mut pos)?;
    Some((
        name,
        Macro {
            params,
            body: compile_macro_body(&body_tokens, params),
        },
        pos,
    ))
}

/// Read the `params` arguments a macro use supplies: a braced group contributes its inner tokens, an
/// unbraced token contributes itself. Returns `None` for a missing or unclosed argument.
fn read_macro_arguments(
    frames: &mut Vec<Frame>,
    active: &mut BTreeSet<Call>,
    params: usize,
) -> Option<Vec<Vec<Token>>> {
    let mut arguments = Vec::with_capacity(params);
    for _ in 0..params {
        let mut token = next_token(frames, active)?;
        while token == Token::Space {
            token = next_token(frames, active)?;
        }
        if token != Token::GroupOpen {
            arguments.push(vec![token]);
            continue;
        }
        let mut group = Vec::new();
        let mut depth = 1usize;
        loop {
            let token = next_token(frames, active)?;
            match token {
                Token::GroupOpen => depth += 1,
                Token::GroupClose => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            group.push(token);
        }
        arguments.push(group);
    }
    Some(arguments)
}

/// Expand `\newcommand`/`\renewcommand` macros in a token stream: collect every definition and drop
/// it, then replace each later use with its body, substituting `#n` placeholders with the supplied
/// arguments. A stream that defines no macro is returned untouched. Returns `None` when expansion
/// exceeds its work limits, so the whole expression falls back to verbatim.
pub(super) fn expand_macros(tokens: Vec<Token>) -> Option<Vec<Token>> {
    let mut macros: BTreeMap<String, Macro> = BTreeMap::new();
    let mut stripped = Vec::new();
    let mut pos = 0;
    while let Some(tok) = tokens.get(pos) {
        if let Token::Command(name) = tok
            && (name == "newcommand" || name == "renewcommand")
            && let Some((name, definition, next)) = parse_macro_definition(&tokens, pos)
        {
            macros.insert(name, definition);
            pos = next;
            continue;
        }
        stripped.push(tok.clone());
        pos += 1;
    }
    if macros.is_empty() {
        return Some(tokens);
    }
    let weight = tokens
        .iter()
        .map(token_weight)
        .fold(0usize, usize::saturating_add);
    let mut budget = weight
        .saturating_mul(64)
        .saturating_add(MACRO_EXPANSION_BUDGET);
    let mut token_budget = weight.saturating_mul(64).saturating_add(MACRO_TOKEN_BUDGET);
    let mut active = BTreeSet::new();
    let mut frames = vec![Frame {
        tokens: stripped.into_iter(),
        call: None,
    }];
    let mut output = Vec::new();
    while let Some(token) = next_token(&mut frames, &mut active) {
        let Token::Command(name) = &token else {
            output.push(token);
            continue;
        };
        let Some(definition) = macros.get(name) else {
            output.push(token);
            continue;
        };
        budget = budget.checked_sub(1)?;
        let arguments = read_macro_arguments(&mut frames, &mut active, definition.params)?;
        let call = (name.clone(), arguments.clone());
        if !active.insert(call.clone()) {
            return None;
        }
        let mut expanded = Vec::new();
        for piece in &definition.body {
            let replacement = match piece {
                BodyPiece::Literal(token) => std::slice::from_ref(token),
                BodyPiece::Param(reference) => arguments.get(reference - 1)?.as_slice(),
            };
            for token in replacement {
                token_budget = token_budget.checked_sub(token_weight(token))?;
                expanded.push(token.clone());
            }
        }
        frames.push(Frame {
            tokens: expanded.into_iter(),
            call: Some(call),
        });
    }
    Some(output)
}
