//! Writer for the `OpenDocument` Text (ODT) package format.
//!
//! Renders a [`carta_ast::Document`] into a complete, deterministic ODT archive: a Zip container
//! holding the uncompressed `mimetype` marker, the package manifest, the document body
//! (`content.xml`), the stylesheet (`styles.xml`), the metadata (`meta.xml`), an RDF package
//! description, and any embedded images under `Pictures/`. The archive bytes are reproducible across
//! runs: the Zip layer stamps a fixed timestamp, maps are ordered, and the timestamps written into
//! the metadata derive from a fixed epoch rather than the wall clock.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Format, Inline,
    ListAttributes, ListNumberDelim, ListNumberStyle, MathType, MetaValue, QuoteType, Row, Table,
    Target, Text,
};
use carta_core::container::xml::{escape_attribute, escape_text, is_xml_char};
use carta_core::container::zip::ZipArchive;
use carta_core::media::{decode_data_uri, extension_for_mime, image_mime_for_extension};
use carta_core::{BytesWriter, Extension, Result, WriterOptions};

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

/// The XML declaration prefixing every part.
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

/// Assembles the complete ODT archive from the rendered parts.
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
    /// The raw file bytes.
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
    /// Centered content.
    Center,
    /// Trailing-edge (right) content.
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

    /// Renders the whole document body: the title block, an optional table of contents, then the
    /// document blocks.
    fn document(&mut self, document: &Document) {
        self.title_block(&document.meta);
        self.abstract_block(&document.meta);
        if self.options.toc {
            self.table_of_contents();
        }
        self.render_blocks(&document.blocks, None);
    }

    /// Assembles `content.xml` from the collected automatic styles and the rendered body.
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

    // --- block rendering -------------------------------------------------------------------------

    /// Renders a block sequence. `fixed` names a paragraph style imposed on the sequence's direct
    /// paragraphs (a list item, a cell, a blockquote); `None` selects the flowing body styles.
    fn render_blocks(&mut self, blocks: &[Block], fixed: Option<&str>) {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            for block in blocks {
                self.render_block(block, fixed);
            }
        });
    }

    fn render_block(&mut self, block: &Block, fixed: Option<&str>) {
        match block {
            Block::Para(inlines) | Block::Plain(inlines) => self.paragraph_like(inlines, fixed),
            Block::LineBlock(lines) => self.line_block_like(lines, fixed),
            Block::Div(attr, inner) => self.div(attr, inner, fixed),
            other => {
                self.block_other(other);
                self.first_para = true;
            }
        }
    }

    /// A paragraph node: styled with the flowing first/body distinction, or with an imposed style.
    ///
    /// The flowing style is chosen after the content is rendered, because a footnote whose body
    /// carries a block-level element (a code listing, say) marks the paragraph that anchors it.
    fn paragraph_like(&mut self, inlines: &[Inline], fixed: Option<&str>) {
        if inlines.iter().any(is_block_math) {
            self.paragraphs_split_on_block_math(inlines);
            return;
        }
        if let Some(style) = fixed {
            self.paragraph(style, inlines);
        } else if !(inlines_are_empty(inlines) && !self.keep_empty) {
            self.flowing_paragraph(inlines);
        }
    }

    /// A paragraph in the flowing body styles: `First_20_paragraph` when it opens a run of body text,
    /// `Text_20_body` otherwise. The style is read after the content renders, so a footnote whose body
    /// carries a block element can mark this as the paragraph that anchors it.
    fn flowing_paragraph(&mut self, inlines: &[Inline]) {
        let start = self.body.len();
        self.inlines(inlines);
        let style = self.flowing_style();
        self.body
            .insert_str(start, &format!("<text:p text:style-name=\"{style}\">"));
        self.body.push_str("</text:p>");
    }

    /// The flowing body style for the paragraph now opening: `First_20_paragraph` when it leads a run
    /// of body text, `Text_20_body` after. Reading it clears the first-of-run flag.
    fn flowing_style(&mut self) -> &'static str {
        let style = if self.first_para {
            "First_20_paragraph"
        } else {
            "Text_20_body"
        };
        self.first_para = false;
        style
    }

    /// Renders a paragraph whose inlines carry a display formula. A display formula stands alone in
    /// the text flow, so the run is broken at every formula boundary: the text on either side becomes
    /// its own paragraph — with the spacing that flanked the formula trimmed away, and an all-spacing
    /// remainder dropped — and each formula, or a cluster of formulas set directly against one
    /// another, its own. Every piece takes the flowing body styles regardless of the surrounding
    /// block style.
    fn paragraphs_split_on_block_math(&mut self, inlines: &[Inline]) {
        let mut index = 0;
        while index < inlines.len() {
            let math_run = inlines.get(index).is_some_and(is_block_math);
            let start = index;
            while inlines
                .get(index)
                .is_some_and(|inline| is_block_math(inline) == math_run)
            {
                index += 1;
            }
            let segment = inlines.get(start..index).unwrap_or_default();
            if math_run {
                self.flowing_paragraph(segment);
            } else {
                let text = trim_flanking_spacing(segment);
                if !inlines_are_empty(text) {
                    self.flowing_paragraph(text);
                }
            }
        }
    }

    /// A line block: one paragraph whose source line divisions become hard breaks.
    fn line_block_like(&mut self, lines: &[Vec<Inline>], fixed: Option<&str>) {
        let style = match fixed {
            Some(style) => style,
            None => self.flowing_style(),
        };
        self.body.push_str("<text:p text:style-name=\"");
        self.body.push_str(style);
        self.body.push_str("\">");
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                self.body.push_str("<text:line-break />");
            }
            self.inlines(line);
        }
        self.body.push_str("</text:p>");
    }

    /// A generic container: transparent to the flow, wrapped in a section when it carries an id, and
    /// re-styling its direct paragraphs when it carries a recognized `custom-style`.
    fn div(&mut self, attr: &Attr, inner: &[Block], fixed: Option<&str>) {
        let section = !attr.id.is_empty();
        if section {
            self.body.push_str("<text:section text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\">");
        }
        match custom_style(attr) {
            Some(name) => self.render_blocks(inner, Some(name)),
            None => self.render_blocks(inner, fixed),
        }
        if section {
            self.body.push_str("</text:section>");
        }
    }

    /// Renders a block that is neither a paragraph nor a transparent container.
    fn block_other(&mut self, block: &Block) {
        match block {
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines),
            Block::CodeBlock(_, text) => self.code_block(text),
            Block::BlockQuote(blocks) => {
                self.first_para = true;
                self.render_blocks(blocks, Some("Quotations"));
            }
            Block::BulletList(items) => self.bullet_list(items),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items),
            Block::DefinitionList(items) => self.definition_list(items),
            Block::HorizontalRule => {
                self.body
                    .push_str("<text:p text:style-name=\"Horizontal_20_Line\" />");
            }
            Block::Table(table) => self.table(table),
            Block::Figure(_, caption, content) => self.figure(caption, content),
            Block::RawBlock(format, text) => {
                if is_opendocument(format) {
                    self.body.push_str(text);
                }
            }
            // Paragraphs, line blocks and containers are handled before dispatch reaches here; the
            // arms below keep the match total without a panicking fallback, mirroring the flowing
            // dispatch so they cannot drift from it.
            Block::Para(inlines) | Block::Plain(inlines) => self.paragraph_like(inlines, None),
            Block::LineBlock(lines) => self.line_block_like(lines, None),
            Block::Div(attr, inner) => self.div(attr, inner, None),
        }
    }

    fn header(&mut self, level: i32, attr: &Attr, inlines: &[Inline]) {
        let style_level = level.clamp(1, 6);
        let outline = level.max(1);
        let _ = write!(
            self.body,
            "<text:h text:style-name=\"Heading_20_{style_level}\" text:outline-level=\"{outline}\">"
        );
        if !attr.id.is_empty() {
            self.body.push_str("<text:bookmark-start text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\" />");
        }
        self.inlines(inlines);
        if !attr.id.is_empty() {
            self.body.push_str("<text:bookmark-end text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\" />");
        }
        self.body.push_str("</text:h>");
    }

    fn code_block(&mut self, text: &str) {
        let trimmed = text.strip_suffix('\n').unwrap_or(text);
        for line in trimmed.split('\n') {
            self.body
                .push_str("<text:p text:style-name=\"Preformatted_20_Text\">");
            self.push_verbatim(line);
            self.body.push_str("</text:p>");
        }
    }

    fn bullet_list(&mut self, items: &[Vec<Block>]) {
        let item_style = if is_tight(items) {
            "List_20_Bullet_20_Tight"
        } else {
            "List_20_Bullet"
        };
        self.body
            .push_str("<text:list text:style-name=\"List_20_1\">");
        self.first_para = true;
        self.list_items(items, item_style);
        self.body.push_str("</text:list>");
    }

    fn ordered_list(&mut self, attrs: &ListAttributes, items: &[Vec<Block>]) {
        let item_style = if is_tight(items) {
            "List_20_Number_20_Tight"
        } else {
            "List_20_Number"
        };
        let use_builtin = matches!(
            attrs.style,
            ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal
        ) && matches!(
            attrs.delim,
            ListNumberDelim::DefaultDelim | ListNumberDelim::Period
        );
        let list_style = if use_builtin {
            "Numbering_20_1".to_string()
        } else {
            self.auto_list_style(attrs.style, attrs.delim)
        };
        self.body.push_str("<text:list text:style-name=\"");
        escape_attribute(&list_style, &mut self.body);
        if attrs.start != 1 {
            let _ = write!(self.body, "\" text:start-value=\"{}", attrs.start);
        }
        self.body.push_str("\">");
        self.first_para = true;
        self.list_items(items, item_style);
        self.body.push_str("</text:list>");
    }

    /// Emits the `<text:list-item>` children of a list.
    fn list_items(&mut self, items: &[Vec<Block>], item_style: &str) {
        for item in items {
            self.body.push_str("<text:list-item>");
            let checkpoint = self.body.len();
            self.render_blocks(item, Some(item_style));
            self.close_or_self_close(checkpoint, "</text:list-item>");
        }
    }

    /// Closes the element whose start tag ends just before `checkpoint`: when nothing was written
    /// since, the start tag's `>` is rewritten to `/>` for an empty element; otherwise `close_tag`
    /// ends it. `checkpoint` is the body length captured immediately after that `>`.
    fn close_or_self_close(&mut self, checkpoint: usize, close_tag: &str) {
        if self.body.len() == checkpoint {
            self.body.truncate(checkpoint - 1);
            self.body.push_str("/>");
        } else {
            self.body.push_str(close_tag);
        }
    }

    fn definition_list(&mut self, items: &[(Vec<Inline>, Vec<Vec<Block>>)]) {
        let tight = items
            .first()
            .and_then(|(_, defs)| defs.first())
            .map(Vec::as_slice)
            .is_none_or(leads_with_plain);
        let (term_style, def_style) = if tight {
            (
                "Definition_20_Term_20_Tight",
                "Definition_20_Definition_20_Tight",
            )
        } else {
            ("Definition_20_Term", "Definition_20_Definition")
        };
        for (term, defs) in items {
            self.paragraph(term_style, term);
            for def in defs {
                let checkpoint = self.body.len();
                self.render_blocks(def, Some(def_style));
                if self.body.len() == checkpoint {
                    self.paragraph(def_style, &[]);
                }
            }
        }
    }

    fn figure(&mut self, caption: &Caption, content: &[Block]) {
        self.render_blocks(content, Some("FigureWithCaption"));
        if !caption.long.is_empty() {
            self.caption_block("FigureCaption", &caption.long);
        }
    }

    /// Renders a caption's blocks as a single paragraph, joining its paragraph-level runs with hard
    /// line breaks so a multi-paragraph caption reads as one caption line.
    fn caption_block(&mut self, style: &str, blocks: &[Block]) {
        let mut runs: Vec<&[Inline]> = Vec::new();
        collect_caption_runs(blocks, &mut runs);
        self.body.push_str("<text:p text:style-name=\"");
        self.body.push_str(style);
        self.body.push_str("\">");
        for (index, inlines) in runs.iter().enumerate() {
            if index > 0 {
                self.body.push_str("<text:line-break />");
            }
            self.inlines(inlines);
        }
        self.body.push_str("</text:p>");
    }

    fn table(&mut self, table: &Table) {
        if !table.caption.long.is_empty() {
            self.caption_block("TableCaption", &table.caption.long);
        }
        self.table_index += 1;
        let n = self.table_index;
        let columns = table_column_count(table);
        let has_widths = table
            .col_specs
            .iter()
            .any(|spec| matches!(spec.width, ColWidth::ColWidth(_)));
        self.push_table_style(n, columns, &table.col_specs, has_widths);

        let _ = write!(
            self.body,
            "<table:table table:name=\"Table{n}\" table:style-name=\"Table{n}\">"
        );
        for column in 0..columns {
            let _ = write!(
                self.body,
                "<table:table-column table:style-name=\"Table{n}.{}\" />",
                column_letter(column)
            );
        }

        let mut covered = vec![0usize; columns];
        if !table.head.rows.is_empty() {
            self.body.push_str("<table:table-header-rows>");
            self.table_rows(
                &table.head.rows,
                true,
                columns,
                columns,
                &table.col_specs,
                &mut covered,
            );
            self.body.push_str("</table:table-header-rows>");
        }
        for section in &table.bodies {
            let head_columns = section.row_head_columns.max(0) as usize;
            self.table_rows(
                &section.head,
                false,
                columns,
                columns,
                &table.col_specs,
                &mut covered,
            );
            self.table_rows(
                &section.body,
                false,
                head_columns,
                columns,
                &table.col_specs,
                &mut covered,
            );
        }
        self.table_rows(
            &table.foot.rows,
            false,
            0,
            columns,
            &table.col_specs,
            &mut covered,
        );
        self.body.push_str("</table:table>");
    }

    /// Records the table's automatic table and column styles.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn push_table_style(&mut self, n: usize, columns: usize, specs: &[ColSpec], has_widths: bool) {
        let _ = write!(
            self.table_styles,
            "<style:style style:name=\"Table{n}\" style:family=\"table\">\
             <style:table-properties table:align=\"center\""
        );
        if has_widths {
            let total: f64 = specs
                .iter()
                .map(|spec| match spec.width {
                    ColWidth::ColWidth(fraction) => fraction,
                    ColWidth::ColWidthDefault => 0.0,
                })
                .sum();
            let percent = (total * 100.0).round() as i64;
            let _ = write!(self.table_styles, " style:rel-width=\"{percent}%\"");
        }
        self.table_styles.push_str(" /></style:style>");

        for column in 0..columns {
            let letter = column_letter(column);
            if has_widths {
                let fraction = match specs.get(column).map(|spec| &spec.width) {
                    Some(ColWidth::ColWidth(value)) => *value,
                    _ => 0.0,
                };
                let relative = (fraction * 65535.0) as u64;
                let _ = write!(
                    self.table_styles,
                    "<style:style style:name=\"Table{n}.{letter}\" style:family=\"table-column\">\
                     <style:table-column-properties style:rel-column-width=\"{relative}*\" /></style:style>"
                );
            } else {
                let _ = write!(
                    self.table_styles,
                    "<style:style style:name=\"Table{n}.{letter}\" style:family=\"table-column\" />"
                );
            }
        }
    }

    #[allow(clippy::cast_sign_loss)]
    fn table_rows(
        &mut self,
        rows: &[Row],
        cell_header: bool,
        head_columns: usize,
        columns: usize,
        specs: &[ColSpec],
        covered: &mut [usize],
    ) {
        for row in rows {
            self.body.push_str("<table:table-row>");
            let mut cells = row.cells.iter();
            let mut column = 0usize;
            while column < columns {
                if let Some(remaining) = covered.get_mut(column)
                    && *remaining > 0
                {
                    *remaining -= 1;
                    column += 1;
                    continue;
                }
                if let Some(cell) = cells.next() {
                    let span = (cell.col_span.max(1) as usize).min(columns - column);
                    let rows_spanned = cell.row_span.max(1) as usize;
                    let para_header = column < head_columns;
                    self.emit_cell(
                        cell,
                        cell_header,
                        para_header,
                        column,
                        specs,
                        span,
                        rows_spanned,
                    );
                    if rows_spanned > 1 {
                        for offset in 0..span {
                            if let Some(slot) = covered.get_mut(column + offset) {
                                *slot = rows_spanned - 1;
                            }
                        }
                    }
                    column += span;
                } else {
                    self.emit_empty_cell(cell_header);
                    column += 1;
                }
            }
            self.body.push_str("</table:table-row>");
        }
    }

    fn emit_cell(
        &mut self,
        cell: &Cell,
        cell_header: bool,
        para_header: bool,
        column: usize,
        specs: &[ColSpec],
        span: usize,
        rows_spanned: usize,
    ) {
        let para_style = self.cell_paragraph_style(para_header, column, specs, &cell.align);
        let cell_style = if cell_header {
            "TableHeaderRowCell"
        } else {
            "TableRowCell"
        };
        let _ = write!(
            self.body,
            "<table:table-cell table:style-name=\"{cell_style}\" office:value-type=\"string\""
        );
        if span > 1 {
            let _ = write!(self.body, " table:number-columns-spanned=\"{span}\"");
        }
        if rows_spanned > 1 {
            let _ = write!(self.body, " table:number-rows-spanned=\"{rows_spanned}\"");
        }
        self.body.push('>');
        let checkpoint = self.body.len();
        self.render_blocks(&cell.content, Some(&para_style));
        self.close_or_self_close(checkpoint, "</table:table-cell>");
    }

    fn emit_empty_cell(&mut self, cell_header: bool) {
        let cell_style = if cell_header {
            "TableHeaderRowCell"
        } else {
            "TableRowCell"
        };
        let _ = write!(
            self.body,
            "<table:table-cell table:style-name=\"{cell_style}\" office:value-type=\"string\" />"
        );
    }

    /// The paragraph style a cell's content takes, folding the cell's own alignment over the
    /// column's default and mapping centered and trailing alignment to automatic styles.
    fn cell_paragraph_style(
        &mut self,
        para_header: bool,
        column: usize,
        specs: &[ColSpec],
        cell_align: &Alignment,
    ) -> String {
        let column_default = Alignment::AlignDefault;
        let effective = match cell_align {
            Alignment::AlignDefault => specs
                .get(column)
                .map_or(&column_default, |spec| &spec.align),
            other => other,
        };
        let base = if para_header {
            "Table_20_Heading"
        } else {
            "Table_20_Contents"
        };
        match effective {
            Alignment::AlignCenter => self.align_style(para_header, AlignKind::Center),
            Alignment::AlignRight => self.align_style(para_header, AlignKind::Right),
            _ => base.to_string(),
        }
    }

    /// The name of the automatic paragraph style realizing an alignment over a table base style,
    /// registering it on first use.
    fn align_style(&mut self, para_header: bool, kind: AlignKind) -> String {
        let key = ParaStyleKey {
            header: para_header,
            align: kind,
        };
        if let Some(index) = self
            .para_styles
            .iter()
            .position(|existing| *existing == key)
        {
            return format!("P{}", index + 1);
        }
        self.para_styles.push(key);
        format!("P{}", self.para_styles.len())
    }

    /// The style name a text run carries under an active set of decorations: a fixed named style for
    /// a lone named decoration, otherwise an automatic style registered on first use.
    fn run_style(&mut self, decos: &[Deco]) -> String {
        let mut key = decos.to_vec();
        key.sort_unstable();
        key.dedup();
        if let [only] = key.as_slice()
            && let Some(named) = only.named_style()
        {
            return named.to_string();
        }
        if let Some(index) = self
            .text_styles
            .iter()
            .position(|existing| *existing == key)
        {
            return format!("T{}", index + 1);
        }
        self.text_styles.push(key);
        format!("T{}", self.text_styles.len())
    }

    /// Builds and records an automatic numbered-list style for a non-default numbering, returning
    /// its name.
    fn auto_list_style(&mut self, style: ListNumberStyle, delim: ListNumberDelim) -> String {
        self.list_auto_index += 1;
        let name = format!("L{}", self.list_auto_index);
        let format = num_format(style);
        let (prefix, suffix) = delim_fixes(delim);
        let mut out = format!("<text:list-style style:name=\"{name}\">");
        for level in 1..=10 {
            let space = format!("{:.4}in", f64::from(level - 1) * 0.1972);
            let _ = write!(
                out,
                "<text:list-level-style-number text:level=\"{level}\" \
                 text:style-name=\"Numbering_20_Symbols\" style:num-format=\"{format}\""
            );
            if let Some(prefix) = prefix {
                let _ = write!(out, " style:num-prefix=\"{prefix}\"");
            }
            let _ = write!(
                out,
                " style:num-suffix=\"{suffix}\">\
                 <style:list-level-properties text:space-before=\"{space}\" \
                 text:min-label-width=\"0.1965in\" text:min-label-distance=\"0.1in\" />\
                 </text:list-level-style-number>"
            );
        }
        out.push_str("</text:list-style>");
        self.list_styles.push_str(&out);
        name
    }

    /// The title, subtitle, author, and date paragraphs standing at the head of the body.
    fn title_block(&mut self, meta: &BTreeMap<Text, MetaValue>) {
        if let Some(inlines) = meta_inlines(meta, "title") {
            self.paragraph("Title", &inlines);
        }
        if let Some(inlines) = meta_inlines(meta, "subtitle") {
            self.paragraph("Subtitle", &inlines);
        }
        for author in meta_authors(meta) {
            self.paragraph("Author", &author);
        }
        if let Some(inlines) = meta_inlines(meta, "date") {
            self.paragraph("Date", &inlines);
        }
    }

    /// The abstract, a bare run standing after the title block, its paragraphs separated by line
    /// breaks. The run is a text node directly in the body, so the surrounding newlines are part of
    /// its adjacent text and set it off on its own line.
    fn abstract_block(&mut self, meta: &BTreeMap<Text, MetaValue>) {
        let Some(value) = meta.get("abstract") else {
            return;
        };
        let paragraphs = abstract_paragraphs(value);
        if paragraphs.is_empty() {
            return;
        }
        self.body.push('\n');
        for (index, inlines) in paragraphs.iter().enumerate() {
            if index > 0 {
                self.body.push_str("<text:line-break />");
            }
            self.inlines(inlines);
        }
        self.body.push('\n');
    }

    /// A table-of-contents field whose entries the reader regenerates on open.
    fn table_of_contents(&mut self) {
        let depth = self.options.toc_depth.unwrap_or(3);
        self.body
            .push_str("<text:table-of-content text:name=\"Table of Contents1\">");
        let _ = write!(
            self.body,
            "<text:table-of-content-source text:outline-level=\"{depth}\">"
        );
        self.body.push_str(
            "<text:index-title-template text:style-name=\"Contents_20_Heading\"></text:index-title-template>",
        );
        for level in 1..=10 {
            let _ = write!(
                self.body,
                "<text:table-of-content-entry-template text:outline-level=\"{level}\" \
                 text:style-name=\"Contents_20_{level}\">\
                 <text:index-entry-link-start text:style-name=\"Internet_20_link\" />\
                 <text:index-entry-chapter />\
                 <text:index-entry-text />\
                 <text:index-entry-link-end />\
                 <text:index-entry-tab-stop style:type=\"right\" style:leader-char=\".\" />\
                 <text:index-entry-link-start text:style-name=\"Internet_20_link\" />\
                 <text:index-entry-page-number />\
                 <text:index-entry-link-end />\
                 </text:table-of-content-entry-template>"
            );
        }
        self.body.push_str("</text:table-of-content-source>");
        self.body.push_str("</text:table-of-content>");
    }

    // --- inline rendering ------------------------------------------------------------------------

    /// Entry point for rendering an inline sequence, with no decoration active.
    fn inlines(&mut self, inlines: &[Inline]) {
        self.walk(inlines, &[]);
    }

    /// Renders an inline sequence under a set of active decorations. Plain content accumulates into a
    /// run emitted as a single styled span; a formatting node extends the decoration set over its
    /// content; a structural node (link, span, note, …) breaks the run and is emitted in place,
    /// carrying the active decorations into any content of its own.
    fn walk<'i>(&mut self, inlines: &'i [Inline], decos: &[Deco]) {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            let mut run: Vec<&'i Inline> = Vec::new();
            for inline in inlines {
                match inline {
                    Inline::Str(_)
                    | Inline::Space
                    | Inline::SoftBreak
                    | Inline::LineBreak
                    | Inline::Math(..) => run.push(inline),
                    Inline::RawInline(format, _) if is_opendocument(format) => run.push(inline),
                    Inline::RawInline(..) => {}
                    Inline::Emph(inner) => self.nested(&mut run, decos, Deco::Emph, inner),
                    Inline::Strong(inner) => self.nested(&mut run, decos, Deco::Strong, inner),
                    Inline::Underline(inner) => {
                        self.nested(&mut run, decos, Deco::Underline, inner)
                    }
                    Inline::SmallCaps(inner) => {
                        self.nested(&mut run, decos, Deco::SmallCaps, inner);
                    }
                    Inline::Strikeout(inner) => {
                        self.nested(&mut run, decos, Deco::Strikeout, inner);
                    }
                    Inline::Superscript(inner) => {
                        self.nested(&mut run, decos, Deco::Superscript, inner);
                    }
                    Inline::Subscript(inner) => {
                        self.nested(&mut run, decos, Deco::Subscript, inner);
                    }
                    Inline::Quoted(kind, inner) => {
                        self.flush_run(&mut run, decos);
                        let (open, close) = match kind {
                            QuoteType::DoubleQuote => ('\u{201C}', '\u{201D}'),
                            QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
                        };
                        self.body.push(open);
                        self.walk(inner, decos);
                        self.body.push(close);
                    }
                    Inline::Code(_, text) => {
                        self.flush_run(&mut run, decos);
                        self.body
                            .push_str("<text:span text:style-name=\"Source_20_Text\">");
                        self.push_verbatim(text);
                        self.body.push_str("</text:span>");
                    }
                    Inline::Link(_, text, target) => {
                        self.flush_run(&mut run, decos);
                        self.link(text, target, decos);
                    }
                    Inline::Image(attr, alt, target) => {
                        self.flush_run(&mut run, decos);
                        self.image(attr, alt, target);
                    }
                    Inline::Note(blocks) => {
                        self.flush_run(&mut run, decos);
                        self.note(blocks);
                    }
                    Inline::Span(attr, inner) => {
                        self.flush_run(&mut run, decos);
                        self.span(attr, inner, decos);
                    }
                    Inline::Cite(_, inner) => {
                        self.flush_run(&mut run, decos);
                        self.walk(inner, decos);
                    }
                }
            }
            self.flush_run(&mut run, decos);
        });
    }

    /// Flushes the pending run, then renders a formatting node's content with one more decoration.
    fn nested<'i>(
        &mut self,
        run: &mut Vec<&'i Inline>,
        decos: &[Deco],
        add: Deco,
        inner: &'i [Inline],
    ) {
        self.flush_run(run, decos);
        let mut extended = decos.to_vec();
        extended.push(add);
        self.walk(inner, &extended);
    }

    /// Emits the accumulated run wrapped in one styled span (bare, when no decoration is active) and
    /// clears it. A run that renders to nothing contributes nothing.
    fn flush_run(&mut self, run: &mut Vec<&Inline>, decos: &[Deco]) {
        if run.is_empty() {
            return;
        }
        let start = self.body.len();
        self.render_run_content(run);
        run.clear();
        if self.body.len() == start || decos.is_empty() {
            return;
        }
        let style = self.run_style(decos);
        self.body
            .insert_str(start, &format!("<text:span text:style-name=\"{style}\">"));
        self.body.push_str("</text:span>");
    }

    /// Renders the plain inlines gathered into a run, collapsing breaking spaces the way a flowing
    /// paragraph does.
    fn render_run_content(&mut self, run: &[&Inline]) {
        let mut pending_space = false;
        for inline in run {
            if matches!(inline, Inline::Space | Inline::SoftBreak) {
                pending_space = true;
                continue;
            }
            if pending_space {
                self.body.push(' ');
                pending_space = false;
            }
            match inline {
                Inline::Str(text) => self.push_verbatim(text),
                Inline::LineBreak => self.body.push_str("<text:line-break />"),
                Inline::Math(kind, tex) => self.math(kind, tex),
                Inline::RawInline(_, text) => self.body.push_str(text),
                _ => {}
            }
        }
        if pending_space {
            self.body.push(' ');
        }
    }

    /// A span: an `id` becomes a bookmark bracketing the content, and a `custom-style` wraps the
    /// content in a named text span. When both are present the bookmark encloses the styled span, so
    /// the anchor survives alongside the styling rather than being dropped for it.
    fn span(&mut self, attr: &Attr, inner: &[Inline], decos: &[Deco]) {
        let anchored = !attr.id.is_empty();
        if anchored {
            self.body.push_str("<text:bookmark-start text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\" />");
        }
        match custom_style(attr) {
            Some(name) => {
                self.body.push_str("<text:span text:style-name=\"");
                escape_attribute(name, &mut self.body);
                self.body.push_str("\">");
                self.walk(inner, decos);
                self.body.push_str("</text:span>");
            }
            None => self.walk(inner, decos),
        }
        if anchored {
            self.body.push_str("<text:bookmark-end text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\" />");
        }
    }

    fn link(&mut self, text: &[Inline], target: &Target, decos: &[Deco]) {
        self.body
            .push_str("<text:a xlink:type=\"simple\" xlink:href=\"");
        let url = target.url.as_str();
        match parent_prefix_index(url) {
            Some(at) => {
                escape_attribute(url.get(..at).unwrap_or_default(), &mut self.body);
                self.body.push_str("../");
                escape_attribute(url.get(at..).unwrap_or(url), &mut self.body);
            }
            None => escape_attribute(url, &mut self.body),
        }
        self.body.push_str("\" office:name=\"");
        escape_attribute(target.title.as_str(), &mut self.body);
        self.body.push_str("\">");
        self.body
            .push_str("<text:span text:style-name=\"Definition\">");
        self.walk(text, decos);
        self.body.push_str("</text:span></text:a>");
    }

    fn note(&mut self, blocks: &[Block]) {
        let id = self.note_id;
        self.note_id += 1;
        let _ = write!(
            self.body,
            "<text:note text:id=\"ftn{id}\" text:note-class=\"footnote\">\
             <text:note-citation>{}</text:note-citation><text:note-body>",
            id + 1
        );
        let checkpoint = self.body.len();
        self.render_blocks(blocks, Some("Footnote"));
        if self.body.len() == checkpoint {
            self.paragraph("Footnote", &[]);
        }
        self.body.push_str("</text:note-body></text:note>");
    }

    /// Renders inline or display math as an embedded formula object: a drawing frame that references
    /// a `Formula-N/` sub-object holding the Presentation MathML. The inline form anchors as a
    /// character in the text flow; the display form anchors to its paragraph. Math that cannot be
    /// parsed degrades to its verbatim source set as text.
    fn math(&mut self, kind: &MathType, tex: &str) {
        let display = matches!(kind, MathType::DisplayMath);
        let Some(mathml) = crate::math::to_mathml(tex, display) else {
            escape_text(tex, &mut self.body);
            return;
        };
        let index = self.object_index;
        let (style, anchor) = if display {
            ("fr2", "paragraph")
        } else {
            ("fr1", "as-char")
        };
        let _ = write!(
            self.body,
            "<draw:frame draw:style-name=\"{style}\" text:anchor-type=\"{anchor}\">\
             <draw:object xlink:href=\"Formula-{index}/\" xlink:type=\"simple\" \
             xlink:show=\"embed\" xlink:actuate=\"onLoad\" /></draw:frame>"
        );
        self.formulas.push(Formula {
            index,
            mathml,
            text_mode: !display,
        });
        self.object_index += 2;
    }

    fn image(&mut self, attr: &Attr, alt: &[Inline], target: &Target) {
        match self.resolve_image(target.url.as_str()) {
            Some((bytes, mime)) => {
                let extension = extension_for_mime(&mime).to_string();
                let index = self.object_index;
                let ordinal = self.image_ordinal;
                let file_name = format!("{index}.{extension}");
                let ((width, height), density) = image_metrics(&bytes);
                let size = image_size(attr, width, height, density);
                let _ = write!(
                    self.body,
                    "<draw:frame draw:name=\"img{}\"{size}>\
                     <draw:image xlink:href=\"Pictures/{file_name}\" xlink:type=\"simple\" \
                     xlink:show=\"embed\" xlink:actuate=\"onLoad\" /></draw:frame>",
                    ordinal + 1
                );
                self.images.push(Image {
                    file_name,
                    mime,
                    bytes,
                });
                self.object_index += 1;
                self.image_ordinal += 1;
            }
            None => {
                if !alt.is_empty() {
                    self.walk(alt, &[Deco::Emph]);
                }
            }
        }
    }

    /// Resolves an image reference to its bytes and MIME type, from the media bag or an inline data
    /// URI. Returns `None` when neither carries the resource, so the caller degrades to the alt text.
    fn resolve_image(&self, url: &str) -> Option<(Vec<u8>, String)> {
        if let Some(item) = self.options.media.get(url) {
            let mime = item
                .mime
                .clone()
                .or_else(|| image_mime_for_extension(url).map(str::to_string))
                .unwrap_or_else(|| "application/octet-stream".to_string());
            return Some((item.bytes.clone(), mime));
        }
        decode_data_uri(url)
    }

    // --- shared paragraph and text helpers -------------------------------------------------------

    fn paragraph(&mut self, style: &str, inlines: &[Inline]) {
        self.body.push_str("<text:p text:style-name=\"");
        escape_attribute(style, &mut self.body);
        self.body.push_str("\">");
        self.inlines(inlines);
        self.body.push_str("</text:p>");
    }

    /// Appends verbatim text, preserving space runs (as `<text:s>`) and tabs (as `<text:tab>`), so
    /// indentation and internal spacing survive the layout engine's whitespace collapsing.
    fn push_verbatim(&mut self, text: &str) {
        let mut chars = text.chars().peekable();
        let mut at_start = true;
        while let Some(ch) = chars.next() {
            match ch {
                ' ' => {
                    let mut run = 1usize;
                    while chars.peek() == Some(&' ') {
                        chars.next();
                        run += 1;
                    }
                    if at_start || run > 1 {
                        let _ = write!(self.body, "<text:s text:c=\"{run}\" />");
                    } else {
                        self.body.push(' ');
                    }
                }
                '\t' => self.body.push_str("<text:tab />"),
                '&' => self.body.push_str("&amp;"),
                '<' => self.body.push_str("&lt;"),
                '>' => self.body.push_str("&gt;"),
                other if is_xml_char(other) => self.body.push(other),
                _ => {}
            }
            at_start = false;
        }
    }
}

