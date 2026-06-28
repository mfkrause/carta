//! Notebook reader: parses a Jupyter notebook (`.ipynb`, nbformat v4) into the document model.
//!
//! A notebook is a JSON document with a `cells` array, a `metadata` object, and the `nbformat`
//! version pair. Each cell becomes a `Div` carrying the classes `cell` and the cell kind, the
//! cell's `id` as its identifier, and the cell's `metadata` (plus a code cell's `execution_count`)
//! as ordered attributes:
//!
//! - A **markdown** cell's source is parsed as Markdown and its blocks become the `Div` body. The
//!   embedded Markdown honors the reader's full extension set, so tables, fenced code, task lists,
//!   and the rest are recognized exactly as configured. An image whose URL begins with
//!   `attachment:` refers to the cell's inline attachments; the prefix is stripped to leave the
//!   bare reference.
//! - A **code** cell yields a `CodeBlock` of its source (tagged with the kernel language) followed
//!   by one `Div` per execution output: a `stream` (stdout/stderr text), an `execute_result` or
//!   `display_data` (the richest renderable bundle in the output's `data`), or an `error`
//!   (its traceback).
//! - A **raw** cell yields a single `RawBlock` whose target format is read from the cell's
//!   `raw_mimetype` metadata, falling back to its `format` metadata.
//!
//! Notebook-level metadata is exposed under a single `jupyter` metadata key, with the `nbformat`
//! and `nbformat_minor` versions folded in. Image payloads in the outputs are referenced by a
//! content-addressed file name (the SHA-1 of the decoded bytes); the bytes themselves are not
//! retained in the tree.

use std::collections::BTreeMap;

use carta_ast::{ApiVersion, Attr, Block, Document, Format, Inline, MetaValue, Target};
use carta_core::{Error, Reader, ReaderOptions, Result};
use serde_json::Value;

use crate::commonmark::CommonmarkReader;

/// Parses a notebook document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct IpynbReader;

impl Reader for IpynbReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let notebook: Value = serde_json::from_str(input)?;
        let nbformat = notebook
            .get("nbformat")
            .and_then(Value::as_i64)
            .unwrap_or(4);
        if nbformat < 4 {
            return Err(Error::UnsupportedFormat(format!(
                "notebook format version {nbformat} (only nbformat 4 and later are read)"
            )));
        }
        let nbformat_minor = notebook
            .get("nbformat_minor")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let language = notebook_language(&notebook);
        let meta = build_meta(&notebook, nbformat, nbformat_minor);

        let mut blocks = Vec::new();
        if let Some(Value::Array(cells)) = notebook.get("cells") {
            for cell in cells {
                if let Some(block) = cell_to_block(cell, &language, options)? {
                    blocks.push(block);
                }
            }
        }
        Ok(Document {
            api_version: ApiVersion::default(),
            meta,
            blocks,
        })
    }
}

/// The kernel language, taken from `metadata.kernelspec.language`. Code without a declared language
/// is tagged `python`.
fn notebook_language(notebook: &Value) -> String {
    notebook
        .get("metadata")
        .and_then(|metadata| metadata.get("kernelspec"))
        .and_then(|kernelspec| kernelspec.get("language"))
        .and_then(Value::as_str)
        .unwrap_or("python")
        .to_owned()
}

/// The document metadata: every notebook-level metadata entry, with the `nbformat`/`nbformat_minor`
/// versions added, all wrapped under a single `jupyter` key.
fn build_meta(notebook: &Value, nbformat: i64, nbformat_minor: i64) -> BTreeMap<String, MetaValue> {
    let mut jupyter: BTreeMap<String, MetaValue> = BTreeMap::new();
    if let Some(Value::Object(metadata)) = notebook.get("metadata") {
        for (key, value) in metadata {
            jupyter.insert(key.clone(), meta_value(value));
        }
    }
    jupyter.insert(
        "nbformat".to_owned(),
        MetaValue::MetaString(nbformat.to_string()),
    );
    jupyter.insert(
        "nbformat_minor".to_owned(),
        MetaValue::MetaString(nbformat_minor.to_string()),
    );
    let mut meta = BTreeMap::new();
    meta.insert("jupyter".to_owned(), MetaValue::MetaMap(jupyter));
    meta
}

/// Convert a JSON value to a metadata value. Scalars become strings (a null becomes the empty
/// string, a boolean a `MetaBool`); arrays and objects recurse. A number is rendered without a
/// redundant fractional part, so an integer-valued float reads as an integer.
fn meta_value(value: &Value) -> MetaValue {
    match value {
        Value::Null => MetaValue::MetaString(String::new()),
        Value::Bool(flag) => MetaValue::MetaBool(*flag),
        Value::Number(number) => MetaValue::MetaString(meta_number(number)),
        Value::String(text) => MetaValue::MetaString(text.clone()),
        Value::Array(items) => MetaValue::MetaList(items.iter().map(meta_value).collect()),
        Value::Object(map) => MetaValue::MetaMap(
            map.iter()
                .map(|(key, value)| (key.clone(), meta_value(value)))
                .collect(),
        ),
    }
}

