//! Golden checks for the Presentation MathML backend. Each expected string is a fixed golden: the
//! exact `<math>` element tree the backend emits for one construct. The suite is fully offline:
//! every value is embedded here, so nothing is generated at test time.

mod atoms;
mod structures;

use super::to_mathml;

#[test]
fn construct_goldens() {
    for (source, display, expected) in atoms::GOLDENS.iter().chain(structures::GOLDENS) {
        assert_eq!(
            to_mathml(source, *display).as_deref(),
            *expected,
            "source: {source:?} (display: {display})"
        );
    }
}

#[test]
fn xml_special_characters_are_escaped() {
    // A less-than in a leaf is escaped in element content, never emitted raw.
    let rendered = to_mathml("a<b", false).unwrap_or_default();
    assert!(rendered.contains("&lt;"));
    assert!(!rendered.contains("<mo><"));
}

#[test]
fn deeply_nested_input_does_not_panic() {
    // a pathological brace nest is bounded by the depth limit: no stack overflow
    let source = format!("{}x{}", "{".repeat(400), "}".repeat(400));
    let _ = to_mathml(&source, false);
}
