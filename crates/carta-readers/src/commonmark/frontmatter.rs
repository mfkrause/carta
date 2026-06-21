//! Document front matter: a leading YAML metadata block (`yaml_metadata_block`) or a percent-line
//! title block (`pandoc_title_block`). Either populates the document's `meta` map from material at
//! the very top of the input and removes it from the body.
//!
//! Both are recognized only when their extension is enabled and only at the first line of the
//! document. [`extract`] returns the metadata together with the body that remains once the front
//! matter is stripped.

use std::collections::BTreeMap;

use carta_ast::MetaValue;
use carta_core::{Error, Extension, ReaderOptions, Result};

use super::inline::parse_meta_inlines;
use super::parse_meta_blocks;
use super::yaml::{self, Scalar, Yaml};

/// Document metadata together with the body that remains once any front matter is stripped. A `None`
/// body means the input carried no front matter and should be parsed unchanged.
pub(crate) struct FrontMatter {
    pub(crate) meta: BTreeMap<String, MetaValue>,
    pub(crate) body: Option<String>,
}

/// Extract document metadata from a leading YAML or title block, if either applies. Returns the
/// metadata and, when a block is consumed, the remaining body text. Malformed YAML is an error.
pub(crate) fn extract(normalized: &str, options: &ReaderOptions) -> Result<FrontMatter> {
    if options.extensions.contains(Extension::YamlMetadataBlock)
        && let Some(front) = yaml_block(normalized, options)?
    {
        return Ok(front);
    }
    if options.extensions.contains(Extension::PandocTitleBlock)
        && let Some((meta, body)) = title_block(normalized, options)
    {
        return Ok(FrontMatter {
            meta,
            body: Some(body),
        });
    }
    Ok(FrontMatter {
        meta: BTreeMap::new(),
        body: None,
    })
}

/// Try to consume a leading YAML metadata block. `Ok(None)` means the input does not open one (fall
/// through); `Ok(Some(..))` carries the metadata and body; `Err` marks malformed YAML.
fn yaml_block(normalized: &str, options: &ReaderOptions) -> Result<Option<FrontMatter>> {
    let lines: Vec<&str> = normalized.split('\n').collect();
    // A fence line is `---` (open or close) or `...` (close) with optional trailing whitespace;
    // leading whitespace disqualifies it, so the comparison trims only the end.
    if !lines.first().is_some_and(|line| line.trim_end() == "---") {
        return Ok(None);
    }
    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|&(_, &line)| {
            let line = line.trim_end();
            line == "---" || line == "..."
        })
        .map(|(i, _)| i);
    // The closing fence is mandatory; without it the opening `---` is an ordinary thematic break.
    let Some(close) = close else {
        return Ok(None);
    };
    let content = lines.get(1..close).unwrap_or(&[]).join("\n");
    match yaml::parse(&content) {
        Ok(yaml::TopLevel::Mapping(entries)) => {
            let meta = entries
                .into_iter()
                .map(|(key, value)| (key, yaml_to_meta(value, options)))
                .collect();
            let body = lines.get(close + 1..).unwrap_or(&[]).join("\n");
            Ok(Some(FrontMatter {
                meta,
                body: Some(body),
            }))
        }
        // Valid YAML that is not a mapping does not become metadata; the fences fall through.
        Ok(yaml::TopLevel::NotMapping) => Ok(None),
        Err(()) => Err(Error::InvalidMetadata(
            "could not parse YAML metadata block".to_owned(),
        )),
    }
}

/// Parse a standalone YAML metadata file (no surrounding fences) into a metadata map, reusing the
/// same scalar→[`MetaValue`] mapping as a leading metadata block. A non-mapping file contributes no
/// metadata.
///
/// # Errors
/// [`Error::InvalidMetadata`] if the content is not valid YAML.
pub fn parse_metadata_yaml(
    content: &str,
    options: &ReaderOptions,
) -> Result<BTreeMap<String, MetaValue>> {
    match yaml::parse(content) {
        Ok(yaml::TopLevel::Mapping(entries)) => Ok(entries
            .into_iter()
            .map(|(key, value)| (key, yaml_to_meta(value, options)))
            .collect()),
        Ok(yaml::TopLevel::NotMapping) => Ok(BTreeMap::new()),
        Err(()) => Err(Error::InvalidMetadata(
            "could not parse YAML metadata file".to_owned(),
        )),
    }
}

