//! Compact JSON writer for [`Document`]: appends the whole tree into one growable [`String`].
//!
//! Structural bytes and escape sequences are ASCII, and payload text is copied as whole UTF-8
//! slices, so the buffer stays valid UTF-8 throughout and never needs a revalidation pass.

use crate::ast::{
    Alignment, Attr, Block, Caption, Cell, Citation, CitationMode, ColSpec, ColWidth, Document,
    Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType, MetaValue, QuoteType, Row,
    Table, TableBody, TableFoot, TableHead, Target, Text,
};
use std::collections::BTreeMap;

/// Serialize a document to a compact JSON string with no surrounding whitespace.
pub(super) fn write_document_string(document: &Document) -> String {
    let mut out = String::with_capacity(1024);
    write_document(&mut out, document);
    out
}

fn write_document(out: &mut String, document: &Document) {
    out.push('{');
    write_string(out, crate::API_VERSION_KEY);
    out.push(':');
    write_u32_array(out, &document.api_version.0);
    out.push_str(",\"meta\":");
    write_meta_map(out, &document.meta);
    out.push_str(",\"blocks\":");
    write_block_list(out, &document.blocks);
    out.push('}');
}

fn write_u32_array(out: &mut String, values: &[u32]) {
    out.push('[');
    let mut buffer = itoa::Buffer::new();
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(buffer.format(*value));
    }
    out.push(']');
}

fn write_i32(out: &mut String, value: i32) {
    let mut buffer = itoa::Buffer::new();
    out.push_str(buffer.format(value));
}

fn write_f64(out: &mut String, value: f64) {
    if value.is_finite() {
        let mut buffer = ryu::Buffer::new();
        out.push_str(buffer.format_finite(value));
    } else {
        out.push_str("null");
    }
}

fn write_meta_map(out: &mut String, map: &BTreeMap<Text, MetaValue>) {
    out.push('{');
    for (index, (key, value)) in map.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_string(out, key);
        out.push(':');
        write_meta(out, value);
    }
    out.push('}');
}

fn write_meta(out: &mut String, meta: &MetaValue) {
    match meta {
        MetaValue::MetaMap(map) => {
            open_tag(out, "MetaMap");
            write_meta_map(out, map);
            out.push('}');
        }
        MetaValue::MetaList(values) => {
            open_tag(out, "MetaList");
            write_list(out, values, write_meta);
            out.push('}');
        }
        MetaValue::MetaBool(value) => {
            open_tag(out, "MetaBool");
            out.push_str(if *value { "true" } else { "false" });
            out.push('}');
        }
        MetaValue::MetaString(text) => {
            open_tag(out, "MetaString");
            write_string(out, text);
            out.push('}');
        }
        MetaValue::MetaInlines(inlines) => {
            open_tag(out, "MetaInlines");
            write_inline_list(out, inlines);
            out.push('}');
        }
        MetaValue::MetaBlocks(blocks) => {
            open_tag(out, "MetaBlocks");
            write_block_list(out, blocks);
            out.push('}');
        }
    }
}

fn write_block_list(out: &mut String, blocks: &[Block]) {
    write_list(out, blocks, write_block);
}

fn write_block(out: &mut String, block: &Block) {
    match block {
        Block::Plain(inlines) => {
            open_tag(out, "Plain");
            write_inline_list(out, inlines);
            out.push('}');
        }
        Block::Para(inlines) => {
            open_tag(out, "Para");
            write_inline_list(out, inlines);
            out.push('}');
        }
        Block::LineBlock(lines) => {
            open_tag(out, "LineBlock");
            write_list(out, lines, |out, line| write_inline_list(out, line));
            out.push('}');
        }
        Block::CodeBlock(attr, text) => {
            open_tag(out, "CodeBlock");
            out.push('[');
            write_attr(out, attr);
            out.push(',');
            write_string(out, text);
            out.push_str("]}");
        }
        Block::RawBlock(format, text) => {
            open_tag(out, "RawBlock");
            out.push('[');
            write_string(out, &format.0);
            out.push(',');
            write_string(out, text);
            out.push_str("]}");
        }
        Block::BlockQuote(blocks) => {
            open_tag(out, "BlockQuote");
            write_block_list(out, blocks);
            out.push('}');
        }
        Block::OrderedList(attributes, items) => {
            open_tag(out, "OrderedList");
            out.push('[');
            write_list_attributes(out, attributes);
            out.push(',');
            write_list(out, items, |out, item| write_block_list(out, item));
            out.push_str("]}");
        }
        Block::BulletList(items) => {
            open_tag(out, "BulletList");
            write_list(out, items, |out, item| write_block_list(out, item));
            out.push('}');
        }
        Block::DefinitionList(entries) => {
            open_tag(out, "DefinitionList");
            write_list(out, entries, |out, (term, definitions)| {
                out.push('[');
                write_inline_list(out, term);
                out.push(',');
                write_list(out, definitions, |out, item| write_block_list(out, item));
                out.push(']');
            });
            out.push('}');
        }
        Block::Header(level, attr, inlines) => {
            open_tag(out, "Header");
            out.push('[');
            write_i32(out, *level);
            out.push(',');
            write_attr(out, attr);
            out.push(',');
            write_inline_list(out, inlines);
            out.push_str("]}");
        }
        Block::HorizontalRule => unit_tag(out, "HorizontalRule"),
        Block::Table(table) => {
            open_tag(out, "Table");
            write_table(out, table);
            out.push('}');
        }
        Block::Figure(attr, caption, blocks) => {
            open_tag(out, "Figure");
            out.push('[');
            write_attr(out, attr);
            out.push(',');
            write_caption(out, caption);
            out.push(',');
            write_block_list(out, blocks);
            out.push_str("]}");
        }
        Block::Div(attr, blocks) => {
            open_tag(out, "Div");
            out.push('[');
            write_attr(out, attr);
            out.push(',');
            write_block_list(out, blocks);
            out.push_str("]}");
        }
    }
}

