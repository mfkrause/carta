//! Reader for the Rich Text Format (RTF), the word-processor interchange language.
//!
//! An RTF document is a tree of brace-delimited groups holding three things: control words
//! (`\word`, optionally with a numeric argument), control symbols (`\` before a single non-letter),
//! and literal text. A group scopes formatting: entering one saves the character state, leaving one
//! restores it. Reading proceeds in one pass over a token stream ([`tokenize`]), with a stack of
//! group states carrying the active character formatting and a stack of block-building contexts (a
//! fresh one opens for each footnote).
//!
//! Character control words toggle formatting (`\b`, `\i`, `\ul`, `\strike`, `\super`, `\sub`,
//! `\scaps`, `\caps`); text between them is wrapped in the corresponding inline nodes, nested in a
//! fixed order and coalesced so a run that stays bold across an italic span keeps one enclosing
//! bold. Paragraph breaks (`\par`) close paragraphs; `\outlinelevelN` turns one into a heading;
//! `\line` is a hard break. Encoded characters arrive as `\'xx` (a byte in the ANSI code page) or
//! `\uN` (a Unicode scalar with a following fallback the reader skips). Structural groups are
//! recognized by their leading destination word: `\info` fills document metadata, `\pict` decodes an
//! embedded image into the media bag, `\field` unpacks a hyperlink, `\footnote` becomes a note, and
//! `\*\bkmkstart`/`\*\bkmkend` bracket a bookmark span. Font, color, and style tables — and any
//! group flagged ignorable with `\*` — are skipped. A run of `\trowd`/`\cell`/`\row` rows assembles
//! a table.

use std::collections::BTreeMap;
use std::mem::take;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, MetaValue, Row,
    Table, TableBody, Target, Text,
};
use carta_core::media::content_addressed_name;
use carta_core::{MediaBag, Reader, ReaderOptions, Result};

/// Parses a Rich Text Format document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct RtfReader;

impl Reader for RtfReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        Ok(self.read_media(input, options)?.0)
    }

    fn read_media(&self, input: &str, _options: &ReaderOptions) -> Result<(Document, MediaBag)> {
        let tokens = tokenize(input);
        let mut parser = Parser::new(tokens);
        parser.run();
        let (meta, blocks, media) = parser.finish();
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

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// One lexical unit of an RTF stream.
#[derive(Debug, Clone)]
enum Token {
    /// `{` — opens a group.
    GroupStart,
    /// `}` — closes a group.
    GroupEnd,
    /// `\word` with an optional trailing numeric argument.
    Control(String, Option<i32>),
    /// `\` before a single non-letter character (e.g. `\~`, `\-`, `\\`).
    Symbol(char),
    /// `\'xx` — a raw byte in the document's code page.
    Hex(u8),
    /// A literal text character.
    Char(char),
    /// A literal space; a run of them collapses when emitted.
    Space,
}

/// Splits an RTF source string into its token stream. Carriage returns, line feeds, and literal
/// tabs are structural whitespace in the wire form and carry no content, so they are dropped.
fn tokenize(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '{' => {
                tokens.push(Token::GroupStart);
                i += 1;
            }
            '}' => {
                tokens.push(Token::GroupEnd);
                i += 1;
            }
            '\\' => {
                i = lex_backslash(&chars, i, &mut tokens);
            }
            '\r' | '\n' | '\t' => i += 1,
            ' ' => {
                tokens.push(Token::Space);
                i += 1;
            }
            _ => {
                tokens.push(Token::Char(c));
                i += 1;
            }
        }
    }
    tokens
}

/// Whether `c` is the delimiter that ends a control word and is consumed with it. A space always
/// qualifies; any other single printable punctuation mark does too, except the three characters that
/// open their own token (`{` and `}` begin and end groups, `\` starts the next control sequence) and
/// letters or digits, which would extend the word or its numeric parameter instead.
fn is_control_delimiter(c: char) -> bool {
    c == ' '
        || (c.is_ascii_graphic() && !c.is_ascii_alphanumeric() && !matches!(c, '{' | '}' | '\\'))
}

/// Lexes one backslash-introduced token starting at `start` (the backslash). Returns the index just
/// past what was consumed.
fn lex_backslash(chars: &[char], start: usize, tokens: &mut Vec<Token>) -> usize {
    let mut i = start + 1;
    match chars.get(i) {
        None => i,
        Some(&n) if n.is_ascii_alphabetic() => {
            let word_start = i;
            while matches!(chars.get(i), Some(c) if c.is_ascii_alphabetic()) {
                i += 1;
            }
            let word: String = chars.get(word_start..i).unwrap_or(&[]).iter().collect();
            let negative = matches!(chars.get(i), Some('-'));
            let digits_start = if negative { i + 1 } else { i };
            let mut j = digits_start;
            while matches!(chars.get(j), Some(c) if c.is_ascii_digit()) {
                j += 1;
            }
            let param = if j > digits_start {
                let digits: String = chars.get(digits_start..j).unwrap_or(&[]).iter().collect();
                i = j;
                digits.parse::<i64>().ok().map(|value| {
                    let signed = if negative { -value } else { value };
                    let clamped = signed.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
                    i32::try_from(clamped).unwrap_or(0)
                })
            } else {
                None
            };
            // A control word ends at a delimiter, which is absorbed with the word when it is a
            // space or a lone punctuation mark that does not itself begin another token.
            if matches!(chars.get(i), Some(&c) if is_control_delimiter(c)) {
                i += 1;
            }
            tokens.push(Token::Control(word, param));
            i
        }
        Some(&'\'') => {
            i += 1;
            let hi = chars.get(i).and_then(|c| c.to_digit(16));
            let lo = chars.get(i + 1).and_then(|c| c.to_digit(16));
            match (hi, lo) {
                (Some(hi), Some(lo)) => {
                    tokens.push(Token::Hex(u8::try_from((hi << 4) | lo).unwrap_or(0)));
                    i + 2
                }
                _ => i,
            }
        }
        Some(&symbol) => {
            tokens.push(Token::Symbol(symbol));
            i + 1
        }
    }
}

// ---------------------------------------------------------------------------
// Character state
// ---------------------------------------------------------------------------

/// The active character formatting. Copied on group entry and restored on exit. Compared for
/// equality to merge adjacent text sharing the same formatting into one run. Each field is an
/// independent on/off attribute the format toggles separately, so a flat set of flags models it
/// directly.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct CharProps {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    superscript: bool,
    subscript: bool,
    smallcaps: bool,
    allcaps: bool,
    hidden: bool,
}

