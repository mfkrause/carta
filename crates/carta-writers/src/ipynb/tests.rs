use super::*;
use carta_ast::Inline;

fn para(text: &str) -> Block {
    Block::Para(vec![Inline::Str(text.to_owned().into())])
}

fn write(blocks: Vec<Block>) -> String {
    IpynbWriter
        .write(
            &Document {
                blocks,
                ..Document::default()
            },
            &WriterOptions::default(),
        )
        .expect("write")
}

#[test]
fn empty_document_is_an_empty_notebook() {
    assert_eq!(
        write(Vec::new()),
        "{\n \"cells\": [],\n \"nbformat\": 4,\n \"nbformat_minor\": 5,\n \"metadata\": {}\n}"
    );
}

#[test]
fn loose_blocks_become_one_markdown_cell() {
    let notebook = write(vec![
        Block::Header(
            1,
            Box::default(),
            vec![Inline::Str("Title".to_owned().into())],
        ),
        para("Body."),
    ]);
    assert!(notebook.contains("\"cell_type\": \"markdown\""));
    assert!(notebook.contains("\"# Title\\n\""));
    assert!(notebook.contains("\"\\n\""));
    assert!(notebook.contains("\"Body.\""));
    assert_eq!(notebook.matches("\"cell_type\"").count(), 1);
}

#[test]
fn source_lines_keep_trailing_newlines() {
    let Json::Array(lines) = source_lines("a\n\nb") else {
        panic!("expected array");
    };
    assert_eq!(lines.len(), 3);
    let mut rendered = String::new();
    Json::Array(lines).write_to(&mut rendered, 0);
    assert_eq!(rendered, "[\n \"a\\n\",\n \"\\n\",\n \"b\"\n]");

    let Json::Array(empty) = source_lines("") else {
        panic!("expected array");
    };
    assert!(empty.is_empty());
}

#[test]
fn cell_div_selects_kind_and_keeps_id() {
    let notebook = write(vec![Block::Div(
        Box::new(Attr {
            id: "given".to_owned().into(),
            classes: vec!["cell".to_owned().into(), "code".to_owned().into()],
            attributes: vec![("execution_count".to_owned().into(), "7".to_owned().into())],
        }),
        vec![Block::CodeBlock(
            Box::default(),
            "print(1)".to_owned().into(),
        )],
    )]);
    assert!(notebook.contains("\"cell_type\": \"code\""));
    assert!(notebook.contains("\"execution_count\": 7"));
    assert!(notebook.contains("\"outputs\": []"));
    assert!(notebook.contains("\"print(1)\""));
    assert!(notebook.contains("\"id\": \"given\""));
}

#[test]
fn raw_cell_carries_mime_type() {
    let notebook = write(vec![Block::Div(
        Box::new(Attr {
            id: String::new().into(),
            classes: vec!["cell".to_owned().into(), "raw".to_owned().into()],
            attributes: Vec::new(),
        }),
        vec![Block::RawBlock(
            Format("html".to_owned().into()),
            "<b>x</b>".to_owned().into(),
        )],
    )]);
    assert!(notebook.contains("\"cell_type\": \"raw\""));
    assert!(notebook.contains("\"raw_mimetype\": \"text/html\""));
    assert!(notebook.contains("\"<b>x</b>\""));
}

#[test]
fn raw_cell_maps_asciidoc_mime() {
    let notebook = write(vec![Block::Div(
        Box::new(Attr {
            id: String::new().into(),
            classes: vec!["cell".to_owned().into(), "raw".to_owned().into()],
            attributes: Vec::new(),
        }),
        vec![Block::RawBlock(
            Format("asciidoc".to_owned().into()),
            "[NOTE]\n====\nbody\n====".to_owned().into(),
        )],
    )]);
    assert!(notebook.contains("\"cell_type\": \"raw\""));
    assert!(notebook.contains("\"raw_mimetype\": \"text/asciidoc\""));
}

