use super::{MAX_REPEATED_SPACES, OdtReader, parse_length};
use carta_ast::{Attr, Block, Citation, CitationMode, Inline, Target};
use carta_core::container::zip::ZipArchive;
use carta_core::{BytesReader, ReaderOptions};

/// Wraps body markup in a minimal `content.xml` document.
fn content(body: &str) -> String {
    format!(
        "<office:document-content>\
             <office:body><office:text>{body}</office:text></office:body>\
             </office:document-content>"
    )
}

/// Packages named parts into an ODT (ZIP) archive.
fn package(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = ZipArchive::new();
    for (name, data) in parts {
        archive.deflate(name, data).expect("store part");
    }
    archive.finish().expect("finish archive")
}

fn read(input: &[u8]) -> carta_core::Result<carta_ast::Document> {
    OdtReader.read(input, &ReaderOptions::default())
}

#[test]
fn a_well_formed_package_reads_its_body() {
    let odt = package(&[("content.xml", content("<text:p>Hi</text:p>").as_bytes())]);
    let document = read(&odt).expect("read odt");
    assert_eq!(
        document.blocks,
        vec![Block::Para(vec![Inline::Str("Hi".into())])]
    );
}

#[test]
fn inline_containers_do_not_leak_text_or_register_hidden_bookmarks() {
    let body = content(
        "<text:p>Before<text:p>Hidden</text:p>\
         <text:list><text:list-item><text:p>Hidden</text:p></text:list-item></text:list>\
         <text:unknown><text:bookmark text:name=\"hidden\"/>Hidden</text:unknown>\
         <text:sequence>A<text:span>Hidden</text:span>B</text:sequence>After</text:p>\
         <text:p><text:bookmark text:name=\"shown\"/></text:p>",
    );
    let odt = package(&[("content.xml", body.as_bytes())]);
    assert_eq!(
        read(&odt).expect("read inline containers").blocks,
        vec![
            Block::Para(vec![Inline::Str("BeforeABAfter".into())]),
            Block::Para(vec![Inline::Span(
                Box::new(Attr {
                    id: "anchor".into(),
                    ..Attr::default()
                }),
                Vec::new(),
            )]),
        ]
    );
}

#[test]
fn cross_references_and_marks_share_forward_targets() {
    for reference in ["bookmark-ref", "reference-ref"] {
        for mark in ["bookmark", "reference-mark-start"] {
            let body = content(&format!(
                "<text:p><text:{reference}>A<text:span>Hidden</text:span>B</text:{reference}>\
                 <text:{reference} text:ref-name=\"target\">A<text:span>Hidden</text:span>B</text:{reference}>\
                 <text:{mark} text:name=\"target\"/></text:p>"
            ));
            let odt = package(&[("content.xml", body.as_bytes())]);
            assert_eq!(
                read(&odt).expect("read shared forward target").blocks,
                vec![Block::Para(vec![
                    Inline::Str("AB".into()),
                    Inline::Link(
                        Box::default(),
                        vec![Inline::Str("AB".into())],
                        Box::new(Target {
                            url: "#anchor".into(),
                            title: "".into(),
                        }),
                    ),
                    Inline::Span(
                        Box::new(Attr {
                            id: "anchor".into(),
                            ..Attr::default()
                        }),
                        Vec::new(),
                    ),
                ])]
            );
        }
    }
}

#[test]
fn bibliography_marks_keep_the_identifier_and_only_direct_text() {
    let mark = "<text:bibliography-mark text:identifier=\"source\">\
                A<text:span>Hidden</text:span> B</text:bibliography-mark>";
    let body = content(&format!(
        "<text:p>{mark}</text:p><text:h text:outline-level=\"1\">{mark}</text:h>"
    ));
    let odt = package(&[("content.xml", body.as_bytes())]);
    let citation = Inline::Cite(
        vec![Citation {
            id: "source".into(),
            prefix: Vec::new(),
            suffix: Vec::new(),
            mode: CitationMode::NormalCitation,
            note_num: 0,
            hash: 0,
        }],
        vec![
            Inline::Str("A".into()),
            Inline::Space,
            Inline::Str("B".into()),
        ],
    );
    assert_eq!(
        read(&odt).expect("read bibliography mark").blocks,
        vec![
            Block::Para(vec![citation.clone()]),
            Block::Header(
                1,
                Box::new(Attr {
                    id: "a-b".into(),
                    ..Attr::default()
                }),
                vec![citation],
            )
        ]
    );
}

