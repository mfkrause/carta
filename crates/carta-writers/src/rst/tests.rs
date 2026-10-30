use super::*;

use super::block::{join_loose_items, show_dimension};
use super::inline::escape;
use carta_ast::{Attr, ColWidth, Row, Table, Target};

fn unit(simple: bool, text: &str) -> (bool, String) {
    (simple, text.to_string())
}

#[test]
fn escape_flanking_tests_see_the_neighbor_before_a_trigger() {
    assert_eq!(escape("text *star", false), "text \\*star");
    assert_eq!(escape("text* tail", false), "text\\* tail");
    assert_eq!(escape("a*b", false), "a*b");
}

#[test]
fn escape_underscore_depends_on_both_neighbors() {
    assert_eq!(escape("snake_case", false), "snake_case");
    assert_eq!(escape("word_ end", false), "word\\_ end");
    assert_eq!(escape("tail_", false), "tail\\_");
}

#[test]
fn escape_multibyte_neighbors_survive_the_verbatim_copy() {
    assert_eq!(escape("caf\u{e9}_x", false), "caf\u{e9}_x");
    assert_eq!(escape("\u{e9} *x", true), "\u{e9} \\*x");
}

#[test]
fn smart_punctuation_keeps_literal_dash_and_dot_runs() {
    for (text, expected) in [
        ("a--b", "a\\--b"),
        ("a---b", "a\\-\\--b"),
        ("a...b", "a\\...b"),
        ("a....b", "a\\.\\...b"),
        ("\u{e9}...x--y", "\u{e9}\\...x\\--y"),
    ] {
        assert_eq!(escape(text, true), expected);
        assert_eq!(escape(text, false), text);
    }
    assert_eq!(escape("\u{2013} \u{2014} \u{2026}", true), "-- --- ...");
}

#[test]
fn smart_punctuation_leaves_code_literal() {
    let mut options = WriterOptions::default();
    options.extensions.insert(Extension::Smart);
    let document = Document {
        blocks: vec![
            Block::Para(vec![Inline::Str("-- ...".into())]),
            Block::CodeBlock(Box::default(), "-- ...".into()),
        ],
        ..Document::default()
    };
    let output = RstWriter.write(&document, &options).unwrap();
    assert!(output.contains("\\-- \\..."));
    assert!(output.contains("-- ..."));
}

#[test]
fn all_single_line_units_join_tightly() {
    let joined = join_loose_items(vec![unit(true, "a"), unit(true, "b"), unit(true, "c")]);
    assert_eq!(joined, "a\nb\nc");
}

#[test]
fn all_multi_line_units_join_loosely() {
    let joined = join_loose_items(vec![unit(false, "a"), unit(false, "b")]);
    assert_eq!(joined, "a\n\nb");
}

#[test]
fn the_gap_below_a_unit_follows_that_unit_not_the_whole_list() {
    let joined = join_loose_items(vec![
        unit(true, "one"),
        unit(false, "two\n\n  - sub"),
        unit(true, "three"),
    ]);
    assert_eq!(joined, "one\ntwo\n\n  - sub\n\nthree");
}

#[test]
fn empty_units_are_dropped_and_do_not_set_the_gap() {
    let joined = join_loose_items(vec![unit(false, ""), unit(true, "a"), unit(true, "b")]);
    assert_eq!(joined, "a\nb");
}

#[test]
fn show_dimension_normalizes_lengths() {
    assert_eq!(show_dimension("1.0in"), Some("1in".to_owned()));
    assert_eq!(show_dimension("2in"), Some("2in".to_owned()));
    assert_eq!(show_dimension("0.5in"), Some("0.5in".to_owned()));
    assert_eq!(show_dimension("50%"), Some("50.0%".to_owned()));
    assert_eq!(show_dimension("200px"), Some("200px".to_owned()));
    assert_eq!(show_dimension("200"), Some("200px".to_owned()));
    assert_eq!(show_dimension("4ex"), None);
}

