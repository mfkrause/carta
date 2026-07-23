use super::{IrBlock, html_element, parse};
use carta_core::{Extension, Extensions, presets};

fn md(input: &str) -> Vec<IrBlock> {
    parse(input, presets::MARKDOWN, true).0
}

fn with(input: &str, exts: &[Extension]) -> Vec<IrBlock> {
    parse(input, Extensions::from_list(exts), true).0
}

#[test]
fn div_becomes_a_div_with_parsed_attributes_and_content() {
    let out = md("<div class=\"n\" id=\"d\">\n\n*hi* there\n\n</div>\n");
    let [IrBlock::Div(attr, content)] = out.as_slice() else {
        panic!("expected one div, got {out:?}");
    };
    assert_eq!(attr.id, "d");
    assert_eq!(attr.classes, vec!["n".to_owned()]);
    assert!(attr.attributes.is_empty());
    assert!(matches!(content.as_slice(), [IrBlock::Para(_)]));
}

#[test]
fn div_attributes_split_class_keep_id_and_preserve_keyval_order() {
    let out = md("<div data-z=\"1\" id=\"i\" data-a=\"2\" class=\"a b\">\n\nx\n\n</div>\n");
    let [IrBlock::Div(attr, _)] = out.as_slice() else {
        panic!("expected one div, got {out:?}");
    };
    assert_eq!(attr.id, "i");
    assert_eq!(attr.classes, vec!["a".to_owned(), "b".to_owned()]);
    assert_eq!(
        attr.attributes,
        vec![("data-z".into(), "1".into()), ("data-a".into(), "2".into()),]
    );
}

#[test]
fn nested_divs_balance_into_a_tree() {
    let out = md("<div class=\"outer\">\n\n<div class=\"inner\">\n\ntext\n\n</div>\n\n</div>\n");
    let [IrBlock::Div(outer, outer_children)] = out.as_slice() else {
        panic!("expected one outer div, got {out:?}");
    };
    assert_eq!(outer.classes, vec!["outer".to_owned()]);
    let [IrBlock::Div(inner, inner_children)] = outer_children.as_slice() else {
        panic!("expected one inner div, got {outer_children:?}");
    };
    assert_eq!(inner.classes, vec!["inner".to_owned()]);
    assert!(matches!(inner_children.as_slice(), [IrBlock::Para(_)]));
}

#[test]
fn div_final_block_tightens_only_when_the_close_tag_trails_content() {
    // Close tag on its own line keeps the final block as `Para`, even without blank lines.
    let para = md("<div>\nfoo\n</div>\n");
    assert!(matches!(
        para.as_slice(),
        [IrBlock::Div(_, c)] if matches!(c.as_slice(), [IrBlock::Para(_)])
    ));
    // Close tag trailing content on the same line tightens the final block to `Plain`.
    let plain = md("<div>\nfoo\nbar</div>\n");
    assert!(matches!(
        plain.as_slice(),
        [IrBlock::Div(_, c)] if matches!(c.as_slice(), [IrBlock::Plain(_)])
    ));
    // An earlier block stays `Para`; only the trailing one tightens.
    let mixed = md("<div>\n\nfoo\n\nbar</div>\n");
    let [IrBlock::Div(_, content)] = mixed.as_slice() else {
        panic!("expected one div, got {mixed:?}");
    };
    assert!(matches!(
        content.as_slice(),
        [IrBlock::Para(_), IrBlock::Plain(_)]
    ));
}

#[test]
fn multibyte_attribute_values_do_not_leak_into_following_content() {
    // The open tag is consumed by byte length, so a multibyte character in an attribute value
    // leaves no stray bytes (e.g. the trailing `>`) to be re-read as a spurious block.
    let out = md("<div class=\"café\">\n\nx\n\n</div>\n");
    let [IrBlock::Div(attr, content)] = out.as_slice() else {
        panic!("expected one div, got {out:?}");
    };
    assert_eq!(attr.classes, vec!["café".to_owned()]);
    assert!(matches!(content.as_slice(), [IrBlock::Para(_)]));
}

#[test]
fn content_after_the_close_tag_is_a_following_block() {
    let out = md("<div>\nfoo\n</div>more\n");
    assert!(matches!(
        out.as_slice(),
        [IrBlock::Div(..), IrBlock::Para(_)]
    ));
}

