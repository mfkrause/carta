//! Footnote, list-marker, fenced-div, block-quote and implicit-figure tests.

use super::*;

#[test]
fn markdown_keeps_task_and_ordinary_items_in_one_list() {
    let source = "- [ ] First\n- Ordinary\n- [x] Last\n";
    let document = read_markdown(source, &[Extension::TaskLists]);
    let [Block::BulletList(items)] = document.blocks.as_slice() else {
        panic!("expected one list, got {:?}", document.blocks);
    };
    assert_eq!(items.len(), 3);
    for (item, expected) in items.iter().zip(["☐", "Ordinary", "☒"]) {
        let [Block::Plain(inlines)] = item.as_slice() else {
            panic!("expected a tight item, got {item:?}");
        };
        assert!(matches!(inlines.first(), Some(Inline::Str(text)) if text == expected));
    }
    assert_eq!(blocks_with(source, Extension::TaskLists).len(), 3);
}

#[test]
fn footnote_reference_resolves_to_a_note_and_lifts_the_definition() {
    let inlines = para_inlines("text[^a]\n\n[^a]: body\n", Extension::Footnotes);
    let note = inlines
        .iter()
        .find_map(|inline| match inline {
            Inline::Note(blocks) => Some(blocks.clone()),
            _ => None,
        })
        .expect("a note should be present");
    assert!(matches!(note.as_slice(), [Block::Para(_)]));
}

