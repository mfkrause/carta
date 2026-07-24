use super::*;
use carta_ast::{Attr, Cell, Document, ListNumberDelim, ListNumberStyle};

fn render(blocks: Vec<Block>) -> String {
    let document = Document {
        blocks,
        ..Document::default()
    };
    ManWriter
        .write(&document, &WriterOptions::default())
        .unwrap()
}

fn para(inlines: Vec<Inline>) -> Block {
    Block::Para(inlines)
}

fn s(text: &str) -> Inline {
    Inline::Str(text.to_owned().into())
}

#[test]
fn empty_document() {
    assert_eq!(render(vec![]), "");
}

#[test]
fn single_paragraph_gets_pp() {
    assert_eq!(render(vec![para(vec![s("hi")])]), ".PP\nhi");
}

#[test]
fn paragraph_after_header_omits_pp() {
    assert_eq!(
        render(vec![
            Block::Header(1, Box::default(), vec![s("H")]),
            para(vec![s("body")]),
        ]),
        ".SH H\nbody"
    );
}

#[test]
fn header_levels() {
    assert_eq!(
        render(vec![
            Block::Header(1, Box::default(), vec![s("A")]),
            Block::Header(3, Box::default(), vec![s("B")]),
        ]),
        ".SH A\n.SS B"
    );
}

#[test]
fn font_stack_nests() {
    assert_eq!(
        render(vec![para(vec![Inline::Strong(vec![
            s("b"),
            Inline::Emph(vec![s("i")]),
        ])])]),
        ".PP\n\\f[B]b\\f[BI]i\\f[B]\\f[R]"
    );
}

#[test]
fn code_uses_mono_font() {
    assert_eq!(
        render(vec![para(vec![Inline::Code(Box::default(), "x".into())])]),
        ".PP\n\\f[CR]x\\f[R]"
    );
}

#[test]
fn special_characters_escaped() {
    assert_eq!(render(vec![para(vec![s("a~b@c")])]), ".PP\na\\(tib\\(atc");
    assert_eq!(render(vec![para(vec![s("a-b")])]), ".PP\na\\-b");
}

#[test]
fn line_start_dot_is_protected() {
    assert_eq!(render(vec![para(vec![s(".dot")])]), ".PP\n\\&.dot");
}

#[test]
fn bullet_list_items() {
    assert_eq!(
        render(vec![Block::BulletList(vec![
            vec![Block::Plain(vec![s("a")])],
            vec![Block::Plain(vec![s("b")])],
        ])]),
        ".IP \\(bu 2\na\n.IP \\(bu 2\nb"
    );
}

#[test]
fn ordered_list_markers_align() {
    let attrs = ListAttributes {
        start: 1,
        style: ListNumberStyle::Decimal,
        delim: ListNumberDelim::Period,
    };
    let items = vec![
        vec![Block::Plain(vec![s("a")])],
        vec![Block::Plain(vec![s("b")])],
    ];
    assert_eq!(
        render(vec![Block::OrderedList(attrs, items)]),
        ".IP \"1.\" 3\na\n.IP \"2.\" 3\nb"
    );
}

#[test]
fn list_item_continuation_indents() {
    let items = vec![vec![Block::Para(vec![s("a")]), Block::Para(vec![s("b")])]];
    assert_eq!(
        render(vec![Block::BulletList(items)]),
        ".IP \\(bu 2\na\n.RS 2\n.PP\nb\n.RE"
    );
}

#[test]
fn block_quote_wraps_in_rs() {
    assert_eq!(
        render(vec![Block::BlockQuote(vec![para(vec![s("q")])])]),
        ".RS\n.PP\nq\n.RE"
    );
}

#[test]
fn code_block_example_group() {
    assert_eq!(
        render(vec![Block::CodeBlock(Box::default(), "a\nb\n".into())]),
        ".IP\n.EX\na\nb\n.EE"
    );
}

#[test]
fn definition_list() {
    assert_eq!(
        render(vec![Block::DefinitionList(vec![(
            vec![s("T")],
            vec![vec![Block::Plain(vec![s("d")])]],
        )])]),
        ".TP\nT\nd"
    );
}

#[test]
fn footnote_becomes_notes_section() {
    assert_eq!(
        render(vec![para(vec![
            s("a"),
            Inline::Note(vec![Block::Para(vec![s("note")])]),
        ])]),
        ".PP\na[1]\n.SH NOTES\n.SS [1]\n.PP\nnote"
    );
}

