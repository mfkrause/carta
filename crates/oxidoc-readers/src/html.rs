//! HTML reader.
//!
//! Parsing runs in three stages: a tokenizer ([`tokenize`]) turns the source into a flat stream of
//! start tags, end tags and text; a tree builder ([`build_tree`]) assembles that stream into a node
//! tree, applying void-element and implied-end-tag rules; and a [`Converter`] walks the tree into a
//! [`Document`]. Document metadata is read from a `<head>` element when present.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use oxidoc_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MetaValue, QuoteType, Row, Table, TableBody, TableFoot,
    TableHead, Target, slug, to_plain_text,
};
use oxidoc_core::{Reader, ReaderOptions, Result};

use crate::entities::{code_point, lookup_named};

/// Parses HTML text into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlReader;

impl Reader for HtmlReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        Ok(parse(input))
    }
}

fn parse(input: &str) -> Document {
    let normalized = normalize(input);
    let chars: Vec<char> = normalized.chars().collect();
    let tokens = tokenize(&chars);
    let roots = build_tree(tokens);
    let (head, body) = locate(&roots);

    let mut converter = Converter::default();
    let meta = head.map(extract_meta).unwrap_or_default();
    let blocks = converter.blocks(&body, false);
    Document {
        meta,
        blocks,
        ..Document::default()
    }
}

/// Normalize line endings to `\n` and strip a leading byte-order mark.
fn normalize(input: &str) -> Cow<'_, str> {
    let without_bom = input.strip_prefix('\u{feff}').unwrap_or(input);
    if !without_bom.contains('\r') {
        return Cow::Borrowed(without_bom);
    }
    let mut out = String::with_capacity(without_bom.len());
    let mut chars = without_bom.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            other => out.push(other),
        }
    }
    Cow::Owned(out)
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Token {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    End(String),
    Text(String),
}

/// Elements whose content is read verbatim up to the matching end tag. Entities are still resolved
/// in the text-only group; the script-like group is passed through untouched.
fn raw_text_mode(name: &str) -> Option<bool> {
    match name {
        "script" | "style" => Some(false),
        "title" | "textarea" => Some(true),
        _ => None,
    }
}

fn tokenize(chars: &[char]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut pos = 0;
    while pos < chars.len() {
        if chars.get(pos) == Some(&'<')
            && let Some(next) = read_markup(chars, pos, &mut tokens)
        {
            pos = next;
            continue;
        }
        pos = read_text(chars, pos, &mut tokens);
    }
    tokens
}

/// Consume one `<…>` construct. Returns the position past it, or `None` when the `<` does not begin
/// a tag and should be treated as literal text.
fn read_markup(chars: &[char], pos: usize, tokens: &mut Vec<Token>) -> Option<usize> {
    match chars.get(pos + 1)? {
        '!' => Some(skip_declaration(chars, pos)),
        '?' => Some(skip_to_gt(chars, pos + 1)),
        '/' => read_end_tag(chars, pos, tokens),
        c if c.is_ascii_alphabetic() => Some(read_start_tag(chars, pos, tokens)),
        _ => None,
    }
}

/// Skip `<!-- … -->`, `<![CDATA[ … ]]>` or a `<!doctype …>` declaration.
fn skip_declaration(chars: &[char], pos: usize) -> usize {
    if chars.get(pos + 2) == Some(&'-') && chars.get(pos + 3) == Some(&'-') {
        let mut i = pos + 4;
        while i < chars.len() {
            if chars.get(i) == Some(&'-')
                && chars.get(i + 1) == Some(&'-')
                && chars.get(i + 2) == Some(&'>')
            {
                return i + 3;
            }
            i += 1;
        }
        return chars.len();
    }
    skip_to_gt(chars, pos)
}

fn skip_to_gt(chars: &[char], pos: usize) -> usize {
    let mut i = pos + 1;
    while i < chars.len() {
        if chars.get(i) == Some(&'>') {
            return i + 1;
        }
        i += 1;
    }
    chars.len()
}

fn read_end_tag(chars: &[char], pos: usize, tokens: &mut Vec<Token>) -> Option<usize> {
    let start = pos + 2;
    if !chars.get(start).copied()?.is_ascii_alphabetic() {
        return None;
    }
    let mut i = start;
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_whitespace() || c == '>' {
            break;
        }
        i += 1;
    }
    let name: String = collect_lower(chars, start, i);
    tokens.push(Token::End(name));
    Some(skip_to_gt(chars, i - 1))
}

fn read_start_tag(chars: &[char], pos: usize, tokens: &mut Vec<Token>) -> usize {
    let start = pos + 1;
    let mut i = start;
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_whitespace() || c == '>' || c == '/' {
            break;
        }
        i += 1;
    }
    let name = collect_lower(chars, start, i);
    let (attrs, next, self_closing) = read_attributes(chars, i);

    if let Some(decode) = raw_text_mode(&name) {
        let (text, after) = read_raw_text(chars, next, &name, decode);
        tokens.push(Token::Start {
            name: name.clone(),
            attrs,
            self_closing: false,
        });
        if !text.is_empty() {
            tokens.push(Token::Text(text));
        }
        tokens.push(Token::End(name));
        return after;
    }

    tokens.push(Token::Start {
        name,
        attrs,
        self_closing,
    });
    next
}

/// Parse a start tag's attribute list. `pos` points just after the tag name; the result position is
/// just past the closing `>`.
fn read_attributes(chars: &[char], pos: usize) -> (Vec<(String, String)>, usize, bool) {
    let mut attrs = Vec::new();
    let mut i = pos;
    let mut self_closing = false;
    loop {
        i = skip_whitespace(chars, i);
        match chars.get(i) {
            None => break,
            Some('>') => {
                i += 1;
                break;
            }
            Some('/') => {
                if chars.get(i + 1) == Some(&'>') {
                    self_closing = true;
                    i += 2;
                    break;
                }
                i += 1;
            }
            Some(_) => {
                let (name, after_name) = read_attr_name(chars, i);
                i = skip_whitespace(chars, after_name);
                if chars.get(i) == Some(&'=') {
                    i = skip_whitespace(chars, i + 1);
                    let (value, after_value) = read_attr_value(chars, i);
                    i = after_value;
                    if !name.is_empty() {
                        attrs.push((name, value));
                    }
                } else if !name.is_empty() {
                    attrs.push((name, String::new()));
                }
            }
        }
    }
    (attrs, i, self_closing)
}

