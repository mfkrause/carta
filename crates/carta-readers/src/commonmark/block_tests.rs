use super::super::postprocess::strip_caption_marker;

#[test]
fn recognizes_the_three_caption_markers() {
    assert_eq!(strip_caption_marker("Table: A caption"), Some("A caption"));
    assert_eq!(strip_caption_marker("table: A caption"), Some("A caption"));
    assert_eq!(strip_caption_marker(": A caption"), Some("A caption"));
}

#[test]
fn drops_the_spaces_after_the_marker() {
    assert_eq!(strip_caption_marker("Table:caption"), Some("caption"));
    assert_eq!(strip_caption_marker("Table:    caption"), Some("caption"));
    assert_eq!(strip_caption_marker(":caption"), Some("caption"));
}

#[test]
fn only_the_first_letter_may_vary_in_case() {
    assert_eq!(strip_caption_marker("TABLE: x"), None);
    assert_eq!(strip_caption_marker("TAble: x"), None);
    assert_eq!(strip_caption_marker("tABLE: x"), None);
}

#[test]
fn a_space_before_the_colon_is_not_a_marker() {
    assert_eq!(strip_caption_marker("Table : x"), None);
    assert_eq!(strip_caption_marker("table : x"), None);
}

#[test]
fn a_line_without_a_marker_is_rejected() {
    assert_eq!(strip_caption_marker("Just a paragraph"), None);
    assert_eq!(strip_caption_marker("Tablexyz"), None);
}

use super::build_fence_close_candidates;

#[test]
fn fence_close_candidate_index_honors_the_closing_fence_rules() {
    let lines = [
        "```rust", // 0: an info string follows the run, so not a bare closing fence
        "code",    // 1
        "```",     // 2: bare backtick run of three — a candidate
        "~~~~",    // 3: tilde run of four — a candidate
        "    ```", // 4: indented four columns — not a candidate
        "``",      // 5: run shorter than three — not a candidate
    ];
    let candidates = build_fence_close_candidates(&lines);

    // The only backtick candidate is line 2.
    assert!(candidates.reaches_close(b'`', 3, 1));
    assert!(!candidates.reaches_close(b'`', 3, 2));
    // A three-marker run cannot close a four-marker opener.
    assert!(!candidates.reaches_close(b'`', 4, 1));
    // Tilde and backtick are indexed separately; the tilde run of four closes openers up to four.
    assert!(candidates.reaches_close(b'~', 3, 0));
    assert!(candidates.reaches_close(b'~', 4, 0));
    assert!(!candidates.reaches_close(b'~', 5, 0));
    // Past the last candidate of a marker there is nothing left to close either kind.
    assert!(!candidates.reaches_close(b'`', 3, 3));
    assert!(!candidates.reaches_close(b'~', 3, 3));
}

use super::super::postprocess::raw_block_format;

#[test]
fn plain_raw_format_marker_is_recognized() {
    assert_eq!(raw_block_format("{=html}"), Some("html".to_owned()));
    assert_eq!(raw_block_format("{=latex}"), Some("latex".to_owned()));
    assert_eq!(raw_block_format("{=html-foo}"), Some("html-foo".to_owned()));
    assert_eq!(raw_block_format("{=html_foo}"), Some("html_foo".to_owned()));
    assert_eq!(raw_block_format("{=html5}"), Some("html5".to_owned()));
}

#[test]
fn whitespace_around_the_marker_is_tolerated() {
    assert_eq!(raw_block_format("{ =html}"), Some("html".to_owned()));
    assert_eq!(raw_block_format("{=html }"), Some("html".to_owned()));
    assert_eq!(raw_block_format("  {=html}  "), Some("html".to_owned()));
}

#[test]
fn a_gap_after_the_equals_is_not_a_marker() {
    assert_eq!(raw_block_format("{= html}"), None);
}

#[test]
fn extra_attributes_or_an_empty_format_are_not_markers() {
    assert_eq!(raw_block_format("{=html .foo}"), None);
    assert_eq!(raw_block_format("{=html foo}"), None);
    assert_eq!(raw_block_format("{=}"), None);
    assert_eq!(raw_block_format("{}"), None);
}

#[test]
fn a_symbol_in_the_format_name_is_not_a_marker() {
    assert_eq!(raw_block_format("{=ht.ml}"), None);
    assert_eq!(raw_block_format("{=ht/ml}"), None);
    assert_eq!(raw_block_format("{=ht+ml}"), None);
    assert_eq!(raw_block_format("{=ht:ml}"), None);
}