// --- image dimension probing ---------------------------------------------------------------------

/// The intrinsic pixel dimensions of an encoded image paired with the pixel density (horizontal,
/// vertical dots per inch) that maps those pixels to a physical size. Unrecognized data resolves to
/// a square default so a frame still gets a sensible size rather than a degenerate zero one.
fn image_metrics(bytes: &[u8]) -> ((u32, u32), (f64, f64)) {
    if let Some(dimensions) = png_dimensions(bytes) {
        return (dimensions, (72.0, 72.0));
    }
    if let Some(dimensions) = gif_dimensions(bytes) {
        return (dimensions, (72.0, 72.0));
    }
    if let Some(dimensions) = jpeg_dimensions(bytes) {
        return (dimensions, jpeg_density(bytes).unwrap_or((72.0, 72.0)));
    }
    if let Some(dimensions) = webp_dimensions(bytes) {
        return (dimensions, (96.0, 96.0));
    }
    if let Some(dimensions) = svg_dimensions(bytes) {
        return (dimensions, (96.0, 96.0));
    }
    ((100, 100), (72.0, 72.0))
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let signature: [u8; 8] = bytes.get(0..8)?.try_into().ok()?;
    if signature != SIGNATURE {
        return None;
    }
    Some((be_u32(bytes, 16)?, be_u32(bytes, 20)?))
}

fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let header: [u8; 4] = bytes.get(0..4)?.try_into().ok()?;
    if &header != b"GIF8" {
        return None;
    }
    Some((u32::from(le_u16(bytes, 6)?), u32::from(le_u16(bytes, 8)?)))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let start: [u8; 2] = bytes.get(0..2)?.try_into().ok()?;
    if start != [0xFF, 0xD8] {
        return None;
    }
    let mut pos = 2usize;
    for _ in 0..8192 {
        if *bytes.get(pos)? != 0xFF {
            return None;
        }
        let mut marker_pos = pos + 1;
        while *bytes.get(marker_pos)? == 0xFF {
            marker_pos += 1;
        }
        let marker = *bytes.get(marker_pos)?;
        let segment = marker_pos + 1;
        match marker {
            0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE
            | 0xCF => {
                // The frame header is length (2) then sample precision (1) before the dimensions.
                let height = be_u16(bytes, segment + 3)?;
                let width = be_u16(bytes, segment + 5)?;
                return Some((u32::from(width), u32::from(height)));
            }
            0xD8 | 0xD9 => return None,
            _ => {
                let length = be_u16(bytes, segment)? as usize;
                pos = segment + length;
            }
        }
    }
    None
}

/// The pixel dimensions a WebP file encodes, across the simple lossy (`VP8 `), simple lossless
/// (`VP8L`), and extended (`VP8X`) chunk layouts.
fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let riff: [u8; 4] = bytes.get(0..4)?.try_into().ok()?;
    let form: [u8; 4] = bytes.get(8..12)?.try_into().ok()?;
    if riff != *b"RIFF" || form != *b"WEBP" {
        return None;
    }
    match bytes.get(12..16)? {
        b"VP8X" => Some((le_u24(bytes, 24)? + 1, le_u24(bytes, 27)? + 1)),
        b"VP8L" => {
            if *bytes.get(20)? != 0x2F {
                return None;
            }
            let [b0, b1, b2, b3]: [u8; 4] = bytes.get(21..25)?.try_into().ok()?;
            let packed = u32::from(b0)
                | (u32::from(b1) << 8)
                | (u32::from(b2) << 16)
                | (u32::from(b3) << 24);
            Some(((packed & 0x3FFF) + 1, ((packed >> 14) & 0x3FFF) + 1))
        }
        b"VP8 " => {
            let start: [u8; 3] = bytes.get(23..26)?.try_into().ok()?;
            if start != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let width = u32::from(le_u16(bytes, 26)?) & 0x3FFF;
            let height = u32::from(le_u16(bytes, 28)?) & 0x3FFF;
            Some((width, height))
        }
        _ => None,
    }
}