fn write_inline_list(out: &mut String, inlines: &[Inline]) {
    write_list(out, inlines, write_inline);
}

fn write_inline(out: &mut String, inline: &Inline) {
    match inline {
        Inline::Str(text) => {
            open_tag(out, "Str");
            write_string(out, text);
            out.push('}');
        }
        Inline::Emph(inlines) => wrap_inlines(out, "Emph", inlines),
        Inline::Underline(inlines) => wrap_inlines(out, "Underline", inlines),
        Inline::Strong(inlines) => wrap_inlines(out, "Strong", inlines),
        Inline::Strikeout(inlines) => wrap_inlines(out, "Strikeout", inlines),
        Inline::Superscript(inlines) => wrap_inlines(out, "Superscript", inlines),
        Inline::Subscript(inlines) => wrap_inlines(out, "Subscript", inlines),
        Inline::SmallCaps(inlines) => wrap_inlines(out, "SmallCaps", inlines),
        Inline::Quoted(quote, inlines) => {
            open_tag(out, "Quoted");
            out.push('[');
            write_quote_type(out, quote);
            out.push(',');
            write_inline_list(out, inlines);
            out.push_str("]}");
        }
        Inline::Cite(citations, inlines) => {
            open_tag(out, "Cite");
            out.push('[');
            write_list(out, citations, write_citation);
            out.push(',');
            write_inline_list(out, inlines);
            out.push_str("]}");
        }
        Inline::Code(attr, text) => {
            open_tag(out, "Code");
            out.push('[');
            write_attr(out, attr);
            out.push(',');
            write_string(out, text);
            out.push_str("]}");
        }
        Inline::Space => unit_tag(out, "Space"),
        Inline::SoftBreak => unit_tag(out, "SoftBreak"),
        Inline::LineBreak => unit_tag(out, "LineBreak"),
        Inline::Math(math, text) => {
            open_tag(out, "Math");
            out.push('[');
            write_math_type(out, math);
            out.push(',');
            write_string(out, text);
            out.push_str("]}");
        }
        Inline::RawInline(format, text) => {
            open_tag(out, "RawInline");
            out.push('[');
            write_string(out, &format.0);
            out.push(',');
            write_string(out, text);
            out.push_str("]}");
        }
        Inline::Link(attr, inlines, target) => {
            open_tag(out, "Link");
            out.push('[');
            write_attr(out, attr);
            out.push(',');
            write_inline_list(out, inlines);
            out.push(',');
            write_target(out, target);
            out.push_str("]}");
        }
        Inline::Image(attr, inlines, target) => {
            open_tag(out, "Image");
            out.push('[');
            write_attr(out, attr);
            out.push(',');
            write_inline_list(out, inlines);
            out.push(',');
            write_target(out, target);
            out.push_str("]}");
        }
        Inline::Note(blocks) => {
            open_tag(out, "Note");
            write_block_list(out, blocks);
            out.push('}');
        }
        Inline::Span(attr, inlines) => {
            open_tag(out, "Span");
            out.push('[');
            write_attr(out, attr);
            out.push(',');
            write_inline_list(out, inlines);
            out.push_str("]}");
        }
    }
}

fn wrap_inlines(out: &mut String, name: &str, inlines: &[Inline]) {
    open_tag(out, name);
    write_inline_list(out, inlines);
    out.push('}');
}