fn read_attr_name(chars: &[char], pos: usize) -> (String, usize) {
    let mut i = pos;
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_whitespace() || c == '=' || c == '>' || c == '/' {
            break;
        }
        i += 1;
    }
    (collect_lower(chars, pos, i), i)
}

fn read_attr_value(chars: &[char], pos: usize) -> (String, usize) {
    if let Some(&quote @ ('"' | '\'')) = chars.get(pos) {
        let mut i = pos + 1;
        let value_start = i;
        while let Some(&c) = chars.get(i) {
            if c == quote {
                break;
            }
            i += 1;
        }
        let raw: String = slice(chars, value_start, i);
        (decode_entities(raw), i + 1)
    } else {
        let mut i = pos;
        while let Some(&c) = chars.get(i) {
            if c.is_ascii_whitespace() || c == '>' {
                break;
            }
            i += 1;
        }
        let raw: String = slice(chars, pos, i);
        (decode_entities(raw), i)
    }
}

fn read_raw_text(chars: &[char], pos: usize, name: &str, decode: bool) -> (String, usize) {
    let mut i = pos;
    while i < chars.len() {
        if chars.get(i) == Some(&'<')
            && chars.get(i + 1) == Some(&'/')
            && matches_name(chars, i + 2, name)
        {
            let raw: String = slice(chars, pos, i);
            let text = if decode { decode_entities(raw) } else { raw };
            return (text, skip_to_gt(chars, i + 1));
        }
        i += 1;
    }
    let raw: String = slice(chars, pos, chars.len());
    let text = if decode { decode_entities(raw) } else { raw };
    (text, chars.len())
}

fn read_text(chars: &[char], pos: usize, tokens: &mut Vec<Token>) -> usize {
    let start = pos;
    let mut i = pos;
    while let Some(&c) = chars.get(i) {
        if c == '<' && begins_markup(chars, i) {
            break;
        }
        i += 1;
    }
    let next = if i == start { start + 1 } else { i };
    let raw: String = slice(chars, start, next);
    tokens.push(Token::Text(decode_entities(raw)));
    next
}

fn begins_markup(chars: &[char], pos: usize) -> bool {
    match chars.get(pos + 1) {
        Some('!' | '?' | '/') => true,
        Some(c) => c.is_ascii_alphabetic(),
        None => false,
    }
}

fn matches_name(chars: &[char], pos: usize, name: &str) -> bool {
    let candidate = name.chars().enumerate().all(|(offset, expected)| {
        chars
            .get(pos + offset)
            .is_some_and(|&c| c.eq_ignore_ascii_case(&expected))
    });
    if !candidate {
        return false;
    }
    match chars.get(pos + name.chars().count()) {
        None => true,
        Some(&c) => c.is_ascii_whitespace() || c == '>' || c == '/',
    }
}

fn skip_whitespace(chars: &[char], pos: usize) -> usize {
    let mut i = pos;
    while chars.get(i).is_some_and(char::is_ascii_whitespace) {
        i += 1;
    }
    i
}

fn slice(chars: &[char], start: usize, end: usize) -> String {
    chars
        .get(start..end)
        .map(|s| s.iter().collect())
        .unwrap_or_default()
}