#[test]
fn stream_and_error_outputs_round_trip() {
    let notebook = write(vec![Block::Div(
        Box::new(Attr {
            id: String::new().into(),
            classes: vec!["cell".to_owned().into(), "code".to_owned().into()],
            attributes: Vec::new(),
        }),
        vec![
            Block::CodeBlock(Box::default(), "x".to_owned().into()),
            Block::Div(
                Box::new(Attr {
                    id: String::new().into(),
                    classes: vec![
                        "output".to_owned().into(),
                        "stream".to_owned().into(),
                        "stdout".to_owned().into(),
                    ],
                    attributes: Vec::new(),
                }),
                vec![Block::CodeBlock(Box::default(), "hi\n".to_owned().into())],
            ),
            Block::Div(
                Box::new(Attr {
                    id: String::new().into(),
                    classes: vec!["output".to_owned().into(), "error".to_owned().into()],
                    attributes: vec![
                        ("ename".to_owned().into(), "ValueError".to_owned().into()),
                        ("evalue".to_owned().into(), "bad".to_owned().into()),
                    ],
                }),
                vec![Block::CodeBlock(
                    Box::default(),
                    "trace\n".to_owned().into(),
                )],
            ),
        ],
    )]);
    assert!(notebook.contains("\"output_type\": \"stream\""));
    assert!(notebook.contains("\"name\": \"stdout\""));
    assert!(notebook.contains("\"output_type\": \"error\""));
    assert!(notebook.contains("\"ename\": \"ValueError\""));
    assert!(notebook.contains("\"evalue\": \"bad\""));
}

/// A one-code-cell document whose single `display_data` output is an image referencing `url`.
fn image_output_document(url: &str) -> Document {
    Document {
        blocks: vec![Block::Div(
            Box::new(Attr {
                id: String::new().into(),
                classes: vec!["cell".to_owned().into(), "code".to_owned().into()],
                attributes: Vec::new(),
            }),
            vec![
                Block::CodeBlock(Box::default(), "plot()".to_owned().into()),
                Block::Div(
                    Box::new(Attr {
                        id: String::new().into(),
                        classes: vec!["output".to_owned().into(), "display_data".to_owned().into()],
                        attributes: Vec::new(),
                    }),
                    vec![Block::Para(vec![Inline::Image(
                        Box::default(),
                        Vec::new(),
                        Box::new(carta_ast::Target {
                            url: url.to_owned().into(),
                            title: String::new().into(),
                        }),
                    )])],
                ),
            ],
        )],
        ..Document::default()
    }
}

#[test]
fn image_output_without_embedded_data_is_unrepresentable() {
    // With no media bag, the referenced bytes are unavailable, so the output cannot be rebuilt.
    let document = image_output_document("plot.png");
    match IpynbWriter.write(&document, &WriterOptions::default()) {
        Err(Error::Unrepresentable(message)) => assert!(
            message.contains("plot.png"),
            "message should name the file: {message}"
        ),
        other => panic!("expected an unrepresentable error, got {other:?}"),
    }
}

#[test]
fn image_output_data_is_re_embedded_from_the_media_bag() {
    let bytes = vec![1u8, 2, 3, 4, 5, 6, 7];
    let mut bag = MediaBag::new();
    bag.insert("plot.png", Some("image/png".to_owned()), bytes.clone());
    let mut options = WriterOptions::default();
    options.media = std::sync::Arc::new(bag);

    let notebook = IpynbWriter
        .write(&image_output_document("plot.png"), &options)
        .expect("the image bytes are available, so the output writes");
    // The bundle names the image's MIME type and carries the base64 of the bag's bytes.
    assert!(notebook.contains("\"image/png\""));
    let encoded = base64_encode_mime(&bytes);
    assert!(
        notebook.contains(encoded.trim_end_matches('\n')),
        "notebook should embed the base64 payload"
    );
}

#[test]
fn svg_output_data_is_re_embedded_as_source_lines() {
    let mut bag = MediaBag::new();
    bag.insert(
        "fig.svg",
        Some("image/svg+xml".to_owned()),
        b"<svg/>".to_vec(),
    );
    let mut options = WriterOptions::default();
    options.media = std::sync::Arc::new(bag);

    let notebook = IpynbWriter
        .write(&image_output_document("fig.svg"), &options)
        .expect("the svg bytes are available, so the output writes");
    assert!(notebook.contains("\"image/svg+xml\""));
    assert!(notebook.contains("<svg/>"));
}

