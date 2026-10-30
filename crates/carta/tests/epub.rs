//! Snapshot unpacked EPUB archives for both dialects.
//!
//! Binary resources use size and content fingerprints. Review changes with `cargo insta review`.

#![cfg(all(feature = "read-commonmark", feature = "write-epub"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use carta::{
    EpubOptions, MediaBag, Output, ReaderOptions, WriterOptions, read_document, render_document,
};
use carta_core::container::zip;
use carta_core::media::{base64_encode, sha1_hex};

/// The directory holding the EPUB Markdown fixtures.
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/epub")
}

/// Every `*.md` fixture as `(stem, contents)`, ordered by stem for stable snapshot names.
fn fixtures() -> Vec<(String, String)> {
    let mut files: Vec<PathBuf> = fs::read_dir(fixtures_dir())
        .expect("read fixtures dir")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let stem = path
                .file_stem()
                .expect("file stem")
                .to_string_lossy()
                .replace('-', "_");
            let contents = fs::read_to_string(&path).expect("read fixture");
            (stem, contents)
        })
        .collect()
}

/// Render `markdown` to the EPUB dialect named by `to`, returning the raw archive bytes.
fn epub_bytes(markdown: &str, to: &str, options: &WriterOptions) -> Vec<u8> {
    let (document, media) =
        read_document("markdown", markdown.as_bytes(), &ReaderOptions::default())
            .expect("read markdown fixture");
    match render_document(to, document, media, options).expect("render epub") {
        Output::Bytes(bytes) => bytes,
        Output::Text(_) => panic!("an EPUB writer must produce bytes"),
    }
}

/// Whether an archive entry holds reviewable text (markup, styles) rather than binary payload.
fn is_text_entry(name: &str) -> bool {
    name == "mimetype"
        || [".xml", ".opf", ".ncx", ".xhtml", ".css"]
            .iter()
            .any(|extension| name.ends_with(extension))
}

/// Unpack an archive into a readable transcript: the stored entries in order, then every text
/// entry's contents in full and every binary entry summarized by its byte length.
fn describe_archive(bytes: &[u8]) -> String {
    let entries = zip::read(bytes).expect("valid epub archive");
    let mut out = String::from("=== entries ===\n");
    for entry in &entries {
        let _ = writeln!(out, "{} ({} bytes)", entry.name, entry.data.len());
    }
    for entry in &entries {
        let _ = write!(out, "\n=== {} ===\n", entry.name);
        if is_text_entry(&entry.name) {
            let text = String::from_utf8_lossy(&entry.data);
            out.push_str(&text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        } else {
            let fingerprint: String = sha1_hex(&entry.data).chars().take(16).collect();
            let _ = writeln!(
                out,
                "<{} bytes, binary sha1:{fingerprint}>",
                entry.data.len()
            );
        }
    }
    out
}

/// A minimal PNG header the cover-page sizing reads its 2×3 dimensions from. Only the signature and
/// `IHDR` fields are needed; the writer stores the bytes verbatim without decoding the image.
fn tiny_png() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(&[0, 0, 0, 13]);
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&2u32.to_be_bytes());
    bytes.extend_from_slice(&3u32.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes
}

/// A minimal GIF header carrying a 10×20 logical screen, enough for the writer to read the stored
/// image's dimensions. The bytes are kept verbatim.
fn tiny_gif() -> Vec<u8> {
    let mut bytes = b"GIF89a".to_vec();
    bytes.extend_from_slice(&10u16.to_le_bytes());
    bytes.extend_from_slice(&20u16.to_le_bytes());
    bytes
}

/// A minimal JPEG carrying a start-of-frame with a 40×30 frame, enough for the writer to read the
/// stored image's dimensions. The bytes are kept verbatim.
fn tiny_jpeg() -> Vec<u8> {
    vec![
        0xff, 0xd8, // start of image
        0xff, 0xc0, 0x00, 0x11, 0x08, // start of frame, precision 8
        0x00, 0x1e, // height 30
        0x00, 0x28, // width 40
    ]
}

#[test]
fn epub_fixture_snapshots() {
    for (stem, markdown) in fixtures() {
        for (target, dialect) in [("epub3", "epub3"), ("epub2", "epub2")] {
            let mut options = WriterOptions::default();
            options.toc = true;
            let bytes = epub_bytes(&markdown, target, &options);
            insta::assert_snapshot!(format!("{stem}__{dialect}"), describe_archive(&bytes));
        }
    }
}

