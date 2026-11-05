#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::AsciidocReader;
use carta_ast::{Attr, Block, Inline, MetaValue};
use carta_core::{Reader, ReaderOptions};

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "carta-asciidoc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn first_item_inlines(item: &[Block]) -> &[Inline] {
    match item.first() {
        Some(Block::Para(inlines) | Block::Plain(inlines)) => inlines,
        _ => panic!("expected a list item paragraph"),
    }
}

#[test]
fn preserved_hyphens_allow_later_arrow_replacements() {
    for (source, expected) in [
        ("<!---->", "<!---→"),
        ("<!--x---->", "<!--x---→"),
        ("a---->", "a---→"),
        ("---->", "---→"),
    ] {
        let document = AsciidocReader
            .read(source, &ReaderOptions::default())
            .unwrap();
        assert_eq!(
            document.blocks,
            vec![Block::Para(vec![Inline::Str(expected.into())])],
            "{source:?}"
        );
    }
}

#[test]
fn included_checkboxes_require_ascii_spaces() {
    let directory = Directory::new();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    let literal = vec![
        Inline::Str("[".into()),
        Inline::Space,
        Inline::Str("]".into()),
        Inline::Space,
        Inline::Str("x".into()),
    ];
    let mut checked_literal = vec![Inline::Str("☒".into()), Inline::Space];
    checked_literal.extend(literal.clone());
    for (source, expected) in [
        ("1. [ ]\tx\n", literal.clone()),
        (
            "* [x]\tx\n",
            vec![
                Inline::Str("[x]".into()),
                Inline::Space,
                Inline::Str("x".into()),
            ],
        ),
        ("* \t[ ] x\n", literal),
        ("* [x] \t[ ] x\n", checked_literal),
    ] {
        std::fs::write(directory.0.join("i"), source).unwrap();
        let document = AsciidocReader.read("include::i[]\n", &options).unwrap();
        let Some(Block::Div(_, included)) = document.blocks.first() else {
            panic!("expected an included document");
        };
        let Some(Block::BulletList(items) | Block::OrderedList(_, items)) = included.first() else {
            panic!("expected a list");
        };
        assert_eq!(first_item_inlines(&items[0]), expected, "{source:?}");
    }
}

#[test]
fn empty_checkbox_items_keep_their_list_state() {
    let source = concat!("1. [ ]", " \n", "2. [x] later\n");
    let document = AsciidocReader
        .read(source, &ReaderOptions::default())
        .unwrap();
    let Block::OrderedList(_, items) = &document.blocks[0] else {
        panic!("expected ordered list");
    };
    assert!(items[0].is_empty());
    assert_eq!(
        &first_item_inlines(&items[1])[..3],
        [
            Inline::Str("☒".into()),
            Inline::Space,
            Inline::Str("later".into())
        ]
    );

    let source = concat!("* [ ]", " \n", "* [x] later\n");
    let document = AsciidocReader
        .read(source, &ReaderOptions::default())
        .unwrap();
    let Block::BulletList(items) = &document.blocks[0] else {
        panic!("expected bullet list");
    };
    assert_eq!(first_item_inlines(&items[0]), [Inline::Str("☐".into())]);
}

#[test]
fn shallower_same_kind_markers_start_a_new_list() {
    for (source, text) in [("-- - -", "-"), ("** * *", "*"), ("--- - -", "-")] {
        let blocks = AsciidocReader
            .read(source, &ReaderOptions::default())
            .unwrap()
            .blocks;
        assert_eq!(
            blocks,
            vec![
                Block::BulletList(vec![vec![]]),
                Block::BulletList(vec![vec![Block::Para(vec![Inline::Str(text.into())])]]),
            ],
            "{source:?}"
        );
    }

    let blocks = AsciidocReader
        .read("-- * *", &ReaderOptions::default())
        .unwrap()
        .blocks;
    assert_eq!(
        blocks,
        vec![Block::BulletList(vec![vec![Block::BulletList(vec![
            vec![Block::Para(vec![Inline::Str("*".into())]),]
        ])]])]
    );
}

