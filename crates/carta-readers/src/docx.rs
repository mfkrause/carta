//! Reader for `WordprocessingML` (`.docx`), the zipped-XML word-processor package.
//!
//! A `.docx` file is a ZIP archive of XML parts. The main story lives in `word/document.xml`; it
//! references companion parts by relationship id (`word/_rels/document.xml.rels`) and by convention:
//! `word/styles.xml` names the paragraph and character styles, `word/numbering.xml` defines list
//! marker shapes, and `word/footnotes.xml` / `word/endnotes.xml` hold note bodies. Embedded images
//! live under `word/media/` and are carried into the media bag.
//!
//! Reading proceeds in three stages. First the archive is unzipped and each needed part is parsed
//! into an element tree by a small hand-rolled XML parser (the format ships no DTD the reader must
//! honor, so a permissive well-formed-XML scan suffices). Then the style, numbering, relationship,
//! and note tables are indexed. Finally the body is walked once: each `w:p` becomes a block whose
//! kind is decided by its style name — a `heading N` style is a section heading, `Quote` a block
//! quote, `Source Code` a code block, `Title`/`Author`/`Date`/`Abstract` document metadata — while a
//! paragraph carrying list numbering (`w:numPr`) joins a reconstructed list and everything else is a
//! plain paragraph. Runs (`w:r`) contribute inline content, with each run's properties toggling the
//! emphasis, strong, underline, strike, superscript, subscript, and small-caps wrappers nested in a
//! fixed order. Tables, drawings (images), hyperlinks, note references, and inline `m:oMath` (mapped
//! to TeX) are handled in place. Paragraph text is normalized like prose: whitespace runs collapse to
//! a single space and the leading and trailing edges are trimmed.

use std::collections::{BTreeMap, VecDeque};

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MathType, MetaValue, Row, Table, TableBody, TableFoot,
    TableHead, Target, Text, slug, slug_gfm,
};
use carta_core::container::zip;
use carta_core::{
    BytesReader, DeepStack, Extension, MediaBag, ReaderOptions, Result, on_deep_stack,
};

use crate::heading_ids::IdRegistry;
use crate::xml::{self, Element, local_name};

/// Parses a `WordprocessingML` package into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct DocxReader;

impl BytesReader for DocxReader {
    fn read(&self, input: &[u8], options: &ReaderOptions) -> Result<Document> {
        Ok(self.read_media(input, options)?.0)
    }

    fn read_media(&self, input: &[u8], options: &ReaderOptions) -> Result<(Document, MediaBag)> {
        let parts = zip::read_map(input)?;
        let mut media = MediaBag::new();
        let (meta, blocks) = convert_on_owned_stack(&parts, options, &mut media);
        Ok((
            Document {
                meta,
                blocks,
                ..Document::default()
            },
            media,
        ))
    }
}

/// Runs the body conversion on a dedicated large stack so deep nesting is walked safely regardless of
/// the caller's stack size. Nested block structure (a table inside a cell inside a table, and so on)
/// is walked by mutual recursion that deepens with the nesting, so a legitimately deep document could
/// exhaust a small caller stack. Falls back to the current stack if a worker thread cannot be spawned.
fn convert_on_owned_stack(
    parts: &BTreeMap<String, Vec<u8>>,
    options: &ReaderOptions,
    media: &mut MediaBag,
) -> (BTreeMap<Text, MetaValue>, Vec<Block>) {
    match on_deep_stack(|| Converter::new(parts, options, media).run()) {
        DeepStack::Completed(result) => result,
        // A worker that panicked poisons its join; only an unspawnable thread is worth a retry, run
        // on the current stack instead.
        DeepStack::Panicked => (BTreeMap::new(), Vec::new()),
        DeepStack::NotSpawned => Converter::new(parts, options, media).run(),
    }
}

/// Upper bound on element nesting the parser materializes; content deeper than this is folded in
/// without being descended into, so adversarially deep markup cannot exhaust memory. Body conversion
/// runs on a dedicated stack (see [`convert_on_owned_stack`]), so this ceiling is set well above the
/// nesting genuine documents reach — a chain of a thousand tables nested one inside another survives
/// intact — while still bounding the emitted tree to a depth downstream output can carry on a normal
/// application stack.
const MAX_XML_DEPTH: usize = 3072;

// ---------------------------------------------------------------------------
// Style, numbering, relationship, and note tables
// ---------------------------------------------------------------------------

/// Run-level character formatting accumulated from direct run properties and any referenced
/// character style.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RunFmt {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    superscript: bool,
    subscript: bool,
    smallcaps: bool,
    /// Highlighted (marked) text, wrapped in a `mark`-classed span.
    mark: bool,
    /// Custom character-style names, innermost first, wrapped as `custom-style` spans when the
    /// `styles` extension is on.
    custom: Vec<Text>,
}

/// A style definition indexed from `styles.xml`.
#[derive(Debug, Clone, Default)]
struct StyleDef {
    name: String,
    based_on: Option<String>,
    /// The style's own run properties, before folding in the `basedOn` chain.
    own: RunToggles,
    /// The `numId` this style declares in its own `pPr/numPr`, which enrolls a paragraph carrying
    /// the style into that list even when the paragraph itself has no numbering properties.
    own_num_id: Option<i32>,
    /// The `ilvl` this style declares alongside its own `numId`, if any.
    own_ilvl: Option<i32>,
}

/// The block role a paragraph style denotes, resolved by walking its `basedOn` chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParaRole {
    Heading(i32),
    Quote,
    Code,
    Caption,
    Normal,
}

/// The tri-state run toggles a single style or run-properties element declares.
#[derive(Debug, Clone, Copy, Default)]
struct RunToggles {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    superscript: Option<bool>,
    subscript: Option<bool>,
    smallcaps: Option<bool>,
    mark: Option<bool>,
}

impl RunToggles {
    fn apply(self, fmt: &mut RunFmt) {
        if let Some(value) = self.bold {
            fmt.bold = value;
        }
        if let Some(value) = self.italic {
            fmt.italic = value;
        }
        if let Some(value) = self.underline {
            fmt.underline = value;
        }
        if let Some(value) = self.strike {
            fmt.strike = value;
        }
        if let Some(value) = self.superscript {
            fmt.superscript = value;
        }
        if let Some(value) = self.subscript {
            fmt.subscript = value;
        }
        if let Some(value) = self.smallcaps {
            fmt.smallcaps = value;
        }
        if let Some(value) = self.mark {
            fmt.mark = value;
        }
    }
}

/// One list level's marker configuration.
#[derive(Debug, Clone)]
struct LevelDef {
    /// The ordered-list numeral style, or `None` for a bullet level.
    style: Option<ListNumberStyle>,
    delim: ListNumberDelim,
    start: i32,
}

/// A relationship target from a `.rels` part.
#[derive(Debug, Clone)]
struct Rel {
    target: String,
    external: bool,
}

// ---------------------------------------------------------------------------
// Converter
// ---------------------------------------------------------------------------

/// Which slug algorithm and disambiguation a heading identifier uses.
#[derive(Clone, Copy)]
enum IdMode {
    Plain,
    Gfm,
}

#[allow(clippy::struct_excessive_bools)]
struct Converter<'a> {
    parts: &'a BTreeMap<String, Vec<u8>>,
    media: &'a mut MediaBag,
    styles: BTreeMap<String, StyleDef>,
    /// `numId` → resolved level definitions by `ilvl`.
    lists: BTreeMap<i32, BTreeMap<i32, LevelDef>>,
    rels: BTreeMap<String, Rel>,
    footnotes: BTreeMap<String, Element>,
    endnotes: BTreeMap<String, Element>,
    id_mode: IdMode,
    ascii_ids: bool,
    styles_ext: bool,
    empty_paragraphs: bool,
    ids: IdRegistry,
    meta: BTreeMap<Text, MetaValue>,
    authors: Vec<Vec<Inline>>,
    /// Each abstract-styled paragraph's inlines, in document order. A single paragraph resolves to
    /// `MetaInlines`; several to `MetaBlocks`, one `Para` per paragraph.
    abstract_paras: Vec<Vec<Inline>>,
    note_depth: usize,
    // The leading run of the document body holds the title block: metadata-bearing styles are lifted
    // to the document metadata only while it is open, and the first non-metadata block closes it.
    in_title_block: bool,
}

/// The base path of the main document part, i.e. the directory a relationship target is resolved
/// against (`word/document.xml` → `word/`).
const DOC_DIR: &str = "word/";

/// Upper bound on nested inline containers (hyperlinks, insertions, smart tags, content controls)
/// the inline walk descends through. Real documents nest these only a handful deep; the ceiling
/// keeps a pathologically deep chain from exhausting the call stack and bounds the depth of the
/// emitted tree so no downstream writer can be driven past its own recursion limit.
const MAX_INLINE_DEPTH: usize = 128;

impl<'a> Converter<'a> {
    fn new(
        parts: &'a BTreeMap<String, Vec<u8>>,
        options: &ReaderOptions,
        media: &'a mut MediaBag,
    ) -> Self {
        let extensions = options.extensions;
        let id_mode = if extensions.contains(Extension::GfmAutoIdentifiers) {
            IdMode::Gfm
        } else {
            IdMode::Plain
        };
        Self {
            parts,
            media,
            styles: BTreeMap::new(),
            lists: BTreeMap::new(),
            rels: BTreeMap::new(),
            footnotes: BTreeMap::new(),
            endnotes: BTreeMap::new(),
            id_mode,
            ascii_ids: extensions.contains(Extension::AsciiIdentifiers),
            styles_ext: extensions.contains(Extension::Styles),
            empty_paragraphs: extensions.contains(Extension::EmptyParagraphs),
            ids: IdRegistry::default(),
            meta: BTreeMap::new(),
            authors: Vec::new(),
            abstract_paras: Vec::new(),
            note_depth: 0,
            in_title_block: true,
        }
    }

    fn part(&self, name: &str) -> Option<&[u8]> {
        self.parts.get(name).map(Vec::as_slice)
    }

    /// Parses a part and returns its single top-level element (`<w:document>`, `<w:styles>`, …).
    fn parse_part(&self, name: &str) -> Option<Element> {
        xml::parse(self.part(name)?, MAX_XML_DEPTH)
    }

    /// Reads all index tables, walks the body, and returns the finished metadata and blocks.
    fn run(&mut self) -> (BTreeMap<Text, MetaValue>, Vec<Block>) {
        self.load_relationships();
        self.load_styles();
        self.load_numbering();
        self.load_notes();

        let document = self
            .relationship_part("officeDocument")
            .and_then(|name| self.parse_part(&name))
            .or_else(|| self.parse_part("word/document.xml"));
        let blocks = match document.as_ref().and_then(|root| root.descendant("body")) {
            Some(body) => self.convert_blocks(body),
            None => Vec::new(),
        };
        self.finish_authors();
        self.finish_abstract();
        (std::mem::take(&mut self.meta), blocks)
    }

    /// The part name reached from the package root by a relationship of the given type suffix.
    fn relationship_part(&self, type_suffix: &str) -> Option<String> {
        let root = self.parse_part("_rels/.rels")?;
        for rel in root.elements() {
            if local_name(&rel.name) == "Relationship"
                && rel.attr("Type").is_some_and(|ty| ty.ends_with(type_suffix))
                && let Some(target) = rel.attr("Target")
            {
                return Some(normalize_part(target, ""));
            }
        }
        None
    }

    fn load_relationships(&mut self) {
        let Some(root) = self.parse_part("word/_rels/document.xml.rels") else {
            return;
        };
        for rel in root.elements() {
            if local_name(&rel.name) != "Relationship" {
                continue;
            }
            let (Some(id), Some(target)) = (rel.attr("Id"), rel.attr("Target")) else {
                continue;
            };
            let external = rel
                .attr("TargetMode")
                .is_some_and(|mode| mode == "External");
            self.rels.insert(
                id.to_owned(),
                Rel {
                    target: target.to_owned(),
                    external,
                },
            );
        }
    }

    /// The part name a relationship of the given type suffix points to, resolved against the document
    /// directory, falling back to a conventional `word/<default>` name.
    fn typed_part(&self, type_suffix: &str, default: &str) -> Option<Element> {
        for rel in self.rels.values() {
            if !rel.external
                && rel.target.contains(type_suffix)
                && let Some(element) = self.parse_part(&normalize_part(&rel.target, DOC_DIR))
            {
                return Some(element);
            }
        }
        self.parse_part(default)
    }

    fn load_styles(&mut self) {
        // Style definitions are found by the document relationship of type `styles`, or the
        // conventional part name.
        let Some(root) = self.styles_root() else {
            return;
        };
        for style in root.elements() {
            if local_name(&style.name) != "style" {
                continue;
            }
            let Some(id) = style.attr("styleId") else {
                continue;
            };
            let name = style
                .child("name")
                .and_then(|element| element.attr("val"))
                .unwrap_or("")
                .to_owned();
            let based_on = style
                .child("basedOn")
                .and_then(|element| element.attr("val"))
                .map(str::to_owned);
            let own = style.child("rPr").map(read_toggles).unwrap_or_default();
            let style_num = style.child("pPr").and_then(|ppr| ppr.child("numPr"));
            let own_num_id = style_num
                .and_then(|np| np.child("numId"))
                .and_then(|element| element.attr("val"))
                .and_then(parse_int);
            let own_ilvl = style_num
                .and_then(|np| np.child("ilvl"))
                .and_then(|element| element.attr("val"))
                .and_then(parse_int);
            self.styles.insert(
                id.to_owned(),
                StyleDef {
                    name,
                    based_on,
                    own,
                    own_num_id,
                    own_ilvl,
                },
            );
        }
    }