/// The pixel dimensions declared by an SVG document's `<svg>` element: its `width` and `height`
/// attributes when both are present, otherwise the extents given by its `viewBox`.
fn svg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let text = core::str::from_utf8(bytes).ok()?;
    let opening = text.get(text.find("<svg")?..)?;
    let tag = opening.get(..opening.find('>')?)?;
    let width = svg_attribute(tag, "width").and_then(|value| svg_length_pixels(&value));
    let height = svg_attribute(tag, "height").and_then(|value| svg_length_pixels(&value));
    if let (Some(w), Some(h)) = (width, height) {
        return Some((w, h));
    }
    let view_box = svg_attribute(tag, "viewBox")?;
    let mut extents = view_box
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|part| !part.is_empty());
    let w = extents.nth(2)?;
    let h = extents.next()?;
    Some((svg_number_pixels(w)?, svg_number_pixels(h)?))
}

/// Extracts the value of a whole-token attribute from an element's opening tag, ignoring names that
/// merely occur as a suffix of another attribute (so `width` is not read out of `stroke-width`).
fn svg_attribute(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = tag.get(from..)?.find(name) {
        let index = from + offset;
        let before = index.checked_sub(1).and_then(|i| bytes.get(i)).copied();
        let after = bytes.get(index + name.len()).copied();
        let starts = before.map_or(true, |b| b.is_ascii_whitespace() || b == b'<');
        let ends = after.map_or(false, |b| b == b'=' || b.is_ascii_whitespace());
        if starts && ends {
            let tail = tag.get(index + name.len()..)?;
            let after_equals = tail.get(tail.find('=')? + 1..)?.trim_start();
            let quote = after_equals.chars().next()?;
            if quote == '"' || quote == '\'' {
                let value = after_equals.get(1..)?;
                return value.get(..value.find(quote)?).map(str::to_string);
            }
            return None;
        }
        from = index + name.len();
    }
    None
}