#[test]
fn stripped_space_marker_leaves_the_list_body() {
    for (source, tab_stop, expected_marker) in [
        ("* \t* b", 0, "*"),
        ("* \x0c* b", 4, "*"),
        ("* \x0b* b", 4, "*"),
        ("* \u{00a0}* b", 4, "*"),
        ("* \t \t* b", 0, "*"),
        ("* \u{00a0}\t* b", 0, "*"),
        ("1. \x0c. b", 4, "."),
        ("-- \x0b- -", 4, "- -"),
    ] {
        let mut options = ReaderOptions::default();
        options.tab_stop = tab_stop;
        let blocks = AsciidocReader.read(source, &options).unwrap().blocks;
        let second_item = if expected_marker == "- -" {
            vec![
                Inline::Str("-".into()),
                Inline::Space,
                Inline::Str("-".into()),
            ]
        } else {
            vec![
                Inline::Str(expected_marker.into()),
                Inline::Space,
                Inline::Str("b".into()),
            ]
        };
        assert_eq!(blocks.len(), 2, "{source:?}");
        if source.starts_with("1.") {
            let Block::OrderedList(_, items) = &blocks[0] else {
                panic!("expected ordered list: {source:?}");
            };
            assert_eq!(items, &[vec![]], "{source:?}");
        } else {
            let Block::BulletList(items) = &blocks[0] else {
                panic!("expected bullet list: {source:?}");
            };
            assert_eq!(items, &[vec![]], "{source:?}");
        }
        assert_eq!(blocks[1], Block::Para(second_item), "{source:?}");
    }

    let mut options = ReaderOptions::default();
    options.tab_stop = 0;
    for source in ["* \t* ", "* \t* \n"] {
        let blocks = AsciidocReader.read(source, &options).unwrap().blocks;
        assert_eq!(
            blocks,
            vec![Block::BulletList(vec![vec![Block::Para(vec![
                Inline::Str("*".into())
            ])]])],
            "{source:?}"
        );
    }

    let mut options = ReaderOptions::default();
    options.tab_stop = 8;
    let blocks = AsciidocReader.read("* \t* b", &options).unwrap().blocks;
    assert_eq!(
        blocks,
        vec![Block::BulletList(vec![
            vec![],
            vec![Block::Para(vec![Inline::Str("b".into())])],
        ])]
    );
}

#[test]
fn includes_resolve_nested_paths_and_keep_options() {
    let directory = Directory::new();
    let subdirectory = directory.0.join("sub");
    std::fs::create_dir(&subdirectory).unwrap();
    std::fs::write(
        subdirectory.join("one.adoc"),
        "= Included title\n\ninclude::two.adoc[]\n",
    )
    .unwrap();
    std::fs::write(subdirectory.join("two.adoc"), "\tinside\n").unwrap();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    let document = AsciidocReader
        .read("include::sub/one.adoc[indent=2]\n", &options)
        .unwrap();
    let included = |name: &str, attributes: Vec<_>, blocks| {
        let mut pairs = attributes;
        pairs.push((
            "path".into(),
            subdirectory.join(name).to_string_lossy().as_ref().into(),
        ));
        Block::Div(
            Box::new(Attr {
                classes: vec!["included".into()],
                attributes: pairs,
                ..Attr::default()
            }),
            blocks,
        )
    };
    assert!(document.meta.is_empty());
    assert_eq!(
        document.blocks,
        vec![included(
            "one.adoc",
            vec![("indent".into(), "2".into())],
            vec![included(
                "two.adoc",
                vec![],
                vec![Block::Para(vec![Inline::Str("inside".into())])]
            )]
        )]
    );
}

#[test]
fn missing_includes_report_the_file() {
    let directory = Directory::new();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    for source in [
        "include::missing.adoc[]",
        "include::missing.adoc[opts=optional]",
        "[,rust]\n----\ninclude::missing.rs[]\n----",
        "[cols=a]\n|===\n|include::missing.adoc[]\n|===\n",
        "include::missing.adoc[opts=optional\n\ninclude::sub dir/file.adoc[]",
    ] {
        let error = AsciidocReader
            .read(source, &options)
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing"), "{error}");
    }
}

#[test]
fn block_macros_take_priority_in_definition_bodies() {
    let directory = Directory::new();
    std::fs::write(directory.0.join("i"), "").unwrap();
    let include_path = directory.0.join("i").to_string_lossy().into_owned();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    for source in [
        "::include::i[]\n",
        "Term::include::i[]\n",
        "::\ninclude::i[]\n",
        "Term::\n  include::i[]\n",
        "* ::include::i[]\n",
        "* Outer\n+\n** ::include::i[]\n",
    ] {
        let mut document = AsciidocReader.read(source, &options).unwrap();
        let mut includes = 0;
        carta_core::walk::for_each_document_block(&mut document, &mut |block| {
            if let Block::Div(attr, _) = block
                && attr.classes.iter().any(|class| class == "included")
                && attr
                    .attributes
                    .iter()
                    .any(|(name, value)| name == "path" && value.as_str() == include_path)
            {
                includes += 1;
            }
        });
        assert_eq!(includes, 1, "{source:?}");
    }

    let document = AsciidocReader
        .read("Term::include::i[] tail\n", &options)
        .unwrap();
    assert_eq!(
        document.blocks,
        vec![Block::DefinitionList(vec![(
            vec![Inline::Str("Term".into())],
            vec![vec![
                Block::Div(
                    Box::new(Attr {
                        classes: vec!["included".into()],
                        attributes: vec![("path".into(), include_path.into())],
                        ..Default::default()
                    }),
                    vec![],
                ),
                Block::Para(vec![Inline::Str("tail".into())]),
            ]],
        )])]
    );
}

