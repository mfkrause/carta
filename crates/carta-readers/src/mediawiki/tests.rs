//! Unit tests for the wikitext reader.

use super::*;
use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Inline, Row, Table, TableBody,
    TableHead, Target,
};

use super::links::is_scheme;
use super::lists::default_list_attrs;
use super::tags::close_tag;

fn parse(input: &str) -> Vec<Block> {
    let mut options = ReaderOptions::default();
    options.extensions = Extensions::from_list(&[Extension::AutoIdentifiers]);
    MediawikiReader
        .read(input, &options)
        .expect("read should not fail")
        .blocks
}

fn parse_gfm(input: &str) -> Vec<Block> {
    let mut options = ReaderOptions::default();
    options.extensions = Extensions::from_list(&[Extension::GfmAutoIdentifiers]);
    MediawikiReader.read(input, &options).expect("read").blocks
}

#[test]
fn doi_and_javascript_are_recognized_schemes() {
    assert!(is_scheme("doi"));
    assert!(is_scheme("javascript"));
    assert!(is_scheme("DOI"));
    assert!(is_scheme("http"));
    assert!(!is_scheme("notascheme"));
}

fn cell_with(content: Vec<Block>) -> Cell {
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content,
    }
}

fn data_cell(text: &str) -> Cell {
    cell_with(vec![Block::Para(vec![Inline::Str(text.into())])])
}

fn table_row(cells: Vec<Cell>) -> Row {
    Row {
        attr: Attr::default(),
        cells,
    }
}

fn default_col() -> ColSpec {
    ColSpec {
        align: Alignment::AlignDefault,
        width: ColWidth::ColWidthDefault,
    }
}

#[test]
fn table_markup_becomes_a_table() {
    assert_eq!(
        parse("{|\n! Header\n|-\n| Cell\n|}\nafter"),
        vec![
            Block::Table(Box::new(Table {
                col_specs: vec![default_col()],
                head: TableHead {
                    rows: vec![table_row(vec![data_cell("Header")])],
                    ..Default::default()
                },
                bodies: vec![TableBody {
                    body: vec![table_row(vec![data_cell("Cell")])],
                    ..Default::default()
                }],
                ..Default::default()
            })),
            Block::Para(vec![Inline::Str("after".into())]),
        ]
    );
}

#[test]
fn unterminated_table_markup_does_not_panic() {
    assert_eq!(
        parse("{|"),
        vec![Block::Table(Box::new(Table {
            bodies: vec![TableBody {
                body: vec![table_row(Vec::new())],
                ..Default::default()
            }],
            ..Default::default()
        }))]
    );
}

#[test]
fn a_huge_colspan_is_clamped_and_does_not_blow_up_the_grid() {
    // The first row fixes the grid width, so an oversized colspan is clamped, not trusted.
    let blocks = parse("{|\n| colspan=222222222 | wide\n|-\n| a\n|}");
    let Some(Block::Table(table)) = blocks.first() else {
        panic!("expected a table, got {blocks:?}");
    };
    assert_eq!(table.col_specs.len(), 1000);
    let first_cell = table
        .bodies
        .first()
        .and_then(|body| body.body.first())
        .and_then(|row| row.cells.first())
        .expect("table should have a first cell");
    assert_eq!(first_cell.col_span, 1000);
    assert!(first_cell.attr.attributes.is_empty());
}

#[test]
fn nested_table_markup_closes_at_the_outer_marker() {
    let inner = Block::Table(Box::new(Table {
        col_specs: vec![default_col()],
        bodies: vec![TableBody {
            body: vec![table_row(vec![data_cell("inner")])],
            ..Default::default()
        }],
        ..Default::default()
    }));
    assert_eq!(
        parse("{|\n|\n{|\n| inner\n|}\n|}"),
        vec![Block::Table(Box::new(Table {
            col_specs: vec![default_col()],
            bodies: vec![TableBody {
                body: vec![table_row(vec![cell_with(vec![inner])])],
                ..Default::default()
            }],
            ..Default::default()
        }))]
    );
}

#[test]
fn paragraph_joins_lines_with_soft_breaks() {
    assert_eq!(
        parse("one two\nthree"),
        vec![Block::Para(vec![
            Inline::Str("one".into()),
            Inline::Space,
            Inline::Str("two".into()),
            Inline::SoftBreak,
            Inline::Str("three".into()),
        ])]
    );
}