#[test]
fn markdown_cell_image_becomes_an_inline_attachment() {
    let bytes = vec![9u8, 8, 7, 6];
    let mut bag = MediaBag::new();
    bag.insert("cell-diagram.png", Some("image/png".to_owned()), bytes);
    let mut options = WriterOptions::default();
    options.media = std::sync::Arc::new(bag);

    // A markdown cell whose image references a bag entry by its file name.
    let document = Document {
        blocks: vec![Block::Div(
            Box::new(Attr {
                id: "cell".to_owned().into(),
                classes: vec!["cell".to_owned().into(), "markdown".to_owned().into()],
                attributes: Vec::new(),
            }),
            vec![Block::Para(vec![Inline::Image(
                Box::default(),
                vec![Inline::Str("a diagram".to_owned().into())],
                Box::new(carta_ast::Target {
                    url: "cell-diagram.png".to_owned().into(),
                    title: String::new().into(),
                }),
            )])],
        )],
        ..Document::default()
    };
    let notebook = IpynbWriter.write(&document, &options).expect("writes");
    // The link is rewritten to the attachment form and the payload restored under that name.
    assert!(notebook.contains("attachment:cell-diagram.png"));
    assert!(notebook.contains("\"attachments\""));
    assert!(notebook.contains("\"cell-diagram.png\""));
}

#[test]
fn image_output_metadata_is_restored_sorted_and_typed() {
    // Display metadata rides on the image's attributes; restored sorted and typed.
    let bytes = vec![1u8, 2, 3, 4];
    let mut bag = MediaBag::new();
    bag.insert("plot.png", Some("image/png".to_owned()), bytes);
    let mut options = WriterOptions::default();
    options.media = std::sync::Arc::new(bag);

    let image = Inline::Image(
        Box::new(Attr {
            id: String::new().into(),
            classes: Vec::new(),
            attributes: vec![
                ("width".to_owned().into(), "320".to_owned().into()),
                ("height".to_owned().into(), "240".to_owned().into()),
                (
                    "needs_background".to_owned().into(),
                    "light".to_owned().into(),
                ),
            ],
        }),
        Vec::new(),
        Box::new(carta_ast::Target {
            url: "plot.png".to_owned().into(),
            title: String::new().into(),
        }),
    );
    let document = Document {
        blocks: vec![Block::Div(
            Box::new(Attr {
                id: String::new().into(),
                classes: vec!["cell".to_owned().into(), "code".to_owned().into()],
                attributes: Vec::new(),
            }),
            vec![
                Block::CodeBlock(Box::default(), "plot()".to_owned().into()),
                Block::Div(
                    Box::new(Attr {
                        id: String::new().into(),
                        classes: vec!["output".to_owned().into(), "display_data".to_owned().into()],
                        attributes: Vec::new(),
                    }),
                    vec![Block::Para(vec![image])],
                ),
            ],
        )],
        ..Document::default()
    };

    let notebook = IpynbWriter.write(&document, &options).expect("writes");
    assert!(notebook.contains("\"height\": 240"));
    assert!(notebook.contains("\"width\": 320"));
    assert!(notebook.contains("\"needs_background\": \"light\""));
    let height = notebook.find("\"height\"").expect("height key");
    let background = notebook
        .find("\"needs_background\"")
        .expect("background key");
    let width = notebook.find("\"width\"").expect("width key");
    assert!(
        height < background && background < width,
        "metadata keys are not sorted:\n{notebook}"
    );
}

#[test]
fn image_output_without_metadata_has_an_empty_metadata_object() {
    let Json::Object(entries) =
        output_metadata(&[Block::Para(vec![Inline::Str("text".to_owned().into())])])
    else {
        panic!("expected object");
    };
    assert!(entries.is_empty());
}