#[test]
fn definition_entries_keep_following_blocks_after_a_macro() {
    let blocks = AsciidocReader
        .read("A::toc::[]\nB::x\n* y\n", &ReaderOptions::default())
        .unwrap()
        .blocks;
    assert_eq!(
        blocks,
        vec![Block::DefinitionList(vec![
            (
                vec![Inline::Str("A".into())],
                vec![vec![Block::Div(
                    Box::new(Attr {
                        id: "toc".into(),
                        ..Default::default()
                    }),
                    vec![],
                )]],
            ),
            (
                vec![Inline::Str("B".into())],
                vec![vec![
                    Block::Para(vec![Inline::Str("x".into())]),
                    Block::BulletList(vec![vec![Block::Para(vec![Inline::Str("y".into(),)])]]),
                ]],
            ),
        ])]
    );

    let blocks = AsciidocReader
        .read("* A::toc::[]\n* y\n", &ReaderOptions::default())
        .unwrap()
        .blocks;
    let Some(Block::BulletList(items)) = blocks.first() else {
        panic!("expected a bullet list");
    };
    assert_eq!(items.len(), 2);
    assert!(matches!(items[0].first(), Some(Block::DefinitionList(_))));
    assert_eq!(items[1], vec![Block::Para(vec![Inline::Str("y".into())])]);

    let blocks = AsciidocReader
        .read("* A::toc::[]\nnext\n", &ReaderOptions::default())
        .unwrap()
        .blocks;
    let Some(Block::BulletList(items)) = blocks.first() else {
        panic!("expected a bullet list");
    };
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0].first(), Some(Block::DefinitionList(_))));
    assert_eq!(
        items[0].get(1),
        Some(&Block::Para(vec![Inline::Str("next".into())]))
    );
}

#[test]
fn include_cycles_leave_the_recursive_include_unexpanded() {
    let directory = Directory::new();
    let source = "before\n\ninclude::cycle.adoc[]\n\nafter\n";
    let path = directory.0.join("cycle.adoc");
    std::fs::write(&path, source).unwrap();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    options.source_paths.insert(0, path);
    assert_eq!(
        AsciidocReader.read(source, &options).unwrap().blocks,
        vec![
            Block::Para(vec![Inline::Str("before".into())]),
            Block::Para(vec![Inline::Str("after".into())])
        ]
    );
}

#[test]
fn source_boundaries_keep_independent_headers() {
    let first = "= First\n\nOne.\n\n";
    let second = "= Second\n\nTwo.\n";
    let mut options = ReaderOptions::default();
    options.source_dirs.insert(0, PathBuf::new());
    options.source_dirs.insert(first.len(), PathBuf::new());
    let document = AsciidocReader
        .read(&format!("{first}{second}"), &options)
        .unwrap();
    assert_eq!(
        document.meta.get("title"),
        Some(&carta_ast::MetaValue::MetaInlines(vec![Inline::Str(
            "First".into()
        )]))
    );
    assert_eq!(
        document.blocks,
        vec![
            Block::Para(vec![Inline::Str("One.".into())]),
            Block::Para(vec![Inline::Str("Two.".into())])
        ]
    );
}

#[test]
fn real_header_metadata_overrides_other_sources_attributes() {
    let first = ":author: Attribute author\n:title: Attribute title\n:version: attr-v\n:date: attr-date\n:remark: attr-remark\n\nA\n";
    let second = "= Real title\nHeader author\nv1, 2025-01-02: Real remark\n\nB\n";
    let mut options = ReaderOptions::default();
    options.source_dirs.insert(0, PathBuf::new());
    options.source_dirs.insert(first.len(), PathBuf::new());
    let combined = AsciidocReader
        .read(&format!("{first}{second}"), &options)
        .unwrap();

    assert_eq!(
        combined.meta.get("author"),
        Some(&MetaValue::MetaList(vec![MetaValue::MetaInlines(vec![
            Inline::Str("Header".into()),
            Inline::Space,
            Inline::Str("author".into()),
        ])]))
    );
    assert_eq!(
        combined.meta.get("title"),
        Some(&MetaValue::MetaInlines(vec![
            Inline::Str("Real".into()),
            Inline::Space,
            Inline::Str("title".into()),
        ]))
    );
    assert_eq!(
        combined.meta.get("version"),
        Some(&MetaValue::MetaString("1".into()))
    );
    assert_eq!(
        combined.meta.get("date"),
        Some(&MetaValue::MetaString("2025-01-02".into()))
    );
    assert_eq!(
        combined.meta.get("remark"),
        Some(&MetaValue::MetaString("Real remark".into()))
    );
}

