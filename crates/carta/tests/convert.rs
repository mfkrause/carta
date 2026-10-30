//! Offline conversion and format-resolution tests across feature sets.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use carta::{
    Error, Output, ReaderOptions, WriterOptions, any_reader_for, any_writer_for, convert,
    convert_text, reader_for, writer_for,
};

#[cfg(all(
    feature = "read-commonmark",
    feature = "write-latex",
    feature = "write-beamer"
))]
#[test]
fn tex_text_conversion_requires_embedded_image_extraction() {
    for target in ["latex", "beamer"] {
        let error = convert_text(
            "markdown",
            target,
            "![image](data:image/png;base64,/wA=)",
            &ReaderOptions::default(),
            &WriterOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(error, Error::Unrepresentable(_)), "{error}");
        assert!(
            error.to_string().contains("extract embedded images"),
            "{error}"
        );
    }
}

#[cfg(all(feature = "read-commonmark", feature = "write-asciidoc"))]
#[test]
fn list_markers_count_toward_the_first_wrapped_line() {
    let mut options = WriterOptions::default();
    options.columns = Some(10);
    for (source, expected) in [
        ("- alpha beta gamma", "* alpha\nbeta gamma"),
        ("- [ ] alpha beta gamma", "* [ ]\nalpha beta\ngamma"),
        ("- one\n  - alpha beta gamma", "* one\n** alpha\nbeta gamma"),
        ("1. alpha beta gamma", "[arabic]\n. alpha\nbeta gamma"),
    ] {
        let output = convert_text(
            "markdown",
            "asciidoc",
            source,
            &ReaderOptions::default(),
            &options,
        )
        .unwrap();
        assert_eq!(output, expected);
    }
}

#[cfg(all(feature = "read-commonmark", feature = "write-asciidoc"))]
#[test]
fn inline_footnotes_wrap_with_the_surrounding_text() {
    use carta::ast::{Block, Document, Inline};
    let mut options = WriterOptions::default();
    options.columns = Some(20);
    let output = convert_text(
        "markdown",
        "asciidoc",
        "A note[^x] follows.\n\n[^x]: alpha beta gamma delta epsilon zeta eta theta\n",
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    assert_eq!(
        output,
        "A\nnotefootnote:[alpha\nbeta gamma delta\nepsilon zeta eta\ntheta] follows."
    );
    options.wrap = carta::WrapMode::None;
    let document = Document {
        blocks: vec![Block::Para(vec![
            Inline::Str("text".into()),
            Inline::Note(vec![Block::Para(vec![
                Inline::Space,
                Inline::Str("body".into()),
                Inline::Space,
            ])]),
        ])],
        ..Document::default()
    };
    let output =
        carta::render_document("asciidoc", document, Default::default(), &options).unwrap();
    assert_eq!(output, Output::Text("textfootnote:[ body ]".into()));
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "write-commonmark",
    feature = "write-markdown",
    feature = "write-plain",
    feature = "write-man",
    feature = "write-asciidoc"
))]
#[test]
fn line_blocks_wrap_without_adding_verse_lines() {
    let source = "| alpha beta gamma delta epsilon zeta\n| second line has several more words\n";
    let mut options = WriterOptions::default();
    options.columns = Some(20);
    for (target, expected) in [
        (
            "markdown",
            "| alpha beta gamma\n  delta epsilon zeta\n| second line has\n  several more words",
        ),
        (
            "commonmark",
            "alpha beta gamma\ndelta epsilon zeta\\\nsecond line has\nseveral more words",
        ),
        (
            "plain",
            "alpha beta gamma\ndelta epsilon zeta\nsecond line has\nseveral more words",
        ),
        (
            "man",
            ".PP\nalpha beta gamma\ndelta epsilon zeta\n.PD 0\n.P\n.PD\nsecond line has\nseveral more words",
        ),
        (
            "asciidoc",
            "[verse]\n--\nalpha beta gamma delta epsilon zeta\nsecond line has several more words\n--",
        ),
    ] {
        let output = convert_text(
            "markdown",
            target,
            source,
            &ReaderOptions::default(),
            &options,
        )
        .unwrap();
        assert_eq!(output, expected, "{target}");
        if target == "markdown" {
            let (before, _) =
                carta::read_document("markdown", source.as_bytes(), &ReaderOptions::default())
                    .unwrap();
            let (after, _) =
                carta::read_document("markdown", output.as_bytes(), &ReaderOptions::default())
                    .unwrap();
            assert_eq!(after, before);
        }
    }
}