#[test]
fn strikeout_and_scripts() {
    assert_eq!(
        render(vec![para(vec![Inline::Strikeout(vec![s("x")])])]),
        ".PP\n[STRIKEOUT:x]"
    );
    assert_eq!(
        render(vec![para(vec![Inline::Superscript(vec![s("2")])])]),
        ".PP\n^2^"
    );
}

#[test]
fn quoted_uses_roff_quotes() {
    assert_eq!(
        render(vec![para(vec![Inline::Quoted(
            QuoteType::DoubleQuote,
            vec![s("hi")],
        )])]),
        ".PP\n\\(lqhi\\(rq"
    );
}

#[test]
fn image_renders_placeholder() {
    assert_eq!(
        render(vec![para(vec![Inline::Image(
            Box::default(),
            vec![],
            Box::new(Target {
                url: "i.png".into(),
                title: String::new().into(),
            }),
        )])]),
        ".PP\n[IMAGE: image]"
    );
}

#[test]
fn horizontal_rule() {
    assert_eq!(
        render(vec![Block::HorizontalRule]),
        ".PP\n   *   *   *   *   *"
    );
}

#[test]
fn simple_table() {
    let cell = |text: &str| Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content: vec![Block::Plain(vec![s(text)])],
    };
    let row = |a: &str, b: &str| Row {
        attr: Attr::default(),
        cells: vec![cell(a), cell(b)],
    };
    let table = Table {
        attr: Attr::default(),
        caption: Caption::default(),
        col_specs: vec![
            carta_ast::ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            },
            carta_ast::ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            },
        ],
        head: carta_ast::TableHead {
            attr: Attr::default(),
            rows: vec![row("A", "B")],
        },
        bodies: vec![carta_ast::TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: vec![],
            body: vec![row("1", "2")],
        }],
        foot: carta_ast::TableFoot {
            attr: Attr::default(),
            rows: vec![],
        },
    };
    assert_eq!(
        render(vec![Block::Table(Box::new(table))]),
        ".PP\n.TS\ntab(@);\nl l.\nT{\nA\nT}@T{\nB\nT}\n_\nT{\n1\nT}@T{\n2\nT}\n.TE"
    );
}

#[test]
fn paragraph_wraps_at_fill_column() {
    let mut inlines = Vec::new();
    for index in 0..30 {
        if index > 0 {
            inlines.push(Inline::Space);
        }
        inlines.push(s("wordwordword"));
    }
    let rendered = render(vec![para(inlines)]);
    for line in rendered.lines().skip(1) {
        assert!(visible_width(line) <= FILL_COLUMN, "line too wide: {line}");
    }
}

fn render_columns(blocks: Vec<Block>, columns: usize) -> String {
    let document = Document {
        blocks,
        ..Document::default()
    };
    let mut options = WriterOptions::default();
    options.columns = Some(columns);
    ManWriter.write(&document, &options).unwrap()
}

fn many_words() -> Vec<Block> {
    let mut inlines = Vec::new();
    for index in 0..30 {
        if index > 0 {
            inlines.push(Inline::Space);
        }
        inlines.push(s("wordwordword"));
    }
    vec![para(inlines)]
}

