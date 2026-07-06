//! Inline phase: parse the raw text of leaf blocks into inline nodes.
//!
//! Implements the spec's inline algorithm — a left-to-right scan that resolves code spans,
//! autolinks, raw HTML, entities and escapes immediately, records `*`/`_`/`[`/`![` runs on a
//! delimiter stack, resolves links/images at each `]`, and finally collapses emphasis. The raw
//! char-slice scanners it drives (autolinks, HTML tags, entities, link targets) live in `scan`.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell as TableCell, Citation, CitationMode, ColSpec, ColWidth,
    Inline, MathType, QuoteType, Row, Table, TableBody, TableFoot, TableHead, Target, Text,
};
use carta_core::{Extension, Extensions};

use super::attr;
use super::block::is_format_name_char;
use super::identifiers::HeaderNumbering;
use super::scan::{
    escape_uri, is_ascii_punctuation, normalize_label, scan_autolink, scan_entity,
    scan_following_label, scan_html_tag, scan_inline_target, unescape_string,
};
use super::{ExampleMap, FootnoteDefs, IrBlock, LinkDef, RefMap, para, plain};
use crate::emoji;
use crate::inline_scan::{fold_dash_run, fold_ellipsis_run, is_unicode_whitespace};

/// The empty checkbox emitted for an unchecked task-list item (`- [ ]`).
const TASK_UNCHECKED: &str = "\u{2610}";
/// The checked checkbox emitted for a checked task-list item (`- [x]`).
const TASK_CHECKED: &str = "\u{2612}";

/// Document-level reference context threaded through the inline phase.
///
/// A footnote reference `[^label]` resolves only when `label` is in `defined`. At the top level it
/// becomes a `Note` carrying the matching content from `by_id`; inside a definition's own body it
/// collapses to an empty string rather than nesting another note. An example reference `@label`
/// resolves to its number from `examples`.
#[derive(Clone, Copy)]
struct RefContext<'a> {
    defined: &'a BTreeSet<String>,
    by_id: &'a BTreeMap<String, Vec<Block>>,
    in_definition: bool,
    /// The markdown dialect, where a backslash-escaped space becomes a non-breaking space, a
    /// superscript or subscript span may not hold an unescaped space, and a code span's content is
    /// stripped of all surrounding whitespace. The strict dialect leaves each of these alone.
    markdown: bool,
    examples: &'a ExampleMap,
    /// A running count of the citation groups resolved so far. Every `Cite` inline is stamped with
    /// the next value, and all the `Citation` entries inside one group share that number. The count
    /// is threaded across the whole document so the value rises in reading order. A citation nested
    /// in another's affixes advances the count as it is built, so the enclosing group ends up
    /// stamped with the highest number it contains.
    cite_count: &'a Cell<i32>,
}

/// One heading's inline parse, cached under its raw (unparsed) content string, keyed with a queue
/// so repeated identical heading text pops its own matching entry rather than reusing one that
/// belongs to a different occurrence.
type HeaderParseCache = BTreeMap<String, VecDeque<Vec<Inline>>>;