/// Converts a CSS length (a number with an optional unit) to pixels at 96 dots per inch; a bare
/// number is already in pixels.
fn svg_length_pixels(value: &str) -> Option<u32> {
    let text = value.trim();
    let units: [(&str, f64); 7] = [
        ("px", 1.0),
        ("pt", 96.0 / 72.0),
        ("pc", 16.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("em", 16.0),
    ];
    for (unit, factor) in units {
        if let Some(number) = text.strip_suffix(unit)
            && let Ok(measure) = number.trim().parse::<f64>()
        {
            return Some((measure * factor).round() as u32);
        }
    }
    text.parse::<f64>()
        .ok()
        .map(|measure| measure.round() as u32)
}

/// A `viewBox` extent is expressed in unit-less user-space coordinates, which map one to one to
/// pixels.
fn svg_number_pixels(value: &str) -> Option<u32> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .map(|measure| measure.round() as u32)
}

fn jpeg_density(bytes: &[u8]) -> Option<(f64, f64)> {
    let start: [u8; 2] = bytes.get(0..2)?.try_into().ok()?;
    if start != [0xFF, 0xD8] {
        return None;
    }
    let mut pos = 2usize;
    for _ in 0..8192 {
        if *bytes.get(pos)? != 0xFF {
            return None;
        }
        let mut marker_pos = pos + 1;
        while *bytes.get(marker_pos)? == 0xFF {
            marker_pos += 1;
        }
        let marker = *bytes.get(marker_pos)?;
        let segment = marker_pos + 1;
        let length = be_u16(bytes, segment)? as usize;
        match marker {
            // The JFIF application header records a density in dots per inch or per centimetre.
            0xE0 if bytes.get(segment + 2..segment + 7)? == b"JFIF\0" => {
                let units = *bytes.get(segment + 9)?;
                let x = f64::from(be_u16(bytes, segment + 10)?);
                let y = f64::from(be_u16(bytes, segment + 12)?);
                if x > 0.0 && y > 0.0 {
                    return match units {
                        1 => Some((x, y)),
                        2 => Some((x * 2.54, y * 2.54)),
                        _ => None,
                    };
                }
                pos = segment + length;
            }
            0xD8 | 0xD9 | 0xDA => return None,
            _ => pos = segment + length,
        }
    }
    None
}

fn be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(slice))
}

fn be_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(u16::from_be_bytes(slice))
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(slice))
}

fn le_u24(bytes: &[u8], offset: usize) -> Option<u32> {
    let [a, b, c]: [u8; 3] = bytes.get(offset..offset + 3)?.try_into().ok()?;
    Some(u32::from(a) | (u32::from(b) << 8) | (u32::from(c) << 16))
}

// --- image sizing --------------------------------------------------------------------------------

/// A parsed image dimension: a percentage of the available width, or an absolute length already
/// resolved to inches.
enum Dimension {
    Percent(f64),
    Inches(f64),
}

/// The size attributes for an image frame. An explicit `width`/`height` attribute is resolved to
/// inches; a single explicit dimension carries the other along the image's aspect ratio. Absent
/// both, the frame takes the intrinsic pixel size mapped to points through the image's density.
fn image_size(attr: &Attr, width: u32, height: u32, density: (f64, f64)) -> String {
    let (dpi_x, dpi_y) = density;
    let requested_width = attr_value(attr, "width").and_then(parse_dimension);
    let requested_height = attr_value(attr, "height").and_then(parse_dimension);
    let natural_width = format!("{}pt", show_number(f64::from(width) * 72.0 / dpi_x));
    let natural_height = format!("{}pt", show_number(f64::from(height) * 72.0 / dpi_y));

    if let Some(Dimension::Percent(percent)) = &requested_width {
        return format!(
            " style:rel-width=\"{percent:.1}%\" style:rel-height=\"scale\" \
             svg:width=\"{natural_width}\" svg:height=\"{natural_height}\""
        );
    }

    let width_inches = match requested_width {
        Some(Dimension::Inches(value)) => Some(value),
        _ => None,
    };
    let height_inches = match requested_height {
        Some(Dimension::Inches(value)) => Some(value),
        _ => None,
    };

    let (final_width, final_height) = match (width_inches, height_inches) {
        (Some(w), Some(h)) => (inches(show_inches(w)), inches(show_inches(h))),
        (Some(w), None) => {
            let h = if width > 0 {
                inches(show_number(w * (f64::from(height) / f64::from(width))))
            } else {
                natural_height
            };
            (inches(show_inches(w)), h)
        }
        (None, Some(h)) => {
            let w = if height > 0 {
                inches(show_number(h * (f64::from(width) / f64::from(height))))
            } else {
                natural_width
            };
            (w, inches(show_inches(h)))
        }
        (None, None) => (natural_width, natural_height),
    };
    format!(" svg:width=\"{final_width}\" svg:height=\"{final_height}\"")
}

fn inches(value: String) -> String {
    format!("{value}in")
}

/// Parses an image dimension attribute into a percentage or an absolute length in inches. A bare
/// number and `px` are pixels at 96 per inch; other units convert to inches by their fixed ratio.
fn parse_dimension(raw: &str) -> Option<Dimension> {
    let text = raw.trim();
    if let Some(number) = text.strip_suffix('%') {
        return number.trim().parse::<f64>().ok().map(Dimension::Percent);
    }
    let units: [(&str, f64); 7] = [
        ("in", 1.0),
        ("cm", 0.393_700_787_4),
        ("mm", 0.039_370_078_74),
        ("pt", 1.0 / 72.0),
        ("pc", 1.0 / 6.0),
        ("em", 0.171_875),
        ("px", 1.0 / 96.0),
    ];
    for (unit, factor) in units {
        if let Some(number) = text.strip_suffix(unit)
            && let Ok(value) = number.trim().parse::<f64>()
        {
            return Some(Dimension::Inches(value * factor));
        }
    }
    text.parse::<f64>()
        .ok()
        .map(|value| Dimension::Inches(value / 96.0))
}

