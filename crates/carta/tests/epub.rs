//! Layer 1 golden tests for the EPUB container writer. Each fixture is read from Markdown, rendered
//! to both EPUB dialects, and the archive is unpacked into a readable transcript — an entry listing
//! followed by every text file's contents — that `insta` freezes. Binary resources are summarized by
//! size, since their bytes are not reviewable. The output is byte-reproducible: the archive uses a
//! fixed timestamp and a content-derived identifier, so these snapshots are stable across runs.
//!
//! Reviewed with `cargo insta review`; never hand-edit the `.snap` files.

#![cfg(all(feature = "read-commonmark", feature = "write-epub"))]
// Integration-test harness code: panicking on a known fixture is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use carta::{
    EpubOptions, MediaBag, Output, ReaderOptions, WriterOptions, read_document, render_document,
};
use carta_core::container::zip;

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
            let _ = writeln!(out, "<{} bytes, binary>", entry.data.len());
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

/// Headings whose source carries no identifier — as bare `CommonMark` leaves them — must still yield
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
    options.epub = epub;
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
    options.epub = epub;
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
