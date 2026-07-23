//! Jupyter notebook writer: renders the document model to a notebook (`nbformat` 4.5) JSON document.
//!
//! The notebook is a list of cells followed by the format version and notebook metadata. Cells are
//! derived from the top-level block sequence: a [`Block::Div`] carrying the `cell` class is one cell,
//! and any run of blocks between such divs is grouped into a markdown cell of its own. A cell's
//! second class selects its kind: `code` and `raw` are recognized, anything else is markdown.
//!
//! A markdown cell renders its blocks through the markdown engine and stores the result as `source`.
//! A code cell takes the text of its first [`Block::CodeBlock`] as `source`, reads its execution
//! count from the `execution_count` attribute, and reconstructs each sibling `output` div as an
//! output object. A raw cell carries the text of its [`Block::RawBlock`] as `source` under the
//! `raw_mimetype` derived from the block's format.
//!
//! Notebook metadata is the contents of the `jupyter` metadata entry (minus the version keys, which
//! are fixed at 4 and 5). Every cell receives an `id`: the div's identifier when it has one, and
//! otherwise a stable identifier derived from the cell's position and content. The document is
//! pretty-printed with one-space indentation; an empty object or array stays on one line. The result
//! carries no trailing newline; the caller appends one.

use std::collections::BTreeMap;
use std::iter::Peekable;
use std::str::Chars;

use carta_ast::{Attr, Block, Document, Format, Inline, MetaValue, Text, to_plain_text};
use carta_core::media::base64_encode_mime;
use carta_core::{Error, Extension, Extensions, MediaBag, Result, Writer, WriterOptions};

use crate::markdown::MarkdownWriter;

/// The markdown dialect a notebook's cells are written in when the caller selects no extensions:
/// the constructs a notebook's markdown cells render natively.
const CELL_MARKDOWN_EXTENSIONS: Extensions = Extensions::from_list(&[
    Extension::Strikeout,
    Extension::PipeTables,
    Extension::BacktickCodeBlocks,
    Extension::FencedCodeBlocks,
    Extension::TaskLists,
    Extension::Autolink,
    Extension::TexMathDollars,
    Extension::AutoIdentifiers,
    Extension::GfmAutoIdentifiers,
    Extension::RawHtml,
    Extension::IntrawordUnderscores,
]);

/// Renders a document to a Jupyter notebook (no trailing newline).
#[derive(Debug, Default, Clone, Copy)]
pub struct IpynbWriter;

impl Writer for IpynbWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let cells = build_cells(&document.blocks, options)?;
        let notebook = Json::Object(vec![
            ("cells".to_owned(), Json::Array(cells)),
            ("nbformat".to_owned(), Json::Number("4".to_owned())),
            ("nbformat_minor".to_owned(), Json::Number("5".to_owned())),
            ("metadata".to_owned(), notebook_metadata(&document.meta)),
        ]);
        Ok(notebook.render())
    }
}

/// Splits the top-level blocks into cells: each `cell` div becomes its own cell, and each maximal
/// run of other blocks is grouped into a markdown cell.
fn build_cells(blocks: &[Block], options: &WriterOptions) -> Result<Vec<Json>> {
    let mut cells = Vec::new();
    let mut loose: Vec<Block> = Vec::new();
    let mut counter = 0usize;

    for block in blocks {
        match block {
            Block::Div(attr, content) if has_class(attr, "cell") => {
                if !loose.is_empty() {
                    cells.push(markdown_cell(
                        &loose,
                        &Attr::default(),
                        options,
                        &mut counter,
                    )?);
                    loose.clear();
                }
                cells.push(typed_cell(attr, content, options, &mut counter)?);
            }
            other => loose.push(other.clone()),
        }
    }
    if !loose.is_empty() {
        cells.push(markdown_cell(
            &loose,
            &Attr::default(),
            options,
            &mut counter,
        )?);
    }
    Ok(cells)
}

/// Builds a cell from a `cell` div, dispatching on its second class.
fn typed_cell(
    attr: &Attr,
    content: &[Block],
    options: &WriterOptions,
    counter: &mut usize,
) -> Result<Json> {
    if has_class(attr, "code") {
        code_cell(attr, content, &options.media, counter)
    } else if has_class(attr, "raw") {
        Ok(raw_cell(attr, content, counter))
    } else {
        markdown_cell(content, attr, options, counter)
    }
}

/// A markdown cell: its blocks rendered as markdown, with the div's attributes as cell metadata. An
/// image whose file name the media bag holds is restored to the cell's inline-attachment form: an
/// `attachment:` reference and an entry in the cell's `attachments` object.
fn markdown_cell(
    blocks: &[Block],
    attr: &Attr,
    options: &WriterOptions,
    counter: &mut usize,
) -> Result<Json> {
    let mut blocks = blocks.to_vec();
    let attachments = reattach_media(&mut blocks, &options.media)?;
    let source = render_cell_markdown(&blocks, options)?;
    let id = next_id(&attr.id, &source, counter);
    let mut fields = vec![
        ("cell_type".to_owned(), Json::Str("markdown".to_owned())),
        (
            "metadata".to_owned(),
            attribute_metadata(&attr.attributes, &[]),
        ),
        ("source".to_owned(), source_lines(&source)),
    ];
    if let Some(attachments) = attachments {
        fields.push(("attachments".to_owned(), attachments));
    }
    fields.push(("id".to_owned(), Json::Str(id)));
    Ok(Json::Object(fields))
}