    fn styles_root(&self) -> Option<Element> {
        for rel in self.rels.values() {
            if !rel.external
                && rel.target.ends_with("styles.xml")
                && let Some(element) = self.parse_part(&normalize_part(&rel.target, DOC_DIR))
            {
                return Some(element);
            }
        }
        self.parse_part("word/styles.xml")
    }

    /// The display name of the style with the given id.
    fn style_name(&self, id: &str) -> Option<&str> {
        self.styles.get(id).map(|style| style.name.as_str())
    }

    /// A style and its `basedOn` ancestors, innermost first. A fixed guard bounds the walk so a self-
    /// or mutually-referential chain terminates rather than looping forever.
    fn style_chain(&self, id: &str) -> impl Iterator<Item = &StyleDef> {
        let mut current = self.styles.get(id);
        let mut remaining = 33u32;
        std::iter::from_fn(move || {
            let style = current?;
            remaining -= 1;
            current = if remaining == 0 {
                None
            } else {
                style
                    .based_on
                    .as_deref()
                    .and_then(|base| self.styles.get(base))
            };
            Some(style)
        })
    }

    /// Whether a character style resolves, through its `basedOn` chain, to the verbatim style that
    /// marks inline code.
    fn is_code_style(&self, id: &str) -> bool {
        self.style_chain(id)
            .any(|style| canonical_style(&style.name) == "verbatim char")
    }

    /// The block role a paragraph style denotes, resolved through its `basedOn` chain. The metadata
    /// roles (title, subtitle, author, date, abstract) are intentionally excluded here: those are
    /// recognized only by a style's own display name, not inherited.
    fn paragraph_role(&self, id: &str) -> ParaRole {
        self.style_chain(id)
            .find_map(|style| {
                let canonical = canonical_style(&style.name);
                if let Some(level) = heading_level(&canonical) {
                    return Some(ParaRole::Heading(level));
                }
                if canonical == "source code" {
                    return Some(ParaRole::Code);
                }
                if canonical == "quote"
                    || canonical == "block text"
                    || canonical == "intense quote"
                    || canonical == "block quote"
                {
                    return Some(ParaRole::Quote);
                }
                if is_caption_style(&canonical) {
                    return Some(ParaRole::Caption);
                }
                None
            })
            .unwrap_or(ParaRole::Normal)
    }

    /// The list numbering a paragraph style contributes through its own `pPr/numPr`, resolved along
    /// the `basedOn` chain: `(numId, ilvl)`. The first style in the chain that declares a `numId`
    /// wins, so a paragraph carrying such a style joins the list without any direct numbering
    /// properties of its own.
    fn style_num_pr(&self, id: &str) -> Option<(i32, Option<i32>)> {
        self.style_chain(id)
            .find_map(|style| style.own_num_id.map(|num_id| (num_id, style.own_ilvl)))
    }

    /// The effective run formatting a character style contributes, folding its `basedOn` chain.
    fn style_fmt(&self, id: &str, fmt: &mut RunFmt) {
        let chain: Vec<&StyleDef> = self.style_chain(id).collect();
        for style in chain.iter().rev() {
            style.own.apply(fmt);
        }
    }

    fn load_numbering(&mut self) {
        let Some(root) = self.typed_part("numbering", "word/numbering.xml") else {
            return;
        };
        // Abstract definitions first, then the concrete `num` entries that point at them.
        let mut abstracts: BTreeMap<String, BTreeMap<i32, LevelDef>> = BTreeMap::new();
        for abstract_num in root.elements() {
            if local_name(&abstract_num.name) != "abstractNum" {
                continue;
            }
            let Some(id) = abstract_num.attr("abstractNumId") else {
                continue;
            };
            let mut levels: BTreeMap<i32, LevelDef> = BTreeMap::new();
            for lvl in abstract_num.elements() {
                if local_name(&lvl.name) != "lvl" {
                    continue;
                }
                let ilvl = lvl.attr("ilvl").and_then(parse_int).unwrap_or(0);
                levels.insert(ilvl, read_level(lvl));
            }
            abstracts.insert(id.to_owned(), levels);
        }
        for num in root.elements() {
            if local_name(&num.name) != "num" {
                continue;
            }
            let Some(num_id) = num.attr("numId").and_then(parse_int) else {
                continue;
            };
            let Some(abstract_id) = num
                .child("abstractNumId")
                .and_then(|element| element.attr("val"))
            else {
                continue;
            };
            if let Some(mut levels) = abstracts.get(abstract_id).cloned() {
                apply_level_overrides(num, &mut levels);
                self.lists.insert(num_id, levels);
            }
        }
    }

    fn load_notes(&mut self) {
        if let Some(root) = self.typed_part("footnotes", "word/footnotes.xml") {
            index_notes(&root, "footnote", &mut self.footnotes);
        }
        if let Some(root) = self.typed_part("endnotes", "word/endnotes.xml") {
            index_notes(&root, "endnote", &mut self.endnotes);
        }
    }

    // -- block conversion ---------------------------------------------------

    /// Walks a block container (`w:body`, `w:tc`, or a note), reconstructing lists, block quotes, and
    /// code blocks from consecutive same-styled paragraphs.
    fn convert_blocks(&mut self, container: &Element) -> Vec<Block> {
        let mut sink = BlockSink::default();
        for element in container.elements() {
            match local_name(&element.name) {
                "p" => self.convert_paragraph(element, &mut sink),
                "tbl" => {
                    self.in_title_block = false;
                    if let Some(table) = self.convert_table(element) {
                        sink.emit_table(table);
                    }
                }
                "sdt" => {
                    if let Some(content) = element.child("sdtContent") {
                        let inner = self.convert_blocks(content);
                        for block in inner {
                            sink.emit(block);
                        }
                    }
                }
                _ => {}
            }
        }
        sink.finish()
    }

    #[allow(clippy::too_many_lines)]
    fn convert_paragraph(&mut self, paragraph: &Element, sink: &mut BlockSink) {
        let properties = paragraph.child("pPr");
        let style_id = properties
            .and_then(|pr| pr.child("pStyle"))
            .and_then(|element| element.attr("val"));
        let style_name = style_id
            .and_then(|id| self.style_name(id))
            .map(str::to_owned)
            .unwrap_or_default();
        let canonical = canonical_style(&style_name);
        // The compact style marks a paragraph that carries no block spacing, so its body reads as a
        // bare line rather than a spaced paragraph.
        let compact = canonical == "compact";

        let inlines = self.convert_inlines(paragraph);

        // Metadata-bearing styles consume the paragraph, but only inside the leading title block; the
        // first paragraph that is not one of them closes the block, and any later paragraph carrying
        // such a style is ordinary body content.
        let metadata_style = matches!(
            canonical.as_str(),
            "title" | "subtitle" | "author" | "date" | "abstract"
        );
        if self.in_title_block && metadata_style {
            sink.flush();
            match canonical.as_str() {
                "author" => self.authors.push(inlines),
                "abstract" => self.abstract_paras.push(inlines),
                key => {
                    self.meta
                        .insert(key.into(), MetaValue::MetaInlines(inlines));
                }
            }
            return;
        }
        self.in_title_block = false;

        // A style's block role is resolved through its `basedOn` chain, so a custom style built on a
        // heading, quote, code, or caption style takes that role rather than falling back to a plain
        // paragraph.
        let role = style_id.map_or(ParaRole::Normal, |id| self.paragraph_role(id));
        let custom = !style_name.is_empty() && !is_builtin_style(&canonical);
        // Under the `styles` extension a non-builtin custom style keeps its identity as a
        // `custom-style` container; its role still shapes the block placed inside.
        let wrap_custom = self.styles_ext && custom;

        // A heading is always a section header — never a custom-style container; a custom style that
        // inherits the heading role contributes its name as a class.
        if let ParaRole::Heading(level) = role {
            let id = self.heading_id(&inlines);
            let classes = if custom {
                vec![style_class(&style_name)]
            } else {
                Vec::new()
            };
            sink.emit(Block::Header(
                level,
                Box::new(Attr {
                    id: id.into(),
                    classes,
                    attributes: Vec::new(),
                }),
                inlines,
            ));
            return;
        }

        // The remaining roles sink into the surrounding flow, unless a custom style defers them to a
        // container below.
        if !wrap_custom {
            match role {
                // A caption paragraph folds into an adjacent image or table; on its own it stays a
                // paragraph.
                ParaRole::Caption if !inlines.is_empty() => {
                    sink.push_caption(vec![Block::Para(inlines)]);
                    return;
                }
                ParaRole::Code => {
                    sink.push_code(code_paragraph_text(paragraph));
                    return;
                }
                ParaRole::Quote => {
                    sink.push_quote(Block::Para(inlines));
                    return;
                }
                _ => {}
            }
        }

        // List membership is decided by numbering properties: the paragraph's own `numPr` wins, and
        // in its absence the paragraph style contributes numbering through its `basedOn` chain.
        let direct_num = properties.and_then(|pr| pr.child("numPr"));
        let direct_num_id = direct_num
            .and_then(|np| np.child("numId"))
            .and_then(|element| element.attr("val"))
            .and_then(parse_int);
        let direct_ilvl = direct_num
            .and_then(|np| np.child("ilvl"))
            .and_then(|element| element.attr("val"))
            .and_then(parse_int);
        let (num_id, ilvl) = match direct_num_id {
            Some(id) => (Some(id), direct_ilvl.unwrap_or(0)),
            None => match style_id.and_then(|id| self.style_num_pr(id)) {
                Some((id, style_ilvl)) => (Some(id), direct_ilvl.or(style_ilvl).unwrap_or(0)),
                None => (None, 0),
            },
        };
        if let Some(num_id) = num_id {
            let numbering = self.list_numbering(num_id, ilvl);
            let item = if compact {
                Block::Plain(inlines)
            } else {
                Block::Para(inlines)
            };
            // A custom-styled list paragraph keeps its style identity: under the `styles` extension
            // each item is wrapped in its own `custom-style` container.
            let block = if wrap_custom {
                Block::Div(Box::new(custom_style_attr(&style_name)), vec![item])
            } else {
                item
            };
            sink.push_list(ListEntry {
                num_id,
                level: ilvl,
                numbering,
                block,
            });
            return;
        }

        if inlines.is_empty() {
            if self.empty_paragraphs {
                sink.emit(Block::Para(Vec::new()));
            } else {
                sink.interrupt();
            }
            return;
        }

        // A lone-image paragraph is a figure candidate: a caption on either side folds it into a
        // figure, and on its own it stays an image paragraph.
        if single_image(&inlines) {
            sink.emit_image(inlines);
            return;
        }

        // A paragraph indented from the left margin is a block quote; consecutive indented paragraphs
        // fold into one quote regardless of their individual depths.
        let indented = net_left_indent(properties) > 0;

        if wrap_custom {
            let inner = match role {
                ParaRole::Quote => vec![Block::BlockQuote(vec![Block::Para(inlines)])],
                ParaRole::Code => vec![Block::CodeBlock(
                    Box::default(),
                    code_paragraph_text(paragraph).into(),
                )],
                _ => {
                    let para = Block::Para(inlines);
                    if indented {
                        vec![Block::BlockQuote(vec![para])]
                    } else {
                        vec![para]
                    }
                }
            };
            sink.emit(Block::Div(Box::new(custom_style_attr(&style_name)), inner));
            return;
        }

        let body = if compact {
            Block::Plain(inlines)
        } else {
            Block::Para(inlines)
        };
        if indented {
            sink.push_quote(body);
        } else {
            sink.emit(body);
        }
    }

    /// The marker configuration for a list paragraph, or `None` when its level is a bullet or the
    /// numbering id is unknown.
    fn list_numbering(&self, num_id: i32, ilvl: i32) -> Option<ListAttributes> {
        let level = self.lists.get(&num_id)?.get(&ilvl)?;
        level.style.map(|style| ListAttributes {
            start: level.start,
            style,
            delim: level.delim,
        })
    }

    // -- inline conversion --------------------------------------------------

    /// Builds the inline content of a paragraph or paragraph-like container from its runs.
    fn convert_inlines(&mut self, container: &Element) -> Vec<Inline> {
        let mut builder = InlineBuilder::default();
        self.walk_inlines(container, &mut builder, true, 0);
        builder.finish()
    }

    /// Recursively emits inline leaves from a container's children. `top` marks the paragraph level,
    /// where `pPr` is skipped. `depth` counts nested inline containers and is capped by
    /// [`MAX_INLINE_DEPTH`] so unbounded nesting cannot exhaust the stack.
    fn walk_inlines(
        &mut self,
        container: &Element,
        builder: &mut InlineBuilder,
        top: bool,
        depth: usize,
    ) {
        for element in container.elements() {
            match local_name(&element.name) {
                "pPr" if top => {}
                "r" => self.convert_run(element, builder),
                "hyperlink" => self.convert_hyperlink(element, builder, depth),
                "ins" | "moveTo" | "smartTag" if depth < MAX_INLINE_DEPTH => {
                    self.walk_inlines(element, builder, false, depth + 1);
                }
                "sdt" if depth < MAX_INLINE_DEPTH => {
                    if let Some(content) = element.child("sdtContent") {
                        self.walk_inlines(content, builder, false, depth + 1);
                    }
                }
                "oMath" => {
                    let tex = omml_to_tex(element);
                    builder.push_node(
                        RunFmt::default(),
                        Inline::Math(MathType::InlineMath, tex.into()),
                    );
                }
                "oMathPara" => {
                    if let Some(math) = element.child("oMath") {
                        let tex = omml_to_tex(math);
                        builder.push_node(
                            RunFmt::default(),
                            Inline::Math(MathType::DisplayMath, tex.into()),
                        );
                    }
                }
                // Deletions, comment anchors, bookmarks, and proofing marks carry no body content.
                _ => {}
            }
        }
    }