#[test]
fn emphasis_runs_decompose() {
    assert_eq!(
        parse("''i'' '''b''' '''''both'''''"),
        vec![Block::Para(vec![
            Inline::Emph(vec![Inline::Str("i".into())]),
            Inline::Space,
            Inline::Strong(vec![Inline::Str("b".into())]),
            Inline::Space,
            Inline::Strong(vec![Inline::Emph(vec![Inline::Str("both".into())])]),
        ])]
    );
}

#[test]
fn header_carries_mediawiki_identifier() {
    assert_eq!(
        parse("== Hello World =="),
        vec![Block::Header(
            2,
            Box::new(Attr {
                id: "hello_world".into(),
                classes: vec![],
                attributes: vec![],
            }),
            vec![
                Inline::Str("Hello".into()),
                Inline::Space,
                Inline::Str("World".into()),
            ],
        )]
    );
}

#[test]
fn duplicate_identifiers_are_suffixed() {
    let blocks = parse("== Dup ==\n== Dup ==");
    let ids: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Header(_, attr, _) => Some(attr.id.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["dup".to_string(), "dup_1".to_string()]);
}

#[test]
fn gfm_identifier_scheme_uses_hyphens() {
    let blocks = parse_gfm("== Hello World ==");
    match blocks.first() {
        Some(Block::Header(_, attr, _)) => assert_eq!(attr.id, "hello-world"),
        other => panic!("expected header, got {other:?}"),
    }
}

#[test]
fn empty_identifier_falls_back_to_section() {
    let blocks = parse("== !!! ==\n== ??? ==");
    let ids: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Header(_, attr, _) => Some(attr.id.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["section".to_string(), "section_1".to_string()]);
}

#[test]
fn malformed_header_is_a_paragraph() {
    assert_eq!(
        parse("== a=b =="),
        vec![Block::Para(vec![
            Inline::Str("==".into()),
            Inline::Space,
            Inline::Str("a=b".into()),
            Inline::Space,
            Inline::Str("==".into()),
        ])]
    );
}

#[test]
fn header_leftover_becomes_paragraph() {
    assert_eq!(
        parse("== H ==="),
        vec![
            Block::Header(
                2,
                Box::new(Attr {
                    id: "h".into(),
                    classes: vec![],
                    attributes: vec![],
                }),
                vec![Inline::Str("H".into())],
            ),
            Block::Para(vec![Inline::Str("=".into())]),
        ]
    );
}

#[test]
fn nested_bullets_and_ordered() {
    assert_eq!(
        parse("* a\n** b\n*# c"),
        vec![Block::BulletList(vec![vec![
            Block::Plain(vec![Inline::Str("a".into())]),
            Block::BulletList(vec![vec![Block::Plain(vec![Inline::Str("b".into())])]]),
            Block::OrderedList(
                default_list_attrs(),
                vec![vec![Block::Plain(vec![Inline::Str("c".into())])]]
            ),
        ]])]
    );
}

#[test]
fn definition_list_splits_inline_definition() {
    assert_eq!(
        parse("; term : def"),
        vec![Block::DefinitionList(vec![(
            vec![Inline::Str("term".into())],
            vec![vec![Block::Plain(vec![Inline::Str("def".into())])]],
        )])]
    );
}

#[test]
fn internal_link_with_trail() {
    assert_eq!(
        parse("[[Page]]s"),
        vec![Block::Para(vec![Inline::Link(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["wikilink".into()],
                attributes: vec![],
            }),
            vec![Inline::Str("Pages".into())],
            Box::new(Target {
                url: "Page".into(),
                title: "Page".into(),
            }),
        )])]
    );
}

#[test]
fn lone_file_embed_becomes_a_figure() {
    assert_eq!(
        parse("[[File:Foo.jpg|thumb|A caption]]"),
        vec![Block::Figure(
            Box::default(),
            Box::new(Caption {
                short: None,
                long: vec![Block::Plain(vec![
                    Inline::Str("A".into()),
                    Inline::Space,
                    Inline::Str("caption".into()),
                ])],
            }),
            vec![Block::Plain(vec![Inline::Image(
                Box::default(),
                vec![],
                Box::new(Target {
                    url: "Foo.jpg".into(),
                    title: "A caption".into(),
                }),
            )])],
        )]
    );
}

