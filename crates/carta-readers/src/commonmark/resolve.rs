//! Block-resolution phase: walk the block-phase IR tree, parse each leaf's raw text through the
//! inline phase, and assemble the final AST blocks — headings with attributes and identifiers,
//! tables with captions, task-list markers, and figure promotion.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell as TableCell, ColSpec, ColWidth, Inline, Row, Table,
    TableBody, TableFoot, TableHead,
};
use carta_core::{Extension, Extensions};

use super::attr;
use super::identifiers::HeaderNumbering;
use super::inline::{char_before, parse_inlines};
use super::scan::normalize_label;
use super::{ExampleMap, FootnoteDefs, IrBlock, LinkDef, RefMap, para, plain};
use crate::inline_scan::is_unicode_whitespace;

/// The empty checkbox emitted for an unchecked task-list item (`- [ ]`).
pub(super) const TASK_UNCHECKED: &str = "\u{2610}";
/// The checked checkbox emitted for a checked task-list item (`- [x]`).
pub(super) const TASK_CHECKED: &str = "\u{2612}";

/// Document-level reference context threaded through the inline phase.
///
/// A footnote reference `[^label]` resolves only when `label` is in `defined`. At the top level it
/// becomes a `Note` carrying the matching content from `by_id`; inside a definition's own body it
/// collapses to an empty string rather than nesting another note. An example reference `@label`
/// resolves to its number from `examples`.
#[derive(Clone, Copy)]
pub(super) struct RefContext<'a> {
    pub(super) defined: &'a BTreeSet<String>,
    pub(super) by_id: &'a BTreeMap<String, Vec<Block>>,
    pub(super) in_definition: bool,
    /// The markdown dialect, where a backslash-escaped space becomes a non-breaking space, a
    /// superscript or subscript span may not hold an unescaped space, and a code span's content is
    /// stripped of all surrounding whitespace. The strict dialect leaves each of these alone.
    pub(super) markdown: bool,
    pub(super) examples: &'a ExampleMap,
    /// A running count of the citation groups resolved so far. Every `Cite` inline is stamped with
    /// the next value, and all the `Citation` entries inside one group share that number. The count
    /// is threaded across the whole document so the value rises in reading order. A citation nested
    /// in another's affixes advances the count as it is built, so the enclosing group ends up
    /// stamped with the highest number it contains.
    pub(super) cite_count: &'a Cell<i32>,
}

/// One heading's inline parse, cached under its raw (unparsed) content string, keyed with a queue
/// so repeated identical heading text pops its own matching entry rather than reusing one that
/// belongs to a different occurrence.
pub(super) type HeaderParseCache = BTreeMap<String, VecDeque<Vec<Inline>>>;

/// Whether a heading's raw content parses identically under every `RefContext` this reader ever
/// builds. Reference links, footnote references, and citations are the only inline constructs
/// that consult `RefContext`'s reference-scoped fields (`by_id`, `defined`'s effect on notes,
/// `cite_count`, and `refs` itself), and they are triggered only by `[`, `^`, and `@`
/// respectively. Content with none of those characters is safe to parse once and reuse.
pub(super) fn heading_content_is_context_independent(content: &str) -> bool {
    !content.contains(['[', '@', '^'])
}

