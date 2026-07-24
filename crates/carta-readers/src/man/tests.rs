use super::*;
use carta_core::Extension;

fn read(input: &str) -> Document {
    read_with(input, Extensions::from_list(&[Extension::AutoIdentifiers]))
}

fn read_with(input: &str, extensions: Extensions) -> Document {
    let mut options = ReaderOptions::default();
    options.extensions = extensions;
    ManReader.read(input, &options).expect("read")
}

#[test]
fn title_populates_metadata() {
    let doc = read(".TH FOO 1 \"2024-01-01\" \"version 1.0\" \"Foo Manual\"\n");
    assert_eq!(
        doc.meta.get("title"),
        Some(&MetaValue::MetaInlines(vec![Inline::Str("FOO".into())]))
    );
    assert_eq!(
        doc.meta.get("section"),
        Some(&MetaValue::MetaInlines(vec![Inline::Str("1".into())]))
    );
    assert_eq!(
        doc.meta.get("header"),
        Some(&MetaValue::MetaInlines(vec![
            Inline::Str("Foo".into()),
            Inline::Space,
            Inline::Str("Manual".into()),
        ]))
    );
}

#[test]
fn section_headings_get_identifiers() {
    let doc = read(".TH T 1\n.SH NAME\nfoo\n.SS Sub Title\nbar\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Header(
            1,
            Box::new(Attr {
                id: "name".into(),
                ..Attr::default()
            }),
            vec![Inline::Str("NAME".into())]
        ))
    );
    assert!(matches!(
        doc.blocks.get(2),
        Some(Block::Header(2, attr, _)) if attr.id == "sub-title"
    ));
}

#[test]
fn duplicate_headings_disambiguate() {
    let doc = read(".TH T 1\n.SH Foo\nx\n.SH Foo\ny\n");
    let ids: Vec<&str> = doc
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Header(_, attr, _) => Some(attr.id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["foo", "foo-1"]);
}

#[test]
fn auto_identifiers_off_leaves_empty_id() {
    let doc = read_with(".TH T 1\n.SH Foo Bar\nx\n", Extensions::empty());
    assert!(matches!(
        doc.blocks.first(),
        Some(Block::Header(1, attr, _)) if attr.id.is_empty()
    ));
}

#[test]
fn lines_fill_into_one_paragraph() {
    let doc = read(".TH T 1\nfirst line\nsecond line\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("first".into()),
            Inline::Space,
            Inline::Str("line".into()),
            Inline::Space,
            Inline::Str("second".into()),
            Inline::Space,
            Inline::Str("line".into()),
        ]))
    );
}

#[test]
fn blank_line_separates_paragraphs() {
    let doc = read(".TH T 1\none\n\ntwo\n");
    assert_eq!(doc.blocks.len(), 2);
}

#[test]
fn bold_macro_joins_arguments() {
    let doc = read(".TH T 1\n.B \"two words\" tail\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![Inline::Strong(vec![
            Inline::Str("two".into()),
            Inline::Space,
            Inline::Str("words".into()),
            Inline::Space,
            Inline::Str("tail".into()),
        ])]))
    );
}

#[test]
fn font_macro_nests_an_inner_font_escape() {
    let doc = read(".TH T 1\n.B \\-f \\fIfile\\fR tail\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![Inline::Strong(vec![
            Inline::Str("-f".into()),
            Inline::Space,
            Inline::Emph(vec![Inline::Str("file".into())]),
            Inline::Space,
            Inline::Str("tail".into()),
        ])]))
    );
}

#[test]
fn alternating_font_arg_wraps_an_inner_escape() {
    let doc = read(".TH T 1\n.BR a\\fIx\\fR b\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Strong(vec![
                Inline::Str("a".into()),
                Inline::Emph(vec![Inline::Str("x".into())]),
            ]),
            Inline::Str("b".into()),
        ]))
    );
}

#[test]
fn alternating_fonts_abut_without_space() {
    let doc = read(".TH T 1\n.BR bold roman\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Strong(vec![Inline::Str("bold".into())]),
            Inline::Str("roman".into()),
        ]))
    );
}

#[test]
fn inline_font_escape_groups_run() {
    let doc = read(".TH T 1\n\\fBtwo words\\fR plain\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Strong(vec![
                Inline::Str("two".into()),
                Inline::Space,
                Inline::Str("words".into()),
            ]),
            Inline::Space,
            Inline::Str("plain".into()),
        ]))
    );
}

