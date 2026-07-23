//! Layer 1 writer golden tests: snapshot carta's own output for each `corpus/ast/<feature>/*` case
//! rendered to every target, minus the `(target, feature)` pairs listed in `corpus/exclusions.tsv`.
//!
//! The corpus is full-model AST-JSON, so this exercises writer node shapes no reader can produce.
//! Snapshots freeze current output and run offline. Reviewed with `cargo insta review`; never
//! hand-edit the `.snap` files.
//!
//! Each target and each extension spec gets its own `#[test]` so a single failing case cannot abort
//! the rest and nextest can run and parallelize them independently. A guard test asserts the target
//! macro's list equals `TARGETS`, and another asserts the ext macro's list equals the
//! `corpus/ast-ext/` directory set. The `assert_snapshot!` first argument fixes each `.snap`
//! filename.

// Integration-test harness code: panicking on a known corpus case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// The corpus is JSON AST: without the JSON reader there is nothing to render.
#![cfg(feature = "read-json")]

mod common;

use carta::{ReaderOptions, WriterOptions};
use common::{corpus_cases, corpus_groups, exclusions, is_excluded};

const TARGETS: &[&str] = &[
    "html",
    "html4",
    "json",
    "plain",
    "native",
    "latex",
    "commonmark",
    "commonmark_x",
    "markdown",
    "markdown_github",
    "markdown_phpextra",
    "markdown_mmd",
    "markdown_strict",
    "gfm",
    "rst",
    "mediawiki",
    "typst",
    "dokuwiki",
    "jira",
    "asciidoc",
    "man",
    "opml",
    "org",
    "beamer",
    "revealjs",
    "ipynb",
    "rtf",
];

/// Render every `corpus/ast/*` case to one target, skipping the target when its writer is not
/// compiled in and skipping the `(target, feature/case)` pairs listed in `corpus/exclusions.tsv`.
fn writer_snapshots_for(target: &str) {
    let excluded = exclusions();
    let writers = carta::supported_output_formats();
    if !writers.contains(&target) {
        return;
    }
    for case in corpus_cases("ast") {
        if is_excluded(&excluded, target, &case.group, &case.label) {
            continue;
        }
        let output = carta::convert_text(
            "json",
            target,
            &case.input,
            &ReaderOptions::default(),
            &WriterOptions::default(),
        )
        .unwrap_or_else(|error| {
            panic!(
                "convert json -> {target} {}/{}: {error}",
                case.group, case.label
            )
        });
        insta::assert_snapshot!(format!("{target}__{}__{}", case.group, case.label), output);
    }
}

