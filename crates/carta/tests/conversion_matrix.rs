//! Check that every text corpus case renders with every writer.
//!
//! The reader and writer snapshots freeze the outputs; this covers the pairs that no snapshot holds.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use carta::{ReaderOptions, WriterOptions};
use common::corpus_cases;

#[test]
fn every_text_case_renders_with_every_writer() {
    let readers = carta::supported_input_formats();
    // The PDF writer needs an external engine.
    let targets: Vec<&str> = carta::supported_output_formats()
        .into_iter()
        .filter(|name| *name != "pdf")
        .collect();
    for case in corpus_cases("text") {
        if !readers.contains(&case.group.as_str()) {
            continue;
        }
        let (document, media) = carta::read_document(
            &case.group,
            case.input.as_bytes(),
            &ReaderOptions::default(),
        )
        .unwrap_or_else(|error| panic!("read {}/{}: {error}", case.group, case.label));
        for &target in &targets {
            let rendered = carta::render_document(
                target,
                document.clone(),
                media.clone(),
                &WriterOptions::default(),
            );
            if let Err(error) = rendered {
                panic!("render {}/{} -> {target}: {error}", case.group, case.label);
            }
        }
    }
}
