//! A permissive read-only XML parser shared by the container-format readers.
//!
//! Zipped-XML packages ship no DTD a reader must honor, so a permissive well-formed-XML scan
//! suffices. The parser is iterative and never recurses, so adversarially deep markup cannot
//! overflow the stack; a caller-supplied depth ceiling bounds the materialized tree instead:
//! content nested past the ceiling is kept but not descended into. Two entry points serve the two
//! shapes callers need: [`parse`] returns the single root element or `None` when the input holds no
//! element, while [`parse_tolerant`] never fails, folding every top-level node (and anything left
//! open by truncated input) into a synthetic root whose children are the document's top-level
//! nodes.

use crate::xml_entities::decode_entities;

/// A node within an element: a child element or a run of character data.
#[derive(Debug, Clone)]
pub(crate) enum Node {
    Element(Element),
    Text(String),
}

/// A parsed XML element: its qualified tag name, attributes in source order, and child nodes.
#[derive(Debug, Clone, Default)]
pub(crate) struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

/// The local part of a qualified name (`w:pStyle` → `pStyle`, `dc:title` → `title`).
pub(crate) fn local_name(name: &str) -> &str {
    match name.rsplit_once(':') {
        Some((_, tail)) => tail,
        None => name,
    }
}

impl Element {
    /// The value of the attribute whose local name is `key`, if any.
    pub(crate) fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| local_name(name) == key)
            .map(|(_, value)| value.as_str())
    }

    /// The value of the attribute with exactly the qualified name `qualified`, falling back to any
    /// attribute sharing its local part `key`. Used for relationship references (`r:id`, `r:embed`)
    /// whose namespace prefix is conventional but whose local name is unambiguous in context.
    #[cfg(feature = "docx")]
    pub(crate) fn attr_qualified(&self, qualified: &str, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| name == qualified)
            .or_else(|| self.attrs.iter().find(|(name, _)| local_name(name) == key))
            .map(|(_, value)| value.as_str())
    }

    /// The child elements in order.
    pub(crate) fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            Node::Text(_) => None,
        })
    }

    /// The first child element with local name `key`.
    pub(crate) fn child(&self, key: &str) -> Option<&Element> {
        self.elements()
            .find(|element| local_name(&element.name) == key)
    }

    /// The first descendant element with local name `key`, searched depth-first.
    pub(crate) fn descendant(&self, key: &str) -> Option<&Element> {
        for element in self.elements() {
            if local_name(&element.name) == key {
                return Some(element);
            }
            if let Some(found) = element.descendant(key) {
                return Some(found);
            }
        }
        None
    }

    /// The concatenated character data of this element and its descendants.
    pub(crate) fn text(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut String) {
        for node in &self.children {
            match node {
                Node::Text(text) => out.push_str(text),
                Node::Element(element) => element.collect_text(out),
            }
        }
    }

    /// Consumes a synthetic root, yielding its first top-level element.
    fn into_first_element(self) -> Option<Element> {
        self.children.into_iter().find_map(|node| match node {
            Node::Element(element) => Some(element),
            Node::Text(_) => None,
        })
    }
}

/// Parses `input` into the single root element, returning `None` when it holds no element (empty,
/// blank, or non-markup input). `max_depth` bounds materialized nesting.
pub(crate) fn parse(input: &[u8], max_depth: usize) -> Option<Element> {
    parse_tolerant(input, max_depth).into_first_element()
}

/// Parses `input` into a synthetic root whose children are the document's top-level nodes. Never
/// fails: unterminated constructs end the scan, stray close tags are ignored, and elements left open
/// by truncated input are folded back into the root. `max_depth` bounds materialized nesting.
pub(crate) fn parse_tolerant(input: &[u8], max_depth: usize) -> Element {
    let input = input.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(input);
    let mut stack: Vec<Element> = vec![Element::default()];
    let mut i = 0;
    while i < input.len() {
        if input.get(i) == Some(&b'<') {
            if starts_with(input, i, b"<!--") {
                i = find(input, i + 4, b"-->").map_or(input.len(), |end| end + 3);
            } else if starts_with(input, i, b"<![CDATA[") {
                let end = find(input, i + 9, b"]]>").unwrap_or(input.len());
                if let Some(text) = slice_str(input, i + 9, end) {
                    push_text(&mut stack, text.to_owned());
                }
                i = (end + 3).min(input.len());
            } else if starts_with(input, i, b"<!") || starts_with(input, i, b"<?") {
                i = find_byte(input, i + 2, b'>').map_or(input.len(), |end| end + 1);
            } else if starts_with(input, i, b"</") {
                let end = find_byte(input, i, b'>').unwrap_or(input.len());
                close_element(&mut stack);
                i = (end + 1).min(input.len());
            } else {
                i = parse_start_tag(input, i, &mut stack, max_depth);
            }
        } else {
            let end = find_byte(input, i, b'<').unwrap_or(input.len());
            if let Some(text) = slice_str(input, i, end)
                && !text.is_empty()
            {
                push_text(&mut stack, decode_entities(text));
            }
            i = end;
        }
    }
    while stack.len() > 1 {
        close_element(&mut stack);
    }
    stack.into_iter().next().unwrap_or_default()
}