/// Rewrites every image in a cell whose file name the bag holds into an `attachment:` reference and
/// returns the cell's reconstructed `attachments` object, or `None` when the cell draws on no bag
/// resources. Each attachment is keyed by the image's file name (the reference the rewritten link now
/// points at) and carries the same MIME-to-payload bundle an output image would. The keys are emitted in
/// sorted order, independent of where each reference first appears in the cell.
fn reattach_media(blocks: &mut [Block], media: &MediaBag) -> Result<Option<Json>> {
    let mut names: Vec<String> = Vec::new();
    carta_core::walk::for_each_image_target(blocks, &mut |target| {
        let name = target.url.as_str();
        if media.contains(name) {
            if !names.iter().any(|seen| seen == name) {
                names.push(name.to_owned());
            }
            target.url = format!("attachment:{name}").into();
        }
    });
    if names.is_empty() {
        return Ok(None);
    }
    names.sort();
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let bundle = Json::Object(vec![image_entry(&name, media)?]);
        entries.push((name, bundle));
    }
    Ok(Some(Json::Object(entries)))
}

/// The `execution_count` field for a cell or result: the attribute parsed as an integer, or null
/// when it is absent or not an integer.
fn execution_count_json(attr: &Attr) -> Json {
    match attribute_value(attr, "execution_count") {
        Some(raw) => match raw.parse::<i64>() {
            Ok(count) => Json::Number(count.to_string()),
            Err(_) => Json::Null,
        },
        None => Json::Null,
    }
}

/// A code cell: the first code block's text as source, the execution count lifted out of the
/// attributes, and each sibling `output` div reconstructed as an output object.
fn code_cell(
    attr: &Attr,
    content: &[Block],
    media: &MediaBag,
    counter: &mut usize,
) -> Result<Json> {
    let mut source = String::new();
    let mut found_source = false;
    let mut outputs = Vec::new();

    for block in content {
        match block {
            Block::CodeBlock(_, text) if !found_source => {
                source = text.to_string();
                found_source = true;
            }
            Block::Div(output_attr, inner) if has_class(output_attr, "output") => {
                outputs.push(output_object(output_attr, inner, media)?);
            }
            _ => {}
        }
    }

    let execution_count = execution_count_json(attr);
    let id = next_id(&attr.id, &source, counter);

    Ok(Json::Object(vec![
        ("cell_type".to_owned(), Json::Str("code".to_owned())),
        ("execution_count".to_owned(), execution_count),
        (
            "metadata".to_owned(),
            attribute_metadata(&attr.attributes, &["execution_count"]),
        ),
        ("outputs".to_owned(), Json::Array(outputs)),
        ("source".to_owned(), source_lines(&source)),
        ("id".to_owned(), Json::Str(id)),
    ]))
}

/// A raw cell: the first raw block's text as source under the mime type its format maps to.
fn raw_cell(attr: &Attr, content: &[Block], counter: &mut usize) -> Json {
    let mut source = String::new();
    let mut metadata = Vec::new();
    for block in content {
        if let Block::RawBlock(Format(format), text) = block {
            source = text.to_string();
            metadata.push(("raw_mimetype".to_owned(), Json::Str(format_to_mime(format))));
            break;
        }
    }
    let id = next_id(&attr.id, &source, counter);
    Json::Object(vec![
        ("cell_type".to_owned(), Json::Str("raw".to_owned())),
        ("metadata".to_owned(), Json::Object(metadata)),
        ("source".to_owned(), source_lines(&source)),
        ("id".to_owned(), Json::Str(id)),
    ])
}

/// Reconstructs an output object from an `output` div. The output kind is the div's second class.
fn output_object(attr: &Attr, content: &[Block], media: &MediaBag) -> Result<Json> {
    let output = match attr.classes.get(1).map(Text::as_str) {
        Some("stream") => {
            let name = attr
                .classes
                .get(2)
                .map_or_else(|| "stdout".to_owned(), ToString::to_string);
            Json::Object(vec![
                ("output_type".to_owned(), Json::Str("stream".to_owned())),
                ("name".to_owned(), Json::Str(name)),
                ("text".to_owned(), source_lines(&first_verbatim(content))),
            ])
        }
        Some("error") => Json::Object(vec![
            ("output_type".to_owned(), Json::Str("error".to_owned())),
            (
                "ename".to_owned(),
                Json::Str(
                    attribute_value(attr, "ename")
                        .unwrap_or_default()
                        .to_owned(),
                ),
            ),
            (
                "evalue".to_owned(),
                Json::Str(
                    attribute_value(attr, "evalue")
                        .unwrap_or_default()
                        .to_owned(),
                ),
            ),
            (
                "traceback".to_owned(),
                source_lines(&first_verbatim(content)),
            ),
        ]),
        Some("execute_result") => {
            let execution_count = execution_count_json(attr);
            Json::Object(vec![
                (
                    "output_type".to_owned(),
                    Json::Str("execute_result".to_owned()),
                ),
                ("execution_count".to_owned(), execution_count),
                ("metadata".to_owned(), output_metadata(content)),
                ("data".to_owned(), data_bundle(content, media)?),
            ])
        }
        _ => Json::Object(vec![
            (
                "output_type".to_owned(),
                Json::Str("display_data".to_owned()),
            ),
            ("metadata".to_owned(), output_metadata(content)),
            ("data".to_owned(), data_bundle(content, media)?),
        ]),
    };
    Ok(output)
}

