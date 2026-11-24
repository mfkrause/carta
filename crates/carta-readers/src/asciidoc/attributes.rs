use super::inline::{ListKind, Lookahead, alpha_numeric};
use carta_ast::{Attr, Block, Document, Inline};
use std::collections::BTreeMap;

const USER_ATTRIBUTE_PREFIX: &str = "asciidoc-carta-user-attribute-";

#[derive(Default, Clone)]
pub(super) struct Attributes {
    pub positional: Vec<String>,
    pub named: BTreeMap<String, String>,
}

/// Splits the content of an attribute list into its fields.
///
/// The space after a comma belongs to no field, and neither does a comma at the end of the
/// list; the first field keeps its leading space.
fn fields(text: &str, whole: bool) -> Vec<&str> {
    let mut output = Vec::new();
    let mut rest = text;
    loop {
        let (field, after) = rest.split_at(field_length(rest, whole && output.is_empty()));
        let Some(after) = after.strip_prefix(',') else {
            if !field.is_empty() || output.is_empty() {
                output.push(field);
            }
            break;
        };
        output.push(field);
        rest = super::trim_space_start(after);
    }
    output
}

/// Length of the field at the start of `text`, the content of an attribute list.
///
/// The fields are those that [`Lookahead::list_end`] reads: a field ends at a comma, but a value
/// that opens with a quote, positional or right after the equals sign of a named one, runs to
/// its closing quote. A first field in shorthand holds no such value.
fn field_length(text: &str, first: bool) -> usize {
    let comma = text.find(',').unwrap_or(text.len());
    if first && shorthand(text) {
        return comma;
    }
    let value = name_end(text).map_or(0, |equals| equals + 1);
    text[value..]
        .strip_prefix('"')
        .and_then(|quoted| quoted.find('"'))
        .map(|close| value + close + 2)
        .filter(|end| text[*end..].is_empty() || text[*end..].starts_with(','))
        .unwrap_or(comma)
}

/// Whether the first field of a list, which starts `text`, is in shorthand.
///
/// Shorthand puts an id, role, or option marker after a bare positional word, which may be
/// absent, and the space after it. Such a field names no value and quotes none.
pub(super) fn shorthand(text: &str) -> bool {
    super::trim_space_start(text.trim_start_matches(|c| alpha_numeric(c) || c == '-' || c == '_'))
        .starts_with(['.', '#', '%'])
}

/// Offset of the equals sign after the name of the field at the start of `text`.
///
/// A field names its value when the equals sign comes after some text and before any comma or
/// bracket.
fn name_end(text: &str) -> Option<usize> {
    text.find([',', ']', '='])
        .filter(|at| *at > 0 && text[*at..].starts_with('='))
}

impl Attributes {
    pub(super) fn parse(text: &str) -> Self {
        let mut attributes = Self::default();
        attributes.extend(text);
        attributes
    }

    /// Parses the fields that follow the first field of a list: none of them is in shorthand.
    pub(super) fn parse_rest(text: &str) -> Self {
        let mut attributes = Self::default();
        attributes.read_fields(text, false);
        attributes
    }

    pub(super) fn extend(&mut self, text: &str) {
        let mut parsed = Self::default();
        parsed.read_fields(text, true);
        self.positional.extend(parsed.positional);
        for (name, value) in parsed.named {
            merge_named(&mut self.named, name, value);
        }
    }