#[test]
fn raw_html_block_trailing_newline_depends_on_dialect() {
    // A raw HTML block (here a verbatim `<pre>`) keeps its final newline in the strict dialect
    // and drops it in the markdown dialect.
    let input = "<pre>\nhi\n</pre>\n";
    let strict = parse(input, presets::COMMONMARK, false).0;
    let [IrBlock::RawHtml(text)] = strict.as_slice() else {
        panic!("expected one raw HTML block, got {strict:?}");
    };
    assert_eq!(text, "<pre>\nhi\n</pre>\n");

    let markdown = parse(input, presets::COMMONMARK, true).0;
    let [IrBlock::RawHtml(text)] = markdown.as_slice() else {
        panic!("expected one raw HTML block, got {markdown:?}");
    };
    assert_eq!(text, "<pre>\nhi\n</pre>");
}

#[test]
fn non_div_block_tag_keeps_raw_tags_around_parsed_content() {
    let out = md("<section class=\"n\">\n\n*hi*\n\n</section>\n");
    let [
        IrBlock::RawHtml(open),
        IrBlock::Para(_),
        IrBlock::RawHtml(close),
    ] = out.as_slice()
    else {
        panic!("expected raw-open, para, raw-close; got {out:?}");
    };
    assert_eq!(open, "<section class=\"n\">");
    assert_eq!(close, "</section>");
}

#[test]
fn raw_element_final_block_tightens_when_no_blank_precedes_the_close() {
    // No blank line before the close tag: the final block is `Plain`.
    let tight = md("<section>\nfoo\n</section>\n");
    assert!(matches!(
        tight.as_slice(),
        [IrBlock::RawHtml(_), IrBlock::Plain(_), IrBlock::RawHtml(_)]
    ));
    // A blank line before the close tag keeps the final block `Para`.
    let loose = md("<section>\nfoo\n\n</section>\n");
    assert!(matches!(
        loose.as_slice(),
        [IrBlock::RawHtml(_), IrBlock::Para(_), IrBlock::RawHtml(_)]
    ));
}

#[test]
fn native_divs_off_renders_a_div_as_a_raw_element() {
    let out = with(
        "<div class=\"n\">\n*hi*\n</div>\n",
        &[Extension::MarkdownInHtmlBlocks],
    );
    let [
        IrBlock::RawHtml(open),
        IrBlock::Plain(_),
        IrBlock::RawHtml(close),
    ] = out.as_slice()
    else {
        panic!("expected raw div fallback, got {out:?}");
    };
    assert_eq!(open, "<div class=\"n\">");
    assert_eq!(close, "</div>");
}

#[test]
fn both_extensions_off_spans_a_block_element_to_its_balanced_close() {
    // With neither extension, a block-level tag at the left margin is kept verbatim as one raw
    // block spanning to its balanced close — blank lines included — rather than parsed as a div.
    let out = with("<div>\n\nfoo\n\n</div>\n", &[]);
    let [IrBlock::RawHtml(html)] = out.as_slice() else {
        panic!("expected one raw HTML block, got {out:?}");
    };
    assert_eq!(html, "<div>\n\nfoo\n\n</div>");
}

#[test]
fn inline_and_unknown_tags_are_not_block_elements() {
    // `<em>`/`<span>` are inline and `<custom>` is unrecognized: none open a block element, so
    // none produce a div.
    for input in ["<em>\n\nx\n\n</em>\n", "<custom>\n\nx\n\n</custom>\n"] {
        let out = md(input);
        assert!(
            !out.iter().any(|b| matches!(b, IrBlock::Div(..))),
            "{input:?} should not produce a div, got {out:?}"
        );
    }
}

#[test]
fn an_unclosed_element_closes_at_end_of_input_without_a_close_tag() {
    let out = md("<div>\n\nfoo\n");
    let [IrBlock::Div(_, content)] = out.as_slice() else {
        panic!("expected one div, got {out:?}");
    };
    assert!(matches!(content.as_slice(), [IrBlock::Para(_)]));
    // A raw element left open emits no trailing close tag.
    let raw = md("<section>\n\nfoo\n");
    assert!(
        !raw.iter()
            .any(|b| matches!(b, IrBlock::RawHtml(t) if t.contains("</section>"))),
        "an unclosed raw element should emit no close tag, got {raw:?}"
    );
}

