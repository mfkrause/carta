//! Snapshot DOCX extension output from `corpus/ast-ext/docx*`.
//!
//! Review changes with `cargo insta review`; do not edit snapshot files directly.

#![cfg(all(feature = "read-json", feature = "write-docx"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use carta::{Output, ReaderOptions, WriterOptions};
use carta_core::container::zip;
use common::corpus_cases;

/// Renders `input` (a JSON AST document) to the DOCX format spec `to`, returning the archive bytes.
fn docx_bytes(to: &str, input: &str) -> Vec<u8> {
    let output = carta::convert(
        "json",
        to,
        input.as_bytes(),
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap_or_else(|error| panic!("render {to}: {error}"));
    match output {
        Output::Bytes(bytes) => bytes,
        Output::Text(_) => panic!("a DOCX writer must produce bytes"),
    }
}

/// The main document part of a DOCX archive, pretty-printed one element per line for review.
fn document_part(bytes: &[u8]) -> String {
    let entries = zip::read(bytes).expect("read docx archive");
    let part = entries
        .iter()
        .find(|entry| entry.name == "word/document.xml")
        .expect("word/document.xml present");
    let xml = std::str::from_utf8(&part.data).expect("document.xml is utf-8");
    xml.replace("><", ">\n<")
}

#[test]
fn docx_extension_toggle_snapshots() {
    for case in corpus_cases("ast-ext") {
        if !case.group.starts_with("docx") {
            continue;
        }
        let output = document_part(&docx_bytes(&case.group, &case.input));
        insta::assert_snapshot!(format!("{}__{}", case.group, case.label), output);
    }
}
