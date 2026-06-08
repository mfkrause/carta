//! Layer 1 writer golden tests: snapshot oxidoc's own output for each `corpus/ast/<feature>/*` case
//! rendered to every target, minus the `(target, feature)` pairs listed in `corpus/exclusions.tsv`.
//!
//! The corpus is full-model AST-JSON, so this exercises writer node shapes no reader can produce.
//! Snapshots freeze current output and run offline; parity against pandoc is the conformance suite's
//! job. Reviewed with `cargo insta review`; never hand-edit the `.snap` files.

// Integration-test harness code: panicking on a known corpus case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{corpus_cases, exclusions, is_excluded};
use oxidoc::{ReaderOptions, WriterOptions};

const TARGETS: &[&str] = &[
    "html",
    "json",
    "plain",
    "native",
    "latex",
    "commonmark",
    "rst",
    "mediawiki",
];

#[test]
fn writer_output_snapshots() {
    let excluded = exclusions();
    for case in corpus_cases("ast") {
        for &target in TARGETS {
            if is_excluded(&excluded, target, &case.group) {
                continue;
            }
            let output = oxidoc::convert(
                "json",
                target,
                &case.input,
                &ReaderOptions::default(),
                &WriterOptions::default(),
            )
            .unwrap_or_else(|error| {
                panic!(
                    "convert json -> {target} {}/{}: {error}",
                    case.group, case.label
                )
            });
            insta::assert_snapshot!(format!("{target}__{}__{}", case.group, case.label), output);
        }
    }
}