#[test]
fn combined_sources_keep_the_first_header_revision_as_a_group() {
    let first = "= First\nAlice\nv1.0\n\nA\n";
    let second = "= Second\nBob\nv2.0, 2026-10-01: Later\n\nB\n";
    let mut options = ReaderOptions::default();
    options.source_dirs.insert(0, PathBuf::new());
    options.source_dirs.insert(first.len(), PathBuf::new());
    let combined = AsciidocReader
        .read(&format!("{first}{second}"), &options)
        .unwrap();
    assert_eq!(
        combined.meta.get("version"),
        Some(&MetaValue::MetaString("1.0".into()))
    );
    assert_eq!(combined.meta.get("date"), None);
    assert_eq!(combined.meta.get("remark"), None);
}

#[test]
fn revision_attributes_fall_back_across_sources() {
    let first = ":version: attr-v\n:date: attr-date\n:remark: attr-remark\n\nA\n";
    let second = ":date: later-date\n:remark: later-remark\n\nB\n";
    let mut options = ReaderOptions::default();
    options.source_dirs.insert(0, PathBuf::new());
    options.source_dirs.insert(first.len(), PathBuf::new());
    let combined = AsciidocReader
        .read(&format!("{first}{second}"), &options)
        .unwrap();
    for (name, value) in [
        ("version", "attr-v"),
        ("date", "attr-date"),
        ("remark", "attr-remark"),
    ] {
        assert_eq!(
            combined.meta.get(name),
            Some(&MetaValue::MetaString(value.into()))
        );
    }
}

#[test]
fn title_attributes_resolve_from_final_values() {
    let document = AsciidocReader
        .read(
            ":x: old\n= {x}\n:x: new\n\nbody\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    assert_eq!(
        document.meta.get("title"),
        Some(&MetaValue::MetaInlines(vec![Inline::Str("new".into())]))
    );
    assert_eq!(
        document.meta.get("x"),
        Some(&MetaValue::MetaString("new".into()))
    );
}

#[test]
fn header_comment_boundaries_match_document_header_groups() {
    assert!(
        AsciidocReader
            .read(
                ":a: one\n// c\n:b: two\n= Title\n",
                &ReaderOptions::default(),
            )
            .is_err()
    );

    for source in [
        "// lead\n:a: one\n\n= Title\n\nbody\n",
        ":a: one\n// separator\n= Title\n\nbody\n",
        "= Title\n:a: one\n\nbody\n",
    ] {
        let document = AsciidocReader
            .read(source, &ReaderOptions::default())
            .unwrap();
        assert_eq!(
            document.meta.get("title"),
            Some(&MetaValue::MetaInlines(vec![Inline::Str("Title".into())]))
        );
        assert_eq!(
            document.meta.get("a"),
            Some(&MetaValue::MetaString("one".into()))
        );
    }
}

#[test]
fn negated_attribute_references_remain_literal() {
    let document = AsciidocReader
        .read(
            ":foo: one\n:!foo:\n\n{foo}|{!foo}|{blank}\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    assert_eq!(
        document.blocks,
        vec![Block::Para(vec![Inline::Str("{foo}|{!foo}|".into())])]
    );
}

#[test]
fn combined_sources_share_notes_and_use_first_icon_attributes() {
    let first = ":icons: font\n\n= First\nAlice; Carol\n\nfootnoteref:[named,Hello {late}]\n\n:late: Ada\n\n";
    let second = ":icons: image\n\n= Second\nBob; Dana\n\nfootnoteref:[named]\n\nicon:heart[]\n";
    let mut options = ReaderOptions::default();
    options.source_dirs.insert(0, PathBuf::new());
    options.source_dirs.insert(first.len(), PathBuf::new());
    let document = AsciidocReader
        .read(&format!("{first}{second}"), &options)
        .unwrap();

    assert_eq!(
        document.meta.get("author"),
        Some(&MetaValue::MetaList(vec![
            MetaValue::MetaInlines(vec![Inline::Str("Alice".into())]),
            MetaValue::MetaInlines(vec![Inline::Str("Carol".into())]),
            MetaValue::MetaInlines(vec![Inline::Str("Bob".into())]),
            MetaValue::MetaInlines(vec![Inline::Str("Dana".into())]),
        ]))
    );
    let note = Block::Para(vec![Inline::Note(vec![Block::Para(vec![
        Inline::Str("Hello".into()),
        Inline::Space,
        Inline::Str("Ada".into()),
    ])])]);
    assert_eq!(document.blocks.first(), Some(&note));
    assert_eq!(document.blocks.get(1), Some(&note));
    assert_eq!(
        document.blocks.get(2),
        Some(&Block::Para(vec![Inline::Span(
            Box::new(Attr {
                classes: vec!["fa fa-heart".into()],
                ..Default::default()
            }),
            vec![],
        )]))
    );
}