/// Render a number for metadata: an integer-valued float drops its trailing `.0` so it reads as an
/// integer, while a fractional value keeps its decimals.
fn meta_number(number: &serde_json::Number) -> String {
    if number.is_f64() {
        let rendered = number.to_string();
        match rendered.strip_suffix(".0") {
            Some(integer) => integer.to_owned(),
            None => rendered,
        }
    } else {
        number.to_string()
    }
}

/// Convert one cell into its `Div`, or `None` for an unrecognized cell kind.
fn cell_to_block(cell: &Value, language: &str, options: &ReaderOptions) -> Result<Option<Block>> {
    let Some(kind) = cell.get("cell_type").and_then(Value::as_str) else {
        return Ok(None);
    };
    let attr = cell_attr(cell, kind);
    let block = match kind {
        "markdown" => Block::Div(attr, markdown_cell_blocks(cell, options)?),
        "code" => Block::Div(attr, code_cell_blocks(cell, language)),
        "raw" => Block::Div(attr, vec![raw_cell_block(cell)]),
        _ => return Ok(None),
    };
    Ok(Some(block))
}

/// The cell's attributes: its `id`, the classes `cell` and the cell kind, then a code cell's
/// `execution_count` followed by the cell's own metadata entries in key order.
fn cell_attr(cell: &Value, kind: &str) -> Attr {
    let id = cell
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let classes = vec!["cell".to_owned(), kind.to_owned()];
    let mut attributes = Vec::new();
    if kind == "code"
        && let Some(count) = cell.get("execution_count").and_then(Value::as_i64)
    {
        attributes.push(("execution_count".to_owned(), count.to_string()));
    }
    if let Some(Value::Object(metadata)) = cell.get("metadata") {
        for (key, value) in metadata {
            attributes.push((key.clone(), attribute_value(value)));
        }
    }
    Attr {
        id,
        classes,
        attributes,
    }
}