impl CharProps {
    /// Whether two runs share the same inline wrappers, so a text run stays unbroken across them.
    /// `allcaps` is folded into each character as it is pushed and `hidden` content is dropped before
    /// it becomes a run, so neither contributes a wrapper and neither alone splits a run.
    fn same_run(self, other: Self) -> bool {
        Self {
            allcaps: false,
            hidden: false,
            ..self
        } == Self {
            allcaps: false,
            hidden: false,
            ..other
        }
    }
}

/// A group's saved state: the character formatting plus the Unicode fallback skip count (`\ucN`).
#[derive(Debug, Clone, Copy)]
struct GroupState {
    props: CharProps,
    uc: i32,
}

impl Default for GroupState {
    fn default() -> Self {
        Self {
            props: CharProps::default(),
            uc: 1,
        }
    }
}

/// One enclosing inline wrapper. The declaration order is the nesting order applied to a run:
/// earlier variants wrap later ones, regardless of the order the source enabled them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wrapper {
    Strong,
    Emph,
    Strikeout,
    Subscript,
    Superscript,
    SmallCaps,
    Underline,
}

impl Wrapper {
    fn wrap(self, children: Vec<Inline>) -> Inline {
        match self {
            Wrapper::Strong => Inline::Strong(children),
            Wrapper::Emph => Inline::Emph(children),
            Wrapper::Strikeout => Inline::Strikeout(children),
            Wrapper::Subscript => Inline::Subscript(children),
            Wrapper::Superscript => Inline::Superscript(children),
            Wrapper::SmallCaps => Inline::SmallCaps(children),
            Wrapper::Underline => Inline::Underline(children),
        }
    }
}

/// The wrapper path implied by a character state, outermost first.
fn wrappers(props: CharProps) -> Vec<Wrapper> {
    let mut path = Vec::new();
    if props.bold {
        path.push(Wrapper::Strong);
    }
    if props.italic {
        path.push(Wrapper::Emph);
    }
    if props.strike {
        path.push(Wrapper::Strikeout);
    }
    if props.subscript {
        path.push(Wrapper::Subscript);
    }
    if props.superscript {
        path.push(Wrapper::Superscript);
    }
    if props.smallcaps {
        path.push(Wrapper::SmallCaps);
    }
    if props.underline {
        path.push(Wrapper::Underline);
    }
    path
}

// ---------------------------------------------------------------------------
// Inline assembly
// ---------------------------------------------------------------------------

/// A leaf produced within a paragraph, tagged with the formatting active when it was emitted.
#[derive(Debug, Clone)]
struct Atom {
    props: CharProps,
    kind: AtomKind,
}

#[derive(Debug, Clone)]
enum AtomKind {
    Text(String),
    Space,
    LineBreak,
    /// An already-built inline (link, image, note, or bookmark span) inserted verbatim.
    Node(Inline),
}

/// One level of the bookmark nesting active while a paragraph is being built. The root frame has no
/// bookmark; each `\*\bkmkstart` pushes a named frame that `\*\bkmkend` folds into a span.
#[derive(Debug, Clone)]
struct Frame {
    bookmark: Option<Text>,
    atoms: Vec<Atom>,
}

/// Turns a bookmark frame into a span carrying the bookmark name as its identifier.
fn bookmark_span(frame: Frame) -> Inline {
    Inline::Span(
        Box::new(Attr {
            id: frame.bookmark.unwrap_or_default(),
            classes: Vec::new(),
            attributes: Vec::new(),
        }),
        build_inlines(frame.atoms),
    )
}

/// Builds nested inlines from a flat, formatting-tagged atom sequence. Adjacent atoms sharing a
/// common wrapper prefix stay inside a single instance of that wrapper; a divergence closes the
/// wrappers past the shared prefix and opens the new ones.
fn build_inlines(atoms: Vec<Atom>) -> Vec<Inline> {
    let mut root: Vec<Inline> = Vec::new();
    let mut open: Vec<(Wrapper, Vec<Inline>)> = Vec::new();

    let close_to =
        |open: &mut Vec<(Wrapper, Vec<Inline>)>, root: &mut Vec<Inline>, depth: usize| {
            while open.len() > depth {
                if let Some((wrapper, children)) = open.pop() {
                    let inline = wrapper.wrap(children);
                    match open.last_mut() {
                        Some((_, parent)) => parent.push(inline),
                        None => root.push(inline),
                    }
                }
            }
        };

    for atom in atoms {
        let path = wrappers(atom.props);
        let mut shared = 0;
        while shared < open.len()
            && open
                .get(shared)
                .zip(path.get(shared))
                .is_some_and(|((wrapper, _), next)| wrapper == next)
        {
            shared += 1;
        }
        close_to(&mut open, &mut root, shared);
        for &wrapper in path.get(shared..).unwrap_or(&[]) {
            open.push((wrapper, Vec::new()));
        }
        let base = match atom.kind {
            AtomKind::Text(text) => Inline::Str(text.into()),
            AtomKind::Space => Inline::Space,
            AtomKind::LineBreak => Inline::LineBreak,
            AtomKind::Node(node) => node,
        };
        match open.last_mut() {
            Some((_, children)) => children.push(base),
            None => root.push(base),
        }
    }
    close_to(&mut open, &mut root, 0);
    root
}

// ---------------------------------------------------------------------------
// Block context
// ---------------------------------------------------------------------------

/// One block-building context: the emitted blocks plus the paragraph and table under construction.
/// The document has one; every footnote opens another.
#[derive(Debug)]
struct Emitter {
    blocks: Vec<Block>,
    frames: Vec<Frame>,
    pending_text: String,
    pending_props: CharProps,
    pending_space: bool,
    space_props: CharProps,
    has_content: bool,
    outline_level: Option<i32>,
    in_table_para: bool,
    rows: Vec<Row>,
    cells: Vec<Cell>,
    cell_blocks: Vec<Block>,
    columns: usize,
    row_cell_bounds: usize,
    list_active: bool,
    list_level: i32,
    pending_list: Vec<ListParagraph>,
}

/// One paragraph belonging to a list, tagged with its nesting level (`\ilvl`). Consecutive entries
/// are reassembled into nested [`Block::BulletList`]s when the list run ends.
#[derive(Debug)]
struct ListParagraph {
    level: i32,
    block: Block,
}