#[test]
fn cells_keep_paragraph_roles_and_lists_without_reserving_hidden_heading_ids() {
    let body = content(
        "<table:table><table:table-row><table:table-cell>\
         <text:p>Keep</text:p>\
         <text:p text:style-name=\"Preformatted_20_Text\">Code</text:p>\
         <text:p text:style-name=\"Table\">Caption</text:p>\
         <text:list><text:list-item><text:p>Item</text:p></text:list-item></text:list>\
         <text:h text:outline-level=\"1\">Heading</text:h>\
         <table:table><table:table-row><table:table-cell><text:p>Hidden</text:p>\
         </table:table-cell></table:table-row></table:table>\
         <text:section><text:p>Hidden</text:p></text:section>\
         </table:table-cell></table:table-row></table:table>\
         <text:h text:outline-level=\"1\">Heading</text:h>",
    );
    let styles = "<office:document-styles><office:styles>\
        <style:style style:name=\"Table\" style:family=\"paragraph\"/>\
        <style:style style:name=\"Preformatted_20_Text\" style:family=\"paragraph\"/>\
        </office:styles></office:document-styles>";
    let odt = package(&[
        ("content.xml", body.as_bytes()),
        ("styles.xml", styles.as_bytes()),
    ]);
    let document = read(&odt).expect("read cell blocks");
    let Some(Block::Table(table)) = document.blocks.first() else {
        panic!("expected a table");
    };
    let cell = table
        .bodies
        .first()
        .and_then(|body| body.body.first())
        .and_then(|row| row.cells.first())
        .expect("expected a cell");
    let [
        Block::Para(keep),
        Block::CodeBlock(_, code),
        Block::Div(attr, caption),
        Block::OrderedList(_, items),
    ] = cell.content.as_slice()
    else {
        panic!("expected paragraph roles and a list");
    };
    assert_eq!(keep, &vec![Inline::Str("Keep".into())]);
    assert_eq!(code, "Code");
    assert!(matches!(attr.classes.as_slice(), [class] if class == "caption"));
    assert_eq!(
        caption,
        &vec![Block::Para(vec![Inline::Str("Caption".into())])]
    );
    assert_eq!(
        items,
        &vec![vec![Block::Plain(vec![Inline::Str("Item".into())])]]
    );
    assert!(
        matches!(document.blocks.get(1), Some(Block::Header(_, attr, _)) if attr.id == "heading")
    );
}

#[test]
fn direct_tables_in_list_items_do_not_split_code_runs() {
    let body = content(
        "<text:list><text:list-item>\
         <text:p text:style-name=\"Preformatted_20_Text\">Before</text:p>\
         <table:table><table:table-row><table:table-cell><text:p>Hidden</text:p>\
         </table:table-cell></table:table-row></table:table>\
         <text:p text:style-name=\"Preformatted_20_Text\">After</text:p>\
         </text:list-item></text:list>",
    );
    let styles = "<office:document-styles><office:styles>\
        <style:style style:name=\"Preformatted_20_Text\" style:family=\"paragraph\"/>\
        </office:styles></office:document-styles>";
    let odt = package(&[
        ("content.xml", body.as_bytes()),
        ("styles.xml", styles.as_bytes()),
    ]);
    let document = read(&odt).expect("read list with a direct table");
    let [Block::OrderedList(_, items)] = document.blocks.as_slice() else {
        panic!("expected a list");
    };
    assert_eq!(
        items,
        &vec![vec![Block::CodeBlock(
            Box::default(),
            "Before\nAfter".into()
        )]]
    );
}

#[test]
fn notes_skip_other_blocks_without_splitting_code_or_reserving_heading_ids() {
    for note_class in ["footnote", "endnote"] {
        let body = content(&format!(
            "<text:p><text:note text:note-class=\"{note_class}\"><text:note-body>\
             <text:p text:style-name=\"Preformatted_20_Text\">Before</text:p>\
             <text:h text:outline-level=\"1\">Heading</text:h>\
             <text:list><text:list-item><text:p>Hidden</text:p></text:list-item></text:list>\
             <table:table><table:table-row><table:table-cell><text:p>Hidden</text:p>\
             </table:table-cell></table:table-row></table:table>\
             <text:section><text:p>Hidden</text:p></text:section>\
             <text:p text:style-name=\"Preformatted_20_Text\">After</text:p>\
             </text:note-body></text:note></text:p>\
             <text:h text:outline-level=\"1\">Heading</text:h>"
        ));
        let styles = "<office:document-styles><office:styles>\
            <style:style style:name=\"Preformatted_20_Text\" style:family=\"paragraph\"/>\
            </office:styles></office:document-styles>";
        let odt = package(&[
            ("content.xml", body.as_bytes()),
            ("styles.xml", styles.as_bytes()),
        ]);
        assert_eq!(
            read(&odt).expect("read note blocks").blocks,
            vec![
                Block::Para(vec![Inline::Note(vec![Block::CodeBlock(
                    Box::default(),
                    "Before\nAfter".into()
                )])]),
                Block::Header(
                    1,
                    Box::new(Attr {
                        id: "heading".into(),
                        ..Attr::default()
                    }),
                    vec![Inline::Str("Heading".into())]
                )
            ]
        );
    }
}