    fn convert_run(&mut self, run: &Element, builder: &mut InlineBuilder) {
        let mut fmt = RunFmt::default();
        let mut is_code = false;
        let properties = run.child("rPr");
        if let Some(properties) = properties {
            if let Some(style_id) = properties
                .child("rStyle")
                .and_then(|element| element.attr("val"))
            {
                is_code = self.is_code_style(style_id);
                let custom_name = self.styles_ext.then(|| self.style_name(style_id)).flatten();
                match custom_name
                    .filter(|name| !name.is_empty() && !is_builtin_style(&canonical_style(name)))
                {
                    // A run styled with a non-builtin character style is wrapped in a custom-style
                    // span; the style's own formatting rides with the span rather than also being
                    // reapplied here as toggles.
                    Some(name) => fmt.custom.push(name.into()),
                    None => self.style_fmt(style_id, &mut fmt),
                }
            }
            read_toggles(properties).apply(&mut fmt);
        }
        if is_code {
            // A verbatim-styled run is a single inline code span; its text is taken literally, with
            // internal whitespace preserved rather than collapsed as prose.
            let mut text = String::new();
            for child in run.elements() {
                match local_name(&child.name) {
                    "t" => text.push_str(&child.text()),
                    "tab" => text.push('\t'),
                    "br" | "cr" => text.push('\n'),
                    "noBreakHyphen" => text.push('\u{2011}'),
                    _ => {}
                }
            }
            builder.push_node(RunFmt::default(), Inline::Code(Box::default(), text.into()));
            return;
        }
        // A run keyed to a legacy pictorial font (Symbol, Wingdings) does not carry those glyphs'
        // letters literally; each maps to the Unicode character it renders.
        let sub = properties.and_then(symbol_font);
        for child in run.elements() {
            if local_name(&child.name) == "AlternateContent" {
                // A markup-compatibility container guards feature-gated content behind a Choice and
                // supplies a Fallback for readers without that feature. This reader renders the
                // Fallback's run content and ignores the Choice.
                if let Some(fallback) = child.child("Fallback") {
                    for leaf in fallback.elements() {
                        self.run_child(leaf, &fmt, sub, builder);
                    }
                }
                continue;
            }
            self.run_child(child, &fmt, sub, builder);
        }
    }

    /// Emits one run-content child (text, break, image, math, note reference, or field marker).
    fn run_child(
        &mut self,
        child: &Element,
        fmt: &RunFmt,
        sub: Option<SymbolFont>,
        builder: &mut InlineBuilder,
    ) {
        match local_name(&child.name) {
            "t" => match sub {
                Some(font) => builder.push_text(fmt, &font.substitute(&child.text())),
                None => builder.push_text(fmt, &child.text()),
            },
            "tab" => builder.push_space(fmt),
            "br" => {
                // A page or column break renders no line of its own and carries no content.
                let kind = child.attr("type").unwrap_or("textWrapping");
                if kind == "textWrapping" {
                    builder.push_break(fmt);
                }
            }
            "cr" => builder.push_break(fmt),
            "noBreakHyphen" => builder.push_text(fmt, "\u{2011}"),
            "sym" => {
                if let Some(ch) = child
                    .attr("char")
                    .and_then(|code| u32::from_str_radix(code, 16).ok())
                    .and_then(char::from_u32)
                {
                    builder.push_text(fmt, &ch.to_string());
                }
            }
            "drawing" | "pict" => {
                if let Some(image) = self.convert_drawing(child) {
                    // An image carries no character formatting: it neither takes emphasis nor holds a
                    // run's wrappers together, so it is emitted unformatted and splits any span it
                    // falls inside.
                    builder.push_node(RunFmt::default(), image);
                }
            }
            "oMath" => {
                let tex = omml_to_tex(child);
                builder.push_node(fmt.clone(), Inline::Math(MathType::InlineMath, tex.into()));
            }
            "footnoteReference" => {
                if let Some(note) = self.convert_note(child, false) {
                    // A footnote mark's run styling (the superscript reference style) is
                    // presentational; the note stands on its own with no inline wrappers.
                    builder.push_node(RunFmt::default(), note);
                }
            }
            "endnoteReference" => {
                if let Some(note) = self.convert_note(child, true) {
                    builder.push_node(RunFmt::default(), note);
                }
            }
            "fldChar" => match child.attr("fldCharType") {
                Some("begin") => builder.field_begin(),
                Some("separate") => builder.field_separate(),
                Some("end") => builder.field_end(),
                _ => {}
            },
            "instrText" => builder.field_instr(&child.text()),
            _ => {}
        }
    }

    fn convert_hyperlink(&mut self, link: &Element, builder: &mut InlineBuilder, depth: usize) {
        if depth >= MAX_INLINE_DEPTH {
            return;
        }
        let mut inner = InlineBuilder::default();
        self.walk_inlines(link, &mut inner, false, depth + 1);
        let content = inner.finish();
        if content.is_empty() {
            return;
        }
        let url = if let Some(anchor) = link.attr("anchor") {
            format!("#{anchor}")
        } else if let Some(id) = link.attr_qualified("r:id", "id") {
            self.rels
                .get(id)
                .map(|rel| rel.target.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        builder.push_node(
            RunFmt::default(),
            Inline::Link(
                Box::default(),
                content,
                Box::new(Target {
                    url: url.into(),
                    title: Text::default(),
                }),
            ),
        );
    }

    fn convert_note(&mut self, reference: &Element, endnote: bool) -> Option<Inline> {
        if self.note_depth >= 8 {
            return None;
        }
        let id = reference.attr("id")?;
        let note = if endnote {
            self.endnotes.get(id)
        } else {
            self.footnotes.get(id)
        }?
        .clone();
        self.note_depth += 1;
        // A note body is its own block flow; title-block metadata is only harvested from the leading
        // run of the main document, so suspend extraction here and restore the outer state after.
        let outer_title_block = self.in_title_block;
        self.in_title_block = false;
        let blocks = self.convert_blocks(&note);
        self.in_title_block = outer_title_block;
        self.note_depth -= 1;
        Some(Inline::Note(blocks))
    }

    fn convert_drawing(&mut self, drawing: &Element) -> Option<Inline> {
        // A DrawingML picture carries its relationship on an `a:blip`; a legacy VML shape or an
        // embedded OLE preview carries it on a `v:imagedata`. Either identifies the packaged image.
        let rel_id = drawing
            .descendant("blip")
            .and_then(|blip| {
                blip.attr_qualified("r:embed", "embed")
                    .or_else(|| blip.attr_qualified("r:link", "link"))
            })
            .or_else(|| {
                drawing.descendant("imagedata").and_then(|data| {
                    data.attr_qualified("r:id", "id")
                        .or_else(|| data.attr_qualified("r:link", "link"))
                })
            })?;
        let rel = self.rels.get(rel_id)?;
        let alt = drawing
            .descendant("docPr")
            .and_then(|doc_pr| doc_pr.attr("descr"))
            .map(tokenize_inlines)
            .unwrap_or_default();
        if rel.external {
            // An externally linked image is referenced by its URL and carries no packaged bytes.
            return Some(Inline::Image(
                Box::new(image_attr(drawing)),
                alt,
                Box::new(Target {
                    url: rel.target.clone().into(),
                    title: Text::default(),
                }),
            ));
        }
        let url = normalize_target(&rel.target);
        // The bytes are only fetched and copied when the image is new to the bag; a repeat reference
        // skips the allocation entirely.
        if !self.media.contains(&url) {
            let part_name = format!("{DOC_DIR}{url}");
            if let Some(bytes) = self.part(&part_name).map(<[u8]>::to_vec) {
                self.media.insert(url.clone(), Some(mime_for(&url)), bytes);
            }
        }
        Some(Inline::Image(
            Box::new(image_attr(drawing)),
            alt,
            Box::new(Target {
                url: url.into(),
                title: Text::default(),
            }),
        ))
    }

    // -- tables -------------------------------------------------------------

    #[allow(clippy::too_many_lines)]
    fn convert_table(&mut self, table: &Element) -> Option<Block> {
        let grid: Vec<i64> = table
            .child("tblGrid")
            .into_iter()
            .flat_map(Element::elements)
            .filter(|element| local_name(&element.name) == "gridCol")
            .map(|element| element.attr("w").and_then(parse_int).map_or(0, i64::from))
            .collect();
        let total: i64 = grid.iter().sum();
        // A grid column's width is expressed as a fraction of a reference text width: a default page's
        // printable width, reduced by a fixed allowance for each inter-column boundary, but never
        // narrower than the grid's own total so an over-wide table stays normalized to itself.
        let column_count = i64::try_from(grid.len()).unwrap_or(i64::MAX);
        let reference_width =
            (DEFAULT_TEXT_WIDTH_TWIPS - INTER_COLUMN_TWIPS * (column_count - 1).max(0)).max(total);

        // A table look requesting first-row conditional formatting promotes the first row to a header
        // when no row carries its own header marker.
        let look_first_row = table
            .child("tblPr")
            .and_then(|pr| pr.child("tblLook"))
            .is_some_and(table_look_first_row);

        // The column count is fixed by the grid; a table without a grid takes the width of its widest
        // row. Every cell span is validated against this count, so a malformed `gridSpan` can neither
        // overflow a column position nor inflate the table's width.
        let columns = if grid.is_empty() {
            table
                .elements()
                .filter(|tr| local_name(&tr.name) == "tr")
                .map(|tr| {
                    tr.elements()
                        .filter(|tc| local_name(&tc.name) == "tc")
                        .count()
                })
                .max()
                .unwrap_or(0)
        } else {
            grid.len()
        };

        // Parse each row into positioned cells so vertical merges can be resolved by grid column.
        let mut rows: Vec<RowRaw> = Vec::new();
        for tr in table.elements() {
            if local_name(&tr.name) != "tr" {
                continue;
            }
            let header = tr
                .child("trPr")
                .is_some_and(|pr| pr.child("tblHeader").is_some());
            let mut cells: Vec<CellRaw> = Vec::new();
            let mut col = 0usize;
            for tc in tr.elements() {
                if local_name(&tc.name) != "tc" {
                    continue;
                }
                if col >= columns {
                    // No column remains: a surplus cell is dropped, never widening the table.
                    continue;
                }
                let properties = tc.child("tcPr");
                let requested = properties
                    .and_then(|pr| pr.child("gridSpan"))
                    .and_then(|element| element.attr("val"))
                    .and_then(parse_int)
                    .unwrap_or(1)
                    .max(1);
                let remaining = columns - col;
                let span = usize::try_from(requested)
                    .unwrap_or(remaining)
                    .min(remaining);
                let vmerge = properties.and_then(|pr| pr.child("vMerge")).map(|element| {
                    match element.attr("val") {
                        Some("restart") => VMerge::Restart,
                        _ => VMerge::Continue,
                    }
                });
                let align = tc
                    .child("p")
                    .and_then(|p| p.child("pPr"))
                    .and_then(|pr| pr.child("jc"))
                    .and_then(|element| element.attr("val"))
                    .map_or(Alignment::AlignDefault, alignment);
                let content = self.convert_cell(tc);
                cells.push(CellRaw {
                    start_col: col,
                    span,
                    vmerge,
                    align,
                    content,
                });
                col += span;
            }
            rows.push(RowRaw { header, cells });
        }

        if rows.is_empty() {
            return None;
        }

        // A row is a header when it is flagged so; absent any such flag, a `firstRow` table look marks
        // the first row.
        let any_header = rows.iter().any(|row| row.header);
        let head_count = if any_header {
            rows.iter().take_while(|row| row.header).count()
        } else {
            usize::from(look_first_row)
        };

        let col_specs = (0..columns)
            .map(|index| {
                let width = match grid.get(index) {
                    Some(&value) if total > 0 => ColWidth::ColWidth(ratio(value, reference_width)),
                    _ => ColWidth::ColWidthDefault,
                };
                let align = column_alignment(&rows, head_count, index);
                ColSpec { align, width }
            })
            .collect();

        let mut head_rows = Vec::new();
        let mut body_rows = Vec::new();
        for (index, row) in build_rows(rows).into_iter().enumerate() {
            if index < head_count {
                head_rows.push(row);
            } else {
                body_rows.push(row);
            }
        }

        Some(Block::Table(Box::new(Table {
            attr: Attr::default(),
            caption: Caption::default(),
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: head_rows,
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: body_rows,
            }],
            foot: TableFoot::default(),
        })))
    }

    /// Cell content: a single paragraph becomes a `Plain` block; anything richer keeps its blocks.
    fn convert_cell(&mut self, cell: &Element) -> Vec<Block> {
        // Cell content is a nested block flow, never the document's leading title block.
        let outer_title_block = self.in_title_block;
        self.in_title_block = false;
        let mut blocks = self.convert_blocks(cell);
        self.in_title_block = outer_title_block;
        if matches!(blocks.as_slice(), [Block::Para(_)])
            && let Some(Block::Para(inlines)) = blocks.pop()
        {
            return vec![Block::Plain(inlines)];
        }
        blocks
    }

    // -- identifiers & metadata --------------------------------------------

    /// Derives a unique heading identifier from heading text, honoring the ASCII- and GitHub-slug
    /// extensions and disambiguating repeats with an incrementing numeric suffix.
    fn heading_id(&mut self, inlines: &[Inline]) -> String {
        let text = carta_ast::to_plain_text(inlines);
        let source = if self.ascii_ids {
            fold_to_ascii(&text)
        } else {
            text
        };
        let base = match self.id_mode {
            IdMode::Plain => slug(&source),
            IdMode::Gfm => slug_gfm(&source),
        };
        self.ids.assign_with_separator(base, '-')
    }

