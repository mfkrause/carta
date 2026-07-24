//! Reader for `OpenDocument` Text (`.odt`), the zipped-XML office document package.
//!
//! An `.odt` file is a ZIP archive of XML parts. The prose lives in `content.xml`; shared style
//! definitions live in `styles.xml`, and embedded images live under `Pictures/` and are carried
//! into the media bag. Each part is unzipped and parsed into an element tree by a small permissive
//! XML scan (the format ships no DTD the reader must honor).
//!
//! Reading proceeds in two stages. First the style tables are indexed from both parts: character
//! styles (`style:family="text"`) contribute their own formatting toggles and name, paragraph
//! styles (`style:family="paragraph"`) contribute a parent link, a left margin, and a name, and list
//! styles (`text:list-style`) describe marker shapes per nesting level. Then the body is walked once.
//!
//! Each paragraph's block kind is decided by resolving its paragraph style through the full parent
//! chain: a style named `Preformatted Text` anywhere in the chain makes a code block, a resolved left
//! margin past the quote threshold makes a block quote (except directly inside a list item, where that
//! margin is the list's own indentation), and everything else is a plain paragraph; consecutive quote
//! or code paragraphs merge. A heading (`text:h`) becomes a section header whose level is its
//! outline level and whose identifier is the slug of its text, disambiguated against every identifier
//! already issued. Character spans toggle the emphasis, strong, strikeout, superscript, and subscript
//! wrappers (nested in a fixed order) from the directly referenced style's own properties, while a
//! span named `Source Text` becomes inline code. Lists, tables, hyperlinks, note references, and
//! framed images are handled in place. Paragraph text collapses each run of ASCII whitespace to a
//! single space, with a run containing a line ending becoming a soft break; every other space-like
//! character is content and survives verbatim.

use std::collections::BTreeMap;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MathType, Row, Table, TableBody, TableFoot, TableHead,
    Target, Text, slug,
};
use carta_core::container::zip;
use carta_core::{BytesReader, DeepStack, Error, MediaBag, ReaderOptions, Result, on_deep_stack};

use crate::heading_ids::IdRegistry;
use crate::xml::{self, Element, Node, local_name};

mod support;

use support::{
    apply_wrappers, coalesce_text, compact, decode_style_name, empty_span, figure_paragraph,
    flush_code, flush_quote, formula_part_path, inlines_to_plain, intern_anchor, is_drawing_shape,
    map_delim, map_number_style, parse_length, push_text, read_text_props, row_width, square_rows,
};

/// The most columns a table grid is allowed to span. Far wider than any authored table, this bounds
/// the column vector so a document declaring an enormous column repeat cannot exhaust memory.
const MAX_TABLE_COLUMNS: i32 = 10_000;

/// Upper bound on the number of spaces a single `<text:s>` run expands to, so a document declaring a
/// pathological repeat count cannot exhaust memory. Set well above any run real prose contains.
const MAX_REPEATED_SPACES: usize = 100_000;

/// Parses an `OpenDocument` Text package into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct OdtReader;

impl BytesReader for OdtReader {
    fn read(&self, input: &[u8], options: &ReaderOptions) -> Result<Document> {
        Ok(self.read_media(input, options)?.0)
    }

