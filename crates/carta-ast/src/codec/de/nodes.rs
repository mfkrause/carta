//! Decoders for documents, metadata, blocks, and inline nodes.

use std::collections::BTreeMap;

use crate::ast::{ApiVersion, Block, Citation, Document, Format, Inline, MetaValue, Text};

use super::{Content, Parsed, Reader};

impl Reader<'_> {
    pub(super) fn parse_document(&mut self) -> Parsed<Document> {
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

    pub(super) fn parse_block(&mut self) -> Parsed<Block> {
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
    pub(super) fn parse_inline(&mut self) -> Parsed<Inline> {
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
}
