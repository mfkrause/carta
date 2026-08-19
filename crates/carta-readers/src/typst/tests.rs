use super::*;
use carta_ast::{Block, Inline};

fn parse(input: &str) -> Vec<Block> {
    TypstReader
        .read(input, &ReaderOptions::default())
        .expect("reader does not fail")
        .blocks
}

fn inlines(input: &str) -> Vec<Inline> {
    match parse(input).into_iter().next() {
        Some(Block::Para(inlines) | Block::Plain(inlines)) => inlines,
        other => panic!("expected a paragraph, got {other:?}"),
    }
}

fn text(value: &str) -> Inline {
    Inline::Str(value.into())
}

fn positional_value(value: Value) -> Arg {
    Arg { name: None, value }
}

fn named_value(name: &str, value: Value) -> Arg {
    Arg {
        name: Some(name.to_string()),
        value,
    }
}

#[test]
fn content_block_keeps_the_spaces_at_its_edges() {
    assert_eq!(
        inlines("A#emph[ b ]C"),
        vec![
            text("A"),
            Inline::Emph(vec![Inline::Space, text("b"), Inline::Space]),
            text("C"),
        ]
    );
}

#[test]
fn a_line_ending_inside_a_content_block_stands_for_a_break() {
    assert_eq!(
        inlines("A#strong[\nb]C"),
        vec![
            text("A"),
            Inline::Strong(vec![Inline::SoftBreak, text("b")]),
            text("C"),
        ]
    );
}

#[test]
fn a_content_block_of_only_spaces_is_one_separator() {
    assert_eq!(
        inlines("A#emph[  ]C"),
        vec![text("A"), Inline::Emph(vec![Inline::Space]), text("C")]
    );
}

#[test]
fn edge_spaces_meeting_the_surrounding_line_fold_together() {
    assert_eq!(
        inlines("A #box[ b ] C"),
        vec![
            text("A"),
            Inline::Space,
            Inline::Span(
                Box::new(Attr {
                    classes: vec!["box".into()],
                    ..Attr::default()
                }),
                vec![Inline::Space, text("b"), Inline::Space],
            ),
            Inline::Space,
            text("C"),
        ]
    );
}

#[test]
fn a_heading_body_keeps_the_spaces_at_its_edges() {
    assert_eq!(
        parse("#heading(level: 2)[ h ]"),
        vec![Block::Header(
            2,
            Box::default(),
            vec![Inline::Space, text("h"), Inline::Space],
        )]
    );
}

#[test]
fn block_edges_drop_the_spaces_a_content_block_carried() {
    assert_eq!(
        inlines("#footnote[ n ]"),
        vec![Inline::Note(vec![Block::Para(vec![text("n")])])]
    );
}

#[test]
fn loop_rounds_keep_the_padding_between_them() {
    assert_eq!(
        inlines("#for value in (1, 2) [ #value ]"),
        vec![text("1"), Inline::Space, text("2")]
    );
}

#[test]
fn a_whitespace_only_attribution_still_forms_its_own_paragraph() {
    assert_eq!(
        parse("#quote(block: true, attribution: [ ])[Q]"),
        vec![Block::BlockQuote(vec![
            Block::Para(vec![text("Q")]),
            Block::Para(vec![text("\u{2014}\u{a0}"), Inline::Space]),
        ])]
    );
}

#[test]
fn an_empty_attribution_forms_no_paragraph() {
    assert_eq!(
        parse("#quote(block: true, attribution: [])[Q]"),
        vec![Block::BlockQuote(vec![Block::Para(vec![text("Q")])])]
    );
}

#[test]
fn a_length_may_carry_an_exponent() {
    assert_eq!(
        parse("#image(\"a.png\", width: 1e3pt)"),
        vec![Block::Para(vec![Inline::Image(
            Box::new(Attr {
                attributes: [("width".into(), "1000.0pt".into())].into_iter().collect(),
                ..Attr::default()
            }),
            Vec::new(),
            Box::new(Target {
                url: "a.png".into(),
                title: Text::default(),
            }),
        )])]
    );
}

