use super::escape::escape_text;
use super::*;
use carta_ast::Document;
use carta_ast::{MathType, QuoteType, Target};
use carta_core::Extensions;

#[test]
fn escape_text_position_rules_survive_a_verbatim_prefix() {
    // `.`/`;` escape only first-with-more-text; `first` must go false after a copied run.
    assert_eq!(escape_text(".x", true, false, false), "\\.x");
    assert_eq!(escape_text("ab.x", true, false, false), "ab.x");
    assert_eq!(escape_text(".", true, false, false), ".");
    assert_eq!(escape_text("(ab)", false, false, false), "\\(ab)");
    assert_eq!(escape_text("x(ab)", false, false, false), "x(ab)");
}

#[test]
fn escape_text_run_rules_see_the_previous_character() {
    // `-` and `/` escape only when doubled; the first of a pair follows a verbatim-copied run.
    assert_eq!(escape_text("a--b", true, false, false), "a-\\-b");
    assert_eq!(escape_text("http://a", true, false, false), "http:/\\/a");
    assert_eq!(escape_text("a\u{a0}b", true, false, false), "a~b");
    assert_eq!(escape_text("plain text", true, false, false), "plain text");
}

fn smart_options() -> WriterOptions {
    // Mirrors the format's smart-on default (`default_extensions` in `format_spec`).
    let mut options = WriterOptions::default();
    options.extensions = Extensions::from_list(&[Extension::Smart]);
    options
}

fn render(blocks: Vec<Block>) -> String {
    let document = Document {
        blocks,
        ..Document::default()
    };
    TypstWriter.write(&document, &smart_options()).unwrap()
}

fn para(inlines: Vec<Inline>) -> Block {
    Block::Para(inlines)
}

fn str_inline(text: &str) -> Inline {
    Inline::Str(text.to_owned().into())
}

#[test]
fn empty_document() {
    assert_eq!(render(vec![]), "");
}

#[test]
fn empty_and_nested_sections_keep_block_boundaries() {
    assert_eq!(
        render(vec![Block::Div(Box::default(), Vec::new())]),
        "#block[\n]"
    );
    let inner = Block::Div(Box::default(), vec![para(vec![str_inline("text")])]);
    assert_eq!(
        render(vec![Block::Div(Box::default(), vec![inner])]),
        "#block[\n#block[\ntext\n\n]\n]"
    );
}

#[test]
fn plain_blocks_keep_the_next_block_on_the_following_line() {
    for (next, expected) in [
        (Block::Para(vec![str_inline("next")]), "first\nnext"),
        (
            Block::BulletList(vec![vec![Block::Plain(vec![str_inline("next")])]]),
            "first\n- next",
        ),
    ] {
        let output = render(vec![Block::Plain(vec![str_inline("first")]), next]);
        assert_eq!(output, expected);
    }
}

#[test]
fn code_blocks_preserve_trailing_newlines() {
    for (text, expected) in [
        ("", "```\n```"),
        ("x", "```\nx\n```"),
        ("x\n", "```\nx\n```"),
        ("x\n\n", "```\nx\n\n```"),
    ] {
        assert_eq!(
            render(vec![Block::CodeBlock(Box::default(), text.into())]),
            expected
        );
    }
}

#[test]
fn paragraph_with_emphasis() {
    assert_eq!(
        render(vec![para(vec![
            Inline::Strong(vec![str_inline("bold")]),
            Inline::Space,
            Inline::Emph(vec![str_inline("italic")]),
        ])]),
        "#strong[bold] #emph[italic]"
    );
}

#[test]
fn heading_with_label() {
    assert_eq!(
        render(vec![Block::Header(
            2,
            Box::new(Attr {
                id: "intro".into(),
                ..Attr::default()
            }),
            vec![str_inline("H")],
        )]),
        "== H\n<intro>"
    );
}

#[test]
fn internal_links_use_label_targets() {
    for (url, expected) in [
        ("#section-one", "#link(<section-one>)[go]"),
        ("#has space", "#link(label(\"has space\"))[go]"),
        ("#a<b>", "#link(label(\"a<b>\"))[go]"),
        ("#a\"b", "#link(label(\"a\\\"b\"))[go]"),
        ("#", "#link()[go]"),
    ] {
        assert_eq!(
            render(vec![para(vec![Inline::Link(
                Box::default(),
                vec![str_inline("go")],
                Box::new(Target {
                    url: url.into(),
                    title: "".into(),
                }),
            )])]),
            expected
        );
    }
}

