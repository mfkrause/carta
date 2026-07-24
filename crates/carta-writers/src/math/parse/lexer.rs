//! Tokenizer and user-macro expansion for TeX math source.

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

/// The most user-macro expansions performed for one expression, and the most tokens the expanded
/// stream may hold. Both bound a recursive definition (`\renewcommand{\a}{\a\a}`) so expansion always
/// halts; once either ceiling is reached, remaining uses stay unexpanded and fall back to verbatim.
const MACRO_EXPANSION_BUDGET: usize = 4096;
const MACRO_EXPANSION_MAX_TOKENS: usize = 65_536;

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
        // An optional-argument default (`[1][d]`) is not modeled; unexpanded keeps the expression verbatim.
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
/// unbraced token contributes itself. Returns `None` if the stream runs out before every argument is
/// read, so the use is left unexpanded.
fn read_macro_arguments(
    tokens: &[Token],
    pos: &mut usize,
    params: usize,
) -> Option<Vec<Vec<Token>>> {
    let mut arguments = Vec::with_capacity(params);
    for _ in 0..params {
        skip_spaces(tokens, pos);
        match tokens.get(*pos)? {
            Token::GroupOpen => arguments.push(read_group(tokens, pos)?),
            single => {
                arguments.push(vec![single.clone()]);
                *pos += 1;
            }
        }
    }
    Some(arguments)
}

/// Expand `\newcommand`/`\renewcommand` macros in a token stream: collect every definition and drop
/// it, then replace each later use with its body, substituting `#n` placeholders with the supplied
/// arguments. A stream that defines no macro is returned untouched. Expansion is bounded so a
/// self-referential definition halts, leaving any still-unexpanded use to fall back to verbatim.
pub(super) fn expand_macros(tokens: Vec<Token>) -> Vec<Token> {
    let mut macros: std::collections::BTreeMap<String, Macro> = std::collections::BTreeMap::new();
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
        return tokens;
    }
    let mut current = stripped;
    let mut budget = MACRO_EXPANSION_BUDGET;
    loop {
        let mut expanded = Vec::new();
        let mut changed = false;
        let mut index = 0;
        while let Some(tok) = current.get(index) {
            if budget > 0
                && let Token::Command(name) = tok
                && let Some(definition) = macros.get(name)
            {
                let mut after = index + 1;
                if let Some(arguments) =
                    read_macro_arguments(&current, &mut after, definition.params)
                {
                    for piece in &definition.body {
                        match piece {
                            BodyPiece::Literal(token) => expanded.push(token.clone()),
                            BodyPiece::Param(reference) => {
                                if let Some(argument) = arguments.get(reference - 1) {
                                    expanded.extend(argument.iter().cloned());
                                }
                            }
                        }
                    }
                    index = after;
                    changed = true;
                    budget -= 1;
                    continue;
                }
            }
            expanded.push(tok.clone());
            index += 1;
        }
        current = expanded;
        if !changed || budget == 0 || current.len() > MACRO_EXPANSION_MAX_TOKENS {
            return current;
        }
    }
}
