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
//! and `nbformat_minor` versions folded in. An output's image payload is referenced by a
//! content-addressed file name (the SHA-1 of the decoded bytes) and a markdown cell's attachment by
//! a cell-scoped name; in both cases the bytes are lifted out of the tree into the media bag, which
//! [`IpynbReader::read_media`] returns alongside the document.

use std::collections::BTreeMap;

use carta_ast::{
    ApiVersion, Attr, Block, Document, Format, Inline, MetaValue, Target, ToCompactString,
};
use carta_core::media::{base64_decode, content_addressed_name};
use carta_core::{Error, MediaBag, Reader, ReaderOptions, Result};
use serde_json::Value;

use crate::commonmark::CommonmarkReader;
use crate::numeric::general_decimal;

/// Parses a notebook document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct IpynbReader;

impl Reader for IpynbReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        self.read_media(input, options)
            .map(|(document, _)| document)
    }

    fn read_media(&self, input: &str, options: &ReaderOptions) -> Result<(Document, MediaBag)> {
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

        let mut media = MediaBag::new();
        let mut blocks = Vec::new();
        if let Some(Value::Array(cells)) = notebook.get("cells") {
            for cell in cells {
                if let Some(block) = cell_to_block(cell, &language, options, &mut media)? {
                    blocks.push(block);
                }
            }
        }
        let document = Document {
            api_version: ApiVersion::default(),
            meta: meta.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            blocks,
        };
        Ok((document, media))
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
        MetaValue::MetaString(nbformat.to_compact_string()),
    );
    jupyter.insert(
        "nbformat_minor".to_owned(),
        MetaValue::MetaString(nbformat_minor.to_compact_string()),
    );
    let mut meta = BTreeMap::new();
    meta.insert(
        "jupyter".to_owned(),
        MetaValue::MetaMap(jupyter.into_iter().map(|(k, v)| (k.into(), v)).collect()),
    );
    meta
}

/// Convert a JSON value to a metadata value. Scalars become strings (a null becomes the empty
/// string, a boolean a `MetaBool`); arrays and objects recurse. A number that is integer-valued
/// (whether written as an integer or as a float like `3.0`) reads as a plain integer; a fractional
/// number keeps the general decimal form, falling to scientific notation for very small or very
/// large magnitudes.
fn meta_value(value: &Value) -> MetaValue {
    match value {
        Value::Null => MetaValue::MetaString(carta_ast::Text::default()),
        Value::Bool(flag) => MetaValue::MetaBool(*flag),
        Value::Number(number) => MetaValue::MetaString(meta_number(number).into()),
        Value::String(text) => MetaValue::MetaString(text.clone().into()),
        Value::Array(items) => MetaValue::MetaList(items.iter().map(meta_value).collect()),
        Value::Object(map) => MetaValue::MetaMap(
            map.iter()
                .map(|(key, value)| (key.clone().into(), meta_value(value)))
                .collect(),
        ),
    }
}

/// Render a number for notebook-level metadata: an integer-valued number reads as a plain integer,
/// while a fractional value is rendered in the general decimal form (scientific notation outside the
/// magnitude range `[0.1, 10^7)`).
fn meta_number(number: &serde_json::Number) -> String {
    if let Some(integer) = number.as_i64() {
        return integer.to_string();
    }
    if let Some(integer) = number.as_u64() {
        return integer.to_string();
    }
    match number.as_f64() {
        Some(value) if value.is_finite() && value.fract() == 0.0 => integer_string(value),
        Some(value) => general_decimal(value),
        None => number.to_string(),
    }
}

/// Render a number as a JSON scalar would appear in a serialized bundle: an integer keeps its exact
/// digits, while a fractional number takes the general decimal form. Unlike [`meta_number`], a value
/// written with a fractional part such as `3.0` keeps that part (it is not folded to an integer).
fn json_number(number: &serde_json::Number) -> String {
    if let Some(integer) = number.as_i64() {
        return integer.to_string();
    }
    if let Some(integer) = number.as_u64() {
        return integer.to_string();
    }
    match number.as_f64() {
        Some(value) => general_decimal(value),
        None => number.to_string(),
    }
}

/// Render an integer-valued floating-point number as a bare integer (no fractional part, no
/// exponent). Negative zero renders as `0`.
fn integer_string(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    format!("{value}")
}

/// Serialize a JSON value to compact text, rendering numbers as [`json_number`] does. Object keys are
/// emitted in sorted order (the parser stores them sorted), so the output is deterministic.
fn json_render(value: &Value) -> String {
    let mut out = String::new();
    json_write(value, &mut out);
    out
}