/// Formats an explicitly requested length in inches: rounded to five decimals, with trailing zeros
/// and a trailing point trimmed.
fn show_inches(value: f64) -> String {
    let text = format!("{value:.5}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Formats a derived or intrinsic length at full precision, using the shortest decimal that round
/// trips: a plain decimal (always with a fractional digit) for magnitudes in `[0.1, 1e7)`, and
/// scientific notation otherwise.
fn show_number(value: f64) -> String {
    if value == 0.0 {
        return "0.0".to_string();
    }
    let negative = value.is_sign_negative();
    let plain = format!("{}", value.abs());
    let (integer_part, fraction_part) = plain.split_once('.').unwrap_or((plain.as_str(), ""));
    let mut digits = String::with_capacity(integer_part.len() + fraction_part.len());
    digits.push_str(integer_part);
    digits.push_str(fraction_part);
    let significant = digits.trim_start_matches('0');
    let leading_zeros = digits.len() - significant.len();
    let point = integer_part.len() as isize - leading_zeros as isize;
    let significant = significant.trim_end_matches('0');
    if significant.is_empty() {
        return "0.0".to_string();
    }
    let magnitude = value.abs();
    let body = if (0.1..1e7).contains(&magnitude) {
        format_fixed(significant, point)
    } else {
        format_scientific(significant, point - 1)
    };
    if negative { format!("-{body}") } else { body }
}

/// Renders significant digits in fixed-point form, `point` giving how many digits precede the
/// decimal separator.
fn format_fixed(significant: &str, point: isize) -> String {
    if point <= 0 {
        let zeros = "0".repeat(point.unsigned_abs());
        return format!("0.{zeros}{significant}");
    }
    let point = point.unsigned_abs();
    if significant.len() <= point {
        let padding = "0".repeat(point - significant.len());
        return format!("{significant}{padding}.0");
    }
    let head: String = significant.chars().take(point).collect();
    let tail: String = significant.chars().skip(point).collect();
    format!("{head}.{tail}")
}

/// Renders significant digits in scientific form `d.ddde±p`, with a single leading digit.
fn format_scientific(significant: &str, exponent: isize) -> String {
    let mut chars = significant.chars();
    let lead = chars.next().unwrap_or('0');
    let rest: String = chars.collect();
    let mantissa = if rest.is_empty() {
        format!("{lead}.0")
    } else {
        format!("{lead}.{rest}")
    };
    format!("{mantissa}e{exponent}")
}

// --- style fragment builders ---------------------------------------------------------------------

fn deco_style_xml(index: usize, key: &[Deco]) -> String {
    // Superscript and Subscript both realize as a single text-position property, so a run that
    // carries both (nested in either order) must not repeat the attribute; the superscript position
    // is the one kept.
    let drops_subscript = key.contains(&Deco::Superscript) && key.contains(&Deco::Subscript);
    let mut properties = String::new();
    for deco in key {
        if drops_subscript && *deco == Deco::Subscript {
            continue;
        }
        if !properties.is_empty() {
            properties.push(' ');
        }
        properties.push_str(deco.properties());
    }
    format!(
        "<style:style style:name=\"T{index}\" style:family=\"text\">\
         <style:text-properties {properties} /></style:style>"
    )
}

fn para_style_xml(index: usize, header: bool, kind: AlignKind) -> String {
    let parent = if header {
        "Table_20_Heading"
    } else {
        "Table_20_Contents"
    };
    let align = match kind {
        AlignKind::Center => "center",
        AlignKind::Right => "end",
    };
    format!(
        "<style:style style:name=\"P{index}\" style:family=\"paragraph\" \
         style:parent-style-name=\"{parent}\">\
         <style:paragraph-properties fo:text-align=\"{align}\" \
         style:justify-single-word=\"false\" /></style:style>"
    )
}

fn num_format(style: ListNumberStyle) -> &'static str {
    match style {
        ListNumberStyle::LowerRoman => "i",
        ListNumberStyle::UpperRoman => "I",
        ListNumberStyle::LowerAlpha => "a",
        ListNumberStyle::UpperAlpha => "A",
        _ => "1",
    }
}

fn delim_fixes(delim: ListNumberDelim) -> (Option<&'static str>, &'static str) {
    match delim {
        ListNumberDelim::OneParen => (None, ")"),
        ListNumberDelim::TwoParens => (Some("("), ")"),
        _ => (None, "."),
    }
}

// --- metadata and manifest -----------------------------------------------------------------------

/// The `meta.xml` part: the producer, the document's title and creators, and reproducible
/// timestamps.
fn meta_xml(meta: &BTreeMap<Text, MetaValue>) -> String {
    let stamp = iso_utc(DATE_EPOCH);
    let escaped = |key: &str| {
        let mut out = String::new();
        escape_text(&meta_text(meta, key), &mut out);
        out
    };
    let title = escaped("title");
    let description = escaped("description");
    let subject = escaped("subject");
    let mut keywords = String::new();
    escape_text(&meta_keywords(meta), &mut keywords);
    let language = escaped("lang");
    let creator = escaped("author");

    let mut out = String::new();
    out.push_str(DECL);
    out.push_str("<office:document-meta");
    out.push_str(NS);
    out.push_str(" office:version=\"1.3\"><office:meta>");
    out.push_str("<meta:generator>carta</meta:generator>");
    let _ = write!(out, "<dc:title>{title}</dc:title>");
    let _ = write!(out, "<dc:description>{description}</dc:description>");
    let _ = write!(out, "<dc:subject>{subject}</dc:subject>");
    let _ = write!(out, "<meta:keyword>{keywords}</meta:keyword>");
    // The document language is recorded only when the metadata names one.
    if !language.is_empty() {
        let _ = write!(out, "<dc:language>{language}</dc:language>");
    }
    let _ = write!(
        out,
        "<meta:initial-creator>{creator}</meta:initial-creator>"
    );
    let _ = write!(out, "<dc:creator>{creator}</dc:creator>");
    let _ = write!(out, "<meta:creation-date>{stamp}</meta:creation-date>");
    let _ = write!(out, "<dc:date>{stamp}</dc:date>");
    // Every field the standard elements above do not claim is preserved as a custom property, keyed
    // by its metadata name. The map iterates in key order, so the properties come out sorted.
    for (key, value) in meta {
        if is_standard_meta(key.as_str()) {
            continue;
        }
        let mut name = String::new();
        escape_attribute(key.as_str(), &mut name);
        let mut text = String::new();
        escape_text(&user_defined_value(value), &mut text);
        let _ = write!(
            out,
            "<meta:user-defined meta:name=\"{name}\" meta:value-type=\"string\">{text}\
             </meta:user-defined>"
        );
    }
    out.push_str("</office:meta></office:document-meta>");
    out
}

/// Whether a metadata key is carried by a dedicated `meta.xml` element rather than a custom property.
fn is_standard_meta(key: &str) -> bool {
    matches!(
        key,
        "title" | "description" | "subject" | "keywords" | "lang" | "author"
    )
}

/// Flattens a custom metadata field to the plain text of a user-defined property: scalar fields
/// yield their text, a boolean its capitalized name, and structured fields (lists, maps) yield
/// nothing.
fn user_defined_value(value: &MetaValue) -> String {
    match value {
        MetaValue::MetaString(text) => text.to_string(),
        MetaValue::MetaInlines(inlines) => carta_ast::to_plain_text(inlines),
        MetaValue::MetaBlocks(blocks) => {
            carta_ast::to_plain_text(carta_ast::single_block_inlines(blocks))
        }
        MetaValue::MetaBool(flag) => if *flag { "True" } else { "False" }.to_string(),
        MetaValue::MetaList(_) | MetaValue::MetaMap(_) => String::new(),
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

/// The `Formula-N/content.xml` part wrapping a Presentation MathML document.
fn formula_content_xml(mathml: &str) -> String {
    let mut out = String::with_capacity(mathml.len() + DECL.len());
    out.push_str(DECL);
    out.push_str(mathml);
    out
}

/// The `META-INF/manifest.xml` part listing every package member.
fn manifest_xml(images: &[Image], formulas: &[Formula]) -> String {
    let mut out = String::new();
    out.push_str(DECL);
    out.push_str(
        "<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" \
         manifest:version=\"1.3\">",
    );
    let _ = write!(
        out,
        "<manifest:file-entry manifest:full-path=\"/\" manifest:version=\"1.3\" \
         manifest:media-type=\"{MIMETYPE}\" />"
    );
    for (path, media_type) in [
        ("content.xml", "application/xml"),
        ("styles.xml", "application/xml"),
        ("meta.xml", "application/xml"),
        ("manifest.rdf", "application/rdf+xml"),
    ] {
        let _ = write!(
            out,
            "<manifest:file-entry manifest:full-path=\"{path}\" \
             manifest:media-type=\"{media_type}\" />"
        );
    }
    for image in images {
        let mut media_type = String::new();
        escape_attribute(&image.mime, &mut media_type);
        let _ = write!(
            out,
            "<manifest:file-entry manifest:full-path=\"Pictures/{}\" \
             manifest:media-type=\"{media_type}\" />",
            image.file_name
        );
    }
    for formula in formulas {
        let _ = write!(
            out,
            "<manifest:file-entry \
             manifest:media-type=\"application/vnd.oasis.opendocument.formula\" \
             manifest:full-path=\"Formula-{index}/\" manifest:version=\"1.3\" />\
             <manifest:file-entry manifest:full-path=\"Formula-{index}/content.xml\" \
             manifest:media-type=\"text/xml\" />\
             <manifest:file-entry manifest:full-path=\"Formula-{index}/settings.xml\" \
             manifest:media-type=\"text/xml\" />",
            index = formula.index
        );
    }
    out.push_str("</manifest:manifest>");
    out
}

/// Flattens a metadata field to plain text, joining a list with `; `.
fn meta_text(meta: &BTreeMap<Text, MetaValue>, key: &str) -> String {
    meta.get(key).map(plain_meta).unwrap_or_default()
}

fn plain_meta(value: &MetaValue) -> String {
    match value {
        MetaValue::MetaString(text) => text.to_string(),
        MetaValue::MetaInlines(inlines) => carta_ast::to_plain_text(inlines),
        MetaValue::MetaBlocks(blocks) => {
            carta_ast::to_plain_text(carta_ast::single_block_inlines(blocks))
        }
        MetaValue::MetaBool(flag) => flag.to_string(),
        MetaValue::MetaList(items) => items.iter().map(plain_meta).collect::<Vec<_>>().join("; "),
        MetaValue::MetaMap(_) => String::new(),
    }
}

/// The inline content of a metadata field, or `None` when it is absent or empty.
fn meta_inlines(meta: &BTreeMap<Text, MetaValue>, key: &str) -> Option<Vec<Inline>> {
    let inlines = value_inlines(meta.get(key)?);
    if inlines.is_empty() {
        None
    } else {
        Some(inlines)
    }
}

fn value_inlines(value: &MetaValue) -> Vec<Inline> {
    match value {
        MetaValue::MetaInlines(inlines) => inlines.clone(),
        MetaValue::MetaString(text) => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![Inline::Str(text.clone())]
            }
        }
        MetaValue::MetaBlocks(blocks) => carta_ast::single_block_inlines(blocks).to_vec(),
        MetaValue::MetaList(items) => {
            let mut out = Vec::new();
            for item in items {
                if !out.is_empty() {
                    out.push(Inline::Space);
                }
                out.extend(value_inlines(item));
            }
            out
        }
        _ => Vec::new(),
    }
}

/// The abstract's paragraphs, one inline sequence per `Para`/`Plain` block, or a single sequence for
/// an inline-valued field.
fn abstract_paragraphs(value: &MetaValue) -> Vec<Vec<Inline>> {
    if let MetaValue::MetaBlocks(blocks) = value {
        return blocks
            .iter()
            .filter_map(|block| match block {
                Block::Para(inlines) | Block::Plain(inlines) => Some(inlines.clone()),
                _ => None,
            })
            .collect();
    }
    let inlines = value_inlines(value);
    if inlines.is_empty() {
        Vec::new()
    } else {
        vec![inlines]
    }
}

/// The document's authors, one inline sequence per author.
fn meta_authors(meta: &BTreeMap<Text, MetaValue>) -> Vec<Vec<Inline>> {
    match meta.get("author") {
        Some(MetaValue::MetaList(items)) => items
            .iter()
            .map(value_inlines)
            .filter(|inlines| !inlines.is_empty())
            .collect(),
        Some(value) => {
            let inlines = value_inlines(value);
            if inlines.is_empty() {
                Vec::new()
            } else {
                vec![inlines]
            }
        }
        None => Vec::new(),
    }
}

/// Formats seconds since the Unix epoch as a W3C date-time in UTC (`YYYY-MM-DDThh:mm:ssZ`).
fn iso_utc(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let seconds = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Converts a count of days since 1970-01-01 to a `(year, month, day)` civil date.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

// --- small shared helpers ------------------------------------------------------------------------

/// Whether an inline sequence carries no visible content, so a flowing paragraph of it is dropped.
fn inlines_are_empty(inlines: &[Inline]) -> bool {
    inlines.iter().all(|inline| match inline {
        Inline::Space | Inline::SoftBreak => true,
        Inline::RawInline(format, _) => !is_opendocument(format),
        _ => false,
    })
}

/// Whether an inline is a display formula, which occupies a paragraph of its own.
fn is_block_math(inline: &Inline) -> bool {
    matches!(inline, Inline::Math(MathType::DisplayMath, _))
}

/// The inline slice with any leading and trailing inter-word spacing (spaces and soft breaks) removed,
/// so text lifted out from beside a display formula does not carry the gap that abutted the formula.
fn trim_flanking_spacing(inlines: &[Inline]) -> &[Inline] {
    let is_spacing = |inline: &&Inline| matches!(inline, Inline::Space | Inline::SoftBreak);
    let start = inlines.iter().take_while(is_spacing).count();
    let trailing = inlines.iter().rev().take_while(is_spacing).count();
    let end = inlines.len().saturating_sub(trailing);
    inlines.get(start..end).unwrap_or_default()
}

/// Whether a list renders tight (compact) rather than loose. A list is tight when the first block of
/// its first item is plain, flowing text; a leading paragraph or a leading block structure makes the
/// whole list loose.
fn is_tight(items: &[Vec<Block>]) -> bool {
    leads_with_plain(items.first().map(Vec::as_slice).unwrap_or_default())
}

/// Whether a block sequence begins with a `Plain`, the marker distinguishing a tight list from a
/// loose one. An empty sequence counts as tight.
fn leads_with_plain(blocks: &[Block]) -> bool {
    blocks
        .first()
        .is_none_or(|block| matches!(block, Block::Plain(_)))
}

/// Gathers the inline runs a caption contributes, in document order, descending through wrapper
/// blocks so a caption written as a `Div` still surfaces its text.
fn collect_caption_runs<'a>(blocks: &'a [Block], runs: &mut Vec<&'a [Inline]>) {
    for block in blocks {
        match block {
            Block::Para(inlines) | Block::Plain(inlines) => runs.push(inlines),
            Block::Div(_, children) | Block::BlockQuote(children) => {
                collect_caption_runs(children, runs);
            }
            _ => {}
        }
    }
}

/// The number of columns a table has: its column specs, or the widest row when it declares none.
fn table_column_count(table: &Table) -> usize {
    if !table.col_specs.is_empty() {
        return table.col_specs.len();
    }
    let row_width = |rows: &[Row]| rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
    let mut columns = row_width(&table.head.rows).max(row_width(&table.foot.rows));
    for section in &table.bodies {
        columns = columns
            .max(row_width(&section.head))
            .max(row_width(&section.body));
    }
    columns
}

/// The spreadsheet-style column label for a zero-based index (`A`, `B`, …, `Z`, `AA`, …).
#[allow(clippy::cast_possible_truncation)]
fn column_letter(mut index: usize) -> String {
    let mut label = String::new();
    loop {
        let remainder = index % 26;
        label.insert(0, char::from(b'A' + remainder as u8));
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    label
}

/// Whether a raw format targets this writer.
fn is_opendocument(format: &Format) -> bool {
    let name = format.0.as_str();
    name.eq_ignore_ascii_case("opendocument") || name.eq_ignore_ascii_case("odt")
}

/// The byte offset at which a link target takes a `../` step toward the package root, or `None` when
/// the target resolves without one. A relative reference resolves against the directory that holds
/// the content part, so its path component gains one `../`; the step is spliced in front of the path,
/// after any `//authority`, leaving scheme, authority, query, and fragment untouched. The step is
/// withheld from a target with a URI scheme (already absolute), from one whose path component is
/// empty (a bare query or fragment addresses the document itself), and from one carrying a character
/// no URI reference admits — a non-ASCII letter, a control byte, a stray backslash, a space, a
/// bracket, or a malformed percent escape — since that cannot be resolved as a path.
fn parent_prefix_index(url: &str) -> Option<usize> {
    if has_scheme(url) || !is_relative_reference(url) {
        return None;
    }
    let path_start = authority_end(url);
    let reference = url.split(['?', '#']).next().unwrap_or_default();
    (reference.len() > path_start).then_some(path_start)
}

/// The byte offset just past a `//authority`, or `0` when the reference has none. The authority runs
/// from the opening `//` to the first `/`, `?`, or `#`, or to the end of the reference.
fn authority_end(url: &str) -> usize {
    match url.strip_prefix("//") {
        Some(rest) => rest
            .find(['/', '?', '#'])
            .map_or(url.len(), |offset| 2 + offset),
        None => 0,
    }
}

/// Whether `url` is a well-formed relative reference that a `../` prefix can resolve.
///
/// Every byte must be admissible in the URI grammar: ASCII within `0x21..=0x7E`, none of the
/// characters the grammar excludes (space, `"`, `<`, `>`, `[`, `\`, `]`, `^`, `` ` ``, `{`, `|`,
/// `}`), and every `%` the start of a two-digit hexadecimal escape. The brackets `[` and `]` are
/// admitted only within a `//authority`, where they delimit an IP-literal host. The first path
/// segment — the run before the first `/`, `?`, or `#` — additionally admits no colon, since a colon
/// there would parse as a scheme delimiter rather than as part of the path.
fn is_relative_reference(url: &str) -> bool {
    let authority = authority_end(url);
    let bytes = url.as_bytes();
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'%' => {
                if !bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                    || !bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit)
                {
                    return false;
                }
                index += 3;
            }
            b'[' | b']' if (2..authority).contains(&index) => index += 1,
            b' ' | b'"' | b'<' | b'>' | b'[' | b'\\' | b']' | b'^' | b'`' | b'{' | b'|' | b'}' => {
                return false;
            }
            0x21..=0x7E => index += 1,
            _ => return false,
        }
    }
    let first_segment = url.split(['/', '?', '#']).next().unwrap_or_default();
    !first_segment.contains(':')
}

