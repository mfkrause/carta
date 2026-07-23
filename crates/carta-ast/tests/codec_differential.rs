//! Differential coverage for the hand-written JSON codec: it must serialize byte-for-byte like the
//! derived serde path and accept exactly the same inputs. Every check runs the hand-written codec
//! alongside `serde_json` over the shared AST corpus and a battery of adversarial documents and
//! inputs, so any divergence in bytes or acceptance fails a test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use carta_ast::{
    Alignment, ApiVersion, Attr, Block, Caption, Cell, Citation, CitationMode, ColSpec, ColWidth,
    Document, Format, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType,
    MetaValue, QuoteType, Row, Table, TableBody, TableFoot, TableHead, Target, from_json, to_json,
    to_json_writer,
};

/// The hand-written serializer must produce exactly the bytes the derived serde path produces, on
/// both the string and writer entry points.
fn assert_serialize_parity(document: &Document, label: &str) {
    let mine = to_json(document).expect("hand-written to_json");
    let reference = serde_json::to_string(document).expect("serde to_string");
    assert_eq!(mine, reference, "serialization bytes differ for {label}");

    let mut buffer = Vec::new();
    to_json_writer(document, &mut buffer).expect("hand-written to_json_writer");
    assert_eq!(
        buffer,
        mine.as_bytes(),
        "writer bytes differ from string for {label}"
    );
}

/// Decoding must reproduce the document, and re-encoding must reproduce the exact bytes.
fn assert_roundtrip(document: &Document, label: &str) {
    let json = to_json(document).expect("to_json");
    let decoded = from_json(json.as_bytes()).expect("from_json");
    assert_eq!(&decoded, document, "round-trip value differs for {label}");
}

/// The hand-written reader must accept exactly what the derived serde reader accepts, and agree on
/// the decoded value when both accept.
fn assert_decode_parity(bytes: &[u8], label: &str) {
    let mine = from_json(bytes);
    let reference: Result<Document, _> = serde_json::from_slice(bytes);
    match (mine, reference) {
        (Ok(mine), Ok(reference)) => {
            assert_eq!(mine, reference, "decoded value differs for {label}");
        }
        (Err(_), Err(_)) => {}
        (Ok(_), Err(error)) => {
            panic!("{label}: hand-written reader accepted input the serde reader rejected: {error}")
        }
        (Err(error), Ok(_)) => {
            panic!("{label}: hand-written reader rejected input the serde reader accepted: {error}")
        }
    }
}

fn corpus_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/ast");
    let mut files = Vec::new();
    collect_json(&root, &mut files);
    files.sort();
    files
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_json(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "json") {
            out.push(path);
        }
    }
}

#[test]
fn corpus_matches_serde_both_directions() {
    let files = corpus_files();
    assert!(!files.is_empty(), "no corpus AST files found");
    for path in &files {
        let label = path.display().to_string();
        let bytes = fs::read(path).expect("read corpus file");

        assert_decode_parity(&bytes, &label);

        // Decode with the derived reader so the input to both serializers is identical.
        let document: Document = serde_json::from_slice(&bytes).expect("serde decode corpus");
        assert_serialize_parity(&document, &label);
        assert_roundtrip(&document, &label);
    }
}

fn attr() -> Attr {
    Attr {
        id: "the-id".into(),
        classes: vec!["a".into(), "b".into()],
        attributes: vec![("k".into(), "v".into()), ("width".into(), "3".into())],
    }
}

fn target() -> Target {
    Target {
        url: "https://example.com/x?a=b".into(),
        title: "a \"title\"".into(),
    }
}

fn citation() -> Citation {
    Citation {
        id: "key2020".into(),
        prefix: vec![Inline::Str("see".into())],
        suffix: vec![Inline::Str("p. 3".into())],
        mode: CitationMode::NormalCitation,
        note_num: 7,
        hash: 42,
    }
}

