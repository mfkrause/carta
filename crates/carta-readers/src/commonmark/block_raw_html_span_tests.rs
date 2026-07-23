use super::{IrBlock, parse};
use carta_core::{Extension, Extensions};

/// The Markdown family reading raw HTML where inner content is not parsed (the column-zero,
/// no-interrupt gate: neither `markdown_attribute` nor the div/markdown-in-HTML extensions).
fn strict(input: &str) -> Vec<IrBlock> {
    parse(input, Extensions::empty(), true).0
}

/// The same reading with `markdown_attribute`, which lets the block indent and interrupt.
fn attr(input: &str) -> Vec<IrBlock> {
    parse(
        input,
        Extensions::from_list(&[Extension::MarkdownAttribute]),
        true,
    )
    .0
}

#[test]
fn a_block_element_spans_to_its_balanced_close() {
    let out = strict("<div>\nx\n\ny\n</div>\n");
    let [IrBlock::RawHtml(html)] = out.as_slice() else {
        panic!("expected one raw HTML block, got {out:?}");
    };
    assert_eq!(html, "<div>\nx\n\ny\n</div>");
}

#[test]
fn nested_same_name_tags_balance() {
    let out = strict("<div>\n<div>\na\n</div>\nb\n</div>\n");
    let [IrBlock::RawHtml(html)] = out.as_slice() else {
        panic!("expected one raw HTML block, got {out:?}");
    };
    assert_eq!(html, "<div>\n<div>\na\n</div>\nb\n</div>");
}

#[test]
fn a_void_tag_is_a_single_line_block_and_the_rest_parses() {
    let out = strict("<hr>\ntext\n");
    let [IrBlock::RawHtml(html), IrBlock::Para(text)] = out.as_slice() else {
        panic!("expected a single-tag raw block then a paragraph, got {out:?}");
    };
    assert_eq!(html, "<hr>");
    assert_eq!(text, "text");
}

#[test]
fn a_self_closing_tag_is_a_single_line_block() {
    let out = strict("<div/>\ntext\n");
    assert!(
        matches!(out.first(), Some(IrBlock::RawHtml(h)) if h == "<div/>"),
        "a self-closing tag opens no span: {out:?}"
    );
}

#[test]
fn a_bare_close_tag_stands_alone() {
    let out = strict("</div>\ntext\n");
    let [IrBlock::RawHtml(html), IrBlock::Para(_)] = out.as_slice() else {
        panic!("expected a lone close tag then a paragraph, got {out:?}");
    };
    assert_eq!(html, "</div>");
}

#[test]
fn an_open_and_close_on_one_line_re_feeds_the_trailing_text() {
    let out = strict("<div>x</div> tail\n");
    let [IrBlock::RawHtml(html), IrBlock::Para(text)] = out.as_slice() else {
        panic!("expected a raw block then the trailing text, got {out:?}");
    };
    assert_eq!(html, "<div>x</div>");
    assert_eq!(text, "tail");
}

#[test]
fn an_unclosed_open_tag_is_the_tag_alone() {
    let out = strict("<div>\nx\n\ny\n");
    let [IrBlock::RawHtml(html), IrBlock::Para(a), IrBlock::Para(b)] = out.as_slice() else {
        panic!("expected the tag alone then two paragraphs, got {out:?}");
    };
    assert_eq!(html, "<div>");
    assert_eq!(a, "x");
    assert_eq!(b, "y");
}

#[test]
fn an_inline_or_unknown_tag_opens_no_block() {
    for input in ["<span>x</span>\ntext\n", "<foo>\nx\n</foo>\n"] {
        let out = strict(input);
        assert!(
            !out.iter().any(|b| matches!(b, IrBlock::RawHtml(_))),
            "a non-block tag stays inline: {out:?}"
        );
    }
}

#[test]
fn without_markdown_attribute_an_indented_tag_is_inline() {
    let out = strict("   <div>\nx\n</div>\n");
    assert!(
        !out.iter().any(|b| matches!(b, IrBlock::RawHtml(_))),
        "an indented tag folds into a paragraph: {out:?}"
    );
}

#[test]
fn with_markdown_attribute_an_indented_tag_opens_a_block() {
    let out = attr("   <div>\nx\n</div>\n");
    let [IrBlock::RawHtml(html)] = out.as_slice() else {
        panic!("expected one raw HTML block, got {out:?}");
    };
    assert_eq!(html, "<div>\nx\n</div>");
}

#[test]
fn without_markdown_attribute_the_block_folds_into_a_paragraph() {
    let out = strict("text\n<div>\nx\n</div>\n");
    assert!(
        matches!(out.as_slice(), [IrBlock::Para(_)]),
        "the block does not interrupt the paragraph: {out:?}"
    );
}

#[test]
fn with_markdown_attribute_the_block_interrupts_a_paragraph() {
    let out = attr("text\n<div>\nx\n</div>\n");
    let [IrBlock::Plain(text), IrBlock::RawHtml(html)] = out.as_slice() else {
        panic!("expected a tight paragraph then a raw block, got {out:?}");
    };
    assert_eq!(text, "text");
    assert_eq!(html, "<div>\nx\n</div>");
}
