//! Outline reader: parses a nested outline of `<outline>` elements into the document model.
//!
//! Each outline becomes a header whose level is its nesting depth (a top-level outline is level 1,
//! its child level 2, and so on). The header inlines come from the outline's `text` attribute,
//! tokenized as plain text on whitespace; the outline's `_note` attribute is parsed as markdown
//! blocks. The document metadata is drawn from the document head: `title`, `ownerName` (as the
//! author list), and `dateModified` (as the date).
//!
//! XML is parsed by a small hand-written scanner over the subset the format uses — elements,
//! attributes with entity decoding, self-closing tags, and nesting. The scanner is panic-free on
//! malformed input: unrecognized or unbalanced markup is skipped rather than rejected.

use std::collections::BTreeMap;

use carta_ast::{Attr, Block, Document, Inline, MetaValue};
use carta_core::{Reader, ReaderOptions, Result};

use crate::commonmark::CommonmarkReader;

/// Parses an outline document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpmlReader;

impl Reader for OpmlReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        let nodes = parse_nodes(input);
        let mut blocks = Vec::new();
        let head = find_child(&nodes, "head");
        let body = find_child(&nodes, "body");
        for node in body.map(element_children).unwrap_or_default() {
            emit_outline(node, 1, &mut blocks)?;
        }
        Ok(Document {
            api_version: carta_ast::ApiVersion::default(),
            meta: build_meta(head),
            blocks,
        })
    }
}

/// A parsed XML element with its decoded attributes and its element children. Text nodes are not
/// retained: the format carries its content in attributes.
#[derive(Debug)]
struct Element {
    name: String,
    attributes: BTreeMap<String, String>,
    children: Vec<Element>,
}

fn element_children(element: &Element) -> Vec<&Element> {
    element.children.iter().collect()
}

/// The first descendant search is shallow by design: `head` and `body` are direct children of the
/// document root, found among the top-level parse and the root `opml` element's children.
fn find_child<'a>(nodes: &'a [Element], name: &str) -> Option<&'a Element> {
    for node in nodes {
        if node.name == name {
            return Some(node);
        }
        if let Some(found) = node.children.iter().find(|child| child.name == name) {
            return Some(found);
        }
    }
    None
}

fn emit_outline(outline: &Element, level: i32, blocks: &mut Vec<Block>) -> Result<()> {
    if outline.name != "outline" {
        return Ok(());
    }
    let heading = outline
        .attributes
        .get("text")
        .map(|text| tokenize(text))
        .unwrap_or_default();
    blocks.push(Block::Header(level, Attr::default(), heading));
    if let Some(note) = outline.attributes.get("_note") {
        let parsed = CommonmarkReader.read(note, &ReaderOptions::default())?;
        blocks.extend(parsed.blocks);
    }
    for child in &outline.children {
        emit_outline(child, level + 1, blocks)?;
    }
    Ok(())
}

