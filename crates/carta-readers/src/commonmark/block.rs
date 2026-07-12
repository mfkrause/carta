//! Block-structure phase: consume input line by line into a tree of [`IrBlock`]s, following the
//! `CommonMark` spec's open-block algorithm (spec appendix "A parsing strategy"). Leaf text is left
//! raw for the inline phase; link reference definitions are stripped from paragraph fronts and
//! collected into the [`RefMap`].

use carta_ast::{Attr, Format, ListAttributes, ListNumberDelim, ListNumberStyle};
use carta_core::{Extension, Extensions};

use super::cursor::{Cursor, FenceInfo, ListMarkerParse};
use super::{
    ExampleMap, FootnoteDefs, IrBlock, IrDefItem, RefMap, TAB_STOP, attr, grid, html_block, scan,
    table, texttable,
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
    for index in 0..lines.len() {
        let line = lines.get(index).copied().unwrap_or("");
        let following = lines.get(index + 1..).unwrap_or(&[]);
        parser.process_line(line, following);
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
struct ListInfo {
    bullet: bool,
    marker: u8,
    style: ListNumberStyle,
    delim: ListNumberDelim,
    start: i32,
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
}

impl Parser {
    fn new(extensions: Extensions, greedy_paragraphs: bool) -> Self {
        Parser {
            nodes: vec![Node::new(Kind::Document)],
            refs: RefMap::new(),
            extensions,
            greedy_paragraphs,
            fence_fold: None,
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
    fn process_line(&mut self, line: &str, following: &[&str]) {
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
                if let Some(opened) = self.try_open(container, &mut cursor, following) {
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
                self.process_line(&trailing, &[]);
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
            self.process_line(&trailing, &[]);
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
            self.process_line(&rest, &[]);
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
            self.process_line(line, following);
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
            self.process_line(before, &[]);
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
            self.process_line(&after, &[]);
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
                self.process_line(&rest, &[]);
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
    fn fence_reaches_close(&self, container: usize, fence: &FenceInfo, following: &[&str]) -> bool {
        if !self.greedy_paragraphs {
            return true;
        }
        let path = self.container_path(container);
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
                    && self.fence_reaches_close(container, &fence, following)
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

/// Tighten an HTML element's final content block from `Para` to `Plain` when no blank line
/// separated it from the close tag.
fn tighten_last_block(blocks: &mut [IrBlock]) {
    if let Some(block) = blocks.last_mut()
        && let IrBlock::Para(text) = block
    {
        *block = IrBlock::Plain(std::mem::take(text));
    }
}

/// In a tight list, item paragraphs render as `Plain` rather than `Para`.
pub(crate) fn demote_loose_paragraphs(blocks: &mut [IrBlock]) {
    for block in blocks {
        if let IrBlock::Para(text) = block {
            *block = IrBlock::Plain(std::mem::take(text));
        }
    }
}

/// Attach table captions: a paragraph led by `Table:`, `table:`, or `:` becomes the caption of the
/// table — pipe, dash-ruled, or grid — immediately before it, or, failing that, immediately after
/// it. The caption attaches to the nearer uncaptioned table and is removed from the block list; with
/// no such table it stays an ordinary paragraph. Working in document order, a caption above a table
/// is reached first, so it wins over one below. The pass recurses into nested block containers first.
fn attach_table_captions(blocks: &mut Vec<IrBlock>, ext: Extensions) {
    for block in blocks.iter_mut() {
        match block {
            IrBlock::Div(_, children) | IrBlock::BlockQuote(children) => {
                attach_table_captions(children, ext);
            }
            IrBlock::BulletList(items) | IrBlock::OrderedList(_, items) => {
                for item in items {
                    attach_table_captions(item, ext);
                }
            }
            IrBlock::DefinitionList(items) => {
                for item in items {
                    for definition in &mut item.definitions {
                        attach_table_captions(definition, ext);
                    }
                }
            }
            _ => {}
        }
    }
    if !ext.contains(Extension::TableCaptions) {
        return;
    }
    let mut i = 0;
    while i < blocks.len() {
        let Some(caption) = caption_text(blocks.get(i)) else {
            i += 1;
            continue;
        };
        let attached = (i >= 1 && set_table_caption(blocks, i - 1, &caption, ext))
            || (i + 1 < blocks.len() && set_table_caption(blocks, i + 1, &caption, ext));
        if attached {
            blocks.remove(i);
        } else {
            i += 1;
        }
    }
}

/// The caption text of a paragraph block led by a `Table:`/`table:`/`:` marker, with the marker
/// stripped; `None` for any other block.
fn caption_text(block: Option<&IrBlock>) -> Option<String> {
    let IrBlock::Para(text) = block? else {
        return None;
    };
    let (first, rest) = match text.split_once('\n') {
        Some((first, rest)) => (first, Some(rest)),
        None => (text.as_str(), None),
    };
    let body = strip_caption_marker(first)?;
    Some(match rest {
        Some(rest) => format!("{body}\n{rest}"),
        None => body.to_owned(),
    })
}

/// Strip a leading `Table:`, `table:`, or `:` caption marker and the spaces after it, returning the
/// remaining first-line text; `None` when no marker is present. Only the marker's first letter may
/// vary in case, so `TABLE:` is not a marker.
fn strip_caption_marker(first: &str) -> Option<&str> {
    for marker in ["Table:", "table:"] {
        if let Some(rest) = first.strip_prefix(marker) {
            return Some(rest.trim_start());
        }
    }
    first.strip_prefix(':').map(str::trim_start)
}

/// Set `text` as the caption of the table at `index`, if that block is a pipe, dash-ruled, or grid
/// table that has no caption yet. Returns whether the caption was attached. With
/// [`Extension::TableAttributes`] enabled, a trailing `{…}` attribute block on the caption is split
/// off and applied to the table's outer attributes; the remaining text becomes the caption.
fn set_table_caption(blocks: &mut [IrBlock], index: usize, text: &str, ext: Extensions) -> bool {
    let (caption_slot, attr_slot) = match blocks.get_mut(index) {
        Some(IrBlock::Table { caption, attr, .. }) => (caption, attr),
        Some(IrBlock::TextTable(table)) => (&mut table.caption, &mut table.attr),
        Some(IrBlock::GridTable(table)) => (&mut table.caption, &mut table.attr),
        _ => return false,
    };
    if caption_slot.is_some() {
        return false;
    }
    let (body, parsed) = if ext.contains(Extension::TableAttributes) {
        split_trailing_attr(text)
    } else {
        (text, None)
    };
    *caption_slot = Some(body.to_owned());
    if let Some(parsed) = parsed {
        *attr_slot = parsed;
    }
    true
}

/// Split a trailing `{…}` attribute block off the end of a caption. Returns the caption text with the
/// block and any whitespace before it removed, alongside the parsed attributes. When the text has no
/// well-formed trailing attribute block, the text is returned unchanged with `None`.
fn split_trailing_attr(text: &str) -> (&str, Option<Attr>) {
    let trimmed = text.trim_end();
    if !trimmed.ends_with('}') {
        return (text, None);
    }
    // The trailing block opens at some `{`; find the one whose attribute parse consumes exactly to
    // the end. Earlier `{` characters stay part of the caption text.
    for (open, _) in trimmed.char_indices().filter(|&(_, ch)| ch == '{') {
        if let Some(rest) = trimmed.get(open..)
            && let Some((attr, consumed)) = attr::parse_attributes(rest)
            && consumed == rest.len()
        {
            let body = trimmed.get(..open).map_or("", str::trim_end);
            return (body, Some(attr));
        }
    }
    (text, None)
}

enum Continue {
    Matched,
    MatchedLeaf,
    NotMatched,
}

/// Math environments are rendered inline rather than as block-level raw TeX, so a `\begin` opening
/// one is not a block environment. The base name is matched exactly; a single trailing `*` (the
/// unnumbered variant) counts as the same environment.
fn is_math_environment(name: &str) -> bool {
    const MATH_ENVS: &[&str] = &[
        "equation",
        "align",
        "gather",
        "multline",
        "eqnarray",
        "flalign",
        "alignat",
        "displaymath",
        "math",
        "dmath",
    ];
    let base = name.strip_suffix('*').unwrap_or(name);
    MATH_ENVS.contains(&base)
}

/// If `s` begins with `\<keyword>` (optionally followed by spaces) then a braced `{name}`, return
/// the literal brace content. The leading backslash must not itself be escaped — callers pass the
/// raw slice from a line start, where this holds. The brace content runs to the first `}` and may be
/// empty; it is compared exactly elsewhere, so inner spaces are significant.
fn raw_tex_env_name(s: &str, keyword: &[u8]) -> Option<String> {
    let after_backslash = s.strip_prefix('\\')?;
    let after_keyword = after_backslash.strip_prefix(std::str::from_utf8(keyword).ok()?)?;
    let after_spaces = after_keyword.trim_start_matches(' ');
    let body = after_spaces.strip_prefix('{')?;
    let close = body.find('}')?;
    body.get(..close).map(str::to_owned)
}

/// Scan one source `line` of an open environment named `name`, starting at nesting `depth`. Returns
/// the depth after the line and, when the environment's matching `\end{name}` is reached (depth back
/// to zero), the byte offset just past that `\end{...}`. Backslash escapes are honored: a `\\`
/// consumes both characters so an escaped command never counts toward the depth.
fn raw_tex_scan(line: &str, name: &str, mut depth: usize) -> (usize, Option<usize>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes.get(i) != Some(&b'\\') {
            i += 1;
            continue;
        }
        // An escaped backslash consumes both bytes and starts no command.
        if bytes.get(i + 1) == Some(&b'\\') {
            i += 2;
            continue;
        }
        let rest = line.get(i..).unwrap_or("");
        if raw_tex_env_name(rest, b"begin").as_deref() == Some(name) {
            depth += 1;
            i += 1;
            continue;
        }
        if raw_tex_env_name(rest, b"end").as_deref() == Some(name) {
            depth = depth.saturating_sub(1);
            // Advance past this `\end{...}` so the close offset lands just after its brace.
            let end_off = rest.find('}').map_or(line.len(), |brace| i + brace + 1);
            if depth == 0 {
                return (0, Some(end_off));
            }
            i = end_off;
            continue;
        }
        i += 1;
    }
    (depth, None)
}

/// The number for an example item, given its `@label` (or `None` for the anonymous `@`). A new or
/// anonymous item advances the shared counter; a repeated label reuses its first number.
fn next_example_number(label: Option<String>, counter: &mut i32, map: &mut ExampleMap) -> i32 {
    if let Some(label) = &label
        && let Some(&number) = map.get(label)
    {
        return number;
    }
    *counter += 1;
    if let Some(label) = label {
        map.insert(label, *counter);
    }
    *counter
}

fn list_info(parsed: &ListMarkerParse) -> ListInfo {
    ListInfo {
        bullet: parsed.bullet,
        marker: parsed.marker,
        style: parsed.style,
        delim: parsed.delim,
        start: parsed.start,
    }
}

/// Reread a lone roman `i`/`I` (the only roman enumerator whose start is one) as the ninth letter
/// of its alphabet. Any other list info is returned unchanged.
fn demote_lone_roman(info: ListInfo) -> ListInfo {
    let style = match info.style {
        ListNumberStyle::LowerRoman if info.start == 1 => ListNumberStyle::LowerAlpha,
        ListNumberStyle::UpperRoman if info.start == 1 => ListNumberStyle::UpperAlpha,
        _ => return info,
    };
    ListInfo {
        style,
        start: 9,
        ..info
    }
}

/// Whether `marker` reads as a continuation of an ordered list whose established style is
/// `list_style` (the delimiter is checked separately). The list's first item fixes the style; each
/// later marker is reread in that style rather than its own:
///
/// - a decimal list takes only decimal markers;
/// - an alphabetic list takes any single letter of its case (so `h. i. j.` is one list, `i` read as
///   the ninth letter);
/// - a roman list takes any roman numeral of its case, plus the single letters whose position is a
///   roman value (`a`, `e`, `j`) — the same letters a roman sequence can reach.
fn continues_ordered(list_style: ListNumberStyle, marker: &ListMarkerParse) -> bool {
    use ListNumberStyle::{Decimal, LowerAlpha, LowerRoman, UpperAlpha, UpperRoman};
    let lower = matches!(marker.style, LowerAlpha | LowerRoman);
    let upper = matches!(marker.style, UpperAlpha | UpperRoman);
    match list_style {
        Decimal => matches!(marker.style, Decimal),
        LowerAlpha => lower && marker.single_letter,
        UpperAlpha => upper && marker.single_letter,
        LowerRoman => lower && continues_roman(marker),
        UpperRoman => upper && continues_roman(marker),
        // An example list groups every example marker of the same delimiter, regardless of label.
        ListNumberStyle::Example => matches!(marker.style, ListNumberStyle::Example),
        ListNumberStyle::DefaultStyle => false,
    }
}

/// Whether `marker` reads as a roman numeral continuing a roman list: a multi-letter roman, the lone
/// roman `i`/`I`, or a single letter whose alphabet position is itself a roman digit or a roman
/// value (`a`=1, `e`=5, `j`=10).
fn continues_roman(marker: &ListMarkerParse) -> bool {
    if !marker.single_letter {
        return matches!(
            marker.style,
            ListNumberStyle::LowerRoman | ListNumberStyle::UpperRoman
        );
    }
    matches!(
        marker.style,
        ListNumberStyle::LowerRoman | ListNumberStyle::UpperRoman
    ) || matches!(marker.start, 1 | 3 | 4 | 5 | 9 | 10 | 12 | 13 | 22 | 24)
}

/// If a fence's info string is exactly an attribute block holding only a raw-format marker — a `{`,
/// optional whitespace, `=`, a format name, optional whitespace, then `}` — return that name. The
/// fence's contents are then raw output for that format. A format name is one run of letters,
/// digits, `-`, or `_`; anything else (extra attributes, a space inside the name, a stray symbol)
/// is not a raw marker and the fence stays an ordinary code block.
fn raw_block_format(info: &str) -> Option<String> {
    let inner = info.trim().strip_prefix('{')?.strip_suffix('}')?;
    // The `=` immediately precedes the name: `{= html}` (a gap after `=`) is not a raw marker,
    // while surrounding whitespace (`{ =html }`) is allowed.
    let name = inner.trim_start().strip_prefix('=')?.trim_end();
    if name.is_empty() || !name.chars().all(is_format_name_char) {
        return None;
    }
    Some(name.to_owned())
}

pub(super) fn is_format_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')
}

fn fence_attr(info: &str, extensions: Extensions) -> Attr {
    let info = info.trim();
    if info.is_empty() {
        return Attr::default();
    }
    // With fenced-code attributes enabled, a `{…}` info string is a full attribute block; the whole
    // info must be the block, else it falls back to the bare-language reading.
    if (extensions.contains(Extension::FencedCodeAttributes)
        || extensions.contains(Extension::Attributes))
        && info.starts_with('{')
        && let Some((parsed, consumed)) = attr::parse_attributes(info)
        && info
            .get(consumed..)
            .is_some_and(|rest| rest.trim().is_empty())
    {
        return parsed;
    }
    let language = info.split_whitespace().next().unwrap_or("");
    Attr {
        id: carta_ast::Text::default(),
        classes: vec![language.into()],
        attributes: Vec::new(),
    }
}

/// If `line` (already past any container markers and leading indent) opens a fenced div — a run of
/// three or more colons followed by a valid attribute spec — return the colon count and the parsed
/// attributes. A bare colon run with no spec is not an opener (it can only close).
fn div_open_fence(line: &str) -> Option<(usize, Attr)> {
    let after_colons = line.trim_start_matches(':');
    let count = line.len() - after_colons.len();
    if count < 3 {
        return None;
    }
    let attr = div_open_attr(after_colons.trim())?;
    Some((count, attr))
}

/// The recognized alert kinds: the marker spelling (recognized only in all-uppercase), the lowercased
/// class applied to the wrapping div, and the display title.
struct AlertType {
    class: &'static str,
    title: &'static str,
}

const ALERT_TYPES: &[(&str, AlertType)] = &[
    (
        "note",
        AlertType {
            class: "note",
            title: "Note",
        },
    ),
    (
        "tip",
        AlertType {
            class: "tip",
            title: "Tip",
        },
    ),
    (
        "important",
        AlertType {
            class: "important",
            title: "Important",
        },
    ),
    (
        "warning",
        AlertType {
            class: "warning",
            title: "Warning",
        },
    ),
    (
        "caution",
        AlertType {
            class: "caution",
            title: "Caution",
        },
    ),
];

/// If `line` is exactly an alert marker `[!TYPE]` followed by only trailing whitespace — with no
/// leading whitespace and a recognized `TYPE` — return its kind. The broad Markdown dialect
/// (`uppercase_only`) admits only the all-uppercase spelling `[!NOTE]`; the `CommonMark` engine
/// accepts any casing (`[!note]`, `[!Note]`).
fn alert_marker_type(line: &str, uppercase_only: bool) -> Option<&'static AlertType> {
    let inner = line.strip_prefix("[!")?;
    let close = inner.find(']')?;
    let name = inner.get(..close)?;
    // Only whitespace may follow the closing bracket.
    if !inner.get(close + 1..)?.chars().all(char::is_whitespace) {
        return None;
    }
    if uppercase_only && name.bytes().any(|b| b.is_ascii_lowercase()) {
        return None;
    }
    ALERT_TYPES
        .iter()
        .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
        .map(|(_, ty)| ty)
}

/// Parse a fenced-div opener's attribute spec (the text after the colons, already trimmed). It is
/// either a single brace block of valid attributes or a single bare word taken verbatim as the sole
/// class; anything else (empty, multiple words, junk after a brace) is not a valid opener.
fn div_open_attr(spec: &str) -> Option<Attr> {
    if spec.is_empty() {
        return None;
    }
    if spec.starts_with('{')
        && let Some((attr, consumed)) = attr::parse_attributes_first_id(spec)
        && attr::is_non_empty(&attr)
        && spec
            .get(consumed..)
            .is_some_and(|rest| rest.trim().is_empty())
    {
        return Some(attr);
    }
    // Bare-word form: a single whitespace-free token becomes the sole class, kept verbatim (a
    // leading dot is not stripped).
    if spec.chars().any(char::is_whitespace) {
        return None;
    }
    Some(Attr {
        id: carta_ast::Text::default(),
        classes: vec![spec.into()],
        attributes: Vec::new(),
    })
}

/// If `line` (already past any container markers) is a closing div fence — up to three spaces of
/// indent, then a run of three or more colons, then only whitespace — return the colon count.
fn div_close_fence(line: &str) -> Option<usize> {
    let after_spaces = line.trim_start_matches(' ');
    if line.len() - after_spaces.len() > 3 {
        return None;
    }
    let after_colons = after_spaces.trim_start_matches(':');
    let count = after_spaces.len() - after_colons.len();
    if count < 3 || !after_colons.trim().is_empty() {
        return None;
    }
    Some(count)
}

fn strip_one_trailing_newline(text: &str) -> String {
    text.strip_suffix('\n').unwrap_or(text).to_owned()
}

/// A line opens or extends a line block when a `|` sits at its start, followed by a space or the
/// line's end.
fn is_line_block_marker(line: &str) -> bool {
    line == "|" || line.starts_with("| ")
}

/// Whether `text` (a node's accumulated lines, each terminated by a newline) holds exactly one
/// non-empty line.
fn single_line(text: &str) -> bool {
    let body = text.strip_suffix('\n').unwrap_or(text);
    !body.is_empty() && !body.contains('\n')
}

/// Split an accumulated table leaf's text into its physical lines, dropping the trailing empty piece
/// left by the final newline.
fn split_table_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn owned_lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

/// The last non-blank physical line of an accumulated leaf's text, scanning back from the end so the
/// cost is the length of that line rather than the whole accumulation.
fn last_nonempty_line(text: &str) -> &str {
    text.trim_end_matches('\n')
        .rsplit('\n')
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

/// Whether a dash-only line is a thematic break: three or more dashes, with spaces allowed between
/// them. Used to settle a dash-ruled table candidate that turned out not to be a table.
fn is_thematic_dash_line(line: &str) -> bool {
    texttable::is_dash_line(line) && line.bytes().filter(|byte| *byte == b'-').count() >= 3
}

/// Whether a line block's current (final) entry is empty: its last line is a `|` marker carrying no
/// content. A content-bearing line stays non-empty once written, so checking the final line alone is
/// enough — an empty entry is only ever followed by another marker line, never folded into.
fn last_entry_is_empty(text: &str) -> bool {
    let last = text
        .trim_end_matches('\n')
        .rsplit('\n')
        .next()
        .unwrap_or("");
    last.strip_prefix('|')
        .is_some_and(|rest| rest.trim_matches([' ', '\t']).is_empty())
}

/// Split a line block's accumulated raw lines into prepared per-entry strings. A `|`-led line opens
/// a new entry — its `|` and one following space dropped, any remaining leading spaces kept as
/// non-breaking spaces so they survive inline parsing — while any other line continues the previous
/// entry, joined to it by a single space.
fn line_block_lines(text: &str) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix('|') {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            // Trailing whitespace is dropped first, so an all-space entry collapses to empty
            // rather than to a run of preserved leading spaces.
            entries.push(preserve_leading_spaces(rest.trim_end_matches([' ', '\t'])));
        } else if let Some(last) = entries.last_mut() {
            last.push(' ');
            last.push_str(raw.trim());
        } else {
            entries.push(raw.trim().to_owned());
        }
    }
    // A whitespace-only continuation folds nothing into its entry but leaves a dangling separator
    // space; drop any such trailing run, leaving preserved leading spaces untouched.
    for entry in &mut entries {
        let kept = entry.trim_end_matches([' ', '\t']).len();
        entry.truncate(kept);
    }
    entries
}

/// Replace a run of leading ASCII spaces with non-breaking spaces.
fn preserve_leading_spaces(s: &str) -> String {
    let trimmed = s.trim_start_matches(' ');
    let spaces = s.len() - trimmed.len();
    let mut out = String::with_capacity(s.len() + spaces);
    for _ in 0..spaces {
        out.push('\u{a0}');
    }
    out.push_str(trimmed);
    out
}

/// Drop trailing whitespace-only lines (and the final line ending), keeping interior blank lines.
fn strip_trailing_blank_lines(text: &str) -> String {
    let mut lines: Vec<&str> = text.split('\n').collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Trim an ATX heading's content: drop surrounding spaces/tabs and an optional closing run of `#`
/// (which must be preceded by whitespace or form the whole line, else it belongs to the content).
fn strip_atx_closing(content: &str, require_preceding_space: bool) -> String {
    let trimmed = content.trim_matches([' ', '\t']);
    let without_hashes = trimmed.trim_end_matches('#');
    if without_hashes.len() == trimmed.len() {
        return trimmed.to_owned();
    }
    // A closing hash run always terminates the heading when the dialect does not require a space
    // after the opener; otherwise the run must be set off from the content by whitespace.
    if !require_preceding_space
        || without_hashes.is_empty()
        || without_hashes.ends_with([' ', '\t'])
    {
        without_hashes.trim_end_matches([' ', '\t']).to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Recognition of block-level HTML elements whose inner content is parsed as markdown: scanning an
/// open tag (name, attributes, extent) and locating its matching close tag. Pure functions over the
/// raw line text.
mod html_element {
    use carta_ast::Attr;

    /// A recognized open tag at the start of a line.
    pub(super) struct OpenTag {
        /// The lowercased tag name.
        pub(super) tag: String,
        /// Attributes parsed from the tag (`id`, `class`, and other key/values).
        pub(super) attr: Attr,
        /// Byte length of the whole tag, up to and including the closing `>`.
        pub(super) len: usize,
        /// Whether the tag closes itself (`<div/>`), so it opens no element to balance.
        pub(super) self_closing: bool,
    }

    /// A located close tag within a line.
    pub(super) struct CloseTag {
        /// Byte offset where `</` begins.
        pub(super) start: usize,
        /// Byte offset just past the closing `>`.
        pub(super) end: usize,
    }

    /// Block-level tag names whose elements carry parsed markdown content. Inline tags (`em`, `span`,
    /// `a`, …) and unrecognized names are left for the inline phase as raw HTML.
    const BLOCK_TAGS: &[&str] = &[
        "address",
        "article",
        "aside",
        "base",
        "basefont",
        "blockquote",
        "body",
        "caption",
        "center",
        "col",
        "colgroup",
        "dd",
        "details",
        "dialog",
        "dir",
        "div",
        "dl",
        "dt",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "frame",
        "frameset",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "head",
        "header",
        "hr",
        "html",
        "iframe",
        "legend",
        "li",
        "link",
        "main",
        "menu",
        "menuitem",
        "nav",
        "noframes",
        "ol",
        "optgroup",
        "option",
        "p",
        "param",
        "search",
        "section",
        "summary",
        "table",
        "tbody",
        "td",
        "tfoot",
        "th",
        "thead",
        "title",
        "tr",
        "track",
        "ul",
    ];

    fn is_block_tag(name: &str) -> bool {
        BLOCK_TAGS.contains(&name)
    }

    /// If `s` begins with a recognized block-level HTML open tag, return its name, attributes, and
    /// byte extent. A self-closing tag (`<div/>`) parses as an ordinary open tag here.
    pub(super) fn parse_open_tag(s: &str) -> Option<OpenTag> {
        let bytes = s.as_bytes();
        if bytes.first() != Some(&b'<') {
            return None;
        }
        let mut i = 1;
        let name_start = i;
        if !bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
            return None;
        }
        i += 1;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            i += 1;
        }
        let name = s.get(name_start..i)?.to_ascii_lowercase();
        if !is_block_tag(&name) {
            return None;
        }
        let mut attr = Attr::default();
        loop {
            let after_ws = skip_ws(bytes, i);
            // A `>` (optionally preceded by a self-closing `/`) ends the tag.
            let self_closing = bytes.get(after_ws) == Some(&b'/');
            let close = if self_closing { after_ws + 1 } else { after_ws };
            if bytes.get(close) == Some(&b'>') {
                return Some(OpenTag {
                    tag: name,
                    attr,
                    len: close + 1,
                    self_closing,
                });
            }
            // An attribute must be separated from the name (or a previous attribute) by whitespace.
            if after_ws == i {
                return None;
            }
            i = read_attribute(bytes, after_ws, &mut attr)?;
        }
    }

    /// Read one `name[=value]` attribute starting at `start`, folding it into `attr`, and return the
    /// index just past it. `id` sets the identifier, `class` adds whitespace-separated classes (the
    /// first `class` wins), and any other name becomes a key/value pair in source order.
    fn read_attribute(bytes: &[u8], start: usize, attr: &mut Attr) -> Option<usize> {
        let mut i = start;
        let name_start = i;
        if !bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'_' | b':'))
        {
            return None;
        }
        i += 1;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
        {
            i += 1;
        }
        let name = ascii_lower(bytes.get(name_start..i)?);
        let probe = skip_ws(bytes, i);
        let mut value = String::new();
        let mut end = i;
        if bytes.get(probe) == Some(&b'=') {
            let (val, next) = read_value(bytes, probe + 1)?;
            value = val;
            end = next;
        }
        match name.as_str() {
            "id" => attr.id = value.into(),
            "class" => {
                if attr.classes.is_empty() {
                    attr.classes = value.split_whitespace().map(Into::into).collect();
                }
            }
            _ => attr.attributes.push((name.into(), value.into())),
        }
        Some(end)
    }

    /// Read an attribute value (quoted or bare) starting at `start`, returning it and the index just
    /// past it. A started-but-unterminated value is malformed.
    fn read_value(bytes: &[u8], start: usize) -> Option<(String, usize)> {
        let i = skip_ws(bytes, start);
        match bytes.get(i) {
            Some(quote @ (b'"' | b'\'')) => {
                let quote = *quote;
                let value_start = i + 1;
                let mut j = value_start;
                while bytes.get(j).is_some_and(|b| *b != quote) {
                    j += 1;
                }
                if bytes.get(j) != Some(&quote) {
                    return None;
                }
                Some((bytes_to_string(bytes.get(value_start..j)?), j + 1))
            }
            Some(_) => {
                let value_start = i;
                let mut j = i;
                while bytes.get(j).is_some_and(|b| {
                    !matches!(b, b' ' | b'\t' | b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')
                }) {
                    j += 1;
                }
                if j == value_start {
                    return None;
                }
                Some((bytes_to_string(bytes.get(value_start..j)?), j))
            }
            None => None,
        }
    }

    /// Locate the first matching close tag `</name>` (with optional trailing whitespace before `>`)
    /// in `s`, returning its byte range. The name match is case-insensitive.
    pub(super) fn find_close_tag(s: &str, tag: &str) -> Option<CloseTag> {
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes.get(i) == Some(&b'<')
                && bytes.get(i + 1) == Some(&b'/')
                && let Some(end) = close_tag_at(bytes, i, tag)
            {
                return Some(CloseTag { start: i, end });
            }
            i += 1;
        }
        None
    }

    /// If a close tag for `tag` begins at `start`, return the index just past its `>`.
    fn close_tag_at(bytes: &[u8], start: usize, tag: &str) -> Option<usize> {
        let name_start = start + 2;
        let mut i = name_start;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            i += 1;
        }
        let name = ascii_lower(bytes.get(name_start..i)?);
        if name != tag {
            return None;
        }
        i = skip_ws(bytes, i);
        (bytes.get(i) == Some(&b'>')).then_some(i + 1)
    }

    /// If `s` begins with a block-level close tag (`</div>`, optional whitespace before `>`), return
    /// its byte length. A bare close tag at a line start stands alone as a raw block.
    pub(super) fn parse_close_tag(s: &str) -> Option<usize> {
        let bytes = s.as_bytes();
        if bytes.first() != Some(&b'<') || bytes.get(1) != Some(&b'/') {
            return None;
        }
        let name_start = 2;
        let mut i = name_start;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            i += 1;
        }
        let name = s.get(name_start..i)?.to_ascii_lowercase();
        if !is_block_tag(&name) {
            return None;
        }
        i = skip_ws(bytes, i);
        (bytes.get(i) == Some(&b'>')).then_some(i + 1)
    }

    /// Walk `s` from the given open-nesting `depth`, tracking only tags named `tag`: each non-self-
    /// closing open raises the depth, each close lowers it, and any other tag is skipped whole so a
    /// `>` inside its attributes cannot be miscounted. Returns the depth after `s` and, when a close
    /// brings the depth to zero within `s`, the byte offset just past that close tag.
    pub(super) fn scan_depth(s: &str, tag: &str, mut depth: usize) -> (usize, Option<usize>) {
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes.get(i) == Some(&b'<') {
                if let Some(open) = parse_open_tag(s.get(i..).unwrap_or("")) {
                    if open.tag == tag && !open.self_closing {
                        depth += 1;
                    }
                    i += open.len;
                    continue;
                }
                if let Some(end) = close_tag_at(bytes, i, tag) {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return (0, Some(end));
                    }
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
        (depth, None)
    }

    fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
        while bytes.get(i).is_some_and(|b| matches!(b, b' ' | b'\t')) {
            i += 1;
        }
        i
    }

    fn ascii_lower(bytes: &[u8]) -> String {
        bytes_to_string(bytes).to_ascii_lowercase()
    }

    fn bytes_to_string(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::strip_caption_marker;

    #[test]
    fn recognizes_the_three_caption_markers() {
        assert_eq!(strip_caption_marker("Table: A caption"), Some("A caption"));
        assert_eq!(strip_caption_marker("table: A caption"), Some("A caption"));
        assert_eq!(strip_caption_marker(": A caption"), Some("A caption"));
    }

    #[test]
    fn drops_the_spaces_after_the_marker() {
        assert_eq!(strip_caption_marker("Table:caption"), Some("caption"));
        assert_eq!(strip_caption_marker("Table:    caption"), Some("caption"));
        assert_eq!(strip_caption_marker(":caption"), Some("caption"));
    }

    #[test]
    fn only_the_first_letter_may_vary_in_case() {
        assert_eq!(strip_caption_marker("TABLE: x"), None);
        assert_eq!(strip_caption_marker("TAble: x"), None);
        assert_eq!(strip_caption_marker("tABLE: x"), None);
    }

    #[test]
    fn a_space_before_the_colon_is_not_a_marker() {
        assert_eq!(strip_caption_marker("Table : x"), None);
        assert_eq!(strip_caption_marker("table : x"), None);
    }

    #[test]
    fn a_line_without_a_marker_is_rejected() {
        assert_eq!(strip_caption_marker("Just a paragraph"), None);
        assert_eq!(strip_caption_marker("Tablexyz"), None);
    }

    use super::raw_block_format;

    #[test]
    fn plain_raw_format_marker_is_recognized() {
        assert_eq!(raw_block_format("{=html}"), Some("html".to_owned()));
        assert_eq!(raw_block_format("{=latex}"), Some("latex".to_owned()));
        assert_eq!(raw_block_format("{=html-foo}"), Some("html-foo".to_owned()));
        assert_eq!(raw_block_format("{=html_foo}"), Some("html_foo".to_owned()));
        assert_eq!(raw_block_format("{=html5}"), Some("html5".to_owned()));
    }

    #[test]
    fn whitespace_around_the_marker_is_tolerated() {
        assert_eq!(raw_block_format("{ =html}"), Some("html".to_owned()));
        assert_eq!(raw_block_format("{=html }"), Some("html".to_owned()));
        assert_eq!(raw_block_format("  {=html}  "), Some("html".to_owned()));
    }

    #[test]
    fn a_gap_after_the_equals_is_not_a_marker() {
        assert_eq!(raw_block_format("{= html}"), None);
    }

    #[test]
    fn extra_attributes_or_an_empty_format_are_not_markers() {
        assert_eq!(raw_block_format("{=html .foo}"), None);
        assert_eq!(raw_block_format("{=html foo}"), None);
        assert_eq!(raw_block_format("{=}"), None);
        assert_eq!(raw_block_format("{}"), None);
    }

    #[test]
    fn a_symbol_in_the_format_name_is_not_a_marker() {
        assert_eq!(raw_block_format("{=ht.ml}"), None);
        assert_eq!(raw_block_format("{=ht/ml}"), None);
        assert_eq!(raw_block_format("{=ht+ml}"), None);
        assert_eq!(raw_block_format("{=ht:ml}"), None);
    }

    #[test]
    fn an_ordinary_info_string_is_not_a_marker() {
        assert_eq!(raw_block_format("html"), None);
        assert_eq!(raw_block_format("=html"), None);
        assert_eq!(raw_block_format("{.html}"), None);
    }

    use super::{IrBlock, is_math_environment, parse, raw_tex_env_name, raw_tex_scan};
    use carta_ast::Format;
    use carta_core::presets;

    fn blocks(input: &str) -> Vec<IrBlock> {
        parse(input, presets::MARKDOWN, true).0
    }

    #[test]
    fn reads_the_begin_environment_name() {
        assert_eq!(
            raw_tex_env_name("\\begin{center}", b"begin").as_deref(),
            Some("center")
        );
        assert_eq!(
            raw_tex_env_name("\\begin {center}", b"begin").as_deref(),
            Some("center")
        );
        assert_eq!(
            raw_tex_env_name("\\end{a}rest", b"end").as_deref(),
            Some("a")
        );
        assert_eq!(
            raw_tex_env_name("\\begin{ a }", b"begin").as_deref(),
            Some(" a ")
        );
        assert_eq!(raw_tex_env_name("\\begin{}", b"begin").as_deref(), Some(""));
        // A bare word, a missing brace, or the wrong keyword is not a match.
        assert_eq!(raw_tex_env_name("\\beginabc", b"begin"), None);
        assert_eq!(raw_tex_env_name("\\begin center", b"begin"), None);
        assert_eq!(raw_tex_env_name("begin{a}", b"begin"), None);
    }

    #[test]
    fn math_environments_are_excluded() {
        assert!(is_math_environment("equation"));
        assert!(is_math_environment("align*"));
        assert!(is_math_environment("math"));
        assert!(is_math_environment("dmath"));
        assert!(!is_math_environment("center"));
        assert!(!is_math_environment("align**"));
        assert!(!is_math_environment("xalignat"));
        assert!(!is_math_environment("Equation"));
    }

    #[test]
    fn scan_tracks_depth_and_finds_the_close() {
        // The opener line alone opens at depth one and does not close.
        assert_eq!(raw_tex_scan("\\begin{a}", "a", 0), (1, None));
        // A matching end on a content line returns the offset past its brace.
        let (depth, close) = raw_tex_scan("\\end{a}rest", "a", 1);
        assert_eq!(depth, 0);
        assert_eq!(close, Some("\\end{a}".len()));
        // An unrelated end name is content, not a close.
        assert_eq!(raw_tex_scan("\\end{c}", "a", 1), (1, None));
        // Same-name nesting deepens and lifts the count.
        assert_eq!(raw_tex_scan("\\begin{a}\\end{a}", "a", 1), (1, None));
        // An escaped command does not count toward the depth.
        assert_eq!(raw_tex_scan("\\\\end{a}", "a", 1), (1, None));
    }

    #[test]
    fn a_full_environment_becomes_a_raw_tex_block() {
        let out = blocks("\\begin{center}\nx\n\\end{center}\nafter\n");
        assert!(matches!(
            out.first(),
            Some(IrBlock::RawBlock(Format(fmt), body))
                if fmt == "tex" && body == "\\begin{center}\nx\n\\end{center}"
        ));
        assert!(matches!(out.get(1), Some(IrBlock::Para(p)) if p == "after"));
    }

    #[test]
    fn a_single_line_environment_closes_on_its_own_line() {
        let out = blocks("\\begin{center}x\\end{center}\ny\n");
        assert!(matches!(
            out.first(),
            Some(IrBlock::RawBlock(Format(fmt), body))
                if fmt == "tex" && body == "\\begin{center}x\\end{center}"
        ));
        assert!(matches!(out.get(1), Some(IrBlock::Para(p)) if p == "y"));
    }

    #[test]
    fn an_unclosed_environment_falls_back_to_a_paragraph() {
        let out = blocks("\\begin{center}\nx\ny\n");
        assert_eq!(out.len(), 1);
        assert!(matches!(out.first(), Some(IrBlock::Para(_))));
    }

    #[test]
    fn a_math_environment_stays_out_of_a_raw_block() {
        // An \begin opening a math environment is not a block-level raw TeX environment.
        let out = blocks("\\begin{align}\nx\n\\end{align}\n");
        assert!(!matches!(out.first(), Some(IrBlock::RawBlock(..))));
    }

    #[test]
    fn an_indented_begin_is_a_code_block() {
        let out = blocks("    \\begin{center}\n    x\n    \\end{center}\n");
        assert!(matches!(out.first(), Some(IrBlock::CodeBlock(..))));
    }

    #[test]
    fn the_extension_off_leaves_the_syntax_literal() {
        let out = parse(
            "\\begin{center}\nx\n\\end{center}\n",
            presets::COMMONMARK,
            false,
        )
        .0;
        assert_eq!(out.len(), 1);
        assert!(matches!(out.first(), Some(IrBlock::Para(_))));
    }

    use super::alert_marker_type;

    fn gfm_blocks(input: &str) -> Vec<IrBlock> {
        parse(input, presets::GFM, false).0
    }

    #[test]
    fn alert_marker_recognizes_every_kind() {
        assert_eq!(
            alert_marker_type("[!NOTE]", true).map(|t| t.class),
            Some("note")
        );
        assert_eq!(
            alert_marker_type("[!TIP]", true).map(|t| t.class),
            Some("tip")
        );
        assert_eq!(
            alert_marker_type("[!IMPORTANT]", true).map(|t| t.class),
            Some("important")
        );
        assert_eq!(
            alert_marker_type("[!WARNING]", true).map(|t| t.class),
            Some("warning")
        );
        assert_eq!(
            alert_marker_type("[!CAUTION]", true).map(|t| t.title),
            Some("Caution")
        );
    }

    #[test]
    fn alert_marker_casing_depends_on_the_dialect() {
        // The broad Markdown dialect admits only the uppercase spelling.
        assert!(alert_marker_type("[!note]", true).is_none());
        assert!(alert_marker_type("[!Tip]", true).is_none());
        assert!(alert_marker_type("[!wArNiNg]", true).is_none());
        // The CommonMark engine accepts any casing.
        assert_eq!(
            alert_marker_type("[!note]", false).map(|t| t.class),
            Some("note")
        );
        assert_eq!(
            alert_marker_type("[!Tip]", false).map(|t| t.class),
            Some("tip")
        );
    }

    #[test]
    fn alert_marker_allows_only_trailing_whitespace() {
        assert!(alert_marker_type("[!NOTE]", true).is_some());
        assert!(alert_marker_type("[!NOTE]   ", true).is_some());
        assert!(alert_marker_type("[!NOTE]\t", true).is_some());
        // Anything other than whitespace after the bracket disqualifies the marker.
        assert!(alert_marker_type("[!NOTE] hi", true).is_none());
        assert!(alert_marker_type("[!NOTE]x", true).is_none());
    }

    #[test]
    fn alert_marker_rejects_unknown_or_malformed_markers() {
        assert!(alert_marker_type("[!FOO]", true).is_none());
        assert!(alert_marker_type("[!]", true).is_none());
        assert!(alert_marker_type("[NOTE]", true).is_none());
        assert!(alert_marker_type(" [!NOTE]", true).is_none());
        assert!(alert_marker_type("[!NOTE", true).is_none());
    }

    #[test]
    fn an_alert_blockquote_becomes_a_titled_div() {
        let out = gfm_blocks("> [!NOTE]\n> This is a note.\n");
        let Some(IrBlock::Div(attr, content)) = out.first() else {
            panic!("expected a div, got {out:?}");
        };
        assert_eq!(attr.classes, vec!["note".to_owned()]);
        let Some(IrBlock::Div(title_attr, title)) = content.first() else {
            panic!("expected a title div");
        };
        assert_eq!(title_attr.classes, vec!["title".to_owned()]);
        assert!(matches!(title.as_slice(), [IrBlock::Para(t)] if t == "Note"));
        assert!(matches!(content.get(1), Some(IrBlock::Para(t)) if t == "This is a note."));
    }

    #[test]
    fn a_marker_only_alert_carries_just_its_title() {
        let out = gfm_blocks("> [!TIP]\n");
        let Some(IrBlock::Div(attr, content)) = out.first() else {
            panic!("expected a div");
        };
        assert_eq!(attr.classes, vec!["tip".to_owned()]);
        assert_eq!(content.len(), 1);
        assert!(matches!(content.first(), Some(IrBlock::Div(..))));
    }

    #[test]
    fn an_alert_preserves_richer_body_content() {
        let out = gfm_blocks("> [!WARNING]\n> # Heading\n");
        let Some(IrBlock::Div(_, content)) = out.first() else {
            panic!("expected a div");
        };
        assert!(matches!(content.get(1), Some(IrBlock::Heading(1, _))));
    }

    #[test]
    fn an_unknown_marker_leaves_the_blockquote_intact() {
        let out = gfm_blocks("> [!FOO]\n> x\n");
        assert!(matches!(out.first(), Some(IrBlock::BlockQuote(_))));
    }

    #[test]
    fn trailing_text_on_the_marker_line_leaves_the_blockquote_intact() {
        let out = gfm_blocks("> [!NOTE] hello\n> x\n");
        assert!(matches!(out.first(), Some(IrBlock::BlockQuote(_))));
    }

    #[test]
    fn alerts_off_leaves_the_marker_literal() {
        let out = parse("> [!NOTE]\n> x\n", presets::COMMONMARK, true).0;
        assert!(matches!(out.first(), Some(IrBlock::BlockQuote(_))));
    }

    use super::split_trailing_attr;

    fn table_attr_and_caption(input: &str) -> (carta_ast::Attr, Option<String>) {
        let out = blocks(input);
        match out.into_iter().next() {
            Some(IrBlock::Table { attr, caption, .. }) => (attr, caption),
            Some(IrBlock::TextTable(table)) => (table.attr, table.caption),
            Some(IrBlock::GridTable(table)) => (table.attr, table.caption),
            other => panic!("expected a table, got {other:?}"),
        }
    }

    #[test]
    fn split_trailing_attr_strips_the_block() {
        let (body, attr) = split_trailing_attr("My cap {#t .w}");
        assert_eq!(body, "My cap");
        let attr = attr.expect("attr parsed");
        assert_eq!(attr.id, "t");
        assert_eq!(attr.classes, ["w"]);
    }

    #[test]
    fn split_trailing_attr_keeps_non_trailing_braces_literal() {
        // Only the last block at the very end is an attribute block.
        let (body, attr) = split_trailing_attr("Cap {#x} {#y}");
        assert_eq!(body, "Cap {#x}");
        assert_eq!(attr.expect("attr parsed").id, "y");
        // A block followed by more text is not trailing and stays untouched.
        assert_eq!(
            split_trailing_attr("Cap {#x} more"),
            ("Cap {#x} more", None)
        );
    }

    #[test]
    fn split_trailing_attr_rejects_malformed_blocks() {
        assert_eq!(split_trailing_attr("Cap {#x").0, "Cap {#x");
        assert!(split_trailing_attr("Cap {#x").1.is_none());
        assert_eq!(split_trailing_attr("Cap {bad !!}").0, "Cap {bad !!}");
        assert!(split_trailing_attr("Cap {bad !!}").1.is_none());
        assert_eq!(split_trailing_attr("plain text").0, "plain text");
    }

    #[test]
    fn caption_attributes_attach_to_the_table() {
        let (attr, caption) = table_attr_and_caption("| a |\n|---|\n| 1 |\n\n: My cap {#t .w}\n");
        assert_eq!(attr.id, "t");
        assert_eq!(attr.classes, ["w"]);
        assert_eq!(caption.as_deref(), Some("My cap"));
    }

    #[test]
    fn caption_keyvals_attach_to_the_table() {
        let (attr, _) = table_attr_and_caption("| a |\n|---|\n| 1 |\n\n: c {key=val}\n");
        assert_eq!(attr.attributes, [("key".into(), "val".into())]);
    }

    #[test]
    fn caption_without_a_block_leaves_attr_empty() {
        let (attr, caption) = table_attr_and_caption("| a |\n|---|\n| 1 |\n\n: just a caption\n");
        assert!(attr.id.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty());
        assert_eq!(caption.as_deref(), Some("just a caption"));
    }

    #[test]
    fn caption_attributes_are_inert_without_the_extension() {
        // With table attributes disabled, the trailing block on a caption is kept verbatim as
        // caption text rather than split off onto the table's attributes.
        let mut table = blocks("| a |\n|---|\n| 1 |\n");
        assert!(super::set_table_caption(
            &mut table,
            0,
            "c {#t}",
            presets::COMMONMARK
        ));
        let (attr, caption) = match table.into_iter().next() {
            Some(IrBlock::Table { attr, caption, .. }) => (attr, caption),
            Some(IrBlock::TextTable(t)) => (t.attr, t.caption),
            other => panic!("expected a table, got {other:?}"),
        };
        assert!(attr.id.is_empty());
        assert_eq!(caption.as_deref(), Some("c {#t}"));
    }
}

#[cfg(test)]
mod html_element_tests {
    use super::{IrBlock, html_element, parse};
    use carta_core::{Extension, Extensions, presets};

    fn md(input: &str) -> Vec<IrBlock> {
        parse(input, presets::MARKDOWN, true).0
    }

    fn with(input: &str, exts: &[Extension]) -> Vec<IrBlock> {
        parse(input, Extensions::from_list(exts), true).0
    }

    #[test]
    fn div_becomes_a_div_with_parsed_attributes_and_content() {
        let out = md("<div class=\"n\" id=\"d\">\n\n*hi* there\n\n</div>\n");
        let [IrBlock::Div(attr, content)] = out.as_slice() else {
            panic!("expected one div, got {out:?}");
        };
        assert_eq!(attr.id, "d");
        assert_eq!(attr.classes, vec!["n".to_owned()]);
        assert!(attr.attributes.is_empty());
        assert!(matches!(content.as_slice(), [IrBlock::Para(_)]));
    }

    #[test]
    fn div_attributes_split_class_keep_id_and_preserve_keyval_order() {
        let out = md("<div data-z=\"1\" id=\"i\" data-a=\"2\" class=\"a b\">\n\nx\n\n</div>\n");
        let [IrBlock::Div(attr, _)] = out.as_slice() else {
            panic!("expected one div, got {out:?}");
        };
        assert_eq!(attr.id, "i");
        assert_eq!(attr.classes, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(
            attr.attributes,
            vec![("data-z".into(), "1".into()), ("data-a".into(), "2".into()),]
        );
    }

    #[test]
    fn nested_divs_balance_into_a_tree() {
        let out =
            md("<div class=\"outer\">\n\n<div class=\"inner\">\n\ntext\n\n</div>\n\n</div>\n");
        let [IrBlock::Div(outer, outer_children)] = out.as_slice() else {
            panic!("expected one outer div, got {out:?}");
        };
        assert_eq!(outer.classes, vec!["outer".to_owned()]);
        let [IrBlock::Div(inner, inner_children)] = outer_children.as_slice() else {
            panic!("expected one inner div, got {outer_children:?}");
        };
        assert_eq!(inner.classes, vec!["inner".to_owned()]);
        assert!(matches!(inner_children.as_slice(), [IrBlock::Para(_)]));
    }

    #[test]
    fn div_final_block_tightens_only_when_the_close_tag_trails_content() {
        // Close tag on its own line keeps the final block as `Para`, even without blank lines.
        let para = md("<div>\nfoo\n</div>\n");
        assert!(matches!(
            para.as_slice(),
            [IrBlock::Div(_, c)] if matches!(c.as_slice(), [IrBlock::Para(_)])
        ));
        // Close tag trailing content on the same line tightens the final block to `Plain`.
        let plain = md("<div>\nfoo\nbar</div>\n");
        assert!(matches!(
            plain.as_slice(),
            [IrBlock::Div(_, c)] if matches!(c.as_slice(), [IrBlock::Plain(_)])
        ));
        // An earlier block stays `Para`; only the trailing one tightens.
        let mixed = md("<div>\n\nfoo\n\nbar</div>\n");
        let [IrBlock::Div(_, content)] = mixed.as_slice() else {
            panic!("expected one div, got {mixed:?}");
        };
        assert!(matches!(
            content.as_slice(),
            [IrBlock::Para(_), IrBlock::Plain(_)]
        ));
    }

    #[test]
    fn multibyte_attribute_values_do_not_leak_into_following_content() {
        // The open tag is consumed by byte length, so a multibyte character in an attribute value
        // leaves no stray bytes (e.g. the trailing `>`) to be re-read as a spurious block.
        let out = md("<div class=\"café\">\n\nx\n\n</div>\n");
        let [IrBlock::Div(attr, content)] = out.as_slice() else {
            panic!("expected one div, got {out:?}");
        };
        assert_eq!(attr.classes, vec!["café".to_owned()]);
        assert!(matches!(content.as_slice(), [IrBlock::Para(_)]));
    }

    #[test]
    fn content_after_the_close_tag_is_a_following_block() {
        let out = md("<div>\nfoo\n</div>more\n");
        assert!(matches!(
            out.as_slice(),
            [IrBlock::Div(..), IrBlock::Para(_)]
        ));
    }

    #[test]
    fn raw_html_block_trailing_newline_depends_on_dialect() {
        // A raw HTML block (here a verbatim `<pre>`) keeps its final newline in the strict dialect
        // and drops it in the markdown dialect.
        let input = "<pre>\nhi\n</pre>\n";
        let strict = parse(input, presets::COMMONMARK, false).0;
        let [IrBlock::RawHtml(text)] = strict.as_slice() else {
            panic!("expected one raw HTML block, got {strict:?}");
        };
        assert_eq!(text, "<pre>\nhi\n</pre>\n");

        let markdown = parse(input, presets::COMMONMARK, true).0;
        let [IrBlock::RawHtml(text)] = markdown.as_slice() else {
            panic!("expected one raw HTML block, got {markdown:?}");
        };
        assert_eq!(text, "<pre>\nhi\n</pre>");
    }

    #[test]
    fn non_div_block_tag_keeps_raw_tags_around_parsed_content() {
        let out = md("<section class=\"n\">\n\n*hi*\n\n</section>\n");
        let [
            IrBlock::RawHtml(open),
            IrBlock::Para(_),
            IrBlock::RawHtml(close),
        ] = out.as_slice()
        else {
            panic!("expected raw-open, para, raw-close; got {out:?}");
        };
        assert_eq!(open, "<section class=\"n\">");
        assert_eq!(close, "</section>");
    }

    #[test]
    fn raw_element_final_block_tightens_when_no_blank_precedes_the_close() {
        // No blank line before the close tag: the final block is `Plain`.
        let tight = md("<section>\nfoo\n</section>\n");
        assert!(matches!(
            tight.as_slice(),
            [IrBlock::RawHtml(_), IrBlock::Plain(_), IrBlock::RawHtml(_)]
        ));
        // A blank line before the close tag keeps the final block `Para`.
        let loose = md("<section>\nfoo\n\n</section>\n");
        assert!(matches!(
            loose.as_slice(),
            [IrBlock::RawHtml(_), IrBlock::Para(_), IrBlock::RawHtml(_)]
        ));
    }

    #[test]
    fn native_divs_off_renders_a_div_as_a_raw_element() {
        let out = with(
            "<div class=\"n\">\n*hi*\n</div>\n",
            &[Extension::MarkdownInHtmlBlocks],
        );
        let [
            IrBlock::RawHtml(open),
            IrBlock::Plain(_),
            IrBlock::RawHtml(close),
        ] = out.as_slice()
        else {
            panic!("expected raw div fallback, got {out:?}");
        };
        assert_eq!(open, "<div class=\"n\">");
        assert_eq!(close, "</div>");
    }

    #[test]
    fn both_extensions_off_spans_a_block_element_to_its_balanced_close() {
        // With neither extension, a block-level tag at the left margin is kept verbatim as one raw
        // block spanning to its balanced close — blank lines included — rather than parsed as a div.
        let out = with("<div>\n\nfoo\n\n</div>\n", &[]);
        let [IrBlock::RawHtml(html)] = out.as_slice() else {
            panic!("expected one raw HTML block, got {out:?}");
        };
        assert_eq!(html, "<div>\n\nfoo\n\n</div>");
    }

    #[test]
    fn inline_and_unknown_tags_are_not_block_elements() {
        // `<em>`/`<span>` are inline and `<custom>` is unrecognized: none open a block element, so
        // none produce a div.
        for input in ["<em>\n\nx\n\n</em>\n", "<custom>\n\nx\n\n</custom>\n"] {
            let out = md(input);
            assert!(
                !out.iter().any(|b| matches!(b, IrBlock::Div(..))),
                "{input:?} should not produce a div, got {out:?}"
            );
        }
    }

    #[test]
    fn an_unclosed_element_closes_at_end_of_input_without_a_close_tag() {
        let out = md("<div>\n\nfoo\n");
        let [IrBlock::Div(_, content)] = out.as_slice() else {
            panic!("expected one div, got {out:?}");
        };
        assert!(matches!(content.as_slice(), [IrBlock::Para(_)]));
        // A raw element left open emits no trailing close tag.
        let raw = md("<section>\n\nfoo\n");
        assert!(
            !raw.iter()
                .any(|b| matches!(b, IrBlock::RawHtml(t) if t.contains("</section>"))),
            "an unclosed raw element should emit no close tag, got {raw:?}"
        );
    }

    #[test]
    fn parse_open_tag_reads_name_attributes_and_extent() {
        let tag = html_element::parse_open_tag("<div id=\"x\" class=\"a b\" data-k=v>rest")
            .expect("a div open tag");
        assert_eq!(tag.tag, "div");
        assert_eq!(tag.attr.id, "x");
        assert_eq!(tag.attr.classes, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(tag.attr.attributes, vec![("data-k".into(), "v".into())]);
        // The extent stops just past the `>`, leaving any same-line remainder.
        assert_eq!(tag.len, "<div id=\"x\" class=\"a b\" data-k=v>".len());
    }

    #[test]
    fn parse_open_tag_rejects_non_block_and_malformed_tags() {
        assert!(html_element::parse_open_tag("<em>").is_none());
        assert!(html_element::parse_open_tag("<custom>").is_none());
        assert!(html_element::parse_open_tag("not a tag").is_none());
        assert!(html_element::parse_open_tag("<div class=\"oops>").is_none());
    }

    #[test]
    fn parse_open_tag_keeps_only_the_first_class_attribute() {
        let tag = html_element::parse_open_tag("<div class=\"a\" class=\"b\">").expect("a div");
        assert_eq!(tag.attr.classes, vec!["a".to_owned()]);
    }

    #[test]
    fn parse_open_tag_records_a_valueless_attribute_as_an_empty_value() {
        let tag = html_element::parse_open_tag("<div hidden class=\"a\">").expect("a div");
        assert_eq!(tag.attr.classes, vec!["a".to_owned()]);
        assert_eq!(tag.attr.attributes, vec![("hidden".into(), "".into())]);
    }

    #[test]
    fn find_close_tag_locates_the_matching_name_and_skips_unrelated_ones() {
        let found = html_element::find_close_tag("foo</div>bar", "div").expect("a close tag");
        assert_eq!(&"foo</div>bar"[found.start..found.end], "</div>");
        // A different name is not the match.
        assert!(html_element::find_close_tag("</span>", "div").is_none());
        // Trailing whitespace before `>` is allowed; a bare name is not a close tag.
        assert!(html_element::find_close_tag("</div >", "div").is_some());
        assert!(html_element::find_close_tag("no tag here", "div").is_none());
    }

    #[test]
    fn parse_open_tag_flags_a_self_closing_tag() {
        assert!(
            html_element::parse_open_tag("<div/>")
                .expect("a div")
                .self_closing
        );
        assert!(
            html_element::parse_open_tag("<hr />")
                .expect("an hr")
                .self_closing
        );
        assert!(
            !html_element::parse_open_tag("<div>")
                .expect("a div")
                .self_closing
        );
    }

    #[test]
    fn parse_close_tag_matches_a_leading_block_close_tag() {
        assert_eq!(
            html_element::parse_close_tag("</div>rest"),
            Some("</div>".len())
        );
        assert_eq!(
            html_element::parse_close_tag("</div  >"),
            Some("</div  >".len())
        );
        // Only a block-level name, only at the very start.
        assert_eq!(html_element::parse_close_tag("</span>"), None);
        assert_eq!(html_element::parse_close_tag("x</div>"), None);
        assert_eq!(html_element::parse_close_tag("<div>"), None);
    }

    #[test]
    fn scan_depth_balances_nested_same_name_tags() {
        // A same-name open raises the depth; the matching close returns it to zero mid-line.
        let (depth, close) = html_element::scan_depth("<div>a</div></div>", "div", 1);
        assert_eq!(depth, 0);
        assert_eq!(close, Some("<div>a</div>".len() + "</div>".len()));
        // A self-closing same-name tag does not raise the depth.
        assert_eq!(html_element::scan_depth("<div/>", "div", 1), (1, None));
        // A different tag's `>` inside its attributes is skipped whole.
        assert_eq!(
            html_element::scan_depth("<td x=\">\">", "div", 1),
            (1, None)
        );
    }
}

#[cfg(test)]
mod dialect_tests {
    use super::{IrBlock, parse};
    use carta_core::{Extension, Extensions, presets};

    /// Parse with a given extension set in the Markdown dialect (greedy paragraphs).
    fn markdown_with(input: &str, extensions: Extensions) -> Vec<IrBlock> {
        parse(input, extensions, true).0
    }

    /// Parse with a given extension set in the `CommonMark` family (non-greedy paragraphs).
    fn strict_with(input: &str, extensions: Extensions) -> Vec<IrBlock> {
        parse(input, extensions, false).0
    }

    fn ordered_start(blocks: &[IrBlock]) -> Option<i32> {
        match blocks {
            [IrBlock::OrderedList(attrs, _)] => Some(attrs.start),
            _ => None,
        }
    }

    fn heading_level(blocks: &[IrBlock]) -> Option<i32> {
        match blocks {
            [IrBlock::Heading(level, _)] => Some(*level),
            _ => None,
        }
    }

    #[test]
    fn markdown_honors_start_number_when_startnum_enabled() {
        // The default Markdown preset enables `startnum`, so the list begins at its written number.
        assert!(presets::MARKDOWN.contains(Extension::Startnum));
        let blocks = markdown_with("3. a\n4. b\n", presets::MARKDOWN);
        assert_eq!(ordered_start(&blocks), Some(3));
    }

    #[test]
    fn markdown_forces_start_to_one_when_startnum_disabled() {
        let extensions = {
            let mut set = presets::MARKDOWN;
            set.remove(Extension::Startnum);
            set
        };
        assert!(!extensions.contains(Extension::Startnum));
        let blocks = markdown_with("3. a\n4. b\n", extensions);
        assert_eq!(ordered_start(&blocks), Some(1));
    }

    #[test]
    fn commonmark_always_honors_start_number() {
        // CommonMark has no `startnum` extension; the written number is always kept.
        let blocks = strict_with("3. a\n4. b\n", presets::COMMONMARK);
        assert_eq!(ordered_start(&blocks), Some(3));
        let gfm = strict_with("3. a\n4. b\n", presets::GFM);
        assert_eq!(ordered_start(&gfm), Some(3));
    }

    #[test]
    fn markdown_setext_underline_needs_a_single_line_paragraph() {
        // A single line above the underline forms a heading in the markdown dialect.
        let one = markdown_with("one line\n===\n", presets::MARKDOWN);
        assert_eq!(heading_level(&one), Some(1));
        // Two or more lines keep the underline as ordinary paragraph text: no heading forms, and the
        // `===` line is retained as part of the paragraph.
        let many = markdown_with("line one\nline two\n===\n", presets::MARKDOWN);
        assert!(matches!(many.as_slice(), [IrBlock::Para(text)] if text.contains("===")));
        // A leading reference definition does not count toward the line budget: the single content
        // line still heads.
        let refd = markdown_with("[x]: /u\ncontent\n===\n", presets::MARKDOWN);
        assert_eq!(heading_level(&refd), Some(1));
        // The CommonMark family heads a multi-line paragraph, per its setext rule.
        let cm = strict_with("line one\nline two\n===\n", presets::COMMONMARK);
        assert_eq!(heading_level(&cm), Some(1));
    }

    #[test]
    fn markdown_reads_seven_hashes_as_level_seven() {
        let blocks = markdown_with("####### h\n", presets::MARKDOWN);
        assert_eq!(heading_level(&blocks), Some(7));
    }

    #[test]
    fn markdown_reads_eight_hashes_as_level_eight() {
        let blocks = markdown_with("######## h\n", presets::MARKDOWN);
        assert_eq!(heading_level(&blocks), Some(8));
    }

    #[test]
    fn markdown_does_not_cap_deep_heading_levels() {
        let blocks = markdown_with("############## deep\n", presets::MARKDOWN);
        assert_eq!(heading_level(&blocks), Some(14));
    }

    #[test]
    fn commonmark_reads_seven_hashes_as_a_paragraph() {
        let blocks = strict_with("####### h\n", presets::COMMONMARK);
        assert!(matches!(blocks.as_slice(), [IrBlock::Para(_)]));
        let gfm = strict_with("####### h\n", presets::GFM);
        assert!(matches!(gfm.as_slice(), [IrBlock::Para(_)]));
    }

    #[test]
    fn deep_heading_still_requires_a_space_after_the_hashes() {
        // Seven hashes glued to content is not a heading in either dialect.
        let blocks = markdown_with("#######nospace\n", presets::MARKDOWN);
        assert!(matches!(blocks.as_slice(), [IrBlock::Para(_)]));
    }

    #[test]
    fn classic_dialect_reads_a_hash_run_glued_to_text_as_a_heading() {
        // With space_in_atx_header off, a hash run needs no following space.
        for input in ["#heading\n", "##heading\n", "###heading\n"] {
            let blocks = markdown_with(input, presets::MARKDOWN_STRICT_READ);
            assert_eq!(
                heading_level(&blocks),
                i32::try_from(input.bytes().take_while(|&b| b == b'#').count()).ok(),
                "expected heading for {input:?}, got {blocks:?}"
            );
        }
    }

    #[test]
    fn classic_dialect_strips_a_glued_closing_hash_run() {
        // With space_in_atx_header off, a trailing hash run always terminates the heading,
        // even glued to the content; an interior hash is kept.
        let cases = [
            ("#foo#\n", "foo"),
            ("#foo ###\n", "foo"),
            ("#foo#bar#\n", "foo#bar"),
        ];
        for (input, want) in cases {
            let blocks = markdown_with(input, presets::MARKDOWN_STRICT_READ);
            match blocks.as_slice() {
                [IrBlock::Heading(1, text)] => assert_eq!(text, want, "for {input:?}"),
                other => panic!("expected level-1 heading for {input:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn extended_dialect_requires_a_space_after_the_hash_run() {
        // space_in_atx_header is on in the extended dialect: a glued run is a paragraph.
        let blocks = markdown_with("#heading\n", presets::MARKDOWN);
        assert!(matches!(blocks.as_slice(), [IrBlock::Para(_)]));
    }

    #[test]
    fn commonmark_requires_a_space_after_the_hash_run() {
        let blocks = strict_with("#heading\n", presets::COMMONMARK);
        assert!(matches!(blocks.as_slice(), [IrBlock::Para(_)]));
    }

    #[test]
    fn markdown_rejects_an_indented_atx_heading() {
        // The Markdown dialect requires the hash run to start at the left margin.
        for input in ["  # h\n", "   ###### h\n", "   ####### h\n"] {
            let blocks = markdown_with(input, presets::MARKDOWN);
            assert!(
                matches!(blocks.as_slice(), [IrBlock::Para(_)]),
                "expected paragraph for {input:?}, got {blocks:?}"
            );
        }
    }

    #[test]
    fn commonmark_allows_up_to_three_spaces_before_an_atx_heading() {
        let blocks = strict_with("   ###### h\n", presets::COMMONMARK);
        assert_eq!(heading_level(&blocks), Some(6));
    }

    use carta_ast::{ListNumberDelim, ListNumberStyle};

    /// The (start, style, delim) of a single ordered list, or `None` for anything else.
    fn ordered_attrs(blocks: &[IrBlock]) -> Option<(i32, ListNumberStyle, ListNumberDelim)> {
        match blocks {
            [IrBlock::OrderedList(attrs, _)] => Some((attrs.start, attrs.style, attrs.delim)),
            _ => None,
        }
    }

    fn list_item_count(blocks: &[IrBlock]) -> usize {
        match blocks {
            [IrBlock::OrderedList(_, items)] => items.len(),
            _ => 0,
        }
    }

    // --- Gap 3: multi-letter roman-numeral ordered lists ---

    #[test]
    fn markdown_reads_a_multi_letter_roman_list() {
        // `II.`/`III.` are unambiguously roman: the list is UpperRoman starting at two.
        let blocks = markdown_with("II. two\nIII. three\n", presets::MARKDOWN);
        assert_eq!(
            ordered_attrs(&blocks),
            Some((2, ListNumberStyle::UpperRoman, ListNumberDelim::Period))
        );
        assert_eq!(list_item_count(&blocks), 2);
    }

    #[test]
    fn markdown_reads_a_lowercase_roman_paren_list() {
        let blocks = markdown_with("ii) a\niii) b\n", presets::MARKDOWN);
        assert_eq!(
            ordered_attrs(&blocks),
            Some((2, ListNumberStyle::LowerRoman, ListNumberDelim::OneParen))
        );
    }

    #[test]
    fn markdown_computes_the_start_ordinal_from_the_roman_value() {
        // `IV` is four; the list begins there.
        let blocks = markdown_with("IV. a\nV. b\n", presets::MARKDOWN);
        assert_eq!(
            ordered_attrs(&blocks),
            Some((4, ListNumberStyle::UpperRoman, ListNumberDelim::Period))
        );
    }

    #[test]
    fn markdown_reads_a_two_place_roman_numeral() {
        // `XII` is twelve: the tens and ones places combine.
        let blocks = markdown_with("XII. a\nXIII. b\n", presets::MARKDOWN);
        assert_eq!(
            ordered_attrs(&blocks),
            Some((12, ListNumberStyle::UpperRoman, ListNumberDelim::Period))
        );
    }

    #[test]
    fn markdown_reads_a_thousands_roman_numeral() {
        let blocks = markdown_with("MII. a\nMIII. b\n", presets::MARKDOWN);
        assert_eq!(
            ordered_attrs(&blocks),
            Some((1002, ListNumberStyle::UpperRoman, ListNumberDelim::Period))
        );
    }

    #[test]
    fn markdown_keeps_a_lone_capital_letter_marker_as_a_paragraph() {
        // A single uppercase letter followed by a period and one space is ambiguous with an initial
        // (`I. only` could be a name), so it stays a paragraph rather than opening a list.
        let blocks = markdown_with("I. only\n", presets::MARKDOWN);
        assert!(
            matches!(blocks.as_slice(), [IrBlock::Para(_)]),
            "expected a paragraph, got {blocks:?}"
        );
    }

    #[test]
    fn markdown_reads_a_lone_capital_letter_marker_with_two_spaces_as_a_list() {
        // Two spaces after the marker disambiguate it from an initial, so the single `I` opens a
        // one-letter roman list (value one).
        let blocks = markdown_with("I.  only\n", presets::MARKDOWN);
        assert_eq!(
            ordered_attrs(&blocks),
            Some((1, ListNumberStyle::UpperRoman, ListNumberDelim::Period))
        );
    }

    // --- Gap 4: `#.` fancy hash-marker ordered lists ---

    #[test]
    fn markdown_reads_a_hash_period_list() {
        let blocks = markdown_with("#. one\n#. two\n", presets::MARKDOWN);
        assert_eq!(
            ordered_attrs(&blocks),
            Some((
                1,
                ListNumberStyle::DefaultStyle,
                ListNumberDelim::DefaultDelim
            ))
        );
        assert_eq!(list_item_count(&blocks), 2);
    }

    #[test]
    fn markdown_reads_a_hash_paren_list() {
        let blocks = markdown_with("#) one\n#) two\n", presets::MARKDOWN);
        assert_eq!(
            ordered_attrs(&blocks),
            Some((1, ListNumberStyle::DefaultStyle, ListNumberDelim::OneParen))
        );
    }

    #[test]
    fn commonmark_does_not_read_a_hash_marker_as_a_list() {
        // The fancy hash marker is a Markdown-dialect feature; CommonMark keeps it literal.
        let blocks = strict_with("#. one\n#. two\n", presets::COMMONMARK);
        assert!(
            ordered_attrs(&blocks).is_none(),
            "CommonMark should not form a list from `#.`, got {blocks:?}"
        );
    }
}

#[cfg(test)]
mod abbreviation_tests {
    use super::{IrBlock, parse};
    use carta_core::{Extension, Extensions};

    fn with_abbr(input: &str) -> Vec<IrBlock> {
        parse(
            input,
            Extensions::from_list(&[Extension::Abbreviations]),
            true,
        )
        .0
    }

    fn plain(input: &str) -> Vec<IrBlock> {
        parse(input, Extensions::empty(), true).0
    }

    #[test]
    fn a_definition_at_the_left_edge_is_consumed() {
        let out = with_abbr("*[HTML]: markup\n\nBody.\n");
        assert!(
            matches!(out.as_slice(), [IrBlock::Para(p)] if p == "Body."),
            "definition should be dropped, leaving only the body: {out:?}"
        );
    }

    #[test]
    fn a_definition_is_stripped_from_a_paragraph_front() {
        let out = with_abbr("*[HTML]: markup\nmore text\n");
        assert!(
            matches!(out.as_slice(), [IrBlock::Para(p)] if p == "more text"),
            "only the definition line should be removed: {out:?}"
        );
    }

    #[test]
    fn consecutive_definitions_are_all_consumed() {
        let out = with_abbr("*[A]: x\n*[B]: y\nmore\n");
        assert!(
            matches!(out.as_slice(), [IrBlock::Para(p)] if p == "more"),
            "both definitions should be removed: {out:?}"
        );
    }

    #[test]
    fn an_indented_definition_is_left_as_text() {
        // A definition must sit flush at the container's left edge; one space in front keeps it a
        // paragraph.
        let out = with_abbr(" *[HTML]: markup\n\nBody.\n");
        assert_eq!(
            out.len(),
            2,
            "indented definition stays a paragraph: {out:?}"
        );
    }

    #[test]
    fn without_the_extension_a_definition_is_ordinary_text() {
        let out = plain("*[HTML]: markup\n\nBody.\n");
        assert_eq!(
            out.len(),
            2,
            "no consumption without the extension: {out:?}"
        );
    }
}

#[cfg(test)]
mod fence_interrupt_tests {
    use super::{IrBlock, parse};
    use carta_core::{Extension, Extensions};

    fn md(input: &str, exts: &[Extension]) -> Vec<IrBlock> {
        parse(input, Extensions::from_list(exts), true).0
    }

    #[test]
    fn a_tilde_fence_does_not_interrupt_a_paragraph() {
        let out = md("text\n~~~\ncode\n~~~\n", &[Extension::FencedCodeBlocks]);
        assert!(
            matches!(out.as_slice(), [IrBlock::Para(_)]),
            "a tilde fence folds into the open paragraph: {out:?}"
        );
    }

    #[test]
    fn a_tilde_fence_opens_a_block_at_the_top_level() {
        let out = md("~~~\ncode\n~~~\n", &[Extension::FencedCodeBlocks]);
        assert!(
            matches!(out.as_slice(), [IrBlock::CodeBlock(..)]),
            "a top-level tilde fence still opens a code block: {out:?}"
        );
    }

    #[test]
    fn a_backtick_fence_still_interrupts_a_paragraph() {
        let out = md("text\n```\ncode\n```\n", &[Extension::BacktickCodeBlocks]);
        assert!(
            matches!(out.as_slice(), [IrBlock::Para(_), IrBlock::CodeBlock(..)]),
            "a backtick fence interrupts the paragraph: {out:?}"
        );
    }

    #[test]
    fn an_opener_after_a_non_interrupting_tilde_still_fires() {
        // The tilde line folds in, but the following heading is read normally rather than absorbed.
        let out = md(
            "text\n~~~\n# h\n~~~\nmore\n",
            &[Extension::FencedCodeBlocks],
        );
        assert!(
            matches!(out.get(1), Some(IrBlock::Heading(1, _))),
            "a heading after a non-interrupting tilde fence still opens: {out:?}"
        );
    }
}

#[cfg(test)]
mod raw_html_span_tests {
    use super::{IrBlock, parse};
    use carta_core::{Extension, Extensions};

    /// The Markdown family reading raw HTML where inner content is not parsed (the column-zero,
    /// no-interrupt gate: neither `markdown_attribute` nor the div/markdown-in-HTML extensions).
    fn strict(input: &str) -> Vec<IrBlock> {
        parse(input, Extensions::empty(), true).0
    }

    /// The same reading with `markdown_attribute`, which lets the block indent and interrupt.
    fn attr(input: &str) -> Vec<IrBlock> {
        parse(
            input,
            Extensions::from_list(&[Extension::MarkdownAttribute]),
            true,
        )
        .0
    }

    #[test]
    fn a_block_element_spans_to_its_balanced_close() {
        let out = strict("<div>\nx\n\ny\n</div>\n");
        let [IrBlock::RawHtml(html)] = out.as_slice() else {
            panic!("expected one raw HTML block, got {out:?}");
        };
        assert_eq!(html, "<div>\nx\n\ny\n</div>");
    }

    #[test]
    fn nested_same_name_tags_balance() {
        let out = strict("<div>\n<div>\na\n</div>\nb\n</div>\n");
        let [IrBlock::RawHtml(html)] = out.as_slice() else {
            panic!("expected one raw HTML block, got {out:?}");
        };
        assert_eq!(html, "<div>\n<div>\na\n</div>\nb\n</div>");
    }

    #[test]
    fn a_void_tag_is_a_single_line_block_and_the_rest_parses() {
        let out = strict("<hr>\ntext\n");
        let [IrBlock::RawHtml(html), IrBlock::Para(text)] = out.as_slice() else {
            panic!("expected a single-tag raw block then a paragraph, got {out:?}");
        };
        assert_eq!(html, "<hr>");
        assert_eq!(text, "text");
    }

    #[test]
    fn a_self_closing_tag_is_a_single_line_block() {
        let out = strict("<div/>\ntext\n");
        assert!(
            matches!(out.first(), Some(IrBlock::RawHtml(h)) if h == "<div/>"),
            "a self-closing tag opens no span: {out:?}"
        );
    }

    #[test]
    fn a_bare_close_tag_stands_alone() {
        let out = strict("</div>\ntext\n");
        let [IrBlock::RawHtml(html), IrBlock::Para(_)] = out.as_slice() else {
            panic!("expected a lone close tag then a paragraph, got {out:?}");
        };
        assert_eq!(html, "</div>");
    }

    #[test]
    fn an_open_and_close_on_one_line_re_feeds_the_trailing_text() {
        let out = strict("<div>x</div> tail\n");
        let [IrBlock::RawHtml(html), IrBlock::Para(text)] = out.as_slice() else {
            panic!("expected a raw block then the trailing text, got {out:?}");
        };
        assert_eq!(html, "<div>x</div>");
        assert_eq!(text, "tail");
    }

    #[test]
    fn an_unclosed_open_tag_is_the_tag_alone() {
        let out = strict("<div>\nx\n\ny\n");
        let [IrBlock::RawHtml(html), IrBlock::Para(a), IrBlock::Para(b)] = out.as_slice() else {
            panic!("expected the tag alone then two paragraphs, got {out:?}");
        };
        assert_eq!(html, "<div>");
        assert_eq!(a, "x");
        assert_eq!(b, "y");
    }

    #[test]
    fn an_inline_or_unknown_tag_opens_no_block() {
        for input in ["<span>x</span>\ntext\n", "<foo>\nx\n</foo>\n"] {
            let out = strict(input);
            assert!(
                !out.iter().any(|b| matches!(b, IrBlock::RawHtml(_))),
                "a non-block tag stays inline: {out:?}"
            );
        }
    }

    #[test]
    fn without_markdown_attribute_an_indented_tag_is_inline() {
        let out = strict("   <div>\nx\n</div>\n");
        assert!(
            !out.iter().any(|b| matches!(b, IrBlock::RawHtml(_))),
            "an indented tag folds into a paragraph: {out:?}"
        );
    }

    #[test]
    fn with_markdown_attribute_an_indented_tag_opens_a_block() {
        let out = attr("   <div>\nx\n</div>\n");
        let [IrBlock::RawHtml(html)] = out.as_slice() else {
            panic!("expected one raw HTML block, got {out:?}");
        };
        assert_eq!(html, "<div>\nx\n</div>");
    }

    #[test]
    fn without_markdown_attribute_the_block_folds_into_a_paragraph() {
        let out = strict("text\n<div>\nx\n</div>\n");
        assert!(
            matches!(out.as_slice(), [IrBlock::Para(_)]),
            "the block does not interrupt the paragraph: {out:?}"
        );
    }

    #[test]
    fn with_markdown_attribute_the_block_interrupts_a_paragraph() {
        let out = attr("text\n<div>\nx\n</div>\n");
        let [IrBlock::Plain(text), IrBlock::RawHtml(html)] = out.as_slice() else {
            panic!("expected a tight paragraph then a raw block, got {out:?}");
        };
        assert_eq!(text, "text");
        assert_eq!(html, "<div>\nx\n</div>");
    }
}