#[test]
fn a_measure_reads_with_its_fractional_part() {
    assert_eq!(inlines("#(2 * 3em)"), vec![text("6.0em")]);
    assert_eq!(inlines("#(1.5e-2)"), vec![text("1.5e-2")]);
    assert_eq!(inlines("#(1e15)"), vec![text("1.0e15")]);
    assert_eq!(inlines("#(6 / 3)"), vec![text("2")]);
}

#[test]
fn a_ratio_reads_as_whole_percent() {
    assert_eq!(inlines("#(50.5%)"), vec![text("50%")]);
}

#[test]
fn a_code_block_binding_leaves_no_trace_behind_it() {
    assert_eq!(
        parse("#let a = 1\n\n#{ let a = 2 }\n\n#a\n"),
        vec![Block::Para(vec![text("1")])]
    );
}

#[test]
fn a_loop_binding_outlives_its_loop() {
    assert_eq!(
        parse("#for i in (1, 2) [x]\n\n#i\n"),
        vec![Block::Para(vec![text("xx")]), Block::Para(vec![text("2")]),]
    );
}

/// Write the named files into a fresh directory and read the first one from there, as a document
/// named on the command line is read.
fn parse_in_directory(label: &str, files: &[(&str, &str)]) -> Vec<Block> {
    let directory =
        std::env::temp_dir().join(format!("carta-typst-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for (name, contents) in files {
        let path = directory.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("directory is writable");
        }
        std::fs::write(&path, contents).expect("file is writable");
    }
    let (_, source) = files.first().expect("at least one file");
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.clone());
    let blocks = TypstReader
        .read(source, &options)
        .expect("reader does not fail")
        .blocks;
    let _ = std::fs::remove_dir_all(&directory);
    blocks
}

#[test]
fn an_image_resolves_against_the_directory_of_its_source() {
    let blocks = parse_in_directory("image", &[("doc.typ", "#image(\"pics/a.png\")\n")]);
    let directory = std::env::temp_dir().join(format!("carta-typst-{}-image", std::process::id()));
    let expected = directory.join("pics/a.png");
    let expected = expected.to_str().expect("a printable path");
    match blocks.as_slice() {
        [Block::Para(inlines)] => match inlines.as_slice() {
            [Inline::Image(_, _, target)] => assert_eq!(target.url.as_str(), expected),
            other => panic!("expected one image, got {other:?}"),
        },
        other => panic!("expected one paragraph, got {other:?}"),
    }
}

#[test]
fn an_included_file_contributes_its_own_blocks() {
    assert_eq!(
        parse_in_directory(
            "include",
            &[
                ("doc.typ", "#include \"part.typ\"\nafter\n"),
                ("part.typ", "Included body.\n"),
            ],
        ),
        vec![
            Block::Para(vec![text("Included"), Inline::Space, text("body.")]),
            Block::Para(vec![text("after")]),
        ]
    );
}

#[test]
fn an_import_binds_the_names_a_file_defines() {
    assert_eq!(
        parse_in_directory(
            "import",
            &[
                ("doc.typ", "#import \"lib.typ\": greet\n#greet world\n"),
                ("lib.typ", "#let greet = [Hello]\n"),
            ],
        ),
        vec![Block::Para(vec![
            text("Hello"),
            Inline::Space,
            text("world")
        ])]
    );
}

#[test]
fn an_import_resolves_a_nested_file_against_its_own_directory() {
    assert_eq!(
        parse_in_directory(
            "nested",
            &[
                ("doc.typ", "#import \"sub/lib.typ\": w\n#w\n"),
                ("sub/lib.typ", "#import \"dep.typ\": u\n#let w = [w=#u]\n"),
                ("sub/dep.typ", "#let u = [U]\n"),
            ],
        ),
        vec![Block::Para(vec![text("w=U")])]
    );
}

#[test]
fn a_file_that_includes_itself_terminates() {
    assert_eq!(
        parse_in_directory("cycle", &[("doc.typ", "#include \"doc.typ\"\nx\n")]),
        vec![Block::Para(vec![text("x")]), Block::Para(vec![text("x")])]
    );
}