/// The mime bundle of a rich output: each recognized block contributes one mime entry.
///
/// An image output references its payload by file name; the bytes are drawn from the media bag and
/// re-embedded under the image's MIME type. An image whose bytes the bag does not hold cannot be
/// reconstructed and is reported as unrepresentable rather than written as a broken bundle.
fn data_bundle(content: &[Block], media: &MediaBag) -> Result<Json> {
    let mut entries = Vec::new();
    for block in content {
        match block {
            Block::CodeBlock(_, text) => {
                entries.push(("text/plain".to_owned(), source_lines(text)));
            }
            Block::RawBlock(Format(format), text) => {
                entries.push((format_to_mime(format), source_lines(text)));
            }
            Block::Para(inlines) | Block::Plain(inlines) => {
                if let Some(url) = image_url(inlines) {
                    entries.push(image_entry(url, media)?);
                }
            }
            _ => {}
        }
    }
    Ok(Json::Object(entries))
}

/// The `data` entry for an image output: its MIME type mapped to the payload drawn from the media bag
/// under the image's file name. A textual image (SVG) is written as a source-line array; every other
/// type is written as a single base64 string wrapped to notebook line width. An image the bag does
/// not hold, or one carrying no MIME type, cannot be reconstructed and is unrepresentable.
fn image_entry(url: &str, media: &MediaBag) -> Result<(String, Json)> {
    let item = media.get(url).ok_or_else(|| {
        Error::Unrepresentable(format!(
            "an image output references the file {url:?}, whose bytes are not available"
        ))
    })?;
    let mime = item.mime.clone().ok_or_else(|| {
        Error::Unrepresentable(format!(
            "the image output {url:?} carries no MIME type, so its data bundle cannot be rebuilt"
        ))
    })?;
    let data = if mime == "image/svg+xml" {
        source_lines(&String::from_utf8_lossy(&item.bytes))
    } else {
        Json::Str(base64_encode_mime(&item.bytes))
    };
    Ok((mime, data))
}

/// The text of the first code block in a sequence, or empty when there is none.
fn first_verbatim(content: &[Block]) -> String {
    for block in content {
        if let Block::CodeBlock(_, text) = block {
            return text.to_string();
        }
    }
    String::new()
}

fn first_image(inlines: &[Inline]) -> Option<(&Attr, &str)> {
    inlines.iter().find_map(|inline| match inline {
        Inline::Image(attr, _, target) => Some((attr.as_ref(), target.url.as_str())),
        _ => None,
    })
}

fn image_url(inlines: &[Inline]) -> Option<&str> {
    first_image(inlines).map(|(_, url)| url)
}

/// The metadata object of a rich output, reconstructed from its image payload. An image output's
/// per-MIME display metadata (dimensions, background hint) is carried on the image's attributes;
/// this restores it as the output's own metadata, sorted by key. An output with no image, or an
/// image bearing no such attributes, has empty metadata.
fn output_metadata(content: &[Block]) -> Json {
    for block in content {
        if let Block::Para(inlines) | Block::Plain(inlines) = block
            && let Some((attr, _)) = first_image(inlines)
            && !attr.attributes.is_empty()
        {
            return attribute_metadata(&attr.attributes, &[]);
        }
    }
    Json::Object(Vec::new())
}

/// Builds cell metadata from a div's key/value attributes, skipping the named keys. Each value is
/// the attribute parsed as a JSON value, or the raw text when it is not valid JSON. Keys are ordered.
fn attribute_metadata(attributes: &[(Text, Text)], skip: &[&str]) -> Json {
    let mut entries: Vec<(String, Json)> = attributes
        .iter()
        .filter(|(key, _)| !skip.contains(&key.as_str()))
        .map(|(key, value)| (key.to_string(), parse_metadata_value(value)))
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Json::Object(entries)
}

/// Notebook metadata: the `jupyter` map minus the version keys, or empty when there is none.
fn notebook_metadata(meta: &BTreeMap<Text, MetaValue>) -> Json {
    let Some(MetaValue::MetaMap(map)) = meta.get("jupyter") else {
        return Json::Object(Vec::new());
    };
    let pairs = map
        .iter()
        .filter(|(key, _)| key.as_str() != "nbformat" && key.as_str() != "nbformat_minor")
        .map(|(key, value)| (key.to_string(), meta_to_json(value)))
        .collect();
    Json::Object(pairs)
}

fn meta_to_json(value: &MetaValue) -> Json {
    match value {
        MetaValue::MetaMap(map) => Json::Object(
            map.iter()
                .map(|(key, value)| (key.to_string(), meta_to_json(value)))
                .collect(),
        ),
        MetaValue::MetaList(values) => Json::Array(values.iter().map(meta_to_json).collect()),
        MetaValue::MetaBool(flag) => Json::Bool(*flag),
        MetaValue::MetaString(text) => Json::Str(text.to_string()),
        MetaValue::MetaInlines(inlines) => Json::Str(to_plain_text(inlines)),
        MetaValue::MetaBlocks(blocks) => Json::Str(meta_blocks_text(blocks)),
    }
}

