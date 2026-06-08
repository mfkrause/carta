//! Layer 1 reader golden tests: snapshot oxidoc's own JSON AST for each `corpus/text/<fmt>/*` case.
//!
//! These freeze current reader output and run fully offline. Correctness against pandoc is the
//! conformance suite's job; this layer is the regression net and the no-oracle guarantee. Snapshots
//! are reviewed with `cargo insta review`; never hand-edit the `.snap` files.

// Integration-test harness code: panicking on a known corpus case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::corpus_cases;
use oxidoc::{ReaderOptions, WriterOptions};

#[test]
fn reader_ast_snapshots() {
    for case in corpus_cases("text") {
        let json = oxidoc::convert(
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