    fn read_media(&self, input: &[u8], _options: &ReaderOptions) -> Result<(Document, MediaBag)> {
        let parts = zip::read_map(input)?;
        let mut media = MediaBag::new();
        let blocks = convert_on_owned_stack(&parts, &mut media)?;
        Ok((
            Document {
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
    media: &mut MediaBag,
) -> Result<Vec<Block>> {
    match on_deep_stack(|| Converter::new(parts, media).run()) {
        DeepStack::Completed(blocks) => blocks,
        // A panicked worker poisons its join; only an unspawnable thread is retried on this stack.
        DeepStack::Panicked => Err(Error::Container("worker thread failed".into())),
        DeepStack::NotSpawned => Converter::new(parts, media).run(),
    }
}

/// Upper bound on element nesting the parser materializes; content deeper than this is folded in
/// without being descended into, so adversarially deep markup cannot exhaust memory. Body conversion
/// runs on a dedicated stack (see [`convert_on_owned_stack`]), so this ceiling is set well above the
/// nesting genuine documents reach while still bounding the emitted tree to a depth downstream output
/// can carry on a normal application stack.
const MAX_XML_DEPTH: usize = 3072;

/// The smallest left indent, in inches, at which a paragraph reads as a block quote rather than a
/// merely indented paragraph. Indents at or below this (footnote and table-cell insets, for instance)
/// stay ordinary paragraphs.
const BLOCK_QUOTE_MARGIN_INCHES: f64 = 0.2165;

/// The vertical position a character style declares, which selects a superscript or subscript wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Vertical {
    #[default]
    Baseline,
    Super,
    Sub,
}

/// The formatting a character (`text`-family) style declares in its own properties, plus whether it
/// names inline code. Character styles do not inherit, so only the directly referenced style counts.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default)]
struct TextProps {
    strong: bool,
    emph: bool,
    strike: bool,
    vertical: Vertical,
    code: bool,
}

/// A paragraph (`paragraph`-family) style's own contributions to block classification, before the
/// parent chain is folded in: whether it names preformatted text, its left margin, and its parent.
#[derive(Debug, Clone, Default)]
struct ParaStyle {
    preformatted: bool,
    margin_left: Option<f64>,
    parent: Option<String>,
}

/// A list marker's shape at one nesting level.
#[derive(Debug, Clone)]
enum LevelStyle {
    Bullet,
    Number(ListNumberStyle, ListNumberDelim, i32),
}

/// A list style's marker shapes indexed by nesting level.
#[derive(Debug, Clone, Default)]
struct ListStyle {
    levels: BTreeMap<i32, LevelStyle>,
}

impl ListStyle {
    /// The marker shape for `depth`, falling back to the nearest shallower level and then to any
    /// defined level, so a nested list styled only at its outer levels still finds a marker.
    fn level_for(&self, depth: i32) -> Option<&LevelStyle> {
        if let Some(level) = self.levels.get(&depth) {
            return Some(level);
        }
        if let Some((_, level)) = self.levels.range(..=depth).next_back() {
            return Some(level);
        }
        self.levels.values().next()
    }
}

/// The block role a paragraph style denotes once its parent chain is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParaRole {
    Code,
    Quote,
    Normal,
}

/// Walks the archive parts once, indexing style tables and converting the body.
struct Converter<'a> {
    parts: &'a BTreeMap<String, Vec<u8>>,
    media: &'a mut MediaBag,
    text_styles: BTreeMap<String, TextProps>,
    para_styles: BTreeMap<String, ParaStyle>,
    list_styles: BTreeMap<String, ListStyle>,
    ids: IdRegistry,
    /// Bookmark names mapped to the anchor identifier assigned on first sighting, so repeated uses of
    /// one name resolve to the same anchor.
    bookmarks: BTreeMap<String, String>,
    /// Reference-mark names mapped to the identifier assigned on first sighting, so repeated uses of
    /// one name resolve to the same anchor.
    reference_marks: BTreeMap<String, String>,
    /// The identifier of the heading currently being converted, so a bookmark that merely repeats it
    /// is recognized as the heading's own anchor rather than a distinct one.
    heading_anchor: Option<String>,
}

impl<'a> Converter<'a> {
    fn new(parts: &'a BTreeMap<String, Vec<u8>>, media: &'a mut MediaBag) -> Self {
        Self {
            parts,
            media,
            text_styles: BTreeMap::new(),
            para_styles: BTreeMap::new(),
            list_styles: BTreeMap::new(),
            ids: IdRegistry::default(),
            bookmarks: BTreeMap::new(),
            reference_marks: BTreeMap::new(),
            heading_anchor: None,
        }
    }

    fn run(mut self) -> Result<Vec<Block>> {
        // Shared styles first so a like-named automatic style in the content part overrides.
        if let Some(root) = self
            .parts
            .get("styles.xml")
            .and_then(|b| xml::parse(b, MAX_XML_DEPTH))
        {
            self.index_styles(&root);
        }
        let content = self
            .parts
            .get("content.xml")
            .ok_or_else(|| Error::Container("could not find content.xml".into()))?;
        let content = xml::parse(content, MAX_XML_DEPTH)
            .ok_or_else(|| Error::Container("content.xml is not well-formed XML".into()))?;
        self.index_styles(&content);
        Ok(
            match content.child("body").and_then(|body| body.child("text")) {
                Some(text) => self.convert_body_blocks(text),
                None => Vec::new(),
            },
        )
    }