/// The plain-text content of block-shaped metadata, with paragraphs separated by a blank line.
fn meta_blocks_text(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            Block::Para(inlines) | Block::Plain(inlines) | Block::Header(_, _, inlines) => {
                to_plain_text(inlines)
            }
            Block::CodeBlock(_, text) | Block::RawBlock(_, text) => text.to_string(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Renders a cell's blocks through the markdown engine, dropping any trailing newline so the final
/// source line does not carry one.
fn render_cell_markdown(blocks: &[Block], options: &WriterOptions) -> Result<String> {
    let mut cell_options = options.clone();
    cell_options.extensions = if options.extensions.is_empty() {
        CELL_MARKDOWN_EXTENSIONS
    } else {
        options.extensions
    };
    let document = Document {
        blocks: blocks.to_vec(),
        ..Document::default()
    };
    let rendered = MarkdownWriter.write(&document, &cell_options)?;
    Ok(rendered.trim_end_matches('\n').to_owned())
}

/// Splits text into the `source`-style line array: each line keeps its trailing newline, and a
/// trailing newline does not produce an empty final entry. Empty text yields an empty array.
fn source_lines(text: &str) -> Json {
    Json::Array(
        text.split_inclusive('\n')
            .map(|line| Json::Str(line.to_owned()))
            .collect(),
    )
}

fn has_class(attr: &Attr, class: &str) -> bool {
    attr.classes.iter().any(|candidate| candidate == class)
}

fn attribute_value<'a>(attr: &'a Attr, key: &str) -> Option<&'a str> {
    attr.attributes
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

/// Maps a raw-passthrough format to its mime type. An unrecognized format is used verbatim.
fn format_to_mime(format: &str) -> String {
    match format {
        "html" | "html5" | "html4" | "revealjs" => "text/html",
        "latex" => "text/latex",
        "markdown" => "text/markdown",
        "rst" => "text/restructuredtext",
        "asciidoc" => "text/asciidoc",
        other => other,
    }
    .to_owned()
}

/// The identifier for a cell: the div's own identifier when set, and otherwise a stable identifier
/// derived from the cell's ordinal and content. The counter advances for every cell so generated
/// identifiers stay distinct.
fn next_id(div_id: &str, seed: &str, counter: &mut usize) -> String {
    let ordinal = *counter;
    *counter += 1;
    if !div_id.is_empty() {
        return div_id.to_owned();
    }
    generated_id(ordinal, seed)
}

/// A deterministic identifier in the canonical 8-4-4-4-12 hexadecimal layout, hashed from the cell's
/// ordinal and content so repeated runs over the same document reproduce it byte for byte.
// The single-letter bindings mirror the byte layout the format string lays out.
#[allow(clippy::many_single_char_names)]
fn generated_id(ordinal: usize, seed: &str) -> String {
    let mut material = ordinal.to_string();
    material.push('\u{0}');
    material.push_str(seed);
    let high = fnv1a(0xcbf2_9ce4_8422_2325, material.as_bytes());
    let low = fnv1a(high ^ 0x9e37_79b9_7f4a_7c15, material.as_bytes());

    let [a, b, c, d, e, f, g, h] = high.to_be_bytes();
    let [i, j, k, l, m, n, o, p] = low.to_be_bytes();
    let g = (g & 0x0f) | 0x40;
    let i = (i & 0x3f) | 0x80;
    format!(
        "{a:02x}{b:02x}{c:02x}{d:02x}-{e:02x}{f:02x}-{g:02x}{h:02x}-{i:02x}{j:02x}-{k:02x}{l:02x}{m:02x}{n:02x}{o:02x}{p:02x}"
    )
}

fn fnv1a(basis: u64, bytes: &[u8]) -> u64 {
    let mut hash = basis;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// An ordered JSON value rendered with one-space indentation.
enum Json {
    Null,
    Bool(bool),
    /// A numeric literal stored verbatim so its exact spelling round-trips.
    Number(String),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn render(&self) -> String {
        let mut out = String::new();
        self.write_to(&mut out, 0);
        out
    }

    fn write_to(&self, out: &mut String, level: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Number(literal) => out.push_str(literal),
            Json::Str(text) => escape_string(text, out),
            Json::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                let last = items.len().saturating_sub(1);
                for (index, item) in items.iter().enumerate() {
                    push_spaces(out, level + 1);
                    item.write_to(out, level + 1);
                    if index != last {
                        out.push(',');
                    }
                    out.push('\n');
                }
                push_spaces(out, level);
                out.push(']');
            }
            Json::Object(pairs) => {
                if pairs.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                let last = pairs.len().saturating_sub(1);
                for (index, (key, value)) in pairs.iter().enumerate() {
                    push_spaces(out, level + 1);
                    escape_string(key, out);
                    out.push_str(": ");
                    value.write_to(out, level + 1);
                    if index != last {
                        out.push(',');
                    }
                    out.push('\n');
                }
                push_spaces(out, level);
                out.push('}');
            }
        }
    }
}