/// Render every `corpus/ast-ext/<spec>/*` case with its full target format spec (e.g.
/// `markdown-fenced_divs`, `latex-smart`). `convert` resolves the spec's base writer and the
/// `±toggle` extensions, freezing toggled output the bare-name `corpus/ast` pass never reaches. A
/// spec whose base writer is not compiled, or is byte-shaped (no string form), is skipped at runtime.
fn writer_ext_snapshots_for(spec: &str) {
    let base = spec.split(['+', '-']).next().unwrap_or(spec);
    // Byte-shaped targets have no string form; their own container tests exercise them.
    if !matches!(carta::any_writer_for(base), Ok(carta::AnyWriter::Text(_))) {
        return;
    }
    for case in corpus_cases("ast-ext")
        .into_iter()
        .filter(|case| case.group == spec)
    {
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

macro_rules! writer_golden {
    ($helper:ident, $list:ident; $($name:ident => $target:literal),+ $(,)?) => {
        $(
            #[test]
            fn $name() { $helper($target); }
        )+
        const $list: &[&str] = &[$($target),+];
    };
}

writer_golden! {
    writer_snapshots_for, WRITER_TARGETS;
    writer_output_snapshots_html => "html",
    writer_output_snapshots_html4 => "html4",
    writer_output_snapshots_json => "json",
    writer_output_snapshots_plain => "plain",
    writer_output_snapshots_native => "native",
    writer_output_snapshots_latex => "latex",
    writer_output_snapshots_commonmark => "commonmark",
    writer_output_snapshots_commonmark_x => "commonmark_x",
    writer_output_snapshots_markdown => "markdown",
    writer_output_snapshots_markdown_github => "markdown_github",
    writer_output_snapshots_markdown_phpextra => "markdown_phpextra",
    writer_output_snapshots_markdown_mmd => "markdown_mmd",
    writer_output_snapshots_markdown_strict => "markdown_strict",
    writer_output_snapshots_gfm => "gfm",
    writer_output_snapshots_rst => "rst",
    writer_output_snapshots_mediawiki => "mediawiki",
    writer_output_snapshots_typst => "typst",
    writer_output_snapshots_dokuwiki => "dokuwiki",
    writer_output_snapshots_jira => "jira",
    writer_output_snapshots_asciidoc => "asciidoc",
    writer_output_snapshots_man => "man",
    writer_output_snapshots_opml => "opml",
    writer_output_snapshots_org => "org",
    writer_output_snapshots_beamer => "beamer",
    writer_output_snapshots_revealjs => "revealjs",
    writer_output_snapshots_ipynb => "ipynb",
    writer_output_snapshots_rtf => "rtf",
}

#[test]
fn writer_output_snapshots_all_targets_partitioned() {
    let mut expected: Vec<&str> = TARGETS.to_vec();
    expected.sort_unstable();
    let mut actual: Vec<&str> = WRITER_TARGETS.to_vec();
    actual.sort_unstable();
    assert_eq!(
        actual, expected,
        "TARGETS and the writer macro's test entries have diverged"
    );
}

#[test]
fn targets_match_registry_text_writers() {
    // Text-shaped writers: writer_for(name) succeeds; byte writers return Error::BinaryFormat.
    let mut text: Vec<&str> = carta::supported_output_formats()
        .into_iter()
        .filter(|name| carta::writer_for(name).is_ok())
        .collect();
    // `html5` is a text alias of `html`; golden coverage uses `html` only.
    text.retain(|name| *name != "html5");
    text.sort_unstable();
    let mut expected = TARGETS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        text, expected,
        "TARGETS drifted from the registry's text writers (excluding the html5 alias)"
    );
}

writer_golden! {
    writer_ext_snapshots_for, WRITER_EXT_GROUPS;
    writer_ext_output_snapshots_beamer_smart => "beamer-smart",
    writer_ext_output_snapshots_docx => "docx",
    writer_ext_output_snapshots_docx_empty_paragraphs => "docx+empty_paragraphs",
    writer_ext_output_snapshots_docx_native_numbering => "docx+native_numbering",
    writer_ext_output_snapshots_docx_styles => "docx+styles",
    writer_ext_output_snapshots_gfm_definition_lists => "gfm+definition_lists",
    writer_ext_output_snapshots_latex_smart => "latex-smart",
    writer_ext_output_snapshots_markdown_bracketed_spans_native_spans => "markdown-bracketed_spans-native_spans",
    writer_ext_output_snapshots_markdown_fenced_code_attributes => "markdown-fenced_code_attributes",
    writer_ext_output_snapshots_markdown_fenced_divs => "markdown-fenced_divs",
    writer_ext_output_snapshots_markdown_strikeout => "markdown-strikeout",
    writer_ext_output_snapshots_odt => "odt",
    writer_ext_output_snapshots_odt_empty_paragraphs => "odt+empty_paragraphs",
    writer_ext_output_snapshots_plain_smart => "plain+smart",
    writer_ext_output_snapshots_rst_smart => "rst+smart",
    writer_ext_output_snapshots_typst_smart => "typst-smart",
}

#[test]
fn writer_ext_output_snapshots_all_groups_partitioned() {
    let mut expected = corpus_groups("ast-ext");
    expected.sort();
    let mut actual: Vec<String> = WRITER_EXT_GROUPS
        .iter()
        .map(|group| (*group).to_owned())
        .collect();
    actual.sort();
    assert_eq!(
        actual, expected,
        "corpus/ast-ext directories and the ext macro's test entries have diverged"
    );
}
