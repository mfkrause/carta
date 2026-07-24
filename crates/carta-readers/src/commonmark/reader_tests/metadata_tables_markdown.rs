//! YAML metadata, title-block, table-caption and markdown inline tests.

use super::*;
use carta_ast::Alignment;

#[test]
fn a_yaml_metadata_block_populates_meta_and_is_removed_from_the_body() {
    use carta_ast::MetaValue;
    let doc = document(
        "---\ntitle: A Note\nflag: true\nempty: ~\nrevision: 007\n---\n\nBody.\n",
        &[Extension::YamlMetadataBlock],
    );
    assert!(matches!(
        doc.meta.get("title"),
        Some(MetaValue::MetaInlines(_))
    ));
    assert_eq!(doc.meta.get("flag"), Some(&MetaValue::MetaBool(true)));
    assert_eq!(
        doc.meta.get("empty"),
        Some(&MetaValue::MetaString(carta_ast::Text::default()))
    );
    // An unquoted numeric scalar is canonicalized before it is parsed as inline markdown.
    assert_eq!(
        doc.meta.get("revision"),
        Some(&MetaValue::MetaInlines(vec![Inline::Str(
            "7".to_owned().into()
        )]))
    );
    assert!(matches!(doc.blocks.as_slice(), [Block::Para(_)]));
}

#[test]
fn a_yaml_block_without_a_closing_fence_is_not_metadata() {
    let doc = document(
        "---\ntitle: A Note\n\nBody.\n",
        &[Extension::YamlMetadataBlock],
    );
    assert!(doc.meta.is_empty());
}

#[test]
fn yaml_metadata_is_inert_without_the_extension() {
    let doc = document("---\nk: v\n---\n\nBody.\n", &[]);
    assert!(doc.meta.is_empty());
}

#[test]
fn a_title_block_sets_title_author_and_date() {
    use carta_ast::MetaValue;
    let doc = document(
        "% A Note\n% Ada; Grace\n% 2026\n\nBody.\n",
        &[Extension::PandocTitleBlock],
    );
    assert!(matches!(
        doc.meta.get("title"),
        Some(MetaValue::MetaInlines(_))
    ));
    match doc.meta.get("author") {
        Some(MetaValue::MetaList(authors)) => assert_eq!(authors.len(), 2),
        other => panic!("expected two authors, got {other:?}"),
    }
    assert!(matches!(
        doc.meta.get("date"),
        Some(MetaValue::MetaInlines(_))
    ));
    assert!(matches!(doc.blocks.as_slice(), [Block::Para(_)]));
}

#[test]
fn malformed_yaml_metadata_is_an_error() {
    let mut options = ReaderOptions::default();
    options.extensions = Extensions::from_list(&[Extension::YamlMetadataBlock]);
    let error = CommonmarkReader
        .read("---\nx: [\n---\n\nBody.\n", &options)
        .expect_err("malformed metadata should fail");
    assert!(matches!(error, carta_core::Error::InvalidMetadata(_)));
}

#[test]
fn a_pipe_table_takes_a_below_caption() {
    let doc = document(
        "| a | b |\n|---|---|\n| 1 | 2 |\n\nTable: A caption.\n",
        &[Extension::PipeTables, Extension::TableCaptions],
    );
    assert!(matches!(doc.blocks.as_slice(), [Block::Table(_)]));
    let inlines = caption_inlines(&doc.blocks).expect("captioned table");
    assert_eq!(inlines.first(), Some(&Inline::Str("A".to_owned().into())));
}

#[test]
fn an_indented_simple_table_header_aligns_against_its_own_column() {
    // Alignment reads the header against each column's own ruling, not the ruling's left margin.
    let doc = read_markdown(
        "  Right     Left     Center\n-------   ------   ----------\n     12     12        12\n",
        &[Extension::SimpleTables],
    );
    let aligns: Vec<Alignment> = match doc.blocks.as_slice() {
        [Block::Table(table)] => table
            .col_specs
            .iter()
            .map(|spec| spec.align.clone())
            .collect(),
        other => panic!("expected a single table, got {other:?}"),
    };
    assert_eq!(
        aligns,
        vec![
            Alignment::AlignRight,
            Alignment::AlignRight,
            Alignment::AlignCenter,
        ]
    );
}

