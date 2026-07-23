//! Block-structure phase: consume input line by line into a tree of [`IrBlock`]s, following the
//! `CommonMark` spec's open-block algorithm (spec appendix "A parsing strategy"). Leaf text is left
//! raw for the inline phase; link reference definitions are stripped from paragraph fronts and
//! collected into the [`RefMap`].

use carta_ast::{Attr, Format, ListAttributes, ListNumberDelim, ListNumberStyle};
use carta_core::{Extension, Extensions};

use super::cursor::{Cursor, FenceInfo, ListMarkerParse};
use super::postprocess::{
    Continue, alert_marker_type, attach_table_captions, continues_ordered, demote_lone_roman,
    demote_loose_paragraphs, div_close_fence, div_open_fence, fence_attr, is_line_block_marker,
    is_math_environment, is_thematic_dash_line, last_entry_is_empty, last_nonempty_line,
    line_block_lines, list_info, next_example_number, owned_lines, raw_block_format,
    raw_tex_env_name, raw_tex_scan, single_line, split_table_lines, strip_atx_closing,
    strip_one_trailing_newline, strip_trailing_blank_lines, tighten_last_block,
};
use super::{
    ExampleMap, FootnoteDefs, IrBlock, IrDefItem, RefMap, TAB_STOP, grid, html_block, html_element,
    scan, table, texttable,
};

/// Parse the normalized input into the block tree plus the collected link, footnote, and example
/// references.
pub(crate) fn parse(
    input: &str,
    extensions: Extensions,
    greedy_paragraphs: bool,
) -> (Vec<IrBlock>, RefMap, FootnoteDefs, ExampleMap) {
    let mut parser = Parser::new(extensions, greedy_paragraphs);
    let lines = split_lines(input);
    if greedy_paragraphs {
        parser.fence_close_candidates = build_fence_close_candidates(&lines);
    }
    for index in 0..lines.len() {
        let line = lines.get(index).copied().unwrap_or("");
        let following = lines.get(index + 1..).unwrap_or(&[]);
        parser.process_line(line, following, Some(index));
    }
    parser.finalize_open_text_tables();
    parser.finish()
}

/// Split into lines on `\n`, dropping a single trailing empty line produced by a final newline.
fn split_lines(input: &str) -> Vec<&str> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = input.split('\n').collect();
    if input.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Document-level index of lines that could close a code fence, one sorted list per marker kind.
/// Each entry is `(line index, run length)` for a line that — read at the document root — satisfies
/// the closing-fence test (`indent <= 3` and a run of at least three markers followed only by
/// whitespace). Consulting this instead of rescanning every remaining line keeps unclosed-fence
/// spam from costing O(n²) in the greedy (markdown-dialect) paragraph mode.
#[derive(Default)]
struct FenceCloseCandidates {
    backtick: Vec<(usize, usize)>,
    tilde: Vec<(usize, usize)>,
}

impl FenceCloseCandidates {
    /// Whether some candidate after `after` closes a fence of `marker` and at least `length` markers.
    /// The candidates are recorded in ascending line order, so a binary search skips the openers that
    /// precede this one; the run-length filter mirrors [`Cursor::is_closing_fence`]'s length rule.
    fn reaches_close(&self, marker: u8, length: usize, after: usize) -> bool {
        let candidates = match marker {
            b'`' => &self.backtick,
            b'~' => &self.tilde,
            _ => return false,
        };
        let start = candidates.partition_point(|&(index, _)| index <= after);
        candidates
            .get(start..)
            .is_some_and(|rest| rest.iter().any(|&(_, run)| run >= length))
    }
}

fn build_fence_close_candidates(lines: &[&str]) -> FenceCloseCandidates {
    let mut candidates = FenceCloseCandidates::default();
    for (index, &line) in lines.iter().enumerate() {
        for (marker, bucket) in [
            (b'`', &mut candidates.backtick),
            (b'~', &mut candidates.tilde),
        ] {
            let cursor = Cursor::new(line);
            if cursor.indent() <= 3
                && let Some(run) = leading_fence_run(line, marker)
                && cursor.is_closing_fence(marker, run)
            {
                bucket.push((index, run));
            }
        }
    }
    candidates
}

/// Length of the run of `marker` bytes that begins `line` after up to any number of leading spaces —
/// counted exactly as [`Cursor::is_closing_fence`] does — when it is at least three; otherwise `None`.
fn leading_fence_run(line: &str, marker: u8) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while bytes.get(index).copied() == Some(b' ') {
        index += 1;
    }
    let mut run = 0;
    while bytes.get(index).copied() == Some(marker) {
        run += 1;
        index += 1;
    }
    (run >= 3).then_some(run)
}

#[derive(Debug, Clone)]
enum Kind {
    Document,
    BlockQuote,
    List(ListInfo),
    Item(ItemInfo),
    /// A footnote definition, holding its normalized label. Its content is gathered like a list
    /// item (a four-column continuation indent) and collected out of the body by [`Parser::finish`].
    FootnoteDef(String),
    /// A fenced div (see [`DivInfo`]). Content is parsed recursively; a bare colon-run line of at
    /// least the opening length closes it.
    FencedDiv(DivInfo),
    Paragraph,
    /// A line block: each source line is appended raw (a `|`-led line opens a new entry, an
    /// indented line continues the previous). The lines are split apart and prepared in
    /// [`Parser::build_block`].
    LineBlock,
    /// A definition list, holding one [`Kind::DefinitionItem`] per term. A transparent container:
    /// it consumes nothing on continuation and defers to its items.
    DefinitionList,
    /// One term of a definition list: its raw term text and whether the entry is tight (its
    /// definition paragraphs render as `Plain` rather than `Para`). A transparent container holding
    /// one [`Kind::Definition`] per `:`/`~` marker.
    DefinitionItem {
        term: String,
        tight: bool,
    },
    /// One definition body of a definition list. Its content continues under a four-column indent
    /// like a list item; `indent` is the column its content begins at.
    Definition {
        indent: usize,
    },
    Heading(i32),
    IndentedCode,
    FencedCode(FenceInfo),
    HtmlBlock(u8),
    /// A raw HTML block in the Markdown family when inner content is not parsed (neither
    /// `native_divs` nor `markdown_in_html_blocks` applies). A block-level open tag at the left
    /// margin begins a span that accumulates lines verbatim — nested same-name tags counted, blank
    /// lines kept — until a close tag brings `depth` back to zero; the whole span is one `RawBlock`.
    /// A self-closing, void, or bare close tag opens a span already at `depth` zero: a single line.
    RawHtmlSpan {
        tag: String,
        depth: usize,
    },
    /// A block-level HTML element whose inner content is parsed as markdown. A `<div>` (with
    /// `native_divs` enabled) becomes an [`IrBlock::Div`] carrying the tag's attributes; any other
    /// recognized block tag (with `markdown_in_html_blocks` enabled) keeps its open and close tags as
    /// raw HTML with the parsed content between them. The element is a transparent container: nested
    /// same-name elements nest as their own containers, so tag balancing falls out of the tree.
    HtmlElement(HtmlElementInfo),
    /// A raw TeX environment opened by `\begin{NAME}` at a line start. It accumulates lines verbatim
    /// until a matching `\end{NAME}` brings the nesting `depth` back to zero, then becomes a
    /// `RawBlock` for the `tex` format. `name` is the literal brace content of the opener, compared
    /// exactly. Math environments (`equation`, `align`, …) are excluded — they stay inline.
    RawTex {
        name: String,
        depth: usize,
    },
    ThematicBreak,
    /// A dash-ruled table candidate, accumulating its physical lines (each `\n`-terminated). Its
    /// exact extent is settled when the block closes: the lines are parsed into a table, with any
    /// surplus rows after the table re-fed as following blocks, or — when they form no table — the
    /// leaf is repurposed into the thematic break or paragraph the lines actually are.
    TextTable,
}

#[derive(Debug, Clone)]
pub(super) struct ListInfo {
    pub(super) bullet: bool,
    pub(super) marker: u8,
    pub(super) style: ListNumberStyle,
    pub(super) delim: ListNumberDelim,
    pub(super) start: i32,
}

#[derive(Debug, Clone)]
struct ItemInfo {
    /// Column indent required for a continuation line to belong to this item.
    indent: usize,
    /// For an example-list item, its `@label` (or `None` for the anonymous `@`); `None` for every
    /// other item. The label resolves later `@label` references to this item's number.
    example_label: Option<String>,
}

#[derive(Debug, Clone)]
struct HtmlElementInfo {
    /// The lowercased tag name (e.g. `div`, `section`), used to recognize the matching close tag.
    tag: String,
    /// Attributes parsed from the open tag; only meaningful when `as_div` holds.
    attr: Attr,
    /// The open tag's raw text, kept verbatim for the leading raw block when not rendered as a div.
    raw_open: String,
    /// The matching close tag's raw text, kept verbatim for the trailing raw block when not a div.
    /// Empty until the element is closed (or it closed implicitly at end of input).
    raw_close: String,
    /// When set, the element renders as an [`IrBlock::Div`]; otherwise the open/close tags are kept
    /// as raw HTML around the parsed content.
    as_div: bool,
    /// Whether the element's final content block tightens from `Para` to `Plain` (set when no blank
    /// line separates that content from the close tag).
    tighten_last: bool,
}

#[derive(Debug, Clone)]
struct DivInfo {
    /// The opening fence's colon-run length; a closing fence must be at least this long.
    fence: usize,
    /// The column the opening fence began at. Continuation lines re-base to this column, so the
    /// indentation of inner blocks is measured relative to it rather than the line start.
    indent: usize,
    attr: Attr,
}

/// Outcome of the phase-1 container descent for one line.
struct Descent {
    /// The deepest container that matched the line's continuation markers.
    container: usize,
    /// Whether every open container matched (a `false` marks where lazy continuation and block
    /// closing apply).
    all_matched: bool,
    /// The fenced divs matched on the descent, innermost last, each with the line as it stood at
    /// that div's indentation.
    div_path: Vec<(usize, String)>,
    /// Set when an open leaf consumed the whole line, so no further processing is needed.
    consumed: bool,
}

/// The deepest a container (block quote, list item, fenced div, …) may nest. Past this the opener is
/// left as ordinary text rather than descended into, so a pathologically nested input — thousands of
/// `>` on one line, say — cannot build a tree deep enough for the recursive block/inline tree-walks
/// to overflow the stack. Set well above any nesting a genuine document reaches, yet low enough that
/// the walks stay within the smallest stack the reader runs on — a 1 MiB Windows main thread, or the
/// sanitizer build the fuzzer uses, whose deeper per-frame cost still clears this ceiling comfortably.
const MAX_CONTAINER_DEPTH: usize = 128;

// The bool fields are independent per-node facts (open state, plain rendering, blank-follow,
// pipe-table), not a combination where some pairings are invalid.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
struct Node {
    kind: Kind,
    open: bool,
    parent: usize,
    children: Vec<usize>,
    text: String,
    /// The visual column this block's first line began at, before its leading indentation was
    /// consumed. Recorded for paragraphs so a dash ruling on the next line can read a headed
    /// table's column alignment against the header's true position; zero for every other block.
    indent: usize,
    /// Render this paragraph as `Plain` rather than `Para`. Set when a block-level HTML element
    /// interrupts the paragraph with no blank line between, so the interrupted paragraph reads tight.
    as_plain: bool,
    /// Whether the line that followed this block (while it was the deepest open block) was blank.
    /// Drives loose-vs-tight list classification.
    last_line_blank: bool,
    /// Set once this paragraph's header and delimiter rows have validated as a pipe table, so each
    /// following body line is classified against the new line alone rather than re-verifying the
    /// header and delimiter (which never change) every time.
    pipe_table: bool,
    /// This node's nesting depth in the block tree: the document root is `0` and every child is one
    /// deeper than its parent. Read to cap how deeply containers may nest, so a pathologically nested
    /// input cannot build a tree the recursive tree-walks would overflow the stack descending. Set
    /// once, when the node is linked in by [`Parser::append_child`].
    depth: usize,
}

impl Node {
    fn new(kind: Kind) -> Self {
        Node {
            kind,
            open: true,
            parent: 0,
            children: Vec::new(),
            text: String::new(),
            indent: 0,
            as_plain: false,
            last_line_blank: false,
            pipe_table: false,
            depth: 0,
        }
    }
}

/// How an open paragraph absorbs the block openers on the next line (see [`Parser::greedy_gates`]).
// Four independent fold decisions, each governing a distinct opener — they are flags by nature.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Default)]
struct GreedyGates {
    /// The foldable openers — block quote, heading, thematic break, fenced div, footnote definition
    /// — continue the paragraph as a lazy line rather than interrupting it.
    foldable: bool,
    /// A list marker that would *start* a fresh list folds; one continuing an open list still opens.
    list_start: bool,
    /// The block-quote fold, gated further on `blank_before_blockquote`.
    blockquote: bool,
    /// The heading fold, gated further on `blank_before_header`.
    heading: bool,
}

