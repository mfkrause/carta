//! Snapshot offline reader-to-writer conversions from the text corpus.
//!
//! Review changes with `cargo insta review`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use carta::{ReaderOptions, WriterOptions};
use common::corpus_cases;

/// The bounded target set every text case is rendered to. JSON is omitted because `golden_reader.rs`
/// already freezes `text -> json`; the remaining targets each exercise a distinct writer against every
/// reader's AST.
const TARGETS: &[&str] = &[
    "html",
    "latex",
    "rst",
    "plain",
    "commonmark",
    "mediawiki",
    "native",
];

/// Snapshots every `corpus/text/<format>/*` case as rendered to each compiled target in [`TARGETS`],
/// when the format's reader is compiled in.
#[test]
fn e2e_snapshots() {
    let readers = carta::supported_input_formats();
    let writers = carta::supported_output_formats();
    for case in corpus_cases("text") {
        if !readers.contains(&case.group.as_str()) {
            continue;
        }
        for &target in TARGETS {
            if !writers.contains(&target) {
                continue;
            }
            let output = carta::convert_text(
                &case.group,
                target,
                &case.input,
                &ReaderOptions::default(),
                &WriterOptions::default(),
            )
            .unwrap_or_else(|error| {
                panic!("convert {}/{} -> {target}: {error}", case.group, case.label)
            });
            insta::assert_snapshot!(format!("{}__{target}__{}", case.group, case.label), output);
        }
    }
}