/// Whether a heading's raw content parses identically under every `RefContext` this reader ever
/// builds. Reference links, footnote references, and citations are the only inline constructs
/// that consult `RefContext`'s reference-scoped fields (`by_id`, `defined`'s effect on notes,
/// `cite_count`, and `refs` itself), and they are triggered only by `[`, `^`, and `@`
/// respectively. Content with none of those characters is safe to parse once and reuse.
fn heading_content_is_context_independent(content: &str) -> bool {
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

fn gather_headers(
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

fn resolve_block(
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
fn task_marker_replacement(text: &str) -> Option<String> {
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
fn split_header_attr(text: &str, ext: Extensions) -> (&str, Attr) {
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

/// A node in the in-progress inline list. Delimiter runs stay as nodes until emphasis resolution.
#[derive(Debug, Clone)]
enum Node {
    Text(String),
    Inline(Inline),
    SoftBreak,
    LineBreak,
    Delimiter(Delimiter),
    /// A slot vacated during emphasis resolution: its former content was folded into a wrapping
    /// inline. Emitted by [`process_emphasis`] and dropped by [`collapse`].
    Empty,
}

// The flags are independent properties of a delimiter run, not a state enum.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
struct Delimiter {
    ch: u8,
    count: usize,
    can_open: bool,
    can_close: bool,
    /// Whether this is an image opener (`![`).
    image: bool,
    /// Source index just past a bracket opener, where its raw label text begins. Unused otherwise.
    text_start: usize,
    /// Whether this bracket opener is still eligible to form a link or image. Non-bracket
    /// delimiters leave this `false` (the field is unused for them).
    ///
    /// A `[` opener is deactivated when a link is successfully built whose text span contains
    /// it — a link may not contain another link. On `]`, an inactive opener is popped and
    /// literalized without attempting any link-target parse (spec §6.3, rule 6).
    active: bool,
    /// The citation count at the moment this bracket opened. If the bracket later resolves to a
    /// single citation, any bare citations counted while scanning its interior are discarded along
    /// with their nodes, so the count rewinds to this value first. Unused for non-bracket
    /// delimiters.
    cite_count_at_open: i32,
}

/// Outcome of resolving an explicit link target after a closing `]`.
enum Explicit {
    /// An inline or reference target resolved to this destination, ending at the given position.
    Target(Target, usize),
    /// An explicit reference was present but its label is undefined: not a link.
    Failed,
    /// No explicit target syntax follows; a span or shortcut reference may still apply.
    None,
}

// `notes` (the footnote context) and `nodes` (the in-progress inline list) are distinct concepts
// that unavoidably read alike.
/// Run the gated highlight-mark pass and emphasis resolution over a node list, then collapse it into
/// inlines — the shared finishing sequence for a parsed inline run, a span body, and a link label.
fn resolve_inline_nodes(mut nodes: Vec<Node>, ext: Extensions, markdown: bool) -> Vec<Inline> {
    if ext.contains(Extension::Mark) {
        resolve_mark(&mut nodes, ext, markdown);
    }
    process_emphasis(&mut nodes, 0, ext, markdown);
    collapse(nodes)
}

/// The character beginning at byte offset `at`, or `None` at or past the end of `text`.
fn char_at(text: &str, at: usize) -> Option<char> {
    text.get(at..).and_then(|rest| rest.chars().next())
}

/// The character ending just before byte offset `at`, or `None` at the start of `text`.
fn char_before(text: &str, at: usize) -> Option<char> {
    text.get(..at).and_then(|head| head.chars().next_back())
}

#[allow(clippy::similar_names)]
fn parse_inlines(text: &str, refs: &RefMap, notes: RefContext, ext: Extensions) -> Vec<Inline> {
    let mut parser = InlineParser {
        text,
        pos: 0,
        nodes: Vec::new(),
        refs,
        notes,
        ext,
        bracket_stack: Vec::new(),
        interesting: interesting_chars(ext),
        backtick_runs: None,
    };
    parser.run();
    let mut inlines = resolve_inline_nodes(parser.nodes, ext, notes.markdown);
    if ext.contains(Extension::Autolink) {
        super::autolink::autolink_inlines(&mut inlines, notes.markdown);
    }
    if ext.contains(Extension::NativeSpans) {
        inlines = pair_native_spans(inlines);
    }
    inlines
}

/// Parse standalone text — a document metadata value — into inlines with no reference context, so
/// footnote and example references in the text resolve to nothing. `markdown` selects the markdown
/// dialect's inline rules, matching the dialect the document body is parsed under.
pub(crate) fn parse_meta_inlines(text: &str, ext: Extensions, markdown: bool) -> Vec<Inline> {
    let defined = BTreeSet::new();
    let by_id = BTreeMap::new();
    let examples = ExampleMap::new();
    let refs = RefMap::new();
    let cite_count = Cell::new(0);
    let notes = RefContext {
        defined: &defined,
        by_id: &by_id,
        in_definition: false,
        markdown,
        examples: &examples,
        cite_count: &cite_count,
    };
    parse_inlines(text, &refs, notes, ext)
}

/// A link label may hold at most 999 characters between its brackets (`CommonMark`, "Links"). A
/// UTF-8 character is at most four bytes, so a span longer than this in bytes exceeds that
/// character bound. Reference lookups treat such a span as no label at all — skipping extraction,
/// normalization, and the map lookup — regardless of what definitions exist, which also keeps a
/// close's label work bounded on adversarially nested brackets.
const MAX_LABEL_BYTES: usize = 999 * 4;

struct InlineParser<'a> {
    text: &'a str,
    /// Cursor as a byte offset into `text`; every mutation lands on a UTF-8 character boundary.
    pos: usize,
    nodes: Vec<Node>,
    refs: &'a RefMap,
    notes: RefContext<'a>,
    ext: Extensions,
    /// Indices into `nodes` for each open `[` or `![` delimiter, in parse order. O(1) lookup of
    /// the most recent bracket opener instead of a backward scan through all nodes.
    bracket_stack: Vec<usize>,
    /// For each ASCII code, whether a character can start a syntactic construct under the active
    /// extensions. Everything else is ordinary text a run scan can skip over in one step.
    interesting: [bool; 128],
    /// Byte offsets of every maximal backtick run in `text`, grouped by run length and ascending
    /// within each group. Built lazily on the first code-span attempt (`None` until then, so
    /// backtick-free text pays nothing) and immutable afterwards: it stays valid because `text`
    /// never changes during inline parsing. A feature that mutated the buffer mid-parse would have
    /// to rebuild it.
    backtick_runs: Option<BTreeMap<usize, Vec<usize>>>,
}

/// The ASCII characters that can begin an inline construct under `ext`; every other character is
/// ordinary text. Must stay in lockstep with the dispatch arms in [`InlineParser::run`] — any new
/// arm's trigger character has to be marked here, or the construct becomes unreachable mid-text.
fn interesting_chars(ext: Extensions) -> [bool; 128] {
    let smart = ext.contains(Extension::Smart);
    core::array::from_fn(|code| {
        let Ok(byte) = u8::try_from(code) else {
            return false;
        };
        match char::from(byte) {
            '\\' | '`' | '<' | '&' | '\n' | '*' | '_' | '[' | ']' | '!' => true,
            '$' => ext.contains(Extension::TexMathDollars),
            '~' => ext.contains(Extension::Subscript) || ext.contains(Extension::Strikeout),
            '^' => ext.contains(Extension::InlineNotes) || ext.contains(Extension::Superscript),
            '=' => ext.contains(Extension::Mark),
            '@' => ext.contains(Extension::ExampleLists) || ext.contains(Extension::Citations),
            ':' => ext.contains(Extension::Emoji),
            '\'' | '"' | '-' | '.' => smart,
            _ => false,
        }
    })
}

impl<'a> InlineParser<'a> {
    fn peek(&self) -> Option<char> {
        char_at(self.text, self.pos)
    }

    fn at(&self, offset: usize) -> Option<char> {
        self.text
            .get(self.pos..)
            .and_then(|rest| rest.chars().nth(offset))
    }

    fn is_interesting(&self, ch: char) -> bool {
        usize::try_from(u32::from(ch))
            .ok()
            .and_then(|code| self.interesting.get(code))
            .copied()
            .unwrap_or(false)
    }

    /// Whether the byte at `pos` begins an interesting construct. Every interesting trigger is an
    /// ASCII character, so a byte `>= 128` (any part of a multi-byte character) is never a boundary
    /// this scan stops on — the run scan can advance one byte at a time and still break only on a
    /// character boundary.
    fn interesting_byte(&self, byte: u8) -> bool {
        byte < 128
            && self
                .interesting
                .get(usize::from(byte))
                .copied()
                .unwrap_or(false)
    }

    fn run(&mut self) {
        while let Some(ch) = self.peek() {
            if !self.is_interesting(ch) {
                let start = self.pos;
                let bytes = self.text.as_bytes();
                while let Some(&next) = bytes.get(self.pos) {
                    if self.interesting_byte(next) {
                        break;
                    }
                    self.pos += 1;
                }
                if let Some(run) = self.text.get(start..self.pos) {
                    self.push_str(run);
                }
                continue;
            }
            match ch {
                '\\' => self.backslash(),
                '`' => self.code_span(),
                '$' if self.ext.contains(Extension::TexMathDollars) => self.dollar_math(),
                '<' => self.left_angle(),
                '&' => self.entity(),
                '\n' => self.line_ending(),
                '*' | '_' => self.emphasis_run(ch as u8),
                '~' if self.ext.contains(Extension::Subscript)
                    && self.ext.contains(Extension::ShortSubsuperscripts)
                    && self.try_short_script('~') => {}
                '~' if self.ext.contains(Extension::Subscript)
                    || self.ext.contains(Extension::Strikeout) =>
                {
                    self.emphasis_run(b'~');
                }
                '^' if self.ext.contains(Extension::InlineNotes)
                    && self.at(1) == Some('[')
                    && self.try_inline_note() => {}
                '^' if self.ext.contains(Extension::Superscript)
                    && self.ext.contains(Extension::ShortSubsuperscripts)
                    && self.try_short_script('^') => {}
                '^' if self.ext.contains(Extension::Superscript) => self.emphasis_run(b'^'),
                '=' if self.ext.contains(Extension::Mark) => self.emphasis_run(b'='),
                '@' if self.ext.contains(Extension::ExampleLists)
                    || self.ext.contains(Extension::Citations) =>
                {
                    self.at_sign();
                }
                ':' if self.ext.contains(Extension::Emoji) && self.try_emoji() => {}
                '\'' | '"' if self.ext.contains(Extension::Smart) => self.emphasis_run(ch as u8),
                '-' if self.ext.contains(Extension::Smart) => self.smart_dash(),
                '.' if self.ext.contains(Extension::Smart) => self.smart_ellipsis(),
                '[' => {
                    self.pos += 1;
                    self.push_open_bracket(false);
                }
                '!' if self.at(1) == Some('[') => {
                    self.pos += 2;
                    self.push_open_bracket(true);
                }
                ']' => self.close_bracket(),
                _ => {
                    self.pos += ch.len_utf8();
                    self.push_text(ch);
                }
            }
        }
    }

    fn push_text(&mut self, ch: char) {
        if let Some(Node::Text(text)) = self.nodes.last_mut() {
            text.push(ch);
        } else {
            self.nodes.push(Node::Text(ch.to_string()));
        }
    }

    fn push_str(&mut self, value: &str) {
        if let Some(Node::Text(text)) = self.nodes.last_mut() {
            text.push_str(value);
        } else {
            self.nodes.push(Node::Text(value.to_owned()));
        }
    }

    /// Resolve an `@` at the cursor. An example-list label assigned a number becomes that number; a
    /// well-formed citation key becomes a bare author-in-text `Cite`; anything else leaves the `@`
    /// as literal text, so the rest of the run reparses normally.
    fn at_sign(&mut self) {
        if self.ext.contains(Extension::ExampleLists) && self.try_example_ref() {
            return;
        }
        if self.ext.contains(Extension::Citations) && self.try_bare_citation() {
            return;
        }
        self.pos += 1;
        self.push_text('@');
    }

    /// Try an example-list reference `@label` at the cursor. A label assigned a number by an example
    /// item is replaced with that number and the cursor advances past it, returning `true`. An
    /// undefined or empty label leaves the cursor in place and returns `false`.
    fn try_example_ref(&mut self) -> bool {
        // The `@` and every label character (`[0-9A-Za-z_-]`) are ASCII, so byte and character
        // offsets coincide across the label.
        let name_start = self.pos + 1;
        let mut end = name_start;
        while matches!(
            char_at(self.text, end),
            Some('0'..='9' | 'a'..='z' | 'A'..='Z' | '-' | '_')
        ) {
            end += 1;
        }
        if end == name_start {
            return false;
        }
        let Some(label) = self.text.get(name_start..end) else {
            return false;
        };
        if let Some(number) = self.notes.examples.get(label) {
            self.pos = end;
            self.push_str(&number.to_string());
            return true;
        }
        false
    }

    /// Try a bare author-in-text citation `@key` at the cursor (which sits on the `@`). It forms a
    /// citation only when the `@` is not glued to a preceding word character and a well-formed key
    /// follows. On success the cursor advances past the key, the running citation count rises, and a
    /// single-entry `Cite` is pushed whose fallback text is the literal `@key`. Returns `false`
    /// (without advancing) otherwise, leaving the `@` for literal handling.
    fn try_bare_citation(&mut self) -> bool {
        if matches!(char_before(self.text, self.pos), Some(c) if is_citation_word(c)) {
            return false;
        }
        let Some((id, next)) = scan_citation_id(self.text, self.pos + 1) else {
            return false;
        };
        let note_num = self.bump_cite_count();
        self.pos = next;
        let citation = Citation {
            id: id.clone().into(),
            prefix: Vec::new(),
            suffix: Vec::new(),
            mode: CitationMode::AuthorInText,
            note_num,
            hash: 0,
        };
        self.nodes.push(Node::Inline(Inline::Cite(
            vec![citation],
            vec![Inline::Str(format!("@{id}").into())],
        )));
        true
    }

    /// Advance the document-wide citation count and return the new value.
    fn bump_cite_count(&self) -> i32 {
        let next = self.notes.cite_count.get().saturating_add(1);
        self.notes.cite_count.set(next);
        next
    }

    /// Resolve an emoji shortcode `:name:` at the cursor (which sits on the opening `:`). A name is
    /// one or more ASCII letters, digits, `_`, `+`, or `-`, terminated by a closing `:`. When the
    /// name is in the curated table, the whole `:name:` becomes a `Span` classed `emoji` carrying the
    /// name in a `data-emoji` attribute and the unicode character as its text; the cursor advances
    /// past the closing `:` and `true` is returned. An unrecognized name (or no closing `:`) leaves
    /// the leading `:` untouched and returns `false`, so the run reparses as literal text.
    fn try_emoji(&mut self) -> bool {
        // The `:` delimiters and every name character (`[0-9A-Za-z_+-]`) are ASCII.
        let name_start = self.pos + 1;
        let mut index = name_start;
        while matches!(
            char_at(self.text, index),
            Some('0'..='9' | 'a'..='z' | 'A'..='Z' | '_' | '+' | '-')
        ) {
            index += 1;
        }
        if index == name_start || char_at(self.text, index) != Some(':') {
            return false;
        }
        let Some(name) = self.text.get(name_start..index) else {
            return false;
        };
        let Some(codepoints) = emoji::lookup(name) else {
            return false;
        };
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: vec!["emoji".into()],
            attributes: vec![("data-emoji".into(), name.into())],
        };
        self.pos = index + 1;
        self.nodes.push(Node::Inline(Inline::Span(
            Box::new(attr),
            vec![Inline::Str(codepoints.into())],
        )));
        true
    }

    /// Try a short sub/superscript at the cursor (`short_subsuperscripts`): a `~` or `^` directly
    /// followed by a run of alphanumerics, taken as the sub/superscript content without a closing
    /// delimiter. Within a caret's whitespace-bounded span the delimiters pair up left to right into
    /// the delimited `^x^`/`~x~` form, which the delimiter stack resolves; the short form applies
    /// only to an unpaired opener — an even number of matching delimiters precede it in the span and
    /// none follow. An empty alphanumeric run (a delimiter met by a non-alphanumeric or the line's
    /// end) is not a script.
    fn try_short_script(&mut self, delimiter: char) -> bool {
        let mut preceding = 0usize;
        let mut behind = self.pos;
        while let Some(ch) = char_before(self.text, behind) {
            if ch.is_whitespace() {
                break;
            }
            if ch == delimiter {
                preceding += 1;
            }
            behind -= ch.len_utf8();
        }
        // An odd count leaves this delimiter closing a prior opener, never starting a short script.
        if preceding % 2 == 1 {
            return false;
        }
        // An opener pairs into the delimited form when another matching delimiter follows in-span.
        // The delimiter (`~`/`^`) is ASCII, so the span begins one byte past the cursor.
        let mut ahead = self.pos + 1;
        while let Some(ch) = char_at(self.text, ahead) {
            if ch.is_whitespace() {
                break;
            }
            if ch == delimiter {
                return false;
            }
            ahead += ch.len_utf8();
        }
        let start = self.pos + 1;
        let mut end = start;
        while let Some(ch) = char_at(self.text, end) {
            if ch.is_alphanumeric() {
                end += ch.len_utf8();
            } else {
                break;
            }
        }
        let content = match self.text.get(start..end) {
            Some(slice) if !slice.is_empty() => slice,
            _ => return false,
        };
        let inner = vec![Inline::Str(content.into())];
        let node = if delimiter == '^' {
            Inline::Superscript(inner)
        } else {
            Inline::Subscript(inner)
        };
        self.nodes.push(Node::Inline(node));
        self.pos = end;
        true
    }

    fn backslash(&mut self) {
        // Backslash-delimited TeX math and raw TeX commands take precedence over a plain escape,
        // each gated behind its own extension. `\\(`/`\\[` (double backslash) is tried before
        // `\(`/`\[` (single backslash) so the longer opener wins.
        if self.try_backslash_math() || self.try_raw_tex() {
            return;
        }
        self.pos += 1;
        // With `all_symbols_escapable` — and always in the bare CommonMark engine — a backslash
        // escapes any ASCII punctuation character (and, in the markdown dialect, turns a following
        // space into a non-breaking space). Without it a markdown dialect escapes only the classic
        // set and leaves every other backslash literal.
        let broad = !self.notes.markdown || self.ext.contains(Extension::AllSymbolsEscapable);
        match self.peek() {
            // In the broad Markdown dialect a backslash before a line ending is a hard break only
            // when `escaped_line_breaks` is on; with it off the backslash is literal and the line
            // ending is an ordinary soft break. The bare CommonMark engine always hard-breaks here.
            Some('\n')
                if self.notes.markdown && !self.ext.contains(Extension::EscapedLineBreaks) =>
            {
                self.push_text('\\');
            }
            Some('\n') => {
                self.pos += 1;
                while matches!(self.peek(), Some(' ' | '\t')) {
                    self.pos += 1;
                }
                self.nodes.push(Node::LineBreak);
            }
            // In the markdown dialect a backslash before a space is a non-breaking space, which binds
            // into the surrounding text rather than splitting it on whitespace.
            Some(' ') if self.notes.markdown && broad => {
                self.pos += 1;
                self.push_text('\u{a0}');
            }
            Some(ch) if broad && is_ascii_punctuation(ch) => {
                self.pos += 1;
                self.push_text(ch);
            }
            Some(ch) if is_classic_markdown_escapable(ch) => {
                self.pos += 1;
                self.push_text(ch);
            }
            _ => self.push_text('\\'),
        }
    }

    /// Try the backslash math delimiters at the cursor (which sits on the leading `\`). `\(…\)` is
    /// inline math and `\[…\]` is display math; the double-backslash forms `\\(…\\)` and `\\[…\\]`
    /// use the same shapes with a doubled delimiter. Each form is gated behind its extension, and the
    /// double-backslash form is preferred so its longer opener is not stolen by the single form.
    /// Returns `true` (and advances past the closer) on a match, leaving a fallback escape otherwise.
    fn try_backslash_math(&mut self) -> bool {
        if self.ext.contains(Extension::TexMathDoubleBackslash)
            && char_at(self.text, self.pos) == Some('\\')
            && char_at(self.text, self.pos + 1) == Some('\\')
            && self.scan_backslash_math(2)
        {
            return true;
        }
        if self.ext.contains(Extension::TexMathSingleBackslash)
            && char_at(self.text, self.pos) == Some('\\')
            && self.scan_backslash_math(1)
        {
            return true;
        }
        false
    }

    /// Scan a backslash math span at the cursor (on the first backslash), pushing a `Math` node and
    /// advancing past the closer on a match. See [`crate::inline_scan::scan_backslash_math_bytes`].
    fn scan_backslash_math(&mut self, slashes: usize) -> bool {
        match crate::inline_scan::scan_backslash_math_bytes(self.text, self.pos, slashes) {
            Some((math_type, content, next)) => {
                self.pos = next;
                self.nodes
                    .push(Node::Inline(Inline::Math(math_type, content.into())));
                true
            }
            None => false,
        }
    }

    /// Try a raw inline TeX command at the cursor (on the leading `\`), gated behind `raw_tex`. A
    /// command is a backslash, an ASCII letter, and any following ASCII alphanumerics, optionally
    /// followed by balanced `{…}` and `[…]` argument groups. A `{`-group that opens but cannot be
    /// balance-closed reverts the whole command to literal text; an unclosable `[`-group simply ends
    /// the group run, leaving the command captured so far. A command with no argument groups and an
    /// all-letter name absorbs any run of trailing spaces and tabs (but not a newline). On a match the
    /// verbatim source becomes a `RawInline (Format "tex")` and the cursor advances past it.
    ///
    /// Known limitations:
    /// - Every group is consumed greedily, so a command takes all the `{…}`/`[…]` groups that
    ///   directly follow it. Some commands accept only a fixed number of arguments and leave the
    ///   rest as text; that per-command arity is not modeled here.
    /// - A paragraph that is wholly a `\begin{env}…\end{env}` environment is recognized in the block
    ///   phase; here every `\begin`/`\end` is treated as an ordinary inline command.
    fn try_raw_tex(&mut self) -> bool {
        if !self.ext.contains(Extension::RawTex) {
            return false;
        }
        if char_at(self.text, self.pos) != Some('\\') {
            return false;
        }
        // The leading `\` and the command name (ASCII letters and digits) are single-byte, so
        // byte and character offsets coincide up to `i`.
        let mut i = self.pos + 1;
        if !char_at(self.text, i).is_some_and(|c| c.is_ascii_alphabetic()) {
            return false;
        }
        i += 1;
        let mut name_all_letters = true;
        while let Some(ch) = char_at(self.text, i) {
            if ch.is_ascii_alphabetic() {
                i += 1;
            } else if ch.is_ascii_digit() {
                name_all_letters = false;
                i += 1;
            } else {
                break;
            }
        }
        // `\begin`/`\end` are raw TeX only as a complete, matched environment, never as bare
        // commands: capture the whole `\begin{ENV}`…`\end{ENV}`, or leave the text literal.
        let name = self.text.get(self.pos + 1..i);
        if name == Some("begin") {
            return self.try_raw_tex_environment(i);
        }
        if name == Some("end") {
            return false;
        }
        // Consume argument groups. A `{`-group must balance or the entire command reverts to text.
        let mut had_group = false;
        loop {
            match char_at(self.text, i) {
                Some('{') => match self.scan_balanced_group(i, '{', '}') {
                    Some(end) => {
                        i = end;
                        had_group = true;
                    }
                    None => return false,
                },
                Some('[') => match self.scan_balanced_group(i, '[', ']') {
                    Some(end) => {
                        i = end;
                        had_group = true;
                    }
                    None => break,
                },
                _ => break,
            }
        }

        // A bare command whose name is all letters (no argument groups, no digits) absorbs a
        // trailing run of spaces and tabs.
        if !had_group && name_all_letters {
            while matches!(char_at(self.text, i), Some(' ' | '\t')) {
                i += 1;
            }
        }

        let source = match self.text.get(self.pos..i) {
            Some(slice) => slice.to_owned(),
            None => return false,
        };
        self.pos = i;
        self.nodes.push(Node::Inline(Inline::RawInline(
            carta_ast::Format("tex".into()),
            source.into(),
        )));
        true
    }

    /// Capture a complete `\begin{ENV}`…matching `\end{ENV}` as a single raw TeX inline. The
    /// opener's `{ENV}` group names the environment; nested `\begin{ENV}`/`\end{ENV}` of that same
    /// name deepen and lift the nesting, and the capture ends at the `\end{ENV}` that returns the
    /// depth to zero. Without a `{ENV}` group or a matching close the `\begin` is not raw TeX and
    /// the call reverts to literal text by returning `false`.
    fn try_raw_tex_environment(&mut self, name_end: usize) -> bool {
        if char_at(self.text, name_end) != Some('{') {
            return false;
        }
        let Some(group_end) = self.scan_balanced_group(name_end, '{', '}') else {
            return false;
        };
        // `group_end` sits just past the closing `}` (ASCII), so `group_end - 1` is its byte offset
        // and `name_end + 1` the byte past the opening `{`.
        let Some(env) = self.text.get(name_end + 1..group_end - 1) else {
            return false;
        };
        let Some(end) = self.scan_environment_close(group_end, env) else {
            return false;
        };
        let source = match self.text.get(self.pos..end) {
            Some(slice) => slice.to_owned(),
            None => return false,
        };
        self.pos = end;
        self.nodes.push(Node::Inline(Inline::RawInline(
            carta_ast::Format("tex".into()),
            source.into(),
        )));
        true
    }

    /// From `from`, find the index just past the `\end{ENV}` that closes an open `\begin{ENV}`,
    /// tracking nested same-name environments by depth. `None` when no matching close is found.
    fn scan_environment_close(&self, from: usize, env: &str) -> Option<usize> {
        let mut depth = 1usize;
        let mut i = from;
        while let Some(ch) = char_at(self.text, i) {
            if ch == '\\' {
                if let Some(after) = self.match_environment_marker(i, "begin", env) {
                    depth += 1;
                    i = after;
                    continue;
                }
                if let Some(after) = self.match_environment_marker(i, "end", env) {
                    depth -= 1;
                    if depth == 0 {
                        return Some(after);
                    }
                    i = after;
                    continue;
                }
            }
            i += ch.len_utf8();
        }
        None
    }

    /// If the characters at `at` spell `\KEYWORD{ENV}` (e.g. `\end{equation}`), return the index
    /// just past the closing brace; otherwise `None`.
    fn match_environment_marker(&self, at: usize, keyword: &str, env: &str) -> Option<usize> {
        let mut i = at;
        if char_at(self.text, i) != Some('\\') {
            return None;
        }
        i += 1;
        for kc in keyword.chars() {
            if char_at(self.text, i) != Some(kc) {
                return None;
            }
            i += kc.len_utf8();
        }
        if char_at(self.text, i) != Some('{') {
            return None;
        }
        i += 1;
        for ec in env.chars() {
            if char_at(self.text, i) != Some(ec) {
                return None;
            }
            i += ec.len_utf8();
        }
        if char_at(self.text, i) != Some('}') {
            return None;
        }
        Some(i + 1)
    }

    /// Scan a balanced group `open`…`close` starting at index `start` (which must hold `open`),
    /// returning the index just past the matching `close`, or `None` if it never closes. Nested
    /// same-kind delimiters are tracked by depth. `open` and `close` are ASCII delimiters.
    fn scan_balanced_group(&self, start: usize, open: char, close: char) -> Option<usize> {
        let mut depth = 0usize;
        let mut i = start;
        while let Some(ch) = char_at(self.text, i) {
            if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            i += ch.len_utf8();
        }
        None
    }

    fn code_span(&mut self) {
        let start = self.pos;
        let open = backtick_run_len(self.text, self.pos);
        self.pos += open;
        if let Some(close) = self.next_backtick_run(open, self.pos) {
            let content = self
                .text
                .get(self.pos..close)
                .map(str::to_owned)
                .unwrap_or_default();
            self.pos = close + open;
            if let Some((format, next)) = self.scan_raw_format() {
                self.pos = next;
                self.nodes.push(Node::Inline(Inline::RawInline(
                    carta_ast::Format(format.into()),
                    normalize_code(&content, self.notes.markdown).into(),
                )));
                return;
            }
            let attr = self.take_code_attr();
            self.nodes.push(Node::Inline(Inline::Code(
                Box::new(attr),
                normalize_code(&content, self.notes.markdown).into(),
            )));
            return;
        }
        // No closing run: emit the opening backticks literally.
        let literal = self
            .text
            .get(start..self.pos)
            .map(str::to_owned)
            .unwrap_or_default();
        self.push_str(&literal);
    }

    /// The start of the first maximal run of exactly `len` backticks at or after `from`, or `None`
    /// if the buffer holds no such run. Backed by a per-buffer index of every maximal run's start
    /// keyed by run length; the index is built once on first use and then binary-searched, so a
    /// close search costs O(log n) rather than a scan to end-of-buffer.
    fn next_backtick_run(&mut self, len: usize, from: usize) -> Option<usize> {
        let text = self.text;
        let runs = self.backtick_runs.get_or_insert_with(|| {
            let mut index: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            let bytes = text.as_bytes();
            let mut scan = 0;
            while scan < bytes.len() {
                // The backtick is ASCII, so advancing one byte over any non-backtick byte (including
                // every byte of a multi-byte character) never lands mid-run or records a false start.
                if bytes.get(scan) == Some(&b'`') {
                    let run = backtick_run_len(text, scan);
                    index.entry(run).or_default().push(scan);
                    scan += run;
                } else {
                    scan += 1;
                }
            }
            index
        });
        let positions = runs.get(&len)?;
        let at = positions.partition_point(|&p| p < from);
        positions.get(at).copied()
    }

    /// Parse `$…$` (inline) or `$$…$$` (display) TeX math at the cursor.
    ///
    /// A `$$` opener is display math, closed by the next `$$`; if no closing `$$` follows, the first
    /// `$` is literal and the second is reconsidered (it may open inline math). A single `$` opens
    /// inline math only when followed by a non-space character and closed by an unescaped `$` that is
    /// preceded by a non-space and not followed by a digit; inline content holds no unescaped `$`, so
    /// a failed first closer leaves the opener literal.
    fn dollar_math(&mut self) {
        if self.at(1) == Some('$') {
            if let Some((content, next)) =
                crate::inline_scan::scan_display_math_bytes(self.text, self.pos)
            {
                self.pos = next;
                self.nodes.push(Node::Inline(Inline::Math(
                    MathType::DisplayMath,
                    content.into(),
                )));
                return;
            }
        } else if let Some((content, next)) =
            crate::inline_scan::scan_inline_math_bytes(self.text, self.pos)
        {
            self.pos = next;
            self.nodes.push(Node::Inline(Inline::Math(
                MathType::InlineMath,
                content.into(),
            )));
            return;
        }
        self.pos += 1;
        self.push_text('$');
    }

    fn left_angle(&mut self) {
        if let Some((inline, next)) = scan_autolink(self.text, self.pos) {
            self.pos = next;
            // The markdown dialect tags an explicit angle autolink with a `uri` or `email` class
            // and percent-encodes its destination; the strict dialect leaves it unclassed and
            // verbatim. The destination is encoded after classification, which compares the shown
            // text against the still-raw destination to tell a `uri` from an `email`.
            let inline = if self.notes.markdown {
                escape_link_destination(classify_angle_autolink(inline))
            } else {
                inline
            };
            self.nodes.push(Node::Inline(inline));
            return;
        }
        if let Some((html, next)) = scan_html_tag(self.text, self.pos) {
            self.pos = next;
            // In the Markdown dialect with `raw_html` off the tag is still recognized as a unit but
            // kept as literal text rather than a passthrough span. The bare CommonMark engine always
            // emits raw HTML, since HTML is part of its core grammar.
            if self.notes.markdown && !self.ext.contains(Extension::RawHtml) {
                self.push_str(&html);
            } else {
                self.nodes.push(Node::Inline(Inline::RawInline(
                    carta_ast::Format("html".into()),
                    html.into(),
                )));
            }
            return;
        }
        self.pos += 1;
        self.push_text('<');
    }

    fn entity(&mut self) {
        if let Some((decoded, next)) = scan_entity(self.text, self.pos) {
            self.pos = next;
            self.push_str(&decoded);
        } else {
            self.pos += 1;
            self.push_text('&');
        }
    }

    fn line_ending(&mut self) {
        // Trailing spaces before the newline determine hard vs soft break.
        let hard = matches!(self.nodes.last(), Some(Node::Text(t)) if t.ends_with("  "));
        let backslash_hard = matches!(self.nodes.last(), Some(Node::LineBreak));
        if let Some(Node::Text(text)) = self.nodes.last_mut() {
            let trimmed = text.trim_end_matches(' ').to_owned();
            *text = trimmed;
            if text.is_empty() {
                self.nodes.pop();
            }
        }
        self.pos += 1;
        // Skip leading spaces/tabs of the next line.
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.pos += 1;
        }
        if hard || backslash_hard || self.ext.contains(Extension::HardLineBreaks) {
            self.nodes.push(Node::LineBreak);
        } else {
            self.nodes.push(Node::SoftBreak);
        }
    }

    fn emphasis_run(&mut self, ch: u8) {
        let start = self.pos;
        while self.peek() == Some(ch as char) {
            self.pos += 1;
        }
        // The delimiter is ASCII, so the run's byte length equals its character count.
        let count = self.pos - start;
        let before = char_before(self.text, start);
        let after = self.peek();
        // With `intraword_underscores` off in the Markdown dialect, a `_` run pairs like `*`,
        // emphasizing even between word characters; otherwise the CommonMark `_` rule keeps an
        // intraword run inert.
        let relax_underscore =
            self.notes.markdown && !self.ext.contains(Extension::IntrawordUnderscores);
        let (can_open, can_close) = run_flanking(ch, before, after, relax_underscore);
        self.nodes.push(Node::Delimiter(Delimiter {
            ch,
            count,
            can_open,
            can_close,
            image: false,
            text_start: self.pos,
            active: false,
            cite_count_at_open: 0,
        }));
    }

    /// Replace a run of two or more `-` with em/en dashes; a lone `-` stays literal. A run folds
    /// into the fewest dashes that reproduce its length: groups of three become em dashes (`—`)
    /// and groups of two become en dashes (`–`), preferring em dashes for any odd remainder.
    fn smart_dash(&mut self) {
        let mut len = 0;
        while self.peek() == Some('-') {
            self.pos += 1;
            len += 1;
        }
        if len == 1 {
            self.push_text('-');
            return;
        }
        let out = fold_dash_run(len);
        self.push_str(&out);
    }

    /// Replace each run of three dots with an ellipsis (`…`), leaving any remaining one or two dots
    /// literal. Dots separated by other characters are never joined.
    fn smart_ellipsis(&mut self) {
        let mut len = 0;
        while self.peek() == Some('.') {
            self.pos += 1;
            len += 1;
        }
        let out = fold_ellipsis_run(len);
        self.push_str(&out);
    }

    fn push_open_bracket(&mut self, image: bool) {
        let node_index = self.nodes.len();
        self.bracket_stack.push(node_index);
        self.nodes.push(Node::Delimiter(Delimiter {
            ch: b'[',
            count: 1,
            can_open: true,
            can_close: false,
            image,
            text_start: self.pos,
            active: true,
            cite_count_at_open: self.notes.cite_count.get(),
        }));
    }

    fn close_bracket(&mut self) {
        self.pos += 1;
        let Some(&opener_index) = self.bracket_stack.last() else {
            self.push_text(']');
            return;
        };
        let (is_image, is_active) = match self.nodes.get(opener_index) {
            Some(Node::Delimiter(d)) => (d.image, d.active),
            _ => (false, false),
        };

        // A defined footnote reference `[^label]` wins over every other use of the brackets: it
        // consumes nothing past the `]` and ignores any following inline target or reference.
        if is_active
            && self.ext.contains(Extension::Footnotes)
            && self.try_footnote(opener_index, is_image)
        {
            return;
        }

        // An active opener may form a link or image from an explicit inline `(...)` target or an
        // explicit `[label]`/`[]` reference. (An inactive `[` cannot — a link may not contain
        // another link, spec §6.3 rule 6 — but it may still open a bracketed span.)
        if is_active {
            match self.resolve_explicit(opener_index) {
                Explicit::Target(target, next) => {
                    self.finish_link(opener_index, is_image, target, next);
                    return;
                }
                // An explicit reference whose label is undefined is not a link; the brackets stay
                // literal and no span or shortcut fallback is tried.
                Explicit::Failed => {
                    self.bracket_stack.pop();
                    self.literalize_bracket(opener_index);
                    self.push_text(']');
                    return;
                }
                Explicit::None => {}
            }
        }

        // With no explicit target, a non-image bracket directly followed by a non-empty attribute
        // block is a span — this wins over a shortcut reference of the same label.
        if !is_image
            && self.ext.contains(Extension::BracketedSpans)
            && let Some((attr, next)) = self.scan_attr_block()
        {
            self.bracket_stack.pop();
            self.pos = next;
            self.build_span(opener_index, attr);
            return;
        }

        // A shortcut reference: the bracket's own text names the definition. Skip the whole lookup
        // when no definitions exist or the span is too long to be a label.
        if is_active && !self.refs.is_empty() {
            let raw = self.raw_label(opener_index);
            if raw.len() <= MAX_LABEL_BYTES
                && let Some(target) = self.refs.get(&normalize_label(raw)).map(def_target)
            {
                self.finish_link(opener_index, is_image, target, self.pos);
                return;
            }
        }

        // A bracket whose content is a well-formed citation list becomes a `Cite`. An image's `!`
        // survives as literal text before it.
        if self.ext.contains(Extension::Citations)
            && self.try_bracket_citation(opener_index, is_image)
        {
            return;
        }

        // Otherwise the opener reverts to its literal `[` / `![`, and `]` stays literal.
        self.bracket_stack.pop();
        self.literalize_bracket(opener_index);
        self.push_text(']');
    }

    /// If the bracket opener encloses a well-formed citation list `[ ... @key ... ]`, emit a `Cite`
    /// and return `true`. The content is split on top-level semicolons into entries; every entry
    /// must hold one top-level `@key`, and no entry may be empty. Each entry's text before the key
    /// is its prefix and the text after is its suffix (both parsed as inlines, so a nested bare
    /// `@key` there becomes its own citation); a `-` glued to the front of the key suppresses the
    /// author. The whole group shares one citation number, raised to cover any nested citation. The
    /// fallback field is the raw bracket source parsed as ordinary inlines. Returns `false` (leaving
    /// the brackets for literal handling) when the content is not a citation list.
    fn try_bracket_citation(&mut self, opener_index: usize, is_image: bool) -> bool {
        let raw = self.raw_label(opener_index);
        // A citation list must carry at least one `@key`; without an `@` the span cannot be one, so
        // skip the segment scan entirely.
        if !raw.as_bytes().contains(&b'@') {
            return false;
        }
        let Some(segments) = split_citation_segments(raw) else {
            return false;
        };
        // Scanning the interior may have counted bare citations that are about to be discarded with
        // their nodes; rewind to the count this bracket opened with before numbering the group. (For
        // the rare `![@key]`, the discarded interior count is not added back, so such a group's
        // number is one lower than a longer document with the same citation order would otherwise
        // give it.)
        if let Some(Node::Delimiter(d)) = self.nodes.get(opener_index) {
            self.notes.cite_count.set(d.cite_count_at_open);
        }
        // Reserve this group's number before parsing affixes, so nested citations are counted after
        // it and the group ends up stamped with the highest number it contains.
        self.bump_cite_count();
        let mut citations = Vec::with_capacity(segments.len());
        for segment in &segments {
            let Some(entry) = self.parse_citation_entry(raw, segment.clone()) else {
                return false;
            };
            citations.push(entry);
        }
        let group_num = self.notes.cite_count.get();
        for citation in &mut citations {
            citation.note_num = group_num;
        }
        let fallback = citation_fallback_inlines(&format!("[{raw}]"));
        self.nodes.truncate(opener_index);
        self.bracket_stack.retain(|&ni| ni < opener_index);
        if is_image {
            self.push_text('!');
        }
        self.nodes
            .push(Node::Inline(Inline::Cite(citations, fallback)));
        true
    }

    /// Parse one citation entry from `chars[range]`: locate the first top-level `@key`, taking the
    /// text before it as the prefix and the text after as the suffix. A `-` directly before the key
    /// (itself at the segment start or preceded by whitespace) suppresses the author. Returns `None`
    /// when the segment holds no top-level key.
    fn parse_citation_entry(&self, raw: &str, range: std::ops::Range<usize>) -> Option<Citation> {
        let key = find_citation_key(raw, range.clone())?;
        let prefix_end = if key.suppress { key.dash } else { key.at };
        let prefix_src = raw.get(range.start..prefix_end)?;
        let suffix_src = raw.get(key.id_end..range.end)?;
        let mode = if key.suppress {
            CitationMode::SuppressAuthor
        } else {
            CitationMode::NormalCitation
        };
        // The prefix is trimmed of surrounding whitespace; the suffix keeps any leading space (so
        // `@a x` separates the key from `x`) but drops trailing space.
        //
        // A suffix opening with a locator label such as `p.` or `vol.` carries a non-breaking space
        // before its number (`p.\u{a0}5`). That join, gated on a fixed set of abbreviations, is not
        // applied here: the suffix is tokenized as ordinary inlines, so `p. 5` stays three tokens.
        Some(Citation {
            id: key.id.into(),
            prefix: parse_inlines(prefix_src.trim(), self.refs, self.notes, self.ext),
            suffix: parse_inlines(suffix_src.trim_end(), self.refs, self.notes, self.ext),
            mode,
            note_num: 0,
            hash: 0,
        })
    }

    /// Pop the opener, consume an optional trailing attribute block, and emit the link or image.
    fn finish_link(&mut self, opener_index: usize, is_image: bool, target: Target, next: usize) {
        self.bracket_stack.pop();
        self.pos = next;
        let attr = self.take_link_attr();
        self.build_link(opener_index, is_image, target, attr);
        if !is_image {
            self.deactivate_earlier_brackets(opener_index);
        }
    }

    /// Parse one or more consecutive non-empty attribute blocks at the cursor, merged into a single
    /// [`Attr`], with the position past the last block. An empty block (`{}`) alone is not consumed;
    /// a space between blocks ends the run.
    fn scan_attr_block(&self) -> Option<(Attr, usize)> {
        let (mut merged, mut next) = attr::parse_attributes_bytes(self.text, self.pos)?;
        while let Some((more, after)) = attr::parse_attributes_bytes(self.text, next) {
            attr::merge(&mut merged, more);
            next = after;
        }
        attr::is_non_empty(&merged).then_some((merged, next))
    }

    /// Scan a raw-format marker `{=FORMAT}` at the cursor, returning the format name and the index
    /// past the closing brace. The braces may hold surrounding whitespace (`{ =html }`), but no
    /// space may sit between `=` and the format, and the format may carry nothing but the marker:
    /// any further content (`{=html .foo}`) is not a raw marker. The format token is one or more
    /// ASCII alphanumerics, `-`, or `_`. Active only when `raw_attribute` is enabled.
    fn scan_raw_format(&self) -> Option<(String, usize)> {
        if !self.ext.contains(Extension::RawAttribute) {
            return None;
        }
        if char_at(self.text, self.pos) != Some('{') {
            return None;
        }
        let mut index = self.pos + 1;
        while let Some(ch) = char_at(self.text, index) {
            if ch == ' ' || ch == '\t' {
                index += 1;
            } else {
                break;
            }
        }
        if char_at(self.text, index) != Some('=') {
            return None;
        }
        index += 1;
        let format_start = index;
        while let Some(ch) = char_at(self.text, index) {
            if is_format_name_char(ch) {
                index += ch.len_utf8();
            } else {
                break;
            }
        }
        if index == format_start {
            return None;
        }
        let format = self.text.get(format_start..index)?.to_owned();
        while let Some(ch) = char_at(self.text, index) {
            if ch == ' ' || ch == '\t' {
                index += 1;
            } else {
                break;
            }
        }
        if char_at(self.text, index) != Some('}') {
            return None;
        }
        Some((format, index + 1))
    }

    /// Consume an attribute block following an inline code span when the relevant extension is on,
    /// advancing the cursor; otherwise the default attribute.
    fn take_code_attr(&mut self) -> Attr {
        if (self.ext.contains(Extension::InlineCodeAttributes)
            || self.ext.contains(Extension::Attributes))
            && let Some((parsed, next)) = self.scan_attr_block()
        {
            self.pos = next;
            return parsed;
        }
        Attr::default()
    }

    /// Consume an attribute block following a link or image when the relevant extension is on,
    /// advancing the cursor; otherwise the default attribute.
    fn take_link_attr(&mut self) -> Attr {
        if (self.ext.contains(Extension::LinkAttributes)
            || self.ext.contains(Extension::Attributes))
            && let Some((parsed, next)) = self.scan_attr_block()
        {
            self.pos = next;
            return parsed;
        }
        Attr::default()
    }

    /// Build a span from a non-image bracket opener and its inner content.
    fn build_span(&mut self, opener_index: usize, attr: Attr) {
        let inner: Vec<Node> = self.nodes.split_off(opener_index + 1);
        self.nodes.pop(); // remove the opener delimiter
        self.bracket_stack.retain(|&ni| ni < opener_index);
        let content = resolve_inline_nodes(inner, self.ext, self.notes.markdown);
        self.nodes
            .push(Node::Inline(Inline::Span(Box::new(attr), content)));
    }

    /// Turn an unmatched bracket opener back into the literal text it stands for.
    fn literalize_bracket(&mut self, opener_index: usize) {
        if let Some(node) = self.nodes.get_mut(opener_index)
            && let Node::Delimiter(d) = node
        {
            let literal = if d.image { "![" } else { "[" };
            *node = Node::Text(literal.to_owned());
        }
    }

    /// Mark all non-image `[` openers that appear before `before` in the node list as inactive,
    /// preventing them from forming links that would contain the link just built. Inactive openers
    /// remain on the bracket stack so that a later `]` can consume them one at a time (spec §6.3,
    /// rule 6): each `]` pops the top inactive entry, literalizes it, and emits `]` as text.
    fn deactivate_earlier_brackets(&mut self, before: usize) {
        for &ni in &self.bracket_stack {
            if ni >= before {
                continue;
            }
            if let Some(Node::Delimiter(d)) = self.nodes.get_mut(ni)
                && !d.image
            {
                d.active = false;
            }
        }
    }

    /// Resolve an explicit link target following `]`: an inline `(...)` destination or an explicit
    /// `[label]`/`[]` reference. Shortcut references (the bracket's own text) are handled separately
    /// so a bracketed span can take precedence over them.
    fn resolve_explicit(&self, opener_index: usize) -> Explicit {
        if self.at(0) == Some('(') {
            // The markdown dialect lets an unbracketed destination hold spaces and balanced
            // parentheses; the strict dialect ends a destination at the first space.
            let scanned = if self.notes.markdown {
                scan_markdown_inline_target(self.text, self.pos)
            } else {
                scan_inline_target(self.text, self.pos)
            };
            if let Some((target, next)) = scanned {
                return Explicit::Target(target, next);
            }
        }
        // Explicit reference. Labels match on their raw source text (the closing `]` sits at `pos - 1`).
        // With `spaced_reference_links`, whitespace may separate the text bracket from the reference
        // label bracket — `[text] [ref]` and `[text]\n[ref]` — though not from the inline `(...)`
        // target handled above.
        let mut label_start = self.pos;
        if self.ext.contains(Extension::SpacedReferenceLinks) {
            while matches!(char_at(self.text, label_start), Some(' ' | '\t' | '\n')) {
                label_start += 1;
            }
        }
        if let Some((label, next)) = scan_following_label(self.text, label_start) {
            // An explicit reference with no definitions in scope can never resolve, so the brackets
            // stay literal without extracting or normalizing the label.
            if self.refs.is_empty() {
                return Explicit::Failed;
            }
            let key = if label.is_empty() {
                // A collapsed reference is keyed on the bracket's own span, which is unbounded
                // source text; past the label limit it is no label at all.
                let raw = self.raw_label(opener_index);
                if raw.len() > MAX_LABEL_BYTES {
                    return Explicit::Failed;
                }
                normalize_label(raw)
            } else {
                normalize_label(&label)
            };
            return match self.refs.get(&key).map(def_target) {
                Some(target) => Explicit::Target(target, next),
                None => Explicit::Failed,
            };
        }
        Explicit::None
    }

    /// The raw source between a bracket opener and the closing `]` just consumed.
    fn raw_label(&self, opener_index: usize) -> &'a str {
        let start = match self.nodes.get(opener_index) {
            Some(Node::Delimiter(d)) => d.text_start,
            _ => return "",
        };
        // The closing `]` is ASCII, so its byte offset is `self.pos - 1`.
        self.text
            .get(start..self.pos.saturating_sub(1))
            .unwrap_or_default()
    }

    /// If the bracket opener encloses a defined footnote reference (`[^label]`), emit the note and
    /// return `true`. The opener's raw label must begin with `^` and name a known footnote; the
    /// brackets and their content are then replaced wholesale, and an image opener's `!` survives as
    /// literal text. Inside a footnote definition's own body a reference collapses to an empty string
    /// rather than nesting a note. Returns `false` (leaving the brackets for other resolution) when
    /// the label has no `^` prefix, holds a bracket, or matches no definition.
    fn try_footnote(&mut self, opener_index: usize, is_image: bool) -> bool {
        if self.notes.defined.is_empty() {
            return false;
        }
        let raw = self.raw_label(opener_index);
        let Some(label) = raw.strip_prefix('^') else {
            return false;
        };
        if label.is_empty() || label.contains('[') || label.contains(']') {
            return false;
        }
        let key = normalize_label(label);
        if !self.notes.defined.contains(&key) {
            return false;
        }
        self.nodes.truncate(opener_index);
        self.bracket_stack.retain(|&ni| ni < opener_index);
        if is_image {
            self.push_text('!');
        }
        let note = if self.notes.in_definition {
            Inline::Str(carta_ast::Text::default())
        } else {
            Inline::Note(self.notes.by_id.get(&key).cloned().unwrap_or_default())
        };
        self.nodes.push(Node::Inline(note));
        true
    }

    /// Resolve an inline note `^[...]` at the cursor (which sits on the `^`, with `[` following).
    /// The bracket content runs up to its balanced closing `]`, is parsed as inline markdown, and
    /// becomes a single-paragraph `Note`. Returns `false` without advancing when the bracket has no
    /// balanced closer, leaving the `^` for literal/superscript handling.
    fn try_inline_note(&mut self) -> bool {
        // self.pos is the caret; the `[` sits at self.pos + 1 (the `^` is ASCII). Walk forward
        // tracking bracket depth.
        let mut depth = 0usize;
        let mut index = self.pos + 1;
        let mut end = None;
        while let Some(ch) = char_at(self.text, index) {
            match ch {
                '\\' => index += 1 + char_at(self.text, index + 1).map_or(0, char::len_utf8),
                '[' => {
                    depth += 1;
                    index += 1;
                }
                ']' => {
                    depth -= 1;
                    index += 1;
                    if depth == 0 {
                        end = Some(index);
                        break;
                    }
                }
                _ => index += ch.len_utf8(),
            }
        }
        let Some(end) = end else {
            return false;
        };
        // The bracket content lies between `[` (at self.pos + 1) and the closing `]` (ASCII, at
        // end - 1).
        let inner = self
            .text
            .get(self.pos + 2..end.saturating_sub(1))
            .map(str::to_owned)
            .unwrap_or_default();
        let inlines = parse_inlines(&inner, self.refs, self.notes, self.ext);
        self.pos = end;
        self.nodes
            .push(Node::Inline(Inline::Note(vec![para(inlines)])));
        true
    }

    fn build_link(&mut self, opener_index: usize, is_image: bool, mut target: Target, attr: Attr) {
        // The markdown dialect percent-encodes a destination's unsafe characters; the strict
        // CommonMark and GitHub dialects keep it verbatim.
        if self.notes.markdown {
            target.url = escape_uri(&target.url).into();
        }
        let inner: Vec<Node> = self.nodes.split_off(opener_index + 1);
        self.nodes.pop(); // remove the opener delimiter
        // Any bracket stack entries that pointed into the split-off range are now part of the
        // inner node list passed to emphasis resolution; they no longer belong to the outer parse.
        self.bracket_stack.retain(|&ni| ni < opener_index);
        let content = resolve_inline_nodes(inner, self.ext, self.notes.markdown);
        let inline = if is_image {
            Inline::Image(Box::new(attr), content, Box::new(target))
        } else {
            Inline::Link(Box::new(attr), content, Box::new(target))
        };
        self.nodes.push(Node::Inline(inline));
    }
}

/// A character that may appear directly before `@` and block a bare citation: an alphanumeric
/// glues the `@` to a preceding word (`foo@bar`, an email-like run), so no citation forms there.
fn is_citation_word(ch: char) -> bool {
    ch.is_alphanumeric()
}

/// Tokenize raw citation source into the literal inlines that stand in for the citation: whitespace
/// runs become `SoftBreak` (when they hold a newline) or `Space`, and every other run becomes a
/// `Str`. No inline markup is interpreted, so the source reads back verbatim word by word.
fn citation_fallback_inlines(raw: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut had_space = false;
    let mut had_newline = false;
    let flush_word = |out: &mut Vec<Inline>, word: &mut String| {
        if !word.is_empty() {
            out.push(Inline::Str(std::mem::take(word).into()));
        }
    };
    for ch in raw.chars() {
        if ch.is_whitespace() {
            flush_word(&mut out, &mut word);
            had_space = true;
            had_newline |= ch == '\n';
        } else {
            if had_space {
                out.push(if had_newline {
                    Inline::SoftBreak
                } else {
                    Inline::Space
                });
                had_space = false;
                had_newline = false;
            }
            word.push(ch);
        }
    }
    flush_word(&mut out, &mut word);
    if had_space {
        out.push(if had_newline {
            Inline::SoftBreak
        } else {
            Inline::Space
        });
    }
    out
}

/// A character that always belongs to a citation key: an alphanumeric or `_`. A key begins with one
/// of these.
fn is_citation_key_start(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Scan a citation key beginning at `start` (the index just past `@`). A key opens with an
/// alphanumeric or `_` and runs over further such characters; the internal punctuation `-`, `.`,
/// `:`, and `/` extend it only when another key character follows, so a trailing `-key.` keeps the
/// `key` but drops the `.`. Returns the key text and the index just past it, or `None` when no key
/// begins at `start`.
fn scan_citation_id(text: &str, start: usize) -> Option<(String, usize)> {
    let first = char_at(text, start)?;
    if !is_citation_key_start(first) {
        return None;
    }
    let mut end = start + first.len_utf8();
    while let Some(ch) = char_at(text, end) {
        if is_citation_key_start(ch) {
            end += ch.len_utf8();
        } else if matches!(ch, '-' | '.' | ':' | '/')
            // The internal punctuation is ASCII, so the following key character begins one byte on.
            && matches!(char_at(text, end + 1), Some(next) if is_citation_key_start(next))
        {
            end += 1 + char_at(text, end + 1).map_or(0, char::len_utf8);
        } else {
            break;
        }
    }
    let id = text.get(start..end)?.to_owned();
    Some((id, end))
}

/// Advance past one escape, backtick code span, or bracket at `index`, updating bracket `depth` and
/// returning the next index. Returns `None` when the character is none of those — the caller then
/// inspects it for a top-level delimiter (`;` or `@`) and advances itself.
fn step_citation_scan(text: &str, index: usize, depth: &mut usize) -> Option<usize> {
    match char_at(text, index) {
        Some('\\') => Some(index + 1 + char_at(text, index + 1).map_or(0, char::len_utf8)),
        Some('`') => {
            let run = backtick_run_len(text, index);
            Some(skip_code_span(text, index, run))
        }
        Some('[') => {
            *depth += 1;
            Some(index + 1)
        }
        Some(']') => {
            *depth = depth.saturating_sub(1);
            Some(index + 1)
        }
        _ => None,
    }
}

/// Split a bracket's raw content into citation segments on top-level semicolons. A semicolon inside
/// a nested `[...]` or a backtick code span does not split. Returns `None` when the content is not a
/// citation list: it has no `@` at all, or any segment is empty (including a leading or trailing
/// empty segment from a stray semicolon).
fn split_citation_segments(text: &str) -> Option<Vec<std::ops::Range<usize>>> {
    if !text.contains('@') {
        return None;
    }
    let mut segments = Vec::new();
    let mut start = 0;
    let mut index = 0;
    let mut depth = 0usize;
    while index < text.len() {
        if let Some(next) = step_citation_scan(text, index, &mut depth) {
            index = next;
        } else if char_at(text, index) == Some(';') && depth == 0 {
            segments.push(start..index);
            start = index + 1;
            index += 1;
        } else {
            index += char_at(text, index).map_or(1, char::len_utf8);
        }
    }
    segments.push(start..text.len());
    // An empty segment (a stray `;`, or whitespace-only between separators) is not a citation list.
    for segment in &segments {
        if text
            .get(segment.clone())
            .is_none_or(|s| s.chars().all(char::is_whitespace))
        {
            return None;
        }
    }
    Some(segments)
}

/// The length in bytes of the backtick run starting at byte offset `index`.
fn backtick_run_len(text: &str, index: usize) -> usize {
    let bytes = text.as_bytes();
    let mut len = 0;
    while bytes.get(index + len) == Some(&b'`') {
        len += 1;
    }
    len
}

/// Skip past a code span opened by `run` backticks at `index`, returning the index just past its
/// closing run. With no matching closer the backticks are not a code span and only the opening run
/// is skipped.
fn skip_code_span(text: &str, index: usize, run: usize) -> usize {
    let bytes = text.as_bytes();
    let mut scan = index + run;
    while scan < bytes.len() {
        if bytes.get(scan) == Some(&b'`') {
            let closer = backtick_run_len(text, scan);
            if closer == run {
                return scan + closer;
            }
            scan += closer;
        } else {
            scan += 1;
        }
    }
    index + run
}

/// The located citation key of one segment: the index of `@`, the key text and the index past it,
/// whether a `-` author-suppression marker precedes the `@`, and that marker's index.
struct CitationKey {
    at: usize,
    dash: usize,
    id: String,
    id_end: usize,
    suppress: bool,
}

/// Find the first top-level `@key` within `chars[range]`: an `@` not inside a nested `[...]` or a
/// backtick code span, immediately followed by a key. A `-` directly before the `@`, itself at the
/// segment start or preceded by whitespace, marks author suppression. Returns `None` when no such
/// key is present.
fn find_citation_key(text: &str, range: std::ops::Range<usize>) -> Option<CitationKey> {
    let mut index = range.start;
    let mut depth = 0usize;
    while index < range.end {
        if let Some(next) = step_citation_scan(text, index, &mut depth) {
            index = next;
            continue;
        }
        if depth == 0
            && char_at(text, index) == Some('@')
            && let Some((id, id_end)) = scan_citation_id(text, index + 1)
        {
            // A `-` is ASCII, so when it precedes the `@` it sits at byte `index - 1`, and the
            // character before it ends at `index - 1`.
            let dash_before = index > range.start && char_before(text, index) == Some('-');
            let dash_anchored = dash_before
                && (index - 1 == range.start
                    || char_before(text, index - 1).is_some_and(char::is_whitespace));
            let suppress = dash_anchored;
            return Some(CitationKey {
                at: index,
                dash: if suppress { index - 1 } else { index },
                id,
                id_end,
                suppress,
            });
        }
        index += char_at(text, index).map_or(1, char::len_utf8);
    }
    None
}

fn def_target(def: &LinkDef) -> Target {
    Target {
        url: def.url.clone().into(),
        title: def.title.clone().into(),
    }
}

/// A record in the delimiter list used by [`process_emphasis`].
///
/// Entries are held in a `Vec` whose indices are stable for the lifetime of one resolution pass —
/// an entry is never moved or removed, only unlinked. `prev`/`next` thread the still-active entries
/// into a doubly-linked list; consuming a matched pair unlinks the delimiters between and around it
/// in O(1), so the pass stays linear on delimiter-heavy input.
#[derive(Debug, Clone)]
struct DelimEntry {
    /// Index into `nodes` where this delimiter lives. Stable: nodes are never spliced during a pass.
    node_index: usize,
    ch: u8,
    count: usize,
    can_open: bool,
    can_close: bool,
    /// Previous still-active entry (a smaller `delims` index), or `None` at the list head.
    prev: Option<usize>,
    /// Next still-active entry (a larger `delims` index), or `None` at the list tail.
    next: Option<usize>,
}

/// Resolve emphasis/strong (`*`/`_`) and format (`~`/`^`) delimiters in `nodes`, starting at
/// `stack_bottom`.
///
/// Implements the linear algorithm from the spec ("An algorithm for parsing nested emphasis and
/// links", `CommonMark` spec §A): a single left-to-right pass over closers, with per-bucket
/// `openers_bottom` lower bounds that prevent re-scanning already-rejected opener ranges.
///
/// All four delimiter kinds share one matching loop. They differ only in how a matched pair's
/// length maps to a node — see [`match_use_count`] and [`wrap_emphasis`].
// `opener_di` (delimiter-list index) and `opener_ni` (node index) are intentionally close names
// for two distinct indices into two distinct arrays.
#[allow(clippy::similar_names, clippy::too_many_lines)]
fn process_emphasis(nodes: &mut [Node], stack_bottom: usize, ext: Extensions, markdown: bool) {
    // Build the delimiter list: one entry per Node::Delimiter in [stack_bottom..] that is an
    // emphasis-class delimiter (not a bracket opener).
    let mut delims: Vec<DelimEntry> = nodes
        .iter()
        .enumerate()
        .skip(stack_bottom)
        .filter_map(|(ni, node)| match node {
            Node::Delimiter(d) if is_delimiter_char(d.ch) => Some(DelimEntry {
                node_index: ni,
                ch: d.ch,
                count: d.count,
                can_open: d.can_open,
                can_close: d.can_close,
                prev: None,
                next: None,
            }),
            _ => None,
        })
        .collect();

    // Thread every entry into one doubly-linked list, in build order.
    let count = delims.len();
    for (i, entry) in delims.iter_mut().enumerate() {
        entry.prev = i.checked_sub(1);
        entry.next = (i + 1 < count).then_some(i + 1);
    }
    // `head` is the first still-active entry; it moves forward only when the entry at the front is
    // unlinked. Used solely for the final sweep that literalizes leftover delimiters.
    let mut head: Option<usize> = (count > 0).then_some(0);

    // `openers_bottom[bucket]` is the minimum delimiter-list index to search for an opener.
    //
    // Bucket key: `(char_index, count_mod3, can_also_open, long_enough_for_two)`.
    // The first three fields follow the spec directly (§A: "indexed to the length of the
    // closing delimiter run modulo 3 and to whether the closing delimiter can also be an opener").
    // The fourth — `closer_count >= 2` — is required for `~` when strikeout is on but subscript
    // is off: `match_use_count` returns `None` for a length-1 tilde pair, so a length-1 closer
    // must not share an `openers_bottom` slot with a length-2+ closer. Any future delimiter kind
    // whose opener acceptance depends on a count threshold must derive its slot key from the same
    // invariant: two closers may share a slot only if every opener accepts or rejects them
    // identically.
    let mut openers_bottom = std::collections::BTreeMap::<(u8, usize, bool, bool), usize>::new();

    let mut current: Option<usize> = head; // active-list cursor, walks forward via `next`

    while let Some(cur) = current {
        let Some(current_entry) = delims.get(cur) else {
            break;
        };
        let (closer_ch, closer_count, closer_can_open, closer_can_close) = (
            current_entry.ch,
            current_entry.count,
            current_entry.can_open,
            current_entry.can_close,
        );
        let closer_ni = current_entry.node_index;
        let cur_next = current_entry.next;
        let scan_start = current_entry.prev;
        if !closer_can_close {
            current = cur_next;
            continue;
        }

        let bucket = (
            closer_ch,
            closer_count % 3,
            closer_can_open,
            closer_count >= 2,
        );
        let bottom = *openers_bottom.get(&bucket).unwrap_or(&0);

        // Scan backward through active openers, down to and including `bottom`, for a match.
        let mut found: Option<usize> = None; // delimiter-list index of the matched opener
        let mut scan = scan_start;
        while let Some(si) = scan {
            if si < bottom {
                break;
            }
            let Some(entry) = delims.get(si) else {
                break;
            };
            let scan_prev = entry.prev;
            if !entry.can_open || entry.ch != closer_ch {
                scan = scan_prev;
                continue;
            }
            // The markdown dialect treats an emphasis run of four or more `*`/`_` as inert: it
            // opens no emphasis and stays literal. Only runs of one to three open (one emphasis,
            // one strong, or a strong wrapping an emphasis).
            if markdown && markdown_opener_inert(closer_ch, entry.count) {
                scan = scan_prev;
                continue;
            }
            // Rule of 3 and match_use_count check — we need a temporary Delimiter value to
            // reuse `emphasis_match`, which borrows `nodes` by index.
            let Some(use_count) =
                match_use_count_md(entry.count, closer_count, closer_ch, ext, markdown)
            else {
                // `match_use_count` rejected this opener; keep scanning — do not advance
                // `openers_bottom` for this slot just because one opener was rejected.
                scan = scan_prev;
                continue;
            };
            // Re-derive the Delimiter from `nodes` for the rule-of-3 check.
            let ni = entry.node_index;
            let rule_ok = match nodes.get(ni) {
                Some(Node::Delimiter(d)) => emphasis_match(d, nodes, closer_ni),
                _ => false,
            };
            if rule_ok {
                // The markdown dialect forbids whitespace inside a superscript or subscript: if the
                // span between this opener and the closer carries any, the pair does not match and
                // the scan continues looking for a tighter opener.
                if markdown
                    && rejects_inner_space(closer_ch, use_count)
                    && nodes.get(ni + 1..closer_ni).is_some_and(nodes_carry_break)
                {
                    scan = scan_prev;
                    continue;
                }
                // In markdown a single `*`/`_` and a doubled one never pair across an emphasis run:
                // a lone delimiter cannot draw from a two-delimiter run (which is wholly a strong
                // marker), and vice versa, so the run stays literal.
                if markdown && markdown_emphasis_runs_mismatch(closer_ch, entry.count, closer_count)
                {
                    scan = scan_prev;
                    continue;
                }
                found = Some(si);
                break;
            }
            scan = scan_prev;
        }

        let Some(opener_di) = found else {
            // No opener found: advance openers_bottom to exclude this closer's position in future
            // searches for the same bucket. Node indices are stable, so no adjustment is needed on
            // later matches.
            openers_bottom.insert(bucket, cur);
            // A delimiter that can't open is now known to be inert as a closer too.
            if !closer_can_open {
                convert_delimiter_to_text(nodes, closer_ni);
            }
            current = cur_next;
            continue;
        };

        // --- Match found: fold the inner span into a wrapping inline, leaving node indices stable ---

        let Some(opener_entry) = delims.get(opener_di) else {
            break;
        };
        let (opener_ni, opener_count) = (opener_entry.node_index, opener_entry.count);

        // Retrieve use_count (already validated above).
        let use_count =
            match_use_count_md(opener_count, closer_count, closer_ch, ext, markdown).unwrap_or(1);

        // Move the nodes strictly between opener and closer into the wrapped inline, leaving
        // tombstones behind so every surviving delimiter keeps its node_index.
        let mut inner: Vec<Node> = Vec::new();
        for slot in nodes
            .get_mut(opener_ni + 1..closer_ni)
            .into_iter()
            .flatten()
        {
            inner.push(std::mem::replace(slot, Node::Empty));
        }
        let content = collapse(inner);
        let wrapped = wrap_emphasis(closer_ch, use_count, content);
        // The opener and closer are separate runs, so at least one node sits between them: this
        // slot is never the closer's own node.
        if let Some(slot) = nodes.get_mut(opener_ni + 1) {
            *slot = Node::Inline(wrapped);
        }

        // Decrement both runs; mirror the change into the list entries.
        decrement_delimiter(nodes, closer_ni, use_count);
        decrement_delimiter(nodes, opener_ni, use_count);
        let new_closer_count = closer_count.saturating_sub(use_count);
        let new_opener_count = opener_count.saturating_sub(use_count);
        if let Some(e) = delims.get_mut(cur) {
            e.count = new_closer_count;
        }
        if let Some(e) = delims.get_mut(opener_di) {
            e.count = new_opener_count;
        }

        // The list entry following the closer — the resume point when both runs are spent.
        let after_closer = delims.get(cur).and_then(|e| e.next);

        // Unlink every delimiter strictly between opener and closer at once: they were folded into
        // the wrap and can never match again.
        if let Some(e) = delims.get_mut(opener_di) {
            e.next = Some(cur);
        }
        if let Some(e) = delims.get_mut(cur) {
            e.prev = Some(opener_di);
        }

        // Unlink a spent run and blank its node. Closer first (it is the higher node index).
        let closer_empty = new_closer_count == 0;
        let opener_empty = new_opener_count == 0;
        if closer_empty {
            unlink_delim(&mut delims, &mut head, cur);
            if let Some(slot) = nodes.get_mut(closer_ni) {
                *slot = Node::Empty;
            }
        }
        if opener_empty {
            unlink_delim(&mut delims, &mut head, opener_di);
            if let Some(slot) = nodes.get_mut(opener_ni) {
                *slot = Node::Empty;
            }
        }

        // Resume at the surviving opener if it lives (it may match a still-earlier closer), else the
        // surviving closer (it may match a still-earlier opener), else the entry past the closer.
        current = if !opener_empty {
            Some(opener_di)
        } else if !closer_empty {
            Some(cur)
        } else {
            after_closer
        };
    }

    // Any delimiter still on the active list never matched: it reverts to literal text.
    let mut leftover = head;
    while let Some(i) = leftover {
        let Some(entry) = delims.get(i) else {
            break;
        };
        let next = entry.next;
        convert_delimiter_to_text(nodes, entry.node_index);
        leftover = next;
    }
}

/// Unlink `i` from the doubly-linked delimiter list, advancing `head` if `i` was at the front.
fn unlink_delim(delims: &mut [DelimEntry], head: &mut Option<usize>, i: usize) {
    let (prev, next) = match delims.get(i) {
        Some(entry) => (entry.prev, entry.next),
        None => return,
    };
    match prev {
        Some(pi) => {
            if let Some(prev_entry) = delims.get_mut(pi) {
                prev_entry.next = next;
            }
        }
        None => *head = next,
    }
    if let Some(ni) = next
        && let Some(next_entry) = delims.get_mut(ni)
    {
        next_entry.prev = prev;
    }
}

/// Resolve `==`-delimited highlight runs into `Span` inlines carrying the `mark` class.
///
/// A run is delimited by two `=` on each side. Scanning left to right, each `=` closer pairs with
/// the nearest preceding `=` opener; the pair consumes exactly two `=` from each side and the inner
/// nodes — with their own emphasis resolved — become the span's content. Any `=` left over on either
/// side, or a lone `=`, stays literal text. Resolving here, ahead of the shared emphasis pass, keeps
/// each run to a single span: leftover `=` do not re-pair into nested marks.
fn resolve_mark(nodes: &mut Vec<Node>, ext: Extensions, markdown: bool) {
    let mut current = 0usize;
    while current < nodes.len() {
        let is_closer = matches!(
            nodes.get(current),
            Some(Node::Delimiter(d)) if d.ch == b'=' && d.can_close && d.count >= 2
        );
        if !is_closer {
            current += 1;
            continue;
        }
        // Find the nearest preceding `=` opener with at least two delimiters.
        let mut opener = None;
        for i in (0..current).rev() {
            if matches!(
                nodes.get(i),
                Some(Node::Delimiter(d)) if d.ch == b'=' && d.can_open && d.count >= 2
            ) {
                opener = Some(i);
                break;
            }
        }
        let Some(opener_ni) = opener else {
            current += 1;
            continue;
        };

        let inner: Vec<Node> = nodes.drain(opener_ni + 1..current).collect();
        let mut inner = inner;
        process_emphasis(&mut inner, 0, ext, markdown);
        let content = collapse(inner);
        let span = Inline::Span(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["mark".into()],
                attributes: Vec::new(),
            }),
            content,
        );
        // After the drain, the closer sits directly after the opener.
        let closer_ni = opener_ni + 1;
        nodes.insert(closer_ni, Node::Inline(span));
        // The closer has shifted one further along by the insert.
        let closer_ni = opener_ni + 2;

        // Consume two `=` from each delimiter; convert any remainder to literal text, drop empties.
        consume_mark_side(nodes, closer_ni);
        consume_mark_side(nodes, opener_ni);

        // Resume scanning from the opener position: nodes there are now resolved, so re-derive.
        current = opener_ni;
    }

    // Any `=` delimiter that never formed a span reverts to literal text.
    for i in 0..nodes.len() {
        if matches!(nodes.get(i), Some(Node::Delimiter(d)) if d.ch == b'=') {
            convert_delimiter_to_text(nodes, i);
        }
    }
}