struct Parser {
    nodes: Vec<Node>,
    refs: RefMap,
    extensions: Extensions,
    /// When set, most block openers do not interrupt an open paragraph (see
    /// [`ReaderOptions::greedy_paragraphs`](carta_core::ReaderOptions::greedy_paragraphs)).
    greedy_paragraphs: bool,
    /// Set while a code fence that could not open a code block (its info names no language, or it
    /// has no closing fence) is folding into a paragraph. Until a matching closing fence or a blank
    /// line, each following line is absorbed as paragraph text with no block opener allowed to fire.
    fence_fold: Option<FenceInfo>,
    /// Precomputed close-fence positions for the whole document, consulted from the root container so
    /// a fence opener does not rescan every remaining line (built only in greedy-paragraph mode).
    fence_close_candidates: FenceCloseCandidates,
}

impl Parser {
    fn new(extensions: Extensions, greedy_paragraphs: bool) -> Self {
        Parser {
            nodes: vec![Node::new(Kind::Document)],
            refs: RefMap::new(),
            extensions,
            greedy_paragraphs,
            fence_fold: None,
            fence_close_candidates: FenceCloseCandidates::default(),
        }
    }

    fn kind(&self, index: usize) -> Option<&Kind> {
        self.nodes.get(index).map(|node| &node.kind)
    }

    fn last_open_child(&self, index: usize) -> Option<usize> {
        let node = self.nodes.get(index)?;
        let &last = node.children.last()?;
        match self.nodes.get(last) {
            Some(child) if child.open => Some(last),
            _ => None,
        }
    }

    fn close(&mut self, index: usize) {
        if let Some(node) = self.nodes.get_mut(index) {
            node.open = false;
        }
    }

    fn append_child(&mut self, parent: usize, mut node: Node) -> usize {
        let index = self.nodes.len();
        node.parent = parent;
        node.depth = self.nodes.get(parent).map_or(0, |p| p.depth + 1);
        self.nodes.push(node);
        if let Some(parent_node) = self.nodes.get_mut(parent) {
            parent_node.children.push(index);
        }
        index
    }

    fn parent(&self, index: usize) -> usize {
        self.nodes.get(index).map_or(0, |node| node.parent)
    }

    /// Whether a block of `kind` may be a direct child of `container`.
    fn can_contain(&self, container: usize, kind: &Kind) -> bool {
        match self.kind(container) {
            Some(
                Kind::Document
                | Kind::BlockQuote
                | Kind::Item(_)
                | Kind::FootnoteDef(_)
                | Kind::FencedDiv(..)
                | Kind::HtmlElement(_)
                | Kind::Definition { .. },
            ) => !matches!(kind, Kind::Item(_)),
            Some(Kind::List(_)) => matches!(kind, Kind::Item(_)),
            _ => false,
        }
    }

    /// Walk up from `container`, closing any block that cannot contain `kind` (e.g. a finished
    /// list when a non-item block follows), and return the nearest ancestor that can.
    fn place(&mut self, mut container: usize, kind: &Kind) -> usize {
        while !self.can_contain(container, kind) {
            self.close(container);
            let parent = self.parent(container);
            if parent == container {
                break;
            }
            container = parent;
        }
        container
    }

    /// Descend through the open containers, matching each one's continuation marker against the
    /// line. Leaves terminate the descent — they are handled by [`Parser::process_line`], not
    /// entered as containers.
    fn descend_containers(&mut self, cursor: &mut Cursor) -> Descent {
        let mut container = 0;
        let mut all_matched = true;
        // The fenced divs matched on this descent, innermost last, each paired with the line as it
        // stood at that div's own indentation (before it re-based its content). A close fence is
        // judged against this path: it is read at each div's level in turn, so one buried under a
        // div's re-base indent (or a nested item's deeper indent) reads as content instead.
        let mut div_path: Vec<(usize, String)> = Vec::new();
        loop {
            let Some(child) = self.last_open_child(container) else {
                break;
            };
            if !self.is_container(child) {
                break;
            }
            let div_fence_line =
                matches!(self.kind(child), Some(Kind::FencedDiv(..))).then(|| cursor.rest());
            match self.try_continue(child, cursor) {
                Continue::Matched => {
                    container = child;
                    if let Some(line) = div_fence_line {
                        div_path.push((child, line));
                    }
                }
                Continue::NotMatched => {
                    all_matched = false;
                    break;
                }
                Continue::MatchedLeaf => {
                    return Descent {
                        container,
                        all_matched,
                        div_path,
                        consumed: true,
                    };
                }
            }
        }
        Descent {
            container,
            all_matched,
            div_path,
            consumed: false,
        }
    }

    // The per-line state machine runs its container-matching and block-opening phases in sequence
    // over one shared cursor; splitting the phases into helpers would only thread that cursor through
    // extra signatures.
    #[allow(clippy::too_many_lines)]
    fn process_line(&mut self, line: &str, following: &[&str], line_index: Option<usize>) {
        let mut cursor = Cursor::new(line);

        // Phase 1: descend through open containers, matching each one's continuation marker.
        let Descent {
            mut container,
            all_matched,
            div_path,
            consumed,
        } = self.descend_containers(&mut cursor);
        if consumed {
            return;
        }

        // A bare colon-run line can close an open fenced div, popping everything nested inside it.
        if self.close_fenced_div(container, &div_path) {
            return;
        }

        // A matching close tag (`</tag>`) closes the innermost open HTML element and everything
        // nested under it. Any content before the tag on its line is the element's final content;
        // any content after is re-fed. The tag sits at the element's own level, so an inner block
        // (a list, a block quote) left unmatched on this line does not prevent the close.
        if self.close_html_element(container, &cursor) {
            return;
        }

        // An open raw TeX environment swallows this line verbatim until its matching `\end`.
        if self.continue_raw_tex(container, all_matched, &cursor) {
            return;
        }

        // An open code/html leaf (reachable only when every container matched) consumes the line.
        if all_matched
            && let Some(leaf) = self
                .last_open_child(container)
                .filter(|&c| !self.is_container(c))
            && matches!(
                self.kind(leaf),
                Some(
                    Kind::FencedCode(_)
                        | Kind::IndentedCode
                        | Kind::HtmlBlock(_)
                        | Kind::RawHtmlSpan { .. }
                )
            )
        {
            match self.try_continue(leaf, &mut cursor) {
                Continue::MatchedLeaf | Continue::Matched => return,
                Continue::NotMatched => self.close(leaf),
            }
        }

        let blank = cursor.is_blank();

        // A folding code fence (one that opened no code block) absorbs each following line into its
        // paragraph verbatim — no setext underline, table, or other opener may fire — until its
        // matching closing fence (kept as the last line) or a blank line ends the fold.
        if let Some(fence) = self.fence_fold.clone() {
            if all_matched
                && !blank
                && matches!(self.last_open_leaf_kind(container), Some(Kind::Paragraph))
            {
                if cursor.indent() <= 3 && cursor.is_closing_fence(fence.marker, fence.length) {
                    self.fence_fold = None;
                }
                self.add_line(container, false, false, &mut cursor);
                return;
            }
            self.fence_fold = None;
        }

        // A setext underline converts an open paragraph (directly under the matched container,
        // so fully matched) into a heading.
        if all_matched
            && !blank
            && let Some(para) = self
                .last_open_child(container)
                .filter(|&c| matches!(self.kind(c), Some(Kind::Paragraph)))
        {
            cursor.note_indent();
            if cursor.indent() < TAB_STOP
                && let Some(level) = cursor.setext_underline()
            {
                // Leading link reference definitions belong to neither the heading nor the
                // underline. Find the body that remains after them without registering them yet: the
                // underline forms a heading only over what remains.
                let text = self.node_text(para);
                let mut body = text.as_str();
                while let Some((_, _, rest)) =
                    scan::parse_link_reference_definition(body, self.greedy_paragraphs)
                {
                    body = rest;
                }
                // In the markdown family the underline heads a single-line paragraph only; a body of
                // two or more lines keeps the underline as ordinary continuation text instead.
                let multiline_body = self.greedy_paragraphs && body.trim_end().contains('\n');
                if !multiline_body {
                    let only_definitions = body.trim().is_empty();
                    // Pull the definitions out for real now, then head or consume what remains. If
                    // nothing remains, the paragraph was pure definitions — it is consumed and this
                    // line is reparsed as ordinary content (not an underline).
                    let column_zero = self.nodes.get(para).is_some_and(|node| node.indent == 0);
                    let remaining = self.extract_refs(&text, column_zero);
                    if let Some(node) = self.nodes.get_mut(para) {
                        node.text = remaining;
                    }
                    if only_definitions {
                        self.close(para);
                    } else {
                        self.convert_paragraph_to_heading(para, level);
                        return;
                    }
                }
            }
        }

        // An open multi-line construct — a grid table, a pipe table, or a line block — claims this
        // continuation line before the block openers below could read its leading `+`, `|`, `-`,
        // `>`, `#`, etc. as the start of a new block.
        if self.continue_open_construct(container, all_matched, blank, &cursor) {
            return;
        }

        // The deepest block open before this line opens any new ones. When a container went
        // unmatched in phase 1, this is the tip of the unmatched chain hanging below `matched`.
        let matched = container;
        let old_tip = self.deepest_open(matched);

        // Record the blank against the block it trails — this drives loose-list classification.
        // When every container matched, it lands on the deepest already-finalized block (or the
        // still-open container if it has no content yet), so a blank after an item's last block (or
        // after an empty item) marks the line blank even though the block was closed before this
        // line was read. When a container went unmatched, the blank ends that container at the
        // boundary: it attaches to the deepest matched container's last child, so a block quote (or
        // any block) that a blank line terminates still counts toward the enclosing list's
        // looseness.
        if blank {
            let target = if all_matched {
                self.blank_trails(old_tip)
            } else {
                self.nodes
                    .get(matched)
                    .and_then(|node| node.children.last().copied())
                    .unwrap_or(matched)
            };
            // A blank line that lands inside a still-open fenced div is internal to the div, not a
            // gap between the enclosing item's blocks, so it must not make that item's list loose.
            // (Once the div closes, a following blank trails it at the item's level and does count.)
            let internal_to_open_div = matches!(self.kind(target), Some(Kind::FencedDiv(..)))
                && self.nodes.get(target).is_some_and(|node| node.open);
            if !internal_to_open_div && let Some(node) = self.nodes.get_mut(target) {
                node.last_line_blank = true;
            }
        }

        // Phase 2: open new blocks at the matched container.
        let mut started_new = false;
        if !blank {
            loop {
                // Stop descending once the tree is absurdly deep: leave the rest of the line as text
                // rather than opening yet another container the recursive tree-walks would have to
                // recurse through. Only pathological input ever reaches this.
                if self
                    .nodes
                    .get(container)
                    .is_some_and(|n| n.depth >= MAX_CONTAINER_DEPTH)
                {
                    break;
                }
                cursor.note_indent();
                if let Some(opened) = self.try_open(container, &mut cursor, following, line_index) {
                    started_new = true;
                    container = opened;
                    // Descend into the new container to open the next block on this line — but only
                    // if real content remains. A container marker trailed by whitespace alone (e.g.
                    // a bare `-` with spaces after) leaves an empty container: the leftover spaces
                    // are not a non-blank line and must not open an indented code block.
                    if self.is_container(container) && !cursor.is_blank() {
                        continue;
                    }
                }
                break;
            }
        }

        // Spec "close unmatched blocks": a container left unmatched in phase 1 stays open only to
        // absorb a lazy paragraph continuation (non-blank text that opens no new block into an open
        // paragraph). Otherwise it closes before the matched container is reused, so e.g. a blank
        // line ends a block quote rather than letting the next `>` rejoin it.
        let lazy = !all_matched
            && !blank
            && !started_new
            && matches!(self.kind(old_tip), Some(Kind::Paragraph));
        if !all_matched && !lazy {
            self.close_chain(old_tip, matched);
        }

        self.add_line(container, started_new, blank, &mut cursor);
    }

    /// Try each open multi-line construct in turn, returning `true` as soon as one absorbs the line.
    /// Grid runs first: its `+`-ruled border is unambiguous and a pipe table never opens with one.
    fn continue_open_construct(
        &mut self,
        container: usize,
        all_matched: bool,
        blank: bool,
        cursor: &Cursor,
    ) -> bool {
        self.continue_grid_table(container, all_matched, blank, cursor)
            || self.continue_pipe_table(container, all_matched, blank, cursor)
            || self.continue_line_block(container, all_matched, cursor)
            || self.continue_text_table(container, all_matched, blank, cursor)
    }

    /// Let an in-progress grid table claim its `+`/`|` continuation lines before the block openers
    /// run. A paragraph whose first line is a grid top border is a candidate; each following grid
    /// line is absorbed into it. A non-grid line ends the candidate: if the lines so far already
    /// form a complete table the paragraph closes (and builds as a table) so the new line starts
    /// fresh, otherwise the paragraph stays open to take the line as a lazy continuation. Returns
    /// `true` when the line was absorbed.
    fn continue_grid_table(
        &mut self,
        container: usize,
        all_matched: bool,
        blank: bool,
        cursor: &Cursor,
    ) -> bool {
        if !self.extensions.contains(Extension::GridTables) || !all_matched {
            return false;
        }
        let Some(leaf) = self
            .last_open_child(container)
            .filter(|&c| matches!(self.kind(c), Some(Kind::Paragraph)))
        else {
            return false;
        };
        let Some(text) = self.node_text_ref(leaf) else {
            return false;
        };
        let first = text.split('\n').next().unwrap_or("");
        if !grid::is_top_border(first) || blank {
            return false;
        }
        let line = cursor.remaining();
        if grid::is_grid_line(line) {
            self.append_text(leaf, line.trim_start_matches(' '));
            self.append_text(leaf, "\n");
            return true;
        }
        if grid::parse(text).is_some() {
            self.close(leaf);
        }
        false
    }