    fn index_styles(&mut self, root: &Element) {
        for group in root.elements() {
            if !matches!(local_name(&group.name), "automatic-styles" | "styles") {
                continue;
            }
            for style in group.elements() {
                match local_name(&style.name) {
                    "style" => self.index_style(style),
                    "list-style" => self.index_list_style(style),
                    _ => {}
                }
            }
        }
    }

    fn index_style(&mut self, style: &Element) {
        let Some(name) = style.attr("name") else {
            return;
        };
        let decoded = decode_style_name(name);
        match style.attr("family") {
            Some("text") => {
                self.text_styles
                    .insert(name.to_owned(), read_text_props(&decoded, style));
            }
            Some("paragraph") => {
                let margin_left = style
                    .child("paragraph-properties")
                    .and_then(|props| props.attr("margin-left"))
                    .and_then(parse_length);
                self.para_styles.insert(
                    name.to_owned(),
                    ParaStyle {
                        preformatted: decoded == "Preformatted Text",
                        margin_left,
                        parent: style.attr("parent-style-name").map(str::to_owned),
                    },
                );
            }
            _ => {}
        }
    }

    fn index_list_style(&mut self, style: &Element) {
        let Some(name) = style.attr("name") else {
            return;
        };
        let mut levels = BTreeMap::new();
        for level in style.elements() {
            let index = level
                .attr("level")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(1);
            match local_name(&level.name) {
                "list-level-style-bullet" | "list-level-style-image" => {
                    levels.insert(index, LevelStyle::Bullet);
                }
                "list-level-style-number" => {
                    // A number level with no numbering format renders unnumbered, like a bullet.
                    match level.attr("num-format") {
                        None | Some("") => {
                            levels.insert(index, LevelStyle::Bullet);
                        }
                        Some(format) => {
                            let number = map_number_style(Some(format));
                            let delim = map_delim(
                                level.attr("num-prefix").unwrap_or(""),
                                level.attr("num-suffix").unwrap_or(""),
                            );
                            let start = level
                                .attr("start-value")
                                .and_then(|value| value.parse::<i32>().ok())
                                .unwrap_or(1);
                            levels.insert(index, LevelStyle::Number(number, delim, start));
                        }
                    }
                }
                _ => {}
            }
        }
        self.list_styles
            .insert(name.to_owned(), ListStyle { levels });
    }

    /// Resolves a paragraph style's block role by folding its parent chain: preformatted anywhere in
    /// the chain wins, then a positive left margin from the nearest ancestor that declares one.
    fn para_role(&self, style_name: Option<&str>) -> ParaRole {
        let mut preformatted = false;
        let mut margin = None;
        let mut current = style_name;
        // A misauthored parent cycle is bounded so resolution always terminates.
        for _ in 0..64 {
            let Some(name) = current else {
                break;
            };
            let Some(style) = self.para_styles.get(name) else {
                break;
            };
            preformatted = preformatted || style.preformatted;
            if margin.is_none() {
                margin = style.margin_left;
            }
            current = style.parent.as_deref();
        }
        if preformatted {
            ParaRole::Code
        } else if margin.is_some_and(|value| value > BLOCK_QUOTE_MARGIN_INCHES) {
            ParaRole::Quote
        } else {
            ParaRole::Normal
        }
    }

    fn convert_body_blocks(&mut self, container: &Element) -> Vec<Block> {
        self.convert_blocks(container, 1, None, false)
    }

