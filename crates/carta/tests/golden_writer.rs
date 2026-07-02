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
    "commonmark_x",
    "markdown",
    "markdown_github",
    "markdown_phpextra",
    "markdown_mmd",
    "markdown_strict",
    "gfm",
    "rst",
    "mediawiki",
    "typst",
    "dokuwiki",
    "jira",
    "asciidoc",
    "man",
    "opml",
    "org",
    "beamer",
    "revealjs",
    "ipynb",
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

/// Extension-toggle golden pass: each `corpus/ast-ext/<spec>/*` directory is named for the full
/// target format spec (e.g. `markdown-fenced_divs`, `latex-smart`) it is rendered with. `convert`
/// resolves the spec's base writer and the `±toggle` extensions, so this freezes the toggled output
/// that the default `corpus/ast` pass (which only renders bare format names) never reaches. A spec
/// whose base writer is not compiled into this build is skipped.
#[test]
fn writer_ext_output_snapshots() {
    let writers = carta::supported_output_formats();
    for case in corpus_cases("ast-ext") {
        let base = case.group.split(['+', '-']).next().unwrap_or(&case.group);
        if !writers.contains(&base) {
            continue;
        }
        let output = carta::convert(
            "json",
            &case.group,
            &case.input,
            &ReaderOptions::default(),
            &WriterOptions::default(),
        )
        .unwrap_or_else(|error| panic!("convert json -> {} {}: {error}", case.group, case.label));
        insta::assert_snapshot!(format!("ast-ext__{}__{}", case.group, case.label), output);
    }
}
