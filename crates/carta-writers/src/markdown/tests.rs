use super::yaml::{yaml_inline_scalar, yaml_needs_quoting};

#[test]
fn commonmark_x_keeps_wiki_images_with_spaced_filenames() {
    use carta_ast::{Attr, Block, Document, Inline, Target};
    use carta_core::{Extension, Writer, WriterOptions, presets};
    let document = Document {
        blocks: vec![Block::Para(vec![Inline::Image(
            Box::new(Attr {
                classes: vec!["wikilink".into()],
                ..Attr::default()
            }),
            vec![Inline::Str("caption".into())],
            Box::new(Target {
                url: "page name.png".into(),
                title: "".into(),
            }),
        )])],
        ..Document::default()
    };
    let mut options = WriterOptions::default();
    options.extensions = presets::COMMONMARK_X;
    options
        .extensions
        .insert(Extension::WikilinksTitleAfterPipe);
    assert_eq!(
        super::CommonmarkXWriter.write(&document, &options).unwrap(),
        "<img src=\"page name.png\" class=\"wikilink\" alt=\"caption\" />"
    );
}

#[test]
fn deeply_nested_tables_render_without_compounding_measurement() {
    use super::MarkdownWriter;
    use carta_ast::{
        Alignment, Attr, Block, Cell, ColSpec, ColWidth, Document, Inline, Row, Table, TableBody,
    };
    use carta_core::{Writer, WriterOptions};

    fn nested_table(content: Vec<Block>) -> Block {
        let cell = Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content,
        };
        let filler = Cell {
            content: vec![Block::Para(vec![Inline::Str("cell".into())])],
            ..cell.clone()
        };
        let spec = ColSpec {
            align: Alignment::AlignDefault,
            width: ColWidth::ColWidthDefault,
        };
        Block::Table(Box::new(Table {
            col_specs: vec![spec.clone(), spec],
            bodies: vec![TableBody {
                body: vec![Row {
                    attr: Attr::default(),
                    cells: vec![cell, filler],
                }],
                ..TableBody::default()
            }],
            ..Table::default()
        }))
    }

    let mut block = Block::Para(vec![Inline::Str("innermost".into())]);
    for _ in 0..9 {
        block = nested_table(vec![block]);
    }
    let doc = Document {
        blocks: vec![block],
        ..Document::default()
    };
    MarkdownWriter
        .write(&doc, &WriterOptions::default())
        .expect("write");
}

#[test]
fn table_measurement_stays_within_the_fill_column() {
    use super::MarkdownWriter;
    use carta_ast::{
        Alignment, Attr, Block, Cell, ColSpec, ColWidth, Document, Inline, Row, Table, TableBody,
    };
    use carta_core::{Writer, WriterOptions};

    fn table(width: ColWidth, content: Vec<Block>) -> Block {
        Block::Table(Box::new(Table {
            col_specs: vec![ColSpec {
                align: Alignment::AlignDefault,
                width,
            }],
            bodies: vec![TableBody {
                body: vec![Row {
                    attr: Attr::default(),
                    cells: vec![Cell {
                        attr: Attr::default(),
                        align: Alignment::AlignDefault,
                        row_span: 1,
                        col_span: 1,
                        content,
                    }],
                }],
                ..TableBody::default()
            }],
            ..Table::default()
        }))
    }

    // Check oversized fractions against the fill width during nested measurement.
    let inner = table(
        ColWidth::ColWidth(1e300),
        vec![Block::Para(vec![Inline::Str("inner".into())])],
    );
    let doc = Document {
        blocks: vec![table(
            ColWidth::ColWidthDefault,
            vec![inner, Block::HorizontalRule],
        )],
        ..Document::default()
    };
    let mut options = WriterOptions::default();
    options.columns = Some(40);
    let rendered = MarkdownWriter.write(&doc, &options).expect("write");
    // Nested borders can exceed the fill width; the measuring width must not determine output.
    let widest = rendered.lines().map(str::len).max().unwrap_or(0);
    assert!(widest < 80, "widest line was {widest} columns");
}

#[test]
fn yaml_quotes_only_scalars_that_would_reparse_wrongly() {
    // Colon, ` #`, leading indicator, surrounding space, emptiness, and bool/null keywords force quoting.
    for forced in [
        "Chapter 1: The Beginning",
        "a:b",
        "ends:",
        "http://example.com",
        "has #comment",
        "-leading",
        "#leading",
        "@leading",
        "!leading",
        "%leading",
        " leading",
        "trailing ",
        "",
        "true",
        "False",
        "NULL",
        "yes",
        "off",
    ] {
        assert!(
            yaml_needs_quoting(forced),
            "expected quoting for {forced:?}"
        );
    }

    for bare in [
        "plain words",
        "interior-dash here",
        "comma,here",
        "has \" quote",
        "back\\slash",
        "123",
        "1.5",
        "None",
        "under_score",
    ] {
        assert!(
            !yaml_needs_quoting(bare),
            "expected no quoting for {bare:?}"
        );
    }
}