fn push_spaces(out: &mut String, count: usize) {
    for _ in 0..count {
        out.push(' ');
    }
}

/// Writes a string as a quoted JSON literal: the quote and backslash are escaped, the newline,
/// carriage return, and tab use their short escapes, and every other control character uses a
/// `\u00xx` escape. All other characters, including `/` and non-ASCII, are written verbatim.
fn escape_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str("\\u");
                let code = control as u32;
                for shift in [12, 8, 4, 0] {
                    let nibble = (code >> shift) & 0xf;
                    out.push(hex_digit(nibble));
                }
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

fn hex_digit(value: u32) -> char {
    char::from_digit(value, 16).unwrap_or('0')
}

/// Parses a string as a JSON value, falling back to the string itself when it is not valid JSON.
/// This decides whether a cell-metadata attribute is a number, boolean, list, or object versus text.
fn parse_metadata_value(input: &str) -> Json {
    let mut chars = input.chars().peekable();
    match parse_value(&mut chars) {
        Some(value) => {
            skip_whitespace(&mut chars);
            if chars.next().is_none() {
                value
            } else {
                Json::Str(input.to_owned())
            }
        }
        None => Json::Str(input.to_owned()),
    }
}

fn parse_value(chars: &mut Peekable<Chars>) -> Option<Json> {
    skip_whitespace(chars);
    match chars.peek()? {
        '"' => parse_json_string(chars).map(Json::Str),
        '{' => parse_object(chars),
        '[' => parse_array(chars),
        't' => parse_keyword(chars, "true", Json::Bool(true)),
        'f' => parse_keyword(chars, "false", Json::Bool(false)),
        'n' => parse_keyword(chars, "null", Json::Null),
        digit if *digit == '-' || digit.is_ascii_digit() => parse_number(chars),
        _ => None,
    }
}

fn parse_keyword(chars: &mut Peekable<Chars>, word: &str, value: Json) -> Option<Json> {
    for expected in word.chars() {
        if chars.next()? != expected {
            return None;
        }
    }
    Some(value)
}

fn parse_number(chars: &mut Peekable<Chars>) -> Option<Json> {
    let mut literal = String::new();
    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E') {
            literal.push(ch);
            chars.next();
        } else {
            break;
        }
    }
    if literal.parse::<f64>().is_ok() {
        Some(Json::Number(literal))
    } else {
        None
    }
}

fn parse_json_string(chars: &mut Peekable<Chars>) -> Option<String> {
    if chars.next()? != '"' {
        return None;
    }
    let mut text = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(text),
            '\\' => match chars.next()? {
                '"' => text.push('"'),
                '\\' => text.push('\\'),
                '/' => text.push('/'),
                'b' => text.push('\u{0008}'),
                'f' => text.push('\u{000c}'),
                'n' => text.push('\n'),
                'r' => text.push('\r'),
                't' => text.push('\t'),
                'u' => {
                    let mut code = 0u32;
                    for _ in 0..4 {
                        code = code * 16 + chars.next()?.to_digit(16)?;
                    }
                    text.push(char::from_u32(code)?);
                }
                _ => return None,
            },
            other => text.push(other),
        }
    }
}

fn parse_array(chars: &mut Peekable<Chars>) -> Option<Json> {
    if chars.next()? != '[' {
        return None;
    }
    let mut items = Vec::new();
    skip_whitespace(chars);
    if chars.peek() == Some(&']') {
        chars.next();
        return Some(Json::Array(items));
    }
    loop {
        items.push(parse_value(chars)?);
        skip_whitespace(chars);
        match chars.next()? {
            ',' => {}
            ']' => return Some(Json::Array(items)),
            _ => return None,
        }
    }
}

fn parse_object(chars: &mut Peekable<Chars>) -> Option<Json> {
    if chars.next()? != '{' {
        return None;
    }
    let mut pairs = Vec::new();
    skip_whitespace(chars);
    if chars.peek() == Some(&'}') {
        chars.next();
        return Some(Json::Object(pairs));
    }
    loop {
        skip_whitespace(chars);
        let key = parse_json_string(chars)?;
        skip_whitespace(chars);
        if chars.next()? != ':' {
            return None;
        }
        pairs.push((key, parse_value(chars)?));
        skip_whitespace(chars);
        match chars.next()? {
            ',' => {}
            '}' => return Some(Json::Object(pairs)),
            _ => return None,
        }
    }
}

