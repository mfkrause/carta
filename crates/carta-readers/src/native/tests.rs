use super::*;

fn parse(input: &str) -> Document {
    NativeReader
        .read(input, &ReaderOptions::default())
        .expect("native input should parse")
}

fn parse_err(input: &str) -> String {
    NativeReader
        .read(input, &ReaderOptions::default())
        .expect_err("native input should fail")
        .to_string()
}

fn only_block(input: &str) -> Block {
    let Document { blocks, .. } = parse(input);
    match blocks.into_iter().next() {
        Some(block) => block,
        None => panic!("expected a single block"),
    }
}

fn str_inline(text: &str) -> Inline {
    Inline::Str(text.to_string().into())
}

#[test]
fn parses_full_document_with_meta() {
    let document = parse(
        r#"Pandoc (Meta {unMeta = fromList [("title", MetaInlines [Str "Hi"])]}) [Para [Str "Body"]]"#,
    );
    assert_eq!(
        document.meta.get("title"),
        Some(&MetaValue::MetaInlines(vec![str_inline("Hi")]))
    );
    assert_eq!(document.blocks, vec![Block::Para(vec![str_inline("Body")])]);
}

#[test]
fn parses_every_meta_value_shape() {
    let document = parse(
        r#"Pandoc (Meta {unMeta = fromList [("m", MetaMap (fromList [("k", MetaString "v")])), ("l", MetaList [MetaBool True, MetaBool False]), ("b", MetaBlocks [Plain [Str "p"]])]}) []"#,
    );
    assert_eq!(
        document.meta.get("m"),
        Some(&MetaValue::MetaMap(
            [(
                "k".to_string().into(),
                MetaValue::MetaString("v".to_string().into())
            )]
            .into_iter()
            .collect()
        ))
    );
    assert_eq!(
        document.meta.get("l"),
        Some(&MetaValue::MetaList(vec![
            MetaValue::MetaBool(true),
            MetaValue::MetaBool(false)
        ]))
    );
    assert_eq!(
        document.meta.get("b"),
        Some(&MetaValue::MetaBlocks(vec![Block::Plain(vec![
            str_inline("p")
        ])]))
    );
}

