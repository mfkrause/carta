use std::fmt::Write as _;

use super::{MAX_NESTING_DEPTH, Scalar, TopLevel, Yaml, canonicalize_number, parse};

fn map(content: &str) -> Vec<(String, Yaml)> {
    match parse(content) {
        Ok(TopLevel::Mapping(entries)) => entries,
        other => panic!("expected a mapping, got {other:?}"),
    }
}

fn plain(content: &str) -> String {
    match map(content).into_iter().next() {
        Some((_, Yaml::Scalar(Scalar::Plain(text)))) => text,
        other => panic!("expected one plain entry, got {other:?}"),
    }
}

#[test]
fn a_simple_mapping_keeps_insertion_order_and_plain_values() {
    assert_eq!(
        map("a: 1\nb: two\n"),
        vec![
            ("a".to_owned(), Yaml::Scalar(Scalar::Plain("1".to_owned()))),
            (
                "b".to_owned(),
                Yaml::Scalar(Scalar::Plain("two".to_owned()))
            ),
        ]
    );
}

#[test]
fn an_empty_value_is_an_empty_plain_scalar() {
    assert_eq!(plain("k:\n"), "");
    assert_eq!(plain("k: # only a comment\n"), "");
}

#[test]
fn a_trailing_comment_is_dropped_from_a_plain_scalar() {
    assert_eq!(plain("k: value # note\n"), "value");
}

#[test]
fn empty_content_is_an_empty_mapping() {
    assert_eq!(map(""), Vec::new());
    assert_eq!(map("# just a comment\n"), Vec::new());
}

#[test]
fn a_top_level_sequence_or_scalar_is_not_a_mapping() {
    assert_eq!(parse("- a\n- b\n"), Ok(TopLevel::NotMapping));
    assert_eq!(parse("foo\n"), Ok(TopLevel::NotMapping));
}

#[test]
fn a_block_sequence_value_may_sit_at_the_key_column() {
    let entries = map("tags:\n- x\n- y\n");
    let [(key, Yaml::Sequence(items))] = entries.as_slice() else {
        panic!("expected one sequence entry, got {entries:?}");
    };
    assert_eq!(key, "tags");
    assert_eq!(items.len(), 2);
}

#[test]
fn a_nested_mapping_is_parsed_by_indentation() {
    let entries = map("m:\n  k: v\n");
    let [(_, Yaml::Mapping(inner))] = entries.as_slice() else {
        panic!("expected a nested mapping, got {entries:?}");
    };
    assert_eq!(inner.len(), 1);
    assert_eq!(inner.first().map(|(k, _)| k.as_str()), Some("k"));
}

#[test]
fn a_flow_sequence_parses_its_elements() {
    let entries = map("k: [x, y]\n");
    let [(_, Yaml::Sequence(items))] = entries.as_slice() else {
        panic!("expected a flow sequence, got {entries:?}");
    };
    assert_eq!(
        items,
        &[
            Yaml::Scalar(Scalar::Plain("x".to_owned())),
            Yaml::Scalar(Scalar::Plain("y".to_owned())),
        ]
    );
}

#[test]
fn an_unclosed_flow_sequence_is_malformed() {
    assert_eq!(parse("k: [unclosed\n"), Err(()));
}

#[test]
fn quotes_force_a_string_and_unescape() {
    let entries = map("a: \"x\\ty\"\nb: 'it''s'\n");
    assert_eq!(
        entries,
        vec![
            (
                "a".to_owned(),
                Yaml::Scalar(Scalar::Quoted("x\ty".to_owned()))
            ),
            (
                "b".to_owned(),
                Yaml::Scalar(Scalar::Quoted("it's".to_owned()))
            ),
        ]
    );
}

#[test]
fn a_literal_block_scalar_keeps_newlines_and_a_clip_newline() {
    let entries = map("k: |\n  a\n  b\n");
    let [(_, Yaml::Scalar(Scalar::Block(text)))] = entries.as_slice() else {
        panic!("expected a block scalar, got {entries:?}");
    };
    assert_eq!(text, "a\nb\n");
}

#[test]
fn a_strip_block_scalar_drops_the_trailing_newline() {
    let entries = map("k: |-\n  a\n  b\n");
    let [(_, Yaml::Scalar(Scalar::Block(text)))] = entries.as_slice() else {
        panic!("expected a block scalar, got {entries:?}");
    };
    assert_eq!(text, "a\nb");
}

#[test]
fn a_folded_block_scalar_joins_lines_with_spaces() {
    let entries = map("k: >\n  a\n  b\n");
    let [(_, Yaml::Scalar(Scalar::Block(text)))] = entries.as_slice() else {
        panic!("expected a block scalar, got {entries:?}");
    };
    assert_eq!(text, "a b\n");
}

#[test]
fn a_multi_line_plain_scalar_folds_with_spaces() {
    assert_eq!(plain("k: one\n  two\n"), "one two");
}