#[test]
fn substitution_names_stay_unique() {
    let mut state = State::default();
    assert_eq!(state.substitution_name("image".to_owned()), "image");
    assert_eq!(state.substitution_name("image".to_owned()), "image1");
    assert_eq!(state.substitution_name("logo".to_owned()), "logo");
    assert_eq!(state.substitution_name(String::new()), "image2");
    assert_eq!(state.substitution_name("image".to_owned()), "image3");
}

#[test]
fn image_run_names_dedupe_across_registrations() {
    // Shared image labels need distinct substitution names.
    let link = Inline::Link(
        Box::default(),
        vec![Inline::Str("L".into())],
        Box::new(Target {
            url: "http://x".into(),
            ..Default::default()
        }),
    );
    let with_link = Inline::Image(
        Box::default(),
        vec![Inline::Str("dup".into()), Inline::Space, link],
        Box::new(Target {
            url: "a.png".into(),
            ..Default::default()
        }),
    );
    let plain = Inline::Image(
        Box::default(),
        vec![Inline::Str("dup".into())],
        Box::new(Target {
            url: "b.png".into(),
            ..Default::default()
        }),
    );
    let doc = Document {
        blocks: vec![Block::Para(vec![with_link]), Block::Para(vec![plain])],
        ..Document::default()
    };
    let out = RstWriter.write(&doc, &WriterOptions::default()).unwrap();
    assert_eq!(out.matches(".. |dup| image::").count(), 1);
    assert!(out.contains(".. |image1| image:: b.png"));
}

#[test]
fn table_measurement_keeps_note_numbers_and_image_names() {
    use carta_ast::{Alignment, Cell, ColSpec, TableBody};

    let image = Inline::Image(
        Box::default(),
        Vec::new(),
        Box::new(Target {
            url: "a.png".into(),
            ..Target::default()
        }),
    );
    let note = Inline::Note(vec![Block::Para(vec![Inline::Str("note".into())])]);
    let cell = Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content: vec![Block::Para(vec![
            Inline::Str("long cell text".into()),
            Inline::Space,
            note,
            Inline::Space,
            image,
        ])],
    };
    let spec = ColSpec {
        align: Alignment::AlignDefault,
        width: ColWidth::ColWidthDefault,
    };
    let document = Document {
        blocks: vec![Block::Table(Box::new(Table {
            col_specs: vec![spec.clone(), spec],
            bodies: vec![TableBody {
                body: vec![Row {
                    cells: vec![cell.clone(), cell],
                    ..Row::default()
                }],
                ..TableBody::default()
            }],
            ..Table::default()
        }))],
        ..Document::default()
    };
    for (columns, border) in [(72, '='), (10, '+')] {
        let mut options = WriterOptions::default();
        options.columns = Some(columns);
        let output = RstWriter.write(&document, &options).unwrap();
        assert!(output.starts_with(border));
        for number in 1..=2 {
            assert_eq!(output.matches(&format!(" [{number}]_")).count(), 1);
            assert_eq!(output.matches(&format!(".. [{number}]\n")).count(), 1);
            assert_eq!(
                output
                    .matches(&format!(".. |image{number}| image:: a.png"))
                    .count(),
                1
            );
        }
        assert!(!output.contains(".. [3]"));
        assert!(!output.contains("image3"));
    }
}

#[test]
fn deeply_nested_tables_render_without_compounding_measurement() {
    use carta_ast::{Alignment, Cell, ColSpec, TableBody};

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

    let mut block = Block::Para(vec![Inline::Str("innermost".into())]);
    for _ in 0..16 {
        block = nested_table(vec![block]);
    }
    let doc = Document {
        blocks: vec![block],
        ..Document::default()
    };
    RstWriter
        .write(&doc, &WriterOptions::default())
        .expect("write");
}