#[test]
fn figures_keep_label_targets() {
    for (id, label) in [
        ("photo", "<photo>"),
        ("has space", "#label(\"has space\")"),
        (" a  b ", "#label(\" a  b \")"),
        ("a<b>", "#label(\"a<b>\")"),
        ("a\"b", "#label(\"a\\\"b\")"),
    ] {
        let figure = Block::Figure(
            Box::new(Attr {
                id: id.into(),
                ..Attr::default()
            }),
            Box::new(Caption {
                short: None,
                long: vec![Block::Plain(vec![str_inline("caption")])],
            }),
            vec![Block::Plain(vec![str_inline("body")])],
        );
        assert_eq!(
            render(vec![figure]),
            format!("#figure([body],\n  caption: [\n    caption\n  ]\n)\n{label}")
        );
    }
}

#[test]
fn embedded_image_sources_use_bytes() {
    for (url, source) in [
        ("data:image/png;base64,YQ==", "bytes((97))"),
        ("data:image/png,ab%20cd", "bytes((97,98,32,99,100))"),
        ("data:image/png,%FF%00", "bytes((239,191,189,0))"),
        ("data:image/svg+xml,%3Csvg/%3E", "bytes(\"<svg/>\")"),
        ("data:missing", "bytes(())"),
        ("data:image/png,%GG", "\"data:image/png,%GG\""),
    ] {
        let image = Inline::Image(
            Box::default(),
            Vec::new(),
            Box::new(Target {
                url: url.into(),
                title: "".into(),
            }),
        );
        assert_eq!(
            render(vec![para(vec![image])]),
            format!("#box(image({source}))")
        );
    }
}

#[test]
fn heading_unnumbered_uses_function() {
    assert_eq!(
        render(vec![Block::Header(
            1,
            Box::new(Attr {
                id: "hidden".into(),
                classes: vec!["unnumbered".into()],
                ..Attr::default()
            }),
            vec![str_inline("Hidden")],
        )]),
        "#heading(level: 1, numbering: none)[Hidden]\n<hidden>"
    );
}

#[test]
fn default_ordered_list_uses_plus() {
    let attrs = ListAttributes {
        start: 1,
        style: ListNumberStyle::Decimal,
        delim: ListNumberDelim::Period,
    };
    let items = vec![vec![Block::Plain(vec![str_inline("a")])]];
    assert_eq!(render(vec![Block::OrderedList(attrs, items)]), "+ a");
}

#[test]
fn loose_bullet_list_separates_items() {
    let items = vec![
        vec![Block::Para(vec![str_inline("a")])],
        vec![Block::Para(vec![str_inline("b")])],
    ];
    assert_eq!(render(vec![Block::BulletList(items)]), "- a\n\n- b");
}

#[test]
fn empty_first_items_keep_the_same_list() {
    for empty in [
        Vec::new(),
        vec![Block::Plain(Vec::new())],
        vec![Block::Para(Vec::new())],
    ] {
        let items = vec![empty, vec![para(vec![str_inline("two")])]];
        assert_eq!(render(vec![Block::BulletList(items.clone())]), "-\n\n- two");
        let attributes = ListAttributes {
            start: 1,
            style: ListNumberStyle::Decimal,
            delim: ListNumberDelim::Period,
        };
        assert_eq!(
            render(vec![Block::OrderedList(attributes, items)]),
            "+\n\n+ two"
        );
    }
}

#[test]
fn inline_code_uses_backticks() {
    assert_eq!(
        render(vec![para(vec![Inline::Code(
            Box::default(),
            "let x = 1;".into()
        )])]),
        "`let x = 1;`"
    );
}

#[test]
fn inline_code_with_backtick_falls_back() {
    assert_eq!(
        render(vec![para(vec![Inline::Code(Box::default(), "a`b".into())])]),
        "#raw(\"a`b\")"
    );
}

#[test]
fn paragraph_wraps_at_fill_column() {
    let words: Vec<Inline> = std::iter::repeat_n(
        [str_inline("word"), Inline::Space]
            .into_iter()
            .collect::<Vec<_>>(),
        15,
    )
    .flatten()
    .chain(std::iter::once(str_inline("end")))
    .collect();
    let rendered = render(vec![para(words)]);
    assert!(rendered.contains('\n'));
    assert!(rendered.lines().all(|line| line.len() <= FILL_COLUMN));
}

fn render_columns(blocks: Vec<Block>, columns: usize) -> String {
    let document = Document {
        blocks,
        ..Document::default()
    };
    let mut options = smart_options();
    options.columns = Some(columns);
    TypstWriter.write(&document, &options).unwrap()
}

