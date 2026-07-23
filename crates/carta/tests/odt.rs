//! Layer 1 golden test for the ODT writer's extension toggles. Each `corpus/ast-ext/odt*` case is
//! rendered to the format spec its directory names, the package's body part is unpacked, and its
//! markup (pretty-printed one element per line) is frozen by `insta`. This pins the output the
//! `empty_paragraphs` toggle produces, which the text-only writer golden pass (a byte-shaped target
//! has no string form) never reaches.
//!
//! Reviewed with `cargo insta review`; never hand-edit the `.snap` files.

#![cfg(all(feature = "read-json", feature = "write-odt"))]
// Integration-test harness code: panicking on a known corpus case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use carta::{Output, ReaderOptions, WriterOptions};
use carta_core::container::zip;
use common::corpus_cases;

/// Renders `input` (a JSON AST document) to the ODT format spec `to`, returning the archive bytes.
fn odt_bytes(to: &str, input: &str) -> Vec<u8> {
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
        Output::Text(_) => panic!("an ODT writer must produce bytes"),
    }
}

/// One XML part, pretty-printed one element per line for review.
fn pretty_part(data: &[u8]) -> String {
    let xml = std::str::from_utf8(data).expect("part is utf-8");
    xml.replace("><", ">\n<")
}

/// The body part of an ODT archive, followed by every embedded formula part (each a Presentation
/// MathML document under `Formula-N/content.xml`), pretty-printed one element per line for review.
/// Math renders into these side parts, not into `content.xml`, so freezing them keeps the formula
/// markup under snapshot review alongside the body that references it.
fn content_part(bytes: &[u8]) -> String {
    let entries = zip::read(bytes).expect("read odt archive");
    let body = entries
        .iter()
        .find(|entry| entry.name == "content.xml")
        .expect("content.xml present");
    let mut out = pretty_part(&body.data);
    let mut formulas: Vec<_> = entries
        .iter()
        .filter(|entry| entry.name.starts_with("Formula") && entry.name.ends_with("/content.xml"))
        .collect();
    formulas.sort_by(|a, b| a.name.cmp(&b.name));
    for formula in formulas {
        out.push_str("\n<!-- ");
        out.push_str(&formula.name);
        out.push_str(" -->\n");
        out.push_str(&pretty_part(&formula.data));
    }
    out
}

/// Writer body golden pass: every `corpus/ast/<feature>/*` case (full-model AST JSON that exercises
/// node shapes no reader can produce) is rendered to ODT and its `content.xml` (plus any embedded
/// formula parts) frozen. The text-writer golden pass skips this byte-shaped target, so this is where
/// the writer's block and inline rendering is pinned. Output is byte-reproducible across runs.
#[test]
fn odt_writer_corpus_snapshots() {
    for case in corpus_cases("ast") {
        let output = content_part(&odt_bytes("odt", &case.input));
        insta::assert_snapshot!(format!("corpus__{}__{}", case.group, case.label), output);
    }
}

#[test]
fn odt_extension_toggle_snapshots() {
    for case in corpus_cases("ast-ext") {
        if !case.group.starts_with("odt") {
            continue;
        }
        let output = content_part(&odt_bytes(&case.group, &case.input));
        insta::assert_snapshot!(format!("{}__{}", case.group, case.label), output);
    }
}

/// The document-metadata parts of an ODT archive: `meta.xml` in full, followed by the default
/// paragraph style extracted from `styles.xml`. Document metadata (title, description, subject,
/// keywords, and the language recorded on the default style) lands in these parts, never in
/// `content.xml`, so freezing them here is what keeps that output under snapshot review. Both parts
/// are byte-reproducible, so the snapshots are stable across runs.
fn metadata_parts(bytes: &[u8]) -> String {
    let entries = zip::read(bytes).expect("read odt archive");
    let part = |name: &str| {
        let entry = entries
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("{name} present"));
        std::str::from_utf8(&entry.data)
            .expect("part is utf-8")
            .to_owned()
    };
    let meta = part("meta.xml");
    let styles = part("styles.xml");
    let start = styles
        .find("<style:default-style style:family=\"paragraph\">")
        .expect("default paragraph style present");
    let end = styles[start..]
        .find("</style:default-style>")
        .map(|offset| start + offset + "</style:default-style>".len())
        .expect("default paragraph style closed");
    let default_style = styles.get(start..end).expect("default style slice");
    format!(
        "{}\n<!-- styles.xml default-style -->\n{}",
        pretty_part(meta.as_bytes()),
        pretty_part(default_style.as_bytes())
    )
}

/// Freezes the metadata parts (`meta.xml` and the default paragraph style) for the metadata fixture,
/// whose AST carries a title, author, description, subject, keyword list, and a `de-DE` language.
/// This pins how each metadata field maps onto the package and how the language sets the default
/// style's `fo:language`/`fo:country`, output the content-part pass never reaches.
#[test]
fn odt_metadata_part_snapshots() {
    for case in corpus_cases("ast-ext") {
        if case.group != "odt" || case.label != "metadata" {
            continue;
        }
        let output = metadata_parts(&odt_bytes(&case.group, &case.input));
        insta::assert_snapshot!(format!("metadata__{}__{}", case.group, case.label), output);
    }
}