#[test]
fn integers_canonicalize_across_radixes() {
    assert_eq!(canonicalize_number("007").as_deref(), Some("7"));
    assert_eq!(canonicalize_number("010").as_deref(), Some("10"));
    assert_eq!(canonicalize_number("0o10").as_deref(), Some("8"));
    assert_eq!(canonicalize_number("0x10").as_deref(), Some("16"));
    assert_eq!(canonicalize_number("0xFF").as_deref(), Some("255"));
    assert_eq!(canonicalize_number("+7").as_deref(), Some("7"));
    assert_eq!(canonicalize_number("-0").as_deref(), Some("0"));
    assert_eq!(canonicalize_number("-7").as_deref(), Some("-7"));
}

#[test]
fn whole_numbers_past_the_64_bit_range_render_in_scientific_form() {
    // The largest magnitudes the signed 64-bit range holds stay plain integers.
    assert_eq!(
        canonicalize_number("9223372036854775807").as_deref(),
        Some("9223372036854775807")
    );
    assert_eq!(
        canonicalize_number("-9223372036854775808").as_deref(),
        Some("-9223372036854775808")
    );
    // One step past either bound switches to scientific notation, full precision preserved.
    assert_eq!(
        canonicalize_number("9223372036854775808").as_deref(),
        Some("9.223372036854775808e18")
    );
    assert_eq!(
        canonicalize_number("-9223372036854775809").as_deref(),
        Some("-9.223372036854775809e18")
    );
    assert_eq!(
        canonicalize_number("100000000000000000000").as_deref(),
        Some("1.0e20")
    );
    // The same threshold applies to hexadecimal and octal whole numbers.
    assert_eq!(
        canonicalize_number("0x8000000000000000").as_deref(),
        Some("9.223372036854775808e18")
    );
    // A signed radix token is not a number at all; it stays a verbatim string.
    assert_eq!(canonicalize_number("-0xF"), None);
    assert_eq!(canonicalize_number("+0o17"), None);
}

#[test]
fn floats_canonicalize_to_fixed_or_scientific() {
    assert_eq!(canonicalize_number("1e3").as_deref(), Some("1000"));
    assert_eq!(canonicalize_number("1.5e3").as_deref(), Some("1500"));
    assert_eq!(canonicalize_number("3.14").as_deref(), Some("3.14"));
    assert_eq!(canonicalize_number("1.0").as_deref(), Some("1"));
    assert_eq!(canonicalize_number("12.340").as_deref(), Some("12.34"));
    assert_eq!(canonicalize_number("100.00").as_deref(), Some("100"));
    assert_eq!(canonicalize_number("0.0").as_deref(), Some("0"));
    assert_eq!(
        canonicalize_number("1e18").as_deref(),
        Some("1000000000000000000")
    );
    assert_eq!(canonicalize_number("1e19").as_deref(), Some("1.0e19"));
    assert_eq!(canonicalize_number("6.022e23").as_deref(), Some("6.022e23"));
    assert_eq!(canonicalize_number("0.09").as_deref(), Some("9.0e-2"));
    assert_eq!(canonicalize_number("0.1").as_deref(), Some("0.1"));
    // A fractional value stays plain up to a leading digit at 10^6, then turns scientific.
    assert_eq!(
        canonicalize_number("1234567.5").as_deref(),
        Some("1234567.5")
    );
    assert_eq!(
        canonicalize_number("12345678.5").as_deref(),
        Some("1.23456785e7")
    );
    // Below 10^-1 it likewise turns scientific.
    assert_eq!(canonicalize_number("0.9").as_deref(), Some("0.9"));
    assert_eq!(canonicalize_number("0.05").as_deref(), Some("5.0e-2"));
    // An integral float that fits stays a plain integer, however it is written.
    assert_eq!(canonicalize_number("2.5e8").as_deref(), Some("250000000"));
    assert_eq!(canonicalize_number("1.5e19").as_deref(), Some("1.5e19"));
    // Scientific notation keeps every significant digit rather than rounding to a float.
    assert_eq!(
        canonicalize_number("1234567890123456.5").as_deref(),
        Some("1.2345678901234565e15")
    );
}

#[test]
fn nesting_past_the_limit_is_rejected_rather_than_overflowing() {
    let depth = MAX_NESTING_DEPTH + 50;
    // Flow collections nest within bounded input, the recursion the depth guard protects.
    let flow = format!("k: {}{}", "[".repeat(depth), "]".repeat(depth));
    assert_eq!(parse(&flow), Err(()));
    // The same guard covers block-mapping nesting.
    let mut block = String::new();
    for i in 0..depth {
        block.push_str(&" ".repeat(i));
        let _ = writeln!(block, "k{i}:");
    }
    assert_eq!(parse(&block), Err(()));
}

#[test]
fn non_numbers_stay_verbatim_strings() {
    assert_eq!(canonicalize_number(".5"), None);
    assert_eq!(canonicalize_number("1_000"), None);
    assert_eq!(canonicalize_number("0b101"), None);
    assert_eq!(canonicalize_number("07:30"), None);
    assert_eq!(canonicalize_number("v1"), None);
    assert_eq!(canonicalize_number(""), None);
}