/// Resolve the whole document: collect the headings reachable by implicit reference, then each
/// footnote definition's body (where nested references collapse to empty), then the body itself
/// (where references become notes).
pub(crate) fn resolve_document(
    ir: &[IrBlock],
    mut refs: RefMap,
    footnotes: &FootnoteDefs,
    examples: &ExampleMap,
    ext: Extensions,
    markdown: bool,
) -> Vec<Block> {
    let defined: BTreeSet<String> = footnotes.keys().cloned().collect();
    let empty = BTreeMap::new();
    // Heading reference-gathering and footnote-body resolution each run a separate count so that
    // pre-parsing them does not advance the body's citation numbering. The body carries its own
    // count, raised in reading order across the whole document body.
    let scratch_count = Cell::new(0);
    let body_count = Cell::new(0);
    // Headings whose content is context-independent (see `heading_content_is_context_independent`)
    // are parsed once here and reused by the body pass instead of being parsed twice.
    let mut header_parse_cache: HeaderParseCache = BTreeMap::new();
    // Headings register their references up front so a reference resolves to a heading anywhere in
    // the document, including one that appears later or inside a footnote definition.
    if ext.contains(Extension::ImplicitHeaderReferences) {
        let probe = RefContext {
            defined: &defined,
            by_id: &empty,
            in_definition: false,
            markdown,
            examples,
            cite_count: &scratch_count,
        };
        register_header_references(ir, &mut refs, probe, ext, &mut header_parse_cache);
    }
    let in_def = RefContext {
        defined: &defined,
        by_id: &empty,
        in_definition: true,
        markdown,
        examples,
        cite_count: &scratch_count,
    };
    let by_id: BTreeMap<String, Vec<Block>> = footnotes
        .iter()
        .map(|(key, body)| {
            (
                key.clone(),
                resolve_blocks(body, &refs, in_def, ext, &mut header_parse_cache),
            )
        })
        .collect();
    let top = RefContext {
        defined: &defined,
        by_id: &by_id,
        in_definition: false,
        markdown,
        examples,
        cite_count: &body_count,
    };
    let mut blocks = resolve_blocks(ir, &refs, top, ext, &mut header_parse_cache);
    super::identifiers::assign_header_identifiers(&mut blocks, ext, markdown);
    blocks
}

/// Walk the block tree in reading order, registering one reference definition per heading: its
/// source text, normalized like any reference label, paired with a `#id` target. Headings are
/// numbered with the same algorithm that later assigns their `attr` ids, so the two agree. An
/// already-defined label is left untouched, so an explicit definition outranks a heading and, among
/// headings, the first with a given label wins — while every heading still advances the numbering.
fn register_header_references(
    ir: &[IrBlock],
    refs: &mut RefMap,
    notes: RefContext,
    ext: Extensions,
    cache: &mut HeaderParseCache,
) {
    let mut numbering = HeaderNumbering::new(ext, notes.markdown);
    gather_headers(ir, refs, notes, ext, &mut numbering, cache);
}

pub(super) fn gather_headers(
    ir: &[IrBlock],
    refs: &mut RefMap,
    notes: RefContext,
    ext: Extensions,
    numbering: &mut HeaderNumbering,
    cache: &mut HeaderParseCache,
) {
    for block in ir {
        match block {
            IrBlock::Heading(_, text) => {
                let (content, attr) = split_header_attr(text, ext);
                let inlines = parse_inlines(content, refs, notes, ext);
                let id = numbering.id_for(&attr.id, &inlines);
                refs.entry(normalize_label(content)).or_insert(LinkDef {
                    url: format!("#{id}"),
                    title: String::new(),
                });
                if heading_content_is_context_independent(content) {
                    cache
                        .entry(content.to_owned())
                        .or_default()
                        .push_back(inlines);
                }
            }
            IrBlock::Div(_, children) | IrBlock::BlockQuote(children) => {
                gather_headers(children, refs, notes, ext, numbering, cache);
            }
            IrBlock::BulletList(items) | IrBlock::OrderedList(_, items) => {
                for item in items {
                    gather_headers(item, refs, notes, ext, numbering, cache);
                }
            }
            IrBlock::DefinitionList(items) => {
                for item in items {
                    for definition in &item.definitions {
                        gather_headers(definition, refs, notes, ext, numbering, cache);
                    }
                }
            }
            _ => {}
        }
    }
}

fn resolve_blocks(
    ir: &[IrBlock],
    refs: &RefMap,
    notes: RefContext,
    ext: Extensions,
    cache: &mut HeaderParseCache,
) -> Vec<Block> {
    let mut out = Vec::with_capacity(ir.len());
    for block in ir {
        resolve_block(block, refs, notes, ext, cache, &mut out);
    }
    out
}