#[test]
fn boundary_space_leaves_the_font_run() {
    let doc = read(".TH T 1\n\\fBbold \\fRroman\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Strong(vec![Inline::Str("bold".into())]),
            Inline::Space,
            Inline::Str("roman".into()),
        ]))
    );
}

#[test]
fn break_macro_is_a_line_break() {
    let doc = read(".TH T 1\nbefore\n.br\nafter\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("before".into()),
            Inline::LineBreak,
            Inline::Str("after".into()),
        ]))
    );
}

#[test]
fn comment_is_transparent() {
    let doc = read(".TH T 1\nvisible\n.\\\" a comment\nstill\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("visible".into()),
            Inline::Space,
            Inline::Str("still".into()),
        ]))
    );
}

#[test]
fn special_characters_resolve() {
    let doc = read(".TH T 1\ndash \\- bullet \\(bu em \\(em\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("dash".into()),
            Inline::Space,
            Inline::Str("-".into()),
            Inline::Space,
            Inline::Str("bullet".into()),
            Inline::Space,
            Inline::Str("\u{00b7}".into()),
            Inline::Space,
            Inline::Str("em".into()),
            Inline::Space,
            Inline::Str("\u{2014}".into()),
        ]))
    );
}

#[test]
fn unknown_special_character_is_replacement() {
    let doc = read(".TH T 1\nx \\(ZZ y\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("x".into()),
            Inline::Space,
            Inline::Str("\u{fffd}".into()),
            Inline::Space,
            Inline::Str("y".into()),
        ]))
    );
}

#[test]
fn unicode_escape_resolves() {
    let doc = read(".TH T 1\n\\[u00C9]\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![Inline::Str("\u{00c9}".into())]))
    );
}

#[test]
fn tbl_region_becomes_a_table() {
    let doc = read(".TH T 1\n.TS\nl r.\nName\tAge\n_\nAda\t36\n.TE\nafter\n");
    let Some(Block::Table(table)) = doc.blocks.first() else {
        panic!("expected a table");
    };
    // Alignments come from the format line; widths stay default.
    assert_eq!(
        table.col_specs,
        vec![
            ColSpec {
                align: Alignment::AlignLeft,
                width: ColWidth::ColWidthDefault,
            },
            ColSpec {
                align: Alignment::AlignRight,
                width: ColWidth::ColWidthDefault,
            },
        ]
    );
    // The rule line under the first data row promotes it to the head.
    assert_eq!(table.head.rows.len(), 1);
    assert_eq!(table.head.rows.first().map(|row| row.cells.len()), Some(2));
    assert_eq!(table.bodies.first().map(|body| body.body.len()), Some(1));
    assert_eq!(
        doc.blocks.get(1),
        Some(&Block::Para(vec![Inline::Str("after".into())]))
    );
}

#[test]
fn tbl_without_header_rule_puts_every_row_in_the_body() {
    let doc = read(".TH T 1\n.TS\nc c.\nName\tAge\nAda\t36\n.TE\n");
    let Some(Block::Table(table)) = doc.blocks.first() else {
        panic!("expected a table");
    };
    assert!(table.head.rows.is_empty());
    assert_eq!(table.bodies.first().map(|body| body.body.len()), Some(2));
}

#[test]
fn malformed_tbl_region_yields_no_block() {
    let doc = read(".TS");
    assert!(doc.blocks.is_empty());
}

#[test]
fn tagged_paragraphs_become_a_definition_list() {
    let doc = read(".TH T 1\n.TP\n.B \\-v\nVerbose mode.\n.TP\n.B \\-f\nUse a file.\n");
    let Some(Block::DefinitionList(items)) = doc.blocks.first() else {
        panic!("expected a definition list");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        items.first().map(|(term, _)| term.clone()),
        Some(vec![Inline::Strong(vec![Inline::Str("-v".into())])])
    );
}

#[test]
fn bullet_indented_paragraphs_become_a_bullet_list() {
    let doc = read(".TH T 1\n.IP \\(bu 2\none\n.IP \\(bu 2\ntwo\n");
    let Some(Block::BulletList(items)) = doc.blocks.first() else {
        panic!("expected a bullet list");
    };
    assert_eq!(items.len(), 2);
}

