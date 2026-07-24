use carta_ast::{Attr, Block, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, Target};

use super::inline::scan_budget;
use super::links::parse_link;
use super::*;

fn blocks(input: &str) -> Vec<Block> {
    JiraReader
        .read(input, &ReaderOptions::default())
        .expect("jira reader should not fail")
        .blocks
}

fn para(input: &str) -> Vec<Inline> {
    match blocks(input).into_iter().next() {
        Some(Block::Para(inlines)) => inlines,
        other => panic!("expected a paragraph, got {other:?}"),
    }
}

fn str_node(text: &str) -> Inline {
    Inline::Str(text.to_string().into())
}

#[test]
fn empty_input_yields_no_blocks() {
    assert!(blocks("").is_empty());
}

#[test]
fn link_budget_does_not_fire_on_genuine_content() {
    // A genuine link must parse identically under the default and an unbounded budget.
    let lead = "See the docs and other notes here, for example: ".repeat(20);
    let chars: Vec<char> = format!("{lead}[Example|https://example.com] end.")
        .chars()
        .collect();
    let open = chars
        .iter()
        .position(|&c| c == '[')
        .expect("input has a link opener");
    let mut default_budget = scan_budget(0, chars.len());
    let mut huge_budget = usize::MAX;
    let with_default = parse_link(&chars, open, chars.len(), 0, &mut default_budget);
    let with_huge = parse_link(&chars, open, chars.len(), 0, &mut huge_budget);
    assert!(
        with_default.is_some(),
        "genuine link must parse under the default budget"
    );
    assert_eq!(with_default, with_huge);
}

#[test]
fn heading_levels() {
    assert_eq!(
        blocks("h2. Title"),
        vec![Block::Header(2, Box::default(), vec![str_node("Title")])]
    );
    // Level seven is not a heading.
    assert!(matches!(blocks("h7. Title").as_slice(), [Block::Para(_)]));
}

#[test]
fn text_effects() {
    assert_eq!(para("*bold*"), vec![Inline::Strong(vec![str_node("bold")])]);
    assert_eq!(para("_em_"), vec![Inline::Emph(vec![str_node("em")])]);
    assert_eq!(
        para("+ins+"),
        vec![Inline::Underline(vec![str_node("ins")])]
    );
    assert_eq!(
        para("^sup^"),
        vec![Inline::Superscript(vec![str_node("sup")])]
    );
    assert_eq!(
        para("~sub~"),
        vec![Inline::Subscript(vec![str_node("sub")])]
    );
}

#[test]
fn nested_effects() {
    assert_eq!(
        para("*_both_*"),
        vec![Inline::Strong(vec![Inline::Emph(vec![str_node("both")])])]
    );
}

#[test]
fn intraword_underscore_is_literal() {
    assert_eq!(para("snake_case_here"), vec![str_node("snake_case_here")]);
}

#[test]
fn monospace_stringifies_inner_markup() {
    assert_eq!(
        para("{{a *b* c}}"),
        vec![Inline::Code(Box::default(), "a b c".to_string().into())]
    );
}

#[test]
fn color_span() {
    assert_eq!(
        para("{color:red}x{color}"),
        vec![Inline::Span(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: Vec::new(),
                attributes: vec![("color".to_string().into(), "red".to_string().into())],
            }),
            vec![str_node("x")],
        )]
    );
}

#[test]
fn color_block_wraps_in_div() {
    let attr = Attr {
        id: carta_ast::Text::default(),
        classes: Vec::new(),
        attributes: vec![("color".to_string().into(), "red".to_string().into())],
    };
    assert_eq!(
        blocks("{color:red}\nstuff\n{color}"),
        vec![Block::Div(
            Box::new(attr),
            vec![Block::Para(vec![Inline::LineBreak, str_node("stuff")])],
        )]
    );
    // A close that is not alone on its line keeps the colour inline.
    assert!(matches!(
        blocks("{color:red}a\nb{color}").as_slice(),
        [Block::Para(_)]
    ));
}

#[test]
fn anchor_span() {
    assert_eq!(
        para("{anchor:foo}bar"),
        vec![
            Inline::Span(
                Box::new(Attr {
                    id: "foo".to_string().into(),
                    classes: Vec::new(),
                    attributes: Vec::new(),
                }),
                Vec::new(),
            ),
            str_node("bar"),
        ]
    );
}

#[test]
fn citation_renders_with_em_dash_prefix() {
    assert_eq!(
        para("??cited??"),
        vec![
            str_node("\u{2014}"),
            Inline::Space,
            Inline::Emph(vec![str_node("cited")]),
        ]
    );
}

#[test]
fn dash_folding() {
    assert_eq!(
        para("a -- b"),
        vec![
            str_node("a"),
            Inline::Space,
            str_node("\u{2013}"),
            Inline::Space,
            str_node("b"),
        ]
    );
    assert_eq!(
        para("a --- b"),
        vec![
            str_node("a"),
            Inline::Space,
            str_node("\u{2014}"),
            Inline::Space,
            str_node("b"),
        ]
    );
}