#[test]
fn a_show_rule_leaves_what_an_earlier_rule_produced_alone() {
    assert_eq!(
        inlines("#show \"a\": \"b\"\n#show \"b\": \"c\"\naaa\n"),
        vec![text("bbb")]
    );
}

#[test]
fn a_show_rule_governs_only_the_item_holding_it() {
    assert_eq!(
        parse("- #show \"z\": \"Z\"\n  z here\n- two z\n"),
        vec![Block::BulletList(vec![
            vec![Block::Para(vec![text("Z"), Inline::Space, text("here")])],
            vec![Block::Para(vec![text("two"), Inline::Space, text("z")])],
        ])]
    );
}

#[test]
fn a_show_rule_rebuilds_a_match_as_the_element_it_names() {
    assert_eq!(
        inlines("#show emph: strong\n_e_\n"),
        vec![Inline::Strong(vec![Inline::Emph(vec![text("e")])])]
    );
}

#[test]
fn a_context_expression_reads_the_operators_on_its_line() {
    assert_eq!(inlines("#context 1 + 1.\n"), vec![text("2.0")]);
}

#[test]
fn an_emoji_name_reads_as_its_glyph() {
    assert_eq!(inlines("#emoji.rocket\n"), vec![text("\u{1f680}")]);
}

#[test]
fn an_unrecognised_numbering_pattern_numbers_the_items_plainly() {
    assert_eq!(
        parse("#set enum(numbering: \"Q1.\")\n+ x\n"),
        vec![Block::OrderedList(
            ListAttributes {
                start: 1,
                style: ListNumberStyle::DefaultStyle,
                delim: ListNumberDelim::DefaultDelim,
            },
            vec![vec![Block::Para(vec![text("x")])]]
        )]
    );
}

#[test]
fn a_bibliography_heads_its_reference_section() {
    assert_eq!(
        parse("#bibliography(\"refs.bib\")\n"),
        vec![
            Block::Header(1, Box::default(), vec![text("References")]),
            Block::Div(
                Box::new(Attr {
                    id: "refs".into(),
                    ..Attr::default()
                }),
                Vec::new()
            ),
        ]
    );
}

#[test]
fn a_bibliography_titled_with_nothing_heads_no_reference_section() {
    assert_eq!(
        parse("#bibliography(\"refs.bib\", title: none)\n"),
        vec![Block::Div(
            Box::new(Attr {
                id: "refs".into(),
                ..Attr::default()
            }),
            Vec::new()
        )]
    );
}

#[test]
fn a_bibliography_takes_the_title_it_names() {
    assert_eq!(
        parse("#bibliography(\"refs.bib\", title: [Works cited])\n"),
        vec![
            Block::Header(
                1,
                Box::default(),
                vec![text("Works"), Inline::Space, text("cited")]
            ),
            Block::Div(
                Box::new(Attr {
                    id: "refs".into(),
                    ..Attr::default()
                }),
                Vec::new()
            ),
        ]
    );
}

#[test]
fn bindings_that_splice_one_another_stop_expanding() {
    let levels = 40;
    let mut source = String::from("#let a0 = [x]\n");
    for level in 1..=levels {
        let _ = writeln!(source, "#let a{level} = [#a{} #a{}]", level - 1, level - 1);
    }
    let _ = writeln!(source, "#a{levels}");
    let words = parse(&source)
        .iter()
        .filter(|block| matches!(block, Block::Para(_)))
        .count();
    assert!(words <= 1);
}

#[test]
fn reused_bindings_stay_within_the_copying_allowance() {
    let source = format!("#let note = [x]\n{}\n", "#note ".repeat(500));
    assert_eq!(
        inlines(&source).iter().filter(|i| **i == text("x")).count(),
        500
    );
}

#[test]
fn generated_regular_expressions_stay_within_the_compilation_bound() {
    let source = format!(
        "#show regex(\"a\" * {}): \"x\"\nbody\n",
        MAX_REGEX_BYTES + 1
    );
    assert_eq!(parse(&source), vec![Block::Para(vec![text("body")])]);
}

