//! The data-loading calls (`read`, `csv`, `json`, `toml`, `yaml`, `xml`): a named file's text
//! turned into the code-mode value the rest of the document computes with.

use super::{Integer, Value};
use crate::xml::{Element, Node};
use crate::yaml::{Scalar, Yaml};

/// How deep a loaded XML document may nest before the rest of it is left unread.
const MAX_NESTING: usize = 64;

/// Read `text` as the format the call `name` loads, or report `None` when the name loads no data.
pub(super) fn load(name: &str, text: &str) -> Option<Value> {
    match name {
        "read" => Some(Value::Str(text.to_string())),
        "csv" => Some(rows(text)),
        "json" => Some(json(text)),
        "toml" => Some(Value::Dict(toml(text))),
        "yaml" => Some(yaml(text)),
        "xml" => Some(xml(text)),
        _ => None,
    }
}

/// A mapping with its keys in the order a loaded document exposes them: sorted by name.
fn mapping(mut entries: Vec<(String, Value)>) -> Value {
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Value::Dict(entries)
}

/// Comma-separated records as an array of arrays of strings.
fn rows(text: &str) -> Value {
    Value::Array(
        crate::csv::parse_records(text, ',', true)
            .into_iter()
            .map(|record| Value::Array(record.into_iter().map(Value::Str).collect()))
            .collect(),
    )
}

/// A JSON document as the value it describes, or nothing when it does not parse.
fn json(text: &str) -> Value {
    serde_json::from_str::<serde_json::Value>(text).map_or(Value::Nothing, from_json)
}

fn from_json(value: serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Nothing,
        serde_json::Value::Bool(flag) => Value::Bool(flag),
        serde_json::Value::Number(number) => number.as_i64().map_or_else(
            || Value::Number(number.as_f64().unwrap_or_default(), String::new()),
            |number| Value::Int(Integer::from(number)),
        ),
        serde_json::Value::String(text) => Value::Str(text),
        serde_json::Value::Array(items) => Value::Array(items.into_iter().map(from_json).collect()),
        serde_json::Value::Object(fields) => mapping(
            fields
                .into_iter()
                .map(|(key, held)| (key, from_json(held)))
                .collect(),
        ),
    }
}

/// A YAML document as the value it describes, or nothing when it does not parse.
fn yaml(text: &str) -> Value {
    crate::yaml::parse_document(text).map_or(Value::Nothing, from_yaml)
}

fn from_yaml(value: Yaml) -> Value {
    match value {
        Yaml::Mapping(entries) => mapping(
            entries
                .into_iter()
                .map(|(key, held)| (key, from_yaml(held)))
                .collect(),
        ),
        Yaml::Sequence(items) => Value::Array(items.into_iter().map(from_yaml).collect()),
        Yaml::Scalar(Scalar::Quoted(text) | Scalar::Block(text)) => Value::Str(text),
        Yaml::Scalar(Scalar::Plain(text)) => match crate::yaml::canonicalize_number(&text) {
            Some(canonical) => scalar(&canonical),
            None => scalar(&text),
        },
    }
}

/// An XML document as the array of its root elements. Only elements live at document level, so the
/// declaration and the whitespace around them are not part of the result.
fn xml(text: &str) -> Value {
    let document = crate::xml::parse_tolerant(text.as_bytes(), MAX_NESTING);
    Value::Array(
        children(&document)
            .into_iter()
            .filter(|node| matches!(node, Value::Dict(_)))
            .collect(),
    )
}

/// The nodes of an element: a child element becomes a `tag`/`attrs`/`children` record, character
/// data a string.
fn children(element: &Element) -> Vec<Value> {
    element
        .children
        .iter()
        .map(|node| match node {
            Node::Text(text) => Value::Str(text.clone()),
            Node::Element(child) => Value::Dict(vec![
                ("tag".to_string(), Value::Str(child.name.clone())),
                (
                    "attrs".to_string(),
                    mapping(
                        child
                            .attrs
                            .iter()
                            .map(|(key, held)| (key.clone(), Value::Str(held.clone())))
                            .collect(),
                    ),
                ),
                ("children".to_string(), Value::Array(children(child))),
            ]),
        })
        .collect()
}

