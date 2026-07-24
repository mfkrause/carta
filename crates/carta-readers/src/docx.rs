//! Reader for `WordprocessingML` (`.docx`), the zipped-XML word-processor package.
//!
//! A `.docx` file is a ZIP archive of XML parts. The main story lives in `word/document.xml`; it
//! references companion parts by relationship id (`word/_rels/document.xml.rels`) and by convention:
//! `word/styles.xml` names the paragraph and character styles, `word/numbering.xml` defines list
//! marker shapes, and `word/footnotes.xml` / `word/endnotes.xml` hold note bodies. Embedded images
//! live under `word/media/` and are carried into the media bag.
//!
//! Each `w:p` becomes a block whose kind is decided by its style name: a `heading N` style is a
//! section heading, `Quote` a block quote, `Source Code` a code block, and
//! `Title`/`Author`/`Date`/`Abstract` document metadata, while a
//! paragraph carrying list numbering (`w:numPr`) joins a reconstructed list and everything else is a
//! plain paragraph. Runs (`w:r`) contribute inline content, with each run's properties toggling the
//! emphasis, strong, underline, strike, superscript, subscript, and small-caps wrappers nested in a
//! fixed order. Tables, drawings (images), hyperlinks, note references, and inline `m:oMath` (mapped
//! to TeX) are handled in place. Paragraph text is normalized like prose: whitespace runs collapse to
//! a single space and the leading and trailing edges are trimmed.

use std::collections::BTreeMap;

use carta_ast::{Block, Document, Inline, ListNumberDelim, ListNumberStyle, MetaValue, Text};
use carta_core::container::zip;
use carta_core::{
    BytesReader, DeepStack, Extension, MediaBag, ReaderOptions, Result, on_deep_stack,
};

use crate::heading_ids::IdRegistry;
use crate::xml::{self, Element, local_name};

mod blocks;
mod convert;
mod helpers;
mod inline;
mod omml;
mod symbols;
mod tables;

use helpers::{
    apply_level_overrides, canonical_style, heading_level, index_notes, is_caption_style,
    normalize_part, parse_int, read_level, read_toggles,
};

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
        // A panicked worker is not retried; only spawn failure falls back to the current stack.
        DeepStack::Panicked => (BTreeMap::new(), Vec::new()),
        DeepStack::NotSpawned => Converter::new(parts, options, media).run(),
    }
}

/// Upper bound on element nesting the parser materializes; content deeper than this is folded in
/// without being descended into, so adversarially deep markup cannot exhaust memory. Body conversion
/// runs on a dedicated stack (see [`convert_on_owned_stack`]), so this ceiling is set well above the
/// nesting genuine documents reach (a chain of a thousand tables nested one inside another survives
/// intact) while still bounding the emitted tree to a depth downstream output can carry on a normal
/// application stack.
const MAX_XML_DEPTH: usize = 3072;

// --- Style, numbering, relationship, and note tables ---

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

// --- Converter ---

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
    // True while the leading title block is open; the first non-metadata block closes it.
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
}

#[cfg(test)]
mod tests;