#[test]
fn epub_numbered_sections_snapshot() {
    let markdown = fs::read_to_string(fixtures_dir().join("book.md")).expect("read book fixture");
    let mut options = WriterOptions::default();
    options.toc = true;
    options.number_sections = true;
    let bytes = epub_bytes(&markdown, "epub3", &options);
    insta::assert_snapshot!("book_numbered__epub3", describe_archive(&bytes));
}

#[test]
fn epub_generated_identifiers_snapshot() {
    let markdown = concat!(
        "# Getting Started\n\n",
        "An opening chapter with two subsections, neither carrying an identifier.\n\n",
        "## Installation\n\n",
        "Install steps.\n\n",
        "## Configuration\n\n",
        "Configuration steps.\n\n",
        "# Getting Started\n\n",
        "A second chapter reusing the first chapter's title.\n",
    );
    for dialect in ["epub3", "epub2"] {
        let (document, media) =
            read_document("commonmark", markdown.as_bytes(), &ReaderOptions::default())
                .expect("read commonmark fixture");
        let mut options = WriterOptions::default();
        options.toc = true;
        let bytes = match render_document(dialect, document, media, &options).expect("render epub")
        {
            Output::Bytes(bytes) => bytes,
            Output::Text(_) => panic!("an EPUB writer must produce bytes"),
        };
        insta::assert_snapshot!(
            format!("commonmark_generated_ids__{dialect}"),
            describe_archive(&bytes)
        );
    }
}

#[test]
fn epub_with_cover_snapshot() {
    let markdown = fs::read_to_string(fixtures_dir().join("book.md")).expect("read book fixture");
    let mut epub = EpubOptions::default();
    epub.cover_image = Some((String::from("cover.png"), tiny_png()));
    let mut options = WriterOptions::default();
    options.toc = true;
    options.epub = Arc::new(epub);
    let bytes = epub_bytes(&markdown, "epub3", &options);
    insta::assert_snapshot!("book_cover__epub3", describe_archive(&bytes));
}

#[test]
fn epub_embedded_resources_snapshot() {
    let markdown = fs::read_to_string(fixtures_dir().join("book.md")).expect("read book fixture");
    let mut epub = EpubOptions::default();
    epub.fonts = vec![
        (String::from("SourceSerif.otf"), b"otf-font-bytes".to_vec()),
        (String::from("SourceMono.ttf"), b"ttf-font-bytes".to_vec()),
    ];
    epub.stylesheets = vec![String::from("body { color: teal; }\n")];
    epub.subdirectory = Some(String::from("OEBPS"));
    epub.metadata_xml = Some(String::from(concat!(
        "<dc:identifier opf:scheme=\"ISBN-13\">978-3-16-148410-0</dc:identifier>\n",
        "<dc:source>Original manuscript</dc:source>\n",
    )));
    let mut options = WriterOptions::default();
    options.toc = true;
    options.epub = Arc::new(epub);
    let bytes = epub_bytes(&markdown, "epub3", &options);
    insta::assert_snapshot!("book_embedded_resources__epub3", describe_archive(&bytes));
}

#[test]
fn epub_body_images_snapshot() {
    let markdown = concat!(
        "# Illustrated\n\n",
        "A diagram: ![Diagram](diagram.gif)\n\n",
        "A photo: ![Photo](photo.jpg)\n\n",
        "Remote: ![Remote](https://example.com/remote.png)\n\n",
        "Missing: ![Missing](assets/missing.png)\n\n",
        "| Link | Note |\n",
        "| --- | --- |\n",
        "| [top](#illustrated) | see above |\n",
    );
    let mut media = MediaBag::new();
    media.insert("diagram.gif", Some(String::from("image/gif")), tiny_gif());
    media.insert("photo.jpg", Some(String::from("image/jpeg")), tiny_jpeg());
    let (document, _) = read_document("markdown", markdown.as_bytes(), &ReaderOptions::default())
        .expect("read markdown fixture");
    let mut options = WriterOptions::default();
    options.toc = true;
    let bytes = match render_document("epub3", document, media, &options).expect("render epub") {
        Output::Bytes(bytes) => bytes,
        Output::Text(_) => panic!("an EPUB writer must produce bytes"),
    };
    insta::assert_snapshot!("body_images__epub3", describe_archive(&bytes));
}