#[test]
fn wide_soft_breaks_are_optional_and_hard_breaks_remain() {
    let body = content(
        "<text:p>漢\n字</text:p><text:p>漢<text:line-break/>字</text:p>\
         <text:p><text:note><text:note-citation>1</text:note-citation>\
         <text:note-body><text:p>🙂\n🙃</text:p></text:note-body></text:note></text:p>",
    );
    let odt = package(&[("content.xml", body.as_bytes())]);
    let default = read(&odt).expect("read default breaks");
    assert!(matches!(
        default.blocks.first(),
        Some(Block::Para(inlines))
            if matches!(inlines.as_slice(), [Inline::Str(_), Inline::SoftBreak, Inline::Str(_)])
    ));
    let mut options = ReaderOptions::default();
    options.extensions =
        carta_core::Extensions::from_list(&[carta_core::Extension::EastAsianLineBreaks]);
    assert_eq!(
        OdtReader
            .read(&odt, &options)
            .expect("read wide breaks")
            .blocks,
        vec![
            Block::Para(vec![Inline::Str("漢".into()), Inline::Str("字".into())]),
            Block::Para(vec![
                Inline::Str("漢".into()),
                Inline::LineBreak,
                Inline::Str("字".into()),
            ]),
            Block::Para(vec![Inline::Note(vec![Block::Para(vec![
                Inline::Str("🙂".into()),
                Inline::Str("🙃".into()),
            ])])]),
        ]
    );
}

#[test]
fn cached_fields_do_not_add_display_text() {
    for field in [
        "date",
        "page-number",
        "page-count",
        "author-name",
        "title",
        "chapter",
        "file-name",
        "sequence-ref",
        "note-ref",
        "meta",
    ] {
        let body = content(&format!(
            "<text:p>Before<text:{field}><text:span>cached</text:span></text:{field}>After</text:p>"
        ));
        let odt = package(&[("content.xml", body.as_bytes())]);
        assert_eq!(
            read(&odt).expect("read cached field").blocks,
            vec![Block::Para(vec![Inline::Str("BeforeAfter".into())])]
        );
    }
}

#[test]
fn first_style_definitions_win_in_each_family() {
    let first = "<style:style style:name=\"Inline\" style:family=\"text\">\
        <style:text-properties fo:font-weight=\"bold\"/></style:style>\
        <style:style style:name=\"Paragraph\" style:family=\"paragraph\">\
        <style:paragraph-properties fo:margin-left=\"1in\"/></style:style>\
        <text:list-style style:name=\"List\"><text:list-level-style-bullet \
        text:level=\"1\" text:bullet-char=\"*\"/></text:list-style>";
    let later = "<style:style style:name=\"Inline\" style:family=\"text\">\
        <style:text-properties fo:font-style=\"italic\"/></style:style>\
        <style:style style:name=\"Paragraph\" style:family=\"paragraph\">\
        <style:paragraph-properties fo:margin-left=\"0in\"/></style:style>\
        <text:list-style style:name=\"List\"><text:list-level-style-number \
        text:level=\"1\" style:num-format=\"I\"/></text:list-style>";
    let body = content(
        "<text:p><text:span text:style-name=\"Inline\">Styled</text:span></text:p>\
         <text:p text:style-name=\"Paragraph\">Quoted</text:p>\
         <text:list text:style-name=\"List\"><text:list-item><text:p>Item</text:p></text:list-item></text:list>",
    )
    .replace(
        "<office:body>",
        &format!("<office:automatic-styles>{later}</office:automatic-styles><office:body>"),
    );
    for groups in [
        format!("<office:styles>{first}{later}</office:styles>"),
        format!(
            "<office:styles>{later}</office:styles><office:automatic-styles>{first}</office:automatic-styles>"
        ),
        format!(
            "<office:automatic-styles>{first}</office:automatic-styles><office:styles>{later}</office:styles>"
        ),
    ] {
        let styles = format!("<office:document-styles>{groups}</office:document-styles>");
        let odt = package(&[
            ("content.xml", body.as_bytes()),
            ("styles.xml", styles.as_bytes()),
        ]);
        assert_eq!(
            read(&odt).expect("read colliding styles").blocks,
            vec![
                Block::Para(vec![Inline::Strong(vec![Inline::Str("Styled".into())])]),
                Block::BlockQuote(vec![Block::Para(vec![Inline::Str("Quoted".into())])]),
                Block::BulletList(vec![vec![Block::Plain(vec![Inline::Str("Item".into())])]]),
            ]
        );
    }
}

