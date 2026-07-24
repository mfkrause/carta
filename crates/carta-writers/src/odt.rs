//! Writer for the `OpenDocument` Text (ODT) package format.
//!
//! Renders a [`carta_ast::Document`] into a complete, deterministic ODT archive: a Zip container
//! holding the uncompressed `mimetype` marker, the package manifest, the document body
//! (`content.xml`), the stylesheet (`styles.xml`), the metadata (`meta.xml`), an RDF package
//! description, and any embedded images under `Pictures/`. The archive bytes are reproducible across
//! runs: the Zip layer stamps a fixed timestamp, maps are ordered, and the timestamps written into
//! the metadata derive from a fixed epoch rather than the wall clock.

use std::collections::BTreeMap;

use carta_ast::{Document, MetaValue, Text};
use carta_core::container::zip::ZipArchive;
use carta_core::{BytesWriter, Extension, Result, WriterOptions};

mod blocks;
mod helpers;
mod inlines;
mod media;
mod meta;
mod styles;

use meta::{formula_content_xml, manifest_xml, meta_xml, plain_meta};
use styles::{deco_style_xml, para_style_xml, styles_xml};

/// The package's MIME marker; the first archive entry, stored uncompressed.
const MIMETYPE: &str = "application/vnd.oasis.opendocument.text";

/// Timestamp seed for the metadata dates. A fixed value keeps the output byte-reproducible; it names
/// the first second of 1970 in UTC.
const DATE_EPOCH: i64 = 1;

/// Headroom kept below the stack limit before a nested sequence grows a fresh segment.
const STACK_RED_ZONE: usize = 128 * 1024;
/// Size of each stack segment grown on demand once [`STACK_RED_ZONE`] headroom is unavailable.
const STACK_SEGMENT: usize = 32 * 1024 * 1024;

/// The namespace declarations shared by every top-level part element.
const NS: &str = concat!(
    " xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\"",
    " xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\"",
    " xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\"",
    " xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\"",
    " xmlns:draw=\"urn:oasis:names:tc:opendocument:xmlns:drawing:1.0\"",
    " xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\"",
    " xmlns:xlink=\"http://www.w3.org/1999/xlink\"",
    " xmlns:dc=\"http://purl.org/dc/elements/1.1/\"",
    " xmlns:meta=\"urn:oasis:names:tc:opendocument:xmlns:meta:1.0\"",
    " xmlns:number=\"urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0\"",
    " xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\"",
    " xmlns:math=\"http://www.w3.org/1998/Math/MathML\"",
);

const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

/// The two table-cell styles a table references; emitted once when the document contains any table.
const CELL_STYLES: &str = concat!(
    "<style:style style:name=\"TableHeaderRowCell\" style:family=\"table-cell\">",
    "<style:table-cell-properties fo:border=\"none\" /></style:style>",
    "<style:style style:name=\"TableRowCell\" style:family=\"table-cell\">",
    "<style:table-cell-properties fo:border=\"none\" /></style:style>",
);

/// The fixed RDF description of the package parts.
const MANIFEST_RDF: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n",
    "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">",
    "<rdf:Description rdf:about=\"styles.xml\">",
    "<rdf:type rdf:resource=\"http://docs.oasis-open.org/ns/office/1.2/meta/odf#StylesFile\" />",
    "</rdf:Description>",
    "<rdf:Description rdf:about=\"\">",
    "<ns0:hasPart xmlns:ns0=\"http://docs.oasis-open.org/ns/office/1.2/meta/pkg#\" rdf:resource=\"styles.xml\" />",
    "</rdf:Description>",
    "<rdf:Description rdf:about=\"content.xml\">",
    "<rdf:type rdf:resource=\"http://docs.oasis-open.org/ns/office/1.2/meta/odf#ContentFile\" />",
    "</rdf:Description>",
    "<rdf:Description rdf:about=\"\">",
    "<ns0:hasPart xmlns:ns0=\"http://docs.oasis-open.org/ns/office/1.2/meta/pkg#\" rdf:resource=\"content.xml\" />",
    "<rdf:type xmlns:ns0=\"http://docs.oasis-open.org/ns/office/1.2/meta/pkg#\" rdf:resource=\"http://docs.oasis-open.org/ns/office/1.2/meta/pkg#Document\" />",
    "</rdf:Description>",
    "</rdf:RDF>",
);