fn write_citation(out: &mut String, citation: &Citation) {
    out.push_str("{\"citationId\":");
    write_string(out, &citation.id);
    out.push_str(",\"citationPrefix\":");
    write_inline_list(out, &citation.prefix);
    out.push_str(",\"citationSuffix\":");
    write_inline_list(out, &citation.suffix);
    out.push_str(",\"citationMode\":");
    write_citation_mode(out, &citation.mode);
    out.push_str(",\"citationNoteNum\":");
    write_i32(out, citation.note_num);
    out.push_str(",\"citationHash\":");
    write_i32(out, citation.hash);
    out.push('}');
}

fn write_table(out: &mut String, table: &Table) {
    out.push('[');
    write_attr(out, &table.attr);
    out.push(',');
    write_caption(out, &table.caption);
    out.push(',');
    write_list(out, &table.col_specs, write_col_spec);
    out.push(',');
    write_table_head(out, &table.head);
    out.push(',');
    write_list(out, &table.bodies, write_table_body);
    out.push(',');
    write_table_foot(out, &table.foot);
    out.push(']');
}

fn write_table_head(out: &mut String, head: &TableHead) {
    out.push('[');
    write_attr(out, &head.attr);
    out.push(',');
    write_list(out, &head.rows, write_row);
    out.push(']');
}

fn write_table_body(out: &mut String, body: &TableBody) {
    out.push('[');
    write_attr(out, &body.attr);
    out.push(',');
    write_i32(out, body.row_head_columns);
    out.push(',');
    write_list(out, &body.head, write_row);
    out.push(',');
    write_list(out, &body.body, write_row);
    out.push(']');
}

fn write_table_foot(out: &mut String, foot: &TableFoot) {
    out.push('[');
    write_attr(out, &foot.attr);
    out.push(',');
    write_list(out, &foot.rows, write_row);
    out.push(']');
}

fn write_row(out: &mut String, row: &Row) {
    out.push('[');
    write_attr(out, &row.attr);
    out.push(',');
    write_list(out, &row.cells, write_cell);
    out.push(']');
}

fn write_cell(out: &mut String, cell: &Cell) {
    out.push('[');
    write_attr(out, &cell.attr);
    out.push(',');
    write_alignment(out, &cell.align);
    out.push(',');
    write_i32(out, cell.row_span);
    out.push(',');
    write_i32(out, cell.col_span);
    out.push(',');
    write_block_list(out, &cell.content);
    out.push(']');
}

fn write_col_spec(out: &mut String, spec: &ColSpec) {
    out.push('[');
    write_alignment(out, &spec.align);
    out.push(',');
    write_col_width(out, &spec.width);
    out.push(']');
}

fn write_col_width(out: &mut String, width: &ColWidth) {
    match width {
        ColWidth::ColWidth(value) => {
            open_tag(out, "ColWidth");
            write_f64(out, *value);
            out.push('}');
        }
        ColWidth::ColWidthDefault => unit_tag(out, "ColWidthDefault"),
    }
}

fn write_caption(out: &mut String, caption: &Caption) {
    out.push('[');
    match &caption.short {
        Some(inlines) => write_inline_list(out, inlines),
        None => out.push_str("null"),
    }
    out.push(',');
    write_block_list(out, &caption.long);
    out.push(']');
}

fn write_attr(out: &mut String, attr: &Attr) {
    out.push('[');
    write_string(out, &attr.id);
    out.push(',');
    write_text_array(out, &attr.classes);
    out.push(',');
    out.push('[');
    for (index, (key, value)) in attr.attributes.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('[');
        write_string(out, key);
        out.push(',');
        write_string(out, value);
        out.push(']');
    }
    out.push_str("]]");
}

fn write_target(out: &mut String, target: &Target) {
    out.push('[');
    write_string(out, &target.url);
    out.push(',');
    write_string(out, &target.title);
    out.push(']');
}

fn write_list_attributes(out: &mut String, attributes: &ListAttributes) {
    out.push('[');
    write_i32(out, attributes.start);
    out.push(',');
    write_list_number_style(out, attributes.style);
    out.push(',');
    write_list_number_delim(out, attributes.delim);
    out.push(']');
}

fn write_text_array(out: &mut String, values: &[Text]) {
    out.push('[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_string(out, value);
    }
    out.push(']');
}

fn write_quote_type(out: &mut String, quote: &QuoteType) {
    unit_tag(
        out,
        match quote {
            QuoteType::SingleQuote => "SingleQuote",
            QuoteType::DoubleQuote => "DoubleQuote",
        },
    );
}