#[test]
fn deeply_nested_links_render_each_label_once() {
    // a probing render per ancestor would be exponential down a chain of links in labels
    let mut inline = Inline::Str("innermost".into());
    for level in 0..24 {
        inline = Inline::Link(
            Box::default(),
            vec![Inline::Str("label".into()), Inline::Space, inline],
            Box::new(Target {
                url: format!("https://example.com/{level}").into(),
                ..Target::default()
            }),
        );
    }
    let doc = Document {
        blocks: vec![Block::Para(vec![inline])],
        ..Document::default()
    };
    RstWriter
        .write(&doc, &WriterOptions::default())
        .expect("write");
}

#[test]
fn a_run_flush_against_a_math_directive_keeps_the_null_separator() {
    use carta_ast::MathType;

    let math = |tex: &str| Inline::Math(MathType::DisplayMath, tex.into());
    let doc = Document {
        blocks: vec![Block::Para(vec![
            Inline::Str("a".into()),
            math("x"),
            Inline::Str("b".into()),
            Inline::Space,
            math("y"),
            Inline::Str("c".into()),
        ])],
        ..Document::default()
    };
    let out = RstWriter.write(&doc, &WriterOptions::default()).unwrap();
    assert_eq!(out, "a\\ \n\n.. math:: x\n\n\\ b\n\n.. math:: y\n\n\\ c");
}

#[test]
fn only_the_first_math_directive_hands_a_spaced_run_back_the_flow() {
    use carta_ast::MathType;

    let math = |tex: &str| Inline::Math(MathType::DisplayMath, tex.into());
    let spaced = |blocks| Document {
        blocks,
        ..Document::default()
    };
    let doc = spaced(vec![Block::Para(vec![
        Inline::Str("a".into()),
        Inline::Space,
        math("x"),
        Inline::Space,
        Inline::Str("b".into()),
        Inline::Space,
        math("y"),
        Inline::Space,
        Inline::Str("c".into()),
    ])]);
    let out = RstWriter.write(&doc, &WriterOptions::default()).unwrap();
    assert_eq!(out, "a\n\n.. math:: x\n\n\\ b\n\n.. math:: y\n\nc");

    let broken = spaced(vec![Block::Para(vec![
        Inline::Str("a".into()),
        Inline::Space,
        math("x"),
        Inline::SoftBreak,
        Inline::Str("b".into()),
    ])]);
    let out = RstWriter.write(&broken, &WriterOptions::default()).unwrap();
    assert_eq!(out, "a\n\n.. math:: x\n\nb");
}

#[test]
fn figures_keep_names_captions_and_non_image_content() {
    let image = Inline::Image(
        Box::default(),
        vec![Inline::Str("Alt".into())],
        Box::new(Target {
            url: "image.png".into(),
            title: "".into(),
        }),
    );
    let paragraph = |text: &str| Block::Para(vec![Inline::Str(text.into())]);
    for (blocks, expected) in [
        (
            Vec::new(),
            ".. container:: figure\n   :name: named\n\n   Caption",
        ),
        (
            vec![Block::Plain(vec![image.clone()])],
            ".. figure:: image.png\n   :name: named\n   :alt: Alt\n\n   Caption",
        ),
        (
            vec![
                paragraph("Before"),
                Block::Plain(vec![image.clone()]),
                paragraph("After"),
            ],
            ".. container:: figure\n   :name: named\n\n   Before\n\n   |Alt|\n   After\n\n   Caption\n\n.. |Alt| image:: image.png",
        ),
        (
            vec![Block::Para(vec![
                Inline::Str("Before".into()),
                Inline::Space,
                image,
            ])],
            ".. container:: figure\n   :name: named\n\n   Before |Alt|\n\n   Caption\n\n.. |Alt| image:: image.png",
        ),
    ] {
        let document = Document {
            blocks: vec![Block::Figure(
                Box::new(Attr {
                    id: "named".into(),
                    ..Attr::default()
                }),
                Box::new(carta_ast::Caption {
                    long: vec![paragraph("Caption")],
                    ..carta_ast::Caption::default()
                }),
                blocks,
            )],
            ..Document::default()
        };
        let output = RstWriter
            .write(&document, &WriterOptions::default())
            .unwrap();
        assert_eq!(output, expected);
    }
}