fn build_meta(head: Option<&Element>) -> BTreeMap<String, MetaValue> {
    let mut meta = BTreeMap::new();
    let value = |name: &str| {
        head.and_then(|head| head.children.iter().find(|child| child.name == name))
            .and_then(|element| element.attributes.get("__text"))
            .map(String::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let title = tokenize(&value("title"));
    let owner = tokenize(&value("ownerName"));
    let date = tokenize(&value("dateModified"));
    let author = if owner.is_empty() {
        Vec::new()
    } else {
        vec![MetaValue::MetaInlines(owner)]
    };
    meta.insert("title".to_owned(), MetaValue::MetaInlines(title));
    meta.insert("author".to_owned(), MetaValue::MetaList(author));
    meta.insert("date".to_owned(), MetaValue::MetaInlines(date));
    meta
}

/// Tokenize plain text into inlines: whitespace runs collapse to a single `Space`, leading and
/// trailing whitespace is dropped, and each maximal non-whitespace run becomes one `Str`.
fn tokenize(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    for (index, word) in text.split_whitespace().enumerate() {
        if index > 0 {
            out.push(Inline::Space);
        }
        out.push(Inline::Str(word.to_owned()));
    }
    out
}

/// Parse the top-level elements of a document. Anything outside an element (prolog, stray text) is
/// skipped.
fn parse_nodes(input: &str) -> Vec<Element> {
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    let mut nodes = Vec::new();
    while let Some(element) = next_element(&chars, &mut pos) {
        nodes.push(element);
    }
    nodes
}

/// Scan the next element starting at or after `pos`. Returns `None` at end of input. Comments,
/// processing instructions, declarations, and DOCTYPE are skipped; text between elements is
/// captured into the parent via [`parse_children`].
fn next_element(chars: &[char], pos: &mut usize) -> Option<Element> {
    loop {
        skip_to_tag(chars, pos);
        if *pos >= chars.len() {
            return None;
        }
        if skip_non_element(chars, pos) {
            continue;
        }
        return parse_element(chars, pos);
    }
}

/// Skip past characters until the next `<`.
fn skip_to_tag(chars: &[char], pos: &mut usize) {
    while let Some(&ch) = chars.get(*pos) {
        if ch == '<' {
            return;
        }
        *pos += 1;
    }
}

/// If the tag at `pos` is a comment, processing instruction, declaration, or DOCTYPE, skip it and
/// return `true`. A closing tag is also consumed here so a caller scanning siblings stops.
fn skip_non_element(chars: &[char], pos: &mut usize) -> bool {
    if starts_with(chars, *pos, "<!--") {
        skip_until(chars, pos, "-->");
        return true;
    }
    if starts_with(chars, *pos, "<?") {
        skip_until(chars, pos, "?>");
        return true;
    }
    if starts_with(chars, *pos, "<!") {
        skip_until(chars, pos, ">");
        return true;
    }
    false
}

/// Parse one element whose `<` is at `pos`, including its children up to the matching close tag.
fn parse_element(chars: &[char], pos: &mut usize) -> Option<Element> {
    if chars.get(*pos) != Some(&'<') {
        return None;
    }
    *pos += 1;
    let name = read_name(chars, pos);
    if name.is_empty() {
        skip_until(chars, pos, ">");
        return None;
    }
    let mut attributes = BTreeMap::new();
    loop {
        skip_whitespace(chars, pos);
        match chars.get(*pos) {
            None => {
                return Some(Element {
                    name,
                    attributes,
                    children: Vec::new(),
                });
            }
            Some('/') => {
                *pos += 1;
                skip_until(chars, pos, ">");
                return Some(Element {
                    name,
                    attributes,
                    children: Vec::new(),
                });
            }
            Some('>') => {
                *pos += 1;
                break;
            }
            Some(_) => {
                if let Some((key, value)) = read_attribute(chars, pos) {
                    attributes.insert(key, value);
                } else {
                    *pos += 1;
                }
            }
        }
    }
    let (children, text) = parse_children(chars, pos);
    if !text.is_empty() {
        attributes.insert("__text".to_owned(), text);
    }
    Some(Element {
        name,
        attributes,
        children,
    })
}

/// Parse the content of an open element up to its matching `</name>`: nested elements become
/// children, and the concatenated raw text (entity-decoded) is returned for leaf elements.
fn parse_children(chars: &[char], pos: &mut usize) -> (Vec<Element>, String) {
    let mut children = Vec::new();
    let mut text = String::new();
    loop {
        let mut run = String::new();
        while let Some(&ch) = chars.get(*pos) {
            if ch == '<' {
                break;
            }
            run.push(ch);
            *pos += 1;
        }
        text.push_str(&decode_entities(&run));
        if *pos >= chars.len() {
            break;
        }
        if starts_with(chars, *pos, "</") {
            *pos += 2;
            let _ = read_name(chars, pos);
            skip_until(chars, pos, ">");
            break;
        }
        if skip_non_element(chars, pos) {
            continue;
        }
        if let Some(child) = parse_element(chars, pos) {
            children.push(child);
        } else {
            skip_to_tag(chars, pos);
            *pos = (*pos).saturating_add(1);
        }
    }
    (children, text.trim().to_owned())
}

fn read_name(chars: &[char], pos: &mut usize) -> String {
    let mut name = String::new();
    while let Some(&ch) = chars.get(*pos) {
        if ch.is_whitespace() || ch == '>' || ch == '/' {
            break;
        }
        name.push(ch);
        *pos += 1;
    }
    name
}

/// Read one `key="value"` (or single-quoted) attribute. Returns `None` when the cursor is not at a
/// name character.
fn read_attribute(chars: &[char], pos: &mut usize) -> Option<(String, String)> {
    let key = read_attr_name(chars, pos);
    if key.is_empty() {
        return None;
    }
    skip_whitespace(chars, pos);
    if chars.get(*pos) != Some(&'=') {
        return Some((key, String::new()));
    }
    *pos += 1;
    skip_whitespace(chars, pos);
    let Some(&quote @ ('"' | '\'')) = chars.get(*pos) else {
        return Some((key, String::new()));
    };
    *pos += 1;
    let mut raw = String::new();
    while let Some(&ch) = chars.get(*pos) {
        if ch == quote {
            *pos += 1;
            break;
        }
        raw.push(ch);
        *pos += 1;
    }
    Some((key, decode_entities(&raw)))
}

fn read_attr_name(chars: &[char], pos: &mut usize) -> String {
    let mut name = String::new();
    while let Some(&ch) = chars.get(*pos) {
        if ch.is_whitespace() || ch == '=' || ch == '>' || ch == '/' {
            break;
        }
        name.push(ch);
        *pos += 1;
    }
    name
}

fn skip_whitespace(chars: &[char], pos: &mut usize) {
    while let Some(&ch) = chars.get(*pos) {
        if !ch.is_whitespace() {
            return;
        }
        *pos += 1;
    }
}

fn starts_with(chars: &[char], pos: usize, prefix: &str) -> bool {
    prefix
        .chars()
        .enumerate()
        .all(|(offset, expected)| chars.get(pos + offset) == Some(&expected))
}

/// Advance the cursor past the next occurrence of `marker`, consuming the marker. If the marker is
/// absent the cursor moves to the end of input.
fn skip_until(chars: &[char], pos: &mut usize, marker: &str) {
    let marker_len = marker.chars().count();
    while *pos < chars.len() {
        if starts_with(chars, *pos, marker) {
            *pos += marker_len;
            return;
        }
        *pos += 1;
    }
}

/// Decode the XML entity references the format uses: the five named entities and numeric character
/// references in decimal and hexadecimal. An unrecognized or malformed reference is left verbatim.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    while let Some(&ch) = chars.get(pos) {
        if ch != '&' {
            out.push(ch);
            pos += 1;
            continue;
        }
        let Some(end) = (pos + 1..chars.len()).find(|&index| chars.get(index) == Some(&';')) else {
            out.push('&');
            pos += 1;
            continue;
        };
        let body: String = chars.get(pos + 1..end).unwrap_or_default().iter().collect();
        if let Some(decoded) = decode_reference(&body) {
            out.push_str(&decoded);
            pos = end + 1;
        } else {
            out.push('&');
            pos += 1;
        }
    }
    out
}

