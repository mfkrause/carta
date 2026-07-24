//! Line-block and definition-list tests.

use super::*;

#[test]
fn line_block_keeps_each_marked_line_as_its_own_entry() {
    let blocks = blocks_with_many("| Line one\n| Line two\n", LINE_BLOCKS);
    assert!(matches!(blocks.as_slice(), [Block::LineBlock(_)]));
    assert_eq!(line_block_entries(&blocks), ["Line one", "Line two"]);
}

#[test]
fn line_block_preserves_leading_spaces_as_non_breaking() {
    let blocks = blocks_with_many("|   indented\n", LINE_BLOCKS);
    assert_eq!(line_block_entries(&blocks), ["\u{a0}\u{a0}indented"]);
}

#[test]
fn line_block_bar_alone_is_an_empty_entry() {
    let blocks = blocks_with_many("|\n| after\n", LINE_BLOCKS);
    assert_eq!(line_block_entries(&blocks), ["", "after"]);
}

#[test]
fn line_block_folds_an_indented_continuation_into_the_entry_above() {
    let blocks = blocks_with_many("| first part\n  second part\n", LINE_BLOCKS);
    assert_eq!(line_block_entries(&blocks), ["first part second part"]);
}

#[test]
fn line_block_collapses_internal_runs_and_drops_trailing_space() {
    let blocks = blocks_with_many("| a    b    c   \n", LINE_BLOCKS);
    assert_eq!(line_block_entries(&blocks), ["a b c"]);
}

#[test]
fn line_block_all_space_entry_collapses_to_empty() {
    let blocks = blocks_with_many("|    \n| x\n", LINE_BLOCKS);
    assert_eq!(line_block_entries(&blocks), ["", "x"]);
}

#[test]
fn a_bar_without_a_following_space_is_not_a_line_block() {
    let blocks = blocks_with_many("|nospace\n", LINE_BLOCKS);
    assert!(matches!(blocks.as_slice(), [Block::Para(_)]));
}

#[test]
fn a_line_block_does_not_interrupt_a_paragraph() {
    let blocks = blocks_with_many("ordinary text\n| still the paragraph\n", LINE_BLOCKS);
    assert!(matches!(blocks.as_slice(), [Block::Para(_)]));
    assert!(line_block_entries(&blocks).is_empty());
}

#[test]
fn a_blank_line_ends_a_line_block() {
    let blocks = blocks_with_many("| a\n\nplain\n", LINE_BLOCKS);
    assert!(matches!(
        blocks.as_slice(),
        [Block::LineBlock(_), Block::Para(_)]
    ));
}

#[test]
fn a_whitespace_only_line_continues_a_non_empty_entry() {
    // Unlike a blank line, a space-only line folds into the entry above, so the block stays open.
    let blocks = blocks_with_many("| a\n  \n| b\n", LINE_BLOCKS);
    assert!(matches!(blocks.as_slice(), [Block::LineBlock(_)]));
    assert_eq!(line_block_entries(&blocks), ["a", "b"]);
}

#[test]
fn a_continuation_under_an_empty_entry_ends_the_block() {
    // With no content to extend, a whitespace-led line closes the block and is reparsed.
    let blocks = blocks_with_many("| \n |\n", LINE_BLOCKS);
    assert!(matches!(
        blocks.as_slice(),
        [Block::LineBlock(_), Block::Para(_)]
    ));
    assert_eq!(line_block_entries(&blocks), [""]);
}

#[test]
fn a_delimiter_row_under_a_single_bar_line_makes_a_table() {
    let blocks = blocks_with_many("| a | b |\n|---|---|\n| 1 | 2 |\n", LINE_BLOCKS_TABLES);
    assert!(matches!(blocks.as_slice(), [Block::Table(_)]));
    assert!(line_block_entries(&blocks).is_empty());
}

#[test]
fn a_bar_line_with_no_delimiter_stays_a_line_block() {
    let blocks = blocks_with_many("| a | b |\nplain\n", LINE_BLOCKS_TABLES);
    assert!(matches!(
        blocks.as_slice(),
        [Block::LineBlock(_), Block::Para(_)]
    ));
}

#[test]
fn with_the_extension_off_a_bar_line_is_literal_paragraph_text() {
    let blocks = blocks("| a\n");
    let [Block::Para(inlines)] = blocks.as_slice() else {
        panic!("expected a single paragraph, got {blocks:?}");
    };
    assert!(matches!(inlines.first(), Some(Inline::Str(text)) if text == "|"));
}

#[test]
fn a_term_above_a_colon_line_becomes_one_tight_definition() {
    let items = definition_items(&blocks_with("apple\n: red\n", Extension::DefinitionLists));
    let [(term, defs)] = items.as_slice() else {
        panic!("expected one item, got {items:?}");
    };
    assert_eq!(term, "apple");
    assert!(matches!(defs.as_slice(), [one] if matches!(one.as_slice(), [Block::Plain(_)])));
}

