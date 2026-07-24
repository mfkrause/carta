//! Block-building context that turns formatting-tagged atoms into paragraphs, tables, and lists.

use std::mem::take;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, Row, Table, TableBody, Text,
};

use super::inlines::{
    Atom, AtomKind, CharProps, build_inlines, mono_code_block, strip_heading_emphasis,
};

/// One level of the bookmark nesting active while a paragraph is being built. The root frame has no
/// bookmark; each `\*\bkmkstart` pushes a named frame that `\*\bkmkend` folds into a span.
#[derive(Debug, Clone)]
struct Frame {
    bookmark: Option<Text>,
    atoms: Vec<Atom>,
}

/// One block-building context: the emitted blocks plus the paragraph and table under construction.
/// The document has one; every footnote opens another.
// The bools are independent state bits that can hold at once, not a configuration enum.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub(super) struct Emitter {
    blocks: Vec<Block>,
    frames: Vec<Frame>,
    pending_text: String,
    pending_props: CharProps,
    pending_space: bool,
    space_props: CharProps,
    has_content: bool,
    /// Keeps a space at the leading or trailing edge of the content instead of trimming it. A
    /// paragraph trims its edge whitespace, but a hyperlink's display text is inline and its edge
    /// space is meaningful (it separates the link from the word beside it), so it is preserved.
    pub(super) preserve_edge_space: bool,
    pub(super) outline_level: Option<i32>,
    pub(super) in_table_para: bool,
    rows: Vec<Row>,
    cells: Vec<Cell>,
    cell_blocks: Vec<Block>,
    columns: usize,
    row_cell_bounds: usize,
    pub(super) list_active: bool,
    pub(super) list_id: i32,
    pub(super) list_level: i32,
    pub(super) list_levels: Vec<LevelDef>,
    pending_list: Vec<ListParagraph>,
    /// Fallback buffer returned by [`Emitter::frame_atoms`] only if the frame stack is ever empty,
    /// which its guard prevents; keeps the accessor total without a panic or a leak.
    scratch_atoms: Vec<Atom>,
}

/// One paragraph belonging to a list, tagged with the list it selects (`\ls`), its nesting level
/// (`\ilvl`) and, when the level's definition numbers it, the start value and numeral style read from
/// the list table. Consecutive entries are reassembled into nested lists when the list run ends: a
/// numbered level becomes an [`Block::OrderedList`], an unnumbered one a [`Block::BulletList`]. Two
/// adjacent paragraphs that select different lists stay in separate sibling lists even at the same
/// level, so a numbered list directly followed by a bulleted one is not fused.
#[derive(Debug)]
struct ListParagraph {
    list_id: i32,
    level: i32,
    numbering: Option<(i32, ListNumberStyle)>,
    block: Block,
}

/// One list level's marker configuration, read from a `\listlevel` group: the numeral style (absent
/// when the level is a bullet or carries no number) and the start value (`\levelstartat`, default 1).
#[derive(Debug, Clone, Copy)]
pub(super) struct LevelDef {
    pub(super) style: Option<ListNumberStyle>,
    pub(super) start: i32,
}

impl LevelDef {
    /// The start value and numeral style when this level is numbered; `None` when it is a bullet.
    fn numbering(self) -> Option<(i32, ListNumberStyle)> {
        self.style.map(|style| (self.start, style))
    }
}

/// Maps a `\levelnfc` number-format code to a numeral style. Bullet (`23`) and unnumbered (`255`)
/// levels, and a level that declares no format, carry no numeral style and render as bullets; every
/// other code is a numbered level, with the common decimal, roman, and alphabetic codes named and
/// the rest left to the target format's default numbering.
pub(super) fn nfc_to_style(nfc: Option<i32>) -> Option<ListNumberStyle> {
    match nfc {
        None | Some(23 | 255) => None,
        Some(0) => Some(ListNumberStyle::Decimal),
        Some(1) => Some(ListNumberStyle::UpperRoman),
        Some(2) => Some(ListNumberStyle::LowerRoman),
        Some(3) => Some(ListNumberStyle::UpperAlpha),
        Some(4) => Some(ListNumberStyle::LowerAlpha),
        Some(_) => Some(ListNumberStyle::DefaultStyle),
    }
}