/// Render a JSON value as an attribute string. A non-string takes its compact JSON form. A string
/// keeps its own text, except that one which would otherwise read back as a number or boolean — an
/// all-digit run such as `007`, or the literal `true`/`false` — is wrapped in double quotes so the
/// distinction between the string and the scalar survives the round trip.
fn attribute_value(value: &Value) -> String {
    match value {
        Value::String(text) if is_integer_literal(text) || text == "true" || text == "false" => {
            format!("\"{text}\"")
        }
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Whether `text` is a non-empty run of ASCII digits (`^[0-9]+$`).
fn is_integer_literal(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// A markdown cell's blocks: its source parsed as Markdown with the reader's extensions, then with
/// `attachment:` image references rewritten to the cell-scoped name `<cell id>-<reference>`. A cell
/// that carries no `id` field leaves the bare reference in place.
fn markdown_cell_blocks(cell: &Value, options: &ReaderOptions) -> Result<Vec<Block>> {
    let source = multiline_text(cell.get("source"));
    let mut markdown_options = ReaderOptions::default();
    markdown_options.extensions = options.extensions;
    // A notebook's markdown cells are written in the broad Markdown dialect (greedy paragraphs),
    // not strict CommonMark: nested emphasis nests strong outside emph, a bare URI or email becomes
    // a classed autolink, an ordered list's marker style and start are normalized unless the
    // fancy-list and start-number extensions ask otherwise, and a raw HTML block carries no trailing
    // newline.
    markdown_options.greedy_paragraphs = true;
    let mut blocks = CommonmarkReader.read(&source, &markdown_options)?.blocks;
    let prefix = cell
        .get("id")
        .map(|id| format!("{}-", id.as_str().unwrap_or_default()));
    strip_attachment_blocks(&mut blocks, prefix.as_deref());
    Ok(blocks)
}

/// A code cell's blocks: a `CodeBlock` of its source tagged with the kernel language, then one
/// block per renderable output.
fn code_cell_blocks(cell: &Value, language: &str) -> Vec<Block> {
    let source = multiline_text(cell.get("source"));
    let source_attr = Attr {
        id: String::new(),
        classes: vec![language.to_owned()],
        attributes: Vec::new(),
    };
    let mut blocks = vec![Block::CodeBlock(source_attr, source)];
    if let Some(Value::Array(outputs)) = cell.get("outputs") {
        for output in outputs {
            if let Some(block) = output_to_block(output) {
                blocks.push(block);
            }
        }
    }
    blocks
}

/// A raw cell's block: a `RawBlock` whose format is read from the cell's media type, or `ipynb`
/// when none is declared. The media type is taken from `raw_mimetype`, falling back to `format` when
/// the former is absent.
fn raw_cell_block(cell: &Value) -> Block {
    let source = multiline_text(cell.get("source"));
    let metadata = cell.get("metadata");
    let mime = metadata
        .and_then(|metadata| metadata.get("raw_mimetype"))
        .or_else(|| metadata.and_then(|metadata| metadata.get("format")))
        .and_then(Value::as_str);
    let format = mime.map_or_else(|| "ipynb".to_owned(), format_from_mime);
    Block::RawBlock(Format(format), source)
}

/// Convert one execution output into its `Div`, or `None` for an unrecognized output kind.
fn output_to_block(output: &Value) -> Option<Block> {
    match output.get("output_type").and_then(Value::as_str)? {
        "stream" => Some(stream_output(output)),
        "execute_result" => Some(result_output(output, true)),
        "display_data" => Some(result_output(output, false)),
        "error" => Some(error_output(output)),
        _ => None,
    }
}

/// A `stream` output: a plain `CodeBlock` of the stream text inside a `Div` classed by the stream
/// name (stdout or stderr). Terminal control sequences in the text are removed.
fn stream_output(output: &Value) -> Block {
    let name = output
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("stdout");
    let text = strip_ansi(&multiline_text(output.get("text")));
    let attr = Attr {
        id: String::new(),
        classes: vec!["output".to_owned(), "stream".to_owned(), name.to_owned()],
        attributes: Vec::new(),
    };
    Block::Div(attr, vec![Block::CodeBlock(Attr::default(), text)])
}

/// An `execute_result` or `display_data` output: the richest renderable bundle from its `data`,
/// inside a `Div`. A result carries its `execution_count` as an attribute.
fn result_output(output: &Value, is_result: bool) -> Block {
    let kind = if is_result {
        "execute_result"
    } else {
        "display_data"
    };
    let mut attributes = Vec::new();
    if is_result && let Some(count) = output.get("execution_count").and_then(Value::as_i64) {
        attributes.push(("execution_count".to_owned(), count.to_string()));
    }
    let attr = Attr {
        id: String::new(),
        classes: vec!["output".to_owned(), kind.to_owned()],
        attributes,
    };
    Block::Div(
        attr,
        data_to_blocks(output.get("data"), output.get("metadata")),
    )
}

/// An `error` output: its traceback as a plain `CodeBlock` inside a `Div` carrying the exception
/// name and value. Terminal control sequences in the traceback are removed.
fn error_output(output: &Value) -> Block {
    let ename = output
        .get("ename")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let evalue = output
        .get("evalue")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let traceback = match output.get("traceback") {
        Some(Value::Array(lines)) => {
            let joined = lines
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n");
            format!("{joined}\n")
        }
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    };
    let attr = Attr {
        id: String::new(),
        classes: vec!["output".to_owned(), "error".to_owned()],
        attributes: vec![("ename".to_owned(), ename), ("evalue".to_owned(), evalue)],
    };
    Block::Div(
        attr,
        vec![Block::CodeBlock(Attr::default(), strip_ansi(&traceback))],
    )
}

/// Pick the richest renderable representation from an output's `data` bundle. An image (or PDF)
/// representation wins, taken in MIME-name order; otherwise structured JSON — `application/json` or
/// any `+json` structured-syntax type — then plain text, HTML, LaTeX, and Markdown are tried in that
/// order. Among several JSON representations the lowest MIME name is taken. An empty or absent bundle
/// yields no blocks.
fn data_to_blocks(data: Option<&Value>, metadata: Option<&Value>) -> Vec<Block> {
    let Some(Value::Object(data)) = data else {
        return Vec::new();
    };
    if let Some((mime, value)) = data.iter().find(|(mime, _)| is_image_like(mime)) {
        return vec![image_block(mime, value, metadata)];
    }
    if let Some((mime, value)) = data.iter().find(|(mime, _)| is_json_like(mime)) {
        return vec![non_image_block(mime, value)];
    }
    for mime in ["text/plain", "text/html", "text/latex", "text/markdown"] {
        if let Some(value) = data.get(mime) {
            return vec![non_image_block(mime, value)];
        }
    }
    Vec::new()
}

/// Render a non-image output representation. Structured JSON becomes a `json`-classed code block of
/// its compact form; plain text becomes a code block (control sequences removed); HTML, LaTeX, and
/// Markdown become raw passthrough blocks.
fn non_image_block(mime: &str, value: &Value) -> Block {
    if is_json_like(mime) {
        return Block::CodeBlock(
            Attr {
                id: String::new(),
                classes: vec!["json".to_owned()],
                attributes: Vec::new(),
            },
            value.to_string(),
        );
    }
    match mime {
        "text/html" => Block::RawBlock(Format("html".to_owned()), multiline_text(Some(value))),
        "text/latex" => Block::RawBlock(Format("latex".to_owned()), multiline_text(Some(value))),
        "text/markdown" => {
            Block::RawBlock(Format("markdown".to_owned()), multiline_text(Some(value)))
        }
        // The fallthrough is plain text; the preference list only routes the cases above here.
        _ => Block::CodeBlock(Attr::default(), strip_ansi(&multiline_text(Some(value)))),
    }
}

/// A `Para` holding a single image whose URL is the content-addressed file name of the decoded
/// payload. SVG is stored as its source text; every other type is base64-decoded to bytes, falling
/// back to the raw source bytes when the payload is not well-formed base64. Any entry the output's
/// `metadata` records under the chosen MIME type becomes an attribute on the image.
fn image_block(mime: &str, value: &Value, metadata: Option<&Value>) -> Block {
    let payload = multiline_text(Some(value));
    let bytes = if mime == "image/svg+xml" {
        payload.into_bytes()
    } else {
        base64_decode(&payload).unwrap_or_else(|| payload.into_bytes())
    };
    let name = format!("{}.{}", sha1_hex(&bytes), extension_for_mime(mime));
    Block::Para(vec![Inline::Image(
        image_attr(mime, metadata),
        Vec::new(),
        Target {
            url: name,
            title: String::new(),
        },
    )])
}

/// The image attributes drawn from an output's `metadata`: every key under the entry named for the
/// chosen MIME type, in sorted order, each value rendered as an attribute string.
fn image_attr(mime: &str, metadata: Option<&Value>) -> Attr {
    let mut attributes = Vec::new();
    if let Some(Value::Object(by_mime)) = metadata
        && let Some(Value::Object(entry)) = by_mime.get(mime)
    {
        for (key, value) in entry {
            attributes.push((key.clone(), attribute_value(value)));
        }
    }
    Attr {
        id: String::new(),
        classes: Vec::new(),
        attributes,
    }
}

/// Whether a MIME type denotes an image-like payload that is referenced as a file: any `image/*`
/// type, or PDF.
fn is_image_like(mime: &str) -> bool {
    mime.starts_with("image/") || mime == "application/pdf"
}

/// Whether a MIME type denotes structured JSON: the `application/json` type or any type whose
/// structured-syntax suffix is `+json` (for example `application/geo+json`).
fn is_json_like(mime: &str) -> bool {
    mime == "application/json" || mime.ends_with("+json")
}

/// The file extension for an image-like MIME type.
fn extension_for_mime(mime: &str) -> &str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        // An unrecognized image type falls back to its subtype, dropping any structured suffix.
        other => other
            .rsplit('/')
            .next()
            .and_then(|subtype| subtype.split('+').next())
            .unwrap_or(other),
    }
}

/// The raw-passthrough format name for a cell's `format` MIME type. A few media types map onto a
/// writer's short name; anything else is kept verbatim.
fn format_from_mime(mime: &str) -> String {
    match mime {
        "text/html" => "html",
        "text/latex" | "application/pdf" => "latex",
        "text/markdown" => "markdown",
        "text/restructuredtext" | "text/x-rst" => "rst",
        other => other,
    }
    .to_owned()
}

/// Join a JSON value that is either a single string or an array of string lines into one string.
/// Array elements are concatenated as stored (each line carries its own trailing newline).
fn multiline_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// Remove ANSI terminal control sequences from text. A control sequence introducer (`ESC [`) and
/// its parameter, intermediate, and final bytes are dropped; a stray escape is dropped on its own.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for byte in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&byte) {
                    break;
                }
            }
        }
    }
    out
}

