//! Layer 1 reader golden tests: snapshot carta's own JSON AST for each `corpus/text/<fmt>/*` case.
//!
//! These freeze current reader output and run fully offline; this layer is the regression net and
//! the offline guarantee, while differential parity is the conformance suite's job. Snapshots are
//! reviewed with `cargo insta review`; never hand-edit the `.snap` files.
//!
//! Each reader format, extension spec, and byte-container group gets its own `#[test]` so a single
//! failing case cannot abort the rest and nextest can run and parallelize them independently. A
//! per-kind guard test asserts the macro's group list still equals the `corpus/` directory set, so a
//! new corpus directory without a matching test fails loudly. Snapshot names are unchanged: the
//! `assert_snapshot!` first argument still fixes each `.snap` filename.

// Integration-test harness code: panicking on a known corpus case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// The JSON writer is the interchange used to snapshot reader output; without it there is nothing to
// freeze. Per-case, a corpus group is snapshotted only when its reader is also compiled in.
#![cfg(feature = "write-json")]

mod common;

use carta::{Output, ReaderOptions, WriterOptions};
use common::{corpus_binary_cases, corpus_cases, corpus_groups};

/// Snapshot carta's JSON AST for every `corpus/text/<group>/*` case, when the group's reader is
/// compiled in. A group whose reader is absent from this build is skipped at runtime.
fn reader_snapshots_for(group: &str) {
    let readers = carta::supported_input_formats();
    if !readers.contains(&group) {
        return;
    }
    for case in corpus_cases("text")
        .into_iter()
        .filter(|case| case.group == group)
    {
        let json = carta::convert_text(
            &case.group,
            "json",
            &case.input,
            &ReaderOptions::default(),
            &WriterOptions::default(),
        )
        .unwrap_or_else(|error| panic!("convert {}/{} -> json: {error}", case.group, case.label));
        insta::assert_snapshot!(format!("{}__{}", case.group, case.label), json);
    }
}

/// Snapshot carta's JSON AST for every `corpus/binary/<group>/*` case, reading each fixture as raw
/// bytes through the byte-input `convert` path (these are binary archives, not UTF-8 text). A group
/// whose byte-container reader is absent from this build is skipped at runtime.
fn reader_binary_snapshots_for(group: &str) {
    let readers = carta::supported_input_formats();
    if !readers.contains(&group) {
        return;
    }
    for case in corpus_binary_cases("binary")
        .into_iter()
        .filter(|case| case.group == group)
    {
        let output = carta::convert(
            &case.group,
            "json",
            &case.input,
            &ReaderOptions::default(),
            &WriterOptions::default(),
        )
        .unwrap_or_else(|error| panic!("convert {}/{} -> json: {error}", case.group, case.label));
        let json = match output {
            Output::Text(json) => json,
            Output::Bytes(_) => panic!(
                "json target must yield text for {}/{}",
                case.group, case.label
            ),
        };
        insta::assert_snapshot!(format!("{}__{}", case.group, case.label), json);
    }
}

/// Snapshot carta's JSON AST for every `corpus/text-ext/<spec>/*` case. The directory name is the
/// full format spec it is parsed with (e.g. `commonmark+strikeout`). A spec whose base resolves to no
/// compiled reader is skipped at runtime.
fn reader_ext_snapshots_for(spec: &str) {
    let (base, _) = carta::parse_format_spec(spec)
        .unwrap_or_else(|error| panic!("parse format spec {spec}: {error}"));
    // A base that resolves to a compiled reader is testable, including the dialect aliases
    // (`markdown`, `gfm`, `commonmark_x`) that share one reader but are not its canonical name.
    if carta::reader_for(&base).is_err() {
        return;
    }
    for case in corpus_cases("text-ext")
        .into_iter()
        .filter(|case| case.group == spec)
    {
        let json = carta::convert_text(
            &case.group,
            "json",
            &case.input,
            &ReaderOptions::default(),
            &WriterOptions::default(),
        )
        .unwrap_or_else(|error| panic!("convert {}/{} -> json: {error}", case.group, case.label));
        insta::assert_snapshot!(format!("{}__{}", case.group, case.label), json);
    }
}