#[test]
fn cached_list_numbers_do_not_add_item_text() {
    let body = content(
        "<text:list><text:list-item><text:p><text:number>1.</text:number>Text</text:p>\
         </text:list-item></text:list>",
    );
    let odt = package(&[("content.xml", body.as_bytes())]);
    let document = read(&odt).expect("read cached list number");
    let Some(Block::OrderedList(_, items)) = document.blocks.first() else {
        panic!("expected an ordered list");
    };
    assert_eq!(
        items,
        &vec![vec![Block::Plain(vec![Inline::Str("Text".into())])]]
    );
}

#[test]
fn ruby_wrappers_do_not_add_base_or_annotation_text() {
    let body = content(
        "<text:p>Before<text:ruby><text:ruby-base>Base</text:ruby-base>\
         <text:ruby-text>Annotation</text:ruby-text></text:ruby>After</text:p>",
    );
    let odt = package(&[("content.xml", body.as_bytes())]);
    assert_eq!(
        read(&odt).expect("read ruby text").blocks,
        vec![Block::Para(vec![Inline::Str("BeforeAfter".into())])]
    );
}

#[test]
fn non_block_containers_do_not_add_blocks_or_heading_identifiers() {
    for container in ["numbered-paragraph", "unknown"] {
        let body = content(&format!(
            "<text:p>Before</text:p><text:{container}>\
         <text:p>Hidden</text:p><text:h text:outline-level=\"1\">Title</text:h>\
         </text:{container}><text:p>After</text:p>\
         <text:h text:outline-level=\"1\">Title</text:h>"
        ));
        let odt = package(&[("content.xml", body.as_bytes())]);
        assert_eq!(
            read(&odt).expect("read non-block container").blocks,
            vec![
                Block::Para(vec![Inline::Str("Before".into())]),
                Block::Para(vec![Inline::Str("After".into())]),
                Block::Header(
                    1,
                    Box::new(Attr {
                        id: "title".into(),
                        ..Attr::default()
                    }),
                    vec![Inline::Str("Title".into())]
                ),
            ]
        );
    }
}

#[test]
fn images_without_targets_are_dropped() {
    for target in ["", "xlink:href=\"\""] {
        let body = content(&format!(
            "<text:p>Before<draw:frame><draw:image {target}>\
             <office:binary-data>iVBORw0KGgo=</office:binary-data>\
             </draw:image></draw:frame>After</text:p>"
        ));
        let odt = package(&[("content.xml", body.as_bytes())]);
        assert_eq!(
            read(&odt).expect("read image without a target").blocks,
            vec![Block::Para(vec![Inline::Str("BeforeAfter".into())])]
        );
    }
}

#[test]
fn package_targets_remove_one_parent_prefix_and_keep_image_bytes() {
    let body = content(
        "<text:p><text:a xlink:href=\"../../document#section\">Link</text:a>\
         <draw:frame><draw:image xlink:href=\"../Pictures/image.png\"/></draw:frame></text:p>",
    );
    let image = b"image bytes";
    let odt = package(&[
        ("content.xml", body.as_bytes()),
        ("Pictures/image.png", image),
    ]);
    let (document, media) = OdtReader
        .read_media(&odt, &ReaderOptions::default())
        .expect("read package targets");
    let Some(Block::Para(inlines)) = document.blocks.first() else {
        panic!("expected a paragraph");
    };
    assert!(matches!(
        inlines.as_slice(),
        [Inline::Link(_, _, link), Inline::Image(_, _, image)]
            if link.url == "../document#section" && image.url == "Pictures/image.png"
    ));
    assert_eq!(
        media
            .get("Pictures/image.png")
            .map(|item| item.bytes.as_slice()),
        Some(image.as_slice())
    );
}