#[test]
fn strikeout_span() {
    assert_eq!(
        para("-gone-"),
        vec![Inline::Strikeout(vec![str_node("gone")])]
    );
}

#[test]
fn escape_emits_literal() {
    assert_eq!(
        para("\\*not bold\\*"),
        vec![str_node("*not"), Inline::Space, str_node("bold*")]
    );
}

#[test]
fn forced_line_break() {
    assert_eq!(
        para("one\\\\two"),
        vec![str_node("one"), Inline::LineBreak, str_node("two")]
    );
}

#[test]
fn newline_within_paragraph_is_hard_break() {
    assert_eq!(
        para("one\ntwo"),
        vec![str_node("one"), Inline::LineBreak, str_node("two")]
    );
}

#[test]
fn horizontal_rule() {
    assert_eq!(blocks("----"), vec![Block::HorizontalRule]);
}

#[test]
fn blockquote_prefix() {
    assert_eq!(
        blocks("bq. quoted"),
        vec![Block::BlockQuote(vec![Block::Para(vec![str_node(
            "quoted"
        )])])]
    );
}

#[test]
fn link_with_label() {
    assert_eq!(
        para("[home|http://example.com]"),
        vec![Inline::Link(
            Box::default(),
            vec![str_node("home")],
            Box::new(Target {
                url: "http://example.com".to_string().into(),
                title: carta_ast::Text::default(),
            }),
        )]
    );
}

#[test]
fn link_bare_url_label() {
    assert_eq!(
        para("[http://example.com]"),
        vec![Inline::Link(
            Box::default(),
            vec![str_node("http://example.com")],
            Box::new(Target {
                url: "http://example.com".to_string().into(),
                title: carta_ast::Text::default(),
            }),
        )]
    );
}

#[test]
fn attachment_link_carries_class() {
    assert_eq!(
        para("[^file.txt]"),
        vec![Inline::Link(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["attachment".to_string().into()],
                attributes: Vec::new(),
            }),
            vec![str_node("file.txt")],
            Box::new(Target {
                url: "file.txt".to_string().into(),
                title: carta_ast::Text::default(),
            }),
        )]
    );
}

#[test]
fn bare_autolink() {
    assert_eq!(
        para("see http://example.com here"),
        vec![
            str_node("see"),
            Inline::Space,
            Inline::Link(
                Box::default(),
                vec![str_node("http://example.com")],
                Box::new(Target {
                    url: "http://example.com".to_string().into(),
                    title: carta_ast::Text::default(),
                }),
            ),
            Inline::Space,
            str_node("here"),
        ]
    );
}

#[test]
fn image_with_properties() {
    assert_eq!(
        para("!pic.png|align=right, vspace=4!"),
        vec![Inline::Image(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: Vec::new(),
                attributes: vec![
                    ("align".to_string().into(), "right".to_string().into()),
                    ("vspace".to_string().into(), "4".to_string().into()),
                ],
            }),
            Vec::new(),
            Box::new(Target {
                url: "pic.png".to_string().into(),
                title: carta_ast::Text::default(),
            }),
        )]
    );
}

#[test]
fn image_thumbnail() {
    assert_eq!(
        para("!pic.png|thumbnail!"),
        vec![Inline::Image(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["thumbnail".to_string().into()],
                attributes: Vec::new(),
            }),
            Vec::new(),
            Box::new(Target {
                url: "pic.png".to_string().into(),
                title: carta_ast::Text::default(),
            }),
        )]
    );
}

#[test]
fn symbols_and_emoticons() {
    assert_eq!(para("(!)"), vec![str_node("\u{2757}")]);
    assert_eq!(para("(y)"), vec![str_node("\u{1F44D}")]);
    assert_eq!(para(":)"), vec![str_node("\u{1F642}")]);
    // A symbol is recognised even when it abuts a preceding word.
    assert_eq!(para("a(!)"), vec![str_node("a\u{2757}")]);
}

#[test]
fn bullet_list_nesting() {
    assert_eq!(
        blocks("* a\n** b"),
        vec![Block::BulletList(vec![vec![
            Block::Para(vec![str_node("a")]),
            Block::BulletList(vec![vec![Block::Para(vec![str_node("b")])]]),
        ]])]
    );
}

#[test]
fn ordered_list_attributes() {
    assert_eq!(
        blocks("# one\n# two"),
        vec![Block::OrderedList(
            ListAttributes {
                start: 1,
                style: ListNumberStyle::DefaultStyle,
                delim: ListNumberDelim::DefaultDelim,
            },
            vec![
                vec![Block::Para(vec![str_node("one")])],
                vec![Block::Para(vec![str_node("two")])],
            ],
        )]
    );
}

#[test]
fn distinct_markers_split_lists() {
    assert_eq!(
        blocks("* a\n- b"),
        vec![
            Block::BulletList(vec![vec![Block::Para(vec![str_node("a")])]]),
            Block::BulletList(vec![vec![Block::Para(vec![str_node("b")])]]),
        ]
    );
}

