use super::{IrBlock, parse};
use carta_core::{Extension, Extensions, presets};

/// Parse with a given extension set in the Markdown dialect (greedy paragraphs).
fn markdown_with(input: &str, extensions: Extensions) -> Vec<IrBlock> {
    parse(input, extensions, true).0
}

/// Parse with a given extension set in the `CommonMark` family (non-greedy paragraphs).
fn strict_with(input: &str, extensions: Extensions) -> Vec<IrBlock> {
    parse(input, extensions, false).0
}

fn ordered_start(blocks: &[IrBlock]) -> Option<i32> {
    match blocks {
        [IrBlock::OrderedList(attrs, _)] => Some(attrs.start),
        _ => None,
    }
}

fn heading_level(blocks: &[IrBlock]) -> Option<i32> {
    match blocks {
        [IrBlock::Heading(level, _)] => Some(*level),
        _ => None,
    }
}

#[test]
fn markdown_honors_start_number_when_startnum_enabled() {
    // The default Markdown preset enables `startnum`, so the list begins at its written number.
    assert!(presets::MARKDOWN.contains(Extension::Startnum));
    let blocks = markdown_with("3. a\n4. b\n", presets::MARKDOWN);
    assert_eq!(ordered_start(&blocks), Some(3));
}

#[test]
fn markdown_forces_start_to_one_when_startnum_disabled() {
    let extensions = {
        let mut set = presets::MARKDOWN;
        set.remove(Extension::Startnum);
        set
    };
    assert!(!extensions.contains(Extension::Startnum));
    let blocks = markdown_with("3. a\n4. b\n", extensions);
    assert_eq!(ordered_start(&blocks), Some(1));
}

#[test]
fn commonmark_always_honors_start_number() {
    // CommonMark has no `startnum` extension; the written number is always kept.
    let blocks = strict_with("3. a\n4. b\n", presets::COMMONMARK);
    assert_eq!(ordered_start(&blocks), Some(3));
    let gfm = strict_with("3. a\n4. b\n", presets::GFM);
    assert_eq!(ordered_start(&gfm), Some(3));
}

#[test]
fn markdown_setext_underline_needs_a_single_line_paragraph() {
    // A single line above the underline forms a heading in the markdown dialect.
    let one = markdown_with("one line\n===\n", presets::MARKDOWN);
    assert_eq!(heading_level(&one), Some(1));
    // Two or more lines: no heading; the `===` line stays in the paragraph.
    let many = markdown_with("line one\nline two\n===\n", presets::MARKDOWN);
    assert!(matches!(many.as_slice(), [IrBlock::Para(text)] if text.contains("===")));
    // A leading reference definition does not count toward the line budget.
    let refd = markdown_with("[x]: /u\ncontent\n===\n", presets::MARKDOWN);
    assert_eq!(heading_level(&refd), Some(1));
    // The CommonMark family heads a multi-line paragraph, per its setext rule.
    let cm = strict_with("line one\nline two\n===\n", presets::COMMONMARK);
    assert_eq!(heading_level(&cm), Some(1));
}

#[test]
fn markdown_reads_seven_hashes_as_level_seven() {
    let blocks = markdown_with("####### h\n", presets::MARKDOWN);
    assert_eq!(heading_level(&blocks), Some(7));
}

#[test]
fn markdown_reads_eight_hashes_as_level_eight() {
    let blocks = markdown_with("######## h\n", presets::MARKDOWN);
    assert_eq!(heading_level(&blocks), Some(8));
}

#[test]
fn markdown_does_not_cap_deep_heading_levels() {
    let blocks = markdown_with("############## deep\n", presets::MARKDOWN);
    assert_eq!(heading_level(&blocks), Some(14));
}

#[test]
fn commonmark_reads_seven_hashes_as_a_paragraph() {
    let blocks = strict_with("####### h\n", presets::COMMONMARK);
    assert!(matches!(blocks.as_slice(), [IrBlock::Para(_)]));
    let gfm = strict_with("####### h\n", presets::GFM);
    assert!(matches!(gfm.as_slice(), [IrBlock::Para(_)]));
}

#[test]
fn deep_heading_still_requires_a_space_after_the_hashes() {
    // Seven hashes glued to content is not a heading in either dialect.
    let blocks = markdown_with("#######nospace\n", presets::MARKDOWN);
    assert!(matches!(blocks.as_slice(), [IrBlock::Para(_)]));
}

#[test]
fn classic_dialect_reads_a_hash_run_glued_to_text_as_a_heading() {
    // With space_in_atx_header off, a hash run needs no following space.
    for input in ["#heading\n", "##heading\n", "###heading\n"] {
        let blocks = markdown_with(input, presets::MARKDOWN_STRICT_READ);
        assert_eq!(
            heading_level(&blocks),
            i32::try_from(input.bytes().take_while(|&b| b == b'#').count()).ok(),
            "expected heading for {input:?}, got {blocks:?}"
        );
    }
}

#[test]
fn classic_dialect_strips_a_glued_closing_hash_run() {
    // space_in_atx_header off: a trailing hash run terminates even glued; interior hashes stay.
    let cases = [
        ("#foo#\n", "foo"),
        ("#foo ###\n", "foo"),
        ("#foo#bar#\n", "foo#bar"),
    ];
    for (input, want) in cases {
        let blocks = markdown_with(input, presets::MARKDOWN_STRICT_READ);
        match blocks.as_slice() {
            [IrBlock::Heading(1, text)] => assert_eq!(text, want, "for {input:?}"),
            other => panic!("expected level-1 heading for {input:?}, got {other:?}"),
        }
    }
}

