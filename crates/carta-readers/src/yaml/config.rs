//! Parse YAML values without rounding numeric scalars.

use std::collections::BTreeMap;
use std::iter::Peekable;

use carta_core::{Error, Result};
use granit_parser::{Event, Parser, ScalarStyle};
use serde_json::{Map, Value};

#[derive(Clone)]
struct Node {
    value: Value,
    cost: usize,
}

struct State {
    anchors: BTreeMap<usize, Node>,
    remaining: usize,
}

impl State {
    fn charge(&mut self, amount: usize) -> std::result::Result<(), String> {
        self.remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or_else(|| "YAML expansion exceeds its work allowance".to_owned())?;
        Ok(())
    }
}

/// Parse one YAML value, keeping numbers as exact, canonical text.
///
/// Booleans and null retain their types. Collections, quoted text, aliases, and merge keys are
/// supported. Nesting and alias expansion are bounded.
pub fn parse(source: &str) -> Result<Value> {
    parse_value(source).map_err(Error::InvalidMetadata)
}

fn parse_value(source: &str) -> std::result::Result<Value, String> {
    let mut state = State {
        anchors: BTreeMap::new(),
        remaining: source.len().saturating_mul(32).saturating_add(8192),
    };
    let mut events = Vec::new();
    for event in Parser::new_from_str(source) {
        state.charge(1)?;
        events.push(event.map_err(|error| error.to_string())?.0);
    }
    let mut events = events.into_iter().peekable();
    if !matches!(events.next(), Some(Event::StreamStart)) {
        return Err("expected a YAML stream".to_owned());
    }
    if matches!(events.peek(), Some(Event::StreamEnd)) {
        return Ok(Value::Null);
    }
    if !matches!(events.next(), Some(Event::DocumentStart(..))) {
        return Err("expected a YAML document".to_owned());
    }
    let node = read(&mut events, &mut state, 0)?;
    if !matches!(events.next(), Some(Event::DocumentEnd))
        || !matches!(events.next(), Some(Event::StreamEnd))
    {
        return Err("expected one complete YAML document".to_owned());
    }
    Ok(node.value)
}

fn read<'a>(
    events: &mut Peekable<impl Iterator<Item = Event<'a>>>,
    state: &mut State,
    depth: usize,
) -> std::result::Result<Node, String> {
    if depth > 64 {
        return Err("YAML nesting exceeds its limit".to_owned());
    }
    state.charge(1)?;
    let (node, anchor) = match events.next() {
        Some(Event::Scalar(text, style, anchor, tag)) => {
            let kind = tag
                .as_ref()
                .filter(|tag| tag.handle() == "tag:yaml.org,2002:")
                .map(|tag| tag.suffix());
            let plain = style == ScalarStyle::Plain;
            let value = if kind == Some("str") || (!plain && kind.is_none()) {
                Value::String(text.into_owned())
            } else if (text.is_empty() || super::is_null(&text))
                && kind.is_none_or(|kind| kind == "null")
            {
                Value::Null
            } else if let Some(boolean) =
                super::as_bool(&text).filter(|_| kind.is_none_or(|kind| kind == "bool"))
            {
                Value::Bool(boolean)
            } else {
                if matches!(kind, Some("null" | "bool")) {
                    return Err("invalid typed YAML scalar".to_owned());
                }
                Value::String(
                    super::canonicalize_number(&text).unwrap_or_else(|| text.into_owned()),
                )
            };
            let cost = value
                .as_str()
                .map_or(1, |text| text.len().saturating_add(1));
            state.charge(cost)?;
            (Node { value, cost }, anchor)
        }
        Some(Event::Alias(anchor)) => {
            let cost = state
                .anchors
                .get(&anchor)
                .ok_or_else(|| "unknown or recursive YAML alias".to_owned())?
                .cost;
            state.charge(cost)?;
            return state
                .anchors
                .get(&anchor)
                .cloned()
                .ok_or_else(|| "unknown YAML alias".to_owned());
        }
        Some(Event::SequenceStart(_, anchor, _)) => {
            let mut values = Vec::new();
            let mut cost = 1usize;
            while !matches!(events.peek(), Some(Event::SequenceEnd)) {
                let node = read(events, state, depth + 1)?;
                cost = cost.saturating_add(node.cost);
                values.push(node.value);
            }
            events.next();
            (
                Node {
                    value: Value::Array(values),
                    cost,
                },
                anchor,
            )
        }
        Some(Event::MappingStart(_, anchor, _)) => (read_mapping(events, state, depth)?, anchor),
        _ => return Err("expected a YAML value".to_owned()),
    };
    if anchor != 0 {
        state.charge(node.cost)?;
        state.anchors.insert(anchor, node.clone());
    }
    Ok(node)
}

