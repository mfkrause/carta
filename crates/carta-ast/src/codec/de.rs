//! Recursive-descent JSON reader for [`Document`]: parses interchange bytes straight into the model.
//!
//! Errors are reported as [`serde_json::Error`] values built through [`serde::de::Error::custom`],
//! so the public entry points keep their signatures. A fixed nesting limit bounds stack use on
//! adversarial input. Payload strings borrow the input when they carry no escapes, copying once
//! into their inline storage; only escaped strings take the decoding path.

use std::collections::BTreeMap;
use std::fmt::Display;

use serde::de::Error as _;

use crate::ast::{
    Alignment, ApiVersion, Attr, Block, Caption, Cell, Citation, CitationMode, ColSpec, ColWidth,
    Document, Format, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType,
    MetaValue, QuoteType, Row, Table, TableBody, TableFoot, TableHead, Target, Text,
};

type Parsed<T> = Result<T, serde_json::Error>;

/// The nesting limit: exceeding it errors rather than risking a stack overflow. Each array and
/// object counts as one level, so this bounds recursion the same way for every container shape.
const MAX_DEPTH: usize = 128;

/// Whether a tagged node's content value is positioned for parsing or was omitted entirely.
#[derive(Clone, Copy)]
enum Content {
    Present,
    Absent,
}

pub(super) fn from_json_bytes(bytes: &[u8]) -> Parsed<Document> {
    // One validation pass up front lets every later string slice come out of `text` without
    // re-checking UTF-8; slice boundaries always fall on ASCII delimiter bytes.
    let text = std::str::from_utf8(bytes)
        .map_err(|error| serde_json::Error::custom(format_args!("invalid UTF-8: {error}")))?;
    let mut reader = Reader {
        input: bytes,
        text,
        pos: 0,
        depth: 0,
    };
    let document = reader.parse_document()?;
    reader.skip_whitespace();
    if reader.pos != reader.input.len() {
        return reader.fail("trailing characters");
    }
    Ok(document)
}

struct Reader<'a> {
    input: &'a [u8],
    text: &'a str,
    pos: usize,
    depth: usize,
}