#[cfg(all(
    feature = "write-markdown",
    feature = "write-plain",
    feature = "write-man"
))]
#[test]
fn quoted_line_blocks_keep_their_initial_space() {
    use carta::ast::{Block, Document, Inline};
    let document = Document {
        blocks: vec![Block::BlockQuote(vec![Block::LineBlock(vec![vec![
            Inline::Space,
            Inline::Str("alpha".into()),
            Inline::Space,
            Inline::Str("beta".into()),
            Inline::Space,
            Inline::Str("gamma".into()),
        ]])])],
        ..Document::default()
    };
    let mut options = WriterOptions::default();
    options.columns = Some(12);
    for (target, expected) in [
        ("markdown-line_blocks", ">  alpha\n> beta gamma"),
        ("plain", "   alpha\n  beta gamma"),
        ("man", ".RS\n.PP\n\\ alpha beta\ngamma\n.RE"),
    ] {
        let output =
            carta::render_document(target, document.clone(), Default::default(), &options).unwrap();
        assert_eq!(output, Output::Text(expected.into()), "{target}");
    }
}

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn convert_on_text_target_matches_convert_text() {
    let bytes = convert(
        "commonmark",
        "html",
        b"# Hi\n",
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap();
    let text = convert_text(
        "commonmark",
        "html",
        "# Hi\n",
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap();
    assert_eq!(bytes, Output::Text(text));
}

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn convert_rejects_invalid_utf8_for_a_text_reader() {
    let error = convert(
        "commonmark",
        "html",
        &[0xff, 0xfe],
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::InvalidUtf8(_)), "{error:?}");
}

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn commonmark_to_html() {
    let output = convert_text(
        "commonmark",
        "html",
        "# Hi\n",
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap();
    assert_eq!(output, "<h1>Hi</h1>");
}

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn number_sections_without_standalone_numbers_the_body_in_place() {
    let mut writer_options = WriterOptions::default();
    writer_options.number_sections = true;
    let output = convert_text(
        "commonmark",
        "html",
        "# First\n\n## Nested\n",
        &ReaderOptions::default(),
        &writer_options,
    )
    .unwrap();

    assert!(
        output.contains(r#"<span class="header-section-number">1</span>"#),
        "top heading not numbered: {output}"
    );
    assert!(
        output.contains(r#"<span class="header-section-number">1.1</span>"#),
        "nested heading not numbered: {output}"
    );
}

#[cfg(all(feature = "read-commonmark", feature = "write-revealjs"))]
#[test]
fn revealjs_numbers_slide_titles_and_body_headings() {
    let mut options = WriterOptions::default();
    options.number_sections = true;
    let output = convert_text(
        "markdown",
        "revealjs",
        "# First\n\nbody\n\n## Nested\n\nmore\n\n# Next {.unnumbered}\n",
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    assert_eq!(
        output,
        "<section id=\"first\" class=\"slide level1\" data-number=\"1\">\n\
         <h1 data-number=\"1\"><span class=\"header-section-number\">1</span>\n\
         First</h1>\n<p>body</p>\n\
         <h2 data-number=\"1.1\" id=\"nested\"><span\n\
         class=\"header-section-number\">1.1</span> Nested</h2>\n<p>more</p>\n</section>\n\
         <section id=\"next\" class=\"slide level1 unnumbered\">\n\
         <h1 class=\"unnumbered\">Next</h1>\n</section>"
    );
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "write-revealjs",
    feature = "standalone"
))]
#[test]
fn revealjs_contents_slide_has_navigation_anchors() {
    for numbered in [false, true] {
        let mut options = WriterOptions::default();
        options.standalone = true;
        options.toc = true;
        options.number_sections = numbered;
        let output = convert_text(
            "markdown",
            "revealjs",
            "# First [label]{#heading-span} {#first}\n\nbody\n\n## Nested\n\n[owned](#first){#toc-owned}\n",
            &ReaderOptions::default(),
            &options,
        )
        .unwrap();
        assert!(output.contains("<section id=\"TOC\">\n<nav role=\"doc-toc\">\n<ul>"));
        assert!(output.contains(r##"href="#/first" id="/toc-first""##));
        assert!(output.contains(r##"href="#/nested" id="/toc-nested""##));
        assert!(output.contains(r##"href="#/first" id="toc-owned""##));
        assert!(output.contains(r#"id="/heading-span""#));
        assert!(output.contains(r#"id="heading-span""#));
        let count = if numbered { 2 } else { 0 };
        assert_eq!(
            output.matches(r#"class="toc-section-number""#).count(),
            count
        );
        assert_eq!(
            output.matches(r#"class="header-section-number""#).count(),
            count
        );
    }
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "write-html",
    feature = "standalone"
))]
#[test]
fn number_sections_with_standalone_toc_numbers_body_and_toc_exactly_once() {
    let mut writer_options = WriterOptions::default();
    writer_options.number_sections = true;
    writer_options.standalone = true;
    writer_options.toc = true;
    let output = convert_text(
        "commonmark",
        "html",
        "# First\n\n## Nested\n",
        &ReaderOptions::default(),
        &writer_options,
    )
    .unwrap();

    assert!(
        output.contains(r#"<span class="toc-section-number">1</span>"#)
            && output.contains(r#"<span class="toc-section-number">1.1</span>"#),
        "contents entries not numbered: {output}"
    );
    assert_eq!(
        output.matches(r#"class="header-section-number""#).count(),
        2,
        "body section numbers doubled or leaked into the contents: {output}"
    );
    assert_eq!(
        output.matches(r#"class="toc-section-number""#).count(),
        2,
        "contents section numbers doubled: {output}"
    );
}

#[cfg(all(feature = "read-html", feature = "write-gfm"))]
#[test]
fn gfm_without_raw_html_uses_markdown_fallbacks() {
    let output = convert_text(
        "html+raw_html",
        "gfm-raw_html",
        "<div id=\"x\"><p><span class=\"x\">hello</span> <u>under</u> <sup>up</sup> <sub>down</sub> <span class=\"smallcaps\"><a href=\"/Mixed\">label</a></span> <custom-tag>raw</custom-tag></p></div>",
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap();
    assert_eq!(output, "hello *under* ^(up) _(down) [LABEL](/Mixed) raw");
}

#[cfg(all(feature = "read-json", feature = "write-json"))]
#[test]
fn json_round_trips() {
    let input = r#"{"pandoc-api-version":[1,23,1,2],"meta":{},"blocks":[{"t":"Para","c":[{"t":"Str","c":"hi"}]}]}"#;
    let output = convert_text(
        "json",
        "json",
        input,
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap();
    assert_eq!(output, input);
}

#[test]
fn unknown_input_format_is_unsupported() {
    let error = reader_for("notaformat").err().expect("expected an error");
    assert!(matches!(error, Error::UnsupportedFormat(_)), "{error:?}");
}

#[test]
fn unknown_output_format_is_unsupported() {
    let error = writer_for("notaformat").err().expect("expected an error");
    assert!(matches!(error, Error::UnsupportedFormat(_)), "{error:?}");
}

#[cfg(not(feature = "read-json"))]
#[test]
fn recognized_but_disabled_input_is_not_enabled() {
    let error = reader_for("json").err().expect("expected an error");
    assert!(matches!(error, Error::FormatNotEnabled(_)), "{error:?}");
}

#[cfg(feature = "read-commonmark")]
#[test]
fn supported_input_formats_reflect_build() {
    assert!(carta::supported_input_formats().contains(&"commonmark"));
}

#[test]
fn every_supported_format_resolves() {
    // Kind-agnostic: `writer_for` rejects byte-shaped formats, so it cannot stand in here.
    for name in carta::supported_input_formats() {
        assert!(
            any_reader_for(name).is_ok(),
            "input format {name} does not resolve"
        );
    }
    for name in carta::supported_output_formats() {
        assert!(
            any_writer_for(name).is_ok(),
            "output format {name} does not resolve"
        );
    }
}

#[test]
fn format_names_are_sorted_and_every_name_resolves() {
    let inputs = carta::input_format_names();
    let mut sorted = inputs.clone();
    sorted.sort_unstable();
    assert_eq!(inputs, sorted, "input names are not sorted");
    for name in &inputs {
        assert!(
            any_reader_for(name).is_ok(),
            "input name {name} does not resolve"
        );
    }

    let outputs = carta::output_format_names();
    let mut sorted = outputs.clone();
    sorted.sort_unstable();
    assert_eq!(outputs, sorted, "output names are not sorted");
    for name in &outputs {
        assert!(
            any_writer_for(name).is_ok(),
            "output name {name} does not resolve"
        );
    }
}

#[cfg(feature = "read-commonmark")]
#[test]
fn format_names_include_aliases() {
    let inputs = carta::input_format_names();
    for alias in ["commonmark", "commonmark_x", "gfm", "markdown"] {
        assert!(inputs.contains(&alias), "missing input alias {alias}");
    }
}

#[test]
fn format_extensions_default_is_the_markdown_dialect() {
    let entries = carta::format_extensions(None).unwrap();
    let enabled = |name: &str| {
        entries
            .iter()
            .find(|(ext, _)| ext.name() == name)
            .map(|(_, on)| *on)
    };
    assert_eq!(enabled("smart"), Some(true));
    assert_eq!(enabled("gfm_auto_identifiers"), Some(false));

    let names: Vec<&str> = entries.iter().map(|(ext, _)| ext.name()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
}

#[test]
fn format_extensions_honor_a_format_spec() {
    let entries = carta::format_extensions(Some("commonmark+strikeout")).unwrap();
    let enabled = |name: &str| entries.iter().any(|(ext, on)| ext.name() == name && *on);
    assert!(enabled("raw_html"));
    assert!(enabled("strikeout"));
    assert!(!entries.iter().any(|(ext, on)| ext.name() == "smart" && *on));
}

#[test]
fn format_extensions_reject_an_unknown_format() {
    let error = carta::format_extensions(Some("bogus")).unwrap_err();
    assert!(matches!(error, Error::UnsupportedFormat(name) if name == "bogus"));
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "write-markdown",
    feature = "write-plain"
))]
#[test]
fn four_column_list_output_keeps_long_markers_and_continuation_blocks() {
    let source = "- first\n\n  next\n\n1000. a b c d e f g h i j k l m n o p\n\n      next\n\nTerm\n\n: first\n\n  next\n";
    let expected = "-   first\n\n    next\n\n1000. a b c d e f g\n    h i j k l m n o\n    p\n\n    next\n\nTerm\n\n:   first\n\n    next";
    let mut options = WriterOptions::default();
    options.columns = Some(20);
    for format in ["markdown+four_space_rule", "plain+four_space_rule"] {
        let output = convert_text(
            "markdown",
            format,
            source,
            &ReaderOptions::default(),
            &options,
        )
        .unwrap();
        let expected = if format.starts_with("plain") {
            expected.replace(":   first", "    first")
        } else {
            expected.to_owned()
        };
        assert_eq!(output, expected, "{format}");
        if format.starts_with("markdown") {
            let (mut original, _) =
                carta::read_document("markdown", source.as_bytes(), &ReaderOptions::default())
                    .unwrap();
            let (mut restored, _) =
                carta::read_document(format, output.as_bytes(), &ReaderOptions::default()).unwrap();
            for document in [&mut original, &mut restored] {
                carta::walk::for_each_document_inline(document, &mut |inline| {
                    if matches!(inline, carta::ast::Inline::SoftBreak) {
                        *inline = carta::ast::Inline::Space;
                    }
                });
            }
            assert_eq!(restored, original);
        }
    }
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "write-markdown",
    feature = "write-plain"
))]
#[test]
fn four_column_lists_account_for_quote_and_task_prefixes() {
    let quoted = "1000. > a b c d e f g h i j k l m n o p\n";
    let task = "- [x] a b c d e f g h i j k l m n o p\n";
    let mut options = WriterOptions::default();
    options.columns = Some(12);
    for (format, source, expected) in [
        (
            "markdown+four_space_rule",
            quoted,
            "1000. > a b\n    > c d e\n    > f g h\n    > i j k\n    > l m n\n    > o p",
        ),
        (
            "plain+four_space_rule",
            quoted,
            "1000.   a b\n      c d e\n      f g h\n      i j k\n      l m n\n      o p",
        ),
        (
            "markdown+four_space_rule",
            task,
            "-   [x] a b\n    c d e f\n    g h i j\n    k l m n\n    o p",
        ),
        (
            "markdown-four_space_rule",
            task,
            "- [x] a b c\n  d e f g h\n  i j k l m\n  n o p",
        ),
    ] {
        assert_eq!(
            convert_text(
                "markdown",
                format,
                source,
                &ReaderOptions::default(),
                &options
            )
            .unwrap(),
            expected,
            "{format} {source}",
        );
    }
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "write-markdown",
    feature = "write-gfm"
))]
#[test]
fn wrapped_footnotes_keep_all_text_inside_the_definition() {
    let source = "body[^a]\n\n[^a]: a b c d e f g h i j k l m n o p\n\n    second paragraph\n";
    let mut options = WriterOptions::default();
    options.columns = Some(12);
    for format in ["markdown", "gfm"] {
        let output = convert_text(
            "markdown",
            format,
            source,
            &ReaderOptions::default(),
            &options,
        )
        .unwrap();
        assert_eq!(
            output,
            "body[^1]\n\n[^1]: a b c\n    d e f g\n    h i j k\n    l m n o\n    p\n\n    second\n    paragraph"
        );
        let (restored, _) =
            carta::read_document("markdown", output.as_bytes(), &ReaderOptions::default()).unwrap();
        assert!(matches!(
            restored.blocks.as_slice(),
            [carta::ast::Block::Para(_)]
        ));
    }
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "read-rst",
    feature = "read-latex"
))]
#[test]
fn literate_readers_keep_program_text_and_language_classes() {
    use carta::ast::{Attr, Block};
    let expected = vec![Block::CodeBlock(
        Box::new(Attr {
            classes: vec!["haskell".into(), "literate".into()],
            ..Attr::default()
        }),
        "square x = x * x\nmain = print (square 3)".into(),
    )];
    for (format, source) in [
        (
            "markdown",
            "> square x = x * x\n> main = print (square 3)\n",
        ),
        ("rst", "> square x = x * x\n> main = print (square 3)\n"),
        (
            "latex",
            "\\begin{code}\nsquare x = x * x\nmain = print (square 3)\n\\end{code}\n",
        ),
    ] {
        for extension in [carta::Extension::LiterateHaskell.name(), "lhs"] {
            let format = format!("{format}+{extension}");
            let (document, _) =
                carta::read_document(&format, source.as_bytes(), &ReaderOptions::default())
                    .unwrap();
            assert_eq!(document.blocks, expected, "{format}");
        }
    }
}

#[cfg(feature = "read-commonmark")]
#[test]
fn literate_markdown_keeps_quotes_prose_and_equals_headings_distinct() {
    use carta::ast::{Block, to_plain_text};
    let source = "# prose\n\n= Heading =\n\n > quote\n\n> code\n\ntext\n> continuation\n";
    let (document, _) =
        carta::read_document("markdown+lhs", source.as_bytes(), &ReaderOptions::default()).unwrap();
    let [
        Block::Para(prose),
        Block::Header(1, _, title),
        Block::BlockQuote(_),
        Block::CodeBlock(_, code),
        Block::Para(last),
    ] = document.blocks.as_slice()
    else {
        panic!("unexpected blocks: {:?}", document.blocks);
    };
    assert_eq!(to_plain_text(prose), "# prose");
    assert_eq!(to_plain_text(title), "Heading");
    assert_eq!(code, "code");
    assert_eq!(to_plain_text(last), "text > continuation");
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "read-rst",
    feature = "read-latex"
))]
#[test]
fn literate_code_keeps_blank_lines_and_trailing_spaces() {
    use carta::ast::Block;
    for (format, source) in [
        ("markdown+lhs", ">\n> a  \n>\n"),
        ("rst+lhs", ">\n> a  \n>\n"),
        ("rst+lhs", "\\begin{code}\n\na  \n\n\\end{code}\n"),
        ("latex+lhs", "\\begin{code}\n\na  \n\n\\end{code}\n"),
    ] {
        let (document, _) =
            carta::read_document(format, source.as_bytes(), &ReaderOptions::default()).unwrap();
        let [Block::CodeBlock(_, text)] = document.blocks.as_slice() else {
            panic!("expected code: {:?}", document.blocks);
        };
        assert_eq!(text, "\na  \n", "{format}");
    }
}

#[cfg(feature = "read-rst")]
#[test]
fn literate_code_keeps_list_paragraphs_separate() {
    use carta::ast::Block;
    let (document, _) = carta::read_document(
        "rst+lhs",
        b"- item\n\n  > code\n",
        &ReaderOptions::default(),
    )
    .unwrap();
    let [Block::BulletList(items)] = document.blocks.as_slice() else {
        panic!("expected a list: {:?}", document.blocks);
    };
    assert!(matches!(
        items[0].as_slice(),
        [Block::Para(_), Block::CodeBlock(_, _)]
    ));
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "read-rst",
    feature = "read-latex",
    feature = "write-markdown",
    feature = "write-rst",
    feature = "write-latex",
    feature = "write-plain"
))]
#[test]
fn literate_writers_keep_empty_code_and_terminal_blank_lines() {
    use carta::ast::{Attr, Block, Document};
    for text in ["", "x = 1", "x\n", "\nx\n\n"] {
        let document = Document {
            blocks: vec![Block::CodeBlock(
                Box::new(Attr {
                    classes: vec!["haskell".into(), "literate".into()],
                    ..Attr::default()
                }),
                text.into(),
            )],
            ..Document::default()
        };
        for (target, reader) in [
            ("markdown+lhs", "markdown+lhs"),
            ("rst+lhs", "rst+lhs"),
            ("latex+lhs", "latex+lhs"),
            ("plain+lhs", "markdown+lhs"),
        ] {
            let Output::Text(output) = carta::render_document(
                target,
                document.clone(),
                Default::default(),
                &WriterOptions::default(),
            )
            .unwrap() else {
                panic!("expected text")
            };
            let (restored, _) =
                carta::read_document(reader, output.as_bytes(), &ReaderOptions::default()).unwrap();
            assert_eq!(restored, document, "{target} {text:?}");
        }
    }
}

#[cfg(all(feature = "read-commonmark", feature = "write-markdown"))]
#[test]
fn literate_headings_keep_levels_equals_signs_and_link_targets() {
    use carta::ast::{Attr, Block, Document, Inline, Target, to_plain_text};
    let document = Document {
        blocks: vec![
            Block::Header(
                1,
                Box::new(Attr {
                    id: "heading".into(),
                    ..Attr::default()
                }),
                vec![Inline::Str("Heading=".into())],
            ),
            Block::Header(
                3,
                Box::new(Attr {
                    id: "third".into(),
                    ..Attr::default()
                }),
                vec![Inline::Str("Third".into())],
            ),
            Block::Para(vec![
                Inline::Str("= prose".into()),
                Inline::Space,
                Inline::Code(Box::default(), "=".into()),
                Inline::Space,
                Inline::Link(
                    Box::default(),
                    vec![Inline::Str("go".into())],
                    Box::new(Target {
                        url: "#heading".into(),
                        title: Default::default(),
                    }),
                ),
            ]),
        ],
        ..Document::default()
    };
    let Output::Text(output) = carta::render_document(
        "markdown+lhs",
        document,
        Default::default(),
        &WriterOptions::default(),
    )
    .unwrap() else {
        panic!("expected text")
    };
    assert!(output.starts_with("= Heading= =\n\n=== Third ==="));
    let (restored, _) =
        carta::read_document("markdown+lhs", output.as_bytes(), &ReaderOptions::default()).unwrap();
    let [
        Block::Header(1, first, title),
        Block::Header(3, _, _),
        Block::Para(prose),
    ] = restored.blocks.as_slice()
    else {
        panic!("unexpected blocks: {:?}", restored.blocks)
    };
    assert_eq!(first.id, "heading");
    assert_eq!(to_plain_text(title), "Heading=");
    assert_eq!(to_plain_text(prose), "= prose = go");
    assert!(
        prose
            .iter()
            .any(|inline| matches!(inline, Inline::Code(_, text) if text == "="))
    );
    assert!(
        prose
            .iter()
            .any(|inline| matches!(inline, Inline::Link(_, _, target) if target.url == "#heading"))
    );
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "read-rst",
    feature = "write-markdown",
    feature = "write-rst"
))]
#[test]
fn literate_code_output_keeps_identifiers_and_rich_markdown_attributes() {
    use carta::ast::{Attr, Block, Document};
    for target in ["markdown+lhs", "rst+lhs"] {
        let attributes = if target.starts_with("markdown") {
            vec![("key".into(), "value".into())]
        } else {
            Vec::new()
        };
        let document = Document {
            blocks: vec![Block::CodeBlock(
                Box::new(Attr {
                    id: "program".into(),
                    classes: vec!["haskell".into(), "literate".into()],
                    attributes,
                }),
                "x = 1\n".into(),
            )],
            ..Document::default()
        };
        let Output::Text(output) = carta::render_document(
            target,
            document.clone(),
            Default::default(),
            &WriterOptions::default(),
        )
        .unwrap() else {
            panic!("expected text")
        };
        let (restored, _) =
            carta::read_document(target, output.as_bytes(), &ReaderOptions::default()).unwrap();
        if target.starts_with("rst") {
            let [Block::Div(anchor, blocks)] = restored.blocks.as_slice() else {
                panic!("missing target: {restored:?}")
            };
            assert_eq!(anchor.id, "program");
            assert!(matches!(blocks.as_slice(), [Block::CodeBlock(_, text)] if text == "x = 1\n"));
        } else {
            assert_eq!(restored, document, "{target} {output}");
        }
    }
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "write-markdown",
    feature = "write-plain"
))]
#[test]
fn literate_quotes_keep_nested_code_intact() {
    use carta::ast::{Attr, Block, Document};
    let document = Document {
        blocks: vec![Block::BlockQuote(vec![Block::CodeBlock(
            Box::new(Attr {
                classes: vec!["haskell".into(), "literate".into()],
                ..Attr::default()
            }),
            "x = 1\ny = 2".into(),
        )])],
        ..Document::default()
    };
    for target in ["markdown+lhs", "plain+lhs"] {
        let Output::Text(output) = carta::render_document(
            target,
            document.clone(),
            Default::default(),
            &WriterOptions::default(),
        )
        .unwrap() else {
            panic!("expected text")
        };
        let (restored, _) =
            carta::read_document("markdown+lhs", output.as_bytes(), &ReaderOptions::default())
                .unwrap();
        assert_eq!(restored, document, "{target} {output}");
    }
}

#[cfg(all(feature = "read-latex", feature = "write-latex"))]
#[test]
fn code_in_tex_lists_keeps_program_indentation() {
    use carta::ast::{Attr, Block, Document, Inline};
    for literal in [false, true] {
        let classes = if literal {
            vec!["haskell".into(), "literate".into()]
        } else {
            Vec::new()
        };
        let document = Document {
            blocks: vec![Block::BulletList(vec![vec![
                Block::Para(vec![Inline::Str("text".into())]),
                Block::CodeBlock(
                    Box::new(Attr {
                        classes,
                        ..Attr::default()
                    }),
                    "x = 1\n  y = 2".into(),
                ),
            ]])],
            ..Document::default()
        };
        let Output::Text(output) = carta::render_document(
            "latex+lhs",
            document,
            Default::default(),
            &WriterOptions::default(),
        )
        .unwrap() else {
            panic!("expected text")
        };
        let (mut restored, _) =
            carta::read_document("latex+lhs", output.as_bytes(), &ReaderOptions::default())
                .unwrap();
        let mut code = Vec::new();
        carta::walk::for_each_block(&mut restored.blocks, &mut |block| {
            if let Block::CodeBlock(_, text) = block {
                code.push(text.clone());
            }
        });
        assert_eq!(code, ["x = 1\n  y = 2"], "{output}");
    }
}
