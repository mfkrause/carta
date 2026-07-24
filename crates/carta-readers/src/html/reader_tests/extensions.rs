//! Tests for extension toggles: native divs/spans, auto identifiers, line blocks, smart, and math.

use super::common::{first_block, para_inlines, para_inlines_ext, read_with, read_with_text_ext};
use carta_ast::{Block, Inline, MathType};
use carta_core::{Extension, Extensions};

#[test]
fn line_block_div_becomes_line_block() {
    let Block::LineBlock(lines) = first_block(r#"<div class="line-block">a<br>b</div>"#) else {
        panic!("expected line block");
    };
    assert_eq!(lines.len(), 2);
}

#[test]
fn line_block_div_with_id_stays_div() {
    assert!(matches!(
        first_block(r#"<div class="line-block" id="x">a</div>"#),
        Block::Div(..)
    ));
}

#[test]
fn native_divs_off_splices_div_children() {
    let result = read_with("<div class=\"c\"><p>x</p></div>", Extensions::empty());
    assert!(matches!(result.as_slice(), [Block::Para(_)]));
}

#[test]
fn native_divs_off_drops_sectioning_wrapper() {
    let result = read_with("<section><p>x</p></section>", Extensions::empty());
    assert!(matches!(result.as_slice(), [Block::Para(_)]));
}

#[test]
fn native_spans_off_unwraps_span_and_small_caps() {
    let plain = read_with("<p><span class=\"c\">x</span></p>", Extensions::empty());
    let Some(Block::Para(inlines)) = plain.first() else {
        panic!("expected paragraph");
    };
    assert_eq!(inlines.as_slice(), [Inline::Str("x".to_string().into())]);

    let caps = read_with(
        "<p><span style=\"font-variant: small-caps\">x</span></p>",
        Extensions::empty(),
    );
    let Some(Block::Para(inlines)) = caps.first() else {
        panic!("expected paragraph");
    };
    assert_eq!(inlines.as_slice(), [Inline::Str("x".to_string().into())]);
}

#[test]
fn native_spans_off_keeps_class_carrying_inlines() {
    // mark/kbd are their own constructs, not <span> elements, so the toggle leaves them
    let result = read_with("<p><mark>m</mark></p>", Extensions::empty());
    let Some(Block::Para(inlines)) = result.first() else {
        panic!("expected paragraph");
    };
    assert!(matches!(inlines.first(), Some(Inline::Span(_, _))));
}

#[test]
fn auto_identifiers_off_leaves_id_empty_but_keeps_explicit() {
    let generated = read_with("<h1>Hello World</h1>", Extensions::empty());
    let Some(Block::Header(_, attr, _)) = generated.first() else {
        panic!("expected header");
    };
    assert_eq!(attr.id, "");

    let explicit = read_with("<h2 id=\"keep\">T</h2>", Extensions::empty());
    let Some(Block::Header(_, attr, _)) = explicit.first() else {
        panic!("expected header");
    };
    assert_eq!(attr.id, "keep");
}

#[test]
fn line_blocks_off_keeps_a_plain_div() {
    let result = read_with(
        "<div class=\"line-block\">a<br>b</div>",
        Extensions::from_list(&[Extension::NativeDivs]),
    );
    let Some(Block::Div(attr, children)) = result.first() else {
        panic!("expected div");
    };
    assert_eq!(attr.classes, vec!["line-block".to_string()]);
    assert!(matches!(children.as_slice(), [Block::Plain(_)]));
}

#[test]
fn smart_off_keeps_literal_punctuation() {
    let inlines = para_inlines("<p>\"a\" -- ... ---</p>");
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Str("\"a\"".to_string().into()),
            Inline::Space,
            Inline::Str("--".to_string().into()),
            Inline::Space,
            Inline::Str("...".to_string().into()),
            Inline::Space,
            Inline::Str("---".to_string().into()),
        ]
    );
}

#[test]
fn smart_on_curls_quotes_and_folds_dashes() {
    let inlines = para_inlines_ext("<p>\"a\" -- ... ---</p>", &[Extension::Smart]);
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Quoted(
                carta_ast::QuoteType::DoubleQuote,
                vec![Inline::Str("a".to_string().into())]
            ),
            Inline::Space,
            Inline::Str("\u{2013}".to_string().into()),
            Inline::Space,
            Inline::Str("\u{2026}".to_string().into()),
            Inline::Space,
            Inline::Str("\u{2014}".to_string().into()),
        ]
    );
}

