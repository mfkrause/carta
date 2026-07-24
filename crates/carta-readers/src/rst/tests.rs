use super::definitions::{format_date, render_date};
use super::*;
use carta_ast::{Attr, Block, Inline, ListNumberDelim, ListNumberStyle, Target};
use carta_core::Extension;

fn parse(input: &str) -> Vec<Block> {
    parse_ext(input, Extensions::default())
}

fn parse_ext(input: &str, extensions: Extensions) -> Vec<Block> {
    let reader = RstReader;
    let mut options = ReaderOptions::default();
    options.extensions = extensions;
    reader
        .read(input, &options)
        .expect("reader does not fail")
        .blocks
}

fn with_auto_ids() -> Extensions {
    let mut extensions = Extensions::default();
    extensions.insert(Extension::AutoIdentifiers);
    extensions
}

#[test]
fn leading_punctuation_before_name_does_not_underflow() {
    // Backwards name scan must stop at index zero; leading punctuation cannot extend the name.
    let _ = parse("_C_\n");
    let _ = parse("_C");
    let _ = parse(":a");
}

#[test]
fn circular_substitution_does_not_overflow_the_stack() {
    // Fuzz-found: circular substitutions must stay unexpanded, not overflow the stack.
    let _ = parse(".. |a| replace:: |a|\n\n|a|\n");
    let _ = parse(".. |a| replace:: |b|\n.. |b| replace:: |a|\n\n|a|\n");
    // libFuzzer-minimized reproducer.
    let bytes = [
        84u8, 46, 46, 32, 124, 124, 97, 112, 124, 0, 32, 10, 46, 46, 32, 124, 46, 46, 32, 124, 117,
        110, 105, 99, 111, 100, 101, 58, 58, 32, 124, 124, 124, 124, 95, 58, 58, 32, 124, 124, 46,
        124, 46, 9, 124, 1, 0, 46, 46, 32, 124, 117, 110, 32, 124, 46, 46, 32, 124, 117, 110, 105,
        99, 111, 100, 101, 58, 58, 32, 124, 124, 124, 124, 95, 58, 58, 32, 124, 124, 46, 46, 124,
        124, 1, 9, 0, 46, 46, 32, 124, 117, 110, 105, 99, 111, 100, 101, 58, 44, 32, 124, 124, 46,
        105, 99, 111, 100, 101, 58, 44, 32, 124, 124, 46, 124, 46, 9, 124, 1, 0, 0, 114, 10, 9, 46,
        116, 0,
    ];
    let _ = parse(std::str::from_utf8(&bytes).unwrap());
}

#[test]
fn pipe_not_followed_by_space_does_not_stall_the_scan() {
    // A `|` without a following space/EOL is not a line block; the scan must advance past its line.
    let _ = parse("\u{0b}\t|\u{0}");
    let _ = parse("   |x");
    let _ = parse("|x\n");
}

#[test]
fn paragraph_with_inline_markup() {
    let blocks = parse("A *word* and **two** and ``lit``.\n");
    assert_eq!(
        blocks,
        vec![Block::Para(vec![
            Inline::Str("A".into()),
            Inline::Space,
            Inline::Emph(vec![Inline::Str("word".into())]),
            Inline::Space,
            Inline::Str("and".into()),
            Inline::Space,
            Inline::Strong(vec![Inline::Str("two".into())]),
            Inline::Space,
            Inline::Str("and".into()),
            Inline::Space,
            Inline::Code(Box::default(), "lit".into()),
            Inline::Str(".".into()),
        ])]
    );
}

#[test]
fn underline_section_header_gets_slug_id() {
    let blocks = parse_ext("Title\n=====\n", with_auto_ids());
    assert_eq!(
        blocks,
        vec![Block::Header(
            1,
            Box::new(Attr {
                id: "title".into(),
                classes: Vec::new(),
                attributes: Vec::new(),
            }),
            vec![Inline::Str("Title".into())],
        )]
    );
}