#[test]
fn sequence_repetition_spends_the_materialization_allowance() {
    let mut copies = 8;
    let count = Value::Int(Integer::from(100usize));
    let repeated = combine(
        Value::Str("ab".to_string()),
        count.clone(),
        '*',
        &mut copies,
    );
    assert_eq!(repeated, Value::Str("abab".to_string()));
    assert_eq!(
        combine(repeated, count, '*', &mut copies),
        Value::Str(String::new())
    );
}

/// Reads from a caller stack too shallow for the nesting, which proves the read runs on its own
/// deep worker stack.
fn parse_from_a_shallow_stack(source: String) -> Vec<Block> {
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || parse(&source))
        .expect("worker thread spawns")
        .join()
        .expect("worker thread finishes")
}

#[test]
fn deeply_nested_groups_read_without_exhausting_the_stack() {
    let depth = 3_000;
    let source = format!("#{}1{}\n", "(".repeat(depth), ")".repeat(depth));
    assert_eq!(
        parse_from_a_shallow_stack(source),
        vec![Block::Para(vec![text("1")])]
    );
}

#[test]
fn deeply_nested_conditionals_read_without_exhausting_the_stack() {
    let depth = 3_000;
    let source = format!(
        "#if true {}[ok]{}\n",
        "{ if true ".repeat(depth),
        " }".repeat(depth)
    );
    assert_eq!(
        parse_from_a_shallow_stack(source),
        vec![Block::Para(vec![text("ok")])]
    );
}

#[test]
fn numeric_helpers_cover_whole_and_fractional_results() {
    assert!((arrangements(5.0, 2.0) - 20.0).abs() < f64::EPSILON);
    assert!(arrangements(2.0, 3.0).abs() < f64::EPSILON);
    assert!((factorial_of(5.0) - 120.0).abs() < f64::EPSILON);
    assert_eq!(scalar(3.0, true), Value::Int(Integer::from(3usize)));
    assert_eq!(scalar(3.5, false), Value::Number(3.5, String::new()));

    let digits = [named_value("digits", Value::Number(2.0, String::new()))];
    assert_eq!(
        round_to_digits(1.235, &digits),
        Value::Number(1.24, String::new())
    );
    assert_eq!(round_to_digits(1.6, &[]), Value::Int(Integer::from(2usize)));

    let values = [
        Value::Nothing,
        Value::Number(3.0, String::new()),
        Value::Int(Integer::from(2usize)),
    ];
    let references: Vec<&Value> = values.iter().collect();
    assert_eq!(
        extreme(&references, Ordering::Less),
        Value::Int(Integer::from(2usize))
    );
    assert_eq!(
        extreme(&references, Ordering::Greater),
        Value::Number(3.0, String::new())
    );
    assert_eq!(power(2.0, 4.0, true), Value::Int(Integer::from(16usize)));
    assert_eq!(
        power(2.0, 70.0, true),
        Value::Number(2.0f64.powf(70.0), String::new())
    );
    assert!((greatest_common_divisor(54.0, 24.0) - 6.0).abs() < f64::EPSILON);
    assert_eq!(factorial(5.0), Value::Int(Integer::from(120usize)));
    assert_eq!(factorial(30.0), Value::Nothing);

    for function in [
        "pow", "rem", "quo", "gcd", "even", "odd", "fact", "clamp", "sin", "cos", "tan", "sinh",
        "cosh", "tanh", "asin", "acos", "atan", "atan2", "exp", "ln", "log",
    ] {
        let result = calc_call(
            function,
            &[
                positional_value(Value::Number(2.5, String::new())),
                positional_value(Value::Number(1.5, String::new())),
                positional_value(Value::Number(4.0, String::new())),
            ],
        );
        assert_ne!(result, Value::Nothing, "{function}");
    }
}

