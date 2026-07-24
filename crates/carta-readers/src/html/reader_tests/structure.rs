//! Tests for block structure, lists, tables, and loose-run handling.

use super::common::{blocks, first_block, para_inlines};
use carta_ast::{Alignment, Block, ColWidth, Inline, ListNumberStyle};

#[test]
fn paragraph_with_emphasis() {
    let result = blocks("<p>a <em>b</em></p>");
    assert!(matches!(result.as_slice(), [Block::Para(_)]));
}

#[test]
fn loose_text_is_plain() {
    assert!(matches!(blocks("hello").as_slice(), [Block::Plain(_)]));
}

#[test]
fn paragraph_sibling_promotes_loose_text() {
    let result = blocks("loose<p>para</p>");
    assert!(matches!(
        result.as_slice(),
        [Block::Para(_), Block::Para(_)]
    ));
}

#[test]
fn horizontal_rule_does_not_promote() {
    let result = blocks("loose<hr>");
    assert!(matches!(
        result.as_slice(),
        [Block::Plain(_), Block::HorizontalRule]
    ));
}

#[test]
fn nested_list_inside_item_stays_tight() {
    let result = blocks("<ul><li>a<ul><li>b</li></ul></li></ul>");
    let Some(Block::BulletList(items)) = result.first() else {
        panic!("expected bullet list");
    };
    let Some(item) = items.first() else {
        panic!("expected one item");
    };
    assert!(matches!(item.first(), Some(Block::Plain(_))));
}

#[test]
fn framing_div_keeps_loose_run_plain() {
    let Block::Div(_, inner) = first_block("<div>loose<p>para</p></div>") else {
        panic!("expected div");
    };
    assert!(matches!(
        inner.as_slice(),
        [Block::Plain(_), Block::Para(_)]
    ));
}

#[test]
fn blockquote_promotes_loose_run() {
    let Block::BlockQuote(inner) = first_block("<blockquote>loose<p>para</p></blockquote>") else {
        panic!("expected blockquote");
    };
    assert!(matches!(inner.as_slice(), [Block::Para(_), Block::Para(_)]));
}

#[test]
fn figure_caption_and_content_keep_loose_runs_plain() {
    let Block::Figure(_, caption, content) = first_block(
        "<figure>loose fig<p>fig para</p><figcaption>loose cap<p>cap para</p></figcaption></figure>",
    ) else {
        panic!("expected figure");
    };
    assert!(matches!(
        content.as_slice(),
        [Block::Plain(_), Block::Para(_)]
    ));
    assert!(matches!(
        caption.long.as_slice(),
        [Block::Plain(_), Block::Para(_)]
    ));
}

#[test]
fn table_cell_keeps_loose_run_plain() {
    let Block::Table(table) = first_block("<table><tr><td>loose<p>para</p></td></tr></table>")
    else {
        panic!("expected table");
    };
    let content = table
        .bodies
        .first()
        .and_then(|body| body.body.first())
        .and_then(|row| row.cells.first())
        .map(|cell| cell.content.as_slice());
    assert!(matches!(content, Some([Block::Plain(_), Block::Para(_)])));
}