#[test]
fn numbered_indented_paragraphs_become_an_ordered_list() {
    let doc = read(".TH T 1\n.IP 3. 4\nthree\n.IP 4. 4\nfour\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::OrderedList(
            ListAttributes {
                start: 3,
                style: ListNumberStyle::Decimal,
                delim: ListNumberDelim::Period,
            },
            vec![
                vec![Block::Para(vec![Inline::Str("three".into())])],
                vec![Block::Para(vec![Inline::Str("four".into())])],
            ]
        ))
    );
}

#[test]
fn roman_marker_is_lower_roman() {
    assert!(matches!(
        parse_enumerator("iv."),
        Some(ListAttributes {
            start: 4,
            style: ListNumberStyle::LowerRoman,
            delim: ListNumberDelim::Period,
        })
    ));
}

#[test]
fn bare_letter_marker_uses_its_position() {
    assert!(matches!(
        parse_enumerator("o"),
        Some(ListAttributes {
            start: 15,
            style: ListNumberStyle::LowerAlpha,
            delim: ListNumberDelim::DefaultDelim,
        })
    ));
}

#[test]
fn unmarked_indented_paragraph_is_an_inset() {
    let doc = read(".TH T 1\n.IP\nplain indented\n");
    assert!(matches!(doc.blocks.first(), Some(Block::BlockQuote(_))));
}

#[test]
fn relative_inset_becomes_a_block_quote() {
    let doc = read(".TH T 1\n.RS\ninside\n.RE\nafter\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::BlockQuote(vec![Block::Para(vec![Inline::Str(
            "inside".into()
        )])]))
    );
    assert_eq!(
        doc.blocks.get(1),
        Some(&Block::Para(vec![Inline::Str("after".into())]))
    );
}

#[test]
fn nested_insets_nest_block_quotes() {
    let doc = read(".TH T 1\n.RS\nouter\n.RS\ninner\n.RE\n.RE\n");
    assert!(matches!(
        doc.blocks.first(),
        Some(Block::BlockQuote(inner)) if inner.iter().any(|b| matches!(b, Block::BlockQuote(_)))
    ));
}

#[test]
fn no_fill_region_becomes_a_code_block() {
    let doc = read(".TH T 1\n.nf\nline one\n  indented\n.fi\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::CodeBlock(
            Box::default(),
            "line one\n  indented".into()
        ))
    );
}

#[test]
fn example_region_becomes_a_code_block() {
    let doc = read(".TH T 1\n.EX\n\\fBcode\\fR \\- here\n.EE\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::CodeBlock(Box::default(), "code - here".into()))
    );
}

#[test]
fn uri_macro_becomes_a_link() {
    let doc = read(".TH T 1\n.UR https://example.com\nthe text\n.UE\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![Inline::Link(
            Box::default(),
            vec![
                Inline::Str("the".into()),
                Inline::Space,
                Inline::Str("text".into()),
            ],
            Box::new(Target {
                url: "https://example.com".into(),
                title: carta_ast::Text::default(),
            }),
        )]))
    );
}

#[test]
fn mail_macro_uses_mailto() {
    let doc = read(".TH T 1\n.MT user@example.com\nwrite me\n.ME\n");
    let Some(Block::Para(inlines)) = doc.blocks.first() else {
        panic!("expected a paragraph");
    };
    assert!(matches!(
        inlines.first(),
        Some(Inline::Link(_, _, target)) if target.url == "mailto:user@example.com"
    ));
}

#[test]
fn link_trailing_text_attaches_without_space() {
    let doc = read(".TH T 1\nsee\n.UR https://x.org\nhere\n.UE .\nnext\n");
    let Some(Block::Para(inlines)) = doc.blocks.first() else {
        panic!("expected a paragraph");
    };
    // … the link, then the trailing "." with no separating space.
    let link_index = inlines
        .iter()
        .position(|i| matches!(i, Inline::Link(..)))
        .expect("link present");
    assert_eq!(inlines.get(link_index + 1), Some(&Inline::Str(".".into())));
}

#[test]
fn unknown_macro_breaks_the_paragraph() {
    let doc = read(".TH T 1\nbefore\n.XYZ args\nafter\n");
    assert_eq!(doc.blocks.len(), 2);
}