#[test]
fn links_filter_direct_children_but_keep_span_content() {
    for (markup, middle) in [
        (
            "<draw:frame><draw:image xlink:href=\"Pictures/image.png\"/></draw:frame>",
            Inline::Image(
                Box::default(),
                vec![],
                Box::new(Target {
                    url: "Pictures/image.png".into(),
                    title: "".into(),
                }),
            ),
        ),
        ("<text:line-break/>", Inline::LineBreak),
        (
            "<text:a xlink:href=\"/inner\">Inner</text:a>",
            Inline::Link(
                Box::default(),
                vec![Inline::Str("Inner".into())],
                Box::new(Target {
                    url: "/inner".into(),
                    title: "".into(),
                }),
            ),
        ),
    ] {
        for wrapped in [false, true] {
            let inner = if wrapped {
                format!("<text:span>{markup}</text:span>")
            } else {
                markup.to_owned()
            };
            let body = content(&format!(
                "<text:p><text:a xlink:href=\"/outer\">Before{inner}After</text:a></text:p>"
            ));
            let odt = package(&[
                ("content.xml", body.as_bytes()),
                ("Pictures/image.png", b"image bytes"),
            ]);
            let (document, media) = OdtReader
                .read_media(&odt, &ReaderOptions::default())
                .expect("read link content");
            let Some(Block::Para(inlines)) = document.blocks.first() else {
                panic!("expected a paragraph");
            };
            let [Inline::Link(_, label, target)] = inlines.as_slice() else {
                panic!("expected a link");
            };
            assert_eq!(target.url, "/outer");
            let expected = if wrapped {
                vec![
                    Inline::Str("Before".into()),
                    middle.clone(),
                    Inline::Str("After".into()),
                ]
            } else {
                vec![Inline::Str("BeforeAfter".into())]
            };
            assert_eq!(label, &expected);
            assert_eq!(
                media.get("Pictures/image.png").is_some(),
                wrapped && matches!(&middle, Inline::Image(..))
            );
        }
    }
}

#[test]
fn space_counts_follow_integer_reading_rules() {
    for (value, count) in [
        ("-1", 0),
        ("+2", 1),
        (" 2 ", 2),
        ("0x2", 2),
        ("(2)", 2),
        ("0b10", 0),
        ("2.5", 1),
        ("invalid", 1),
    ] {
        let body = content(&format!("<text:p>A<text:s text:c=\"{value}\"/>B</text:p>"));
        let odt = package(&[("content.xml", body.as_bytes())]);
        let document = read(&odt).expect("read space count");
        assert_eq!(
            carta_ast::to_plain_text(carta_ast::single_block_inlines(&document.blocks)),
            format!("A{}B", " ".repeat(count))
        );
    }
}

#[test]
fn inline_boundaries_merge_breaks_but_keep_explicit_spaces() {
    for (markup, whitespace) in [
        (" <text:soft-page-break/> ", vec![Inline::Space]),
        (" <text:bookmark-end/>\n", vec![Inline::SoftBreak]),
        (" <draw:frame/><text:line-break/>", vec![Inline::LineBreak]),
        (" <text:s text:c=\"3\"/> ", vec![Inline::Space; 3]),
        (
            "<text:line-break/><draw:frame/><text:line-break/>",
            vec![Inline::LineBreak; 2],
        ),
    ] {
        let body = content(&format!("<text:p>Before{markup}After</text:p>"));
        let odt = package(&[("content.xml", body.as_bytes())]);
        let mut inlines = vec![Inline::Str("Before".into())];
        inlines.extend(whitespace);
        inlines.push(Inline::Str("After".into()));
        assert_eq!(
            read(&odt).expect("read inline whitespace").blocks,
            vec![Block::Para(inlines)]
        );
    }
}

#[test]
fn heading_levels_keep_large_signed_values() {
    for level in [i64::MIN, -2_147_483_649, 2_147_483_648, i64::MAX] {
        let body = content(&format!(
            "<text:h text:outline-level=\"{level}\">Heading</text:h>"
        ));
        let odt = package(&[("content.xml", body.as_bytes())]);
        let document = read(&odt).expect("read heading level");
        assert!(
            matches!(document.blocks.as_slice(), [Block::Header(actual, _, _)] if *actual == level)
        );
    }
}

#[test]
fn list_starts_keep_large_signed_values() {
    let body = content(
        "<text:list text:style-name=\"L\"><text:list-item><text:p>Item</text:p></text:list-item></text:list>",
    );
    for start in [i64::MIN, -2_147_483_649, 2_147_483_648, i64::MAX] {
        let styles = format!(
            "<office:document-styles><office:styles><text:list-style style:name=\"L\">\
             <text:list-level-style-number text:level=\"1\" style:num-format=\"1\" text:start-value=\"{start}\"/>\
             </text:list-style></office:styles></office:document-styles>"
        );
        let odt = package(&[
            ("content.xml", body.as_bytes()),
            ("styles.xml", styles.as_bytes()),
        ]);
        let document = read(&odt).expect("read list start");
        assert!(
            matches!(document.blocks.as_slice(), [Block::OrderedList(attr, _)] if attr.start == start)
        );
    }
}

