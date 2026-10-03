//! HTML MathML rendering and fallback checks.

#![cfg(feature = "html")]
#![allow(clippy::unwrap_used)]

use carta_ast::{Block, Document, Inline, MathType};
use carta_core::{MathMethod, WrapMode, Writer, WriterOptions};
use carta_writers::{Html4Writer, HtmlWriter};

fn document(kind: MathType, source: &str) -> Document {
    Document {
        blocks: vec![Block::Para(vec![Inline::Math(kind, source.into())])],
        ..Document::default()
    }
}

fn options() -> WriterOptions {
    let mut options = WriterOptions::default();
    options.math_method = MathMethod::Mathml;
    options.wrap = WrapMode::None;
    options
}

#[test]
fn both_html_dialects_embed_mathml() {
    for (kind, display, source, body) in [
        (
            MathType::InlineMath,
            "inline",
            "x^2",
            "<msup><mi>x</mi><mn>2</mn></msup>",
        ),
        (
            MathType::DisplayMath,
            "block",
            "\\frac{a}{b}",
            "<mfrac><mi>a</mi><mi>b</mi></mfrac>",
        ),
        (MathType::InlineMath, "inline", "", "<mrow></mrow>"),
    ] {
        let document = document(kind, source);
        let expected = format!(
            "<p><math display=\"{display}\" xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics>{body}<annotation encoding=\"application/x-tex\">{source}</annotation></semantics></math></p>"
        );
        assert_eq!(HtmlWriter.write(&document, &options()).unwrap(), expected);
        assert_eq!(Html4Writer.write(&document, &options()).unwrap(), expected);
    }
}

#[test]
fn tex_methods_keep_their_math_spans() {
    let document = document(MathType::InlineMath, "x^2");
    for (method, expected) in [
        (MathMethod::Plain, "\\(x^2\\)"),
        (MathMethod::MathJax("math.js".into()), "\\(x^2\\)"),
        (MathMethod::Katex("assets/".into()), "x^2"),
    ] {
        let mut options = options();
        options.math_method = method;
        assert_eq!(
            HtmlWriter.write(&document, &options).unwrap(),
            format!("<p><span class=\"math inline\">{expected}</span></p>"),
        );
    }
}

#[test]
fn mathml_escapes_text_and_source_annotations() {
    let source = "\\text{</annotation><script>&}\u{0}\u{1}\u{b}\u{c}";
    let output = HtmlWriter
        .write(&document(MathType::InlineMath, source), &options())
        .unwrap();
    assert!(output.contains("&lt;/annotation&gt;&lt;script&gt;&amp;"));
    assert!(output.contains(
        "<annotation encoding=\"application/x-tex\">\\text{&lt;/annotation&gt;&lt;script&gt;&amp;}</annotation>"
    ));
    assert!(!output.contains("<script>"));
    assert!(!output.contains(['\u{0}', '\u{1}', '\u{b}', '\u{c}']));
}

#[test]
fn malformed_math_falls_back_to_escaped_tex() {
    for (kind, delimiters) in [(MathType::InlineMath, "$"), (MathType::DisplayMath, "$$")] {
        let source = "\\frac{<a&}";
        let class = if kind == MathType::InlineMath {
            "inline"
        } else {
            "display"
        };
        let output = HtmlWriter
            .write(&document(kind, source), &options())
            .unwrap();
        assert_eq!(
            output,
            format!(
                "<p><span class=\"math {class}\">{delimiters}\\frac{{&lt;a&amp;}}{delimiters}</span></p>"
            ),
        );
    }
}

#[test]
fn wrapping_preserves_math_text_and_annotation_spaces() {
    let document = document(MathType::InlineMath, "\\text{two  words}");
    let expected = HtmlWriter.write(&document, &options()).unwrap();
    for wrap in [WrapMode::Auto, WrapMode::Preserve] {
        let mut options = options();
        options.wrap = wrap;
        options.columns = Some(10);
        assert_eq!(HtmlWriter.write(&document, &options).unwrap(), expected);
    }
}

#[test]
fn footnotes_use_the_selected_math_method() {
    let document = Document {
        blocks: vec![Block::Para(vec![Inline::Note(vec![Block::Para(vec![
            Inline::Math(MathType::InlineMath, "x".into()),
        ])])])],
        ..Document::default()
    };
    let output = HtmlWriter.write(&document, &options()).unwrap();
    assert!(output.contains("<math display=\"inline\""));
    assert!(!output.contains("class=\"math"));
}
