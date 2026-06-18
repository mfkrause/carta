//! Inline phase: parse the raw text of leaf blocks into inline nodes.
//!
//! Implements the spec's inline algorithm — a left-to-right scan that resolves code spans,
//! autolinks, raw HTML, entities and escapes immediately, records `*`/`_`/`[`/`![` runs on a
//! delimiter stack, resolves links/images at each `]`, and finally collapses emphasis. The raw
//! char-slice scanners it drives (autolinks, HTML tags, entities, link targets) live in `scan`.

use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Inline, MathType, QuoteType, Row,
    Table, TableBody, TableFoot, TableHead, Target,
};
use carta_core::{Extension, Extensions};

use super::attr;
use super::scan::{
    is_ascii_punctuation, normalize_label, scan_autolink, scan_entity, scan_following_label,
    scan_html_tag, scan_inline_target,
};
use super::{FootnoteDefs, IrBlock, LinkDef, RefMap, para, plain};

/// The empty checkbox emitted for an unchecked task-list item (`- [ ]`).
const TASK_UNCHECKED: &str = "\u{2610}";
/// The checked checkbox emitted for a checked task-list item (`- [x]`).
const TASK_CHECKED: &str = "\u{2612}";

/// Footnote resolution context threaded through the inline phase.
///
/// A reference `[^label]` resolves only when `label` is in `defined`. At the top level it becomes a
/// `Note` carrying the matching content from `by_id`; inside a definition's own body it collapses to
/// an empty string rather than nesting another note.
#[derive(Clone, Copy)]
struct Notes<'a> {
    defined: &'a BTreeSet<String>,
    by_id: &'a BTreeMap<String, Vec<Block>>,
    in_definition: bool,
}

/// Resolve the whole document: first each footnote definition's body (where nested references
/// collapse to empty), then the body itself (where references become notes).
pub(crate) fn resolve_document(
    ir: &[IrBlock],
    refs: &RefMap,
    footnotes: &FootnoteDefs,
    ext: Extensions,
) -> Vec<Block> {
    let defined: BTreeSet<String> = footnotes.keys().cloned().collect();
    let empty = BTreeMap::new();
    let in_def = Notes {
        defined: &defined,
        by_id: &empty,
        in_definition: true,
    };
    let by_id: BTreeMap<String, Vec<Block>> = footnotes
        .iter()
        .map(|(key, body)| (key.clone(), resolve_blocks(body, refs, in_def, ext)))
        .collect();
    let top = Notes {
        defined: &defined,
        by_id: &by_id,
        in_definition: false,
    };
    resolve_blocks(ir, refs, top, ext)
}

fn resolve_blocks(ir: &[IrBlock], refs: &RefMap, notes: Notes, ext: Extensions) -> Vec<Block> {
    let mut out = Vec::with_capacity(ir.len());
    for block in ir {
        resolve_block(block, refs, notes, ext, &mut out);
    }
    out
}

fn resolve_block(
    block: &IrBlock,
    refs: &RefMap,
    notes: Notes,
    ext: Extensions,
    out: &mut Vec<Block>,
) {
    match block {
        IrBlock::Para(text) => out.push(para(parse_inlines(text, refs, notes, ext))),
        IrBlock::Plain(text) => out.push(plain(parse_inlines(text, refs, notes, ext))),
        IrBlock::Heading(level, text) => {
            let (content, attr) = split_header_attr(text, ext);
            out.push(Block::Header(
                *level,
                attr,
                parse_inlines(content, refs, notes, ext),
            ));
        }
        IrBlock::CodeBlock(attr, text) => out.push(Block::CodeBlock(attr.clone(), text.clone())),
        IrBlock::RawHtml(text) => {
            out.push(Block::RawBlock(
                carta_ast::Format("html".to_owned()),
                text.clone(),
            ));
        }
        IrBlock::ThematicBreak => out.push(Block::HorizontalRule),
        IrBlock::Div(attr, children) => {
            out.push(Block::Div(attr.clone(), resolve_blocks(children, refs, notes, ext)));
        }
        IrBlock::BlockQuote(children) => {
            out.push(Block::BlockQuote(resolve_blocks(children, refs, notes, ext)));
        }
        IrBlock::BulletList(items) => resolve_bullet_list(items, refs, notes, ext, out),
        IrBlock::OrderedList(attrs, items) => out.push(Block::OrderedList(
            attrs.clone(),
            items
                .iter()
                .map(|i| resolve_blocks(i, refs, notes, ext))
                .collect(),
        )),
        IrBlock::Table {
            alignments,
            header,
            rows,
        } => out.push(resolve_table(alignments, header, rows, refs, notes, ext)),
    }
}