#[test]
fn unknown_number_formats_make_bullet_lists() {
    let body = content(
        "<text:list text:style-name=\"L\"><text:list-item><text:p>Item</text:p></text:list-item></text:list>",
    );
    for format in ["", "x", "01", "一"] {
        let styles = format!(
            "<office:document-styles><office:styles><text:list-style style:name=\"L\">\
             <text:list-level-style-number text:level=\"1\" style:num-format=\"{format}\"/>\
             </text:list-style></office:styles></office:document-styles>"
        );
        let odt = package(&[
            ("content.xml", body.as_bytes()),
            ("styles.xml", styles.as_bytes()),
        ]);
        let document = read(&odt).expect("read unknown number format");
        assert!(matches!(document.blocks.as_slice(), [Block::BulletList(_)]));
    }
}

#[test]
fn sections_keep_their_containers_inside_lists_and_body() {
    let body = content(
        "<text:section text:name=\"Named\"><text:section/><text:p>Text</text:p></text:section>\
         <text:list><text:list-item><text:section><text:p>Inside</text:p></text:section>\
         <text:p>After</text:p></text:list-item></text:list>",
    );
    let odt = package(&[("content.xml", body.as_bytes())]);
    let document = read(&odt).expect("read sections");
    assert_eq!(
        document.blocks[0],
        Block::Div(
            Box::default(),
            vec![
                Block::Div(Box::default(), vec![]),
                Block::Para(vec![Inline::Str("Text".into())]),
            ]
        )
    );
    let Block::OrderedList(_, items) = &document.blocks[1] else {
        panic!("expected a list");
    };
    assert_eq!(
        items,
        &vec![vec![
            Block::Div(
                Box::default(),
                vec![Block::Para(vec![Inline::Str("Inside".into())])]
            ),
            Block::Plain(vec![Inline::Str("After".into())]),
        ]]
    );
}

#[test]
fn compaction_depends_on_the_final_paragraph() {
    let paragraph = Block::Para(vec![Inline::Str("Text".into())]);
    let plain = Block::Plain(vec![Inline::Str("Text".into())]);
    let container = Block::Div(Box::default(), vec![paragraph.clone()]);
    for (blocks, expected) in [
        (
            vec![container.clone(), paragraph.clone()],
            vec![container.clone(), plain.clone()],
        ),
        (
            vec![paragraph.clone(), container.clone()],
            vec![paragraph.clone(), container.clone()],
        ),
        (
            vec![plain.clone(), paragraph.clone(), paragraph.clone()],
            vec![paragraph.clone(), paragraph.clone(), paragraph.clone()],
        ),
        (
            vec![plain.clone(), container.clone()],
            vec![plain, container],
        ),
    ] {
        assert_eq!(super::compact(blocks), expected);
    }
}

#[test]
fn code_flattens_inline_breaks_but_keeps_paragraph_breaks() {
    let body = content(
        "<text:p text:style-name=\"Preformatted_20_Text\">A<text:line-break/>B</text:p>\
         <text:p text:style-name=\"Preformatted_20_Text\">C\nD</text:p>\
         <text:p><text:span text:style-name=\"Source_20_Text\">E<text:line-break/>F</text:span></text:p>",
    );
    let styles = "<office:document-styles><office:styles>\
        <style:style style:name=\"Preformatted_20_Text\" style:family=\"paragraph\"/>\
        <style:style style:name=\"Source_20_Text\" style:family=\"text\"/>\
        </office:styles></office:document-styles>";
    let odt = package(&[
        ("content.xml", body.as_bytes()),
        ("styles.xml", styles.as_bytes()),
    ]);
    assert_eq!(
        read(&odt).expect("read code breaks").blocks,
        vec![
            Block::CodeBlock(Box::default(), "A B\nC D".into()),
            Block::Para(vec![Inline::Code(Box::default(), "E F".into())]),
        ]
    );
}