    /// Walks a block container, gathering its children into blocks. `list_depth` and `list_inherited`
    /// describe the context a direct `<text:list>` child sits in: at body level a list starts at
    /// depth 1 with no inherited style, while inside a list item it starts one level deeper and
    /// inherits the enclosing list's style. `in_list_item` is set when the container is a list item:
    /// there a paragraph's left margin encodes the list's own indentation rather than a quote, so the
    /// margin heuristic that flags a block quote in body flow is held back. Preformatted classification
    /// and run merging apply uniformly, so a code paragraph reads the same wherever it appears.
    #[allow(clippy::too_many_lines)]
    fn convert_blocks(
        &mut self,
        container: &Element,
        list_depth: i32,
        list_inherited: Option<&str>,
        in_list_item: bool,
    ) -> Vec<Block> {
        let mut out = Vec::new();
        // Consecutive quote/preformatted paragraphs gather into one block; a different kind flushes.
        let mut quote: Vec<Block> = Vec::new();
        let mut code: Vec<String> = Vec::new();
        for element in container.elements() {
            // Block-anchored drawing shapes are floating layout, not body flow; dropped whole so
            // the transparent-container fallback below cannot lift their prose.
            if is_drawing_shape(&element.name) {
                continue;
            }
            match local_name(&element.name) {
                "p" => {
                    // A paragraph that is entirely a framed, captioned image lifts to a figure.
                    if let Some(figure) =
                        figure_paragraph(element).and_then(|textbox| self.convert_figure(textbox))
                    {
                        flush_code(&mut out, &mut code);
                        flush_quote(&mut out, &mut quote);
                        out.push(figure);
                        continue;
                    }
                    let role = match self.para_role(element.attr("style-name")) {
                        // In a list item a margin is list indentation, not a quote signal.
                        ParaRole::Quote if in_list_item => ParaRole::Normal,
                        role => role,
                    };
                    let inlines = self.convert_inlines(&element.children);
                    match role {
                        ParaRole::Code => {
                            flush_quote(&mut out, &mut quote);
                            code.push(inlines_to_plain(&inlines));
                        }
                        ParaRole::Quote => {
                            flush_code(&mut out, &mut code);
                            quote.push(Block::Para(inlines));
                        }
                        ParaRole::Normal => {
                            flush_code(&mut out, &mut code);
                            flush_quote(&mut out, &mut quote);
                            out.push(Block::Para(inlines));
                        }
                    }
                }
                "h" => {
                    flush_code(&mut out, &mut code);
                    flush_quote(&mut out, &mut quote);
                    out.push(self.convert_heading(element));
                }
                "list" => {
                    flush_code(&mut out, &mut code);
                    flush_quote(&mut out, &mut quote);
                    out.push(self.convert_list(element, list_depth, list_inherited));
                }
                "table" => {
                    flush_code(&mut out, &mut code);
                    flush_quote(&mut out, &mut quote);
                    out.push(self.convert_table(element));
                }
                // No rendered content: stray notes, breaks, declarations, forms, change records;
                // and generated indexes, whose stale cached entries would surface as orphans.
                "soft-page-break" | "sequence-decls" | "forms" | "tracked-changes" | "note"
                | "table-of-content" | "table-of-contents" | "bibliography"
                | "alphabetical-index" | "illustration-index" | "table-index" | "object-index"
                | "user-index" => {}
                // Transparent containers lift their block children in place, dropping nothing.
                _ => {
                    flush_code(&mut out, &mut code);
                    flush_quote(&mut out, &mut quote);
                    out.extend(self.convert_body_blocks(element));
                }
            }
        }
        flush_code(&mut out, &mut code);
        flush_quote(&mut out, &mut quote);
        out
    }

    fn convert_heading(&mut self, element: &Element) -> Block {
        let level = element
            .attr("outline-level")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(1);
        self.heading_anchor = Some(slug(&element.text()));
        let inlines = self.convert_inlines(&element.children);
        self.heading_anchor = None;
        let id = self
            .ids
            .assign_with_separator(slug(&inlines_to_plain(&inlines)), '-');
        Block::Header(
            level,
            Box::new(Attr {
                id: id.into(),
                classes: Vec::new(),
                attributes: Vec::new(),
            }),
            inlines,
        )
    }