#[test]
fn embed_without_caption_defaults_to_the_file_name() {
    assert_eq!(
        parse("[[Image:My Photo.jpg]]"),
        vec![Block::Figure(
            Box::default(),
            Box::new(Caption {
                short: None,
                long: vec![Block::Plain(vec![Inline::Str("My_Photo.jpg".into())])],
            }),
            vec![Block::Plain(vec![Inline::Image(
                Box::default(),
                vec![],
                Box::new(Target {
                    url: "My_Photo.jpg".into(),
                    title: "My_Photo.jpg".into(),
                }),
            )])],
        )]
    );
}

#[test]
fn embed_size_parameters_set_width_and_height() {
    assert_eq!(
        parse("[[File:Foo.jpg|100x200px|cap]]"),
        vec![Block::Figure(
            Box::default(),
            Box::new(Caption {
                short: None,
                long: vec![Block::Plain(vec![Inline::Str("cap".into())])],
            }),
            vec![Block::Plain(vec![Inline::Image(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec![],
                    attributes: vec![
                        ("width".into(), "100".into()),
                        ("height".into(), "200".into()),
                    ],
                }),
                vec![],
                Box::new(Target {
                    url: "Foo.jpg".into(),
                    title: "cap".into(),
                }),
            )])],
        )]
    );
}

#[test]
fn inline_embed_stays_an_image_not_a_figure() {
    assert_eq!(
        parse("x [[File:Foo.jpg|cap]]"),
        vec![Block::Para(vec![
            Inline::Str("x".into()),
            Inline::Space,
            Inline::Image(
                Box::default(),
                vec![Inline::Str("cap".into())],
                Box::new(Target {
                    url: "Foo.jpg".into(),
                    title: "cap".into(),
                }),
            ),
        ])]
    );
}

#[test]
fn empty_file_embed_is_an_ordinary_wikilink() {
    assert_eq!(
        parse("[[File:]]"),
        vec![Block::Para(vec![Inline::Link(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["wikilink".into()],
                attributes: vec![],
            }),
            vec![Inline::Str("File:".into())],
            Box::new(Target {
                url: "File:".into(),
                title: "File:".into(),
            }),
        )])]
    );
}

#[test]
fn external_links_number_and_label() {
    assert_eq!(
        parse("[http://x.com lbl] [http://y.com]"),
        vec![Block::Para(vec![
            Inline::Link(
                Box::default(),
                vec![Inline::Str("lbl".into())],
                Box::new(Target {
                    url: "http://x.com".into(),
                    title: carta_ast::Text::default(),
                }),
            ),
            Inline::Space,
            Inline::Link(
                Box::default(),
                vec![Inline::Str("1".into())],
                Box::new(Target {
                    url: "http://y.com".into(),
                    title: carta_ast::Text::default(),
                }),
            ),
        ])]
    );
}

#[test]
fn bare_url_trims_trailing_punctuation() {
    assert_eq!(
        parse("see http://x.com."),
        vec![Block::Para(vec![
            Inline::Str("see".into()),
            Inline::Space,
            Inline::Link(
                Box::default(),
                vec![Inline::Str("http://x.com".into())],
                Box::new(Target {
                    url: "http://x.com".into(),
                    title: carta_ast::Text::default(),
                }),
            ),
            Inline::Str(".".into()),
        ])]
    );
}

#[test]
fn entities_are_decoded_in_text() {
    assert_eq!(
        parse("AT&amp;T &copy;"),
        vec![Block::Para(vec![
            Inline::Str("AT&T".into()),
            Inline::Space,
            Inline::Str("\u{a9}".into()),
        ])]
    );
}

#[test]
fn nowiki_is_literal_text() {
    assert_eq!(
        parse("<nowiki>'''raw'''</nowiki>"),
        vec![Block::Para(vec![Inline::Str("'''raw'''".into())])]
    );
}

#[test]
fn reference_becomes_a_note() {
    assert_eq!(
        parse("x<ref>note</ref>"),
        vec![Block::Para(vec![
            Inline::Str("x".into()),
            Inline::Note(vec![Block::Plain(vec![Inline::Str("note".into())])]),
        ])]
    );
}

#[test]
fn code_tag_decodes_entities() {
    assert_eq!(
        parse("<code>a &amp; b</code>"),
        vec![Block::Para(vec![Inline::Code(
            Box::default(),
            "a & b".into()
        )])]
    );
}