fn skip_whitespace(chars: &mut Peekable<Chars>) {
    while let Some(&ch) = chars.peek() {
        if matches!(ch, ' ' | '\t' | '\n' | '\r') {
            chars.next();
        } else {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::Inline;

    fn para(text: &str) -> Block {
        Block::Para(vec![Inline::Str(text.to_owned().into())])
    }

    fn write(blocks: Vec<Block>) -> String {
        IpynbWriter
            .write(
                &Document {
                    blocks,
                    ..Document::default()
                },
                &WriterOptions::default(),
            )
            .expect("write")
    }

    #[test]
    fn empty_document_is_an_empty_notebook() {
        assert_eq!(
            write(Vec::new()),
            "{\n \"cells\": [],\n \"nbformat\": 4,\n \"nbformat_minor\": 5,\n \"metadata\": {}\n}"
        );
    }

    #[test]
    fn loose_blocks_become_one_markdown_cell() {
        let notebook = write(vec![
            Block::Header(
                1,
                Box::default(),
                vec![Inline::Str("Title".to_owned().into())],
            ),
            para("Body."),
        ]);
        assert!(notebook.contains("\"cell_type\": \"markdown\""));
        assert!(notebook.contains("\"# Title\\n\""));
        assert!(notebook.contains("\"\\n\""));
        assert!(notebook.contains("\"Body.\""));
        assert_eq!(notebook.matches("\"cell_type\"").count(), 1);
    }

    #[test]
    fn source_lines_keep_trailing_newlines() {
        let Json::Array(lines) = source_lines("a\n\nb") else {
            panic!("expected array");
        };
        assert_eq!(lines.len(), 3);
        let mut rendered = String::new();
        Json::Array(lines).write_to(&mut rendered, 0);
        assert_eq!(rendered, "[\n \"a\\n\",\n \"\\n\",\n \"b\"\n]");

        let Json::Array(empty) = source_lines("") else {
            panic!("expected array");
        };
        assert!(empty.is_empty());
    }

    #[test]
    fn cell_div_selects_kind_and_keeps_id() {
        let notebook = write(vec![Block::Div(
            Box::new(Attr {
                id: "given".to_owned().into(),
                classes: vec!["cell".to_owned().into(), "code".to_owned().into()],
                attributes: vec![("execution_count".to_owned().into(), "7".to_owned().into())],
            }),
            vec![Block::CodeBlock(
                Box::default(),
                "print(1)".to_owned().into(),
            )],
        )]);
        assert!(notebook.contains("\"cell_type\": \"code\""));
        assert!(notebook.contains("\"execution_count\": 7"));
        assert!(notebook.contains("\"outputs\": []"));
        assert!(notebook.contains("\"print(1)\""));
        assert!(notebook.contains("\"id\": \"given\""));
    }

    #[test]
    fn raw_cell_carries_mime_type() {
        let notebook = write(vec![Block::Div(
            Box::new(Attr {
                id: String::new().into(),
                classes: vec!["cell".to_owned().into(), "raw".to_owned().into()],
                attributes: Vec::new(),
            }),
            vec![Block::RawBlock(
                Format("html".to_owned().into()),
                "<b>x</b>".to_owned().into(),
            )],
        )]);
        assert!(notebook.contains("\"cell_type\": \"raw\""));
        assert!(notebook.contains("\"raw_mimetype\": \"text/html\""));
        assert!(notebook.contains("\"<b>x</b>\""));
    }

    #[test]
    fn raw_cell_maps_asciidoc_mime() {
        let notebook = write(vec![Block::Div(
            Box::new(Attr {
                id: String::new().into(),
                classes: vec!["cell".to_owned().into(), "raw".to_owned().into()],
                attributes: Vec::new(),
            }),
            vec![Block::RawBlock(
                Format("asciidoc".to_owned().into()),
                "[NOTE]\n====\nbody\n====".to_owned().into(),
            )],
        )]);
        assert!(notebook.contains("\"cell_type\": \"raw\""));
        assert!(notebook.contains("\"raw_mimetype\": \"text/asciidoc\""));
    }

    #[test]
    fn stream_and_error_outputs_round_trip() {
        let notebook = write(vec![Block::Div(
            Box::new(Attr {
                id: String::new().into(),
                classes: vec!["cell".to_owned().into(), "code".to_owned().into()],
                attributes: Vec::new(),
            }),
            vec![
                Block::CodeBlock(Box::default(), "x".to_owned().into()),
                Block::Div(
                    Box::new(Attr {
                        id: String::new().into(),
                        classes: vec![
                            "output".to_owned().into(),
                            "stream".to_owned().into(),
                            "stdout".to_owned().into(),
                        ],
                        attributes: Vec::new(),
                    }),
                    vec![Block::CodeBlock(Box::default(), "hi\n".to_owned().into())],
                ),
                Block::Div(
                    Box::new(Attr {
                        id: String::new().into(),
                        classes: vec!["output".to_owned().into(), "error".to_owned().into()],
                        attributes: vec![
                            ("ename".to_owned().into(), "ValueError".to_owned().into()),
                            ("evalue".to_owned().into(), "bad".to_owned().into()),
                        ],
                    }),
                    vec![Block::CodeBlock(
                        Box::default(),
                        "trace\n".to_owned().into(),
                    )],
                ),
            ],
        )]);
        assert!(notebook.contains("\"output_type\": \"stream\""));
        assert!(notebook.contains("\"name\": \"stdout\""));
        assert!(notebook.contains("\"output_type\": \"error\""));
        assert!(notebook.contains("\"ename\": \"ValueError\""));
        assert!(notebook.contains("\"evalue\": \"bad\""));
    }

    /// A one-code-cell document whose single `display_data` output is an image referencing `url`.
    fn image_output_document(url: &str) -> Document {
        Document {
            blocks: vec![Block::Div(
                Box::new(Attr {
                    id: String::new().into(),
                    classes: vec!["cell".to_owned().into(), "code".to_owned().into()],
                    attributes: Vec::new(),
                }),
                vec![
                    Block::CodeBlock(Box::default(), "plot()".to_owned().into()),
                    Block::Div(
                        Box::new(Attr {
                            id: String::new().into(),
                            classes: vec![
                                "output".to_owned().into(),
                                "display_data".to_owned().into(),
                            ],
                            attributes: Vec::new(),
                        }),
                        vec![Block::Para(vec![Inline::Image(
                            Box::default(),
                            Vec::new(),
                            Box::new(carta_ast::Target {
                                url: url.to_owned().into(),
                                title: String::new().into(),
                            }),
                        )])],
                    ),
                ],
            )],
            ..Document::default()
        }
    }

    #[test]
    fn image_output_without_embedded_data_is_unrepresentable() {
        // With no media bag, the referenced bytes are unavailable, so the output cannot be rebuilt.
        let document = image_output_document("plot.png");
        match IpynbWriter.write(&document, &WriterOptions::default()) {
            Err(Error::Unrepresentable(message)) => assert!(
                message.contains("plot.png"),
                "message should name the file: {message}"
            ),
            other => panic!("expected an unrepresentable error, got {other:?}"),
        }
    }

    #[test]
    fn image_output_data_is_re_embedded_from_the_media_bag() {
        let bytes = vec![1u8, 2, 3, 4, 5, 6, 7];
        let mut bag = MediaBag::new();
        bag.insert("plot.png", Some("image/png".to_owned()), bytes.clone());
        let mut options = WriterOptions::default();
        options.media = std::sync::Arc::new(bag);

        let notebook = IpynbWriter
            .write(&image_output_document("plot.png"), &options)
            .expect("the image bytes are available, so the output writes");
        // The bundle names the image's MIME type and carries the base64 of the bag's bytes.
        assert!(notebook.contains("\"image/png\""));
        let encoded = base64_encode_mime(&bytes);
        assert!(
            notebook.contains(encoded.trim_end_matches('\n')),
            "notebook should embed the base64 payload"
        );
    }

    #[test]
    fn svg_output_data_is_re_embedded_as_source_lines() {
        let mut bag = MediaBag::new();
        bag.insert(
            "fig.svg",
            Some("image/svg+xml".to_owned()),
            b"<svg/>".to_vec(),
        );
        let mut options = WriterOptions::default();
        options.media = std::sync::Arc::new(bag);

        let notebook = IpynbWriter
            .write(&image_output_document("fig.svg"), &options)
            .expect("the svg bytes are available, so the output writes");
        assert!(notebook.contains("\"image/svg+xml\""));
        assert!(notebook.contains("<svg/>"));
    }

    #[test]
    fn markdown_cell_image_becomes_an_inline_attachment() {
        let bytes = vec![9u8, 8, 7, 6];
        let mut bag = MediaBag::new();
        bag.insert("cell-diagram.png", Some("image/png".to_owned()), bytes);
        let mut options = WriterOptions::default();
        options.media = std::sync::Arc::new(bag);

        // A markdown cell whose image references a bag entry by its file name.
        let document = Document {
            blocks: vec![Block::Div(
                Box::new(Attr {
                    id: "cell".to_owned().into(),
                    classes: vec!["cell".to_owned().into(), "markdown".to_owned().into()],
                    attributes: Vec::new(),
                }),
                vec![Block::Para(vec![Inline::Image(
                    Box::default(),
                    vec![Inline::Str("a diagram".to_owned().into())],
                    Box::new(carta_ast::Target {
                        url: "cell-diagram.png".to_owned().into(),
                        title: String::new().into(),
                    }),
                )])],
            )],
            ..Document::default()
        };
        let notebook = IpynbWriter.write(&document, &options).expect("writes");
        // The link is rewritten to the attachment form and the payload restored under that name.
        assert!(notebook.contains("attachment:cell-diagram.png"));
        assert!(notebook.contains("\"attachments\""));
        assert!(notebook.contains("\"cell-diagram.png\""));
    }

    #[test]
    fn image_output_metadata_is_restored_sorted_and_typed() {
        // Display metadata rides on the image's attributes; restored sorted and typed.
        let bytes = vec![1u8, 2, 3, 4];
        let mut bag = MediaBag::new();
        bag.insert("plot.png", Some("image/png".to_owned()), bytes);
        let mut options = WriterOptions::default();
        options.media = std::sync::Arc::new(bag);

        let image = Inline::Image(
            Box::new(Attr {
                id: String::new().into(),
                classes: Vec::new(),
                attributes: vec![
                    ("width".to_owned().into(), "320".to_owned().into()),
                    ("height".to_owned().into(), "240".to_owned().into()),
                    (
                        "needs_background".to_owned().into(),
                        "light".to_owned().into(),
                    ),
                ],
            }),
            Vec::new(),
            Box::new(carta_ast::Target {
                url: "plot.png".to_owned().into(),
                title: String::new().into(),
            }),
        );
        let document = Document {
            blocks: vec![Block::Div(
                Box::new(Attr {
                    id: String::new().into(),
                    classes: vec!["cell".to_owned().into(), "code".to_owned().into()],
                    attributes: Vec::new(),
                }),
                vec![
                    Block::CodeBlock(Box::default(), "plot()".to_owned().into()),
                    Block::Div(
                        Box::new(Attr {
                            id: String::new().into(),
                            classes: vec![
                                "output".to_owned().into(),
                                "display_data".to_owned().into(),
                            ],
                            attributes: Vec::new(),
                        }),
                        vec![Block::Para(vec![image])],
                    ),
                ],
            )],
            ..Document::default()
        };

        let notebook = IpynbWriter.write(&document, &options).expect("writes");
        assert!(notebook.contains("\"height\": 240"));
        assert!(notebook.contains("\"width\": 320"));
        assert!(notebook.contains("\"needs_background\": \"light\""));
        let height = notebook.find("\"height\"").expect("height key");
        let background = notebook
            .find("\"needs_background\"")
            .expect("background key");
        let width = notebook.find("\"width\"").expect("width key");
        assert!(
            height < background && background < width,
            "metadata keys are not sorted:\n{notebook}"
        );
    }

    #[test]
    fn image_output_without_metadata_has_an_empty_metadata_object() {
        let Json::Object(entries) =
            output_metadata(&[Block::Para(vec![Inline::Str("text".to_owned().into())])])
        else {
            panic!("expected object");
        };
        assert!(entries.is_empty());
    }

    #[test]
    fn cell_attachments_are_emitted_in_sorted_key_order() {
        let bytes = vec![9u8, 8, 7, 6];
        let mut bag = MediaBag::new();
        bag.insert("fig-a.png", Some("image/png".to_owned()), bytes.clone());
        bag.insert("fig-b.png", Some("image/png".to_owned()), bytes);
        let mut options = WriterOptions::default();
        options.media = std::sync::Arc::new(bag);

        // The cell references b before a; the emitted attachments object is keyed in sorted order.
        let reference = |name: &str| {
            Block::Para(vec![Inline::Image(
                Box::default(),
                Vec::new(),
                Box::new(carta_ast::Target {
                    url: name.to_owned().into(),
                    title: String::new().into(),
                }),
            )])
        };
        let document = Document {
            blocks: vec![Block::Div(
                Box::new(Attr {
                    id: "fig".to_owned().into(),
                    classes: vec!["cell".to_owned().into(), "markdown".to_owned().into()],
                    attributes: Vec::new(),
                }),
                vec![reference("fig-b.png"), reference("fig-a.png")],
            )],
            ..Document::default()
        };

        let notebook = IpynbWriter.write(&document, &options).expect("writes");
        let (_, attachments) = notebook
            .split_once("\"attachments\"")
            .expect("attachments object present");
        let a = attachments.find("fig-a.png").expect("a key present");
        let b = attachments.find("fig-b.png").expect("b key present");
        assert!(a < b, "attachments are not sorted:\n{notebook}");
    }

    #[test]
    fn metadata_attribute_values_are_typed() {
        let attributes = vec![
            ("collapsed".to_owned().into(), "true".to_owned().into()),
            ("count".to_owned().into(), "5".to_owned().into()),
            ("name".to_owned().into(), "hello".to_owned().into()),
            ("tags".to_owned().into(), "[\"a\",\"b\"]".to_owned().into()),
        ];
        let Json::Object(entries) = attribute_metadata(&attributes, &[]) else {
            panic!("expected object");
        };
        let mut rendered = String::new();
        Json::Object(entries).write_to(&mut rendered, 0);
        assert!(rendered.contains("\"collapsed\": true"));
        assert!(rendered.contains("\"count\": 5"));
        assert!(rendered.contains("\"name\": \"hello\""));
        assert!(rendered.contains("\"a\","));
    }

    #[test]
    fn notebook_metadata_drops_version_keys() {
        let mut jupyter = BTreeMap::new();
        jupyter.insert(
            "nbformat".to_owned().into(),
            MetaValue::MetaString("4".to_owned().into()),
        );
        jupyter.insert(
            "nbformat_minor".to_owned().into(),
            MetaValue::MetaString("5".to_owned().into()),
        );
        jupyter.insert("kept".to_owned().into(), MetaValue::MetaBool(true));
        let mut meta = BTreeMap::new();
        meta.insert("jupyter".to_owned().into(), MetaValue::MetaMap(jupyter));

        let Json::Object(pairs) = notebook_metadata(&meta) else {
            panic!("expected object");
        };
        assert_eq!(pairs.len(), 1);
        let mut rendered = String::new();
        Json::Object(pairs).write_to(&mut rendered, 0);
        assert!(rendered.contains("\"kept\": true"));
        assert!(!rendered.contains("nbformat"));
    }

    #[test]
    fn control_characters_use_short_or_unicode_escapes() {
        let mut out = String::new();
        escape_string("a\"\\\n\t\u{8}\u{c}\u{1}/<", &mut out);
        assert_eq!(out, "\"a\\\"\\\\\\n\\t\\u0008\\u000c\\u0001/<\"");
    }

    #[test]
    fn generated_ids_are_stable_and_distinct() {
        let mut counter = 0;
        let first = next_id("", "alpha", &mut counter);
        let second = next_id("", "beta", &mut counter);
        assert_ne!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(first.as_bytes().get(14), Some(&b'4'));

        let mut again = 0;
        assert_eq!(next_id("", "alpha", &mut again), first);
    }
}
