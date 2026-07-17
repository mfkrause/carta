//! A [`MathTree`] view over the HTML node tree, so the shared MathML → TeX renderer can walk a
//! `<math>` subtree the HTML reader parsed.

use crate::mathml::MathTree;

use super::tree::{Element, Node, attr_value, collect_text};

pub(super) use crate::mathml::to_tex;

impl MathTree for Element {
    fn tag(&self) -> &str {
        &self.name
    }
    fn attribute(&self, key: &str) -> Option<String> {
        attr_value(self, key)
    }
    fn inner_text(&self) -> String {
        collect_text(self)
    }
    fn element_children(&self) -> Vec<&Self> {
        self.children
            .iter()
            .filter_map(|node| match node {
                Node::Element(element) => Some(element),
                _ => None,
            })
            .collect()
    }
    fn nth_element_child(&self, index: usize) -> Option<&Self> {
        self.children
            .iter()
            .filter_map(|node| match node {
                Node::Element(element) => Some(element),
                _ => None,
            })
            .nth(index)
    }
}
