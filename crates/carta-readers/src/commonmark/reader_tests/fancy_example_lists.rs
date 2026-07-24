//! Fancy ordered-list and example-list tests.

use super::*;

#[test]
fn lowercase_letters_form_an_alphabetic_list() {
    assert_eq!(
        ordered_lists("a. one\nb. two\nc. three\n"),
        [(1, ListNumberStyle::LowerAlpha, ListNumberDelim::Period, 3)]
    );
}

#[test]
fn an_alphabetic_list_starts_at_its_first_letter() {
    assert_eq!(
        ordered_lists("c. three\nd. four\n"),
        [(3, ListNumberStyle::LowerAlpha, ListNumberDelim::Period, 2)]
    );
}

#[test]
fn a_roman_run_is_a_roman_list() {
    assert_eq!(
        ordered_lists("i. one\nii. two\niii. three\niv. four\n"),
        [(1, ListNumberStyle::LowerRoman, ListNumberDelim::Period, 4)]
    );
}

#[test]
fn a_lone_i_opens_a_roman_list() {
    assert_eq!(
        ordered_lists("i. only\n"),
        [(1, ListNumberStyle::LowerRoman, ListNumberDelim::Period, 1)]
    );
}

#[test]
fn an_alphabetic_list_absorbs_a_following_i() {
    // `i` continues the alphabetic run as the ninth letter, not a fresh roman list.
    assert_eq!(
        ordered_lists("h. eight\ni. nine\nj. ten\n"),
        [(8, ListNumberStyle::LowerAlpha, ListNumberDelim::Period, 3)]
    );
}

#[test]
fn a_multi_letter_roman_does_not_continue_an_alphabetic_list() {
    assert_eq!(
        ordered_lists("a. one\nii. two\n"),
        [
            (1, ListNumberStyle::LowerAlpha, ListNumberDelim::Period, 1),
            (2, ListNumberStyle::LowerRoman, ListNumberDelim::Period, 1),
        ]
    );
}

#[test]
fn a_lone_i_after_a_list_reads_as_the_ninth_letter() {
    // Following another list, the ambiguous `i` resolves to the alphabetic reading.
    assert_eq!(
        ordered_lists("1. one\ni. two\n"),
        [
            (1, ListNumberStyle::Decimal, ListNumberDelim::Period, 1),
            (9, ListNumberStyle::LowerAlpha, ListNumberDelim::Period, 1),
        ]
    );
}

#[test]
fn parenthesized_and_single_paren_delimiters_are_distinguished() {
    assert_eq!(
        ordered_lists("(a) one\n"),
        [(
            1,
            ListNumberStyle::LowerAlpha,
            ListNumberDelim::TwoParens,
            1
        )]
    );
    assert_eq!(
        ordered_lists("a) one\n"),
        [(1, ListNumberStyle::LowerAlpha, ListNumberDelim::OneParen, 1)]
    );
}

#[test]
fn an_uppercase_letter_and_period_need_two_spaces() {
    // One space reads as an ordinary sentence; two spaces make it a list.
    assert!(matches!(
        blocks_with("B. Franklin\n", Extension::FancyLists).as_slice(),
        [Block::Para(_)]
    ));
    assert_eq!(
        ordered_lists("B.  item\n"),
        [(2, ListNumberStyle::UpperAlpha, ListNumberDelim::Period, 1)]
    );
}

#[test]
fn an_uppercase_letter_with_one_space_is_a_list_under_other_delimiters() {
    // The two-space rule guards only the period; a paren delimiter is unambiguous.
    assert_eq!(
        ordered_lists("B) item\n"),
        [(2, ListNumberStyle::UpperAlpha, ListNumberDelim::OneParen, 1)]
    );
}

#[test]
fn only_a_decimal_one_interrupts_a_paragraph() {
    assert!(matches!(
        blocks_with("text\na. item\n", Extension::FancyLists).as_slice(),
        [Block::Para(_)]
    ));
    assert!(matches!(
        blocks_with("text\n1. item\n", Extension::FancyLists).as_slice(),
        [Block::Para(_), Block::OrderedList(..)]
    ));
    assert!(matches!(
        blocks_with("text\n(1) item\n", Extension::FancyLists).as_slice(),
        [Block::Para(_), Block::OrderedList(..)]
    ));
}

