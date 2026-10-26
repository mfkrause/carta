//! Shared sequence segments for repetition, concatenation, and slicing.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Array,
    Content,
    Inlines,
}

#[derive(Debug, Clone, PartialEq)]
struct Part {
    value: Rc<Value>,
    characters: Option<Rc<Vec<char>>>,
    period: usize,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Sequence {
    parts: Vec<Part>,
    kind: Kind,
    length: usize,
    block: bool,
    copies: Rc<cell::Cell<usize>>,
}

impl Drop for Sequence {
    fn drop(&mut self) {
        let mut parts = std::mem::take(&mut self.parts);
        while let Some(part) = parts.pop() {
            if let Ok(Value::Sequence(sequence)) = Rc::try_unwrap(part.value)
                && let Ok(mut sequence) = Rc::try_unwrap(sequence)
            {
                parts.append(&mut sequence.parts);
            }
        }
    }
}

fn kind(value: &Value) -> Option<Kind> {
    match value {
        Value::Str(_) => Some(Kind::Text),
        Value::Array(_) => Some(Kind::Array),
        Value::Content(_) => Some(Kind::Content),
        Value::Inlines(_) => Some(Kind::Inlines),
        Value::Sequence(sequence) => Some(sequence.kind),
        _ => None,
    }
}

pub(super) fn compatible(left: &Value, right: &Value) -> bool {
    kind(left).is_some_and(|left| Some(left) == kind(right))
}

impl Sequence {
    pub(super) fn repeated(value: Value, count: usize, copies: &Rc<cell::Cell<usize>>) -> Value {
        if count == 1 && matches!(value, Value::Sequence(_)) {
            return value;
        }
        let Some(kind) = kind(&value) else {
            return value;
        };
        if let Value::Sequence(sequence) = &value
            && let [part] = sequence.parts.as_slice()
            && sequence.length % part.period == 0
        {
            let Some(length) = sequence.length.checked_mul(count) else {
                return Value::Nothing;
            };
            let mut part = part.clone();
            part.end = length;
            return Value::Sequence(Rc::new(Self {
                parts: if length == 0 { Vec::new() } else { vec![part] },
                kind,
                length,
                block: sequence.block,
                copies: Rc::clone(copies),
            }));
        }
        let block = value.is_block();
        let characters = match &value {
            Value::Str(text) => Some(Rc::new(text.chars().collect::<Vec<_>>())),
            _ => None,
        };
        let period = match &value {
            Value::Str(_) => characters.as_ref().map_or(0, |characters| characters.len()),
            Value::Array(items) => items.len(),
            Value::Sequence(sequence) => sequence.length,
            _ => 1,
        };
        let Some(length) = period.checked_mul(count) else {
            return Value::Nothing;
        };
        let parts = if length == 0 {
            Vec::new()
        } else {
            vec![Part {
                value: Rc::new(value),
                characters,
                period,
                start: 0,
                end: length,
            }]
        };
        Value::Sequence(Rc::new(Self {
            parts,
            kind,
            length,
            block,
            copies: Rc::clone(copies),
        }))
    }

    pub(super) fn concatenate(left: Value, right: Value, copies: &Rc<cell::Cell<usize>>) -> Value {
        let Value::Sequence(left) = Self::repeated(left, 1, copies) else {
            return Value::Nothing;
        };
        let Value::Sequence(right) = Self::repeated(right, 1, copies) else {
            return Value::Nothing;
        };
        let offset = left.length;
        let Some(length) = offset.checked_add(right.length) else {
            return Value::Nothing;
        };
        let kind = left.kind;
        let block = left.block || right.block;
        let mut parts = Self::into_parts(left);
        let mut tail = Self::into_parts(right);
        for part in &mut tail {
            part.end = part.end.saturating_add(offset);
        }
        parts.append(&mut tail);
        Value::Sequence(Rc::new(Self {
            parts,
            kind,
            length,
            block,
            copies: Rc::clone(copies),
        }))
    }

    fn into_parts(sequence: Rc<Self>) -> Vec<Part> {
        match Rc::try_unwrap(sequence) {
            Ok(mut sequence) => std::mem::take(&mut sequence.parts),
            Err(sequence) if sequence.length == 0 => Vec::new(),
            Err(sequence) => {
                let period = sequence.length;
                vec![Part {
                    value: Rc::new(Value::Sequence(sequence)),
                    characters: None,
                    period,
                    start: 0,
                    end: period,
                }]
            }
        }
    }