#[test]
fn header_levels_follow_first_seen_adornment_order() {
    let blocks = parse("A\n=\n\nB\n-\n\nC\n=\n");
    let levels: Vec<i32> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Header(level, _, _) => Some(*level),
            _ => None,
        })
        .collect();
    assert_eq!(levels, vec![1, 2, 1]);
}

#[test]
fn transition_is_a_horizontal_rule() {
    let blocks = parse("Above\n\n----\n\nBelow\n");
    assert_eq!(blocks.get(1), Some(&Block::HorizontalRule));
}

#[test]
fn bullet_list_is_tight() {
    let blocks = parse("- one\n- two\n");
    assert_eq!(
        blocks,
        vec![Block::BulletList(vec![
            vec![Block::Plain(vec![Inline::Str("one".into())])],
            vec![Block::Plain(vec![Inline::Str("two".into())])],
        ])]
    );
}

#[test]
fn enumerated_list_carries_style_and_start() {
    let blocks = parse("3. third\n4. fourth\n");
    match blocks.first() {
        Some(Block::OrderedList(attrs, items)) => {
            assert_eq!(attrs.start, 3);
            assert_eq!(attrs.style, ListNumberStyle::Decimal);
            assert_eq!(attrs.delim, ListNumberDelim::Period);
            assert_eq!(items.len(), 2);
        }
        other => panic!("expected ordered list, got {other:?}"),
    }
}

#[test]
fn literal_block_drops_marker_paragraph() {
    let blocks = parse("::\n\n    code line\n");
    assert_eq!(
        blocks,
        vec![Block::CodeBlock(Box::default(), "code line".into())]
    );
}

#[test]
fn literal_block_keeps_single_colon() {
    let blocks = parse("Example::\n\n    code\n");
    assert_eq!(
        blocks.first(),
        Some(&Block::Para(vec![Inline::Str("Example:".into())]))
    );
}

#[test]
fn field_list_becomes_definition_list() {
    let blocks = parse(":Author: Me\n");
    assert_eq!(
        blocks,
        vec![Block::DefinitionList(vec![(
            vec![Inline::Str("Author".into())],
            vec![vec![Block::Plain(vec![Inline::Str("Me".into())])]],
        )])]
    );
}