#[test]
fn element_helpers_cover_structural_values() {
    assert_eq!(
        datetime(&[
            named_value("year", Value::Number(2026.0, String::new())),
            named_value("month", Value::Number(8.0, String::new())),
            named_value("day", Value::Number(10.0, String::new())),
            named_value("hour", Value::Number(9.0, String::new())),
            named_value("minute", Value::Number(7.0, String::new())),
            named_value("second", Value::Number(5.0, String::new())),
        ]),
        Value::Str("2026-08-10 09:07:05".to_string())
    );
    assert_eq!(datetime(&[]), Value::Nothing);
    assert_eq!(
        format_date("2026-08-10", "[day]/[month]/[year]"),
        "10/08/2026"
    );

    assert_eq!(
        horizontal_space(&[positional_value(Value::Number(-1.0, "em".to_string()))]),
        "\u{200b}"
    );
    assert!(
        !horizontal_space(&[positional_value(Value::Number(1.25, "em".to_string()))]).is_empty()
    );
    assert!(
        !horizontal_space(&[positional_value(Value::Number(6.0, "pt".to_string()))]).is_empty()
    );
    assert!(!horizontal_space(&[positional_value(Value::Int(Integer::one()))]).is_empty());
    for fraction in [0.05, 0.15, 0.2, 0.27, 0.38, 0.49] {
        assert_ne!(fraction_space(fraction), '\0');
    }

    let content = Value::Content(vec![Block::Para(vec![text("body")])]);
    assert_eq!(
        place_like("hide", &[positional_value(content.clone())]),
        Value::Nothing
    );
    assert_eq!(place_like("place", &[]), Value::Nothing);
    assert!(matches!(
        place_like("place", &[positional_value(content)]),
        Value::Content(_)
    ));

    assert_eq!(
        rotation_angle(&[named_value(
            "angle",
            Value::Number(180.0, "deg".to_string())
        )]),
        vec![("angle".into(), "180.0".into())]
    );
    assert_eq!(
        rotation_angle(&[positional_value(Value::Number(
            std::f64::consts::PI,
            "rad".to_string()
        ))]),
        vec![("angle".into(), "180.0".into())]
    );
    assert!(rotation_angle(&[positional_value(Value::Number(1.0, "em".to_string()))]).is_empty());

    let terms = term_entries(&[positional_value(Value::Array(vec![
        Value::Str("term".to_string()),
        Value::Str("definition".to_string()),
    ]))]);
    assert_eq!(terms.len(), 1);
    assert_eq!(
        lorem(&[positional_value(Value::Number(5.0, String::new()))])
            .split(' ')
            .count(),
        5
    );
}

#[test]
fn value_weight_visits_each_document_shape() {
    let table = build_table(
        &[
            named_value("columns", Value::Int(Integer::from(2usize))),
            positional_value(Value::Str("a".to_string())),
            positional_value(Value::Str("b".to_string())),
        ],
        Caption {
            short: Some(vec![text("short")]),
            long: vec![Block::Para(vec![text("long")])],
        },
    );
    let blocks = vec![
        Block::LineBlock(vec![vec![text("line")]]),
        Block::CodeBlock(Box::default(), "code".into()),
        Block::RawBlock(carta_ast::Format("typst".into()), "raw".into()),
        Block::BlockQuote(vec![Block::Para(vec![text("quote")])]),
        Block::OrderedList(
            carta_ast::ListAttributes {
                start: 1,
                style: carta_ast::ListNumberStyle::DefaultStyle,
                delim: carta_ast::ListNumberDelim::DefaultDelim,
            },
            vec![vec![Block::Para(vec![text("item")])]],
        ),
        Block::DefinitionList(vec![(
            vec![text("term")],
            vec![vec![Block::Para(vec![text("definition")])]],
        )]),
        Block::Figure(
            Box::default(),
            Box::new(Caption {
                short: Some(vec![text("caption")]),
                long: Vec::new(),
            }),
            vec![Block::HorizontalRule],
        ),
        table,
    ];
    let value = Value::Content(blocks);
    assert!(value_weight(&value) > 40);

    let compound = Value::Dict(vec![
        ("array".to_string(), Value::Array(vec![Value::Bool(true)])),
        (
            "group".to_string(),
            Value::Group(
                GroupKind::Header,
                vec![positional_value(Value::Str("x".to_string()))],
            ),
        ),
        ("label".to_string(), Value::Label("id".to_string())),
        ("regex".to_string(), Value::Regex("x+".to_string())),
    ]);
    assert!(value_weight(&compound) > 15);
}