#[test]
fn defined_string_interpolates_and_rescans_its_escapes() {
    let doc = read(".TH T 1\n.ds B \\fBbold\\fP\nx \\*B y\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("x".into()),
            Inline::Space,
            Inline::Strong(vec![Inline::Str("bold".into())]),
            Inline::Space,
            Inline::Str("y".into()),
        ]))
    );
}

#[test]
fn predefined_strings_resolve() {
    let doc = read(".TH T 1\n\\*(lq x \\*(rq \\*(Tm \\*R\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("\u{201c}".into()),
            Inline::Space,
            Inline::Str("x".into()),
            Inline::Space,
            Inline::Str("\u{201d}".into()),
            Inline::Space,
            Inline::Str("\u{2122}".into()),
            Inline::Space,
            Inline::Str("\u{00ae}".into()),
        ]))
    );
}

#[test]
fn accented_special_characters_resolve() {
    let doc = read(".TH T 1\n\\(:a\\(ss\\('e\\(la\\(,c\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![Inline::Str(
            "\u{e4}\u{df}\u{e9}\u{27e8}\u{e7}".into()
        )]))
    );
}

#[test]
fn tab_escape_becomes_a_space() {
    let doc = read(".TH T 1\na\\tb\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("a".into()),
            Inline::Space,
            Inline::Str("b".into()),
        ]))
    );
}

#[test]
fn continuation_escape_is_dropped() {
    // `\c` vanishes; the two text lines still fill with a separating space.
    let doc = read(".TH T 1\nabc\\c\ndef\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("abc".into()),
            Inline::Space,
            Inline::Str("def".into()),
        ]))
    );
}

#[test]
fn zero_width_and_motion_escapes_drop_their_glyphs() {
    // `\z` drops the following glyph, `\u`/`\d` take no argument, `\k` reads a register name.
    let doc = read(".TH T 1\na\\zbc up\\udown\\d mark\\kx end\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("ac".into()),
            Inline::Space,
            Inline::Str("updown".into()),
            Inline::Space,
            Inline::Str("mark".into()),
            Inline::Space,
            Inline::Str("end".into()),
        ]))
    );
}

#[test]
fn trailing_backslash_joins_the_next_line_without_a_space() {
    let doc = read(".TH T 1\nfoo\\\nbar\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![Inline::Str("foobar".into())]))
    );
}

#[test]
fn supplementary_tag_joins_terms_with_a_line_break() {
    let doc = read(".TH T 1\n.TP\n.B \\-a\n.TQ\n.B \\-b\nbody.\n");
    let Some(Block::DefinitionList(items)) = doc.blocks.first() else {
        panic!("expected a definition list");
    };
    assert_eq!(
        items.first().map(|(term, _)| term.clone()),
        Some(vec![
            Inline::Strong(vec![Inline::Str("-a".into())]),
            Inline::LineBreak,
            Inline::Strong(vec![Inline::Str("-b".into())]),
        ])
    );
}

#[test]
fn request_in_link_label_aborts_the_link() {
    // The request voids the link: the label emits as its own block, trailing text is dropped.
    let doc = read(".TH T 1\nbefore\n.UR u\n.B bold\n.UE after\nnext\n");
    assert_eq!(
        doc.blocks,
        vec![
            Block::Para(vec![Inline::Str("before".into())]),
            Block::Para(vec![Inline::Strong(vec![Inline::Str("bold".into())])]),
            Block::Para(vec![Inline::Str("next".into())]),
        ]
    );
}

#[test]
fn link_without_a_terminator_emits_its_label() {
    let doc = read(".TH T 1\n.UR u\nlabel\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![Inline::Str("label".into())]))
    );
}

#[test]
fn whitespace_only_line_does_not_break_the_paragraph() {
    let doc = read(".TH T 1\none\n \ntwo\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("one".into()),
            Inline::Space,
            Inline::Str("two".into()),
        ]))
    );
    assert_eq!(doc.blocks.len(), 1);
}

#[test]
fn lone_whitespace_line_is_an_empty_paragraph() {
    let doc = read(".TH T 1\n \n");
    assert_eq!(doc.blocks.first(), Some(&Block::Para(Vec::new())));
}