fn every_inline() -> Vec<Inline> {
    vec![
        Inline::Str("word".into()),
        Inline::Emph(vec![Inline::Str("e".into())]),
        Inline::Underline(vec![Inline::Str("u".into())]),
        Inline::Strong(vec![Inline::Str("s".into())]),
        Inline::Strikeout(vec![Inline::Str("x".into())]),
        Inline::Superscript(vec![Inline::Str("sup".into())]),
        Inline::Subscript(vec![Inline::Str("sub".into())]),
        Inline::SmallCaps(vec![Inline::Str("sc".into())]),
        Inline::Quoted(QuoteType::SingleQuote, vec![Inline::Str("q1".into())]),
        Inline::Quoted(QuoteType::DoubleQuote, vec![Inline::Str("q2".into())]),
        Inline::Cite(vec![citation()], vec![Inline::Str("cite".into())]),
        Inline::Code(Box::new(attr()), "let x = 1;".into()),
        Inline::Space,
        Inline::SoftBreak,
        Inline::LineBreak,
        Inline::Math(MathType::InlineMath, "a^2".into()),
        Inline::Math(MathType::DisplayMath, "\\sum_i x_i".into()),
        Inline::RawInline(Format("html".into()), "<br/>".into()),
        Inline::Link(
            Box::new(attr()),
            vec![Inline::Str("link".into())],
            Box::new(target()),
        ),
        Inline::Image(
            Box::new(attr()),
            vec![Inline::Str("alt".into())],
            Box::new(target()),
        ),
        Inline::Note(vec![Block::Para(vec![Inline::Str("note".into())])]),
        Inline::Span(Box::new(attr()), vec![Inline::Str("span".into())]),
    ]
}

fn full_table() -> Table {
    let cell = |text: &str, align: Alignment| Cell {
        attr: attr(),
        align,
        row_span: 1,
        col_span: 1,
        content: vec![Block::Plain(vec![Inline::Str(text.into())])],
    };
    let row = |a: &str, b: &str| Row {
        attr: attr(),
        cells: vec![
            cell(a, Alignment::AlignLeft),
            cell(b, Alignment::AlignRight),
        ],
    };
    Table {
        attr: attr(),
        caption: Caption {
            short: Some(vec![Inline::Str("short".into())]),
            long: vec![Block::Para(vec![Inline::Str("long caption".into())])],
        },
        col_specs: vec![
            ColSpec {
                align: Alignment::AlignCenter,
                width: ColWidth::ColWidth(0.25),
            },
            ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            },
        ],
        head: TableHead {
            attr: attr(),
            rows: vec![row("H1", "H2")],
        },
        bodies: vec![TableBody {
            attr: attr(),
            row_head_columns: 1,
            head: vec![row("bh1", "bh2")],
            body: vec![row("a", "b"), row("c", "d")],
        }],
        foot: TableFoot {
            attr: attr(),
            rows: vec![row("F1", "F2")],
        },
    }
}

fn every_block() -> Vec<Block> {
    let list_attrs = |style, delim| ListAttributes {
        start: 3,
        style,
        delim,
    };
    vec![
        Block::Plain(vec![Inline::Str("plain".into())]),
        Block::Para(every_inline()),
        Block::LineBlock(vec![
            vec![Inline::Str("l1".into())],
            vec![Inline::Str("l2".into())],
        ]),
        Block::CodeBlock(Box::new(attr()), "code\n\tblock".into()),
        Block::RawBlock(Format("latex".into()), "\\emph{x}".into()),
        Block::BlockQuote(vec![Block::Para(vec![Inline::Str("quote".into())])]),
        Block::OrderedList(
            list_attrs(ListNumberStyle::Decimal, ListNumberDelim::Period),
            vec![vec![Block::Plain(vec![Inline::Str("i1".into())])]],
        ),
        Block::OrderedList(
            list_attrs(ListNumberStyle::UpperRoman, ListNumberDelim::TwoParens),
            vec![vec![Block::Plain(vec![Inline::Str("i2".into())])]],
        ),
        Block::BulletList(vec![
            vec![Block::Plain(vec![Inline::Str("b1".into())])],
            vec![Block::Plain(vec![Inline::Str("b2".into())])],
        ]),
        Block::DefinitionList(vec![(
            vec![Inline::Str("term".into())],
            vec![vec![Block::Para(vec![Inline::Str("def".into())])]],
        )]),
        Block::Header(1, Box::new(attr()), vec![Inline::Str("h".into())]),
        Block::Header(i32::MIN, Box::new(attr()), vec![Inline::Str("min".into())]),
        Block::Header(i32::MAX, Box::new(attr()), vec![Inline::Str("max".into())]),
        Block::HorizontalRule,
        Block::Table(Box::new(full_table())),
        Block::Figure(
            Box::new(attr()),
            Box::new(Caption {
                short: None,
                long: vec![Block::Para(vec![Inline::Str("fig".into())])],
            }),
            vec![Block::Para(vec![Inline::Str("figbody".into())])],
        ),
        Block::Div(
            Box::new(attr()),
            vec![Block::Para(vec![Inline::Str("div".into())])],
        ),
    ]
}