    /// Let an in-progress pipe table claim a continuation line before the block openers run,
    /// returning `true` when the line was absorbed as a table row and needs no further handling. A
    /// row without a pipe ends the table: the open paragraph closes and the line is reparsed.
    fn continue_pipe_table(
        &mut self,
        container: usize,
        all_matched: bool,
        blank: bool,
        cursor: &Cursor,
    ) -> bool {
        if !self.extensions.contains(Extension::PipeTables) || !all_matched || blank {
            return false;
        }
        let Some(leaf) = self
            .last_open_child(container)
            .filter(|&c| matches!(self.kind(c), Some(Kind::Paragraph | Kind::LineBlock)))
        else {
            return false;
        };
        let rest = cursor.remaining();
        let Some(header) = self.node_text_ref(leaf) else {
            return false;
        };
        // A line block becomes a table only on its very first line: a delimiter row directly under
        // the opening `|` line reinterprets it as a table header. Past the first line the block is
        // committed to being a line block.
        if matches!(self.kind(leaf), Some(Kind::LineBlock)) {
            if !single_line(header)
                || !table::opens_table(header.trim_end(), rest, self.greedy_paragraphs)
            {
                return false;
            }
            if let Some(node) = self.nodes.get_mut(leaf) {
                node.kind = Kind::Paragraph;
                node.pipe_table = true;
            }
            self.append_text(leaf, rest);
            self.append_text(leaf, "\n");
            return true;
        }
        let established = self.nodes.get(leaf).is_some_and(|node| node.pipe_table);
        match table::classify_continuation(header, rest, self.greedy_paragraphs, established) {
            table::Continuation::Absorb => {
                if let Some(node) = self.nodes.get_mut(leaf) {
                    node.pipe_table = true;
                }
                self.append_text(leaf, rest);
                self.append_text(leaf, "\n");
                true
            }
            table::Continuation::Terminate => {
                self.close(leaf);
                false
            }
            table::Continuation::NotTable => false,
        }
    }

    /// Let an open line block claim its continuation lines before the block openers run. A `|` flush
    /// at the line start opens a new entry. A line led by whitespace continues the previous entry,
    /// but only while that entry holds content: a continuation under an empty entry, a flush-left
    /// non-bar line, and a wholly empty line all end the block (the line is then reparsed). Returns
    /// `true` when the line was absorbed.
    fn continue_line_block(
        &mut self,
        container: usize,
        all_matched: bool,
        cursor: &Cursor,
    ) -> bool {
        if !self.extensions.contains(Extension::LineBlocks) || !all_matched {
            return false;
        }
        let Some(block) = self
            .last_open_child(container)
            .filter(|&c| matches!(self.kind(c), Some(Kind::LineBlock)))
        else {
            return false;
        };
        let remaining = cursor.remaining();
        let absorb = is_line_block_marker(remaining)
            || (remaining.starts_with(' ')
                && self
                    .node_text_ref(block)
                    .is_some_and(|text| !last_entry_is_empty(text)));
        if absorb {
            self.append_text(block, remaining);
            self.append_text(block, "\n");
            true
        } else {
            self.close(block);
            false
        }
    }

    /// Feed a continuation line to an open raw TeX environment, returning `true` when the line was
    /// absorbed. Reachable only when every container matched, so the verbatim text stays aligned.
    fn continue_raw_tex(&mut self, container: usize, all_matched: bool, cursor: &Cursor) -> bool {
        if !all_matched {
            return false;
        }
        let Some(leaf) = self
            .last_open_child(container)
            .filter(|&c| matches!(self.kind(c), Some(Kind::RawTex { .. })))
        else {
            return false;
        };
        self.feed_raw_tex(leaf, cursor.remaining());
        true
    }

    /// Open a raw TeX environment when the cursor sits on a `\begin{NAME}` at the line start. The
    /// environment gathers lines verbatim through its matching `\end{NAME}` and renders as a
    /// `RawBlock` for `tex`. Math environments stay inline, so they fall through to a paragraph here.
    /// Unlike the foldable openers this one interrupts an open paragraph; a `\begin` that never finds
    /// its `\end` is settled back into a paragraph at end of input.
    ///
    /// Known limitations, both niche and exact in the common free-standing form:
    /// - When the environment directly interrupts an open paragraph with no blank line between, that
    ///   preceding paragraph renders as `Para` rather than the tighter `Plain`.
    /// - A math environment hands its body to the inline phase rather than being gathered verbatim
    ///   here, so a non-math `\begin{…}` sitting at column 0 *inside* a math environment opens a
    ///   fresh block environment there instead of staying part of the enclosing inline math span. A
    ///   nested environment indented or sharing a line with surrounding math — the usual way it is
    ///   written — stays within the span.
    fn open_raw_tex(&mut self, container: usize, cursor: &mut Cursor) -> Option<usize> {
        if !self.extensions.contains(Extension::RawTex) {
            return None;
        }
        let name = raw_tex_env_name(cursor.remaining(), b"begin")?;
        if is_math_environment(&name) {
            return None;
        }
        let line = cursor.rest();
        cursor.advance_chars(line.chars().count());
        let kind = Kind::RawTex { name, depth: 0 };
        let parent = self.place(container, &kind);
        let index = self.append_child(parent, Node::new(kind));
        self.feed_raw_tex(index, &line);
        Some(index)
    }

    /// Append one source `line` to an open raw TeX environment and advance its nesting depth. Each
    /// `\begin{NAME}` of the opener's own name deepens the nesting and each `\end{NAME}` lifts it;
    /// when the depth returns to zero the environment closes at that `\end`, dropping the trailing
    /// newline. Any content after the closing `\end` on the same line is re-fed as a fresh line.
    fn feed_raw_tex(&mut self, index: usize, line: &str) {
        let Some(Kind::RawTex { name, depth }) = self.kind(index) else {
            return;
        };
        let (new_depth, close_at) = raw_tex_scan(line, name, *depth);
        if let Some(end) = close_at {
            // The matching `\end` ends the environment; the closing newline is dropped, and any
            // content past the `\end` on this line is re-fed as a fresh line.
            self.append_text(index, line.get(..end).unwrap_or(line));
            self.set_raw_tex_depth(index, 0);
            self.close(index);
            let trailing = line.get(end..).unwrap_or("").to_owned();
            if !trailing.is_empty() {
                self.process_line(&trailing, &[], None);
            }
            return;
        }
        self.append_text(index, line);
        self.append_text(index, "\n");
        self.set_raw_tex_depth(index, new_depth);
    }

    fn set_raw_tex_depth(&mut self, index: usize, value: usize) {
        if let Some(node) = self.nodes.get_mut(index)
            && let Kind::RawTex { depth, .. } = &mut node.kind
        {
            *depth = value;
        }
    }

    /// Open a block-level HTML element when the cursor sits on a recognized open tag. A `<div>`
    /// becomes an [`IrBlock::Div`] when `native_divs` is on; any other block tag (and a `<div>` when
    /// only `markdown_in_html_blocks` is on) keeps its tags as raw HTML around the parsed content.
    /// The whole open tag is consumed; any same-line remainder is re-fed so its content — including a
    /// close tag on the same line — flows through the normal line handling.
    ///
    /// When the element directly interrupts an open paragraph with no blank line between, that
    /// preceding paragraph reads tight — `Plain` rather than `Para` — under `markdown_in_html_blocks`.
    /// A self-closing tag (`<div/>`) is read as an ordinary open and stays open until end of input.
    fn open_html_element(
        &mut self,
        container: usize,
        indent: usize,
        cursor: &mut Cursor,
    ) -> Option<usize> {
        let native_divs = self.extensions.contains(Extension::NativeDivs);
        let markdown_in_html = self.extensions.contains(Extension::MarkdownInHtmlBlocks);
        if !native_divs && !markdown_in_html {
            return None;
        }
        let remaining = cursor.remaining();
        let open = html_element::parse_open_tag(remaining)?;
        let is_div = open.tag == "div";
        // A div renders as a native div only with `native_divs`; otherwise it (and every other block
        // tag) needs `markdown_in_html_blocks` to have its content parsed. A block tag governed by
        // neither extension falls through to the raw HTML-block reading.
        let as_div = is_div && native_divs;
        if !as_div && !markdown_in_html {
            return None;
        }
        // A paragraph still open here was not separated from the element by a blank line; with
        // `markdown_in_html_blocks` the element interrupts it as a block and it reads tight.
        if markdown_in_html {
            self.tighten_interrupted_paragraph(container);
        }
        let raw_open = remaining.get(..open.len).unwrap_or(remaining).to_owned();
        let trailing = remaining.get(open.len..).unwrap_or("").to_owned();
        // Consume the whole remainder so the line is fully read here; any content after the open tag
        // is handled by re-feeding `trailing` rather than by the cursor. The cursor advances one byte
        // per step, so the byte length is the right amount even with multibyte characters present.
        cursor.advance_chars(remaining.len());
        let kind = Kind::HtmlElement(HtmlElementInfo {
            tag: open.tag,
            attr: open.attr,
            raw_open: format!("{}{raw_open}", " ".repeat(indent)),
            raw_close: String::new(),
            as_div,
            tighten_last: false,
        });
        let parent = self.place(container, &kind);
        let index = self.append_child(parent, Node::new(kind));
        if !trailing.trim().is_empty() {
            self.process_line(&trailing, &[], None);
        }
        Some(index)
    }

    /// The Markdown family reading raw HTML where inner content is not parsed: a block-level tag is
    /// kept verbatim rather than opened as a native div or a markdown-in-HTML element.
    fn markdown_raw_html(&self) -> bool {
        self.greedy_paragraphs
            && !self.extensions.contains(Extension::MarkdownInHtmlBlocks)
            && !self.extensions.contains(Extension::NativeDivs)
    }

    /// In the Markdown family reading raw HTML, a block-level HTML tag that starts the line opens a
    /// raw block. A non-self-closing open tag with a balanced matching close is a span running to that
    /// close (nested same-name tags and blank lines included); every other block tag — self-closing,
    /// void (no close ahead), or a bare close tag — is a single-line raw block. With
    /// `markdown_attribute` the block reads like the other block openers — it accepts up to three
    /// columns of indentation and interrupts an open paragraph (which then reads tight); without it
    /// the tag must stand at column zero and folds into an open paragraph as ordinary text instead.
    fn open_markdown_raw_html(
        &mut self,
        container: usize,
        indent: usize,
        in_paragraph: bool,
        cursor: &mut Cursor,
        following: &[&str],
    ) -> Option<usize> {
        enum Opener {
            /// A single-line raw block holding `line[..end]`.
            Single(usize),
            /// A multi-line span with the given tag name and open-nesting depth after its first line.
            Span(String, usize),
        }
        let interrupts = self.extensions.contains(Extension::MarkdownAttribute);
        let max_indent = if interrupts { 3 } else { 0 };
        if !self.markdown_raw_html() || indent > max_indent || (in_paragraph && !interrupts) {
            return None;
        }
        let line = cursor.remaining().to_owned();
        let opener = if let Some(open) = html_element::parse_open_tag(&line) {
            let after_tag = line.get(open.len..).unwrap_or("");
            let (line_depth, same_line) = if open.self_closing {
                (0, None)
            } else {
                html_element::scan_depth(after_tag, &open.tag, 1)
            };
            match same_line {
                Some(offset) => Opener::Single(open.len + offset),
                // A self-closing/void tag, or an open tag with no balanced close ahead: the tag alone
                // is the raw block, and anything after it on the line is parsed normally.
                None if line_depth == 0
                    || !self.raw_html_span_closes(container, line_depth, &open.tag, following) =>
                {
                    Opener::Single(open.len)
                }
                None => Opener::Span(open.tag, line_depth),
            }
        } else if let Some(len) = html_element::parse_close_tag(&line) {
            Opener::Single(len)
        } else {
            return None;
        };
        // The whole opener line is read here; content past a close tag is re-fed as a fresh line.
        cursor.advance_chars(line.len());
        if in_paragraph {
            let leaf = self.deepest_open(container);
            if matches!(self.kind(leaf), Some(Kind::Paragraph)) {
                if let Some(node) = self.nodes.get_mut(leaf) {
                    node.as_plain = true;
                }
                self.close(leaf);
            }
        }
        match opener {
            Opener::Single(end) => Some(self.emit_raw_html_leaf(container, &line, end)),
            Opener::Span(tag, depth) => {
                let kind = Kind::RawHtmlSpan { tag, depth };
                let parent = self.place(container, &kind);
                let index = self.append_child(parent, Node::new(kind));
                self.append_text(index, &line);
                self.append_text(index, "\n");
                Some(index)
            }
        }
    }

    /// Emit a single-line raw HTML block holding `line[..end]`, re-feeding any trailing content on the
    /// line as a fresh line, and return the closed leaf.
    fn emit_raw_html_leaf(&mut self, container: usize, line: &str, end: usize) -> usize {
        let kept = line.get(..end).unwrap_or(line).to_owned();
        let rest = line.get(end..).unwrap_or("").to_owned();
        let kind = Kind::RawHtmlSpan {
            tag: String::new(),
            depth: 0,
        };
        let parent = self.place(container, &kind);
        let index = self.append_child(parent, Node::new(kind));
        self.append_text(index, &kept);
        self.close(index);
        if !rest.trim().is_empty() {
            self.process_line(&rest, &[], None);
        }
        index
    }