pub(super) fn resolve_block(
    block: &IrBlock,
    refs: &RefMap,
    notes: RefContext,
    ext: Extensions,
    cache: &mut HeaderParseCache,
    out: &mut Vec<Block>,
) {
    match block {
        IrBlock::Para(text) => {
            let inlines = parse_inlines(text, refs, notes, ext);
            out.push(para_or_figure(inlines, ext));
        }
        IrBlock::Plain(text) => out.push(plain(parse_inlines(text, refs, notes, ext))),
        IrBlock::Heading(level, text) => {
            let (content, attr) = split_header_attr(text, ext);
            // A heading's content that is safe to cache (see
            // `heading_content_is_context_independent`) was already parsed once by the reference
            // pre-pass; reuse that parse instead of running the inline scan again.
            let inlines = if heading_content_is_context_independent(content) {
                cache
                    .get_mut(content)
                    .and_then(VecDeque::pop_front)
                    .unwrap_or_else(|| parse_inlines(content, refs, notes, ext))
            } else {
                parse_inlines(content, refs, notes, ext)
            };
            out.push(Block::Header(*level, Box::new(attr), inlines));
        }
        IrBlock::CodeBlock(attr, text) => {
            out.push(Block::CodeBlock(
                Box::new(attr.clone()),
                text.clone().into(),
            ));
        }
        IrBlock::RawHtml(text) => {
            // In the Markdown dialect with `raw_html` off an HTML block degrades to an ordinary
            // paragraph of its literal text; the inline pass keeps the tags as text. The bare
            // CommonMark engine always emits a raw HTML block.
            if notes.markdown && !ext.contains(Extension::RawHtml) {
                out.push(Block::Para(parse_inlines(text, refs, notes, ext)));
            } else {
                out.push(Block::RawBlock(
                    carta_ast::Format("html".into()),
                    text.clone().into(),
                ));
            }
        }
        IrBlock::RawBlock(format, text) => {
            out.push(Block::RawBlock(format.clone(), text.clone().into()));
        }
        IrBlock::ThematicBreak => out.push(Block::HorizontalRule),
        IrBlock::Div(attr, children) => {
            out.push(Block::Div(
                Box::new(attr.clone()),
                resolve_blocks(children, refs, notes, ext, cache),
            ));
        }
        IrBlock::BlockQuote(children) => {
            out.push(Block::BlockQuote(resolve_blocks(
                children, refs, notes, ext, cache,
            )));
        }
        IrBlock::LineBlock(lines) => out.push(Block::LineBlock(
            lines
                .iter()
                .map(|line| parse_inlines(line, refs, notes, ext))
                .collect(),
        )),
        IrBlock::DefinitionList(items) => out.push(Block::DefinitionList(
            items
                .iter()
                .map(|item| {
                    let term = parse_inlines(&item.term, refs, notes, ext);
                    let definitions = item
                        .definitions
                        .iter()
                        .map(|blocks| resolve_blocks(blocks, refs, notes, ext, cache))
                        .collect();
                    (term, definitions)
                })
                .collect(),
        )),
        IrBlock::BulletList(items) => resolve_bullet_list(items, refs, notes, ext, cache, out),
        IrBlock::OrderedList(attrs, items) => out.push(Block::OrderedList(
            attrs.clone(),
            items
                .iter()
                .map(|i| resolve_blocks(i, refs, notes, ext, cache))
                .collect(),
        )),
        IrBlock::Table {
            alignments,
            header,
            rows,
            caption,
            attr,
        } => out.push(resolve_table(
            alignments,
            header,
            rows,
            caption.as_deref(),
            attr,
            refs,
            notes,
            ext,
        )),
        IrBlock::GridTable(table) => out.push(resolve_grid_table(table, refs, notes, ext)),
        IrBlock::TextTable(table) => out.push(resolve_text_table(table, refs, notes, ext)),
    }
}

/// Rewrite an image-only paragraph into a `Figure` when `implicit_figures` is enabled.
///
/// The trigger is exact: the paragraph's sole inline is an `Image` whose alt-text list is
/// non-empty. The image's identifier is hoisted onto the figure; its classes and key/value
/// attributes stay on the image. The caption is a clone of the alt inlines wrapped in a `Plain`,
/// and the image (with its id cleared) is preserved verbatim in the figure body. Anything else —
/// extra inlines, an empty alt, a link- or emphasis-wrapped image — stays an ordinary paragraph.
fn para_or_figure(inlines: Vec<Inline>, ext: Extensions) -> Block {
    if !ext.contains(Extension::ImplicitFigures) {
        return para(inlines);
    }
    let one: Result<[Inline; 1], Vec<Inline>> = inlines.try_into();
    match one {
        Ok([Inline::Image(mut attr, alt, target)]) if !alt.is_empty() => {
            let figure_attr = Attr {
                id: std::mem::take(&mut attr.id),
                classes: Vec::new(),
                attributes: Vec::new(),
            };
            let caption = Caption {
                short: None,
                long: vec![Block::Plain(alt.clone())],
            };
            let image = Inline::Image(attr, alt, target);
            Block::Figure(
                Box::new(figure_attr),
                Box::new(caption),
                vec![Block::Plain(vec![image])],
            )
        }
        Ok([only]) => para(vec![only]),
        Err(inlines) => para(inlines),
    }
}