/// Parse a standalone JSON metadata file into a metadata map. String and number scalars are parsed as
/// inline Markdown (matching the YAML path), booleans become [`MetaValue::MetaBool`], and arrays and
/// objects recurse.
///
/// # Errors
/// [`Error::InvalidMetadata`] if the content is not a valid JSON object.
pub fn parse_metadata_json(
    content: &str,
    options: &ReaderOptions,
) -> Result<BTreeMap<String, MetaValue>> {
    let value: serde_json::Value = serde_json::from_str(content).map_err(|error| {
        Error::InvalidMetadata(format!("could not parse JSON metadata file: {error}"))
    })?;
    match value {
        serde_json::Value::Object(map) => Ok(map
            .into_iter()
            .map(|(key, value)| (key, json_to_meta(value, options)))
            .collect()),
        _ => Err(Error::InvalidMetadata(
            "JSON metadata file must be an object".to_owned(),
        )),
    }
}

/// Convert a parsed JSON value into a metadata value, mirroring the YAML scalar typing: strings and
/// numbers parse as inline Markdown, booleans stay boolean, and arrays/objects recurse.
fn json_to_meta(value: serde_json::Value, options: &ReaderOptions) -> MetaValue {
    match value {
        serde_json::Value::Null => MetaValue::MetaString(String::new()),
        serde_json::Value::Bool(b) => MetaValue::MetaBool(b),
        serde_json::Value::Number(n) => MetaValue::MetaInlines(parse_meta_inlines(
            &n.to_string(),
            options.extensions,
            options.greedy_paragraphs,
        )),
        serde_json::Value::String(s) => MetaValue::MetaInlines(parse_meta_inlines(
            &s,
            options.extensions,
            options.greedy_paragraphs,
        )),
        serde_json::Value::Array(items) => MetaValue::MetaList(
            items
                .into_iter()
                .map(|item| json_to_meta(item, options))
                .collect(),
        ),
        serde_json::Value::Object(map) => MetaValue::MetaMap(
            map.into_iter()
                .map(|(key, item)| (key, json_to_meta(item, options)))
                .collect(),
        ),
    }
}

/// Convert a parsed YAML value into a metadata value, recursing through mappings and sequences.
fn yaml_to_meta(value: Yaml, options: &ReaderOptions) -> MetaValue {
    match value {
        Yaml::Mapping(entries) => MetaValue::MetaMap(
            entries
                .into_iter()
                .map(|(key, value)| (key, yaml_to_meta(value, options)))
                .collect(),
        ),
        Yaml::Sequence(items) => MetaValue::MetaList(
            items
                .into_iter()
                .map(|v| yaml_to_meta(v, options))
                .collect(),
        ),
        Yaml::Scalar(scalar) => scalar_to_meta(scalar, options),
    }
}

/// Resolve a scalar to a metadata value. Plain scalars are typed (null, boolean, number, or inline
/// text); quoted scalars are always inline text; block scalars are block- or inline-level depending
/// on whether their text keeps a trailing newline.
fn scalar_to_meta(scalar: Scalar, options: &ReaderOptions) -> MetaValue {
    match scalar {
        Scalar::Plain(text) => plain_scalar_to_meta(&text, options),
        Scalar::Quoted(text) => MetaValue::MetaInlines(parse_meta_inlines(
            &text,
            options.extensions,
            options.greedy_paragraphs,
        )),
        Scalar::Block(text) => text_to_meta(&text, options),
    }
}

fn plain_scalar_to_meta(text: &str, options: &ReaderOptions) -> MetaValue {
    if text.is_empty() || is_null(text) {
        return MetaValue::MetaString(String::new());
    }
    if let Some(value) = as_bool(text) {
        return MetaValue::MetaBool(value);
    }
    if let Some(canonical) = yaml::canonicalize_number(text) {
        return MetaValue::MetaInlines(parse_meta_inlines(
            &canonical,
            options.extensions,
            options.greedy_paragraphs,
        ));
    }
    MetaValue::MetaInlines(parse_meta_inlines(
        text,
        options.extensions,
        options.greedy_paragraphs,
    ))
}

/// Text whose trailing newline survived block-scalar chomping is parsed as block-level markdown;
/// otherwise it is inline markdown.
fn text_to_meta(text: &str, options: &ReaderOptions) -> MetaValue {
    if text.ends_with('\n') {
        MetaValue::MetaBlocks(parse_meta_blocks(
            text,
            options.extensions,
            options.greedy_paragraphs,
        ))
    } else {
        MetaValue::MetaInlines(parse_meta_inlines(
            text,
            options.extensions,
            options.greedy_paragraphs,
        ))
    }
}