#[test]
fn an_ordinary_info_string_is_not_a_marker() {
    assert_eq!(raw_block_format("html"), None);
    assert_eq!(raw_block_format("=html"), None);
    assert_eq!(raw_block_format("{.html}"), None);
}

use super::super::postprocess::{is_math_environment, raw_tex_env_name, raw_tex_scan};
use super::{IrBlock, parse};
use carta_ast::Format;
use carta_core::presets;

fn blocks(input: &str) -> Vec<IrBlock> {
    parse(input, presets::MARKDOWN, true).0
}

#[test]
fn reads_the_begin_environment_name() {
    assert_eq!(
        raw_tex_env_name("\\begin{center}", b"begin").as_deref(),
        Some("center")
    );
    assert_eq!(
        raw_tex_env_name("\\begin {center}", b"begin").as_deref(),
        Some("center")
    );
    assert_eq!(
        raw_tex_env_name("\\end{a}rest", b"end").as_deref(),
        Some("a")
    );
    assert_eq!(
        raw_tex_env_name("\\begin{ a }", b"begin").as_deref(),
        Some(" a ")
    );
    assert_eq!(raw_tex_env_name("\\begin{}", b"begin").as_deref(), Some(""));
    // A bare word, a missing brace, or the wrong keyword is not a match.
    assert_eq!(raw_tex_env_name("\\beginabc", b"begin"), None);
    assert_eq!(raw_tex_env_name("\\begin center", b"begin"), None);
    assert_eq!(raw_tex_env_name("begin{a}", b"begin"), None);
}

#[test]
fn math_environments_are_excluded() {
    assert!(is_math_environment("equation"));
    assert!(is_math_environment("align*"));
    assert!(is_math_environment("math"));
    assert!(is_math_environment("dmath"));
    assert!(!is_math_environment("center"));
    assert!(!is_math_environment("align**"));
    assert!(!is_math_environment("xalignat"));
    assert!(!is_math_environment("Equation"));
}

#[test]
fn scan_tracks_depth_and_finds_the_close() {
    // The opener line alone opens at depth one and does not close.
    assert_eq!(raw_tex_scan("\\begin{a}", "a", 0), (1, None));
    // A matching end on a content line returns the offset past its brace.
    let (depth, close) = raw_tex_scan("\\end{a}rest", "a", 1);
    assert_eq!(depth, 0);
    assert_eq!(close, Some("\\end{a}".len()));
    // An unrelated end name is content, not a close.
    assert_eq!(raw_tex_scan("\\end{c}", "a", 1), (1, None));
    // Same-name nesting deepens and lifts the count.
    assert_eq!(raw_tex_scan("\\begin{a}\\end{a}", "a", 1), (1, None));
    // An escaped command does not count toward the depth.
    assert_eq!(raw_tex_scan("\\\\end{a}", "a", 1), (1, None));
}

#[test]
fn a_full_environment_becomes_a_raw_tex_block() {
    let out = blocks("\\begin{center}\nx\n\\end{center}\nafter\n");
    assert!(matches!(
        out.first(),
        Some(IrBlock::RawBlock(Format(fmt), body))
            if fmt == "tex" && body == "\\begin{center}\nx\n\\end{center}"
    ));
    assert!(matches!(out.get(1), Some(IrBlock::Para(p)) if p == "after"));
}

#[test]
fn a_single_line_environment_closes_on_its_own_line() {
    let out = blocks("\\begin{center}x\\end{center}\ny\n");
    assert!(matches!(
        out.first(),
        Some(IrBlock::RawBlock(Format(fmt), body))
            if fmt == "tex" && body == "\\begin{center}x\\end{center}"
    ));
    assert!(matches!(out.get(1), Some(IrBlock::Para(p)) if p == "y"));
}

#[test]
fn an_unclosed_environment_falls_back_to_a_paragraph() {
    let out = blocks("\\begin{center}\nx\ny\n");
    assert_eq!(out.len(), 1);
    assert!(matches!(out.first(), Some(IrBlock::Para(_))));
}

#[test]
fn a_math_environment_stays_out_of_a_raw_block() {
    // An \begin opening a math environment is not a block-level raw TeX environment.
    let out = blocks("\\begin{align}\nx\n\\end{align}\n");
    assert!(!matches!(out.first(), Some(IrBlock::RawBlock(..))));
}