#[test]
fn table_caption_style_keeps_its_class() {
    let body = content(
        "<text:p text:style-name=\"Table\">Caption</text:p>\
         <text:p text:style-name=\"Alias\">Alias</text:p>\
         <text:p text:style-name=\"_54_able\">Encoded</text:p>",
    );
    let styles = "<office:document-styles><office:styles>\
        <style:style style:name=\"Table\" style:family=\"paragraph\"/>\
        <style:style style:name=\"Alias\" style:family=\"paragraph\" style:parent-style-name=\"Table\"/>\
        <style:style style:name=\"_54_able\" style:family=\"paragraph\"/>\
        </office:styles></office:document-styles>";
    let odt = package(&[
        ("content.xml", body.as_bytes()),
        ("styles.xml", styles.as_bytes()),
    ]);
    let document = read(&odt).expect("read caption");
    let caption = Block::Para(vec![Inline::Str("Caption".into())]);
    assert_eq!(
        document.blocks,
        vec![
            Block::Div(
                Box::new(Attr {
                    classes: vec!["caption".into()],
                    ..Attr::default()
                }),
                vec![caption.clone()]
            ),
            Block::Para(vec![Inline::Str("Alias".into())]),
            Block::Para(vec![Inline::Str("Encoded".into())]),
        ]
    );
    let caption_body = content("<text:p text:style-name=\"Table\">Caption</text:p>");
    let unstyled = package(&[("content.xml", caption_body.as_bytes())]);
    assert_eq!(
        read(&unstyled).expect("read unstyled caption").blocks,
        vec![caption]
    );
    let preformatted_styles = styles.replace(
        "<style:style style:name=\"Table\" style:family=\"paragraph\"/>",
        "<style:style style:name=\"Table\" style:family=\"paragraph\" style:parent-style-name=\"Preformatted_20_Text\"/>\
         <style:style style:name=\"Preformatted_20_Text\" style:family=\"paragraph\"/>",
    );
    let preformatted = package(&[
        ("content.xml", caption_body.as_bytes()),
        ("styles.xml", preformatted_styles.as_bytes()),
    ]);
    assert_eq!(
        read(&preformatted).expect("read code caption").blocks,
        vec![Block::CodeBlock(Box::default(), "Caption".into())]
    );
}

#[test]
fn figure_conversion_follows_list_and_cell_compaction() {
    let figure = "<text:p><draw:frame><draw:text-box><text:p>\
        <draw:frame><draw:image xlink:href=\"image.png\"/></draw:frame>Figure 1: Caption\
        </text:p></draw:text-box></draw:frame></text:p>";
    for (extra, expected_figures, expected_title) in
        [("", 0, "fig:"), ("<text:p>After</text:p>", 1, "")]
    {
        for body in [
            format!("<text:list><text:list-item>{figure}{extra}</text:list-item></text:list>"),
            format!(
                "<table:table><table:table-row><table:table-cell>{figure}{extra}</table:table-cell></table:table-row></table:table>"
            ),
        ] {
            let body = content(&body);
            let odt = package(&[("content.xml", body.as_bytes())]);
            let mut document = read(&odt).expect("read compacted figure");
            assert!(matches!(
                document.blocks.first(),
                Some(Block::OrderedList(..) | Block::Table(_))
            ));
            let mut figures = 0;
            let mut images = Vec::new();
            carta_core::walk::for_each_block(&mut document.blocks, &mut |block| {
                if matches!(block, Block::Figure(..)) {
                    figures += 1;
                }
                if let Block::Plain(inlines) = block
                    && let Some(Inline::Image(_, alt, target)) = inlines.first()
                {
                    images.push((carta_ast::to_plain_text(alt), target.title.to_string()));
                }
            });
            assert_eq!(figures, expected_figures);
            assert_eq!(
                images,
                vec![("Figure 1: Caption".to_string(), expected_title.to_string())]
            );
        }
    }
}

#[test]
fn caption_classes_wrap_framed_figures() {
    let body = content(
        "<text:p text:style-name=\"Table\"><draw:frame><draw:text-box><text:p>\
         <draw:frame><draw:image xlink:href=\"image.png\"/></draw:frame>Figure 1: Caption\
         </text:p></draw:text-box></draw:frame></text:p>",
    );
    let styles = "<office:document-styles><office:styles>\
        <style:style style:name=\"Table\" style:family=\"paragraph\"/>\
        </office:styles></office:document-styles>";
    let odt = package(&[
        ("content.xml", body.as_bytes()),
        ("styles.xml", styles.as_bytes()),
    ]);
    let document = read(&odt).expect("read framed caption");
    let Some(Block::Div(attr, blocks)) = document.blocks.first() else {
        panic!("expected a caption container");
    };
    assert!(matches!(attr.classes.as_slice(), [class] if class == "caption"));
    let Some(Block::Figure(_, caption, _)) = blocks.first() else {
        panic!("expected a figure");
    };
    assert_eq!(
        carta_ast::to_plain_text(carta_ast::single_block_inlines(&caption.long)),
        "Caption"
    );
}

