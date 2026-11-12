//! Snapshot AST corpus rendering across targets.
//!
//! Review changes with `cargo insta review`; do not edit snapshot files directly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// The corpus is JSON AST: without the JSON reader there is nothing to render.
#![cfg(feature = "read-json")]

mod common;

use carta::{ReaderOptions, WriterOptions};
use common::corpus_cases;

#[cfg(feature = "write-html")]
#[test]
fn writer_mathml_output_snapshots_html() {
    let mut options = WriterOptions::default();
    options.math_method = carta::MathMethod::Mathml;
    for case in corpus_cases("ast")
        .into_iter()
        .filter(|case| case.group == "math")
    {
        let output = carta::convert_text(
            "json",
            "html",
            &case.input,
            &ReaderOptions::default(),
            &options,
        )
        .unwrap();
        insta::assert_snapshot!(format!("html_mathml__math__{}", case.label), output);
    }
}

/// Snapshots every `corpus/ast/*` case as rendered by every compiled writer that has a text form.
#[test]
fn writer_output_snapshots() {
    let targets: Vec<&str> = carta::supported_output_formats()
        .into_iter()
        .filter(|name| carta::writer_for(name).is_ok())
        .collect();
    for case in corpus_cases("ast") {
        for &target in &targets {
            let output = render(target, &case.input).unwrap_or_else(|error| {
                panic!(
                    "convert json -> {target} {}/{}: {error}",
                    case.group, case.label
                )
            });
            insta::assert_snapshot!(format!("{target}__{}__{}", case.group, case.label), output);
        }
    }
}

/// Render a JSON document to `target`. LaTeX and Beamer refuse embedded images, so their `data:`
/// images are first moved to `media/`, as the CLI does.
fn render(target: &str, input: &str) -> carta::Result<String> {
    let reader = ReaderOptions::default();
    let writer = WriterOptions::default();
    if !matches!(target, "latex" | "beamer") || !input.contains("\"data:") {
        return carta::convert_text("json", target, input, &reader, &writer);
    }
    let (mut document, _) = carta::read_document("json", input.as_bytes(), &reader)?;
    let mut media = carta::MediaBag::new();
    carta::walk::for_each_image_target(&mut document.blocks, &mut |target| {
        if let Some((bytes, mime)) = carta::media::decode_data_uri(&target.url) {
            media.insert(target.url.to_string(), Some(mime), bytes);
        }
    });
    carta::media::rewrite_extracted_references(&mut document.blocks, &media, "media");
    match carta::render_document(target, document, carta::MediaBag::new(), &writer)? {
        carta::Output::Text(text) => Ok(text),
        carta::Output::Bytes(_) => panic!("{target} output is text"),
    }
}

/// Snapshots every `corpus/ast-ext/<spec>/*` case as rendered with its full target format spec (e.g.
/// `markdown-fenced_divs`, `latex-smart`), which the bare-name `corpus/ast` pass never reaches.
#[test]
fn writer_extension_output_snapshots() {
    for case in corpus_cases("ast-ext") {
        let base = case.group.split(['+', '-']).next().unwrap_or(&case.group);
        // Byte-shaped targets have no string form; their own container tests exercise them.
        if !matches!(carta::any_writer_for(base), Ok(carta::AnyWriter::Text(_))) {
            continue;
        }
        let output = carta::convert_text(
            "json",
            &case.group,
            &case.input,
            &ReaderOptions::default(),
            &WriterOptions::default(),
        )
        .unwrap_or_else(|error| panic!("convert json -> {} {}: {error}", case.group, case.label));
        insta::assert_snapshot!(format!("ast-ext__{}__{}", case.group, case.label), output);
    }
}