    fn finish_authors(&mut self) {
        let entries = std::mem::take(&mut self.authors);
        let value = match entries.len() {
            0 => return,
            1 => MetaValue::MetaInlines(entries.into_iter().next().unwrap_or_default()),
            _ => MetaValue::MetaList(entries.into_iter().map(MetaValue::MetaInlines).collect()),
        };
        self.meta.insert("author".into(), value);
    }

    fn finish_abstract(&mut self) {
        let paras = std::mem::take(&mut self.abstract_paras);
        let value = match paras.len() {
            0 => return,
            1 => MetaValue::MetaInlines(paras.into_iter().next().unwrap_or_default()),
            _ => MetaValue::MetaBlocks(paras.into_iter().map(Block::Para).collect()),
        };
        self.meta.insert("abstract".into(), value);
    }
}

// ---------------------------------------------------------------------------
// Block accumulation
// ---------------------------------------------------------------------------

/// One list paragraph awaiting reassembly into nested lists.
struct ListEntry {
    num_id: i32,
    level: i32,
    numbering: Option<ListAttributes>,
    block: Block,
}

/// Whether the block most recently placed can still absorb a caption paragraph, and how.
#[derive(Default, PartialEq, Eq)]
enum Attachable {
    /// The last block is neither an image paragraph nor a table awaiting a caption.
    #[default]
    None,
    /// The last block is a lone-image paragraph that a caption turns into a figure.
    Figure,
    /// The last block is a table whose caption slot is still empty.
    Table,
}

/// Collects converted blocks while merging consecutive list, block-quote, and code paragraphs and
/// folding caption paragraphs into an adjacent image (as a figure) or table.
#[derive(Default)]
struct BlockSink {
    blocks: Vec<Block>,
    pending_list: Vec<ListEntry>,
    pending_quote: Vec<Block>,
    pending_code: Vec<String>,
    /// A caption paragraph held for the block that follows it, set when no image or table precedes it.
    pending_caption: Option<Vec<Block>>,
    /// Whether the last placed block can still take a caption, so an image or table before a caption
    /// wins over one after it.
    last_attachable: Attachable,
    /// Running ordinal for each numbered level, keyed by `(numId, ilvl)`, so a list resumes its count
    /// after an interrupting block instead of restarting.
    list_counters: BTreeMap<(i32, i32), i32>,
}

impl BlockSink {
    /// Appends a finished block and records whether it can still take a caption.
    fn place(&mut self, block: Block, attachable: Attachable) {
        self.blocks.push(block);
        self.last_attachable = attachable;
    }

    /// Emits any held caption as its own paragraph, used when the block that follows it cannot be a
    /// figure or table target.
    fn release_caption(&mut self) {
        if let Some(long) = self.pending_caption.take() {
            for block in long {
                self.place(block, Attachable::None);
            }
        }
    }

    /// Records an ordinary finished block, first flushing merge runs and releasing any held caption.
    fn emit(&mut self, block: Block) {
        self.flush();
        self.release_caption();
        self.place(block, Attachable::None);
    }

    /// Records a lone-image paragraph, forming a figure with a preceding caption when one is held.
    fn emit_image(&mut self, inlines: Vec<Inline>) {
        self.flush();
        match self.pending_caption.take() {
            Some(long) => self.place(
                Block::Figure(
                    Box::default(),
                    Box::new(Caption { short: None, long }),
                    vec![Block::Plain(inlines)],
                ),
                Attachable::None,
            ),
            None => self.place(Block::Para(inlines), Attachable::Figure),
        }
    }

    /// Records a table, folding a preceding caption into its caption slot when one is held.
    fn emit_table(&mut self, block: Block) {
        self.flush();
        if let Block::Table(mut table) = block {
            match self.pending_caption.take() {
                Some(long) => {
                    table.caption = Caption { short: None, long };
                    self.place(Block::Table(table), Attachable::None);
                }
                None => self.place(Block::Table(table), Attachable::Table),
            }
        } else {
            self.release_caption();
            self.place(block, Attachable::None);
        }
    }

    /// Records a caption paragraph, attaching it to a preceding image or table when one is available
    /// and otherwise holding it for the block that follows.
    fn push_caption(&mut self, long: Vec<Block>) {
        self.flush();
        match self.last_attachable {
            Attachable::Figure if matches!(self.blocks.last(), Some(Block::Para(_))) => {
                self.last_attachable = Attachable::None;
                if let Some(Block::Para(inlines)) = self.blocks.pop() {
                    self.place(
                        Block::Figure(
                            Box::default(),
                            Box::new(Caption { short: None, long }),
                            vec![Block::Plain(inlines)],
                        ),
                        Attachable::None,
                    );
                }
            }
            Attachable::Table if matches!(self.blocks.last(), Some(Block::Table(_))) => {
                self.last_attachable = Attachable::None;
                if let Some(Block::Table(mut table)) = self.blocks.pop() {
                    table.caption = Caption { short: None, long };
                    self.place(Block::Table(table), Attachable::None);
                }
            }
            _ => {
                self.release_caption();
                self.pending_caption = Some(long);
            }
        }
    }

    /// Ends any merge run and caption context without placing a block, as a dropped empty paragraph
    /// does between an image and a caption.
    fn interrupt(&mut self) {
        self.flush();
        self.release_caption();
        self.last_attachable = Attachable::None;
    }

    fn push_list(&mut self, mut entry: ListEntry) {
        self.release_caption();
        self.flush_quote();
        self.flush_code();
        if let Some(attrs) = entry.numbering.as_mut() {
            let key = (entry.num_id, entry.level);
            let ordinal = match self.list_counters.get(&key) {
                Some(previous) => previous.saturating_add(1),
                None => attrs.start,
            };
            self.list_counters.insert(key, ordinal);
            // Advancing a level restarts every level nested under it.
            self.list_counters
                .retain(|(num_id, level), _| !(*num_id == entry.num_id && *level > entry.level));
            attrs.start = ordinal;
        }
        self.pending_list.push(entry);
    }

    fn push_quote(&mut self, block: Block) {
        self.release_caption();
        self.flush_list();
        self.flush_code();
        self.pending_quote.push(block);
    }

    fn push_code(&mut self, line: String) {
        self.release_caption();
        self.flush_list();
        self.flush_quote();
        self.pending_code.push(line);
    }

    fn flush(&mut self) {
        self.flush_list();
        self.flush_quote();
        self.flush_code();
    }

    fn flush_list(&mut self) {
        if self.pending_list.is_empty() {
            return;
        }
        let entries = VecDeque::from(std::mem::take(&mut self.pending_list));
        for block in build_lists(entries) {
            self.place(block, Attachable::None);
        }
    }

    fn flush_quote(&mut self) {
        if self.pending_quote.is_empty() {
            return;
        }
        let inner = std::mem::take(&mut self.pending_quote);
        self.place(Block::BlockQuote(inner), Attachable::None);
    }

    fn flush_code(&mut self) {
        if self.pending_code.is_empty() {
            return;
        }
        let code = std::mem::take(&mut self.pending_code).join("\n");
        self.place(
            Block::CodeBlock(Box::default(), code.into()),
            Attachable::None,
        );
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush();
        self.release_caption();
        self.blocks
    }
}

/// Reassembles list paragraphs into nested lists: a maximal span at the shallowest level forms one
/// list, a deeper span nests inside the preceding item, and a same-level paragraph that selects a
/// different numbering begins a fresh sibling list.
fn build_lists(mut entries: VecDeque<ListEntry>) -> Vec<Block> {
    let mut out = Vec::new();
    while !entries.is_empty() {
        out.push(build_one_list(&mut entries, 0));
    }
    out
}

/// Consumes the leading run of `entries` that shares the shallowest level and numbering, folding any
/// deeper run that follows an item into that item as a nested list. The consumed entries are removed
/// from the front of the deque so their block content is moved into the tree rather than cloned.
fn build_one_list(entries: &mut VecDeque<ListEntry>, depth: usize) -> Block {
    const MAX_LIST_DEPTH: usize = 256;
    let base = entries.front().map_or(0, |entry| entry.level);
    let num_id = entries.front().map_or(0, |entry| entry.num_id);
    let numbering = entries.front().and_then(|entry| entry.numbering.clone());
    let mut items: Vec<Vec<Block>> = Vec::new();
    while entries
        .front()
        .is_some_and(|entry| entry.level == base && entry.num_id == num_id)
    {
        let Some(entry) = entries.pop_front() else {
            break;
        };
        let mut item = vec![entry.block];
        if matches!(entries.front(), Some(next) if next.level > base) && depth < MAX_LIST_DEPTH {
            item.push(build_one_list(entries, depth + 1));
        }
        items.push(item);
    }
    match numbering {
        Some(attrs) => Block::OrderedList(attrs, items),
        None => Block::BulletList(items),
    }
}

// ---------------------------------------------------------------------------
// Table row assembly
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum VMerge {
    Restart,
    Continue,
}

struct CellRaw {
    start_col: usize,
    span: usize,
    vmerge: Option<VMerge>,
    align: Alignment,
    content: Vec<Block>,
}

struct RowRaw {
    header: bool,
    cells: Vec<CellRaw>,
}

/// Resolves vertical merges into row spans and drops the continuation cells they absorb.
fn build_rows(rows: Vec<RowRaw>) -> Vec<Row> {
    // A first pass over the light merge markers resolves each retained cell's vertical span, aligned
    // one-to-one with `row.cells` (`None` marks a continuation cell that is dropped). The second pass
    // can then move the heavy cell content out of `rows` instead of cloning it.
    let spans: Vec<Vec<Option<i32>>> = rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            row.cells
                .iter()
                .map(|cell| match cell.vmerge {
                    Some(VMerge::Continue) => None,
                    Some(VMerge::Restart) => {
                        let mut span = 1;
                        for below in rows.iter().skip(row_index + 1) {
                            if below.cells.iter().any(|other| {
                                other.start_col == cell.start_col
                                    && other.vmerge == Some(VMerge::Continue)
                            }) {
                                span += 1;
                            } else {
                                break;
                            }
                        }
                        Some(span)
                    }
                    None => Some(1),
                })
                .collect()
        })
        .collect();
    rows.into_iter()
        .zip(spans)
        .map(|(row, row_spans)| {
            let cells = row
                .cells
                .into_iter()
                .zip(row_spans)
                .filter_map(|(cell, span)| {
                    span.map(|row_span| Cell {
                        attr: Attr::default(),
                        align: cell.align,
                        row_span,
                        col_span: i32::try_from(cell.span).unwrap_or(1).max(1),
                        content: cell.content,
                    })
                })
                .collect();
            Row {
                attr: Attr::default(),
                cells,
            }
        })
        .collect()
}

/// The alignment shared by a column, taken from the first body-row cell that begins in it.
fn column_alignment(rows: &[RowRaw], head_count: usize, column: usize) -> Alignment {
    for row in rows.iter().skip(head_count) {
        for cell in &row.cells {
            if cell.start_col == column && cell.vmerge != Some(VMerge::Continue) {
                return cell.align.clone();
            }
        }
    }
    Alignment::AlignDefault
}

// ---------------------------------------------------------------------------
// Inline assembly
// ---------------------------------------------------------------------------

/// One enclosing inline wrapper. The declaration order is the nesting order applied to a leaf:
/// earlier variants wrap later ones regardless of the order the source turned them on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Wrapper {
    Custom(Text),
    Emph,
    Strong,
    Mark,
    SmallCaps,
    Strikeout,
    Superscript,
    Subscript,
    Underline,
}

impl Wrapper {
    fn wrap(self, children: Vec<Inline>) -> Inline {
        match self {
            Wrapper::Custom(name) => Inline::Span(Box::new(custom_style_attr(&name)), children),
            Wrapper::Emph => Inline::Emph(children),
            Wrapper::Strong => Inline::Strong(children),
            Wrapper::Mark => Inline::Span(Box::new(mark_attr()), children),
            Wrapper::SmallCaps => Inline::SmallCaps(children),
            Wrapper::Strikeout => Inline::Strikeout(children),
            Wrapper::Superscript => Inline::Superscript(children),
            Wrapper::Subscript => Inline::Subscript(children),
            Wrapper::Underline => Inline::Underline(children),
        }
    }
}

/// Extracts a source-code paragraph's text verbatim from its runs, preserving all whitespace and
/// rendering each line break, carriage return, or tab as the character it stands for so the code
/// block's exact layout survives.
fn code_paragraph_text(paragraph: &Element) -> String {
    let mut out = String::new();
    push_code_runs(paragraph, &mut out);
    out
}

fn push_code_runs(element: &Element, out: &mut String) {
    for child in element.elements() {
        match local_name(&child.name) {
            "t" => out.push_str(&child.text()),
            "tab" => out.push('\t'),
            "br" => {
                // A page or column break carries no line of its own; only a text-wrapping break
                // advances to the next code line.
                if child.attr("type").unwrap_or("textWrapping") == "textWrapping" {
                    out.push('\n');
                }
            }
            "cr" => out.push('\n'),
            "noBreakHyphen" => out.push('\u{2011}'),
            "sym" => {
                if let Some(ch) = child
                    .attr("char")
                    .and_then(|code| u32::from_str_radix(code, 16).ok())
                    .and_then(char::from_u32)
                {
                    out.push(ch);
                }
            }
            // Run and paragraph properties hold no text; tracked deletions and field instructions
            // are not part of the rendered code.
            "rPr" | "pPr" | "del" | "delText" | "instrText" => {}
            _ => push_code_runs(child, out),
        }
    }
}