/// The two graphic styles a formula frame references: `fr1` anchors an inline formula in the text
/// line, `fr2` centers a display formula in its own paragraph. Emitted once when the document holds
/// any formula.
const FORMULA_STYLES: &str = concat!(
    "<style:style style:name=\"fr1\" style:family=\"graphic\" style:parent-style-name=\"Formula\">",
    "<style:graphic-properties style:vertical-pos=\"middle\" style:vertical-rel=\"text\" />",
    "</style:style>",
    "<style:style style:name=\"fr2\" style:family=\"graphic\" style:parent-style-name=\"Formula\">",
    "<style:graphic-properties style:vertical-pos=\"middle\" style:vertical-rel=\"text\" ",
    "style:horizontal-pos=\"center\" style:horizontal-rel=\"paragraph-content\" style:wrap=\"none\" />",
    "</style:style>",
);

/// The `Formula-N/settings.xml` part, recording whether the formula is set in text mode (inline) or
/// display mode.
fn formula_settings_xml(text_mode: bool) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <office:document-settings \
         xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
         xmlns:config=\"urn:oasis:names:tc:opendocument:xmlns:config:1.0\" office:version=\"1.3\">\
         <office:settings><config:config-item-set config:name=\"ooo:configuration-settings\">\
         <config:config-item config:name=\"IsTextMode\" config:type=\"boolean\">{text_mode}\
         </config:config-item></config:config-item-set></office:settings></office:document-settings>"
    )
}

/// Renders documents to the `OpenDocument` Text package format.
#[derive(Debug, Default, Clone, Copy)]
pub struct OdtWriter;

impl BytesWriter for OdtWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<Vec<u8>> {
        write_odt(document, options)
    }
}

fn write_odt(document: &Document, options: &WriterOptions) -> Result<Vec<u8>> {
    let mut builder = Builder::new(options);
    builder.document(document);

    let content = builder.finish_content();
    let styles = styles_xml(&document.meta);
    let meta = meta_xml(&document.meta);
    let manifest = manifest_xml(&builder.images, &builder.formulas);

    let mut archive = ZipArchive::new();
    archive.store("mimetype", MIMETYPE.as_bytes())?;
    archive.deflate("content.xml", content.as_bytes())?;
    archive.deflate("styles.xml", styles.as_bytes())?;
    archive.deflate("meta.xml", meta.as_bytes())?;
    archive.deflate("manifest.rdf", MANIFEST_RDF.as_bytes())?;
    archive.deflate("META-INF/manifest.xml", manifest.as_bytes())?;
    for image in &builder.images {
        archive.store(&format!("Pictures/{}", image.file_name), &image.bytes)?;
    }
    for formula in &builder.formulas {
        archive.deflate(
            &format!("Formula-{}/content.xml", formula.index),
            formula_content_xml(&formula.mathml).as_bytes(),
        )?;
        archive.deflate(
            &format!("Formula-{}/settings.xml", formula.index),
            formula_settings_xml(formula.text_mode).as_bytes(),
        )?;
    }
    archive.finish()
}

/// An embedded formula sub-object collected while rendering the body. Each becomes a `Formula-N/`
/// package member holding a Presentation MathML document.
struct Formula {
    /// The shared object index `N`, naming the `Formula-N/` directory.
    index: usize,
    /// The Presentation MathML `<math>` element written to `Formula-N/content.xml`.
    mathml: String,
    /// Whether the object is set in text mode: true for an inline formula sitting in a text line,
    /// false for a display formula standing in its own paragraph.
    text_mode: bool,
}

/// An embedded raster or vector image collected while rendering the body.
struct Image {
    /// The name under `Pictures/`, for example `0.png`.
    file_name: String,
    /// The MIME type, recorded in the manifest.
    mime: String,
    bytes: Vec<u8>,
}