#[test]
fn tex_math_dollars_off_keeps_literal_text() {
    let inlines = para_inlines("<p>$x^2$ and $$y$$</p>");
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Str("$x^2$".to_string().into()),
            Inline::Space,
            Inline::Str("and".to_string().into()),
            Inline::Space,
            Inline::Str("$$y$$".to_string().into()),
        ]
    );
}

#[test]
fn tex_math_dollars_on_splits_inline_and_display() {
    let inlines = para_inlines_ext("<p>$x^2$ and $$y$$</p>", &[Extension::TexMathDollars]);
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Math(MathType::InlineMath, "x^2".to_string().into()),
            Inline::Space,
            Inline::Str("and".to_string().into()),
            Inline::Space,
            Inline::Math(MathType::DisplayMath, "y".to_string().into()),
        ]
    );
}

#[test]
fn tex_math_single_backslash_on_splits_inline_and_display() {
    let inlines = para_inlines_ext(
        "<p>\\(x\\) and \\[y\\]</p>",
        &[Extension::TexMathSingleBackslash],
    );
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Math(MathType::InlineMath, "x".to_string().into()),
            Inline::Space,
            Inline::Str("and".to_string().into()),
            Inline::Space,
            Inline::Math(MathType::DisplayMath, "y".to_string().into()),
        ]
    );
}

#[test]
fn tex_math_double_backslash_on_splits_inline_and_display() {
    let inlines = para_inlines_ext(
        "<p>\\\\(x\\\\) and \\\\[y\\\\]</p>",
        &[Extension::TexMathDoubleBackslash],
    );
    assert_eq!(
        inlines.as_slice(),
        [
            Inline::Math(MathType::InlineMath, "x".to_string().into()),
            Inline::Space,
            Inline::Str("and".to_string().into()),
            Inline::Space,
            Inline::Math(MathType::DisplayMath, "y".to_string().into()),
        ]
    );
}

fn header_ids(input: &str, added: &[Extension]) -> Vec<String> {
    read_with_text_ext(input, added)
        .into_iter()
        .filter_map(|block| match block {
            Block::Header(_, attr, _) => Some(attr.id.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn gfm_auto_identifiers_drops_dots_keeps_digits_and_does_not_collapse() {
    // gfm slug: dots dropped, leading digits kept, no separator-run collapsing
    let ids = header_ids(
        "<h2>1.2 Section A.B</h2><h2>Tools &amp; Tips</h2>",
        &[Extension::GfmAutoIdentifiers],
    );
    assert_eq!(ids, vec!["12-section-ab", "tools--tips"]);
}

#[test]
fn gfm_auto_identifiers_keep_the_section_fallback_and_increment_on_collision() {
    let ids = header_ids(
        "<h2>Repeat</h2><h2>Repeat</h2><h3>!!!</h3>",
        &[Extension::GfmAutoIdentifiers],
    );
    assert_eq!(ids, vec!["repeat", "repeat-1", "section"]);
}

#[test]
fn gfm_auto_identifiers_need_auto_identifiers_to_take_effect() {
    let ids = read_with(
        "<h2>1.2 Section A.B</h2>",
        Extensions::from_list(&[Extension::GfmAutoIdentifiers]),
    )
    .into_iter()
    .filter_map(|block| match block {
        Block::Header(_, attr, _) => Some(attr.id.to_string()),
        _ => None,
    })
    .collect::<Vec<_>>();
    assert_eq!(ids, vec![String::new()]);
}

#[test]
fn repeated_headings_resume_probing_from_the_last_issued_suffix() {
    let ids = header_ids("<h2>Same</h2><h2>Same</h2><h2>Same</h2><h2>Same</h2>", &[]);
    assert_eq!(ids, vec!["same", "same-1", "same-2", "same-3"]);
}

#[test]
fn repeated_headings_skip_an_id_reserved_by_an_explicit_heading() {
    let ids = header_ids(
        "<h2 id=\"same-2\">Explicit</h2><h2>Same</h2><h2>Same</h2><h2>Same</h2>",
        &[],
    );
    assert_eq!(ids, vec!["same-2", "same", "same-1", "same-3"]);
}