#[test]
fn an_indented_begin_is_a_code_block() {
    let out = blocks("    \\begin{center}\n    x\n    \\end{center}\n");
    assert!(matches!(out.first(), Some(IrBlock::CodeBlock(..))));
}

#[test]
fn the_extension_off_leaves_the_syntax_literal() {
    let out = parse(
        "\\begin{center}\nx\n\\end{center}\n",
        presets::COMMONMARK,
        false,
    )
    .0;
    assert_eq!(out.len(), 1);
    assert!(matches!(out.first(), Some(IrBlock::Para(_))));
}

use super::super::postprocess::alert_marker_type;

fn gfm_blocks(input: &str) -> Vec<IrBlock> {
    parse(input, presets::GFM, false).0
}

#[test]
fn alert_marker_recognizes_every_kind() {
    assert_eq!(
        alert_marker_type("[!NOTE]", true).map(|t| t.class),
        Some("note")
    );
    assert_eq!(
        alert_marker_type("[!TIP]", true).map(|t| t.class),
        Some("tip")
    );
    assert_eq!(
        alert_marker_type("[!IMPORTANT]", true).map(|t| t.class),
        Some("important")
    );
    assert_eq!(
        alert_marker_type("[!WARNING]", true).map(|t| t.class),
        Some("warning")
    );
    assert_eq!(
        alert_marker_type("[!CAUTION]", true).map(|t| t.title),
        Some("Caution")
    );
}

#[test]
fn alert_marker_casing_depends_on_the_dialect() {
    // The broad Markdown dialect admits only the uppercase spelling.
    assert!(alert_marker_type("[!note]", true).is_none());
    assert!(alert_marker_type("[!Tip]", true).is_none());
    assert!(alert_marker_type("[!wArNiNg]", true).is_none());
    // The CommonMark engine accepts any casing.
    assert_eq!(
        alert_marker_type("[!note]", false).map(|t| t.class),
        Some("note")
    );
    assert_eq!(
        alert_marker_type("[!Tip]", false).map(|t| t.class),
        Some("tip")
    );
}

#[test]
fn alert_marker_allows_only_trailing_whitespace() {
    assert!(alert_marker_type("[!NOTE]", true).is_some());
    assert!(alert_marker_type("[!NOTE]   ", true).is_some());
    assert!(alert_marker_type("[!NOTE]\t", true).is_some());
    // Anything other than whitespace after the bracket disqualifies the marker.
    assert!(alert_marker_type("[!NOTE] hi", true).is_none());
    assert!(alert_marker_type("[!NOTE]x", true).is_none());
}

#[test]
fn alert_marker_rejects_unknown_or_malformed_markers() {
    assert!(alert_marker_type("[!FOO]", true).is_none());
    assert!(alert_marker_type("[!]", true).is_none());
    assert!(alert_marker_type("[NOTE]", true).is_none());
    assert!(alert_marker_type(" [!NOTE]", true).is_none());
    assert!(alert_marker_type("[!NOTE", true).is_none());
}

#[test]
fn an_alert_blockquote_becomes_a_titled_div() {
    let out = gfm_blocks("> [!NOTE]\n> This is a note.\n");
    let Some(IrBlock::Div(attr, content)) = out.first() else {
        panic!("expected a div, got {out:?}");
    };
    assert_eq!(attr.classes, vec!["note".to_owned()]);
    let Some(IrBlock::Div(title_attr, title)) = content.first() else {
        panic!("expected a title div");
    };
    assert_eq!(title_attr.classes, vec!["title".to_owned()]);
    assert!(matches!(title.as_slice(), [IrBlock::Para(t)] if t == "Note"));
    assert!(matches!(content.get(1), Some(IrBlock::Para(t)) if t == "This is a note."));
}

#[test]
fn a_marker_only_alert_carries_just_its_title() {
    let out = gfm_blocks("> [!TIP]\n");
    let Some(IrBlock::Div(attr, content)) = out.first() else {
        panic!("expected a div");
    };
    assert_eq!(attr.classes, vec!["tip".to_owned()]);
    assert_eq!(content.len(), 1);
    assert!(matches!(content.first(), Some(IrBlock::Div(..))));
}

#[test]
fn an_alert_preserves_richer_body_content() {
    let out = gfm_blocks("> [!WARNING]\n> # Heading\n");
    let Some(IrBlock::Div(_, content)) = out.first() else {
        panic!("expected a div");
    };
    assert!(matches!(content.get(1), Some(IrBlock::Heading(1, _))));
}