#[test]
fn value_text_covers_each_scalar_and_collection_shape() {
    let values = [
        (Value::Ident("name".to_string()), "name"),
        (Value::Label("key".to_string()), "key"),
        (Value::Int(Integer::from(12usize)), "12"),
        (Value::Number(1.5, "em".to_string()), "1.5em"),
        (Value::Bool(true), "true"),
        (Value::Regex("a+".to_string()), "/a+/"),
        (Value::Nothing, ""),
    ];
    for (value, expected) in values {
        assert_eq!(value.as_text(), expected);
    }
    assert_eq!(
        Value::Array(vec![Value::Str("a".to_string()), Value::Bool(false)]).as_text(),
        "a, false"
    );
    assert_eq!(
        Value::Dict(vec![("key".to_string(), Value::Str("value".to_string()))]).as_text(),
        "(key: \"value\")"
    );
    assert_eq!(
        Value::Content(vec![Block::Para(vec![text("body")])]).as_text(),
        "body"
    );
    assert_eq!(Value::Inlines(vec![text("inline")]).as_text(), "inline");
}

#[test]
fn value_weight_visits_each_inline_shape() {
    let target = Box::new(carta_ast::Target {
        url: "target".into(),
        title: "title".into(),
    });
    let nested = vec![
        Inline::Emph(vec![text("em")]),
        Inline::Underline(vec![text("under")]),
        Inline::Strong(vec![text("strong")]),
        Inline::Strikeout(vec![text("strike")]),
        Inline::Superscript(vec![text("super")]),
        Inline::Subscript(vec![text("sub")]),
        Inline::SmallCaps(vec![text("small")]),
        Inline::Quoted(carta_ast::QuoteType::SingleQuote, vec![text("quote")]),
        Inline::Span(Box::default(), vec![text("span")]),
        Inline::Code(Box::default(), "code".into()),
        Inline::Math(carta_ast::MathType::InlineMath, "x".into()),
        Inline::RawInline(carta_ast::Format("html".into()), "<b>".into()),
        Inline::Cite(
            vec![carta_ast::Citation {
                id: "source".into(),
                prefix: vec![text("see")],
                suffix: vec![text("page")],
                mode: carta_ast::CitationMode::NormalCitation,
                note_num: 0,
                hash: 0,
            }],
            vec![text("citation")],
        ),
        Inline::Link(Box::default(), vec![text("link")], target.clone()),
        Inline::Image(Box::default(), vec![text("image")], target),
        Inline::Note(vec![Block::Para(vec![text("note")])]),
        Inline::Space,
        Inline::SoftBreak,
        Inline::LineBreak,
    ];
    assert!(value_weight(&Value::Inlines(nested)) > 100);
}

#[test]
fn table_and_text_helpers_cover_edge_shapes() {
    let definition = Block::DefinitionList(vec![(
        vec![text("term")],
        vec![vec![Block::Para(vec![text("meaning")])]],
    )]);
    let flattened = blocks_to_inlines(vec![
        Block::BlockQuote(vec![Block::Para(vec![text("quote")])]),
        Block::BulletList(vec![vec![Block::Para(vec![text("item")])]]),
        Block::OrderedList(
            carta_ast::ListAttributes {
                start: 1,
                style: carta_ast::ListNumberStyle::Decimal,
                delim: carta_ast::ListNumberDelim::Period,
            },
            vec![vec![Block::Para(vec![text("ordered")])]],
        ),
        Block::CodeBlock(Box::default(), "code".into()),
        definition,
        Block::HorizontalRule,
    ]);
    assert!(flattened.contains(&Inline::LineBreak));

    let cell = cell_from(&Value::Group(
        GroupKind::Cell,
        vec![
            named_value("colspan", Value::Number(2.0, String::new())),
            named_value("rowspan", Value::Number(3.0, String::new())),
            named_value("align", Value::Ident("right".to_string())),
            positional_value(Value::Str("cell".to_string())),
        ],
    ));
    assert_eq!((cell.col_span, cell.row_span), (2, 3));
    assert_eq!(cell.align, carta_ast::Alignment::AlignRight);

    let rows = lay_out_rows(vec![cell, blank_cell(), blank_cell()], 2);
    assert_eq!(rows.len(), 4);
    assert_eq!(column_shares(&[Some(2.0), None], 2), vec![Some(2.0); 2]);
    assert_eq!(track_widths(&Value::Int(Integer::from(3usize))).len(), 3);
    assert_eq!(
        alignment_of(&Value::Ident("center".to_string())),
        carta_ast::Alignment::AlignCenter
    );
}