#[test]
fn yaml_quote_escapes_backslash_and_quote() {
    assert_eq!(yaml_inline_scalar("a: b"), "\"a: b\"");
    assert_eq!(yaml_inline_scalar("a \" b"), "a \" b");
    assert_eq!(yaml_inline_scalar(": x\\y"), "\": x\\\\y\"");
    assert_eq!(yaml_inline_scalar("plain"), "plain");
}

mod columns {
    use carta_ast::{Block, Document, Inline};
    use carta_core::{Writer, WriterOptions};

    use crate::markdown::MarkdownWriter;

    fn long_paragraph() -> Vec<Block> {
        let words: Vec<Inline> =
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu"
                .split(' ')
                .flat_map(|word| [Inline::Str(word.to_owned().into()), Inline::Space])
                .collect();
        vec![Block::Para(words)]
    }

    fn render(blocks: Vec<Block>, columns: Option<usize>) -> String {
        let document = Document {
            blocks,
            ..Document::default()
        };
        let mut options = WriterOptions::default();
        options.columns = columns;
        MarkdownWriter.write(&document, &options).unwrap()
    }

    #[test]
    fn absurd_column_width_stays_bounded() {
        use carta_ast::{Alignment, Attr, Caption, ColSpec, ColWidth, Table, TableFoot, TableHead};
        let table = Table {
            attr: Attr::default(),
            caption: Caption::default(),
            col_specs: vec![ColSpec {
                align: Alignment::AlignCenter,
                width: ColWidth::ColWidth(3.14e99),
            }],
            head: TableHead::default(),
            bodies: Vec::new(),
            foot: TableFoot::default(),
        };
        // A fractional spec far past the whole line must not inflate the rule allocation.
        let output = render(vec![Block::Table(Box::new(table))], None);
        assert!(
            output.len() < 1_000,
            "unbounded table output: {} bytes",
            output.len()
        );
    }

    #[test]
    fn narrow_columns_wraps_a_paragraph_sooner() {
        let narrow = render(long_paragraph(), Some(20));
        let wide = render(long_paragraph(), Some(60));
        assert!(narrow.lines().count() > wide.lines().count());
        assert!(narrow.lines().all(|line| line.chars().count() <= 20));
        assert!(wide.lines().all(|line| line.chars().count() <= 60));
    }

    #[test]
    fn omitted_columns_uses_the_default_fill_width() {
        assert_eq!(
            render(long_paragraph(), None),
            render(long_paragraph(), Some(72))
        );
    }
}

mod raw_blocks {
    use carta_ast::{Block, Document, Format};
    use carta_core::{Writer, WriterOptions};

    use crate::markdown::MarkdownWriter;

    #[test]
    fn raw_attribute_fence_outgrows_a_backtick_run_in_the_body() {
        let document = Document {
            blocks: vec![Block::RawBlock(
                Format("dot".to_owned().into()),
                "```\ngraph {}\n```".to_owned().into(),
            )],
            ..Document::default()
        };
        let output = MarkdownWriter
            .write(&document, &WriterOptions::default())
            .unwrap();
        assert_eq!(output, "````{=dot}\n```\ngraph {}\n```\n````");
    }
}

mod escaping {
    use carta_ast::{Block, Document, Inline, MathType};
    use carta_core::{Extension, Extensions, Writer, WriterOptions, presets};

    use crate::markdown::MarkdownWriter;

    fn s(text: &str) -> Inline {
        Inline::Str(text.to_owned().into())
    }

    fn render(blocks: Vec<Block>) -> String {
        render_with(blocks, presets::MARKDOWN)
    }

    fn render_with(blocks: Vec<Block>, extensions: Extensions) -> String {
        let document = Document {
            blocks,
            ..Document::default()
        };
        let mut options = WriterOptions::default();
        options.extensions = extensions;
        MarkdownWriter.write(&document, &options).unwrap()
    }

    fn without(ext: Extension) -> Extensions {
        let mut extensions = presets::MARKDOWN;
        extensions.remove(ext);
        extensions
    }

    #[test]
    fn legacy_dashes_use_the_selected_smart_spelling() {
        let mut extensions = presets::MARKDOWN;
        extensions.insert(Extension::OldDashes);
        let paragraph = || vec![Block::Para(vec![s("x — y – 2 1–2")])];
        assert_eq!(render_with(paragraph(), extensions), "x -- y – 2 1-2");
        extensions.remove(Extension::Smart);
        assert_eq!(render_with(paragraph(), extensions), "x — y – 2 1–2");
        assert_eq!(render(paragraph()), "x --- y -- 2 1--2");
    }