#[test]
fn table_header_and_body() {
    let blocks = blocks("||h1||h2||\n|a|b|");
    let table = match blocks.first() {
        Some(Block::Table(table)) => table,
        other => panic!("expected a table, got {other:?}"),
    };
    assert_eq!(table.col_specs.len(), 2);
    assert_eq!(table.head.rows.len(), 1);
    assert_eq!(table.bodies.len(), 1);
    assert_eq!(table.bodies.first().map(|b| b.body.len()), Some(1));
}

#[test]
fn code_block_default_language() {
    assert_eq!(
        blocks("{code}\nint x = 1;\n{code}"),
        vec![Block::CodeBlock(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["java".to_string().into()],
                attributes: Vec::new(),
            }),
            "int x = 1;\n".to_string().into(),
        )]
    );
}

#[test]
fn code_block_named_language() {
    assert_eq!(
        blocks("{code:python}\npass\n{code}"),
        vec![Block::CodeBlock(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["python".to_string().into()],
                attributes: Vec::new(),
            }),
            "pass\n".to_string().into(),
        )]
    );
}

#[test]
fn noformat_has_no_language_class() {
    assert_eq!(
        blocks("{noformat}\nraw\n{noformat}"),
        vec![Block::CodeBlock(Box::default(), "raw\n".to_string().into())]
    );
}

#[test]
fn unterminated_code_block_is_dropped() {
    assert!(blocks("{code}\nno close").is_empty());
}

#[test]
fn quote_macro_holds_blocks() {
    assert_eq!(
        blocks("{quote}\ninside\n{quote}"),
        vec![Block::BlockQuote(vec![Block::Para(vec![str_node(
            "inside"
        )])])]
    );
}

#[test]
fn panel_with_title() {
    assert_eq!(
        blocks("{panel:title=Note}\nbody\n{panel}"),
        vec![Block::Div(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["panel".to_string().into()],
                attributes: Vec::new(),
            }),
            vec![
                Block::Div(
                    Box::new(Attr {
                        id: carta_ast::Text::default(),
                        classes: vec!["panelheader".to_string().into()],
                        attributes: Vec::new(),
                    }),
                    vec![Block::Plain(vec![Inline::Strong(vec![str_node("Note")])])],
                ),
                Block::Para(vec![str_node("body")]),
            ],
        )]
    );
}

#[test]
fn paragraph_separation() {
    assert_eq!(
        blocks("one\n\ntwo"),
        vec![
            Block::Para(vec![str_node("one")]),
            Block::Para(vec![str_node("two")]),
        ]
    );
}

#[test]
fn leading_space_opens_paragraph() {
    assert_eq!(para(" hello"), vec![Inline::Space, str_node("hello")]);
    assert_eq!(
        para("   indented"),
        vec![Inline::Space, str_node("indented")]
    );
}

#[test]
fn backslash_before_non_escapable_stays_literal() {
    assert_eq!(para("a\\1b"), vec![str_node("a\\1b")]);
}

#[test]
fn named_and_decimal_entities_decode_but_hex_does_not() {
    assert_eq!(
        para("&copy; &#169; &#x41;"),
        vec![
            str_node("\u{a9}"),
            Inline::Space,
            str_node("\u{a9}"),
            Inline::Space,
            str_node("&#x41;"),
        ]
    );
}

#[test]
fn empty_color_macro_is_literal() {
    assert_eq!(para("{color:}x"), vec![str_node("{color:}x")]);
}

#[test]
fn four_dash_run_folds_to_hyphen_and_em_dash() {
    assert_eq!(
        para("a ---- b"),
        vec![
            str_node("a"),
            Inline::Space,
            str_node("-\u{2014}"),
            Inline::Space,
            str_node("b"),
        ]
    );
}

#[test]
fn dash_run_at_line_end_stays_literal() {
    assert_eq!(
        para("x --"),
        vec![str_node("x"), Inline::Space, str_node("--")]
    );
}

#[test]
fn repeated_markers_nest_bullet_lists() {
    assert_eq!(
        blocks("*** x"),
        vec![Block::BulletList(vec![vec![Block::BulletList(vec![
            vec![Block::BulletList(vec![vec![Block::Para(vec![str_node(
                "x"
            )])]]),]
        ])]])]
    );
}

#[test]
fn indented_marker_still_opens_list() {
    assert_eq!(
        blocks(" * x"),
        vec![Block::BulletList(vec![vec![Block::Para(vec![str_node(
            "x"
        )])]])]
    );
}

#[test]
fn indented_dash_run_is_paragraph_not_rule() {
    assert_eq!(
        blocks("  ----"),
        vec![Block::Para(vec![Inline::Space, str_node("----")])]
    );
}

#[test]
fn same_marker_nesting_caps_at_two() {
    assert_eq!(
        para("*a**b*"),
        vec![Inline::Strong(vec![str_node("a"), str_node("b")])]
    );
    assert_eq!(
        para("**x**"),
        vec![Inline::Strong(vec![Inline::Strong(vec![str_node("x")])])]
    );
}

#[test]
fn strikeout_nests() {
    assert_eq!(
        para("--x--"),
        vec![Inline::Strikeout(vec![Inline::Strikeout(vec![str_node(
            "x"
        )])])]
    );
}
