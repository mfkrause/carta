//! Read Jupyter notebook cells, metadata, and embedded resources.

use std::collections::BTreeMap;

use carta_ast::{
    ApiVersion, Attr, Block, Document, Format, Inline, MetaValue, Target, ToCompactString,
};
use carta_core::media::{base64_decode, content_addressed_name};
use carta_core::{Error, MediaBag, Reader, ReaderOptions, Result};
use serde_json::{Map, Value};

use crate::commonmark::CommonmarkReader;

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
            .and_then(Value::as_number)
            .and_then(|number| meta_number(number).parse::<i64>().ok())
            .ok_or_else(|| notebook_error("notebook nbformat must be an integer"))?;
        if nbformat < 3 {
            return Err(Error::UnsupportedFormat(format!(
                "notebook format version {nbformat} (only nbformat 3 and later are read)"
            )));
        }
        let nbformat_minor = notebook
            .get("nbformat_minor")
            .and_then(Value::as_number)
            .and_then(|number| meta_number(number).parse::<i64>().ok())
            .unwrap_or(0);
        let language = notebook_language(&notebook);
        let meta = build_meta(&notebook, nbformat, nbformat_minor);

        let mut media = MediaBag::new();
        let mut blocks = Vec::new();
        let groups = if nbformat == 3 {
            notebook
                .get("worksheets")
                .and_then(Value::as_array)
                .ok_or_else(|| notebook_error("notebook worksheets must be an array"))?
                .as_slice()
        } else {
            std::slice::from_ref(&notebook)
        };
        for group in groups {
            let cells = group
                .get("cells")
                .and_then(Value::as_array)
                .ok_or_else(|| notebook_error("notebook cells must be an array"))?;
            for cell in cells {
                blocks.push(cell_to_block(
                    cell,
                    nbformat == 3,
                    &language,
                    options,
                    &mut media,
                )?);
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

fn notebook_error(message: impl Into<String>) -> Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into()).into()
}

fn normalize_legacy_cell(cell: &Value) -> Result<Value> {
    let mut cell = cell
        .as_object()
        .cloned()
        .ok_or_else(|| notebook_error("notebook cells must be objects"))?;
    let kind = cell
        .get("cell_type")
        .and_then(Value::as_str)
        .ok_or_else(|| notebook_error("notebook cells need a cell_type"))?
        .to_owned();
    match kind.as_str() {
        "code" => {
            let source = cell
                .remove("input")
                .ok_or_else(|| notebook_error("code cells need input"))?;
            cell.insert("source".to_owned(), source);
            if let Some(count) = cell.remove("prompt_number") {
                cell.insert("execution_count".to_owned(), count);
            }
            let mut metadata = match cell.remove("metadata") {
                Some(Value::Object(metadata)) => metadata,
                None => Map::new(),
                Some(_) => return Err(notebook_error("cell metadata must be an object")),
            };
            for field in ["collapsed", "language"] {
                if let Some(value) = cell.remove(field) {
                    metadata.entry(field.to_owned()).or_insert(value);
                }
            }
            cell.insert("metadata".to_owned(), Value::Object(metadata));
            let outputs = cell
                .get_mut("outputs")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| notebook_error("code cell outputs must be an array"))?;
            for output in outputs {
                *output = normalize_legacy_output(std::mem::take(output))?;
            }
        }
        "heading" => {
            let source = multiline_text(cell.get("source"));
            cell.insert("source".to_owned(), Value::String(format!("# {source}")));
        }
        "html" => {
            cell.insert("cell_type".to_owned(), Value::String("markdown".to_owned()));
        }
        "markdown" | "raw" => {}
        _ => {
            return Err(notebook_error(format!(
                "unknown notebook cell type: {kind}"
            )));
        }
    }
    Ok(Value::Object(cell))
}

