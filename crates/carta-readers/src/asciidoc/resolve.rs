use super::{
    ATTRIBUTE_REFERENCE, IDENTIFIER_SOURCE, MONOSPACE_ATTRIBUTE, MONOSPACE_SCOPE, text_inlines,
};
use carta_ast::{Attr, Block, Document, Inline, MetaValue};
use std::collections::BTreeMap;

struct MonospaceScope {
    output_len: usize,
    merge_code_before: bool,
    merge_text_before: bool,
    emitted_content: bool,
}

pub(super) fn has_term_stem_error(document: &mut Document) -> bool {
    let mut found = false;
    carta_core::walk::for_each_document_inline(document, &mut |inline| {
        found |=
            matches!(inline, Inline::RawInline(format, _) if format.0 == super::TERM_STEM_ERROR);
    });
    found
}

pub(super) fn attributes(document: &mut Document, values: &BTreeMap<String, String>) {
    carta_core::walk::for_each_document_block(document, &mut |block| match block {
        Block::Plain(nodes) | Block::Para(nodes) | Block::Header(_, _, nodes) => {
            inlines(nodes, values);
        }
        Block::LineBlock(lines) => {
            for nodes in lines {
                inlines(nodes, values);
            }
        }
        Block::DefinitionList(entries) => {
            for (term, _) in entries {
                inlines(term, values);
            }
        }
        _ => {}
    });
    for value in document.meta.values_mut() {
        metadata(value, values);
    }
}

fn metadata(value: &mut MetaValue, values: &BTreeMap<String, String>) {
    match value {
        MetaValue::MetaInlines(nodes) => inlines(nodes, values),
        MetaValue::MetaList(items) => {
            for value in items {
                metadata(value, values);
            }
        }
        MetaValue::MetaMap(items) => {
            for value in items.values_mut() {
                metadata(value, values);
            }
        }
        _ => {}
    }
}

pub(super) fn inlines(nodes: &mut Vec<Inline>, values: &BTreeMap<String, String>) {
    inlines_in_scope(nodes, values, 0);
}