impl Emitter {
    fn new() -> Self {
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
            outline_level: None,
            in_table_para: false,
            rows: Vec::new(),
            cells: Vec::new(),
            cell_blocks: Vec::new(),
            columns: 0,
            row_cell_bounds: 0,
            list_active: false,
            list_level: 0,
            pending_list: Vec::new(),
        }
    }

    fn frame_atoms(&mut self) -> &mut Vec<Atom> {
        if self.frames.is_empty() {
            self.frames.push(Frame {
                bookmark: None,
                atoms: Vec::new(),
            });
        }
        // The guard above guarantees a last element.
        match self.frames.last_mut() {
            Some(frame) => &mut frame.atoms,
            None => unreachable_empty(),
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
            if self.has_content {
                let props = self.space_props;
                self.frame_atoms().push(Atom {
                    props,
                    kind: AtomKind::Space,
                });
            }
        }
    }

    fn push_char(&mut self, c: char, props: CharProps) {
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

    fn push_str(&mut self, s: &str, props: CharProps) {
        for c in s.chars() {
            self.push_char(c, props);
        }
    }

    fn push_space(&mut self, props: CharProps) {
        if props.hidden {
            return;
        }
        self.flush_text();
        if !self.pending_space {
            self.pending_space = true;
            self.space_props = props;
        }
    }

    fn push_break(&mut self, props: CharProps) {
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

    fn push_node(&mut self, node: Inline, props: CharProps) {
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

    fn open_bookmark(&mut self, id: Text) {
        self.flush_text();
        self.resolve_space();
        self.frames.push(Frame {
            bookmark: Some(id),
            atoms: Vec::new(),
        });
    }

    fn close_bookmark(&mut self) {
        self.flush_text();
        self.resolve_space();
        if self.frames.len() <= 1 {
            return;
        }
        if let Some(frame) = self.frames.pop() {
            self.fold_bookmark(frame);
            self.has_content = true;
        }
    }

    /// Wraps a popped bookmark frame into a span node in its parent frame.
    fn fold_bookmark(&mut self, frame: Frame) {
        let span = bookmark_span(frame);
        self.frame_atoms().push(Atom {
            props: CharProps::default(),
            kind: AtomKind::Node(span),
        });
    }

    /// Closes the current paragraph's inline content, folding any unclosed bookmarks into spans.
    fn take_inlines(&mut self) -> Vec<Inline> {
        self.flush_text();
        self.pending_space = false;
        while self.frames.len() > 1 {
            if let Some(frame) = self.frames.pop() {
                self.fold_bookmark(frame);
            }
        }
        self.has_content = false;
        let atoms = match self.frames.first_mut() {
            Some(frame) => take(&mut frame.atoms),
            None => Vec::new(),
        };
        build_inlines(atoms)
    }

    /// Ends a paragraph: the accumulated inlines become a paragraph or heading, routed into the
    /// current table cell when the paragraph is marked in-table, otherwise into the block stream.
    fn end_paragraph(&mut self) {
        let inlines = self.take_inlines();
        if inlines.is_empty() {
            return;
        }
        let block = match self.outline_level {
            Some(level) => Block::Header((level + 1).max(1), Box::default(), inlines),
            None => Block::Para(inlines),
        };
        if self.in_table_para {
            self.cell_blocks.push(block);
        } else if self.list_active {
            // A list paragraph joins the pending run; any open table is closed ahead of it so the
            // list and table keep their source order.
            self.finish_table();
            self.pending_list.push(ListParagraph {
                level: self.list_level,
                block,
            });
        } else {
            self.flush_list();
            self.finish_table();
            self.blocks.push(block);
        }
    }

    /// Emits the buffered list paragraphs as nested bullet lists, in source order.
    fn flush_list(&mut self) {
        if self.pending_list.is_empty() {
            return;
        }
        let entries = take(&mut self.pending_list);
        self.blocks.extend(build_bullet_lists(&entries));
    }

    fn end_cell(&mut self) {
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

    fn end_row(&mut self) {
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

    fn begin_row_definition(&mut self) {
        self.row_cell_bounds = 0;
    }

    fn note_cell_boundary(&mut self) {
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

    fn finish_blocks(mut self) -> Vec<Block> {
        self.end_paragraph();
        self.flush_list();
        self.finish_table();
        self.blocks
    }

    /// Flattens the context to a single inline sequence, for content that is inline by construction
    /// (a hyperlink's display text). Any block breaks inside contribute their inlines in order.
    fn finish_inlines(mut self) -> Vec<Inline> {
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

/// Reassembles a run of list paragraphs into nested bullet lists. A maximal span of entries at the
/// shallowest level forms one list; a return to that level after a deeper span starts a fresh
/// sibling list, so denesting is expressed as consecutive lists just as the format records it.
fn build_bullet_lists(entries: &[ListParagraph]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut rest = entries;
    while !rest.is_empty() {
        let (block, consumed) = build_one_bullet_list(rest, 0);
        out.push(block);
        let step = consumed.max(1).min(rest.len());
        rest = rest.get(step..).unwrap_or(&[]);
    }
    out
}

/// Builds a single [`Block::BulletList`] from the front of `entries`, consuming every entry at the
/// list's own level and folding any deeper entry that follows an item into a nested list inside it.
/// Returns the list and how many entries it consumed. `depth` caps recursion so pathologically deep
/// nesting degrades into sibling lists instead of overflowing the stack.
fn build_one_bullet_list(entries: &[ListParagraph], depth: usize) -> (Block, usize) {
    const MAX_LIST_DEPTH: usize = 256;
    let base = entries.first().map_or(0, |entry| entry.level);
    let mut items: Vec<Vec<Block>> = Vec::new();
    let mut i = 0;
    while let Some(entry) = entries.get(i) {
        if entry.level != base {
            break;
        }
        let mut item = vec![entry.block.clone()];
        i += 1;
        if matches!(entries.get(i), Some(next) if next.level > base) && depth < MAX_LIST_DEPTH {
            let (sub, consumed) = build_one_bullet_list(entries.get(i..).unwrap_or(&[]), depth + 1);
            item.push(sub);
            i += consumed;
            items.push(item);
            break;
        }
        items.push(item);
    }
    (Block::BulletList(items), i)
}

/// Unreachable helper kept panic-free: the callers guarantee a non-empty frame stack, but rather
/// than index or unwrap, an empty stack yields an empty static scratch buffer.
fn unreachable_empty() -> &'static mut Vec<Atom> {
    // A leaked empty vector is only ever reached if the frame stack is empty, which the guards
    // above prevent; leaking nothing keeps the signature total without a panic.
    Box::leak(Box::new(Vec::new()))
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Document-metadata destination words carried into the `meta` map as inline values.
const META_FIELDS: &[&str] = &[
    "title", "author", "keywords", "subject", "comment", "company", "doccomm", "operator",
    "category", "manager",
];

/// Destination words whose entire group is discarded (tables, styling, and layout apparatus that
/// carries no body content).
const SKIP_DESTINATIONS: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "listtable",
    "listoverridetable",
    "listtext",
    "revtbl",
    "rsidtbl",
    "generator",
    "filetbl",
    "pgdsctbl",
    "header",
    "headerl",
    "headerr",
    "headerf",
    "footer",
    "footerl",
    "footerr",
    "footerf",
    "pnseclvl",
    "themedata",
    "colorschememapping",
    "latentstyles",
    "datastore",
    "nonshppict",
    "xmlnstbl",
    "wgrffmtfilter",
    "template",
    "fchars",
    "lchars",
    "atnid",
    "atnauthor",
    "annotation",
    "pict",
];

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    states: Vec<GroupState>,
    emitters: Vec<Emitter>,
    media: MediaBag,
    meta: BTreeMap<Text, MetaValue>,
    skip: usize,
    pending_high_surrogate: Option<u32>,
    depth: usize,
}

/// Ceiling on nested group depth. Beyond it a group's content is discarded rather than descended
/// into, so adversarially deep nesting cannot exhaust the call stack.
const MAX_GROUP_DEPTH: usize = 512;

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            states: vec![GroupState::default()],
            emitters: vec![Emitter::new()],
            media: MediaBag::new(),
            meta: BTreeMap::new(),
            skip: 0,
            pending_high_surrogate: None,
            depth: 0,
        }
    }

    fn run(&mut self) {
        self.process();
    }

    fn finish(mut self) -> (BTreeMap<Text, MetaValue>, Vec<Block>, MediaBag) {
        while self.emitters.len() > 1 {
            self.emitters.pop();
        }
        let blocks = match self.emitters.pop() {
            Some(emitter) => emitter.finish_blocks(),
            None => Vec::new(),
        };
        (self.meta, blocks, self.media)
    }

    fn props(&self) -> CharProps {
        self.states
            .last()
            .map(|state| state.props)
            .unwrap_or_default()
    }

    fn state_mut(&mut self) -> &mut GroupState {
        if self.states.is_empty() {
            self.states.push(GroupState::default());
        }
        self.states
            .last_mut()
            .unwrap_or_else(|| unreachable_state())
    }

    fn emitter(&mut self) -> Option<&mut Emitter> {
        self.emitters.last_mut()
    }

    /// Processes tokens at the current group level, returning once the matching `}` is consumed or
    /// input is exhausted.
    fn process(&mut self) {
        while self.pos < self.tokens.len() {
            if self.skip > 0 && self.consume_skipped() {
                continue;
            }
            let token = match self.tokens.get(self.pos) {
                Some(token) => token.clone(),
                None => break,
            };
            match token {
                Token::GroupEnd => {
                    self.pos += 1;
                    return;
                }
                Token::GroupStart => {
                    self.pos += 1;
                    self.enter_group();
                }
                Token::Control(word, param) => {
                    self.pos += 1;
                    self.handle_control(&word, param);
                }
                Token::Symbol(symbol) => {
                    self.pos += 1;
                    self.handle_symbol(symbol);
                }
                Token::Hex(byte) => {
                    self.pos += 1;
                    let props = self.props();
                    if let Some(emitter) = self.emitter() {
                        emitter.push_char(code_page_char(byte), props);
                    }
                }
                Token::Char(c) => {
                    self.pos += 1;
                    let props = self.props();
                    if let Some(emitter) = self.emitter() {
                        emitter.push_char(c, props);
                    }
                }
                Token::Space => {
                    self.pos += 1;
                    let props = self.props();
                    if let Some(emitter) = self.emitter() {
                        emitter.push_space(props);
                    }
                }
            }
        }
    }

    /// Skips one Unicode fallback item following a `\uN`. Returns whether an item was consumed;
    /// a group boundary ends the skip run so the boundary itself is handled normally.
    fn consume_skipped(&mut self) -> bool {
        match self.tokens.get(self.pos) {
            Some(Token::GroupEnd) | None => {
                self.skip = 0;
                false
            }
            Some(Token::GroupStart) => {
                self.pos += 1;
                self.skip_group();
                self.skip -= 1;
                true
            }
            Some(_) => {
                self.pos += 1;
                self.skip -= 1;
                true
            }
        }
    }

    /// Handles a group opened at the current position, dispatching on its destination word. Nesting
    /// past [`MAX_GROUP_DEPTH`] discards the group's content instead of descending into it.
    fn enter_group(&mut self) {
        self.depth += 1;
        if self.depth > MAX_GROUP_DEPTH {
            self.skip_group();
            self.depth -= 1;
            return;
        }
        let (ignorable, dest) = self.peek_destination();
        match dest.as_deref() {
            Some("info") => self.parse_info(),
            Some("pict") => self.parse_picture(),
            Some("field") => self.parse_field(),
            Some("footnote") => self.parse_footnote(),
            Some("bkmkstart") => self.parse_bookmark(true),
            Some("bkmkend") => self.parse_bookmark(false),
            Some("shppict") => {
                // A drawing wrapper around a `\pict`; process transparently so the picture inside
                // is decoded, but do not save/restore character state for it.
                self.skip_optional_marker();
                self.pos += 1; // the `shppict` word
                self.process();
            }
            Some(word) if SKIP_DESTINATIONS.contains(&word) => self.skip_group(),
            _ if ignorable => self.skip_group(),
            _ => {
                self.states
                    .push(self.states.last().copied().unwrap_or_default());
                self.process();
                self.states.pop();
            }
        }
        self.depth -= 1;
    }

    /// Looks at the token after `{` to classify the group: whether it is flagged ignorable (`\*`)
    /// and its leading destination word, if any. Does not advance.
    fn peek_destination(&self) -> (bool, Option<String>) {
        let ignorable = matches!(self.tokens.get(self.pos), Some(Token::Symbol('*')));
        let word_pos = if ignorable { self.pos + 1 } else { self.pos };
        let word = match self.tokens.get(word_pos) {
            Some(Token::Control(word, _)) => Some(word.clone()),
            _ => None,
        };
        (ignorable, word)
    }

    fn skip_optional_marker(&mut self) {
        if matches!(self.tokens.get(self.pos), Some(Token::Symbol('*'))) {
            self.pos += 1;
        }
    }

    /// Consumes the current group in full, discarding its content.
    fn skip_group(&mut self) {
        let mut depth = 1;
        while let Some(token) = self.tokens.get(self.pos) {
            self.pos += 1;
            match token {
                Token::GroupStart => depth += 1,
                Token::GroupEnd => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_control(&mut self, word: &str, param: Option<i32>) {
        let on = param != Some(0);
        match word {
            "b" => self.state_mut().props.bold = on,
            "i" => self.state_mut().props.italic = on,
            "ul" => self.state_mut().props.underline = on,
            "ulnone" => self.state_mut().props.underline = false,
            "uld" | "uldb" | "ulw" | "uldash" | "uldashd" | "uldashdd" | "ulhwave" | "ulth"
            | "ulthd" | "ulwave" => self.state_mut().props.underline = true,
            "strike" | "striked" => self.state_mut().props.strike = on,
            "super" | "superscript" => self.state_mut().props.superscript = on,
            "sub" | "subscript" => self.state_mut().props.subscript = on,
            "nosupersub" => {
                let props = &mut self.state_mut().props;
                props.superscript = false;
                props.subscript = false;
            }
            "scaps" => self.state_mut().props.smallcaps = on,
            "caps" => self.state_mut().props.allcaps = on,
            "v" => self.state_mut().props.hidden = on,
            "plain" => self.state_mut().props = CharProps::default(),
            "pard" => {
                self.state_mut().props = CharProps::default();
                if let Some(emitter) = self.emitter() {
                    emitter.outline_level = None;
                    emitter.in_table_para = false;
                    emitter.list_active = false;
                    emitter.list_level = 0;
                }
            }
            "ls" => {
                if let Some(emitter) = self.emitter() {
                    emitter.list_active = true;
                }
            }
            "ilvl" => {
                if let Some(emitter) = self.emitter() {
                    emitter.list_level = param.unwrap_or(0);
                }
            }
            "uc" => self.state_mut().uc = param.unwrap_or(1).max(0),
            "u" => self.handle_unicode(param),
            "par" | "sectd" if word == "par" => {
                if let Some(emitter) = self.emitter() {
                    emitter.end_paragraph();
                }
            }
            "line" | "softline" => {
                let props = self.props();
                if let Some(emitter) = self.emitter() {
                    emitter.push_break(props);
                }
            }
            "tab" => {
                let props = self.props();
                if let Some(emitter) = self.emitter() {
                    emitter.push_space(props);
                }
            }
            "cell" | "nestcell" => {
                if let Some(emitter) = self.emitter() {
                    emitter.end_cell();
                }
            }
            "row" | "nestrow" => {
                if let Some(emitter) = self.emitter() {
                    emitter.end_row();
                }
            }
            "intbl" => {
                if let Some(emitter) = self.emitter() {
                    emitter.in_table_para = true;
                }
            }
            "trowd" => {
                if let Some(emitter) = self.emitter() {
                    emitter.begin_row_definition();
                }
            }
            "cellx" => {
                if let Some(emitter) = self.emitter() {
                    emitter.note_cell_boundary();
                }
            }
            "outlinelevel" => {
                if let Some(emitter) = self.emitter() {
                    emitter.outline_level = Some(param.unwrap_or(0));
                }
            }
            _ => {
                if let Some(text) = special_char(word) {
                    let props = self.props();
                    if let Some(emitter) = self.emitter() {
                        emitter.push_str(text, props);
                    }
                }
            }
        }
    }

    fn handle_symbol(&mut self, symbol: char) {
        if let Some(text) = symbol_char(symbol) {
            let props = self.props();
            if let Some(emitter) = self.emitter() {
                emitter.push_str(text, props);
            }
        }
    }

    fn handle_unicode(&mut self, param: Option<i32>) {
        let raw = param.unwrap_or(0);
        let code = if raw < 0 {
            u32::try_from(i64::from(raw) + 65536).unwrap_or(0)
        } else {
            u32::try_from(raw).unwrap_or(0)
        };
        if (0xD800..=0xDBFF).contains(&code) {
            self.pending_high_surrogate = Some(code);
        } else if (0xDC00..=0xDFFF).contains(&code) {
            if let Some(high) = self.pending_high_surrogate.take() {
                let combined = 0x1_0000 + ((high - 0xD800) << 10) + (code - 0xDC00);
                self.emit_scalar(combined);
            }
        } else {
            self.pending_high_surrogate = None;
            self.emit_scalar(code);
        }
        let uc = self.states.last().map_or(1, |state| state.uc);
        self.skip = usize::try_from(uc).unwrap_or(0);
    }

    fn emit_scalar(&mut self, code: u32) {
        if let Some(c) = char::from_u32(code) {
            let props = self.props();
            if let Some(emitter) = self.emitter() {
                emitter.push_char(c, props);
            }
        }
    }

    // --- Structural destinations ------------------------------------------

    fn parse_info(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `info` word
        loop {
            match self.tokens.get(self.pos) {
                None => break,
                Some(Token::GroupEnd) => {
                    self.pos += 1;
                    break;
                }
                Some(Token::GroupStart) => {
                    self.pos += 1;
                    self.parse_info_field();
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    fn parse_info_field(&mut self) {
        self.skip_optional_marker();
        let name = match self.tokens.get(self.pos) {
            Some(Token::Control(word, _)) => {
                let word = word.clone();
                self.pos += 1;
                Some(word)
            }
            _ => None,
        };
        let text = self.collect_text();
        let Some(name) = name.filter(|name| META_FIELDS.contains(&name.as_str())) else {
            return;
        };
        let inlines = text_to_inlines(&text);
        if !inlines.is_empty() {
            self.meta
                .insert(name.into(), MetaValue::MetaInlines(inlines));
        }
    }

    fn parse_picture(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `pict` word
        let mut extension: Option<&'static str> = None;
        let mut hex = String::new();
        let mut depth = 1;
        while let Some(token) = self.tokens.get(self.pos) {
            self.pos += 1;
            match token {
                Token::GroupStart => depth += 1,
                Token::GroupEnd => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Token::Control(word, _) => {
                    extension = extension.or(match word.as_str() {
                        "pngblip" => Some("png"),
                        "jpegblip" => Some("jpg"),
                        "emfblip" => Some("emf"),
                        _ => None,
                    });
                }
                Token::Char(c) if c.is_ascii_hexdigit() => hex.push(*c),
                _ => {}
            }
        }
        if let Some(extension) = extension {
            let bytes = decode_hex(&hex);
            if !bytes.is_empty() {
                let mime = match extension {
                    "png" => "image/png",
                    "jpg" => "image/jpeg",
                    _ => "image/emf",
                };
                let name = content_addressed_name(mime, &bytes);
                self.media
                    .insert(name.clone(), Some(mime.to_string()), bytes);
                let props = self.props();
                let image = Inline::Image(
                    Box::default(),
                    vec![Inline::Str(Text::from("image"))],
                    Box::new(Target {
                        url: name.into(),
                        title: Text::default(),
                    }),
                );
                if let Some(emitter) = self.emitter() {
                    emitter.push_node(image, props);
                }
            }
        }
    }

    fn parse_field(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `field` word
        let mut url: Option<String> = None;
        let mut display: Vec<Inline> = Vec::new();
        let mut depth = 1;
        while let Some(token) = self.tokens.get(self.pos) {
            match token {
                Token::GroupEnd => {
                    self.pos += 1;
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Token::GroupStart => {
                    self.pos += 1;
                    self.skip_optional_marker();
                    let word = match self.tokens.get(self.pos) {
                        Some(Token::Control(word, _)) => {
                            let word = word.clone();
                            self.pos += 1;
                            Some(word)
                        }
                        _ => None,
                    };
                    match word.as_deref() {
                        Some("fldinst") => {
                            let instruction = self.collect_text();
                            if let Some(found) = parse_hyperlink(&instruction) {
                                url = Some(found);
                            }
                        }
                        Some("fldrslt") => display = self.collect_group_inlines(),
                        _ => self.skip_current_group(),
                    }
                }
                _ => self.pos += 1,
            }
        }
        let props = self.props();
        match url {
            Some(url) => {
                let link = Inline::Link(
                    Box::default(),
                    display,
                    Box::new(Target {
                        url: url.into(),
                        title: Text::default(),
                    }),
                );
                if let Some(emitter) = self.emitter() {
                    emitter.push_node(link, props);
                }
            }
            None => {
                for inline in display {
                    if let Some(emitter) = self.emitter() {
                        emitter.push_node(inline, CharProps::default());
                    }
                }
            }
        }
    }

    fn parse_footnote(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `footnote` word
        self.emitters.push(Emitter::new());
        self.states.push(GroupState::default());
        self.process();
        self.states.pop();
        let blocks = match self.emitters.pop() {
            Some(emitter) => emitter.finish_blocks(),
            None => Vec::new(),
        };
        let props = self.props();
        if let Some(emitter) = self.emitter() {
            emitter.push_node(Inline::Note(blocks), props);
        }
    }

    fn parse_bookmark(&mut self, start: bool) {
        self.skip_optional_marker();
        self.pos += 1; // the `bkmkstart` / `bkmkend` word
        let name = self.collect_text();
        let name = name.trim();
        if let Some(emitter) = self.emitter() {
            if start {
                emitter.open_bookmark(Text::from(name));
            } else {
                emitter.close_bookmark();
            }
        }
    }

    /// Builds the inline content of the group opened at the current position, in a throwaway block
    /// context, and returns it flattened.
    fn collect_group_inlines(&mut self) -> Vec<Inline> {
        self.emitters.push(Emitter::new());
        self.states
            .push(self.states.last().copied().unwrap_or_default());
        self.process();
        self.states.pop();
        match self.emitters.pop() {
            Some(emitter) => emitter.finish_inlines(),
            None => Vec::new(),
        }
    }

    fn skip_current_group(&mut self) {
        let mut depth = 1;
        while let Some(token) = self.tokens.get(self.pos) {
            self.pos += 1;
            match token {
                Token::GroupStart => depth += 1,
                Token::GroupEnd => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    /// Gathers the plain text of the group currently open (its `{` already consumed), through the
    /// matching `}`. Nested groups contribute their text too.
    fn collect_text(&mut self) -> String {
        let mut out = String::new();
        let mut depth = 1;
        while let Some(token) = self.tokens.get(self.pos) {
            self.pos += 1;
            match token {
                Token::GroupStart => depth += 1,
                Token::GroupEnd => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Token::Char(c) => out.push(*c),
                Token::Space => out.push(' '),
                Token::Hex(byte) => out.push(code_page_char(*byte)),
                Token::Control(word, _) => {
                    if let Some(text) = special_char(word) {
                        out.push_str(text);
                    }
                }
                Token::Symbol(symbol) => {
                    if let Some(text) = symbol_char(*symbol) {
                        out.push_str(text);
                    }
                }
            }
        }
        out
    }
}

fn unreachable_state() -> &'static mut GroupState {
    Box::leak(Box::new(GroupState::default()))
}

// ---------------------------------------------------------------------------
// Character mappings and helpers
// ---------------------------------------------------------------------------

/// The Unicode string a special-character control word stands for, or `None` if the word carries no
/// character.
fn special_char(word: &str) -> Option<&'static str> {
    Some(match word {
        "emdash" => "\u{2014}",
        "endash" => "\u{2013}",
        "bullet" => "\u{2022}",
        "lquote" => "\u{2018}",
        "rquote" => "\u{2019}",
        "ldblquote" => "\u{201C}",
        "rdblquote" => "\u{201D}",
        "emspace" => "\u{2003}",
        "enspace" => "\u{2002}",
        "qmspace" => "\u{2005}",
        "zwj" => "\u{200D}",
        "zwnj" => "\u{200C}",
        "ltrmark" => "\u{200E}",
        "rtlmark" => "\u{200F}",
        _ => return None,
    })
}

/// The character a control symbol (`\` before one non-letter) stands for, or `None` if it carries
/// no text.
fn symbol_char(symbol: char) -> Option<&'static str> {
    Some(match symbol {
        '\\' => "\\",
        '{' => "{",
        '}' => "}",
        '~' => "\u{00A0}",
        '-' => "\u{00AD}",
        '_' => "\u{2011}",
        _ => return None,
    })
}

/// Maps a code-page byte to a character. Bytes outside `0x80..=0x9F` are Latin-1; that window uses
/// the Windows-1252 assignments, the code page an unqualified `\ansi` document carries.
fn code_page_char(byte: u8) -> char {
    let scalar: u32 = match byte {
        0x80 => 0x20AC,
        0x82 => 0x201A,
        0x83 => 0x0192,
        0x84 => 0x201E,
        0x85 => 0x2026,
        0x86 => 0x2020,
        0x87 => 0x2021,
        0x88 => 0x02C6,
        0x89 => 0x2030,
        0x8A => 0x0160,
        0x8B => 0x2039,
        0x8C => 0x0152,
        0x8E => 0x017D,
        0x91 => 0x2018,
        0x92 => 0x2019,
        0x93 => 0x201C,
        0x94 => 0x201D,
        0x95 => 0x2022,
        0x96 => 0x2013,
        0x97 => 0x2014,
        0x98 => 0x02DC,
        0x99 => 0x2122,
        0x9A => 0x0161,
        0x9B => 0x203A,
        0x9C => 0x0153,
        0x9E => 0x017E,
        0x9F => 0x0178,
        other => u32::from(other),
    };
    char::from_u32(scalar).unwrap_or('\u{FFFD}')
}

/// Decodes a hex-digit string into bytes, ignoring a trailing unpaired digit.
fn decode_hex(hex: &str) -> Vec<u8> {
    let digits: Vec<u32> = hex.chars().filter_map(|c| c.to_digit(16)).collect();
    digits
        .chunks_exact(2)
        .filter_map(|pair| match pair {
            [hi, lo] => u8::try_from((hi << 4) | lo).ok(),
            _ => None,
        })
        .collect()
}

/// Splits flat text into inline words separated by single spaces.
fn text_to_inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    for (index, word) in text.split_whitespace().enumerate() {
        if index > 0 {
            out.push(Inline::Space);
        }
        out.push(Inline::Str(word.into()));
    }
    out
}

/// Extracts a link target from a field instruction. A `HYPERLINK` instruction carries a quoted URL
/// and, with `\l`, a quoted in-document anchor joined to it with `#`. Instructions that are not
/// hyperlinks yield `None`.
fn parse_hyperlink(instruction: &str) -> Option<String> {
    let trimmed = instruction.trim_start();
    if !trimmed
        .get(..9)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("HYPERLINK"))
    {
        return None;
    }
    let quoted = quoted_strings(instruction);
    let has_anchor = instruction.contains("\\l");
    if has_anchor {
        match quoted.split_last() {
            Some((anchor, [])) => Some(format!("#{anchor}")),
            Some((anchor, rest)) => {
                let base = rest.last().map_or("", String::as_str);
                Some(format!("{base}#{anchor}"))
            }
            None => None,
        }
    } else {
        quoted.into_iter().next()
    }
}

/// The double-quoted substrings of a field instruction, in order.
fn quoted_strings(instruction: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut inside = false;
    for c in instruction.chars() {
        if c == '"' {
            if inside {
                out.push(take(&mut current));
            }
            inside = !inside;
        } else if inside {
            current.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(input: &str) -> Document {
        RtfReader
            .read(input, &ReaderOptions::default())
            .expect("read")
    }

    fn read_media(input: &str) -> (Document, MediaBag) {
        RtfReader
            .read_media(input, &ReaderOptions::default())
            .expect("read")
    }

    fn para(inlines: Vec<Inline>) -> Block {
        Block::Para(inlines)
    }

    fn s(text: &str) -> Inline {
        Inline::Str(text.into())
    }

    #[test]
    fn plain_paragraph_splits_words() {
        let doc = read(r"{\rtf1\ansi Hello world.\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("Hello"), Inline::Space, s("world.")])]
        );
    }

    #[test]
    fn collapses_runs_of_spaces() {
        let doc = read(r"{\rtf1\ansi a  b   c\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                s("a"),
                Inline::Space,
                s("b"),
                Inline::Space,
                s("c"),
            ])]
        );
    }

    #[test]
    fn bold_and_italic_map_to_strong_and_emph() {
        let doc = read(r"{\rtf1\ansi \b bold\b0  normal\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                Inline::Strong(vec![s("bold")]),
                Inline::Space,
                s("normal"),
            ])]
        );
        let doc = read(r"{\rtf1\ansi \i italic\i0\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Emph(vec![s("italic")])])]
        );
    }

    #[test]
    fn nesting_order_is_fixed() {
        let doc = read(r"{\rtf1\ansi \i\b x\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Strong(vec![Inline::Emph(vec![s("x")])])])]
        );
    }

    #[test]
    fn shared_formatting_coalesces_across_inner_group() {
        let doc = read(r"{\rtf1\ansi \b bold {\i both} stillbold\b0 normal\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                Inline::Strong(vec![
                    s("bold"),
                    Inline::Space,
                    Inline::Emph(vec![s("both")]),
                    Inline::Space,
                    s("stillbold"),
                ]),
                s("normal"),
            ])]
        );
    }

    #[test]
    fn formatting_persists_across_par_but_pard_resets() {
        let doc = read(r"{\rtf1\ansi \b bold\par next\par}");
        assert_eq!(
            doc.blocks,
            vec![
                para(vec![Inline::Strong(vec![s("bold")])]),
                para(vec![Inline::Strong(vec![s("next")])]),
            ]
        );
        let doc = read(r"{\rtf1\ansi \b bold\par\pard normal\par}");
        assert_eq!(
            doc.blocks,
            vec![
                para(vec![Inline::Strong(vec![s("bold")])]),
                para(vec![s("normal")]),
            ]
        );
    }

    #[test]
    fn line_break_and_tab() {
        let doc = read(r"{\rtf1\ansi one\line two\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("one"), Inline::LineBreak, s("two")])]
        );
        let doc = read(r"{\rtf1\ansi a\tab b\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("a"), Inline::Space, s("b")])]);
    }

    #[test]
    fn escapes_and_special_characters() {
        let doc = read(r"{\rtf1\ansi a\{b\}c\\d\par}");
        assert_eq!(doc.blocks, vec![para(vec![s(r"a{b}c\d")])]);
        let doc = read(r"{\rtf1\ansi em\emdash dash\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("em\u{2014}dash")])]);
        let doc = read(r"{\rtf1\ansi non\~breaking\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("non\u{00A0}breaking")])]);
    }

    #[test]
    fn hex_escape_uses_code_page() {
        let doc = read("{\\rtf1\\ansi caf\\'e9\\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("caf\u{00E9}")])]);
        let doc = read("{\\rtf1\\ansi \\'80\\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("\u{20AC}")])]);
    }

    #[test]
    fn unicode_with_fallback_skip() {
        let doc = read(r"{\rtf1\ansi \u233 e\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("\u{00E9}")])]);
        let doc = read(r"{\rtf1\ansi \uc2\u233 xx after\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("\u{00E9}"), Inline::Space, s("after")])]
        );
        let doc = read(r"{\rtf1\ansi \uc0\u233 x\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("\u{00E9}x")])]);
    }

    #[test]
    fn negative_unicode_wraps() {
        let doc = read(r"{\rtf1\ansi \u-3647 ?after\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("\u{F1C1}after")])]);
    }

    #[test]
    fn outline_level_becomes_header() {
        let doc = read(r"{\rtf1\ansi \outlinelevel0 Chapter\par}");
        assert_eq!(
            doc.blocks,
            vec![Block::Header(1, Box::default(), vec![s("Chapter")])]
        );
        let doc = read(r"{\rtf1\ansi \outlinelevel2 Sub\par}");
        assert_eq!(
            doc.blocks,
            vec![Block::Header(3, Box::default(), vec![s("Sub")])]
        );
    }

    #[test]
    fn info_group_populates_metadata() {
        let doc = read(r"{\rtf1{\info{\title My Title}{\author Jane Doe}}\ansi Body\par}");
        assert_eq!(
            doc.meta.get("title"),
            Some(&MetaValue::MetaInlines(vec![
                s("My"),
                Inline::Space,
                s("Title")
            ]))
        );
        assert_eq!(
            doc.meta.get("author"),
            Some(&MetaValue::MetaInlines(vec![
                s("Jane"),
                Inline::Space,
                s("Doe")
            ]))
        );
        assert_eq!(doc.blocks, vec![para(vec![s("Body")])]);
    }

    #[test]
    fn destinations_are_skipped() {
        let doc = read(r"{\rtf1{\fonttbl{\f0 Times;}}\ansi text {\*\generator X;}more\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("text"), Inline::Space, s("more")])]
        );
    }

    #[test]
    fn unknown_group_word_keeps_text() {
        let doc = read(r"{\rtf1\ansi text {\madeupword hidden} more\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                s("text"),
                Inline::Space,
                s("hidden"),
                Inline::Space,
                s("more"),
            ])]
        );
    }

    #[test]
    fn hyperlink_field_becomes_link() {
        let doc = read(
            r#"{\rtf1\ansi {\field{\*\fldinst HYPERLINK "http://x.com"}{\fldrslt click}}\par}"#,
        );
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Link(
                Box::default(),
                vec![s("click")],
                Box::new(Target {
                    url: "http://x.com".into(),
                    title: Text::default(),
                }),
            )])]
        );
    }

    #[test]
    fn footnote_becomes_note() {
        let doc = read(r"{\rtf1\ansi text{\footnote note body}more\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                s("text"),
                Inline::Note(vec![para(vec![s("note"), Inline::Space, s("body")])]),
                s("more"),
            ])]
        );
    }

    #[test]
    fn bookmark_becomes_span() {
        let doc = read(r"{\rtf1\ansi {\*\bkmkstart mark}anchored{\*\bkmkend mark}\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Span(
                Box::new(Attr {
                    id: "mark".into(),
                    classes: Vec::new(),
                    attributes: Vec::new(),
                }),
                vec![s("anchored")],
            )])]
        );
    }

    #[test]
    fn table_is_reconstructed() {
        let doc = read(
            r"{\rtf1\ansi \trowd\cellx3000\cellx6000\pard\intbl A\cell\pard\intbl B\cell\row\pard after\par}",
        );
        let Some(Block::Table(table)) = doc.blocks.first() else {
            panic!("expected a leading table, got {:?}", doc.blocks);
        };
        assert_eq!(table.col_specs.len(), 2);
        assert_eq!(table.head.rows.len(), 0);
        let body = table.bodies.first().expect("body");
        assert_eq!(body.row_head_columns, 0);
        assert_eq!(body.body.len(), 1);
        let row = body.body.first().expect("row");
        assert_eq!(row.cells.len(), 2);
        assert_eq!(
            row.cells.first().expect("cell").content,
            vec![para(vec![s("A")])]
        );
        assert_eq!(
            row.cells.get(1).expect("cell").content,
            vec![para(vec![s("B")])]
        );
        assert_eq!(doc.blocks.get(1), Some(&para(vec![s("after")])));
    }

    #[test]
    fn picture_decodes_into_media() {
        let (doc, media) = read_media(r"{\rtf1\ansi {\pict\pngblip 89504e47}\par}");
        let name = content_addressed_name("image/png", &[0x89, 0x50, 0x4e, 0x47]);
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Image(
                Box::default(),
                vec![s("image")],
                Box::new(Target {
                    url: name.clone().into(),
                    title: Text::default(),
                }),
            )])]
        );
        assert!(media.contains(&name));
    }

    #[test]
    fn empty_paragraphs_are_dropped() {
        let doc = read(r"{\rtf1\ansi text\par\par\par more\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("text")]), para(vec![s("more")])]
        );
    }

    #[test]
    fn trailing_content_without_par_flushes() {
        let doc = read(r"{\rtf1\ansi Hello}");
        assert_eq!(doc.blocks, vec![para(vec![s("Hello")])]);
    }

    #[test]
    fn allcaps_uppercases_text() {
        let doc = read(r"{\rtf1\ansi \caps upper\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("UPPER")])]);
    }

    #[test]
    fn small_caps_wraps() {
        let doc = read(r"{\rtf1\ansi \scaps x\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::SmallCaps(vec![s("x")])])]
        );
    }

    #[test]
    fn empty_input_is_empty_document() {
        let doc = read("");
        assert!(doc.blocks.is_empty());
        assert!(doc.meta.is_empty());
    }
}
