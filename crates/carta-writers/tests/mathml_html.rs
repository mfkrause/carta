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
fn math_methods_use_their_selected_markup() {
    let document = document(MathType::InlineMath, "x^2");
    for (method, expected) in [
        (MathMethod::Plain, "<em>x</em><sup>2</sup>"),
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

#[test]
fn webtex_encodes_unicode_and_escapes_image_attributes() {
    let mut options = options();
    options.math_method = MathMethod::Webtex("https://example.test/?a=1&tex=".into());
    for (kind, class, source, encoded, escaped) in [
        (
            MathType::InlineMath,
            "inline",
            " x + α ",
            "%5Ctextstyle%20x%20%2B%20%CE%B1",
            "x + α",
        ),
        (
            MathType::DisplayMath,
            "display",
            "\\frac{1}{2}<&\"'",
            "%5Cdisplaystyle%20%5Cfrac%7B1%7D%7B2%7D%3C%26%22%27",
            "\\frac{1}{2}&lt;&amp;&quot;&#39;",
        ),
    ] {
        let expected = format!(
            "<p><img style=\"vertical-align:middle\" src=\"https://example.test/?a=1&amp;tex={encoded}\" alt=\"{escaped}\" title=\"{escaped}\" class=\"math {class}\" /></p>"
        );
        let document = document(kind, source);
        assert_eq!(HtmlWriter.write(&document, &options).unwrap(), expected);
        assert_eq!(Html4Writer.write(&document, &options).unwrap(), expected);
    }
}

#[cfg(feature = "revealjs")]
#[test]
fn slides_use_the_selected_math_method() {
    let document = document(MathType::InlineMath, "x^2");
    for (method, markup) in [
        (MathMethod::MathJax("math.js".into()), "\\(x^2\\)"),
        (
            MathMethod::Webtex("https://example.test/?q=".into()),
            "src=\"https://example.test/?q=%5Ctextstyle%20x%5E2\"",
        ),
    ] {
        let mut options = options();
        options.math_method = method;
        let output = carta_writers::RevealjsWriter
            .write(&document, &options)
            .unwrap();
        assert!(output.contains(markup), "{output}");
    }
}
