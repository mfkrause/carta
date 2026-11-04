use super::inline::alpha_numeric;
use carta_ast::{Attr, Block, Document, Inline};
use std::collections::BTreeMap;

const USER_ATTRIBUTE_PREFIX: &str = "asciidoc-carta-user-attribute-";

#[derive(Default, Clone)]
pub(super) struct Attributes {
    pub positional: Vec<String>,
    pub named: BTreeMap<String, String>,
}

pub(super) fn fields(text: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut field = String::new();
    let mut quote = None;
    for (position, character) in text.char_indices() {
        if Some(character) == quote {
            quote = None;
            field.push(character);
        } else if character == '"' && quote.is_none() {
            if text[position + 1..].contains('"') {
                quote = Some(character);
            }
            field.push(character);
        } else if character == ',' && quote.is_none() {
            output.push(field.trim_matches(super::is_space).to_owned());
            field.clear();
        } else {
            field.push(character);
        }
    }
    output.push(field.trim_matches(super::is_space).to_owned());
    output
}

impl Attributes {
    pub(super) fn parse(text: &str) -> Self {
        let mut attributes = Self::default();
        attributes.extend(text);
        attributes
    }

    pub(super) fn extend(&mut self, text: &str) {
        let mut parsed = Self::default();
        parsed.read_fields(text);
        self.positional.extend(parsed.positional);
        for (name, value) in parsed.named {
            merge_named(&mut self.named, name, value);
        }
    }

    pub(super) fn valid(text: &str) -> bool {
        if text.trim_matches(super::is_space).is_empty() {
            return text.is_empty();
        }
        fields(text).iter().all(|field| {
            let value = field
                .split_once('=')
                .map_or(field.as_str(), |(_, value)| value);
            !value.starts_with('"')
                || value[1..]
                    .find('"')
                    .is_none_or(|position| position == value.len().saturating_sub(2))
        })
    }

    fn read_fields(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut named = BTreeMap::new();
        for (index, field) in fields(text).into_iter().enumerate() {
            if let Some((name, value)) = field.split_once('=') {
                let name = name.trim_start_matches(super::is_space);
                let value = if value.starts_with('"') {
                    unquote(value)
                } else {
                    value.trim_matches(super::is_space)
                };
                named.insert(name.to_owned(), value.to_owned());
            } else {
                let field = unquote(&field);
                let position = if index == 0 {
                    field
                        .find(['#', '.', '%'])
                        .filter(|position| {
                            field[..*position]
                                .chars()
                                .all(|c| alpha_numeric(c) || c == '-' || c == '_')
                        })
                        .unwrap_or(field.len())
                } else {
                    field.len()
                };
                if position != 0 || field.is_empty() {
                    self.positional.push(field[..position].to_owned());
                }
                let mut rest = &field[position..];
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

pub(super) fn formatted_attributes(text: &str) -> Option<Attributes> {
    let mut attributes = Attributes::default();
    let mut rest = text.trim_start_matches(super::is_space);
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
    let values = if attributes.named.is_empty() {
        text
    } else if rest.is_empty() {
        ""
    } else {
        rest.strip_prefix(',')?
    };
    if !values.trim_matches(super::is_space).is_empty() && !Attributes::valid(values) {
        return None;
    }
    if attributes.named.is_empty() {
        let role = fields(text).into_iter().next().unwrap_or_default();
        attributes
            .named
            .insert("role".to_owned(), unquote(&role).to_owned());
    }
    Some(attributes)
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
    let source = line.strip_prefix('[')?;
    let mut quoted = false;
    for (position, character) in source.char_indices() {
        if character == '"' && (quoted || source[position + 1..].contains('"')) {
            quoted = !quoted;
        }
        if character == ']' && !quoted {
            return (source[position + 1..]
                .trim_matches(super::is_space)
                .is_empty()
                && Attributes::valid(&source[..position]))
            .then_some(&source[..position]);
        }
    }
    None
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
    let end = super::inline::attribute_end(&source)?;
    let tail = source
        .get(end + 1..)?
        .split('\n')
        .next()
        .unwrap_or_default();
    if !tail.is_empty() {
        return None;
    }
    let content = source.get(1..end)?;
    if !Attributes::valid(content) {
        return None;
    }
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

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
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
