//! Parsing the `$`-delimited template language into a [`Template`] tree.
//!
//! Three passes: a lexer splits the source into literal text and directive tokens (handling `$$`
//! escapes and `$-- …` comments inline); a whitespace pass strips the lines occupied by the
//! directives of a block construct; and a tree builder folds the flat token list into nested
//! `$if$`/`$for$` nodes.
//!
//! ## Comments
//!
//! `$-- …` runs to the end of its line. When the comment begins at the very start of a line (column
//! zero, no preceding character on the line) the line's newline is swallowed with it; otherwise the
//! preceding content and the newline survive.
//!
//! ## Block control directives
//!
//! Whether a `$if$…$endif$` or `$for$…$endfor$` construct is laid out as a block is decided by its
//! opening directive: if `$if$`/`$for$` is the last non-whitespace on its line, the construct is a
//! block. In a block construct, the opening's trailing newline is swallowed, and every other
//! directive of that same construct (`$elseif$`/`$else$`/`$sep$` and the closing `$endif$`/
//! `$endfor$`) that likewise ends its own line has its leading indentation and trailing newline
//! removed. When the opening shares its line with other content the construct is inline: every one
//! of its directives — even a closing one alone on its line — keeps its surrounding whitespace and
//! newline verbatim, so an inline `$for$` whose `$endfor$` sits on its own line emits the blank
//! line that follows it.

use super::TemplateError;
use super::node::{Align, Expr, Node, Pipe, Template};

impl Template {
    /// Parse template source into a tree.
    ///
    /// # Errors
    /// [`TemplateError`] on an unterminated directive, an unmatched `$if$`/`$for$`, a dangling
    /// `$endif$`/`$endfor$`/`$else$`, or an unknown pipe.
    pub fn parse(source: &str) -> Result<Template, TemplateError> {
        let mut tokens = lex(source)?;
        trim_standalone(&mut tokens);
        let mut builder = Builder {
            tokens: &tokens,
            pos: 0,
        };
        let nodes = builder.sequence()?;
        if builder.pos != tokens.len() {
            return Err(TemplateError::new("unexpected control directive"));
        }
        Ok(Template { nodes })
    }
}

/// A lexer token: literal text, or a single directive.
#[derive(Debug, Clone)]
enum Token {
    Text(String),
    Var(Expr),
    Partial {
        name: String,
        map_over: Option<Expr>,
        sep: Option<String>,
    },
    If(Expr),
    ElseIf(Expr),
    Else,
    EndIf,
    For(Expr),
    Sep,
    EndFor,
}

/// Horizontal whitespace for the standalone-line and comment rules (a newline is never "blank").
fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t' || c == '\r'
}

fn lex(source: &str) -> Result<Vec<Token>, TemplateError> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens: Vec<Token> = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    // True at the start of a line before any character or directive on it — used to decide whether a
    // `$-- …` comment swallows its newline.
    let mut col_clean = true;

    while let Some(&c) = chars.get(i) {
        if c != '$' {
            text.push(c);
            col_clean = c == '\n';
            i += 1;
            continue;
        }

        match chars.get(i + 1) {
            Some('$') => {
                text.push('$');
                col_clean = false;
                i += 2;
            }
            Some('-') if chars.get(i + 2) == Some(&'-') => {
                let mut j = i + 3;
                while let Some(&d) = chars.get(j) {
                    if d == '\n' {
                        break;
                    }
                    j += 1;
                }
                if col_clean {
                    // Column-zero comment: drop the trailing newline too, keeping the line clean.
                    if chars.get(j) == Some(&'\n') {
                        j += 1;
                    }
                } // otherwise the newline (if any) is read as ordinary text next.
                i = j;
            }
            _ => {
                if !text.is_empty() {
                    tokens.push(Token::Text(std::mem::take(&mut text)));
                }
                let (token, next) = directive(&chars, i + 1)?;
                tokens.push(token);
                col_clean = false;
                i = next;
            }
        }
    }
    if !text.is_empty() {
        tokens.push(Token::Text(text));
    }
    Ok(tokens)
}