impl Emitter {
    pub(super) fn new() -> Self {
        Self {
            blocks: Vec::new(),
            frames: vec![Frame {
                bookmark: None,
                atoms: Vec::new(),
            }],
            pending_text: String::new(),
            pending_props: CharProps::default(),
            pending_space: false,
            space_props: CharProps::default(),
            has_content: false,
            preserve_edge_space: false,
            outline_level: None,
            in_table_para: false,
            rows: Vec::new(),
            cells: Vec::new(),
            cell_blocks: Vec::new(),
            columns: 0,
            row_cell_bounds: 0,
            list_active: false,
            list_id: 0,
            list_level: 0,
            list_levels: Vec::new(),
            pending_list: Vec::new(),
            scratch_atoms: Vec::new(),
        }
    }

    fn frame_atoms(&mut self) -> &mut Vec<Atom> {
        if self.frames.is_empty() {
            self.frames.push(Frame {
                bookmark: None,
                atoms: Vec::new(),
            });
        }
        // The guard above guarantees a last element; the scratch buffer is dead-code fallback.
        match self.frames.last_mut() {
            Some(frame) => &mut frame.atoms,
            None => &mut self.scratch_atoms,
        }
    }

    fn flush_text(&mut self) {
        if !self.pending_text.is_empty() {
            let props = self.pending_props;
            let text = take(&mut self.pending_text);
            self.frame_atoms().push(Atom {
                props,
                kind: AtomKind::Text(text),
            });
        }
    }

    fn resolve_space(&mut self) {
        if self.pending_space {
            self.pending_space = false;
            if self.has_content || self.preserve_edge_space {
                let props = self.space_props;
                self.frame_atoms().push(Atom {
                    props,
                    kind: AtomKind::Space,
                });
                self.has_content = true;
            }
        }
    }

    pub(super) fn push_char(&mut self, c: char, props: CharProps) {
        if props.hidden {
            return;
        }
        self.resolve_space();
        if !self.pending_text.is_empty() && !self.pending_props.same_run(props) {
            self.flush_text();
        }
        if self.pending_text.is_empty() {
            self.pending_props = props;
        }
        if props.allcaps {
            for upper in c.to_uppercase() {
                self.pending_text.push(upper);
            }
        } else {
            self.pending_text.push(c);
        }
        self.has_content = true;
    }

    pub(super) fn push_str(&mut self, s: &str, props: CharProps) {
        for c in s.chars() {
            self.push_char(c, props);
        }
    }

    pub(super) fn push_space(&mut self, props: CharProps) {
        if props.hidden {
            return;
        }
        self.flush_text();
        if !self.pending_space {
            self.pending_space = true;
            self.space_props = props;
        }
    }

    pub(super) fn push_break(&mut self, props: CharProps) {
        if props.hidden {
            return;
        }
        self.flush_text();
        self.resolve_space();
        self.frame_atoms().push(Atom {
            props,
            kind: AtomKind::LineBreak,
        });
        self.has_content = true;
    }

    pub(super) fn push_node(&mut self, node: Inline, props: CharProps) {
        if props.hidden {
            return;
        }
        self.flush_text();
        self.resolve_space();
        self.frame_atoms().push(Atom {
            props,
            kind: AtomKind::Node(node),
        });
        self.has_content = true;
    }

    pub(super) fn open_bookmark(&mut self, id: Text) {
        self.flush_text();
        self.resolve_space();
        self.frames.push(Frame {
            bookmark: Some(id),
            atoms: Vec::new(),
        });
    }

    pub(super) fn close_bookmark(&mut self) {
        self.flush_text();
        self.resolve_space();
        if self.frames.len() <= 1 {
            return;
        }
        if let Some(frame) = self.frames.pop()
            && self.fold_bookmark(frame)
        {
            self.has_content = true;
        }
    }

    /// Wraps a popped bookmark frame into a span node in its parent frame, and reports whether it
    /// produced one. A bookmark with no content between its start and end (the point anchors a word
    /// processor scatters for every cross-reference and revision cursor) carries no span and is
    /// dropped, so an empty span never appears in the output.
    fn fold_bookmark(&mut self, frame: Frame) -> bool {
        let id = frame.bookmark.unwrap_or_default();
        let inlines = build_inlines(frame.atoms);
        if inlines.is_empty() {
            return false;
        }
        let span = Inline::Span(
            Box::new(Attr {
                id,
                classes: Vec::new(),
                attributes: Vec::new(),
            }),
            inlines,
        );
        self.frame_atoms().push(Atom {
            props: CharProps::default(),
            kind: AtomKind::Node(span),
        });
        true
    }

