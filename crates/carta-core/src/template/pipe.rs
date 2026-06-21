//! Pipe (filter) evaluation: each turns one [`Value`] into another.

use std::collections::BTreeMap;

use super::Value;
use super::node::{Align, Pipe};

/// Apply one pipe to a value.
pub(crate) fn apply(value: &Value, pipe: &Pipe) -> Value {
    match pipe {
        Pipe::Uppercase => map_str(value, str::to_uppercase),
        Pipe::Lowercase => map_str(value, lowercase),
        Pipe::Length => Value::Str(length(value).to_string()),
        Pipe::Reverse => reverse(value),
        Pipe::First => nth_end(value, End::First),
        Pipe::Last => nth_end(value, End::Last),
        Pipe::Rest => drop_end(value, End::First),
        Pipe::AllButLast => drop_end(value, End::Last),
        Pipe::Pairs => pairs(value),
        Pipe::Alpha => Value::Str(alpha(value)),
        Pipe::Roman => Value::Str(roman(value)),
        Pipe::Chomp => Value::Str(stringify(value).trim_end_matches(['\n', '\r']).to_string()),
        Pipe::Nowrap => value.clone(),
        Pipe::Block {
            align,
            width,
            left,
            right,
        } => Value::Str(block(&stringify(value), *align, *width, left, right)),
    }
}

/// Flatten a value to its bare string form (lists concatenate with no separator). A map is a present,
/// non-empty value whose bare form is `true`, so it reads the same way a boolean `true` does.
pub(crate) fn stringify(value: &Value) -> String {
    match value {
        Value::Str(s) => s.clone(),
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::List(items) => items.iter().map(stringify).collect(),
        Value::Map(_) => "true".to_string(),
    }
}

/// Apply a text transformation to a string-shaped value. A boolean or map is not textual, so the
/// string filters leave it untouched and it keeps rendering as `true`/`false`.
fn map_str(value: &Value, f: impl Fn(&str) -> String) -> Value {
    match value {
        Value::Bool(_) | Value::Map(_) => value.clone(),
        other => Value::Str(f(&stringify(other))),
    }
}

/// Lowercase character by character, so a capital sigma always becomes a plain `σ` rather than the
/// word-final `ς` that a context-sensitive full-string mapping would pick.
fn lowercase(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

fn length(value: &Value) -> usize {
    match value {
        Value::List(items) => items.len(),
        Value::Map(map) => map.len(),
        Value::Bool(_) => 0,
        Value::Str(s) => s.chars().count(),
    }
}

fn reverse(value: &Value) -> Value {
    match value {
        Value::List(items) => Value::List(items.iter().rev().cloned().collect()),
        Value::Str(s) => Value::Str(s.chars().rev().collect()),
        Value::Bool(_) | Value::Map(_) => value.clone(),
    }
}

#[derive(Clone, Copy)]
enum End {
    First,
    Last,
}

/// Select one end of a list. On a non-list the value passes through unchanged; an empty list yields
/// the empty string.
fn nth_end(value: &Value, end: End) -> Value {
    let Value::List(items) = value else {
        return value.clone();
    };
    let picked = match end {
        End::First => items.first(),
        End::Last => items.last(),
    };
    picked.cloned().unwrap_or(Value::Str(String::new()))
}

/// Drop one end of a list. On a non-list the value passes through unchanged.
fn drop_end(value: &Value, end: End) -> Value {
    let Value::List(items) = value else {
        return value.clone();
    };
    let kept: Vec<Value> = match end {
        End::First => items.iter().skip(1).cloned().collect(),
        End::Last => {
            let take = items.len().saturating_sub(1);
            items.iter().take(take).cloned().collect()
        }
    };
    Value::List(kept)
}

/// Enumerate a map as a sorted list of `{key, value}` records (the iteration order of a [`BTreeMap`]
/// is already key-sorted). A list yields `{key, value}` with 1-based string indices.
fn pairs(value: &Value) -> Value {
    match value {
        Value::Map(map) => Value::List(
            map.iter()
                .map(|(key, val)| record(key.clone(), val.clone()))
                .collect(),
        ),
        Value::List(items) => Value::List(
            items
                .iter()
                .enumerate()
                .map(|(i, val)| record((i + 1).to_string(), val.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn record(key: String, value: Value) -> Value {
    let mut map = BTreeMap::new();
    map.insert("key".to_string(), Value::Str(key));
    map.insert("value".to_string(), value);
    Value::Map(map)
}

/// A value's integer form, when it is one. The grammar is a run of ASCII digits with an optional
/// leading `-`: surrounding whitespace or a leading `+` disqualifies it, so the numbering pipes leave
/// such values untouched.
fn as_int(value: &Value) -> Option<i64> {
    let text = stringify(value);
    let digits = text.strip_prefix('-').unwrap_or(&text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Single-letter cyclic numbering over the lowercase alphabet: `1`→`a` … `25`→`y`, with the cycle
/// boundary (`0`, `26`, …) landing on the character just before `a`. Non-integer or negative values
/// are left as their own text.
fn alpha(value: &Value) -> String {
    match as_int(value) {
        Some(n) if n >= 0 => {
            let offset = u8::try_from(n % 26).unwrap_or(0);
            char::from(b'a' - 1 + offset).to_string()
        }
        _ => stringify(value),
    }
}

/// Lowercase Roman numerals for `1..=3999`; `0` is empty and non-integer or negative values are left
/// as their own text. Inputs above `3999` are out of range and continue the greedy expansion.
fn roman(value: &Value) -> String {
    let Some(mut n) = as_int(value).filter(|n| *n >= 0) else {
        return stringify(value);
    };
    let mut out = String::new();
    for (amount, glyph) in [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ] {
        while n >= amount {
            out.push_str(glyph);
            n -= amount;
        }
    }
    out
}

/// Pad `content` into a `width`-wide field with the given alignment, framed by the border strings.
/// With no right border the trailing pad is dropped, so a borderless `left` is a no-op.
fn block(content: &str, align: Align, width: usize, left: &str, right: &str) -> String {
    let len = content.chars().count();
    let pad = width.saturating_sub(len);
    let body = match align {
        Align::Left => format!("{content}{}", " ".repeat(pad)),
        Align::Right => format!("{}{content}", " ".repeat(pad)),
        Align::Center => {
            let lead = pad / 2;
            format!("{}{content}{}", " ".repeat(lead), " ".repeat(pad - lead))
        }
    };
    let body = if right.is_empty() {
        body.trim_end_matches(' ').to_string()
    } else {
        body
    };
    format!("{left}{body}{right}")
}
