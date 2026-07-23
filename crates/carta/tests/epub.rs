//! Layer 1 golden tests for the EPUB container writer. Each fixture is read from Markdown, rendered
//! to both EPUB dialects, and the archive is unpacked into a readable transcript (an entry listing
//! followed by every text file's contents) that `insta` freezes. Binary resources are summarized by
//! size and a short content fingerprint, since their bytes are not reviewable; the fingerprint still
//! pins the exact payload, so swapping two same-sized binaries is caught. The output is
//! byte-reproducible: the archive uses a fixed timestamp and a content-derived identifier, so these
//! snapshots are stable across runs.
//!
//! Reviewed with `cargo insta review`; never hand-edit the `.snap` files.

#![cfg(all(feature = "read-commonmark", feature = "write-epub"))]
// Integration-test harness code: panicking on a known fixture is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use carta::{
    EpubOptions, MediaBag, Output, ReaderOptions, WriterOptions, read_document, render_document,
};
use carta_core::container::zip;
use carta_core::media::sha1_hex;

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

/// Each fixture, rendered to both dialects with a table of contents requested, exercises the shared
/// pipeline end to end: metadata projection, chapter splitting, both navigation documents, the
/// package document, and the fixed-order archive assembly.
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

/// Numbering the sections threads a section-number span through each heading; the body carries it and
/// the navigation relabels it, so the numbered output is frozen on its own.
#[test]
fn epub_numbered_sections_snapshot() {
    let markdown = fs::read_to_string(fixtures_dir().join("book.md")).expect("read book fixture");
    let mut options = WriterOptions::default();
    options.toc = true;
    options.number_sections = true;
    let bytes = epub_bytes(&markdown, "epub3", &options);
    insta::assert_snapshot!("book_numbered__epub3", describe_archive(&bytes));
}

/// Headings whose source carries no identifier (as bare `CommonMark` leaves them) must still yield
/// live navigation targets. The writer derives each section's identifier from its heading text and
/// disambiguates a repeated one with a numeric suffix; reading through `CommonMark` (which assigns no
/// identifiers) freezes that derivation and the fallback on the second identical heading.
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

/// A cover image adds a cover page, a `cover-image` manifest entry, a spine slot and reading-order
/// landmarks; this freezes that whole cover machinery.
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

/// Embedded fonts, a replacement stylesheet, a custom container subdirectory, and a Dublin Core
/// metadata fragment together exercise the resource and packaging options: the fonts are stored and
/// manifested, the built-in stylesheet is replaced, every path moves under the chosen directory, and
/// the fragment overrides the publication identifier and carries an extra element through to the
/// package.
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

/// Body images are stored under `media/` and their references rewritten: an embedded GIF and JPEG
/// are sized from their headers and carried into the container, a remote reference is left as
/// authored, and an unresolved relative reference is climbed back to the container root. A table
/// carrying a cross-reference exercises identifier collection and link rewriting through table cells.
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

/// An identifier borne by a span inside a table cell in one chapter still anchors a link authored in
/// another. Identifier collection descends into table cells and link rewriting resolves the fragment
/// to the file holding the cell, so the cross-chapter reference targets the right chapter document
/// rather than dangling at an unresolved `#fragment`.
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

/// A control character an author slips into a heading must never reach the container: it would make
/// the package, navigation and NCX documents ill-formed XML. Every XML-forbidden character is
/// dropped on the way out, so no archive file carries one while the surrounding text survives.
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

/// An empty document still yields a valid reading order: a publication must carry at least one linear
/// spine item, so a reading system has a first page to open. Whether or not a table of contents is
/// requested, not every spine item may be marked non-linear.
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

/// A font whose file name carries a space is stored under a sanitized path, so neither the archive
/// entry nor its manifest href holds a character that would need escaping.
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