/// The wrapper path implied by a run's formatting, outermost first.
fn wrappers(fmt: &RunFmt) -> Vec<Wrapper> {
    let mut path = Vec::new();
    // `custom` is stored innermost-first; reversed here so the outermost style opens first.
    for name in fmt.custom.iter().rev() {
        path.push(Wrapper::Custom(name.clone()));
    }
    if fmt.italic {
        path.push(Wrapper::Emph);
    }
    if fmt.bold {
        path.push(Wrapper::Strong);
    }
    if fmt.mark {
        path.push(Wrapper::Mark);
    }
    if fmt.smallcaps {
        path.push(Wrapper::SmallCaps);
    }
    if fmt.strike {
        path.push(Wrapper::Strikeout);
    }
    if fmt.superscript {
        path.push(Wrapper::Superscript);
    }
    if fmt.subscript {
        path.push(Wrapper::Subscript);
    }
    if fmt.underline {
        path.push(Wrapper::Underline);
    }
    path
}

/// A leaf produced within a paragraph, tagged with the formatting active when it was emitted.
struct Leaf {
    fmt: RunFmt,
    inline: Inline,
}

/// One open complex field, tracked while its runs stream by. Its `instr` collects the field code
/// (before the `separate`); `result_start` marks where its displayed result begins in `leaves`.
struct FieldFrame {
    instr: String,
    result_start: Option<usize>,
}

/// Assembles a paragraph's inline content, collapsing whitespace and trimming its edges as prose,
/// then nesting formatting wrappers by shared prefix.
#[derive(Default)]
struct InlineBuilder {
    leaves: Vec<Leaf>,
    text: String,
    text_fmt: RunFmt,
    pending_space: Option<RunFmt>,
    has_content: bool,
    fields: Vec<FieldFrame>,
}

impl InlineBuilder {
    /// Whether the builder is inside a complex field's code region, where run content is the field
    /// instruction rather than displayed text and so emits nothing.
    fn in_field_code(&self) -> bool {
        self.fields
            .last()
            .is_some_and(|frame| frame.result_start.is_none())
    }

    /// Opens a complex field. Content up to its `separate` is field code and is suppressed.
    fn field_begin(&mut self) {
        self.fields.push(FieldFrame {
            instr: String::new(),
            result_start: None,
        });
    }

    /// Appends a chunk of the current field's instruction text.
    fn field_instr(&mut self, text: &str) {
        if let Some(frame) = self.fields.last_mut() {
            frame.instr.push_str(text);
        }
    }

    /// Marks the boundary between a field's code and its displayed result.
    fn field_separate(&mut self) {
        self.flush_text();
        self.resolve_space();
        let start = self.leaves.len();
        if let Some(frame) = self.fields.last_mut() {
            frame.result_start = Some(start);
        }
    }

    /// Closes a complex field. A hyperlink or reference field wraps its result in a link; any other
    /// field leaves its result in place as ordinary inlines.
    fn field_end(&mut self) {
        self.flush_text();
        let Some(frame) = self.fields.pop() else {
            return;
        };
        let Some(target) = field_link_target(&frame.instr) else {
            return;
        };
        let start = frame.result_start.unwrap_or(self.leaves.len());
        let start = start.min(self.leaves.len());
        let result = self.leaves.split_off(start);
        let content = build_nested(result);
        if content.is_empty() {
            return;
        }
        self.leaves.push(Leaf {
            fmt: RunFmt::default(),
            inline: Inline::Link(
                Box::default(),
                content,
                Box::new(Target {
                    url: target.into(),
                    title: Text::default(),
                }),
            ),
        });
        self.has_content = true;
    }

    fn push_text(&mut self, fmt: &RunFmt, text: &str) {
        if self.in_field_code() {
            return;
        }
        for ch in text.chars() {
            if is_break_space(ch) {
                self.flush_text();
                self.pending_space = Some(fmt.clone());
            } else {
                self.resolve_space();
                if !self.text.is_empty() && &self.text_fmt != fmt {
                    self.flush_text();
                }
                if self.text.is_empty() {
                    self.text_fmt = fmt.clone();
                }
                self.text.push(ch);
                self.has_content = true;
            }
        }
    }

    fn push_space(&mut self, fmt: &RunFmt) {
        if self.in_field_code() {
            return;
        }
        self.flush_text();
        self.pending_space = Some(fmt.clone());
    }

    fn push_break(&mut self, fmt: &RunFmt) {
        if self.in_field_code() {
            return;
        }
        self.flush_text();
        self.pending_space = None;
        self.leaves.push(Leaf {
            fmt: fmt.clone(),
            inline: Inline::LineBreak,
        });
        self.has_content = true;
    }

    fn push_node(&mut self, fmt: RunFmt, inline: Inline) {
        if self.in_field_code() {
            return;
        }
        self.flush_text();
        self.resolve_space();
        self.leaves.push(Leaf { fmt, inline });
        self.has_content = true;
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.leaves.push(Leaf {
                fmt: self.text_fmt.clone(),
                inline: Inline::Str(text.into()),
            });
        }
    }

    fn resolve_space(&mut self) {
        if let Some(fmt) = self.pending_space.take()
            && self.has_content
        {
            self.leaves.push(Leaf {
                fmt,
                inline: Inline::Space,
            });
        }
    }

    fn finish(mut self) -> Vec<Inline> {
        self.flush_text();
        // A trailing space is dropped: paragraph edges carry no whitespace.
        self.pending_space = None;
        build_nested(self.leaves)
    }
}

/// Nests a flat, formatting-tagged leaf sequence into wrapper inlines. Adjacent leaves are grouped
/// under whichever of a leaf's formats spans the longest unbroken run, factored outermost; each
/// leaf then keeps its remaining formats inside. So a bold-italic run beside a bold run share one
/// outer emphasis-strong split rather than each carrying its own copy.
fn build_nested(leaves: Vec<Leaf>) -> Vec<Inline> {
    let items = leaves
        .into_iter()
        .map(|leaf| (wrappers(&leaf.fmt), leaf.inline))
        .collect();
    build_grouped(items)
}

/// The number of consecutive leading items whose formatting includes `wrapper`.
fn run_length(items: &VecDeque<(Vec<Wrapper>, Inline)>, wrapper: &Wrapper) -> usize {
    let mut len = 0;
    while items
        .get(len)
        .is_some_and(|(path, _)| path.contains(wrapper))
    {
        len += 1;
    }
    len
}

fn build_grouped(mut items: VecDeque<(Vec<Wrapper>, Inline)>) -> Vec<Inline> {
    let mut out = Vec::new();
    loop {
        // Decide how to open the front leaf without holding a borrow across the mutation below.
        let choice = match items.front() {
            None => break,
            Some((path, _)) => match path.first() {
                None => None,
                Some(first) => {
                    let mut best = first.clone();
                    let mut best_len = run_length(&items, &best);
                    for wrapper in path.iter().skip(1) {
                        let len = run_length(&items, wrapper);
                        if len > best_len {
                            best_len = len;
                            best = wrapper.clone();
                        }
                    }
                    Some((best, best_len))
                }
            },
        };
        match choice {
            // An unformatted leaf contributes its inline directly.
            None => {
                if let Some((_, inline)) = items.pop_front() {
                    out.push(inline);
                }
            }
            // Peel the run this wrapper spans, dropping the wrapper from each member and nesting the
            // rest inside it.
            Some((wrapper, len)) => {
                let mut group = VecDeque::new();
                for _ in 0..len {
                    if let Some((mut path, inline)) = items.pop_front() {
                        path.retain(|candidate| *candidate != wrapper);
                        group.push_back((path, inline));
                    }
                }
                out.extend(wrap_factored(wrapper, build_grouped(group)));
            }
        }
    }
    out
}

/// Whether an inline carries no formatting of its own and so may sit outside an enclosing wrapper
/// rather than inside it. Only an inter-word space qualifies; a line break belongs to the span it
/// falls in and stays inside.
fn is_neutral(inline: &Inline) -> bool {
    matches!(inline, Inline::Space)
}

/// Wraps `inner` in `wrapper`, but lifts any leading and trailing formatting-neutral inlines out as
/// siblings so a span never opens or closes on a bare space. An all-neutral body drops the wrapper
/// entirely.
fn wrap_factored(wrapper: Wrapper, mut inner: Vec<Inline>) -> Vec<Inline> {
    let lead = inner.iter().take_while(|item| is_neutral(item)).count();
    let trail = inner
        .iter()
        .rev()
        .take_while(|item| is_neutral(item))
        .count()
        .min(inner.len() - lead);
    let trailing = inner.split_off(inner.len() - trail);
    let mut out: Vec<Inline> = inner.drain(..lead).collect();
    if !inner.is_empty() {
        out.push(wrapper.wrap(inner));
    }
    out.extend(trailing);
    out
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Reads the tri-state run toggles from an `rPr` element.
fn read_toggles(properties: &Element) -> RunToggles {
    let mut toggles = RunToggles::default();
    for child in properties.elements() {
        let on = !matches!(child.attr("val"), Some("false" | "0" | "off" | "none"));
        match local_name(&child.name) {
            "b" => toggles.bold = Some(on),
            "i" => toggles.italic = Some(on),
            "u" => toggles.underline = Some(on),
            "strike" | "dstrike" => toggles.strike = Some(on),
            "smallCaps" => toggles.smallcaps = Some(on),
            "highlight" => toggles.mark = Some(on),
            "vertAlign" => match child.attr("val") {
                Some("superscript") => toggles.superscript = Some(true),
                Some("subscript") => toggles.subscript = Some(true),
                _ => {
                    toggles.superscript = Some(false);
                    toggles.subscript = Some(false);
                }
            },
            _ => {}
        }
    }
    toggles
}

/// A paragraph's net left indent in twips: the left (or start) margin less any hanging indent. A
/// first-line indent does not shift the block edge and is excluded. Only a paragraph's own `ind`
/// counts; an indent inherited from its style does not.
fn net_left_indent(properties: Option<&Element>) -> i32 {
    let Some(ind) = properties.and_then(|pr| pr.child("ind")) else {
        return 0;
    };
    let left = ind
        .attr("left")
        .or_else(|| ind.attr("start"))
        .and_then(parse_int)
        .unwrap_or(0);
    let hanging = ind.attr("hanging").and_then(parse_int).unwrap_or(0);
    left.saturating_sub(hanging)
}

/// Reads a list level's marker configuration from a `w:lvl` element.
fn read_level(lvl: &Element) -> LevelDef {
    let num_fmt = lvl
        .child("numFmt")
        .and_then(|element| element.attr("val"))
        .unwrap_or("decimal");
    let lvl_text = lvl
        .child("lvlText")
        .and_then(|element| element.attr("val"))
        .unwrap_or("");
    let start = lvl
        .child("start")
        .and_then(|element| element.attr("val"))
        .and_then(parse_int)
        .unwrap_or(1);
    LevelDef {
        style: number_style(num_fmt),
        delim: number_delim(lvl_text),
        start,
    }
}

/// Applies any per-level start overrides a concrete `w:num` declares.
fn apply_level_overrides(num: &Element, levels: &mut BTreeMap<i32, LevelDef>) {
    for override_element in num.elements() {
        if local_name(&override_element.name) != "lvlOverride" {
            continue;
        }
        let Some(ilvl) = override_element.attr("ilvl").and_then(parse_int) else {
            continue;
        };
        if let Some(start) = override_element
            .child("startOverride")
            .and_then(|element| element.attr("val"))
            .and_then(parse_int)
            && let Some(level) = levels.get_mut(&ilvl)
        {
            level.start = start;
        }
    }
}

/// Maps an OOXML number format to a list numeral style; a bullet or unnumbered level has none.
fn number_style(num_fmt: &str) -> Option<ListNumberStyle> {
    match num_fmt {
        "bullet" | "none" => None,
        "decimal" | "decimalZero" => Some(ListNumberStyle::Decimal),
        "upperRoman" => Some(ListNumberStyle::UpperRoman),
        "lowerRoman" => Some(ListNumberStyle::LowerRoman),
        "upperLetter" => Some(ListNumberStyle::UpperAlpha),
        "lowerLetter" => Some(ListNumberStyle::LowerAlpha),
        _ => Some(ListNumberStyle::DefaultStyle),
    }
}

/// Reads the marker delimiter from a level's format text (`%1.` → period, `%1)` → one-paren,
/// `(%1)` → two-parens).
fn number_delim(lvl_text: &str) -> ListNumberDelim {
    let trimmed = lvl_text.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        ListNumberDelim::TwoParens
    } else if trimmed.ends_with(')') {
        ListNumberDelim::OneParen
    } else {
        ListNumberDelim::Period
    }
}

/// Indexes note bodies by id, skipping the separator pseudo-notes.
fn index_notes(root: &Element, tag: &str, out: &mut BTreeMap<String, Element>) {
    for note in root.elements() {
        if local_name(&note.name) != tag {
            continue;
        }
        if matches!(
            note.attr("type"),
            Some("separator" | "continuationSeparator" | "continuationNotice")
        ) {
            continue;
        }
        if let Some(id) = note.attr("id") {
            out.insert(id.to_owned(), note.clone());
        }
    }
}

/// A custom style's name rendered as a heading class: interior spaces become hyphens, case is kept.
fn style_class(name: &str) -> Text {
    name.replace(' ', "-").into()
}

/// The `custom-style` attribute wrapper for a named Word style.
fn custom_style_attr(name: &str) -> Attr {
    Attr {
        id: Text::default(),
        classes: Vec::new(),
        attributes: vec![("custom-style".into(), name.into())],
    }
}

