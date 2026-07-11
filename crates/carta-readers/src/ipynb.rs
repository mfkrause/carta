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
/// string, a boolean a `MetaBool`); arrays and objects recurse. A number that is integer-valued —
/// whether written as an integer or as a float like `3.0` — reads as a plain integer; a fractional
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
/// cell carries — a code cell's image outputs, a markdown cell's attachments — are lifted into
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
/// otherwise read back as a number, a boolean, or the empty attribute — an all-digit run such as
/// `007`, the literal `true`/`false`, or `""` — is wrapped in double quotes so the distinction
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
/// representation wins, taken in MIME-name order; otherwise structured JSON — `application/json` or
/// any `+json` structured-syntax type — then plain text, HTML, LaTeX, and Markdown are tried in that
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
mod tests {
    use super::*;
    use carta_core::MediaBag;

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

    fn read_media(input: &str) -> (Document, MediaBag) {
        IpynbReader
            .read_media(input, &ReaderOptions::default())
            .expect("notebook input parses")
    }

    fn jupyter(document: &Document) -> &BTreeMap<carta_ast::Text, MetaValue> {
        match document.meta.get("jupyter") {
            Some(MetaValue::MetaMap(map)) => map,
            _ => panic!("expected a jupyter metadata map"),
        }
    }

    #[test]
    fn empty_notebook_exposes_only_version_metadata() {
        let document = read(r#"{"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#);
        assert!(document.blocks.is_empty());
        let map = jupyter(&document);
        assert_eq!(
            map.get("nbformat"),
            Some(&MetaValue::MetaString("4".to_owned().into()))
        );
        assert_eq!(
            map.get("nbformat_minor"),
            Some(&MetaValue::MetaString("5".to_owned().into()))
        );
    }

    #[test]
    fn missing_minor_version_defaults_to_zero() {
        let document = read(r#"{"cells": [], "metadata": {}, "nbformat": 4}"#);
        assert_eq!(
            jupyter(&document).get("nbformat_minor"),
            Some(&MetaValue::MetaString("0".to_owned().into()))
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
            Some(&MetaValue::MetaString("3".to_owned().into()))
        );
        assert_eq!(
            map.get("aint"),
            Some(&MetaValue::MetaString("7".to_owned().into()))
        );
        assert_eq!(map.get("abool"), Some(&MetaValue::MetaBool(true)));
        assert_eq!(
            map.get("anull"),
            Some(&MetaValue::MetaString(carta_ast::Text::default()))
        );
        assert_eq!(
            map.get("alist"),
            Some(&MetaValue::MetaList(vec![
                MetaValue::MetaString("1".to_owned().into()),
                MetaValue::MetaString("two".to_owned().into()),
                MetaValue::MetaString("3".to_owned().into()),
            ]))
        );
        let Some(MetaValue::MetaMap(nested)) = map.get("amap") else {
            panic!("expected a nested map");
        };
        assert_eq!(
            nested.get("a"),
            Some(&MetaValue::MetaString("2".to_owned().into()))
        );
        assert_eq!(
            nested.get("z"),
            Some(&MetaValue::MetaString("1".to_owned().into()))
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
                ("execution_count".into(), "5".into()),
                ("scrolled".into(), "true".into()),
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
            vec![("execution_count".into(), "5".into())]
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
            vec![("ename".into(), "E".into()), ("evalue".into(), "v".into()),]
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
    fn image_output_bytes_are_lifted_into_the_media_bag() {
        let (document, media) = read_media(
            r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        // The tree references the image by its content-addressed name...
        let Some(Block::Div(_, body)) = first_output(&document) else {
            panic!("expected an output div");
        };
        let Some(Block::Para(inlines)) = body.first() else {
            panic!("expected a paragraph");
        };
        let Some(Inline::Image(_, _, target)) = inlines.first() else {
            panic!("expected an image");
        };
        let name = "22f545ac6b50163ce39bac49094c3f64e0858403.png";
        assert_eq!(target.url, name);
        // ...and the bag holds exactly that name, with the decoded bytes and their MIME type.
        assert_eq!(media.len(), 1);
        let item = media.get(name).expect("image is in the bag");
        assert_eq!(item.mime.as_deref(), Some("image/png"));
        assert_eq!(
            item.bytes,
            carta_core::media::base64_decode("iVBORw0KGgoAAAANSUhEUg==").unwrap()
        );
    }

    #[test]
    fn svg_output_is_stored_as_its_source_bytes() {
        let (_, media) = read_media(
            r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/svg+xml": ["<svg/>"]},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        let name = "1c3ba3b813e1080e9721846f23a21c09e5c3fd27.svg";
        let item = media.get(name).expect("svg is in the bag");
        assert_eq!(item.mime.as_deref(), Some("image/svg+xml"));
        assert_eq!(item.bytes, b"<svg/>");
    }

    #[test]
    fn identical_image_outputs_share_one_bag_entry() {
        let (_, media) = read_media(
            r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="},
                  "metadata": {}},
                 {"output_type": "display_data", "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        // Content addressing means equal bytes collapse to a single entry.
        assert_eq!(media.len(), 1);
    }

    #[test]
    fn markdown_attachment_bytes_are_lifted_into_the_media_bag() {
        let (_, media) = read_media(
            r#"{"cells": [{"cell_type": "markdown", "id": "cell9", "metadata": {},
               "attachments": {"a.png": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="}},
               "source": ["![alt](attachment:a.png)"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        // The attachment is keyed by the same cell-scoped name the image reference resolves to.
        let item = media.get("cell9-a.png").expect("attachment is in the bag");
        assert_eq!(item.mime.as_deref(), Some("image/png"));
        assert_eq!(
            item.bytes,
            carta_core::media::base64_decode("iVBORw0KGgoAAAANSUhEUg==").unwrap()
        );
    }

    #[test]
    fn attachment_without_a_cell_id_uses_the_bare_reference() {
        let (_, media) = read_media(
            r#"{"cells": [{"cell_type": "markdown", "metadata": {},
               "attachments": {"a.png": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="}},
               "source": ["![alt](attachment:a.png)"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
        );
        assert!(media.contains("a.png"));
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
                ("height".into(), "50".into()),
                ("needs_background".into(), "light".into()),
                ("width".into(), "100".into()),
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
        assert_eq!(attr.attributes, vec![("format".into(), "text/html".into())]);
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