#[test]
fn named_target_resolves_reference() {
    let blocks = parse("See website_.\n\n.. _website: https://example.org\n");
    match blocks.first() {
        Some(Block::Para(inlines)) => {
            let link = inlines.iter().find(|i| matches!(i, Inline::Link(..)));
            assert_eq!(
                link,
                Some(&Inline::Link(
                    Box::default(),
                    vec![Inline::Str("website".into())],
                    Box::new(Target {
                        url: "https://example.org".into(),
                        title: carta_ast::Text::default(),
                    }),
                ))
            );
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn footnote_reference_inlines_the_note() {
    let blocks = parse("Ref [1]_\n\n.. [1] The note.\n");
    match blocks.first() {
        Some(Block::Para(inlines)) => {
            assert!(inlines.iter().any(|i| matches!(i, Inline::Note(_))));
            // The space before the note marker is dropped.
            assert_eq!(inlines.first(), Some(&Inline::Str("Ref".into())));
            assert!(matches!(inlines.get(1), Some(Inline::Note(_))));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn comment_produces_no_output() {
    let blocks = parse(".. This is a comment.\n");
    assert!(blocks.is_empty());
}

#[test]
fn interpreted_text_defaults_to_title_reference() {
    let blocks = parse("A `book title` here.\n");
    match blocks.first() {
        Some(Block::Para(inlines)) => {
            assert!(inlines.iter().any(|i| matches!(
                i,
                Inline::Span(attr, _) if attr.classes == vec!["title-ref".to_string()]
            )));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn auto_identifiers_off_yields_no_id() {
    let blocks = parse_ext("Title\n=====\n", Extensions::empty());
    match blocks.first() {
        Some(Block::Header(_, attr, _)) => assert!(attr.id.is_empty()),
        other => panic!("expected header, got {other:?}"),
    }
}

#[test]
fn date_renders_strftime_fields_for_fixed_timestamps() {
    // Frozen epoch timestamps (Gregorian, UTC) keep the assertions reproducible.
    let cases: &[(i64, &str, &str)] = &[
        // 2026-06-29 14:50:50 UTC, a Monday.
        (1_782_744_650, "%Y-%m-%d", "2026-06-29"),
        (1_782_744_650, "%j", "180"),
        (1_782_744_650, "%A %a", "Monday Mon"),
        (1_782_744_650, "%B %b %h", "June Jun Jun"),
        (1_782_744_650, "%u %w", "1 1"),
        (1_782_744_650, "%U %W", "26 26"),
        (1_782_744_650, "%V %G %g", "27 2026 26"),
        (1_782_744_650, "%I %l %p %P", "02  2 PM pm"),
        (1_782_744_650, "%C %y", "20 26"),
        (1_782_744_650, "%D", "06/29/26"),
        (1_782_744_650, "%F %T", "2026-06-29 14:50:50"),
        (1_782_744_650, "%R %k", "14:50 14"),
        (1_782_744_650, "%r", "02:50:50 PM"),
        (1_782_744_650, "%e", "29"),
        // 2024-02-29 00:00:00 UTC, a leap day on a Thursday.
        (1_709_164_800, "%Y-%m-%d", "2024-02-29"),
        (1_709_164_800, "%j", "060"),
        (1_709_164_800, "%A", "Thursday"),
        (1_709_164_800, "%U %W", "08 09"),
        (1_709_164_800, "%V %G %g", "09 2024 24"),
        (1_709_164_800, "%I %p", "12 AM"),
        (1_709_164_800, "%e", "29"),
        // 1970-01-01 00:00:00 UTC, the epoch, a Thursday.
        (0, "%Y-%m-%d", "1970-01-01"),
        (0, "%j", "001"),
        (0, "%A", "Thursday"),
        (0, "%U %W", "00 00"),
        (0, "%V %G %g", "01 1970 70"),
        (0, "%e", " 1"),
        // 2027-01-01 12:00:00 UTC: an ISO week that rolls back into the previous year.
        (1_798_804_800, "%V %G %g", "53 2026 26"),
        (1_798_804_800, "%A", "Friday"),
        (1_798_804_800, "%r", "12:00:00 PM"),
        // A literal percent, and an unrecognized code emitted verbatim.
        (0, "before %% after", "before % after"),
        (0, "%Q", "%Q"),
    ];
    for (secs, format, expected) in cases {
        assert_eq!(
            &render_date(*secs, format),
            expected,
            "render_date({secs}, {format:?})"
        );
    }
    // The empty format string falls back to an ISO date, whatever today happens to be.
    let today = format_date("");
    assert_eq!(today.len(), 10);
    assert_eq!(today.matches('-').count(), 2);
}

#[test]
fn include_directive_splices_referenced_file() {
    let path = std::env::temp_dir().join(format!("carta_rst_include_{}.rst", std::process::id()));
    std::fs::write(&path, "Pulled in **bold** text.\n").expect("write temp include");
    let source = format!("Before.\n\n.. include:: {}\n\nAfter.\n", path.display());
    let blocks = parse(&source);
    std::fs::remove_file(&path).ok();

    let paragraphs: Vec<&Vec<Inline>> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Para(inlines) => Some(inlines),
            _ => None,
        })
        .collect();
    assert_eq!(paragraphs.len(), 3);
    let included = paragraphs.get(1).expect("the spliced include paragraph");
    assert!(
        included
            .iter()
            .any(|inline| matches!(inline, Inline::Strong(_)))
    );
}

/// The attributes of the first image found in a paragraph or plain block.
fn first_image_attr(blocks: &[Block]) -> Option<Attr> {
    for block in blocks {
        let (Block::Para(inlines) | Block::Plain(inlines)) = block else {
            continue;
        };
        for inline in inlines {
            if let Inline::Image(attr, _, _) = inline {
                return Some(*attr.clone());
            }
        }
    }
    None
}

fn image_width(source: &str) -> Option<String> {
    first_image_attr(&parse(source))?
        .attributes
        .into_iter()
        .find(|(key, _)| key == "width")
        .map(|(_, value)| value.to_string())
}

#[test]
fn image_directive_resolves_width_and_scale() {
    // Pixel width truncates to an integer at parse time; scaling rounds to even at the boundary.
    assert_eq!(
        image_width(".. image:: a.png\n   :width: 200px\n   :scale: 50%\n"),
        Some("100px".into())
    );
    assert_eq!(
        image_width(".. image:: a.png\n   :width: 201px\n   :scale: 50%\n"),
        Some("100px".into())
    );
    assert_eq!(
        image_width(".. image:: a.png\n   :width: 100.7px\n"),
        Some("100px".into())
    );
    // A percentage width keeps a single fractional digit.
    assert_eq!(
        image_width(".. image:: a.png\n   :width: 100%\n   :scale: 33\n"),
        Some("3300.0%".into())
    );
    // A physical unit scales and renders in the shortest form.
    assert_eq!(
        image_width(".. image:: a.png\n   :width: 2.5in\n   :scale: 50%\n"),
        Some("1.25in".into())
    );
    assert_eq!(
        image_width(".. image:: a.png\n   :width: 3cm\n"),
        Some("3cm".into())
    );
}

#[test]
fn image_directive_doubles_classes_and_appends_alignment() {
    let classes = |source: &str| first_image_attr(&parse(source)).expect("an image").classes;
    // Alignment alone becomes an `align-<value>` class.
    assert_eq!(
        classes(".. image:: a.png\n   :align: center\n"),
        vec!["align-center".to_string()]
    );
    // An explicit class list is doubled, with the alignment fused onto the final entry.
    assert_eq!(
        classes(".. image:: a.png\n   :class: foo\n   :align: center\n"),
        vec!["foo".to_string(), "fooalign-center".to_string()]
    );
    assert_eq!(
        classes(".. image:: a.png\n   :class: foo bar\n"),
        vec![
            "foo".to_string(),
            "bar".to_string(),
            "foo".to_string(),
            "bar".to_string()
        ]
    );
}

#[test]
fn substitution_image_carries_options() {
    let badge = parse("|i|\n\n.. |i| image:: a.png\n   :class: foo\n   :align: middle\n");
    assert_eq!(
        first_image_attr(&badge).expect("an image").classes,
        vec!["foo".to_string(), "fooalign-middle".to_string()]
    );
    assert_eq!(
        image_width("|i|\n\n.. |i| image:: a.png\n   :width: 200px\n   :scale: 50%\n"),
        Some("100px".into())
    );
}

#[test]
fn figure_directive_separates_figure_and_image_attributes() {
    // `:name:` identifies the inner image, `:align:` classes the figure; the figure id is empty.
    let blocks = parse(".. figure:: a.png\n   :name: first\n   :align: center\n\n   Cap\n");
    let (outer, body) = match blocks.first() {
        Some(Block::Figure(attr, _, body)) => (attr.clone(), body.clone()),
        other => panic!("expected a figure, got {other:?}"),
    };
    assert!(outer.id.is_empty());
    assert_eq!(outer.classes, vec!["align-center".to_string()]);
    let inner = first_image_attr(&body).expect("an inner image");
    assert_eq!(inner.id.as_str(), "first");
    assert!(inner.classes.is_empty());

    // `:figclass:` and `:class:` both class the figure; only `:class:` reaches the inner image.
    let blocks = parse(".. figure:: a.png\n   :figclass: frame\n   :class: photo\n\n   Cap\n");
    let (outer, body) = match blocks.first() {
        Some(Block::Figure(attr, _, body)) => (attr.clone(), body.clone()),
        other => panic!("expected a figure, got {other:?}"),
    };
    assert_eq!(
        outer.classes,
        vec!["frame".to_string(), "photo".to_string()]
    );
    let inner = first_image_attr(&body).expect("an inner image");
    assert_eq!(inner.classes, vec!["photo".to_string()]);
}