#[test]
fn a_term_carries_several_definitions_under_colon_or_tilde_markers() {
    let items = definition_items(&blocks_with(
        "water\n: clear\n~ vital\n",
        Extension::DefinitionLists,
    ));
    let [(term, defs)] = items.as_slice() else {
        panic!("expected one item, got {items:?}");
    };
    assert_eq!(term, "water");
    assert_eq!(defs.len(), 2);
}

#[test]
fn consecutive_terms_join_one_list() {
    let items = definition_items(&blocks_with(
        "a\n: x\n\nb\n: y\n",
        Extension::DefinitionLists,
    ));
    let terms: Vec<&str> = items.iter().map(|(term, _)| term.as_str()).collect();
    assert_eq!(terms, ["a", "b"]);
}

#[test]
fn a_blank_line_before_the_marker_makes_the_definition_loose() {
    let items = definition_items(&blocks_with(
        "planet\n\n: orbits\n",
        Extension::DefinitionLists,
    ));
    let [(_, defs)] = items.as_slice() else {
        panic!("expected one item, got {items:?}");
    };
    assert!(matches!(defs.as_slice(), [one] if matches!(one.as_slice(), [Block::Para(_)])));
}

#[test]
fn an_indented_continuation_keeps_a_second_block_in_the_definition() {
    let items = definition_items(&blocks_with(
        "essay\n: first.\n\n  second.\n",
        Extension::DefinitionLists,
    ));
    let [(_, defs)] = items.as_slice() else {
        panic!("expected one item, got {items:?}");
    };
    let [blocks] = defs.as_slice() else {
        panic!("expected one definition, got {defs:?}");
    };
    assert_eq!(blocks.len(), 2);
}

#[test]
fn a_definition_holds_a_nested_block_when_indented_to_the_content_column() {
    let items = definition_items(&blocks_with(
        "shapes\n: items:\n\n    - circle\n    - square\n",
        Extension::DefinitionLists,
    ));
    let [(_, defs)] = items.as_slice() else {
        panic!("expected one item, got {items:?}");
    };
    let [blocks] = defs.as_slice() else {
        panic!("expected one definition, got {defs:?}");
    };
    assert!(matches!(
        blocks.as_slice(),
        [Block::Plain(_), Block::BulletList(_)]
    ));
}

#[test]
fn lines_above_the_marker_fold_into_one_term() {
    let items = definition_items(&blocks_with(
        "one\ntwo\n: both\n",
        Extension::DefinitionLists,
    ));
    let [(term, _)] = items.as_slice() else {
        panic!("expected one item, got {items:?}");
    };
    assert_eq!(term, "one two");
}

#[test]
fn an_unindented_line_lazily_continues_the_definition() {
    let items = definition_items(&blocks_with(
        "apple\n: red\norange\n",
        Extension::DefinitionLists,
    ));
    let [(_, defs)] = items.as_slice() else {
        panic!("expected one item, got {items:?}");
    };
    let [blocks] = defs.as_slice() else {
        panic!("expected one definition, got {defs:?}");
    };
    assert!(matches!(blocks.as_slice(), [Block::Plain(_)]));
}

#[test]
fn a_colon_without_a_following_space_is_not_a_marker() {
    let blocks = blocks_with("term\n:def\n", Extension::DefinitionLists);
    assert!(matches!(blocks.as_slice(), [Block::Para(_)]));
}

#[test]
fn an_empty_definition_yields_an_empty_block_list() {
    let blocks = blocks_with("T\n:\nmore\n", Extension::DefinitionLists);
    let items = definition_items(&blocks);
    let [(term, defs)] = items.as_slice() else {
        panic!("expected one item, got {items:?}");
    };
    assert_eq!(term, "T");
    assert!(matches!(defs.as_slice(), [one] if one.is_empty()));
    // The unindented line ends the list and stands as its own paragraph.
    assert!(matches!(
        blocks.as_slice(),
        [Block::DefinitionList(_), Block::Para(_)]
    ));
}

#[test]
fn an_empty_definition_absorbs_a_deferred_indented_block() {
    // A blank line does not close an as-yet-empty definition.
    let items = definition_items(&blocks_with(
        "T\n:\n\n    code\n",
        Extension::DefinitionLists,
    ));
    let [(_, defs)] = items.as_slice() else {
        panic!("expected one item, got {items:?}");
    };
    assert!(matches!(defs.as_slice(), [one] if matches!(one.as_slice(), [Block::Plain(_)])));
}

#[test]
fn with_the_extension_off_a_colon_line_is_literal_paragraph_text() {
    let blocks = blocks("apple\n: red\n");
    assert!(matches!(blocks.as_slice(), [Block::Para(_)]));
    assert!(definition_items(&blocks).is_empty());
}