/// Take two `=` off the delimiter node at `index`: a remainder of zero removes the node, otherwise
/// it becomes literal text of the remaining `=`. Returns nothing; callers index high-to-low so the
/// node positions they still hold stay valid (the opener is below the closer).
fn consume_mark_side(nodes: &mut Vec<Node>, index: usize) {
    let remainder = match nodes.get(index) {
        Some(Node::Delimiter(d)) => d.count.saturating_sub(2),
        _ => return,
    };
    if remainder == 0 {
        nodes.remove(index);
    } else if let Some(node) = nodes.get_mut(index) {
        *node = Node::Text("=".repeat(remainder));
    }
}

/// Whether `ch` names a delimiter run resolved by [`process_emphasis`].
fn is_delimiter_char(ch: u8) -> bool {
    matches!(ch, b'*' | b'_' | b'~' | b'^' | b'\'' | b'"' | b'=')
}

/// Whether `ch` is a smart-quote delimiter (`'` or `"`).
fn is_quote(ch: u8) -> bool {
    matches!(ch, b'\'' | b'"')
}

/// Open/close eligibility for a delimiter run, dispatching to the smart-quote rule for `'`/`"` and
/// to the emphasis rule for everything else.
fn run_flanking(
    ch: u8,
    before: Option<char>,
    after: Option<char>,
    relax_underscore: bool,
) -> (bool, bool) {
    if is_quote(ch) {
        quote_flanking(ch, before, after)
    } else if ch == b'_' && relax_underscore {
        // Pair underscores by the same boundary rule as `*`, which permits intraword emphasis.
        flanking(b'*', before, after)
    } else {
        flanking(ch, before, after)
    }
}

