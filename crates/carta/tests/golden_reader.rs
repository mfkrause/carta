//! Layer 1 reader golden tests: snapshot carta's own JSON AST for each `corpus/text/<fmt>/*` case.
//!
//! These freeze current reader output and run fully offline; this layer is the regression net and
//! the offline guarantee, while differential parity is the conformance suite's job. Snapshots are
//! reviewed with `cargo insta review`; never hand-edit the `.snap` files.

// Integration-test harness code: panicking on a known corpus case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// The JSON writer is the interchange used to snapshot reader output; without it there is nothing to
// freeze. Per-case, a corpus group is snapshotted only when its reader is also compiled in.
#![cfg(feature = "write-json")]

mod common;

use carta::{ReaderOptions, WriterOptions};
use common::corpus_cases;

#[test]
fn reader_ast_snapshots() {
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

/// Reader golden tests for extension toggles: each `corpus/text-ext/<spec>/*` directory is named for
/// the full format spec it is parsed with (e.g. `commonmark+strikeout`).
#[test]
fn reader_ext_ast_snapshots() {
    for case in corpus_cases("text-ext") {
        let (base, _) = carta::parse_format_spec(&case.group)
            .unwrap_or_else(|error| panic!("parse format spec {}: {error}", case.group));
        // A base that resolves to a compiled reader is testable, including the dialect aliases
        // (`markdown`, `gfm`, `commonmark_x`) that share one reader but are not its canonical name.
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