    fn read_fields(&mut self, text: &str, whole: bool) {
        if text.is_empty() {
            return;
        }
        let mut named = BTreeMap::new();
        for (index, field) in fields(text, whole).into_iter().enumerate() {
            let shorthand = whole && index == 0 && shorthand(field);
            if !shorthand && let Some(equals) = name_end(field) {
                let (name, value) = (&field[..equals], &field[equals + 1..]);
                named.insert(name.to_owned(), unquote(value).to_owned());
                continue;
            }
            let markers = if shorthand {
                field.find(['#', '.', '%']).unwrap_or(field.len())
            } else {
                field.len()
            };
            let (value, mut rest) = field.split_at(markers);
            let value = unquote(value);
            if !(shorthand && value.is_empty()) {
                self.positional.push(value.to_owned());
            }
            while !rest.is_empty() {
                let marker = rest.bytes().next().unwrap_or_default();
                rest = &rest[1..];
                let end = rest.find(['#', '.', '%']).unwrap_or(rest.len());
                let value = rest[..end].trim_matches(super::is_space);
                let key = match marker {
                    b'#' => "id",
                    b'.' => "role",
                    _ => "options",
                };
                self.named
                    .entry(key.to_owned())
                    .and_modify(|s| {
                        if key == "id" {
                            s.clear();
                        } else {
                            s.push(if key == "options" { ',' } else { ' ' });
                        }
                        s.push_str(value);
                    })
                    .or_insert_with(|| value.to_owned());
                rest = &rest[end..];
            }
        }
        for (name, value) in named {
            merge_named(&mut self.named, name, value);
        }
    }

    pub(super) fn style(&self) -> &str {
        self.positional.first().map_or("", String::as_str)
    }
    pub(super) fn get(&self, name: &str) -> Option<&str> {
        self.named.get(name).map(String::as_str)
    }
    pub(super) fn has_empty_id(&self) -> bool {
        self.named.get("id").is_some_and(String::is_empty)
    }
    pub(super) fn has(&self, option: &str) -> bool {
        self.get("options")
            .is_some_and(|s| s.split([' ', ',']).any(|v| v == option))
    }
    pub(super) fn attr(&self) -> Attr {
        let mut attr = Attr::default();
        for (name, value) in &self.named {
            match name.as_str() {
                "id" => attr.id = value.into(),
                "role" => attr.classes.push(value.into()),
                _ => attr.attributes.push((name.into(), value.into())),
            }
        }
        protect_attribute_names(&mut attr);
        attr
    }
}

pub(super) fn protect_attribute_names(attr: &mut Attr) {
    for (name, _) in &mut attr.attributes {
        if name.starts_with("asciidoc-") || name == "wrapper" {
            *name = format!("{USER_ATTRIBUTE_PREFIX}{name}").into();
        }
    }
}

pub(super) fn restore_attribute_names(document: &mut Document) {
    carta_core::walk::for_each_document_attribute(document, &mut |attr| {
        for (name, _) in &mut attr.attributes {
            if let Some(original) = name.strip_prefix(USER_ATTRIBUTE_PREFIX) {
                *name = original.into();
            }
        }
    });
}

pub(super) fn add_inline_attr(mut inlines: Vec<Inline>, attr: &Attr, present: bool) -> Vec<Inline> {
    if *attr == Attr::default() && !present {
        return inlines;
    }
    for inline in &mut inlines {
        match inline {
            Inline::Span(existing, children)
                if existing
                    .attributes
                    .iter()
                    .any(|(key, _)| key == super::IDENTIFIER_SOURCE) =>
            {
                *children = add_inline_attr(std::mem::take(children), attr, present);
            }
            Inline::Code(existing, _)
            | Inline::Span(existing, _)
            | Inline::Link(existing, _, _)
            | Inline::Image(existing, _, _) => merge(existing, attr),
            _ => {
                let mut attr = attr.clone();
                attr.attributes.insert(0, ("wrapper".into(), "1".into()));
                let node = std::mem::replace(inline, Inline::Space);
                *inline = Inline::Span(Box::new(attr), vec![node]);
            }
        }
    }
    inlines
}

