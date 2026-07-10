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

use carta_ast::{Attr, Block, Document, Inline, Text};
use carta_core::{HighlightOptions, Writer, WriterOptions};
use carta_highlight::Highlighter;
use carta_writers::LatexWriter;

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

/// Idiomatic presentation runs no tokenizer and wraps a code block, whatever its language, in the
/// `lstlisting` environment instead of `verbatim`.
fn idiomatic_render(block: Block) -> String {
    let mut options = WriterOptions::default();
    options.highlight = HighlightOptions {
        idiomatic: true,
        ..HighlightOptions::default()
    };
    LatexWriter
        .write(
            &Document {
                blocks: vec![block],
                ..Document::default()
            },
            &options,
        )
        .expect("render")
}

#[test]
fn idiomatic_names_a_known_language() {
    let out = idiomatic_render(code_block("", &["python"], &[], "int x\n"));
    assert_eq!(
        out,
        "\\begin{lstlisting}[language=Python]\nint x\n\\end{lstlisting}"
    );
}

#[test]
fn idiomatic_leaves_an_unknown_language_bare() {
    let out = idiomatic_render(code_block("", &["rust"], &[], "int x\n"));
    assert_eq!(out, "\\begin{lstlisting}\nint x\n\\end{lstlisting}");
}

#[test]
fn idiomatic_uses_lstlisting_without_a_language() {
    let out = idiomatic_render(code_block("", &[], &[], "plain\n"));
    assert_eq!(out, "\\begin{lstlisting}\nplain\n\\end{lstlisting}");
}

#[test]
fn idiomatic_carries_numbering_start_and_identifier_as_options() {
    let out = idiomatic_render(code_block(
        "snippet",
        &["python", "numberLines"],
        &[("startFrom", "5")],
        "a = 1\n",
    ));
    assert_eq!(
        out,
        "\\begin{lstlisting}[language=Python, numbers=left, firstnumber=5, label=snippet]\na = 1\n\\end{lstlisting}"
    );
}

fn code_inline(classes: &[&str], text: &str) -> Block {
    Block::Para(vec![Inline::Code(
        Box::new(Attr {
            id: Text::from(""),
            classes: classes.iter().map(|c| Text::from(*c)).collect(),
            attributes: Vec::new(),
        }),
        Text::from(text),
    )])
}

#[test]
fn inline_code_tokenizes_inside_a_verb_group() {
    let out = render(vec![code_inline(&["python"], "print(x)")]);
    assert_eq!(out, "\\VERB|\\BuiltInTok{print}\\NormalTok{(x)}|");
}

#[test]
fn inline_code_escapes_a_literal_bar_so_it_cannot_close_the_group() {
    let out = render(vec![code_inline(&["python"], "a|b")]);
    assert_eq!(
        out,
        "\\VERB|\\NormalTok{a}\\OperatorTok{\\VerbBar{}}\\NormalTok{b}|"
    );
}

#[test]
fn inline_code_without_a_known_language_stays_verbatim() {
    let out = render(vec![code_inline(&["foobar"], "a b")]);
    assert_eq!(out, "\\texttt{a\\ b}");
}

#[test]
fn idiomatic_inline_code_names_a_known_language() {
    let out = idiomatic_render(code_inline(&["python"], "print(x)"));
    assert_eq!(out, "\\passthrough{\\lstinline[language=Python]!print(x)!}");
}

#[test]
fn idiomatic_inline_code_leaves_an_unknown_language_bare() {
    let out = idiomatic_render(code_inline(&["foobar"], "a b"));
    assert_eq!(out, "\\passthrough{\\lstinline!a b!}");
}

#[test]
fn idiomatic_inline_code_shifts_the_delimiter_off_the_source() {
    let out = idiomatic_render(code_inline(&["python"], "a!|b"));
    assert_eq!(out, "\\passthrough{\\lstinline[language=Python]\"a!|b\"}");
}