    /// Whether a raw HTML span opened at `depth` finds its balanced close somewhere in the following
    /// lines (read at the container's own indentation). A span cannot outlast its container.
    fn raw_html_span_closes(
        &self,
        container: usize,
        depth: usize,
        tag: &str,
        following: &[&str],
    ) -> bool {
        let path = self.container_path(container);
        let mut depth = depth;
        for line in following {
            let mut cursor = Cursor::new(line);
            if !self.strip_container_path(&path, &mut cursor) {
                return false;
            }
            let (next, close) = html_element::scan_depth(cursor.remaining(), tag, depth);
            if close.is_some() {
                return true;
            }
            depth = next;
        }
        false
    }

    fn text_tables_enabled(&self) -> bool {
        self.extensions.contains(Extension::SimpleTables)
            || self.extensions.contains(Extension::MultilineTables)
    }

    /// Let a dash-ruled table claim its lines before the block openers run. A single-line paragraph
    /// directly above a dash ruling is the header of a new table: the paragraph is retyped and the
    /// ruling folded onto it, so the rows below gather into one leaf. An already-open table leaf
    /// absorbs each further line, and a blank line settles it (see [`Parser::finalize_text_table`]).
    /// Returns `true` when the line was absorbed.
    fn continue_text_table(
        &mut self,
        container: usize,
        all_matched: bool,
        blank: bool,
        cursor: &Cursor,
    ) -> bool {
        if !self.text_tables_enabled() || !all_matched {
            return false;
        }
        let Some(leaf) = self.last_open_child(container) else {
            return false;
        };
        match self.kind(leaf) {
            Some(Kind::Paragraph) => {
                if blank {
                    return false;
                }
                let Some(header) = self.node_text_ref(leaf) else {
                    return false;
                };
                if !single_line(header) || !texttable::is_dash_line(cursor.remaining()) {
                    return false;
                }
                // A dash-ruled table has no cell delimiters: its columns are positional, and per-column
                // alignment is read from where the header text sits relative to the dash runs. So the
                // header and ruling must share one coordinate. The header reached here de-indented
                // through the paragraph path, so it is restored to the column it began at; the ruling
                // and the rows below keep their own leading whitespace.
                let header_indent = self.nodes.get(leaf).map_or(0, |node| node.indent);
                let ruling = cursor.remaining();
                let header = format!("{}{header}", " ".repeat(header_indent));
                if let Some(node) = self.nodes.get_mut(leaf) {
                    node.kind = Kind::TextTable;
                    node.text = header;
                }
                self.append_text(leaf, ruling);
                self.append_text(leaf, "\n");
                true
            }
            Some(Kind::TextTable) => {
                if blank {
                    let Some(text) = self.node_text_ref(leaf) else {
                        return false;
                    };
                    let first = text.split('\n').next().unwrap_or("");
                    // A header-led table (its first line is text, not a ruling) ends at the blank.
                    // A dash-led table ends only once its closing ruling has been read; until then a
                    // blank separates the multi-line rows of the body and is kept.
                    let settle = !texttable::is_dash_line(first)
                        || texttable::is_dash_line(last_nonempty_line(text));
                    if settle {
                        self.finalize_text_table(leaf);
                        return false;
                    }
                    self.append_text(leaf, "\n");
                    return true;
                }
                self.append_line(leaf, cursor);
                true
            }
            _ => false,
        }
    }

    /// Settle an open dash-ruled table leaf. Its accumulated lines are parsed into a table: when they
    /// all belong to it the leaf closes as the table; when only a prefix does, the leaf keeps that
    /// prefix and the surplus lines are re-fed as following blocks; when they form no table the leaf
    /// is repurposed into the thematic break or paragraph its first line is, with the rest re-fed.
    fn finalize_text_table(&mut self, leaf: usize) {
        let text = self.node_text(leaf);
        let lines = split_table_lines(&text);
        match texttable::parse(&lines, self.extensions) {
            Some((_, consumed)) if consumed >= lines.len() => self.close(leaf),
            Some((_, consumed)) => {
                let kept = lines.get(..consumed).unwrap_or(&[]).join("\n");
                let rest = owned_lines(lines.get(consumed..).unwrap_or(&[]));
                if let Some(node) = self.nodes.get_mut(leaf) {
                    node.text = if kept.is_empty() {
                        kept
                    } else {
                        format!("{kept}\n")
                    };
                }
                self.close(leaf);
                self.refeed_lines(&rest);
            }
            None => {
                let first = lines.first().copied().unwrap_or("");
                let rest = owned_lines(lines.get(1..).unwrap_or(&[]));
                if let Some(node) = self.nodes.get_mut(leaf) {
                    if is_thematic_dash_line(first) {
                        node.kind = Kind::ThematicBreak;
                        node.text = String::new();
                    } else {
                        node.kind = Kind::Paragraph;
                        node.text = format!("{first}\n");
                    }
                }
                self.close(leaf);
                self.refeed_lines(&rest);
            }
        }
    }

    /// Re-feed a run of buffered lines through the line handler, each seeing the ones after it as
    /// its look-ahead so a fenced code block among them can still find its closing fence.
    fn refeed_lines(&mut self, lines: &[String]) {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        for index in 0..refs.len() {
            let line = refs.get(index).copied().unwrap_or("");
            let following = refs.get(index + 1..).unwrap_or(&[]);
            self.process_line(line, following, None);
        }
    }

    /// Settle every dash-ruled table leaf still open at end of input. Re-feeding surplus lines may
    /// open a fresh candidate, which the next pass settles; each pass strictly shrinks the work.
    fn finalize_open_text_tables(&mut self) {
        while let Some(leaf) = self.open_text_table_leaf() {
            self.finalize_text_table(leaf);
        }
    }

    fn open_text_table_leaf(&self) -> Option<usize> {
        (0..self.nodes.len()).find(|&index| {
            matches!(self.kind(index), Some(Kind::TextTable))
                && self.nodes.get(index).is_some_and(|node| node.open)
        })
    }

    /// Close `tip` and each ancestor up to (but not including) `until`.
    fn close_chain(&mut self, mut tip: usize, until: usize) {
        while tip != until {
            self.close(tip);
            let parent = self.parent(tip);
            if parent == tip {
                break;
            }
            tip = parent;
        }
    }

    /// The block a trailing blank line attaches to: descend through finalized last-children so the
    /// blank lands on the content it follows (e.g. a closed code block) rather than its still-open
    /// container. Stops at an empty container, which the blank then trails directly.
    ///
    /// Descent follows the list structure only. Loose-list classification reads the same
    /// List/Item chain, so the blank must mark a block on it: descending into a closed block quote
    /// or fenced div would bury the mark where the classification never looks, leaving the list
    /// wrongly tight. Such a block is itself the trailing block, so the blank stops there.
    fn blank_trails(&self, mut index: usize) -> usize {
        while let Some(&last) = self.nodes.get(index).and_then(|node| node.children.last()) {
            if self.nodes.get(last).is_some_and(|node| !node.open) {
                index = last;
                if !matches!(self.kind(index), Some(Kind::List(_) | Kind::Item(_))) {
                    break;
                }
            } else {
                break;
            }
        }
        index
    }

    fn deepest_open(&self, mut index: usize) -> usize {
        while let Some(child) = self.last_open_child(index) {
            index = child;
        }
        index
    }

    /// Whether an open footnote definition sits in the chain strictly below `container` — that is,
    /// the paragraph a line would lazily continue is that definition's body. A definition marker then
    /// ends the open definition and starts a new one rather than folding into it.
    fn footnote_def_open_below(&self, container: usize) -> bool {
        let mut index = self.deepest_open(container);
        while index != container {
            if matches!(self.kind(index), Some(Kind::FootnoteDef(_))) {
                return true;
            }
            let parent = self.parent(index);
            if parent == index {
                break;
            }
            index = parent;
        }
        false
    }

    /// Mark the open paragraph interrupted by a block opener under `container` so it renders tight
    /// (`Plain`). A no-op when the deepest open block is not a paragraph.
    fn tighten_interrupted_paragraph(&mut self, container: usize) {
        let leaf = self.deepest_open(container);
        if matches!(self.kind(leaf), Some(Kind::Paragraph))
            && let Some(node) = self.nodes.get_mut(leaf)
        {
            node.as_plain = true;
        }
    }

    /// If the current line closes an open fenced div, close that div and everything nested inside it
    /// — a colon fence preempts even an open code block — and return `true`. It is honored only when
    /// the descent reached the innermost open div, so a fence shallower than a still-open nested div
    /// stays ordinary content (which an enclosing div may then hold) rather than closing that div or
    /// an ancestor. `div_path` holds the divs matched on this line, innermost last, each paired with
    /// the line as it stood at that div's indentation.
    fn close_fenced_div(&mut self, container: usize, div_path: &[(usize, String)]) -> bool {
        if !self.extensions.contains(Extension::FencedDivs) {
            return false;
        }
        let Some((inner, inner_line)) = div_path.last() else {
            return false;
        };
        if self.innermost_open_div() != Some(*inner) {
            return false;
        }
        let Some(count) = div_close_fence(inner_line) else {
            return false;
        };
        let Some(target) = self.div_close_target(div_path, count) else {
            return false;
        };
        let tip = self.deepest_open(container);
        let stop = self.parent(target);
        self.close_chain(tip, stop);
        true
    }

    /// If this line carries the matching close tag of the innermost open HTML element, close that
    /// element and return `true`. Content preceding the tag on its line is fed as the element's final
    /// content (which is then tightened to `Plain`); content after the tag is re-fed as a fresh line.
    fn close_html_element(&mut self, container: usize, cursor: &Cursor) -> bool {
        if !self.extensions.contains(Extension::NativeDivs)
            && !self.extensions.contains(Extension::MarkdownInHtmlBlocks)
        {
            return false;
        }
        let Some(element) = self.innermost_open_html_element() else {
            return false;
        };
        let line = cursor.remaining();
        let trimmed = line.trim_start_matches(' ');
        let leading = line.len() - trimmed.len();
        if leading > 3 {
            return false;
        }
        let (found, as_div) = match self.kind(element) {
            Some(Kind::HtmlElement(info)) => {
                let Some(found) = html_element::find_close_tag(trimmed, &info.tag) else {
                    return false;
                };
                (found, info.as_div)
            }
            _ => return false,
        };
        let before = trimmed.get(..found.start).unwrap_or("");
        let close_tag = trimmed.get(found.start..found.end).unwrap_or("").to_owned();
        let after = trimmed.get(found.end..).unwrap_or("").to_owned();
        let trails = !before.trim().is_empty();
        if trails {
            self.process_line(before, &[], None);
        }
        // Whether the element's final content block tightens from `Para` to `Plain`. A native div
        // tightens only when the close tag physically trails content on its own line. A raw element
        // tightens whenever no blank line separates its last content from the close tag — which holds
        // exactly when a paragraph is still open at the close (a blank line would have closed it).
        let tighten = if as_div {
            trails
        } else {
            matches!(self.kind(self.deepest_open(element)), Some(Kind::Paragraph))
        };
        // The deepest open block under the element is the close boundary's chain tip.
        let tip = self.deepest_open(container);
        self.close_chain(tip, element);
        if let Some(node) = self.nodes.get_mut(element)
            && let Kind::HtmlElement(info) = &mut node.kind
        {
            info.raw_close = close_tag;
            info.tighten_last = tighten;
            node.open = false;
        }
        if !after.trim().is_empty() {
            self.process_line(&after, &[], None);
        }
        true
    }

    /// The innermost open HTML element anywhere in the tree, or `None` when none is open.
    fn innermost_open_html_element(&self) -> Option<usize> {
        let mut node = self.deepest_open(0);
        loop {
            if matches!(self.kind(node), Some(Kind::HtmlElement(_))) {
                return Some(node);
            }
            let parent = self.parent(node);
            if parent == node {
                return None;
            }
            node = parent;
        }
    }

    /// The innermost open fenced div anywhere in the tree, or `None` when none is open.
    fn innermost_open_div(&self) -> Option<usize> {
        let mut node = self.deepest_open(0);
        loop {
            if matches!(self.kind(node), Some(Kind::FencedDiv(..))) {
                return Some(node);
            }
            let parent = self.parent(node);
            if parent == node {
                return None;
            }
            node = parent;
        }
    }

    /// Choose which fenced div a closing run of `count` colons shuts. The matched divs are read
    /// innermost first, each paired with the closing line as it stood at that div's indentation; the
    /// first div long enough to be closed by `count` colons is the target. A div the closing line
    /// sits more than three columns into is unreachable and, with every div outside it indented at
    /// least as far, ends the search — the line is ordinary content rather than a fence.
    fn div_close_target(&self, div_path: &[(usize, String)], count: usize) -> Option<usize> {
        for (node, line) in div_path.iter().rev() {
            let leading = line.len() - line.trim_start_matches(' ').len();
            if leading > 3 {
                return None;
            }
            if let Some(Kind::FencedDiv(info)) = self.kind(*node)
                && info.fence <= count
            {
                return Some(*node);
            }
        }
        None
    }