/// Reads the content of the attribute list before formatted text: only its first field counts.
pub(super) fn formatted_attributes(text: &str) -> Attributes {
    let mut attributes = Attributes::default();
    let text = super::trim_space_start(text);
    let mut rest = text;
    while let Some(marker) = rest.chars().next().filter(|c| matches!(c, '.' | '#' | '%')) {
        rest = &rest[1..];
        let end = rest.find(['.', '#', '%', ',']).unwrap_or(rest.len());
        let name = match marker {
            '.' => "role",
            '#' => "id",
            _ => "options",
        };
        merge_named(
            &mut attributes.named,
            name.to_owned(),
            rest[..end].trim_matches(super::is_space).to_owned(),
        );
        rest = rest[end..].trim_start_matches(super::is_space);
    }
    if attributes.named.is_empty() {
        let role = match text
            .strip_prefix('"')
            .and_then(|quoted| quoted.split_once('"'))
        {
            Some((quoted, _)) => quoted,
            None => super::trim_space(text.split(',').next().unwrap_or_default()),
        };
        attributes.named.insert("role".to_owned(), role.to_owned());
    }
    attributes
}

pub(super) fn merge(existing: &mut Attr, attr: &Attr) {
    if !attr.id.is_empty() {
        existing.id.clone_from(&attr.id);
    }
    let mut classes = attr.classes.clone();
    classes.append(&mut existing.classes);
    existing.classes = classes;
    let mut attributes = attr.attributes.clone();
    attributes.append(&mut existing.attributes);
    existing.attributes = attributes;
}

pub(super) fn add_block_attr(block: Block, attr: Attr, empty_id: bool) -> Block {
    if attr == Attr::default() && !empty_id {
        return block;
    }
    let mut block = block;
    match &mut block {
        Block::CodeBlock(existing, _) | Block::Header(_, existing, _) | Block::Div(existing, _) => {
            if empty_id && attr.id.is_empty() {
                existing.id = "".into();
            }
            merge(existing, &attr);
        }
        Block::Table(table) => {
            if empty_id && attr.id.is_empty() {
                table.attr.id = "".into();
            }
            merge(&mut table.attr, &attr);
        }
        _ => {
            let mut attr = attr;
            attr.attributes.insert(0, ("wrapper".into(), "1".into()));
            return Block::Div(Box::new(attr), vec![block]);
        }
    }
    block
}

pub(super) fn block_attributes(line: &str) -> Option<&str> {
    if !line.starts_with('[') {
        return None;
    }
    let end = Lookahead::new(line).list_end(0, ListKind::Named)?;
    line.get(end + 1..)?
        .trim_matches(super::is_space)
        .is_empty()
        .then_some(line.get(1..end)?)
}

pub(super) fn block_anchor(line: &str) -> Option<&str> {
    let source = line.strip_prefix("[[")?;
    let end = source.find(|character: char| super::is_space(character) || character == ']')?;
    (end > 0 && source.get(end..) == Some("]]")).then_some(&source[..end])
}

pub(super) fn block_attributes_at(lines: &[&str], position: usize) -> Option<(String, usize)> {
    let line = *lines.get(position)?;
    let continued_quote = line.chars().filter(|character| *character == '"').count() % 2 == 1
        && lines
            .get(position + 1..)
            .is_some_and(|rest| rest.iter().any(|line| line.contains('"')));
    if !continued_quote
        && line.ends_with(']')
        && let Some(content) = block_attributes(line)
    {
        return Some((content.to_owned(), 1));
    }
    if !line.starts_with('[') || line.starts_with("[[") {
        return None;
    }
    let source = lines.get(position..)?.join("\n");
    let end = Lookahead::new(&source).list_end(0, ListKind::Named)?;
    let tail = source
        .get(end + 1..)?
        .split('\n')
        .next()
        .unwrap_or_default();
    if !tail.is_empty() {
        return None;
    }
    let content = source.get(1..end)?;
    Some((
        content.to_owned(),
        source
            .get(..end)?
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1,
    ))
}

/// The value of a field: a quote at its start quotes it up to a quote at its end, and space
/// around a bare value does not count.
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or_else(|| super::trim_space(value))
}

fn merge_named(values: &mut BTreeMap<String, String>, name: String, value: String) {
    if matches!(name.as_str(), "role" | "options")
        && let Some(previous) = values.get_mut(&name)
    {
        previous.push(if name == "role" { ' ' } else { ',' });
        previous.push_str(&value);
    } else {
        values.insert(name, value);
    }
}