    #[test]
    fn a_leading_ordered_marker_is_escaped_only_when_a_list_would_open() {
        assert_eq!(
            render(vec![Block::Para(vec![s("1."), Inline::Space, s("Item")])]),
            "1\\. Item"
        );
        assert_eq!(
            render(vec![Block::Para(vec![s("1)"), Inline::Space, s("Item")])]),
            "1\\) Item"
        );
        assert_eq!(
            render(vec![Block::Para(vec![s("12."), Inline::Space, s("Item")])]),
            "12\\. Item"
        );
        assert_eq!(render(vec![Block::Para(vec![s("1.")])]), "1\\.");
        assert_eq!(render(vec![Block::Para(vec![s("1.Item")])]), "1.Item");
    }

    #[test]
    fn a_leading_bullet_marker_is_escaped() {
        assert_eq!(
            render(vec![Block::Para(vec![s("-"), Inline::Space, s("x")])]),
            "\\- x"
        );
        assert_eq!(
            render(vec![Block::Para(vec![s("+"), Inline::Space, s("x")])]),
            "\\+ x"
        );
        assert_eq!(
            render(vec![Block::Plain(vec![s("-"), Inline::Space, s("x")])]),
            "\\- x"
        );
    }

    #[test]
    fn a_marker_past_the_first_token_is_left_alone() {
        assert_eq!(
            render(vec![Block::Para(vec![
                s("text"),
                Inline::SoftBreak,
                s("-"),
                Inline::Space,
                s("x"),
            ])]),
            "text - x"
        );
    }

    #[test]
    fn a_double_tilde_is_escaped_under_strikeout() {
        assert_eq!(
            render_with(
                vec![Block::Para(vec![s("~~foo~~")])],
                without(Extension::Subscript)
            ),
            "\\~~foo\\~~"
        );
        let mut bare = presets::MARKDOWN;
        bare.remove(Extension::Subscript);
        bare.remove(Extension::Strikeout);
        assert_eq!(
            render_with(vec![Block::Para(vec![s("~~foo~~")])], bare),
            "~~foo~~"
        );
    }

    #[test]
    fn a_trailing_backslash_run_pads_to_an_even_length() {
        let exts = without(Extension::RawTex);
        assert_eq!(
            render_with(vec![Block::Para(vec![s("a\\")])], exts),
            "a\\\\"
        );
        assert_eq!(
            render_with(vec![Block::Para(vec![s("a\\\\")])], exts),
            "a\\\\"
        );
        assert_eq!(
            render_with(vec![Block::Para(vec![s("a\\\\\\")])], exts),
            "a\\\\\\\\"
        );
        assert_eq!(
            render_with(vec![Block::Para(vec![s("a\\b")])], exts),
            "a\\b"
        );
    }

    #[test]
    fn the_backslash_math_surfaces_wrap_the_expression() {
        let single = {
            let mut exts = presets::MARKDOWN;
            exts.remove(Extension::TexMathDollars);
            exts.insert(Extension::TexMathSingleBackslash);
            exts
        };
        assert_eq!(
            render_with(
                vec![Block::Para(vec![Inline::Math(
                    MathType::InlineMath,
                    "x^2".to_owned().into()
                )])],
                single
            ),
            "\\(x^2\\)"
        );
        assert_eq!(
            render_with(
                vec![Block::Para(vec![Inline::Math(
                    MathType::DisplayMath,
                    "x^2".to_owned().into()
                )])],
                single
            ),
            "\\[x^2\\]"
        );
        let double = {
            let mut exts = presets::MARKDOWN;
            exts.remove(Extension::TexMathDollars);
            exts.insert(Extension::TexMathDoubleBackslash);
            exts
        };
        assert_eq!(
            render_with(
                vec![Block::Para(vec![Inline::Math(
                    MathType::InlineMath,
                    "x^2".to_owned().into()
                )])],
                double
            ),
            "\\\\(x^2\\\\)"
        );
    }

    #[test]
    fn an_unwritable_display_math_falls_back_on_its_own_line() {
        assert_eq!(
            render_with(
                vec![Block::Para(vec![
                    s("before"),
                    Inline::Space,
                    Inline::Math(MathType::DisplayMath, "x^2".to_owned().into()),
                    Inline::Space,
                    s("after"),
                ])],
                without(Extension::TexMathDollars),
            ),
            "before\n*x*^2^\nafter"
        );
    }
}
