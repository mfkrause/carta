//! Greedy-paragraph (markdown dialect) tests.

use super::*;

#[test]
fn a_greedy_paragraph_folds_a_following_block_quote_heading_and_break() {
    // Quote and heading folds are gated on the `blank_before_*` toggles; the break folds on greedy alone.
    let toggles = &[
        Extension::BlankBeforeBlockquote,
        Extension::BlankBeforeHeader,
    ];
    for line in ["> quote", "# heading", "***"] {
        let input = format!("text\n{line}\n");
        assert!(
            matches!(greedy_blocks(&input, toggles).as_slice(), [Block::Para(_)]),
            "expected one paragraph for {input:?}"
        );
    }
}

#[test]
fn a_heading_or_block_quote_interrupts_without_its_blank_before_toggle() {
    // Without its toggle the opener interrupts as in strict CommonMark, greediness aside.
    assert!(matches!(
        greedy_blocks("text\n# heading\n", &[]).as_slice(),
        [Block::Para(_), Block::Header(_, _, _)]
    ));
    assert!(matches!(
        greedy_blocks("text\n> quote\n", &[]).as_slice(),
        [Block::Para(_), Block::BlockQuote(_)]
    ));
    // The thematic break is not toggle-gated, so it still folds into the greedy paragraph.
    assert!(matches!(
        greedy_blocks("text\n***\n", &[]).as_slice(),
        [Block::Para(_)]
    ));
}

#[test]
fn a_greedy_paragraph_is_not_interrupted_by_a_list_marker() {
    // At the top level a fresh list cannot interrupt a paragraph; the marker reads as text.
    assert!(matches!(
        greedy_blocks("text\n- item\n", &[]).as_slice(),
        [Block::Para(_)]
    ));
}

#[test]
fn lists_without_preceding_blankline_lets_a_fresh_list_interrupt_a_paragraph() {
    let ext = &[Extension::ListsWithoutPrecedingBlankline];
    assert!(matches!(
        greedy_blocks("text\n- item\n", ext).as_slice(),
        [Block::Para(_), Block::BulletList(_)]
    ));
    assert!(matches!(
        greedy_blocks("text\n2. item\n", ext).as_slice(),
        [Block::Para(_), Block::OrderedList(_, _)]
    ));
}

#[test]
fn a_list_shaped_line_ends_a_paragraph_even_when_no_list_opens() {
    // With no enabled enumerator style, the line still ends the paragraph and starts a fresh one.
    let ext = &[Extension::ListsWithoutPrecedingBlankline];
    for line in ["(5) item", "ii. item", "a) item"] {
        let input = format!("text\n{line}\n");
        assert!(
            matches!(
                greedy_blocks(&input, ext).as_slice(),
                [Block::Para(_), Block::Para(_)]
            ),
            "expected two paragraphs for {input:?}"
        );
    }
    // With `space_in_atx_header` off, a glued hash run opens a heading.
    assert!(
        matches!(
            greedy_blocks("text\n#) item\n", ext).as_slice(),
            [Block::Para(_), Block::Header(1, _, _)]
        ),
        "expected a paragraph then a heading for a glued hash marker"
    );
}

#[test]
fn definition_and_example_markers_end_a_greedy_paragraph() {
    // Each marker ends the greedy paragraph even though its own list extension is off and no list opens.
    let ext = &[Extension::ListsWithoutPrecedingBlankline];
    for line in [": def", "~ def", "(@) item", "(@label) item"] {
        let input = format!("text\n{line}\n");
        assert!(
            matches!(
                greedy_blocks(&input, ext).as_slice(),
                [Block::Para(_), Block::Para(_)]
            ),
            "expected two paragraphs for {input:?}"
        );
    }
}

#[test]
fn a_definition_marker_opens_a_list_when_definition_lists_are_on() {
    // With definition lists on the marker turns the paragraph into a term instead of splitting it.
    let ext = &[
        Extension::ListsWithoutPrecedingBlankline,
        Extension::DefinitionLists,
    ];
    assert!(matches!(
        greedy_blocks("text\n: def\n", ext).as_slice(),
        [Block::DefinitionList(_)]
    ));
}