/// A character-level decoration contributing font properties to a text run. Ordered so a set of
/// decorations always serializes its properties the same way.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Deco {
    Emph,
    Strong,
    Underline,
    SmallCaps,
    Strikeout,
    Superscript,
    Subscript,
}

impl Deco {
    /// The fixed named style that renders this decoration on its own, when one exists. Decorations
    /// without a named style, and every combination of decorations, use an automatic style instead.
    fn named_style(self) -> Option<&'static str> {
        match self {
            Deco::Emph => Some("Emphasis"),
            Deco::Strong => Some("Strong_20_Emphasis"),
            Deco::Strikeout => Some("Strikeout"),
            Deco::Superscript => Some("Superscript"),
            Deco::Subscript => Some("Subscript"),
            Deco::Underline | Deco::SmallCaps => None,
        }
    }

    /// The text-properties attributes this decoration contributes to an automatic style.
    fn properties(self) -> &'static str {
        match self {
            Deco::Emph => {
                "fo:font-style=\"italic\" style:font-style-asian=\"italic\" \
                 style:font-style-complex=\"italic\""
            }
            Deco::Strong => {
                "fo:font-weight=\"bold\" style:font-weight-asian=\"bold\" \
                 style:font-weight-complex=\"bold\""
            }
            Deco::Underline => {
                "style:text-underline-color=\"font-color\" style:text-underline-style=\"solid\" \
                 style:text-underline-width=\"auto\""
            }
            Deco::SmallCaps => "fo:font-variant=\"small-caps\"",
            Deco::Strikeout => "style:text-line-through-style=\"solid\"",
            Deco::Superscript => "style:text-position=\"super 58%\"",
            Deco::Subscript => "style:text-position=\"sub 58%\"",
        }
    }
}

/// A horizontal alignment that a table cell realizes through an automatic paragraph style.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AlignKind {
    Center,
    Right,
}

/// The identity of an automatic table-cell paragraph style: the base style it inherits (a heading
/// cell when `header`, a contents cell otherwise) folded with the alignment it imposes. Two cells
/// asking for the same pair share one registered style.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ParaStyleKey {
    header: bool,
    align: AlignKind,
}

/// Threads the render state: the growing body, the automatic styles discovered along the way, the
/// collected images, and the per-document counters.
#[allow(clippy::struct_excessive_bools)]
struct Builder<'a> {
    options: &'a WriterOptions,
    body: String,
    /// Whether the next flowing paragraph is the first of its section (styled distinctly).
    first_para: bool,
    /// Whether empty paragraphs are preserved rather than dropped.
    keep_empty: bool,
    text_styles: Vec<Vec<Deco>>,
    para_styles: Vec<ParaStyleKey>,
    list_styles: String,
    table_styles: String,
    note_id: usize,
    /// A shared counter over every drawing object (image or formula), advanced by one for an image
    /// and by two for a formula, that names the `Pictures/` and `Formula-N/` members.
    object_index: usize,
    /// A per-image ordinal, separate from the object index, that names each frame `imgN`.
    image_ordinal: usize,
    list_auto_index: usize,
    table_index: usize,
    images: Vec<Image>,
    formulas: Vec<Formula>,
}

impl<'a> Builder<'a> {
    fn new(options: &'a WriterOptions) -> Self {
        Builder {
            options,
            body: String::new(),
            first_para: false,
            keep_empty: options.extensions.contains(Extension::EmptyParagraphs),
            text_styles: Vec::new(),
            para_styles: Vec::new(),
            list_styles: String::new(),
            table_styles: String::new(),
            note_id: 0,
            object_index: 0,
            image_ordinal: 0,
            list_auto_index: 0,
            table_index: 0,
            images: Vec::new(),
            formulas: Vec::new(),
        }
    }

    fn document(&mut self, document: &Document) {
        self.title_block(&document.meta);
        self.abstract_block(&document.meta);
        if self.options.toc {
            self.table_of_contents();
        }
        self.render_blocks(&document.blocks, None);
    }

