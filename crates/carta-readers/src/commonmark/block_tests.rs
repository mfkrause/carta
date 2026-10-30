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
        "```",     // 2: bare backtick run of three, a candidate
        "~~~~",    // 3: tilde run of four, a candidate
        "    ```", // 4: indented four columns, not a candidate
        "``",      // 5: run shorter than three, not a candidate
    ];
    let candidates = build_fence_close_candidates(&lines);

    assert!(candidates.reaches_close(b'`', 3, 1));
    assert!(!candidates.reaches_close(b'`', 3, 2));
    assert!(!candidates.reaches_close(b'`', 4, 1));
    assert!(candidates.reaches_close(b'~', 3, 0));
    assert!(candidates.reaches_close(b'~', 4, 0));
    assert!(!candidates.reaches_close(b'~', 5, 0));
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
use carta_core::{Extension, presets};

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
    assert_eq!(raw_tex_scan("\\begin{a}", "a", 0), (1, None));
    let (depth, close) = raw_tex_scan("\\end{a}rest", "a", 1);
    assert_eq!(depth, 0);
    assert_eq!(close, Some("\\end{a}".len()));
    assert_eq!(raw_tex_scan("\\end{c}", "a", 1), (1, None));
    assert_eq!(raw_tex_scan("\\begin{a}\\end{a}", "a", 1), (1, None));
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
fn only_a_closed_environment_tightens_an_adjacent_paragraph() {
    for (source, tight) in [
        ("before\n\\begin{center}\nx\n\\end{center}\n", true),
        ("before\n\\begin{center}x\\end{center} tail\n", true),
        ("before\n\n\\begin{center}\nx\n\\end{center}\n", false),
        ("before\n\\begin{center}\nx\n", false),
        ("before\n\n> \\begin{center}\n> x\n> \\end{center}\n", false),
    ] {
        let out = blocks(source);
        match out.first() {
            Some(IrBlock::Plain(text)) if tight => assert_eq!(text, "before"),
            Some(IrBlock::Para(text)) if !tight => assert_eq!(text, "before"),
            other => panic!("wrong paragraph type for {source:?}: {other:?}"),
        }
    }
}