/// A TOML document as the table it describes.
///
/// The grammar read here is the one data files use: `key = value` pairs under `[table]` and
/// `[a.b]` headers, with strings, numbers, booleans, inline arrays, and inline tables as values.
fn toml(text: &str) -> Vec<(String, Value)> {
    let mut root: Vec<(String, Value)> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = strip_toml_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let header = header.strip_prefix('[').unwrap_or(header);
            let header = header.strip_suffix(']').unwrap_or(header);
            path = header
                .split('.')
                .map(|part| unquote(part.trim()))
                .filter(|part| !part.is_empty())
                .take(MAX_NESTING)
                .collect();
            continue;
        }
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = unquote(key.trim());
        if key.is_empty() {
            continue;
        }
        insert_at(&mut root, &path, key, toml_value(rest.trim()));
    }
    sort_tables(&mut root);
    root
}

/// Put every table in key order, top down.
fn sort_tables(table: &mut [(String, Value)]) {
    table.sort_by(|left, right| left.0.cmp(&right.0));
    for (_, value) in table.iter_mut() {
        if let Value::Dict(inner) = value {
            sort_tables(inner);
        }
    }
}

/// Set a key in the table a header path names, making the tables along the way.
fn insert_at(table: &mut Vec<(String, Value)>, path: &[String], key: String, value: Value) {
    let Some((head, rest)) = path.split_first() else {
        match table.iter_mut().find(|(name, _)| name == &key) {
            Some(entry) => entry.1 = value,
            None => table.push((key, value)),
        }
        return;
    };
    if !table.iter().any(|(name, _)| name == head) {
        table.push((head.clone(), Value::Dict(Vec::new())));
    }
    if let Some((_, Value::Dict(inner))) = table.iter_mut().find(|(name, _)| name == head) {
        insert_at(inner, rest, key, value);
    }
}

/// Drop the `#` comment a TOML line ends with, keeping any `#` written inside a quoted string.
fn strip_toml_comment(line: &str) -> &str {
    let mut quote = None;
    for (index, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), c) if c == open => quote = None,
            (None, '#') => return line.get(..index).unwrap_or(line),
            _ => {}
        }
    }
    line
}

fn toml_value(text: &str) -> Value {
    if let Some(inner) = text
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return Value::Array(
            split_items(inner)
                .iter()
                .map(|part| toml_value(part))
                .collect(),
        );
    }
    if let Some(inner) = text
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    {
        return mapping(
            split_items(inner)
                .iter()
                .filter_map(|part| {
                    let (key, held) = part.split_once('=')?;
                    Some((unquote(key.trim()), toml_value(held.trim())))
                })
                .collect(),
        );
    }
    if text.starts_with('"') || text.starts_with('\'') {
        return Value::Str(unquote(text));
    }
    scalar(text)
}

/// The comma-separated items of an inline array or table, split outside quotes and nesting.
fn split_items(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quote = None;
    for c in text.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), c) if c == open => quote = None,
            (None, '[' | '{') => depth = depth.saturating_add(1),
            (None, ']' | '}') => depth = depth.saturating_sub(1),
            (None, ',') if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    parts.push(current);
    parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Take the quotes off a quoted key or string.
fn unquote(text: &str) -> String {
    for quote in ['"', '\''] {
        if let Some(inner) = text
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.to_string();
        }
    }
    text.to_string()
}