/// Parse one `$…$` directive whose interior begins at `start`. Returns the token and the index just
/// past the closing `$`.
fn directive(chars: &[char], start: usize) -> Result<(Token, usize), TemplateError> {
    let close = close_index(chars, start)
        .ok_or_else(|| TemplateError::new("unterminated directive (missing closing `$`)"))?;
    let interior: String = chars.get(start..close).unwrap_or_default().iter().collect();
    Ok((interior_token(&interior)?, close + 1))
}

/// Index of the `$` that closes a directive opened at `start`, skipping `$` that fall inside `[…]`
/// separator literals or `"…"` pipe arguments. A newline before the close means the directive is
/// unterminated.
fn close_index(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    let mut in_bracket = false;
    let mut in_quote = false;
    while let Some(&c) = chars.get(i) {
        match c {
            '\n' => return None,
            '"' if !in_bracket => in_quote = !in_quote,
            '[' if !in_quote => in_bracket = true,
            ']' if !in_quote => in_bracket = false,
            '$' if !in_quote && !in_bracket => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Classify a directive's interior text into a token.
fn interior_token(interior: &str) -> Result<Token, TemplateError> {
    let trimmed = interior.trim();
    match trimmed {
        "else" => return Ok(Token::Else),
        "endif" => return Ok(Token::EndIf),
        "sep" => return Ok(Token::Sep),
        "endfor" => return Ok(Token::EndFor),
        _ => {}
    }
    if let Some(arg) = keyword_arg(trimmed, "if") {
        return Ok(Token::If(parse_expr(arg)?));
    }
    if let Some(arg) = keyword_arg(trimmed, "elseif") {
        return Ok(Token::ElseIf(parse_expr(arg)?));
    }
    if let Some(arg) = keyword_arg(trimmed, "for") {
        return Ok(Token::For(parse_expr(arg)?));
    }
    value_token(trimmed)
}

/// If `text` is `keyword(<arg>)`, return the trimmed `<arg>`.
fn keyword_arg<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(keyword)?.trim_start();
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    Some(inner.trim())
}

/// Parse a non-keyword directive: a mapped partial, a plain partial, or a variable interpolation.
fn value_token(text: &str) -> Result<Token, TemplateError> {
    if let Some((target, rest)) = text.split_once(':') {
        let (name, sep) = partial_parts(rest)?;
        return Ok(Token::Partial {
            name,
            map_over: Some(parse_expr(target.trim())?),
            sep,
        });
    }
    if text.contains("()") {
        let (name, sep) = partial_parts(text)?;
        return Ok(Token::Partial {
            name,
            map_over: None,
            sep,
        });
    }
    Ok(Token::Var(parse_expr(text)?))
}

/// Split a partial reference `name()` or `name()[sep]` into its name and optional separator.
fn partial_parts(text: &str) -> Result<(String, Option<String>), TemplateError> {
    let (name, after) = text
        .trim()
        .split_once("()")
        .ok_or_else(|| TemplateError::new("malformed partial (expected `name()`)"))?;
    let after = after.trim();
    let sep = if after.is_empty() {
        None
    } else {
        Some(
            after
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .ok_or_else(|| TemplateError::new("malformed partial separator (expected `[…]`)"))?
                .to_string(),
        )
    };
    Ok((name.trim().to_string(), sep))
}

/// Parse a variable expression: a dotted path followed by `/pipe` filters.
fn parse_expr(text: &str) -> Result<Expr, TemplateError> {
    let mut parts = text.split('/');
    let head = parts.next().unwrap_or("").trim();
    let path: Vec<String> = head
        .split('.')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut pipes = Vec::new();
    for part in parts {
        pipes.push(parse_pipe(part.trim())?);
    }
    Ok(Expr { path, pipes })
}

fn parse_pipe(text: &str) -> Result<Pipe, TemplateError> {
    let args = pipe_args(text);
    let name = args.first().map_or("", String::as_str);
    let pipe = match name {
        "uppercase" => Pipe::Uppercase,
        "lowercase" => Pipe::Lowercase,
        "length" => Pipe::Length,
        "reverse" => Pipe::Reverse,
        "first" => Pipe::First,
        "last" => Pipe::Last,
        "rest" => Pipe::Rest,
        "allbutlast" => Pipe::AllButLast,
        "pairs" => Pipe::Pairs,
        "alpha" => Pipe::Alpha,
        "roman" => Pipe::Roman,
        "chomp" => Pipe::Chomp,
        "nowrap" => Pipe::Nowrap,
        "left" | "right" | "center" => {
            let align = match name {
                "right" => Align::Right,
                "center" => Align::Center,
                _ => Align::Left,
            };
            let width = args
                .get(1)
                .and_then(|w| w.parse::<usize>().ok())
                .ok_or_else(|| TemplateError::new("block pipe requires a width"))?;
            Pipe::Block {
                align,
                width,
                left: args.get(2).cloned().unwrap_or_default(),
                right: args.get(3).cloned().unwrap_or_default(),
            }
        }
        other => return Err(TemplateError::new(format!("unknown pipe: {other}"))),
    };
    Ok(pipe)
}

/// Tokenize a pipe's whitespace-separated arguments, honoring `"…"` so a border may contain spaces.
fn pipe_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut buf = String::new();
        if c == '"' {
            chars.next();
            for d in chars.by_ref() {
                if d == '"' {
                    break;
                }
                buf.push(d);
            }
        } else {
            while let Some(&d) = chars.peek() {
                if d.is_whitespace() {
                    break;
                }
                buf.push(d);
                chars.next();
            }
        }
        out.push(buf);
    }
    out
}