fn is_null(text: &str) -> bool {
    matches!(text, "null" | "Null" | "NULL" | "~")
}

/// The unquoted YAML 1.1 boolean tokens.
fn as_bool(text: &str) -> Option<bool> {
    match text {
        "y" | "Y" | "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on" | "On" | "ON" => {
            Some(true)
        }
        "n" | "N" | "no" | "No" | "NO" | "false" | "False" | "FALSE" | "off" | "Off" | "OFF" => {
            Some(false)
        }
        _ => None,
    }
}

/// Try to consume a leading title block: up to three percent-introduced fields (title, author(s),
/// date) at the top of the document. Returns the metadata and the remaining body.
fn title_block(
    normalized: &str,
    options: &ReaderOptions,
) -> Option<(BTreeMap<String, MetaValue>, String)> {
    let lines: Vec<&str> = normalized.split('\n').collect();
    if !lines.first().is_some_and(|line| line.starts_with('%')) {
        return None;
    }
    let labels = ["title", "author", "date"];
    let mut meta = BTreeMap::new();
    let mut idx = 0;
    for label in labels {
        let Some(&line) = lines.get(idx) else { break };
        if !line.starts_with('%') {
            break;
        }
        let mut field = vec![strip_field_marker(line).to_owned()];
        idx += 1;
        while let Some(&cont) = lines.get(idx) {
            if cont.starts_with('%') || cont.trim().is_empty() || !starts_with_space(cont) {
                break;
            }
            field.push(cont.trim().to_owned());
            idx += 1;
        }
        insert_field(&mut meta, label, &field, options);
    }
    let body = lines.get(idx..).unwrap_or(&[]).join("\n");
    Some((meta, body))
}

/// Add one title-block field to the metadata. Title and date are inline markdown (continuation lines
/// join as soft breaks) and are omitted when empty; the author field is always a list, split on `;`
/// and on continuation lines.
fn insert_field(
    meta: &mut BTreeMap<String, MetaValue>,
    label: &str,
    field: &[String],
    options: &ReaderOptions,
) {
    if label == "author" {
        let mut authors = Vec::new();
        for line in field {
            for chunk in line.split(';') {
                authors.push(MetaValue::MetaInlines(parse_meta_inlines(
                    chunk.trim(),
                    options.extensions,
                    options.greedy_paragraphs,
                )));
            }
        }
        meta.insert("author".to_owned(), MetaValue::MetaList(authors));
        return;
    }
    let text = field.join("\n");
    if !text.trim().is_empty() {
        meta.insert(
            label.to_owned(),
            MetaValue::MetaInlines(parse_meta_inlines(
                &text,
                options.extensions,
                options.greedy_paragraphs,
            )),
        );
    }
}

/// Strip a field's leading `%` and the single optional space that follows it.
fn strip_field_marker(line: &str) -> &str {
    let rest = line.strip_prefix('%').unwrap_or(line);
    rest.strip_prefix(' ').unwrap_or(rest)
}

fn starts_with_space(line: &str) -> bool {
    line.starts_with([' ', '\t'])
}

#[cfg(test)]
mod tests {
    use crate::commonmark::CommonmarkReader;
    use carta_ast::{Block, MetaValue};
    use carta_core::{Extension, Extensions, Reader, ReaderOptions};

    fn read(input: &str) -> carta_ast::Document {
        let mut options = ReaderOptions::default();
        let mut extensions = Extensions::empty();
        extensions.insert(Extension::YamlMetadataBlock);
        options.extensions = extensions;
        CommonmarkReader
            .read(input, &options)
            .expect("reader should not fail")
    }

    #[test]
    fn fence_lines_tolerate_trailing_whitespace() {
        // Trailing spaces or tabs on either fence still delimit the block, and the body that
        // follows is not part of the metadata.
        let document = read("---   \ntitle: T\n---\t\n\nBody\n");
        assert_eq!(
            document.meta.get("title"),
            Some(&MetaValue::MetaInlines(vec![carta_ast::Inline::Str(
                "T".to_owned()
            )]))
        );
        assert!(matches!(document.blocks.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn closing_ellipsis_fence_tolerates_trailing_whitespace() {
        let document = read("---\ntitle: T\n...  \n\nBody\n");
        assert!(document.meta.contains_key("title"));
    }

    #[test]
    fn an_indented_opening_fence_is_not_front_matter() {
        // A fence must start at the line's first column; leading whitespace disqualifies it.
        let document = read("   ---\ntitle: T\n---\n\nBody\n");
        assert!(document.meta.is_empty());
    }
}
