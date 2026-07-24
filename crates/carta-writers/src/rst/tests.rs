use super::*;

use super::block::{join_loose_items, show_dimension};
use super::inline::escape;
use carta_ast::{Attr, ColWidth, Row, Table, Target};

fn unit(simple: bool, text: &str) -> (bool, String) {
    (simple, text.to_string())
}

#[test]
fn escape_flanking_tests_see_the_neighbor_before_a_trigger() {
    // space before makes a potential start-string, word char before whitespace a potential end-string, buried star neither
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
    // a single-line unit joins tightly even when a later unit is multi-line; a multi-line unit forces a blank after it
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
    // whole lengths drop the trailing zero, percentages keep one decimal, unitless renders in px, unknown units dropped
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
    // repeats and empty labels fall back to a counter name so every reference resolves uniquely
    assert_eq!(state.substitution_name("image".to_owned()), "image");
    assert_eq!(state.substitution_name("image".to_owned()), "image1");
    assert_eq!(state.substitution_name("logo".to_owned()), "logo");
    assert_eq!(state.substitution_name(String::new()), "image2");
    assert_eq!(state.substitution_name("image".to_owned()), "image3");
}

#[test]
fn image_run_names_dedupe_across_registrations() {
    // link-embedding alt text registers each run as a substitution; a later image sharing the name must fall back so no two definitions share a label
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
fn deeply_nested_tables_render_without_compounding_measurement() {
    // without the nesting cap, measurement renders would compound exponentially in depth
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

    // deep enough that compounding measurement would take minutes; capped stays under a second
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
