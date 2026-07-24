//! Decoders for tables, attributes, targets, and enumerated leaf values.

use crate::ast::{
    Alignment, Attr, Caption, Cell, CitationMode, ColSpec, ColWidth, ListAttributes,
    ListNumberDelim, ListNumberStyle, MathType, QuoteType, Row, Table, TableBody, TableFoot,
    TableHead, Target, Text,
};

use super::{Parsed, Reader};

impl Reader<'_> {
    pub(super) fn parse_table(&mut self) -> Parsed<Table> {
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

    pub(super) fn parse_caption(&mut self) -> Parsed<Caption> {
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

    pub(super) fn parse_attr(&mut self) -> Parsed<Attr> {
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

    pub(super) fn parse_target(&mut self) -> Parsed<Target> {
        self.open_array()?;
        let url = self.parse_text()?;
        self.comma()?;
        let title = self.parse_text()?;
        self.close_array()?;
        Ok(Target { url, title })
    }

    pub(super) fn parse_list_attributes(&mut self) -> Parsed<ListAttributes> {
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

    pub(super) fn parse_quote_type(&mut self) -> Parsed<QuoteType> {
        let tag = self.parse_tag_only()?;
        match tag.as_str() {
            "SingleQuote" => Ok(QuoteType::SingleQuote),
            "DoubleQuote" => Ok(QuoteType::DoubleQuote),
            other => self.fail(format_args!("unknown variant `{other}`")),
        }
    }

    pub(super) fn parse_math_type(&mut self) -> Parsed<MathType> {
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

    pub(super) fn parse_citation_mode(&mut self) -> Parsed<CitationMode> {
        let tag = self.parse_tag_only()?;
        match tag.as_str() {
            "AuthorInText" => Ok(CitationMode::AuthorInText),
            "SuppressAuthor" => Ok(CitationMode::SuppressAuthor),
            "NormalCitation" => Ok(CitationMode::NormalCitation),
            other => self.fail(format_args!("unknown variant `{other}`")),
        }
    }
}
