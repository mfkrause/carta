//! Native reader: parses the document model's printed textual form back into the model.
//!
//! The native format is the human-readable rendering of the AST: constructor names applied to
//! their arguments, with strings, tuples, lists, and records written in a small constructor-
//! application value syntax (`Para [ Str "x" ]`, `("id", ["class"], [("k","v")])`). Parsing has two
//! stages: a
//! lexer (`tokenize`) splits the source into `Token`s, and a recursive-descent `Parser`
//! consumes them type-directedly: each AST shape has a dedicated method, so the same `(…, …, …)`
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

mod lexer;

use lexer::tokenize;

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
mod tests;