#[test]
fn with_the_extension_off_a_letter_marker_is_paragraph_text() {
    assert!(matches!(blocks("a. one\n").as_slice(), [Block::Para(_)]));
}

#[test]
fn the_three_example_markers_open_example_lists() {
    use ListNumberDelim::{OneParen, Period, TwoParens};
    use ListNumberStyle::Example;
    assert_eq!(
        example_lists("(@) one\n\n@. two\n\n@) three\n"),
        [
            (1, Example, TwoParens, 1),
            (2, Example, Period, 1),
            (3, Example, OneParen, 1),
        ]
    );
}

#[test]
fn a_reference_resolves_to_its_example_number() {
    assert_eq!(example_text("(@a) apple\n\nSee (@a).\n"), "See (1).");
}

#[test]
fn a_bare_reference_drops_the_parentheses() {
    assert_eq!(example_text("(@a) apple\n\nbare @a end\n"), "bare 1 end");
}

#[test]
fn the_counter_skips_ordinary_ordered_lists() {
    // A plain decimal list between two examples does not advance the example counter.
    assert_eq!(
        example_lists("(@a) x\n\n1. p\n2. q\n\n(@b) y\n"),
        [
            (1, ListNumberStyle::Example, ListNumberDelim::TwoParens, 1),
            (1, ListNumberStyle::Decimal, ListNumberDelim::Period, 2),
            (2, ListNumberStyle::Example, ListNumberDelim::TwoParens, 1),
        ]
    );
    assert_eq!(
        example_text("(@a) x\n\n1. p\n2. q\n\n(@b) y\n\nRefs (@a) and (@b)\n"),
        "Refs (1) and (2)"
    );
}

#[test]
fn a_repeated_label_reuses_its_number() {
    use ListNumberDelim::{OneParen, Period, TwoParens};
    use ListNumberStyle::Example;
    // A repeated label neither takes a fresh number nor advances the counter.
    assert_eq!(
        example_lists("(@a) x\n\n@a. y\n\n@b) z\n"),
        [
            (1, Example, TwoParens, 1),
            (1, Example, Period, 1),
            (2, Example, OneParen, 1),
        ]
    );
    assert_eq!(
        example_text("(@a) x\n\n@a. y\n\n@b) z\n\nRef (@a) (@b)\n"),
        "Ref (1) (2)"
    );
}

#[test]
fn an_anonymous_example_advances_the_counter() {
    // The unreferenceable `(@)` takes number one, so the following labelled example is two.
    assert_eq!(
        example_lists("(@) x\n\n@a. y\n"),
        [
            (1, ListNumberStyle::Example, ListNumberDelim::TwoParens, 1),
            (2, ListNumberStyle::Example, ListNumberDelim::Period, 1),
        ]
    );
    assert_eq!(example_text("(@) x\n\n@a. y\n\nSee (@a)\n"), "See (2)");
}

#[test]
fn an_anonymous_reference_stays_literal() {
    assert_eq!(example_text("(@) x\n\nSee (@).\n"), "See (@).");
}

#[test]
fn an_undefined_reference_stays_literal() {
    assert_eq!(example_text("(@a) x\n\nSee (@b).\n"), "See (@b).");
}

#[test]
fn a_reference_resolves_within_emphasis_but_not_within_code() {
    // Emphasis content is parsed, so the reference resolves; a code span is verbatim.
    assert_eq!(example_text("(@a) x\n\n*em (@a)*\n"), "em (1)");
    assert_eq!(example_text("(@a) x\n\n`(@a)`\n"), "(@a)");
}

#[test]
fn the_counter_spans_nested_example_lists() {
    // Reading order crosses container boundaries: the example nested in a bullet is two.
    assert_eq!(
        example_text("(@a) x\n\n- bullet\n\n    (@b) nested\n\nRefs (@a) and (@b)\n"),
        "Refs (1) and (2)"
    );
}

#[test]
fn with_the_extension_off_an_example_marker_is_paragraph_text() {
    assert!(matches!(blocks("(@) one\n").as_slice(), [Block::Para(_)]));
    assert!(matches!(blocks("@a. one\n").as_slice(), [Block::Para(_)]));
}
