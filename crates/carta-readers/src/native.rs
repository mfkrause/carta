//! Native reader: parses the document model's printed textual form back into the model.
//!
//! The native format is the human-readable rendering of the AST — constructor names applied to
//! their arguments, with strings, tuples, lists, and records written in a small constructor-
//! application value syntax (`Para [ Str "x" ]`, `("id", ["class"], [("k","v")])`). Parsing has two
//! stages: a
//! lexer (`tokenize`) splits the source into `Token`s, and a recursive-descent `Parser`
//! consumes them type-directedly — each AST shape has a dedicated method, so the same `(…, …, …)`
//! tuple is read as the type the position calls for.
//!
//! Top level accepts, in order of preference, a whole document (`Pandoc <meta> <blocks>`), a block
//! list, a single block, an inline list, or a single inline; the last three are wrapped to form a
//! document (a lone inline or inline list becomes a `Plain` block).

use std::collections::BTreeMap;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, Citation, CitationMode, ColSpec, ColWidth, Document,
    Format, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType, MetaValue,
    QuoteType, Row, Table, TableBody, TableFoot, TableHead, Target,
};
use carta_core::{Error, Reader, ReaderOptions, Result};

/// Parses the document model's printed textual form into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeReader;

impl Reader for NativeReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        let tokens = tokenize(input)?;
        let mut parser = Parser { tokens, pos: 0 };
        let document = parser.parse_document()?;
        if parser.pos != parser.tokens.len() {
            return Err(syntax_error("unexpected trailing input"));
        }
        Ok(document)
    }
}

fn syntax_error(message: impl Into<String>) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    ))
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Equals,
    Ident(String),
    Str(String),
    Num(String),
}

