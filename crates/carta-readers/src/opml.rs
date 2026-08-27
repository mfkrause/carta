//! Outline reader: parses a nested outline of `<outline>` elements into the document model.
//!
//! Each outline becomes a header whose level is its nesting depth (a top-level outline is level 1,
//! its child level 2, and so on). The header inlines come from the outline's `text` attribute,
//! parsed as a fragment of HTML inline markup (so `<strong>`, `<em>`, `<code>`, links, and the like
//! become their inline constructs); the outline's `_note` attribute is parsed as markdown blocks. An
//! outline of `type="link"` wraps its heading content in a link to its `url`. The document metadata
//! is drawn from the document head: `title`, `ownerName` (as the author list), and `dateModified`
//! (as the date), each taken as plain text.
//!
//! XML parsing is permissive: malformed or unbalanced markup is kept where possible.

use std::collections::BTreeMap;

use carta_ast::{Block, Document, Inline, MetaValue, Target};
use carta_core::{Reader, ReaderOptions, Result, presets};

use crate::commonmark::CommonmarkReader;
use crate::html::parse_inline_fragment;
use crate::xml::{Element, Node, parse_tolerant};

/// Parses an outline document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpmlReader;

impl Reader for OpmlReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        let document = parse_tolerant(input.as_bytes(), 512);
        let mut blocks = Vec::new();
        let head = find_child(&document, "head");
        if let Some(body) = find_child(&document, "body") {
            for node in body.elements() {
                emit_outline(node, 1, &mut blocks)?;
            }
        }
        Ok(Document {
            api_version: carta_ast::ApiVersion::default(),
            meta: build_meta(head)
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
            blocks,
        })
    }
}

fn find_child<'a>(document: &'a Element, name: &str) -> Option<&'a Element> {
    for node in document.elements() {
        if node.name == name {
            return Some(node);
        }
        if let Some(found) = node.elements().find(|child| child.name == name) {
            return Some(found);
        }
    }
    None
}

fn attr<'a>(element: &'a Element, name: &str) -> Option<&'a str> {
    element
        .attrs
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn emit_outline(outline: &Element, level: i32, blocks: &mut Vec<Block>) -> Result<()> {
    if outline.name != "outline" {
        return Ok(());
    }
    let heading = attr(outline, "text")
        .map(parse_inline_fragment)
        .unwrap_or_default();
    let heading = if is_link_outline(outline) {
        vec![Inline::Link(
            Box::default(),
            heading,
            Box::new(Target {
                url: attr(outline, "url").unwrap_or_default().to_owned().into(),
                title: carta_ast::Text::default(),
            }),
        )]
    } else {
        heading
    };
    blocks.push(Block::Header(i64::from(level), Box::default(), heading));
    if let Some(note) = attr(outline, "_note") {
        blocks.extend(CommonmarkReader.read(note, &note_options())?.blocks);
    }
    for child in outline.elements() {
        emit_outline(child, level.saturating_add(1), blocks)?;
    }
    Ok(())
}

fn is_link_outline(outline: &Element) -> bool {
    attr(outline, "type").is_some_and(|kind| kind.eq_ignore_ascii_case("link"))
}

fn note_options() -> ReaderOptions {
    let mut options = ReaderOptions::default();
    options.extensions = presets::MARKDOWN;
    options.greedy_paragraphs = true;
    options
}

fn build_meta(head: Option<&Element>) -> BTreeMap<String, MetaValue> {
    let value = |name: &str| {
        head.and_then(|head| head.elements().find(|child| child.name == name))
            .map(direct_text)
    };
    let title = value("title").unwrap_or_default();
    let date = value("dateModified").unwrap_or_default();
    let author = value("ownerName")
        .map(|owner| MetaValue::MetaInlines(tokenize_meta(&owner)))
        .into_iter()
        .collect();
    BTreeMap::from([
        (
            "title".to_owned(),
            MetaValue::MetaInlines(tokenize_meta(&title)),
        ),
        ("author".to_owned(), MetaValue::MetaList(author)),
        (
            "date".to_owned(),
            MetaValue::MetaInlines(tokenize_meta(&date)),
        ),
    ])
}

fn direct_text(element: &Element) -> String {
    let mut text = String::new();
    for node in &element.children {
        if let Node::Text(value) = node {
            text.push_str(value);
        }
    }
    text
}

fn tokenize_meta(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    let mut word = String::new();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word).into()));
            }
            let mut has_newline = ch == '\n' || ch == '\r';
            while let Some(&next) = chars.peek() {
                if !next.is_whitespace() {
                    break;
                }
                has_newline |= next == '\n' || next == '\r';
                chars.next();
            }
            out.push(if has_newline {
                Inline::SoftBreak
            } else {
                Inline::Space
            });
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word.into()));
    }
    out
}

#[cfg(test)]
mod tests;
