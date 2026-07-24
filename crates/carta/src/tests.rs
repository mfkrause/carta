// Indexing by a just-inserted key: a missing key should fail the test loudly.
#![allow(clippy::indexing_slicing)]

use super::{Cli, parse_metadata, parse_toc_depth, parse_variables, parse_wrap, template_dir};
use carta::WrapMode;
use carta::ast::MetaValue;
use clap::CommandFactory;
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
            // Only the first `=` splits, so a value may itself contain `=`.
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
            // The first separator wins: a `:` before an `=` keeps the `=` in the value.
            ("colon".to_owned(), "a=b".to_owned()),
            ("equals".to_owned(), "a:b".to_owned()),
        ]
    );
}

#[test]
fn metadata_splits_on_the_first_colon_or_equals() {
    let map = parse_metadata(
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
    let map = parse_metadata(
        &["a=true", "b=false", "c=text", "d", "e=True"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>(),
    );
    assert_eq!(map["a"], MetaValue::MetaBool(true));
    assert_eq!(map["b"], MetaValue::MetaBool(false));
    assert_eq!(map["c"], MetaValue::MetaString("text".into()));
    assert_eq!(map["d"], MetaValue::MetaBool(true));
    // Only lowercase `true`/`false` are booleans; anything else stays a string.
    assert_eq!(map["e"], MetaValue::MetaString("True".into()));
}

#[test]
fn repeated_metadata_key_accumulates_into_a_list() {
    // Two occurrences promote the key to a two-element list, in order.
    let two = parse_metadata(&["k=first".to_owned(), "k=second".to_owned()]);
    assert_eq!(
        two["k"],
        MetaValue::MetaList(vec![
            MetaValue::MetaString("first".into()),
            MetaValue::MetaString("second".into()),
        ])
    );
    // Further occurrences append; a bare first occurrence keeps its boolean element.
    let mixed = parse_metadata(&["k".to_owned(), "k=a".to_owned(), "k=b".to_owned()]);
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
    // Catches a clap configuration error (e.g. a duplicate short flag) at test time.
    Cli::command().debug_assert();
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