    /// Closes the current paragraph's atom sequence, folding any unclosed bookmarks into spans.
    fn take_atoms(&mut self) -> Vec<Atom> {
        self.flush_text();
        if self.preserve_edge_space && self.pending_space {
            let props = self.space_props;
            self.frame_atoms().push(Atom {
                props,
                kind: AtomKind::Space,
            });
        }
        self.pending_space = false;
        while self.frames.len() > 1 {
            if let Some(frame) = self.frames.pop() {
                self.fold_bookmark(frame);
            }
        }
        self.has_content = false;
        match self.frames.first_mut() {
            Some(frame) => take(&mut frame.atoms),
            None => Vec::new(),
        }
    }

    /// Closes the current paragraph's inline content, folding any unclosed bookmarks into spans.
    fn take_inlines(&mut self) -> Vec<Inline> {
        build_inlines(self.take_atoms())
    }

    /// Ends a paragraph: the accumulated content becomes a heading, a code block, or a paragraph,
    /// routed into the current table cell when the paragraph is marked in-table, otherwise into the
    /// block stream. A paragraph whose every run is monospace and otherwise unformatted is a code
    /// block; an outline level makes it a heading.
    pub(super) fn end_paragraph(&mut self) {
        let mut atoms = self.take_atoms();
        // One trailing hard break renders no line and is dropped; a paragraph left empty by this
        // is emitted as nothing.
        if atoms
            .last()
            .is_some_and(|atom| matches!(atom.kind, AtomKind::LineBreak))
        {
            atoms.pop();
        }
        if atoms.is_empty() {
            return;
        }
        let block = if let Some(level) = self.outline_level {
            Block::Header(
                level.saturating_add(1).max(1),
                Box::default(),
                strip_heading_emphasis(build_inlines(atoms)),
            )
        } else if let Some(code) = mono_code_block(&atoms) {
            Block::CodeBlock(Box::default(), code.into())
        } else {
            Block::Para(build_inlines(atoms))
        };
        if self.in_table_para {
            self.cell_blocks.push(block);
        } else if self.list_active {
            // Close any open table first so the list and table keep their source order.
            self.finish_table();
            let numbering = usize::try_from(self.list_level)
                .ok()
                .and_then(|index| self.list_levels.get(index))
                .and_then(|level| level.numbering());
            self.pending_list.push(ListParagraph {
                list_id: self.list_id,
                level: self.list_level,
                numbering,
                block,
            });
        } else {
            self.flush_list();
            self.finish_table();
            self.blocks.push(block);
        }
    }

    /// Emits the buffered list paragraphs as nested lists, in source order.
    fn flush_list(&mut self) {
        if self.pending_list.is_empty() {
            return;
        }
        let entries = take(&mut self.pending_list);
        self.blocks.extend(build_lists(&entries));
    }