/// The attribute wrapper for highlighted text: a span carrying the `mark` class.
fn mark_attr() -> Attr {
    Attr {
        id: Text::default(),
        classes: vec!["mark".into()],
        attributes: Vec::new(),
    }
}

/// The link destination a complex field's instruction points to, or `None` when the field is not a
/// link. A `HYPERLINK` links to its address (with `\l` adding a fragment); a `REF` or `PAGEREF`
/// links to its bookmark only when the `\h` switch requests a hyperlink.
fn field_link_target(instr: &str) -> Option<String> {
    let tokens = tokenize_field(instr);
    let (name, rest) = tokens.split_first()?;
    match name.to_ascii_uppercase().as_str() {
        "HYPERLINK" => {
            let mut url: Option<&str> = None;
            let mut anchor: Option<&str> = None;
            let mut index = 0;
            while let Some(token) = rest.get(index) {
                match token.strip_prefix('\\') {
                    // `\l` gives an in-document anchor; `\o` and `\t` carry an argument to ignore.
                    Some("l") => {
                        anchor = rest.get(index + 1).map(String::as_str);
                        index += 2;
                    }
                    Some("o" | "t") => index += 2,
                    Some(_) => index += 1,
                    None => {
                        if url.is_none() {
                            url = Some(token);
                        }
                        index += 1;
                    }
                }
            }
            let mut target = url.unwrap_or_default().to_owned();
            if let Some(anchor) = anchor {
                target.push('#');
                target.push_str(anchor);
            }
            (!target.is_empty()).then_some(target)
        }
        "REF" | "PAGEREF" => {
            let hyperlink = rest.iter().any(|token| token.eq_ignore_ascii_case("\\h"));
            if !hyperlink {
                return None;
            }
            let bookmark = rest.iter().find(|token| !token.starts_with('\\'))?;
            Some(format!("#{bookmark}"))
        }
        _ => None,
    }
}

/// Splits a field instruction into its type keyword, switches, and arguments. Whitespace separates
/// tokens except inside double quotes, where `\"` is a literal quote and `\\` a literal backslash.
fn tokenize_field(instr: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = instr.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        let mut token = String::new();
        if ch == '"' {
            chars.next();
            while let Some(ch) = chars.next() {
                match ch {
                    '"' => break,
                    '\\' => match chars.peek() {
                        Some('"' | '\\') => {
                            if let Some(escaped) = chars.next() {
                                token.push(escaped);
                            }
                        }
                        _ => token.push('\\'),
                    },
                    other => token.push(other),
                }
            }
        } else {
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() || ch == '"' {
                    break;
                }
                token.push(ch);
                chars.next();
            }
        }
        tokens.push(token);
    }
    tokens
}

/// Reads a drawing's `wp:extent` into `width`/`height` attributes measured in inches.
fn image_attr(drawing: &Element) -> Attr {
    let mut attributes = Vec::new();
    if let Some(extent) = drawing.descendant("extent") {
        if let Some(cx) = extent
            .attr("cx")
            .and_then(|value| value.parse::<i64>().ok())
            && cx > 0
        {
            attributes.push(("width".into(), emu_to_inches(cx).into()));
        }
        if let Some(cy) = extent
            .attr("cy")
            .and_then(|value| value.parse::<i64>().ok())
            && cy > 0
        {
            attributes.push(("height".into(), emu_to_inches(cy).into()));
        }
    }
    Attr {
        id: Text::default(),
        classes: Vec::new(),
        attributes,
    }
}

/// Formats an English Metric Unit length as inches (914400 EMU per inch). A whole-number result
/// carries an explicit `.0` fractional part, so the shortest round-tripping decimal always shows a
/// decimal point.
#[allow(clippy::cast_precision_loss)]
fn emu_to_inches(emu: i64) -> String {
    let mut digits = format!("{}", emu as f64 / 914_400.0);
    if !digits.contains('.') {
        digits.push_str(".0");
    }
    format!("{digits}in")
}

/// The proportion `value / total` as a fraction, for column-width ratios.
#[allow(clippy::cast_precision_loss)]
fn ratio(value: i64, total: i64) -> f64 {
    value as f64 / total as f64
}

/// A default page's printable width in twips (a US-Letter page less one-inch side margins), the
/// baseline a table's column-width fractions are measured against, independent of the document's own
/// declared page geometry.
const DEFAULT_TEXT_WIDTH_TWIPS: i64 = 9360;

/// The width allowance deducted for each boundary between grid columns when sizing table columns.
const INTER_COLUMN_TWIPS: i64 = 10;

/// Splits plain text into `Str`/`Space` inlines, collapsing whitespace and trimming the edges.
fn tokenize_inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut first = true;
    for word in text.split_whitespace() {
        if !first {
            out.push(Inline::Space);
        }
        out.push(Inline::Str(word.into()));
        first = false;
    }
    out
}

/// Lowercases and trims a style's display name for classification.
fn canonical_style(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Whether a canonical paragraph-style name marks a caption, whose content folds into an adjacent
/// image (as a figure) or table.
fn is_caption_style(canonical: &str) -> bool {
    matches!(canonical, "caption" | "image caption" | "table caption")
}

/// Whether an inline sequence is exactly one image and nothing else, so its paragraph can become a
/// figure.
fn single_image(inlines: &[Inline]) -> bool {
    matches!(inlines, [Inline::Image(..)])
}

/// Whether a character is prose whitespace that collapses to a single inter-word space. Only the
/// ASCII space, tab, and the two line-ending characters fold; every other space character — the
/// non-breaking space and the fixed-width Unicode spaces among them — is literal text and is carried
/// through verbatim.
fn is_break_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
}

/// The heading level a canonical style name denotes, if any (`heading 3` → level 3).
fn heading_level(canonical: &str) -> Option<i32> {
    let rest = canonical.strip_prefix("heading ")?;
    let level = rest.trim().parse::<i32>().ok()?;
    (1..=9).contains(&level).then_some(level)
}

/// Whether a canonical style name is one the reader gives dedicated block semantics, so the `styles`
/// extension leaves it alone rather than wrapping it in a `custom-style` container.
fn is_builtin_style(canonical: &str) -> bool {
    heading_level(canonical).is_some()
        || matches!(
            canonical,
            "" | "normal"
                | "body text"
                | "first paragraph"
                | "compact"
                | "title"
                | "subtitle"
                | "author"
                | "date"
                | "abstract"
                | "quote"
                | "block text"
                | "intense quote"
                | "block quote"
                | "source code"
                | "verbatim char"
                | "hyperlink"
                | "footnote text"
                | "footnote reference"
        )
}

fn alignment(value: &str) -> Alignment {
    match value {
        "left" | "both" => Alignment::AlignLeft,
        "right" => Alignment::AlignRight,
        "center" => Alignment::AlignCenter,
        _ => Alignment::AlignDefault,
    }
}

fn truthy(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true" | "on"))
}

/// Whether a `w:tblLook` requests first-row conditional formatting. Two encodings coexist: the packed
/// hex bitmask carried by `@w:val`, whose `0x0020` bit selects the first row, and the older boolean
/// `@w:firstRow` attribute. Either one asserting the flag promotes the first row to a header.
fn table_look_first_row(look: &Element) -> bool {
    const FIRST_ROW_BIT: u16 = 0x0020;
    let from_mask = look
        .attr("val")
        .and_then(|val| u16::from_str_radix(val.trim(), 16).ok())
        .is_some_and(|bits| bits & FIRST_ROW_BIT != 0);
    from_mask || truthy(look.attr("firstRow"))
}

fn parse_int(value: &str) -> Option<i32> {
    value.trim().parse::<i32>().ok()
}

/// A relationship target reduced to the document-relative media path, dropping any `word/` prefix or
/// `../` segments so it names the media-bag key.
fn normalize_target(target: &str) -> String {
    let trimmed = target.trim_start_matches("./");
    let trimmed = trimmed.trim_start_matches("../");
    trimmed.strip_prefix("word/").unwrap_or(trimmed).to_owned()
}

/// Resolves a relationship target against a base directory into an archive part name.
fn normalize_part(target: &str, base: &str) -> String {
    let cleaned = target.trim_start_matches("./");
    if cleaned.starts_with("../") {
        return cleaned.trim_start_matches("../").to_owned();
    }
    if base.is_empty() || cleaned.starts_with("word/") || cleaned.contains(":/") {
        cleaned.to_owned()
    } else {
        format!("{base}{cleaned}")
    }
}

/// A conservative MIME type from a media path's extension. Recognized image types come from the
/// shared table; the legacy metafile formats and the fallbacks (an unrecognized extension keeps its
/// own `image/*` subtype, an extensionless path is treated as opaque binary) are docx-specific.
fn mime_for(path: &str) -> String {
    if let Some(mime) = carta_core::media::image_mime_for_extension(path) {
        return mime.to_owned();
    }
    match path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
    {
        Some(ext) => match ext.as_str() {
            "emf" => "image/x-emf".to_owned(),
            "wmf" => "image/x-wmf".to_owned(),
            other => format!("image/{other}"),
        },
        None => "application/octet-stream".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Legacy pictorial-font substitution
// ---------------------------------------------------------------------------

/// A legacy font whose printable-ASCII slots hold glyphs unrelated to the letters' code points, so a
/// run styled with it must have its text remapped to the Unicode characters those glyphs stand for.
#[derive(Debug, Clone, Copy)]
enum SymbolFont {
    Symbol,
    Wingdings,
}

impl SymbolFont {
    /// The Unicode replacement for a single character, or `None` when the character is kept as-is:
    /// either it lies outside the printable-ASCII range the font remaps, or the font leaves that slot
    /// unassigned (an empty table entry), in which case the original character stands.
    fn map(self, ch: char) -> Option<&'static str> {
        let code = ch as u32;
        if !(0x20..=0x7E).contains(&code) {
            return None;
        }
        let index = (code - 0x20) as usize;
        let table = match self {
            SymbolFont::Symbol => &SYMBOL_TABLE,
            SymbolFont::Wingdings => &WINGDINGS_TABLE,
        };
        table.get(index).copied().filter(|slot| !slot.is_empty())
    }

    /// Remaps every character of a run's text to its Unicode equivalent.
    fn substitute(self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            match self.map(ch) {
                Some(replacement) => out.push_str(replacement),
                None => out.push(ch),
            }
        }
        out
    }
}

/// The legacy pictorial font a run's properties select through their `rFonts` ascii or high-ANSI
/// slot, if any. The complex-script slot is not consulted: it governs a separate script run.
fn symbol_font(properties: &Element) -> Option<SymbolFont> {
    let fonts = properties.child("rFonts")?;
    for slot in ["ascii", "hAnsi"] {
        match fonts.attr(slot) {
            Some("Symbol") => return Some(SymbolFont::Symbol),
            Some("Wingdings") => return Some(SymbolFont::Wingdings),
            _ => {}
        }
    }
    None
}

/// Adobe Symbol's printable-ASCII slots (`0x20`–`0x7E`) mapped to the Unicode characters they render.
#[rustfmt::skip]
static SYMBOL_TABLE: [&str; 95] = [
    "\u{a0}", "!", "\u{2200}", "#", "\u{2203}", "%", "&", "\u{220b}", "(", ")", "\u{2217}", "+",
    ",", "\u{2212}", ".", "/", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", ":", ";", "<",
    "=", ">", "?", "\u{2245}", "\u{391}", "\u{392}", "\u{3a7}", "\u{2206}", "\u{395}", "\u{3a6}",
    "\u{393}", "\u{397}", "\u{399}", "\u{3d1}", "\u{39a}", "\u{39b}", "\u{39c}", "\u{39d}",
    "\u{39f}", "\u{3a0}", "\u{398}", "\u{3a1}", "\u{3a3}", "\u{3a4}", "\u{3a5}", "\u{3c2}",
    "\u{2126}", "\u{39e}", "\u{3a8}", "\u{396}", "[", "\u{2234}", "]", "\u{22a5}", "_", "\u{f8e5}",
    "\u{3b1}", "\u{3b2}", "\u{3c7}", "\u{3b4}", "\u{3b5}", "\u{3c6}", "\u{3b3}", "\u{3b7}",
    "\u{3b9}", "\u{3d5}", "\u{3ba}", "\u{3bb}", "\u{3bc}", "\u{3bd}", "\u{3bf}", "\u{3c0}",
    "\u{3b8}", "\u{3c1}", "\u{3c3}", "\u{3c4}", "\u{3c5}", "\u{3d6}", "\u{3c9}", "\u{3be}",
    "\u{3c8}", "\u{3b6}", "{", "|", "}", "\u{223c}",
];

