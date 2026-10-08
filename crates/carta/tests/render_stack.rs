//! Check deep rendering from a shallow caller stack.

// Serializing to JSON drives the deepest writer recursion with the least output per node.
#![cfg(feature = "write-json")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use carta::ast::{Block, Inline};
use carta::{Document, MediaBag, WriterOptions, render_document};

#[test]
fn deeply_nested_document_renders_from_a_small_caller_stack() {
    const DEPTH: usize = 20_000;

    let mut block = Block::Para(vec![Inline::Str("leaf".into())]);
    for _ in 0..DEPTH {
        block = Block::BlockQuote(vec![block]);
    }
    let document = Document {
        blocks: vec![block],
        ..Document::default()
    };

    // Use a shallow caller stack to check deep rendering and recursive drop.
    let rendered = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            render_document("json", document, MediaBag::new(), &WriterOptions::default()).is_ok()
        })
        .expect("spawn shallow caller")
        .join()
        .expect("shallow caller finished");

    assert!(
        rendered,
        "deep document serialized to JSON without overflowing"
    );
}