    fn finish_content(&self) -> String {
        let mut out = String::with_capacity(self.body.len() + 4096);
        out.push_str(DECL);
        out.push_str("<office:document-content");
        out.push_str(NS);
        out.push_str(" office:version=\"1.3\">");
        out.push_str("<office:scripts />");
        out.push_str("<office:font-face-decls>");
        out.push_str(
            "<style:font-face style:name=\"Courier New\" style:font-family-generic=\"modern\" \
             style:font-pitch=\"fixed\" svg:font-family=\"'Courier New'\" />",
        );
        out.push_str("</office:font-face-decls>");

        out.push_str("<office:automatic-styles>");
        for (index, key) in self.text_styles.iter().enumerate() {
            out.push_str(&deco_style_xml(index + 1, key));
        }
        for (index, key) in self.para_styles.iter().enumerate() {
            out.push_str(&para_style_xml(index + 1, key.header, key.align));
        }
        if self.table_index != 0 {
            out.push_str(CELL_STYLES);
        }
        out.push_str(&self.table_styles);
        out.push_str(&self.list_styles);
        if !self.formulas.is_empty() {
            out.push_str(FORMULA_STYLES);
        }
        out.push_str("</office:automatic-styles>");

        out.push_str("<office:body><office:text>");
        out.push_str(&self.body);
        out.push_str("</office:text></office:body></office:document-content>");
        out
    }
}

/// The document's keywords as one comma-separated string: a metadata list joins its items with a
/// comma, and a single value contributes its own flattened text.
fn meta_keywords(meta: &BTreeMap<Text, MetaValue>) -> String {
    match meta.get("keywords") {
        Some(MetaValue::MetaList(items)) => {
            items.iter().map(plain_meta).collect::<Vec<_>>().join(", ")
        }
        Some(value) => plain_meta(value),
        None => String::new(),
    }
}

/// Splits a language tag into its `fo:language` and `fo:country` parts: the primary subtag becomes
/// the lowercase language, and the first region subtag (a two-letter or three-digit code) becomes
/// the uppercase country. Script and other subtags carry no country and are skipped.
fn language_country(lang: &str) -> (String, String) {
    let mut subtags = lang.split('-');
    let language = subtags.next().unwrap_or_default().to_ascii_lowercase();
    let country = subtags
        .find(|tag| is_region_subtag(tag))
        .map(str::to_ascii_uppercase)
        .unwrap_or_default();
    (language, country)
}

fn is_region_subtag(tag: &str) -> bool {
    (tag.len() == 2 && tag.bytes().all(|byte| byte.is_ascii_alphabetic()))
        || (tag.len() == 3 && tag.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tag_splits_into_language_and_region() {
        assert_eq!(language_country("de-DE"), ("de".into(), "DE".into()));
        // A bare primary subtag carries no region.
        assert_eq!(language_country("en"), ("en".into(), String::new()));
        assert_eq!(language_country("yue"), ("yue".into(), String::new()));
        // A three-digit UN M.49 code is a region.
        assert_eq!(language_country("es-419"), ("es".into(), "419".into()));
        // A four-letter script subtag is skipped; the region is taken from a later subtag.
        assert_eq!(language_country("de-Latn-DE"), ("de".into(), "DE".into()));
        // Casing is normalised: language lowercased, region uppercased.
        assert_eq!(language_country("EN-us"), ("en".into(), "US".into()));
    }

    #[test]
    fn keywords_join_a_metadata_list_with_commas() {
        let mut meta = BTreeMap::new();
        meta.insert(
            Text::from("keywords"),
            MetaValue::MetaList(vec![
                MetaValue::MetaString(Text::from("alpha")),
                MetaValue::MetaString(Text::from("beta")),
            ]),
        );
        assert_eq!(meta_keywords(&meta), "alpha, beta");
        // A single scalar contributes its own text; an absent field is empty.
        meta.insert(
            Text::from("keywords"),
            MetaValue::MetaString(Text::from("solo")),
        );
        assert_eq!(meta_keywords(&meta), "solo");
        assert_eq!(meta_keywords(&BTreeMap::new()), "");
    }
}