#[test]
fn parse_open_tag_reads_name_attributes_and_extent() {
    let tag = html_element::parse_open_tag("<div id=\"x\" class=\"a b\" data-k=v>rest")
        .expect("a div open tag");
    assert_eq!(tag.tag, "div");
    assert_eq!(tag.attr.id, "x");
    assert_eq!(tag.attr.classes, vec!["a".to_owned(), "b".to_owned()]);
    assert_eq!(tag.attr.attributes, vec![("data-k".into(), "v".into())]);
    // The extent stops just past the `>`, leaving any same-line remainder.
    assert_eq!(tag.len, "<div id=\"x\" class=\"a b\" data-k=v>".len());
}

#[test]
fn parse_open_tag_rejects_non_block_and_malformed_tags() {
    assert!(html_element::parse_open_tag("<em>").is_none());
    assert!(html_element::parse_open_tag("<custom>").is_none());
    assert!(html_element::parse_open_tag("not a tag").is_none());
    assert!(html_element::parse_open_tag("<div class=\"oops>").is_none());
}

#[test]
fn parse_open_tag_keeps_only_the_first_class_attribute() {
    let tag = html_element::parse_open_tag("<div class=\"a\" class=\"b\">").expect("a div");
    assert_eq!(tag.attr.classes, vec!["a".to_owned()]);
}

#[test]
fn parse_open_tag_records_a_valueless_attribute_as_an_empty_value() {
    let tag = html_element::parse_open_tag("<div hidden class=\"a\">").expect("a div");
    assert_eq!(tag.attr.classes, vec!["a".to_owned()]);
    assert_eq!(tag.attr.attributes, vec![("hidden".into(), "".into())]);
}

#[test]
fn find_close_tag_locates_the_matching_name_and_skips_unrelated_ones() {
    let found = html_element::find_close_tag("foo</div>bar", "div").expect("a close tag");
    assert_eq!(&"foo</div>bar"[found.start..found.end], "</div>");
    // A different name is not the match.
    assert!(html_element::find_close_tag("</span>", "div").is_none());
    // Trailing whitespace before `>` is allowed; a bare name is not a close tag.
    assert!(html_element::find_close_tag("</div >", "div").is_some());
    assert!(html_element::find_close_tag("no tag here", "div").is_none());
}

#[test]
fn parse_open_tag_flags_a_self_closing_tag() {
    assert!(
        html_element::parse_open_tag("<div/>")
            .expect("a div")
            .self_closing
    );
    assert!(
        html_element::parse_open_tag("<hr />")
            .expect("an hr")
            .self_closing
    );
    assert!(
        !html_element::parse_open_tag("<div>")
            .expect("a div")
            .self_closing
    );
}

#[test]
fn parse_close_tag_matches_a_leading_block_close_tag() {
    assert_eq!(
        html_element::parse_close_tag("</div>rest"),
        Some("</div>".len())
    );
    assert_eq!(
        html_element::parse_close_tag("</div  >"),
        Some("</div  >".len())
    );
    // Only a block-level name, only at the very start.
    assert_eq!(html_element::parse_close_tag("</span>"), None);
    assert_eq!(html_element::parse_close_tag("x</div>"), None);
    assert_eq!(html_element::parse_close_tag("<div>"), None);
}

#[test]
fn scan_depth_balances_nested_same_name_tags() {
    // A same-name open raises the depth; the matching close returns it to zero mid-line.
    let (depth, close) = html_element::scan_depth("<div>a</div></div>", "div", 1);
    assert_eq!(depth, 0);
    assert_eq!(close, Some("<div>a</div>".len() + "</div>".len()));
    // A self-closing same-name tag does not raise the depth.
    assert_eq!(html_element::scan_depth("<div/>", "div", 1), (1, None));
    // A different tag's `>` inside its attributes is skipped whole.
    assert_eq!(
        html_element::scan_depth("<td x=\">\">", "div", 1),
        (1, None)
    );
}