/// Wingdings' printable-ASCII slots (`0x20`–`0x7E`) mapped to the Unicode characters they render.
#[rustfmt::skip]
static WINGDINGS_TABLE: [&str; 95] = [
    "", "\u{1f589}", "\u{2702}", "\u{2701}", "\u{1f453}", "\u{1f56d}", "\u{1f56e}", "\u{1f56f}",
    "\u{1f57f}", "\u{2706}", "\u{1f582}", "\u{1f583}", "\u{1f4ea}", "\u{1f4eb}", "\u{1f4ec}",
    "\u{1f4ed}", "\u{1f4c1}", "\u{1f4c2}", "\u{1f4c4}", "\u{1f5cf}", "\u{1f5d0}", "\u{1f5c4}",
    "\u{231b}", "\u{1f5ae}", "\u{1f5b0}", "\u{1f5b2}", "\u{1f5b3}", "\u{1f5b4}", "\u{1f5ab}",
    "\u{1f5ac}", "\u{2707}", "\u{270d}", "\u{1f58e}", "\u{270c}", "\u{1f44c}", "\u{1f44d}",
    "\u{1f44e}", "\u{261c}", "\u{261e}", "\u{261d}", "\u{261f}", "\u{1f590}", "\u{263a}",
    "\u{1f610}", "\u{2639}", "\u{1f4a3}", "\u{2620}", "\u{1f3f3}", "\u{1f3f1}", "\u{2708}",
    "\u{263c}", "\u{1f4a7}", "\u{2744}", "\u{1f546}", "\u{271e}", "\u{1f548}", "\u{2720}",
    "\u{2721}", "\u{262a}", "\u{262f}", "\u{950}", "\u{2638}", "\u{2648}", "\u{2649}", "\u{264a}",
    "\u{264b}", "\u{264c}", "\u{264d}", "\u{264e}", "\u{264f}", "\u{2650}", "\u{2651}", "\u{2652}",
    "\u{2653}", "\u{1f670}", "\u{1f675}", "\u{25cf}", "\u{1f53e}", "\u{25a0}", "\u{25a1}",
    "\u{1f790}", "\u{2751}", "\u{2752}", "\u{2b27}", "\u{29eb}", "\u{25c6}", "\u{2756}",
    "\u{2b25}", "\u{2327}", "\u{2bb9}", "\u{2318}", "\u{1f3f5}", "\u{1f3f6}", "\u{1f676}",
    "\u{1f677}",
];

// ---------------------------------------------------------------------------
// ASCII folding for the ascii_identifiers extension
// ---------------------------------------------------------------------------

/// Transliterates text to ASCII: an accented Latin letter folds to its unaccented base, plain ASCII
/// is kept, and any other character is dropped. Covers the Latin-1 Supplement and Latin Extended-A
/// ranges that dominate Western text.
fn fold_to_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else if let Some(base) = ascii_base(ch) {
            out.push(base);
        }
    }
    out
}

#[allow(clippy::match_same_arms)]
fn ascii_base(ch: char) -> Option<char> {
    let base = match ch {
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'Ā' | 'ā' | 'Ă'
        | 'ă' | 'Ą' | 'ą' => 'a',
        'Ç' | 'ç' | 'Ć' | 'ć' | 'Ĉ' | 'ĉ' | 'Ċ' | 'ċ' | 'Č' | 'č' => 'c',
        'Ď' | 'ď' | 'Đ' | 'đ' => 'd',
        'È' | 'É' | 'Ê' | 'Ë' | 'è' | 'é' | 'ê' | 'ë' | 'Ē' | 'ē' | 'Ĕ' | 'ĕ' | 'Ė' | 'ė' | 'Ę'
        | 'ę' | 'Ě' | 'ě' => 'e',
        'Ĝ' | 'ĝ' | 'Ğ' | 'ğ' | 'Ġ' | 'ġ' | 'Ģ' | 'ģ' => 'g',
        'Ĥ' | 'ĥ' | 'Ħ' | 'ħ' => 'h',
        'Ì' | 'Í' | 'Î' | 'Ï' | 'ì' | 'í' | 'î' | 'ï' | 'Ĩ' | 'ĩ' | 'Ī' | 'ī' | 'Ĭ' | 'ĭ' | 'Į'
        | 'į' | 'İ' | 'ı' => 'i',
        'Ĵ' | 'ĵ' => 'j',
        'Ķ' | 'ķ' => 'k',
        'Ĺ' | 'ĺ' | 'Ļ' | 'ļ' | 'Ľ' | 'ľ' | 'Ł' | 'ł' => 'l',
        'Ñ' | 'ñ' | 'Ń' | 'ń' | 'Ņ' | 'ņ' | 'Ň' | 'ň' => 'n',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'Ō' | 'ō' | 'Ŏ' | 'ŏ' | 'Ő'
        | 'ő' | 'Ø' | 'ø' => 'o',
        'Ŕ' | 'ŕ' | 'Ŗ' | 'ŗ' | 'Ř' | 'ř' => 'r',
        'Ś' | 'ś' | 'Ŝ' | 'ŝ' | 'Ş' | 'ş' | 'Š' | 'š' => 's',
        'Ţ' | 'ţ' | 'Ť' | 'ť' | 'Ŧ' | 'ŧ' => 't',
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'ù' | 'ú' | 'û' | 'ü' | 'Ũ' | 'ũ' | 'Ū' | 'ū' | 'Ŭ' | 'ŭ' | 'Ů'
        | 'ů' | 'Ű' | 'ű' | 'Ų' | 'ų' => 'u',
        'Ŵ' | 'ŵ' => 'w',
        'Ý' | 'ý' | 'ÿ' | 'Ŷ' | 'ŷ' | 'Ÿ' => 'y',
        'Ź' | 'ź' | 'Ż' | 'ż' | 'Ž' | 'ž' => 'z',
        _ => return None,
    };
    Some(base)
}

// ---------------------------------------------------------------------------
// OMML → TeX
// ---------------------------------------------------------------------------

/// Renders an Office `MathML` element to a TeX string. The core constructs — fractions, scripts,
/// radicals, n-ary operators, delimiters, functions, accents, bars, matrices, and limits — are
/// mapped directly; anything unmodeled falls back to its rendered child content so no math is lost.
fn omml_to_tex(element: &Element) -> String {
    let mut out = String::new();
    render_math_children(element, &mut out);
    out
}

fn render_math_children(element: &Element, out: &mut String) {
    for child in element.elements() {
        render_math(child, out);
    }
}

// The explicit transparent-wrapper arm mirrors the wildcard on purpose: it documents which elements
// are known pass-throughs versus merely unhandled.
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn render_math(element: &Element, out: &mut String) {
    match local_name(&element.name) {
        "r" | "t" => out.push_str(&map_math_text(&element.text())),
        "f" => {
            out.push_str("\\frac");
            push_group(element.child("num"), out);
            push_group(element.child("den"), out);
        }
        "sSup" => {
            push_base(element.child("e"), out);
            out.push('^');
            push_group(element.child("sup"), out);
        }
        "sSub" => {
            push_base(element.child("e"), out);
            out.push('_');
            push_group(element.child("sub"), out);
        }
        "sSubSup" => {
            push_base(element.child("e"), out);
            out.push('_');
            push_group(element.child("sub"), out);
            out.push('^');
            push_group(element.child("sup"), out);
        }
        "sPre" => {
            out.push_str("{}");
            out.push('_');
            push_group(element.child("sub"), out);
            out.push('^');
            push_group(element.child("sup"), out);
            push_base(element.child("e"), out);
        }
        "rad" => {
            let degree = element.child("deg").map(render_element).unwrap_or_default();
            if degree.is_empty() {
                out.push_str("\\sqrt");
            } else {
                out.push_str("\\sqrt[");
                out.push_str(&degree);
                out.push(']');
            }
            push_group(element.child("e"), out);
        }
        "nary" => render_nary(element, out),
        "d" => render_delimiter(element, out),
        "func" => {
            let name = element
                .child("fName")
                .map(render_element)
                .unwrap_or_default();
            out.push_str(&map_function(&name));
            out.push(' ');
            out.push_str(&element.child("e").map(render_element).unwrap_or_default());
        }
        "acc" => {
            let chr = element
                .child("accPr")
                .and_then(|pr| pr.child("chr"))
                .and_then(|element| element.attr("val"))
                .and_then(|value| value.chars().next())
                .unwrap_or('\u{0302}');
            out.push_str(accent_command(chr));
            push_group(element.child("e"), out);
        }
        "bar" => {
            let top = element
                .child("barPr")
                .and_then(|pr| pr.child("pos"))
                .and_then(|element| element.attr("val"))
                == Some("top");
            out.push_str(if top { "\\overline" } else { "\\underline" });
            push_group(element.child("e"), out);
        }
        "groupChr" => render_group_char(element, out),
        "m" => render_matrix(element, out),
        "limLow" => {
            push_base(element.child("e"), out);
            out.push('_');
            push_group(element.child("lim"), out);
        }
        "limUpp" => {
            push_base(element.child("e"), out);
            out.push('^');
            push_group(element.child("lim"), out);
        }
        "eqArr" => render_equation_array(element, out),
        // Boxes, phantoms, and other transparent wrappers contribute their content.
        "e" | "box" | "borderBox" | "num" | "den" | "sup" | "sub" | "deg" | "lim" | "fName"
        | "oMath" => render_math_children(element, out),
        _ => render_math_children(element, out),
    }
}

fn render_nary(element: &Element, out: &mut String) {
    let properties = element.child("naryPr");
    let chr = properties
        .and_then(|pr| pr.child("chr"))
        .and_then(|c| c.attr("val"))
        .and_then(|value| value.chars().next());
    out.push_str(nary_command(chr));
    let sub = element.child("sub").map(render_element).unwrap_or_default();
    if !sub.is_empty() {
        out.push('_');
        out.push('{');
        out.push_str(&sub);
        out.push('}');
    }
    let sup = element.child("sup").map(render_element).unwrap_or_default();
    if !sup.is_empty() {
        out.push('^');
        out.push('{');
        out.push_str(&sup);
        out.push('}');
    }
    out.push_str(&element.child("e").map(render_element).unwrap_or_default());
}

fn render_delimiter(element: &Element, out: &mut String) {
    let properties = element.child("dPr");
    let sep = properties
        .and_then(|pr| pr.child("sepChr"))
        .and_then(|c| c.attr("val"));
    // A missing fence defaults to a parenthesis; an explicitly empty one is a null delimiter.
    let beg = properties
        .and_then(|pr| pr.child("begChr"))
        .and_then(|c| c.attr("val"))
        .unwrap_or("(");
    let end = properties
        .and_then(|pr| pr.child("endChr"))
        .and_then(|c| c.attr("val"))
        .unwrap_or(")");
    let bodies: Vec<&Element> = element
        .elements()
        .filter(|child| local_name(&child.name) == "e")
        .collect();
    let rendered: Vec<String> = bodies.iter().map(|body| render_element(body)).collect();

    // Parentheses, brackets and single bars stay unsized around short, flat content; anything taller
    // than a run, multiple compartments, or a fence that must scale (braces, floors, angles, …) is
    // wrapped in `\left … \right` so it grows with its content.
    let sized = rendered.len() > 1
        || !plain_delimiter(beg)
        || !plain_delimiter(end)
        || bodies.iter().any(|body| tall_math(body));

    if !sized {
        let open = delimiter_token(beg);
        out.push_str(&open);
        if control_word(&open) {
            out.push(' ');
        }
        if let Some(inner) = rendered.first() {
            out.push_str(inner);
        }
        out.push_str(&delimiter_token(end));
        return;
    }

    out.push_str("\\left");
    out.push_str(&delimiter_token(beg));
    out.push(' ');
    for (index, inner) in rendered.iter().enumerate() {
        if index > 0 {
            // A bar separator scales with `\middle`; any other character is written literally, since
            // `\middle` only accepts a delimiter.
            match sep {
                None | Some("|") => out.push_str(" \\middle| "),
                Some(other) => out.push_str(other),
            }
        }
        out.push_str(inner);
    }
    out.push_str(" \\right");
    out.push_str(&delimiter_token(end));
}

/// Whether a fence character renders unsized when it surrounds short, flat content. The scalable
/// fences (braces, floors, ceilings, angles, double bars, the null delimiter) are excluded so they
/// always take `\left … \right`.
fn plain_delimiter(chr: &str) -> bool {
    matches!(chr, "(" | ")" | "[" | "]" | "|")
}

/// Whether a delimiter token is a control word, i.e. needs a following space so it does not run into
/// the next token (`\lbrack x`, not `\lbrackx`).
fn control_word(token: &str) -> bool {
    token.starts_with('\\')
        && token
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphabetic())
}

/// Whether an `m:e` compartment holds anything taller than a run of text, which forces an enclosing
/// delimiter to scale. Fractions, scripts, radicals, nested delimiters, accents and the like all
/// appear as a child element other than a run.
fn tall_math(body: &Element) -> bool {
    body.elements().any(|child| local_name(&child.name) != "r")
}

/// Renders an equation array (`m:eqArr`): each `m:e` is one right-aligned row of a TeX `array`.
fn render_equation_array(element: &Element, out: &mut String) {
    out.push_str("\\begin{array}{r}\n");
    let rows: Vec<String> = element
        .elements()
        .filter(|child| local_name(&child.name) == "e")
        .map(render_element)
        .collect();
    out.push_str(&rows.join(" \\\\\n"));
    out.push_str("\n\\end{array}");
}

fn render_group_char(element: &Element, out: &mut String) {
    let chr = element
        .child("groupChrPr")
        .and_then(|pr| pr.child("chr"))
        .and_then(|c| c.attr("val"))
        .and_then(|value| value.chars().next());
    let command = match chr {
        Some('\u{23DF}') => "\\underbrace",
        _ => "\\overbrace",
    };
    out.push_str(command);
    push_group(element.child("e"), out);
}

fn render_matrix(element: &Element, out: &mut String) {
    out.push_str("\\begin{matrix}\n");
    let rows: Vec<String> = element
        .elements()
        .filter(|child| local_name(&child.name) == "mr")
        .map(|row| {
            row.elements()
                .filter(|cell| local_name(&cell.name) == "e")
                .map(render_element)
                .collect::<Vec<_>>()
                .join(" & ")
        })
        .collect();
    out.push_str(&rows.join(" \\\\\n"));
    out.push_str("\n\\end{matrix}");
}

/// Renders one element's math content to a fresh string.
fn render_element(element: &Element) -> String {
    let mut out = String::new();
    render_math(element, &mut out);
    out
}

