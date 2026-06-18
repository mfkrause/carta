//! Layer 1 writer golden tests: snapshot carta's own output for each `corpus/ast/<feature>/*` case
//! rendered to every target, minus the `(target, feature)` pairs listed in `corpus/exclusions.tsv`.
//!
//! The corpus is full-model AST-JSON, so this exercises writer node shapes no reader can produce.
//! Snapshots freeze current output and run offline; differential parity is the conformance suite's
//! job. Reviewed with `cargo insta review`; never hand-edit the `.snap` files.

// Integration-test harness code: panicking on a known corpus case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// The corpus is JSON AST, so the JSON reader is the interchange every case is read through; without
// it there is nothing to render. Per-target, output is snapshotted only when that writer is compiled.
#![cfg(feature = "read-json")]

mod common;

use carta::{ReaderOptions, WriterOptions};
use common::{corpus_cases, exclusions, is_excluded};

const TARGETS: &[&str] = &[
    "html",
    "html4",
    "json",
    "plain",
    "native",
    "latex",
    "commonmark",
    "markdown",
    "gfm",
    "rst",
    "mediawiki",
    "typst",
    "dokuwiki",
    "jira",
    "asciidoc",
    "man",
    "opml",
    "beamer",
    "revealjs",
];

#[test]
fn writer_output_snapshots() {
    let excluded = exclusions();
    let writers = carta::supported_output_formats();
    for case in corpus_cases("ast") {
        for &target in TARGETS {
            if !writers.contains(&target)
                || is_excluded(&excluded, target, &case.group, &case.label)
            {
                continue;
            }
            let output = carta::convert(
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