#[test]
fn epub_data_images_are_packaged_once() {
    let png = format!("data:image/png;base64,{}", base64_encode(&tiny_png()));
    let svg = "data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22/%3E";
    let markdown = format!("![PNG]({png})\n\n![SVG]({svg})\n\n![Again]({png})\n");
    for target in ["epub2", "epub3"] {
        let bytes = epub_bytes(&markdown, target, &WriterOptions::default());
        insta::assert_snapshot!(format!("data_images__{target}"), describe_archive(&bytes));
    }
}

#[test]
fn unavailable_images_keep_descriptions_captions_and_identifiers() {
    let source = "---\ntitle: '![Title description](missing-title.png)'\n---\n\n# Chapter\n\nInline ![image *description*](missing.png){#picture}.\n\n![Figure caption](https://example.invalid/missing.png)\n";
    for target in ["epub2", "epub3"] {
        let bytes = epub_bytes(source, target, &WriterOptions::default());
        let entries = zip::read(&bytes).unwrap();
        let mut pages = String::new();
        for entry in &entries {
            if entry.name.ends_with(".xhtml") {
                pages.push_str(std::str::from_utf8(&entry.data).unwrap());
            }
        }
        assert!(!pages.contains("<img"), "{pages}");
        for expected in [
            "Title description",
            "id=\"picture\"",
            "<em>description</em>",
            "Figure caption",
        ] {
            assert!(pages.contains(expected), "missing {expected}: {pages}");
        }
    }
}

#[test]
fn metadata_and_body_images_share_one_packaged_asset() {
    let image = format!("data:image/png;base64,{}", base64_encode(&tiny_png()));
    let source = format!("---\ntitle: '![Title]({image})'\n---\n\n# Chapter\n\n![Body]({image})\n");
    for target in ["epub2", "epub3"] {
        let bytes = epub_bytes(&source, target, &WriterOptions::default());
        let entries = zip::read(&bytes).unwrap();
        let assets: Vec<_> = entries
            .iter()
            .filter(|entry| entry.name.contains("/media/"))
            .collect();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].data, tiny_png());
        for page in ["EPUB/text/title_page.xhtml", "EPUB/text/ch001.xhtml"] {
            let entry = entries.iter().find(|entry| entry.name == page).unwrap();
            let text = std::str::from_utf8(&entry.data).unwrap();
            assert!(text.contains("src=\"../media/file0.png\""), "{text}");
            assert!(!text.contains("data:image"), "{text}");
        }
    }
}

#[test]
fn epub2_keeps_rich_content_without_html5_elements() {
    use carta::ast::{
        Attr, Block, Caption, Document, Inline, ListAttributes, ListNumberDelim, ListNumberStyle,
        Table,
    };
    let document = Document {
        blocks: vec![
            Block::Para(vec![
                Inline::Underline(vec![Inline::Str("underlined".into())]),
                Inline::Space,
                Inline::Span(
                    Box::new(Attr {
                        id: "highlight".into(),
                        classes: vec!["mark".into(), "underline".into()],
                        attributes: vec![("style".into(), "color: red".into())],
                    }),
                    vec![Inline::Str("highlighted".into())],
                ),
            ]),
            Block::OrderedList(
                ListAttributes {
                    start: 5,
                    style: ListNumberStyle::LowerAlpha,
                    delim: ListNumberDelim::OneParen,
                },
                vec![
                    vec![Block::Para(vec![Inline::Str("entry".into())])],
                    Vec::new(),
                ],
            ),
            Block::BulletList(vec![
                vec![Block::Plain(vec![
                    Inline::Str("☐".into()),
                    Inline::Space,
                    Inline::Str("open".into()),
                ])],
                vec![Block::Plain(vec![
                    Inline::Str("☒".into()),
                    Inline::Space,
                    Inline::Str("done".into()),
                ])],
            ]),
            Block::Table(Box::new(Table {
                attr: Attr {
                    id: "empty-table".into(),
                    ..Attr::default()
                },
                caption: Caption {
                    long: vec![
                        Block::Para(vec![Inline::Str("First caption".into())]),
                        Block::Para(vec![Inline::Str("Second caption".into())]),
                    ],
                    ..Caption::default()
                },
                ..Table::default()
            })),
            Block::BulletList(Vec::new()),
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::DefaultDelim,
                },
                Vec::new(),
            ),
            Block::DefinitionList(Vec::new()),
        ],
        ..Document::default()
    };
    let Output::Bytes(bytes) = render_document(
        "epub2",
        document,
        MediaBag::new(),
        &WriterOptions::default(),
    )
    .unwrap() else {
        panic!("expected bytes");
    };
    let entries = zip::read(&bytes).unwrap();
    let chapter = entries
        .iter()
        .find(|entry| entry.name == "EPUB/text/ch001.xhtml")
        .unwrap();
    let text = std::str::from_utf8(&chapter.data).unwrap();
    for forbidden in [
        "<u>",
        "<mark",
        "<input",
        "<label",
        " start=",
        "<caption><p>",
    ] {
        assert!(!text.contains(forbidden), "unexpected {forbidden}: {text}");
    }
    for expected in [
        "underlined",
        "highlighted",
        "text-decoration: underline",
        "background-color: yellow",
        "color: red",
        "id=\"highlight\"",
        "e)",
        "f)",
        "[ ] open",
        "[x] done",
        "First caption",
        "Second caption",
        "id=\"empty-table\"",
        "<td></td>",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}