#[test]
fn tagged_paragraph_with_no_body_becomes_a_paragraph() {
    let doc = read(".TH T 1\n.TP\n.B \\-x\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![Inline::Strong(vec![Inline::Str(
            "-x".into()
        )])]))
    );
}

#[test]
fn empty_tagged_paragraph_nests_the_following_items() {
    let doc = read(".TH T 1\n.TP\n.B \\-a\n.TP\n.B \\-b\nbody.\n");
    let Some(Block::DefinitionList(items)) = doc.blocks.first() else {
        panic!("expected a definition list");
    };
    assert_eq!(items.len(), 1);
    let nested = items
        .first()
        .and_then(|(_, bodies)| bodies.first())
        .and_then(|blocks| blocks.first());
    assert!(matches!(nested, Some(Block::DefinitionList(_))));
}

#[test]
fn marked_item_with_no_body_keeps_an_empty_paragraph() {
    let doc = read(".TH T 1\n.IP \\(bu\n.IP \\(bu\nsecond.\n");
    let Some(Block::BulletList(items)) = doc.blocks.first() else {
        panic!("expected a bullet list");
    };
    assert_eq!(items.first(), Some(&vec![Block::Para(Vec::new())]));
}

#[test]
fn unmarked_item_with_no_body_contributes_nothing() {
    let doc = read(".TH T 1\n.IP\n");
    assert!(doc.blocks.is_empty());
}

#[test]
fn ascii_identifiers_fold_an_accented_heading() {
    let doc = read_with(
        ".TH T 1\n.SH Café\nx\n",
        Extensions::from_list(&[Extension::AutoIdentifiers, Extension::AsciiIdentifiers]),
    );
    assert!(matches!(
        doc.blocks.first(),
        Some(Block::Header(1, attr, _)) if attr.id == "cafe"
    ));
}

#[test]
fn constant_width_font_escape_becomes_code() {
    let doc = read(".TH T 1\nplain \\f(CWmono\\fP back\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("plain".into()),
            Inline::Space,
            Inline::Code(Box::default(), "mono".into()),
            Inline::Space,
            Inline::Str("back".into()),
        ]))
    );
}

#[test]
fn constant_width_bold_font_wraps_code_in_strong() {
    let doc = read(".TH T 1\n\\f(CBmono\\fP\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![Inline::Strong(vec![Inline::Code(
            Box::default(),
            "mono".into()
        )])]))
    );
}

#[test]
fn user_macro_substitutes_call_arguments() {
    let doc = read(".TH T 1\n.de GREET\nHello \\$1 and \\$2.\n..\n.GREET Alice Bob\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("Hello".into()),
            Inline::Space,
            Inline::Str("Alice".into()),
            Inline::Space,
            Inline::Str("and".into()),
            Inline::Space,
            Inline::Str("Bob.".into()),
        ]))
    );
}

#[test]
fn multi_line_macro_expansion_fills_like_inline_text() {
    let inline = read(".TH T 1\nfirst line\nsecond line\n");
    let via_macro = read(".TH T 1\n.de M\nfirst line\nsecond line\n..\n.M\n");
    assert_eq!(inline.blocks, via_macro.blocks);
}

#[test]
fn nested_macro_call_expands_in_place_preserving_order() {
    let doc =
        read(".TH T 1\n.de INNER\nmiddle\n..\n.de OUTER\nbefore\n.INNER\nafter\n..\n.OUTER\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("before".into()),
            Inline::Space,
            Inline::Str("middle".into()),
            Inline::Space,
            Inline::Str("after".into()),
        ]))
    );
}

#[test]
fn macro_whose_expansion_synthesizes_its_own_call_terminates() {
    // `\$.M` expands to the call line `.M`, a self-call invisible to the recursion guard
    // (each is a fresh invocation); only the document-wide budget stops it.
    let _ = read(".TH T 1\n.de M\ntext\n\\$.M\n..\n.M\n");
}

#[test]
fn macro_argument_doubling_across_synthesized_calls_terminates() {
    // Each re-invocation doubles the argument length; the byte budget must cut it off.
    let _ = read(".TH T 1\n.de M\n\\$.M \"\\$1\\$1\"\n..\n.M xxxxxxxx\n");
}