#[test]
fn table_captions_attach_only_to_leading_pairs() {
    let table = "<table:table><table:table-row><table:table-cell>\
        <text:p>Cell</text:p></table:table-cell></table:table-row></table:table>";
    let first = "<text:p text:style-name=\"Table\">First</text:p>";
    let second = "<text:p text:style-name=\"Table\">Second</text:p>";
    let styles = "<office:document-styles><office:styles>\
        <style:style style:name=\"Table\" style:family=\"paragraph\"/>\
        </office:styles></office:document-styles>";
    for (body, expected, count) in [
        (format!("{table}{first}"), vec!["First"], 1),
        (
            format!("{table}{first}{table}{second}"),
            vec!["First", "Second"],
            2,
        ),
        (format!("<text:p>Intro</text:p>{table}{first}"), vec![""], 3),
        (format!("{table}{table}{first}"), vec!["", ""], 3),
    ] {
        let body = content(&body);
        let odt = package(&[
            ("content.xml", body.as_bytes()),
            ("styles.xml", styles.as_bytes()),
        ]);
        let document = read(&odt).expect("read table captions");
        let captions: Vec<_> = document
            .blocks
            .iter()
            .filter_map(|block| {
                if let Block::Table(table) = block {
                    Some(carta_ast::to_plain_text(carta_ast::single_block_inlines(
                        &table.caption.long,
                    )))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(captions, expected);
        assert_eq!(document.blocks.len(), count);
    }
}

#[test]
fn figure_caption_labels_do_not_change_image_alt_text() {
    for (original, expected) in [
        ("Figure 1: Caption", "Caption"),
        ("Illustration 2: Caption", "Caption"),
        ("Label :: Caption", "Caption"),
        ("Figure 1.2: Caption", "Figure 1.2: Caption"),
        ("Figure ١: Caption", "Figure ١: Caption"),
    ] {
        let body = content(&format!(
            "<text:p><draw:frame><draw:text-box><text:p>\
             <draw:frame><draw:image xlink:href=\"image.png\"/></draw:frame>{original}\
             </text:p></draw:text-box></draw:frame></text:p>"
        ));
        let odt = package(&[("content.xml", body.as_bytes())]);
        let document = read(&odt).expect("read figure");
        let Some(Block::Figure(_, caption, blocks)) = document.blocks.first() else {
            panic!("expected a figure");
        };
        assert_eq!(
            carta_ast::to_plain_text(carta_ast::single_block_inlines(&caption.long)),
            expected
        );
        let Some(Block::Plain(inlines)) = blocks.first() else {
            panic!("expected the figure body");
        };
        let Some(Inline::Image(_, alt, _)) = inlines.first() else {
            panic!("expected an image");
        };
        assert_eq!(carta_ast::to_plain_text(alt), original);
    }
}

#[test]
fn a_missing_content_part_is_an_error() {
    let odt = package(&[("styles.xml", b"<office:document-styles/>")]);
    assert!(read(&odt).is_err());
}

#[test]
fn an_unparsable_content_part_is_an_error() {
    let odt = package(&[("content.xml", b"%%% not markup %%%")]);
    assert!(read(&odt).is_err());
}

#[test]
fn a_pathological_space_repeat_is_clamped_not_crashed() {
    // `usize::MAX` spaces would exhaust memory; the count is bounded instead.
    let body = "<text:p>A<text:s text:c=\"18446744073709551615\"/>B</text:p>";
    let odt = package(&[("content.xml", content(body).as_bytes())]);
    let document = read(&odt).expect("read odt");
    let Some(Block::Para(inlines)) = document.blocks.first() else {
        panic!("expected a paragraph");
    };
    let spaces = inlines
        .iter()
        .filter(|inline| matches!(inline, Inline::Space))
        .count();
    assert_eq!(spaces, MAX_REPEATED_SPACES);
}

#[test]
fn parse_length_resolves_absolute_units_only() {
    assert_eq!(parse_length("0.5in"), Some(0.5));
    assert_eq!(parse_length("2.54cm"), Some(1.0));
    assert_eq!(parse_length("72pt"), Some(1.0));
    assert_eq!(parse_length("50%"), None);
    assert_eq!(parse_length("5"), None);
    assert_eq!(parse_length("10zz"), None);
}
