use super::{IrBlock, parse};
use carta_core::{Extension, Extensions};

fn with_abbr(input: &str) -> Vec<IrBlock> {
    parse(
        input,
        Extensions::from_list(&[Extension::Abbreviations]),
        true,
    )
    .0
}

fn plain(input: &str) -> Vec<IrBlock> {
    parse(input, Extensions::empty(), true).0
}

#[test]
fn a_definition_at_the_left_edge_is_consumed() {
    let out = with_abbr("*[HTML]: markup\n\nBody.\n");
    assert!(
        matches!(out.as_slice(), [IrBlock::Para(p)] if p == "Body."),
        "definition should be dropped, leaving only the body: {out:?}"
    );
}

#[test]
fn a_definition_is_stripped_from_a_paragraph_front() {
    let out = with_abbr("*[HTML]: markup\nmore text\n");
    assert!(
        matches!(out.as_slice(), [IrBlock::Para(p)] if p == "more text"),
        "only the definition line should be removed: {out:?}"
    );
}

#[test]
fn consecutive_definitions_are_all_consumed() {
    let out = with_abbr("*[A]: x\n*[B]: y\nmore\n");
    assert!(
        matches!(out.as_slice(), [IrBlock::Para(p)] if p == "more"),
        "both definitions should be removed: {out:?}"
    );
}

#[test]
fn an_indented_definition_is_left_as_text() {
    // A definition must sit flush at the container's left edge; one space in front keeps it a
    // paragraph.
    let out = with_abbr(" *[HTML]: markup\n\nBody.\n");
    assert_eq!(
        out.len(),
        2,
        "indented definition stays a paragraph: {out:?}"
    );
}

#[test]
fn without_the_extension_a_definition_is_ordinary_text() {
    let out = plain("*[HTML]: markup\n\nBody.\n");
    assert_eq!(
        out.len(),
        2,
        "no consumption without the extension: {out:?}"
    );
}