#[allow(
    clippy::too_many_lines,
    reason = "Attribute resolution shares text and monospace merge state."
)]
fn inlines_in_scope(
    nodes: &mut Vec<Inline>,
    values: &BTreeMap<String, String>,
    mut monospace_depth: usize,
) {
    let mut output = Vec::with_capacity(nodes.len());
    let mut merge_next_text = false;
    let mut merge_monospace_code = false;
    let mut scopes = Vec::new();
    for mut node in std::mem::take(nodes) {
        if let Some(start) = monospace_scope(&node) {
            if start {
                scopes.push(MonospaceScope {
                    output_len: output.len(),
                    merge_code_before: merge_monospace_code,
                    merge_text_before: merge_next_text,
                    emitted_content: false,
                });
                monospace_depth += 1;
                merge_next_text = false;
                merge_monospace_code = false;
            } else {
                monospace_depth = monospace_depth.saturating_sub(1);
                if let Some(scope) = scopes.pop() {
                    if scope.emitted_content || output.len() > scope.output_len {
                        merge_next_text = false;
                        merge_monospace_code = false;
                    } else {
                        merge_next_text = scope.merge_text_before
                            || matches!(output.last(), Some(Inline::Str(_)));
                        merge_monospace_code = scope.merge_code_before;
                    }
                } else {
                    merge_next_text = false;
                    merge_monospace_code = false;
                }
            }
            continue;
        }
        if let Some((name, attr)) = monospace_attribute(&node) {
            let value = values
                .get(name)
                .cloned()
                .unwrap_or_else(|| format!("{{{name}}}"));
            let mut replacement = text_inlines(&value);
            super::inline::monospace(&mut replacement);
            if let Some(attr) = attr {
                replacement = super::attributes::add_inline_attr(replacement, &attr, true);
            }
            append_converted(
                &mut output,
                replacement,
                monospace_depth.max(1),
                &mut merge_monospace_code,
                &mut scopes,
            );
            merge_next_text = false;
            continue;
        }
        if let Inline::RawInline(format, name) = &node
            && format.0 == ATTRIBUTE_REFERENCE
        {
            let mut replacement = text_inlines(
                &values
                    .get(name.as_str())
                    .cloned()
                    .unwrap_or_else(|| format!("{{{name}}}")),
            );
            if monospace_depth > 0 {
                super::inline::monospace(&mut replacement);
            }
            append_converted(
                &mut output,
                replacement,
                monospace_depth,
                &mut merge_monospace_code,
                &mut scopes,
            );
            merge_next_text = matches!(output.last(), Some(Inline::Str(_)));
            continue;
        }
        if let Inline::Span(attr, children) = &mut node
            && attr
                .attributes
                .iter()
                .any(|(key, _)| key == IDENTIFIER_SOURCE)
        {
            inlines_in_scope(children, values, monospace_depth);
            append_converted(
                &mut output,
                std::mem::take(children),
                monospace_depth,
                &mut merge_monospace_code,
                &mut scopes,
            );
            merge_next_text = matches!(output.last(), Some(Inline::Str(_)));
            continue;
        }
        match &mut node {
            Inline::Emph(children)
            | Inline::Strong(children)
            | Inline::Underline(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Cite(_, children)
            | Inline::Span(_, children)
            | Inline::Link(_, children, _)
            | Inline::Image(_, children, _) => {
                inlines_in_scope(children, values, monospace_depth);
            }
            Inline::Note(blocks) => inlines_in_blocks(blocks, values),
            _ => {}
        }
        if merge_next_text {
            merge_next_text = false;
            if let (Some(Inline::Str(previous)), Inline::Str(text)) = (output.last_mut(), &node) {
                previous.push_str(text);
                merge_monospace_code = false;
                if !text.is_empty() {
                    mark_scope_content(&mut scopes);
                }
                continue;
            }
        }
        if monospace_depth > 0 && matches!(node, Inline::Code(..)) {
            mark_scope_content(&mut scopes);
            if merge_monospace_code {
                super::inline::append_monospace(&mut output, node);
            } else {
                output.push(node);
            }
            merge_monospace_code = true;
            continue;
        }
        merge_monospace_code = false;
        if matches!(&node, Inline::Str(text) if !text.is_empty()) {
            mark_scope_content(&mut scopes);
        }
        mark_scope_content(&mut scopes);
        push_inline(&mut output, node);
    }
    *nodes = output;
}

pub(super) fn footnotes(
    notes: &mut BTreeMap<String, Vec<Inline>>,
    values: &BTreeMap<String, String>,
) {
    for inlines in notes.values_mut() {
        let mut document = Document {
            blocks: vec![Block::Plain(std::mem::take(inlines))],
            ..Document::default()
        };
        attributes(&mut document, values);
        if let Some(Block::Plain(resolved)) = document.blocks.first_mut() {
            *inlines = std::mem::take(resolved);
        }
    }
}

pub(super) fn icons(document: &mut Document, values: &BTreeMap<String, String>) {
    carta_core::walk::for_each_document_inline(document, &mut |inline| {
        if let Inline::Span(attr, _) = inline
            && let Some(index) = attr
                .attributes
                .iter()
                .position(|(name, _)| name == super::ICON_NAME)
        {
            let (_, name) = attr.attributes.remove(index);
            let font = values.get("icons").is_some_and(|value| value == "font");
            let class = if font {
                format!("fa fa-{name}")
            } else {
                "icon".to_owned()
            };
            attr.classes = vec![
                if attr.classes.is_empty() {
                    class
                } else {
                    format!("{} {class}", attr.classes.join(" "))
                }
                .into(),
            ];
            if !font {
                let directory = values
                    .get("iconsdir")
                    .map_or("./images/icons", String::as_str);
                let kind = values.get("icontype").map_or("png", String::as_str);
                *inline = Inline::Image(
                    std::mem::take(attr),
                    vec![],
                    Box::new(carta_ast::Target {
                        url: format!("{directory}/{name}.{kind}").into(),
                        title: "".into(),
                    }),
                );
            }
        }
    });
}

pub(super) fn icons_from_metadata(document: &mut Document) {
    let values = ["icons", "iconsdir", "icontype"]
        .into_iter()
        .filter_map(|name| {
            let value = match document.meta.get(name) {
                Some(MetaValue::MetaString(value)) => value.to_string(),
                Some(MetaValue::MetaBool(true)) => String::new(),
                _ => return None,
            };
            Some((name.to_owned(), value))
        })
        .collect();
    icons(document, &values);
}

fn append_converted(
    output: &mut Vec<Inline>,
    nodes: Vec<Inline>,
    monospace_depth: usize,
    merge_monospace_code: &mut bool,
    scopes: &mut [MonospaceScope],
) {
    for node in nodes {
        if monospace_depth > 0 && matches!(node, Inline::Code(..)) {
            mark_scope_content(scopes);
            if *merge_monospace_code {
                super::inline::append_monospace(output, node);
            } else {
                output.push(node);
            }
            *merge_monospace_code = true;
        } else if let Inline::Str(text) = &node {
            super::push_text(output, text);
            *merge_monospace_code = false;
            if !text.is_empty() {
                mark_scope_content(scopes);
            }
        } else {
            push_inline(output, node);
            *merge_monospace_code = false;
            mark_scope_content(scopes);
        }
    }
}

fn mark_scope_content(scopes: &mut [MonospaceScope]) {
    for scope in scopes {
        scope.emitted_content = true;
    }
}

fn monospace_scope(node: &Inline) -> Option<bool> {
    let marker = match node {
        Inline::RawInline(format, value) if format.0 == MONOSPACE_SCOPE => Some(value.as_str()),
        Inline::Span(attr, children) if is_inline_wrapper(attr) => match children.as_slice() {
            [Inline::RawInline(format, value)] if format.0 == MONOSPACE_SCOPE => {
                Some(value.as_str())
            }
            _ => None,
        },
        _ => None,
    }?;
    match marker {
        "start" => Some(true),
        "end" => Some(false),
        _ => None,
    }
}

fn monospace_attribute(node: &Inline) -> Option<(&str, Option<Attr>)> {
    match node {
        Inline::RawInline(format, name) if format.0 == MONOSPACE_ATTRIBUTE => {
            Some((name.as_str(), None))
        }
        Inline::Span(attr, children) if is_inline_wrapper(attr) => match children.as_slice() {
            [Inline::RawInline(format, name)] if format.0 == MONOSPACE_ATTRIBUTE => {
                let mut attr = (**attr).clone();
                attr.attributes.remove(0);
                Some((name.as_str(), Some(attr)))
            }
            _ => None,
        },
        _ => None,
    }
}

fn is_inline_wrapper(attr: &Attr) -> bool {
    attr.attributes
        .first()
        .is_some_and(|(name, value)| name == "wrapper" && value == "1")
}

fn inlines_in_blocks(blocks: &mut [Block], values: &BTreeMap<String, String>) {
    carta_core::walk::for_each_block(blocks, &mut |block| match block {
        Block::Plain(nodes) | Block::Para(nodes) | Block::Header(_, _, nodes) => {
            inlines_in_scope(nodes, values, 0);
        }
        Block::LineBlock(lines) => {
            for nodes in lines {
                inlines_in_scope(nodes, values, 0);
            }
        }
        Block::DefinitionList(entries) => {
            for (term, _) in entries {
                inlines_in_scope(term, values, 0);
            }
        }
        Block::Figure(_, caption, _) => {
            if let Some(short) = &mut caption.short {
                inlines_in_scope(short, values, 0);
            }
        }
        Block::Table(table) => {
            if let Some(short) = &mut table.caption.short {
                inlines_in_scope(short, values, 0);
            }
        }
        _ => {}
    });
}

fn push_inline(output: &mut Vec<Inline>, mut node: Inline) {
    if matches!(node, Inline::Space) && matches!(output.last(), Some(Inline::Space)) {
        return;
    }
    match (&mut output.last_mut(), &mut node) {
        (Some(Inline::Strong(previous)), Inline::Strong(next))
        | (Some(Inline::Emph(previous)), Inline::Emph(next)) => previous.append(next),
        _ => output.push(node),
    }
}
