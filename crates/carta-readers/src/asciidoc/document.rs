use super::attributes::block_attributes_at;
use super::{AUTO_HEADING, REFERENCE_LABEL};
use super::{Parser, blank_line, is_space, space_words, text_inlines, trim_space};
use carta_ast::{Block, Document, Inline, MetaValue};
use std::collections::BTreeMap;

impl Parser {
    #[allow(
        clippy::too_many_lines,
        clippy::indexing_slicing,
        reason = "Header parsing bounds each source position."
    )]
    pub(super) fn document(&mut self, source: &str) -> Document {
        let header_notes = (self.include_depth > 0).then(|| std::mem::take(&mut self.footnotes));
        let header_anchors = (self.include_depth > 0).then(|| std::mem::take(&mut self.anchors));
        let lines: Vec<&str> = source.lines().collect();
        let mut position = 0;
        skip_header_blanks(&lines, &mut position);
        let mut document = Document::default();
        self.header_attributes(&lines, &mut position);
        skip_header_blanks(&lines, &mut position);
        let mut title_position = position;
        while let Some(line) = lines.get(title_position) {
            let attributes = block_attributes_at(&lines, title_position);
            if attributes.is_some()
                || line.starts_with("[[") && line.ends_with("]]")
                || line
                    .strip_prefix('.')
                    .is_some_and(|rest| !rest.starts_with([' ', '.']))
            {
                title_position += attributes.map_or(1, |(_, count)| count);
                skip_header_blanks(&lines, &mut title_position);
            } else {
                break;
            }
        }
        if lines
            .get(title_position)
            .is_some_and(|line| line.starts_with("= ") || line.starts_with("# "))
        {
            position = title_position;
        }
        let mut title_meta = None;
        if let Some(title) = lines
            .get(position)
            .and_then(|s| s.strip_prefix("= ").or_else(|| s.strip_prefix("# ")))
        {
            if !trim_space(title).is_empty() {
                title_meta = Some(MetaValue::MetaInlines(self.inlines(title)));
            }
            position += 1;
            if let Some(author) = lines.get(position).filter(|s| {
                !trim_space(title).is_empty()
                    && !s.is_empty()
                    && !s.starts_with(is_space)
                    && !s.starts_with(':')
            }) {
                let authors = author
                    .split(';')
                    .map(|author| {
                        let author = trim_space(author);
                        let mut nodes = Vec::new();
                        if let Some((name, email)) = author.split_once('<') {
                            nodes.extend(text_inlines(trim_space(name)));
                            nodes.push(Inline::Space);
                            nodes.push(Inline::Str("(".into()));
                            let email = email.split('>').next().unwrap_or_default();
                            nodes.push(Inline::Link(
                                Box::default(),
                                vec![Inline::Str(email.into())],
                                Box::new(carta_ast::Target {
                                    url: format!("mailto:{email}").into(),
                                    title: "".into(),
                                }),
                            ));
                            nodes.push(Inline::Str(")".into()));
                        } else {
                            nodes = text_inlines(author);
                        }
                        MetaValue::MetaInlines(nodes)
                    })
                    .collect();
                let authors = MetaValue::MetaList(authors);
                if self.include_depth == 0 {
                    self.header_meta.insert("author".into(), authors.clone());
                }
                document.meta.insert("author".into(), authors);
                position += 1;
                if let Some((version, date, remark)) =
                    lines.get(position).and_then(|line| revision(line))
                {
                    let version = MetaValue::MetaString(version.into());
                    if self.include_depth == 0 {
                        self.header_meta.insert("version".into(), version.clone());
                    }
                    document.meta.insert("version".into(), version);
                    if let Some(date) = date {
                        let date = MetaValue::MetaString(date.into());
                        if self.include_depth == 0 {
                            self.header_meta.insert("date".into(), date.clone());
                        }
                        document.meta.insert("date".into(), date);
                    }
                    if let Some(remark) = remark {
                        let remark = MetaValue::MetaString(remark.into());
                        if self.include_depth == 0 {
                            self.header_meta.insert("remark".into(), remark.clone());
                        }
                        document.meta.insert("remark".into(), remark);
                    }
                    position += 1;
                }
            }
        }
        self.header_attributes(&lines, &mut position);
        if let Some(title) = title_meta {
            if self.include_depth == 0 {
                self.header_meta.insert("title".into(), title.clone());
            }
            document.meta.insert("title".into(), title);
        }
        self.hard_breaks |= self
            .attributes
            .get("hardbreaks-option")
            .is_some_and(String::is_empty);
        if let Some(notes) = header_notes {
            self.footnotes = notes;
        }
        if let Some(anchors) = header_anchors {
            self.anchors = anchors;
        }
        document.blocks = self.blocks(&lines[position..]);
        if self.include_depth > 0 {
            return document;
        }
        self.identifiers.clear();
        assign_heading_ids(
            &mut document.blocks,
            &self.attributes,
            &mut self.identifiers,
        );
        let mut anchors = finish_section_ids(&mut document);
        for value in document.meta.values_mut() {
            assign_metadata_ids(value, &self.attributes, &mut self.identifiers);
        }
        for (identifier, label) in &self.anchors {
            anchors
                .entry(identifier.clone())
                .or_insert_with(|| label.clone());
        }
        for (name, value) in &self.attributes {
            if matches!(name.as_str(), "sectids" | "stem") || name.starts_with('!') {
                continue;
            }
            document
                .meta
                .entry(name.as_str().into())
                .or_insert_with(|| {
                    if value.is_empty() {
                        MetaValue::MetaBool(true)
                    } else {
                        MetaValue::MetaString(value.as_str().into())
                    }
                });
        }
        super::resolve::attributes(&mut document, &self.attributes);
        // Resolve copied labels once, including labels that refer to themselves.
        for nodes in anchors.values_mut() {
            super::resolve::inlines(nodes, &self.attributes);
            let mut label = Document {
                blocks: vec![Block::Plain(std::mem::take(nodes))],
                ..Document::default()
            };
            carta_core::walk::for_each_document_inline(&mut label, &mut |inline| {
                if let Inline::RawInline(format, target) = inline {
                    if format.0 == REFERENCE_LABEL {
                        *inline = Inline::Str(target.clone());
                    } else if format.0 == super::MONOSPACE_REFERENCE {
                        *inline = Inline::Code(Box::default(), target.clone());
                    }
                }
            });
            if let Some(Block::Plain(contents)) = label.blocks.first_mut() {
                *nodes = std::mem::take(contents);
            }
        }
        carta_core::walk::for_each_document_inline(&mut document, &mut |inline| {
            if let Inline::Link(_, label, target) = inline
                && matches!(
                    label.as_slice(),
                    [Inline::RawInline(format, _)]
                        if format.0 == REFERENCE_LABEL || format.0 == super::MONOSPACE_REFERENCE
                )
            {
                let monospace = matches!(
                    label.as_slice(),
                    [Inline::RawInline(format, _)] if format.0 == super::MONOSPACE_REFERENCE
                );
                let identifier = target.url.rsplit('#').next().unwrap_or_default();
                if let Some(contents) = anchors.get(identifier) {
                    label.clone_from(contents);
                    target.url = format!("#{identifier}").into();
                } else {
                    *label = vec![Inline::Str(
                        target.url.strip_prefix('#').unwrap_or(&target.url).into(),
                    )];
                }
                if monospace {
                    super::inline::monospace(label);
                }
            }
        });
        document
    }

    fn header_attributes(&mut self, lines: &[&str], position: &mut usize) {
        while self.attribute_line(lines, position) {}
    }

    pub(super) fn attribute_line(&mut self, lines: &[&str], position: &mut usize) -> bool {
        let Some(line) = lines.get(*position).and_then(|s| s.strip_prefix(':')) else {
            return false;
        };
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        let attribute = name.strip_prefix('!').unwrap_or(name);
        if !attribute
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            || !attribute
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return false;
        }
        *position += 1;
        if name.starts_with('!') {
            self.attributes.remove(name.trim_matches('!'));
            self.attributes
                .insert(format!("!{}", name.trim_matches('!')), String::new());
        } else {
            let mut value = value.trim_start_matches(' ').to_owned();
            while value.ends_with('\\') && *position < lines.len() {
                value.truncate(value.len() - 1);
                if value.ends_with(" +") {
                    value.truncate(value.len() - 2);
                    value.push('\n');
                }
                if let Some(line) = lines.get(*position) {
                    value.push_str(line.trim_start_matches(' '));
                }
                *position += 1;
            }
            self.attributes.insert(name.to_owned(), value);
        }
        true
    }
}

