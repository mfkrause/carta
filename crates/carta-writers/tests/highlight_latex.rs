//! Byte-level checks for the LaTeX writer's colorized code blocks: the `Shaded`/`Highlighting`
//! environment pair, per-token style macros, token escaping, and line numbering. These pin the exact
//! fragment the writer emits when a highlighter is supplied, independent of the command line.

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
use carta_writers::LatexWriter;

fn options() -> WriterOptions {
    let mut options = WriterOptions::default();
    options.highlight = HighlightOptions {
        highlighter: Some(Arc::new(Highlighter::new())),
        theme: None,
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
    LatexWriter
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
        "\\begin{Shaded}\n\
         \\begin{Highlighting}[]\n\
         \\NormalTok{s }\\OperatorTok{=} \\StringTok{\"a\\textless{}b\\&c\\textgreater{}d\"}\n\
         \\end{Highlighting}\n\
         \\end{Shaded}"
    );
}

#[test]
fn unknown_language_is_all_normal() {
    let out = render(vec![code_block("", &["foobar"], &[], "x_y")]);
    assert_eq!(
        out,
        "\\begin{Shaded}\n\
         \\begin{Highlighting}[]\n\
         \\NormalTok{x\\_y}\n\
         \\end{Highlighting}\n\
         \\end{Shaded}"
    );
}

#[test]
fn no_class_stays_verbatim() {
    let out = render(vec![code_block("", &[], &[("foo", "bar")], "x_y\n")]);
    assert_eq!(out, "\\begin{verbatim}\nx_y\n\\end{verbatim}");
}

#[test]
fn number_lines_without_language_numbers_plain_text() {
    let out = render(vec![code_block("", &["numberLines"], &[], "a\nb")]);
    assert_eq!(
        out,
        "\\begin{Shaded}\n\
         \\begin{Highlighting}[numbers=left,,]\n\
         \\NormalTok{a}\n\
         \\NormalTok{b}\n\
         \\end{Highlighting}\n\
         \\end{Shaded}"
    );
}

#[test]
fn number_lines_start_from_sets_first_number() {
    let out = render(vec![code_block(
        "",
        &["python", "numberLines"],
        &[("startFrom", "2")],
        "a",
    )]);
    assert_eq!(
        out,
        "\\begin{Shaded}\n\
         \\begin{Highlighting}[numbers=left,,firstnumber=2,]\n\
         \\NormalTok{a}\n\
         \\end{Highlighting}\n\
         \\end{Shaded}"
    );
}

#[test]
fn explicit_id_prepends_a_phantom_label() {
    let out = render(vec![code_block("c1", &["python"], &[], "x")]);
    assert_eq!(
        out,
        "\\protect\\phantomsection\\label{c1}%\n\
         \\begin{Shaded}\n\
         \\begin{Highlighting}[]\n\
         \\NormalTok{x}\n\
         \\end{Highlighting}\n\
         \\end{Shaded}"
    );
}

#[test]
fn inter_token_whitespace_is_not_wrapped() {
    let out = render(vec![code_block("", &["python"], &[], "x  =  1")]);
    assert!(out.contains("\\NormalTok{x  }\\OperatorTok{=}  \\DecValTok{1}"));
}

#[test]
fn disabled_highlighter_leaves_code_verbatim() {
    let mut options = WriterOptions::default();
    options.highlight = HighlightOptions::default();
    let out = LatexWriter
        .write(
            &Document {
                blocks: vec![code_block("", &["python"], &[], "int x\n")],
                ..Document::default()
            },
            &options,
        )
        .expect("render");
    assert_eq!(out, "\\begin{verbatim}\nint x\n\\end{verbatim}");
}