#[test]
fn extended_dialect_requires_a_space_after_the_hash_run() {
    // space_in_atx_header is on in the extended dialect: a glued run is a paragraph.
    let blocks = markdown_with("#heading\n", presets::MARKDOWN);
    assert!(matches!(blocks.as_slice(), [IrBlock::Para(_)]));
}

#[test]
fn commonmark_requires_a_space_after_the_hash_run() {
    let blocks = strict_with("#heading\n", presets::COMMONMARK);
    assert!(matches!(blocks.as_slice(), [IrBlock::Para(_)]));
}

#[test]
fn markdown_rejects_an_indented_atx_heading() {
    // The Markdown dialect requires the hash run to start at the left margin.
    for input in ["  # h\n", "   ###### h\n", "   ####### h\n"] {
        let blocks = markdown_with(input, presets::MARKDOWN);
        assert!(
            matches!(blocks.as_slice(), [IrBlock::Para(_)]),
            "expected paragraph for {input:?}, got {blocks:?}"
        );
    }
}

#[test]
fn commonmark_allows_up_to_three_spaces_before_an_atx_heading() {
    let blocks = strict_with("   ###### h\n", presets::COMMONMARK);
    assert_eq!(heading_level(&blocks), Some(6));
}

use carta_ast::{ListNumberDelim, ListNumberStyle};

/// The (start, style, delim) of a single ordered list, or `None` for anything else.
fn ordered_attrs(blocks: &[IrBlock]) -> Option<(i32, ListNumberStyle, ListNumberDelim)> {
    match blocks {
        [IrBlock::OrderedList(attrs, _)] => Some((attrs.start, attrs.style, attrs.delim)),
        _ => None,
    }
}

fn list_item_count(blocks: &[IrBlock]) -> usize {
    match blocks {
        [IrBlock::OrderedList(_, items)] => items.len(),
        _ => 0,
    }
}

#[test]
fn markdown_reads_a_multi_letter_roman_list() {
    // `II.`/`III.` are unambiguously roman: the list is UpperRoman starting at two.
    let blocks = markdown_with("II. two\nIII. three\n", presets::MARKDOWN);
    assert_eq!(
        ordered_attrs(&blocks),
        Some((2, ListNumberStyle::UpperRoman, ListNumberDelim::Period))
    );
    assert_eq!(list_item_count(&blocks), 2);
}

#[test]
fn markdown_reads_a_lowercase_roman_paren_list() {
    let blocks = markdown_with("ii) a\niii) b\n", presets::MARKDOWN);
    assert_eq!(
        ordered_attrs(&blocks),
        Some((2, ListNumberStyle::LowerRoman, ListNumberDelim::OneParen))
    );
}

#[test]
fn markdown_computes_the_start_ordinal_from_the_roman_value() {
    // `IV` is four; the list begins there.
    let blocks = markdown_with("IV. a\nV. b\n", presets::MARKDOWN);
    assert_eq!(
        ordered_attrs(&blocks),
        Some((4, ListNumberStyle::UpperRoman, ListNumberDelim::Period))
    );
}

#[test]
fn markdown_reads_a_two_place_roman_numeral() {
    // `XII` is twelve: the tens and ones places combine.
    let blocks = markdown_with("XII. a\nXIII. b\n", presets::MARKDOWN);
    assert_eq!(
        ordered_attrs(&blocks),
        Some((12, ListNumberStyle::UpperRoman, ListNumberDelim::Period))
    );
}

#[test]
fn markdown_reads_a_thousands_roman_numeral() {
    let blocks = markdown_with("MII. a\nMIII. b\n", presets::MARKDOWN);
    assert_eq!(
        ordered_attrs(&blocks),
        Some((1002, ListNumberStyle::UpperRoman, ListNumberDelim::Period))
    );
}

#[test]
fn markdown_keeps_a_lone_capital_letter_marker_as_a_paragraph() {
    // One uppercase letter, period, one space is ambiguous with an initial: stays a paragraph.
    let blocks = markdown_with("I. only\n", presets::MARKDOWN);
    assert!(
        matches!(blocks.as_slice(), [IrBlock::Para(_)]),
        "expected a paragraph, got {blocks:?}"
    );
}

#[test]
fn markdown_reads_a_lone_capital_letter_marker_with_two_spaces_as_a_list() {
    // Two spaces after the marker disambiguate from an initial: `I` opens a roman list at one.
    let blocks = markdown_with("I.  only\n", presets::MARKDOWN);
    assert_eq!(
        ordered_attrs(&blocks),
        Some((1, ListNumberStyle::UpperRoman, ListNumberDelim::Period))
    );
}

#[test]
fn markdown_reads_a_hash_period_list() {
    let blocks = markdown_with("#. one\n#. two\n", presets::MARKDOWN);
    assert_eq!(
        ordered_attrs(&blocks),
        Some((
            1,
            ListNumberStyle::DefaultStyle,
            ListNumberDelim::DefaultDelim
        ))
    );
    assert_eq!(list_item_count(&blocks), 2);
}

#[test]
fn markdown_reads_a_hash_paren_list() {
    let blocks = markdown_with("#) one\n#) two\n", presets::MARKDOWN);
    assert_eq!(
        ordered_attrs(&blocks),
        Some((1, ListNumberStyle::DefaultStyle, ListNumberDelim::OneParen))
    );
}

#[test]
fn commonmark_does_not_read_a_hash_marker_as_a_list() {
    // The fancy hash marker is a Markdown-dialect feature; CommonMark keeps it literal.
    let blocks = strict_with("#. one\n#. two\n", presets::COMMONMARK);
    assert!(
        ordered_attrs(&blocks).is_none(),
        "CommonMark should not form a list from `#.`, got {blocks:?}"
    );
}