fn skip_header_blanks(lines: &[&str], position: &mut usize) {
    while lines.get(*position).is_some_and(|line| blank_line(line)) {
        *position += 1;
    }
}

pub(super) fn builtin_attribute(name: &str) -> Option<&'static str> {
    Some(match name {
        "blank" | "empty" => "",
        "sp" => " ",
        "nbsp" => "\u{a0}",
        "zwsp" => "\u{200b}",
        "wj" => "\u{2060}",
        "tilde" => "~",
        "cxx" | "cpp" => "C++",
        "pp" => "++",
        "apos" => "'",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "deg" => "°",
        "plus" => "+",
        "brvbar" => "¦",
        "vbar" => "|",
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "startsb" => "[",
        "endsb" => "]",
        "caret" => "^",
        "asterisk" => "*",
        "backslash" => "\\",
        "backtick" => "`",
        "two-colons" => "::",
        "two-semicolons" => ";;",
        _ => return None,
    })
}

pub(super) fn identifier_text(nodes: &[Inline]) -> String {
    let mut text = String::new();
    for node in nodes {
        match node {
            Inline::Span(attr, _)
                if attr
                    .attributes
                    .iter()
                    .any(|(name, _)| name == super::IDENTIFIER_SOURCE) =>
            {
                if let Some((_, source)) = attr
                    .attributes
                    .iter()
                    .find(|(name, _)| name == super::IDENTIFIER_SOURCE)
                {
                    text.push_str(source);
                }
            }
            Inline::Str(value) | Inline::Code(_, value) => text.push_str(value),
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => text.push(' '),
            Inline::Strong(children)
            | Inline::Emph(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::Quoted(_, children)
            | Inline::Span(_, children)
            | Inline::Link(_, children, _) => text.push_str(&identifier_text(children)),
            Inline::Note(blocks) => {
                for block in blocks {
                    if let Block::Para(nodes) = block {
                        text.push_str(&identifier_text(nodes));
                    }
                }
            }
            _ => {}
        }
    }
    text
}

pub(super) fn section_level(block: &Block) -> Option<i64> {
    let (Block::Header(_, attr, _) | Block::Div(attr, _)) = block else {
        return None;
    };
    attr.attributes
        .iter()
        .find(|(name, _)| name == super::SECTION)
        .and_then(|(_, level)| level.parse().ok())
}

fn finish_section_ids(document: &mut Document) -> BTreeMap<String, Vec<Inline>> {
    carta_core::walk::for_each_document_block(document, &mut |block| {
        let Some(level) = section_level(block) else {
            return;
        };
        let Block::Div(attr, contents) = block else {
            return;
        };
        let header = contents.iter().position(|child| {
            matches!(child, Block::Header(..)) && section_level(child) == Some(level)
        });
        let Some(header) = header else {
            return;
        };
        if let Some(Block::Header(_, header_attr, _)) = contents.get_mut(header) {
            if attr.id.is_empty() {
                attr.id.clone_from(&header_attr.id);
            }
            header_attr.id = "".into();
        }
    });

    let mut anchors = BTreeMap::new();
    carta_core::walk::for_each_document_block(document, &mut |block| {
        let level = section_level(block);
        match block {
            Block::Header(_, attr, label) if level.is_some() && !attr.id.is_empty() => {
                anchors
                    .entry(attr.id.to_string())
                    .or_insert_with(|| label.clone());
            }
            Block::Div(attr, contents) => {
                let Some(level) = level else {
                    return;
                };
                let label = contents.iter().find_map(|child| match child {
                    Block::Header(_, _, label) if section_level(child) == Some(level) => {
                        Some(label)
                    }
                    _ => None,
                });
                if !attr.id.is_empty()
                    && let Some(label) = label
                {
                    anchors
                        .entry(attr.id.to_string())
                        .or_insert_with(|| label.clone());
                }
            }
            _ => {}
        }
    });
    carta_core::walk::for_each_document_block(document, &mut |block| {
        let (Block::Header(_, attr, _) | Block::Div(attr, _)) = block else {
            return;
        };
        attr.attributes.retain(|(name, _)| name != super::SECTION);
    });
    anchors
}

fn revision(source: &str) -> Option<(&str, Option<&str>, Option<&str>)> {
    let prefixed = source.starts_with('v');
    let source = source.strip_prefix('v').unwrap_or(source);
    let (version, rest) = source
        .split_once(',')
        .map_or((source, None), |(version, rest)| (version, Some(rest)));
    if version.is_empty() {
        return None;
    }
    let Some(rest) = rest else {
        return prefixed.then_some((version, None, None));
    };
    if !rest.starts_with(is_space) {
        return None;
    }
    let (date, remark) = rest
        .split_once(':')
        .map_or((rest, None), |(date, remark)| (date, Some(remark)));
    if trim_space(date).is_empty() {
        return None;
    }
    if remark.is_some_and(|remark| !remark.starts_with(is_space)) {
        return None;
    }
    Some((version, Some(trim_space(date)), remark.map(trim_space)))
}

fn assign_heading_ids(
    blocks: &mut [Block],
    attributes: &BTreeMap<String, String>,
    identifiers: &mut BTreeMap<String, usize>,
) {
    let mut position = 0;
    while position < blocks.len() {
        let Some(level) = blocks.get(position).and_then(section_level) else {
            if let Some(block) = blocks.get_mut(position) {
                assign_nested_heading_ids(block, attributes, identifiers);
            }
            position += 1;
            continue;
        };
        let mut end = position + 1;
        while end < blocks.len() {
            if blocks
                .get(end)
                .and_then(section_level)
                .is_some_and(|next| next <= level)
            {
                break;
            }
            end += 1;
        }
        if let Some((block, body)) = blocks
            .get_mut(position..end)
            .unwrap_or_default()
            .split_first_mut()
        {
            match block {
                Block::Header(_, attr, label) => {
                    assign_heading_ids(body, attributes, identifiers);
                    assign_header_id(attr, attributes, identifiers);
                    assign_inline_heading_ids(label, attributes, identifiers);
                }
                Block::Div(wrapper_attr, contents) => {
                    assign_heading_ids(body, attributes, identifiers);
                    assign_titled_section_ids(
                        wrapper_attr,
                        contents,
                        level,
                        attributes,
                        identifiers,
                    );
                }
                _ => {}
            }
        }
        position = end;
    }
}

fn assign_titled_section_ids(
    wrapper_attr: &mut carta_ast::Attr,
    contents: &mut [Block],
    level: i64,
    attributes: &BTreeMap<String, String>,
    identifiers: &mut BTreeMap<String, usize>,
) {
    let header = contents.iter().position(|block| {
        matches!(block, Block::Header(..)) && section_level(block) == Some(level)
    });
    let Some(header) = header else {
        assign_heading_ids(contents, attributes, identifiers);
        return;
    };
    assign_heading_ids(
        contents.get_mut(..header).unwrap_or_default(),
        attributes,
        identifiers,
    );
    assign_heading_ids(
        contents.get_mut(header + 1..).unwrap_or_default(),
        attributes,
        identifiers,
    );
    if let Some(Block::Header(_, attr, label)) = contents.get_mut(header) {
        if wrapper_attr.id.is_empty() {
            assign_header_id(attr, attributes, identifiers);
        } else {
            attr.attributes.retain(|(name, _)| name != AUTO_HEADING);
        }
        assign_inline_heading_ids(label, attributes, identifiers);
    }
}

fn assign_header_id(
    attr: &mut carta_ast::Attr,
    attributes: &BTreeMap<String, String>,
    identifiers: &mut BTreeMap<String, usize>,
) {
    let Some(index) = attr
        .attributes
        .iter()
        .position(|(name, _)| name == AUTO_HEADING)
    else {
        return;
    };
    let (_, text) = attr.attributes.remove(index);
    if attributes.contains_key("!sectids") {
        return;
    }
    let prefix = attributes.get("idprefix").map_or("_", String::as_str);
    let separator = attributes.get("idseparator").map_or("_", String::as_str);
    let words = text.to_lowercase().replace(['.', '-'], " ");
    let base = format!("{prefix}{}", space_words(&words).join(separator));
    let count = identifiers.entry(base.clone()).or_default();
    *count += 1;
    attr.id = if *count == 1 {
        base
    } else {
        format!("{base}{separator}{count}")
    }
    .into();
}

fn assign_nested_heading_ids(
    block: &mut Block,
    attributes: &BTreeMap<String, String>,
    identifiers: &mut BTreeMap<String, usize>,
) {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
            assign_inline_heading_ids(inlines, attributes, identifiers);
        }
        Block::LineBlock(lines) => {
            for line in lines {
                assign_inline_heading_ids(line, attributes, identifiers);
            }
        }
        Block::BlockQuote(blocks) | Block::Div(_, blocks) => {
            assign_heading_ids(blocks, attributes, identifiers);
        }
        Block::OrderedList(_, items) | Block::BulletList(items) => {
            for item in items {
                assign_heading_ids(item, attributes, identifiers);
            }
        }
        Block::DefinitionList(entries) => {
            for (term, definitions) in entries {
                assign_inline_heading_ids(term, attributes, identifiers);
                for definition in definitions {
                    assign_heading_ids(definition, attributes, identifiers);
                }
            }
        }
        Block::Figure(_, caption, blocks) => {
            if let Some(short) = &mut caption.short {
                assign_inline_heading_ids(short, attributes, identifiers);
            }
            assign_heading_ids(&mut caption.long, attributes, identifiers);
            assign_heading_ids(blocks, attributes, identifiers);
        }
        Block::Table(table) => {
            if let Some(short) = &mut table.caption.short {
                assign_inline_heading_ids(short, attributes, identifiers);
            }
            assign_heading_ids(&mut table.caption.long, attributes, identifiers);
            assign_table_rows(&mut table.head.rows, attributes, identifiers);
            for body in &mut table.bodies {
                assign_table_rows(&mut body.head, attributes, identifiers);
                assign_table_rows(&mut body.body, attributes, identifiers);
            }
            assign_table_rows(&mut table.foot.rows, attributes, identifiers);
        }
        Block::CodeBlock(..) | Block::RawBlock(..) | Block::HorizontalRule => {}
    }
}