/// Assert the macro's covered-group list equals the `corpus/<kind>/` directory set, so a new corpus
/// directory without a matching test entry fails loudly.
fn assert_groups_partitioned(kind: &str, covered: &[&str]) {
    let mut expected = corpus_groups(kind);
    expected.sort();
    let mut actual: Vec<String> = covered.iter().map(|group| (*group).to_owned()).collect();
    actual.sort();
    assert_eq!(
        actual, expected,
        "corpus/{kind} directories and the macro's test entries have diverged"
    );
}

macro_rules! reader_golden {
    ($helper:ident, $list:ident; $($name:ident => $group:literal),+ $(,)?) => {
        $(
            #[test]
            fn $name() { $helper($group); }
        )+
        const $list: &[&str] = &[$($group),+];
    };
}

reader_golden! {
    reader_snapshots_for, READER_TEXT_GROUPS;
    reader_ast_snapshots_commonmark => "commonmark",
    reader_ast_snapshots_csv => "csv",
    reader_ast_snapshots_dokuwiki => "dokuwiki",
    reader_ast_snapshots_html => "html",
    reader_ast_snapshots_ipynb => "ipynb",
    reader_ast_snapshots_jira => "jira",
    reader_ast_snapshots_json => "json",
    reader_ast_snapshots_latex => "latex",
    reader_ast_snapshots_man => "man",
    reader_ast_snapshots_mediawiki => "mediawiki",
    reader_ast_snapshots_native => "native",
    reader_ast_snapshots_opml => "opml",
    reader_ast_snapshots_org => "org",
    reader_ast_snapshots_rst => "rst",
    reader_ast_snapshots_rtf => "rtf",
    reader_ast_snapshots_tsv => "tsv",
}

#[test]
fn reader_ast_snapshots_all_groups_partitioned() {
    assert_groups_partitioned("text", READER_TEXT_GROUPS);
}

reader_golden! {
    reader_binary_snapshots_for, READER_BINARY_GROUPS;
    reader_binary_ast_snapshots_docx => "docx",
    reader_binary_ast_snapshots_epub => "epub",
    reader_binary_ast_snapshots_odt => "odt",
    reader_binary_ast_snapshots_rtf => "rtf",
}

#[test]
fn reader_binary_ast_snapshots_all_groups_partitioned() {
    assert_groups_partitioned("binary", READER_BINARY_GROUPS);
}