fn every_meta() -> BTreeMap<carta_ast::Text, MetaValue> {
    let mut inner = BTreeMap::new();
    inner.insert("nested".into(), MetaValue::MetaBool(false));
    let mut meta = BTreeMap::new();
    meta.insert("map".into(), MetaValue::MetaMap(inner));
    meta.insert(
        "list".into(),
        MetaValue::MetaList(vec![
            MetaValue::MetaBool(true),
            MetaValue::MetaString("s".into()),
        ]),
    );
    meta.insert("flag".into(), MetaValue::MetaBool(true));
    meta.insert("title".into(), MetaValue::MetaString("Doc".into()));
    meta.insert(
        "inlines".into(),
        MetaValue::MetaInlines(vec![Inline::Emph(vec![Inline::Str("i".into())])]),
    );
    meta.insert(
        "blocks".into(),
        MetaValue::MetaBlocks(vec![Block::Para(vec![Inline::Str("b".into())])]),
    );
    meta
}

fn kitchen_sink() -> Document {
    Document {
        api_version: ApiVersion(vec![1, 23, 1, 2, u32::MAX, 0]),
        meta: every_meta(),
        blocks: every_block(),
    }
}

fn nasty_string() -> String {
    let mut text = String::new();
    for byte in 0u8..=0x1F {
        text.push(byte as char);
    }
    text.push('"');
    text.push('\\');
    text.push('/');
    text.push('\u{7F}');
    text.push('😀');
    text.push('e');
    text.push('\u{0301}');
    text.push_str("café — naïve — 日本語");
    text
}

/// Documents whose serialization must be byte-identical, and (unless noted) round-trippable.
fn parity_documents() -> Vec<(String, Document, bool)> {
    let mut docs = Vec::new();
    docs.push(("default".to_string(), Document::default(), true));
    docs.push(("kitchen_sink".to_string(), kitchen_sink(), true));

    docs.push((
        "nasty_string".to_string(),
        Document {
            api_version: ApiVersion::default(),
            meta: {
                let mut meta = BTreeMap::new();
                meta.insert(
                    nasty_string().into(),
                    MetaValue::MetaString(nasty_string().into()),
                );
                meta
            },
            blocks: vec![Block::Para(vec![Inline::Str(nasty_string().into())])],
        },
        true,
    ));

    let widths = [
        ("frac", 0.142_857_142_857_142_85_f64),
        ("tiny", 1e-7),
        ("imprecise_sum", 0.1 + 0.2),
        ("zero", 0.0),
        ("neg_zero", -0.0),
        ("whole", 2.0),
        ("large", 123_456_789.0),
        ("negative", -0.5),
    ];
    for (label, value) in widths {
        docs.push((format!("colwidth_{label}"), colwidth_document(value), true));
    }
    // Non-finite floats serialize (as null) but do not round-trip to an equal value.
    for (label, value) in [
        ("nan", f64::NAN),
        ("inf", f64::INFINITY),
        ("neg_inf", f64::NEG_INFINITY),
    ] {
        docs.push((format!("colwidth_{label}"), colwidth_document(value), false));
    }

    docs.push((
        "int_extremes".to_string(),
        Document {
            api_version: ApiVersion(vec![0, u32::MAX]),
            meta: BTreeMap::new(),
            blocks: vec![
                Block::Header(i32::MIN, Box::default(), vec![]),
                Block::Table(Box::new(Table {
                    bodies: vec![TableBody {
                        attr: Attr::default(),
                        row_head_columns: i32::MAX,
                        head: vec![],
                        body: vec![Row {
                            attr: Attr::default(),
                            cells: vec![Cell {
                                attr: Attr::default(),
                                align: Alignment::AlignDefault,
                                row_span: i32::MIN,
                                col_span: i32::MAX,
                                content: vec![],
                            }],
                        }],
                    }],
                    ..Default::default()
                })),
                Block::Para(vec![Inline::Cite(
                    vec![Citation {
                        id: "k".into(),
                        prefix: vec![],
                        suffix: vec![],
                        mode: CitationMode::AuthorInText,
                        note_num: i32::MIN,
                        hash: i32::MAX,
                    }],
                    vec![],
                )]),
            ],
        },
        true,
    ));

    docs
}