#[test]
fn macro_expansion_seam_keeps_base_lines_in_order() {
    let doc = read(".TH T 1\n.de M\nexpanded\n..\n.M\nbase line\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("expanded".into()),
            Inline::Space,
            Inline::Str("base".into()),
            Inline::Space,
            Inline::Str("line".into()),
        ]))
    );
}

#[test]
fn conditional_inside_macro_expansion_reprocesses_the_queued_line() {
    // `.ie`/`.el` reprocess the queued expansion in place; the base document's next line survives.
    let doc = read(".TH T 1\n.de M\n.ie n kept\n.el dropped\n..\n.M\nbase line\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("kept".into()),
            Inline::Space,
            Inline::Str("base".into()),
            Inline::Space,
            Inline::Str("line".into()),
        ]))
    );
}

#[test]
fn link_label_spanning_macro_expansion_and_base_document_is_recognized() {
    // Label opens queued, terminator unqueued; the lookahead must chain across that seam.
    let doc = read(".TH T 1\n.de LABEL\n.UR https://example.com\nfirst\n..\n.LABEL\nsecond\n.UE\n");
    let Some(Block::Para(inlines)) = doc.blocks.first() else {
        panic!("expected a paragraph");
    };
    assert!(matches!(
        inlines.first(),
        Some(Inline::Link(_, _, target)) if target.url == "https://example.com"
    ));
}

#[test]
fn doubled_backslash_argument_reference_reduces_like_a_single_one() {
    let single = read(".TH T 1\n.de M\nvalue \\$1\n..\n.M x\n");
    let doubled = read(".TH T 1\n.de M\nvalue \\\\$1\n..\n.M x\n");
    assert_eq!(single.blocks, doubled.blocks);
    assert_eq!(
        single.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("value".into()),
            Inline::Space,
            Inline::Str("x".into()),
        ]))
    );
}

#[test]
fn copy_mode_reduces_an_escaped_backslash_before_an_escape() {
    assert_eq!(reduce_copy_mode("x\\\\(buy"), "x\\(buy");
    assert_eq!(reduce_copy_mode("plain text"), "plain text");
}

#[test]
fn font_macro_with_an_explicit_empty_argument_keeps_its_wrapper() {
    let doc = read(".TH T 1\nbefore\n.B \"\"\nafter\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("before".into()),
            Inline::Space,
            Inline::Strong(Vec::new()),
            Inline::Space,
            Inline::Str("after".into()),
        ]))
    );
}

#[test]
fn font_macro_with_no_argument_takes_the_next_line() {
    let doc = read(".TH T 1\nbefore\n.I\nafter\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("before".into()),
            Inline::Space,
            Inline::Emph(vec![Inline::Str("after".into())]),
        ]))
    );
}

#[test]
fn option_synopsis_brackets_a_bold_option_name() {
    let doc = read(".TH T 1\n.OP \\-o file\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![
            Inline::Str("[".into()),
            Inline::Space,
            Inline::Strong(vec![Inline::Str("-o".into())]),
            Inline::Space,
            Inline::Str("file".into()),
            Inline::Space,
            Inline::Str("]".into()),
        ]))
    );
}

#[test]
fn table_with_a_horizontal_span_degrades_to_a_placeholder() {
    let doc = read(".TH T 1\n.TS\nl s l.\nWide\t\tEnd\none\ttwo\tthree\n.TE\n");
    assert_eq!(
        doc.blocks.first(),
        Some(&Block::Para(vec![Inline::Str("TABLE".into())]))
    );
}

#[test]
fn table_text_block_joins_its_lines() {
    let doc = read(".TH T 1\n.TS\nl l.\nName\tT{\nA long\ndescription\nT}\nLeft\tRight\n.TE\n");
    let Some(Block::Table(table)) = doc.blocks.first() else {
        panic!("expected a table");
    };
    // The two source lines of the `T{ … T}` block join into a single cell.
    let cell_text = format!("{table:?}");
    assert!(cell_text.contains("long"));
    assert!(cell_text.contains("description"));
}

#[test]
fn east_asian_line_breaks_is_accepted_and_inert() {
    let input = ".TH T 1\n.SH H\nplain filled text\n";
    let base = read(input);
    let with = read_with(
        input,
        Extensions::from_list(&[Extension::AutoIdentifiers, Extension::EastAsianLineBreaks]),
    );
    assert_eq!(base.blocks, with.blocks);
}