/// Rewrite `attachment:` image references to their cell-scoped names throughout a block sequence.
/// `prefix` is the cell's name prefix (`<id>-`), or `None` for a cell without an `id`.
fn strip_attachment_blocks(blocks: &mut [Block], prefix: Option<&str>) {
    for block in blocks {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
                strip_attachment_inlines(inlines, prefix);
            }
            Block::LineBlock(lines) => {
                for line in lines {
                    strip_attachment_inlines(line, prefix);
                }
            }
            Block::BlockQuote(inner) | Block::Div(_, inner) => {
                strip_attachment_blocks(inner, prefix);
            }
            Block::OrderedList(_, items) | Block::BulletList(items) => {
                for item in items {
                    strip_attachment_blocks(item, prefix);
                }
            }
            Block::DefinitionList(items) => {
                for (term, definitions) in items {
                    strip_attachment_inlines(term, prefix);
                    for definition in definitions {
                        strip_attachment_blocks(definition, prefix);
                    }
                }
            }
            Block::Figure(_, caption, inner) => {
                strip_attachment_caption(caption, prefix);
                strip_attachment_blocks(inner, prefix);
            }
            Block::Table(table) => strip_attachment_table(table, prefix),
            Block::CodeBlock(..) | Block::RawBlock(..) | Block::HorizontalRule => {}
        }
    }
}

/// Rewrite `attachment:` image references throughout a table's caption and cells.
fn strip_attachment_table(table: &mut carta_ast::Table, prefix: Option<&str>) {
    strip_attachment_caption(&mut table.caption, prefix);
    let row_groups = std::iter::once(&mut table.head.rows)
        .chain(table.bodies.iter_mut().flat_map(|body| {
            std::iter::once(&mut body.head).chain(std::iter::once(&mut body.body))
        }))
        .chain(std::iter::once(&mut table.foot.rows));
    for rows in row_groups {
        for row in rows {
            for cell in &mut row.cells {
                strip_attachment_blocks(&mut cell.content, prefix);
            }
        }
    }
}

