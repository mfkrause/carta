//! Footnote reconstruction for the HTML reader.
//!
//! A footnote rendered to HTML leaves a reference anchor at the cite site and an endnotes container
//! at the end of the document:
//!
//! ```html
//! text<a href="#fn1" role="doc-noteref"><sup>1</sup></a>
//! <section role="doc-endnotes"><ol><li id="fn1"><p>the note</p></li></ol></section>
//! ```
//!
//! Reading it back reverses that shaping: the note bodies are indexed by id so each reference can be
//! spliced into an [`carta_ast::Inline::Note`] at its cite site, and the container is dropped. The
//! anchors and container are recognized by their ARIA `role`, and reconstruction is always on: no
//! extension toggle controls it.

use std::collections::BTreeMap;

use super::tree::{Element, Node, attr_value};

/// The `role` marking a footnote reference anchor.
const NOTEREF_ROLE: &str = "doc-noteref";
/// The `role` marking the endnotes container.
pub(super) const ENDNOTES_ROLE: &str = "doc-endnotes";
/// The `role` marking a back-reference anchor inside a note body.
const BACKLINK_ROLE: &str = "doc-backlink";

/// Whether element `e` carries `role="role"`.
pub(super) fn has_role(e: &Element, role: &str) -> bool {
    attr_value(e, "role").is_some_and(|value| value == role)
}

/// The note id a reference anchor points at, if `e` is one: an `<a>` with `role="doc-noteref"` and a
/// same-document `#fragment` href. Such an anchor always becomes a Note, even when no matching body
/// exists, so the caller treats a `None` lookup as an empty note rather than a plain link.
pub(super) fn noteref_target(e: &Element) -> Option<String> {
    if e.name != "a" || !has_role(e, NOTEREF_ROLE) {
        return None;
    }
    let href = attr_value(e, "href")?;
    href.strip_prefix('#').map(str::to_owned)
}

/// Index every `<li id="…">` note body found inside an endnotes container, keyed by id. Each body is
/// the list item's child nodes with any back-reference anchor removed. Containers are located at any
/// depth, so a notes section nested inside another block still contributes; the first definition of a
/// given id wins.
pub(super) fn collect_note_defs(nodes: &[&Node]) -> BTreeMap<String, Vec<Node>> {
    let mut defs = BTreeMap::new();
    for node in nodes {
        collect_from_node(node, false, &mut defs);
    }
    defs
}

fn collect_from_node(node: &Node, in_endnotes: bool, defs: &mut BTreeMap<String, Vec<Node>>) {
    let Node::Element(e) = node else { return };
    let inside = in_endnotes || has_role(e, ENDNOTES_ROLE);
    if inside
        && e.name == "li"
        && let Some(id) = attr_value(e, "id")
    {
        defs.entry(id)
            .or_insert_with(|| strip_backlinks(&e.children));
    }
    for child in &e.children {
        collect_from_node(child, inside, defs);
    }
}

/// Clone `nodes`, dropping any back-reference anchor element and recursing into the rest.
fn strip_backlinks(nodes: &[Node]) -> Vec<Node> {
    nodes
        .iter()
        .filter_map(|node| match node {
            Node::Element(e) if e.name == "a" && has_role(e, BACKLINK_ROLE) => None,
            Node::Element(e) => {
                let mut cloned = e.clone();
                cloned.children = strip_backlinks(&e.children);
                Some(Node::Element(cloned))
            }
            Node::Text(_) | Node::Comment(_) => Some(node.clone()),
        })
        .collect()
}