#[test]
fn a_paragraph_interrupted_by_an_html_block_reads_tight() {
    // No blank before the div: it interrupts, and the paragraph reads tight (`Plain`).
    let doc = read_markdown(
        "text before\n<div>\ninside\n</div>\n",
        &[Extension::MarkdownInHtmlBlocks, Extension::NativeDivs],
    );
    assert!(
        matches!(doc.blocks.as_slice(), [Block::Plain(_), Block::Div(..)]),
        "expected a tight paragraph then a div, got {:?}",
        doc.blocks
    );

    // A blank line before the element leaves the paragraph loose, so it stays a full paragraph.
    let loose = read_markdown(
        "text before\n\n<div>\ninside\n</div>\n",
        &[Extension::MarkdownInHtmlBlocks, Extension::NativeDivs],
    );
    assert!(
        matches!(loose.blocks.as_slice(), [Block::Para(_), Block::Div(..)]),
        "expected a loose paragraph then a div, got {:?}",
        loose.blocks
    );
}

#[test]
fn a_simple_table_takes_an_above_caption() {
    let doc = document(
        "table: Above it.\n\nName   Age\n----   ---\nAnn    9\n",
        &[Extension::SimpleTables, Extension::TableCaptions],
    );
    assert!(matches!(doc.blocks.as_slice(), [Block::Table(_)]));
    assert!(caption_inlines(&doc.blocks).is_some());
}

#[test]
fn a_multiline_caption_folds_across_lines() {
    let doc = document(
        "| a | b |\n|---|---|\n| 1 | 2 |\n\nTable: First line\nsecond line.\n",
        &[Extension::PipeTables, Extension::TableCaptions],
    );
    let inlines = caption_inlines(&doc.blocks).expect("captioned table");
    assert!(inlines.contains(&Inline::SoftBreak));
}

#[test]
fn a_bare_colon_below_a_pipe_table_is_a_caption_not_a_definition() {
    // Below a pipe table `:` is the caption, not a definition marker.
    let doc = document(
        "| a | b |\n|---|---|\n| 1 | 2 |\n\n: A bare-colon caption.\n",
        &[
            Extension::PipeTables,
            Extension::TableCaptions,
            Extension::DefinitionLists,
        ],
    );
    assert!(matches!(doc.blocks.as_slice(), [Block::Table(_)]));
    assert!(caption_inlines(&doc.blocks).is_some());
}

#[test]
fn an_uppercase_table_marker_is_not_a_caption() {
    let doc = document(
        "| a | b |\n|---|---|\n| 1 | 2 |\n\nTABLE: not a caption\n",
        &[Extension::PipeTables, Extension::TableCaptions],
    );
    assert!(matches!(
        doc.blocks.as_slice(),
        [Block::Table(_), Block::Para(_)]
    ));
    assert!(caption_inlines(&doc.blocks).is_none());
}

#[test]
fn an_ordinary_definition_list_is_unaffected_by_caption_handling() {
    let doc = document(
        "Term\n\n: Its definition.\n",
        &[
            Extension::PipeTables,
            Extension::TableCaptions,
            Extension::DefinitionLists,
        ],
    );
    assert!(matches!(doc.blocks.as_slice(), [Block::DefinitionList(_)]));
}

#[test]
fn markdown_nests_strong_outside_emph_for_a_triple_run() {
    let inlines = md_para("***both***\n", &[]);
    assert!(
        matches!(
            inlines.as_slice(),
            [Inline::Strong(inner)]
                if matches!(inner.as_slice(), [Inline::Emph(text)]
                    if matches!(text.as_slice(), [Inline::Str(s)] if s == "both"))
        ),
        "expected Strong[Emph[both]], got {inlines:?}"
    );
}

#[test]
fn markdown_keeps_a_run_of_four_delimiters_literal() {
    let inlines = md_para("****a****\n", &[]);
    assert!(
        matches!(inlines.as_slice(), [Inline::Str(s)] if s == "****a****"),
        "expected literal text, got {inlines:?}"
    );
}

