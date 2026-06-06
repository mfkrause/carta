//! Differential parity tests over the vendored `CommonMark` specification examples. Each example's
//! markdown is run through both oracle surfaces — reader (text → JSON AST) and end-to-end
//! (text → HTML) — and compared against the pinned binary. The oracle is hard-required: its absence
//! fails (with provisioning instructions) rather than silently skipping, so a green run means real
//! parity.

// This whole file is test code, where panicking on a known example is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oxidoc_testkit::commonmark_spec::{self, SpecExample};
use oxidoc_testkit::differential::{self, Diff};
use oxidoc_testkit::pandoc_bin;

fn require_examples() -> Vec<SpecExample> {
    assert!(
        pandoc_bin().is_file(),
        "pinned pandoc binary not found at {}.\nRun tools/install-pandoc.sh.",
        pandoc_bin().display()
    );
    let examples = commonmark_spec::examples();
    assert!(
        !examples.is_empty(),
        "no examples extracted from the vendored spec"
    );
    examples
}

/// Run every example through `surface`, collecting a one-line report for each divergence. An
/// oracle-rejected input is not counted against oxidoc.
fn run_surface(
    surface: fn(&str) -> std::io::Result<Diff>,
    examples: &[SpecExample],
) -> Vec<String> {
    let mut failures = Vec::new();
    for example in examples {
        match surface(&example.markdown).expect("run differential surface") {
            Diff::Match | Diff::OracleRejected { .. } => {}
            Diff::Mismatch { detail } => {
                failures.push(format!("example {}: {detail}", example.number));
            }
            Diff::OxidocError { detail } => {
                failures.push(format!("example {}: error: {detail}", example.number));
            }
        }
    }
    failures
}

#[test]
fn spec_reader_matches_oracle_json() {
    let examples = require_examples();
    let failures = run_surface(differential::reader_json, &examples);
    assert!(
        failures.is_empty(),
        "{}/{} examples diverged on the reader surface:\n{}",
        failures.len(),
        examples.len(),
        failures.join("\n")
    );
}

#[test]
fn spec_end_to_end_matches_oracle_html() {
    let examples = require_examples();
    let failures = run_surface(differential::e2e_html, &examples);
    assert!(
        failures.is_empty(),
        "{}/{} examples diverged on the end-to-end surface:\n{}",
        failures.len(),
        examples.len(),
        failures.join("\n")
    );
}