fn tokenize(input: &str) -> Result<Vec<Token>> {
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

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

/// Defines a parser method that reads a constructor name and maps it to a value of `$ty`. Each arm
/// pairs a constructor name with the expression it produces (which may itself consume further tokens,
/// as a constructor carrying a payload does); an unrecognized name is a syntax error naming `$label`.
macro_rules! parse_constructor {
    (
        $method:ident -> $ty:ty, $label:literal {
            $( $tag:literal => $value:expr ),* $(,)?
        }
    ) => {
        fn $method(&mut self) -> Result<$ty> {
            match self.constructor()?.as_str() {
                $( $tag => Ok($value), )*
                other => {
                    Err(syntax_error(format!(concat!("unknown ", $label, " '{}'"), other)))
                }
            }
        }
    };
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_ident(&self) -> Option<&str> {
        match self.peek() {
            Some(Token::Ident(name)) => Some(name.as_str()),
            _ => None,
        }
    }

    fn advance(&mut self) -> Result<Token> {
        match self.tokens.get(self.pos) {
            Some(token) => {
                let token = token.clone();
                self.pos += 1;
                Ok(token)
            }
            None => Err(syntax_error("unexpected end of input")),
        }
    }

    /// Consume the current token by moving it out of the stream, leaving a cheap placeholder behind.
    /// Used where the token owns heap data (idents, strings, numbers) that the caller takes ownership
    /// of; consumed positions are never revisited.
    fn take(&mut self) -> Result<Token> {
        match self.tokens.get_mut(self.pos) {
            Some(slot) => {
                let token = std::mem::replace(slot, Token::Comma);
                self.pos += 1;
                Ok(token)
            }
            None => Err(syntax_error("unexpected end of input")),
        }
    }

    fn eat(&mut self, expected: &Token) -> Result<()> {
        match self.tokens.get(self.pos) {
            Some(found) if found == expected => {
                self.pos += 1;
                Ok(())
            }
            Some(found) => Err(syntax_error(format!(
                "expected {expected:?}, found {found:?}"
            ))),
            None => Err(syntax_error("unexpected end of input")),
        }
    }

    fn eat_ident(&mut self, name: &str) -> Result<()> {
        match self.tokens.get(self.pos) {
            Some(Token::Ident(found)) if found == name => {
                self.pos += 1;
                Ok(())
            }
            Some(found) => Err(syntax_error(format!("expected '{name}', found {found:?}"))),
            None => Err(syntax_error("unexpected end of input")),
        }
    }

    fn constructor(&mut self) -> Result<String> {
        match self.take()? {
            Token::Ident(name) => Ok(name),
            found => Err(syntax_error(format!(
                "expected a constructor, found {found:?}"
            ))),
        }
    }

    fn open_paren(&mut self) -> bool {
        if self.peek() == Some(&Token::LParen) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn close_if(&mut self, opened: bool) -> Result<()> {
        if opened {
            self.eat(&Token::RParen)
        } else {
            Ok(())
        }
    }

    fn parse_list<T>(&mut self, element: fn(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        self.eat(&Token::LBracket)?;
        let mut items = Vec::new();
        if self.peek() == Some(&Token::RBracket) {
            self.pos += 1;
            return Ok(items);
        }
        loop {
            items.push(element(self)?);
            match self.advance()? {
                Token::Comma => {}
                Token::RBracket => break,
                found => {
                    return Err(syntax_error(format!(
                        "expected ',' or ']', found {found:?}"
                    )));
                }
            }
        }
        Ok(items)
    }

    fn parse_string(&mut self) -> Result<String> {
        match self.take()? {
            Token::Str(text) => Ok(text),
            found => Err(syntax_error(format!("expected a string, found {found:?}"))),
        }
    }

    fn parse_i32(&mut self) -> Result<i32> {
        let opened = self.open_paren();
        let value = match self.take()? {
            Token::Num(number) => number
                .parse::<i32>()
                .map_err(|error| syntax_error(format!("invalid integer '{number}': {error}")))?,
            found => {
                return Err(syntax_error(format!(
                    "expected an integer, found {found:?}"
                )));
            }
        };
        self.close_if(opened)?;
        Ok(value)
    }

    fn parse_f64(&mut self) -> Result<f64> {
        let opened = self.open_paren();
        let value = match self.take()? {
            Token::Num(number) => number
                .parse::<f64>()
                .map_err(|error| syntax_error(format!("invalid number '{number}': {error}")))?,
            found => return Err(syntax_error(format!("expected a number, found {found:?}"))),
        };
        self.close_if(opened)?;
        Ok(value)
    }

    fn parse_document(&mut self) -> Result<Document> {
        if self.peek_ident() == Some("Pandoc") {
            self.pos += 1;
            let meta = self.parse_meta()?;
            let blocks = self.parse_block_list()?;
            return Ok(Document {
                meta: meta.into_iter().map(|(k, v)| (k.into(), v)).collect(),
                blocks,
                ..Default::default()
            });
        }
        if self.peek() == Some(&Token::LBracket) {
            let blocks = match self.tokens.get(self.pos + 1) {
                Some(Token::RBracket) => self.parse_block_list()?,
                Some(Token::Ident(name)) if is_block_tag(name) => self.parse_block_list()?,
                Some(Token::Ident(name)) if is_inline_tag(name) => {
                    vec![Block::Plain(self.parse_inline_list()?)]
                }
                _ => return Err(syntax_error("unrecognized list element")),
            };
            return Ok(Document {
                blocks,
                ..Default::default()
            });
        }
        match self.peek_ident() {
            Some(name) if is_block_tag(name) => {
                let block = self.parse_block()?;
                Ok(Document {
                    blocks: vec![block],
                    ..Default::default()
                })
            }
            Some(name) if is_inline_tag(name) => {
                let inline = self.parse_inline()?;
                Ok(Document {
                    blocks: vec![Block::Plain(vec![inline])],
                    ..Default::default()
                })
            }
            _ => Err(syntax_error("input is not a recognized native document")),
        }
    }

    fn parse_meta(&mut self) -> Result<BTreeMap<String, MetaValue>> {
        let opened = self.open_paren();
        self.eat_ident("Meta")?;
        self.eat(&Token::LBrace)?;
        self.eat_ident("unMeta")?;
        self.eat(&Token::Equals)?;
        let map = self.parse_from_list()?;
        self.eat(&Token::RBrace)?;
        self.close_if(opened)?;
        Ok(map)
    }

    fn parse_from_list(&mut self) -> Result<BTreeMap<String, MetaValue>> {
        let opened = self.open_paren();
        self.eat_ident("fromList")?;
        let pairs = self.parse_list(Self::parse_meta_pair)?;
        self.close_if(opened)?;
        Ok(pairs.into_iter().collect())
    }

    fn parse_meta_pair(&mut self) -> Result<(String, MetaValue)> {
        self.eat(&Token::LParen)?;
        let key = self.parse_string()?;
        self.eat(&Token::Comma)?;
        let value = self.parse_meta_value()?;
        self.eat(&Token::RParen)?;
        Ok((key, value))
    }

    fn parse_meta_value(&mut self) -> Result<MetaValue> {
        let name = self.constructor()?;
        match name.as_str() {
            "MetaMap" => Ok(MetaValue::MetaMap(
                self.parse_from_list()?
                    .into_iter()
                    .map(|(k, v)| (k.into(), v))
                    .collect(),
            )),
            "MetaList" => Ok(MetaValue::MetaList(
                self.parse_list(Self::parse_meta_value)?,
            )),
            "MetaBool" => Ok(MetaValue::MetaBool(self.parse_bool()?)),
            "MetaString" => Ok(MetaValue::MetaString(self.parse_string()?.into())),
            "MetaInlines" => Ok(MetaValue::MetaInlines(self.parse_inline_list()?)),
            "MetaBlocks" => Ok(MetaValue::MetaBlocks(self.parse_block_list()?)),
            other => Err(syntax_error(format!("unknown metadata value '{other}'"))),
        }
    }

    fn parse_bool(&mut self) -> Result<bool> {
        match self.constructor()?.as_str() {
            "True" => Ok(true),
            "False" => Ok(false),
            other => Err(syntax_error(format!("expected a boolean, found '{other}'"))),
        }
    }

    fn parse_block_list(&mut self) -> Result<Vec<Block>> {
        self.parse_list(Self::parse_block)
    }

    fn parse_inline_list(&mut self) -> Result<Vec<Inline>> {
        self.parse_list(Self::parse_inline)
    }

    fn parse_block(&mut self) -> Result<Block> {
        let name = self.constructor()?;
        match name.as_str() {
            "Plain" => Ok(Block::Plain(self.parse_inline_list()?)),
            "Para" => Ok(Block::Para(self.parse_inline_list()?)),
            "LineBlock" => Ok(Block::LineBlock(self.parse_list(Self::parse_inline_list)?)),
            "CodeBlock" => {
                let attr = self.parse_attr()?;
                let text = self.parse_string()?;
                Ok(Block::CodeBlock(Box::new(attr), text.into()))
            }
            "RawBlock" => {
                let format = self.parse_format()?;
                let text = self.parse_string()?;
                Ok(Block::RawBlock(format, text.into()))
            }
            "BlockQuote" => Ok(Block::BlockQuote(self.parse_block_list()?)),
            "OrderedList" => {
                let attributes = self.parse_list_attributes()?;
                let items = self.parse_list(Self::parse_block_list)?;
                Ok(Block::OrderedList(attributes, items))
            }
            "BulletList" => Ok(Block::BulletList(self.parse_list(Self::parse_block_list)?)),
            "DefinitionList" => Ok(Block::DefinitionList(
                self.parse_list(Self::parse_definition_item)?,
            )),
            "Header" => {
                let level = self.parse_i32()?;
                let attr = self.parse_attr()?;
                let inlines = self.parse_inline_list()?;
                Ok(Block::Header(level, Box::new(attr), inlines))
            }
            "HorizontalRule" => Ok(Block::HorizontalRule),
            "Table" => Ok(Block::Table(Box::new(self.parse_table()?))),
            "Figure" => {
                let attr = self.parse_attr()?;
                let caption = self.parse_caption()?;
                let blocks = self.parse_block_list()?;
                Ok(Block::Figure(Box::new(attr), Box::new(caption), blocks))
            }
            "Div" => {
                let attr = self.parse_attr()?;
                let blocks = self.parse_block_list()?;
                Ok(Block::Div(Box::new(attr), blocks))
            }
            other => Err(syntax_error(format!("unknown block '{other}'"))),
        }
    }

    fn parse_inline(&mut self) -> Result<Inline> {
        let name = self.constructor()?;
        match name.as_str() {
            "Str" => Ok(Inline::Str(self.parse_string()?.into())),
            "Emph" => Ok(Inline::Emph(self.parse_inline_list()?)),
            "Underline" => Ok(Inline::Underline(self.parse_inline_list()?)),
            "Strong" => Ok(Inline::Strong(self.parse_inline_list()?)),
            "Strikeout" => Ok(Inline::Strikeout(self.parse_inline_list()?)),
            "Superscript" => Ok(Inline::Superscript(self.parse_inline_list()?)),
            "Subscript" => Ok(Inline::Subscript(self.parse_inline_list()?)),
            "SmallCaps" => Ok(Inline::SmallCaps(self.parse_inline_list()?)),
            "Quoted" => {
                let quote = self.parse_quote_type()?;
                let inlines = self.parse_inline_list()?;
                Ok(Inline::Quoted(quote, inlines))
            }
            "Cite" => {
                let citations = self.parse_list(Self::parse_citation)?;
                let inlines = self.parse_inline_list()?;
                Ok(Inline::Cite(citations, inlines))
            }
            "Code" => {
                let attr = self.parse_attr()?;
                let text = self.parse_string()?;
                Ok(Inline::Code(Box::new(attr), text.into()))
            }
            "Space" => Ok(Inline::Space),
            "SoftBreak" => Ok(Inline::SoftBreak),
            "LineBreak" => Ok(Inline::LineBreak),
            "Math" => {
                let math = self.parse_math_type()?;
                let text = self.parse_string()?;
                Ok(Inline::Math(math, text.into()))
            }
            "RawInline" => {
                let format = self.parse_format()?;
                let text = self.parse_string()?;
                Ok(Inline::RawInline(format, text.into()))
            }
            "Link" => {
                let attr = self.parse_attr()?;
                let inlines = self.parse_inline_list()?;
                let target = self.parse_target()?;
                Ok(Inline::Link(Box::new(attr), inlines, Box::new(target)))
            }
            "Image" => {
                let attr = self.parse_attr()?;
                let inlines = self.parse_inline_list()?;
                let target = self.parse_target()?;
                Ok(Inline::Image(Box::new(attr), inlines, Box::new(target)))
            }
            "Note" => Ok(Inline::Note(self.parse_block_list()?)),
            "Span" => {
                let attr = self.parse_attr()?;
                let inlines = self.parse_inline_list()?;
                Ok(Inline::Span(Box::new(attr), inlines))
            }
            other => Err(syntax_error(format!("unknown inline '{other}'"))),
        }
    }

    fn parse_attr(&mut self) -> Result<Attr> {
        self.eat(&Token::LParen)?;
        let id = self.parse_string()?;
        self.eat(&Token::Comma)?;
        let classes = self.parse_list(Self::parse_string)?;
        self.eat(&Token::Comma)?;
        let attributes = self.parse_list(Self::parse_string_pair)?;
        self.eat(&Token::RParen)?;
        Ok(Attr {
            id: id.into(),
            classes: classes.into_iter().map(Into::into).collect(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        })
    }

    fn parse_string_pair(&mut self) -> Result<(String, String)> {
        self.eat(&Token::LParen)?;
        let key = self.parse_string()?;
        self.eat(&Token::Comma)?;
        let value = self.parse_string()?;
        self.eat(&Token::RParen)?;
        Ok((key, value))
    }

    fn parse_target(&mut self) -> Result<Target> {
        self.eat(&Token::LParen)?;
        let url = self.parse_string()?;
        self.eat(&Token::Comma)?;
        let title = self.parse_string()?;
        self.eat(&Token::RParen)?;
        Ok(Target {
            url: url.into(),
            title: title.into(),
        })
    }

    fn parse_format(&mut self) -> Result<Format> {
        let opened = self.open_paren();
        self.eat_ident("Format")?;
        let name = self.parse_string()?;
        self.close_if(opened)?;
        Ok(Format(name.into()))
    }

    fn parse_list_attributes(&mut self) -> Result<ListAttributes> {
        self.eat(&Token::LParen)?;
        let start = self.parse_i32()?;
        self.eat(&Token::Comma)?;
        let style = self.parse_list_number_style()?;
        self.eat(&Token::Comma)?;
        let delim = self.parse_list_number_delim()?;
        self.eat(&Token::RParen)?;
        Ok(ListAttributes {
            start,
            style,
            delim,
        })
    }

    fn parse_definition_item(&mut self) -> Result<(Vec<Inline>, Vec<Vec<Block>>)> {
        self.eat(&Token::LParen)?;
        let term = self.parse_inline_list()?;
        self.eat(&Token::Comma)?;
        let definitions = self.parse_list(Self::parse_block_list)?;
        self.eat(&Token::RParen)?;
        Ok((term, definitions))
    }

    parse_constructor! {
        parse_quote_type -> QuoteType, "quote type" {
            "SingleQuote" => QuoteType::SingleQuote,
            "DoubleQuote" => QuoteType::DoubleQuote,
        }
    }

    parse_constructor! {
        parse_math_type -> MathType, "math type" {
            "InlineMath" => MathType::InlineMath,
            "DisplayMath" => MathType::DisplayMath,
        }
    }

    parse_constructor! {
        parse_list_number_style -> ListNumberStyle, "list number style" {
            "DefaultStyle" => ListNumberStyle::DefaultStyle,
            "Example" => ListNumberStyle::Example,
            "Decimal" => ListNumberStyle::Decimal,
            "LowerRoman" => ListNumberStyle::LowerRoman,
            "UpperRoman" => ListNumberStyle::UpperRoman,
            "LowerAlpha" => ListNumberStyle::LowerAlpha,
            "UpperAlpha" => ListNumberStyle::UpperAlpha,
        }
    }

    parse_constructor! {
        parse_list_number_delim -> ListNumberDelim, "list number delimiter" {
            "DefaultDelim" => ListNumberDelim::DefaultDelim,
            "Period" => ListNumberDelim::Period,
            "OneParen" => ListNumberDelim::OneParen,
            "TwoParens" => ListNumberDelim::TwoParens,
        }
    }

    parse_constructor! {
        parse_citation_mode -> CitationMode, "citation mode" {
            "AuthorInText" => CitationMode::AuthorInText,
            "SuppressAuthor" => CitationMode::SuppressAuthor,
            "NormalCitation" => CitationMode::NormalCitation,
        }
    }

    parse_constructor! {
        parse_alignment -> Alignment, "alignment" {
            "AlignLeft" => Alignment::AlignLeft,
            "AlignRight" => Alignment::AlignRight,
            "AlignCenter" => Alignment::AlignCenter,
            "AlignDefault" => Alignment::AlignDefault,
        }
    }

    fn parse_col_width(&mut self) -> Result<ColWidth> {
        match self.constructor()?.as_str() {
            "ColWidthDefault" => Ok(ColWidth::ColWidthDefault),
            "ColWidth" => Ok(ColWidth::ColWidth(self.parse_f64()?)),
            other => Err(syntax_error(format!("unknown column width '{other}'"))),
        }
    }

    fn parse_col_spec(&mut self) -> Result<ColSpec> {
        self.eat(&Token::LParen)?;
        let align = self.parse_alignment()?;
        self.eat(&Token::Comma)?;
        let width = self.parse_col_width()?;
        self.eat(&Token::RParen)?;
        Ok(ColSpec { align, width })
    }

    fn parse_citation(&mut self) -> Result<Citation> {
        let opened = self.open_paren();
        self.eat_ident("Citation")?;
        self.eat(&Token::LBrace)?;
        let mut citation = Citation {
            id: carta_ast::Text::default(),
            prefix: Vec::new(),
            suffix: Vec::new(),
            mode: CitationMode::NormalCitation,
            note_num: 0,
            hash: 0,
        };
        loop {
            let field = self.constructor()?;
            self.eat(&Token::Equals)?;
            match field.as_str() {
                "citationId" => citation.id = self.parse_string()?.into(),
                "citationPrefix" => citation.prefix = self.parse_inline_list()?,
                "citationSuffix" => citation.suffix = self.parse_inline_list()?,
                "citationMode" => citation.mode = self.parse_citation_mode()?,
                "citationNoteNum" => citation.note_num = self.parse_i32()?,
                "citationHash" => citation.hash = self.parse_i32()?,
                other => return Err(syntax_error(format!("unknown citation field '{other}'"))),
            }
            match self.advance()? {
                Token::Comma => {}
                Token::RBrace => break,
                found => {
                    return Err(syntax_error(format!(
                        "expected ',' or '}}', found {found:?}"
                    )));
                }
            }
        }
        self.close_if(opened)?;
        Ok(citation)
    }

    fn parse_caption(&mut self) -> Result<Caption> {
        let opened = self.open_paren();
        self.eat_ident("Caption")?;
        let short = self.parse_maybe_inlines()?;
        let long = self.parse_block_list()?;
        self.close_if(opened)?;
        Ok(Caption { short, long })
    }

    fn parse_maybe_inlines(&mut self) -> Result<Option<Vec<Inline>>> {
        let opened = self.open_paren();
        let result = if self.peek_ident() == Some("Nothing") {
            self.pos += 1;
            None
        } else {
            self.eat_ident("Just")?;
            Some(self.parse_inline_list()?)
        };
        self.close_if(opened)?;
        Ok(result)
    }

    fn parse_table(&mut self) -> Result<Table> {
        let attr = self.parse_attr()?;
        let caption = self.parse_caption()?;
        let col_specs = self.parse_list(Self::parse_col_spec)?;
        let head = self.parse_table_head()?;
        let bodies = self.parse_list(Self::parse_table_body)?;
        let foot = self.parse_table_foot()?;
        Ok(Table {
            attr,
            caption,
            col_specs,
            head,
            bodies,
            foot,
        })
    }

    fn parse_table_head(&mut self) -> Result<TableHead> {
        let opened = self.open_paren();
        self.eat_ident("TableHead")?;
        let attr = self.parse_attr()?;
        let rows = self.parse_list(Self::parse_row)?;
        self.close_if(opened)?;
        Ok(TableHead { attr, rows })
    }

    fn parse_table_foot(&mut self) -> Result<TableFoot> {
        let opened = self.open_paren();
        self.eat_ident("TableFoot")?;
        let attr = self.parse_attr()?;
        let rows = self.parse_list(Self::parse_row)?;
        self.close_if(opened)?;
        Ok(TableFoot { attr, rows })
    }

    fn parse_table_body(&mut self) -> Result<TableBody> {
        let opened = self.open_paren();
        self.eat_ident("TableBody")?;
        let attr = self.parse_attr()?;
        let row_head_columns = self.parse_int_newtype("RowHeadColumns")?;
        let head = self.parse_list(Self::parse_row)?;
        let body = self.parse_list(Self::parse_row)?;
        self.close_if(opened)?;
        Ok(TableBody {
            attr,
            row_head_columns,
            head,
            body,
        })
    }

    fn parse_row(&mut self) -> Result<Row> {
        let opened = self.open_paren();
        self.eat_ident("Row")?;
        let attr = self.parse_attr()?;
        let cells = self.parse_list(Self::parse_cell)?;
        self.close_if(opened)?;
        Ok(Row { attr, cells })
    }

    fn parse_cell(&mut self) -> Result<Cell> {
        let opened = self.open_paren();
        self.eat_ident("Cell")?;
        let attr = self.parse_attr()?;
        let align = self.parse_alignment()?;
        let row_span = self.parse_int_newtype("RowSpan")?;
        let col_span = self.parse_int_newtype("ColSpan")?;
        let content = self.parse_block_list()?;
        self.close_if(opened)?;
        Ok(Cell {
            attr,
            align,
            row_span,
            col_span,
            content,
        })
    }

    fn parse_int_newtype(&mut self, name: &str) -> Result<i32> {
        let opened = self.open_paren();
        self.eat_ident(name)?;
        let value = self.parse_i32()?;
        self.close_if(opened)?;
        Ok(value)
    }
}

fn is_block_tag(name: &str) -> bool {
    carta_ast::BLOCK_TAGS.contains(&name)
}

fn is_inline_tag(name: &str) -> bool {
    carta_ast::INLINE_TAGS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Document {
        NativeReader
            .read(input, &ReaderOptions::default())
            .expect("native input should parse")
    }

    fn parse_err(input: &str) -> String {
        NativeReader
            .read(input, &ReaderOptions::default())
            .expect_err("native input should fail")
            .to_string()
    }

    fn only_block(input: &str) -> Block {
        let Document { blocks, .. } = parse(input);
        match blocks.into_iter().next() {
            Some(block) => block,
            None => panic!("expected a single block"),
        }
    }

    fn str_inline(text: &str) -> Inline {
        Inline::Str(text.to_string().into())
    }

    #[test]
    fn parses_full_document_with_meta() {
        let document = parse(
            r#"Pandoc (Meta {unMeta = fromList [("title", MetaInlines [Str "Hi"])]}) [Para [Str "Body"]]"#,
        );
        assert_eq!(
            document.meta.get("title"),
            Some(&MetaValue::MetaInlines(vec![str_inline("Hi")]))
        );
        assert_eq!(document.blocks, vec![Block::Para(vec![str_inline("Body")])]);
    }

    #[test]
    fn parses_every_meta_value_shape() {
        let document = parse(
            r#"Pandoc (Meta {unMeta = fromList [("m", MetaMap (fromList [("k", MetaString "v")])), ("l", MetaList [MetaBool True, MetaBool False]), ("b", MetaBlocks [Plain [Str "p"]])]}) []"#,
        );
        assert_eq!(
            document.meta.get("m"),
            Some(&MetaValue::MetaMap(
                [(
                    "k".to_string().into(),
                    MetaValue::MetaString("v".to_string().into())
                )]
                .into_iter()
                .collect()
            ))
        );
        assert_eq!(
            document.meta.get("l"),
            Some(&MetaValue::MetaList(vec![
                MetaValue::MetaBool(true),
                MetaValue::MetaBool(false)
            ]))
        );
        assert_eq!(
            document.meta.get("b"),
            Some(&MetaValue::MetaBlocks(vec![Block::Plain(vec![
                str_inline("p")
            ])]))
        );
    }

    #[test]
    fn bare_block_list_is_wrapped_into_document() {
        let document = parse(r#"[Para [Str "a"], HorizontalRule]"#);
        assert_eq!(
            document.blocks,
            vec![Block::Para(vec![str_inline("a")]), Block::HorizontalRule]
        );
    }

    #[test]
    fn empty_list_is_an_empty_document() {
        assert_eq!(parse("[]").blocks, vec![]);
    }

    #[test]
    fn bare_inline_list_becomes_a_plain_block() {
        let document = parse(r#"[Str "a", Space, Str "b"]"#);
        assert_eq!(
            document.blocks,
            vec![Block::Plain(vec![
                str_inline("a"),
                Inline::Space,
                str_inline("b")
            ])]
        );
    }

    #[test]
    fn single_block_is_wrapped() {
        assert_eq!(only_block("HorizontalRule"), Block::HorizontalRule);
    }

    #[test]
    fn single_inline_becomes_a_plain_block() {
        assert_eq!(
            only_block(r#"Str "lonely""#),
            Block::Plain(vec![str_inline("lonely")])
        );
    }

    #[test]
    fn parses_code_block_with_attr() {
        assert_eq!(
            only_block(r#"CodeBlock ("i", ["rust", "numberLines"], [("k", "v")]) "let x = 1;""#),
            Block::CodeBlock(
                Box::new(Attr {
                    id: "i".to_string().into(),
                    classes: vec!["rust".to_string().into(), "numberLines".to_string().into()],
                    attributes: vec![("k".to_string().into(), "v".to_string().into())],
                }),
                "let x = 1;".to_string().into()
            )
        );
    }

    #[test]
    fn parses_raw_block_with_format_in_parens() {
        assert_eq!(
            only_block(r#"RawBlock (Format "html") "<hr>""#),
            Block::RawBlock(Format("html".to_string().into()), "<hr>".to_string().into())
        );
    }

    #[test]
    fn parses_line_block() {
        assert_eq!(
            only_block(r#"LineBlock [[Str "one"], [Str "two"]]"#),
            Block::LineBlock(vec![vec![str_inline("one")], vec![str_inline("two")]])
        );
    }

    #[test]
    fn parses_ordered_list_attributes() {
        assert_eq!(
            only_block(r#"OrderedList (3, UpperRoman, TwoParens) [[Plain [Str "x"]]]"#),
            Block::OrderedList(
                ListAttributes {
                    start: 3,
                    style: ListNumberStyle::UpperRoman,
                    delim: ListNumberDelim::TwoParens,
                },
                vec![vec![Block::Plain(vec![str_inline("x")])]]
            )
        );
    }

    #[test]
    fn parses_definition_list() {
        assert_eq!(
            only_block(r#"DefinitionList [([Str "term"], [[Plain [Str "def"]]])]"#),
            Block::DefinitionList(vec![(
                vec![str_inline("term")],
                vec![vec![Block::Plain(vec![str_inline("def")])]]
            )])
        );
    }

    #[test]
    fn parses_header_with_level_and_attr() {
        assert_eq!(
            only_block(r#"Header 2 ("h", [], []) [Str "Title"]"#),
            Block::Header(
                2,
                Box::new(Attr {
                    id: "h".to_string().into(),
                    classes: vec![],
                    attributes: vec![],
                }),
                vec![str_inline("Title")]
            )
        );
    }

    #[test]
    fn parses_div_and_blockquote() {
        assert_eq!(
            only_block(r#"Div ("d", [], []) [BlockQuote [Para [Str "q"]]]"#),
            Block::Div(
                Box::new(Attr {
                    id: "d".to_string().into(),
                    classes: vec![],
                    attributes: vec![],
                }),
                vec![Block::BlockQuote(vec![Block::Para(vec![str_inline("q")])])]
            )
        );
    }

    #[test]
    fn parses_figure_with_caption() {
        let block = only_block(
            r#"Figure ("f", [], []) (Caption Nothing [Plain [Str "cap"]]) [Para [Str "body"]]"#,
        );
        let Block::Figure(attr, caption, blocks) = block else {
            panic!("expected a figure");
        };
        assert_eq!(attr.id, "f");
        assert_eq!(caption.short, None);
        assert_eq!(caption.long, vec![Block::Plain(vec![str_inline("cap")])]);
        assert_eq!(blocks, vec![Block::Para(vec![str_inline("body")])]);
    }

    #[test]
    fn parses_caption_with_short_inlines() {
        let block =
            only_block(r#"Figure ("", [], []) (Caption (Just [Str "s"]) [Plain [Str "l"]]) []"#);
        let Block::Figure(_, caption, _) = block else {
            panic!("expected a figure");
        };
        assert_eq!(caption.short, Some(vec![str_inline("s")]));
    }

    #[test]
    fn parses_every_inline_constructor() {
        let block = only_block(
            r#"Para [Emph [Str "e"], Underline [Str "u"], Strong [Str "s"], Strikeout [Str "k"], Superscript [Str "p"], Subscript [Str "b"], SmallCaps [Str "c"], Space, SoftBreak, LineBreak]"#,
        );
        assert_eq!(
            block,
            Block::Para(vec![
                Inline::Emph(vec![str_inline("e")]),
                Inline::Underline(vec![str_inline("u")]),
                Inline::Strong(vec![str_inline("s")]),
                Inline::Strikeout(vec![str_inline("k")]),
                Inline::Superscript(vec![str_inline("p")]),
                Inline::Subscript(vec![str_inline("b")]),
                Inline::SmallCaps(vec![str_inline("c")]),
                Inline::Space,
                Inline::SoftBreak,
                Inline::LineBreak,
            ])
        );
    }

    #[test]
    fn parses_quoted_math_and_code_inlines() {
        let block = only_block(
            r#"Para [Quoted DoubleQuote [Str "q"], Math InlineMath "x^2", Code ("", [], []) "f()"]"#,
        );
        assert_eq!(
            block,
            Block::Para(vec![
                Inline::Quoted(QuoteType::DoubleQuote, vec![str_inline("q")]),
                Inline::Math(MathType::InlineMath, "x^2".to_string().into()),
                Inline::Code(Box::default(), "f()".to_string().into()),
            ])
        );
    }

    #[test]
    fn parses_link_image_span_and_note() {
        let block = only_block(
            r#"Para [Link ("", [], []) [Str "t"] ("/u", "ti"), Image ("", [], []) [Str "alt"] ("/i", ""), Span ("sp", [], []) [Str "s"], Note [Para [Str "n"]]]"#,
        );
        assert_eq!(
            block,
            Block::Para(vec![
                Inline::Link(
                    Box::default(),
                    vec![str_inline("t")],
                    Box::new(Target {
                        url: "/u".to_string().into(),
                        title: "ti".to_string().into()
                    })
                ),
                Inline::Image(
                    Box::default(),
                    vec![str_inline("alt")],
                    Box::new(Target {
                        url: "/i".to_string().into(),
                        title: carta_ast::Text::default()
                    })
                ),
                Inline::Span(
                    Box::new(Attr {
                        id: "sp".to_string().into(),
                        classes: vec![],
                        attributes: vec![],
                    }),
                    vec![str_inline("s")]
                ),
                Inline::Note(vec![Block::Para(vec![str_inline("n")])]),
            ])
        );
    }

    #[test]
    fn parses_raw_inline_with_bare_format() {
        let block = only_block(r#"Para [RawInline (Format "tex") "\\hi"]"#);
        assert_eq!(
            block,
            Block::Para(vec![Inline::RawInline(
                Format("tex".to_string().into()),
                "\\hi".to_string().into()
            )])
        );
    }

    #[test]
    fn parses_cite_with_all_fields() {
        let block = only_block(
            r#"Para [Cite [Citation {citationId = "x", citationPrefix = [Str "see"], citationSuffix = [Str "p1"], citationMode = AuthorInText, citationNoteNum = 2, citationHash = 0}] [Str "[@x]"]]"#,
        );
        let Block::Para(inlines) = block else {
            panic!("expected a paragraph");
        };
        let citation = match inlines.first() {
            Some(Inline::Cite(citations, _)) => citations.first().cloned(),
            _ => None,
        };
        let citation = citation.expect("a citation");
        assert_eq!(citation.id, "x");
        assert_eq!(citation.prefix, vec![str_inline("see")]);
        assert_eq!(citation.suffix, vec![str_inline("p1")]);
        assert_eq!(citation.mode, CitationMode::AuthorInText);
        assert_eq!(citation.note_num, 2);
    }

    #[test]
    fn parses_table_with_head_body_and_foot() {
        let input = r#"Table ("", [], []) (Caption Nothing [])
            [(AlignDefault, ColWidthDefault), (AlignRight, ColWidth 0.5)]
            (TableHead ("", [], []) [Row ("", [], []) [Cell ("", [], []) AlignDefault (RowSpan 1) (ColSpan 1) [Plain [Str "H"]]]])
            [TableBody ("", [], []) (RowHeadColumns 0) [] [Row ("", [], []) [Cell ("", [], []) AlignLeft (RowSpan 1) (ColSpan 1) [Plain [Str "B"]]]]]
            (TableFoot ("", [], []) [])"#;
        let block = only_block(input);
        let Block::Table(table) = block else {
            panic!("expected a table");
        };
        assert_eq!(table.col_specs.len(), 2);
        assert_eq!(
            table.col_specs.last().map(|spec| spec.width.clone()),
            Some(ColWidth::ColWidth(0.5))
        );
        assert_eq!(table.head.rows.len(), 1);
        assert_eq!(table.bodies.len(), 1);
        assert_eq!(table.foot.rows.len(), 0);
    }

    #[test]
    fn decodes_simple_string_escapes() {
        let block = only_block(r#"Para [Str "a\nb\tc\rd\\e\"f"]"#);
        assert_eq!(block, Block::Para(vec![str_inline("a\nb\tc\rd\\e\"f")]));
    }

    #[test]
    fn decodes_control_and_numeric_escapes() {
        // \f \v \a \b control bytes, an empty \& separator, decimal, hex, and octal escapes.
        let block = only_block(r#"Para [Str "\f\v\a\b\&\65\x41\o101"]"#);
        assert_eq!(
            block,
            Block::Para(vec![str_inline("\u{0C}\u{0B}\u{07}\u{08}AAA")])
        );
    }

    #[test]
    fn decodes_caret_and_mnemonic_control_escapes() {
        // \^A is control-A (U+0001); \ESC and \NUL are mnemonic control codes.
        let block = only_block(r#"Para [Str "\^A\ESC\NUL"]"#);
        assert_eq!(block, Block::Para(vec![str_inline("\u{01}\u{1B}\u{00}")]));
    }

    #[test]
    fn decodes_string_gap() {
        let block = only_block("Para [Str \"a\\   \\b\"]");
        assert_eq!(block, Block::Para(vec![str_inline("ab")]));
    }

    #[test]
    fn parses_negative_and_floating_numbers() {
        assert_eq!(
            only_block(r"OrderedList (-2, Decimal, Period) []"),
            Block::OrderedList(
                ListAttributes {
                    start: -2,
                    style: ListNumberStyle::Decimal,
                    delim: ListNumberDelim::Period,
                },
                vec![]
            )
        );
        let block = only_block(
            r#"Table ("", [], []) (Caption Nothing []) [(AlignDefault, ColWidth 1.5e-1)] (TableHead ("", [], []) []) [] (TableFoot ("", [], []) [])"#,
        );
        let Block::Table(table) = block else {
            panic!("expected a table");
        };
        assert_eq!(
            table.col_specs.first().map(|spec| spec.width.clone()),
            Some(ColWidth::ColWidth(0.15))
        );
    }

    #[test]
    fn rejects_unterminated_string() {
        assert!(parse_err(r#"Para [Str "oops]"#).contains("unterminated string"));
    }

    #[test]
    fn rejects_unexpected_character() {
        assert!(parse_err("Para [Str @]").contains("unexpected character"));
    }

    #[test]
    fn rejects_unknown_constructor() {
        assert!(parse_err("Bogus []").contains("not a recognized native document"));
    }

    #[test]
    fn rejects_unknown_block_in_list() {
        assert!(parse_err("Para [Wat]").contains("unknown inline"));
    }

    #[test]
    fn rejects_trailing_input() {
        assert!(parse_err("HorizontalRule HorizontalRule").contains("trailing input"));
    }

    #[test]
    fn rejects_unknown_escape() {
        assert!(parse_err(r#"Para [Str "\q"]"#).contains("unknown string escape"));
    }
}
