use super::yaml::{yaml_inline_scalar, yaml_needs_quoting};

#[test]
fn deeply_nested_tables_render_without_compounding_measurement() {
    // Without the nesting cap, column measurement compounds exponentially in table depth.
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

    // Deep enough that compounding measurement would take minutes; capped stays under a second.
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

    // Plain text, bare-valid interior punctuation, numbers, and non-keyword words stay unquoted.
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

    // The fence outgrows any backtick run in the body, so a ``` line inside cannot close it early.
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
    fn a_leading_ordered_marker_is_escaped_only_when_a_list_would_open() {
        // A digit run then `.`/`)` followed by a space or the line end would open a list.
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
        // No following break: the token cannot start a list and stays bare.
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
        // A plain block (e.g. a tight list item) is at the same risk.
        assert_eq!(
            render(vec![Block::Plain(vec![s("-"), Inline::Space, s("x")])]),
            "\\- x"
        );
    }

    #[test]
    fn a_marker_past_the_first_token_is_left_alone() {
        // Only the opening token can start a list; a marker on a wrapped continuation cannot.
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
        // With subscript off, only the strikeout-opening tilde of each pair is escaped.
        assert_eq!(
            render_with(
                vec![Block::Para(vec![s("~~foo~~")])],
                without(Extension::Subscript)
            ),
            "\\~~foo\\~~"
        );
        // With strikeout also off, the tildes are literal.
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
        // An odd trailing backslash run doubles its last member, never ending on a stray escape.
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
        // An interior backslash is emitted verbatim.
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
        // With no math surface, a display expression linearizes to markup set off by line breaks.
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