fn normalize_legacy_output(output: Value) -> Result<Value> {
    let Value::Object(output) = output else {
        return Err(notebook_error("notebook outputs must be objects"));
    };
    let kind = match output.get("output_type").and_then(Value::as_str) {
        Some("stream") => "stream",
        Some("pyout") => "execute_result",
        Some("display_data") => "display_data",
        Some("pyerr") => "error",
        Some(kind) => {
            return Err(notebook_error(format!(
                "unknown notebook output type: {kind}"
            )));
        }
        None => return Err(notebook_error("notebook outputs need an output_type")),
    };
    let rich = matches!(kind, "execute_result" | "display_data");
    let mut normalized = Map::new();
    let mut data = Map::new();
    for (key, value) in output {
        if key == "output_type" {
            continue;
        }
        if rich {
            let mime = match key.as_str() {
                "text" => Some("text/plain"),
                "html" => Some("text/html"),
                "latex" => Some("text/latex"),
                "png" => Some("image/png"),
                "jpeg" => Some("image/jpeg"),
                "svg" => Some("image/svg+xml"),
                "pdf" => Some("application/pdf"),
                "json" => Some("application/json"),
                "javascript" => Some("application/javascript"),
                mime if mime.contains('/') => Some(mime),
                _ => None,
            };
            if let Some(mime) = mime {
                let value = if is_json_like(mime) {
                    let text = multiline_text(Some(&value));
                    serde_json::from_str(&text).unwrap_or(Value::String(text))
                } else {
                    value
                };
                data.insert(mime.to_owned(), value);
                continue;
            }
        }
        let key = match key.as_str() {
            "prompt_number" => "execution_count".to_owned(),
            "stream" if kind == "stream" => "name".to_owned(),
            _ => key,
        };
        normalized.insert(key, value);
    }
    normalized.insert("output_type".to_owned(), Value::String(kind.to_owned()));
    if rich {
        normalized.insert("data".to_owned(), Value::Object(data));
    }
    Ok(Value::Object(normalized))
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

/// Converts JSON to metadata without rounding decimal values.
/// Null becomes an empty string; booleans retain their type; arrays and objects recurse.
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

/// Renders exact decimal metadata numbers with bounded zero padding.
fn meta_number(number: &serde_json::Number) -> String {
    let literal = number.to_string();
    decimal_number(&literal, true).unwrap_or(literal)
}

/// Renders JSON numbers exactly, keeping a fractional part when its scale is negative.
fn json_number(number: &serde_json::Number) -> String {
    let literal = number.to_string();
    decimal_number(&literal, false).unwrap_or(literal)
}

fn decimal_number(literal: &str, fold_integers: bool) -> Option<String> {
    let (mantissa, exponent) = match literal.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<isize>().ok()?),
        None => (literal, 0),
    };
    let negative = mantissa.starts_with('-');
    let mantissa = mantissa.strip_prefix('-').unwrap_or(mantissa);
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let exponent = exponent.checked_sub(isize::try_from(fraction.len()).ok()?)?;
    let digits = format!("{whole}{fraction}");
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(
            if fold_integers || exponent >= 0 {
                "0"
            } else {
                "0.0"
            }
            .to_owned(),
        );
    }
    let leading = exponent
        .checked_add(isize::try_from(digits.len()).ok()?)?
        .checked_sub(1)?;
    let significant = digits.trim_end_matches('0');
    let trailing = isize::try_from(digits.len() - significant.len()).ok()?;
    let integer = exponent >= 0 || (fold_integers && exponent.checked_add(trailing)? >= 0);
    // Keep extreme exponents compact to bound zero padding.
    let fixed = (integer && exponent <= 4096) || (-1..=6).contains(&leading);
    let mut rendered = if !fixed {
        let first = significant.get(..1)?;
        let rest = significant.get(1..)?;
        format!(
            "{first}.{}e{leading}",
            if rest.is_empty() { "0" } else { rest }
        )
    } else if leading < 0 {
        format!("0.{significant}")
    } else {
        let point = usize::try_from(leading).ok()?.checked_add(1)?;
        if point >= significant.len() {
            let mut text = format!("{significant}{}", "0".repeat(point - significant.len()));
            if !integer {
                text.push_str(".0");
            }
            text
        } else {
            format!(
                "{}.{}",
                significant.get(..point)?,
                significant.get(point..)?
            )
        }
    };
    if negative {
        rendered.insert(0, '-');
    }
    Some(rendered)
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

