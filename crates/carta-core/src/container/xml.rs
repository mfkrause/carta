//! A small XML emitter that always produces well-formed output.
//!
//! Markup is modelled as a tree of [`Element`]s carrying attributes, text, and pre-serialized raw
//! fragments. Rendering escapes text and attribute values and self-closes empty elements, so the
//! result is well-formed by construction. Attributes and children keep insertion order, keeping
//! output byte-reproducible.

/// The XML declaration a document part begins with.
pub const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

/// A node within an [`Element`]: a child element or escaped text.
#[derive(Debug, Clone)]
enum Node {
    Element(Element),
    Text(String),
}

/// An XML element with ordered attributes and children.
#[derive(Debug, Clone)]
pub struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Node>,
}

impl Element {
    /// An empty element with the given tag name.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            attributes: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Adds an attribute (escaped on render). Repeats are kept in order.
    #[must_use]
    pub fn attr(mut self, name: &str, value: &str) -> Self {
        self.attributes.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Appends escaped text content.
    #[must_use]
    pub fn text(mut self, text: &str) -> Self {
        self.children.push(Node::Text(text.to_owned()));
        self
    }

    /// Appends a child element.
    #[must_use]
    pub fn child(mut self, child: Element) -> Self {
        self.children.push(Node::Element(child));
        self
    }

    /// Appends a child element in place.
    pub fn push(&mut self, child: Element) {
        self.children.push(Node::Element(child));
    }

    /// Serializes this element and its descendants. No XML declaration is prepended; a caller that
    /// wants one writes [`DECLARATION`] first.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.render_into(&mut out);
        out
    }

    /// Serializes a complete document part: the XML [`DECLARATION`], then this element.
    #[must_use]
    pub fn render_document(&self) -> String {
        let mut out = String::from(DECLARATION);
        self.render_into(&mut out);
        out
    }

    /// Serializes a complete document part with two-space indentation: the XML [`DECLARATION`], this
    /// element laid out over multiple lines, and a trailing newline. An element holding only text
    /// stays on one line; an element with child elements puts each child on its own indented line.
    #[must_use]
    pub fn render_document_pretty(&self) -> String {
        let mut out = String::from(DECLARATION);
        self.render_pretty(&mut out, 0);
        out.push('\n');
        out
    }

    fn render_open_tag(&self, out: &mut String) {
        out.push('<');
        out.push_str(&self.name);
        for (name, value) in &self.attributes {
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            escape_attribute(value, out);
            out.push('"');
        }
    }

    fn render_into(&self, out: &mut String) {
        self.render_open_tag(out);
        if self.children.is_empty() {
            out.push_str(" />");
            return;
        }
        out.push('>');
        for child in &self.children {
            match child {
                Node::Element(element) => element.render_into(out),
                Node::Text(text) => escape_text(text, out),
            }
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push('>');
    }

    fn render_pretty(&self, out: &mut String, depth: usize) {
        for _ in 0..depth {
            out.push_str("  ");
        }
        self.render_open_tag(out);
        if self.children.is_empty() {
            out.push_str(" />");
            return;
        }
        // An element whose content is purely text stays on one line; only child elements force the
        // indented, multi-line layout.
        if !self.children.iter().any(|c| matches!(c, Node::Element(_))) {
            out.push('>');
            for child in &self.children {
                match child {
                    Node::Text(text) => escape_text(text, out),
                    Node::Element(_) => {}
                }
            }
            out.push_str("</");
            out.push_str(&self.name);
            out.push('>');
            return;
        }
        out.push_str(">\n");
        for child in &self.children {
            match child {
                Node::Element(element) => {
                    element.render_pretty(out, depth + 1);
                    out.push('\n');
                }
                Node::Text(text) => {
                    for _ in 0..=depth {
                        out.push_str("  ");
                    }
                    escape_text(text, out);
                    out.push('\n');
                }
            }
        }
        for _ in 0..depth {
            out.push_str("  ");
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push('>');
    }
}

/// Escapes text content, the minimal set that keeps character data unambiguous.
pub fn escape_text(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

/// Escapes an attribute value, additionally guarding the quote and whitespace that would break a
/// double-quoted attribute.
pub fn escape_attribute(value: &str, out: &mut String) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            other => out.push(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Element;

    #[test]
    fn empty_element_self_closes() {
        assert_eq!(Element::new("br").render(), "<br />");
    }

    #[test]
    fn attributes_keep_insertion_order_and_escape() {
        let element = Element::new("a")
            .attr("href", "x?q=1&y=\"2\"")
            .attr("rel", "next")
            .text("A & B < C");
        assert_eq!(
            element.render(),
            "<a href=\"x?q=1&amp;y=&quot;2&quot;\" rel=\"next\">A &amp; B &lt; C</a>"
        );
    }

    #[test]
    fn nested_children_render_inline() {
        let element =
            Element::new("ol").child(Element::new("li").child(Element::new("b").text("bold")));
        assert_eq!(element.render(), "<ol><li><b>bold</b></li></ol>");
    }

    #[test]
    fn render_document_prepends_declaration() {
        let doc = Element::new("root").render_document();
        assert!(doc.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root />"));
    }

    #[test]
    fn pretty_layout_indents_element_children_and_inlines_text() {
        let doc = Element::new("root")
            .child(Element::new("group").child(Element::new("item").attr("id", "1").text("hi")))
            .child(Element::new("empty"))
            .render_document_pretty();
        assert_eq!(
            doc,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <root>\n  \
             <group>\n    \
             <item id=\"1\">hi</item>\n  \
             </group>\n  \
             <empty />\n\
             </root>\n"
        );
    }
}