#[test]
fn epub_cross_chapter_table_cell_id_snapshot() {
    let markdown = concat!(
        "# Overview\n\n",
        "Jump to the [definition](#term-widget) in the next chapter.\n\n",
        "# Glossary\n\n",
        "| Term | Meaning |\n",
        "| --- | --- |\n",
        "| [Widget]{#term-widget} | A small component. |\n",
    );
    let mut options = WriterOptions::default();
    options.toc = true;
    let bytes = epub_bytes(markdown, "epub3", &options);
    insta::assert_snapshot!(
        "cross_chapter_table_cell_id__epub3",
        describe_archive(&bytes)
    );
}

#[test]
fn epub_strips_forbidden_control_characters() {
    let markdown = "# Clean\u{b}Title\n\nA\u{b}body.\n";
    let mut options = WriterOptions::default();
    options.toc = true;
    for target in ["epub3", "epub2"] {
        let transcript = describe_archive(&epub_bytes(markdown, target, &options));
        assert!(
            !transcript.contains('\u{b}'),
            "{target}: a forbidden control character reached the archive"
        );
        assert!(
            transcript.contains("CleanTitle"),
            "{target}: heading text was lost when the control character was dropped"
        );
    }
}

#[test]
fn epub_empty_document_keeps_a_linear_spine_item() {
    for target in ["epub3", "epub2"] {
        for toc in [true, false] {
            let mut options = WriterOptions::default();
            options.toc = toc;
            let bytes = epub_bytes("", target, &options);
            let entries = zip::read(&bytes).expect("valid epub archive");
            let package = entries
                .iter()
                .find(|entry| entry.name.ends_with("content.opf"))
                .expect("a package document");
            let opf = String::from_utf8_lossy(&package.data);
            let spine = opf
                .split_once("<spine")
                .and_then(|(_, rest)| rest.split_once("</spine>"))
                .map(|(inner, _)| inner)
                .expect("a spine element");
            let itemrefs = spine.matches("<itemref").count();
            let non_linear = spine.matches("linear=\"no\"").count();
            assert!(itemrefs > 0, "{target}/toc={toc}: the spine is empty");
            assert!(
                itemrefs > non_linear,
                "{target}/toc={toc}: every spine item is non-linear"
            );
        }
    }
}

#[test]
fn epub_sanitizes_a_font_filename_with_spaces() {
    let markdown = "# One\n\nBody.\n";
    let mut epub = EpubOptions::default();
    epub.fonts = vec![(String::from("Source Serif.otf"), b"otf-bytes".to_vec())];
    let mut options = WriterOptions::default();
    options.epub = Arc::new(epub);
    let transcript = describe_archive(&epub_bytes(markdown, "epub3", &options));
    assert!(
        transcript.contains("fonts/Source_Serif.otf"),
        "the font path was not sanitized:\n{transcript}"
    );
    assert!(
        !transcript.contains("Source Serif.otf"),
        "an unsanitized font path with a space leaked into the archive"
    );
}