fn strip_attachment_caption(caption: &mut carta_ast::Caption, prefix: Option<&str>) {
    if let Some(short) = &mut caption.short {
        strip_attachment_inlines(short, prefix);
    }
    strip_attachment_blocks(&mut caption.long, prefix);
}

/// Rewrite `attachment:` image references to their cell-scoped names throughout an inline sequence.
fn strip_attachment_inlines(inlines: &mut [Inline], prefix: Option<&str>) {
    for inline in inlines {
        match inline {
            Inline::Image(_, alt, target) => {
                if let Some(bare) = target.url.strip_prefix("attachment:") {
                    target.url = match prefix {
                        Some(prefix) => format!("{prefix}{bare}"),
                        None => bare.to_owned(),
                    };
                }
                strip_attachment_inlines(alt, prefix);
            }
            Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Link(_, children, _)
            | Inline::Span(_, children) => strip_attachment_inlines(children, prefix),
            Inline::Cite(citations, children) => {
                for citation in citations {
                    strip_attachment_inlines(&mut citation.prefix, prefix);
                    strip_attachment_inlines(&mut citation.suffix, prefix);
                }
                strip_attachment_inlines(children, prefix);
            }
            Inline::Note(blocks) => strip_attachment_blocks(blocks, prefix),
            Inline::Str(_)
            | Inline::Code(..)
            | Inline::Space
            | Inline::SoftBreak
            | Inline::LineBreak
            | Inline::Math(..)
            | Inline::RawInline(..) => {}
        }
    }
}

/// Decode standard base64, ignoring inner whitespace. Returns `None` when the input — once
/// whitespace is removed — is not well-formed: a length that is not a multiple of four, a symbol
/// outside the alphabet, or padding that does not fall at the very end of the final quartet.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let symbols: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if symbols.is_empty() {
        return Some(Vec::new());
    }
    if !symbols.len().is_multiple_of(4) {
        return None;
    }
    let group_count = symbols.len() / 4;
    let mut out = Vec::with_capacity(group_count * 3);
    for (index, chunk) in symbols.chunks_exact(4).enumerate() {
        let last = index + 1 == group_count;
        let &[a, b, c, d] = chunk else { return None };
        let v0 = sextet(a)?;
        let v1 = sextet(b)?;
        out.push((v0 << 2) | (v1 >> 4));
        if c == b'=' {
            if !last || d != b'=' {
                return None;
            }
            continue;
        }
        let v2 = sextet(c)?;
        out.push(((v1 & 0x0f) << 4) | (v2 >> 2));
        if d == b'=' {
            if !last {
                return None;
            }
            continue;
        }
        let v3 = sextet(d)?;
        out.push(((v2 & 0x03) << 6) | v3);
    }
    Some(out)
}

