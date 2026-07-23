//! Layer 1 wrap golden tests: snapshot carta's output for the wrap-sensitive corpus cases under the
//! non-default wrap modes (`none` and `preserve`) across the text writers that lay paragraphs out to
//! a column.
//!
//! The everyday `golden_writer` suite renders every case under the default `auto` wrap; this file
//! pins the other two modes for the cases where they diverge: a long reflowed paragraph, a source
//! soft break, and a long table cell (whose layout differs between bordered-grid and free-flowing
//! cell writers). Reviewed with `cargo insta review`; never hand-edit the `.snap` files.

// Integration-test harness code: panicking on a known corpus case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// The corpus is JSON AST, so the JSON reader is the interchange every case is read through.
#![cfg(feature = "read-json")]

mod common;

use carta::{ReaderOptions, WrapMode, WriterOptions};
use common::corpus_cases;

/// Writers that reflow inline content to a fill column, so a wrap mode changes their output. Spans
/// the cell archetypes: bordered-grid cells (`rst`, `markdown`), free-flowing cells (`asciidoc`,
/// `typst`), and the LaTeX table-row layout (`latex`, `beamer`).
const TARGETS: &[&str] = &[
    "plain",
    "markdown",
    "gfm",
    "commonmark",
    "rst",
    "latex",
    "beamer",
    "typst",
    "asciidoc",
    "man",
    "html",
];

/// The corpus cases whose layout depends on the wrap mode, as `group/label`.
const CASES: &[(&str, &str)] = &[
    ("common", "para-wrap"),
    ("common", "soft-break"),
    ("table", "table-long-cell"),
];

#[test]
fn wrap_mode_output_snapshots() {
    let writers = carta::supported_output_formats();
    let cases = corpus_cases("ast");
    for &(group, label) in CASES {
        let case = cases
            .iter()
            .find(|case| case.group == group && case.label == label)
            .unwrap_or_else(|| panic!("missing corpus case {group}/{label}"));
        for &target in TARGETS {
            if !writers.contains(&target) {
                continue;
            }
            for (mode, mode_name) in [(WrapMode::None, "none"), (WrapMode::Preserve, "preserve")] {
                let mut options = WriterOptions::default();
                options.wrap = mode;
                let output = carta::convert_text(
                    "json",
                    target,
                    &case.input,
                    &ReaderOptions::default(),
                    &options,
                )
                .unwrap_or_else(|error| {
                    panic!("convert json -> {target} {group}/{label}: {error}")
                });
                insta::assert_snapshot!(format!("{target}__{group}__{label}__{mode_name}"), output);
            }
        }
    }
}