#[test]
fn deeply_nested_containers_do_not_overflow_the_stack() {
    // Use 8 MiB to test the nesting cap without hitting the test thread stack limit.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let deep_quotes = ">".repeat(50_000);
            assert!(
                CommonmarkReader
                    .read(&deep_quotes, &ReaderOptions::default())
                    .is_ok()
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn grid_cell_inlines_honor_the_markdown_dialect() {
    let input = "+-------+\n| ^a b^ |\n+-------+\n";
    let doc = read_markdown(input, &[Extension::GridTables, Extension::Superscript]);
    let table = match doc.blocks.as_slice() {
        [Block::Table(table)] => table,
        other => panic!("expected a single table, got {other:?}"),
    };
    let cell = table
        .bodies
        .first()
        .and_then(|body| body.body.first())
        .and_then(|row| row.cells.first())
        .expect("a single body cell");
    let inlines = match cell.content.as_slice() {
        [Block::Plain(inlines)] => inlines,
        other => panic!("expected a plain cell, got {other:?}"),
    };
    assert!(
        inlines.iter().all(|i| !matches!(i, Inline::Superscript(_))),
        "grid cell should not build a superscript around an inner space: {inlines:?}"
    );
}

#[test]
fn metadata_values_honor_the_markdown_dialect() {
    use carta_ast::MetaValue;
    let input = "---\ntitle: ^a b^ `  x  `\n---\n\nbody\n";
    let doc = read_markdown(
        input,
        &[Extension::YamlMetadataBlock, Extension::Superscript],
    );
    let inlines = match doc.meta.get("title") {
        Some(MetaValue::MetaInlines(inlines)) => inlines,
        other => panic!("expected inline metadata, got {other:?}"),
    };
    assert!(
        inlines.iter().all(|i| !matches!(i, Inline::Superscript(_))),
        "metadata should not build a superscript around an inner space: {inlines:?}"
    );
    assert!(
        inlines
            .iter()
            .any(|i| matches!(i, Inline::Code(_, code) if code == "x")),
        "metadata code span should trim to `x`: {inlines:?}"
    );
}

#[test]
fn attribute_only_table_caption_carries_no_blocks() {
    let input = "| a | b |\n|---|---|\n| 1 | 2 |\n\n: {#tid}\n";
    let blocks = blocks_with_many(
        input,
        &[
            Extension::PipeTables,
            Extension::TableCaptions,
            Extension::TableAttributes,
        ],
    );
    let table = match blocks.as_slice() {
        [Block::Table(table)] => table,
        other => panic!("expected a single table, got {other:?}"),
    };
    assert!(table.caption.long.is_empty());
    assert_eq!(table.attr.id, "tid");
}

#[test]
fn undefined_footnote_reference_stays_literal() {
    let inlines = para_inlines("text[^missing]\n", Extension::Footnotes);
    assert!(inlines.iter().all(|i| !matches!(i, Inline::Note(_))));
    assert!(
        inlines
            .iter()
            .any(|i| matches!(i, Inline::Str(s) if s.contains("[^missing]")))
    );
}

#[test]
fn footnote_extension_off_produces_no_note() {
    let result = blocks("text[^a]\n\n[^a]: body\n");
    let [Block::Para(inlines)] = result.as_slice() else {
        panic!("expected a single paragraph, got {result:?}");
    };
    assert!(inlines.iter().any(|i| matches!(i, Inline::Link(..))));
    assert!(inlines.iter().all(|i| !matches!(i, Inline::Note(_))));
}

#[test]
fn footnote_definition_spans_indented_continuation_blocks() {
    let inlines = para_inlines(
        "ref[^a]\n\n[^a]: first\n\n    second\n",
        Extension::Footnotes,
    );
    let note = inlines
        .iter()
        .find_map(|inline| match inline {
            Inline::Note(blocks) => Some(blocks.clone()),
            _ => None,
        })
        .expect("a note should be present");
    assert!(matches!(note.as_slice(), [Block::Para(_), Block::Para(_)]));
}

#[test]
fn nested_footnote_reference_inside_a_definition_does_not_nest() {
    let inlines = para_inlines(
        "ref[^a]\n\n[^a]: see [^b]\n\n[^b]: inner\n",
        Extension::Footnotes,
    );
    let note = inlines
        .iter()
        .find_map(|inline| match inline {
            Inline::Note(blocks) => Some(blocks.clone()),
            _ => None,
        })
        .expect("a note should be present");
    let Some(Block::Para(body)) = note.first() else {
        panic!("note should hold a paragraph");
    };
    assert!(body.iter().all(|i| !matches!(i, Inline::Note(_))));
}

#[test]
fn footnote_labels_fold_case_and_whitespace() {
    let inlines = para_inlines("ref[^A B]\n\n[^a   b]: body\n", Extension::Footnotes);
    assert!(inlines.iter().any(|i| matches!(i, Inline::Note(_))));
}

#[test]
fn defined_footnote_reference_wins_over_a_following_inline_target() {
    let inlines = para_inlines("[^a](url)\n\n[^a]: body\n", Extension::Footnotes);
    assert!(inlines.iter().any(|i| matches!(i, Inline::Note(_))));
    assert!(
        inlines
            .iter()
            .any(|i| matches!(i, Inline::Str(s) if s.contains("(url)")))
    );
}

#[test]
fn empty_list_marker_below_an_unmatched_container_starts_a_list() {
    // The paragraph sits in the unmatched quote a level below, so the marker is not interrupting it.
    let result = blocks("> two\n- \n");
    assert!(matches!(
        result.as_slice(),
        [Block::BlockQuote(_), Block::BulletList(items)] if items.as_slice() == [Vec::new()]
    ));
}

#[test]
fn bare_marker_trailed_by_spaces_leaves_an_empty_item() {
    assert!(matches!(
        blocks("-     \n").as_slice(),
        [Block::BulletList(items)] if items.as_slice() == [Vec::new()]
    ));
}

#[test]
fn empty_list_marker_still_cannot_interrupt_a_same_level_paragraph() {
    // `*` avoids the setext-underline reading a `-` line would get.
    assert!(matches!(blocks("para\n* \n").as_slice(), [Block::Para(_)]));
}

#[test]
fn long_digit_run_is_not_an_ordered_list() {
    // Computing the start value of an over-long digit run must not overflow.
    let input = format!("{}*:*\n", "8".repeat(34));
    assert!(matches!(blocks(&input).as_slice(), [Block::Para(_)]));
}

#[test]
fn ordered_list_start_caps_at_nine_digits() {
    assert!(matches!(
        blocks("999999999. a\n").as_slice(),
        [Block::OrderedList(..)]
    ));
    assert!(matches!(
        blocks("1234567890. a\n").as_slice(),
        [Block::Para(_)]
    ));
}

#[test]
fn fenced_div_bare_word_names_a_single_class() {
    let result = blocks_with("::: warning\nbody\n:::\n", Extension::FencedDivs);
    let [Block::Div(attr, children)] = result.as_slice() else {
        panic!("expected a single div, got {result:?}");
    };
    assert!(attr.id.is_empty());
    assert_eq!(attr.classes, ["warning"]);
    assert!(attr.attributes.is_empty());
    assert!(matches!(children.as_slice(), [Block::Para(_)]));
}

#[test]
fn fenced_div_brace_spec_carries_id_classes_and_pairs() {
    let result = blocks_with("::: {#a .b .c k=v}\nbody\n:::\n", Extension::FencedDivs);
    let [Block::Div(attr, _)] = result.as_slice() else {
        panic!("expected a single div, got {result:?}");
    };
    assert_eq!(attr.id, "a");
    assert_eq!(attr.classes, ["b", "c"]);
    assert_eq!(attr.attributes, [("k".into(), "v".into())]);
}

#[test]
fn fenced_divs_nest_with_the_inner_closing_first() {
    let result = blocks_with(
        "::: outer\n::: inner\nx\n:::\ny\n:::\n",
        Extension::FencedDivs,
    );
    let [Block::Div(outer, outer_children)] = result.as_slice() else {
        panic!("expected a single outer div, got {result:?}");
    };
    assert_eq!(outer.classes, ["outer"]);
    let [Block::Div(inner, _), Block::Para(_)] = outer_children.as_slice() else {
        panic!("outer should hold an inner div then a paragraph, got {outer_children:?}");
    };
    assert_eq!(inner.classes, ["inner"]);
}

#[test]
fn a_shorter_colon_run_does_not_close_a_longer_fence() {
    let result = blocks_with(
        ":::: wide\n:::\nstill inside\n::::\n",
        Extension::FencedDivs,
    );
    let [Block::Div(attr, children)] = result.as_slice() else {
        panic!("expected a single div, got {result:?}");
    };
    assert_eq!(attr.classes, ["wide"]);
    assert!(matches!(children.as_slice(), [Block::Para(_)]));
}

#[test]
fn fenced_div_syntax_without_the_extension_stays_text() {
    let result = blocks("::: warning\nbody\n:::\n");
    assert!(result.iter().all(|b| !matches!(b, Block::Div(..))));
}

#[test]
fn blank_after_a_div_in_a_list_item_makes_the_list_loose() {
    let result = blocks_with(
        "- ::: note\n  inside\n  :::\n\n  after\n",
        Extension::FencedDivs,
    );
    let [Block::BulletList(items)] = result.as_slice() else {
        panic!("expected a single bullet list, got {result:?}");
    };
    let Some([Block::Div(..), tail]) = items.first().map(Vec::as_slice) else {
        panic!("the item should hold a div then a trailing block, got {items:?}");
    };
    assert!(
        matches!(tail, Block::Para(_)),
        "loose list should keep the trailing paragraph as Para, got {tail:?}"
    );
}

#[test]
fn blank_ending_a_nested_block_quote_makes_the_list_loose() {
    let result = blocks("- item\n  > q\n\n- item2\n");
    let [Block::BulletList(items)] = result.as_slice() else {
        panic!("expected a single bullet list, got {result:?}");
    };
    let Some([first, ..]) = items.first().map(Vec::as_slice) else {
        panic!("the first item should have content");
    };
    assert!(
        matches!(first, Block::Para(_)),
        "loose list should keep the item paragraph as Para, got {first:?}"
    );
}

#[test]
fn image_only_paragraph_becomes_a_figure_captioned_by_its_alt_text() {
    let result = blocks_with("![a gull](gull.png)\n", Extension::ImplicitFigures);
    let [Block::Figure(attr, caption, body)] = result.as_slice() else {
        panic!("expected a single figure, got {result:?}");
    };
    assert_eq!(*attr, Box::new(carta_ast::Attr::default()));
    assert!(caption.short.is_none());
    let [Block::Plain(caption_inlines)] = caption.long.as_slice() else {
        panic!("caption should be a single Plain, got {:?}", caption.long);
    };
    assert!(matches!(
        caption_inlines.as_slice(),
        [Inline::Str(a), Inline::Space, Inline::Str(b)] if a == "a" && b == "gull"
    ));
    let [Block::Plain(image_inlines)] = body.as_slice() else {
        panic!("body should be a single Plain, got {body:?}");
    };
    let [Inline::Image(_, alt, target)] = image_inlines.as_slice() else {
        panic!("body should wrap an Image, got {image_inlines:?}");
    };
    assert_eq!(*caption_inlines, *alt, "alt is duplicated into the caption");
    assert_eq!(target.url, "gull.png");
}

#[test]
fn an_empty_alt_image_stays_a_paragraph() {
    let result = blocks_with("![](spacer.png \"t\")\n", Extension::ImplicitFigures);
    let [Block::Para(inlines)] = result.as_slice() else {
        panic!("expected a paragraph, got {result:?}");
    };
    assert!(matches!(inlines.as_slice(), [Inline::Image(_, alt, _)] if alt.is_empty()));
}

#[test]
fn the_image_title_is_not_used_as_the_caption() {
    let result = blocks_with("![cap](c.png \"tooltip\")\n", Extension::ImplicitFigures);
    let [Block::Figure(_, caption, _)] = result.as_slice() else {
        panic!("expected a figure, got {result:?}");
    };
    let [Block::Plain(inlines)] = caption.long.as_slice() else {
        panic!("caption should be a single Plain, got {:?}", caption.long);
    };
    assert!(matches!(inlines.as_slice(), [Inline::Str(s)] if s == "cap"));
}

#[test]
fn an_extra_inline_or_a_wrapper_keeps_the_paragraph() {
    assert!(matches!(
        blocks_with("look at ![this](i.png)\n", Extension::ImplicitFigures).as_slice(),
        [Block::Para(_)]
    ));
    let linked = blocks_with("[![a](i.png)](u)\n", Extension::ImplicitFigures);
    let [Block::Para(inlines)] = linked.as_slice() else {
        panic!("expected a paragraph, got {linked:?}");
    };
    assert!(matches!(inlines.as_slice(), [Inline::Link(..)]));
}

#[test]
fn implicit_figures_off_keeps_the_image_paragraph() {
    assert!(matches!(
        blocks("![a gull](gull.png)\n").as_slice(),
        [Block::Para(_)]
    ));
}
