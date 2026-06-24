//! Offline panic-safety net: parse every worked example from the vendored `CommonMark` spec and
//! require the reader to return `Ok`. The spec text is embedded at build time, so this runs fully
//! offline. It does not assert *what* the AST is (that is the conformance suite's differential job)
//! — only that no spec example makes the reader error or panic.

// The whole suite drives the CommonMark reader, so it only applies when that reader is built in.
#![cfg(feature = "read-commonmark")]
// Integration-test harness code: panicking on a known example is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use carta::ReaderOptions;

/// The vendored specification text, embedded so extraction needs no corpus fetch.
const SPEC: &str = include_str!("../../../vendor/commonmark/spec.txt");

/// The spec writes a literal tab as `→` (U+2192) inside examples.
const EXAMPLE_TAB: char = '\u{2192}';

/// Extract every worked example's markdown input, in document order, restoring tab placeholders.
fn spec_examples(spec: &str) -> Vec<String> {
    let is_fence = |line: &str| line.len() >= 3 && line.bytes().all(|byte| byte == b'`');
    let mut examples = Vec::new();
    let mut lines = spec.lines();
    while let Some(line) = lines.next() {
        if !line.strip_suffix(" example").is_some_and(is_fence) {
            continue;
        }
        let mut markdown = String::new();
        for content in lines.by_ref() {
            if content == "." {
                break;
            }
            markdown.push_str(&content.replace(EXAMPLE_TAB, "\t"));
            markdown.push('\n');
        }
        // Discard the reference HTML up to the closing fence; parity is the suite's job, not this.
        for content in lines.by_ref() {
            if is_fence(content) {
                break;
            }
        }
        examples.push(markdown);
    }
    examples
}

#[test]
fn spec_examples_parse_without_error() {
    let examples = spec_examples(SPEC);
    assert!(
        examples.len() > 600,
        "expected the full spec corpus, got {}",
        examples.len()
    );

    let reader = carta::reader_for("commonmark").expect("commonmark reader enabled");
    let options = ReaderOptions::default();

    let mut failures = Vec::new();
    for (index, markdown) in examples.iter().enumerate() {
        if let Err(error) = reader.read(markdown, &options) {
            failures.push(format!("example {}: {error}", index + 1));
        }
    }

    assert!(
        failures.is_empty(),
        "{} example(s) failed to parse:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