impl Reader<'_> {
    fn make_error(&self, message: impl Display) -> serde_json::Error {
        serde_json::Error::custom(format_args!("{message} at byte {}", self.pos))
    }

    fn fail<T>(&self, message: impl Display) -> Parsed<T> {
        Err(self.make_error(message))
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek();
        if byte.is_some() {
            self.pos += 1;
        }
        byte
    }

    fn skip_whitespace(&mut self) {
        while let Some(byte) = self.peek() {
            if matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, byte: u8, what: &str) -> Parsed<()> {
        self.skip_whitespace();
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            self.fail(format_args!("expected {what}"))
        }
    }

    fn try_literal(&mut self, literal: &[u8]) -> bool {
        let end = self.pos + literal.len();
        if self.input.get(self.pos..end) == Some(literal) {
            self.pos = end;
            true
        } else {
            false
        }
    }

    fn enter(&mut self) -> Parsed<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return self.fail("recursion limit exceeded");
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn open_array(&mut self) -> Parsed<()> {
        self.enter()?;
        self.expect(b'[', "'['")
    }

    fn close_array(&mut self) -> Parsed<()> {
        self.expect(b']', "']'")?;
        self.leave();
        Ok(())
    }

    fn comma(&mut self) -> Parsed<()> {
        self.expect(b',', "','")
    }

    /// Parses a JSON array whose elements are each read by `parse_element`.
    fn parse_array<T>(
        &mut self,
        mut parse_element: impl FnMut(&mut Self) -> Parsed<T>,
    ) -> Parsed<Vec<T>> {
        self.open_array()?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.leave();
            return Ok(items);
        }
        loop {
            items.push(parse_element(self)?);
            self.skip_whitespace();
            match self.bump() {
                Some(b',') => {}
                Some(b']') => break,
                _ => return self.fail("expected ',' or ']'"),
            }
        }
        self.leave();
        Ok(items)
    }

    fn parse_inlines(&mut self) -> Parsed<Vec<Inline>> {
        self.parse_array(Self::parse_inline)
    }

    fn parse_blocks(&mut self) -> Parsed<Vec<Block>> {
        self.parse_array(Self::parse_block)
    }

    /// Reads a JSON object, invoking `on_member` with each key after its colon. Handles the empty
    /// object, comma separators, and the closing brace, and counts one nesting level.
    fn parse_object(
        &mut self,
        mut on_member: impl FnMut(&mut Self, Text) -> Parsed<()>,
    ) -> Parsed<()> {
        self.enter()?;
        self.expect(b'{', "'{'")?;
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.leave();
            return Ok(());
        }
        loop {
            let key = self.parse_text()?;
            self.expect(b':', "':'")?;
            on_member(self, key)?;
            self.skip_whitespace();
            match self.bump() {
                Some(b',') => {}
                Some(b'}') => break,
                _ => return self.fail("expected ',' or '}'"),
            }
        }
        self.leave();
        Ok(())
    }

    fn parse_document(&mut self) -> Parsed<Document> {
        let mut api_version = None;
        let mut meta = None;
        let mut blocks = None;
        self.parse_object(|reader, key| {
            if key.as_str() == crate::API_VERSION_KEY {
                if api_version.is_some() {
                    return reader.fail("duplicate field `pandoc-api-version`");
                }
                api_version = Some(reader.parse_array(Self::parse_u32)?);
            } else if key.as_str() == "meta" {
                if meta.is_some() {
                    return reader.fail("duplicate field `meta`");
                }
                meta = Some(reader.parse_meta_map()?);
            } else if key.as_str() == "blocks" {
                if blocks.is_some() {
                    return reader.fail("duplicate field `blocks`");
                }
                blocks = Some(reader.parse_blocks()?);
            } else {
                return reader.fail(format_args!("unknown field `{key}`"));
            }
            Ok(())
        })?;

        Ok(Document {
            api_version: ApiVersion(
                api_version.ok_or_else(|| self.make_error("missing field `pandoc-api-version`"))?,
            ),
            meta: meta.unwrap_or_default(),
            blocks: blocks.ok_or_else(|| self.make_error("missing field `blocks`"))?,
        })
    }

    fn parse_meta_map(&mut self) -> Parsed<BTreeMap<Text, MetaValue>> {
        let mut map = BTreeMap::new();
        self.parse_object(|reader, key| {
            let value = reader.parse_meta()?;
            map.insert(key, value);
            Ok(())
        })?;
        Ok(map)
    }

    /// Reads a `{"t":…,"c":…}` tagged object. The tag and content may appear in either order, and
    /// any other member is skipped. `dispatch` builds the value from the resolved tag, reading the
    /// content in place when it follows the tag, or from its buffered span otherwise.
    fn parse_tagged<T>(
        &mut self,
        dispatch: impl Fn(&mut Self, &str, Content) -> Parsed<T>,
    ) -> Parsed<T> {
        let mut tag: Option<Text> = None;
        let mut value: Option<T> = None;
        let mut content_span: Option<(usize, usize)> = None;
        let mut seen_content = false;

        self.parse_object(|reader, key| {
            if key.as_str() == "t" {
                if tag.is_some() {
                    return reader.fail("duplicate field `t`");
                }
                tag = Some(reader.parse_text()?);
            } else if key.as_str() == "c" {
                if seen_content {
                    return reader.fail("duplicate field `c`");
                }
                seen_content = true;
                if let Some(name) = &tag {
                    value = Some(dispatch(reader, name.as_str(), Content::Present)?);
                } else {
                    reader.skip_whitespace();
                    let start = reader.pos;
                    reader.skip_value()?;
                    content_span = Some((start, reader.pos));
                }
            } else {
                reader.skip_value()?;
            }
            Ok(())
        })?;

        let tag = tag.ok_or_else(|| self.make_error("missing field `t`"))?;
        if let Some(value) = value {
            return Ok(value);
        }
        match content_span {
            Some((start, end)) => {
                let mut inner = Reader {
                    input: self.input,
                    text: self.text,
                    pos: start,
                    depth: self.depth,
                };
                let value = dispatch(&mut inner, tag.as_str(), Content::Present)?;
                inner.skip_whitespace();
                if inner.pos != end {
                    return self.fail("trailing content in tagged value");
                }
                Ok(value)
            }
            None => dispatch(self, tag.as_str(), Content::Absent),
        }
    }

    /// Reads a `{"t":…}` object for an enum with no content payload, returning the tag; other
    /// members are ignored.
    fn parse_tag_only(&mut self) -> Parsed<Text> {
        let mut tag: Option<Text> = None;
        self.parse_object(|reader, key| {
            if key.as_str() == "t" {
                if tag.is_some() {
                    return reader.fail("duplicate field `t`");
                }
                tag = Some(reader.parse_text()?);
            } else {
                reader.skip_value()?;
            }
            Ok(())
        })?;
        tag.ok_or_else(|| self.make_error("missing field `t`"))
    }

    fn require_present(&self, content: Content) -> Parsed<()> {
        match content {
            Content::Present => Ok(()),
            Content::Absent => self.fail("missing field `c`"),
        }
    }

    fn unit_content(&mut self, content: Content) -> Parsed<()> {
        match content {
            Content::Absent => Ok(()),
            Content::Present => {
                self.skip_whitespace();
                if self.try_literal(b"null") {
                    Ok(())
                } else {
                    self.fail("expected a content-free tagged value")
                }
            }
        }
    }

    fn parse_meta(&mut self) -> Parsed<MetaValue> {
        self.parse_tagged(|reader, tag, content| match tag {
            "MetaMap" => {
                reader.require_present(content)?;
                Ok(MetaValue::MetaMap(reader.parse_meta_map()?))
            }
            "MetaList" => {
                reader.require_present(content)?;
                Ok(MetaValue::MetaList(reader.parse_array(Self::parse_meta)?))
            }
            "MetaBool" => {
                reader.require_present(content)?;
                Ok(MetaValue::MetaBool(reader.parse_bool()?))
            }
            "MetaString" => {
                reader.require_present(content)?;
                Ok(MetaValue::MetaString(reader.parse_text()?))
            }
            "MetaInlines" => {
                reader.require_present(content)?;
                Ok(MetaValue::MetaInlines(reader.parse_inlines()?))
            }
            "MetaBlocks" => {
                reader.require_present(content)?;
                Ok(MetaValue::MetaBlocks(reader.parse_blocks()?))
            }
            other => reader.fail(format_args!("unknown variant `{other}`")),
        })
    }

    fn parse_block(&mut self) -> Parsed<Block> {
        self.parse_tagged(|reader, tag, content| match tag {
            "Plain" => {
                reader.require_present(content)?;
                Ok(Block::Plain(reader.parse_inlines()?))
            }
            "Para" => {
                reader.require_present(content)?;
                Ok(Block::Para(reader.parse_inlines()?))
            }
            "LineBlock" => {
                reader.require_present(content)?;
                Ok(Block::LineBlock(reader.parse_array(Self::parse_inlines)?))
            }
            "CodeBlock" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let attr = reader.parse_attr()?;
                reader.comma()?;
                let text = reader.parse_text()?;
                reader.close_array()?;
                Ok(Block::CodeBlock(Box::new(attr), text))
            }
            "RawBlock" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let format = reader.parse_text()?;
                reader.comma()?;
                let text = reader.parse_text()?;
                reader.close_array()?;
                Ok(Block::RawBlock(Format(format), text))
            }
            "BlockQuote" => {
                reader.require_present(content)?;
                Ok(Block::BlockQuote(reader.parse_blocks()?))
            }
            "OrderedList" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let attributes = reader.parse_list_attributes()?;
                reader.comma()?;
                let items = reader.parse_array(Self::parse_blocks)?;
                reader.close_array()?;
                Ok(Block::OrderedList(attributes, items))
            }
            "BulletList" => {
                reader.require_present(content)?;
                Ok(Block::BulletList(reader.parse_array(Self::parse_blocks)?))
            }
            "DefinitionList" => {
                reader.require_present(content)?;
                Ok(Block::DefinitionList(
                    reader.parse_array(Self::parse_definition_entry)?,
                ))
            }
            "Header" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let level = reader.parse_i32()?;
                reader.comma()?;
                let attr = reader.parse_attr()?;
                reader.comma()?;
                let inlines = reader.parse_inlines()?;
                reader.close_array()?;
                Ok(Block::Header(level, Box::new(attr), inlines))
            }
            "HorizontalRule" => {
                reader.unit_content(content)?;
                Ok(Block::HorizontalRule)
            }
            "Table" => {
                reader.require_present(content)?;
                Ok(Block::Table(Box::new(reader.parse_table()?)))
            }
            "Figure" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let attr = reader.parse_attr()?;
                reader.comma()?;
                let caption = reader.parse_caption()?;
                reader.comma()?;
                let blocks = reader.parse_blocks()?;
                reader.close_array()?;
                Ok(Block::Figure(Box::new(attr), Box::new(caption), blocks))
            }
            "Div" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let attr = reader.parse_attr()?;
                reader.comma()?;
                let blocks = reader.parse_blocks()?;
                reader.close_array()?;
                Ok(Block::Div(Box::new(attr), blocks))
            }
            other => reader.fail(format_args!("unknown variant `{other}`")),
        })
    }

    fn parse_definition_entry(&mut self) -> Parsed<(Vec<Inline>, Vec<Vec<Block>>)> {
        self.open_array()?;
        let term = self.parse_inlines()?;
        self.comma()?;
        let definitions = self.parse_array(Self::parse_blocks)?;
        self.close_array()?;
        Ok((term, definitions))
    }

    #[allow(clippy::too_many_lines)]
    fn parse_inline(&mut self) -> Parsed<Inline> {
        self.parse_tagged(|reader, tag, content| match tag {
            "Str" => {
                reader.require_present(content)?;
                Ok(Inline::Str(reader.parse_text()?))
            }
            "Emph" => reader.wrapped_inlines(content, Inline::Emph),
            "Underline" => reader.wrapped_inlines(content, Inline::Underline),
            "Strong" => reader.wrapped_inlines(content, Inline::Strong),
            "Strikeout" => reader.wrapped_inlines(content, Inline::Strikeout),
            "Superscript" => reader.wrapped_inlines(content, Inline::Superscript),
            "Subscript" => reader.wrapped_inlines(content, Inline::Subscript),
            "SmallCaps" => reader.wrapped_inlines(content, Inline::SmallCaps),
            "Quoted" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let quote = reader.parse_quote_type()?;
                reader.comma()?;
                let inlines = reader.parse_inlines()?;
                reader.close_array()?;
                Ok(Inline::Quoted(quote, inlines))
            }
            "Cite" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let citations = reader.parse_array(Self::parse_citation)?;
                reader.comma()?;
                let inlines = reader.parse_inlines()?;
                reader.close_array()?;
                Ok(Inline::Cite(citations, inlines))
            }
            "Code" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let attr = reader.parse_attr()?;
                reader.comma()?;
                let text = reader.parse_text()?;
                reader.close_array()?;
                Ok(Inline::Code(Box::new(attr), text))
            }
            "Space" => {
                reader.unit_content(content)?;
                Ok(Inline::Space)
            }
            "SoftBreak" => {
                reader.unit_content(content)?;
                Ok(Inline::SoftBreak)
            }
            "LineBreak" => {
                reader.unit_content(content)?;
                Ok(Inline::LineBreak)
            }
            "Math" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let math = reader.parse_math_type()?;
                reader.comma()?;
                let text = reader.parse_text()?;
                reader.close_array()?;
                Ok(Inline::Math(math, text))
            }
            "RawInline" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let format = reader.parse_text()?;
                reader.comma()?;
                let text = reader.parse_text()?;
                reader.close_array()?;
                Ok(Inline::RawInline(Format(format), text))
            }
            "Link" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let attr = reader.parse_attr()?;
                reader.comma()?;
                let inlines = reader.parse_inlines()?;
                reader.comma()?;
                let target = reader.parse_target()?;
                reader.close_array()?;
                Ok(Inline::Link(Box::new(attr), inlines, Box::new(target)))
            }
            "Image" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let attr = reader.parse_attr()?;
                reader.comma()?;
                let inlines = reader.parse_inlines()?;
                reader.comma()?;
                let target = reader.parse_target()?;
                reader.close_array()?;
                Ok(Inline::Image(Box::new(attr), inlines, Box::new(target)))
            }
            "Note" => {
                reader.require_present(content)?;
                Ok(Inline::Note(reader.parse_blocks()?))
            }
            "Span" => {
                reader.require_present(content)?;
                reader.open_array()?;
                let attr = reader.parse_attr()?;
                reader.comma()?;
                let inlines = reader.parse_inlines()?;
                reader.close_array()?;
                Ok(Inline::Span(Box::new(attr), inlines))
            }
            other => reader.fail(format_args!("unknown variant `{other}`")),
        })
    }

    fn wrapped_inlines(
        &mut self,
        content: Content,
        wrap: impl Fn(Vec<Inline>) -> Inline,
    ) -> Parsed<Inline> {
        self.require_present(content)?;
        Ok(wrap(self.parse_inlines()?))
    }

    fn parse_citation(&mut self) -> Parsed<Citation> {
        let mut id = None;
        let mut prefix = None;
        let mut suffix = None;
        let mut mode = None;
        let mut note_num = None;
        let mut hash = None;

        self.parse_object(|reader, key| {
            if key.as_str() == "citationId" {
                let value = reader.parse_text()?;
                Self::store(&mut id, value, reader, "citationId")
            } else if key.as_str() == "citationPrefix" {
                let value = reader.parse_inlines()?;
                Self::store(&mut prefix, value, reader, "citationPrefix")
            } else if key.as_str() == "citationSuffix" {
                let value = reader.parse_inlines()?;
                Self::store(&mut suffix, value, reader, "citationSuffix")
            } else if key.as_str() == "citationMode" {
                let value = reader.parse_citation_mode()?;
                Self::store(&mut mode, value, reader, "citationMode")
            } else if key.as_str() == "citationNoteNum" {
                let value = reader.parse_i32()?;
                Self::store(&mut note_num, value, reader, "citationNoteNum")
            } else if key.as_str() == "citationHash" {
                let value = reader.parse_i32()?;
                Self::store(&mut hash, value, reader, "citationHash")
            } else {
                reader.fail(format_args!("unknown field `{key}`"))
            }
        })?;

        Ok(Citation {
            id: id.ok_or_else(|| self.make_error("missing field `citationId`"))?,
            prefix: prefix.ok_or_else(|| self.make_error("missing field `citationPrefix`"))?,
            suffix: suffix.ok_or_else(|| self.make_error("missing field `citationSuffix`"))?,
            mode: mode.ok_or_else(|| self.make_error("missing field `citationMode`"))?,
            note_num: note_num.ok_or_else(|| self.make_error("missing field `citationNoteNum`"))?,
            hash: hash.ok_or_else(|| self.make_error("missing field `citationHash`"))?,
        })
    }

    fn store<T>(slot: &mut Option<T>, value: T, reader: &Reader, field: &str) -> Parsed<()> {
        if slot.is_some() {
            return reader.fail(format_args!("duplicate field `{field}`"));
        }
        *slot = Some(value);
        Ok(())
    }

    fn parse_table(&mut self) -> Parsed<Table> {
        self.open_array()?;
        let attr = self.parse_attr()?;
        self.comma()?;
        let caption = self.parse_caption()?;
        self.comma()?;
        let col_specs = self.parse_array(Self::parse_col_spec)?;
        self.comma()?;
        let head = self.parse_table_head()?;
        self.comma()?;
        let bodies = self.parse_array(Self::parse_table_body)?;
        self.comma()?;
        let foot = self.parse_table_foot()?;
        self.close_array()?;
        Ok(Table {
            attr,
            caption,
            col_specs,
            head,
            bodies,
            foot,
        })
    }

    fn parse_table_head(&mut self) -> Parsed<TableHead> {
        self.open_array()?;
        let attr = self.parse_attr()?;
        self.comma()?;
        let rows = self.parse_array(Self::parse_row)?;
        self.close_array()?;
        Ok(TableHead { attr, rows })
    }

    fn parse_table_body(&mut self) -> Parsed<TableBody> {
        self.open_array()?;
        let attr = self.parse_attr()?;
        self.comma()?;
        let row_head_columns = self.parse_i32()?;
        self.comma()?;
        let head = self.parse_array(Self::parse_row)?;
        self.comma()?;
        let body = self.parse_array(Self::parse_row)?;
        self.close_array()?;
        Ok(TableBody {
            attr,
            row_head_columns,
            head,
            body,
        })
    }

    fn parse_table_foot(&mut self) -> Parsed<TableFoot> {
        self.open_array()?;
        let attr = self.parse_attr()?;
        self.comma()?;
        let rows = self.parse_array(Self::parse_row)?;
        self.close_array()?;
        Ok(TableFoot { attr, rows })
    }

    fn parse_row(&mut self) -> Parsed<Row> {
        self.open_array()?;
        let attr = self.parse_attr()?;
        self.comma()?;
        let cells = self.parse_array(Self::parse_cell)?;
        self.close_array()?;
        Ok(Row { attr, cells })
    }

    fn parse_cell(&mut self) -> Parsed<Cell> {
        self.open_array()?;
        let attr = self.parse_attr()?;
        self.comma()?;
        let align = self.parse_alignment()?;
        self.comma()?;
        let row_span = self.parse_i32()?;
        self.comma()?;
        let col_span = self.parse_i32()?;
        self.comma()?;
        let content = self.parse_blocks()?;
        self.close_array()?;
        Ok(Cell {
            attr,
            align,
            row_span,
            col_span,
            content,
        })
    }

    fn parse_col_spec(&mut self) -> Parsed<ColSpec> {
        self.open_array()?;
        let align = self.parse_alignment()?;
        self.comma()?;
        let width = self.parse_col_width()?;
        self.close_array()?;
        Ok(ColSpec { align, width })
    }

    fn parse_col_width(&mut self) -> Parsed<ColWidth> {
        self.parse_tagged(|reader, tag, content| match tag {
            "ColWidth" => {
                reader.require_present(content)?;
                Ok(ColWidth::ColWidth(reader.parse_f64()?))
            }
            "ColWidthDefault" => {
                reader.unit_content(content)?;
                Ok(ColWidth::ColWidthDefault)
            }
            other => reader.fail(format_args!("unknown variant `{other}`")),
        })
    }

    fn parse_caption(&mut self) -> Parsed<Caption> {
        self.open_array()?;
        self.skip_whitespace();
        let short = if self.try_literal(b"null") {
            None
        } else {
            Some(self.parse_inlines()?)
        };
        self.comma()?;
        let long = self.parse_blocks()?;
        self.close_array()?;
        Ok(Caption { short, long })
    }

    fn parse_attr(&mut self) -> Parsed<Attr> {
        self.open_array()?;
        let id = self.parse_text()?;
        self.comma()?;
        let classes = self.parse_array(Self::parse_text)?;
        self.comma()?;
        let attributes = self.parse_array(Self::parse_key_value)?;
        self.close_array()?;
        Ok(Attr {
            id,
            classes,
            attributes,
        })
    }

    fn parse_key_value(&mut self) -> Parsed<(Text, Text)> {
        self.open_array()?;
        let key = self.parse_text()?;
        self.comma()?;
        let value = self.parse_text()?;
        self.close_array()?;
        Ok((key, value))
    }

    fn parse_target(&mut self) -> Parsed<Target> {
        self.open_array()?;
        let url = self.parse_text()?;
        self.comma()?;
        let title = self.parse_text()?;
        self.close_array()?;
        Ok(Target { url, title })
    }

    fn parse_list_attributes(&mut self) -> Parsed<ListAttributes> {
        self.open_array()?;
        let start = self.parse_i32()?;
        self.comma()?;
        let style = self.parse_list_number_style()?;
        self.comma()?;
        let delim = self.parse_list_number_delim()?;
        self.close_array()?;
        Ok(ListAttributes {
            start,
            style,
            delim,
        })
    }

    fn parse_quote_type(&mut self) -> Parsed<QuoteType> {
        let tag = self.parse_tag_only()?;
        match tag.as_str() {
            "SingleQuote" => Ok(QuoteType::SingleQuote),
            "DoubleQuote" => Ok(QuoteType::DoubleQuote),
            other => self.fail(format_args!("unknown variant `{other}`")),
        }
    }

    fn parse_math_type(&mut self) -> Parsed<MathType> {
        let tag = self.parse_tag_only()?;
        match tag.as_str() {
            "InlineMath" => Ok(MathType::InlineMath),
            "DisplayMath" => Ok(MathType::DisplayMath),
            other => self.fail(format_args!("unknown variant `{other}`")),
        }
    }

    fn parse_alignment(&mut self) -> Parsed<Alignment> {
        let tag = self.parse_tag_only()?;
        match tag.as_str() {
            "AlignLeft" => Ok(Alignment::AlignLeft),
            "AlignRight" => Ok(Alignment::AlignRight),
            "AlignCenter" => Ok(Alignment::AlignCenter),
            "AlignDefault" => Ok(Alignment::AlignDefault),
            other => self.fail(format_args!("unknown variant `{other}`")),
        }
    }

    fn parse_list_number_style(&mut self) -> Parsed<ListNumberStyle> {
        let tag = self.parse_tag_only()?;
        match tag.as_str() {
            "DefaultStyle" => Ok(ListNumberStyle::DefaultStyle),
            "Example" => Ok(ListNumberStyle::Example),
            "Decimal" => Ok(ListNumberStyle::Decimal),
            "LowerRoman" => Ok(ListNumberStyle::LowerRoman),
            "UpperRoman" => Ok(ListNumberStyle::UpperRoman),
            "LowerAlpha" => Ok(ListNumberStyle::LowerAlpha),
            "UpperAlpha" => Ok(ListNumberStyle::UpperAlpha),
            other => self.fail(format_args!("unknown variant `{other}`")),
        }
    }

    fn parse_list_number_delim(&mut self) -> Parsed<ListNumberDelim> {
        let tag = self.parse_tag_only()?;
        match tag.as_str() {
            "DefaultDelim" => Ok(ListNumberDelim::DefaultDelim),
            "Period" => Ok(ListNumberDelim::Period),
            "OneParen" => Ok(ListNumberDelim::OneParen),
            "TwoParens" => Ok(ListNumberDelim::TwoParens),
            other => self.fail(format_args!("unknown variant `{other}`")),
        }
    }

    fn parse_citation_mode(&mut self) -> Parsed<CitationMode> {
        let tag = self.parse_tag_only()?;
        match tag.as_str() {
            "AuthorInText" => Ok(CitationMode::AuthorInText),
            "SuppressAuthor" => Ok(CitationMode::SuppressAuthor),
            "NormalCitation" => Ok(CitationMode::NormalCitation),
            other => self.fail(format_args!("unknown variant `{other}`")),
        }
    }

    fn parse_bool(&mut self) -> Parsed<bool> {
        self.skip_whitespace();
        if self.try_literal(b"true") {
            Ok(true)
        } else if self.try_literal(b"false") {
            Ok(false)
        } else {
            self.fail("expected a boolean")
        }
    }

    fn parse_u32(&mut self) -> Parsed<u32> {
        self.skip_whitespace();
        let (number, is_float) = self.scan_number()?;
        if is_float {
            return self.fail("expected an integer");
        }
        number
            .parse::<u32>()
            .map_err(|_| self.make_error("integer out of range for u32"))
    }

    fn parse_i32(&mut self) -> Parsed<i32> {
        self.skip_whitespace();
        let (number, is_float) = self.scan_number()?;
        if is_float {
            return self.fail("expected an integer");
        }
        number
            .parse::<i32>()
            .map_err(|_| self.make_error("integer out of range for i32"))
    }

    fn parse_f64(&mut self) -> Parsed<f64> {
        self.skip_whitespace();
        let (number, _is_float) = self.scan_number()?;
        number
            .parse::<f64>()
            .map_err(|_| self.make_error("invalid number"))
    }

    /// Consumes a JSON number token, returning its text and whether it carried a fraction or
    /// exponent. Enforces the JSON grammar: no leading `+`, no redundant leading zero.
    fn scan_number(&mut self) -> Parsed<(&str, bool)> {
        let start = self.pos;
        let mut is_float = false;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return self.fail("invalid number");
                }
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                self.consume_digits();
            }
            _ => return self.fail("expected value"),
        }
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return self.fail("invalid number");
            }
            self.consume_digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return self.fail("invalid number");
            }
            self.consume_digits();
        }
        let text = self.text.get(start..self.pos).unwrap_or_default();
        Ok((text, is_float))
    }

    fn consume_digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
    }

    /// Parses a JSON string into [`Text`]. The escape-free run is borrowed and copied once; escapes
    /// switch to a decoding buffer. Raw control bytes are rejected.
    fn parse_text(&mut self) -> Parsed<Text> {
        self.expect(b'"', "'\"'")?;
        let start = self.pos;
        // Most payload strings are a handful of clean bytes, so a scalar scan beats a vectorized
        // search's per-call setup here.
        loop {
            let byte = *self
                .input
                .get(self.pos)
                .ok_or_else(|| self.make_error("unterminated string"))?;
            if byte == b'"' {
                let text = self.text.get(start..self.pos).unwrap_or_default();
                let value = Text::from(text);
                self.pos += 1;
                return Ok(value);
            }
            if byte == b'\\' || byte < 0x20 {
                return self.parse_escaped_text(start);
            }
            self.pos += 1;
        }
    }

    fn parse_escaped_text(&mut self, start: usize) -> Parsed<Text> {
        let mut out = String::new();
        out.push_str(self.text.get(start..self.pos).unwrap_or_default());
        loop {
            let byte = *self
                .input
                .get(self.pos)
                .ok_or_else(|| self.make_error("unterminated string"))?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(Text::from(out));
                }
                b'\\' => {
                    self.pos += 1;
                    self.parse_escape(&mut out)?;
                }
                _ if byte < 0x20 => {
                    return self.fail("control character in string");
                }
                _ => {
                    let run_start = self.pos;
                    self.pos += 1;
                    while let Some(&next) = self.input.get(self.pos) {
                        if next == b'"' || next == b'\\' || next < 0x20 {
                            break;
                        }
                        self.pos += 1;
                    }
                    out.push_str(self.text.get(run_start..self.pos).unwrap_or_default());
                }
            }
        }
    }

    fn parse_escape(&mut self, out: &mut String) -> Parsed<()> {
        let byte = *self
            .input
            .get(self.pos)
            .ok_or_else(|| self.make_error("unterminated escape"))?;
        self.pos += 1;
        match byte {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{08}'),
            b'f' => out.push('\u{0C}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let ch = self.parse_unicode_escape()?;
                out.push(ch);
            }
            _ => return self.fail("invalid escape"),
        }
        Ok(())
    }

    fn parse_unicode_escape(&mut self) -> Parsed<char> {
        let first = self.parse_hex4()?;
        if (0xD800..=0xDBFF).contains(&first) {
            if self.input.get(self.pos) == Some(&b'\\')
                && self.input.get(self.pos + 1) == Some(&b'u')
            {
                self.pos += 2;
                let second = self.parse_hex4()?;
                if (0xDC00..=0xDFFF).contains(&second) {
                    let combined = 0x1_0000
                        + ((u32::from(first) - 0xD800) << 10)
                        + (u32::from(second) - 0xDC00);
                    char::from_u32(combined).ok_or_else(|| self.make_error("invalid code point"))
                } else {
                    self.fail("unpaired surrogate in escape")
                }
            } else {
                self.fail("unpaired surrogate in escape")
            }
        } else if (0xDC00..=0xDFFF).contains(&first) {
            self.fail("unpaired surrogate in escape")
        } else {
            char::from_u32(u32::from(first)).ok_or_else(|| self.make_error("invalid code point"))
        }
    }

    fn parse_hex4(&mut self) -> Parsed<u16> {
        let mut value = 0u16;
        for _ in 0..4 {
            let byte = *self
                .input
                .get(self.pos)
                .ok_or_else(|| self.make_error("unterminated unicode escape"))?;
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return self.fail("invalid unicode escape"),
            };
            value = value * 16 + u16::from(digit);
            self.pos += 1;
        }
        Ok(value)
    }

    /// Consumes and discards one JSON value of any shape, used for members that carry no meaning
    /// (unknown keys, and content buffered before its tag was seen).
    fn skip_value(&mut self) -> Parsed<()> {
        self.skip_whitespace();
        let byte = self
            .peek()
            .ok_or_else(|| self.make_error("expected value"))?;
        match byte {
            b'"' => {
                self.parse_text()?;
                Ok(())
            }
            b'-' | b'0'..=b'9' => {
                self.scan_number()?;
                Ok(())
            }
            b't' => {
                if self.try_literal(b"true") {
                    Ok(())
                } else {
                    self.fail("invalid literal")
                }
            }
            b'f' => {
                if self.try_literal(b"false") {
                    Ok(())
                } else {
                    self.fail("invalid literal")
                }
            }
            b'n' => {
                if self.try_literal(b"null") {
                    Ok(())
                } else {
                    self.fail("invalid literal")
                }
            }
            b'[' => {
                self.open_array()?;
                self.skip_whitespace();
                if self.peek() == Some(b']') {
                    self.pos += 1;
                    self.leave();
                    return Ok(());
                }
                loop {
                    self.skip_value()?;
                    self.skip_whitespace();
                    match self.bump() {
                        Some(b',') => {}
                        Some(b']') => break,
                        _ => return self.fail("expected ',' or ']'"),
                    }
                }
                self.leave();
                Ok(())
            }
            b'{' => {
                self.enter()?;
                self.pos += 1;
                self.skip_whitespace();
                if self.peek() == Some(b'}') {
                    self.pos += 1;
                    self.leave();
                    return Ok(());
                }
                loop {
                    self.parse_text()?;
                    self.expect(b':', "':'")?;
                    self.skip_value()?;
                    self.skip_whitespace();
                    match self.bump() {
                        Some(b',') => {}
                        Some(b'}') => break,
                        _ => return self.fail("expected ',' or '}'"),
                    }
                }
                self.leave();
                Ok(())
            }
            _ => self.fail("expected value"),
        }
    }
}
