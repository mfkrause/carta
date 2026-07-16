//! Offline stack-safety net for rendering. A pathologically deep document must serialize without
//! exhausting the caller's stack, even when the caller itself runs on a small one. Writers recurse
//! once per level of nesting, so [`carta::render_document`] grows a large stack on demand for that
//! work; this test drives it from a deliberately shallow caller to prove the growth happens.

// Serializing to JSON drives the deepest writer recursion with the least output per node.
#![cfg(feature = "write-json")]
// Integration-test harness code: panicking on failure is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use carta::ast::{Block, Inline};
use carta::{Document, MediaBag, WriterOptions, render_document};

#[test]
fn deeply_nested_document_renders_from_a_small_caller_stack() {
    const DEPTH: usize = 20_000;

    // Build the nested tree iteratively so constructing it does not itself recurse.
    let mut block = Block::Para(vec![Inline::Str("leaf".into())]);
    for _ in 0..DEPTH {
        block = Block::BlockQuote(vec![block]);
    }
    let document = Document {
        blocks: vec![block],
        ..Document::default()
    };

    // Render from a deliberately shallow stack. Serialization recurses once per level, so it can only
    // finish if rendering runs on its own deep stack rather than the caller's. The document is moved
    // into the renderer and dropped there, keeping its own deep drop off this stack as well.
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