    fn is_container(&self, index: usize) -> bool {
        matches!(
            self.kind(index),
            Some(
                Kind::Document
                    | Kind::BlockQuote
                    | Kind::List(_)
                    | Kind::Item(_)
                    | Kind::FootnoteDef(_)
                    | Kind::FencedDiv(..)
                    | Kind::HtmlElement(_)
                    | Kind::DefinitionList
                    | Kind::DefinitionItem { .. }
                    | Kind::Definition { .. }
            )
        )
    }

    fn convert_paragraph_to_heading(&mut self, para: usize, level: i32) {
        if let Some(node) = self.nodes.get_mut(para) {
            node.kind = Kind::Heading(level);
            node.open = false;
            node.text = node.text.trim().to_owned();
        }
    }

    /// Try to continue an open container (block quote / list item) or open leaf on this line.
    fn try_continue(&mut self, index: usize, cursor: &mut Cursor) -> Continue {
        match self.kind(index) {
            // A list (and the two grouping levels of a definition list) is a transparent container:
            // it consumes nothing and defers to its items.
            Some(Kind::List(_) | Kind::DefinitionList | Kind::DefinitionItem { .. }) => {
                Continue::Matched
            }
            // An HTML element is transparent: it consumes no marker and lets its inner lines flow to
            // the openers below. Its matching close tag is detected separately in `process_line`.
            Some(Kind::HtmlElement(_)) => Continue::Matched,
            // A definition body continues under its content indent, like a list item — except that
            // an as-yet-empty body survives a blank line, so a deferred indented paragraph still
            // joins it.
            Some(Kind::Definition { indent }) => {
                let indent = *indent;
                self.continue_item_like(index, indent, true, cursor)
            }
            // A fenced div re-bases its content to the column it opened at: it consumes up to that
            // many leading columns, then stays open until a matching close fence (handled in
            // `process_line`) or end of input.
            Some(Kind::FencedDiv(info)) => {
                cursor.advance_columns(info.indent);
                Continue::Matched
            }
            Some(Kind::BlockQuote) => {
                let checkpoint = cursor.checkpoint();
                cursor.skip_up_to_three_spaces();
                if cursor.peek() == Some(b'>') {
                    cursor.advance_one();
                    cursor.consume_optional_space();
                    Continue::Matched
                } else {
                    // Restore the speculatively consumed indentation so phase 2 sees the line's
                    // true indent (e.g. a non-continued line that is itself indented code).
                    cursor.reset_to(checkpoint);
                    Continue::NotMatched
                }
            }
            Some(Kind::Item(info)) => {
                let indent = info.indent;
                self.continue_item_like(index, indent, false, cursor)
            }
            // A footnote definition's content continues under a four-column indent, the same as a
            // list item; an unindented non-blank line ends it (an open paragraph may still take it
            // as a lazy continuation, handled by the caller).
            Some(Kind::FootnoteDef(_)) => {
                if cursor.is_blank() {
                    if self.nodes.get(index).is_some_and(|n| n.children.is_empty()) {
                        Continue::NotMatched
                    } else {
                        Continue::Matched
                    }
                } else if cursor.indent() >= TAB_STOP {
                    cursor.advance_columns(TAB_STOP);
                    Continue::Matched
                } else {
                    Continue::NotMatched
                }
            }
            Some(Kind::FencedCode(fence)) => {
                let (marker, length, indent) = (fence.marker, fence.length, fence.indent);
                self.continue_fenced(index, marker, length, indent, cursor);
                Continue::MatchedLeaf
            }
            Some(Kind::IndentedCode) => {
                if cursor.is_blank() {
                    cursor.advance_up_to_columns(TAB_STOP);
                    self.append_line(index, cursor);
                    Continue::MatchedLeaf
                } else if cursor.indent() >= TAB_STOP {
                    cursor.advance_columns(TAB_STOP);
                    self.append_line(index, cursor);
                    Continue::MatchedLeaf
                } else {
                    Continue::NotMatched
                }
            }
            Some(Kind::HtmlBlock(kind)) => {
                let kind = *kind;
                self.continue_html(index, kind, cursor);
                Continue::MatchedLeaf
            }
            Some(Kind::RawHtmlSpan { tag, depth }) => {
                let tag = tag.clone();
                let depth = *depth;
                self.continue_raw_html_span(index, &tag, depth, cursor);
                Continue::MatchedLeaf
            }
            _ => Continue::NotMatched,
        }
    }

    /// Continue a content container whose lines belong to it under a fixed `indent` (a list item or
    /// a definition body): a non-blank line continues it when indented to the content column, which
    /// is then consumed. A blank line continues it once it has content; when it is still empty,
    /// `blank_keeps_empty` decides — a list item ends, while a definition body waits for a deferred
    /// indented paragraph.
    fn continue_item_like(
        &self,
        index: usize,
        indent: usize,
        blank_keeps_empty: bool,
        cursor: &mut Cursor,
    ) -> Continue {
        if cursor.is_blank() {
            if !blank_keeps_empty && self.nodes.get(index).is_some_and(|n| n.children.is_empty()) {
                Continue::NotMatched
            } else {
                Continue::Matched
            }
        } else if cursor.indent() >= indent {
            cursor.advance_columns(indent);
            Continue::Matched
        } else {
            Continue::NotMatched
        }
    }

    fn continue_fenced(
        &mut self,
        index: usize,
        marker: u8,
        length: usize,
        fence_indent: usize,
        cursor: &mut Cursor,
    ) {
        let indent = cursor.indent();
        if indent <= 3 && cursor.is_closing_fence(marker, length) {
            if let Some(node) = self.nodes.get_mut(index) {
                node.open = false;
            }
            return;
        }
        cursor.advance_up_to_columns(fence_indent);
        self.append_line(index, cursor);
    }

    fn continue_html(&mut self, index: usize, kind: u8, cursor: &mut Cursor) {
        // Types 6 and 7 are terminated by a blank line, which is not part of the block.
        if matches!(kind, 6 | 7) && cursor.is_blank() {
            self.close(index);
            return;
        }
        let line = cursor.remaining();
        self.append_text(index, line);
        self.append_text(index, "\n");
        if html_block::closes(kind, line) {
            self.close(index);
        }
    }

    /// Continue an open raw HTML span: absorb this line verbatim (blank lines included), tracking the
    /// nesting of same-name tags. When a close tag brings the depth to zero, the span ends with that
    /// tag; any content after it on the line is re-fed as a fresh line.
    fn continue_raw_html_span(
        &mut self,
        index: usize,
        tag: &str,
        depth: usize,
        cursor: &mut Cursor,
    ) {
        let line = cursor.remaining();
        let (new_depth, close) = html_element::scan_depth(line, tag, depth);
        if let Some(offset) = close {
            let kept = line.get(..offset).unwrap_or(line).to_owned();
            let rest = line.get(offset..).unwrap_or("").to_owned();
            self.append_text(index, &kept);
            self.set_raw_html_depth(index, 0);
            self.close(index);
            if !rest.trim().is_empty() {
                self.process_line(&rest, &[], None);
            }
        } else {
            self.append_text(index, line);
            self.append_text(index, "\n");
            self.set_raw_html_depth(index, new_depth);
        }
    }

    fn set_raw_html_depth(&mut self, index: usize, value: usize) {
        if let Some(node) = self.nodes.get_mut(index)
            && let Kind::RawHtmlSpan { depth, .. } = &mut node.kind
        {
            *depth = value;
        }
    }

    /// How a greedy paragraph (the markdown dialect) absorbs a following block opener. The foldable
    /// openers — a block quote, heading, thematic break, fenced div, or footnote definition —
    /// continue an open paragraph as a lazy line rather than interrupting it, even across a container
    /// the line did not match (`> a` then `# b`); only a blank line, a fenced code block, or an HTML
    /// block ends it. The block-quote and heading folds are gated further on the
    /// `blank_before_blockquote` and `blank_before_header` toggles, so dropping a toggle lets that
    /// opener interrupt again. A list marker is structural: it still opens a sibling item in an open
    /// list or a sublist inside an item, and folds only where it would otherwise *start* a fresh list
    /// — when the paragraph is the container's own last child and the container is not itself a list
    /// item or other indented item body. The `lists_without_preceding_blankline` toggle drops that
    /// last fold, so a fresh list interrupts the paragraph instead.
    fn greedy_gates(&self, container: usize, in_paragraph: bool) -> GreedyGates {
        if !self.greedy_paragraphs {
            return GreedyGates::default();
        }
        let fresh_list_into_paragraph = !self
            .extensions
            .contains(Extension::ListsWithoutPrecedingBlankline)
            && !matches!(
                self.kind(container),
                Some(Kind::Item(_) | Kind::Definition { .. } | Kind::FootnoteDef(_))
            )
            && matches!(
                self.last_open_child(container)
                    .and_then(|child| self.kind(child)),
                Some(Kind::Paragraph)
            );
        GreedyGates {
            foldable: in_paragraph,
            list_start: fresh_list_into_paragraph,
            blockquote: in_paragraph && self.extensions.contains(Extension::BlankBeforeBlockquote),
            heading: in_paragraph && self.extensions.contains(Extension::BlankBeforeHeader),
        }
    }

    /// Whether a scanned code fence actually opens a fenced code block. Pure `CommonMark` always
    /// recognizes a fence. The Markdown dialect instead gates each fence character on its own
    /// extension — a backtick fence on `backtick_code_blocks`, a tilde fence on `fenced_code_blocks`
    /// — and, lacking any extension that gives a richer info string meaning, requires the info string
    /// to be a single bare language token: an info string carrying inner whitespace or a brace then
    /// names no language and the fence is left to fold into a paragraph.
    fn fence_opener_accepted(&self, fence: &FenceInfo) -> bool {
        if !self.greedy_paragraphs {
            return true;
        }
        let marker_allowed = match fence.marker {
            b'`' => self.extensions.contains(Extension::BacktickCodeBlocks),
            b'~' => self.extensions.contains(Extension::FencedCodeBlocks),
            _ => false,
        };
        if !marker_allowed {
            return false;
        }
        // A brace-delimited attribute block, or a raw-output marker, gives a non-bare info string
        // meaning; the finalizer then interprets it. Without any of those extensions only a bare
        // language token is accepted.
        if self.extensions.contains(Extension::FencedCodeAttributes)
            || self.extensions.contains(Extension::Attributes)
            || self.extensions.contains(Extension::RawAttribute)
        {
            return true;
        }
        !fence
            .info
            .trim()
            .chars()
            .any(|ch| ch.is_whitespace() || ch == '{' || ch == '}')
    }

    /// Whether an opening code fence has a matching closing fence ahead, within the same container.
    /// In the Markdown dialect a fenced code block must be closed: an unclosed fence — one that would
    /// run to the container's end — does not open, and its lines fold into a paragraph instead. Pure
    /// `CommonMark` lets an unclosed fence run to the end, so there a fence always opens.
    ///
    /// The closing fence is judged at the fence's own container level, so each look-ahead line first
    /// replays the open containers' continuation markers; a line that breaks the chain (a block quote
    /// losing its `>`, a list item losing its indent) cannot carry the close.
    fn fence_reaches_close(
        &self,
        container: usize,
        fence: &FenceInfo,
        following: &[&str],
        line_index: Option<usize>,
    ) -> bool {
        if !self.greedy_paragraphs {
            return true;
        }
        let path = self.container_path(container);
        // At the document root no container prefix can break the look-ahead chain, so the precomputed
        // candidate index gives exactly the lines this scan would accept — consult it in O(log n)
        // instead of walking every remaining line. Nested containers keep the linear replay.
        if let (Some(opener), [_root]) = (line_index, path.as_slice()) {
            return self
                .fence_close_candidates
                .reaches_close(fence.marker, fence.length, opener);
        }
        for line in following {
            let mut cursor = Cursor::new(line);
            if !self.strip_container_path(&path, &mut cursor) {
                return false;
            }
            if cursor.indent() <= 3 && cursor.is_closing_fence(fence.marker, fence.length) {
                return true;
            }
        }
        false
    }

    /// The chain of open containers from the document root down to `container`, root first.
    fn container_path(&self, container: usize) -> Vec<usize> {
        let mut path = Vec::new();
        let mut index = container;
        loop {
            path.push(index);
            let parent = self.parent(index);
            if parent == index {
                break;
            }
            index = parent;
        }
        path.reverse();
        path
    }

    /// Replay each container in `path` against a look-ahead `cursor`, read-only, consuming its
    /// continuation marker and leaving the cursor at the content column. Returns whether every
    /// container still matched. A blank line keeps an indent-based container (a list item, a
    /// definition body) open as interior content, but a block quote requires its `>` on every line.
    fn strip_container_path(&self, path: &[usize], cursor: &mut Cursor) -> bool {
        for &index in path {
            match self.kind(index) {
                Some(
                    Kind::Document
                    | Kind::List(_)
                    | Kind::DefinitionList
                    | Kind::DefinitionItem { .. }
                    | Kind::HtmlElement(_),
                ) => {}
                Some(Kind::BlockQuote) => {
                    cursor.skip_up_to_three_spaces();
                    if cursor.peek() == Some(b'>') {
                        cursor.advance_one();
                        cursor.consume_optional_space();
                    } else {
                        return false;
                    }
                }
                Some(Kind::FencedDiv(info)) => cursor.advance_columns(info.indent),
                Some(Kind::Item(ItemInfo { indent, .. }) | Kind::Definition { indent }) => {
                    let indent = *indent;
                    if !cursor.is_blank() {
                        if cursor.indent() >= indent {
                            cursor.advance_columns(indent);
                        } else {
                            return false;
                        }
                    }
                }
                Some(Kind::FootnoteDef(_)) => {
                    if !cursor.is_blank() {
                        if cursor.indent() >= TAB_STOP {
                            cursor.advance_columns(TAB_STOP);
                        } else {
                            return false;
                        }
                    }
                }
                _ => return false,
            }
        }
        true
    }