/// How many delimiters a matched opener/closer pair consumes, or `None` when the enabled extensions
/// give the pair no meaning (so the search must look further or leave the run literal).
///
/// `*`/`_` consume two when both runs can (strong) else one (emphasis). `^` consumes one per layer
/// (superscript). `~` consumes two for a strikeout when both runs allow it and `strikeout` is on,
/// otherwise one for a subscript when `subscript` is on; with neither it is not a delimiter.
fn match_use_count(
    opener_count: usize,
    closer_count: usize,
    ch: u8,
    ext: Extensions,
) -> Option<usize> {
    let both_at_least_two = opener_count >= 2 && closer_count >= 2;
    match ch {
        b'*' | b'_' => Some(if both_at_least_two { 2 } else { 1 }),
        b'^' | b'\'' | b'"' => Some(1),
        b'~' => {
            if both_at_least_two && ext.contains(Extension::Strikeout) {
                Some(2)
            } else if ext.contains(Extension::Subscript) {
                Some(1)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Scan an inline link tail `(destination "title")` in the markdown dialect, where the unbracketed
/// destination may hold spaces (percent-encoded to `%20`) and balanced parentheses. The destination
/// runs until the parenthesis that balances the link's opener, save for a trailing quoted title
/// separated by whitespace. Returns `None` when the parentheses are unbalanced or a quoted title is
/// not immediately followed by the closing parenthesis. `pos` points at the opening `(`.
fn scan_markdown_inline_target(text: &str, pos: usize) -> Option<(Target, usize)> {
    let mut index = pos + 1;
    skip_target_whitespace(text, &mut index);
    if char_at(text, index) == Some('<') {
        // The angle-bracketed form has no special space handling; defer to the shared scanner,
        // which already reads `<...>` destinations and an optional title.
        return scan_inline_target(text, pos);
    }
    let mut url = String::new();
    let mut title = String::new();
    let mut depth: usize = 0;
    loop {
        match char_at(text, index) {
            None => return None,
            Some(')') if depth == 0 => {
                index += 1;
                break;
            }
            Some(')') => {
                depth -= 1;
                url.push(')');
                index += 1;
            }
            Some('(') => {
                depth += 1;
                url.push('(');
                index += 1;
            }
            // An escaped space is always part of the destination — never a title separator — and
            // encodes as `%20` like any other destination space.
            Some('\\') if matches!(char_at(text, index + 1), Some(' ' | '\t')) => {
                url.push_str("%20");
                index += 2;
            }
            Some('\\') if char_at(text, index + 1).is_some_and(is_ascii_punctuation) => {
                if let Some(next) = char_at(text, index + 1) {
                    url.push('\\');
                    url.push(next);
                    index += 1 + next.len_utf8();
                }
            }
            Some(ch) if ch == ' ' || ch == '\t' => {
                let mut after = index;
                skip_target_whitespace(text, &mut after);
                match char_at(text, after) {
                    // Trailing whitespace before the closing parenthesis ends the destination.
                    Some(')') if depth == 0 => {
                        index = after;
                    }
                    // A quoted title separated by whitespace ends the destination. It must be the
                    // last element before the closing parenthesis, else the whole tail fails.
                    Some('"' | '\'') if depth == 0 => {
                        let (parsed, mut close) = scan_target_title(text, after)?;
                        title = parsed;
                        skip_target_whitespace(text, &mut close);
                        if char_at(text, close) != Some(')') {
                            return None;
                        }
                        index = close + 1;
                        break;
                    }
                    // More destination follows: the whitespace run joins it as a single `%20`.
                    Some(_) => {
                        url.push_str("%20");
                        index = after;
                    }
                    None => return None,
                }
            }
            Some(ch) => {
                url.push(ch);
                index += ch.len_utf8();
            }
        }
    }
    Some((
        Target {
            url: unescape_string(&url).into(),
            title: unescape_string(&title).into(),
        },
        index,
    ))
}

/// Advance `index` past a run of spaces and tabs.
fn skip_target_whitespace(text: &str, index: &mut usize) {
    while matches!(char_at(text, *index), Some(' ' | '\t')) {
        *index += 1;
    }
}

/// Scan a quoted link title starting at `start` (a `"` or `'`), returning its raw content and the
/// index just past the closing quote. A backslash escapes the following punctuation character.
fn scan_target_title(text: &str, start: usize) -> Option<(String, usize)> {
    let close = char_at(text, start)?;
    if close != '"' && close != '\'' {
        return None;
    }
    let mut index = start + 1;
    let mut out = String::new();
    while let Some(ch) = char_at(text, index) {
        if ch == close {
            return Some((out, index + 1));
        }
        if ch == '\\'
            && let Some(next) = char_at(text, index + 1)
            && is_ascii_punctuation(next)
        {
            out.push('\\');
            out.push(next);
            index += 1 + next.len_utf8();
            continue;
        }
        out.push(ch);
        index += ch.len_utf8();
    }
    None
}

/// Tag an explicit angle-bracket autolink with its kind. A link whose visible text differs from its
/// destination is an email autolink (the destination gained a `mailto:` scheme), so it carries the
/// `email` class; every other angle autolink is a URI autolink and carries `uri`. Bare URLs linked
/// by the autolink post-pass keep an empty class list and never reach here.
fn classify_angle_autolink(inline: Inline) -> Inline {
    let Inline::Link(mut attr, text, target) = inline else {
        return inline;
    };
    let is_email = matches!(text.first(), Some(Inline::Str(shown)) if *shown != target.url);
    attr.classes
        .push(if is_email { "email" } else { "uri" }.into());
    Inline::Link(attr, text, target)
}

/// Percent-encode the destination of a link, leaving its shown text untouched.
fn escape_link_destination(inline: Inline) -> Inline {
    let Inline::Link(attr, text, mut target) = inline else {
        return inline;
    };
    target.url = escape_uri(&target.url).into();
    Inline::Link(attr, text, target)
}

/// Whether a `*`/`_` run is too long to open emphasis in the markdown dialect. A run there denotes
/// at most a strong wrapping an emphasis (three delimiters); four or more open nothing and the run
/// stays literal.
fn markdown_opener_inert(ch: u8, count: usize) -> bool {
    matches!(ch, b'*' | b'_') && count > 3
}

/// Whether a `*`/`_` opener and closer have run lengths that cannot pair in the markdown dialect.
/// A run of one delimiter (an emphasis marker) and a run of two (a strong marker) never match each
/// other: a lone delimiter cannot close against a strong marker, nor a strong marker against a lone
/// one, so the pairing fails and both runs stay literal.
fn markdown_emphasis_runs_mismatch(ch: u8, opener_count: usize, closer_count: usize) -> bool {
    matches!(ch, b'*' | b'_')
        && ((opener_count == 1 && closer_count == 2) || (opener_count == 2 && closer_count == 1))
}

/// How many delimiters a matched pair consumes, accounting for the markdown dialect's emphasis
/// rule. For `*`/`_` in markdown, a pair whose opener and closer both still have three or more
/// delimiters consumes a single one first, so the emphasis it forms nests inside the strong that
/// the remaining pair forms — a triple run resolves to a strong wrapping an emphasis. Every other
/// pairing defers to [`match_use_count`].
fn match_use_count_md(
    opener_count: usize,
    closer_count: usize,
    ch: u8,
    ext: Extensions,
    markdown: bool,
) -> Option<usize> {
    if markdown && matches!(ch, b'*' | b'_') && opener_count >= 3 && closer_count >= 3 {
        return Some(1);
    }
    // A symmetric run of three or more tildes resolves to a single subscript when its length is
    // odd: the whole run is consumed and the subscript does not nest a strikeout inside it.
    if markdown
        && ch == b'~'
        && ext.contains(Extension::Subscript)
        && opener_count == closer_count
        && opener_count >= 3
        && opener_count % 2 == 1
    {
        return Some(opener_count);
    }
    match_use_count(opener_count, closer_count, ch, ext)
}

/// Wrap `content` in the inline a matched delimiter pair denotes, given its character and the number
/// of delimiters consumed.
fn wrap_emphasis(ch: u8, use_count: usize, content: Vec<Inline>) -> Inline {
    match (ch, use_count) {
        (b'\'', _) => Inline::Quoted(QuoteType::SingleQuote, content),
        (b'"', _) => Inline::Quoted(QuoteType::DoubleQuote, content),
        (b'~', 2) => Inline::Strikeout(content),
        (b'~', _) => Inline::Subscript(content),
        (b'^', _) => Inline::Superscript(content),
        (_, 2) => Inline::Strong(content),
        (_, _) => Inline::Emph(content),
    }
}

/// Whether a matched delimiter pair forms a superscript or a subscript — the spans the markdown
/// dialect forbids from holding whitespace. A double tilde is a strikeout, which may, so only a
/// single tilde counts.
fn rejects_inner_space(ch: u8, use_count: usize) -> bool {
    ch == b'^' || (ch == b'~' && use_count == 1)
}

/// Whether any node in the slice carries whitespace that, in the markdown dialect, ends a
/// superscript or subscript: a space or tab in text, or a soft or hard line break. A non-breaking
/// space — what an escaped space becomes — does not count, so an escaped space keeps the span open.
fn nodes_carry_break(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::Text(text) => text.chars().any(|c| c == ' ' || c == '\t'),
        Node::SoftBreak | Node::LineBreak => true,
        Node::Inline(inline) => inline_carries_break(inline),
        Node::Delimiter(_) | Node::Empty => false,
    })
}

/// The [`nodes_carry_break`] test for an already-built inline, recursing through the inline
/// containers a superscript or subscript may nest.
fn inline_carries_break(inline: &Inline) -> bool {
    match inline {
        Inline::Space | Inline::SoftBreak | Inline::LineBreak => true,
        Inline::Str(text) => text.chars().any(|c| c == ' ' || c == '\t'),
        Inline::Emph(content)
        | Inline::Underline(content)
        | Inline::Strong(content)
        | Inline::Strikeout(content)
        | Inline::Superscript(content)
        | Inline::Subscript(content)
        | Inline::SmallCaps(content)
        | Inline::Quoted(_, content)
        | Inline::Cite(_, content)
        | Inline::Link(_, content, _)
        | Inline::Image(_, content, _)
        | Inline::Span(_, content) => content.iter().any(inline_carries_break),
        _ => false,
    }
}

fn emphasis_match(opener: &Delimiter, nodes: &[Node], closer: usize) -> bool {
    let Some(Node::Delimiter(closer_delim)) = nodes.get(closer) else {
        return false;
    };
    // Rule of 3: when either run can both open and close, their combined length must not be a
    // multiple of 3 unless both lengths are themselves multiples of 3.
    let either_both =
        (opener.can_open && opener.can_close) || (closer_delim.can_open && closer_delim.can_close);
    if either_both {
        let sum = opener.count + closer_delim.count;
        if sum.is_multiple_of(3)
            && (!opener.count.is_multiple_of(3) || !closer_delim.count.is_multiple_of(3))
        {
            return false;
        }
    }
    true
}

/// The literal text an unmatched delimiter run reverts to. An unmatched smart quote becomes a curly
/// quote — a single quote closes (`’`) and a double quote opens (`“`); every other delimiter is its
/// own character repeated.
fn delimiter_literal(ch: u8, count: usize) -> String {
    match ch {
        b'\'' => "\u{2019}".repeat(count),
        b'"' => "\u{201c}".repeat(count),
        _ => std::iter::repeat_n(ch as char, count).collect(),
    }
}

fn decrement_delimiter(nodes: &mut [Node], index: usize, by: usize) {
    if let Some(Node::Delimiter(d)) = nodes.get_mut(index) {
        d.count = d.count.saturating_sub(by);
    }
}

fn convert_delimiter_to_text(nodes: &mut [Node], index: usize) {
    if let Some(node) = nodes.get_mut(index)
        && let Node::Delimiter(d) = node
        && is_delimiter_char(d.ch)
    {
        *node = Node::Text(delimiter_literal(d.ch, d.count));
    }
}

/// Pair literal `<span …>` / `</span>` raw-inline tags into [`Inline::Span`] nodes.
///
/// The inline phase first leaves both tags as raw inline HTML, so emphasis and links resolve around
/// them exactly as they would around any other tag. This pass then walks the resolved tree and,
/// at each nesting level independently, matches an opening tag with the nearest later closing tag,
/// wrapping the inlines between them in a span whose attributes come from the opening tag. Matching
/// stays within one level: a `<span>` that emphasis pulled inside an `Emph` only pairs with a
/// `</span>` that landed inside that same `Emph`. The content between a matched pair is itself
/// re-paired, so nested spans nest. Unmatched tags keep their raw-inline form.
///
/// Known limitation: when an emphasis run straddles exactly one of the two tags — the run opens
/// before a `<span>` and its closing marker sits just before the matching `</span>` — the two tags
/// can land at different levels and stay raw even though a span could have formed.
fn pair_native_spans(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut input = inlines.into_iter().peekable();
    pair_spans_level(&mut input, false)
}

/// Pair spans within one nesting level. Pulls from `input` until it is drained, or — when
/// `stop_at_close` is set — until an unmatched closing `</span>` tag is reached, which is left
/// unconsumed for the caller to handle. Container inlines have their own children re-paired.
fn pair_spans_level(
    input: &mut std::iter::Peekable<std::vec::IntoIter<Inline>>,
    stop_at_close: bool,
) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    while let Some(item) = input.peek() {
        if let Inline::RawInline(format, text) = item
            && format.0 == "html"
        {
            match classify_span_tag(text) {
                SpanTag::Open(attr) => {
                    let _ = input.next();
                    let inner = pair_spans_level(input, true);
                    if matches!(input.peek(), Some(Inline::RawInline(f, t))
                        if f.0 == "html" && matches!(classify_span_tag(t), SpanTag::Close))
                    {
                        let _ = input.next();
                        out.push(Inline::Span(Box::new(attr), inner));
                    } else {
                        // No matching close at this level: the opener reverts to raw, and its
                        // gathered inner content rejoins the stream.
                        out.push(Inline::RawInline(
                            carta_ast::Format("html".into()),
                            open_tag_raw(&attr).into(),
                        ));
                        out.extend(inner);
                    }
                    continue;
                }
                SpanTag::Close if stop_at_close => break,
                _ => {}
            }
        }
        if let Some(next) = input.next() {
            out.push(recurse_span_children(next));
        }
    }
    out
}

/// Re-pair spans inside the child lists of a container inline.
fn recurse_span_children(inline: Inline) -> Inline {
    match inline {
        Inline::Emph(c) => Inline::Emph(pair_native_spans(c)),
        Inline::Underline(c) => Inline::Underline(pair_native_spans(c)),
        Inline::Strong(c) => Inline::Strong(pair_native_spans(c)),
        Inline::Strikeout(c) => Inline::Strikeout(pair_native_spans(c)),
        Inline::Superscript(c) => Inline::Superscript(pair_native_spans(c)),
        Inline::Subscript(c) => Inline::Subscript(pair_native_spans(c)),
        Inline::SmallCaps(c) => Inline::SmallCaps(pair_native_spans(c)),
        Inline::Quoted(q, c) => Inline::Quoted(q, pair_native_spans(c)),
        Inline::Cite(cites, c) => Inline::Cite(cites, pair_native_spans(c)),
        Inline::Link(a, c, t) => Inline::Link(a, pair_native_spans(c), t),
        Inline::Image(a, c, t) => Inline::Image(a, pair_native_spans(c), t),
        Inline::Span(a, c) => Inline::Span(a, pair_native_spans(c)),
        other => other,
    }
}

/// The role of an HTML tag with respect to span pairing.
enum SpanTag {
    /// A literal `<span …>` opener, with its attributes parsed.
    Open(Attr),
    /// A literal `</span>` closer.
    Close,
    /// Any other tag, which plays no part in span pairing.
    Other,
}

/// Cheap pre-check before the char-by-char classification: does `raw` open with `<span` or `</span`
/// (case-insensitive)? Lets the common non-span inline tag bail out without allocating.
fn opens_span_tag(raw: &str) -> bool {
    let Some(after_lt) = raw.strip_prefix('<') else {
        return false;
    };
    let candidate = after_lt.strip_prefix('/').unwrap_or(after_lt);
    candidate
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("span"))
}