    fn convert_list(&mut self, list: &Element, depth: i32, inherited: Option<&str>) -> Block {
        let style_name = list
            .attr("style-name")
            .map(str::to_owned)
            .or_else(|| inherited.map(str::to_owned));
        let level_style = style_name
            .as_deref()
            .and_then(|name| self.list_styles.get(name))
            .and_then(|style| style.level_for(depth))
            .cloned();
        let mut items = Vec::new();
        for child in list.elements() {
            if matches!(local_name(&child.name), "list-item" | "list-header") {
                items.push(self.convert_list_item(child, depth, style_name.as_deref()));
            }
        }
        match level_style {
            Some(LevelStyle::Bullet) => Block::BulletList(items),
            Some(LevelStyle::Number(style, delim, start)) => Block::OrderedList(
                ListAttributes {
                    start,
                    style,
                    delim,
                },
                items,
            ),
            None => Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::DefaultDelim,
                },
                items,
            ),
        }
    }

    fn convert_list_item(
        &mut self,
        item: &Element,
        depth: i32,
        style_name: Option<&str>,
    ) -> Vec<Block> {
        // Like body content, but a left margin is list indentation, not a quote; a nested list
        // starts one level deeper inheriting this list's style.
        compact(self.convert_blocks(item, depth + 1, style_name, true))
    }

    #[allow(clippy::too_many_lines)]
    fn convert_table(&mut self, table: &Element) -> Block {
        let mut header_rows = Vec::new();
        let mut body_rows = Vec::new();
        for child in table.elements() {
            match local_name(&child.name) {
                "table-header-rows" => {
                    for row in child.elements() {
                        if local_name(&row.name) == "table-row" {
                            header_rows.push(self.convert_row(row));
                        }
                    }
                }
                "table-rows" | "table-row-group" => {
                    for row in child.elements() {
                        if local_name(&row.name) == "table-row" {
                            body_rows.push(self.convert_row(row));
                        }
                    }
                }
                "table-row" => body_rows.push(self.convert_row(child)),
                _ => {}
            }
        }
        // Width comes from the widest real row, not the declared columns: a repeat count can be
        // arbitrarily large, and the ceiling keeps it from exhausting memory.
        let columns = row_width(&header_rows)
            .max(row_width(&body_rows))
            .min(MAX_TABLE_COLUMNS);
        // Short rows are squared off with empty trailing cells; columns covered by an overhanging
        // row span are not filled, so a merged cell leaves no spurious placeholder.
        square_rows(&mut header_rows, columns);
        square_rows(&mut body_rows, columns);
        let col_specs = (0..columns)
            .map(|_| ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            })
            .collect();
        Block::Table(Box::new(Table {
            attr: Attr::default(),
            caption: Caption {
                short: None,
                long: Vec::new(),
            },
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: header_rows,
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: body_rows,
            }],
            foot: TableFoot {
                attr: Attr::default(),
                rows: Vec::new(),
            },
        }))
    }

    fn convert_row(&mut self, row: &Element) -> Row {
        let mut cells = Vec::new();
        for child in row.elements() {
            // A covered cell is the shadow of a neighbor's span and carries no content of its own.
            if local_name(&child.name) != "table-cell" {
                continue;
            }
            let col_span = child
                .attr("number-columns-spanned")
                .and_then(|value| value.parse::<i32>().ok())
                .filter(|span| *span > 0)
                .unwrap_or(1);
            let row_span = child
                .attr("number-rows-spanned")
                .and_then(|value| value.parse::<i32>().ok())
                .filter(|span| *span > 0)
                .unwrap_or(1);
            let content = compact(self.convert_body_blocks(child));
            cells.push(Cell {
                attr: Attr::default(),
                align: Alignment::AlignDefault,
                row_span,
                col_span,
                content,
            });
        }
        Row {
            attr: Attr::default(),
            cells,
        }
    }

    fn convert_inlines(&mut self, children: &[Node]) -> Vec<Inline> {
        let mut out = Vec::new();
        for node in children {
            match node {
                Node::Text(text) => push_text(&mut out, text),
                Node::Element(element) => self.convert_inline_element(element, &mut out),
            }
        }
        coalesce_text(&mut out);
        out
    }

    #[allow(clippy::too_many_lines)]
    fn convert_inline_element(&mut self, element: &Element, out: &mut Vec<Inline>) {
        match local_name(&element.name) {
            "span" => self.convert_span(element, out),
            "s" => {
                let count = element
                    .attr("c")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1)
                    .min(MAX_REPEATED_SPACES);
                for _ in 0..count {
                    out.push(Inline::Space);
                }
            }
            "tab" => out.push(Inline::Space),
            "line-break" => out.push(Inline::LineBreak),
            "a" => self.convert_link(element, out),
            "bookmark" | "bookmark-start" => self.push_bookmark(element, out),
            "reference-mark-start" => self.push_reference_mark(element, out),
            "bookmark-ref" | "reference-ref" => self.convert_cross_reference(element, out),
            // Metadata markers with no rendered content.
            "annotation" | "annotation-start" | "annotation-end" | "bookmark-end"
            | "reference-mark" | "reference-mark-end" | "soft-page-break" => {}
            "note" => out.push(self.convert_note(element)),
            "frame" => self.convert_frame(element, out),
            // Unrecognized inline wrappers contribute their display text, so wrapped words survive.
            _ => {
                let inner = self.convert_inlines(&element.children);
                out.extend(inner);
            }
        }
    }

    fn convert_span(&mut self, element: &Element, out: &mut Vec<Inline>) {
        let props = element
            .attr("style-name")
            .and_then(|name| self.text_styles.get(name))
            .copied()
            .unwrap_or_default();
        if props.code {
            let inner = self.convert_inlines(&element.children);
            out.push(Inline::Code(
                Box::default(),
                inlines_to_plain(&inner).into(),
            ));
            return;
        }
        let inner = self.convert_inlines(&element.children);
        out.extend(apply_wrappers(props, inner));
    }

    fn convert_link(&mut self, element: &Element, out: &mut Vec<Inline>) {
        let url = element.attr("href").unwrap_or_default().to_owned();
        let title = element.attr("title").unwrap_or_default().to_owned();
        let inner = self.convert_inlines(&element.children);
        out.push(Inline::Link(
            Box::default(),
            inner,
            Box::new(Target {
                url: url.into(),
                title: title.into(),
            }),
        ));
    }

    /// A bookmark drops its authored name and takes a generated anchor identifier, disambiguated
    /// against every other identifier in the document. A bookmark that merely restates the identifier
    /// of the heading it sits in is that heading's own anchor and takes the bare anchor name without
    /// consuming a fresh one. Otherwise the same authored name resolves to one shared anchor.
    fn push_bookmark(&mut self, element: &Element, out: &mut Vec<Inline>) {
        let name = element.attr("name").unwrap_or_default();
        if self.heading_anchor.as_deref() == Some(name) {
            out.push(empty_span("anchor".to_owned()));
            return;
        }
        let id = self.bookmark_anchor(name);
        out.push(empty_span(id));
    }

    /// The anchor identifier a bookmark name resolves to: a generated `anchor` id assigned on first
    /// sighting and reused for every later use of the same name, so a bookmark and any reference to it
    /// share one target.
    fn bookmark_anchor(&mut self, name: &str) -> String {
        intern_anchor(&mut self.bookmarks, &mut self.ids, name, "anchor")
    }

    /// A reference mark keeps its authored name as its identifier. The same name reused refers to the
    /// same anchor, so it resolves to one identifier assigned once and reused for later occurrences.
    fn push_reference_mark(&mut self, element: &Element, out: &mut Vec<Inline>) {
        let name = element.attr("name").unwrap_or_default();
        let id = self.reference_mark_anchor(name);
        out.push(empty_span(id));
    }

    /// The anchor identifier a reference-mark name resolves to, assigned on first sighting from the
    /// authored name and reused thereafter so a mark and any reference to it share one target.
    fn reference_mark_anchor(&mut self, name: &str) -> String {
        intern_anchor(&mut self.reference_marks, &mut self.ids, name, name)
    }

    /// A cross-reference field (`bookmark-ref` or `reference-ref`) becomes an internal link to the
    /// anchor its target name resolves to, carrying the field's flattened display text as the link
    /// content.
    fn convert_cross_reference(&mut self, element: &Element, out: &mut Vec<Inline>) {
        let name = element.attr("ref-name").unwrap_or_default().to_owned();
        let anchor = if local_name(&element.name) == "reference-ref" {
            self.reference_mark_anchor(&name)
        } else {
            self.bookmark_anchor(&name)
        };
        let inner = self.convert_inlines(&element.children);
        out.push(Inline::Link(
            Box::default(),
            inner,
            Box::new(Target {
                url: format!("#{anchor}").into(),
                title: Text::default(),
            }),
        ));
    }

    /// A note reference becomes a `Note` carrying its body's blocks. A note that supplies only a
    /// citation and no body is kept as an empty note, so its anchor point still separates the text
    /// around it rather than the whole note vanishing.
    fn convert_note(&mut self, element: &Element) -> Inline {
        match element.child("note-body") {
            Some(body) => Inline::Note(self.convert_body_blocks(body)),
            None => Inline::Note(Vec::new()),
        }
    }

    fn convert_frame(&mut self, element: &Element, out: &mut Vec<Inline>) {
        // A formula object is an equation, preferred over the frame's replacement preview bitmap.
        if let Some(object) = element.child("object")
            && let Some(tex) = self.resolve_formula(object)
        {
            out.push(Inline::Math(MathType::DisplayMath, tex.into()));
            return;
        }
        // A directly wrapped image: title from `svg:title`, no alternate text.
        if element.child("image").is_some() {
            let title = element
                .child("title")
                .map(|node| slug(&node.text()))
                .unwrap_or_default();
            if let Some(image) = self.image_from_frame(element, Vec::new(), &title) {
                out.push(image);
            }
            return;
        }
        // A text box holding an image: inline it becomes an image whose alt text is the caption.
        if let Some(textbox) = element.child("text-box")
            && let Some((frame, caption)) = self.figure_image(textbox)
            && let Some(image) = self.image_from_frame(frame, caption, "fig:")
        {
            out.push(image);
        }
        // Other embedded objects have no inline equivalent and degrade to nothing.
    }

    /// The inline image an image frame denotes: the referenced media is carried into the media bag,
    /// the pixel dimensions become attributes, and the caller supplies the alternate text and the
    /// title marker (empty for a plain image, `fig:` for a captioned one).
    fn image_from_frame(
        &mut self,
        frame: &Element,
        alt: Vec<Inline>,
        title: &str,
    ) -> Option<Inline> {
        let image = frame.child("image")?;
        let href = image.attr("href").unwrap_or_default().to_owned();
        self.register_media(&href);
        let mut attributes = Vec::new();
        if let Some(width) = frame.attr("width") {
            attributes.push(("width".into(), width.into()));
        }
        if let Some(height) = frame.attr("height") {
            attributes.push(("height".into(), height.into()));
        }
        Some(Inline::Image(
            Box::new(Attr {
                id: Text::default(),
                classes: Vec::new(),
                attributes,
            }),
            alt,
            Box::new(Target {
                url: href.into(),
                title: title.into(),
            }),
        ))
    }

    /// Carries an image part into the media bag on first reference, so an image survives conversion
    /// even though the raw bytes live in a separate archive entry.
    fn register_media(&mut self, href: &str) {
        if href.is_empty() || self.media.contains(href) {
            return;
        }
        if let Some(bytes) = self.parts.get(href).cloned() {
            let mime = carta_core::media::image_mime_for_extension(href).map(str::to_owned);
            self.media.insert(href.to_owned(), mime, bytes);
        }
    }

    /// The image frame inside a figure text box and the caption that follows it: the first framed
    /// image within the box, paired with the inline content after it in the image's own paragraph.
    /// Content before the image, and any later paragraph in the box, is layout and carries no caption.
    fn figure_image<'b>(&mut self, textbox: &'b Element) -> Option<(&'b Element, Vec<Inline>)> {
        for paragraph in textbox.elements() {
            if local_name(&paragraph.name) != "p" {
                continue;
            }
            let position = paragraph.children.iter().position(|node| {
                matches!(node, Node::Element(child)
                    if local_name(&child.name) == "frame" && child.child("image").is_some())
            });
            if let Some(index) = position
                && let Some(Node::Element(frame)) = paragraph.children.get(index)
            {
                let caption =
                    self.convert_inlines(paragraph.children.get(index + 1..).unwrap_or_default());
                return Some((frame, caption));
            }
        }
        None
    }

    /// A block-level figure built from a figure text box: the framed image becomes the figure body,
    /// and the caption that trails it becomes the figure caption.
    fn convert_figure(&mut self, textbox: &Element) -> Option<Block> {
        let (frame, caption) = self.figure_image(textbox)?;
        let image = self.image_from_frame(frame, caption.clone(), "")?;
        Some(Block::Figure(
            Box::default(),
            Box::new(Caption {
                short: None,
                long: vec![Block::Plain(caption)],
            }),
            vec![Block::Plain(vec![image])],
        ))
    }

    /// The TeX rendering of a formula object's MathML, taken from MathML embedded directly in the
    /// object or, failing that, from the `content.xml` of the sub-object the frame references.
    fn resolve_formula(&self, object: &Element) -> Option<String> {
        if let Some(math) = object.descendant("math") {
            return Some(crate::mathml::to_tex(math));
        }
        let path = formula_part_path(object.attr("href")?);
        let root = xml::parse(self.parts.get(&path)?, MAX_XML_DEPTH)?;
        let math = if local_name(&root.name) == "math" {
            &root
        } else {
            root.descendant("math")?
        };
        Some(crate::mathml::to_tex(math))
    }
}

#[cfg(test)]
mod tests;