#[test]
fn an_unknown_marker_leaves_the_blockquote_intact() {
    let out = gfm_blocks("> [!FOO]\n> x\n");
    assert!(matches!(out.first(), Some(IrBlock::BlockQuote(_))));
}

#[test]
fn trailing_text_on_the_marker_line_leaves_the_blockquote_intact() {
    let out = gfm_blocks("> [!NOTE] hello\n> x\n");
    assert!(matches!(out.first(), Some(IrBlock::BlockQuote(_))));
}

#[test]
fn alerts_off_leaves_the_marker_literal() {
    let out = parse("> [!NOTE]\n> x\n", presets::COMMONMARK, true).0;
    assert!(matches!(out.first(), Some(IrBlock::BlockQuote(_))));
}

use super::super::postprocess::split_trailing_attr;

fn table_attr_and_caption(input: &str) -> (carta_ast::Attr, Option<String>) {
    let out = blocks(input);
    match out.into_iter().next() {
        Some(IrBlock::Table { attr, caption, .. }) => (attr, caption),
        Some(IrBlock::TextTable(table)) => (table.attr, table.caption),
        Some(IrBlock::GridTable(table)) => (table.attr, table.caption),
        other => panic!("expected a table, got {other:?}"),
    }
}

#[test]
fn split_trailing_attr_strips_the_block() {
    let (body, attr) = split_trailing_attr("My cap {#t .w}");
    assert_eq!(body, "My cap");
    let attr = attr.expect("attr parsed");
    assert_eq!(attr.id, "t");
    assert_eq!(attr.classes, ["w"]);
}

#[test]
fn split_trailing_attr_keeps_non_trailing_braces_literal() {
    // Only the last block at the very end is an attribute block.
    let (body, attr) = split_trailing_attr("Cap {#x} {#y}");
    assert_eq!(body, "Cap {#x}");
    assert_eq!(attr.expect("attr parsed").id, "y");
    // A block followed by more text is not trailing and stays untouched.
    assert_eq!(
        split_trailing_attr("Cap {#x} more"),
        ("Cap {#x} more", None)
    );
}

#[test]
fn split_trailing_attr_rejects_malformed_blocks() {
    assert_eq!(split_trailing_attr("Cap {#x").0, "Cap {#x");
    assert!(split_trailing_attr("Cap {#x").1.is_none());
    assert_eq!(split_trailing_attr("Cap {bad !!}").0, "Cap {bad !!}");
    assert!(split_trailing_attr("Cap {bad !!}").1.is_none());
    assert_eq!(split_trailing_attr("plain text").0, "plain text");
}

#[test]
fn caption_attributes_attach_to_the_table() {
    let (attr, caption) = table_attr_and_caption("| a |\n|---|\n| 1 |\n\n: My cap {#t .w}\n");
    assert_eq!(attr.id, "t");
    assert_eq!(attr.classes, ["w"]);
    assert_eq!(caption.as_deref(), Some("My cap"));
}

#[test]
fn caption_keyvals_attach_to_the_table() {
    let (attr, _) = table_attr_and_caption("| a |\n|---|\n| 1 |\n\n: c {key=val}\n");
    assert_eq!(attr.attributes, [("key".into(), "val".into())]);
}

#[test]
fn caption_without_a_block_leaves_attr_empty() {
    let (attr, caption) = table_attr_and_caption("| a |\n|---|\n| 1 |\n\n: just a caption\n");
    assert!(attr.id.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty());
    assert_eq!(caption.as_deref(), Some("just a caption"));
}

#[test]
fn caption_attributes_are_inert_without_the_extension() {
    // With table attributes disabled, the trailing block on a caption is kept verbatim as
    // caption text rather than split off onto the table's attributes.
    let mut table = blocks("| a |\n|---|\n| 1 |\n");
    assert!(super::super::postprocess::set_table_caption(
        &mut table,
        0,
        "c {#t}",
        presets::COMMONMARK
    ));
    let (attr, caption) = match table.into_iter().next() {
        Some(IrBlock::Table { attr, caption, .. }) => (attr, caption),
        Some(IrBlock::TextTable(t)) => (t.attr, t.caption),
        other => panic!("expected a table, got {other:?}"),
    };
    assert!(attr.id.is_empty());
    assert_eq!(caption.as_deref(), Some("c {#t}"));
}
