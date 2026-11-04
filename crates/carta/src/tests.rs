#![allow(clippy::indexing_slicing)]

use super::{
    Cli, cli_command, parse_metadata_into, parse_tab_stop, parse_toc_depth, parse_variables,
    parse_wrap, template_dir,
};
use carta::WrapMode;
use carta::ast::MetaValue;
use clap::Parser;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn vars(args: &[&str]) -> Vec<(String, String)> {
    parse_variables(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
}

#[test]
fn bare_variable_defaults_to_true() {
    assert_eq!(
        vars(&["flag", "k=v", "eq=a=b"]),
        vec![
            ("flag".to_owned(), "true".to_owned()),
            ("k".to_owned(), "v".to_owned()),
            ("eq".to_owned(), "a=b".to_owned()),
        ]
    );
}

#[test]
fn variable_splits_on_the_first_colon_or_equals() {
    assert_eq!(
        vars(&["k:v", "colon:a=b", "equals=a:b"]),
        vec![
            ("k".to_owned(), "v".to_owned()),
            ("colon".to_owned(), "a=b".to_owned()),
            ("equals".to_owned(), "a:b".to_owned()),
        ]
    );
}

#[test]
fn metadata_splits_on_the_first_colon_or_equals() {
    let mut map = BTreeMap::new();
    parse_metadata_into(
        &mut map,
        &["a:val", "b:true", "c:x=y"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>(),
    );
    assert_eq!(map["a"], MetaValue::MetaString("val".into()));
    assert_eq!(map["b"], MetaValue::MetaBool(true));
    assert_eq!(map["c"], MetaValue::MetaString("x=y".into()));
}

#[test]
fn metadata_typing_distinguishes_booleans_from_strings() {
    let mut map = BTreeMap::new();
    parse_metadata_into(
        &mut map,
        &["a=true", "b=false", "c=text", "d", "e=True"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>(),
    );
    assert_eq!(map["a"], MetaValue::MetaBool(true));
    assert_eq!(map["b"], MetaValue::MetaBool(false));
    assert_eq!(map["c"], MetaValue::MetaString("text".into()));
    assert_eq!(map["d"], MetaValue::MetaBool(true));
    assert_eq!(map["e"], MetaValue::MetaString("True".into()));
}

#[test]
fn repeated_metadata_key_accumulates_into_a_list() {
    let mut two = BTreeMap::new();
    parse_metadata_into(&mut two, &["k=first".to_owned()]);
    parse_metadata_into(&mut two, &["k=second".to_owned()]);
    assert_eq!(
        two["k"],
        MetaValue::MetaList(vec![
            MetaValue::MetaString("first".into()),
            MetaValue::MetaString("second".into()),
        ])
    );
    let mut mixed = BTreeMap::new();
    parse_metadata_into(&mut mixed, &["k".to_owned()]);
    parse_metadata_into(&mut mixed, &["k=a".to_owned(), "k=b".to_owned()]);
    assert_eq!(
        mixed["k"],
        MetaValue::MetaList(vec![
            MetaValue::MetaBool(true),
            MetaValue::MetaString("a".into()),
            MetaValue::MetaString("b".into()),
        ])
    );
}

#[test]
fn template_dir_is_the_file_parent_or_current_dir() {
    assert_eq!(template_dir(Path::new("bare.html")), PathBuf::from("."));
    assert_eq!(
        template_dir(Path::new("sub/dir/t.html")),
        PathBuf::from("sub/dir")
    );
    assert_eq!(
        template_dir(Path::new("/abs/t.html")),
        PathBuf::from("/abs")
    );
}

#[test]
fn cli_definition_is_valid() {
    cli_command().debug_assert();
}

#[test]
fn wrap_mode_parses_the_three_names_and_rejects_others() {
    assert_eq!(parse_wrap("auto"), Ok(WrapMode::Auto));
    assert_eq!(parse_wrap("none"), Ok(WrapMode::None));
    assert_eq!(parse_wrap("preserve"), Ok(WrapMode::Preserve));
    assert!(parse_wrap("soft").is_err());
}

#[test]
fn toc_depth_accepts_one_through_six_and_rejects_the_rest() {
    assert_eq!(parse_toc_depth("1"), Ok(1));
    assert_eq!(parse_toc_depth("6"), Ok(6));
    assert!(parse_toc_depth("0").is_err());
    assert!(parse_toc_depth("7").is_err());
    assert!(parse_toc_depth("two").is_err());
}

#[test]
fn tab_stop_accepts_pandoc_integer_syntax() {
    for (value, expected) in [
        ("4", 4),
        ("04", 4),
        ("0x4", 4),
        ("0X4", 4),
        ("0o10", 8),
        ("0O10", 8),
        ("(4)", 4),
        ("(( 4 ))", 4),
        (" \t4\n", 4),
        ("\u{a0}4\u{2003}", 4),
    ] {
        assert_eq!(parse_tab_stop(value), Ok(expected), "{value:?}");
    }
    assert_eq!(
        parse_tab_stop(&format!("0x{:x}", isize::MAX)),
        Ok(isize::MAX as usize)
    );
    for value in [
        "18446744073709551620",
        "0x10000000000000004",
        "0o2000000000000000000004",
        "36893488147419103236",
        "340282366920938463463374607431768211460",
    ] {
        assert_eq!(parse_tab_stop(value), Ok(4), "{value}");
    }
    for value in [
        "-18446744073709551612",
        "(-18446744073709551612)",
        "-0xfffffffffffffffc",
        "- 18446744073709551612",
        "(- 18446744073709551612)",
        "(\t- \t18446744073709551612\t)",
    ] {
        assert_eq!(parse_tab_stop(value), Ok(4), "{value:?}");
    }
}

#[test]
fn tab_stop_rejects_nonpositive_and_out_of_range_values() {
    for value in [
        "+4",
        "-4",
        "-18446744073709551616",
        "-18446744073709551620",
        "0",
        "0x0",
        "0o0",
        "0b100",
        "0x+4",
        "-0x+4",
        "18446744073709551616",
        "18446744073709551615",
        "0xffffffffffffffff",
        "1_000",
        "4.0",
        "\u{85}4\u{85}",
        "\u{2028}4\u{2028}",
        "\u{2029}4\u{2029}",
    ] {
        assert!(parse_tab_stop(value).is_err(), "{value:?}");
    }
    assert!(parse_tab_stop(&(isize::MAX as u128 + 1).to_string()).is_err());
}

#[test]
fn cli_accepts_negative_wrapped_tab_stops_with_both_argument_forms() {
    for value in ["-18446744073709551612", "-0xfffffffffffffffc"] {
        let equals_argument = format!("--tab-stop={value}");
        let equals = Cli::try_parse_from(["carta", equals_argument.as_str()]).unwrap();
        let separate = Cli::try_parse_from(["carta", "--tab-stop", value]).unwrap();
        assert_eq!(equals.tab_stop, Some(4), "equals form: {value}");
        assert_eq!(separate.tab_stop, Some(4), "separate form: {value}");
    }

    assert!(Cli::try_parse_from(["carta", "--tab-stop=-4"]).is_err());
    assert!(Cli::try_parse_from(["carta", "--tab-stop", "-4"]).is_err());
}