    /// Try to open a new block at the current cursor position inside `container`.
    // A flat dispatch over the block openers, tried in precedence order; it reads best as one sequence.
    #[allow(clippy::too_many_lines)]
    fn try_open(
        &mut self,
        container: usize,
        cursor: &mut Cursor,
        following: &[&str],
        line_index: Option<usize>,
    ) -> Option<usize> {
        let indent = cursor.indent();
        let in_paragraph = matches!(self.last_open_leaf_kind(container), Some(Kind::Paragraph));
        let gates = self.greedy_gates(container, in_paragraph);

        if indent >= TAB_STOP && !in_paragraph {
            cursor.advance_columns(TAB_STOP);
            let parent = self.place(container, &Kind::IndentedCode);
            let index = self.append_child(parent, Node::new(Kind::IndentedCode));
            self.append_line(index, cursor);
            return Some(index);
        }

        if indent < TAB_STOP {
            cursor.skip_indent();
            if let Some(block) = self.open_raw_tex(container, cursor) {
                return Some(block);
            }
            if !gates.blockquote && cursor.peek() == Some(b'>') {
                cursor.advance_one();
                cursor.consume_optional_space();
                let parent = self.place(container, &Kind::BlockQuote);
                return Some(self.append_child(parent, Node::new(Kind::BlockQuote)));
            }
            // A footnote definition opens here; its content is then gathered by the enclosing
            // container loop, the same as a block quote's. Like the other openers it folds into a
            // greedy paragraph rather than interrupting it — except when that paragraph is an open
            // definition's own body, since a definition marker always ends the preceding definition
            // and starts a new one.
            if (!gates.foldable || self.footnote_def_open_below(container))
                && self.extensions.contains(Extension::Footnotes)
                && let Some(label) = cursor.footnote_def_marker()
            {
                let key = scan::normalize_label(&label);
                let parent = self.place(container, &Kind::FootnoteDef(key.clone()));
                return Some(self.append_child(parent, Node::new(Kind::FootnoteDef(key))));
            }
            // A fenced div opens on a colon-run line carrying a valid attribute spec; it may
            // interrupt a paragraph. The whole fence line is consumed, so the div opens empty.
            if !gates.foldable && self.extensions.contains(Extension::FencedDivs) {
                let opener = div_open_fence(cursor.remaining());
                if let Some((count, attr)) = opener {
                    let width = cursor.remaining().chars().count();
                    cursor.advance_chars(width);
                    let kind = Kind::FencedDiv(DivInfo {
                        fence: count,
                        indent,
                        attr,
                    });
                    let parent = self.place(container, &kind);
                    return Some(self.append_child(parent, Node::new(kind)));
                }
            }
            // CommonMark permits up to three columns of indentation before an ATX opener; the
            // Markdown dialect requires the hashes to start at the left margin. A space after the
            // hash run is required in CommonMark and under `space_in_atx_header`; when that
            // extension is off in a Markdown dialect, a hash run glued to text opens a heading.
            let require_space =
                !self.greedy_paragraphs || self.extensions.contains(Extension::SpaceInAtxHeader);
            if !gates.heading
                && (!self.greedy_paragraphs || indent == 0)
                && let Some(level) = cursor.atx_heading(self.greedy_paragraphs, require_space)
            {
                let parent = self.place(container, &Kind::Heading(level));
                let index = self.append_child(parent, Node::new(Kind::Heading(level)));
                self.append_text(index, &strip_atx_closing(cursor.remaining(), require_space));
                self.close(index);
                return Some(index);
            }
            let fence_checkpoint = cursor.checkpoint();
            if let Some(fence) = cursor.fenced_code_start() {
                // In the Markdown family a tilde fence does not interrupt an open paragraph: its
                // opener line becomes ordinary continuation text and the lines after it are still
                // read normally, so a heading or other opener among them may still fire. A backtick
                // fence still interrupts.
                let folds_into_paragraph =
                    in_paragraph && self.greedy_paragraphs && fence.marker == b'~';
                if folds_into_paragraph {
                    cursor.reset_to(fence_checkpoint);
                } else if self.fence_opener_accepted(&fence)
                    && self.fence_reaches_close(container, &fence, following, line_index)
                {
                    let kind = Kind::FencedCode(fence);
                    let parent = self.place(container, &kind);
                    return Some(self.append_child(parent, Node::new(kind)));
                } else {
                    // The fence opens no code block; its opener line folds into a paragraph and the
                    // lines up to its matching close (if any) follow as that paragraph's text.
                    if self.greedy_paragraphs {
                        self.fence_fold = Some(fence);
                    }
                    cursor.reset_to(fence_checkpoint);
                }
            }
            // A recognized block-level HTML element whose inner content is parsed as markdown takes
            // precedence over the raw HTML-block reading, when the governing extension is on.
            if let Some(block) = self.open_html_element(container, indent, cursor) {
                return Some(block);
            }
            // With inner HTML not parsed, a block-level tag at the left margin spans to its balanced
            // close as one raw block.
            if let Some(block) =
                self.open_markdown_raw_html(container, indent, in_paragraph, cursor, following)
            {
                return Some(block);
            }
            // In that same reading, the block-level HTML kinds (6 and 7) are handled above or left as
            // inline text; only the container-agnostic kinds (comment, script/pre/style, processing
            // instruction, declaration, CDATA) still form a raw block here.
            if let Some(kind) = html_block::classify(cursor.remaining(), !in_paragraph)
                && !(self.markdown_raw_html() && matches!(kind, 6 | 7))
            {
                let parent = self.place(container, &Kind::HtmlBlock(kind));
                let index = self.append_child(parent, Node::new(Kind::HtmlBlock(kind)));
                // The start line keeps its leading indentation (always spaces after normalization).
                let line = format!("{}{}", " ".repeat(indent), cursor.remaining());
                self.append_text(index, &line);
                self.append_text(index, "\n");
                if html_block::closes(kind, &line) {
                    self.close(index);
                }
                return Some(index);
            }
            // A dash ruling at the start of a fresh block opens a header-less table candidate: its
            // rows and closing ruling gather into the leaf, settled when the block closes. A
            // paragraph directly above would instead make it a headed table (claimed before the
            // openers run), so the candidate only opens where no paragraph is open. It preempts the
            // thematic break a lone dash ruling would otherwise be — a candidate that yields no table
            // settles back into that break.
            if self.text_tables_enabled()
                && !in_paragraph
                && texttable::opens_dash_table(cursor.remaining())
            {
                let parent = self.place(container, &Kind::TextTable);
                let index = self.append_child(parent, Node::new(Kind::TextTable));
                // The ruling keeps its leading indentation: a dash-ruled table's columns are
                // positional, so every line must share one left margin (here, the rows below).
                let line = format!("{}{}", " ".repeat(indent), cursor.remaining());
                self.append_text(index, &line);
                self.append_text(index, "\n");
                return Some(index);
            }
            if !gates.foldable && cursor.thematic_break() {
                let parent = self.place(container, &Kind::ThematicBreak);
                let index = self.append_child(parent, Node::new(Kind::ThematicBreak));
                self.close(index);
                return Some(index);
            }
            if let Some(block) = self.open_line_block(container, indent, in_paragraph, cursor) {
                return Some(block);
            }
            // A definition marker turns the preceding paragraph into a term, or adds a definition to
            // the open entry. Its content is then opened by the enclosing loop inside the new body.
            if self.extensions.contains(Extension::DefinitionLists)
                && let Some(body) = self.open_definition(container, indent, cursor)
            {
                return Some(body);
            }
            if !gates.list_start
                && let Some(list) = self.list_marker(container, indent, cursor)
            {
                return Some(list);
            }
            // Under `lists_without_preceding_blankline` a line that has a list-marker shape ends a
            // greedy paragraph even when no enabled construct opens there: it starts a fresh
            // paragraph rather than folding into the open one. Three shapes break the paragraph:
            //   - a definition marker (`:`/`~`) when no definition list opens here;
            //   - an example-list marker (`(@)`, `(@label)`) when example lists are off;
            //   - any other enumerator shape carrying content on its line — judged with every
            //     enumerator style allowed, so independent of which styles actually form a list.
            // A decimal enumerator closed by a single `)` (`2)`) is the one exception for that last
            // shape — too easily ordinary prose — so it neither breaks the paragraph nor opens a
            // list. The first two shapes break regardless of any trailing content.
            if in_paragraph
                && self
                    .extensions
                    .contains(Extension::ListsWithoutPrecedingBlankline)
            {
                let definition_shape = cursor.definition_marker_at().is_some();
                let example_shape = matches!(
                    cursor.list_marker_at(true, true, false),
                    Some(marker) if matches!(marker.style, ListNumberStyle::Example)
                );
                let enumerator_shape =
                    cursor
                        .list_marker_at(true, false, false)
                        .is_some_and(|marker| {
                            !(marker.blank_after
                                || matches!(marker.style, ListNumberStyle::Decimal)
                                    && matches!(marker.delim, ListNumberDelim::OneParen))
                        });
                if definition_shape || example_shape || enumerator_shape {
                    if let Some(paragraph) = self
                        .last_open_child(container)
                        .filter(|&child| matches!(self.kind(child), Some(Kind::Paragraph)))
                    {
                        self.close(paragraph);
                    }
                    let parent = self.place(container, &Kind::Paragraph);
                    return Some(self.append_child(parent, Node::new(Kind::Paragraph)));
                }
            }
        }
        None
    }

    fn last_open_leaf_kind(&self, container: usize) -> Option<&Kind> {
        let leaf = self.deepest_open(container);
        self.kind(leaf)
    }

    /// Open a line block on a `|` flush at the line start. A line block never interrupts a paragraph
    /// and never carries leading indentation; its whole line is its first entry, with later lines
    /// claimed by `continue_line_block` before the openers run.
    fn open_line_block(
        &mut self,
        container: usize,
        indent: usize,
        in_paragraph: bool,
        cursor: &mut Cursor,
    ) -> Option<usize> {
        if !self.extensions.contains(Extension::LineBlocks)
            || indent != 0
            || in_paragraph
            || !is_line_block_marker(cursor.remaining())
        {
            return None;
        }
        let raw = cursor.rest();
        cursor.advance_chars(raw.chars().count());
        let parent = self.place(container, &Kind::LineBlock);
        let index = self.append_child(parent, Node::new(Kind::LineBlock));
        self.append_text(index, &raw);
        self.append_text(index, "\n");
        Some(index)
    }

    /// Open a definition body at a `:`/`~` marker, returning the new [`Kind::Definition`] container
    /// the enclosing loop then fills. The marker either starts a fresh definition of the open entry
    /// (when the cursor sits directly in a [`Kind::DefinitionItem`]) or, when the container's last
    /// child is a paragraph, turns that paragraph into a term — extending an immediately preceding
    /// definition list or beginning a new one. A marker in any other position is not consumed.
    fn open_definition(
        &mut self,
        container: usize,
        marker_indent: usize,
        cursor: &mut Cursor,
    ) -> Option<usize> {
        let blank_after = cursor.definition_marker_at()?;
        let item = if matches!(self.kind(container), Some(Kind::DefinitionItem { .. })) {
            container
        } else {
            self.start_definition_item(container)?
        };
        let indent = Self::consume_definition_marker(marker_indent, blank_after, cursor);
        Some(self.append_child(item, Node::new(Kind::Definition { indent })))
    }

    /// Turn the container's last child — which must be a non-empty paragraph — into a definition
    /// term, returning the [`Kind::DefinitionItem`] that now holds it. The term joins an immediately
    /// preceding definition list, else opens a new one. Returns `None` when there is no term
    /// paragraph to consume, leaving the marker as ordinary text.
    fn start_definition_item(&mut self, container: usize) -> Option<usize> {
        let &term_index = self.nodes.get(container)?.children.last()?;
        let term_node = self.nodes.get(term_index)?;
        if !matches!(term_node.kind, Kind::Paragraph) || term_node.text.trim().is_empty() {
            return None;
        }
        // A paragraph that is itself a grid or pipe table is not a definition term; a following `:`
        // line is its caption, not a definition marker.
        if self.extensions.contains(Extension::GridTables)
            && grid::parse(term_node.text.trim()).is_some()
        {
            return None;
        }
        if self.extensions.contains(Extension::PipeTables)
            && table::try_parse(term_node.text.trim(), self.greedy_paragraphs).is_some()
        {
            return None;
        }
        let term = term_node.text.trim().to_owned();
        let tight = !term_node.last_line_blank;
        let previous = self
            .nodes
            .get(container)
            .and_then(|node| node.children.iter().rev().nth(1).copied());
        let list = match previous {
            Some(prev) if matches!(self.kind(prev), Some(Kind::DefinitionList)) => {
                self.reopen_definition_list(prev);
                if let Some(node) = self.nodes.get_mut(container) {
                    node.children.pop();
                }
                prev
            }
            _ => {
                if let Some(node) = self.nodes.get_mut(container) {
                    node.children.pop();
                }
                self.append_child(container, Node::new(Kind::DefinitionList))
            }
        };
        Some(self.append_child(list, Node::new(Kind::DefinitionItem { term, tight })))
    }