fn json_write(value: &Value, out: &mut String) {
    match value {
        Value::Number(number) => out.push_str(&json_number(number)),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                json_write(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                json_write(item, out);
            }
            out.push('}');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Convert one cell into its `Div`, or `None` for an unrecognized cell kind. Any image bytes the
/// cell carries (a code cell's image outputs, a markdown cell's attachments) are lifted into
/// `media`.
fn cell_to_block(
    cell: &Value,
    language: &str,
    options: &ReaderOptions,
    media: &mut MediaBag,
) -> Result<Option<Block>> {
    let Some(kind) = cell.get("cell_type").and_then(Value::as_str) else {
        return Ok(None);
    };
    let attr = cell_attr(cell, kind);
    let block = match kind {
        "markdown" => Block::Div(Box::new(attr), markdown_cell_blocks(cell, options, media)?),
        "code" => Block::Div(Box::new(attr), code_cell_blocks(cell, language, media)),
        "raw" => Block::Div(Box::new(attr), vec![raw_cell_block(cell)]),
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
        id: id.into(),
        classes: classes.into_iter().map(Into::into).collect(),
        attributes: attributes
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
    }
}

/// Render a JSON value as an attribute string. A non-string takes its compact JSON form, with numbers
/// rendered as [`json_number`] does. A string keeps its own text, except that one which would
/// otherwise read back as a number, a boolean, or the empty attribute (an all-digit run such as
/// `007`, the literal `true`/`false`, or `""`) is wrapped in double quotes so the distinction
/// between the string and the scalar survives the round trip.
fn attribute_value(value: &Value) -> String {
    match value {
        Value::String(text)
            if text.is_empty() || is_integer_literal(text) || text == "true" || text == "false" =>
        {
            format!("\"{text}\"")
        }
        Value::String(text) => text.clone(),
        other => json_render(other),
    }
}

/// Whether `text` is a non-empty run of ASCII digits (`^[0-9]+$`).
fn is_integer_literal(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// A markdown cell's blocks: its source parsed as Markdown with the reader's extensions, then with
/// `attachment:` image references rewritten to the cell-scoped name `<cell id>-<reference>`. A cell
/// that carries no `id` field leaves the bare reference in place.
fn markdown_cell_blocks(
    cell: &Value,
    options: &ReaderOptions,
    media: &mut MediaBag,
) -> Result<Vec<Block>> {
    let source = multiline_text(cell.get("source"));
    let mut markdown_options = ReaderOptions::default();
    markdown_options.extensions = options.extensions;
    // Markdown cells use the broad dialect (greedy paragraphs), not strict CommonMark.
    markdown_options.greedy_paragraphs = true;
    let mut blocks = CommonmarkReader.read(&source, &markdown_options)?.blocks;
    let prefix = cell
        .get("id")
        .map(|id| format!("{}-", id.as_str().unwrap_or_default()));
    capture_attachments(cell, prefix.as_deref(), media);
    let prefix = prefix.as_deref().unwrap_or_default();
    carta_core::walk::for_each_image_target(&mut blocks, &mut |target| {
        if let Some(bare) = target.url.strip_prefix("attachment:") {
            target.url = format!("{prefix}{bare}").into();
        }
    });
    Ok(blocks)
}

/// Lift a markdown cell's inline attachments into the media bag. Each entry in the cell's
/// `attachments` object maps a reference name to a MIME→payload bundle; its bytes are stored under
/// the cell-scoped name (`<prefix><reference>`) the `attachment:` references resolve to, so a later
/// extract or re-embed step finds them. The bundle's image representation is preferred; failing that,
/// its first entry in key order is taken.
fn capture_attachments(cell: &Value, prefix: Option<&str>, media: &mut MediaBag) {
    let Some(Value::Object(attachments)) = cell.get("attachments") else {
        return;
    };
    for (reference, bundle) in attachments {
        let Value::Object(by_mime) = bundle else {
            continue;
        };
        let chosen = by_mime
            .iter()
            .find(|(mime, _)| is_image_like(mime))
            .or_else(|| by_mime.iter().next());
        let Some((mime, payload)) = chosen else {
            continue;
        };
        let name = match prefix {
            Some(prefix) => format!("{prefix}{reference}"),
            None => reference.clone(),
        };
        media.insert(name, Some(mime.clone()), decode_payload(mime, payload));
    }
}

/// A code cell's blocks: a `CodeBlock` of its source tagged with the kernel language, then one
/// block per renderable output.
fn code_cell_blocks(cell: &Value, language: &str, media: &mut MediaBag) -> Vec<Block> {
    let source = multiline_text(cell.get("source"));
    let source_attr = Attr {
        id: carta_ast::Text::default(),
        classes: vec![language.into()],
        attributes: Vec::new(),
    };
    let mut blocks = vec![Block::CodeBlock(Box::new(source_attr), source.into())];
    if let Some(Value::Array(outputs)) = cell.get("outputs") {
        for output in outputs {
            if let Some(block) = output_to_block(output, media) {
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
    Block::RawBlock(Format(format.into()), source.into())
}

/// Convert one execution output into its `Div`, or `None` for an unrecognized output kind. An image
/// output's bytes are lifted into `media`.
fn output_to_block(output: &Value, media: &mut MediaBag) -> Option<Block> {
    match output.get("output_type").and_then(Value::as_str)? {
        "stream" => Some(stream_output(output)),
        "execute_result" => Some(result_output(output, true, media)),
        "display_data" => Some(result_output(output, false, media)),
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
        id: carta_ast::Text::default(),
        classes: vec!["output".into(), "stream".into(), name.into()],
        attributes: Vec::new(),
    };
    Block::Div(
        Box::new(attr),
        vec![Block::CodeBlock(Box::default(), text.into())],
    )
}

/// An `execute_result` or `display_data` output: the richest renderable bundle from its `data`,
/// inside a `Div`. A result carries its `execution_count` as an attribute.
fn result_output(output: &Value, is_result: bool, media: &mut MediaBag) -> Block {
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
        id: carta_ast::Text::default(),
        classes: vec!["output".into(), kind.into()],
        attributes: attributes
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
    };
    Block::Div(
        Box::new(attr),
        data_to_blocks(output.get("data"), output.get("metadata"), media),
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
        id: carta_ast::Text::default(),
        classes: vec!["output".into(), "error".into()],
        attributes: vec![
            ("ename".into(), ename.into()),
            ("evalue".into(), evalue.into()),
        ],
    };
    Block::Div(
        Box::new(attr),
        vec![Block::CodeBlock(
            Box::default(),
            strip_ansi(&traceback).into(),
        )],
    )
}

/// Pick the richest renderable representation from an output's `data` bundle. An image (or PDF)
/// representation wins, taken in MIME-name order; otherwise structured JSON (`application/json` or
/// any `+json` structured-syntax type), then plain text, HTML, LaTeX, and Markdown are tried in that
/// order. Among several JSON representations the lowest MIME name is taken. An empty or absent bundle
/// yields no blocks.
fn data_to_blocks(
    data: Option<&Value>,
    metadata: Option<&Value>,
    media: &mut MediaBag,
) -> Vec<Block> {
    let Some(Value::Object(data)) = data else {
        return Vec::new();
    };
    if let Some((mime, value)) = data.iter().find(|(mime, _)| is_image_like(mime)) {
        return vec![image_block(mime, value, metadata, media)];
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
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["json".into()],
                attributes: Vec::new(),
            }),
            json_render(value).into(),
        );
    }
    match mime {
        "text/html" => Block::RawBlock(Format("html".into()), multiline_text(Some(value)).into()),
        "text/latex" => Block::RawBlock(Format("latex".into()), multiline_text(Some(value)).into()),
        "text/markdown" => Block::RawBlock(
            Format("markdown".into()),
            multiline_text(Some(value)).into(),
        ),
        // The fallthrough is plain text; the preference list only routes the cases above here.
        _ => Block::CodeBlock(
            Box::default(),
            strip_ansi(&multiline_text(Some(value))).into(),
        ),
    }
}

/// A `Para` holding a single image whose URL is the content-addressed file name of the decoded
/// payload, whose bytes are lifted into `media` under that same name. Any entry the output's
/// `metadata` records under the chosen MIME type becomes an attribute on the image.
fn image_block(mime: &str, value: &Value, metadata: Option<&Value>, media: &mut MediaBag) -> Block {
    let bytes = decode_payload(mime, value);
    let name = content_addressed_name(mime, &bytes);
    media.insert(name.clone(), Some(mime.to_owned()), bytes);
    Block::Para(vec![Inline::Image(
        Box::new(image_attr(mime, metadata)),
        Vec::new(),
        Box::new(Target {
            url: name.into(),
            title: carta_ast::Text::default(),
        }),
    )])
}

/// The raw bytes of an image payload. An SVG representation is its own UTF-8 source; every other type
/// is base64-decoded, falling back to the raw source bytes when the payload is not well-formed
/// base64.
fn decode_payload(mime: &str, value: &Value) -> Vec<u8> {
    let payload = multiline_text(Some(value));
    if mime == "image/svg+xml" {
        payload.into_bytes()
    } else {
        base64_decode(&payload).unwrap_or_else(|| payload.into_bytes())
    }
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
        id: carta_ast::Text::default(),
        classes: Vec::new(),
        attributes: attributes
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
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

/// The raw-passthrough format name for a cell's `format` MIME type. A few media types map onto a
/// writer's short name; anything else is kept verbatim.
fn format_from_mime(mime: &str) -> String {
    match mime {
        "text/html" => "html",
        "text/latex" | "application/pdf" => "latex",
        "text/markdown" => "markdown",
        "text/restructuredtext" | "text/x-rst" => "rst",
        "text/asciidoc" => "asciidoc",
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

#[cfg(test)]
mod tests;
