//! The DOCX container writer: it renders a document as an Office Open XML word-processing package,
//! a ZIP archive of XML parts holding the main document body, its relationships, list numbering,
//! footnotes, comments, the styling design, and the package metadata.
//!
//! Output is byte-reproducible: the archive stamps fixed timestamps, property dates come from a
//! fixed epoch unless one is supplied, and every part is generated deterministically. A reference
//! document, when given, supplies the styling parts (styles, settings, fonts, theme) so the generated
//! content adopts its look while the body itself stays carta's.

mod document;
mod metadata;
mod notes;
mod numbering;
mod package;
mod styles;

use carta_ast::Document;
use carta_core::container::xml::Element;
use carta_core::container::zip::{self, ZipArchive, ZipEntry};
use carta_core::{BytesWriter, Extension, Result, WriterOptions};

/// The word-processing markup namespaces declared on the document, footnotes and comments roots,
/// each as `(prefix attribute, namespace URI)`. The URIs are the format's published wire vocabulary.
const WML_NAMESPACES: &[(&str, &str)] = &[
    (
        "xmlns:w",
        "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    ),
    (
        "xmlns:m",
        "http://schemas.openxmlformats.org/officeDocument/2006/math",
    ),
    (
        "xmlns:r",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    ),
    ("xmlns:o", "urn:schemas-microsoft-com:office:office"),
    ("xmlns:v", "urn:schemas-microsoft-com:vml"),
    ("xmlns:w10", "urn:schemas-microsoft-com:office:word"),
    (
        "xmlns:a",
        "http://schemas.openxmlformats.org/drawingml/2006/main",
    ),
    (
        "xmlns:pic",
        "http://schemas.openxmlformats.org/drawingml/2006/picture",
    ),
    (
        "xmlns:wp",
        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    ),
];

/// Builds a word-processing markup root element with the full namespace set declared.
fn wml_root(name: &str) -> Element {
    let mut root = Element::new(name);
    for (prefix, uri) in WML_NAMESPACES {
        root = root.attr(prefix, uri);
    }
    root
}

/// The DOCX container writer.
#[derive(Debug)]
pub struct DocxWriter;

impl BytesWriter for DocxWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<Vec<u8>> {
        write_docx(document, options)
    }
}

/// Assembles the complete DOCX archive.
fn write_docx(document: &Document, options: &WriterOptions) -> Result<Vec<u8>> {
    let docx = &options.docx;
    let epoch = docx.source_date_epoch.unwrap_or(1);

    // A reference document is parsed once; its styling parts stand in for carta's defaults.
    let reference = match docx.reference_doc.as_deref() {
        Some(bytes) => Some(zip::read(bytes)?),
        None => None,
    };
    let reference = reference.as_deref();

    let features = document::Features {
        keep_empty_paragraphs: options.extensions.contains(Extension::EmptyParagraphs),
        native_numbering: options.extensions.contains(Extension::NativeNumbering),
    };
    #[cfg(feature = "highlight")]
    let highlighter: document::DocxHl = options.highlight.highlighter.clone();
    #[cfg(not(feature = "highlight"))]
    let highlighter: document::DocxHl = ();
    let rendered = document::document_xml(
        &document.blocks,
        &document.meta,
        options.media.clone(),
        features,
        highlighter,
    );

    let mut archive = ZipArchive::new();
    archive.deflate(
        "[Content_Types].xml",
        package::content_types(&rendered.images).as_bytes(),
    )?;
    archive.deflate("_rels/.rels", package::root_rels().as_bytes())?;
    archive.deflate(
        "word/_rels/document.xml.rels",
        package::document_rels(&rendered.images, &rendered.hyperlinks).as_bytes(),
    )?;
    archive.deflate(
        "word/_rels/footnotes.xml.rels",
        package::footnotes_rels(&rendered.hyperlinks).as_bytes(),
    )?;
    archive.deflate("word/document.xml", rendered.document_xml.as_bytes())?;
    archive.deflate(
        "word/footnotes.xml",
        notes::footnotes_xml(rendered.footnotes).as_bytes(),
    )?;
    archive.deflate(
        "word/comments.xml",
        notes::comments_xml(rendered.comments).as_bytes(),
    )?;
    archive.deflate(
        "word/numbering.xml",
        numbering::numbering_xml(&rendered.numbering).as_bytes(),
    )?;
    // Image bytes are already in a compressed form, so they are stored without re-deflation.
    for image in &rendered.images {
        archive.store(&format!("word/media/{}", image.file_name), &image.bytes)?;
    }
    #[cfg(feature = "highlight")]
    let styles_part: std::borrow::Cow<'_, str> =
        match (&options.highlight.highlighter, &options.highlight.theme) {
            (Some(_), Some(theme)) => {
                std::borrow::Cow::Owned(styles::styles_with_highlighting(theme))
            }
            _ => std::borrow::Cow::Borrowed(styles::STYLES),
        };
    #[cfg(not(feature = "highlight"))]
    let styles_part: std::borrow::Cow<'_, str> = std::borrow::Cow::Borrowed(styles::STYLES);
    styling(&mut archive, reference, "word/styles.xml", &styles_part)?;
    styling(
        &mut archive,
        reference,
        "word/settings.xml",
        styles::SETTINGS,
    )?;
    styling(
        &mut archive,
        reference,
        "word/webSettings.xml",
        styles::WEB_SETTINGS,
    )?;
    styling(
        &mut archive,
        reference,
        "word/fontTable.xml",
        styles::FONT_TABLE,
    )?;
    styling(
        &mut archive,
        reference,
        "word/theme/theme1.xml",
        styles::THEME,
    )?;
    archive.deflate(
        "docProps/core.xml",
        metadata::core_xml(&document.meta, epoch).as_bytes(),
    )?;
    archive.deflate("docProps/app.xml", metadata::app_xml().as_bytes())?;
    archive.deflate(
        "docProps/custom.xml",
        metadata::custom_xml(&document.meta).as_bytes(),
    )?;
    archive.finish()
}

/// Writes a styling part, preferring the reference document's version of it over carta's default.
fn styling(
    archive: &mut ZipArchive,
    reference: Option<&[ZipEntry]>,
    name: &str,
    default: &str,
) -> Result<()> {
    match reference.and_then(|entries| find_part(entries, name)) {
        Some(bytes) => archive.deflate(name, bytes),
        None => archive.deflate(name, default.as_bytes()),
    }
}

/// Finds a part by name among a reference document's entries.
fn find_part<'a>(entries: &'a [ZipEntry], name: &str) -> Option<&'a [u8]> {
    entries
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.data.as_slice())
}