/// The value an unquoted scalar stands for: a boolean, nothing, a number, or the text itself.
fn scalar(text: &str) -> Value {
    match text {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        "null" | "~" => return Value::Nothing,
        _ => {}
    }
    let digits = text.replace('_', "");
    if let Ok(number) = digits.parse::<Integer>() {
        return Value::Int(number);
    }
    if let Ok(number) = digits.parse::<f64>() {
        return Value::Number(number, String::new());
    }
    Value::Str(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Integer, Value, load};

    fn field(value: &Value, key: &str) -> Value {
        match value {
            Value::Dict(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map_or(Value::Nothing, |(_, held)| held.clone()),
            _ => Value::Nothing,
        }
    }

    #[test]
    fn read_keeps_the_text_verbatim() {
        assert_eq!(
            load("read", "a\nb\n"),
            Some(Value::Str("a\nb\n".to_string()))
        );
        assert_eq!(load("elsewhere", "a"), None);
    }

    #[test]
    fn csv_splits_quoted_records() {
        let Some(Value::Array(records)) = load("csv", "a,b\n\"x,y\",2\n") else {
            panic!("expected records")
        };
        assert_eq!(
            records.get(1),
            Some(&Value::Array(vec![
                Value::Str("x,y".to_string()),
                Value::Str("2".to_string()),
            ]))
        );
    }

    #[test]
    fn json_types_every_leaf() {
        let value = load("json", r#"{"s":"a","n":null,"f":1.25,"a":[1,2],"b":true}"#)
            .unwrap_or(Value::Nothing);
        assert_eq!(field(&value, "s"), Value::Str("a".to_string()));
        assert_eq!(field(&value, "n"), Value::Nothing);
        assert_eq!(field(&value, "f"), Value::Number(1.25, String::new()));
        assert_eq!(field(&value, "b"), Value::Bool(true));
        assert_eq!(
            field(&value, "a"),
            Value::Array(vec![
                Value::Int(Integer::from(1i64)),
                Value::Int(Integer::from(2i64))
            ])
        );
        assert_eq!(load("json", "{"), Some(Value::Nothing));
    }

    #[test]
    fn json_keys_come_out_sorted() {
        let Some(Value::Dict(entries)) = load("json", r#"{"z":1,"a":2}"#) else {
            panic!("expected a mapping")
        };
        let keys: Vec<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["a", "z"]);
    }

    #[test]
    fn yaml_resolves_plain_scalars() {
        let value =
            load("yaml", "k: 7\nn: 0x10\ns: hello\nq: \"7\"\nb: true\n").unwrap_or(Value::Nothing);
        assert_eq!(field(&value, "k"), Value::Int(Integer::from(7i64)));
        assert_eq!(field(&value, "n"), Value::Int(Integer::from(16i64)));
        assert_eq!(field(&value, "s"), Value::Str("hello".to_string()));
        assert_eq!(field(&value, "q"), Value::Str("7".to_string()));
        assert_eq!(field(&value, "b"), Value::Bool(true));
    }

    #[test]
    fn yaml_keeps_a_top_level_sequence_or_scalar() {
        assert_eq!(
            load("yaml", "- 1\n- two\n"),
            Some(Value::Array(vec![
                Value::Int(Integer::from(1i64)),
                Value::Str("two".to_string())
            ]))
        );
        assert_eq!(
            load("yaml", "plain\n"),
            Some(Value::Str("plain".to_string()))
        );
        assert_eq!(load("yaml", ""), Some(Value::Dict(Vec::new())));
    }

    #[test]
    fn toml_reads_headers_and_inline_collections() {
        let value = load(
            "toml",
            "title = \"T\" # trailing\nn = 42\narr = [1, 2]\ninline = { a = 1 }\n\n[owner.deep]\nz = 9\n",
        )
        .unwrap_or(Value::Nothing);
        assert_eq!(field(&value, "title"), Value::Str("T".to_string()));
        assert_eq!(field(&value, "n"), Value::Int(Integer::from(42i64)));
        assert_eq!(
            field(&value, "arr"),
            Value::Array(vec![
                Value::Int(Integer::from(1i64)),
                Value::Int(Integer::from(2i64))
            ])
        );
        assert_eq!(
            field(&field(&value, "inline"), "a"),
            Value::Int(Integer::from(1i64))
        );
        assert_eq!(
            field(&field(&field(&value, "owner"), "deep"), "z"),
            Value::Int(Integer::from(9i64))
        );
    }

    #[test]
    fn xml_records_tag_attributes_and_children() {
        let Some(Value::Array(nodes)) = load("xml", "<r z=\"1\" a=\"2\">text<c/></r>") else {
            panic!("expected nodes")
        };
        let root = nodes.first().cloned().unwrap_or(Value::Nothing);
        assert_eq!(field(&root, "tag"), Value::Str("r".to_string()));
        let Value::Dict(attrs) = field(&root, "attrs") else {
            panic!("expected attributes")
        };
        let keys: Vec<&str> = attrs.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["a", "z"]);
        let Value::Array(children) = field(&root, "children") else {
            panic!("expected children")
        };
        assert_eq!(children.first(), Some(&Value::Str("text".to_string())));
        assert_eq!(
            children.get(1).map(|child| field(child, "tag")),
            Some(Value::Str("c".to_string()))
        );
    }
}