#[test]
fn repeated_named_notes_keep_their_inline_children() {
    let first = "footnote:n[*a**b*]\n\n";
    let second = "footnote:n[]";
    let mut options = ReaderOptions::default();
    options.source_dirs.insert(0, PathBuf::new());
    options.source_dirs.insert(first.len(), PathBuf::new());
    let document = AsciidocReader
        .read(&format!("{first}{second}"), &options)
        .unwrap();
    let note = Block::Para(vec![Inline::Note(vec![Block::Para(vec![Inline::Strong(
        vec![Inline::Str("a".into()), Inline::Str("b".into())],
    )])])]);
    assert_eq!(document.blocks, vec![note.clone(), note]);
}

#[test]
fn generated_heading_ids_follow_nested_section_order() {
    let document = AsciidocReader
        .read(
            "== Héllo, world!\n\n=== Héllo, world!\n\n==== Héllo, world!\n\n== Héllo, world!\n\n== Héllo, world!",
            &ReaderOptions::default(),
        )
        .unwrap();
    let identifiers = document
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Header(_, attributes, _) => Some(attributes.id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        identifiers,
        [
            "_héllo,_world!_3",
            "_héllo,_world!_2",
            "_héllo,_world!",
            "_héllo,_world!_4",
            "_héllo,_world!_5",
        ]
    );
}

#[test]
fn duplicate_heading_ids_resolve_to_the_first_heading() {
    let document = AsciidocReader
        .read(
            "[[same]]\n== First\n\n[[same]]\n== Second\n\n<<same>>\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Para(inlines)) = document.blocks.last() else {
        panic!("expected a cross-reference paragraph");
    };
    let Some(Inline::Link(_, label, _)) = inlines.first() else {
        panic!("expected a cross-reference");
    };
    assert_eq!(label, &[Inline::Str("First".into())]);
}

#[test]
fn heading_ids_take_precedence_over_inline_anchors() {
    let document = AsciidocReader
        .read(
            "anchor:same[Inline label]\n\n[[same]]\n== Header label\n\n<<same>>\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Para(inlines)) = document.blocks.last() else {
        panic!("expected a cross-reference paragraph");
    };
    let Some(Inline::Link(_, label, _)) = inlines.first() else {
        panic!("expected a cross-reference");
    };
    assert_eq!(
        label,
        &[
            Inline::Str("Header".into()),
            Inline::Space,
            Inline::Str("label".into())
        ]
    );
}

#[test]
fn duplicate_inline_anchors_resolve_to_the_first_label() {
    let document = AsciidocReader
        .read(
            "foo [[same,First]] bar [[same,Second]] baz <<same>>\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Para(inlines)) = document.blocks.first() else {
        panic!("expected a cross-reference paragraph");
    };
    let Some(Inline::Link(_, label, _)) = inlines.last() else {
        panic!("expected a cross-reference");
    };
    assert_eq!(label, &[Inline::Str("First".into())]);
}

#[test]
fn titled_sections_wrap_the_body_and_move_heading_ids() {
    let document = AsciidocReader
        .read(
            ".Side title\n[sidebar]\n== H\n\nend\n\nSee <<_h>>\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    assert_eq!(
        document.blocks,
        vec![Block::Div(
            Box::new(Attr {
                id: "_h".into(),
                ..Default::default()
            }),
            vec![
                Block::Div(
                    Box::new(Attr {
                        classes: vec!["title".into()],
                        ..Default::default()
                    }),
                    vec![Block::Para(vec![
                        Inline::Str("Side".into()),
                        Inline::Space,
                        Inline::Str("title".into()),
                    ])],
                ),
                Block::Header(1, Box::default(), vec![Inline::Str("H".into())]),
                Block::Para(vec![Inline::Str("end".into())]),
                Block::Para(vec![
                    Inline::Str("See".into()),
                    Inline::Space,
                    Inline::Link(
                        Box::new(Attr {
                            classes: vec!["cross-reference".into()],
                            ..Default::default()
                        }),
                        vec![Inline::Str("H".into())],
                        Box::new(carta_ast::Target {
                            url: "#_h".into(),
                            title: "".into(),
                        }),
                    ),
                ]),
            ],
        ),]
    );
}

#[test]
fn titled_nested_sections_keep_discrete_headings_out_of_the_id_order() {
    let document = AsciidocReader
        .read(
            ".Parent\n== H\n\n[discrete]\n=== H\n\n.Child\n=== H\n\nend\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Div(parent, contents)) = document.blocks.first() else {
        panic!("expected a titled parent section");
    };
    assert_eq!(parent.id, "_h_2");
    assert!(matches!(contents.get(2), Some(Block::Header(2, attr, _)) if attr.id.is_empty()));
    assert!(matches!(contents.get(3), Some(Block::Div(attr, _)) if attr.id == "_h"));
}