/// Build a pipe table: column specs from the alignments, the header in a single-row `TableHead`,
/// and the body rows in one `TableBody`. Every cell's trimmed text parses into inlines wrapped in a
/// single `Plain`; an empty cell carries no blocks. Captions, footers, widths, spans, and row-head
/// columns are all the structurally empty defaults.
fn resolve_table(
    alignments: &[Alignment],
    header: &[String],
    rows: &[Vec<String>],
    refs: &RefMap,
    notes: Notes,
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
    Block::Table(Box::new(Table {
        attr: Attr::default(),
        caption: Caption::default(),
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

/// Build one table cell. A non-empty cell's text parses into inlines wrapped in a `Plain`; an empty
/// or whitespace-only cell carries an empty block list.
fn make_cell(text: &str, refs: &RefMap, notes: Notes, ext: Extensions) -> Cell {
    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![Block::Plain(parse_inlines(text, refs, notes, ext))]
    };
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content,
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
    notes: Notes,
    ext: Extensions,
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
        run.push(resolve_item(item, marker.as_ref(), refs, notes, ext));
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
    notes: Notes,
    ext: Extensions,
) -> Vec<Block> {
    let mut out = Vec::new();
    let mut blocks = item.iter();
    if let Some(first) = blocks.next() {
        resolve_block(marker.unwrap_or(first), refs, notes, ext, &mut out);
    }
    for block in blocks {
        resolve_block(block, refs, notes, ext, &mut out);
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
fn split_header_attr(text: &str, ext: Extensions) -> (&str, Attr) {
    if !(ext.contains(Extension::HeaderAttributes) || ext.contains(Extension::Attributes)) {
        return (text, Attr::default());
    }
    let trimmed = text.trim_end();
    if !trimmed.ends_with('}') {
        return (text, Attr::default());
    }
    let chars: Vec<char> = trimmed.chars().collect();
    for start in (0..chars.len()).rev() {
        if chars.get(start) != Some(&'{') {
            continue;
        }
        // The block must be set off from the heading text by whitespace, else it belongs to the
        // preceding word rather than the heading.
        let preceded_by_space = start == 0 || chars.get(start - 1).copied().is_some_and(is_unicode_whitespace);
        if preceded_by_space
            && let Some((attr, end)) = attr::parse_attributes_chars(&chars, start)
            && end == chars.len()
            && attr::is_non_empty(&attr)
        {
            let byte_start: usize = chars.get(..start).map_or(0, |s| s.iter().map(|c| c.len_utf8()).sum());
            let content = text.get(..byte_start).unwrap_or(text).trim_end();
            return (content, attr);
        }
    }
    (text, Attr::default())
}

/// A node in the in-progress inline list. Delimiter runs stay as nodes until emphasis resolution.
#[derive(Debug, Clone)]
enum Node {
    Text(String),
    Inline(Inline),
    SoftBreak,
    LineBreak,
    Delimiter(Delimiter),
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
#[allow(clippy::similar_names)]
fn parse_inlines(text: &str, refs: &RefMap, notes: Notes, ext: Extensions) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut parser = InlineParser {
        chars: &chars,
        pos: 0,
        nodes: Vec::new(),
        refs,
        notes,
        ext,
        bracket_stack: Vec::new(),
    };
    parser.run();
    let mut nodes = parser.nodes;
    process_emphasis(&mut nodes, 0, ext);
    let mut inlines = collapse(nodes);
    if ext.contains(Extension::Autolink) {
        super::autolink::autolink_inlines(&mut inlines);
    }
    inlines
}

struct InlineParser<'a> {
    chars: &'a [char],
    pos: usize,
    nodes: Vec<Node>,
    refs: &'a RefMap,
    notes: Notes<'a>,
    ext: Extensions,
    /// Indices into `nodes` for each open `[` or `![` delimiter, in parse order. O(1) lookup of
    /// the most recent bracket opener instead of a backward scan through all nodes.
    bracket_stack: Vec<usize>,
}

impl InlineParser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn run(&mut self) {
        while let Some(ch) = self.peek() {
            match ch {
                '\\' => self.backslash(),
                '`' => self.code_span(),
                '$' if self.ext.contains(Extension::TexMathDollars) => self.dollar_math(),
                '<' => self.left_angle(),
                '&' => self.entity(),
                '\n' => self.line_ending(),
                '*' | '_' => self.emphasis_run(ch as u8),
                '~' if self.ext.contains(Extension::Subscript)
                    || self.ext.contains(Extension::Strikeout) =>
                {
                    self.emphasis_run(b'~');
                }
                '^' if self.ext.contains(Extension::Superscript) => self.emphasis_run(b'^'),
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
                    self.pos += 1;
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

    fn backslash(&mut self) {
        self.pos += 1;
        match self.peek() {
            Some('\n') => {
                self.pos += 1;
                while matches!(self.peek(), Some(' ' | '\t')) {
                    self.pos += 1;
                }
                self.nodes.push(Node::LineBreak);
            }
            Some(ch) if is_ascii_punctuation(ch) => {
                self.pos += 1;
                self.push_text(ch);
            }
            _ => self.push_text('\\'),
        }
    }

    fn code_span(&mut self) {
        let start = self.pos;
        let mut open = 0;
        while self.peek() == Some('`') {
            self.pos += 1;
            open += 1;
        }
        // Find a closing run of exactly `open` backticks.
        let mut scan = self.pos;
        while scan < self.chars.len() {
            if self.chars.get(scan).copied() == Some('`') {
                let mut close = 0;
                while self.chars.get(scan + close).copied() == Some('`') {
                    close += 1;
                }
                if close == open {
                    let content: String = self
                        .chars
                        .get(self.pos..scan)
                        .map(|s| s.iter().collect())
                        .unwrap_or_default();
                    self.pos = scan + close;
                    let attr = self.take_code_attr();
                    self.nodes
                        .push(Node::Inline(Inline::Code(attr, normalize_code(&content))));
                    return;
                }
                scan += close;
            } else {
                scan += 1;
            }
        }
        // No closing run: emit the opening backticks literally.
        let literal: String = self
            .chars
            .get(start..self.pos)
            .map(|s| s.iter().collect())
            .unwrap_or_default();
        self.push_str(&literal);
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
            if let Some((content, next)) = self.scan_display_math() {
                self.pos = next;
                self.nodes
                    .push(Node::Inline(Inline::Math(MathType::DisplayMath, content)));
                return;
            }
        } else if let Some((content, next)) = self.scan_inline_math() {
            self.pos = next;
            self.nodes
                .push(Node::Inline(Inline::Math(MathType::InlineMath, content)));
            return;
        }
        self.pos += 1;
        self.push_text('$');
    }

    /// Scan inline math starting at the opening `$`. Returns the content and the index past the
    /// closing `$`, or `None` if no valid `$…$` begins here.
    fn scan_inline_math(&self) -> Option<(String, usize)> {
        if self.at(1).is_none_or(is_unicode_whitespace) {
            return None;
        }
        let content_start = self.pos + 1;
        let mut i = content_start;
        while let Some(&ch) = self.chars.get(i) {
            if ch == '\\' && self.chars.get(i + 1).is_some() {
                i += 2;
                continue;
            }
            if ch == '$' {
                let prev_space = self.chars.get(i - 1).copied().is_none_or(is_unicode_whitespace);
                let next_digit = self.chars.get(i + 1).is_some_and(char::is_ascii_digit);
                if prev_space || next_digit {
                    return None;
                }
                let content: String = self.chars.get(content_start..i)?.iter().collect();
                return Some((content, i + 1));
            }
            i += 1;
        }
        None
    }

    /// Scan display math starting at the opening `$$`. Returns the content and the index past the
    /// closing `$$`, or `None` if no closing `$$` follows.
    fn scan_display_math(&self) -> Option<(String, usize)> {
        let content_start = self.pos + 2;
        let mut i = content_start;
        while self.chars.get(i).is_some() {
            if self.chars.get(i) == Some(&'$') && self.chars.get(i + 1) == Some(&'$') {
                let content: String = self.chars.get(content_start..i)?.iter().collect();
                return Some((content, i + 2));
            }
            i += 1;
        }
        None
    }

    fn left_angle(&mut self) {
        if let Some((inline, next)) = scan_autolink(self.chars, self.pos) {
            self.pos = next;
            self.nodes.push(Node::Inline(inline));
            return;
        }
        if let Some((html, next)) = scan_html_tag(self.chars, self.pos) {
            self.pos = next;
            self.nodes.push(Node::Inline(Inline::RawInline(
                carta_ast::Format("html".to_owned()),
                html,
            )));
            return;
        }
        self.pos += 1;
        self.push_text('<');
    }

    fn entity(&mut self) {
        if let Some((decoded, next)) = scan_entity(self.chars, self.pos) {
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
        let count = self.pos - start;
        let before = if start == 0 {
            None
        } else {
            self.chars.get(start - 1).copied()
        };
        let after = self.peek();
        let (can_open, can_close) = run_flanking(ch, before, after);
        self.nodes.push(Node::Delimiter(Delimiter {
            ch,
            count,
            can_open,
            can_close,
            image: false,
            text_start: self.pos,
            active: false,
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

        // A shortcut reference: the bracket's own text names the definition.
        if is_active {
            let key = normalize_label(&self.raw_label(opener_index));
            if let Some(target) = self.refs.get(&key).map(def_target) {
                self.finish_link(opener_index, is_image, target, self.pos);
                return;
            }
        }

        // Otherwise the opener reverts to its literal `[` / `![`, and `]` stays literal.
        self.bracket_stack.pop();
        self.literalize_bracket(opener_index);
        self.push_text(']');
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
        let (mut merged, mut next) = attr::parse_attributes_chars(self.chars, self.pos)?;
        while let Some((more, after)) = attr::parse_attributes_chars(self.chars, next) {
            attr::merge(&mut merged, more);
            next = after;
        }
        attr::is_non_empty(&merged).then_some((merged, next))
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
        if (self.ext.contains(Extension::LinkAttributes) || self.ext.contains(Extension::Attributes))
            && let Some((parsed, next)) = self.scan_attr_block()
        {
            self.pos = next;
            return parsed;
        }
        Attr::default()
    }

    /// Build a span from a non-image bracket opener and its inner content.
    fn build_span(&mut self, opener_index: usize, attr: Attr) {
        let mut inner: Vec<Node> = self.nodes.split_off(opener_index + 1);
        self.nodes.pop(); // remove the opener delimiter
        self.bracket_stack.retain(|&ni| ni < opener_index);
        process_emphasis(&mut inner, 0, self.ext);
        let content = collapse(inner);
        self.nodes.push(Node::Inline(Inline::Span(attr, content)));
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
        if self.at(0) == Some('(')
            && let Some((target, next)) = scan_inline_target(self.chars, self.pos)
        {
            return Explicit::Target(target, next);
        }
        // Explicit reference. Labels match on their raw source text (the closing `]` sits at `pos - 1`).
        if let Some((label, next)) = scan_following_label(self.chars, self.pos) {
            let key = if label.is_empty() {
                normalize_label(&self.raw_label(opener_index))
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
    fn raw_label(&self, opener_index: usize) -> String {
        let start = match self.nodes.get(opener_index) {
            Some(Node::Delimiter(d)) => d.text_start,
            _ => return String::new(),
        };
        self.chars
            .get(start..self.pos.saturating_sub(1))
            .map(|s| s.iter().collect())
            .unwrap_or_default()
    }

    /// If the bracket opener encloses a defined footnote reference (`[^label]`), emit the note and
    /// return `true`. The opener's raw label must begin with `^` and name a known footnote; the
    /// brackets and their content are then replaced wholesale, and an image opener's `!` survives as
    /// literal text. Inside a footnote definition's own body a reference collapses to an empty string
    /// rather than nesting a note. Returns `false` (leaving the brackets for other resolution) when
    /// the label has no `^` prefix, holds a bracket, or matches no definition.
    fn try_footnote(&mut self, opener_index: usize, is_image: bool) -> bool {
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
            Inline::Str(String::new())
        } else {
            Inline::Note(self.notes.by_id.get(&key).cloned().unwrap_or_default())
        };
        self.nodes.push(Node::Inline(note));
        true
    }

    fn build_link(&mut self, opener_index: usize, is_image: bool, target: Target, attr: Attr) {
        let mut inner: Vec<Node> = self.nodes.split_off(opener_index + 1);
        self.nodes.pop(); // remove the opener delimiter
        // Any bracket stack entries that pointed into the split-off range are now part of the
        // inner node list passed to process_emphasis; they no longer belong to the outer parse.
        self.bracket_stack.retain(|&ni| ni < opener_index);
        process_emphasis(&mut inner, 0, self.ext);
        let content = collapse(inner);
        let inline = if is_image {
            Inline::Image(attr, content, target)
        } else {
            Inline::Link(attr, content, target)
        };
        self.nodes.push(Node::Inline(inline));
    }
}

fn def_target(def: &LinkDef) -> Target {
    Target {
        url: def.url.clone(),
        title: def.title.clone(),
    }
}

/// A record in the delimiter list used by [`process_emphasis`].
#[derive(Debug, Clone)]
struct DelimEntry {
    /// Index into `nodes` where this delimiter lives.
    node_index: usize,
    ch: u8,
    count: usize,
    can_open: bool,
    can_close: bool,
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
fn process_emphasis(nodes: &mut Vec<Node>, stack_bottom: usize, ext: Extensions) {
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
            }),
            _ => None,
        })
        .collect();

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

    let mut current = 0usize; // index into `delims`, advances only forward

    while current < delims.len() {
        let Some(current_entry) = delims.get(current) else {
            break;
        };
        let (closer_ch, closer_count, closer_can_open, closer_can_close) = (
            current_entry.ch,
            current_entry.count,
            current_entry.can_open,
            current_entry.can_close,
        );
        let closer_ni = current_entry.node_index;
        if !closer_can_close {
            current += 1;
            continue;
        }

        let bucket = (
            closer_ch,
            closer_count % 3,
            closer_can_open,
            closer_count >= 2,
        );
        let bottom = *openers_bottom.get(&bucket).unwrap_or(&0);

        // Scan backward from just before `current` down to `bottom` for a matching opener.
        let mut found: Option<usize> = None; // delimiter-list index of the matched opener
        let mut scan = current;
        while scan > bottom {
            scan -= 1;
            let Some(entry) = delims.get(scan) else {
                break;
            };
            if !entry.can_open || entry.ch != closer_ch {
                continue;
            }
            // Rule of 3 and match_use_count check — we need a temporary Delimiter value to
            // reuse `emphasis_match`, which borrows `nodes` by index.
            let use_count = match_use_count(entry.count, closer_count, closer_ch, ext);
            if use_count.is_none() {
                // `match_use_count` rejected this opener; keep scanning — do not advance
                // `openers_bottom` for this slot just because one opener was rejected.
                continue;
            }
            // Re-derive the Delimiter from `nodes` for the rule-of-3 check.
            let ni = entry.node_index;
            let rule_ok = match nodes.get(ni) {
                Some(Node::Delimiter(d)) => emphasis_match(d, nodes, closer_ni),
                _ => false,
            };
            if rule_ok {
                found = Some(scan);
                break;
            }
        }

        let Some(opener_di) = found else {
            // No opener found: advance openers_bottom to exclude this closer's position in future
            // searches for the same bucket.
            openers_bottom.insert(bucket, current);
            // A delimiter that can't open is now known to be inert as a closer too.
            if !closer_can_open {
                // convert_delimiter_to_text replaces the node variant in-place; no index shift.
                convert_delimiter_to_text(nodes, closer_ni);
            }
            current += 1;
            continue;
        };

        // --- Match found: splice nodes and update the delimiter list ---

        let Some(opener_entry) = delims.get(opener_di) else {
            break;
        };
        let (opener_ni, opener_count) = (opener_entry.node_index, opener_entry.count);

        // Retrieve use_count (already validated above).
        let use_count = match_use_count(opener_count, closer_count, closer_ch, ext).unwrap_or(1);

        // Drain all nodes strictly between opener and closer into `content`, collapse, and wrap.
        let inner: Vec<Node> = nodes.drain(opener_ni + 1..closer_ni).collect();
        let content = collapse(inner);
        let wrapped = wrap_emphasis(closer_ch, use_count, content);
        // Insert the wrapped inline where the inner content was.
        nodes.insert(opener_ni + 1, Node::Inline(wrapped));

        // After drain(opener_ni+1..closer_ni) and insert(opener_ni+1), the closer node is at
        // opener_ni + 2. We then conditionally remove the closer and opener delimiter nodes,
        // which shifts remaining node_index values. Track all of that in one place.
        let new_closer_ni = opener_ni + 2;

        // Decrement delimiter counts (closer first; it's at the higher index).
        decrement_delimiter(nodes, new_closer_ni, use_count);
        decrement_delimiter(nodes, opener_ni, use_count);

        // Reflect decrements back into `delims`.
        let new_closer_count = closer_count.saturating_sub(use_count);
        let new_opener_count = opener_count.saturating_sub(use_count);
        if let Some(e) = delims.get_mut(current) {
            e.count = new_closer_count;
        }
        if let Some(e) = delims.get_mut(opener_di) {
            e.count = new_opener_count;
        }

        // Drop emptied delimiter nodes from `nodes`, highest index first so lower indices hold.
        let closer_empty = new_closer_count == 0;
        let opener_empty = new_opener_count == 0;
        if closer_empty {
            nodes.remove(new_closer_ni);
        }
        if opener_empty {
            nodes.remove(opener_ni);
        }

        // Compute the total shift experienced by node indices that were strictly above closer_ni
        // in the original node vector, after all four operations (drain, insert, remove×0/1/2):
        //
        //   drain(opener_ni+1..closer_ni): removes (closer_ni - opener_ni - 1) nodes above opener_ni
        //   insert at opener_ni+1: adds 1 node above opener_ni
        //   remove(new_closer_ni) if closer_empty: removes 1 node that was at new_closer_ni
        //   remove(opener_ni) if opener_empty: removes 1 node at opener_ni (below closer)
        //
        // For a node_index N > closer_ni (i.e., above the old closer):
        //   after drain+insert: new pos = N + opener_ni - closer_ni + 2
        //   after remove(closer) if empty: -1
        //   after remove(opener) if empty: -1
        // Total shift = (opener_ni - closer_ni + 2) - closer_empty - opener_empty.
        let above_shift = 2_isize + (opener_ni.cast_signed() - closer_ni.cast_signed())
            - isize::from(closer_empty)
            - isize::from(opener_empty);

        // The surviving closer's final node_index (only relevant when !closer_empty):
        //   after drain+insert it's at new_closer_ni = opener_ni+2;
        //   after remove(opener) if empty: it shifts to opener_ni+1.
        let final_closer_ni = opener_ni + 1 + usize::from(!opener_empty);

        // Update the delimiter list:
        // Step A: remove inner delimiter entries (consumed into the wrapped span).
        delims.drain(opener_di + 1..current);
        // After this drain, the old `current` entry is now at opener_di + 1.
        let current_di_after = opener_di + 1;

        // Step B: remove the closer and opener entries from `delims` if they are now empty.
        // Closer is at current_di_after; remove it first (higher index).
        if closer_empty {
            delims.remove(current_di_after);
        }
        if opener_empty {
            delims.remove(opener_di);
        }

        // Step C: update node_index for all surviving entries.
        //
        // After Steps A and B, `delims` contains no entries for the now-wrapped inner span.
        // The surviving delimiter entries fall into three groups:
        //   1. Entries at or before opener_di with node_index <= opener_ni: unchanged.
        //   2. The surviving opener (if !opener_empty) at delimiter index opener_di,
        //      node_index = opener_ni: already correct.
        //   3. The surviving closer (if !closer_empty): node_index must be final_closer_ni.
        //   4. Entries after the match with node_index > closer_ni: shift by above_shift.
        //
        // Determine where the "entries after the match" start in the updated delimiter list.
        let first_after_di = match (opener_empty, closer_empty) {
            (true, true) => opener_di,
            (false, true) => opener_di + 1, // opener at opener_di; nothing else in the region
            (true, false) => {
                // closer survived at opener_di; update its node_index.
                if let Some(e) = delims.get_mut(opener_di) {
                    e.node_index = final_closer_ni;
                }
                opener_di + 1
            }
            (false, false) => {
                // opener at opener_di; closer at opener_di + 1; update closer's node_index.
                if let Some(e) = delims.get_mut(opener_di + 1) {
                    e.node_index = final_closer_ni;
                }
                opener_di + 2
            }
        };

        // Apply the total shift to all entries that come after the match region.
        if above_shift != 0 {
            for entry in delims.get_mut(first_after_di..).into_iter().flatten() {
                entry.node_index =
                    usize::try_from(entry.node_index.cast_signed() + above_shift).unwrap_or(0);
            }
        }

        // Adjust `openers_bottom` for the delimiter-list compaction that just happened.
        //
        // After `delims.drain(opener_di+1..current)` + conditional removes:
        //   - Values <= opener_di: unchanged.
        //   - Values in (opener_di, current): pointed into the now-removed inner span → clamp to
        //     opener_di (those openers no longer exist in the list).
        //   - Values >= current: shifted down by (current - opener_di - 1) for the drain, then
        //     by -1 for each removed endpoint (closer and/or opener).
        let inner_drain = current - opener_di - 1;
        let endpoint_removes = usize::from(closer_empty) + usize::from(opener_empty);
        for v in openers_bottom.values_mut() {
            if *v > opener_di && *v < current {
                *v = opener_di;
            } else if *v >= current {
                *v = v.saturating_sub(inner_drain + endpoint_removes);
            }
        }

        // Resume from opener_di: the surviving closer (if any) may still match further openers.
        current = opener_di;
    }

    // Any leftover delimiters become literal text.
    for entry in &delims {
        convert_delimiter_to_text(nodes, entry.node_index);
    }
}

/// Whether `ch` names a delimiter run resolved by [`process_emphasis`].
fn is_delimiter_char(ch: u8) -> bool {
    matches!(ch, b'*' | b'_' | b'~' | b'^' | b'\'' | b'"')
}

/// Whether `ch` is a smart-quote delimiter (`'` or `"`).
fn is_quote(ch: u8) -> bool {
    matches!(ch, b'\'' | b'"')
}

/// Open/close eligibility for a delimiter run, dispatching to the smart-quote rule for `'`/`"` and
/// to the emphasis rule for everything else.
fn run_flanking(ch: u8, before: Option<char>, after: Option<char>) -> (bool, bool) {
    if is_quote(ch) {
        quote_flanking(ch, before, after)
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

/// Fold a run of `len` hyphens (`len >= 2`) into the fewest em (`—`) and en (`–`) dashes that sum to
/// its length: a multiple of three is all em dashes, an even length is all en dashes, and an odd
/// length that is not a multiple of three takes one or two en dashes — whichever leaves a multiple of
/// three — with the rest em dashes.
fn fold_dash_run(len: usize) -> String {
    let (em, en) = if len.is_multiple_of(3) {
        (len / 3, 0)
    } else if len.is_multiple_of(2) {
        (0, len / 2)
    } else {
        let en = if len % 3 == 1 { 2 } else { 1 };
        ((len - 2 * en) / 3, en)
    };
    let mut out = String::with_capacity((em + en) * 3);
    out.extend(std::iter::repeat_n('\u{2014}', em));
    out.extend(std::iter::repeat_n('\u{2013}', en));
    out
}

/// Fold a run of `len` dots into one ellipsis (`…`) per group of three, leaving the remaining one or
/// two dots literal.
fn fold_ellipsis_run(len: usize) -> String {
    let mut out = String::with_capacity(len / 3 * 3 + len % 3);
    out.extend(std::iter::repeat_n('\u{2026}', len / 3));
    out.extend(std::iter::repeat_n('.', len % 3));
    out
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
        }
    }
    flush(&mut text, &mut out);
    out
}

/// Split a text run into `Str` tokens separated by `Space` inlines, collapsing each run of
/// spaces to a single `Space`.
fn push_text_inlines(out: &mut Vec<Inline>, text: &str) {
    let mut chars = text.chars().peekable();
    let mut word = String::new();
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

fn is_unicode_whitespace(ch: char) -> bool {
    ch == ' '
        || ch == '\t'
        || ch == '\n'
        || ch == '\u{0c}'
        || ch == '\u{0b}'
        || ch == '\r'
        || ch.is_whitespace()
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
fn normalize_code(content: &str) -> String {
    let collapsed: String = content
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
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
        TASK_CHECKED, TASK_UNCHECKED, delimiter_literal, flanking, fold_dash_run, fold_ellipsis_run,
        match_use_count, quote_flanking, split_header_attr, task_marker_replacement,
    };
    use carta_core::{Extension, Extensions};

    fn exts(list: &[Extension]) -> Extensions {
        Extensions::from_list(list)
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
            let width: usize = folded
                .chars()
                .map(|c| if c == em { 3 } else { 2 })
                .sum();
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
}

#[cfg(test)]
mod inline_tests {
    use std::collections::{BTreeMap, BTreeSet};

    use carta_ast::{Attr, Block, Inline, Target};

    use super::{LinkDef, Notes, RefMap, parse_inlines};
    use carta_core::{Extension, Extensions};

    static NO_DEFINED: BTreeSet<String> = BTreeSet::new();
    static NO_BY_ID: BTreeMap<String, Vec<Block>> = BTreeMap::new();

    /// A footnote context with no definitions, for tests that exercise non-footnote inline syntax.
    fn no_notes() -> Notes<'static> {
        Notes {
            defined: &NO_DEFINED,
            by_id: &NO_BY_ID,
            in_definition: false,
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

    fn str(s: &str) -> Inline {
        Inline::Str(s.to_owned())
    }

    fn link(content: Vec<Inline>, url: &str) -> Inline {
        Inline::Link(
            Attr::default(),
            content,
            Target {
                url: url.to_owned(),
                title: String::new(),
            },
        )
    }

    fn image(alt: Vec<Inline>, url: &str) -> Inline {
        Inline::Image(
            Attr::default(),
            alt,
            Target {
                url: url.to_owned(),
                title: String::new(),
            },
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
        Inline::Math(carta_ast::MathType::InlineMath, content.to_owned())
    }

    fn math_display(content: &str) -> Inline {
        Inline::Math(carta_ast::MathType::DisplayMath, content.to_owned())
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
            vec![str("$5"), Inline::Space, str("and"), Inline::Space, str("$10")]
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
        Inline::Span(attr, content)
    }

    fn attr(id: &str, classes: &[&str], kv: &[(&str, &str)]) -> Attr {
        Attr {
            id: id.to_owned(),
            classes: classes.iter().map(|c| (*c).to_owned()).collect(),
            attributes: kv
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
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
            pe("[x]{#one .a}{#two .b k=v}", exts(&[Extension::BracketedSpans])),
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
            vec![Inline::Code(attr("x", &["rust"], &[]), "code".to_owned())]
        );
        // A space before the block leaves it unattached (no wrapper artifact is produced).
        assert_eq!(
            pe("`code` x", attrs()),
            vec![Inline::Code(Attr::default(), "code".to_owned()), Inline::Space, str("x")]
        );
    }

    #[test]
    fn link_and_image_take_attributes() {
        let link_with_attr = Inline::Link(
            attr("home", &["external"], &[]),
            vec![str("t")],
            Target { url: "u".to_owned(), title: String::new() },
        );
        assert_eq!(pe("[t](u){.external #home}", attrs()), vec![link_with_attr]);
        let image_with_attr = Inline::Image(
            attr("", &[], &[("width", "200")]),
            vec![str("a")],
            Target { url: "i".to_owned(), title: String::new() },
        );
        assert_eq!(pe("![a](i){width=200}", attrs()), vec![image_with_attr]);
    }

    #[test]
    fn attributes_require_the_extension() {
        // Without any attribute extension the block stays literal text.
        assert_eq!(
            p("[text]{.cls}"),
            vec![str("[text]{.cls}")]
        );
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
}