/// Renders an optional element and wraps it in `{}` as a script or fraction group.
fn push_group(element: Option<&Element>, out: &mut String) {
    out.push('{');
    if let Some(element) = element {
        render_math_children(element, out);
    }
    out.push('}');
}

/// Renders an optional base element without added braces.
fn push_base(element: Option<&Element>, out: &mut String) {
    if let Some(element) = element {
        render_math_children(element, out);
    }
}

/// Maps a delimiter character to its TeX token.
fn delimiter_token(chr: &str) -> String {
    match chr {
        "" => ".".to_owned(),
        "(" | ")" | "|" | "/" => chr.to_owned(),
        "[" => "\\lbrack".to_owned(),
        "]" => "\\rbrack".to_owned(),
        "{" => "\\{".to_owned(),
        "}" => "\\}".to_owned(),
        "‖" => "\\|".to_owned(),
        "⟨" => "\\langle".to_owned(),
        "⟩" => "\\rangle".to_owned(),
        "⌊" => "\\lfloor".to_owned(),
        "⌋" => "\\rfloor".to_owned(),
        "⌈" => "\\lceil".to_owned(),
        "⌉" => "\\rceil".to_owned(),
        other => other.to_owned(),
    }
}

/// Maps an n-ary operator character to its TeX command, defaulting to an integral.
#[allow(clippy::match_same_arms)]
fn nary_command(chr: Option<char>) -> &'static str {
    match chr {
        Some('∑') => "\\sum",
        Some('∏') => "\\prod",
        Some('∐') => "\\coprod",
        Some('∫') => "\\int",
        Some('∬') => "\\iint",
        Some('∭') => "\\iiint",
        Some('∮') => "\\oint",
        Some('⋃') => "\\bigcup",
        Some('⋂') => "\\bigcap",
        Some('⋁') => "\\bigvee",
        Some('⋀') => "\\bigwedge",
        Some('⨁') => "\\bigoplus",
        Some('⨂') => "\\bigotimes",
        Some('⨀') => "\\bigodot",
        Some('⨄') => "\\biguplus",
        Some('⨆') => "\\bigsqcup",
        _ => "\\int",
    }
}

/// Maps a combining accent character to its TeX command, defaulting to a wide hat.
#[allow(clippy::match_same_arms)]
fn accent_command(chr: char) -> &'static str {
    match chr {
        '\u{0300}' => "\\grave",
        '\u{0301}' => "\\acute",
        '\u{0302}' => "\\widehat",
        '\u{0303}' => "\\widetilde",
        '\u{0304}' => "\\bar",
        '\u{0305}' => "\\overline",
        '\u{0306}' => "\\breve",
        '\u{0307}' => "\\dot",
        '\u{0308}' => "\\ddot",
        '\u{030A}' => "\\mathring",
        '\u{030C}' => "\\check",
        '\u{20D7}' => "\\vec",
        _ => "\\widehat",
    }
}

/// Recognized math function names rendered as TeX control words.
const KNOWN_FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "sinh", "cosh", "tanh", "coth", "arcsin", "arccos",
    "arctan", "log", "ln", "lg", "exp", "lim", "max", "min", "det", "gcd", "deg", "dim", "hom",
    "ker", "arg", "sup", "inf", "liminf", "limsup",
];

/// Maps a recognized function name to its TeX command, leaving unknown names verbatim.
fn map_function(name: &str) -> String {
    let trimmed = name.trim();
    if KNOWN_FUNCTIONS.contains(&trimmed) {
        format!("\\{trimmed}")
    } else {
        trimmed.to_owned()
    }
}

/// Maps a math run's text, translating a small set of common symbols to TeX commands and passing
/// everything else through. Exotic symbols outside this set are emitted literally.
fn map_math_text(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if let Some(command) = math_symbol(ch) {
            out.push_str(command);
        } else {
            out.push(ch);
        }
    }
    out
}

fn math_symbol(ch: char) -> Option<&'static str> {
    let command = match ch {
        'α' => "\\alpha ",
        'β' => "\\beta ",
        'γ' => "\\gamma ",
        'δ' => "\\delta ",
        'ε' => "\\varepsilon ",
        'ζ' => "\\zeta ",
        'η' => "\\eta ",
        'θ' => "\\theta ",
        'λ' => "\\lambda ",
        'μ' => "\\mu ",
        'π' => "\\pi ",
        'ρ' => "\\rho ",
        'σ' => "\\sigma ",
        'τ' => "\\tau ",
        'φ' => "\\varphi ",
        'χ' => "\\chi ",
        'ψ' => "\\psi ",
        'ω' => "\\omega ",
        'Γ' => "\\Gamma ",
        'Δ' => "\\Delta ",
        'Θ' => "\\Theta ",
        'Λ' => "\\Lambda ",
        'Π' => "\\Pi ",
        'Σ' => "\\Sigma ",
        'Φ' => "\\Phi ",
        'Ψ' => "\\Psi ",
        'Ω' => "\\Omega ",
        '∞' => "\\infty ",
        '×' => "\\times ",
        '÷' => "\\div ",
        '±' => "\\pm ",
        '∓' => "\\mp ",
        '⋅' => "\\cdot ",
        '≤' => "\\leq ",
        '≥' => "\\geq ",
        '≠' => "\\neq ",
        '≈' => "\\approx ",
        '≡' => "\\equiv ",
        '∈' => "\\in ",
        '∉' => "\\notin ",
        '⊂' => "\\subset ",
        '⊆' => "\\subseteq ",
        '∪' => "\\cup ",
        '∩' => "\\cap ",
        '→' => "\\to ",
        '⇒' => "\\Rightarrow ",
        '⇔' => "\\Leftrightarrow ",
        '∂' => "\\partial ",
        '∇' => "\\nabla ",
        '∀' => "\\forall ",
        '∃' => "\\exists ",
        '∅' => "\\emptyset ",
        _ => return None,
    };
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_core::container::zip::ZipArchive;

    /// Packages a bare `word/document.xml` body into a minimal archive the reader accepts, so a
    /// hand-built story can be fed through the full byte-input path.
    fn docx_from_body(body: &str) -> Vec<u8> {
        docx_from_parts(body, None, None)
    }

    /// Packages a `word/document.xml` body alongside optional `styles.xml` and `numbering.xml`
    /// parts. The parts sit at their conventional names, which the reader resolves without any
    /// relationship entries.
    fn docx_from_parts(body: &str, styles: Option<&str>, numbering: Option<&str>) -> Vec<u8> {
        const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
             xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"";
        let document =
            format!("<?xml version=\"1.0\"?><w:document {NS}><w:body>{body}</w:body></w:document>");
        let mut archive = ZipArchive::new();
        archive
            .deflate("word/document.xml", document.as_bytes())
            .expect("store document part");
        if let Some(styles) = styles {
            let xml = format!("<?xml version=\"1.0\"?><w:styles {NS}>{styles}</w:styles>");
            archive
                .deflate("word/styles.xml", xml.as_bytes())
                .expect("store styles part");
        }
        if let Some(numbering) = numbering {
            let xml = format!("<?xml version=\"1.0\"?><w:numbering {NS}>{numbering}</w:numbering>");
            archive
                .deflate("word/numbering.xml", xml.as_bytes())
                .expect("store numbering part");
        }
        archive.finish().expect("finish archive")
    }

    #[test]
    fn deeply_nested_hyperlinks_do_not_overflow_the_stack() {
        // Each nested hyperlink descends one inline-walk frame. Well-formed input can stack these far
        // deeper than any real document, so the walk is depth-bounded; this proves a pathological
        // chain reads to completion instead of exhausting the call stack. The depth sits above the
        // walk's own ceiling yet within the XML scanner's nesting limit, so the parsed tree really is
        // that deep and, before the bound, the recursion overran the stack on an input this size.
        let depth = 2_000;
        let body = format!(
            "<w:p>{}<w:r><w:t>x</w:t></w:r>{}</w:p>",
            "<w:hyperlink w:anchor=\"a\">".repeat(depth),
            "</w:hyperlink>".repeat(depth)
        );
        let archive = docx_from_body(&body);
        assert!(DocxReader.read(&archive, &ReaderOptions::default()).is_ok());
    }

    /// The `word/document.xml` body of `depth` tables nested one inside another, innermost cell
    /// holding a single marker paragraph.
    fn nested_tables(depth: usize) -> String {
        let mut body = String::from("<w:p><w:r><w:t>core</w:t></w:r></w:p>");
        for _ in 0..depth {
            body = format!(
                "<w:tbl><w:tblGrid><w:gridCol w:w=\"5000\"/></w:tblGrid>\
                 <w:tr><w:tc>{body}</w:tc></w:tr></w:tbl>"
            );
        }
        body
    }

    /// The deepest cell content of a chain of singly-nested tables, and how many tables were
    /// descended to reach it.
    fn descend_tables(blocks: &[Block]) -> (usize, &[Block]) {
        match blocks.first() {
            Some(Block::Table(table)) => {
                match table
                    .bodies
                    .first()
                    .and_then(|section| section.body.first())
                    .and_then(|row| row.cells.first())
                {
                    Some(cell) => {
                        let (deeper, content) = descend_tables(&cell.content);
                        (deeper + 1, content)
                    }
                    None => (0, blocks),
                }
            }
            _ => (0, blocks),
        }
    }

    #[test]
    fn deeply_nested_tables_are_preserved_to_the_scanner_ceiling() {
        // A thousand tables nested one inside another sit just under the scanner's nesting ceiling,
        // so every level survives and the innermost paragraph is intact — the block tree is not
        // silently shortened. Reading converts the body on a generously sized stack of its own;
        // walking and dropping a structure this deep is what would otherwise overrun the slim
        // test-runner stack, so the assertions run on a roomy stack too.
        let outcome = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let archive = docx_from_body(&nested_tables(1000));
                let document = DocxReader
                    .read(&archive, &ReaderOptions::default())
                    .expect("read deeply nested tables");
                let (depth, innermost) = descend_tables(&document.blocks);
                (
                    depth,
                    innermost == [Block::Plain(vec![Inline::Str("core".into())])],
                )
            })
            .expect("spawn worker")
            .join()
            .expect("join worker");
        assert_eq!(outcome, (1000, true));
    }

    #[test]
    fn custom_styled_list_items_carry_a_style_container_only_under_the_styles_extension() {
        let styles = "<w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\">\
             <w:name w:val=\"List Paragraph\"/></w:style>";
        let numbering = "<w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\">\
             <w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"o\"/></w:lvl></w:abstractNum>\
             <w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>";
        let item = |text: &str| {
            format!(
                "<w:p><w:pPr><w:pStyle w:val=\"ListParagraph\"/>\
                 <w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr>\
                 <w:r><w:t>{text}</w:t></w:r></w:p>"
            )
        };
        let body = format!("{}{}", item("alpha"), item("beta"));
        let archive = docx_from_parts(&body, Some(styles), Some(numbering));

        let plain = |text: &str| Block::Para(vec![Inline::Str(text.into())]);
        let default = DocxReader
            .read(&archive, &ReaderOptions::default())
            .expect("read without styles");
        assert_eq!(
            default.blocks,
            vec![Block::BulletList(vec![
                vec![plain("alpha")],
                vec![plain("beta")],
            ])]
        );

        let mut options = ReaderOptions::default();
        options.extensions.insert(Extension::Styles);
        let with_styles = DocxReader
            .read(&archive, &options)
            .expect("read with styles");
        let wrapped = |text: &str| {
            Block::Div(
                Box::new(custom_style_attr("List Paragraph")),
                vec![plain(text)],
            )
        };
        assert_eq!(
            with_styles.blocks,
            vec![Block::BulletList(vec![
                vec![wrapped("alpha")],
                vec![wrapped("beta")],
            ])]
        );
    }

    #[test]
    fn run_toggle_off_value_disables_the_property() {
        let run = |toggle: &str, text: &str| {
            format!("<w:p><w:r><w:rPr>{toggle}</w:rPr><w:t>{text}</w:t></w:r></w:p>")
        };
        let body = format!(
            "{}{}{}",
            run("<w:b w:val=\"off\"/>", "off"),
            run("<w:b w:val=\"false\"/>", "false"),
            run("<w:b/>", "on"),
        );
        let document = DocxReader
            .read(&docx_from_body(&body), &ReaderOptions::default())
            .expect("read toggle runs");
        assert_eq!(
            document.blocks,
            vec![
                Block::Para(vec![Inline::Str("off".into())]),
                Block::Para(vec![Inline::Str("false".into())]),
                Block::Para(vec![Inline::Strong(vec![Inline::Str("on".into())])]),
            ]
        );
    }

    #[test]
    fn tbl_look_hex_val_first_row_bit_promotes_the_header() {
        let table = |look: &str| {
            format!(
                "<w:tbl><w:tblPr><w:tblLook w:val=\"{look}\"/></w:tblPr>\
                 <w:tblGrid><w:gridCol w:w=\"100\"/></w:tblGrid>\
                 <w:tr><w:tc><w:p><w:r><w:t>H</w:t></w:r></w:p></w:tc></w:tr>\
                 <w:tr><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"
            )
        };
        let head_rows = |look: &str| {
            let body = table(look);
            let document = DocxReader
                .read(&docx_from_body(&body), &ReaderOptions::default())
                .expect("read table");
            match document.blocks.first() {
                Some(Block::Table(t)) => t.head.rows.len(),
                other => panic!("expected a table, found {other:?}"),
            }
        };
        // Bit 0x0020 of the packed look bitmask selects the first row for header promotion.
        assert_eq!(head_rows("04A0"), 1);
        // The same bitmask with that bit clear leaves every row in the body.
        assert_eq!(head_rows("0480"), 0);
    }
}