#[test]
fn a_decimal_marker_closed_by_one_paren_stays_prose() {
    // `2)` is too easily ordinary prose, so it neither opens a list nor ends the paragraph.
    let ext = &[Extension::ListsWithoutPrecedingBlankline];
    assert!(matches!(
        greedy_blocks("text\n2) still prose\n", ext).as_slice(),
        [Block::Para(_)]
    ));
}

#[test]
fn a_greedy_paragraph_folds_a_fenced_div_and_footnote_definition() {
    assert!(matches!(
        greedy_blocks("text\n::: note\nx\n:::\n", &[Extension::FencedDivs]).as_slice(),
        [Block::Para(_)]
    ));
    assert!(matches!(
        greedy_blocks("text\n[^1]: a note\n", &[Extension::Footnotes]).as_slice(),
        [Block::Para(_)]
    ));
}

#[test]
fn a_definition_marker_ends_an_open_footnote_definition() {
    // A marker continuing a definition's own body ends it and opens the next, so consecutive
    // definitions stay separate rather than being swallowed.
    let blocks = greedy_blocks(
        "x[^1] y[^2]\n\n[^1]: one\n[^2]: two\n",
        &[Extension::Footnotes],
    );
    let notes: Vec<_> = blocks
        .iter()
        .flat_map(|block| match block {
            Block::Para(inlines) => inlines.clone(),
            _ => Vec::new(),
        })
        .filter(|inline| matches!(inline, Inline::Note(_)))
        .collect();
    assert_eq!(notes.len(), 2, "each definition resolves to its own note");
    for note in &notes {
        let Inline::Note(body) = note else { continue };
        let Some(Block::Para(para)) = body.first() else {
            panic!("a note holds a single-line paragraph");
        };
        assert_eq!(para.len(), 1, "no following definition is swallowed in");
    }
}

#[test]
fn a_closed_fenced_code_block_ends_a_greedy_paragraph() {
    // A fence ends a greedy paragraph only when its character is enabled and it is closed.
    assert!(matches!(
        greedy_blocks("text\n```\ncode\n```\n", &[Extension::BacktickCodeBlocks]).as_slice(),
        [Block::Para(_), Block::CodeBlock(_, _)]
    ));
}

#[test]
fn a_fence_without_its_character_enabled_folds_into_the_paragraph() {
    // The disabled fence stays paragraph text; the backtick runs read as an inline code span.
    assert!(matches!(
        greedy_blocks("text\n```\ncode\n```\n", &[]).as_slice(),
        [Block::Para(_)]
    ));
}

#[test]
fn an_unclosed_fence_folds_into_the_paragraph() {
    // An unclosed fence would run to the container's end, so its lines stay paragraph text.
    assert!(matches!(
        greedy_blocks("text\n```\ncode\n", &[Extension::BacktickCodeBlocks]).as_slice(),
        [Block::Para(_)]
    ));
}

#[test]
fn a_blank_line_lets_a_block_open_after_a_greedy_paragraph() {
    assert!(matches!(
        greedy_blocks("text\n\n# heading\n", &[]).as_slice(),
        [Block::Para(_), Block::Header(_, _, _)]
    ));
    assert!(matches!(
        greedy_blocks("text\n\n- item\n", &[]).as_slice(),
        [Block::Para(_), Block::BulletList(_)]
    ));
}

#[test]
fn sibling_list_items_are_not_folded_into_each_other() {
    // Greediness suppresses only a fresh list interrupting a paragraph, not continuation markers.
    let blocks = greedy_blocks("- a\n- b\n", &[]);
    let [Block::BulletList(items)] = blocks.as_slice() else {
        panic!("expected a bullet list");
    };
    assert_eq!(items.len(), 2);
}

#[test]
fn a_sublist_opens_under_an_item_regardless_of_its_start_number() {
    // An indented ordered marker opens a sublist even when it does not start at one.
    let blocks = greedy_blocks("1. a\n   3. b\n", &[Extension::FancyLists]);
    let [Block::OrderedList(_, items)] = blocks.as_slice() else {
        panic!("expected an ordered list");
    };
    let [first] = items.as_slice() else {
        panic!("expected one outer item");
    };
    assert!(
        first
            .iter()
            .any(|block| matches!(block, Block::OrderedList(_, _))),
        "the item should contain a nested ordered list"
    );
}