/// Classify a raw HTML tag string, parsing attributes for an opening `<span …>`. A self-closing
/// `<span/>` is `Other`: it has no content to wrap and stays raw.
fn classify_span_tag(raw: &str) -> SpanTag {
    if !opens_span_tag(raw) {
        return SpanTag::Other;
    }
    if char_at(raw, 1) == Some('/') {
        // `</span>` with optional trailing whitespace before `>`.
        let mut i = 2;
        if !matches_name(raw, &mut i, "span") {
            return SpanTag::Other;
        }
        while matches!(char_at(raw, i), Some(' ' | '\t' | '\n')) {
            i += 1;
        }
        if char_at(raw, i) == Some('>') && i + 1 == raw.len() {
            return SpanTag::Close;
        }
        return SpanTag::Other;
    }
    let mut i = 1;
    if !matches_name(raw, &mut i, "span") {
        return SpanTag::Other;
    }
    // A name character right after `span` means a different tag (`<spanner>`).
    if matches!(char_at(raw, i), Some(c) if c.is_ascii_alphanumeric() || c == '-') {
        return SpanTag::Other;
    }
    match parse_span_attributes(raw, i) {
        Some(attr) => SpanTag::Open(attr),
        None => SpanTag::Other,
    }
}

/// Match the literal `name` case-insensitively at `*i`, advancing `*i` past it on success. `name`
/// is ASCII, so its character offsets are byte offsets.
fn matches_name(text: &str, i: &mut usize, name: &str) -> bool {
    for (offset, expected) in name.chars().enumerate() {
        match char_at(text, *i + offset) {
            Some(c) if c.eq_ignore_ascii_case(&expected) => {}
            _ => return false,
        }
    }
    *i += name.len();
    true
}

/// Parse the attributes of an opening `<span …>` tag whose name ends at `start`, expecting the tag
/// to end with `>` (a trailing `/` makes it self-closing, which is rejected here). An `id` attribute
/// becomes the identifier and a `class` attribute splits into classes; only the first of each is
/// kept. Every other attribute becomes a key/value pair in source order; a valueless attribute
/// carries an empty value. Entity and numeric character references in values are decoded.
fn parse_span_attributes(text: &str, start: usize) -> Option<Attr> {
    let mut attr = Attr::default();
    let mut seen_class = false;
    let mut i = start;
    loop {
        let ws_start = i;
        while matches!(char_at(text, i), Some(' ' | '\t' | '\n')) {
            i += 1;
        }
        match char_at(text, i) {
            Some('>') if i + 1 == text.len() => return Some(attr),
            // A self-closing tag has no content to wrap.
            Some('/') => return None,
            _ => {}
        }
        // An attribute must be preceded by whitespace.
        if i == ws_start {
            return None;
        }
        let name_start = i;
        while matches!(
            char_at(text, i),
            Some(c) if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')
        ) {
            i += 1;
        }
        if i == name_start {
            return None;
        }
        let name = text.get(name_start..i)?.to_owned();
        let mut value = String::new();
        // Optional `= value` with whitespace allowed around `=`.
        let mut after = i;
        while matches!(char_at(text, after), Some(' ' | '\t' | '\n')) {
            after += 1;
        }
        if char_at(text, after) == Some('=') {
            after += 1;
            while matches!(char_at(text, after), Some(' ' | '\t' | '\n')) {
                after += 1;
            }
            let (parsed, next) = read_attr_value(text, after)?;
            value = parsed;
            i = next;
        } else {
            i = after;
        }
        match name.as_str() {
            "id" => {
                if attr.id.is_empty() {
                    attr.id = value.into();
                }
            }
            "class" => {
                if !seen_class {
                    seen_class = true;
                    attr.classes = value.split_whitespace().map(Into::into).collect();
                }
            }
            _ => attr.attributes.push((name.into(), value.into())),
        }
    }
}

/// Read an HTML attribute value at `start`: a double- or single-quoted string, or an unquoted run.
/// Returns the decoded value and the index just past it. Character references inside the value are
/// decoded.
fn read_attr_value(text: &str, start: usize) -> Option<(String, usize)> {
    let quote = char_at(text, start);
    if matches!(quote, Some('"' | '\'')) {
        let quote = quote?;
        let mut i = start + 1;
        let mut out = String::new();
        loop {
            match char_at(text, i) {
                Some(c) if c == quote => return Some((out, i + 1)),
                Some('&') => {
                    if let Some((decoded, next)) = scan_entity(text, i) {
                        out.push_str(&decoded);
                        i = next;
                    } else {
                        out.push('&');
                        i += 1;
                    }
                }
                Some(c) => {
                    out.push(c);
                    i += c.len_utf8();
                }
                None => return None,
            }
        }
    }
    // Unquoted value: a run with no whitespace, quotes, `=`, `<`, `>`, or backtick.
    let mut i = start;
    let mut out = String::new();
    while let Some(c) = char_at(text, i) {
        if matches!(c, ' ' | '\t' | '\n' | '"' | '\'' | '=' | '<' | '>' | '`') {
            break;
        }
        if c == '&'
            && let Some((decoded, next)) = scan_entity(text, i)
        {
            out.push_str(&decoded);
            i = next;
            continue;
        }
        out.push(c);
        i += c.len_utf8();
    }
    if out.is_empty() {
        return None;
    }
    Some((out, i))
}

/// Reconstruct the raw `<span …>` opener for an opener that found no matching close, so it falls
/// back to literal raw inline HTML. The exact original spelling is not recovered; a normalized form
/// carrying the same attributes is emitted.
fn open_tag_raw(attr: &Attr) -> String {
    let mut s = String::from("<span");
    if !attr.id.is_empty() {
        s.push_str(" id=\"");
        s.push_str(&attr.id);
        s.push('"');
    }
    if !attr.classes.is_empty() {
        s.push_str(" class=\"");
        s.push_str(&attr.classes.join(" "));
        s.push('"');
    }
    for (k, v) in &attr.attributes {
        s.push(' ');
        s.push_str(k);
        s.push_str("=\"");
        s.push_str(v);
        s.push('"');
    }
    s.push('>');
    s
}

/// Collapse the node list into final inlines: leftover delimiters become text, adjacent text is
/// merged, and text is split into `Str`/`Space` runs.
fn collapse(nodes: Vec<Node>) -> Vec<Inline> {
    let mut text = String::new();
    let mut out: Vec<Inline> = Vec::new();
    let flush = |text: &mut String, out: &mut Vec<Inline>| {
        if !text.is_empty() {
            push_text_inlines(out, text);
            text.clear();
        }
    };
    for node in nodes {
        match node {
            Node::Text(t) => text.push_str(&t),
            Node::Delimiter(d) => {
                // An unmatched image opener carries its `!` in the `image` flag rather than a
                // separate node, so restore it when the bracket reverts to literal text.
                if d.image {
                    text.push('!');
                }
                text.push_str(&delimiter_literal(d.ch, d.count));
            }
            Node::Inline(inline) => {
                flush(&mut text, &mut out);
                out.push(inline);
            }
            Node::SoftBreak => {
                flush(&mut text, &mut out);
                out.push(Inline::SoftBreak);
            }
            Node::LineBreak => {
                flush(&mut text, &mut out);
                out.push(Inline::LineBreak);
            }
            Node::Empty => {}
        }
    }
    flush(&mut text, &mut out);
    out
}

/// Split a text run into `Str` tokens separated by `Space` inlines, collapsing each run of
/// spaces to a single `Space`.
fn push_text_inlines(out: &mut Vec<Inline>, text: &str) {
    let mut chars = text.chars().peekable();
    // Built directly as `Text` so a word short enough for inline storage never touches the heap.
    let mut word = Text::default();
    while let Some(ch) = chars.next() {
        if ch == ' ' {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word)));
            }
            while chars.peek() == Some(&' ') {
                chars.next();
            }
            out.push(Inline::Space);
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word));
    }
}

fn flanking(ch: u8, before: Option<char>, after: Option<char>) -> (bool, bool) {
    let before_ws = before.is_none_or(is_unicode_whitespace);
    let after_ws = after.is_none_or(is_unicode_whitespace);
    let before_punct = before.is_some_and(is_punctuation);
    let after_punct = after.is_some_and(is_punctuation);

    let left_flanking = !after_ws && (!after_punct || before_ws || before_punct);
    let right_flanking = !before_ws && (!before_punct || after_ws || after_punct);

    match ch {
        b'_' => {
            let can_open = left_flanking && (!right_flanking || before_punct);
            let can_close = right_flanking && (!left_flanking || after_punct);
            (can_open, can_close)
        }
        // Subscript/superscript/strikeout delimiters anchor only on whitespace: a run opens unless
        // a space follows it and closes unless a space precedes it. The rule-of-three guard
        // (`emphasis_match`) still applies on top of this.
        b'~' | b'^' => (!after_ws, !before_ws),
        _ => (left_flanking, right_flanking),
    }
}

/// Open/close eligibility for a smart-quote run at a boundary. A run opens only when it is
/// left-flanking and not glued to a preceding letter or digit, and closes only when it is
/// right-flanking and not glued to a following letter or digit. The leftover-curly fallback then
/// turns an unmatched single quote into an apostrophe and an unmatched double quote into an opener.
fn quote_flanking(_ch: u8, before: Option<char>, after: Option<char>) -> (bool, bool) {
    let before_ws = before.is_none_or(is_unicode_whitespace);
    let after_ws = after.is_none_or(is_unicode_whitespace);
    let before_punct = before.is_some_and(is_punctuation);
    let after_punct = after.is_some_and(is_punctuation);
    let before_alnum = before.is_some_and(char::is_alphanumeric);
    let after_alnum = after.is_some_and(char::is_alphanumeric);

    let left_flanking = !after_ws && (!after_punct || before_ws || before_punct);
    let right_flanking = !before_ws && (!before_punct || after_ws || after_punct);

    let can_open = left_flanking && !before_alnum;
    let can_close = right_flanking && !after_alnum;
    (can_open, can_close)
}

/// The characters an original-Markdown backslash escape recognizes regardless of
/// `all_symbols_escapable`: the inline delimiters and block markers that carry syntactic weight. A
/// dialect without the broad escape set drops the backslash before only these, leaving every other
/// backslash literal.
fn is_classic_markdown_escapable(ch: char) -> bool {
    matches!(
        ch,
        '\\' | '`'
            | '*'
            | '_'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | '>'
            | '#'
            | '+'
            | '-'
            | '.'
            | '!'
    )
}

/// A Unicode punctuation character per the spec: an ASCII punctuation character or anything in the
/// Unicode `P` (punctuation) or `S` (symbol) general categories.
fn is_punctuation(ch: char) -> bool {
    use unicode_general_category::GeneralCategory::{
        ClosePunctuation, ConnectorPunctuation, CurrencySymbol, DashPunctuation, FinalPunctuation,
        InitialPunctuation, MathSymbol, ModifierSymbol, OpenPunctuation, OtherPunctuation,
        OtherSymbol,
    };
    if ch.is_ascii() {
        return is_ascii_punctuation(ch);
    }
    matches!(
        unicode_general_category::get_general_category(ch),
        ConnectorPunctuation
            | DashPunctuation
            | OpenPunctuation
            | ClosePunctuation
            | InitialPunctuation
            | FinalPunctuation
            | OtherPunctuation
            | MathSymbol
            | CurrencySymbol
            | ModifierSymbol
            | OtherSymbol
    )
}