#[test]
fn wide_text_helpers_visit_nested_inlines() {
    let mut blocks = vec![Block::Para(vec![Inline::Strong(vec![
        text("日"),
        Inline::SoftBreak,
        text("本"),
    ])])];
    strip_wide_line_breaks(&mut blocks);
    assert_eq!(
        blocks,
        vec![Block::Para(vec![Inline::Strong(vec![
            text("日"),
            text("本")
        ])])]
    );
    assert!(is_unspaced_script('日'));
    assert!(is_east_asian_wide('日'));
    assert!(!is_word_char('日'));
    assert!(is_word_char('A'));
    assert_eq!(first_char(&Inline::Emph(vec![text("first")])), Some('f'));
    assert_eq!(last_char(&Inline::Strong(vec![text("last")])), Some('t'));

    let target = Box::new(carta_ast::Target::default());
    let citation = carta_ast::Citation {
        id: "id".into(),
        prefix: Vec::new(),
        suffix: Vec::new(),
        mode: carta_ast::CitationMode::NormalCitation,
        note_num: 0,
        hash: 0,
    };
    let wrappers = vec![
        Inline::Emph(vec![text("日")]),
        Inline::Underline(vec![text("日")]),
        Inline::Strikeout(vec![text("日")]),
        Inline::Superscript(vec![text("日")]),
        Inline::Subscript(vec![text("日")]),
        Inline::SmallCaps(vec![text("日")]),
        Inline::Quoted(carta_ast::QuoteType::DoubleQuote, vec![text("日")]),
        Inline::Cite(vec![citation], vec![text("日")]),
        Inline::Link(Box::default(), vec![text("日")], target.clone()),
        Inline::Image(Box::default(), vec![text("日")], target),
        Inline::Span(Box::default(), vec![text("日")]),
        Inline::Note(vec![Block::Para(vec![text("日")])]),
    ];
    let mut wrappers_for_strip = wrappers.clone();
    strip_wide_in_inlines(&mut wrappers_for_strip);
    for wrapper in &wrappers {
        if !matches!(wrapper, Inline::Image(..) | Inline::Note(..)) {
            assert_eq!(first_char(wrapper), Some('日'));
            assert_eq!(last_char(wrapper), Some('日'));
        }
    }

    for character in [
        '\u{3000}',
        '\u{31f0}',
        '\u{4e00}',
        '\u{a000}',
        '\u{f900}',
        '\u{fe30}',
        '\u{ff00}',
        '\u{1b000}',
        '\u{1f200}',
        '\u{20000}',
    ] {
        assert!(is_unspaced_script(character));
    }
    for character in [
        '\u{1100}',
        '\u{2e80}',
        '\u{2f00}',
        '\u{2ff0}',
        '\u{3041}',
        '\u{3400}',
        '\u{a960}',
        '\u{ac00}',
        '\u{fe10}',
        '\u{ffe0}',
        '\u{30000}',
    ] {
        assert!(is_east_asian_wide(character));
    }
}