/// The 6-bit value of one standard base64 alphabet symbol, or `None` for any other byte.
fn sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The SHA-1 digest of `data` as a 40-character lowercase hex string.
#[allow(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::many_single_char_names,
    reason = "the schedule and chunk indices are bounded by the fixed 80-word/64-byte block sizes; \
              the casts isolate the intended low bits; the single-letter names are the digest's own \
              working-variable notation"
)]
fn sha1_hex(data: &[u8]) -> String {
    const HEX: [u8; 16] = *b"0123456789abcdef";
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for block in message.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (index, word) in block.chunks_exact(4).enumerate() {
            w[index] = u32::from_be_bytes(word.try_into().unwrap_or([0; 4]));
        }
        for index in 16..80 {
            w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (index, &word) in w.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = String::with_capacity(40);
    for word in h {
        for byte in word.to_be_bytes() {
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 0x0f)] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(input: &str) -> Document {
        IpynbReader
            .read(input, &ReaderOptions::default())
            .expect("notebook input parses")
    }

    fn read_with(input: &str, extensions: carta_core::Extensions) -> Document {
        let mut options = ReaderOptions::default();
        options.extensions = extensions;
        IpynbReader.read(input, &options).expect("notebook parses")
    }

    fn jupyter(document: &Document) -> &BTreeMap<String, MetaValue> {
        match document.meta.get("jupyter") {
            Some(MetaValue::MetaMap(map)) => map,
            _ => panic!("expected a jupyter metadata map"),
        }
    }

    #[test]
    fn sha1_matches_known_vectors() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"The quick brown fox jumps over the lazy dog"),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
    }

    #[test]
    fn base64_decodes_and_ignores_whitespace() {
        assert_eq!(base64_decode("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(base64_decode("aGVs\nbG8="), Some(b"hello".to_vec()));
        assert_eq!(base64_decode(""), Some(Vec::new()));
        // A length that is not a multiple of four, a non-alphabet byte, and misplaced padding each
        // fail to decode rather than silently dropping or truncating input.
        assert_eq!(base64_decode("QQ"), None);
        assert_eq!(base64_decode("aGVsbG8@"), None);
        assert_eq!(base64_decode("a=VsbG8="), None);
    }

    #[test]
    fn empty_notebook_exposes_only_version_metadata() {
        let document = read(r#"{"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#);
        assert!(document.blocks.is_empty());
        let map = jupyter(&document);
        assert_eq!(
            map.get("nbformat"),
            Some(&MetaValue::MetaString("4".to_owned()))
        );
        assert_eq!(
            map.get("nbformat_minor"),
            Some(&MetaValue::MetaString("5".to_owned()))
        );
    }

    #[test]
    fn missing_minor_version_defaults_to_zero() {
        let document = read(r#"{"cells": [], "metadata": {}, "nbformat": 4}"#);
        assert_eq!(
            jupyter(&document).get("nbformat_minor"),
            Some(&MetaValue::MetaString("0".to_owned()))
        );
    }

    #[test]
    fn metadata_scalars_normalize_and_recurse() {
        let document = read(
            r#"{"cells": [], "metadata": {"afloat": 3.0, "aint": 7, "abool": true,
               "anull": null, "alist": [1, "two", 3.0], "amap": {"z": 1, "a": 2.0}},
               "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let map = jupyter(&document);
        assert_eq!(
            map.get("afloat"),
            Some(&MetaValue::MetaString("3".to_owned()))
        );
        assert_eq!(
            map.get("aint"),
            Some(&MetaValue::MetaString("7".to_owned()))
        );
        assert_eq!(map.get("abool"), Some(&MetaValue::MetaBool(true)));
        assert_eq!(
            map.get("anull"),
            Some(&MetaValue::MetaString(String::new()))
        );
        assert_eq!(
            map.get("alist"),
            Some(&MetaValue::MetaList(vec![
                MetaValue::MetaString("1".to_owned()),
                MetaValue::MetaString("two".to_owned()),
                MetaValue::MetaString("3".to_owned()),
            ]))
        );
        let Some(MetaValue::MetaMap(nested)) = map.get("amap") else {
            panic!("expected a nested map");
        };
        assert_eq!(
            nested.get("a"),
            Some(&MetaValue::MetaString("2".to_owned()))
        );
        assert_eq!(
            nested.get("z"),
            Some(&MetaValue::MetaString("1".to_owned()))
        );
    }

    #[test]
    fn markdown_cell_becomes_a_div_with_parsed_blocks() {
        let document = read(
            r##"{"cells": [{"cell_type": "markdown", "id": "m1", "metadata": {},
               "source": ["# Title\n", "\n", "text"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"##,
        );
        let Some(Block::Div(attr, blocks)) = document.blocks.first() else {
            panic!("expected a cell div");
        };
        assert_eq!(attr.id, "m1");
        assert_eq!(attr.classes, vec!["cell".to_owned(), "markdown".to_owned()]);
        assert!(matches!(blocks.first(), Some(Block::Header(1, _, _))));
        assert!(matches!(blocks.get(1), Some(Block::Para(_))));
    }

    #[test]
    fn markdown_cell_honors_forwarded_extensions() {
        // A pipe table is recognized only when the table extension is on, confirming the reader's
        // extensions reach the embedded Markdown.
        let input = r#"{"cells": [{"cell_type": "markdown", "metadata": {},
            "source": ["| a | b |\n|---|---|\n| 1 | 2 |\n"]}],
            "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#;
        let with_tables = read_with(input, carta_core::presets::GFM);
        let Some(Block::Div(_, blocks)) = with_tables.blocks.first() else {
            panic!("expected a cell div");
        };
        assert!(matches!(blocks.first(), Some(Block::Table(_))));

        let strict = read_with(input, carta_core::Extensions::empty());
        let Some(Block::Div(_, blocks)) = strict.blocks.first() else {
            panic!("expected a cell div");
        };
        assert!(!matches!(blocks.first(), Some(Block::Table(_))));
    }

    #[test]
    fn markdown_attachment_prefix_is_stripped_from_images() {
        let document = read(
            r#"{"cells": [{"cell_type": "markdown", "metadata": {},
               "attachments": {"a.png": {"image/png": "x"}},
               "source": ["![alt](attachment:a.png)"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let Some(Block::Div(_, blocks)) = document.blocks.first() else {
            panic!("expected a cell div");
        };
        let Some(Block::Para(inlines)) = blocks.first() else {
            panic!("expected a paragraph");
        };
        let Some(Inline::Image(_, _, target)) = inlines.first() else {
            panic!("expected an image");
        };
        // A cell without an `id` leaves the bare reference in place.
        assert_eq!(target.url, "a.png");
    }

    #[test]
    fn markdown_attachment_reference_is_scoped_to_the_cell_id() {
        let document = read(
            r#"{"cells": [{"cell_type": "markdown", "id": "cell9", "metadata": {},
               "attachments": {"a.png": {"image/png": "x"}},
               "source": ["![alt](attachment:a.png)"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let Some(Block::Div(_, blocks)) = document.blocks.first() else {
            panic!("expected a cell div");
        };
        let Some(Block::Para(inlines)) = blocks.first() else {
            panic!("expected a paragraph");
        };
        let Some(Inline::Image(_, _, target)) = inlines.first() else {
            panic!("expected an image");
        };
        // A cell with an `id` scopes the reference to that cell.
        assert_eq!(target.url, "cell9-a.png");
    }

    #[test]
    fn code_cell_emits_source_then_outputs() {
        let document = read(
            r#"{"cells": [{"cell_type": "code", "metadata": {"scrolled": true},
               "execution_count": 5, "source": ["import os\n", "print(1)"],
               "outputs": [
                 {"output_type": "stream", "name": "stdout", "text": ["hello\n"]},
                 {"output_type": "execute_result", "execution_count": 5,
                  "data": {"text/plain": ["42"]}, "metadata": {}},
                 {"output_type": "error", "ename": "E", "evalue": "v",
                  "traceback": ["line1", "line2"]}
               ]}],
               "metadata": {"kernelspec": {"language": "python"}},
               "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let Some(Block::Div(attr, blocks)) = document.blocks.first() else {
            panic!("expected a cell div");
        };
        assert_eq!(
            attr.attributes,
            vec![
                ("execution_count".to_owned(), "5".to_owned()),
                ("scrolled".to_owned(), "true".to_owned()),
            ]
        );
        // Source code block tagged with the language.
        let Some(Block::CodeBlock(source_attr, source)) = blocks.first() else {
            panic!("expected a source code block");
        };
        assert_eq!(source_attr.classes, vec!["python".to_owned()]);
        assert_eq!(source, "import os\nprint(1)");

        // Stream output.
        let Some(Block::Div(stream_attr, stream_body)) = blocks.get(1) else {
            panic!("expected a stream div");
        };
        assert_eq!(
            stream_attr.classes,
            vec![
                "output".to_owned(),
                "stream".to_owned(),
                "stdout".to_owned()
            ]
        );
        assert!(matches!(
            stream_body.first(),
            Some(Block::CodeBlock(_, text)) if text == "hello\n"
        ));

        // execute_result carries its execution count and renders text/plain as a code block.
        let Some(Block::Div(result_attr, result_body)) = blocks.get(2) else {
            panic!("expected a result div");
        };
        assert_eq!(
            result_attr.classes,
            vec!["output".to_owned(), "execute_result".to_owned()]
        );
        assert_eq!(
            result_attr.attributes,
            vec![("execution_count".to_owned(), "5".to_owned())]
        );
        assert!(matches!(
            result_body.first(),
            Some(Block::CodeBlock(_, text)) if text == "42"
        ));

        // error renders its joined traceback with a trailing newline.
        let Some(Block::Div(error_attr, error_body)) = blocks.get(3) else {
            panic!("expected an error div");
        };
        assert_eq!(
            error_attr.attributes,
            vec![
                ("ename".to_owned(), "E".to_owned()),
                ("evalue".to_owned(), "v".to_owned()),
            ]
        );
        assert!(matches!(
            error_body.first(),
            Some(Block::CodeBlock(_, text)) if text == "line1\nline2\n"
        ));
    }

    #[test]
    fn null_execution_count_yields_no_attribute() {
        let document = read(
            r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": null,
               "source": [], "outputs": []}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let Some(Block::Div(attr, _)) = document.blocks.first() else {
            panic!("expected a cell div");
        };
        assert!(attr.attributes.is_empty());
    }

    #[test]
    fn image_output_is_content_addressed() {
        // PNG bytes from base64 are hashed; SVG is hashed as its own text.
        let document = read(
            r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let Some(Block::Div(_, body)) = first_output(&document) else {
            panic!("expected an output div");
        };
        let Some(Block::Para(inlines)) = body.first() else {
            panic!("expected a paragraph");
        };
        let Some(Inline::Image(_, _, target)) = inlines.first() else {
            panic!("expected an image");
        };
        assert_eq!(target.url, "22f545ac6b50163ce39bac49094c3f64e0858403.png");

        let svg = read(
            r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/svg+xml": ["<svg/>"]},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let Some(Block::Div(_, body)) = first_output(&svg) else {
            panic!("expected an output div");
        };
        let Some(Block::Para(inlines)) = body.first() else {
            panic!("expected a paragraph");
        };
        let Some(Inline::Image(_, _, target)) = inlines.first() else {
            panic!("expected an image");
        };
        assert_eq!(target.url, "1c3ba3b813e1080e9721846f23a21c09e5c3fd27.svg");
    }

    #[test]
    fn image_wins_over_text_and_smaller_mime_wins_among_images() {
        let document = read(
            r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data",
                  "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg==", "image/jpeg": "iVBORw0KGgoAAAANSUhEUg==",
                           "text/plain": ["p"]},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let Some(Block::Div(_, body)) = first_output(&document) else {
            panic!("expected an output div");
        };
        let Some(Block::Para(inlines)) = body.first() else {
            panic!("expected a paragraph");
        };
        let Some(Inline::Image(_, _, target)) = inlines.first() else {
            panic!("expected an image");
        };
        // image/jpeg sorts before image/png and both before text/plain.
        assert_eq!(target.url, "22f545ac6b50163ce39bac49094c3f64e0858403.jpg");
    }

    #[test]
    fn image_output_metadata_becomes_sorted_attributes() {
        let document = read(
            r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="},
                  "metadata": {"image/png": {"width": 100, "height": 50, "needs_background": "light"}}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let Some(Block::Div(_, body)) = first_output(&document) else {
            panic!("expected an output div");
        };
        let Some(Block::Para(inlines)) = body.first() else {
            panic!("expected a paragraph");
        };
        let Some(Inline::Image(attr, _, _)) = inlines.first() else {
            panic!("expected an image");
        };
        assert_eq!(
            attr.attributes,
            vec![
                ("height".to_owned(), "50".to_owned()),
                ("needs_background".to_owned(), "light".to_owned()),
                ("width".to_owned(), "100".to_owned()),
            ]
        );
    }

    #[test]
    fn structured_json_output_is_compact_and_sorted() {
        let document = read(
            r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"application/json": {"z": 1, "a": 2.0}},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let Some(Block::Div(_, body)) = first_output(&document) else {
            panic!("expected an output div");
        };
        let Some(Block::CodeBlock(attr, text)) = body.first() else {
            panic!("expected a code block");
        };
        assert_eq!(attr.classes, vec!["json".to_owned()]);
        assert_eq!(text, r#"{"a":2.0,"z":1}"#);
    }

    #[test]
    fn raw_cell_maps_format_to_writer_name() {
        let document = read(
            r#"{"cells": [
                 {"cell_type": "raw", "metadata": {"format": "text/html"}, "source": ["<b>x</b>"]},
                 {"cell_type": "raw", "metadata": {}, "source": ["plain"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let Some(Block::Div(attr, body)) = document.blocks.first() else {
            panic!("expected a raw cell div");
        };
        assert_eq!(
            attr.attributes,
            vec![("format".to_owned(), "text/html".to_owned())]
        );
        assert!(matches!(
            body.first(),
            Some(Block::RawBlock(Format(name), text)) if name == "html" && text == "<b>x</b>"
        ));
        // No declared format falls back to the notebook's own format name.
        let Some(Block::Div(_, body)) = document.blocks.get(1) else {
            panic!("expected a raw cell div");
        };
        assert!(matches!(
            body.first(),
            Some(Block::RawBlock(Format(name), _)) if name == "ipynb"
        ));
    }

    #[test]
    fn unknown_cell_kinds_are_dropped() {
        let document = read(
            r#"{"cells": [{"cell_type": "heading", "level": 2, "metadata": {}, "source": ["H"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        assert!(document.blocks.is_empty());
    }

    #[test]
    fn terminal_control_sequences_are_removed_from_text_outputs() {
        // A control byte is invalid inside a JSON string, so the escape is carried in its JSON
        // numeric form. The escape token is assembled from a backslash char here so this source
        // holds no literal control byte: a backslash followed by the escape code's hex digits.
        let esc = format!("{}u001b", '\\');
        let input = format!(
            r#"{{"cells": [{{"cell_type": "code", "metadata": {{}}, "execution_count": 1,
               "source": [], "outputs": [
                 {{"output_type": "stream", "name": "stdout",
                  "text": ["{esc}[31mred{esc}[0m"]}}]}}],
               "metadata": {{}}, "nbformat": 4, "nbformat_minor": 5}}"#
        );
        let document = read(&input);
        let Some(Block::Div(_, body)) = first_output(&document) else {
            panic!("expected an output div");
        };
        assert!(matches!(
            body.first(),
            Some(Block::CodeBlock(_, text)) if text == "red"
        ));
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(
            IpynbReader
                .read("not json", &ReaderOptions::default())
                .is_err()
        );
        assert!(IpynbReader.read("", &ReaderOptions::default()).is_err());
    }

    #[test]
    fn pre_v4_notebook_is_an_error_not_a_panic() {
        let result = IpynbReader.read(
            r#"{"nbformat": 3, "nbformat_minor": 0, "worksheets": []}"#,
            &ReaderOptions::default(),
        );
        assert!(matches!(result, Err(Error::UnsupportedFormat(_))));
    }

    /// The body of the first output div of the first code cell.
    fn first_output(document: &Document) -> Option<&Block> {
        let Some(Block::Div(_, blocks)) = document.blocks.first() else {
            return None;
        };
        blocks.get(1)
    }
}