#[test]
fn a_math_environment_stays_out_of_a_raw_block() {
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
    assert!(alert_marker_type("[!note]", true).is_none());
    assert!(alert_marker_type("[!Tip]", true).is_none());
    assert!(alert_marker_type("[!wArNiNg]", true).is_none());
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
fn indented_alert_markers_remain_ordinary_quotes() {
    for spaces in 0..=5 {
        let input = format!(">{}[!NOTE]\n> body\n", " ".repeat(spaces));
        let out = gfm_blocks(&input);
        if spaces <= 1 {
            assert!(matches!(out.first(), Some(IrBlock::Div(..))), "{input:?}");
        } else {
            assert!(
                matches!(out.first(), Some(IrBlock::BlockQuote(..))),
                "{input:?}"
            );
        }
    }
    for input in [
        ">\t[!NOTE]\n> body\n",
        "> \t[!NOTE]\n> body\n",
        ">\n> [!NOTE]\n> body\n",
    ] {
        let out = gfm_blocks(input);
        assert!(
            matches!(out.first(), Some(IrBlock::BlockQuote(..))),
            "{input:?}"
        );
    }
    let out = gfm_blocks("  >\t[!NOTE]\n  > body\n");
    assert!(matches!(out.first(), Some(IrBlock::Div(..))));
    let out = gfm_blocks("> > [!NOTE]\n> > body\n");
    assert!(matches!(out.first(), Some(IrBlock::BlockQuote(blocks))
        if matches!(blocks.first(), Some(IrBlock::Div(..)))));
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
    match out.first() {
        Some(IrBlock::Table { attr, caption, .. }) => (attr.clone(), caption.clone()),
        Some(IrBlock::TextTable(table)) => (table.attr.clone(), table.caption.clone()),
        Some(IrBlock::GridTable(table)) => (table.attr.clone(), table.caption.clone()),
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
    let (body, attr) = split_trailing_attr("Cap {#x} {#y}");
    assert_eq!(body, "Cap {#x}");
    assert_eq!(attr.expect("attr parsed").id, "y");
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
    // With table attributes off, the trailing block remains caption text.
    let mut table = blocks("| a |\n|---|\n| 1 |\n");
    assert!(super::super::postprocess::set_table_caption(
        &mut table,
        0,
        "c {#t}",
        presets::COMMONMARK
    ));
    let (attr, caption) = match table.first() {
        Some(IrBlock::Table { attr, caption, .. }) => (attr, caption),
        Some(IrBlock::TextTable(t)) => (&t.attr, &t.caption),
        other => panic!("expected a table, got {other:?}"),
    };
    assert!(attr.id.is_empty());
    assert_eq!(caption.as_deref(), Some("c {#t}"));
}

#[test]
fn inline_html_stays_in_the_markdown_paragraph() {
    for html in [
        "<!--\n\n# hidden\n-->",
        "<?pi\n\n# hidden\n?>",
        "<style>\ninside\n</style>",
        "<!DOCTYPE html>",
    ] {
        let source = format!("before\n{html}\nafter\n");
        let out = blocks(&source);
        assert!(
            matches!(out.as_slice(), [IrBlock::Para(text)] if text == source.trim_end()),
            "{out:?}"
        );
    }
}

#[test]
fn raw_text_html_blocks_tighten_the_markdown_paragraph() {
    for tag in ["pre", "script", "textarea"] {
        let source = format!("before\n<{tag}>\ninside\n</{tag}>\nafter\n");
        let out = blocks(&source);
        assert!(
            matches!(out.as_slice(), [IrBlock::Plain(before), IrBlock::RawHtml(_), IrBlock::Para(after)] if before == "before" && after == "after"),
            "{out:?}"
        );
    }
}

#[test]
fn inline_comments_keep_container_close_tags_literal() {
    let out = blocks("<div>\nbefore\n<!--\n</div>\n-->\nafter\n</div>\n");
    let [IrBlock::Div(_, content)] = out.as_slice() else {
        panic!("expected one div: {out:?}");
    };
    assert!(
        matches!(content.as_slice(), [IrBlock::Para(text)] if text == "before\n<!--\n</div>\n-->\nafter"),
        "{content:?}"
    );
}

#[test]
fn closing_html_tags_keep_following_text_outside_the_raw_block() {
    for tag in ["pre", "script", "style", "textarea", "div"] {
        let source = format!("</{tag}>\nafter\n");
        let out = blocks(&source);
        assert!(
            matches!(out.as_slice(), [IrBlock::RawHtml(tag), IrBlock::Para(after)] if tag == source.lines().next().unwrap() && after == "after"),
            "{out:?}"
        );
    }
}

#[test]
fn inline_tag_lines_stay_in_markdown_paragraphs() {
    for source in ["<span>\nafter\n", "<custom>\nafter\n", "</span>\nafter\n"] {
        let out = blocks(source);
        assert!(
            matches!(out.as_slice(), [IrBlock::Para(text)] if text == source.trim_end()),
            "{out:?}"
        );
    }
}

#[test]
fn indented_inline_html_keeps_its_paragraph_open() {
    for source in [
        "  <!--\n\n# hidden\n-->\nafter\n",
        "  <?pi\n\n# hidden\n?>\nafter\n",
    ] {
        let out = blocks(source);
        assert!(
            matches!(out.as_slice(), [IrBlock::Para(text)] if text == source.trim()),
            "{out:?}"
        );
    }
}

#[test]
fn unclosed_html_keeps_following_paragraphs_and_references() {
    for opening in ["<!--", "<?pi"] {
        let source = format!("before\n{opening}\ninside\n\n# heading\n\n[x]: /target\n");
        let (out, references, _, _) = parse(&source, presets::MARKDOWN, true);
        assert!(
            matches!(out.as_slice(), [IrBlock::Para(_), IrBlock::Heading(1, _)]),
            "{out:?}"
        );
        assert!(references.contains_key("x"));
    }
}

#[test]
fn unclosed_raw_text_html_keeps_only_its_opening_tag_raw() {
    for tag in ["pre", "script", "textarea", "style"] {
        let source = format!("<{tag}>inside\n\n# heading\n");
        let out = blocks(&source);
        assert!(
            matches!(out.as_slice(), [IrBlock::RawHtml(opening), IrBlock::Para(text), IrBlock::Heading(1, _)] if opening == &format!("<{tag}>") && text == "inside"),
            "{out:?}"
        );
    }
}

#[test]
fn repeated_unclosed_comments_do_not_hide_later_blocks() {
    let source = format!("{}\n# heading\n", "<!--\n".repeat(5_000));
    let out = blocks(&source);
    assert!(matches!(out.last(), Some(IrBlock::Heading(1, _))));
}

#[test]
fn unclosed_table_tags_stay_raw_when_only_native_divs_are_enabled() {
    let extensions =
        carta_core::Extensions::from_list(&[Extension::NativeDivs, Extension::RawHtml]);
    let out = parse("<table>\ninside\n", extensions, true).0;
    assert!(
        matches!(out.as_slice(), [IrBlock::RawHtml(tag), IrBlock::Para(text)] if tag == "<table>" && text == "inside"),
        "{out:?}"
    );
}

#[test]
fn nested_commonmark_html_has_no_document_end_padding() {
    for source in ["> <!--\n> inside\n", "- <!--\n  inside\n"] {
        let out = parse(source, carta_core::Extensions::empty(), false).0;
        let block = match out.first() {
            Some(IrBlock::BlockQuote(blocks)) => blocks.first(),
            Some(IrBlock::BulletList(items)) => items.first().and_then(|item| item.first()),
            other => panic!("expected a container: {other:?}"),
        };
        assert!(
            matches!(block, Some(IrBlock::RawHtml(text)) if text == "<!--\ninside\n"),
            "{block:?}"
        );
    }
}

#[test]
fn same_line_unclosed_tags_keep_following_blocks_without_recursion() {
    for (tag, extensions) in [
        ("pre", presets::MARKDOWN),
        ("table", carta_core::Extensions::empty()),
    ] {
        let source = format!("{}# heading", format!("<{tag}>").repeat(2_000));
        let out = parse(&source, extensions, true).0;
        assert_eq!(out.len(), 2_001);
        assert!(matches!(out.last(), Some(IrBlock::Heading(1, _))));
    }
}

#[test]
fn repeated_unclosed_table_tags_keep_following_blocks() {
    let extensions =
        carta_core::Extensions::from_list(&[Extension::NativeDivs, Extension::RawHtml]);
    let source = format!("{}\n# heading", "<table>\n".repeat(5_000));
    let out = parse(&source, extensions, true).0;
    assert!(matches!(out.last(), Some(IrBlock::Heading(1, _))));
}

#[test]
fn raw_html_content_uses_its_first_lines_indent() {
    for tag in ["table", "section", "pre"] {
        let source = format!("  <{tag}>\n  inside\n\n  # heading\n");
        let out = blocks(&source);
        assert!(
            matches!(out.as_slice(), [IrBlock::RawHtml(open), IrBlock::Para(text), IrBlock::Heading(1, _)] if open == &format!("<{tag}>") && text == "inside"),
            "{out:?}"
        );
    }
}

#[test]
fn native_div_content_keeps_its_indent() {
    let out = blocks("<div>\n  # heading\n\n    code\n</div>\n");
    let [IrBlock::Div(_, content)] = out.as_slice() else {
        panic!("expected one div: {out:?}");
    };
    assert!(
        matches!(content.as_slice(), [IrBlock::Para(text), IrBlock::CodeBlock(_, code)] if text == "# heading" && code == "code"),
        "{content:?}"
    );
}

#[test]
fn native_divs_without_parsed_html_need_a_block_boundary() {
    let extensions =
        carta_core::Extensions::from_list(&[Extension::NativeDivs, Extension::RawHtml]);
    for source in ["before\n<div>\ninside\n", "  <div>\n  inside\n"] {
        let out = parse(source, extensions, true).0;
        assert!(matches!(out.as_slice(), [IrBlock::Para(_)]), "{out:?}");
    }
}

#[test]
fn bare_markdown_fence_languages_are_normalized() {
    for (language, expected) in [
        ("Rust", "rust"),
        ("C++", "cpp"),
        ("Objective-C", "objectivec"),
        ("Foo-Bar", "foo-bar"),
    ] {
        let source = format!("```{language}\nx\n```\n");
        let out = blocks(&source);
        assert!(
            matches!(out.as_slice(), [IrBlock::CodeBlock(attr, _)] if attr.classes == [expected]),
            "{out:?}"
        );
        let commonmark = parse(&source, carta_core::Extensions::empty(), false).0;
        assert!(
            matches!(commonmark.as_slice(), [IrBlock::CodeBlock(attr, _)] if attr.classes == [language]),
            "{commonmark:?}"
        );
    }
    let out = blocks("```{.Rust .MyClass}\nx\n```\n");
    assert!(
        matches!(out.as_slice(), [IrBlock::CodeBlock(attr, _)] if attr.classes == ["Rust", "MyClass"]),
        "{out:?}"
    );
}