#[test]
fn math_translation_covers_groups_calls_and_layout() {
    let cases = [
        ("[a, b]", "[a,b]"),
        ("{a + b}", "\\{a + b\\}"),
        ("a & b \\\\ c & d", "\\begin{array}"),
        ("frac(1, 2)", "\\frac{1}{2}"),
        ("sqrt(x)", "\\sqrt{x}"),
        ("floor(x)", "\\lfloor"),
        ("ceil(x)", "\\lceil"),
        ("round(x)", "\\rceil"),
        ("lr(x)", "\\left."),
        ("accent(x, dot)", "\\hat{x}"),
        ("dot(x)", "\\dot{x}"),
        ("bold(x)", "\\mathbf{x}"),
        ("mat(1, 2; 3, 4)", "matrix"),
        ("cases(x & if y, z & otherwise)", "cases"),
        ("sum_(i=1)^n", "\\sum"),
        ("a | b", "~|~"),
        ("a×b", "\\times"),
    ];
    for (source, expected) in cases {
        let rendered = math_to_tex(source);
        assert!(rendered.contains(expected), "{source}: {rendered}");
    }
    assert_eq!(escaped_atom('&'), "\\&");
    assert_eq!(escaped_atom('!'), "!");
    assert!(ends_control_sequence("\\alpha"));
    assert!(!ends_control_sequence("\\alpha "));
    assert_eq!(strip_text("\\text{word}"), "word");
    assert_eq!(accent_command("unknown"), "\\hat");
    assert_eq!(Piece::Group("x".to_string()).text(), "(x)");
    assert_eq!(Piece::Group("x".to_string()).operand(), "x");
    assert_eq!(Piece::Break.text(), "\\\\");
    assert_eq!(Piece::Align.text(), "&");
    assert_eq!(double_struck("RR"), Some("\\mathbb{R}".to_string()));
    assert_eq!(double_struck("R"), None);
    assert_eq!(double_struck("RS"), None);
}

#[test]
fn uncommon_calls_are_evaluated_without_losing_following_text() {
    let source = r#"
#datetime(year: 2026, month: 8, day: 10, hour: 9, minute: 7, second: 5)
#h(1.25em)
#lorem(8)
#place[kept]
#hide[hidden]
#rotate(angle: 180deg)[turned]
#terms(([Term], [Definition]), ([Other], [Meaning]))
#raw("code", block: true, lang: "rust")
#table(columns: 2, [a], [b], table.header([h1], [h2]), table.footer([f1], [f2]))
#repr((key: "value", items: (1, 2)))
#type((key: 1))
#calc.abs(-2) #calc.ceil(1.2) #calc.floor(1.8) #calc.fract(1.25)
#calc.fact(5) #calc.gcd(54, 24) #calc.pow(2, 8) #calc.quo(7, 3) #calc.rem(7, 3)
#calc.clamp(10, 0, 5) #calc.max(1, 4, 2) #calc.min(1, 4, 2) #calc.round(1.235, digits: 2)
after
"#;
    let blocks = parse(source);
    assert!(!blocks.is_empty());
    assert!(blocks.iter().any(|block| matches!(block, Block::Table(_))));
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, Block::DefinitionList(_)))
    );
}

#[test]
fn a_text_show_rule_descends_through_structural_content() {
    let blocks = parse(
        r#"#show "x": "y"
= x
- x
+ x
/ x: x
#quote(block: true)[x]
#figure([x], caption: [x])
#table(columns: 1, table.header([x]), [x], table.footer([x]))
_x_ *x* #underline[x] #strike[x] #super[x] #sub[x] #smallcaps[x] #quote[x]
#link("u")[x] #image("u", alt: "x") #footnote[x]
"#,
    );
    assert!(matches!(blocks.first(), Some(Block::Header(..))));
    assert!(blocks.iter().any(|block| matches!(block, Block::Table(_))));
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, Block::Figure(..)))
    );
    assert!(!format!("{blocks:?}").contains("\"x\""));
}

#[test]
fn show_selectors_cover_patterns_fields_and_kept_styles() {
    assert_eq!(
        inlines("#show regex(\"x+\"): \"y\"\naxxxb\n"),
        vec![text("ayb")]
    );
    assert!(parse("#show heading.where(level: 1): none\n= removed\n").is_empty());
    assert_eq!(
        inlines("#show emph: set text(fill: red)\n_x_\n"),
        vec![Inline::Emph(vec![text("x")])]
    );
}

#[test]
fn malformed_show_rules_leave_following_content_readable() {
    for source in [
        "#show 1\nbody",
        "#show emph body\nbody",
        "#show regex: \"y\"\nbody",
        "#show regex(\"[\"): \"y\"\nbody",
    ] {
        assert_eq!(parse(source), vec![Block::Para(vec![text("body")])]);
    }
}
