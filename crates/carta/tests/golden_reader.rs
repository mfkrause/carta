//! Snapshot reader ASTs from text, extension, and binary corpora.
//!
//! Review changes with `cargo insta review`; do not edit snapshot files directly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// reader output is snapshotted as JSON; without the writer there is nothing to freeze
#![cfg(feature = "write-json")]

mod common;

use carta::{Output, ReaderOptions, WriterOptions};
use common::{corpus_binary_cases, corpus_cases, corpus_groups};

/// Snapshots the JSON AST of every `corpus/text/<format>/*` case whose reader is compiled in.
#[test]
fn text_corpus_ast_snapshots() {
    let readers = carta::supported_input_formats();
    for case in corpus_cases("text") {
        if !readers.contains(&case.group.as_str()) {
            continue;
        }
        let json = carta::convert_text(
            &case.group,
            "json",
            &case.input,
            &ReaderOptions::default(),
            &WriterOptions::default(),
        )
        .unwrap_or_else(|error| panic!("convert {}/{} -> json: {error}", case.group, case.label));
        insta::assert_snapshot!(format!("{}__{}", case.group, case.label), json);
    }
}

/// Snapshots the JSON AST of every `corpus/text-ext/<spec>/*` case. The directory name is the full
/// format spec the case is parsed with (e.g. `commonmark+strikeout`).
#[test]
fn extension_corpus_ast_snapshots() {
    for case in corpus_cases("text-ext") {
        let (base, _) = carta::parse_format_spec(&case.group)
            .unwrap_or_else(|error| panic!("parse format spec {}: {error}", case.group));
        // dialect aliases (markdown, gfm, commonmark_x) share a reader but are not its canonical name
        if carta::reader_for(&base).is_err() {
            continue;
        }
        let json = carta::convert_text(
            &case.group,
            "json",
            &case.input,
            &ReaderOptions::default(),
            &WriterOptions::default(),
        )
        .unwrap_or_else(|error| panic!("convert {}/{} -> json: {error}", case.group, case.label));
        insta::assert_snapshot!(format!("{}__{}", case.group, case.label), json);
    }
}

/// Snapshots the JSON AST of every `corpus/binary/<format>/*` case, read as raw bytes, whose reader
/// is compiled in.
#[test]
fn binary_corpus_ast_snapshots() {
    let readers = carta::supported_input_formats();
    for case in corpus_binary_cases("binary") {
        if !readers.contains(&case.group.as_str()) {
            continue;
        }
        let output = carta::convert(
            &case.group,
            "json",
            &case.input,
            &ReaderOptions::default(),
            &WriterOptions::default(),
        )
        .unwrap_or_else(|error| panic!("convert {}/{} -> json: {error}", case.group, case.label));
        let json = match output {
            Output::Text(json) => json,
            Output::Bytes(_) => panic!(
                "json target must yield text for {}/{}",
                case.group, case.label
            ),
        };
        insta::assert_snapshot!(format!("{}__{}", case.group, case.label), json);
    }
}

#[test]
fn every_reader_has_corpus_fixtures() {
    let mut have: std::collections::BTreeSet<String> = corpus_groups("text").into_iter().collect();
    have.extend(corpus_groups("binary"));
    for name in carta::supported_input_formats() {
        assert!(
            have.contains(name),
            "reader `{name}` has no corpus fixtures under corpus/text or corpus/binary"
        );
    }
}