    pub(super) fn end_cell(&mut self) {
        self.end_paragraph();
        let content = take(&mut self.cell_blocks);
        self.cells.push(Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content,
        });
        self.columns = self.columns.max(self.cells.len());
    }

    pub(super) fn end_row(&mut self) {
        self.end_paragraph();
        if !self.cell_blocks.is_empty() {
            let content = take(&mut self.cell_blocks);
            self.cells.push(Cell {
                attr: Attr::default(),
                align: Alignment::AlignDefault,
                row_span: 1,
                col_span: 1,
                content,
            });
        }
        if !self.cells.is_empty() {
            let cells = take(&mut self.cells);
            self.columns = self.columns.max(cells.len());
            self.rows.push(Row {
                attr: Attr::default(),
                cells,
            });
        }
        self.in_table_para = false;
    }

    pub(super) fn begin_row_definition(&mut self) {
        self.row_cell_bounds = 0;
    }

    pub(super) fn note_cell_boundary(&mut self) {
        self.row_cell_bounds += 1;
        self.columns = self.columns.max(self.row_cell_bounds);
    }

    /// Emits any pending table rows as a [`Block::Table`]. A row shorter than the widest is padded
    /// with empty cells so every row spans the full column count.
    fn finish_table(&mut self) {
        if !self.cells.is_empty() {
            let cells = take(&mut self.cells);
            self.columns = self.columns.max(cells.len());
            self.rows.push(Row {
                attr: Attr::default(),
                cells,
            });
        }
        if self.rows.is_empty() {
            self.columns = 0;
            return;
        }
        let columns = self.columns.max(1);
        let mut rows = take(&mut self.rows);
        for row in &mut rows {
            while row.cells.len() < columns {
                row.cells.push(Cell {
                    attr: Attr::default(),
                    align: Alignment::AlignDefault,
                    row_span: 1,
                    col_span: 1,
                    content: Vec::new(),
                });
            }
        }
        let col_specs = (0..columns)
            .map(|_| ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            })
            .collect();
        let table = Table {
            attr: Attr::default(),
            caption: Caption::default(),
            col_specs,
            head: carta_ast::TableHead::default(),
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: rows,
            }],
            foot: carta_ast::TableFoot::default(),
        };
        self.blocks.push(Block::Table(Box::new(table)));
        self.columns = 0;
    }

    pub(super) fn finish_blocks(mut self) -> Vec<Block> {
        self.end_paragraph();
        self.flush_list();
        self.finish_table();
        self.blocks
    }

    /// Flattens the context to a single inline sequence, for content that is inline by construction
    /// (a hyperlink's display text). Any block breaks inside contribute their inlines in order.
    pub(super) fn finish_inlines(mut self) -> Vec<Inline> {
        let mut out = Vec::new();
        let trailing = self.take_inlines();
        for block in self.blocks {
            if let Block::Para(inlines) | Block::Plain(inlines) = block {
                out.extend(inlines);
            }
        }
        out.extend(trailing);
        out
    }
}

/// Reassembles a run of list paragraphs into nested lists. A maximal span of entries at the
/// shallowest level forms one list; a return to that level after a deeper span starts a fresh
/// sibling list, so denesting is expressed as consecutive lists just as the format records it.
fn build_lists(entries: &[ListParagraph]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut rest = entries;
    while !rest.is_empty() {
        let (block, consumed) = build_one_list(rest, 0);
        out.push(block);
        let step = consumed.max(1).min(rest.len());
        rest = rest.get(step..).unwrap_or(&[]);
    }
    out
}

/// Builds a single list from the front of `entries`, consuming every entry at the list's own level
/// that also selects the same list, and folding any deeper entry that follows an item into a nested
/// list inside it. The list is an [`Block::OrderedList`] when its first entry's level is numbered,
/// otherwise a [`Block::BulletList`]. An entry at the same level that selects a different list ends
/// this one, so adjacent lists of unlike kind (a numbered list then a bulleted one) stay separate.
/// Returns the list and how many entries it consumed. `depth` caps recursion so pathologically deep
/// nesting degrades into sibling lists instead of overflowing the stack.
fn build_one_list(entries: &[ListParagraph], depth: usize) -> (Block, usize) {
    const MAX_LIST_DEPTH: usize = 256;
    let base = entries.first().map_or(0, |entry| entry.level);
    let list_id = entries.first().map_or(0, |entry| entry.list_id);
    let numbering = entries.first().and_then(|entry| entry.numbering);
    let mut items: Vec<Vec<Block>> = Vec::new();
    let mut i = 0;
    while let Some(entry) = entries.get(i) {
        if entry.level != base || entry.list_id != list_id {
            break;
        }
        let mut item = vec![entry.block.clone()];
        i += 1;
        if matches!(entries.get(i), Some(next) if next.level > base) && depth < MAX_LIST_DEPTH {
            let (sub, consumed) = build_one_list(entries.get(i..).unwrap_or(&[]), depth + 1);
            item.push(sub);
            i += consumed;
            items.push(item);
            break;
        }
        items.push(item);
    }
    let block = match numbering {
        Some((start, style)) => Block::OrderedList(
            ListAttributes {
                start,
                style,
                delim: ListNumberDelim::Period,
            },
            items,
        ),
        None => Block::BulletList(items),
    };
    (block, i)
}