/// Parses a start tag beginning at `start` (the `<`). Pushes the element onto the stack, or attaches
/// it directly when self-closing or at the depth ceiling. Returns the index just past the tag.
fn parse_start_tag(
    input: &[u8],
    start: usize,
    stack: &mut Vec<Element>,
    max_depth: usize,
) -> usize {
    let mut i = start + 1;
    let name_start = i;
    while let Some(&byte) = input.get(i) {
        if matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/') {
            break;
        }
        i += 1;
    }
    let name = slice_str(input, name_start, i).unwrap_or("").to_owned();
    let mut element = Element {
        name,
        attrs: Vec::new(),
        children: Vec::new(),
    };
    let mut self_closing = false;
    loop {
        i = skip_ws(input, i);
        match input.get(i) {
            None => break,
            Some(&b'>') => {
                i += 1;
                break;
            }
            Some(&b'/') => {
                self_closing = true;
                i += 1;
            }
            Some(_) => {
                let attr_start = i;
                while let Some(&byte) = input.get(i) {
                    if matches!(byte, b'=' | b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/') {
                        break;
                    }
                    i += 1;
                }
                let attr_name = slice_str(input, attr_start, i).unwrap_or("").to_owned();
                i = skip_ws(input, i);
                let mut value = String::new();
                if input.get(i) == Some(&b'=') {
                    i = skip_ws(input, i + 1);
                    if let Some(&quote) = input.get(i)
                        && (quote == b'"' || quote == b'\'')
                    {
                        let value_start = i + 1;
                        let value_end = find_byte(input, value_start, quote).unwrap_or(input.len());
                        value = slice_str(input, value_start, value_end)
                            .map(decode_entities)
                            .unwrap_or_default();
                        i = (value_end + 1).min(input.len());
                    }
                }
                if !attr_name.is_empty() {
                    element.attrs.push((attr_name, value));
                }
            }
        }
    }
    if self_closing || stack.len() >= max_depth {
        attach(stack, Node::Element(element));
    } else {
        stack.push(element);
    }
    i
}

/// Pops the innermost open element and attaches it to its parent.
fn close_element(stack: &mut Vec<Element>) {
    if stack.len() <= 1 {
        return;
    }
    if let Some(element) = stack.pop() {
        attach(stack, Node::Element(element));
    }
}

fn push_text(stack: &mut [Element], text: String) {
    attach(stack, Node::Text(text));
}

fn attach(stack: &mut [Element], node: Node) {
    if let Some(top) = stack.last_mut() {
        top.children.push(node);
    }
}

fn starts_with(input: &[u8], at: usize, prefix: &[u8]) -> bool {
    input.get(at..at + prefix.len()) == Some(prefix)
}

fn find(input: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from > input.len() {
        return None;
    }
    let mut i = from;
    while i + needle.len() <= input.len() {
        if input.get(i..i + needle.len()) == Some(needle) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_byte(input: &[u8], from: usize, byte: u8) -> Option<usize> {
    let mut i = from;
    while let Some(&current) = input.get(i) {
        if current == byte {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn skip_ws(input: &[u8], from: usize) -> usize {
    let mut i = from;
    while matches!(input.get(i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        i += 1;
    }
    i
}

fn slice_str(input: &[u8], start: usize, end: usize) -> Option<&str> {
    if start > end {
        return None;
    }
    input
        .get(start..end)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
}

#[cfg(test)]
mod tests {
    use super::{local_name, parse, parse_tolerant};

    #[test]
    fn strict_parse_returns_root_and_navigates() {
        let root = parse(
            b"<package><metadata><dc:title>Hi</dc:title></metadata></package>",
            256,
        )
        .expect("well-formed input has a root");
        assert_eq!(root.name, "package");
        let title = root
            .child("metadata")
            .and_then(|meta| meta.child("title"))
            .expect("nested child");
        assert_eq!(title.text(), "Hi");
    }

    #[test]
    fn strict_parse_rejects_input_without_an_element() {
        assert!(parse(b"   \n  ", 256).is_none());
        assert!(parse(b"", 256).is_none());
    }

    #[test]
    fn strict_parse_skips_prolog_and_bom() {
        let input = "\u{feff}<?xml version=\"1.0\"?><!-- note --><!DOCTYPE x><r a=\"1\"/>";
        let root = parse(input.as_bytes(), 256).expect("root after prolog");
        assert_eq!(root.name, "r");
        assert_eq!(root.attr("a"), Some("1"));
    }

    #[test]
    fn tolerant_parse_folds_unclosed_tags_into_root() {
        let root = parse_tolerant(b"<a><b>text", 3072);
        let a = root.child("a").expect("open element preserved");
        assert_eq!(
            a.descendant("b").map(super::Element::text),
            Some("text".to_owned())
        );
    }

    #[cfg(feature = "docx")]
    #[test]
    fn attr_qualified_prefers_exact_then_local() {
        let root = parse_tolerant(br#"<w:blip r:embed="rid7" o:other="x"/>"#, 3072);
        let blip = root.child("blip").expect("child");
        assert_eq!(blip.attr_qualified("r:embed", "embed"), Some("rid7"));
        assert_eq!(blip.attr_qualified("r:missing", "other"), Some("x"));
    }

    #[test]
    fn cdata_is_kept_verbatim_and_text_entities_decoded() {
        let root = parse_tolerant(b"<r><![CDATA[a & <b>]]> then &amp; &#65;&#x42;</r>", 3072);
        assert_eq!(
            root.child("r").map(super::Element::text),
            Some("a & <b> then & AB".to_owned())
        );
    }

    #[test]
    fn local_name_strips_prefix() {
        assert_eq!(local_name("w:pStyle"), "pStyle");
        assert_eq!(local_name("plain"), "plain");
    }
}