/// Normalize the interior of a code span: line endings to spaces, and if it both begins and ends
/// with a space (and is not all spaces), strip one space from each end.
fn normalize_code(content: &str, markdown: bool) -> String {
    let collapsed: String = content
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    // The markdown dialect strips all surrounding whitespace; the strict dialect removes only a
    // single leading and trailing space, and only when the content is not all spaces.
    if markdown {
        return collapsed.trim().to_owned();
    }
    let bytes = collapsed.as_bytes();
    if collapsed.len() >= 2
        && bytes.first() == Some(&b' ')
        && bytes.last() == Some(&b' ')
        && !collapsed.chars().all(|c| c == ' ')
    {
        collapsed
            .get(1..collapsed.len() - 1)
            .unwrap_or("")
            .to_owned()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TASK_CHECKED, TASK_UNCHECKED, delimiter_literal, emoji, flanking, fold_dash_run,
        fold_ellipsis_run, interesting_chars, match_use_count, parse_meta_inlines, quote_flanking,
        split_header_attr, task_marker_replacement,
    };
    use carta_ast::{Attr, Inline, Target};
    use carta_core::{Extension, Extensions};

    fn is_interesting(table: &[bool; 128], ch: char) -> bool {
        usize::try_from(u32::from(ch))
            .ok()
            .and_then(|code| table.get(code))
            .copied()
            .unwrap_or(false)
    }

    #[test]
    fn interesting_set_tracks_active_extensions() {
        let base = interesting_chars(Extensions::empty());
        assert!(is_interesting(&base, '*'));
        assert!(is_interesting(&base, '['));
        assert!(is_interesting(&base, '\n'));
        assert!(!is_interesting(&base, 'q'));
        assert!(!is_interesting(&base, '-'));
        assert!(!is_interesting(&base, ':'));
        assert!(!is_interesting(&base, 'é'));

        let gated = interesting_chars(exts(&[Extension::Smart, Extension::Emoji]));
        assert!(is_interesting(&gated, '-'));
        assert!(is_interesting(&gated, ':'));
        assert!(is_interesting(&gated, '*'));
        assert!(!is_interesting(&gated, 'q'));
        assert!(!is_interesting(&gated, 'é'));
    }

    fn exts(list: &[Extension]) -> Extensions {
        Extensions::from_list(list)
    }

    fn emoji_span(name: &str, text: &str) -> Inline {
        Inline::Span(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["emoji".to_owned().into()],
                attributes: vec![("data-emoji".to_owned().into(), name.to_owned().into())],
            }),
            vec![Inline::Str(text.to_owned().into())],
        )
    }

    #[test]
    fn emoji_table_is_sorted_for_binary_search() {
        let on = exts(&[Extension::Emoji]);
        // Every entry resolves to its own value through the lookup path; a misordered table would
        // make some entry unreachable by binary search.
        assert_eq!(emoji::lookup("smile"), Some("\u{1f604}"));
        assert_eq!(emoji::lookup("+1"), Some("\u{1f44d}"));
        assert_eq!(emoji::lookup("-1"), Some("\u{1f44e}"));
        // The multi-codepoint heart keeps its variation selector.
        assert_eq!(emoji::lookup("heart"), Some("\u{2764}\u{fe0f}"));
        assert_eq!(emoji::lookup("not_an_emoji_name"), None);
        // Parsing round-trips through the table for a representative name.
        assert_eq!(
            parse_meta_inlines(":rocket:", on, false),
            vec![emoji_span("rocket", "\u{1f680}")]
        );
    }

    #[test]
    fn emoji_resolves_known_shortcodes() {
        let on = exts(&[Extension::Emoji]);
        assert_eq!(
            parse_meta_inlines(":smile:", on, false),
            vec![emoji_span("smile", "\u{1f604}")]
        );
        // A shortcode whose name carries `+`/`-` still resolves.
        assert_eq!(
            parse_meta_inlines(":+1:", on, false),
            vec![emoji_span("+1", "\u{1f44d}")]
        );
    }

    #[test]
    fn emoji_unknown_name_stays_literal() {
        let on = exts(&[Extension::Emoji]);
        // An unrecognized name leaves the colons and text verbatim.
        assert_eq!(
            parse_meta_inlines(":unknown_xyz:", on, false),
            vec![Inline::Str(":unknown_xyz:".to_owned().into())]
        );
        // An empty `::` is not a shortcode.
        assert_eq!(
            parse_meta_inlines("::", on, false),
            vec![Inline::Str("::".to_owned().into())]
        );
    }

    #[test]
    fn emoji_requires_extension() {
        let off = Extensions::empty();
        assert_eq!(
            parse_meta_inlines(":smile:", off, false),
            vec![Inline::Str(":smile:".to_owned().into())]
        );
    }

    fn mark_span(content: Vec<Inline>) -> Inline {
        Inline::Span(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["mark".to_owned().into()],
                attributes: Vec::new(),
            }),
            content,
        )
    }

    #[test]
    fn mark_resolves_inside_link_label() {
        let on = exts(&[Extension::Mark]);
        // A `==…==` run in a link's label resolves to a mark span just as it would at top level.
        assert_eq!(
            parse_meta_inlines("[==hi==](u)", on, false),
            vec![Inline::Link(
                Box::default(),
                vec![mark_span(vec![Inline::Str("hi".to_owned().into())])],
                Box::new(Target {
                    url: "u".to_owned().into(),
                    title: carta_ast::Text::default(),
                }),
            )]
        );
    }

    #[test]
    fn mark_resolves_inside_bracketed_span_label() {
        let on = exts(&[Extension::Mark, Extension::BracketedSpans]);
        // A `==…==` run nested in a bracketed span's body resolves there too.
        let span_attr = Attr {
            id: carta_ast::Text::default(),
            classes: vec!["x".to_owned().into()],
            attributes: Vec::new(),
        };
        assert_eq!(
            parse_meta_inlines("[a ==b== c]{.x}", on, false),
            vec![Inline::Span(
                Box::new(span_attr),
                vec![
                    Inline::Str("a".to_owned().into()),
                    Inline::Space,
                    mark_span(vec![Inline::Str("b".to_owned().into())]),
                    Inline::Space,
                    Inline::Str("c".to_owned().into()),
                ],
            )]
        );
    }

    #[test]
    fn mark_in_label_requires_extension() {
        // Without the mark extension a `==…==` run in a link label stays literal text.
        let off = Extensions::empty();
        assert_eq!(
            parse_meta_inlines("[==hi==](u)", off, false),
            vec![Inline::Link(
                Box::default(),
                vec![Inline::Str("==hi==".to_owned().into())],
                Box::new(Target {
                    url: "u".to_owned().into(),
                    title: carta_ast::Text::default(),
                }),
            )]
        );
    }

    #[test]
    fn header_attr_split_requires_extension_and_trailing_block() {
        let on = exts(&[Extension::HeaderAttributes]);
        // A trailing block separated by whitespace is the heading's attribute.
        let (content, attr) = split_header_attr("Title {#id .cls}", on);
        assert_eq!(content, "Title");
        assert_eq!(attr.id, "id");
        assert_eq!(attr.classes, ["cls"]);
        // A block glued to the preceding word belongs to that word, not the heading.
        assert_eq!(split_header_attr("Title{#id}", on).0, "Title{#id}");
        // An empty block is left in the text.
        assert_eq!(split_header_attr("Title {}", on).0, "Title {}");
        // Without the extension the text is untouched.
        let (content, attr) = split_header_attr("Title {#id}", Extensions::empty());
        assert_eq!(content, "Title {#id}");
        assert!(attr.id.is_empty());
    }

    #[test]
    fn mmd_header_identifier_split() {
        let on = exts(&[Extension::MmdHeaderIdentifiers]);
        // A trailing `[id]` label is the identifier; the content keeps everything before it.
        let (content, attr) = split_header_attr("Heading [myid]", on);
        assert_eq!(content, "Heading");
        assert_eq!(attr.id, "myid");
        // The identifier is lowercased with all whitespace removed.
        assert_eq!(split_header_attr("Foo [My Id]", on).1.id, "myid");
        // No whitespace is required before the label.
        let (content, attr) = split_header_attr("Foo[b]", on);
        assert_eq!((content, attr.id.as_str()), ("Foo", "b"));
        // Only the last bracket group is the label; earlier ones stay in the content.
        let (content, attr) = split_header_attr("Foo [a] bar [b]", on);
        assert_eq!((content, attr.id.as_str()), ("Foo [a] bar", "b"));
        // A reference-link tail (label directly after another bracket group) is not an identifier.
        assert_eq!(split_header_attr("See [ref][myid]", on).1.id, "");
        assert_eq!(split_header_attr("Foo [a] [b]", on).1.id, "");
        // An empty label is stripped but leaves the identifier empty (falls to an automatic one).
        assert_eq!(
            split_header_attr("Heading []", on),
            ("Heading", Attr::default())
        );
        // Without the extension the label stays in the text.
        assert_eq!(
            split_header_attr("Heading [myid]", Extensions::empty()).0,
            "Heading [myid]"
        );
    }

    #[test]
    fn subscript_superscript_flanking_anchors_only_on_whitespace() {
        // A run opens unless whitespace follows and closes unless whitespace precedes; the
        // punctuation sub-clauses that `*`/`_` honor do not apply.
        for ch in [b'~', b'^'] {
            assert_eq!(flanking(ch, None, Some('a')), (true, false));
            assert_eq!(flanking(ch, Some('a'), None), (false, true));
            assert_eq!(flanking(ch, Some('.'), Some('a')), (true, true));
            assert_eq!(flanking(ch, Some('a'), Some('!')), (true, true));
            assert_eq!(flanking(ch, Some(' '), Some('a')), (true, false));
            assert_eq!(flanking(ch, Some('a'), Some(' ')), (false, true));
        }
    }

    #[test]
    fn asterisk_flanking_keeps_full_rules() {
        // `*` opener followed by punctuation and preceded by a letter is not left-flanking.
        assert_eq!(flanking(b'*', Some('a'), Some('!')), (false, true));
        // `_` keeps its intraword restriction: between two letters it can neither open nor close.
        assert_eq!(flanking(b'_', Some('a'), Some('b')), (false, false));
    }

    #[test]
    fn use_count_maps_tilde_by_enabled_extension() {
        let strike = exts(&[Extension::Strikeout]);
        let sub = exts(&[Extension::Subscript]);
        let both = exts(&[Extension::Strikeout, Extension::Subscript]);

        // Two-on-two is a strikeout only when strikeout is on; otherwise it falls back to subscript.
        assert_eq!(match_use_count(2, 2, b'~', strike), Some(2));
        assert_eq!(match_use_count(2, 2, b'~', sub), Some(1));
        assert_eq!(match_use_count(2, 2, b'~', both), Some(2));
        // A length-one run can only be a subscript.
        assert_eq!(match_use_count(1, 2, b'~', strike), None);
        assert_eq!(match_use_count(1, 2, b'~', sub), Some(1));
        // With neither extension a tilde is inert.
        assert_eq!(match_use_count(2, 2, b'~', Extensions::empty()), None);
    }

    #[test]
    fn use_count_for_caret_and_emphasis() {
        assert_eq!(match_use_count(1, 1, b'^', Extensions::empty()), Some(1));
        assert_eq!(match_use_count(3, 3, b'^', Extensions::empty()), Some(1));
        assert_eq!(match_use_count(2, 2, b'*', Extensions::empty()), Some(2));
        assert_eq!(match_use_count(1, 2, b'_', Extensions::empty()), Some(1));
    }

    #[test]
    fn dash_runs_fold_em_heavy() {
        let em = '\u{2014}';
        let en = '\u{2013}';
        // Multiples of three are all em; even lengths are all en.
        assert_eq!(fold_dash_run(2), en.to_string());
        assert_eq!(fold_dash_run(3), em.to_string());
        assert_eq!(fold_dash_run(4), format!("{en}{en}"));
        assert_eq!(fold_dash_run(6), format!("{em}{em}"));
        // Odd lengths that are not multiples of three are em-heavy with a one- or two-en tail.
        assert_eq!(fold_dash_run(5), format!("{em}{en}"));
        assert_eq!(fold_dash_run(7), format!("{em}{en}{en}"));
        assert_eq!(fold_dash_run(11), format!("{em}{em}{em}{en}"));
        assert_eq!(fold_dash_run(13), format!("{em}{em}{em}{en}{en}"));
        assert_eq!(fold_dash_run(17), format!("{em}{em}{em}{em}{em}{en}"));
        // Each em dash accounts for three hyphens and each en dash for two, so the widths sum back to
        // the original run length with no hyphens left over.
        for len in 2..=40 {
            let folded = fold_dash_run(len);
            let width: usize = folded.chars().map(|c| if c == em { 3 } else { 2 }).sum();
            assert_eq!(width, len, "len={len} folded={folded}");
        }
    }

    #[test]
    fn ellipsis_runs_fold_in_threes() {
        assert_eq!(fold_ellipsis_run(0), "");
        assert_eq!(fold_ellipsis_run(1), ".");
        assert_eq!(fold_ellipsis_run(2), "..");
        assert_eq!(fold_ellipsis_run(3), "\u{2026}");
        assert_eq!(fold_ellipsis_run(4), "\u{2026}.");
        assert_eq!(fold_ellipsis_run(7), "\u{2026}\u{2026}.");
    }

    #[test]
    fn unmatched_smart_quotes_become_curly() {
        // A single quote that never pairs closes (’); an unmatched double quote opens (“).
        assert_eq!(delimiter_literal(b'\'', 1), "\u{2019}");
        assert_eq!(delimiter_literal(b'"', 1), "\u{201c}");
        assert_eq!(delimiter_literal(b'\'', 2), "\u{2019}\u{2019}");
        // Other delimiters revert to their own character.
        assert_eq!(delimiter_literal(b'*', 3), "***");
    }

    #[test]
    fn quote_flanking_blocks_intraword_pairing() {
        // A quote between alphanumerics can neither open nor close, so contractions stay apostrophes.
        assert_eq!(quote_flanking(b'\'', Some('n'), Some('t')), (false, false));
        // Whitespace-anchored quotes open on the left edge and close on the right.
        assert_eq!(quote_flanking(b'"', Some(' '), Some('a')), (true, false));
        assert_eq!(quote_flanking(b'"', Some('a'), Some(' ')), (false, true));
        // A quote hugging punctuation can both open and close.
        assert_eq!(quote_flanking(b'\'', Some('('), Some('a')), (true, false));
    }

    #[test]
    fn task_marker_replacement_recognizes_only_bounded_markers() {
        assert_eq!(
            task_marker_replacement("[ ] todo").as_deref(),
            Some(&*format!("{TASK_UNCHECKED} todo"))
        );
        assert_eq!(
            task_marker_replacement("[x] done").as_deref(),
            Some(&*format!("{TASK_CHECKED} done"))
        );
        assert_eq!(
            task_marker_replacement("[X]").as_deref(),
            Some(TASK_CHECKED)
        );
        // A marker glued to following text is not a task marker.
        assert_eq!(task_marker_replacement("[ ]todo"), None);
        // Unknown fill characters are not markers.
        assert_eq!(task_marker_replacement("[y] no"), None);
        assert_eq!(task_marker_replacement("plain"), None);
    }

    fn str_node(text: &str) -> Inline {
        Inline::Str(text.to_owned().into())
    }

    fn emph(content: Vec<Inline>) -> Inline {
        Inline::Emph(content)
    }

    fn strong(content: Vec<Inline>) -> Inline {
        Inline::Strong(content)
    }

    fn commonmark(text: &str) -> Vec<Inline> {
        parse_meta_inlines(text, Extensions::empty(), false)
    }

    #[test]
    fn triple_run_nests_strong_inside_emph() {
        assert_eq!(
            commonmark("***a***"),
            vec![emph(vec![strong(vec![str_node("a")])])]
        );
    }

    #[test]
    fn triple_run_markdown_dialect_nests_emph_inside_strong() {
        assert_eq!(
            parse_meta_inlines("***a***", Extensions::empty(), true),
            vec![strong(vec![emph(vec![str_node("a")])])]
        );
    }

    #[test]
    fn strong_wraps_inner_emph_and_text() {
        assert_eq!(
            commonmark("**a *b* c**"),
            vec![strong(vec![
                str_node("a"),
                Inline::Space,
                emph(vec![str_node("b")]),
                Inline::Space,
                str_node("c"),
            ])]
        );
    }

    #[test]
    fn emph_then_trailing_strong() {
        assert_eq!(
            commonmark("*a**b***"),
            vec![emph(vec![str_node("a"), strong(vec![str_node("b")])])]
        );
    }

    #[test]
    fn mixed_underscore_inside_asterisk() {
        assert_eq!(
            commonmark("*a _b_ a*"),
            vec![emph(vec![
                str_node("a"),
                Inline::Space,
                emph(vec![str_node("b")]),
                Inline::Space,
                str_node("a"),
            ])]
        );
    }

    #[test]
    fn unmatched_leading_run_stays_literal() {
        assert_eq!(
            commonmark("***a*"),
            vec![str_node("**"), emph(vec![str_node("a")])]
        );
    }

    #[test]
    fn adjacent_emphasis_chain() {
        assert_eq!(
            commonmark("*a*a*a*"),
            vec![
                emph(vec![str_node("a")]),
                str_node("a"),
                emph(vec![str_node("a")]),
            ]
        );
    }

    #[test]
    fn adjacent_emphasis_chain_ten() {
        let mut expected = Vec::new();
        for _ in 0..5 {
            expected.push(emph(vec![str_node("a")]));
            expected.push(str_node("a"));
        }
        assert_eq!(commonmark("*a*a*a*a*a*a*a*a*a*a"), expected);
    }

    #[test]
    fn strikeout_and_mark_side_by_side() {
        let ext = exts(&[Extension::Strikeout, Extension::Mark]);
        assert_eq!(
            parse_meta_inlines("~~x~~ and ==y==", ext, false),
            vec![
                Inline::Strikeout(vec![str_node("x")]),
                Inline::Space,
                str_node("and"),
                Inline::Space,
                mark_span(vec![str_node("y")]),
            ]
        );
    }

    #[test]
    fn subscript_run() {
        let ext = exts(&[Extension::Subscript]);
        assert_eq!(
            parse_meta_inlines("H~2~O", ext, false),
            vec![
                str_node("H"),
                Inline::Subscript(vec![str_node("2")]),
                str_node("O"),
            ]
        );
    }

    #[test]
    fn mark_wraps_inner_strong() {
        let ext = exts(&[Extension::Mark]);
        assert_eq!(
            parse_meta_inlines("==a **b** c==", ext, false),
            vec![mark_span(vec![
                str_node("a"),
                Inline::Space,
                strong(vec![str_node("b")]),
                Inline::Space,
                str_node("c"),
            ])]
        );
    }
}

#[cfg(test)]
mod inline_tests {
    use std::cell::Cell;
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    use carta_ast::{Attr, Block, Citation, CitationMode, Inline, Target};

    use super::{
        ExampleMap, HeaderNumbering, HeaderParseCache, IrBlock, LinkDef, RefContext, RefMap,
        gather_headers, heading_content_is_context_independent, parse_inlines, resolve_block,
    };
    use carta_core::{Extension, Extensions};

    static NO_DEFINED: BTreeSet<String> = BTreeSet::new();
    static NO_BY_ID: BTreeMap<String, Vec<Block>> = BTreeMap::new();
    static NO_EXAMPLES: ExampleMap = BTreeMap::new();