fn colwidth_document(value: f64) -> Document {
    Document {
        api_version: ApiVersion::default(),
        meta: BTreeMap::new(),
        blocks: vec![Block::Table(Box::new(Table {
            col_specs: vec![ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidth(value),
            }],
            ..Default::default()
        }))],
    }
}

#[test]
fn adversarial_documents_serialize_identically() {
    for (label, document, roundtrips) in parity_documents() {
        assert_serialize_parity(&document, &label);
        if roundtrips {
            assert_roundtrip(&document, &label);
            let json = to_json(&document).unwrap();
            assert_decode_parity(json.as_bytes(), &label);
        }
    }
}

fn document_with_inline(inline_json: &str) -> String {
    format!(
        r#"{{"pandoc-api-version":[1,23,1,2],"meta":{{}},"blocks":[{{"t":"Para","c":[{inline_json}]}}]}}"#
    )
}

fn nested_emph(depth: usize) -> String {
    let mut json = String::new();
    for _ in 0..depth {
        json.push_str(r#"{"t":"Emph","c":["#);
    }
    json.push_str(r#"{"t":"Str","c":"x"}"#);
    for _ in 0..depth {
        json.push_str("]}");
    }
    document_with_inline(&json)
}

#[test]
fn adversarial_inputs_accept_identically() {
    let inline = |body: &str| document_with_inline(body);
    let cases: Vec<(String, String)> = vec![
        // Whitespace handling.
        ("leading_ws".into(), "   {\"pandoc-api-version\":[1],\"blocks\":[]}".into()),
        ("trailing_ws".into(), "{\"pandoc-api-version\":[1],\"blocks\":[]}   \n".into()),
        ("inner_ws".into(), "{ \"pandoc-api-version\" : [ 1 , 2 ] , \"blocks\" : [ ] }".into()),
        ("trailing_junk".into(), "{\"pandoc-api-version\":[1],\"blocks\":[]}x".into()),
        // Document shape.
        ("meta_absent".into(), r#"{"pandoc-api-version":[1],"blocks":[]}"#.into()),
        ("meta_before_version".into(), r#"{"meta":{},"pandoc-api-version":[1],"blocks":[]}"#.into()),
        ("no_version".into(), r#"{"meta":{},"blocks":[]}"#.into()),
        ("no_blocks".into(), r#"{"pandoc-api-version":[1],"meta":{}}"#.into()),
        ("unknown_field".into(), r#"{"pandoc-api-version":[1],"blocks":[],"z":1}"#.into()),
        ("dup_version".into(), r#"{"pandoc-api-version":[1],"pandoc-api-version":[1],"blocks":[]}"#.into()),
        ("dup_blocks".into(), r#"{"pandoc-api-version":[1],"blocks":[],"blocks":[]}"#.into()),
        ("not_object".into(), "[]".into()),
        ("empty_object".into(), "{}".into()),
        ("empty_input".into(), String::new()),
        ("just_ws".into(), "   ".into()),
        // Tagged-enum ordering and content presence.
        ("tag_only".into(), inline(r#"{"t":"Space"}"#)),
        ("tag_null".into(), inline(r#"{"t":"Space","c":null}"#)),
        ("unit_with_content".into(), inline(r#"{"t":"Space","c":5}"#)),
        ("c_before_t".into(), inline(r#"{"c":"hi","t":"Str"}"#)),
        ("unknown_key_after".into(), inline(r#"{"t":"Str","c":"x","extra":1}"#)),
        ("unknown_key_before".into(), inline(r#"{"extra":[1,2],"t":"Str","c":"x"}"#)),
        ("unknown_key_between".into(), inline(r#"{"t":"Str","extra":{"a":1},"c":"x"}"#)),
        ("dup_t".into(), inline(r#"{"t":"Str","t":"Str","c":"x"}"#)),
        ("dup_c".into(), inline(r#"{"t":"Str","c":"a","c":"b"}"#)),
        ("missing_c".into(), inline(r#"{"t":"Str"}"#)),
        ("missing_t".into(), inline(r#"{"c":"x"}"#)),
        ("empty_enum".into(), inline("{}")),
        ("unknown_variant".into(), inline(r#"{"t":"Nope","c":1}"#)),
        // Strings and escapes.
        ("valid_escapes".into(), inline(r#"{"t":"Str","c":"\"\\\/\b\f\n\r\tX"}"#)),
        ("unicode_escape".into(), inline(r#"{"t":"Str","c":"Aé中"}"#)),
        ("surrogate_pair".into(), inline(r#"{"t":"Str","c":"😀"}"#)),
        ("lone_high_surrogate".into(), inline(r#"{"t":"Str","c":"\uD800"}"#)),
        ("lone_low_surrogate".into(), inline(r#"{"t":"Str","c":"\uDC00"}"#)),
        ("high_then_non_low".into(), inline(r#"{"t":"Str","c":"\uD800A"}"#)),
        ("bad_escape".into(), inline(r#"{"t":"Str","c":"\x41"}"#)),
        ("short_unicode".into(), inline(r#"{"t":"Str","c":"\u41"}"#)),
        ("raw_tab".into(), inline("{\"t\":\"Str\",\"c\":\"a\tb\"}")),
        ("raw_newline".into(), inline("{\"t\":\"Str\",\"c\":\"a\nb\"}")),
        ("del_char".into(), inline("{\"t\":\"Str\",\"c\":\"a\u{7f}b\"}")),
        ("emoji".into(), inline("{\"t\":\"Str\",\"c\":\"😀\"}")),
        ("str_null".into(), inline(r#"{"t":"Str","c":null}"#)),
        ("str_number".into(), inline(r#"{"t":"Str","c":5}"#)),
        ("str_bool".into(), inline(r#"{"t":"Str","c":true}"#)),
        ("unterminated_string".into(), inline(r#"{"t":"Str","c":"abc}]}"#)),
        // Numbers.
        ("neg_version".into(), r#"{"pandoc-api-version":[-1],"blocks":[]}"#.into()),
        ("float_version".into(), r#"{"pandoc-api-version":[1.0],"blocks":[]}"#.into()),
        ("exp_version".into(), r#"{"pandoc-api-version":[1e2],"blocks":[]}"#.into()),
        ("over_u32".into(), r#"{"pandoc-api-version":[4294967296],"blocks":[]}"#.into()),
        ("max_u32".into(), r#"{"pandoc-api-version":[4294967295],"blocks":[]}"#.into()),
        ("leading_zero".into(), r#"{"pandoc-api-version":[01],"blocks":[]}"#.into()),
        ("plus_number".into(), r#"{"pandoc-api-version":[+1],"blocks":[]}"#.into()),
        ("version_string".into(), r#"{"pandoc-api-version":["1"],"blocks":[]}"#.into()),
        ("neg_header".into(), inline_block(r#"{"t":"Header","c":[-3,["",[],[]],[]]}"#)),
        ("float_header".into(), inline_block(r#"{"t":"Header","c":[1.0,["",[],[]],[]]}"#)),
        ("i32_min_header".into(), inline_block(r#"{"t":"Header","c":[-2147483648,["",[],[]],[]]}"#)),
        ("i32_over_header".into(), inline_block(r#"{"t":"Header","c":[2147483648,["",[],[]],[]]}"#)),
        ("colwidth_int".into(), colwidth_input("1")),
        ("colwidth_float".into(), colwidth_input("0.5")),
        ("colwidth_exp".into(), colwidth_input("1e-7")),
        ("colwidth_neg".into(), colwidth_input("-0.5")),
        // Array-shaped records.
        ("attr_short".into(), inline_block(r#"{"t":"CodeBlock","c":[["",[]],"x"]}"#)),
        ("attr_long".into(), inline_block(r#"{"t":"CodeBlock","c":[["",[],[],[]],"x"]}"#)),
        ("tuple_short".into(), inline_block(r#"{"t":"CodeBlock","c":[["",[],[]]]}"#)),
        ("tuple_long".into(), inline_block(r#"{"t":"CodeBlock","c":[["",[],[]],"x","y"]}"#)),
        // Caption optional short.
        ("caption_null_short".into(), table_input("[null,[]]")),
        ("caption_some_short".into(), table_input(r#"[[{"t":"Str","c":"s"}],[]]"#)),
        // Citation object.
        ("citation_ok".into(), citation_input(false, false)),
        ("citation_unknown".into(), citation_input(true, false)),
        ("citation_dup".into(), citation_input(false, true)),
        ("citation_reordered".into(), citation_reordered()),
        // Meta.
        ("dup_meta_key".into(), r#"{"pandoc-api-version":[1],"meta":{"k":{"t":"MetaBool","c":true},"k":{"t":"MetaBool","c":false}},"blocks":[]}"#.into()),
        // Nesting depth: shallow accepted, deep rejected by both.
        ("nested_50".into(), nested_emph(50)),
        ("nested_500".into(), nested_emph(500)),
        ("nested_5000".into(), nested_emph(5000)),
    ];

    for (label, json) in cases {
        assert_decode_parity(json.as_bytes(), &label);
    }
}

fn inline_block(block_json: &str) -> String {
    format!(r#"{{"pandoc-api-version":[1,23,1,2],"meta":{{}},"blocks":[{block_json}]}}"#)
}

fn colwidth_input(number: &str) -> String {
    inline_block(&format!(
        r#"{{"t":"Table","c":[["",[],[]],[null,[]],[[{{"t":"AlignDefault"}},{{"t":"ColWidth","c":{number}}}]],[["",[],[]],[]],[],[["",[],[]],[]]]}}"#
    ))
}

fn table_input(caption: &str) -> String {
    inline_block(&format!(
        r#"{{"t":"Table","c":[["",[],[]],{caption},[],[["",[],[]],[]],[],[["",[],[]],[]]]}}"#
    ))
}

fn citation_input(unknown_field: bool, duplicate: bool) -> String {
    let mut fields = String::from(
        r#""citationId":"k","citationPrefix":[],"citationSuffix":[],"citationMode":{"t":"NormalCitation"},"citationNoteNum":0,"citationHash":0"#,
    );
    if unknown_field {
        fields.push_str(r#","bogus":1"#);
    }
    if duplicate {
        fields.push_str(r#","citationId":"k2""#);
    }
    inline_block(&format!(
        r#"{{"t":"Para","c":[{{"t":"Cite","c":[[{{{fields}}}],[]]}}]}}"#
    ))
}

fn citation_reordered() -> String {
    inline_block(
        r#"{"t":"Para","c":[{"t":"Cite","c":[[{"citationHash":1,"citationMode":{"t":"AuthorInText"},"citationId":"k","citationNoteNum":2,"citationSuffix":[],"citationPrefix":[]}],[]]}]}"#,
    )
}