#[test]
fn cell_attachments_are_emitted_in_sorted_key_order() {
    let bytes = vec![9u8, 8, 7, 6];
    let mut bag = MediaBag::new();
    bag.insert("fig-a.png", Some("image/png".to_owned()), bytes.clone());
    bag.insert("fig-b.png", Some("image/png".to_owned()), bytes);
    let mut options = WriterOptions::default();
    options.media = std::sync::Arc::new(bag);

    // The cell references b before a; the emitted attachments object is keyed in sorted order.
    let reference = |name: &str| {
        Block::Para(vec![Inline::Image(
            Box::default(),
            Vec::new(),
            Box::new(carta_ast::Target {
                url: name.to_owned().into(),
                title: String::new().into(),
            }),
        )])
    };
    let document = Document {
        blocks: vec![Block::Div(
            Box::new(Attr {
                id: "fig".to_owned().into(),
                classes: vec!["cell".to_owned().into(), "markdown".to_owned().into()],
                attributes: Vec::new(),
            }),
            vec![reference("fig-b.png"), reference("fig-a.png")],
        )],
        ..Document::default()
    };

    let notebook = IpynbWriter.write(&document, &options).expect("writes");
    let (_, attachments) = notebook
        .split_once("\"attachments\"")
        .expect("attachments object present");
    let a = attachments.find("fig-a.png").expect("a key present");
    let b = attachments.find("fig-b.png").expect("b key present");
    assert!(a < b, "attachments are not sorted:\n{notebook}");
}

#[test]
fn metadata_attribute_values_are_typed() {
    let attributes = vec![
        ("collapsed".to_owned().into(), "true".to_owned().into()),
        ("count".to_owned().into(), "5".to_owned().into()),
        ("name".to_owned().into(), "hello".to_owned().into()),
        ("tags".to_owned().into(), "[\"a\",\"b\"]".to_owned().into()),
    ];
    let Json::Object(entries) = attribute_metadata(&attributes, &[]) else {
        panic!("expected object");
    };
    let mut rendered = String::new();
    Json::Object(entries).write_to(&mut rendered, 0);
    assert!(rendered.contains("\"collapsed\": true"));
    assert!(rendered.contains("\"count\": 5"));
    assert!(rendered.contains("\"name\": \"hello\""));
    assert!(rendered.contains("\"a\","));
}

#[test]
fn notebook_metadata_drops_version_keys() {
    let mut jupyter = BTreeMap::new();
    jupyter.insert(
        "nbformat".to_owned().into(),
        MetaValue::MetaString("4".to_owned().into()),
    );
    jupyter.insert(
        "nbformat_minor".to_owned().into(),
        MetaValue::MetaString("5".to_owned().into()),
    );
    jupyter.insert("kept".to_owned().into(), MetaValue::MetaBool(true));
    let mut meta = BTreeMap::new();
    meta.insert("jupyter".to_owned().into(), MetaValue::MetaMap(jupyter));

    let Json::Object(pairs) = notebook_metadata(&meta) else {
        panic!("expected object");
    };
    assert_eq!(pairs.len(), 1);
    let mut rendered = String::new();
    Json::Object(pairs).write_to(&mut rendered, 0);
    assert!(rendered.contains("\"kept\": true"));
    assert!(!rendered.contains("nbformat"));
}

#[test]
fn control_characters_use_short_or_unicode_escapes() {
    let mut out = String::new();
    escape_string("a\"\\\n\t\u{8}\u{c}\u{1}/<", &mut out);
    assert_eq!(out, "\"a\\\"\\\\\\n\\t\\u0008\\u000c\\u0001/<\"");
}

#[test]
fn generated_ids_are_stable_and_distinct() {
    let mut counter = 0;
    let first = next_id("", "alpha", &mut counter);
    let second = next_id("", "beta", &mut counter);
    assert_ne!(first, second);
    assert_eq!(first.len(), 36);
    assert_eq!(first.as_bytes().get(14), Some(&b'4'));

    let mut again = 0;
    assert_eq!(next_id("", "alpha", &mut again), first);
}