/// Build a pipe table: column specs from the alignments, the header in a single-row `TableHead`,
/// and the body rows in one `TableBody`. Every cell's trimmed text parses into inlines wrapped in a
/// single `Plain`; an empty cell carries no blocks. A caption, when present, is inline markdown
/// wrapped in a `Plain`; footers, widths, spans, and row-head columns are the empty defaults.
// A pipe table carries its shape as loose fields rather than a struct (unlike grid/text tables), so
// the builder threads them alongside the shared inline-resolution context.
#[allow(clippy::too_many_arguments)]
fn resolve_table(
    alignments: &[Alignment],
    header: &[String],
    rows: &[Vec<String>],
    caption: Option<&str>,
    attr: &Attr,
    refs: &RefMap,
    notes: RefContext,
    ext: Extensions,
) -> Block {
    let col_specs = alignments
        .iter()
        .map(|align| ColSpec {
            align: align.clone(),
            width: ColWidth::ColWidthDefault,
        })
        .collect();
    let make_row = |cells: &[String]| Row {
        attr: Attr::default(),
        cells: cells
            .iter()
            .map(|text| make_cell(text, refs, notes, ext))
            .collect(),
    };
    let caption = match caption {
        Some(text) => make_caption(text, refs, notes, ext),
        None => Caption::default(),
    };
    Block::Table(Box::new(Table {
        attr: attr.clone(),
        caption,
        col_specs,
        head: TableHead {
            attr: Attr::default(),
            rows: vec![make_row(header)],
        },
        bodies: vec![TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: Vec::new(),
            body: rows.iter().map(|cells| make_row(cells)).collect(),
        }],
        foot: TableFoot::default(),
    }))
}

/// Build a table caption from its text. Caption text that parses to nothing (an attribute-only or
/// bare caption line) yields an empty block list, not a `Plain` wrapping an empty inline list.
fn make_caption(text: &str, refs: &RefMap, notes: RefContext, ext: Extensions) -> Caption {
    let inlines = parse_inlines(text, refs, notes, ext);
    Caption {
        short: None,
        long: if inlines.is_empty() {
            Vec::new()
        } else {
            vec![Block::Plain(inlines)]
        },
    }
}

/// Build one table cell. A non-empty cell's text parses into inlines wrapped in a `Plain`; an empty
/// or whitespace-only cell carries an empty block list.
fn make_cell(text: &str, refs: &RefMap, notes: RefContext, ext: Extensions) -> TableCell {
    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![Block::Plain(parse_inlines(text, refs, notes, ext))]
    };
    TableCell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content,
    }
}

/// Build a grid table: column specs carry the per-column alignment and fractional width; the header
/// rows form the `TableHead` and the body rows a single `TableBody`. Each cell's raw text parses as
/// block content. A caption, when present, is inline markdown wrapped in a `Plain`.
fn resolve_grid_table(
    table: &super::grid::GridTable,
    refs: &RefMap,
    notes: RefContext,
    ext: Extensions,
) -> Block {
    let col_specs = table
        .columns
        .iter()
        .map(|column| ColSpec {
            align: column.align.clone(),
            width: ColWidth::ColWidth(column.width),
        })
        .collect();
    let make_row = |row: &super::grid::Row| Row {
        attr: Attr::default(),
        cells: row
            .cells
            .iter()
            .map(|cell| make_grid_cell(cell, ext, notes.markdown))
            .collect(),
    };
    let caption = match &table.caption {
        Some(text) => make_caption(text, refs, notes, ext),
        None => Caption::default(),
    };
    Block::Table(Box::new(Table {
        attr: table.attr.clone(),
        caption,
        col_specs,
        head: TableHead {
            attr: Attr::default(),
            rows: table.head.iter().map(make_row).collect(),
        },
        bodies: vec![TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: Vec::new(),
            body: table.body.iter().map(make_row).collect(),
        }],
        foot: TableFoot {
            attr: Attr::default(),
            rows: table.foot.iter().map(make_row).collect(),
        },
    }))
}

