//! Byte-level checks for the HTML writer's colorized code blocks: the `div.sourceCode` scaffolding,
//! per-token spans, line anchors, and line numbering. These pin the exact fragment the writer emits
//! when a highlighter is supplied, independent of the command line.

#![cfg(feature = "highlight")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arc_with_non_send_sync
)]

use std::sync::Arc;

use carta_ast::{Attr, Block, Document, Text};
use carta_core::{HighlightOptions, Writer, WriterOptions};
use carta_highlight::Highlighter;
use carta_writers::HtmlWriter;

fn options() -> WriterOptions {
    let mut options = WriterOptions::default();
    options.highlight = HighlightOptions {
        highlighter: Some(Arc::new(Highlighter::new())),
        theme: None,
        ..HighlightOptions::default()
    };
    options
}

fn code_block(id: &str, classes: &[&str], attributes: &[(&str, &str)], text: &str) -> Block {
    Block::CodeBlock(
        Box::new(Attr {
            id: Text::from(id),
            classes: classes.iter().map(|c| Text::from(*c)).collect(),
            attributes: attributes
                .iter()
                .map(|(k, v)| (Text::from(*k), Text::from(*v)))
                .collect(),
        }),
        Text::from(text),
    )
}

fn render(blocks: Vec<Block>) -> String {
    HtmlWriter
        .write(
            &Document {
                blocks,
                ..Document::default()
            },
            &options(),
        )
        .expect("render")
}

#[test]
fn known_language_tokenizes_with_escaping() {
    let out = render(vec![code_block("", &["python"], &[], "s = \"a<b&c>d\"")]);
    assert_eq!(
        out,
        "<div class=\"sourceCode\" id=\"cb1\"><pre\n\
         class=\"sourceCode python\"><code class=\"sourceCode python\">\
         <span id=\"cb1-1\"><a href=\"#cb1-1\" aria-hidden=\"true\" tabindex=\"-1\"></a>\
         s <span class=\"op\">=</span> \
         <span class=\"st\">&quot;a&lt;b&amp;c&gt;d&quot;</span></span></code></pre></div>"
    );
}

#[test]
fn unknown_language_stays_plain() {
    let out = render(vec![code_block("", &["foobar"], &[], "x")]);
    assert_eq!(out, "<pre class=\"foobar\"><code>x</code></pre>");
}

#[test]
fn no_language_no_numbering_stays_plain() {
    let out = render(vec![code_block("myid", &[], &[("foo", "bar")], "x")]);
    assert_eq!(
        out,
        "<pre id=\"myid\" data-foo=\"bar\"><code>x</code></pre>"
    );
}

#[test]
fn number_lines_without_language_numbers_plain_text() {
    let out = render(vec![code_block("", &["numberLines"], &[], "a\nb")]);
    assert_eq!(
        out,
        "<div class=\"sourceCode\" id=\"cb1\"><pre\n\
         class=\"sourceCode numberSource numberLines\"><code class=\"sourceCode\">\
         <span id=\"cb1-1\"><a href=\"#cb1-1\"></a>a</span>\n\
         <span id=\"cb1-2\"><a href=\"#cb1-2\"></a>b</span></code></pre></div>"
    );
}

#[test]
fn number_lines_start_from_offsets_counter_and_ids() {
    let out = render(vec![code_block(
        "",
        &["python", "numberLines"],
        &[("startFrom", "2")],
        "a",
    )]);
    assert_eq!(
        out,
        "<div class=\"sourceCode\" id=\"cb1\" data-startFrom=\"2\"><pre\n\
         class=\"sourceCode numberSource python numberLines\">\
         <code class=\"sourceCode python\" style=\"counter-reset: source-line 1;\">\
         <span id=\"cb1-2\"><a href=\"#cb1-2\"></a>a</span></code></pre></div>"
    );
}

#[test]
fn start_from_one_carries_no_counter_reset() {
    let out = render(vec![code_block(
        "",
        &["python", "numberLines"],
        &[("startFrom", "1")],
        "a",
    )]);
    assert_eq!(
        out,
        "<div class=\"sourceCode\" id=\"cb1\" data-startFrom=\"1\"><pre\n\
         class=\"sourceCode numberSource python numberLines\"><code class=\"sourceCode python\">\
         <span id=\"cb1-1\"><a href=\"#cb1-1\"></a>a</span></code></pre></div>"
    );
}

#[test]
fn explicit_id_and_keyvals_flow_onto_the_wrapper() {
    let out = render(vec![code_block(
        "c1",
        &["python"],
        &[("foo", "bar baz")],
        "x",
    )]);
    assert_eq!(
        out,
        "<div class=\"sourceCode\" id=\"c1\" data-foo=\"bar baz\"><pre\n\
         class=\"sourceCode python\"><code class=\"sourceCode python\">\
         <span id=\"c1-1\"><a href=\"#c1-1\" aria-hidden=\"true\" tabindex=\"-1\"></a>x</span>\
         </code></pre></div>"
    );
}

#[test]
fn counter_advances_across_every_code_block() {
    let out = render(vec![
        code_block("", &["foobar"], &[], "x"),
        code_block("", &["python"], &[], "y"),
    ]);
    assert!(out.contains("<pre class=\"foobar\"><code>x</code></pre>"));
    assert!(out.contains("id=\"cb2\""));
    assert!(out.contains("<span id=\"cb2-1\">"));
}

#[test]
fn empty_lines_keep_their_anchors() {
    let out = render(vec![code_block("", &["python"], &[], "\nx")]);
    assert!(out.contains(
        "<span id=\"cb1-1\"><a href=\"#cb1-1\" aria-hidden=\"true\" tabindex=\"-1\"></a></span>\n"
    ));
    assert!(out.contains("<span id=\"cb1-2\">"));
}

#[test]
fn disabled_highlighter_leaves_code_plain() {
    let mut options = WriterOptions::default();
    options.highlight = HighlightOptions::default();
    let out = HtmlWriter
        .write(
            &Document {
                blocks: vec![code_block("", &["python"], &[], "int x")],
                ..Document::default()
            },
            &options,
        )
        .expect("render");
    assert_eq!(out, "<pre class=\"python\"><code>int x</code></pre>");
}
