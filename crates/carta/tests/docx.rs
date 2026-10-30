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

#[test]
fn language_and_direction_reach_footnotes_and_comments() {
    for (scope, input) in [
        (
            "language",
            include_str!("../../../corpus/ast-ext/docx/language.json"),
        ),
        (
            "direction",
            include_str!("../../../corpus/ast-ext/docx/direction.json"),
        ),
    ] {
        let bytes = docx_bytes("docx", input);
        let entries = zip::read(&bytes).unwrap();
        for (label, name) in [
            ("footnotes", "word/footnotes.xml"),
            ("comments", "word/comments.xml"),
        ] {
            let entry = entries.iter().find(|entry| entry.name == name).unwrap();
            let xml = std::str::from_utf8(&entry.data).unwrap();
            insta::assert_snapshot!(format!("{scope}__{label}"), xml.replace("><", ">\n<"));
        }
    }
}

#[test]
fn native_numbered_captions_keep_language_and_direction() {
    for (scope, input) in [
        (
            "language",
            include_str!("../../../corpus/ast-ext/docx/language.json"),
        ),
        (
            "direction",
            include_str!("../../../corpus/ast-ext/docx/direction.json"),
        ),
    ] {
        let output = document_part(&docx_bytes("docx+native_numbering", input));
        let caption = output
            .split("<w:p>")
            .find(|paragraph| paragraph.contains("caption</w:t>"))
            .unwrap()
            .split("</w:p>")
            .next()
            .unwrap();
        insta::assert_snapshot!(
            format!("{scope}__numbered_caption"),
            format!("<w:p>{caption}</w:p>")
        );
    }
}
