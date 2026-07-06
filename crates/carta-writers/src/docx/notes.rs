//! The `word/footnotes.xml` and `word/comments.xml` parts.
//!
//! Every document carries the two reserved footnote entries a word processor expects — the separator
//! drawn above footnotes and the one drawn where a footnote continues onto the next page. The
//! document's own footnotes follow, in the order their references appear in the body. The comments
//! part carries the document's comment entries, in the order their ranges open, and is empty when
//! there are none.

use super::wml_root;
use carta_core::container::xml::Element;

/// A reserved footnote of the given kind and identifier, holding the separator glyph named by
/// `mark`.
fn reserved(kind: &str, id: &str, mark: &str) -> Element {
    wml_footnote(kind, id)
        .child(Element::new("w:p").child(Element::new("w:r").child(Element::new(mark))))
}

/// A `w:footnote` element with its type and identifier attributes.
fn wml_footnote(kind: &str, id: &str) -> Element {
    Element::new("w:footnote")
        .attr("w:type", kind)
        .attr("w:id", id)
}

/// The `word/footnotes.xml` part: the two reserved separators followed by the document's footnote
/// entries in reference order.
pub(super) fn footnotes_xml(entries: Vec<Element>) -> String {
    let mut root = wml_root("w:footnotes")
        .child(reserved(
            "continuationSeparator",
            "0",
            "w:continuationSeparator",
        ))
        .child(reserved("separator", "-1", "w:separator"));
    for entry in entries {
        root.push(entry);
    }
    root.render_document()
}

/// The `word/comments.xml` part: the document's comment entries in the order their ranges opened,
/// empty when the document carries none.
pub(super) fn comments_xml(entries: Vec<Element>) -> String {
    let mut root = wml_root("w:comments");
    for entry in entries {
        root.push(entry);
    }
    root.render_document()
}