/// Converts one cell into a div and captures its embedded resources.
fn cell_to_block(
    cell: &Value,
    legacy: bool,
    language: &str,
    options: &ReaderOptions,
    media: &mut MediaBag,
) -> Result<Block> {
    let normalized;
    let cell = if legacy {
        normalized = normalize_legacy_cell(cell)?;
        &normalized
    } else {
        cell
    };
    let kind = cell
        .get("cell_type")
        .and_then(Value::as_str)
        .ok_or_else(|| notebook_error("notebook cell_type must be a string"))?;
    let attr = cell_attr(
        cell,
        if legacy && kind == "heading" {
            "markdown"
        } else {
            kind
        },
    )?;
    let block = match kind {
        "markdown" => Block::Div(Box::new(attr), markdown_cell_blocks(cell, options, media)?),
        "code" => Block::Div(Box::new(attr), code_cell_blocks(cell, language, media)?),
        "raw" => Block::Div(Box::new(attr), vec![raw_cell_block(cell)]),
        "heading" if legacy => {
            let level = cell
                .get("level")
                .and_then(Value::as_i64)
                .filter(|level| *level > 0)
                .ok_or_else(|| notebook_error("heading cells need a positive level"))?;
            let mut blocks = markdown_cell_blocks(cell, options, media)?;
            if let Some(Block::Header(current, _, _)) = blocks.first_mut() {
                *current = level;
            }
            Block::Div(Box::new(attr), blocks)
        }
        _ => {
            return Err(notebook_error(format!(
                "unknown notebook cell type: {kind}"
            )));
        }
    };
    Ok(block)
}

/// The cell's attributes: its `id`, the classes `cell` and the cell kind, then a code cell's
/// `execution_count` followed by the cell's own metadata entries in key order.
fn cell_attr(cell: &Value, kind: &str) -> Result<Attr> {
    let id = cell
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let classes = vec!["cell".to_owned(), kind.to_owned()];
    let mut attributes = Vec::new();
    if kind == "code"
        && let Some(value) = cell.get("execution_count")
        && let Some(count) = execution_count(value)?
    {
        attributes.push(("execution_count".to_owned(), count));
    }
    if let Some(Value::Object(metadata)) = cell.get("metadata") {
        for (key, value) in metadata {
            attributes.push((key.clone(), attribute_value(value)));
        }
    }
    Ok(Attr {
        id: id.into(),
        classes: classes.into_iter().map(Into::into).collect(),
        attributes: attributes
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
    })
}

fn execution_count(value: &Value) -> Result<Option<String>> {
    if value.is_null() {
        return Ok(None);
    }
    let count = value.as_number().map(meta_number);
    match count {
        Some(count) if is_integer_literal(&count) => Ok(Some(count)),
        _ => Err(notebook_error(
            "execution_count must be a nonnegative integer or null",
        )),
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
fn code_cell_blocks(cell: &Value, language: &str, media: &mut MediaBag) -> Result<Vec<Block>> {
    let source = multiline_text(cell.get("source"));
    let source_attr = Attr {
        id: carta_ast::Text::default(),
        classes: vec![language.into()],
        attributes: Vec::new(),
    };
    let mut blocks = vec![Block::CodeBlock(Box::new(source_attr), source.into())];
    if let Some(outputs) = cell.get("outputs") {
        let outputs = outputs
            .as_array()
            .ok_or_else(|| notebook_error("code cell outputs must be an array"))?;
        for output in outputs {
            blocks.push(output_to_block(output, media)?);
        }
    }
    Ok(blocks)
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

/// Converts one execution output into a div and captures its embedded resources.
fn output_to_block(output: &Value, media: &mut MediaBag) -> Result<Block> {
    let kind = output
        .get("output_type")
        .and_then(Value::as_str)
        .ok_or_else(|| notebook_error("notebook output_type must be a string"))?;
    Ok(match kind {
        "stream" => {
            let name = output
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| notebook_error("stream outputs need a string name"))?;
            stream_output(output, name)
        }
        "execute_result" => result_output(output, true, media)?,
        "display_data" => result_output(output, false, media)?,
        "error" => error_output(output),
        _ => {
            return Err(notebook_error(format!(
                "unknown notebook output type: {kind}"
            )));
        }
    })
}

/// A `stream` output: a plain `CodeBlock` of the stream text inside a `Div` classed by the stream
/// name (stdout or stderr). Terminal control sequences in the text are removed.
fn stream_output(output: &Value, name: &str) -> Block {
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
fn result_output(output: &Value, is_result: bool, media: &mut MediaBag) -> Result<Block> {
    let kind = if is_result {
        "execute_result"
    } else {
        "display_data"
    };
    let mut attributes = Vec::new();
    if is_result {
        let count = output
            .get("execution_count")
            .ok_or_else(|| notebook_error("result outputs need an execution_count"))?;
        if let Some(count) = execution_count(count)? {
            attributes.push(("execution_count".to_owned(), count));
        }
    }
    let attr = Attr {
        id: carta_ast::Text::default(),
        classes: vec!["output".into(), kind.into()],
        attributes: attributes
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
    };
    Ok(Block::Div(
        Box::new(attr),
        data_to_blocks(output.get("data"), output.get("metadata"), media),
    ))
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