#[test]
fn bare_block_list_is_wrapped_into_document() {
    let document = parse(r#"[Para [Str "a"], HorizontalRule]"#);
    assert_eq!(
        document.blocks,
        vec![Block::Para(vec![str_inline("a")]), Block::HorizontalRule]
    );
}

#[test]
fn empty_list_is_an_empty_document() {
    assert_eq!(parse("[]").blocks, vec![]);
}

#[test]
fn bare_inline_list_becomes_a_plain_block() {
    let document = parse(r#"[Str "a", Space, Str "b"]"#);
    assert_eq!(
        document.blocks,
        vec![Block::Plain(vec![
            str_inline("a"),
            Inline::Space,
            str_inline("b")
        ])]
    );
}

#[test]
fn single_block_is_wrapped() {
    assert_eq!(only_block("HorizontalRule"), Block::HorizontalRule);
}

#[test]
fn single_inline_becomes_a_plain_block() {
    assert_eq!(
        only_block(r#"Str "lonely""#),
        Block::Plain(vec![str_inline("lonely")])
    );
}

#[test]
fn parses_code_block_with_attr() {
    assert_eq!(
        only_block(r#"CodeBlock ("i", ["rust", "numberLines"], [("k", "v")]) "let x = 1;""#),
        Block::CodeBlock(
            Box::new(Attr {
                id: "i".to_string().into(),
                classes: vec!["rust".to_string().into(), "numberLines".to_string().into()],
                attributes: vec![("k".to_string().into(), "v".to_string().into())],
            }),
            "let x = 1;".to_string().into()
        )
    );
}

#[test]
fn parses_raw_block_with_format_in_parens() {
    assert_eq!(
        only_block(r#"RawBlock (Format "html") "<hr>""#),
        Block::RawBlock(Format("html".to_string().into()), "<hr>".to_string().into())
    );
}

#[test]
fn parses_line_block() {
    assert_eq!(
        only_block(r#"LineBlock [[Str "one"], [Str "two"]]"#),
        Block::LineBlock(vec![vec![str_inline("one")], vec![str_inline("two")]])
    );
}

#[test]
fn parses_ordered_list_attributes() {
    assert_eq!(
        only_block(r#"OrderedList (3, UpperRoman, TwoParens) [[Plain [Str "x"]]]"#),
        Block::OrderedList(
            ListAttributes {
                start: 3,
                style: ListNumberStyle::UpperRoman,
                delim: ListNumberDelim::TwoParens,
            },
            vec![vec![Block::Plain(vec![str_inline("x")])]]
        )
    );
}

#[test]
fn parses_definition_list() {
    assert_eq!(
        only_block(r#"DefinitionList [([Str "term"], [[Plain [Str "def"]]])]"#),
        Block::DefinitionList(vec![(
            vec![str_inline("term")],
            vec![vec![Block::Plain(vec![str_inline("def")])]]
        )])
    );
}

#[test]
fn parses_header_with_level_and_attr() {
    assert_eq!(
        only_block(r#"Header 2 ("h", [], []) [Str "Title"]"#),
        Block::Header(
            2,
            Box::new(Attr {
                id: "h".to_string().into(),
                classes: vec![],
                attributes: vec![],
            }),
            vec![str_inline("Title")]
        )
    );
}

#[test]
fn parses_div_and_blockquote() {
    assert_eq!(
        only_block(r#"Div ("d", [], []) [BlockQuote [Para [Str "q"]]]"#),
        Block::Div(
            Box::new(Attr {
                id: "d".to_string().into(),
                classes: vec![],
                attributes: vec![],
            }),
            vec![Block::BlockQuote(vec![Block::Para(vec![str_inline("q")])])]
        )
    );
}

#[test]
fn parses_figure_with_caption() {
    let block = only_block(
        r#"Figure ("f", [], []) (Caption Nothing [Plain [Str "cap"]]) [Para [Str "body"]]"#,
    );
    let Block::Figure(attr, caption, blocks) = block else {
        panic!("expected a figure");
    };
    assert_eq!(attr.id, "f");
    assert_eq!(caption.short, None);
    assert_eq!(caption.long, vec![Block::Plain(vec![str_inline("cap")])]);
    assert_eq!(blocks, vec![Block::Para(vec![str_inline("body")])]);
}

#[test]
fn parses_caption_with_short_inlines() {
    let block =
        only_block(r#"Figure ("", [], []) (Caption (Just [Str "s"]) [Plain [Str "l"]]) []"#);
    let Block::Figure(_, caption, _) = block else {
        panic!("expected a figure");
    };
    assert_eq!(caption.short, Some(vec![str_inline("s")]));
}

#[test]
fn parses_every_inline_constructor() {
    let block = only_block(
        r#"Para [Emph [Str "e"], Underline [Str "u"], Strong [Str "s"], Strikeout [Str "k"], Superscript [Str "p"], Subscript [Str "b"], SmallCaps [Str "c"], Space, SoftBreak, LineBreak]"#,
    );
    assert_eq!(
        block,
        Block::Para(vec![
            Inline::Emph(vec![str_inline("e")]),
            Inline::Underline(vec![str_inline("u")]),
            Inline::Strong(vec![str_inline("s")]),
            Inline::Strikeout(vec![str_inline("k")]),
            Inline::Superscript(vec![str_inline("p")]),
            Inline::Subscript(vec![str_inline("b")]),
            Inline::SmallCaps(vec![str_inline("c")]),
            Inline::Space,
            Inline::SoftBreak,
            Inline::LineBreak,
        ])
    );
}

#[test]
fn parses_quoted_math_and_code_inlines() {
    let block = only_block(
        r#"Para [Quoted DoubleQuote [Str "q"], Math InlineMath "x^2", Code ("", [], []) "f()"]"#,
    );
    assert_eq!(
        block,
        Block::Para(vec![
            Inline::Quoted(QuoteType::DoubleQuote, vec![str_inline("q")]),
            Inline::Math(MathType::InlineMath, "x^2".to_string().into()),
            Inline::Code(Box::default(), "f()".to_string().into()),
        ])
    );
}

#[test]
fn parses_link_image_span_and_note() {
    let block = only_block(
        r#"Para [Link ("", [], []) [Str "t"] ("/u", "ti"), Image ("", [], []) [Str "alt"] ("/i", ""), Span ("sp", [], []) [Str "s"], Note [Para [Str "n"]]]"#,
    );
    assert_eq!(
        block,
        Block::Para(vec![
            Inline::Link(
                Box::default(),
                vec![str_inline("t")],
                Box::new(Target {
                    url: "/u".to_string().into(),
                    title: "ti".to_string().into()
                })
            ),
            Inline::Image(
                Box::default(),
                vec![str_inline("alt")],
                Box::new(Target {
                    url: "/i".to_string().into(),
                    title: carta_ast::Text::default()
                })
            ),
            Inline::Span(
                Box::new(Attr {
                    id: "sp".to_string().into(),
                    classes: vec![],
                    attributes: vec![],
                }),
                vec![str_inline("s")]
            ),
            Inline::Note(vec![Block::Para(vec![str_inline("n")])]),
        ])
    );
}

#[test]
fn parses_raw_inline_with_bare_format() {
    let block = only_block(r#"Para [RawInline (Format "tex") "\\hi"]"#);
    assert_eq!(
        block,
        Block::Para(vec![Inline::RawInline(
            Format("tex".to_string().into()),
            "\\hi".to_string().into()
        )])
    );
}

#[test]
fn parses_cite_with_all_fields() {
    let block = only_block(
        r#"Para [Cite [Citation {citationId = "x", citationPrefix = [Str "see"], citationSuffix = [Str "p1"], citationMode = AuthorInText, citationNoteNum = 2, citationHash = 0}] [Str "[@x]"]]"#,
    );
    let Block::Para(inlines) = block else {
        panic!("expected a paragraph");
    };
    let citation = match inlines.first() {
        Some(Inline::Cite(citations, _)) => citations.first().cloned(),
        _ => None,
    };
    let citation = citation.expect("a citation");
    assert_eq!(citation.id, "x");
    assert_eq!(citation.prefix, vec![str_inline("see")]);
    assert_eq!(citation.suffix, vec![str_inline("p1")]);
    assert_eq!(citation.mode, CitationMode::AuthorInText);
    assert_eq!(citation.note_num, 2);
}

#[test]
fn parses_table_with_head_body_and_foot() {
    let input = r#"Table ("", [], []) (Caption Nothing [])
        [(AlignDefault, ColWidthDefault), (AlignRight, ColWidth 0.5)]
        (TableHead ("", [], []) [Row ("", [], []) [Cell ("", [], []) AlignDefault (RowSpan 1) (ColSpan 1) [Plain [Str "H"]]]])
        [TableBody ("", [], []) (RowHeadColumns 0) [] [Row ("", [], []) [Cell ("", [], []) AlignLeft (RowSpan 1) (ColSpan 1) [Plain [Str "B"]]]]]
        (TableFoot ("", [], []) [])"#;
    let block = only_block(input);
    let Block::Table(table) = block else {
        panic!("expected a table");
    };
    assert_eq!(table.col_specs.len(), 2);
    assert_eq!(
        table.col_specs.last().map(|spec| spec.width.clone()),
        Some(ColWidth::ColWidth(0.5))
    );
    assert_eq!(table.head.rows.len(), 1);
    assert_eq!(table.bodies.len(), 1);
    assert_eq!(table.foot.rows.len(), 0);
}

#[test]
fn decodes_simple_string_escapes() {
    let block = only_block(r#"Para [Str "a\nb\tc\rd\\e\"f"]"#);
    assert_eq!(block, Block::Para(vec![str_inline("a\nb\tc\rd\\e\"f")]));
}

#[test]
fn decodes_control_and_numeric_escapes() {
    // \f \v \a \b control bytes, an empty \& separator, decimal, hex, and octal escapes.
    let block = only_block(r#"Para [Str "\f\v\a\b\&\65\x41\o101"]"#);
    assert_eq!(
        block,
        Block::Para(vec![str_inline("\u{0C}\u{0B}\u{07}\u{08}AAA")])
    );
}

#[test]
fn decodes_caret_and_mnemonic_control_escapes() {
    // \^A is control-A (U+0001); \ESC and \NUL are mnemonic control codes.
    let block = only_block(r#"Para [Str "\^A\ESC\NUL"]"#);
    assert_eq!(block, Block::Para(vec![str_inline("\u{01}\u{1B}\u{00}")]));
}

#[test]
fn decodes_string_gap() {
    let block = only_block("Para [Str \"a\\   \\b\"]");
    assert_eq!(block, Block::Para(vec![str_inline("ab")]));
}

#[test]
fn parses_negative_and_floating_numbers() {
    assert_eq!(
        only_block(r"OrderedList (-2, Decimal, Period) []"),
        Block::OrderedList(
            ListAttributes {
                start: -2,
                style: ListNumberStyle::Decimal,
                delim: ListNumberDelim::Period,
            },
            vec![]
        )
    );
    let block = only_block(
        r#"Table ("", [], []) (Caption Nothing []) [(AlignDefault, ColWidth 1.5e-1)] (TableHead ("", [], []) []) [] (TableFoot ("", [], []) [])"#,
    );
    let Block::Table(table) = block else {
        panic!("expected a table");
    };
    assert_eq!(
        table.col_specs.first().map(|spec| spec.width.clone()),
        Some(ColWidth::ColWidth(0.15))
    );
}

#[test]
fn rejects_unterminated_string() {
    assert!(parse_err(r#"Para [Str "oops]"#).contains("unterminated string"));
}

#[test]
fn rejects_unexpected_character() {
    assert!(parse_err("Para [Str @]").contains("unexpected character"));
}

#[test]
fn rejects_unknown_constructor() {
    assert!(parse_err("Bogus []").contains("not a recognized native document"));
}

#[test]
fn rejects_unknown_block_in_list() {
    assert!(parse_err("Para [Wat]").contains("unknown inline"));
}

#[test]
fn rejects_trailing_input() {
    assert!(parse_err("HorizontalRule HorizontalRule").contains("trailing input"));
}

#[test]
fn rejects_unknown_escape() {
    assert!(parse_err(r#"Para [Str "\q"]"#).contains("unknown string escape"));
}
