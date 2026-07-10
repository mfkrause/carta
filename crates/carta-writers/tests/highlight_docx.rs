//! Byte-shape checks for the DOCX writer's colorized code blocks: the source-code paragraph, the
//! per-token character-style runs, XML escaping of token text, the line breaks between source lines,
//! and the token character styles injected into the style catalogue from the active theme. These pin
//! the markup the writer emits when a highlighter and theme are supplied, independent of any CLI.

#![cfg(all(feature = "docx", feature = "highlight"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arc_with_non_send_sync
)]

use std::sync::Arc;

use carta_ast::{Attr, Block, Document, Text};
use carta_core::BytesWriter;
use carta_core::container::zip;
use carta_core::{HighlightOptions, WriterOptions};
use carta_highlight::{Highlighter, Theme};
use carta_writers::DocxWriter;

/// A theme customizing two token kinds, enough to see color and weight reach the style catalogue.
const THEME_JSON: &str = r##"{
    "text-color": null,
    "background-color": null,
    "line-number-color": null,
    "line-number-background-color": null,
    "text-styles": {
        "Keyword": { "text-color": "#007020", "background-color": null, "bold": true, "italic": false, "underline": false },
        "String": { "text-color": "#4070a0", "background-color": null, "bold": false, "italic": true, "underline": false }
    }
}"##;

fn options(with_highlighting: bool) -> WriterOptions {
    let mut options = WriterOptions::default();
    if with_highlighting {
        options.highlight = HighlightOptions {
            highlighter: Some(Arc::new(Highlighter::new())),
            theme: Some(Theme::from_json(THEME_JSON.as_bytes()).expect("parse theme")),
            ..HighlightOptions::default()
        };
    }
    options
}

fn code_block(classes: &[&str], text: &str) -> Block {
    Block::CodeBlock(
        Box::new(Attr {
            id: Text::from(""),
            classes: classes.iter().map(|c| Text::from(*c)).collect(),
            attributes: Vec::new(),
        }),
        Text::from(text),
    )
}

/// Renders a one-block document and returns the named part's markup.
fn part(with_highlighting: bool, block: Block, name: &str) -> String {
    let bytes = DocxWriter
        .write(
            &Document {
                blocks: vec![block],
                ..Document::default()
            },
            &options(with_highlighting),
        )
        .expect("render docx");
    let entries = zip::read(&bytes).expect("read docx archive");
    let entry = entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("missing part {name}"));
    String::from_utf8(entry.data.clone()).expect("utf-8 part")
}

fn document(with_highlighting: bool, block: Block) -> String {
    part(with_highlighting, block, "word/document.xml")
}

#[test]
fn known_language_wraps_each_token_in_a_style_run() {
    let doc = document(true, code_block(&["python"], "x = \"hi\"\n"));
    assert!(doc.contains("<w:pStyle w:val=\"SourceCode\" />"));
    assert!(doc.contains("<w:rStyle w:val=\"StringTok\" />"));
    // Even plain source text is wrapped, unlike the verbatim fallback.
    assert!(doc.contains("<w:rStyle w:val=\"NormalTok\" />"));
    assert!(!doc.contains("<w:rStyle w:val=\"VerbatimChar\" />"));
}

#[test]
fn token_text_is_xml_escaped() {
    let doc = document(true, code_block(&["python"], "s = \"a<b&c\"\n"));
    assert!(doc.contains("a&lt;b&amp;c"));
}

#[test]
fn multiple_lines_are_split_by_breaks() {
    let doc = document(true, code_block(&["python"], "a = 1\nb = 2\n"));
    assert!(doc.contains("<w:br />"));
}

#[test]
fn unknown_language_falls_back_to_the_plain_code_style() {
    let doc = document(true, code_block(&["nonexistent-language"], "x_y\n"));
    assert!(doc.contains("<w:pStyle w:val=\"SourceCode\" />"));
    assert!(doc.contains("<w:rStyle w:val=\"VerbatimChar\" />"));
    assert!(!doc.contains("Tok\" />"));
}

#[test]
fn no_class_falls_back_to_the_plain_code_style() {
    let doc = document(true, code_block(&[], "plain text\n"));
    assert!(doc.contains("<w:rStyle w:val=\"VerbatimChar\" />"));
    assert!(!doc.contains("Tok\" />"));
}

#[test]
fn a_disabled_highlighter_leaves_a_known_language_verbatim() {
    let doc = document(false, code_block(&["python"], "x = 1\n"));
    assert!(doc.contains("<w:rStyle w:val=\"VerbatimChar\" />"));
    assert!(!doc.contains("Tok\" />"));
}

#[test]
fn theme_token_styles_reach_the_style_catalogue() {
    let styles = part(true, code_block(&["python"], "x = 1\n"), "word/styles.xml");
    assert!(styles.contains("<w:style w:type=\"character\" w:styleId=\"KeywordTok\">"));
    assert!(styles.contains("<w:name w:val=\"KeywordTok\"/>"));
    assert!(styles.contains("<w:basedOn w:val=\"VerbatimChar\"/>"));
    assert!(styles.contains("<w:b/>"));
    assert!(styles.contains("<w:color w:val=\"007020\"/>"));
    // The italic string style lands too, with its own color.
    assert!(styles.contains("<w:style w:type=\"character\" w:styleId=\"StringTok\">"));
    assert!(styles.contains("<w:i/>"));
    assert!(styles.contains("<w:color w:val=\"4070a0\"/>"));
}

#[test]
fn a_disabled_highlighter_injects_no_token_styles() {
    let styles = part(false, code_block(&["python"], "x = 1\n"), "word/styles.xml");
    assert!(!styles.contains("KeywordTok"));
}