fn decode_reference(body: &str) -> Option<String> {
    match body {
        "amp" => Some("&".to_owned()),
        "lt" => Some("<".to_owned()),
        "gt" => Some(">".to_owned()),
        "quot" => Some("\"".to_owned()),
        "apos" => Some("'".to_owned()),
        _ => {
            let code =
                if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()?
                } else if let Some(dec) = body.strip_prefix('#') {
                    dec.parse().ok()?
                } else {
                    return None;
                };
            char::from_u32(code).map(|ch| ch.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(input: &str) -> Document {
        OpmlReader
            .read(input, &ReaderOptions::default())
            .expect("outline input parses")
    }

    fn headers(document: &Document) -> Vec<(i32, String)> {
        document
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Header(level, _, inlines) => Some((*level, inline_text(inlines))),
                _ => None,
            })
            .collect()
    }

    fn inline_text(inlines: &[Inline]) -> String {
        inlines
            .iter()
            .map(|inline| match inline {
                Inline::Str(text) => text.as_str(),
                Inline::Space => " ",
                _ => "",
            })
            .collect()
    }

    #[test]
    fn nesting_assigns_header_levels() {
        let document = read(
            "<opml><body>\
             <outline text=\"A\">\
             <outline text=\"B\"><outline text=\"C\"/></outline>\
             </outline>\
             </body></opml>",
        );
        assert_eq!(
            headers(&document),
            [
                (1, "A".to_owned()),
                (2, "B".to_owned()),
                (3, "C".to_owned()),
            ]
        );
    }

    #[test]
    fn sibling_outlines_share_a_level() {
        let document = read("<opml><body><outline text=\"A\"/><outline text=\"B\"/></body></opml>");
        assert_eq!(
            headers(&document),
            [(1, "A".to_owned()), (1, "B".to_owned())]
        );
    }

    #[test]
    fn note_attribute_parses_as_markdown() {
        let document = read("<opml><body><outline text=\"H\" _note=\"**b**\"/></body></opml>");
        assert!(matches!(
            document.blocks.first(),
            Some(Block::Header(1, _, _))
        ));
        let Some(Block::Para(inlines)) = document.blocks.get(1) else {
            panic!("expected the note to parse into a paragraph");
        };
        assert!(matches!(inlines.first(), Some(Inline::Strong(_))));
    }

    #[test]
    fn text_attribute_tokenizes_on_whitespace() {
        let document = read("<opml><body><outline text=\"Hello   World\"/></body></opml>");
        let Some(Block::Header(_, _, inlines)) = document.blocks.first() else {
            panic!("expected a header");
        };
        assert!(matches!(
            inlines.as_slice(),
            [Inline::Str(first), Inline::Space, Inline::Str(second)]
                if first == "Hello" && second == "World"
        ));
    }

    #[test]
    fn missing_text_attribute_yields_an_empty_heading() {
        let document = read("<opml><body><outline/></body></opml>");
        assert_eq!(headers(&document), [(1, String::new())]);
    }

    #[test]
    fn single_quoted_attributes_are_read() {
        let document = read("<opml><body><outline text='quoted'/></body></opml>");
        assert_eq!(headers(&document), [(1, "quoted".to_owned())]);
    }

    #[test]
    fn comments_instructions_and_doctype_are_skipped() {
        let document = read(
            "<?xml version=\"1.0\"?><!DOCTYPE opml><opml><!-- c -->\
             <body><outline text=\"A\"/></body></opml>",
        );
        assert_eq!(headers(&document), [(1, "A".to_owned())]);
    }

    #[test]
    fn metadata_is_drawn_from_the_head() {
        let document = read(
            "<opml><head><title>T</title><ownerName>Me</ownerName>\
             <dateModified>2020</dateModified></head><body></body></opml>",
        );
        assert!(matches!(
            document.meta.get("title"),
            Some(MetaValue::MetaInlines(inlines)) if inline_text(inlines) == "T"
        ));
        assert!(matches!(
            document.meta.get("date"),
            Some(MetaValue::MetaInlines(inlines)) if inline_text(inlines) == "2020"
        ));
        let Some(MetaValue::MetaList(authors)) = document.meta.get("author") else {
            panic!("expected an author list");
        };
        assert!(matches!(
            authors.first(),
            Some(MetaValue::MetaInlines(inlines)) if inline_text(inlines) == "Me"
        ));
    }

    #[test]
    fn absent_owner_yields_an_empty_author_list() {
        let document = read("<opml><head><title>T</title></head><body></body></opml>");
        assert!(matches!(
            document.meta.get("author"),
            Some(MetaValue::MetaList(authors)) if authors.is_empty()
        ));
    }

    #[test]
    fn named_entities_decode() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &quot;d&quot; &apos;e&apos;"),
            "a & b <c> \"d\" 'e'"
        );
    }

    #[test]
    fn numeric_entities_decode_in_decimal_and_hex() {
        assert_eq!(decode_entities("&#65;&#x42;&#X43;"), "ABC");
    }

    #[test]
    fn malformed_or_unknown_references_are_left_verbatim() {
        assert_eq!(decode_entities("&amp"), "&amp");
        assert_eq!(decode_entities("&nosuch;"), "&nosuch;");
        assert_eq!(decode_entities("&#zz;"), "&#zz;");
        assert_eq!(decode_entities("bare & text"), "bare & text");
    }

    #[test]
    fn malformed_markup_does_not_panic() {
        let _ = read("<opml><body><outline text=\"x\"><outline text=\"y\"></body>");
        let _ = read("<<<>>><opml attr");
        let _ = read("");
    }
}