/// Whether a URL opens with a `scheme:` prefix — a non-empty run of scheme characters before a colon.
fn has_scheme(url: &str) -> bool {
    match url.split_once(':') {
        Some((scheme, _)) => {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+'))
        }
        None => false,
    }
}

/// The value of a named attribute, if present.
fn attr_value<'a>(attr: &'a Attr, key: &str) -> Option<&'a str> {
    attr.attributes
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, value)| value.as_str())
}

/// The non-empty `custom-style` attribute value, if any.
fn custom_style(attr: &Attr) -> Option<&str> {
    attr_value(attr, "custom-style").filter(|value| !value.is_empty())
}

// --- stylesheet ----------------------------------------------------------------------------------

/// Builds the `styles.xml` part: the named paragraph, text, and list styles the body references,
/// plus the page layout and its master page.
fn styles_xml(meta: &BTreeMap<Text, MetaValue>) -> String {
    let lang = meta_text(meta, "lang");
    let (language, country) = if lang.is_empty() {
        ("en".to_string(), "US".to_string())
    } else {
        language_country(&lang)
    };

    let mut out = String::with_capacity(16 * 1024);
    out.push_str(DECL);
    out.push_str("<office:document-styles");
    out.push_str(NS);
    out.push_str(" office:version=\"1.3\">");

    out.push_str("<office:font-face-decls>");
    out.push_str(
        "<style:font-face style:name=\"Courier New\" style:font-family-generic=\"modern\" \
         style:font-pitch=\"fixed\" svg:font-family=\"'Courier New'\" />\
         <style:font-face style:name=\"Times New Roman\" style:font-family-generic=\"roman\" \
         style:font-pitch=\"variable\" svg:font-family=\"'Times New Roman'\" />\
         <style:font-face style:name=\"Arial\" style:font-family-generic=\"swiss\" \
         style:font-pitch=\"variable\" svg:font-family=\"Arial\" />",
    );
    out.push_str("</office:font-face-decls>");

    out.push_str("<office:styles>");
    push_named_styles(&mut out, &language, &country);
    out.push_str("</office:styles>");

    out.push_str(
        "<office:automatic-styles>\
         <style:page-layout style:name=\"Mpm1\">\
         <style:page-layout-properties fo:page-width=\"8.5in\" fo:page-height=\"11in\" \
         fo:margin-top=\"1in\" fo:margin-bottom=\"1in\" fo:margin-left=\"1in\" \
         fo:margin-right=\"1in\" style:print-orientation=\"portrait\" />\
         </style:page-layout>\
         </office:automatic-styles>",
    );

    out.push_str(
        "<office:master-styles>\
         <style:master-page style:name=\"Standard\" style:page-layout-name=\"Mpm1\" />\
         </office:master-styles>",
    );

    out.push_str("</office:document-styles>");
    out
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

/// A region subtag is either a two-letter alphabetic code or a three-digit numeric code.
fn is_region_subtag(tag: &str) -> bool {
    (tag.len() == 2 && tag.bytes().all(|byte| byte.is_ascii_alphabetic()))
        || (tag.len() == 3 && tag.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Emits every named style the writer references, in the order the schema expects them. The default
/// paragraph style records the document language, which the surrounding builder derives from the
/// metadata.
#[allow(clippy::too_many_lines)]
fn push_named_styles(out: &mut String, language: &str, country: &str) {
    let mut language_attr = String::new();
    escape_attribute(language, &mut language_attr);
    let mut country_attr = String::new();
    escape_attribute(country, &mut country_attr);
    let _ = write!(
        out,
        "<style:default-style style:family=\"paragraph\">\
         <style:paragraph-properties fo:hyphenation-ladder-count=\"no-limit\" \
         style:line-break=\"strict\" style:tab-stop-distance=\"0.5in\" />\
         <style:text-properties style:font-name=\"Times New Roman\" fo:font-size=\"12pt\" \
         fo:language=\"{language_attr}\" fo:country=\"{country_attr}\" /></style:default-style>"
    );

    push_paragraph_style(
        out,
        "Standard",
        None,
        "<style:text-properties style:font-name=\"Times New Roman\" fo:font-size=\"12pt\" />",
    );
    push_paragraph_style(
        out,
        "Text_20_body",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-top=\"0in\" fo:margin-bottom=\"0.0835in\" \
         fo:line-height=\"115%\" />",
    );
    push_paragraph_style(out, "First_20_paragraph", Some("Text_20_body"), "");
    push_paragraph_style(
        out,
        "Heading",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-top=\"0.1665in\" fo:margin-bottom=\"0.0835in\" \
         fo:keep-with-next=\"always\" />\
         <style:text-properties style:font-name=\"Arial\" fo:font-size=\"14pt\" />",
    );
    for level in 1..=6 {
        let size = match level {
            1 => "18pt",
            2 => "16pt",
            3 => "14pt",
            4 => "12pt",
            5 => "11pt",
            _ => "10pt",
        };
        let _ = write!(
            out,
            "<style:style style:name=\"Heading_20_{level}\" style:family=\"paragraph\" \
             style:parent-style-name=\"Heading\" style:default-outline-level=\"{level}\">\
             <style:text-properties fo:font-size=\"{size}\" fo:font-weight=\"bold\" /></style:style>"
        );
    }
    push_paragraph_style(
        out,
        "Title",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />\
         <style:text-properties fo:font-size=\"28pt\" fo:font-weight=\"bold\" />",
    );
    push_paragraph_style(
        out,
        "Subtitle",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />\
         <style:text-properties fo:font-size=\"18pt\" />",
    );
    push_paragraph_style(
        out,
        "Author",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />",
    );
    push_paragraph_style(
        out,
        "Date",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />",
    );
    push_paragraph_style(
        out,
        "Quotations",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-left=\"0.4in\" fo:margin-right=\"0.4in\" \
         fo:margin-top=\"0in\" fo:margin-bottom=\"0.0835in\" />",
    );
    push_paragraph_style(
        out,
        "Preformatted_20_Text",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-top=\"0in\" fo:margin-bottom=\"0in\" />\
         <style:text-properties style:font-name=\"Courier New\" fo:font-size=\"10pt\" />",
    );
    push_paragraph_style(
        out,
        "Horizontal_20_Line",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-top=\"0in\" fo:margin-bottom=\"0.0398in\" \
         style:border-line-width-bottom=\"0.0008in 0.0016in 0.0008in\" \
         fo:padding=\"0in\" fo:border-bottom=\"0.06pt double #808080\" />",
    );
    push_paragraph_style(
        out,
        "Footnote",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-left=\"0.2in\" fo:text-indent=\"-0.2in\" />\
         <style:text-properties fo:font-size=\"10pt\" />",
    );
    push_paragraph_style(out, "List", Some("Text_20_body"), "");
    for tight in [false, true] {
        let suffix = if tight { "_20_Tight" } else { "" };
        push_paragraph_style(out, &format!("List_20_Bullet{suffix}"), Some("List"), "");
        push_paragraph_style(out, &format!("List_20_Number{suffix}"), Some("List"), "");
        push_paragraph_style(
            out,
            &format!("Definition_20_Term{suffix}"),
            Some("Standard"),
            "<style:text-properties fo:font-weight=\"bold\" />",
        );
        push_paragraph_style(
            out,
            &format!("Definition_20_Definition{suffix}"),
            Some("Standard"),
            "<style:paragraph-properties fo:margin-left=\"0.4in\" />",
        );
    }
    push_paragraph_style(
        out,
        "Table_20_Heading",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />\
         <style:text-properties fo:font-weight=\"bold\" />",
    );
    push_paragraph_style(out, "Table_20_Contents", Some("Standard"), "");
    push_paragraph_style(
        out,
        "TableCaption",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" fo:margin-top=\"0.0835in\" \
         fo:margin-bottom=\"0.0835in\" />\
         <style:text-properties fo:font-style=\"italic\" fo:font-size=\"10pt\" />",
    );
    push_paragraph_style(
        out,
        "FigureWithCaption",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />",
    );
    push_paragraph_style(
        out,
        "FigureCaption",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />\
         <style:text-properties fo:font-style=\"italic\" fo:font-size=\"10pt\" />",
    );
    push_paragraph_style(
        out,
        "Contents_20_Heading",
        Some("Heading"),
        "<style:paragraph-properties fo:keep-with-next=\"always\" />\
         <style:text-properties fo:font-size=\"16pt\" fo:font-weight=\"bold\" />",
    );
    for level in 1..=10 {
        let indent = format!("{:.4}in", f64::from(level - 1) * 0.2);
        let _ = write!(
            out,
            "<style:style style:name=\"Contents_20_{level}\" style:family=\"paragraph\" \
             style:parent-style-name=\"Standard\">\
             <style:paragraph-properties fo:margin-left=\"{indent}\" fo:margin-right=\"0in\" \
             fo:text-indent=\"0in\"><style:tab-stops>\
             <style:tab-stop style:position=\"6.5in\" style:type=\"right\" \
             style:leader-style=\"dotted\" style:leader-text=\".\" /></style:tab-stops>\
             </style:paragraph-properties></style:style>"
        );
    }

    // Character styles.
    push_text_style(
        out,
        "Emphasis",
        "<style:text-properties fo:font-style=\"italic\" />",
    );
    push_text_style(
        out,
        "Strong_20_Emphasis",
        "<style:text-properties fo:font-weight=\"bold\" />",
    );
    push_text_style(
        out,
        "Strikeout",
        "<style:text-properties style:text-line-through-style=\"solid\" \
         style:text-line-through-type=\"single\" />",
    );
    push_text_style(
        out,
        "Superscript",
        "<style:text-properties style:text-position=\"super 58%\" />",
    );
    push_text_style(
        out,
        "Subscript",
        "<style:text-properties style:text-position=\"sub 58%\" />",
    );
    push_text_style(
        out,
        "Source_20_Text",
        "<style:text-properties style:font-name=\"Courier New\" />",
    );
    push_text_style(out, "Definition", "");
    push_text_style(
        out,
        "Internet_20_link",
        "<style:text-properties fo:color=\"#000080\" style:text-underline-color=\"font-color\" \
         style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" />",
    );
    push_text_style(out, "Numbering_20_Symbols", "");
    push_text_style(out, "Bullet_20_Symbols", "");

    // Named list styles.
    out.push_str("<text:list-style style:name=\"List_20_1\">");
    for level in 1..=10 {
        let space = format!("{:.4}in", f64::from(level) * 0.25);
        let _ = write!(
            out,
            "<text:list-level-style-bullet text:level=\"{level}\" \
             text:style-name=\"Bullet_20_Symbols\" text:bullet-char=\"\u{2022}\">\
             <style:list-level-properties text:space-before=\"{space}\" \
             text:min-label-width=\"0.25in\" /></text:list-level-style-bullet>"
        );
    }
    out.push_str("</text:list-style>");

    out.push_str("<text:list-style style:name=\"Numbering_20_1\">");
    for level in 1..=10 {
        let space = format!("{:.4}in", f64::from(level) * 0.1972);
        let _ = write!(
            out,
            "<text:list-level-style-number text:level=\"{level}\" \
             text:style-name=\"Numbering_20_Symbols\" style:num-format=\"1\" style:num-suffix=\".\">\
             <style:list-level-properties text:space-before=\"{space}\" \
             text:min-label-width=\"0.1965in\" /></text:list-level-style-number>"
        );
    }
    out.push_str("</text:list-style>");
}

fn push_paragraph_style(out: &mut String, name: &str, parent: Option<&str>, inner: &str) {
    out.push_str("<style:style style:name=\"");
    out.push_str(name);
    out.push_str("\" style:family=\"paragraph\"");
    if let Some(parent) = parent {
        out.push_str(" style:parent-style-name=\"");
        out.push_str(parent);
        out.push('"');
    }
    out.push('>');
    out.push_str(inner);
    out.push_str("</style:style>");
}

fn push_text_style(out: &mut String, name: &str, inner: &str) {
    out.push_str("<style:style style:name=\"");
    out.push_str(name);
    out.push_str("\" style:family=\"text\">");
    out.push_str(inner);
    out.push_str("</style:style>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tag_splits_into_language_and_region() {
        assert_eq!(language_country("de-DE"), ("de".into(), "DE".into()));
        // A bare primary subtag carries no region.
        assert_eq!(language_country("en"), ("en".into(), "".into()));
        assert_eq!(language_country("yue"), ("yue".into(), "".into()));
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