#[test]
fn explicit_titled_section_ids_stay_on_the_wrapper() {
    let document = AsciidocReader
        .read(
            ".Side title\n[sidebar]\n[[fixed]]\n== H\n\nend\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Div(attr, contents)) = document.blocks.first() else {
        panic!("expected a titled section");
    };
    assert_eq!(attr.id, "fixed");
    assert!(matches!(contents.get(1), Some(Block::Header(1, header, _)) if header.id.is_empty()));
}

#[test]
fn unresolved_xrefs_keep_an_explicit_target_marker_in_the_label() {
    let document = AsciidocReader
        .read(
            "See <<missing>> and xref:#missing[].\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Para(inlines)) = document.blocks.first() else {
        panic!("expected a paragraph");
    };
    let labels = inlines
        .iter()
        .filter_map(|inline| match inline {
            Inline::Link(_, label, _) => Some(label.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![
            vec![Inline::Str("missing".into())],
            vec![Inline::Str("#missing".into())],
        ]
    );
}

#[test]
fn attributed_monospace_xrefs_resolve_their_labels() {
    let document = AsciidocReader
        .read(
            "= Doc\n\n[.role]`xref:known[]`\n\nSee xref:known[]\n\n[[known]]\n== Known\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Para(inlines)) = document.blocks.first() else {
        panic!("expected an attributed xref");
    };
    let Some(Inline::Link(attr, label, _)) = inlines.first() else {
        panic!("expected a cross-reference");
    };
    assert_eq!(attr.classes, ["role", "cross-reference"]);
    assert_eq!(label, &[Inline::Code(Box::default(), "Known".into())]);
}

#[test]
fn carriage_returns_do_not_create_line_breaks() {
    let document = AsciidocReader
        .read(
            "== Heading\r\rLine one\rLine two\r",
            &ReaderOptions::default(),
        )
        .unwrap();
    assert_eq!(
        document.blocks,
        vec![Block::Header(
            1,
            Box::new(Attr {
                id: "_headingline_oneline_two".into(),
                ..Default::default()
            }),
            vec![
                Inline::Str("HeadingLine".into()),
                Inline::Space,
                Inline::Str("oneLine".into()),
                Inline::Space,
                Inline::Str("two".into()),
            ]
        )]
    );
}

#[test]
fn includes_remove_carriage_returns_and_keep_tabs() {
    let directory = Directory::new();
    std::fs::write(directory.0.join("body.adoc"), "alpha\rbeta").unwrap();
    std::fs::write(directory.0.join("code.txt"), "\tgamma\rdelta").unwrap();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    let document = AsciidocReader
        .read(
            "include::body.adoc[]\n\n----\ninclude::code.txt[]\n----\n",
            &options,
        )
        .unwrap();
    let Some(Block::Div(_, included)) = document.blocks.first() else {
        panic!("expected included content");
    };
    assert_eq!(
        included,
        &[Block::Para(vec![Inline::Str("alphabeta".into())])]
    );
    assert_eq!(
        document.blocks.get(1),
        Some(&Block::CodeBlock(Box::default(), "\tgammadelta".into()))
    );
}

#[test]
fn invalid_references_and_dangling_attributes_fail() {
    for source in [
        "&#x110000;",
        "[role=red]\n",
        "[[id]]\n",
        "[verse] \n--\n--",
        "[role=red] \ntext\n",
        "text\n[role=red] \nmore\n",
    ] {
        assert!(
            AsciidocReader
                .read(source, &ReaderOptions::default())
                .is_err()
        );
    }
}

#[test]
fn included_title_notes_do_not_define_body_notes() {
    let directory = Directory::new();
    std::fs::write(
        directory.0.join("one.adoc"),
        "= footnote:n[Hidden]\n\nBody\n",
    )
    .unwrap();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    let document = AsciidocReader
        .read("include::one.adoc[]\n\nfootnote:n[]\n", &options)
        .unwrap();
    assert_eq!(
        document.blocks.last(),
        Some(&Block::Para(vec![Inline::Note(vec![Block::Para(vec![])])]))
    );
}

#[test]
fn included_titles_do_not_register_inline_anchors() {
    let directory = Directory::new();
    std::fs::write(
        directory.0.join("one.adoc"),
        "= anchor:lost[Title label]\n\nBody\n",
    )
    .unwrap();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    let document = AsciidocReader
        .read("include::one.adoc[]\n\nSee <<lost>>\n", &options)
        .unwrap();
    let Some(Block::Para(inlines)) = document.blocks.last() else {
        panic!("expected a cross-reference paragraph");
    };
    let Some(Inline::Link(_, label, target)) = inlines.last() else {
        panic!("expected a cross-reference");
    };
    assert_eq!(label, &[Inline::Str("lost".into())]);
    assert_eq!(target.url, "#lost");
}

#[test]
fn listing_includes_use_the_target_prefix() {
    let directory = Directory::new();
    std::fs::write(directory.0.join("source.txt"), "A\nB\n").unwrap();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    for suffix in [
        "[]",
        "[indent=2]",
        "[tags=core]",
        "[lines=1]",
        "[",
        "",
        "[] tail",
    ] {
        let source = format!("----\ninclude::source.txt{suffix}\n----\n");
        assert_eq!(
            AsciidocReader.read(&source, &options).unwrap().blocks,
            vec![Block::CodeBlock(Box::default(), "A\nB".into())],
            "{source}"
        );
    }
    let source = "```\ninclude::source.txt[]\n```\n";
    assert_eq!(
        AsciidocReader.read(source, &options).unwrap().blocks,
        vec![Block::CodeBlock(
            Box::default(),
            "include::source.txt[]".into()
        )]
    );
}

#[test]
fn included_listing_callout_markers_stay_literal() {
    let directory = Directory::new();
    let contents = "<1>\ncall <.>\nA <2>";
    std::fs::write(directory.0.join("source.txt"), contents).unwrap();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    assert_eq!(
        AsciidocReader
            .read("----\ninclude::source.txt[]\n----\n", &options)
            .unwrap()
            .blocks,
        vec![Block::CodeBlock(Box::default(), contents.into())]
    );
}

#[test]
fn syntax_errors_in_table_includes_are_fatal() {
    let directory = Directory::new();
    std::fs::write(directory.0.join("invalid.adoc"), "[verse] \n--\n--\n").unwrap();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    assert!(
        AsciidocReader
            .read("[cols=a]\n|===\n|include::invalid.adoc[]\n|===\n", &options)
            .is_err()
    );
}

#[test]
fn psv_table_closers_keep_inline_cells_and_following_blocks() {
    for source in [
        "|===\n|x |===\nAfter\n",
        "|====\n|x |===== \t// closed\nAfter\n",
        "|===\n|x\n|===\nAfter\n",
    ] {
        let document = AsciidocReader
            .read(source, &ReaderOptions::default())
            .unwrap();
        let Some(Block::Table(table)) = document.blocks.first() else {
            panic!("expected a table");
        };
        let Some(cell) = table.head.rows.first().and_then(|row| row.cells.first()) else {
            panic!("expected a header cell");
        };
        assert_eq!(
            cell.content,
            vec![Block::Para(vec![Inline::Str("x".into())])]
        );
        assert_eq!(
            document.blocks.get(1),
            Some(&Block::Para(vec![Inline::Str("After".into())]))
        );
    }

    let document = AsciidocReader
        .read("|===\n|A\n |===\n", &ReaderOptions::default())
        .unwrap();
    let Some(Block::Table(table)) = document.blocks.first() else {
        panic!("expected a table");
    };
    assert_eq!(
        table.head.rows[0].cells[0].content,
        vec![Block::Para(vec![Inline::Str("A".into())])]
    );
}

#[test]
fn empty_table_closes_only_at_a_border_position() {
    for (source, closing_line) in [
        ("|===\n |===\n", "|==="),
        ("|===\n |=====\n", "|====="),
        ("|===\n :===\n", ":==="),
        ("|===\n ,===\n", ",==="),
    ] {
        let document = AsciidocReader
            .read(source, &ReaderOptions::default())
            .unwrap();
        assert_eq!(
            document.blocks,
            vec![Block::Para(vec![
                Inline::Str("|===".into()),
                Inline::SoftBreak,
                Inline::Str(closing_line.into()),
            ])]
        );
    }
}

#[test]
fn psv_table_closers_follow_custom_separators_and_keep_escaped_text() {
    let document = AsciidocReader
        .read(
            "[format=psv,separator=\";\"]\n|===\n;A;B |===\nAfter\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Table(table)) = document.blocks.first() else {
        panic!("expected a table");
    };
    assert_eq!(table.col_specs.len(), 2);
    assert_eq!(table.head.rows[0].cells.len(), 2);

    let document = AsciidocReader
        .read(
            "[cols=l]\n|===\n|raw \\|=== marker|===\nAfter\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Table(table)) = document.blocks.first() else {
        panic!("expected a table");
    };
    assert_eq!(
        table.head.rows[0].cells[0].content,
        vec![Block::CodeBlock(Box::default(), "raw |=== marker".into())]
    );
    assert_eq!(
        document.blocks.get(1),
        Some(&Block::Para(vec![Inline::Str("After".into())]))
    );

    let document = AsciidocReader
        .read(
            "[cols=\"1,1\"]\n|===\n|A |=== raw text\n|B |C\n|===\nAfter\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Table(table)) = document.blocks.first() else {
        panic!("expected a table");
    };
    assert_eq!(table.head.rows[0].cells.len(), 2);
    assert_eq!(table.head.rows[0].cells[1].content, vec![]);
    assert_eq!(
        document.blocks.get(1),
        Some(&Block::Para(vec![Inline::Str("After".into())]))
    );
}

#[test]
fn delimited_table_formats_close_on_their_own_border_line() {
    let mut options = ReaderOptions::default();
    options.tab_stop = 0;
    for source in [
        "[format=csv]\n|===\na,b\n,===\nAfter\n",
        "[format=dsv]\n|===\na:b\n:===\nAfter\n",
        "[format=tsv]\n|===\na\tb\n|===\nAfter\n",
    ] {
        let document = AsciidocReader.read(source, &options).unwrap();
        let Some(Block::Table(table)) = document.blocks.first() else {
            panic!("expected a table");
        };
        assert_eq!(table.col_specs.len(), 2);
        assert_eq!(
            document.blocks.get(1),
            Some(&Block::Para(vec![Inline::Str("After".into())]))
        );
    }

    let document = AsciidocReader
        .read("[format=csv]\n|===\na,b,===\n", &ReaderOptions::default())
        .unwrap();
    assert!(!matches!(document.blocks.first(), Some(Block::Table(_))));
}

#[test]
fn table_header_and_footer_survive_inline_closing_border() {
    let document = AsciidocReader
        .read(
            "[options=\"header,footer\"]\n|===\n|Head\n|Body\n|Foot |===\n",
            &ReaderOptions::default(),
        )
        .unwrap();
    let Some(Block::Table(table)) = document.blocks.first() else {
        panic!("expected a table");
    };
    assert_eq!(
        table.head.rows[0].cells[0].content,
        vec![Block::Para(vec![Inline::Str("Head".into())])]
    );
    assert_eq!(
        table.bodies[0].body[0].cells[0].content,
        vec![Block::Para(vec![Inline::Str("Body".into())])]
    );
    assert_eq!(
        table.foot.rows[0].cells[0].content,
        vec![Block::Para(vec![Inline::Str("Foot".into())])]
    );
}

#[test]
fn autolinks_leave_invalid_schemes_and_addresses_literal() {
    for source in [
        "http:",
        "https:",
        "irc:",
        "ftp:",
        "mailto:",
        "HTTP:x",
        "file:x",
        "alice@example.x",
        "Ⅻ@example.com",
    ] {
        let document = AsciidocReader
            .read(source, &ReaderOptions::default())
            .unwrap();
        assert_eq!(
            document.blocks,
            [Block::Para(vec![Inline::Str(source.into())])],
            "{source:?}"
        );
    }
}

#[test]
fn email_local_parts_accept_numbers_and_punctuation() {
    for source in ["aⅫb@example.com", "a_b+c@example.test"] {
        let document = AsciidocReader
            .read(source, &ReaderOptions::default())
            .unwrap();
        assert!(
            matches!(document.blocks.as_slice(), [Block::Para(inlines)]
            if matches!(inlines.as_slice(), [Inline::Link(_, _, target)] if target.url == source)),
            "{source:?}"
        );
    }
}

#[test]
fn unclosed_angle_links_keep_the_opening_bracket() {
    let document = AsciidocReader
        .read("<http:x", &ReaderOptions::default())
        .unwrap();
    assert!(matches!(document.blocks.as_slice(), [Block::Para(inlines)]
        if matches!(inlines.as_slice(), [Inline::Str(prefix), Inline::Link(_, _, target)]
            if prefix == "<" && target.url == "http:x")));
}

#[test]
fn monospace_runs_join_literal_and_late_attributes() {
    for (source, expected) in [
        ("`pre{missing}post`", "pre{missing}post"),
        ("`pre{word}post`\n\n:word: x", "prexpost"),
    ] {
        let document = AsciidocReader
            .read(source, &ReaderOptions::default())
            .unwrap();
        assert_eq!(
            document.blocks,
            [Block::Para(vec![Inline::Code(
                Box::default(),
                expected.into()
            )])],
            "{source:?}"
        );
    }
}

#[test]
fn monospace_headings_use_the_code_text_for_identifiers() {
    let document = AsciidocReader
        .read("== `Code heading`\n", &ReaderOptions::default())
        .unwrap();
    assert!(
        matches!(document.blocks.as_slice(), [Block::Header(_, attr, _)]
        if attr.id == "_code_heading")
    );
}

#[test]
fn mailto_macros_apply_label_attributes() {
    let document = AsciidocReader
        .read("mailto:x[role=wide]", &ReaderOptions::default())
        .unwrap();
    assert!(matches!(document.blocks.as_slice(), [Block::Para(inlines)]
        if matches!(inlines.as_slice(), [Inline::Link(attr, label, target)]
            if attr.classes == ["wide"] && label == &[Inline::Str("x".into())] && target.url == "x")));
}

mod definition_terms;
mod entities_and_anchors;
mod html_inlines;
mod recovery;