/// Strip the lines occupied by the directives of every block construct.
///
/// A `$if$`/`$for$` whose opening ends its line opens a block: its trailing newline is swallowed,
/// and each later directive of that construct (`$elseif$`/`$else$`/`$sep$`, the closing) that ends
/// its own line is likewise dropped. The block flag rides a nesting stack, so each construct's
/// interior directives consult the construct they belong to, not whichever directive came last.
///
/// All decisions are taken over the original token text in a first pass, then applied — so trimming
/// one directive's line never perturbs the line analysis of its neighbours.
fn trim_standalone(tokens: &mut [Token]) {
    // First pass over the original tokens: per directive, whether to drop its trailing newline and
    // (separately) the indentation on its own line. The block flag rides a nesting stack.
    let mut blocks: Vec<bool> = Vec::new();
    let decisions: Vec<(bool, bool)> = tokens
        .iter()
        .enumerate()
        .map(|(i, token)| {
            let consume = match token {
                Token::If(_) | Token::For(_) => {
                    let block = forward_blank(tokens, i);
                    blocks.push(block);
                    block
                }
                Token::ElseIf(_) | Token::Else | Token::Sep => {
                    blocks.last().copied().unwrap_or(false) && forward_blank(tokens, i)
                }
                Token::EndIf | Token::EndFor => {
                    blocks.pop().unwrap_or(false) && forward_blank(tokens, i)
                }
                _ => false,
            };
            (consume, consume && backward_blank(tokens, i))
        })
        .collect();
    // Second pass applies the decisions; the analysis above used the untrimmed text, so trimming
    // one directive's line never perturbs a neighbour's.
    for (i, &(drop_newline, drop_indent)) in decisions.iter().enumerate() {
        if drop_newline && let Some(Token::Text(t)) = tokens.get_mut(i + 1) {
            trim_leading_line(t);
        }
        if drop_indent
            && let Some(prev) = i.checked_sub(1)
            && let Some(Token::Text(t)) = tokens.get_mut(prev)
        {
            trim_trailing_line(t);
        }
    }
}

/// Whether everything before token `i` back to the previous newline is whitespace.
fn backward_blank(tokens: &[Token], i: usize) -> bool {
    match i.checked_sub(1) {
        None => true,
        Some(prev) => match tokens.get(prev) {
            Some(Token::Text(t)) => match t.rfind('\n') {
                Some(k) => t.get(k + 1..).unwrap_or("").chars().all(is_blank),
                None => prev == 0 && t.chars().all(is_blank),
            },
            _ => false,
        },
    }
}

/// Whether everything after token `i` through the next newline is whitespace.
fn forward_blank(tokens: &[Token], i: usize) -> bool {
    match tokens.get(i + 1) {
        None => true,
        Some(Token::Text(t)) => match t.find('\n') {
            Some(k) => t.get(..k).unwrap_or("").chars().all(is_blank),
            None => i + 1 == tokens.len() - 1 && t.chars().all(is_blank),
        },
        _ => false,
    }
}

