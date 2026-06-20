//! Tree builder: assembles the token stream into a node tree, applying void-element and implied
//! end-tag rules, then locates the metadata `<head>` and block-content nodes. Also holds the
//! primitive read operations over the resulting node tree.

use super::tokenize::Token;

#[derive(Debug)]
pub(super) enum Node {
    Element(Element),
    Text(String),
}

#[derive(Debug)]
pub(super) struct Element {
    pub(super) name: String,
    pub(super) attrs: Vec<(String, String)>,
    pub(super) children: Vec<Node>,
    /// True for an element that stands alone with no end tag — a content-less void element
    /// (`<br>`, `<wbr>`, …) written without a self-closing slash. An explicit self-closing slash on
    /// any element instead yields an open/close pair, so it does not set this flag.
    pub(super) void: bool,
    /// True once a matching end tag (or a self-closing/void form) has accounted for this element.
    /// An element left open at end of input keeps this `false`, which lets a verbatim-preserving
    /// consumer omit a close tag the source never wrote.
    pub(super) closed: bool,
    /// True for a placeholder standing in for a stray end tag with no matching open element. It
    /// carries only a name; a verbatim-preserving consumer emits its close tag and nothing else.
    pub(super) end_only: bool,
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

pub(super) fn build_tree(tokens: Vec<Token>) -> Vec<Node> {
    let mut stack: Vec<Element> = vec![Element {
        name: String::new(),
        attrs: Vec::new(),
        children: Vec::new(),
        void: false,
        closed: false,
        end_only: false,
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
                let spec_void = is_void(&name);
                let standalone = self_closing || spec_void;
                let element = Element {
                    name,
                    attrs,
                    children: Vec::new(),
                    void: spec_void && !self_closing,
                    closed: standalone,
                    end_only: false,
                };
                if standalone {
                    push_child(&mut stack, Node::Element(element));
                } else {
                    stack.push(element);
                }
            }
            Token::End(name) => {
                if is_void(&name) {
                    continue;
                }
                if stack.iter().any(|e| e.name == name) {
                    close_to(&mut stack, &name);
                } else {
                    push_child(
                        &mut stack,
                        Node::Element(Element {
                            name,
                            attrs: Vec::new(),
                            children: Vec::new(),
                            void: false,
                            closed: true,
                            end_only: true,
                        }),
                    );
                }
            }
        }
    }

    while stack.len() > 1 {
        if let Some(top) = stack.last()
            && top.name == "a"
            && is_blank_anchor(top)
        {
            stack.pop();
            continue;
        }
        fold_top(&mut stack);
    }
    stack
        .into_iter()
        .next()
        .map(|root| root.children)
        .unwrap_or_default()
}

/// Whether an `<a>` left open at end of input carries nothing worth keeping: no element children and
/// only whitespace text. Such a stray anchor contributes no content and is discarded.
fn is_blank_anchor(e: &Element) -> bool {
    e.children.iter().all(|node| match node {
        Node::Text(text) => text.trim().is_empty(),
        Node::Element(_) => false,
    })
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
            "a" => open == "a",
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
        if matched && let Some(top) = stack.last_mut() {
            top.closed = true;
        }
        fold_top(stack);
        if matched {
            return;
        }
    }
}

/// Locate the metadata `<head>` and the block-content nodes, descending through a wrapping
/// `<html>`. Without an explicit `<body>`, every top-level node except the head contributes blocks.
pub(super) fn locate(nodes: &[Node]) -> (Option<&Element>, Vec<&Node>) {
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

pub(super) fn attr_value(e: &Element, key: &str) -> Option<String> {
    e.attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

pub(super) fn style_property(style: &str, property: &str) -> Option<String> {
    style.split(';').find_map(|decl| {
        let (key, value) = decl.split_once(':')?;
        if key.trim().eq_ignore_ascii_case(property) {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

pub(super) fn collect_text(e: &Element) -> String {
    let mut out = String::new();
    gather_text(&e.children, &mut out);
    out
}

pub(super) fn gather_text(nodes: &[Node], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Element(e) => gather_text(&e.children, out),
        }
    }
}

pub(super) fn serialize_element(e: &Element) -> String {
    let mut out = String::new();
    out.push('<');
    out.push_str(&e.name);
    for (key, value) in &e.attrs {
        out.push(' ');
        out.push_str(key);
        out.push_str("=\"");
        out.push_str(value);
        out.push('"');
    }
    out.push('>');
    gather_text(&e.children, &mut out);
    out.push_str("</");
    out.push_str(&e.name);
    out.push('>');
    out
}
