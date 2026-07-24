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
    // No native form: source kept, TeX delimiters reconstructed, whole run escaped as text.
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