    /// Reopen a definition list to accept a further term, closing the entry it last held so the new
    /// item descends as a sibling rather than nesting under the old one.
    fn reopen_definition_list(&mut self, list: usize) {
        if let Some(node) = self.nodes.get_mut(list) {
            node.open = true;
        }
        if let Some(&last_item) = self.nodes.get(list).and_then(|node| node.children.last()) {
            self.close(last_item);
        }
    }

    /// Consume a definition marker (`:`/`~` plus its following spaces) and return the column its
    /// content begins at. One through four spaces widen the content indent by that many columns;
    /// five or more collapse to a single column. An empty marker takes no content column at all, so
    /// its body begins one column past the marker — deferred indented lines join it as their own
    /// paragraph rather than continuing it.
    fn consume_definition_marker(
        marker_indent: usize,
        blank_after: bool,
        cursor: &mut Cursor,
    ) -> usize {
        cursor.advance_chars(1);
        let after_marker = cursor.indent();
        let content_offset = if blank_after {
            0
        } else if after_marker > TAB_STOP {
            1
        } else {
            after_marker
        };
        if !blank_after {
            cursor.advance_columns(content_offset);
        }
        marker_indent + 1 + content_offset
    }

    /// Try to open a list item (and its containing list) at the cursor.
    fn list_marker(
        &mut self,
        container: usize,
        marker_indent: usize,
        cursor: &mut Cursor,
    ) -> Option<usize> {
        let fancy = self.extensions.contains(Extension::FancyLists);
        let example = self.extensions.contains(Extension::ExampleLists);
        // The greedy Markdown dialect without fancy lists still recognizes the `#.` auto-number
        // placeholder, but its ordered lists are limited to the period delimiter: a `)`-delimited
        // enumerator such as `1)` is left as prose.
        let plain_ordered = self.greedy_paragraphs && !fancy;
        let parsed = cursor.list_marker_at(fancy, example, plain_ordered)?;
        if plain_ordered
            && !parsed.bullet
            && !matches!(
                parsed.delim,
                ListNumberDelim::DefaultDelim | ListNumberDelim::Period
            )
        {
            return None;
        }

        // These restrictions apply only when the marker would interrupt a *bare* paragraph (one not
        // already inside a list): an empty item cannot interrupt, and an ordered marker may only
        // interrupt when it is a decimal `1.`/`1)` — any other enumerator (a non-1 start, or an
        // alphabetic/roman/parenthesized one) is too easily confused with running prose. Inside an
        // open list any marker is allowed — a matching one continues the list, a differing one ends
        // it and begins a new sibling list.
        //
        // The paragraph must be a direct child of the container the marker opens into: a paragraph
        // hanging in a deeper, unmatched container (left open only for a possible lazy continuation)
        // is at a different level, so the marker starts a fresh list rather than interrupting it.
        let in_paragraph = self
            .last_open_child(container)
            .is_some_and(|child| matches!(self.kind(child), Some(Kind::Paragraph)));
        let inside_list = matches!(self.kind(container), Some(Kind::List(_)));
        if in_paragraph && !inside_list {
            if parsed.blank_after {
                return None;
            }
            // Where paragraphs are greedy, a fresh list never interrupts one (that is suppressed
            // before the marker is read), so this branch is reached only for a marker indented into
            // an item body — a sublist, which opens regardless of its start enumerator.
            let decimal_one = matches!(parsed.style, ListNumberStyle::Decimal) && parsed.start == 1;
            if !self.greedy_paragraphs && !parsed.bullet && !decimal_one {
                return None;
            }
        }

        cursor.advance_chars(parsed.marker_width);
        let after_marker = cursor.indent();
        // 1–4 spaces after the marker all count toward the content indent; 5+ collapse to one and
        // the rest become leading indentation of the item's content (an indented code block).
        let content_offset = if parsed.blank_after || after_marker > TAB_STOP {
            1
        } else {
            after_marker
        };
        if !parsed.blank_after {
            cursor.advance_columns(content_offset);
        }
        let item_indent = marker_indent + parsed.marker_width + content_offset;

        let list_index = self.ensure_list(container, &parsed);
        let item = Node::new(Kind::Item(ItemInfo {
            indent: item_indent,
            example_label: parsed.example_label.clone(),
        }));
        Some(self.append_child(list_index, item))
    }

    /// Reuse the matching open list for this marker, else start a new one. `container` may itself
    /// be the open list (a continuing item) or the list's parent (a fresh or restarted list).
    fn ensure_list(&mut self, container: usize, parsed: &ListMarkerParse) -> usize {
        if self.list_matches(container, parsed) {
            return container;
        }
        let info = list_info(parsed);
        let parent = self.place(container, &Kind::List(info.clone()));
        if let Some(last) = self.last_open_child(parent) {
            if self.list_matches(last, parsed) {
                return last;
            }
            if matches!(self.kind(last), Some(Kind::List(_))) {
                self.close(last);
            }
        }
        // A lone `i`/`I` enumerator is ambiguous between roman one and the ninth letter. When the
        // new list directly follows another list it reads as alphabetic; opening a document or
        // following any other block it reads as roman (the value chosen by `parse_enum_body`).
        let info = if self.preceding_is_list(parent) {
            demote_lone_roman(info)
        } else {
            info
        };
        self.append_child(parent, Node::new(Kind::List(info)))
    }

    /// Whether `parent`'s last child — open or closed — is a list, so a new sibling list abuts it.
    fn preceding_is_list(&self, parent: usize) -> bool {
        self.nodes
            .get(parent)
            .and_then(|node| node.children.last().copied())
            .is_some_and(|child| matches!(self.kind(child), Some(Kind::List(_))))
    }

    fn list_matches(&self, index: usize, parsed: &ListMarkerParse) -> bool {
        let Some(Kind::List(info)) = self.kind(index) else {
            return false;
        };
        if info.bullet != parsed.bullet {
            return false;
        }
        if info.bullet {
            // Bullet lists group by the marker character: switching `-`/`+`/`*` starts a new list.
            return info.marker == parsed.marker;
        }
        // A `#` placeholder continues any ordered list, adopting the list's own style and delimiter.
        if parsed.hash {
            return true;
        }
        // Ordered lists group by delimiter and by whether this marker reads as a continuation of the
        // list's established number style.
        info.delim == parsed.delim && continues_ordered(info.style, parsed)
    }

    fn add_line(&mut self, container: usize, started_new: bool, blank: bool, cursor: &mut Cursor) {
        if blank {
            let deepest = self.deepest_open(container);
            if matches!(self.kind(deepest), Some(Kind::Paragraph)) {
                self.close(deepest);
            }
            return;
        }

        if started_new {
            // The opener attached its own leaf; only a freshly-opened paragraph or container
            // (block quote / list item with trailing content) still needs this line's text.
            let leaf = self.deepest_open(container);
            match self.kind(leaf) {
                Some(Kind::Paragraph) => {
                    self.note_paragraph_indent(leaf, cursor);
                    self.append_line(leaf, cursor);
                }
                Some(
                    Kind::Heading(_)
                    | Kind::ThematicBreak
                    | Kind::LineBlock
                    | Kind::TextTable
                    | Kind::IndentedCode
                    | Kind::FencedCode(_)
                    | Kind::HtmlBlock(_)
                    | Kind::RawTex { .. },
                ) => {}
                _ => {
                    // An opener whose own line carries no content (a bare marker) leaves its
                    // container empty rather than seeding an empty paragraph.
                    if !cursor.remaining().trim().is_empty() {
                        let index = self.append_child(leaf, Node::new(Kind::Paragraph));
                        self.note_paragraph_indent(index, cursor);
                        self.append_line(index, cursor);
                    }
                }
            }
            return;
        }

        // No new block opened: continue an open paragraph, lazily crossing any unmatched
        // containers, or start a fresh paragraph in the matched container.
        let deepest = self.deepest_open(0);
        if matches!(self.kind(deepest), Some(Kind::Paragraph)) {
            self.append_line(deepest, cursor);
            return;
        }

        let parent = self.place(container, &Kind::Paragraph);
        let index = self.append_child(parent, Node::new(Kind::Paragraph));
        self.note_paragraph_indent(index, cursor);
        self.append_line(index, cursor);
    }

    /// Record the column a freshly opened paragraph's first line began at, so a dash ruling on the
    /// following line can read a headed table's alignment against the header's true position. Only an
    /// empty paragraph is its first line; a continuation must not overwrite the recorded column.
    fn note_paragraph_indent(&mut self, index: usize, cursor: &Cursor) {
        let indent = cursor.noted_indent();
        if let Some(node) = self.nodes.get_mut(index)
            && node.text.is_empty()
        {
            node.indent = indent;
        }
    }

    fn append_text(&mut self, index: usize, text: &str) {
        if let Some(node) = self.nodes.get_mut(index) {
            node.text.push_str(text);
        }
    }

    fn append_line(&mut self, index: usize, cursor: &Cursor) {
        if let Some(node) = self.nodes.get_mut(index) {
            node.text.push_str(cursor.remaining());
            node.text.push('\n');
        }
    }

    fn finish(mut self) -> (Vec<IrBlock>, RefMap, FootnoteDefs, ExampleMap) {
        // Pre-pass: pull link reference definitions out of every paragraph.
        for index in 0..self.nodes.len() {
            if matches!(self.kind(index), Some(Kind::Paragraph)) {
                let column_zero = self.nodes.get(index).is_some_and(|node| node.indent == 0);
                let consumed = self.extract_leading_definitions(index, column_zero);
                if consumed > 0
                    && let Some(node) = self.nodes.get_mut(index)
                {
                    node.text.drain(..consumed);
                }
            }
        }
        let mut footnotes = self.collect_footnotes();
        for blocks in footnotes.values_mut() {
            attach_table_captions(blocks, self.extensions);
        }
        let examples = self.number_examples();
        let mut blocks = self.build_children(0);
        attach_table_captions(&mut blocks, self.extensions);
        (blocks, self.refs, footnotes, examples)
    }

    /// Number every example-list item in one document-wide sequence and record the label→number map.
    /// Each example list's `start` becomes its first item's number; the map drives `@label`
    /// references during the inline phase.
    fn number_examples(&mut self) -> ExampleMap {
        let mut counter = 0;
        let mut map = ExampleMap::new();
        self.number_examples_in(0, &mut counter, &mut map);
        map
    }

    /// Walk `index` and its descendants in document order, assigning example numbers. Items are
    /// visited before the content nested beneath them, so a nested example list continues the same
    /// sequence in reading order.
    fn number_examples_in(&mut self, index: usize, counter: &mut i32, map: &mut ExampleMap) {
        let Some(node) = self.nodes.get(index) else {
            return;
        };
        let is_example =
            matches!(&node.kind, Kind::List(info) if info.style == ListNumberStyle::Example);
        let children = node.children.clone();
        if !is_example {
            for child in children {
                self.number_examples_in(child, counter, map);
            }
            return;
        }
        let mut start = None;
        for item in children {
            let Some(item_node) = self.nodes.get(item) else {
                continue;
            };
            let Kind::Item(info) = &item_node.kind else {
                continue;
            };
            let label = info.example_label.clone();
            let item_children = item_node.children.clone();
            start.get_or_insert(next_example_number(label, counter, map));
            for child in item_children {
                self.number_examples_in(child, counter, map);
            }
        }
        if let Some(start) = start
            && let Some(node) = self.nodes.get_mut(index)
            && let Kind::List(info) = &mut node.kind
        {
            info.start = start;
        }
    }

    /// Gather every footnote definition's block content, keyed by its normalized label. Definitions
    /// are visited in document order (node-creation order); when a label repeats, the first wins.
    fn collect_footnotes(&self) -> FootnoteDefs {
        let mut footnotes = FootnoteDefs::new();
        for index in 0..self.nodes.len() {
            if let Some(Kind::FootnoteDef(key)) = self.kind(index)
                && !footnotes.contains_key(key)
            {
                footnotes.insert(key.clone(), self.build_children(index));
            }
        }
        footnotes
    }

    fn node_text(&self, index: usize) -> String {
        self.nodes
            .get(index)
            .map(|node| node.text.clone())
            .unwrap_or_default()
    }

    /// Borrow a node's accumulated text without copying. Continuation checks that only inspect the
    /// buffer use this; callers that mutate the node afterward take the owned [`Self::node_text`].
    fn node_text_ref(&self, index: usize) -> Option<&str> {
        self.nodes.get(index).map(|node| node.text.as_str())
    }

    /// Pull leading definitions off `text` and return what remains. Link reference definitions are
    /// always eligible; an abbreviation definition (`abbreviations`) requires its host paragraph to
    /// begin flush at the container's left edge, so `column_zero` gates it.
    fn extract_refs(&mut self, text: &str, column_zero: bool) -> String {
        let abbreviations = column_zero && self.extensions.contains(Extension::Abbreviations);
        let mut remaining = text;
        loop {
            if let Some((label, def, rest)) =
                scan::parse_link_reference_definition(remaining, self.greedy_paragraphs)
            {
                self.refs.entry(label).or_insert(def);
                remaining = rest;
                continue;
            }
            if abbreviations && let Some(rest) = scan::parse_abbreviation_definition(remaining) {
                remaining = rest;
                continue;
            }
            break;
        }
        remaining.to_owned()
    }