    /// An empty reference context, for tests that exercise inline syntax without footnotes or example
    /// references. Each call leaks a fresh citation count so a test starts numbering from zero.
    fn no_notes() -> RefContext<'static> {
        RefContext {
            defined: &NO_DEFINED,
            by_id: &NO_BY_ID,
            in_definition: false,
            markdown: false,
            examples: &NO_EXAMPLES,
            cite_count: Box::leak(Box::new(Cell::new(0))),
        }
    }

    fn no_ext() -> Extensions {
        Extensions::empty()
    }

    fn exts(list: &[Extension]) -> Extensions {
        Extensions::from_list(list)
    }

    fn empty_refs() -> RefMap {
        BTreeMap::new()
    }

    fn ref_map(entries: &[(&str, &str)]) -> RefMap {
        let mut m = BTreeMap::new();
        for (k, v) in entries {
            m.insert(
                k.to_string(),
                LinkDef {
                    url: v.to_string(),
                    title: String::new(),
                },
            );
        }
        m
    }

    fn p(text: &str) -> Vec<Inline> {
        parse_inlines(text, &empty_refs(), no_notes(), no_ext())
    }

    fn pe(text: &str, ext: Extensions) -> Vec<Inline> {
        parse_inlines(text, &empty_refs(), no_notes(), ext)
    }

    /// A reference context in the markdown dialect, where escaped spaces bind as non-breaking
    /// spaces, code spans trim their content, and superscripts and subscripts reject inner
    /// whitespace.
    fn md_notes() -> RefContext<'static> {
        RefContext {
            markdown: true,
            ..no_notes()
        }
    }

    fn pm(text: &str, ext: Extensions) -> Vec<Inline> {
        parse_inlines(text, &empty_refs(), md_notes(), ext)
    }

    fn str(s: &str) -> Inline {
        Inline::Str(s.to_owned().into())
    }

    fn link(content: Vec<Inline>, url: &str) -> Inline {
        Inline::Link(
            Box::default(),
            content,
            Box::new(Target {
                url: url.to_owned().into(),
                title: carta_ast::Text::default(),
            }),
        )
    }

    fn image(alt: Vec<Inline>, url: &str) -> Inline {
        Inline::Image(
            Box::default(),
            alt,
            Box::new(Target {
                url: url.to_owned().into(),
                title: carta_ast::Text::default(),
            }),
        )
    }

    // --- Emphasis and strong ---

    #[test]
    fn nested_emphasis_and_strong() {
        // *a **b** c* → Emph([a, Strong([b]), c])
        assert_eq!(
            p("*a **b** c*"),
            vec![Inline::Emph(vec![
                str("a"),
                Inline::Space,
                Inline::Strong(vec![str("b")]),
                Inline::Space,
                str("c"),
            ])]
        );
    }

    #[test]
    fn mixed_asterisk_and_underscore() {
        // *a _b_ c* → Emph([a, Emph([b]), c])
        assert_eq!(
            p("*a _b_ c*"),
            vec![Inline::Emph(vec![
                str("a"),
                Inline::Space,
                Inline::Emph(vec![str("b")]),
                Inline::Space,
                str("c"),
            ])]
        );
    }

    #[test]
    fn triple_asterisk_produces_emph_of_strong() {
        // ***a*** → Emph([Strong([a])])
        assert_eq!(
            p("***a***"),
            vec![Inline::Emph(vec![Inline::Strong(vec![str("a")])])]
        );
    }

    #[test]
    fn rule_of_3_prevents_outer_strong() {
        // **a*b** — the `*` closer + `**` opener sum is 3 which would violate rule-of-3 when one
        // side can both open and close, so the `*b` ends up literal inside Strong.
        assert_eq!(p("**a*b**"), vec![Inline::Strong(vec![str("a*b")])]);
    }

    #[test]
    fn rule_of_3_prevents_inner_strong() {
        // *a**b* — **b closes with * giving sum=3 but both must be mult-of-3 which they aren't,
        // so the **b is left literal.
        assert_eq!(p("*a**b*"), vec![Inline::Emph(vec![str("a**b")])]);
    }

    #[test]
    fn unmatched_openers_become_literal() {
        assert_eq!(p("*a"), vec![str("*a")]);
        assert_eq!(p("a*"), vec![str("a*")]);
        // **a* — the single * can close an emphasis inside the **, leaving ** - 1 = * literal
        assert_eq!(p("**a*"), vec![str("*"), Inline::Emph(vec![str("a")])]);
    }

    #[test]
    fn underscore_intraword_stays_literal() {
        // `_` between word chars cannot open or close (spec §6.3 rules).
        assert_eq!(p("a_b_c"), vec![str("a_b_c")]);
        assert_eq!(p("_a_b"), vec![str("_a_b")]);
    }

    #[test]
    fn emphasis_flanks_across_multi_byte_neighbors() {
        // `*` pairs intraword, so multi-byte word characters around the delimiters behave like
        // ASCII ones.
        assert_eq!(
            p("α*β*γ"),
            vec![str("α"), Inline::Emph(vec![str("β")]), str("γ")]
        );
    }

    #[test]
    fn emphasis_with_multi_byte_content_at_input_edges() {
        // The opener sits at the very start of the input and the closer at its very end, so both
        // boundary lookups run against the buffer's edges.
        assert_eq!(p("*β*"), vec![Inline::Emph(vec![str("β")])]);
    }

    #[test]
    fn emphasis_between_emoji_neighbors() {
        // An emoji is a symbol (punctuation for flanking purposes), not a word character, so the
        // run still opens and closes around it.
        assert_eq!(
            p("😀*a*😀"),
            vec![str("😀"), Inline::Emph(vec![str("a")]), str("😀")]
        );
    }

    #[test]
    fn underscore_intraword_stays_literal_with_multi_byte_neighbors() {
        // The word-character test before and after a `_` run reads whole characters, not bytes.
        assert_eq!(p("α_β_γ"), vec![str("α_β_γ")]);
    }

    #[test]
    fn empty_input_parses_to_nothing() {
        assert_eq!(p(""), Vec::new());
    }

    // --- Links and images ---

    #[test]
    fn inline_link_and_image() {
        assert_eq!(p("[a](u)"), vec![link(vec![str("a")], "u")]);
        assert_eq!(p("![i](u)"), vec![image(vec![str("i")], "u")]);
    }

    #[test]
    fn unmatched_image_opener_keeps_its_bang() {
        // An image opener that never finds a closing `]` reverts to the literal `![`, not `[`.
        assert_eq!(p("![x"), vec![str("![x")]);
        assert_eq!(p("![[a]x"), vec![str("![[a]x")]);
    }

    #[test]
    fn reference_link_with_and_without_ref() {
        // Without ref: stays literal.
        assert_eq!(p("[a][r]"), vec![str("[a][r]")]);
        // With ref defined: resolves.
        let refs = ref_map(&[("r", "http://r")]);
        let result = parse_inlines("[a][r]", &refs, no_notes(), no_ext());
        assert_eq!(result, vec![link(vec![str("a")], "http://r")]);
    }

    #[test]
    fn shortcut_reference_resolves_only_with_a_matching_definition() {
        // No definitions in scope: the brackets stay literal.
        assert_eq!(p("[foo]"), vec![str("[foo]")]);
        // A matching definition resolves the shortcut, with case folding on the label.
        let refs = ref_map(&[("foo", "http://f")]);
        assert_eq!(
            parse_inlines("[Foo]", &refs, no_notes(), no_ext()),
            vec![link(vec![str("Foo")], "http://f")]
        );
    }

    #[test]
    fn shortcut_label_near_the_length_bound_still_resolves() {
        // A 999-character label sits under the byte guard, so its lookup runs normally; the guard
        // only skips spans that are too long to be a label at all.
        let label = "a".repeat(999);
        let refs = ref_map(&[(label.as_str(), "http://f")]);
        let source = format!("[{label}]");
        assert_eq!(
            parse_inlines(&source, &refs, no_notes(), no_ext()),
            vec![link(vec![str(&label)], "http://f")]
        );
    }

    #[test]
    fn span_past_the_label_bound_never_resolves_as_shortcut_or_collapsed() {
        // A span longer than MAX_LABEL_BYTES is no label, even when a definition with the same
        // oversized key exists: both the shortcut and the collapsed lookup leave it literal.
        let oversized = "a".repeat(super::MAX_LABEL_BYTES + 1);
        let refs = ref_map(&[(oversized.as_str(), "http://big")]);
        let shortcut = format!("[{oversized}]");
        assert_eq!(
            parse_inlines(&shortcut, &refs, no_notes(), no_ext()),
            vec![str(&shortcut)]
        );
        let collapsed = format!("[{oversized}][]");
        assert_eq!(
            parse_inlines(&collapsed, &refs, no_notes(), no_ext()),
            vec![str(&collapsed)]
        );
        // The same collapsed form under the bound resolves through the identical path.
        let refs = ref_map(&[("foo", "http://f")]);
        assert_eq!(
            parse_inlines("[Foo][]", &refs, no_notes(), no_ext()),
            vec![link(vec![str("Foo")], "http://f")]
        );
    }

    #[test]
    fn footnote_reference_resolves_at_the_bracket_boundary() {
        let mut defined = BTreeSet::new();
        defined.insert("x".to_owned());
        let mut by_id: BTreeMap<String, Vec<Block>> = BTreeMap::new();
        by_id.insert("x".to_owned(), vec![Block::Para(vec![str("note")])]);
        let examples = ExampleMap::new();
        let cite = Cell::new(0);
        let notes = RefContext {
            defined: &defined,
            by_id: &by_id,
            in_definition: false,
            markdown: false,
            examples: &examples,
            cite_count: &cite,
        };
        let ext = exts(&[Extension::Footnotes]);
        assert_eq!(
            parse_inlines("[^x]", &empty_refs(), notes, ext),
            vec![Inline::Note(vec![Block::Para(vec![str("note")])])]
        );
        // With no footnotes defined, the same syntax stays literal.
        assert_eq!(pe("[^x]", ext), vec![str("[^x]")]);
    }

    #[test]
    fn spaced_reference_link_allows_whitespace_before_the_label() {
        let refs = ref_map(&[("ref", "http://r"), ("text", "http://t")]);
        let ext = exts(&[Extension::SpacedReferenceLinks]);
        // A space or newline separates the text bracket from the reference label; the display comes
        // from the first bracket and the target from the second.
        assert_eq!(
            parse_inlines("[text] [ref]", &refs, no_notes(), ext),
            vec![link(vec![str("text")], "http://r")]
        );
        assert_eq!(
            parse_inlines("[text]\n[ref]", &refs, no_notes(), ext),
            vec![link(vec![str("text")], "http://r")]
        );
        // An empty second bracket is a collapsed reference keyed on the first bracket.
        assert_eq!(
            parse_inlines("[text] []", &refs, no_notes(), ext),
            vec![link(vec![str("text")], "http://t")]
        );
        // A defined text but an undefined second label leaves the whole run literal — the text is
        // not retried as a shortcut.
        let only_text = ref_map(&[("text", "http://t")]);
        assert_eq!(
            parse_inlines("[text] [ref]", &only_text, no_notes(), ext),
            vec![str("[text]"), Inline::Space, str("[ref]")]
        );
        // Without the extension the space breaks the pair into two shortcut references.
        assert_eq!(
            parse_inlines("[text] [ref]", &refs, no_notes(), no_ext()),
            vec![
                link(vec![str("text")], "http://t"),
                Inline::Space,
                link(vec![str("ref")], "http://r"),
            ]
        );
    }

    #[test]
    fn nested_bracket_in_link_text() {
        // [[a]](u) — the inner [a] becomes a literal `[a]` in the link text because it has no
        // matching target of its own, and the outer pair provides the `(u)` target.
        assert_eq!(p("[[a]](u)"), vec![link(vec![str("[a]")], "u")]);
    }

    #[test]
    fn unmatched_brackets_are_literal() {
        assert_eq!(p("]]]"), vec![str("]]]")]);
    }

    #[test]
    fn link_suppresses_earlier_bracket_openers() {
        // [a [b](u) c](v) — the inner [b](u) is a valid link; its `[` opener then causes
        // the outer `[a ` opener to be deactivated (it cannot form a link containing a link),
        // so the outer `[` and `](v)` stay literal.
        assert_eq!(
            p("[a [b](u) c](v)"),
            vec![
                str("[a"),
                Inline::Space,
                link(vec![str("b")], "u"),
                Inline::Space,
                str("c](v)"),
            ]
        );
    }

    #[test]
    fn emphasis_inside_link_text() {
        assert_eq!(
            p("[*a*](u)"),
            vec![link(vec![Inline::Emph(vec![str("a")])], "u")]
        );
    }

    // --- Extension delimiters ---

    #[test]
    fn strikeout_double_tilde() {
        assert_eq!(
            pe("~~a~~", exts(&[Extension::Strikeout])),
            vec![Inline::Strikeout(vec![str("a")])]
        );
    }

    #[test]
    fn subscript_single_tilde() {
        assert_eq!(
            pe("~a~", exts(&[Extension::Subscript])),
            vec![Inline::Subscript(vec![str("a")])]
        );
    }

    #[test]
    fn superscript_caret() {
        assert_eq!(
            pe("^a^", exts(&[Extension::Superscript])),
            vec![Inline::Superscript(vec![str("a")])]
        );
    }

    // --- Markdown-dialect inline rules ---

    #[test]
    fn markdown_escaped_space_becomes_non_breaking() {
        // With the broad escape set a markdown-dialect `\ ` is a non-breaking space bound into the
        // surrounding word; without it (as in the strict dialect) and in the bare CommonMark engine a
        // backslash before a space is a literal backslash and the space splits the run.
        assert_eq!(
            pm("a\\ b", exts(&[Extension::AllSymbolsEscapable])),
            vec![str("a\u{a0}b")]
        );
        assert_eq!(
            pm("a\\ b", no_ext()),
            vec![str("a\\"), Inline::Space, str("b")]
        );
        assert_eq!(p("a\\ b"), vec![str("a\\"), Inline::Space, str("b")]);
    }

    #[test]
    fn broad_escape_set_is_gated_on_all_symbols_escapable() {
        // With the broad escape set a backslash drops before any ASCII punctuation.
        let broad = exts(&[Extension::AllSymbolsEscapable]);
        assert_eq!(pm("x\\|y", broad), vec![str("x|y")]);
        assert_eq!(pm("x\\~y", broad), vec![str("x~y")]);
        assert_eq!(pm("x\\<y", broad), vec![str("x<y")]);
        // Without it only the classic Markdown set is escapable; every other backslash stays literal.
        assert_eq!(pm("x\\|y", no_ext()), vec![str("x\\|y")]);
        assert_eq!(pm("x\\~y", no_ext()), vec![str("x\\~y")]);
        assert_eq!(pm("x\\<y", no_ext()), vec![str("x\\<y")]);
        // The classic set is escapable regardless of the extension.
        assert_eq!(pm("x\\!y", no_ext()), vec![str("x!y")]);
        assert_eq!(pm("x\\*y", no_ext()), vec![str("x*y")]);
        // The bare CommonMark engine escapes all ASCII punctuation with no extension needed.
        assert_eq!(p("x\\|y"), vec![str("x|y")]);
    }

    #[test]
    fn markdown_superscript_rejects_inner_space() {
        // A raw space anywhere inside a superscript voids it; the delimiters stay literal.
        let ext = exts(&[Extension::Superscript, Extension::AllSymbolsEscapable]);
        assert_eq!(pm("^a b^", ext), vec![str("^a"), Inline::Space, str("b^")]);
        // An escaped (non-breaking) space keeps the superscript intact.
        assert_eq!(
            pm("^a\\ b^", ext),
            vec![Inline::Superscript(vec![str("a\u{a0}b")])]
        );
        // No inner whitespace: still a superscript.
        assert_eq!(pm("^ab^", ext), vec![Inline::Superscript(vec![str("ab")])]);
    }

    #[test]
    fn short_subsuperscripts_consume_an_alphanumeric_run() {
        let ext = exts(&[
            Extension::Superscript,
            Extension::Subscript,
            Extension::ShortSubsuperscripts,
        ]);
        // A caret or tilde with an alphanumeric run and no closing delimiter is a short script.
        assert_eq!(
            pm("x^2y", ext),
            vec![str("x"), Inline::Superscript(vec![str("2y")])]
        );
        assert_eq!(
            pm("H~2O", ext),
            vec![str("H"), Inline::Subscript(vec![str("2O")])]
        );
        // The run stops at the first non-alphanumeric character.
        assert_eq!(
            pm("x^2.5", ext),
            vec![str("x"), Inline::Superscript(vec![str("2")]), str(".5")]
        );
        // A closing delimiter in the span forms the delimited pair instead; a leftover unpaired
        // caret then still opens a short script.
        assert_eq!(
            pm("a^b^c", ext),
            vec![str("a"), Inline::Superscript(vec![str("b")]), str("c")]
        );
        assert_eq!(
            pm("a^b^c^d", ext),
            vec![
                str("a"),
                Inline::Superscript(vec![str("b")]),
                str("c"),
                Inline::Superscript(vec![str("d")]),
            ]
        );
        // A delimiter with no alphanumeric run is literal.
        assert_eq!(pm("x^(2)", ext), vec![str("x^(2)")]);
        assert_eq!(pm("foo^", ext), vec![str("foo^")]);
        // Without the extension the short form does not fire.
        let off = exts(&[Extension::Superscript, Extension::Subscript]);
        assert_eq!(pm("x^2y", off), vec![str("x^2y")]);
    }

    #[test]
    fn markdown_subscript_rejects_inner_space_but_strikeout_allows_it() {
        // A single tilde is a subscript and rejects inner whitespace.
        assert_eq!(
            pm("~a b~", exts(&[Extension::Subscript])),
            vec![str("~a"), Inline::Space, str("b~")]
        );
        // A double tilde is a strikeout, which may hold whitespace.
        assert_eq!(
            pm("~~a b~~", exts(&[Extension::Strikeout])),
            vec![Inline::Strikeout(vec![str("a"), Inline::Space, str("b")])]
        );
    }

    #[test]
    fn markdown_superscript_rejects_space_in_nested_span() {
        // Whitespace inside an already-built nested inline voids the superscript too.
        let ext = exts(&[Extension::Superscript]);
        assert_eq!(
            pm("^*a b*^", ext),
            vec![
                str("^"),
                Inline::Emph(vec![str("a"), Inline::Space, str("b")]),
                str("^"),
            ]
        );
    }

    #[test]
    fn markdown_code_span_trims_surrounding_space() {
        // The markdown dialect trims a code span's content; the strict dialect strips at most a
        // single leading and trailing space (and only when the content is not all spaces).
        assert_eq!(pm("`  a  `", no_ext()), vec![code("a")]);
        assert_eq!(p("` a `"), vec![code("a")]);
        assert_eq!(p("`  a  `"), vec![code(" a ")]);
    }

    #[test]
    fn inline_note_parses_bracket_content_as_paragraph() {
        assert_eq!(
            pe("x^[a *b*] y", exts(&[Extension::InlineNotes])),
            vec![
                str("x"),
                Inline::Note(vec![Block::Para(vec![
                    str("a"),
                    Inline::Space,
                    Inline::Emph(vec![str("b")]),
                ])]),
                Inline::Space,
                str("y"),
            ]
        );
    }

    #[test]
    fn inline_note_allows_nested_brackets() {
        assert_eq!(
            pe("^[outer [inner] end]", exts(&[Extension::InlineNotes])),
            vec![Inline::Note(vec![Block::Para(vec![
                str("outer"),
                Inline::Space,
                str("[inner]"),
                Inline::Space,
                str("end"),
            ])])]
        );
    }

    #[test]
    fn empty_inline_note_is_an_empty_paragraph() {
        assert_eq!(
            pe("^[]", exts(&[Extension::InlineNotes])),
            vec![Inline::Note(vec![Block::Para(vec![])])]
        );
    }

    #[test]
    fn unclosed_inline_note_stays_literal() {
        assert_eq!(
            pe("^[unclosed", exts(&[Extension::InlineNotes])),
            vec![str("^[unclosed")]
        );
    }

    #[test]
    fn inline_note_syntax_is_literal_when_extension_off() {
        assert_eq!(
            pe("x^[a] y", Extensions::empty()),
            vec![str("x^[a]"), Inline::Space, str("y")]
        );
    }

    #[test]
    fn inline_note_wins_over_superscript_for_bracket() {
        // With both on, `^[` opens a note; a bare `^2^` would still be a superscript elsewhere.
        assert_eq!(
            pe(
                "y^[n]",
                exts(&[Extension::InlineNotes, Extension::Superscript])
            ),
            vec![str("y"), Inline::Note(vec![Block::Para(vec![str("n")])])]
        );
    }

    #[test]
    fn double_tilde_with_subscript_only_becomes_nested_subscript() {
        // Strikeout off, subscript on: ~~a~~ is two nested subscripts (each `~` consumed one).
        assert_eq!(
            pe("~~a~~", exts(&[Extension::Subscript])),
            vec![Inline::Subscript(vec![Inline::Subscript(vec![str("a")])])]
        );
    }

    #[test]
    fn single_tilde_skipped_when_strikeout_only() {
        // `~a~~b~~` with strikeout on but subscript off: length-1 run has no strikeout mapping
        // (`match_use_count` returns None), so it stays literal; `~~b~~` matches as strikeout.
        assert_eq!(
            pe("~a~~b~~", exts(&[Extension::Strikeout])),
            vec![str("~a"), Inline::Strikeout(vec![str("b")])]
        );
    }

    #[test]
    fn unmatched_tilde_run_stays_literal_when_strikeout_only() {
        // `~~a~` — the single `~` is a closer that can't find an opener (the `~~` needs length-2
        // pair and subscript is off), so the whole thing stays literal.
        assert_eq!(pe("~~a~", exts(&[Extension::Strikeout])), vec![str("~~a~")]);
    }

    #[test]
    fn mixed_asterisk_and_strikeout() {
        assert_eq!(
            pe("*a ~~b~~ c*", exts(&[Extension::Strikeout])),
            vec![Inline::Emph(vec![
                str("a"),
                Inline::Space,
                Inline::Strikeout(vec![str("b")]),
                Inline::Space,
                str("c"),
            ])]
        );
    }

    // --- TeX math ---

    fn math_inline(content: &str) -> Inline {
        Inline::Math(carta_ast::MathType::InlineMath, content.to_owned().into())
    }

    fn math_display(content: &str) -> Inline {
        Inline::Math(carta_ast::MathType::DisplayMath, content.to_owned().into())
    }

    fn math() -> Extensions {
        exts(&[Extension::TexMathDollars])
    }

    #[test]
    fn inline_and_display_math() {
        assert_eq!(pe("$a+b$", math()), vec![math_inline("a+b")]);
        assert_eq!(pe("$$x=y$$", math()), vec![math_display("x=y")]);
        // Display math keeps interior spaces verbatim.
        assert_eq!(pe("$$ x $$", math()), vec![math_display(" x ")]);
    }

    #[test]
    fn dollar_amounts_are_not_math() {
        // An opener must be followed by a non-space; a closer may not follow a digit or trail a space.
        assert_eq!(
            pe("$5 and $10", math()),
            vec![
                str("$5"),
                Inline::Space,
                str("and"),
                Inline::Space,
                str("$10")
            ]
        );
        assert_eq!(pe("$a$5", math()), vec![str("$a$5")]);
        assert_eq!(pe("$ a$", math()), vec![str("$"), Inline::Space, str("a$")]);
    }

    #[test]
    fn math_content_is_verbatim_but_honors_backslash_escape() {
        // `_`/`*` inside math do not start emphasis.
        assert_eq!(pe("$x_1*y*$", math()), vec![math_inline("x_1*y*")]);
        // An escaped dollar inside content does not close the span.
        assert_eq!(pe(r"$a\$b$", math()), vec![math_inline(r"a\$b")]);
    }

    #[test]
    fn failed_display_falls_back_to_inline() {
        // `$$x$` has no closing `$$`; the first `$` is literal and `$x$` parses as inline math.
        assert_eq!(pe("$$x$", math()), vec![str("$"), math_inline("x")]);
    }

    #[test]
    fn dollar_is_literal_without_the_extension() {
        assert_eq!(p("$a+b$"), vec![str("$a+b$")]);
    }

    // --- Attributes: spans, inline code, links ---

    fn span(attr: Attr, content: Vec<Inline>) -> Inline {
        Inline::Span(Box::new(attr), content)
    }

    fn attr(id: &str, classes: &[&str], kv: &[(&str, &str)]) -> Attr {
        Attr {
            id: id.to_owned().into(),
            classes: classes.iter().map(|c| (*c).into()).collect(),
            attributes: kv.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
        }
    }

    fn attrs() -> Extensions {
        exts(&[Extension::Attributes])
    }

    #[test]
    fn bracketed_span_carries_attributes() {
        assert_eq!(
            pe("[text]{.cls #id}", exts(&[Extension::BracketedSpans])),
            vec![span(attr("id", &["cls"], &[]), vec![str("text")])]
        );
    }

    #[test]
    fn empty_attribute_block_is_not_a_span() {
        assert_eq!(
            pe("[text]{}", exts(&[Extension::BracketedSpans])),
            vec![str("[text]{}")]
        );
    }

    #[test]
    fn consecutive_attribute_blocks_merge_first_id_wins() {
        // Adjacent blocks accumulate classes and key/value pairs; the first identifier is kept.
        assert_eq!(
            pe(
                "[x]{#one .a}{#two .b k=v}",
                exts(&[Extension::BracketedSpans])
            ),
            vec![span(
                attr("one", &["a", "b"], &[("k", "v")]),
                vec![str("x")]
            )]
        );
    }

    #[test]
    fn span_wins_over_shortcut_reference() {
        let refs = ref_map(&[("text", "http://r")]);
        let ext = exts(&[Extension::BracketedSpans]);
        assert_eq!(
            parse_inlines("[text]{.c}", &refs, no_notes(), ext),
            vec![span(attr("", &["c"], &[]), vec![str("text")])]
        );
    }

    #[test]
    fn inline_code_takes_attributes() {
        assert_eq!(
            pe("`code`{.rust #x}", attrs()),
            vec![Inline::Code(
                Box::new(attr("x", &["rust"], &[])),
                "code".to_owned().into()
            )]
        );
        // A space before the block leaves it unattached (no wrapper artifact is produced).
        assert_eq!(
            pe("`code` x", attrs()),
            vec![
                Inline::Code(Box::default(), "code".to_owned().into()),
                Inline::Space,
                str("x")
            ]
        );
    }

    #[test]
    fn link_and_image_take_attributes() {
        let link_with_attr = Inline::Link(
            Box::new(attr("home", &["external"], &[])),
            vec![str("t")],
            Box::new(Target {
                url: "u".to_owned().into(),
                title: carta_ast::Text::default(),
            }),
        );
        assert_eq!(pe("[t](u){.external #home}", attrs()), vec![link_with_attr]);
        let image_with_attr = Inline::Image(
            Box::new(attr("", &[], &[("width", "200")])),
            vec![str("a")],
            Box::new(Target {
                url: "i".to_owned().into(),
                title: carta_ast::Text::default(),
            }),
        );
        assert_eq!(pe("![a](i){width=200}", attrs()), vec![image_with_attr]);
    }

    #[test]
    fn attributes_require_the_extension() {
        // Without any attribute extension the block stays literal text.
        assert_eq!(p("[text]{.cls}"), vec![str("[text]{.cls}")]);
    }

    #[test]
    fn nested_image_with_inner_link_and_deactivated_bracket() {
        // ![[[foo](uri1)](uri2)](uri3)
        //
        // The outermost `![` is an image opener. The first `[` inside is a plain bracket opener.
        // `[foo](uri1)` matches as a link; that success deactivates the `[` opener between the
        // image `![` and `[foo]`. The next `]` encounters that deactivated opener: it must pop
        // it, literalize it, and emit `]` as text — not look further to the image opener below.
        // Only the final `](uri3)` closes the image.
        //
        // Expected: Image(uri3, alt=[Str("["), Link([Str("foo")], uri1), Str("](uri2)")])
        assert_eq!(
            p("![[[foo](uri1)](uri2)](uri3)"),
            vec![image(
                vec![str("["), link(vec![str("foo")], "uri1"), str("](uri2)"),],
                "uri3",
            )]
        );
    }

    // --- Inline raw attribute (`{=FORMAT}` on a code span) ---

    fn raw(format: &str, text: &str) -> Inline {
        Inline::RawInline(
            carta_ast::Format(format.to_owned().into()),
            text.to_owned().into(),
        )
    }

    fn code(text: &str) -> Inline {
        Inline::Code(Box::default(), text.to_owned().into())
    }

    #[test]
    fn raw_attribute_turns_code_span_into_raw_inline() {
        let ext = exts(&[Extension::RawAttribute]);
        assert_eq!(pe("`<b>`{=html}", ext), vec![raw("html", "<b>")]);
        assert_eq!(pe("`\\x`{=latex}", ext), vec![raw("latex", "\\x")]);
    }

    #[test]
    fn raw_attribute_format_token_allows_word_chars_dash_underscore() {
        let ext = exts(&[Extension::RawAttribute]);
        assert_eq!(pe("`x`{=my-format}", ext), vec![raw("my-format", "x")]);
        assert_eq!(pe("`x`{=my_fmt}", ext), vec![raw("my_fmt", "x")]);
        assert_eq!(pe("`x`{=3d}", ext), vec![raw("3d", "x")]);
    }

    #[test]
    fn raw_attribute_tolerates_whitespace_around_marker() {
        let ext = exts(&[Extension::RawAttribute]);
        assert_eq!(pe("`x`{ =html }", ext), vec![raw("html", "x")]);
        assert_eq!(pe("`x`{=html }", ext), vec![raw("html", "x")]);
        assert_eq!(pe("`x`{ =html}", ext), vec![raw("html", "x")]);
    }

    #[test]
    fn raw_attribute_normalizes_code_content() {
        let ext = exts(&[Extension::RawAttribute]);
        // A single space padding each side is stripped, exactly as for a code span.
        assert_eq!(pe("` x `{=html}", ext), vec![raw("html", "x")]);
    }

    #[test]
    fn raw_attribute_requires_a_pure_format_marker() {
        let ext = exts(&[Extension::RawAttribute]);
        // A space between `=` and the format is not a marker.
        assert_eq!(
            pe("`x`{= html}", ext),
            vec![code("x"), str("{="), Inline::Space, str("html}"),]
        );
        // An empty format is not a marker.
        assert_eq!(pe("`x`{=}", ext), vec![code("x"), str("{=}")]);
        // Anything beyond the format (a class, a dot) defeats the marker.
        assert_eq!(pe("`x`{=a.b}", ext), vec![code("x"), str("{=a.b}")]);
    }

    #[test]
    fn plain_attribute_block_on_code_span_is_not_raw() {
        // `{.class}` keeps the code span and applies the attribute (inline code attributes on).
        let ext = exts(&[Extension::RawAttribute, Extension::InlineCodeAttributes]);
        assert_eq!(
            pe("`x`{.c}", ext),
            vec![Inline::Code(
                Box::new(Attr {
                    classes: vec!["c".to_owned().into()],
                    ..Attr::default()
                }),
                "x".to_owned().into()
            )]
        );
    }

    #[test]
    fn raw_attribute_off_leaves_marker_literal() {
        assert_eq!(p("`<b>`{=html}"), vec![code("<b>"), str("{=html}")]);
    }

    #[test]
    fn code_span_matches_equal_length_closer() {
        assert_eq!(p("`a`"), vec![code("a")]);
    }

    #[test]
    fn code_span_with_no_closer_stays_literal() {
        assert_eq!(p("`a"), vec![str("`a")]);
    }

    #[test]
    fn code_span_failed_search_does_not_mask_a_different_length_match() {
        // The length-1 opener finds no lone-backtick closer and stays literal; the length-2 span
        // that comes right after must still match, since the close index is keyed by run length
        // and one length's absence does not suppress another's.
        assert_eq!(p("`a ``b``"), vec![str("`a"), Inline::Space, code("b")]);
    }

    #[test]
    fn code_span_opener_is_a_run_suffix_stays_literal() {
        // An escape consumes the backslash plus the first backtick of a run, so the opener that
        // follows is the run's suffix — its length (2 here) need not equal any run's full length.
        // No length-2 run closes it, so both openers emit their backticks literally.
        assert_eq!(
            p("\\``` x \\``` x"),
            vec![
                str("```"),
                Inline::Space,
                str("x"),
                Inline::Space,
                str("```"),
                Inline::Space,
                str("x"),
            ]
        );
    }

    #[test]
    fn code_span_distinct_run_lengths_all_resolve() {
        // Strictly increasing, distinct run lengths with no closers — the correctness face of the
        // adversarial quadratic input: every opener stays literal and the text between is intact.
        assert_eq!(
            p("`a ``b ```c"),
            vec![
                str("`a"),
                Inline::Space,
                str("``b"),
                Inline::Space,
                str("```c"),
            ]
        );
    }

    #[test]
    fn code_span_close_before_cursor_is_not_reused() {
        // The second span's close search must start at its own opener, never returning the first
        // span's already-consumed closer that lies before the cursor.
        assert_eq!(p("`a` `b`"), vec![code("a"), Inline::Space, code("b")]);
    }

    #[test]
    fn code_span_runs_at_buffer_ends_match() {
        // Opener at position 0 and closer as the final characters of the buffer.
        assert_eq!(p("``a``"), vec![code("a")]);
    }

    #[test]
    fn code_span_index_matches_scan_on_tricky_buffers() {
        // Nested lengths, adjacent runs of different lengths, and a triple-adjacency opener where a
        // shorter inner run precedes the matching closer. Expected values encoded literally.
        assert_eq!(p("``x`y``"), vec![code("x`y")]);
        assert_eq!(p("`a ``b`` c`"), vec![code("a ``b`` c")]);
        assert_eq!(p("``a` b``"), vec![code("a` b")]);
    }

    // --- Inline raw TeX and backslash math ---

    fn tex(source: &str) -> Inline {
        Inline::RawInline(
            carta_ast::Format("tex".to_owned().into()),
            source.to_owned().into(),
        )
    }

    fn raw_tex() -> Extensions {
        exts(&[Extension::RawTex])
    }

    fn single_math() -> Extensions {
        exts(&[Extension::TexMathSingleBackslash])
    }

    fn double_math() -> Extensions {
        exts(&[Extension::TexMathDoubleBackslash])
    }

    #[test]
    fn raw_tex_commands_with_argument_groups() {
        // Consecutive `{…}` groups are all captured.
        assert_eq!(
            pe(r"\textbf{b}\emph{c}", raw_tex()),
            vec![tex(r"\textbf{b}"), tex(r"\emph{c}")]
        );
        // A leading optional `[…]` group precedes a `{…}` argument.
        assert_eq!(pe(r"\sqrt[3]{8}", raw_tex()), vec![tex(r"\sqrt[3]{8}")]);
        // Nested braces inside a group are balanced.
        assert_eq!(pe(r"\foo{a{b}c}", raw_tex()), vec![tex(r"\foo{a{b}c}")]);
    }

    #[test]
    fn raw_tex_bare_command_absorbs_trailing_blanks() {
        // A command with no argument group swallows following spaces.
        assert_eq!(pe(r"\alpha y", raw_tex()), vec![tex(r"\alpha "), str("y")]);
        // A command followed by an argument group does not absorb the trailing space.
        assert_eq!(
            pe(r"\foo{a} y", raw_tex()),
            vec![tex(r"\foo{a}"), Inline::Space, str("y")]
        );
        // A command name carrying a digit does not absorb the trailing space.
        assert_eq!(
            pe(r"\foo1 y", raw_tex()),
            vec![tex(r"\foo1"), Inline::Space, str("y")]
        );
        // The first character must be a letter, so a digit after the backslash is not a command.
        assert_eq!(pe(r"\1foo", raw_tex()), vec![str(r"\1foo")]);
    }

    #[test]
    fn raw_tex_unbalanced_brace_reverts_whole_command() {
        // An unclosed `{`-group reverts the entire command to literal text.
        assert_eq!(
            pe(r"\foo{a y", raw_tex()),
            vec![str(r"\foo{a"), Inline::Space, str("y")]
        );
        // An unclosed `[`-group merely stops the group run; the command stands.
        assert_eq!(
            pe(r"\foo[a y", raw_tex()),
            vec![tex(r"\foo"), str("[a"), Inline::Space, str("y")]
        );
    }

    #[test]
    fn raw_tex_off_leaves_escape_behavior() {
        // Without the extension a command name is not raw TeX; `\t` is not punctuation so the
        // backslash stays literal.
        assert_eq!(p(r"\textbf{b}"), vec![str(r"\textbf{b}")]);
        // A backslash escape of punctuation still works regardless of the extension.
        assert_eq!(pe(r"\*", raw_tex()), vec![str("*")]);
    }

    #[test]
    fn raw_tex_environment_captured_as_one_inline() {
        // A complete `\begin{ENV}`…`\end{ENV}` is one raw inline spanning the whole environment,
        // body and interior newlines included.
        assert_eq!(
            pe("\\begin{equation}\nx\n\\end{equation}", raw_tex()),
            vec![tex("\\begin{equation}\nx\n\\end{equation}")]
        );
        // The environment may sit on a single line amid surrounding text.
        assert_eq!(
            pe(r"a \begin{eq} z \end{eq} b", raw_tex()),
            vec![
                str("a"),
                Inline::Space,
                tex(r"\begin{eq} z \end{eq}"),
                Inline::Space,
                str("b"),
            ]
        );
        // A trailing `*` is part of the environment name, so the close must carry it too.
        assert_eq!(
            pe(r"\begin{equation*} x \end{equation*}", raw_tex()),
            vec![tex(r"\begin{equation*} x \end{equation*}")]
        );
    }

    #[test]
    fn raw_tex_environment_balances_nested_begins() {
        // A nested environment of the same name deepens the nesting; the capture ends at the
        // matching outer close, not the first inner one.
        assert_eq!(
            pe(r"\begin{eq}\begin{eq}a\end{eq}\end{eq}", raw_tex()),
            vec![tex(r"\begin{eq}\begin{eq}a\end{eq}\end{eq}")]
        );
        // A nested environment of a different name is just part of the outer body.
        assert_eq!(
            pe(
                r"\begin{align}\begin{matrix}a\end{matrix}\end{align}",
                raw_tex()
            ),
            vec![tex(r"\begin{align}\begin{matrix}a\end{matrix}\end{align}")]
        );
    }

    #[test]
    fn raw_tex_unmatched_environment_reverts_to_text() {
        // Without a matching close, `\begin{ENV}` is literal text, not a raw command.
        assert_eq!(
            pe("\\begin{equation}\nx", raw_tex()),
            vec![str(r"\begin{equation}"), Inline::SoftBreak, str("x")]
        );
        // A bare `\begin` with no `{ENV}` group is not raw TeX: the backslash precedes a letter,
        // so it stays literal and the word is plain text.
        assert_eq!(
            pe(r"\begin x", raw_tex()),
            vec![str(r"\begin"), Inline::Space, str("x")]
        );
        // A standalone `\end{ENV}` is literal text.
        assert_eq!(
            pe(r"\end{equation}", raw_tex()),
            vec![str(r"\end{equation}")]
        );
        // A mismatched close does not satisfy the opener; the whole span reverts to text.
        assert_eq!(
            pe(r"\begin{equation} x \end{align}", raw_tex()),
            vec![
                str(r"\begin{equation}"),
                Inline::Space,
                str("x"),
                Inline::Space,
                str(r"\end{align}"),
            ]
        );
    }

    #[test]
    fn single_backslash_math() {
        assert_eq!(
            pe(r"\(x\) \[y\]", single_math()),
            vec![math_inline("x"), Inline::Space, math_display("y")]
        );
        // Inline content is trimmed; display content is verbatim.
        assert_eq!(pe(r"\( x \)", single_math()), vec![math_inline("x")]);
        assert_eq!(
            pe(r"\[ x = y \]", single_math()),
            vec![math_display(" x = y ")]
        );
    }

    #[test]
    fn single_backslash_math_empty_and_unclosed_fall_back() {
        // Empty content is not a math span: `\(` and `\)` revert to escaped parentheses.
        assert_eq!(pe(r"\(\)", single_math()), vec![str("()")]);
        // No closer: the opener's backslash escapes the `(`.
        assert_eq!(pe(r"\(x", single_math()), vec![str("(x")]);
        // A span of only spaces is still a (trimmed-empty) span.
        assert_eq!(pe(r"\( \)", single_math()), vec![math_inline("")]);
    }

    #[test]
    fn single_backslash_math_escapes_inside_content() {
        // An escaped delimiter inside the content does not close the span.
        assert_eq!(pe(r"\(a\\)b\)", single_math()), vec![math_inline(r"a\\)b")]);
    }

    #[test]
    fn double_backslash_math() {
        assert_eq!(
            pe(r"\\(x\\) \\[y\\]", double_math()),
            vec![math_inline("x"), Inline::Space, math_display("y")]
        );
    }

    #[test]
    fn backslash_math_off_leaves_escape_behavior() {
        // Without the extension `\(` is a plain escaped parenthesis.
        assert_eq!(p(r"\(x\)"), vec![str("(x)")]);
    }

    // --- Native spans (`<span …>` … `</span>`) ---

    fn native() -> Extensions {
        exts(&[Extension::NativeSpans])
    }

    #[test]
    fn native_span_carries_id_class_and_pairs() {
        assert_eq!(
            pe(
                r#"<span id="i" class="a b" data-x="y">hi *there*</span>"#,
                native()
            ),
            vec![span(
                attr("i", &["a", "b"], &[("data-x", "y")]),
                vec![str("hi"), Inline::Space, Inline::Emph(vec![str("there")])]
            )]
        );
    }

    #[test]
    fn native_span_without_attributes() {
        assert_eq!(
            pe("a <span>x</span> b", native()),
            vec![
                str("a"),
                Inline::Space,
                span(attr("", &[], &[]), vec![str("x")]),
                Inline::Space,
                str("b"),
            ]
        );
    }

    #[test]
    fn native_span_empty_content() {
        assert_eq!(
            pe("<span></span>", native()),
            vec![span(attr("", &[], &[]), vec![])]
        );
    }

    #[test]
    fn native_span_nests_innermost_first() {
        assert_eq!(
            pe(
                r#"<span class="o"><span class="i">x</span></span>"#,
                native()
            ),
            vec![span(
                attr("", &["o"], &[]),
                vec![span(attr("", &["i"], &[]), vec![str("x")])]
            )]
        );
    }

    #[test]
    fn native_span_tag_name_is_case_insensitive() {
        assert_eq!(
            pe(r#"<SPAN class="a">x</SPAN>"#, native()),
            vec![span(attr("", &["a"], &[]), vec![str("x")])]
        );
    }

    #[test]
    fn native_span_keeps_non_span_tags_raw() {
        // An unrelated tag inside a span stays raw inline HTML.
        assert_eq!(
            pe(r#"<span class="a">x <b>y</b></span>"#, native()),
            vec![span(
                attr("", &["a"], &[]),
                vec![
                    str("x"),
                    Inline::Space,
                    raw("html", "<b>"),
                    str("y"),
                    raw("html", "</b>"),
                ]
            )]
        );
    }

    #[test]
    fn native_span_attribute_values_and_booleans() {
        // Single-quoted, unquoted, and valueless attributes; a duplicate id/class keeps the first.
        assert_eq!(
            pe("<span data-x='y z'>q</span>", native()),
            vec![span(attr("", &[], &[("data-x", "y z")]), vec![str("q")])]
        );
        assert_eq!(
            pe("<span flag>q</span>", native()),
            vec![span(attr("", &[], &[("flag", "")]), vec![str("q")])]
        );
        assert_eq!(
            pe(
                r#"<span id="a" id="b" class="c" class="d">q</span>"#,
                native()
            ),
            vec![span(attr("a", &["c"], &[]), vec![str("q")])]
        );
    }

    #[test]
    fn native_span_decodes_entities_in_attribute_values() {
        assert_eq!(
            pe(r#"<span title="a &amp; b">q</span>"#, native()),
            vec![span(attr("", &[], &[("title", "a & b")]), vec![str("q")])]
        );
    }

    #[test]
    fn native_span_self_closing_stays_raw() {
        // `<span/>` has no content to wrap.
        assert_eq!(
            pe("a <span/> b", native()),
            vec![
                str("a"),
                Inline::Space,
                raw("html", "<span/>"),
                Inline::Space,
                str("b"),
            ]
        );
    }

    #[test]
    fn native_span_unclosed_opener_reverts_to_raw() {
        assert_eq!(
            pe(r#"<span class="a">no close"#, native()),
            vec![
                raw("html", "<span class=\"a\">"),
                str("no"),
                Inline::Space,
                str("close"),
            ]
        );
    }

    #[test]
    fn native_span_pairs_inside_emphasis() {
        assert_eq!(
            pe("*x <span>y</span> z*", native()),
            vec![Inline::Emph(vec![
                str("x"),
                Inline::Space,
                span(attr("", &[], &[]), vec![str("y")]),
                Inline::Space,
                str("z"),
            ])]
        );
    }

    #[test]
    fn native_span_off_leaves_tags_raw() {
        assert_eq!(
            p(r#"<span class="a">x</span>"#),
            vec![
                raw("html", "<span class=\"a\">"),
                str("x"),
                raw("html", "</span>"),
            ]
        );
    }

    // --- Mark (highlight) ---

    fn mark(content: Vec<Inline>) -> Inline {
        span(attr("", &["mark"], &[]), content)
    }

    #[test]
    fn mark_wraps_a_double_equals_run() {
        let on = exts(&[Extension::Mark]);
        assert_eq!(
            pe("a ==x== b", on),
            vec![
                str("a"),
                Inline::Space,
                mark(vec![str("x")]),
                Inline::Space,
                str("b"),
            ]
        );
    }

    #[test]
    fn mark_resolves_inner_emphasis() {
        let on = exts(&[Extension::Mark]);
        assert_eq!(
            pe("==x *y*==", on),
            vec![mark(vec![
                str("x"),
                Inline::Space,
                Inline::Emph(vec![str("y")]),
            ])]
        );
    }

    #[test]
    fn mark_off_leaves_double_equals_literal() {
        // Without the extension the run is plain text.
        assert_eq!(
            pe("a ==x== b", no_ext()),
            vec![
                str("a"),
                Inline::Space,
                str("==x=="),
                Inline::Space,
                str("b"),
            ]
        );
    }

    #[test]
    fn mark_opener_needs_no_following_space() {
        let on = exts(&[Extension::Mark]);
        // A space just inside either delimiter blocks the run; both sides stay literal.
        assert_eq!(pe("== x==", on), vec![str("=="), Inline::Space, str("x==")]);
        assert_eq!(pe("==x ==", on), vec![str("==x"), Inline::Space, str("==")]);
    }

    #[test]
    fn mark_lone_equals_stays_literal() {
        let on = exts(&[Extension::Mark]);
        assert_eq!(
            pe("a = b", on),
            vec![str("a"), Inline::Space, str("="), Inline::Space, str("b")]
        );
    }

    #[test]
    fn mark_run_pairs_once_and_leaves_excess_literal() {
        let on = exts(&[Extension::Mark]);
        // Four-on-four pairs only the innermost two from each side; the outer `==` stay literal and
        // do not re-pair into a nested mark.
        assert_eq!(
            pe("====x====", on),
            vec![str("=="), mark(vec![str("x")]), str("==")]
        );
        // Two-on-four consumes two from each, leaving the surplus `==` literal.
        assert_eq!(pe("==x====", on), vec![mark(vec![str("x")]), str("==")]);
    }

    // --- Citations ---

    fn cites() -> Extensions {
        exts(&[Extension::Citations])
    }

    fn cite(citations: Vec<Citation>, fallback: Vec<Inline>) -> Inline {
        Inline::Cite(citations, fallback)
    }

    fn citation(
        id: &str,
        prefix: Vec<Inline>,
        suffix: Vec<Inline>,
        mode: CitationMode,
        note_num: i32,
    ) -> Citation {
        Citation {
            id: id.to_owned().into(),
            prefix,
            suffix,
            mode,
            note_num,
            hash: 0,
        }
    }

    #[test]
    fn bare_citation_is_author_in_text() {
        assert_eq!(
            pe("@doe2020", cites()),
            vec![cite(
                vec![citation(
                    "doe2020",
                    vec![],
                    vec![],
                    CitationMode::AuthorInText,
                    1
                )],
                vec![str("@doe2020")],
            )]
        );
    }

    #[test]
    fn bare_citation_needs_a_non_word_before_the_at() {
        // Glued to a preceding word, the `@` is literal — no citation, no email autolink here.
        assert_eq!(pe("foo@bar", cites()), vec![str("foo@bar")]);
        // A space before the `@` lets it open a citation.
        assert_eq!(
            pe("a @b", cites()),
            vec![
                str("a"),
                Inline::Space,
                cite(
                    vec![citation("b", vec![], vec![], CitationMode::AuthorInText, 1)],
                    vec![str("@b")],
                ),
            ]
        );
    }

    #[test]
    fn bracket_citation_carries_prefix_and_suffix() {
        assert_eq!(
            pe("[see @doe2020 and more]", cites()),
            vec![cite(
                vec![citation(
                    "doe2020",
                    vec![str("see")],
                    vec![Inline::Space, str("and"), Inline::Space, str("more")],
                    CitationMode::NormalCitation,
                    1,
                )],
                vec![
                    str("[see"),
                    Inline::Space,
                    str("@doe2020"),
                    Inline::Space,
                    str("and"),
                    Inline::Space,
                    str("more]"),
                ],
            )]
        );
    }

    #[test]
    fn dash_before_at_suppresses_author() {
        assert_eq!(
            pe("[-@k]", cites()),
            vec![cite(
                vec![citation(
                    "k",
                    vec![],
                    vec![],
                    CitationMode::SuppressAuthor,
                    1
                )],
                vec![str("[-@k]")],
            )]
        );
        // A `-` glued to a preceding word is part of the prefix, not a suppression marker.
        assert_eq!(
            pe("[a-@b]", cites()),
            vec![cite(
                vec![citation(
                    "b",
                    vec![str("a-")],
                    vec![],
                    CitationMode::NormalCitation,
                    1
                )],
                vec![str("[a-@b]")],
            )]
        );
    }

    #[test]
    fn semicolon_separates_entries_sharing_one_number() {
        assert_eq!(
            pe("[@a; @b]", cites()),
            vec![cite(
                vec![
                    citation("a", vec![], vec![], CitationMode::NormalCitation, 1),
                    citation("b", vec![], vec![], CitationMode::NormalCitation, 1),
                ],
                vec![str("[@a;"), Inline::Space, str("@b]")],
            )]
        );
    }

    #[test]
    fn comma_nests_a_bare_citation_in_the_suffix() {
        // `@b` after a comma is not a new entry; it becomes a bare citation inside `a`'s suffix, and
        // the enclosing group takes the higher number.
        assert_eq!(
            pe("[@a, @b]", cites()),
            vec![cite(
                vec![citation(
                    "a",
                    vec![],
                    vec![
                        str(","),
                        Inline::Space,
                        cite(
                            vec![citation("b", vec![], vec![], CitationMode::AuthorInText, 2)],
                            vec![str("@b")],
                        ),
                    ],
                    CitationMode::NormalCitation,
                    2,
                )],
                vec![str("[@a,"), Inline::Space, str("@b]")],
            )]
        );
    }

    #[test]
    fn document_order_numbers_each_group() {
        // Two separate groups in one block take consecutive numbers.
        let out = pe("@a and [@b]", cites());
        let nums: Vec<i32> = out
            .iter()
            .filter_map(|inline| match inline {
                Inline::Cite(citations, _) => citations.first().map(|c| c.note_num),
                _ => None,
            })
            .collect();
        assert_eq!(nums, vec![1, 2]);
    }

    #[test]
    fn malformed_bracket_falls_back_to_inline_citations() {
        // A trailing empty segment is not a citation list; the brackets stay literal and the bare
        // `@a` inside becomes an author-in-text citation.
        assert_eq!(
            pe("[@a;]", cites()),
            vec![
                str("["),
                cite(
                    vec![citation("a", vec![], vec![], CitationMode::AuthorInText, 1)],
                    vec![str("@a")],
                ),
                str(";]"),
            ]
        );
    }

    #[test]
    fn segment_without_a_key_is_not_a_citation_list() {
        // The first segment holds no `@`, so the whole bracket is not a citation; only the bare `@b`
        // citation survives.
        assert_eq!(
            pe("[no key; @b]", cites()),
            vec![
                str("[no"),
                Inline::Space,
                str("key;"),
                Inline::Space,
                cite(
                    vec![citation("b", vec![], vec![], CitationMode::AuthorInText, 1)],
                    vec![str("@b")],
                ),
                str("]"),
            ]
        );
    }

    #[test]
    fn key_charset_keeps_internal_punctuation() {
        // Internal `_ : - . /` belong to a key only when more key characters follow.
        assert_eq!(
            pe("[@foo_bar:baz-qux.v/1]", cites()),
            vec![cite(
                vec![citation(
                    "foo_bar:baz-qux.v/1",
                    vec![],
                    vec![],
                    CitationMode::NormalCitation,
                    1,
                )],
                vec![str("[@foo_bar:baz-qux.v/1]")],
            )]
        );
        // A trailing `-` is not part of the key; it falls to the suffix.
        assert_eq!(
            pe("[@a-]", cites()),
            vec![cite(
                vec![citation(
                    "a",
                    vec![],
                    vec![str("-")],
                    CitationMode::NormalCitation,
                    1
                )],
                vec![str("[@a-]")],
            )]
        );
    }

    #[test]
    fn citations_off_leaves_the_syntax_literal() {
        assert_eq!(
            pe("See [@a] and @b.", no_ext()),
            vec![
                str("See"),
                Inline::Space,
                str("[@a]"),
                Inline::Space,
                str("and"),
                Inline::Space,
                str("@b."),
            ]
        );
    }

    #[test]
    fn escaped_at_is_not_a_citation() {
        assert_eq!(pe(r"[\@a]", cites()), vec![str("[@a]")]);
    }

    #[test]
    fn citation_does_not_steal_a_link() {
        // An explicit link target wins; the key inside becomes a bare citation in the link text.
        assert_eq!(
            pe("[@a](http://x.com)", cites()),
            vec![link(
                vec![cite(
                    vec![citation("a", vec![], vec![], CitationMode::AuthorInText, 1)],
                    vec![str("@a")],
                )],
                "http://x.com",
            )]
        );
    }

    #[test]
    fn heading_content_is_context_independent_gates_on_ref_trigger_chars() {
        assert!(heading_content_is_context_independent("Installation"));
        assert!(heading_content_is_context_independent("API reference"));
        assert!(!heading_content_is_context_independent("About @doe99"));
        assert!(!heading_content_is_context_independent("Title[^1]"));
        assert!(!heading_content_is_context_independent("See [spec]"));
    }

    #[test]
    fn a_context_independent_heading_is_parsed_once_and_reused_by_the_body_pass() {
        let ir = vec![IrBlock::Heading(1, "Installation".to_owned())];
        let mut refs = empty_refs();
        let mut cache: HeaderParseCache = BTreeMap::new();
        let mut numbering = HeaderNumbering::new(no_ext(), false);
        gather_headers(
            &ir,
            &mut refs,
            no_notes(),
            no_ext(),
            &mut numbering,
            &mut cache,
        );

        let queued = cache
            .get("Installation")
            .expect("the pre-pass should have cached the heading's parse");
        assert_eq!(queued.len(), 1);

        let heading = ir.first().expect("one heading in the IR");
        let mut out = Vec::new();
        resolve_block(heading, &refs, no_notes(), no_ext(), &mut cache, &mut out);

        // The body pass popped the pre-pass's parse instead of running the inline scan again.
        assert!(cache.get("Installation").is_none_or(VecDeque::is_empty));
        assert_eq!(
            out,
            vec![Block::Header(1, Box::default(), p("Installation"))]
        );
    }

    #[test]
    fn a_second_identical_heading_pops_its_own_queued_parse() {
        let ir = vec![
            IrBlock::Heading(1, "Dup".to_owned()),
            IrBlock::Heading(1, "Dup".to_owned()),
        ];
        let mut refs = empty_refs();
        let mut cache: HeaderParseCache = BTreeMap::new();
        let mut numbering = HeaderNumbering::new(no_ext(), false);
        gather_headers(
            &ir,
            &mut refs,
            no_notes(),
            no_ext(),
            &mut numbering,
            &mut cache,
        );
        assert_eq!(cache.get("Dup").map(VecDeque::len), Some(2));

        let mut out = Vec::new();
        for block in &ir {
            resolve_block(block, &refs, no_notes(), no_ext(), &mut cache, &mut out);
        }

        assert!(cache.get("Dup").is_none_or(VecDeque::is_empty));
        assert_eq!(
            out,
            vec![
                Block::Header(1, Box::default(), p("Dup")),
                Block::Header(1, Box::default(), p("Dup")),
            ]
        );
    }
}
