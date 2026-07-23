//! End-to-end coverage of how syntax highlighting reaches a standalone document's scaffold: the
//! web templates carry the theme stylesheet, the print templates carry the per-token macros, the
//! idiomatic print mode carries the listing package, and an EPUB chapter inlines the stylesheet only
//! when the chapter holds colorized code. These wire the theme through the real reader-to-template
//! path, offline.

#![cfg(all(feature = "highlight", feature = "read-commonmark"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arc_with_non_send_sync
)]

use std::sync::Arc;

use carta::{
    HighlightOptions, Highlighter, Output, ReaderOptions, WriterOptions, builtin_style,
    convert_text, read_document, render_document,
};

/// A document with a heading and a colorizable code block.
const WITH_CODE: &str = "# Heading\n\n```python\nx = 1\n```\n";

/// A document whose only colorizable code is an inline span, no code block.
const WITH_INLINE_CODE: &str = "A line with `print(x)`{.python} inline.\n";

/// A highlighter with the named color theme active. Python is loaded from the runtime pack, since
/// the fixtures colorize Python and the default embedded set omits it.
fn themed(style: &str) -> HighlightOptions {
    let theme = builtin_style(style)
        .unwrap_or_else(|| panic!("style {style}"))
        .unwrap_or_else(|error| panic!("style {style}: {error}"));
    let mut highlighter = Highlighter::new();
    let python = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../carta-highlight/data/syntax-copyleft/python.xml");
    let xml = std::fs::read_to_string(&python)
        .unwrap_or_else(|error| panic!("read {}: {error}", python.display()));
    highlighter
        .registry_mut()
        .add_definition_with_stem(&xml, "python")
        .expect("parse python grammar");
    HighlightOptions {
        highlighter: Some(Arc::new(highlighter)),
        theme: Some(theme),
        idiomatic: false,
    }
}

fn standalone(target: &str, highlight: HighlightOptions, input: &str) -> String {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.highlight = highlight;
    convert_text(
        "markdown",
        target,
        input,
        &ReaderOptions::default(),
        &options,
    )
    .unwrap_or_else(|error| panic!("markdown -> {target}: {error}"))
}

#[test]
#[cfg(feature = "write-html")]
fn standalone_html_head_carries_theme_stylesheet() {
    let out = standalone("html", themed("pygments"), WITH_CODE);
    assert!(out.contains("/* CSS for syntax highlighting */"));
    assert!(out.contains("code span.kw { color: #007020; font-weight: bold; } /* Keyword */"));
}

#[test]
#[cfg(feature = "write-html")]
fn standalone_html_without_code_has_no_stylesheet() {
    let out = standalone("html", themed("pygments"), "Just prose, no code.\n");
    assert!(!out.contains("CSS for syntax highlighting"));
}

#[test]
#[cfg(feature = "write-html")]
fn none_mode_leaves_no_stylesheet() {
    let out = standalone("html", HighlightOptions::default(), WITH_CODE);
    assert!(!out.contains("CSS for syntax highlighting"));
    assert!(!out.contains("class=\"sourceCode"));
}

#[test]
#[cfg(feature = "write-latex")]
fn standalone_latex_preamble_carries_token_macros() {
    let out = standalone("latex", themed("pygments"), WITH_CODE);
    assert!(out.contains("\\newcommand{\\NormalTok}[1]{#1}"));
    assert!(
        out.contains(
            "\\newcommand{\\KeywordTok}[1]{\\textcolor[rgb]{0.00,0.44,0.13}{\\textbf{#1}}}"
        )
    );
    assert!(out.contains("\\begin{Highlighting}"));
}

#[test]
#[cfg(feature = "write-latex")]
fn standalone_latex_default_text_color_reaches_every_macro() {
    // A theme whose default foreground is set colors even the token kinds that specify none.
    let out = standalone("latex", themed("espresso"), WITH_CODE);
    assert!(out.contains("\\newcommand{\\NormalTok}[1]{\\textcolor[rgb]{0.74,0.68,0.62}{#1}}"));
    assert!(out.contains("\\newcommand{\\AttributeTok}[1]{\\textcolor[rgb]{0.74,0.68,0.62}{#1}}"));
}

#[test]
#[cfg(feature = "write-html")]
fn standalone_html_head_carries_stylesheet_for_inline_only_code() {
    let out = standalone("html", themed("pygments"), WITH_INLINE_CODE);
    assert!(out.contains("/* CSS for syntax highlighting */"));
    assert!(out.contains("code span.kw { color: #007020; font-weight: bold; } /* Keyword */"));
}

#[test]
#[cfg(feature = "write-latex")]
fn standalone_latex_preamble_carries_token_macros_for_inline_only_code() {
    let out = standalone("latex", themed("pygments"), WITH_INLINE_CODE);
    assert!(out.contains("\\newcommand{\\VERB}{\\Verb[commandchars=\\\\\\{\\}]}"));
    assert!(out.contains("\\newcommand{\\BuiltInTok}"));
    assert!(out.contains("\\VERB|"));
}

#[test]
#[cfg(feature = "write-latex")]
fn standalone_latex_without_code_has_no_macros() {
    let out = standalone("latex", themed("pygments"), "Just prose, no code.\n");
    assert!(!out.contains("NormalTok"));
}

#[test]
#[cfg(feature = "write-latex")]
fn idiomatic_latex_carries_the_listing_package() {
    let highlight = HighlightOptions {
        idiomatic: true,
        ..HighlightOptions::default()
    };
    let out = standalone("latex", highlight, WITH_CODE);
    assert!(out.contains("\\usepackage{listings}"));
    assert!(out.contains("\\newcommand{\\passthrough}[1]{#1}"));
    assert!(out.contains("\\begin{lstlisting}"));
    assert!(!out.contains("NormalTok"));
}

#[test]
#[cfg(feature = "write-epub")]
fn epub_chapter_inlines_theme_stylesheet_only_with_code() {
    let with_style = epub_chapter(themed("pygments"), WITH_CODE);
    assert!(with_style.contains("/* CSS for syntax highlighting */"));
    assert!(
        with_style.contains("code span.kw { color: #007020; font-weight: bold; } /* Keyword */")
    );

    let without = epub_chapter(themed("pygments"), "Just prose, no code.\n");
    assert!(without.contains("<style>\n  </style>"));
    assert!(!without.contains("CSS for syntax highlighting"));
}

/// Render `input` to an EPUB 3 archive and return the first chapter's XHTML.
#[cfg(feature = "write-epub")]
fn epub_chapter(highlight: HighlightOptions, input: &str) -> String {
    use carta_core::container::zip;

    let (document, media) = read_document("markdown", input.as_bytes(), &ReaderOptions::default())
        .expect("read markdown");
    let mut options = WriterOptions::default();
    options.highlight = highlight;
    let bytes = match render_document("epub3", document, media, &options).expect("render epub") {
        Output::Bytes(bytes) => bytes,
        Output::Text(_) => panic!("an EPUB writer must produce bytes"),
    };
    let entries = zip::read(&bytes).expect("valid epub archive");
    let chapter = entries
        .iter()
        .find(|entry| entry.name.ends_with("ch001.xhtml"))
        .expect("first chapter present");
    String::from_utf8_lossy(&chapter.data).into_owned()
}