    /// Register the definitions that lead a paragraph's text and return the byte length they occupy,
    /// so the caller can strip them from the node's buffer in place. Behaves as [`Self::extract_refs`]
    /// but reports the consumed prefix length rather than copying the remainder.
    fn extract_leading_definitions(&mut self, index: usize, column_zero: bool) -> usize {
        let abbreviations = column_zero && self.extensions.contains(Extension::Abbreviations);
        let greedy = self.greedy_paragraphs;
        let Some(node) = self.nodes.get(index) else {
            return 0;
        };
        let text = node.text.as_str();
        let mut remaining = text;
        loop {
            if let Some((label, def, rest)) =
                scan::parse_link_reference_definition(remaining, greedy)
            {
                self.refs.entry(label).or_insert(def);
                remaining = rest;
                continue;
            }
            if abbreviations && let Some(rest) = scan::parse_abbreviation_definition(remaining) {
                remaining = rest;
                continue;
            }
            break;
        }
        text.len() - remaining.len()
    }

    fn build_children(&self, index: usize) -> Vec<IrBlock> {
        let Some(node) = self.nodes.get(index) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &child in &node.children {
            // A raw HTML element contributes three blocks — its open tag, its parsed content, and its
            // close tag — flattened into this list rather than nested under a single block.
            if let Some(Kind::HtmlElement(info)) = self.kind(child)
                && !info.as_div
            {
                out.push(IrBlock::RawHtml(info.raw_open.clone()));
                let mut content = self.build_children(child);
                if info.tighten_last {
                    tighten_last_block(&mut content);
                }
                out.append(&mut content);
                // An element left open at end of input has no close tag; emit one only when present.
                if !info.raw_close.is_empty() {
                    out.push(IrBlock::RawHtml(info.raw_close.clone()));
                }
                continue;
            }
            if let Some(block) = self.build_block(child) {
                out.push(block);
            }
        }
        out
    }

    fn build_block(&self, index: usize) -> Option<IrBlock> {
        let node = self.nodes.get(index)?;
        let block = match &node.kind {
            // A footnote definition is lifted out of the body into the footnote map; its former
            // container stays but empties. The two grouping levels of a definition list carry no
            // block of their own — the list is built from its items, each item from its definitions.
            Kind::Document
            | Kind::Item(_)
            | Kind::FootnoteDef(_)
            | Kind::DefinitionItem { .. }
            | Kind::Definition { .. } => return None,
            Kind::Paragraph => {
                let trimmed = node.text.trim();
                if trimmed.is_empty() {
                    return None;
                }
                if self.extensions.contains(Extension::GridTables)
                    && let Some(table) = grid::parse(trimmed)
                {
                    IrBlock::GridTable(Box::new(table))
                } else if self.extensions.contains(Extension::PipeTables)
                    && let Some((alignments, header, rows)) =
                        table::try_parse(trimmed, self.greedy_paragraphs)
                {
                    IrBlock::Table {
                        alignments,
                        header,
                        rows,
                        caption: None,
                        attr: Attr::default(),
                    }
                } else if node.as_plain {
                    IrBlock::Plain(trimmed.to_owned())
                } else {
                    IrBlock::Para(trimmed.to_owned())
                }
            }
            Kind::LineBlock => IrBlock::LineBlock(line_block_lines(&node.text)),
            Kind::TextTable => {
                let lines = split_table_lines(&node.text);
                let (table, _) = texttable::parse(&lines, self.extensions)?;
                IrBlock::TextTable(Box::new(table))
            }
            Kind::DefinitionList => self.build_definition_list(index),
            Kind::Heading(level) => IrBlock::Heading(*level, node.text.trim().to_owned()),
            Kind::ThematicBreak => IrBlock::ThematicBreak,
            Kind::IndentedCode => {
                IrBlock::CodeBlock(Attr::default(), strip_trailing_blank_lines(&node.text))
            }
            Kind::FencedCode(fence) => {
                // A closing fence drops the final newline; a block ended by end-of-input (still
                // open) keeps it.
                let text = if node.open {
                    node.text.clone()
                } else {
                    strip_one_trailing_newline(&node.text)
                };
                // An info string that is exactly `{=FORMAT}` marks the fence's contents as raw
                // output for FORMAT, emitted verbatim rather than as a code block.
                if self.extensions.contains(Extension::RawAttribute)
                    && let Some(format) = raw_block_format(&fence.info)
                {
                    IrBlock::RawBlock(Format(format.into()), text)
                } else {
                    IrBlock::CodeBlock(fence_attr(&fence.info, self.extensions), text)
                }
            }
            Kind::HtmlBlock(kind) => IrBlock::RawHtml(self.finalize_html_block(node, *kind)),
            Kind::RawHtmlSpan { .. } => {
                IrBlock::RawHtml(node.text.trim_end_matches('\n').to_owned())
            }
            Kind::RawTex { .. } => {
                // A closed environment is verbatim raw TeX. One left open at end of input never
                // found its `\end`, so its lines are ordinary text: they fall back to a paragraph.
                if node.open {
                    let trimmed = node.text.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    IrBlock::Para(trimmed.to_owned())
                } else {
                    IrBlock::RawBlock(Format("tex".into()), node.text.clone())
                }
            }
            Kind::BlockQuote => {
                if self.extensions.contains(Extension::Alerts)
                    && let Some(alert) = self.build_alert(index)
                {
                    alert
                } else {
                    IrBlock::BlockQuote(self.build_children(index))
                }
            }
            Kind::FencedDiv(info) => IrBlock::Div(info.attr.clone(), self.build_children(index)),
            Kind::HtmlElement(info) => {
                // The raw form is emitted as three blocks (open tag, content, close tag), spliced
                // into the parent by `build_children`; only the div form is a single block here.
                if !info.as_div {
                    return None;
                }
                let mut children = self.build_children(index);
                if info.tighten_last {
                    tighten_last_block(&mut children);
                }
                IrBlock::Div(info.attr.clone(), children)
            }
            Kind::List(info) => self.build_list(index, info),
        };
        Some(block)
    }

    /// Finalize a raw HTML block's text. The markdown dialect drops a block's final newline; the
    /// strict dialect instead pads an unterminated kind 1–5 block, which closes only on an explicit
    /// end tag, so reaching end-of-input with it still open surfaces as a trailing blank line.
    fn finalize_html_block(&self, node: &Node, kind: u8) -> String {
        let mut text = node.text.clone();
        if self.greedy_paragraphs {
            if text.ends_with('\n') {
                text.pop();
            }
        } else if node.open && matches!(kind, 1..=5) {
            text.push('\n');
        }
        text
    }

    /// A blockquote whose first content line is exactly an alert marker `[!TYPE]` (with `TYPE` one of
    /// the recognized kinds, and nothing but trailing whitespace after the `]`) becomes a `Div`
    /// classed by the lowercased type. The broad Markdown dialect requires the uppercase spelling;
    /// the `CommonMark` engine accepts any casing. Its first child is a titled `Div` holding the
    /// type's display name; the marker line is stripped from the quote's first paragraph, and the
    /// rest of the quote's content follows. Returns `None` when the first line is not a clean marker,
    /// leaving the blockquote as an ordinary `BlockQuote`.
    fn build_alert(&self, index: usize) -> Option<IrBlock> {
        let node = self.nodes.get(index)?;
        let &first = node.children.first()?;
        let first_node = self.nodes.get(first)?;
        if !matches!(first_node.kind, Kind::Paragraph) {
            return None;
        }
        // The marker must occupy the paragraph's first line, with no leading whitespace and only
        // trailing whitespace after the closing bracket; inspecting the raw (untrimmed) text keeps
        // a leading space — which disables the marker — visible.
        let (marker_line, rest_of_para) = match first_node.text.split_once('\n') {
            Some((line, rest)) => (line, Some(rest)),
            None => (first_node.text.as_str(), None),
        };
        // Known limitation: a marker indented two or more columns inside the quote (e.g. `>  [!NOTE]`)
        // is not an alert, but the block phase has already folded that insignificant paragraph indent
        // away by this point, so the marker still reads as clean here. Markers at zero or one column
        // — the conventional spelling — are classified correctly.
        let alert_type = alert_marker_type(marker_line, self.greedy_paragraphs)?;

        let title = IrBlock::Div(
            Attr {
                id: carta_ast::Text::default(),
                classes: vec!["title".into()],
                attributes: Vec::new(),
            },
            vec![IrBlock::Para(alert_type.title.to_owned())],
        );

        let mut content = vec![title];
        // Anything left on the marker's own paragraph after dropping its first line stays a paragraph.
        if let Some(rest) = rest_of_para {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                content.push(IrBlock::Para(trimmed.to_owned()));
            }
        }
        for &child in node.children.iter().skip(1) {
            if let Some(block) = self.build_block(child) {
                content.push(block);
            }
        }

        Some(IrBlock::Div(
            Attr {
                id: carta_ast::Text::default(),
                classes: vec![alert_type.class.into()],
                attributes: Vec::new(),
            },
            content,
        ))
    }

    /// Build a definition list from its item and definition containers. Each item contributes its
    /// term text and the block content of each `:`/`~` definition; a tight item demotes its
    /// definitions' top-level paragraphs to `Plain`.
    fn build_definition_list(&self, index: usize) -> IrBlock {
        let mut items = Vec::new();
        if let Some(list) = self.nodes.get(index) {
            for &item_index in &list.children {
                let Some(Kind::DefinitionItem { term, tight }) = self.kind(item_index) else {
                    continue;
                };
                let mut definitions = Vec::new();
                if let Some(item) = self.nodes.get(item_index) {
                    for &def_index in &item.children {
                        let mut blocks = self.build_children(def_index);
                        if *tight {
                            demote_loose_paragraphs(&mut blocks);
                        }
                        definitions.push(blocks);
                    }
                }
                items.push(IrDefItem {
                    term: term.clone(),
                    definitions,
                });
            }
        }
        IrBlock::DefinitionList(items)
    }

    fn build_list(&self, index: usize, info: &ListInfo) -> IrBlock {
        let tight = self.list_is_tight(index);
        let mut items = Vec::new();
        if let Some(node) = self.nodes.get(index) {
            for &item in &node.children {
                let mut blocks = self.build_children(item);
                if tight {
                    demote_loose_paragraphs(&mut blocks);
                }
                items.push(blocks);
            }
        }
        if info.bullet {
            IrBlock::BulletList(items)
        } else {
            // The Markdown dialect honors an ordered list's start number only when the `startnum`
            // extension is enabled; with it disabled every ordered list begins at 1. CommonMark and
            // GFM have no such extension and always honor the start number.
            let start = if self.greedy_paragraphs && !self.extensions.contains(Extension::Startnum)
            {
                1
            } else {
                info.start
            };
            // Without fancy lists the greedy Markdown dialect does not distinguish enumerator styles
            // or delimiters: every ordered list carries the default style and delimiter.
            let (style, delim) =
                if self.greedy_paragraphs && !self.extensions.contains(Extension::FancyLists) {
                    (ListNumberStyle::DefaultStyle, ListNumberDelim::DefaultDelim)
                } else {
                    (info.style, info.delim)
                };
            let attrs = ListAttributes {
                start,
                style,
                delim,
            };
            IrBlock::OrderedList(attrs, items)
        }
    }

    /// A list is tight unless a blank line separates two of its items, or separates two blocks
    /// within an item (`CommonMark` §5.3).
    fn list_is_tight(&self, index: usize) -> bool {
        let Some(list) = self.nodes.get(index) else {
            return true;
        };
        for (position, &item) in list.children.iter().enumerate() {
            let has_next_item = position + 1 < list.children.len();
            if has_next_item && self.ends_with_blank_line(item) {
                return false;
            }
            if let Some(item_node) = self.nodes.get(item) {
                for (child_position, &child) in item_node.children.iter().enumerate() {
                    let has_next = has_next_item || child_position + 1 < item_node.children.len();
                    if has_next && self.ends_with_blank_line(child) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Whether `index` (or the tail of its last-child chain through lists and items) was followed
    /// by a blank line.
    fn ends_with_blank_line(&self, mut index: usize) -> bool {
        loop {
            let Some(node) = self.nodes.get(index) else {
                return false;
            };
            if node.last_line_blank {
                return true;
            }
            if matches!(node.kind, Kind::List(_) | Kind::Item(_)) {
                match node.children.last() {
                    Some(&last) => index = last,
                    None => return false,
                }
            } else {
                return false;
            }
        }
    }
}
#[cfg(test)]
#[path = "block_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "block_dialect_tests.rs"]
mod dialect_tests;

#[cfg(test)]
#[path = "block_abbreviation_tests.rs"]
mod abbreviation_tests;

#[cfg(test)]
#[path = "block_fence_interrupt_tests.rs"]
mod fence_interrupt_tests;

#[cfg(test)]
#[path = "block_raw_html_span_tests.rs"]
mod raw_html_span_tests;

#[cfg(test)]
#[path = "html_element_tests.rs"]
mod html_element_tests;