fn write_math_type(out: &mut String, math: &MathType) {
    unit_tag(
        out,
        match math {
            MathType::InlineMath => "InlineMath",
            MathType::DisplayMath => "DisplayMath",
        },
    );
}

fn write_alignment(out: &mut String, alignment: &Alignment) {
    unit_tag(
        out,
        match alignment {
            Alignment::AlignLeft => "AlignLeft",
            Alignment::AlignRight => "AlignRight",
            Alignment::AlignCenter => "AlignCenter",
            Alignment::AlignDefault => "AlignDefault",
        },
    );
}

fn write_list_number_style(out: &mut String, style: ListNumberStyle) {
    unit_tag(
        out,
        match style {
            ListNumberStyle::DefaultStyle => "DefaultStyle",
            ListNumberStyle::Example => "Example",
            ListNumberStyle::Decimal => "Decimal",
            ListNumberStyle::LowerRoman => "LowerRoman",
            ListNumberStyle::UpperRoman => "UpperRoman",
            ListNumberStyle::LowerAlpha => "LowerAlpha",
            ListNumberStyle::UpperAlpha => "UpperAlpha",
        },
    );
}

fn write_list_number_delim(out: &mut String, delim: ListNumberDelim) {
    unit_tag(
        out,
        match delim {
            ListNumberDelim::DefaultDelim => "DefaultDelim",
            ListNumberDelim::Period => "Period",
            ListNumberDelim::OneParen => "OneParen",
            ListNumberDelim::TwoParens => "TwoParens",
        },
    );
}

fn write_citation_mode(out: &mut String, mode: &CitationMode) {
    unit_tag(
        out,
        match mode {
            CitationMode::AuthorInText => "AuthorInText",
            CitationMode::SuppressAuthor => "SuppressAuthor",
            CitationMode::NormalCitation => "NormalCitation",
        },
    );
}

/// Writes a JSON array by delimiting the results of `write_item` with commas.
fn write_list<T>(out: &mut String, items: &[T], write_item: impl Fn(&mut String, &T)) {
    out.push('[');
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_item(out, item);
    }
    out.push(']');
}

/// Opens a tagged-object node, leaving the parser ready to append the content value: `{"t":"NAME","c":`.
fn open_tag(out: &mut String, name: &str) {
    out.push_str("{\"t\":\"");
    out.push_str(name);
    out.push_str("\",\"c\":");
}

/// Writes a content-free tagged node: `{"t":"NAME"}`.
fn unit_tag(out: &mut String, name: &str) {
    out.push_str("{\"t\":\"");
    out.push_str(name);
    out.push_str("\"}");
}

/// Maps each byte to how it must be escaped inside a JSON string: `0` passes through, `1` needs the
/// six-byte `\u00XX` form, and any other value is the literal byte emitted after a backslash (the
/// short escapes `"` `\` `b` `t` `n` `f` `r`).
static ESCAPE: [u8; 256] = {
    const UU: u8 = 1;
    const BB: u8 = b'b';
    const TT: u8 = b't';
    const NN: u8 = b'n';
    const FF: u8 = b'f';
    const RR: u8 = b'r';
    const QU: u8 = b'"';
    const BS: u8 = b'\\';
    const __: u8 = 0;
    [
        UU, UU, UU, UU, UU, UU, UU, UU, BB, TT, NN, UU, FF, RR, UU, UU, // 0x00
        UU, UU, UU, UU, UU, UU, UU, UU, UU, UU, UU, UU, UU, UU, UU, UU, // 0x10
        __, __, QU, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0x20
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0x30
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0x40
        __, __, __, __, __, __, __, __, __, __, __, __, BS, __, __, __, // 0x50
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0x60
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0x70
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0x80
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0x90
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0xA0
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0xB0
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0xC0
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0xD0
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0xE0
        __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, __, // 0xF0
    ]
};

/// Writes a quoted, escaped JSON string. Clean spans are copied whole; only bytes flagged by
/// [`ESCAPE`] interrupt the copy.
fn write_string(out: &mut String, text: &str) {
    out.push('"');
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let escape = *ESCAPE.get(byte as usize).unwrap_or(&0);
        if escape == 0 {
            index += 1;
            continue;
        }
        if let Some(clean) = text.get(start..index) {
            out.push_str(clean);
        }
        write_escape(out, byte, escape);
        index += 1;
        start = index;
    }
    if let Some(clean) = text.get(start..) {
        out.push_str(clean);
    }
    out.push('"');
}

fn write_escape(out: &mut String, byte: u8, escape: u8) {
    match escape {
        1 => {
            out.push_str("\\u00");
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0F));
        }
        short => {
            out.push('\\');
            out.push(short as char);
        }
    }
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + (nibble - 10)) as char,
    }
}