#[test]
fn unknown_tag_passes_through_as_raw_html() {
    assert_eq!(
        parse("<b>x</b>"),
        vec![Block::Para(vec![
            raw_html("<b>".into()),
            Inline::Str("x".into()),
            raw_html("</b>".into()),
        ])]
    );
}

#[test]
fn whole_line_comment_is_removed_with_its_newline() {
    assert_eq!(
        parse("x\n<!--c-->\ny"),
        vec![Block::Para(vec![
            Inline::Str("x".into()),
            Inline::SoftBreak,
            Inline::Str("y".into()),
        ])]
    );
}

#[test]
fn inline_comment_becomes_a_space() {
    assert_eq!(
        parse("a<!--c-->b"),
        vec![Block::Para(vec![
            Inline::Str("a".into()),
            Inline::Space,
            Inline::Str("b".into()),
        ])]
    );
}

#[test]
fn syntax_highlight_block_keeps_language_and_content() {
    assert_eq!(
        parse("<syntaxhighlight lang=\"rust\">\nfn main(){}\n</syntaxhighlight>"),
        vec![Block::CodeBlock(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["rust".into()],
                attributes: vec![],
            }),
            "fn main(){}".into(),
        )]
    );
}

#[test]
fn horizontal_rule_requires_a_dashes_only_line() {
    assert_eq!(parse("----"), vec![Block::HorizontalRule]);
    assert_eq!(
        parse("----foo"),
        vec![Block::Para(vec![Inline::Str("----foo".into())])]
    );
}

#[test]
fn preformatted_lines_become_code() {
    assert_eq!(
        parse(" indented  line"),
        vec![Block::Para(vec![Inline::Code(
            Box::default(),
            "indented\u{a0}\u{a0}line".into()
        )])]
    );
}

#[test]
fn preformatted_preserves_markup_and_spacing() {
    assert_eq!(
        parse(" a '''b''' c"),
        vec![Block::Para(vec![
            Inline::Code(Box::default(), "a\u{a0}".into()),
            Inline::Strong(vec![Inline::Code(Box::default(), "b".into())]),
            Inline::Code(Box::default(), "\u{a0}c".into()),
        ])]
    );
}

#[test]
fn block_template_is_raw_then_trailing_paragraph() {
    assert_eq!(
        parse("{{tpl}} trailing"),
        vec![
            Block::RawBlock(format_mediawiki(), "{{tpl}}".into()),
            Block::Para(vec![Inline::Str("trailing".into())]),
        ]
    );
}

/// Reads with the default option set and reports only whether the read completed without error,
/// so a deeply nested input can be checked to parse without panicking.
fn reads_ok(input: &str) -> bool {
    MediawikiReader
        .read(input, &ReaderOptions::default())
        .is_ok()
}

#[test]
fn adversarially_nested_wiki_list_does_not_panic() {
    let mut input = String::new();
    for n in 1..4000 {
        input.push_str(&"*".repeat(n));
        input.push_str(" item\n");
    }
    assert!(reads_ok(&input));
    let single = format!("{} item", "*".repeat(20_000));
    assert!(reads_ok(&single));
}

#[test]
fn adversarially_nested_tables_do_not_panic() {
    let input = format!("{}| x\n{}", "{|\n".repeat(4000), "|}\n".repeat(4000));
    assert!(reads_ok(&input));
}

#[test]
fn adversarially_nested_html_list_does_not_panic() {
    let input = format!("{}x{}", "<ul><li>".repeat(4000), "</li></ul>".repeat(4000));
    assert!(reads_ok(&input));
}

#[test]
fn adversarially_nested_refs_do_not_panic() {
    let input = format!("{}x{}", "a<ref>".repeat(4000), "</ref>".repeat(4000));
    assert!(reads_ok(&input));
}

#[test]
fn stacked_header_lines_do_not_blow_up() {
    // without the memoized region scan, stacked `=` lines would recompute each region per enclosing region, exponential in depth
    let input = "== ~iT\n= w e\n= J".repeat(4000);
    assert!(reads_ok(&input));
}

#[test]
fn unclosed_ref_has_no_close_tag() {
    let chars: Vec<char> = "<ref>body with no closer".chars().collect();
    assert_eq!(close_tag(&chars, 5, "ref"), None);
}

#[test]
fn repeated_unterminated_open_stays_literal() {
    let input = "<a".repeat(2000);
    let blocks = parse(&input);
    assert_eq!(blocks, vec![Block::Para(vec![Inline::Str(input.into())])]);
}