/// Build a dash-ruled table: column specs carry per-column alignment and, when the ruling fixed
/// them, fractional widths; an optional header row forms the `TableHead` and the body rows a single
/// `TableBody`. Each cell's raw text parses as inline content wrapped in a `Plain`, with embedded
/// line breaks becoming soft breaks. A caption, when present, is inline markdown wrapped in a
/// `Plain`.
fn resolve_text_table(
    table: &super::texttable::TextTable,
    refs: &RefMap,
    notes: RefContext,
    ext: Extensions,
) -> Block {
    let col_specs = table
        .columns
        .iter()
        .map(|column| ColSpec {
            align: column.align.clone(),
            width: match column.width {
                Some(width) => ColWidth::ColWidth(width),
                None => ColWidth::ColWidthDefault,
            },
        })
        .collect();
    let make_row = |cells: &[String]| Row {
        attr: Attr::default(),
        cells: cells
            .iter()
            .map(|text| make_cell(text, refs, notes, ext))
            .collect(),
    };
    let head_rows = if table.head.is_empty() {
        Vec::new()
    } else {
        vec![make_row(&table.head)]
    };
    let caption = match &table.caption {
        Some(text) => make_caption(text, refs, notes, ext),
        None => Caption::default(),
    };
    Block::Table(Box::new(Table {
        attr: table.attr.clone(),
        caption,
        col_specs,
        head: TableHead {
            attr: Attr::default(),
            rows: head_rows,
        },
        bodies: vec![TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: Vec::new(),
            body: table.body.iter().map(|cells| make_row(cells)).collect(),
        }],
        foot: TableFoot::default(),
    }))
}

/// Build one grid-table cell, parsing its raw text into block content (tight cells demote their
/// paragraphs to `Plain`).
fn make_grid_cell(cell: &super::grid::Cell, ext: Extensions, markdown: bool) -> TableCell {
    TableCell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content: super::parse_table_cell(&cell.text, cell.tight, ext, markdown),
    }
}

/// Resolve a bullet list, applying the `task_lists` transform when enabled.
///
/// With `task_lists` on, a leading `[ ]`/`[x]`/`[X]` marker on an item's first leaf block becomes a
/// checkbox character, and the list is partitioned into maximal runs of consecutive task / non-task
/// items, each run emitted as its own bullet list. With it off, the items form a single list
/// unchanged.
fn resolve_bullet_list(
    items: &[Vec<IrBlock>],
    refs: &RefMap,
    notes: RefContext,
    ext: Extensions,
    cache: &mut HeaderParseCache,
    out: &mut Vec<Block>,
) {
    let task_lists = ext.contains(Extension::TaskLists);
    let mut run: Vec<Vec<Block>> = Vec::new();
    let mut run_is_task: Option<bool> = None;

    for item in items {
        let marker = if task_lists {
            item.first().and_then(task_marker_block)
        } else {
            None
        };
        let is_task = marker.is_some();
        if run_is_task.is_some_and(|previous| previous != is_task) {
            out.push(Block::BulletList(std::mem::take(&mut run)));
        }
        run_is_task = Some(is_task);
        run.push(resolve_item(item, marker.as_ref(), refs, notes, ext, cache));
    }
    if !run.is_empty() {
        out.push(Block::BulletList(run));
    }
}

/// Resolve a single list item's blocks, substituting `marker` (a first block whose task marker has
/// already been rewritten) for the item's original first block when present.
fn resolve_item(
    item: &[IrBlock],
    marker: Option<&IrBlock>,
    refs: &RefMap,
    notes: RefContext,
    ext: Extensions,
    cache: &mut HeaderParseCache,
) -> Vec<Block> {
    let mut out = Vec::new();
    let mut blocks = item.iter();
    if let Some(first) = blocks.next() {
        resolve_block(marker.unwrap_or(first), refs, notes, ext, cache, &mut out);
    }
    for block in blocks {
        resolve_block(block, refs, notes, ext, cache, &mut out);
    }
    out
}