fn collect_lower(chars: &[char], start: usize, end: usize) -> String {
    chars
        .get(start..end)
        .map(|s| s.iter().flat_map(|c| c.to_lowercase()).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Entity decoding
// ---------------------------------------------------------------------------

fn decode_entities(text: String) -> String {
    if !text.contains('&') {
        return text;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c == '&'
            && let Some((decoded, next)) = scan_char_ref(&chars, i)
        {
            out.push_str(&decoded);
            i = next;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Resolve a character reference beginning at `start` (a `&`). Named references decode without a
/// trailing `;` only when the whole alphanumeric run names a known entity.
fn scan_char_ref(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start + 1) == Some(&'#') {
        return scan_numeric_ref(chars, start);
    }
    let mut i = start + 1;
    while chars.get(i).is_some_and(char::is_ascii_alphanumeric) {
        i += 1;
    }
    if i == start + 1 {
        return None;
    }
    let name: String = slice(chars, start + 1, i);
    if chars.get(i) == Some(&';') {
        let decoded = lookup_named(&name)?;
        return Some((decoded.to_string(), i + 1));
    }
    let decoded = lookup_named(&name)?;
    Some((decoded.to_string(), i))
}

fn scan_numeric_ref(chars: &[char], start: usize) -> Option<(String, usize)> {
    let hex = matches!(chars.get(start + 2), Some('x' | 'X'));
    let digits_start = if hex { start + 3 } else { start + 2 };
    let mut i = digits_start;
    let radix = if hex { 16 } else { 10 };
    while chars.get(i).is_some_and(|c| c.is_digit(radix)) {
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    let digits: String = slice(chars, digits_start, i);
    let code = u32::from_str_radix(&digits, radix).ok()?;
    let end = if chars.get(i) == Some(&';') { i + 1 } else { i };
    Some((code_point(code).to_string(), end))
}

// ---------------------------------------------------------------------------
// Tree builder
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Node {
    Element(Element),
    Text(String),
}

#[derive(Debug)]
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn build_tree(tokens: Vec<Token>) -> Vec<Node> {
    let mut stack: Vec<Element> = vec![Element {
        name: String::new(),
        attrs: Vec::new(),
        children: Vec::new(),
    }];

    for token in tokens {
        match token {
            Token::Text(text) => push_child(&mut stack, Node::Text(text)),
            Token::Start {
                name,
                attrs,
                self_closing,
            } => {
                close_implied(&mut stack, &name);
                let void = self_closing || is_void(&name);
                let element = Element {
                    name,
                    attrs,
                    children: Vec::new(),
                };
                if void {
                    push_child(&mut stack, Node::Element(element));
                } else {
                    stack.push(element);
                }
            }
            Token::End(name) => {
                if is_void(&name) {
                    continue;
                }
                close_to(&mut stack, &name);
            }
        }
    }

    while stack.len() > 1 {
        fold_top(&mut stack);
    }
    stack
        .into_iter()
        .next()
        .map(|root| root.children)
        .unwrap_or_default()
}

fn push_child(stack: &mut [Element], node: Node) {
    if let Some(top) = stack.last_mut() {
        top.children.push(node);
    }
}

fn fold_top(stack: &mut Vec<Element>) {
    if let Some(top) = stack.pop() {
        push_child(stack, Node::Element(top));
    }
}

fn current_name(stack: &[Element]) -> Option<&str> {
    stack
        .last()
        .filter(|e| !e.name.is_empty())
        .map(|e| e.name.as_str())
}

/// Pop open elements that the start tag `name` implicitly closes (an unterminated `<li>`, table
/// cell, paragraph, …).
fn close_implied(stack: &mut Vec<Element>, name: &str) {
    loop {
        let Some(open) = current_name(stack) else {
            return;
        };
        let close = match name {
            "li" => open == "li",
            "dt" | "dd" => open == "dt" || open == "dd",
            "option" => open == "option",
            "tr" => matches!(open, "td" | "th" | "tr"),
            "td" | "th" => matches!(open, "td" | "th"),
            "thead" | "tbody" | "tfoot" => {
                matches!(open, "td" | "th" | "tr" | "thead" | "tbody" | "tfoot")
            }
            _ => open == "p" && closes_paragraph(name),
        };
        if close {
            fold_top(stack);
        } else {
            return;
        }
    }
}

fn closes_paragraph(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "details"
            | "div"
            | "dl"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "ul"
    )
}

fn close_to(stack: &mut Vec<Element>, name: &str) {
    if !stack.iter().any(|e| e.name == name) {
        return;
    }
    while let Some(open) = current_name(stack) {
        let matched = open == name;
        fold_top(stack);
        if matched {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Document structure
// ---------------------------------------------------------------------------

/// Locate the metadata `<head>` and the block-content nodes, descending through a wrapping
/// `<html>`. Without an explicit `<body>`, every top-level node except the head contributes blocks.
fn locate(nodes: &[Node]) -> (Option<&Element>, Vec<&Node>) {
    if let Some(html) = find_element(nodes, "html") {
        return locate(&html.children);
    }
    let head = find_element(nodes, "head");
    let body = find_element(nodes, "body");
    let content = if let Some(body) = body {
        body.children.iter().collect()
    } else {
        nodes
            .iter()
            .filter(|node| !is_named_element(node, "head"))
            .collect()
    };
    (head, content)
}

fn find_element<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Element> {
    nodes.iter().find_map(|node| match node {
        Node::Element(e) if e.name == name => Some(e),
        _ => None,
    })
}

fn is_named_element(node: &Node, name: &str) -> bool {
    matches!(node, Node::Element(e) if e.name == name)
}

fn extract_meta(head: &Element) -> BTreeMap<String, MetaValue> {
    let mut meta = BTreeMap::new();
    for child in &head.children {
        let Node::Element(e) = child else { continue };
        match e.name.as_str() {
            "title" => {
                meta.insert("title".to_string(), MetaValue::MetaInlines(text_inlines(e)));
            }
            "meta" => {
                let name = attr_value(e, "name");
                let content = attr_value(e, "content");
                if let (Some(name), Some(content)) = (name, content)
                    && !name.is_empty()
                {
                    meta.insert(name, MetaValue::MetaInlines(inlines_from_text(&content)));
                }
            }
            _ => {}
        }
    }
    meta
}

fn attr_value(e: &Element, key: &str) -> Option<String> {
    e.attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

fn text_inlines(e: &Element) -> Vec<Inline> {
    let mut out = Vec::new();
    for child in &e.children {
        if let Node::Text(text) = child {
            push_text(&mut out, text);
        }
    }
    trim_inlines(out)
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Converter {
    used_ids: BTreeSet<String>,
}

impl Converter {
    fn blocks(&mut self, nodes: &[&Node], in_list: bool) -> Vec<Block> {
        let mut out = Vec::new();
        let mut pending = Vec::new();
        self.process(nodes.iter().copied(), &mut out, &mut pending);
        flush(&mut pending, &mut out);
        fix_plains(out, in_list)
    }

    fn child_blocks(&mut self, children: &[Node], in_list: bool) -> Vec<Block> {
        let refs: Vec<&Node> = children.iter().collect();
        self.blocks(&refs, in_list)
    }

    fn process<'a>(
        &mut self,
        nodes: impl Iterator<Item = &'a Node>,
        out: &mut Vec<Block>,
        pending: &mut Vec<Inline>,
    ) {
        for node in nodes {
            match node {
                Node::Text(text) => push_text(pending, text),
                Node::Element(e) => {
                    if matches!(e.name.as_str(), "script" | "style" | "head") {
                        continue;
                    }
                    if let Some(kind) = block_kind(&e.name) {
                        flush(pending, out);
                        self.emit_block(kind, e, out);
                    } else if is_inline_element(&e.name) {
                        self.append_inline(pending, node);
                    } else {
                        self.process(e.children.iter(), out, pending);
                    }
                }
            }
        }
    }

    fn emit_block(&mut self, kind: BlockKind, e: &Element, out: &mut Vec<Block>) {
        match kind {
            BlockKind::Para => out.push(Block::Para(trim_inlines(self.build_inlines(&e.children)))),
            BlockKind::Header(level) => {
                let inlines = trim_inlines(self.build_inlines(&e.children));
                let attr = self.header_attr(e, &inlines);
                out.push(Block::Header(level, attr, inlines));
            }
            BlockKind::BulletList => out.push(Block::BulletList(self.list_items(e))),
            BlockKind::OrderedList => {
                out.push(Block::OrderedList(list_attributes(e), self.list_items(e)));
            }
            BlockKind::BlockQuote => {
                out.push(Block::BlockQuote(self.child_blocks(&e.children, false)));
            }
            BlockKind::Pre => out.push(Self::code_block(e)),
            BlockKind::HorizontalRule => out.push(Block::HorizontalRule),
            BlockKind::Div { sectioning } => {
                let attr = div_attr(e, sectioning);
                out.push(Block::Div(attr, self.child_blocks(&e.children, false)));
            }
            BlockKind::DefinitionList => out.push(self.definition_list(e)),
            BlockKind::Table => out.push(self.table(e)),
            BlockKind::Figure => out.push(self.figure(e)),
        }
    }

    fn list_items(&mut self, e: &Element) -> Vec<Vec<Block>> {
        e.children
            .iter()
            .filter_map(|node| match node {
                Node::Element(item) if item.name == "li" => {
                    Some(self.child_blocks(&item.children, true))
                }
                _ => None,
            })
            .collect()
    }

    fn definition_list(&mut self, e: &Element) -> Block {
        let mut items: Vec<(Vec<Inline>, Vec<Vec<Block>>)> = Vec::new();
        let mut current: Option<(Vec<Inline>, Vec<Vec<Block>>)> = None;
        for child in &e.children {
            let Node::Element(item) = child else { continue };
            match item.name.as_str() {
                "dt" => {
                    if let Some(done) = current.take() {
                        items.push(done);
                    }
                    current = Some((trim_inlines(self.build_inlines(&item.children)), Vec::new()));
                }
                "dd" => {
                    let definition = self.child_blocks(&item.children, true);
                    current
                        .get_or_insert_with(|| (Vec::new(), Vec::new()))
                        .1
                        .push(definition);
                }
                _ => {}
            }
        }
        if let Some(done) = current {
            items.push(done);
        }
        Block::DefinitionList(items)
    }

    fn figure(&mut self, e: &Element) -> Block {
        let attr = extract_attr(e, &[]);
        let mut caption = Caption::default();
        let mut content_nodes: Vec<&Node> = Vec::new();
        for child in &e.children {
            if let Node::Element(inner) = child
                && inner.name == "figcaption"
            {
                caption = Caption {
                    short: None,
                    long: self.child_blocks(&inner.children, false),
                };
                continue;
            }
            content_nodes.push(child);
        }
        Block::Figure(attr, caption, self.blocks(&content_nodes, false))
    }

    fn code_block(e: &Element) -> Block {
        let mut attr = extract_attr(e, &[]);
        let inner_code = e.children.iter().find_map(|node| match node {
            Node::Element(inner) if inner.name == "code" => Some(inner),
            _ => None,
        });
        let content_source = if let Some(code) = inner_code {
            let mut code_attr = extract_attr(code, &[]);
            for class in &mut code_attr.classes {
                if let Some(rest) = class.strip_prefix("language-") {
                    *class = rest.to_string();
                }
            }
            merge_attr(&mut attr, code_attr);
            code
        } else {
            e
        };
        let mut text = collect_text(content_source);
        if text.ends_with('\n') {
            text.pop();
        }
        Block::CodeBlock(attr, text)
    }

    fn table(&mut self, e: &Element) -> Block {
        let attr = extract_attr(e, &[]);
        let mut caption = Caption::default();
        let mut col_widths: Vec<ColWidth> = Vec::new();
        let mut head_rows = Vec::new();
        let mut body_rows = Vec::new();
        let mut foot_rows = Vec::new();
        for child in &e.children {
            let Node::Element(section) = child else {
                continue;
            };
            match section.name.as_str() {
                "caption" => {
                    caption = Caption {
                        short: None,
                        long: self.child_blocks(&section.children, false),
                    };
                }
                "colgroup" => col_widths = column_widths(section),
                "thead" => head_rows.extend(self.rows(section)),
                "tbody" => body_rows.extend(self.rows(section)),
                "tfoot" => foot_rows.extend(self.rows(section)),
                "tr" => body_rows.push(self.row(section)),
                _ => {}
            }
        }

        let columns = table_width(&head_rows, &body_rows, &foot_rows, col_widths.len());
        let aligns = column_alignments(body_rows.first().or_else(|| head_rows.first()), columns);
        let col_specs = (0..columns)
            .map(|i| ColSpec {
                align: aligns.get(i).cloned().unwrap_or(Alignment::AlignDefault),
                width: col_widths
                    .get(i)
                    .cloned()
                    .unwrap_or(ColWidth::ColWidthDefault),
            })
            .collect();

        Block::Table(Box::new(Table {
            attr,
            caption,
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: head_rows,
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: body_rows,
            }],
            foot: TableFoot {
                attr: Attr::default(),
                rows: foot_rows,
            },
        }))
    }

    fn rows(&mut self, section: &Element) -> Vec<Row> {
        section
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Element(tr) if tr.name == "tr" => Some(self.row(tr)),
                _ => None,
            })
            .collect()
    }

    fn row(&mut self, tr: &Element) -> Row {
        let cells = tr
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Element(cell) if cell.name == "td" || cell.name == "th" => {
                    Some(self.cell(cell))
                }
                _ => None,
            })
            .collect();
        Row {
            attr: Attr::default(),
            cells,
        }
    }

    fn cell(&mut self, cell: &Element) -> Cell {
        Cell {
            attr: Attr::default(),
            align: cell_alignment(cell),
            row_span: span_attr(cell, "rowspan"),
            col_span: span_attr(cell, "colspan"),
            content: self.child_blocks(&cell.children, false),
        }
    }

    fn header_attr(&mut self, e: &Element, inlines: &[Inline]) -> Attr {
        let mut attr = extract_attr(e, &[]);
        if attr.id.is_empty() {
            let base = slug(&to_plain_text(inlines));
            let base = if base.is_empty() {
                "section".to_string()
            } else {
                base
            };
            attr.id = self.unique_id(base);
        } else {
            self.used_ids.insert(attr.id.clone());
        }
        attr
    }

    fn unique_id(&mut self, base: String) -> String {
        if self.used_ids.insert(base.clone()) {
            return base;
        }
        let mut n = 1;
        loop {
            let candidate = format!("{base}-{n}");
            if self.used_ids.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }

    fn build_inlines(&self, nodes: &[Node]) -> Vec<Inline> {
        let mut out = Vec::new();
        for node in nodes {
            self.append_inline(&mut out, node);
        }
        out
    }

    fn append_inline(&self, out: &mut Vec<Inline>, node: &Node) {
        let e = match node {
            Node::Text(text) => {
                push_text(out, text);
                return;
            }
            Node::Element(e) => e,
        };
        match inline_kind(&e.name) {
            InlineKind::Emph => out.push(Inline::Emph(self.build_inlines(&e.children))),
            InlineKind::Strong => out.push(Inline::Strong(self.build_inlines(&e.children))),
            InlineKind::Strikeout => out.push(Inline::Strikeout(self.build_inlines(&e.children))),
            InlineKind::Underline => out.push(Inline::Underline(self.build_inlines(&e.children))),
            InlineKind::Superscript => {
                out.push(Inline::Superscript(self.build_inlines(&e.children)));
            }
            InlineKind::Subscript => out.push(Inline::Subscript(self.build_inlines(&e.children))),
            InlineKind::Quoted => {
                out.push(Inline::Quoted(
                    QuoteType::DoubleQuote,
                    self.build_inlines(&e.children),
                ));
            }
            InlineKind::LineBreak => out.push(Inline::LineBreak),
            InlineKind::Span => {
                out.push(Inline::Span(
                    extract_attr(e, &[]),
                    self.build_inlines(&e.children),
                ));
            }
            InlineKind::SpanClass => {
                let mut attr = extract_attr(e, &[]);
                attr.classes.insert(0, e.name.clone());
                out.push(Inline::Span(attr, self.build_inlines(&e.children)));
            }
            InlineKind::Code(class) => out.push(Self::code_inline(e, class)),
            InlineKind::Anchor => out.push(self.anchor(e)),
            InlineKind::Image => out.push(image(e)),
            InlineKind::Transparent => {
                for child in &e.children {
                    self.append_inline(out, child);
                }
            }
        }
    }

    fn code_inline(e: &Element, forced_class: Option<&str>) -> Inline {
        let mut attr = extract_attr(e, &[]);
        if let Some(class) = forced_class {
            attr.classes = vec![class.to_string()];
        }
        Inline::Code(attr, collect_text(e))
    }

    fn anchor(&self, e: &Element) -> Inline {
        let inner = self.build_inlines(&e.children);
        if let Some(href) = attr_value(e, "href") {
            let title = attr_value(e, "title").unwrap_or_default();
            let attr = extract_attr(e, &["href", "title"]);
            return Inline::Link(attr, inner, Target { url: href, title });
        }
        let mut attr = extract_attr(e, &["name"]);
        if attr.id.is_empty()
            && let Some(name) = attr_value(e, "name")
        {
            attr.id = name;
        }
        Inline::Span(attr, inner)
    }
}

fn image(e: &Element) -> Inline {
    let url = attr_value(e, "src").unwrap_or_default();
    let title = attr_value(e, "title").unwrap_or_default();
    let alt = attr_value(e, "alt").unwrap_or_default();
    let attr = extract_attr(e, &["src", "title", "alt"]);
    Inline::Image(attr, inlines_from_text(&alt), Target { url, title })
}

// ---------------------------------------------------------------------------
// Element classification
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum BlockKind {
    Para,
    Header(i32),
    BulletList,
    OrderedList,
    BlockQuote,
    Pre,
    HorizontalRule,
    Div { sectioning: bool },
    DefinitionList,
    Table,
    Figure,
}

fn block_kind(name: &str) -> Option<BlockKind> {
    Some(match name {
        "p" => BlockKind::Para,
        "h1" => BlockKind::Header(1),
        "h2" => BlockKind::Header(2),
        "h3" => BlockKind::Header(3),
        "h4" => BlockKind::Header(4),
        "h5" => BlockKind::Header(5),
        "h6" => BlockKind::Header(6),
        "ul" | "menu" => BlockKind::BulletList,
        "ol" => BlockKind::OrderedList,
        "blockquote" => BlockKind::BlockQuote,
        "pre" => BlockKind::Pre,
        "hr" => BlockKind::HorizontalRule,
        "div" => BlockKind::Div { sectioning: false },
        "section" | "header" | "aside" => BlockKind::Div { sectioning: true },
        "dl" => BlockKind::DefinitionList,
        "table" => BlockKind::Table,
        "figure" => BlockKind::Figure,
        _ => return None,
    })
}

enum InlineKind {
    Emph,
    Strong,
    Strikeout,
    Underline,
    Superscript,
    Subscript,
    Quoted,
    LineBreak,
    Span,
    SpanClass,
    Code(Option<&'static str>),
    Anchor,
    Image,
    Transparent,
}

fn inline_kind(name: &str) -> InlineKind {
    match name {
        "em" | "i" => InlineKind::Emph,
        "strong" | "b" => InlineKind::Strong,
        "del" | "s" | "strike" => InlineKind::Strikeout,
        "ins" | "u" => InlineKind::Underline,
        "sup" => InlineKind::Superscript,
        "sub" => InlineKind::Subscript,
        "q" => InlineKind::Quoted,
        "br" => InlineKind::LineBreak,
        "span" => InlineKind::Span,
        "mark" | "small" | "abbr" | "kbd" | "dfn" => InlineKind::SpanClass,
        "code" | "tt" => InlineKind::Code(None),
        "samp" => InlineKind::Code(Some("sample")),
        "var" => InlineKind::Code(Some("variable")),
        "a" => InlineKind::Anchor,
        "img" => InlineKind::Image,
        _ => InlineKind::Transparent,
    }
}

fn is_inline_element(name: &str) -> bool {
    !matches!(inline_kind(name), InlineKind::Transparent)
}

// ---------------------------------------------------------------------------
// Attributes
// ---------------------------------------------------------------------------

fn extract_attr(e: &Element, exclude: &[&str]) -> Attr {
    let mut attr = Attr::default();
    for (key, value) in &e.attrs {
        if exclude.contains(&key.as_str()) {
            continue;
        }
        match key.as_str() {
            "id" => attr.id.clone_from(value),
            "class" => attr.classes = value.split_whitespace().map(str::to_string).collect(),
            _ => {
                let name = key.strip_prefix("data-").unwrap_or(key).to_string();
                attr.attributes.push((name, value.clone()));
            }
        }
    }
    attr
}

fn div_attr(e: &Element, sectioning: bool) -> Attr {
    let mut attr = extract_attr(e, &[]);
    if sectioning {
        attr.classes.insert(0, e.name.clone());
    }
    attr
}

fn merge_attr(base: &mut Attr, other: Attr) {
    if base.id.is_empty() {
        base.id = other.id;
    }
    base.classes.extend(other.classes);
    base.attributes.extend(other.attributes);
}

fn list_attributes(e: &Element) -> ListAttributes {
    let start = attr_value(e, "start")
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(1);
    let style = match attr_value(e, "type").as_deref() {
        Some("1") => ListNumberStyle::Decimal,
        Some("a") => ListNumberStyle::LowerAlpha,
        Some("A") => ListNumberStyle::UpperAlpha,
        Some("i") => ListNumberStyle::LowerRoman,
        Some("I") => ListNumberStyle::UpperRoman,
        _ => ListNumberStyle::DefaultStyle,
    };
    ListAttributes {
        start,
        style,
        delim: ListNumberDelim::DefaultDelim,
    }
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

fn span_attr(cell: &Element, key: &str) -> i32 {
    attr_value(cell, key)
        .and_then(|v| v.trim().parse::<i32>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1)
}

fn cell_alignment(cell: &Element) -> Alignment {
    if let Some(align) = attr_value(cell, "align")
        && let Some(parsed) = parse_alignment(&align)
    {
        return parsed;
    }
    if let Some(style) = attr_value(cell, "style")
        && let Some(value) = style_property(&style, "text-align")
        && let Some(parsed) = parse_alignment(&value)
    {
        return parsed;
    }
    Alignment::AlignDefault
}

fn parse_alignment(value: &str) -> Option<Alignment> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" => Some(Alignment::AlignLeft),
        "right" => Some(Alignment::AlignRight),
        "center" => Some(Alignment::AlignCenter),
        _ => None,
    }
}

fn style_property(style: &str, property: &str) -> Option<String> {
    style.split(';').find_map(|decl| {
        let (key, value) = decl.split_once(':')?;
        if key.trim().eq_ignore_ascii_case(property) {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn column_widths(colgroup: &Element) -> Vec<ColWidth> {
    let mut widths = Vec::new();
    for child in &colgroup.children {
        let Node::Element(col) = child else { continue };
        if col.name != "col" {
            continue;
        }
        let span = span_attr(col, "span");
        let width = attr_value(col, "style")
            .and_then(|style| style_property(&style, "width"))
            .and_then(|value| {
                value
                    .strip_suffix('%')
                    .and_then(|n| n.trim().parse::<f64>().ok())
            })
            .map_or(ColWidth::ColWidthDefault, |percent| {
                ColWidth::ColWidth(percent / 100.0)
            });
        for _ in 0..span {
            widths.push(width.clone());
        }
    }
    widths
}

fn row_width(row: &Row) -> usize {
    row.cells
        .iter()
        .map(|cell| usize::try_from(cell.col_span.max(1)).unwrap_or(1))
        .sum()
}

fn table_width(head: &[Row], body: &[Row], foot: &[Row], colgroup_width: usize) -> usize {
    head.iter()
        .chain(body)
        .chain(foot)
        .map(row_width)
        .max()
        .unwrap_or(0)
        .max(colgroup_width)
}

fn column_alignments(row: Option<&Row>, columns: usize) -> Vec<Alignment> {
    let mut aligns = vec![Alignment::AlignDefault; columns];
    let Some(row) = row else { return aligns };
    let mut index = 0;
    for cell in &row.cells {
        for _ in 0..cell.col_span.max(1) {
            if let Some(slot) = aligns.get_mut(index) {
                *slot = cell.align.clone();
            }
            index += 1;
        }
    }
    aligns
}

// ---------------------------------------------------------------------------
// Inline text and whitespace
// ---------------------------------------------------------------------------

fn inlines_from_text(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    push_text(&mut out, text);
    out
}

/// Append `text` to an inline run, collapsing each whitespace span to a single break: a span that
/// spans a line is a soft break, otherwise a space. Non-whitespace merges into the trailing string.
fn push_text(out: &mut Vec<Inline>, text: &str) {
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_whitespace() {
            let mut newline = false;
            while let Some(&w) = chars.peek() {
                if !w.is_ascii_whitespace() {
                    break;
                }
                newline |= w == '\n';
                chars.next();
            }
            push_break(out, newline);
        } else {
            let mut word = String::new();
            while let Some(&w) = chars.peek() {
                if w.is_ascii_whitespace() {
                    break;
                }
                word.push(w);
                chars.next();
            }
            push_str(out, &word);
        }
    }
}

fn push_str(out: &mut Vec<Inline>, word: &str) {
    if let Some(Inline::Str(existing)) = out.last_mut() {
        existing.push_str(word);
    } else {
        out.push(Inline::Str(word.to_string()));
    }
}

fn push_break(out: &mut Vec<Inline>, newline: bool) {
    match out.last() {
        Some(Inline::SoftBreak) => {}
        Some(Inline::Space) => {
            if newline {
                out.pop();
                out.push(Inline::SoftBreak);
            }
        }
        _ => out.push(if newline {
            Inline::SoftBreak
        } else {
            Inline::Space
        }),
    }
}

fn trim_inlines(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.remove(0);
    }
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
    inlines
}

/// Flush a loose inline run as a `Plain` block, dropping it when only whitespace remains.
fn flush(pending: &mut Vec<Inline>, out: &mut Vec<Block>) {
    let trimmed = trim_inlines(std::mem::take(pending));
    if !trimmed.is_empty() {
        out.push(Block::Plain(trimmed));
    }
}

/// A loose inline run is a `Plain` block until a paragraph-like sibling promotes the whole group to
/// `Para`. Nested lists do not promote runs inside an enclosing list item.
fn fix_plains(blocks: Vec<Block>, in_list: bool) -> Vec<Block> {
    if !blocks.iter().any(|block| is_paraish(block, in_list)) {
        return blocks;
    }
    blocks
        .into_iter()
        .map(|block| match block {
            Block::Plain(inlines) => Block::Para(inlines),
            other => other,
        })
        .collect()
}

fn is_paraish(block: &Block, in_list: bool) -> bool {
    match block {
        Block::Para(_) | Block::Header(..) | Block::BlockQuote(_) | Block::CodeBlock(..) => true,
        Block::BulletList(_) | Block::OrderedList(..) => !in_list,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Text gathering and identifier slugs
// ---------------------------------------------------------------------------

fn collect_text(e: &Element) -> String {
    let mut out = String::new();
    gather_text(&e.children, &mut out);
    out
}

fn gather_text(nodes: &[Node], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Element(e) => gather_text(&e.children, out),
        }
    }
}

/// Derive an ASCII-ish anchor identifier: keep letters, digits, spaces and `_-.`, lowercase, join
/// words with hyphens, then drop any leading characters before the first letter.
#[cfg(test)]
mod tests {
    use super::HtmlReader;
    use oxidoc_ast::{Block, Inline};
    use oxidoc_core::{Reader, ReaderOptions};

    fn blocks(input: &str) -> Vec<Block> {
        HtmlReader
            .read(input, &ReaderOptions::default())
            .expect("reader should not fail")
            .blocks
    }

    #[test]
    fn paragraph_with_emphasis() {
        let result = blocks("<p>a <em>b</em></p>");
        assert!(matches!(result.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn loose_text_is_plain() {
        assert!(matches!(blocks("hello").as_slice(), [Block::Plain(_)]));
    }

    #[test]
    fn paragraph_sibling_promotes_loose_text() {
        let result = blocks("loose<p>para</p>");
        assert!(matches!(
            result.as_slice(),
            [Block::Para(_), Block::Para(_)]
        ));
    }

    #[test]
    fn horizontal_rule_does_not_promote() {
        let result = blocks("loose<hr>");
        assert!(matches!(
            result.as_slice(),
            [Block::Plain(_), Block::HorizontalRule]
        ));
    }

    #[test]
    fn nested_list_inside_item_stays_tight() {
        let result = blocks("<ul><li>a<ul><li>b</li></ul></li></ul>");
        let Some(Block::BulletList(items)) = result.first() else {
            panic!("expected bullet list");
        };
        let Some(item) = items.first() else {
            panic!("expected one item");
        };
        assert!(matches!(item.first(), Some(Block::Plain(_))));
    }

    #[test]
    fn heading_generates_identifier() {
        let result = blocks("<h1>Hello World</h1>");
        let Some(Block::Header(level, attr, _)) = result.first() else {
            panic!("expected header");
        };
        assert_eq!(*level, 1);
        assert_eq!(attr.id, "hello-world");
    }

    #[test]
    fn duplicate_identifiers_are_disambiguated() {
        let result = blocks("<h1>Sec</h1><h2>Sec</h2>");
        let ids: Vec<&str> = result
            .iter()
            .filter_map(|block| match block {
                Block::Header(_, attr, _) => Some(attr.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["sec", "sec-1"]);
    }

    #[test]
    fn entities_are_decoded() {
        let result = blocks("<p>a &amp; b &copy; c</p>");
        let Some(Block::Para(inlines)) = result.first() else {
            panic!("expected paragraph");
        };
        assert!(inlines.contains(&Inline::Str("&".to_string())));
        assert!(inlines.contains(&Inline::Str("\u{a9}".to_string())));
    }

    #[test]
    fn comment_joins_surrounding_text() {
        let result = blocks("<p>a<!-- c -->b</p>");
        let Some(Block::Para(inlines)) = result.first() else {
            panic!("expected paragraph");
        };
        assert_eq!(inlines.as_slice(), [Inline::Str("ab".to_string())]);
    }

    #[test]
    fn script_content_is_dropped() {
        assert!(blocks("<script>var x = 1;</script><p>p</p>").len() == 1);
    }

    #[test]
    fn head_metadata_is_extracted() {
        let document = HtmlReader
            .read(
                "<head><title>T</title><meta name=\"author\" content=\"A\"></head><body><p>b</p></body>",
                &ReaderOptions::default(),
            )
            .expect("reader should not fail");
        assert!(document.meta.contains_key("title"));
        assert!(document.meta.contains_key("author"));
    }

    use oxidoc_ast::{Alignment, ColWidth, ListNumberStyle, Target};

    fn first_block(input: &str) -> Block {
        blocks(input).into_iter().next().expect("a block")
    }

    fn para_inlines(input: &str) -> Vec<Inline> {
        match first_block(input) {
            Block::Para(inlines) | Block::Plain(inlines) => inlines,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn normalizes_crlf_and_strips_bom() {
        let inlines = para_inlines("\u{feff}<p>a\r\nb</p>");
        assert_eq!(
            inlines.as_slice(),
            [
                Inline::Str("a".to_string()),
                Inline::SoftBreak,
                Inline::Str("b".to_string())
            ]
        );
    }

    #[test]
    fn ordered_list_reads_type_and_start() {
        let Block::OrderedList(attrs, items) =
            first_block(r#"<ol type="A" start="3"><li>x</li><li>y</li></ol>"#)
        else {
            panic!("expected ordered list");
        };
        assert_eq!(attrs.start, 3);
        assert_eq!(attrs.style, ListNumberStyle::UpperAlpha);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn menu_is_a_bullet_list() {
        assert!(matches!(
            first_block("<menu><li>a</li></menu>"),
            Block::BulletList(_)
        ));
    }

    #[test]
    fn implied_li_close_splits_items() {
        let Block::BulletList(items) = first_block("<ul><li>a<li>b</ul>") else {
            panic!("expected bullet list");
        };
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn pre_with_code_language_class_becomes_code_block() {
        let Block::CodeBlock(attr, text) = first_block(
            r#"<pre><code class="language-rust">let x = 1;
</code></pre>"#,
        ) else {
            panic!("expected code block");
        };
        assert_eq!(attr.classes, vec!["rust".to_string()]);
        assert_eq!(text, "let x = 1;");
    }

    #[test]
    fn definition_list_pairs_terms_and_definitions() {
        let Block::DefinitionList(items) =
            first_block("<dl><dt>term</dt><dd>one</dd><dd>two</dd></dl>")
        else {
            panic!("expected definition list");
        };
        let (term, defs) = items.into_iter().next().expect("an item");
        assert_eq!(term, vec![Inline::Str("term".to_string())]);
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn blockquote_wraps_child_blocks() {
        assert!(matches!(
            first_block("<blockquote><p>q</p></blockquote>"),
            Block::BlockQuote(_)
        ));
    }

    #[test]
    fn sectioning_div_gets_a_class() {
        let Block::Div(attr, _) = first_block("<section><p>x</p></section>") else {
            panic!("expected div");
        };
        assert!(attr.classes.contains(&"section".to_string()));
    }

    #[test]
    fn figure_separates_caption_from_content() {
        let Block::Figure(_, caption, content) =
            first_block("<figure><img src=\"a.png\"><figcaption>cap</figcaption></figure>")
        else {
            panic!("expected figure");
        };
        assert_eq!(caption.short, None);
        assert!(!caption.long.is_empty());
        assert!(!content.is_empty());
    }

    #[test]
    fn table_reads_sections_alignment_and_spans() {
        let input = r#"<table>
            <caption>cap</caption>
            <colgroup><col style="width: 25%"><col></colgroup>
            <thead><tr><th align="right">H1</th><th>H2</th></tr></thead>
            <tbody><tr><td colspan="2">wide</td></tr></tbody>
            <tfoot><tr><td>f1</td><td>f2</td></tr></tfoot>
        </table>"#;
        let Block::Table(table) = first_block(input) else {
            panic!("expected table");
        };
        assert_eq!(table.col_specs.len(), 2);
        assert_eq!(
            table.col_specs.first().map(|spec| spec.width.clone()),
            Some(ColWidth::ColWidth(0.25))
        );
        assert_eq!(
            table
                .head
                .rows
                .first()
                .and_then(|row| row.cells.first())
                .map(|cell| cell.align.clone()),
            Some(Alignment::AlignRight)
        );
        let body_cell_span = table
            .bodies
            .first()
            .and_then(|body| body.body.first())
            .and_then(|row| row.cells.first())
            .map(|cell| cell.col_span);
        assert_eq!(body_cell_span, Some(2));
        assert_eq!(table.foot.rows.len(), 1);
    }

    #[test]
    fn cell_alignment_reads_text_align_style() {
        let Block::Table(table) =
            first_block(r#"<table><tr><td style="text-align: center">c</td></tr></table>"#)
        else {
            panic!("expected table");
        };
        let align = table
            .bodies
            .first()
            .and_then(|body| body.body.first())
            .and_then(|row| row.cells.first())
            .map(|cell| cell.align.clone());
        assert_eq!(align, Some(Alignment::AlignCenter));
    }

    #[test]
    fn every_inline_emphasis_kind_is_mapped() {
        let inlines = para_inlines(
            "<p><em>a</em><b>b</b><del>c</del><u>d</u><sup>e</sup><sub>f</sub><q>g</q></p>",
        );
        assert!(matches!(
            inlines.as_slice(),
            [
                Inline::Emph(_),
                Inline::Strong(_),
                Inline::Strikeout(_),
                Inline::Underline(_),
                Inline::Superscript(_),
                Inline::Subscript(_),
                Inline::Quoted(_, _),
            ]
        ));
    }

    #[test]
    fn class_carrying_inlines_become_spans() {
        let inlines = para_inlines("<p><mark>m</mark><kbd>k</kbd></p>");
        let classes: Vec<&str> = inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Span(attr, _) => attr.classes.first().map(String::as_str),
                _ => None,
            })
            .collect();
        assert_eq!(classes, vec!["mark", "kbd"]);
    }

    #[test]
    fn code_variants_force_classes() {
        let inlines = para_inlines("<p><code>c</code><samp>s</samp><var>v</var></p>");
        let classes: Vec<Vec<String>> = inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Code(attr, _) => Some(attr.classes.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            classes,
            vec![
                Vec::<String>::new(),
                vec!["sample".to_string()],
                vec!["variable".to_string()],
            ]
        );
    }

    #[test]
    fn line_break_element_becomes_line_break() {
        let inlines = para_inlines("<p>a<br>b</p>");
        assert!(inlines.contains(&Inline::LineBreak));
    }

    #[test]
    fn anchor_with_href_is_a_link() {
        let inlines = para_inlines(r#"<p><a href="/u" title="T" class="x">t</a></p>"#);
        let Some(Inline::Link(attr, _, target)) = inlines.first() else {
            panic!("expected link");
        };
        assert_eq!(
            *target,
            Target {
                url: "/u".to_string(),
                title: "T".to_string()
            }
        );
        assert!(attr.classes.contains(&"x".to_string()));
    }

    #[test]
    fn anchor_with_name_is_a_span_with_id() {
        let inlines = para_inlines(r#"<p><a name="anchor">t</a></p>"#);
        let Some(Inline::Span(attr, _)) = inlines.first() else {
            panic!("expected span");
        };
        assert_eq!(attr.id, "anchor");
    }

    #[test]
    fn image_reads_src_title_and_alt() {
        let inlines = para_inlines(r#"<p><img src="a.png" title="T" alt="alt text"></p>"#);
        let Some(Inline::Image(_, alt, target)) = inlines.first() else {
            panic!("expected image");
        };
        assert_eq!(target.url, "a.png");
        assert_eq!(target.title, "T");
        assert_eq!(
            alt.as_slice(),
            [
                Inline::Str("alt".to_string()),
                Inline::Space,
                Inline::Str("text".to_string())
            ]
        );
    }

    #[test]
    fn unknown_inline_element_is_transparent() {
        let inlines = para_inlines("<p>a<bogus>b</bogus>c</p>");
        assert_eq!(inlines.as_slice(), [Inline::Str("abc".to_string())]);
    }

    #[test]
    fn data_attributes_drop_their_prefix() {
        let Block::Div(attr, _) = first_block(r#"<div id="d" data-role="note">x</div>"#) else {
            panic!("expected div");
        };
        assert_eq!(attr.id, "d");
        assert!(
            attr.attributes
                .contains(&("role".to_string(), "note".to_string()))
        );
    }

    #[test]
    fn boolean_and_unquoted_attributes_parse() {
        let Block::OrderedList(attrs, _) = first_block("<ol reversed start=5><li>a</li></ol>")
        else {
            panic!("expected ordered list");
        };
        assert_eq!(attrs.start, 5);
    }

    #[test]
    fn numeric_and_named_references_decode() {
        let inlines = para_inlines("<p>&#65;&#x42;&#X43;&copy</p>");
        assert_eq!(inlines.as_slice(), [Inline::Str("ABC\u{a9}".to_string())]);
    }

    #[test]
    fn unknown_entity_is_left_verbatim() {
        let inlines = para_inlines("<p>&notreal;</p>");
        assert_eq!(inlines.as_slice(), [Inline::Str("&notreal;".to_string())]);
    }

    #[test]
    fn style_block_is_dropped() {
        assert!(blocks("<style>p { color: red }</style><p>x</p>").len() == 1);
    }

    #[test]
    fn textarea_content_is_read_as_text() {
        let inlines = para_inlines("<p><textarea>typed &amp; ok</textarea></p>");
        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Str(s) if s.contains('&')))
        );
    }

    #[test]
    fn cdata_and_processing_instructions_are_skipped() {
        let inlines = para_inlines("<p>a<![CDATA[ junk ]]><?pi here?>b</p>");
        assert_eq!(inlines.as_slice(), [Inline::Str("ab".to_string())]);
    }

    #[test]
    fn doctype_declaration_is_skipped() {
        assert!(matches!(
            first_block("<!DOCTYPE html><p>x</p>"),
            Block::Para(_)
        ));
    }

    #[test]
    fn stray_less_than_is_literal_text() {
        let inlines = para_inlines("<p>a < b</p>");
        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Str(s) if s.contains('<')))
        );
    }

    #[test]
    fn self_closing_span_has_no_children() {
        let inlines = para_inlines("<p>a<span/>b</p>");
        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Span(_, children) if children.is_empty()))
        );
    }

    #[test]
    fn explicit_id_on_heading_is_preserved() {
        let Block::Header(_, attr, _) = first_block(r#"<h2 id="custom">Title</h2>"#) else {
            panic!("expected header");
        };
        assert_eq!(attr.id, "custom");
    }
}