#[test]
fn footnotes_share_the_surrounding_line_width() {
    for (body, expected) in [
        (
            vec![para(vec![
                str_inline("alpha"),
                Inline::Space,
                str_inline("beta"),
                Inline::Space,
                str_inline("gamma"),
            ])],
            "A\nnote#footnote[alpha\nbeta gamma] follows.",
        ),
        (
            vec![
                para(vec![str_inline("alpha"), Inline::Space, str_inline("beta")]),
                para(vec![
                    str_inline("gamma"),
                    Inline::Space,
                    str_inline("delta"),
                ]),
            ],
            "A\nnote#footnote[alpha\nbeta\n\ngamma delta]\nfollows.",
        ),
    ] {
        let output = render_columns(
            vec![para(vec![
                str_inline("A"),
                Inline::Space,
                str_inline("note"),
                Inline::Note(body),
                Inline::Space,
                str_inline("follows."),
            ])],
            20,
        );
        assert_eq!(output, expected);
    }
}

#[test]
fn footnote_paragraphs_keep_line_markers_literal() {
    for marker in ["-", "+", "=", "/"] {
        let output = render_columns(
            vec![para(vec![Inline::Note(vec![
                para(vec![str_inline("first")]),
                para(vec![str_inline(marker), Inline::Space, str_inline("word")]),
            ])])],
            20,
        );
        assert_eq!(output, format!("#footnote[first\n\n\\{marker} word]"));
    }
}

#[test]
fn heading_footnotes_keep_paragraphs_unwrapped_and_markers_literal() {
    let output = render_columns(
        vec![Block::Header(
            1,
            Box::default(),
            vec![Inline::Note(vec![
                para(vec![
                    str_inline("alpha"),
                    Inline::Space,
                    str_inline("beta"),
                    Inline::Space,
                    str_inline("gamma"),
                    Inline::Space,
                    str_inline("delta"),
                ]),
                para(vec![str_inline("-"), Inline::Space, str_inline("word")]),
            ])],
        )],
        12,
    );
    assert_eq!(output, "= #footnote[alpha beta gamma delta\n\n\\- word]");
}

#[test]
fn list_markers_reduce_the_available_width() {
    let output = render_columns(
        vec![Block::BulletList(vec![vec![Block::Plain(vec![
            str_inline("alpha"),
            Inline::Space,
            str_inline("beta"),
            Inline::Space,
            str_inline("gamma"),
        ])]])],
        17,
    );
    assert_eq!(output, "- alpha beta\n  gamma");
}

fn many_words() -> Vec<Block> {
    let words: Vec<Inline> = std::iter::repeat_n(
        [str_inline("word"), Inline::Space]
            .into_iter()
            .collect::<Vec<_>>(),
        20,
    )
    .flatten()
    .chain(std::iter::once(str_inline("end")))
    .collect();
    vec![para(words)]
}

#[test]
fn custom_columns_bound_the_filled_width() {
    let narrow = render_columns(many_words(), 25);
    let wide = render_columns(many_words(), 70);
    assert!(narrow.lines().all(|line| line.len() <= 25));
    assert!(wide.lines().all(|line| line.len() <= 70));
    assert!(narrow.lines().count() > wide.lines().count());
}

#[test]
fn omitted_columns_matches_the_default_fill_width() {
    assert_eq!(
        render(many_words()),
        render_columns(many_words(), FILL_COLUMN)
    );
}

#[test]
fn span_label_at_start_anchors_with_zwsp() {
    assert_eq!(
        render(vec![para(vec![Inline::Span(
            Box::new(Attr {
                id: "sid".into(),
                ..Attr::default()
            }),
            vec![str_inline("a")],
        )])]),
        "\u{200b}a<sid>"
    );
}

#[test]
fn span_label_mid_text_has_no_zwsp() {
    assert_eq!(
        render(vec![para(vec![
            str_inline("a"),
            Inline::Space,
            Inline::Span(
                Box::new(Attr {
                    id: "s".into(),
                    ..Attr::default()
                }),
                vec![str_inline("styled")],
            ),
            Inline::Space,
            str_inline("word"),
        ])]),
        "a styled<s> word"
    );
}

#[test]
fn mark_span_highlights() {
    assert_eq!(
        render(vec![para(vec![Inline::Span(
            Box::new(Attr {
                classes: vec!["mark".into()],
                ..Attr::default()
            }),
            vec![str_inline("x")],
        )])]),
        "#highlight[x]"
    );
}

