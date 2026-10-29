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
            to_mathml(source, *display, false).as_deref(),
            *expected,
            "source: {source:?} (display: {display})"
        );
    }
}

#[test]
fn xml_special_characters_are_escaped() {
    let rendered = to_mathml("a<b", false, false).unwrap_or_default();
    assert!(rendered.contains("&lt;"));
    assert!(!rendered.contains("<mo><"));
}

#[test]
fn function_application_tracks_scripts_and_following_atoms() {
    for (source, display, body) in [
        (
            r"\sin x",
            false,
            "<mrow><mrow><mi mathvariant=\"normal\">sin</mi><mo>⁡</mo></mrow><mi>x</mi></mrow>",
        ),
        (
            r"\sin^2",
            false,
            "<msup><mrow><mi mathvariant=\"normal\">sin</mi><mo>⁡</mo></mrow><mn>2</mn></msup>",
        ),
        (
            r"\lim_x",
            true,
            "<munder><mi mathvariant=\"normal\">lim</mi><mi>x</mi></munder>",
        ),
        (
            r"\sin\label{f}",
            false,
            "<mi mathvariant=\"normal\">sin</mi>",
        ),
        (
            r"\operatorname{foo} x",
            false,
            "<mrow><mrow><mi mathvariant=\"normal\">foo</mi><mo>⁡</mo></mrow><mi>x</mi></mrow>",
        ),
        (
            r"\operatorname*{foo}_x",
            true,
            "<munder><mi mathvariant=\"normal\">foo</mi><mi>x</mi></munder>",
        ),
    ] {
        let mode = if display { "block" } else { "inline" };
        assert_eq!(
            to_mathml(source, display, false),
            Some(format!(
                "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"{mode}\">{body}</math>"
            )),
            "{source}",
        );
    }
}

#[test]
fn deeply_nested_input_does_not_panic() {
    // a pathological brace nest is bounded by the depth limit: no stack overflow
    let source = format!("{}x{}", "{".repeat(400), "}".repeat(400));
    let _ = to_mathml(&source, false, false);
}