fn assign_table_rows(
    rows: &mut [carta_ast::Row],
    attributes: &BTreeMap<String, String>,
    identifiers: &mut BTreeMap<String, usize>,
) {
    for row in rows {
        for cell in &mut row.cells {
            assign_heading_ids(&mut cell.content, attributes, identifiers);
        }
    }
}

fn assign_inline_heading_ids(
    inlines: &mut [Inline],
    attributes: &BTreeMap<String, String>,
    identifiers: &mut BTreeMap<String, usize>,
) {
    for inline in inlines {
        match inline {
            Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Cite(_, children)
            | Inline::Span(_, children)
            | Inline::Link(_, children, _)
            | Inline::Image(_, children, _) => {
                assign_inline_heading_ids(children, attributes, identifiers);
            }
            Inline::Note(blocks) => assign_heading_ids(blocks, attributes, identifiers),
            Inline::Str(_)
            | Inline::Code(..)
            | Inline::Space
            | Inline::SoftBreak
            | Inline::LineBreak
            | Inline::Math(..)
            | Inline::RawInline(..) => {}
        }
    }
}

fn assign_metadata_ids(
    value: &mut MetaValue,
    attributes: &BTreeMap<String, String>,
    identifiers: &mut BTreeMap<String, usize>,
) {
    match value {
        MetaValue::MetaMap(items) => {
            for value in items.values_mut() {
                assign_metadata_ids(value, attributes, identifiers);
            }
        }
        MetaValue::MetaList(items) => {
            for value in items {
                assign_metadata_ids(value, attributes, identifiers);
            }
        }
        MetaValue::MetaBlocks(blocks) => assign_heading_ids(blocks, attributes, identifiers),
        MetaValue::MetaInlines(inlines) => {
            assign_inline_heading_ids(inlines, attributes, identifiers);
        }
        MetaValue::MetaBool(_) | MetaValue::MetaString(_) => {}
    }
}