#[test]
fn image_pixel_width_converts_to_inches() {
    assert_eq!(
        render(vec![para(vec![Inline::Image(
            Box::new(Attr {
                attributes: vec![("width".into(), "200".into())],
                ..Attr::default()
            }),
            vec![str_inline("alt")],
            Box::new(Target {
                url: "i.png".into(),
                title: String::new().into(),
            }),
        )])]),
        "#box(image(\"i.png\", width: 2.08333in, alt: \"alt\"))"
    );
}

#[test]
fn markup_escaping() {
    assert_eq!(render(vec![para(vec![str_inline("a*b_c")])]), "a\\*b\\_c");
    assert_eq!(render(vec![para(vec![str_inline("-x")])]), "\\-x");
    assert_eq!(render(vec![para(vec![str_inline("a-b")])]), "a-b");
    assert_eq!(render(vec![para(vec![str_inline("a---b")])]), "a-\\-\\-b");
    assert_eq!(
        render(vec![para(vec![str_inline("http://a")])]),
        "http:/\\/a"
    );
    assert_eq!(render(vec![para(vec![str_inline("a.b")])]), "a.b");
    assert_eq!(render(vec![para(vec![str_inline(".x")])]), "\\.x");
    assert_eq!(render(vec![para(vec![str_inline("(ab)")])]), "\\(ab)");
}

#[test]
fn period_token_alone_is_not_escaped() {
    assert_eq!(
        render(vec![para(vec![
            Inline::Quoted(QuoteType::SingleQuote, vec![str_inline("hi")]),
            str_inline("."),
        ])]),
        "'hi'."
    );
}

#[test]
fn smart_dashes_spelled_out() {
    assert_eq!(
        render(vec![para(vec![str_inline(
            "en\u{2013}dash em\u{2014}dash"
        )])]),
        "en--dash em---dash"
    );
}

#[test]
fn code_block_with_language() {
    assert_eq!(
        render(vec![Block::CodeBlock(
            Box::new(Attr {
                classes: vec!["rust".into()],
                ..Attr::default()
            }),
            "fn x() {}".into(),
        )]),
        "```rust\nfn x() {}\n```"
    );
}

fn inline_math(text: &str) -> String {
    render(vec![para(vec![Inline::Math(
        MathType::InlineMath,
        text.into(),
    )])])
}

fn display_math(text: &str) -> String {
    render(vec![para(vec![Inline::Math(
        MathType::DisplayMath,
        text.into(),
    )])])
}

#[test]
fn inline_math_translates_to_native_markup() {
    assert_eq!(inline_math("a^2 + b^2 = c^2"), "$a^2 + b^2 = c^2$");
    assert_eq!(inline_math("\\alpha + \\beta"), "$alpha + beta$");
    assert_eq!(inline_math("\\frac{1}{2}"), "$1 / 2$");
    assert_eq!(inline_math("\\mathbb{R}"), "$bb(R)$");
}

#[test]
fn display_math_uses_spaced_delimiters() {
    assert_eq!(
        display_math("\\int_0^1 x \\, dx"),
        "$ integral_0^1 x thin d x $"
    );
}

#[test]
fn untranslatable_inline_math_falls_back_to_escaped_verbatim() {
    assert_eq!(inline_math("\\unknowncmd"), "\\$\\\\unknowncmd\\$");
    assert_eq!(
        inline_math("\\foo #h _u *s"),
        "\\$\\\\foo \\#h \\_u \\*s\\$"
    );
}

#[test]
fn untranslatable_display_math_uses_double_dollar_verbatim() {
    assert_eq!(display_math("\\unknowncmd"), "\\$\\$\\\\unknowncmd\\$\\$");
}

#[test]
fn deeply_nested_tables_render_without_compounding_layout() {
    // A cell laid out once per preceding cell would cost 2^24 layouts at this depth.
    use carta_ast::{Alignment, Cell, ColSpec, ColWidth, Row, Table, TableBody};

    // The filler cell has no break point, so the row's glue reaches into the nesting cell.
    fn nested_table(content: Vec<Block>) -> Block {
        let cell = Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content,
        };
        let filler = Cell {
            content: vec![Block::Plain(vec![str_inline("cell")])],
            ..cell.clone()
        };
        let spec = ColSpec {
            align: Alignment::AlignDefault,
            width: ColWidth::ColWidthDefault,
        };
        Block::Table(Box::new(Table {
            col_specs: vec![spec; 2],
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: vec![Row {
                    attr: Attr::default(),
                    cells: vec![filler, cell],
                }],
            }],
            ..Table::default()
        }))
    }

    let mut blocks = vec![para(vec![str_inline("x")])];
    for _ in 0..24 {
        blocks = vec![nested_table(blocks)];
    }
    assert_eq!(render(blocks).matches("#table(").count(), 24);
}