#[test]
fn markdown_underscore_triple_run_also_nests_strong_outside() {
    let inlines = md_para("___both___\n", &[]);
    assert!(
        matches!(
            inlines.as_slice(),
            [Inline::Strong(inner)] if matches!(inner.as_slice(), [Inline::Emph(_)])
        ),
        "expected Strong[Emph[..]], got {inlines:?}"
    );
}

#[test]
fn markdown_uri_autolink_carries_the_uri_class() {
    let inlines = md_para("<http://example.com>\n", &[]);
    let (attr, target) = single_link(&inlines).expect("a single link");
    assert_eq!(attr.classes, vec!["uri".to_owned()]);
    assert_eq!(target.url, "http://example.com");
}

#[test]
fn markdown_email_autolink_carries_the_email_class_and_mailto_url() {
    let inlines = md_para("<a@b.com>\n", &[]);
    let (attr, target) = single_link(&inlines).expect("a single link");
    assert_eq!(attr.classes, vec!["email".to_owned()]);
    assert_eq!(target.url, "mailto:a@b.com");
}

#[test]
fn markdown_scheme_autolink_carries_the_uri_class() {
    for input in ["<ftp://x.y>\n", "<mailto:a@b.com>\n", "<tel:+123>\n"] {
        let inlines = md_para(input, &[]);
        let (attr, _) = single_link(&inlines).expect("a single link");
        assert_eq!(attr.classes, vec!["uri".to_owned()], "for {input:?}");
    }
}

#[test]
fn commonmark_angle_autolink_carries_no_class() {
    let inlines = match blocks("<http://example.com>\n").as_slice() {
        [Block::Para(inlines)] => inlines.clone(),
        other => panic!("expected a paragraph, got {other:?}"),
    };
    let (attr, _) = single_link(&inlines).expect("a single link");
    assert!(
        attr.classes.is_empty(),
        "expected empty classes, got {attr:?}"
    );
}

#[test]
fn markdown_link_destination_keeps_balanced_inner_parentheses() {
    let inlines = md_para("[c](/u (d))\n", &[]);
    let (_, target) = single_link(&inlines).expect("a single link");
    // The space is percent-encoded and the inner `(d)` is part of the destination.
    assert_eq!(target.url, "/u%20(d)");
    assert_eq!(target.title, "");
}

#[test]
fn markdown_link_destination_separates_a_trailing_title() {
    let inlines = md_para("[c](/u (d) \"t\")\n", &[]);
    let (_, target) = single_link(&inlines).expect("a single link");
    assert_eq!(target.url, "/u%20(d)");
    assert_eq!(target.title, "t");
}

#[test]
fn markdown_link_destination_keeps_nested_balanced_parentheses() {
    let inlines = md_para("[c](/u(a(b)c)d)\n", &[]);
    let (_, target) = single_link(&inlines).expect("a single link");
    assert_eq!(target.url, "/u(a(b)c)d");
}

#[test]
fn markdown_single_tilde_pair_is_a_subscript() {
    let inlines = md_para("z ~x~\n", &[Extension::Subscript, Extension::Strikeout]);
    assert!(
        inlines.iter().any(|i| matches!(i, Inline::Subscript(_))),
        "expected a subscript, got {inlines:?}"
    );
}

#[test]
fn markdown_double_tilde_pair_is_a_strikeout() {
    let inlines = md_para("z ~~x~~\n", &[Extension::Subscript, Extension::Strikeout]);
    assert!(
        inlines.iter().any(|i| matches!(i, Inline::Strikeout(_))),
        "expected a strikeout, got {inlines:?}"
    );
}

#[test]
fn markdown_triple_tilde_run_collapses_to_a_single_subscript() {
    // The whole odd run is consumed into one subscript; no strikeout nests inside it.
    let inlines = md_para(
        "z ~~~triple~~~\n",
        &[Extension::Subscript, Extension::Strikeout],
    );
    let sub = inlines
        .iter()
        .find_map(|i| match i {
            Inline::Subscript(content) => Some(content.clone()),
            _ => None,
        })
        .expect("a subscript");
    assert!(
        matches!(sub.as_slice(), [Inline::Str(s)] if s == "triple"),
        "expected Subscript[triple], got {sub:?}"
    );
    assert!(
        !inlines.iter().any(|i| matches!(i, Inline::Strikeout(_))),
        "a triple-tilde run should not form a strikeout: {inlines:?}"
    );
}