fn read_mapping<'a>(
    events: &mut Peekable<impl Iterator<Item = Event<'a>>>,
    state: &mut State,
    depth: usize,
) -> std::result::Result<Node, String> {
    let mut values = Map::new();
    let mut merged = Map::new();
    let mut cost = 1usize;
    while !matches!(events.peek(), Some(Event::MappingEnd)) {
        let merge = matches!(events.peek(), Some(Event::Scalar(text, style, _, tag)) if text == "<<" && (*style == ScalarStyle::Plain || tag.as_ref().is_some_and(|tag| tag.suffix() == "merge")));
        let spelling = match events.peek() {
            Some(Event::Scalar(text, ..)) => Some(text.to_string()),
            _ => None,
        };
        let name = read(events, state, depth + 1)?;
        let name = spelling
            .or_else(|| match name.value {
                Value::String(text) => Some(text),
                Value::Bool(value) => Some(value.to_string()),
                Value::Null => Some("null".to_owned()),
                _ => None,
            })
            .ok_or_else(|| "YAML mapping keys must be scalars".to_owned())?;
        let node = read(events, state, depth + 1)?;
        cost = cost.saturating_add(name.len()).saturating_add(node.cost);
        if merge {
            merge_values(&mut merged, node.value)?;
        } else {
            values.insert(name, node.value);
        }
    }
    events.next();
    merged.extend(values);
    Ok(Node {
        value: Value::Object(merged),
        cost,
    })
}

fn merge_values(target: &mut Map<String, Value>, value: Value) -> std::result::Result<(), String> {
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                target.entry(key).or_insert(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                merge_values(target, value)?;
            }
        }
        _ => return Err("YAML merge values must be mappings".to_owned()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn numbers_keep_all_digits_and_quoted_values_stay_text() {
        let value = parse("small: 2.0\nprecise: 1.234567890123456789\nlarge: 123456789012345678901234567890\nquoted: '2.0'\nflag: yes\nempty: null\n").unwrap();
        assert_eq!(value["small"], "2");
        assert_eq!(value["precise"], "1.234567890123456789");
        assert_eq!(value["large"], "1.2345678901234567890123456789e29");
        assert_eq!(value["quoted"], "2.0");
        assert_eq!(value["flag"], true);
        assert!(value["empty"].is_null());
    }

    #[test]
    fn aliases_merge_keys_and_multiline_values_preserve_content() {
        let value = parse("---\nbase: &base {name: first, keep: true}\nchild: {<<: *base, name: second}\nitems: [\n  one,\n  \"\\u03bb\"\n]\n...").unwrap();
        assert_eq!(value["child"]["name"], "second");
        assert_eq!(value["child"]["keep"], true);
        assert_eq!(value["items"][1], "λ");
        let tagged = parse("a: !!str null\nb: !!bool \"false\"\nc: !!float \"2.0\"\n").unwrap();
        assert_eq!(tagged["a"], "null");
        assert_eq!(tagged["b"], false);
        assert_eq!(tagged["c"], "2");
    }

    #[test]
    fn recursive_aliases_excessive_expansion_and_extra_documents_fail() {
        assert!(parse("node: &node [*node]").is_err());
        assert!(parse("---\na: 1\n---\nb: 2").is_err());
        let mut source = "a: &a [one, two]\n".to_owned();
        let mut previous = "a".to_owned();
        for index in 0..20 {
            let name = format!("n{index}");
            source.push_str(&format!(
                "{name}: &{name} [*{previous}, *{previous}, *{previous}]\n"
            ));
            previous = name;
        }
        assert!(parse(&source).is_err());
    }
}