/// Drop trailing blanks after the last newline (or the whole string if it has none).
fn trim_trailing_line(text: &mut String) {
    match text.rfind('\n') {
        Some(k) => text.truncate(k + 1),
        None => text.clear(),
    }
}

/// Drop the leading blanks and the first newline (or the whole string if it has none).
fn trim_leading_line(text: &mut String) {
    match text.find('\n') {
        Some(k) => *text = text.get(k + 1..).unwrap_or("").to_string(),
        None => text.clear(),
    }
}

/// Folds the flat token list into a node tree, matching `$if$`/`$for$` with their terminators.
struct Builder<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl Builder<'_> {
    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.pos).cloned()
    }

    /// Parse nodes until a terminator (`elseif`/`else`/`endif`/`sep`/`endfor`) or end of input; the
    /// terminator is left unconsumed for the caller.
    fn sequence(&mut self) -> Result<Vec<Node>, TemplateError> {
        let mut nodes = Vec::new();
        while let Some(token) = self.peek() {
            match token {
                Token::Text(s) => {
                    self.pos += 1;
                    nodes.push(Node::Literal(s));
                }
                Token::Var(expr) => {
                    self.pos += 1;
                    nodes.push(Node::Var(expr));
                }
                Token::Partial {
                    name,
                    map_over,
                    sep,
                } => {
                    self.pos += 1;
                    nodes.push(Node::Partial {
                        name,
                        map_over,
                        sep,
                    });
                }
                Token::If(_) => nodes.push(self.conditional()?),
                Token::For(_) => nodes.push(self.loop_node()?),
                Token::ElseIf(_) | Token::Else | Token::EndIf | Token::Sep | Token::EndFor => break,
            }
        }
        Ok(nodes)
    }

    fn conditional(&mut self) -> Result<Node, TemplateError> {
        let Some(Token::If(cond)) = self.peek() else {
            return Err(TemplateError::new("expected `if`"));
        };
        self.pos += 1;
        let mut branches = vec![(cond, self.sequence()?)];
        loop {
            match self.peek() {
                Some(Token::ElseIf(cond)) => {
                    self.pos += 1;
                    branches.push((cond, self.sequence()?));
                }
                Some(Token::Else) => {
                    self.pos += 1;
                    let otherwise = self.sequence()?;
                    self.expect(&Token::EndIf, "endif")?;
                    return Ok(Node::If {
                        branches,
                        otherwise,
                    });
                }
                Some(Token::EndIf) => {
                    self.pos += 1;
                    return Ok(Node::If {
                        branches,
                        otherwise: Vec::new(),
                    });
                }
                _ => return Err(TemplateError::new("unterminated `if` (missing `endif`)")),
            }
        }
    }

    fn loop_node(&mut self) -> Result<Node, TemplateError> {
        let Some(Token::For(expr)) = self.peek() else {
            return Err(TemplateError::new("expected `for`"));
        };
        self.pos += 1;
        // A single-segment loop expression also binds that name to the current element (so
        // `$for(xs)$…$xs$` works, as does `$for(m/pairs)$…$m.key$`); a pipe on the segment does not
        // change the name. `$it$` always refers to the element regardless.
        let bind = match expr.path.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        };
        let body = self.sequence()?;
        let mut sep = Vec::new();
        match self.peek() {
            Some(Token::Sep) => {
                self.pos += 1;
                sep = self.sequence()?;
                self.expect(&Token::EndFor, "endfor")?;
            }
            Some(Token::EndFor) => {
                self.pos += 1;
            }
            _ => return Err(TemplateError::new("unterminated `for` (missing `endfor`)")),
        }
        Ok(Node::For {
            expr,
            bind,
            body,
            sep,
        })
    }

    fn expect(&mut self, want: &Token, label: &str) -> Result<(), TemplateError> {
        match self.peek() {
            Some(ref got) if std::mem::discriminant(got) == std::mem::discriminant(want) => {
                self.pos += 1;
                Ok(())
            }
            _ => Err(TemplateError::new(format!("expected `{label}`"))),
        }
    }
}