    fn locate(&self, mut index: usize) -> Option<(&Value, usize, Option<&[char]>)> {
        let mut sequence = self;
        loop {
            let position = sequence.parts.partition_point(|part| part.end <= index);
            let part = sequence.parts.get(position)?;
            let previous = position
                .checked_sub(1)
                .and_then(|position| sequence.parts.get(position))
                .map_or(0, |part| part.end);
            index = part.start.saturating_add(index - previous) % part.period;
            match part.value.as_ref() {
                Value::Sequence(inner) => sequence = inner,
                value => {
                    return Some((value, index, part.characters.as_deref().map(Vec::as_slice)));
                }
            }
        }
    }

    fn spend(&self, amount: usize) -> bool {
        let Some(left) = self.copies.get().checked_sub(amount) else {
            return false;
        };
        self.copies.set(left);
        true
    }

    pub(super) fn materialize(&self) -> Value {
        let mut text = String::new();
        let mut items = Vec::new();
        let mut blocks = Vec::new();
        let mut inlines = Vec::new();
        for index in 0..self.length {
            let Some((value, position, characters)) = self.locate(index) else {
                break;
            };
            match value {
                Value::Str(_) => {
                    let Some(&character) =
                        characters.and_then(|characters| characters.get(position))
                    else {
                        break;
                    };
                    if !self.spend(character.len_utf8()) {
                        break;
                    }
                    text.push(character);
                }
                Value::Array(values) => {
                    let Some(value) = values.get(position) else {
                        break;
                    };
                    if !self.spend(value_weight(value)) {
                        break;
                    }
                    items.push(value.clone());
                }
                Value::Content(values) => {
                    if !self.spend(value_weight(value)) {
                        break;
                    }
                    append_content(&mut blocks, values.clone());
                }
                Value::Inlines(values) => {
                    if !self.spend(value_weight(value)) {
                        break;
                    }
                    inlines.extend(values.iter().cloned());
                }
                _ => break,
            }
        }
        match self.kind {
            Kind::Text => Value::Str(text),
            Kind::Array => Value::Array(items),
            Kind::Content => Value::Content(blocks),
            Kind::Inlines => Value::Inlines(inlines),
        }
    }

    pub(super) fn is_truthy(&self) -> bool {
        match self.kind {
            Kind::Text => self.length > 0,
            Kind::Array if self.length != 1 => self.length > 1,
            Kind::Array => match self.locate(0) {
                Some((Value::Array(items), index, _)) => items
                    .get(index)
                    .is_some_and(|value| !value.as_text().is_empty()),
                _ => false,
            },
            _ => !self.materialize().as_text().is_empty(),
        }
    }

    pub(super) fn is_block(&self) -> bool {
        self.block && self.length > 0
    }

    pub(super) fn type_name(&self) -> &'static str {
        match self.kind {
            Kind::Text => "string",
            Kind::Array => "array",
            Kind::Content | Kind::Inlines => "content",
        }
    }

    pub(super) fn method(&self, name: &str, args: &[Arg]) -> Option<Value> {
        if !matches!(self.kind, Kind::Text | Kind::Array) {
            return None;
        }
        Some(match name {
            "len" => Value::Int(Integer::from(self.length)),
            "slice" => {
                let (start, end) = slice_bounds(args, self.length);
                let mut parts = Vec::new();
                let first = self.parts.partition_point(|part| part.end <= start);
                let mut previous = first
                    .checked_sub(1)
                    .and_then(|index| self.parts.get(index))
                    .map_or(0, |part| part.end);
                for part in self.parts.get(first..).unwrap_or_default() {
                    if previous >= end {
                        break;
                    }
                    let from = previous.max(start);
                    let to = part.end.min(end);
                    if from < to {
                        let mut selected = part.clone();
                        selected.start =
                            selected.start.saturating_add(from - previous) % selected.period;
                        selected.end = to - start;
                        parts.push(selected);
                    }
                    previous = part.end;
                }
                Value::Sequence(Rc::new(Self {
                    parts,
                    kind: self.kind,
                    length: end - start,
                    block: self.block,
                    copies: Rc::clone(&self.copies),
                }))
            }
            "at" | "first" | "last" if self.kind == Kind::Array => {
                let index = match name {
                    "first" => 0,
                    "last" => self.length.saturating_sub(1),
                    _ => positional_index(args),
                };
                match self.locate(index) {
                    Some((Value::Array(items), position, _)) if index < self.length => items
                        .get(position)
                        .filter(|value| self.spend(value_weight(value)))
                        .cloned()
                        .unwrap_or(Value::Nothing),
                    _ => Value::Nothing,
                }
            }
            _ => return None,
        })
    }
}