#[test]
fn ordered_list_reads_type_and_start() {
    let Block::OrderedList(attrs, items) =
        first_block(r#"<ol type="A" start="3"><li>x</li><li>y</li></ol>"#)
    else {
        panic!("expected ordered list");
    };
    assert_eq!(attrs.start, 3);
    assert_eq!(attrs.style, ListNumberStyle::UpperAlpha);
    assert_eq!(items.len(), 2);
}

#[test]
fn menu_is_a_bullet_list() {
    assert!(matches!(
        first_block("<menu><li>a</li></menu>"),
        Block::BulletList(_)
    ));
}

#[test]
fn implied_li_close_splits_items() {
    let Block::BulletList(items) = first_block("<ul><li>a<li>b</ul>") else {
        panic!("expected bullet list");
    };
    assert_eq!(items.len(), 2);
}

#[test]
fn pre_with_code_language_class_becomes_code_block() {
    let Block::CodeBlock(attr, text) = first_block(
        r#"<pre><code class="language-rust">let x = 1;
</code></pre>"#,
    ) else {
        panic!("expected code block");
    };
    assert_eq!(attr.classes, vec!["rust".to_string()]);
    assert_eq!(text, "let x = 1;");
}

#[test]
fn definition_list_pairs_terms_and_definitions() {
    let Block::DefinitionList(items) =
        first_block("<dl><dt>term</dt><dd>one</dd><dd>two</dd></dl>")
    else {
        panic!("expected definition list");
    };
    let (term, defs) = items.into_iter().next().expect("an item");
    assert_eq!(term, vec![Inline::Str("term".to_string().into())]);
    assert_eq!(defs.len(), 2);
}

#[test]
fn definition_list_sees_through_grouping_divs() {
    // wrapping each dt/dd pair in a div is valid HTML5; the grouping is transparent
    let Block::DefinitionList(items) =
        first_block("<dl><div><dt>t1</dt><dd>d1</dd></div><div><dt>t2</dt><dd>d2</dd></div></dl>")
    else {
        panic!("expected definition list");
    };
    assert_eq!(items.len(), 2);
    let (term, defs) = items.into_iter().next().expect("an item");
    assert_eq!(term, vec![Inline::Str("t1".to_string().into())]);
    assert_eq!(defs.len(), 1);
}

#[test]
fn block_level_anchor_splits_into_link_and_blocks() {
    // an <a> may wrap block content: the leading inline run becomes a link, block children follow
    let result = blocks("<a href=\"u\">before<p>inside</p>after</a>");
    let Some(Block::Para(lead)) = result.first() else {
        panic!("expected a leading paragraph");
    };
    assert!(matches!(lead.first(), Some(Inline::Link(..))));
    assert!(matches!(
        result.as_slice(),
        [Block::Para(_), Block::Para(_), Block::Para(_)]
    ));
}

#[test]
fn blockquote_wraps_child_blocks() {
    assert!(matches!(
        first_block("<blockquote><p>q</p></blockquote>"),
        Block::BlockQuote(_)
    ));
}

#[test]
fn sectioning_div_gets_a_class() {
    let Block::Div(attr, _) = first_block("<section><p>x</p></section>") else {
        panic!("expected div");
    };
    assert!(attr.classes.contains(&"section".into()));
}

#[test]
fn figure_separates_caption_from_content() {
    let Block::Figure(_, caption, content) =
        first_block("<figure><img src=\"a.png\"><figcaption>cap</figcaption></figure>")
    else {
        panic!("expected figure");
    };
    assert_eq!(caption.short, None);
    assert!(!caption.long.is_empty());
    assert!(!content.is_empty());
}

#[test]
fn table_reads_sections_alignment_and_spans() {
    let input = r#"<table>
        <caption>cap</caption>
        <colgroup><col style="width: 25%"><col></colgroup>
        <thead><tr><th align="right">H1</th><th>H2</th></tr></thead>
        <tbody><tr><td colspan="2">wide</td></tr></tbody>
        <tfoot><tr><td>f1</td><td>f2</td></tr></tfoot>
    </table>"#;
    let Block::Table(table) = first_block(input) else {
        panic!("expected table");
    };
    assert_eq!(table.col_specs.len(), 2);
    assert_eq!(
        table.col_specs.first().map(|spec| spec.width.clone()),
        Some(ColWidth::ColWidth(0.25))
    );
    assert_eq!(
        table
            .head
            .rows
            .first()
            .and_then(|row| row.cells.first())
            .map(|cell| cell.align.clone()),
        Some(Alignment::AlignRight)
    );
    let body_cell_span = table
        .bodies
        .first()
        .and_then(|body| body.body.first())
        .and_then(|row| row.cells.first())
        .map(|cell| cell.col_span);
    assert_eq!(body_cell_span, Some(2));
    assert_eq!(table.foot.rows.len(), 1);
}

#[test]
fn oversized_cell_spans_are_clamped() {
    // spans are clamped to the HTML spec limits: each spanned slot materialises, so an
    // unclamped colspan="90000000" would force a multi-gigabyte allocation
    let input =
        r#"<table><tr><td colspan="90000000" rowspan="2">x</td></tr><tr><td>y</td></tr></table>"#;
    let Block::Table(table) = first_block(input) else {
        panic!("expected table");
    };
    let cell_span = table
        .bodies
        .first()
        .and_then(|body| body.body.first())
        .and_then(|row| row.cells.first())
        .map(|cell| (cell.col_span, cell.row_span));
    assert_eq!(cell_span, Some((1000, 2)));
}

#[test]
fn cell_alignment_reads_text_align_style() {
    let Block::Table(table) =
        first_block(r#"<table><tr><td style="text-align: center">c</td></tr></table>"#)
    else {
        panic!("expected table");
    };
    let align = table
        .bodies
        .first()
        .and_then(|body| body.body.first())
        .and_then(|row| row.cells.first())
        .map(|cell| cell.align.clone());
    assert_eq!(align, Some(Alignment::AlignCenter));
}

#[test]
fn checkbox_in_item_renders_ballot_box() {
    let Block::BulletList(items) =
        first_block(r#"<ul><li><input type="checkbox" checked/>do it</li></ul>"#)
    else {
        panic!("expected bullet list");
    };
    let Some([Block::Plain(inlines)]) = items.first().map(Vec::as_slice) else {
        panic!("expected one plain block");
    };
    assert!(matches!(inlines.first(), Some(Inline::Str(s)) if s == "\u{2612}"));
}

#[test]
fn checkbox_outside_item_is_dropped() {
    let inlines = para_inlines(r#"<p><input type="checkbox"/>text</p>"#);
    assert_eq!(inlines.as_slice(), [Inline::Str("text".to_string().into())]);
}

#[test]
fn paragraph_with_checkbox_demotes_to_plain() {
    assert!(matches!(
        first_block(r#"<p><input type="checkbox"/>x</p>"#),
        Block::Plain(_)
    ));
}

#[test]
fn empty_paragraph_is_dropped() {
    assert!(blocks("<p>hi</p><p></p><p>lo</p>").len() == 2);
}

#[test]
fn consecutive_terms_merge_with_line_break() {
    let Block::DefinitionList(items) = first_block("<dl><dt>a</dt><dt>b</dt><dd>x</dd></dl>")
    else {
        panic!("expected definition list");
    };
    let Some((term, _)) = items.first() else {
        panic!("expected one item");
    };
    assert!(term.contains(&Inline::LineBreak));
}

#[test]
fn stray_paragraph_in_list_attaches_to_item() {
    let Block::BulletList(items) = first_block("<ul><li>a</li><p>b</p></ul>") else {
        panic!("expected bullet list");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items.first().map(Vec::len), Some(2));
}