/// If `block` is a leaf paragraph whose text begins with a task-list marker, return a copy with the
/// marker replaced by its checkbox character; otherwise `None`.
fn task_marker_block(block: &IrBlock) -> Option<IrBlock> {
    match block {
        IrBlock::Para(text) => task_marker_replacement(text).map(IrBlock::Para),
        IrBlock::Plain(text) => task_marker_replacement(text).map(IrBlock::Plain),
        _ => None,
    }
}

/// Replace a leading `[ ]`/`[x]`/`[X]` (followed by a space or end of text) with its checkbox,
/// keeping the remainder; `None` if `text` has no such marker.
pub(super) fn task_marker_replacement(text: &str) -> Option<String> {
    let (marker, rest) = text
        .strip_prefix("[ ]")
        .map(|rest| (TASK_UNCHECKED, rest))
        .or_else(|| text.strip_prefix("[x]").map(|rest| (TASK_CHECKED, rest)))
        .or_else(|| text.strip_prefix("[X]").map(|rest| (TASK_CHECKED, rest)))?;
    if rest.is_empty() || rest.starts_with(' ') {
        Some(format!("{marker}{rest}"))
    } else {
        None
    }
}

/// Split a trailing attribute block off a heading's text when header attributes are enabled,
/// returning the content to parse as inlines and the heading's attribute. The block must be the
/// last non-blank run on the line (`# Title {#id .cls}`); an empty block (`{}`) is left in the text.
/// When `mmd_header_identifiers` is on and no attribute block is present, a trailing `[id]` label is
/// taken as the identifier instead.
pub(super) fn split_header_attr(text: &str, ext: Extensions) -> (&str, Attr) {
    if ext.contains(Extension::HeaderAttributes) || ext.contains(Extension::Attributes) {
        let trimmed = text.trim_end();
        if trimmed.ends_with('}') {
            for (start, ch) in trimmed.char_indices().rev() {
                if ch != '{' {
                    continue;
                }
                // The block must be set off from the heading text by whitespace, else it belongs to
                // the preceding word rather than the heading.
                let preceded_by_space =
                    start == 0 || char_before(trimmed, start).is_some_and(is_unicode_whitespace);
                if preceded_by_space
                    && let Some((attr, end)) = attr::parse_attributes_bytes(trimmed, start)
                    && end == trimmed.len()
                    && attr::is_non_empty(&attr)
                {
                    let content = text.get(..start).unwrap_or(text).trim_end();
                    return (content, attr);
                }
            }
        }
    }
    if ext.contains(Extension::MmdHeaderIdentifiers)
        && let Some((content, id)) = split_mmd_header_id(text)
    {
        return (
            content,
            Attr {
                id: id.into(),
                ..Attr::default()
            },
        );
    }
    (text, Attr::default())
}

/// Split a trailing `[id]` label off a heading's text (`mmd_header_identifiers`), returning the
/// content to keep and the identifier. The label is the last bracket group on the line, its opener
/// reachable without crossing another bracket group's close (so a reference-link tail like
/// `[text][ref]` is not mistaken for it). The identifier is the bracket content lowercased with all
/// whitespace removed; an empty label is still stripped but yields an empty identifier, which then
/// falls to an automatic one.
fn split_mmd_header_id(text: &str) -> Option<(&str, String)> {
    let trimmed = text.trim_end();
    if !trimmed.ends_with(']') {
        return None;
    }
    // The closing `]` is ASCII, so it occupies the final byte of `trimmed`.
    let close = trimmed.len().checked_sub(1)?;
    let mut depth = 0i32;
    let mut open = None;
    for (i, ch) in trimmed.char_indices().rev() {
        match ch {
            ']' => depth += 1,
            '[' => {
                depth -= 1;
                if depth == 0 {
                    open = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let open = open?;
    // A bracket group directly before the label (only whitespace between) makes the pair a
    // reference-link construct, not an identifier.
    let mut before = open;
    while let Some(c) = char_before(trimmed, before) {
        if c.is_whitespace() {
            before -= c.len_utf8();
        } else {
            break;
        }
    }
    if char_before(trimmed, before) == Some(']') {
        return None;
    }
    let id: String = trimmed
        .get(open + 1..close)?
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    let content = text.get(..open).unwrap_or(text).trim_end();
    Some((content, id))
}