reader_golden! {
    reader_ext_snapshots_for, READER_EXT_GROUPS;
    reader_ext_ast_snapshots_commonmark_alerts => "commonmark+alerts",
    reader_ext_ast_snapshots_commonmark_attributes => "commonmark+attributes",
    reader_ext_ast_snapshots_commonmark_autolink_bare_uris => "commonmark+autolink_bare_uris",
    reader_ext_ast_snapshots_commonmark_bracketed_spans => "commonmark+bracketed_spans",
    reader_ext_ast_snapshots_commonmark_definition_lists => "commonmark+definition_lists",
    reader_ext_ast_snapshots_commonmark_emoji => "commonmark+emoji",
    reader_ext_ast_snapshots_commonmark_fancy_lists => "commonmark+fancy_lists",
    reader_ext_ast_snapshots_commonmark_fenced_divs => "commonmark+fenced_divs",
    reader_ext_ast_snapshots_commonmark_footnotes => "commonmark+footnotes",
    reader_ext_ast_snapshots_commonmark_gfm_auto_identifiers => "commonmark+gfm_auto_identifiers",
    reader_ext_ast_snapshots_commonmark_gfm_auto_identifiers_implicit_header_references => "commonmark+gfm_auto_identifiers+implicit_header_references",
    reader_ext_ast_snapshots_commonmark_hard_line_breaks => "commonmark+hard_line_breaks",
    reader_ext_ast_snapshots_commonmark_implicit_figures => "commonmark+implicit_figures",
    reader_ext_ast_snapshots_commonmark_pipe_tables => "commonmark+pipe_tables",
    reader_ext_ast_snapshots_commonmark_raw_attribute => "commonmark+raw_attribute",
    reader_ext_ast_snapshots_commonmark_raw_html => "commonmark+raw_html",
    reader_ext_ast_snapshots_commonmark_smart => "commonmark+smart",
    reader_ext_ast_snapshots_commonmark_strikeout => "commonmark+strikeout",
    reader_ext_ast_snapshots_commonmark_strikeout_subscript_superscript => "commonmark+strikeout+subscript+superscript",
    reader_ext_ast_snapshots_commonmark_subscript => "commonmark+subscript",
    reader_ext_ast_snapshots_commonmark_superscript => "commonmark+superscript",
    reader_ext_ast_snapshots_commonmark_task_lists => "commonmark+task_lists",
    reader_ext_ast_snapshots_commonmark_tex_math_dollars => "commonmark+tex_math_dollars",
    reader_ext_ast_snapshots_commonmark_yaml_metadata_block => "commonmark+yaml_metadata_block",
    reader_ext_ast_snapshots_docx_empty_paragraphs => "docx+empty_paragraphs",
    reader_ext_ast_snapshots_docx_styles => "docx+styles",
    reader_ext_ast_snapshots_dokuwiki_auto_identifiers => "dokuwiki+auto_identifiers",
    reader_ext_ast_snapshots_dokuwiki_auto_identifiers_ascii_identifiers => "dokuwiki+auto_identifiers+ascii_identifiers",
    reader_ext_ast_snapshots_dokuwiki_auto_identifiers_gfm_auto_identifiers => "dokuwiki+auto_identifiers+gfm_auto_identifiers",
    reader_ext_ast_snapshots_dokuwiki_east_asian_line_breaks => "dokuwiki+east_asian_line_breaks",
    reader_ext_ast_snapshots_dokuwiki_tex_math_dollars => "dokuwiki+tex_math_dollars",
    reader_ext_ast_snapshots_dokuwiki_smart => "dokuwiki-smart",
    reader_ext_ast_snapshots_html_gfm_auto_identifiers => "html+gfm_auto_identifiers",
    reader_ext_ast_snapshots_html_smart => "html+smart",
    reader_ext_ast_snapshots_html_tex_math_dollars => "html+tex_math_dollars",
    reader_ext_ast_snapshots_html_tex_math_double_backslash => "html+tex_math_double_backslash",
    reader_ext_ast_snapshots_html_tex_math_single_backslash => "html+tex_math_single_backslash",
    reader_ext_ast_snapshots_html_auto_identifiers => "html-auto_identifiers",
    reader_ext_ast_snapshots_html_line_blocks => "html-line_blocks",
    reader_ext_ast_snapshots_html_native_divs => "html-native_divs",
    reader_ext_ast_snapshots_html_native_spans => "html-native_spans",
    reader_ext_ast_snapshots_ipynb_escaped_line_breaks => "ipynb+escaped_line_breaks",
    reader_ext_ast_snapshots_ipynb_auto_identifiers => "ipynb-auto_identifiers",
    reader_ext_ast_snapshots_ipynb_backtick_code_blocks_fenced_code_blocks => "ipynb-backtick_code_blocks-fenced_code_blocks",
    reader_ext_ast_snapshots_ipynb_example_lists => "ipynb-example_lists",
    reader_ext_ast_snapshots_ipynb_intraword_underscores => "ipynb-intraword_underscores",
    reader_ext_ast_snapshots_ipynb_raw_html => "ipynb-raw_html",
    reader_ext_ast_snapshots_jira_east_asian_line_breaks => "jira+east_asian_line_breaks",
    reader_ext_ast_snapshots_latex_gfm_auto_identifiers => "latex+gfm_auto_identifiers",
    reader_ext_ast_snapshots_latex_raw_tex => "latex+raw_tex",
    reader_ext_ast_snapshots_latex_auto_identifiers => "latex-auto_identifiers",
    reader_ext_ast_snapshots_latex_latex_macros => "latex-latex_macros",
    reader_ext_ast_snapshots_latex_smart => "latex-smart",
    reader_ext_ast_snapshots_man_ascii_identifiers => "man+ascii_identifiers",
    reader_ext_ast_snapshots_man_east_asian_line_breaks => "man+east_asian_line_breaks",
    reader_ext_ast_snapshots_man_gfm_auto_identifiers => "man+gfm_auto_identifiers",
    reader_ext_ast_snapshots_man_auto_identifiers => "man-auto_identifiers",
    reader_ext_ast_snapshots_markdown => "markdown",
    reader_ext_ast_snapshots_markdown_abbreviations => "markdown+abbreviations",
    reader_ext_ast_snapshots_markdown_auto_identifiers => "markdown+auto_identifiers",
    reader_ext_ast_snapshots_markdown_blank_before_blockquote => "markdown+blank_before_blockquote",
    reader_ext_ast_snapshots_markdown_blank_before_header => "markdown+blank_before_header",
    reader_ext_ast_snapshots_markdown_citations => "markdown+citations",
    reader_ext_ast_snapshots_markdown_example_lists => "markdown+example_lists",
    reader_ext_ast_snapshots_markdown_fenced_code_attributes => "markdown+fenced_code_attributes",
    reader_ext_ast_snapshots_markdown_grid_tables => "markdown+grid_tables",
    reader_ext_ast_snapshots_markdown_header_attributes => "markdown+header_attributes",
    reader_ext_ast_snapshots_markdown_inline_code_attributes => "markdown+inline_code_attributes",
    reader_ext_ast_snapshots_markdown_inline_notes => "markdown+inline_notes",
    reader_ext_ast_snapshots_markdown_line_blocks => "markdown+line_blocks",
    reader_ext_ast_snapshots_markdown_link_attributes => "markdown+link_attributes",
    reader_ext_ast_snapshots_markdown_lists_without_preceding_blankline => "markdown+lists_without_preceding_blankline",
    reader_ext_ast_snapshots_markdown_mark => "markdown+mark",
    reader_ext_ast_snapshots_markdown_markdown_in_html_blocks => "markdown+markdown_in_html_blocks",
    reader_ext_ast_snapshots_markdown_mmd_header_identifiers => "markdown+mmd_header_identifiers",
    reader_ext_ast_snapshots_markdown_mmd_title_block => "markdown+mmd_title_block",
    reader_ext_ast_snapshots_markdown_multiline_tables => "markdown+multiline_tables",
    reader_ext_ast_snapshots_markdown_native_divs => "markdown+native_divs",
    reader_ext_ast_snapshots_markdown_native_spans => "markdown+native_spans",
    reader_ext_ast_snapshots_markdown_pandoc_title_block => "markdown+pandoc_title_block",
    reader_ext_ast_snapshots_markdown_raw_tex => "markdown+raw_tex",
    reader_ext_ast_snapshots_markdown_short_subsuperscripts => "markdown+short_subsuperscripts",
    reader_ext_ast_snapshots_markdown_simple_tables => "markdown+simple_tables",
    reader_ext_ast_snapshots_markdown_spaced_reference_links => "markdown+spaced_reference_links",
    reader_ext_ast_snapshots_markdown_startnum => "markdown+startnum",
    reader_ext_ast_snapshots_markdown_table_attributes => "markdown+table_attributes",
    reader_ext_ast_snapshots_markdown_table_captions => "markdown+table_captions",
    reader_ext_ast_snapshots_markdown_tex_math_double_backslash => "markdown+tex_math_double_backslash",
    reader_ext_ast_snapshots_markdown_tex_math_single_backslash => "markdown+tex_math_single_backslash",
    reader_ext_ast_snapshots_markdown_all_symbols_escapable => "markdown-all_symbols_escapable",
    reader_ext_ast_snapshots_markdown_space_in_atx_header => "markdown-space_in_atx_header",
    reader_ext_ast_snapshots_markdown_github => "markdown_github",
    reader_ext_ast_snapshots_markdown_mmd => "markdown_mmd",
    reader_ext_ast_snapshots_markdown_phpextra => "markdown_phpextra",
    reader_ext_ast_snapshots_markdown_strict => "markdown_strict",
    reader_ext_ast_snapshots_markdown_strict_markdown_attribute => "markdown_strict+markdown_attribute",
    reader_ext_ast_snapshots_mediawiki_ascii_identifiers => "mediawiki+ascii_identifiers",
    reader_ext_ast_snapshots_mediawiki_east_asian_line_breaks => "mediawiki+east_asian_line_breaks",
    reader_ext_ast_snapshots_mediawiki_gfm_auto_identifiers => "mediawiki+gfm_auto_identifiers",
    reader_ext_ast_snapshots_mediawiki_smart => "mediawiki+smart",
    reader_ext_ast_snapshots_mediawiki_auto_identifiers => "mediawiki-auto_identifiers",
    reader_ext_ast_snapshots_rst_ascii_identifiers => "rst+ascii_identifiers",
    reader_ext_ast_snapshots_rst_gfm_auto_identifiers => "rst+gfm_auto_identifiers",
    reader_ext_ast_snapshots_rst_smart => "rst+smart",
    reader_ext_ast_snapshots_rst_auto_identifiers => "rst-auto_identifiers",
}

#[test]
fn reader_ext_ast_snapshots_all_groups_partitioned() {
    assert_groups_partitioned("text-ext", READER_EXT_GROUPS);
}