#[test]
fn custom_columns_bound_the_filled_width() {
    let narrow = render_columns(many_words(), 30);
    let wide = render_columns(many_words(), 80);
    for line in narrow.lines().skip(1) {
        assert!(visible_width(line) <= 30, "line too wide: {line}");
    }
    for line in wide.lines().skip(1) {
        assert!(visible_width(line) <= 80, "line too wide: {line}");
    }
    // The narrower budget needs strictly more physical lines.
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
fn raw_block_man_passes_through_other_dropped() {
    assert_eq!(
        render(vec![Block::RawBlock(Format("man".into()), ".XX\n".into())]),
        ".XX"
    );
    assert_eq!(
        render(vec![
            Block::RawBlock(Format("html".into()), "<div>".into()),
            para(vec![s("y")]),
        ]),
        ".PP\ny"
    );
}

fn inline_math(tex: &str) -> Inline {
    Inline::Math(MathType::InlineMath, tex.to_owned().into())
}

fn display_math(tex: &str) -> Inline {
    Inline::Math(MathType::DisplayMath, tex.to_owned().into())
}

#[test]
fn inline_math_lowers_to_font_and_scripts() {
    // A variable renders in italics; a superscript uses `^..^`.
    assert_eq!(
        render(vec![para(vec![inline_math("a^2")])]),
        ".PP\n\\f[I]a\\f[R]^2^"
    );
}

#[test]
fn inline_math_stays_in_the_filled_flow() {
    assert_eq!(
        render(vec![para(vec![
            s("an"),
            Inline::Space,
            s("equation"),
            Inline::Space,
            inline_math("a^2 + b^2 = c^2"),
            Inline::Space,
            s("inline"),
        ])]),
        ".PP\nan equation \\f[I]a\\f[R]^2^\u{2005}+\u{2005}\\f[I]b\\f[R]^2^\u{2004}=\u{2004}\\f[I]c\\f[R]^2^ inline"
    );
}

#[test]
fn display_math_is_set_off_in_a_relative_indent_group() {
    assert_eq!(
        render(vec![para(vec![display_math("\\int_0^1 x \\, dx")])]),
        ".PP\n.RS\n∫~0~^1^\\f[I]x\\f[R]\u{2006}\\f[I]d\\f[R]\\f[I]x\\f[R]\n.RE"
    );
}

#[test]
fn display_math_resumes_following_text_on_the_close_line() {
    assert_eq!(
        render(vec![para(vec![
            s("before"),
            Inline::Space,
            display_math("a^2"),
            Inline::Space,
            s("after"),
        ])]),
        ".PP\nbefore\n.RS\n\\f[I]a\\f[R]^2^\n.RE after"
    );
}

#[test]
fn nonconvertible_inline_math_falls_back_to_escaped_source() {
    // `\frac` has no single-line form: source emitted between `$` delimiters, roff-escaped
    assert_eq!(
        render(vec![para(vec![inline_math("\\frac{1}{2}")])]),
        ".PP\n$\\(rsfrac{1}{2}$"
    );
}

#[test]
fn nonconvertible_display_math_falls_back_inside_the_group() {
    assert_eq!(
        render(vec![para(vec![display_math("\\frac{1}{2}")])]),
        ".PP\n.RS\n$$\\(rsfrac{1}{2}$$\n.RE"
    );
}

#[test]
fn fallback_source_is_roff_escaped() {
    // Characters with roff meaning in the kept source are escaped: `-` and `^` here.
    assert_eq!(
        render(vec![para(vec![inline_math("\\sqrt{a-b}")])]),
        ".PP\n$\\(rssqrt{a\\-b}$"
    );
}

#[test]
fn empty_inline_math_renders_nothing() {
    assert_eq!(
        render(vec![para(vec![
            s("x"),
            Inline::Space,
            inline_math(""),
            Inline::Space,
            s("y")
        ])]),
        ".PP\nx  y"
    );
}

#[test]
fn empty_display_math_keeps_an_empty_group() {
    assert_eq!(
        render(vec![para(vec![
            s("x"),
            Inline::Space,
            display_math(""),
            Inline::Space,
            s("y"),
        ])]),
        ".PP\nx\n.RS\n.RE y"
    );
}

#[test]
fn math_threads_the_surrounding_font() {
    // A bold variable nested in math keeps the surrounding bold weight on its toggle.
    assert_eq!(
        render(vec![para(vec![Inline::Strong(vec![inline_math("a^2")])])]),
        ".PP\n\\f[B]\\f[BI]a\\f[B]^2^\\f[R]"
    );
}

#[test]
fn display_math_sets_off_inside_an_unwrapped_run() {
    // In a heading (an unwrapped run) the group still takes its own lines.
    assert_eq!(
        render(vec![Block::Header(
            1,
            Box::default(),
            vec![s("T"), Inline::Space, display_math("a^2")],
        )]),
        ".SH T \n.RS\n\\f[I]a\\f[R]^2^\n.RE"
    );
}

fn link(label: Vec<Inline>, url: &str) -> Inline {
    Inline::Link(
        Box::default(),
        label,
        Box::new(Target {
            url: url.into(),
            title: String::new().into(),
        }),
    )
}

#[test]
fn decoded_label_link_drops_the_label() {
    assert_eq!(
        render(vec![para(vec![link(
            vec![s("http://e.com/a b")],
            "http://e.com/a%20b"
        )])]),
        ".PP\n\\c\n.UR http://e.com/a%20b\n.UE \\c"
    );
}

#[test]
fn exact_label_link_drops_the_label() {
    assert_eq!(
        render(vec![para(vec![link(
            vec![s("http://e.com/a%20b")],
            "http://e.com/a%20b"
        )])]),
        ".PP\n\\c\n.UR http://e.com/a%20b\n.UE \\c"
    );
}

#[test]
fn distinct_label_link_keeps_the_label() {
    assert_eq!(
        render(vec![para(vec![link(
            vec![s("click")],
            "http://e.com/a%20b"
        )])]),
        ".PP\n\\c\n.UR http://e.com/a%20b\nclick\n.UE \\c"
    );
}
