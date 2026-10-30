//! Parse block structure into [`IrBlock`]s, leaving leaf text for inline parsing.

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

mod build;
mod continuation;
mod divs;
mod html;
mod openers;
mod raw_tex;
mod tables;

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
/// Each entry is `(line index, longest remaining run)` for a line that, read at the document root, satisfies
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
        candidates.get(start).is_some_and(|&(_, run)| run >= length)
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
    for bucket in [&mut candidates.backtick, &mut candidates.tilde] {
        let mut longest = 0;
        for (_, run) in bucket.iter_mut().rev() {
            longest = longest.max(*run);
            *run = longest;
        }
    }
    candidates
}

/// Length of the run of `marker` bytes that begins `line` after up to any number of leading spaces
/// (counted exactly as [`Cursor::is_closing_fence`] does) when it is at least three; otherwise `None`.
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
    BlockQuote {
        allows_alert: bool,
    },
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
    /// margin begins a span that accumulates lines verbatim (nested same-name tags counted, blank
    /// lines kept) until a close tag brings `depth` back to zero; the whole span is one `RawBlock`.
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
    /// exactly. Math environments (`equation`, `align`, …) are excluded: they stay inline.
    RawTex {
        name: String,
        depth: usize,
    },
    ThematicBreak,
    /// A dash-ruled table candidate, accumulating its physical lines (each `\n`-terminated). Its
    /// exact extent is settled when the block closes: the lines are parsed into a table, with any
    /// surplus rows after the table re-fed as following blocks, or, when they form no table, the
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

// Independent per-node flags; no pairing is invalid.
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
    /// Render this paragraph as `Plain` when an HTML or TeX block interrupts it.
    as_plain: bool,
    /// Whether the line that followed this block (while it was the deepest open block) was blank.
    /// Drives loose-vs-tight list classification.
    last_line_blank: bool,
    /// Set once this paragraph's header and delimiter rows have validated as a pipe table, so each
    /// following body line is classified against the new line alone rather than re-verifying the
    /// header and delimiter (which never change) every time.
    pipe_table: bool,
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
        }
    }
}

/// How an open paragraph absorbs the block openers on the next line (see [`Parser::greedy_gates`]).
// Four independent fold decisions, each governing a distinct opener; they are flags by nature.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Default)]
struct GreedyGates {
    /// The foldable openers (block quote, heading, thematic break, fenced div, footnote
    /// definition) continue the paragraph as a lazy line rather than interrupting it.
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

    pub(super) fn kind(&self, index: usize) -> Option<&Kind> {
        self.nodes.get(index).map(|node| &node.kind)
    }

    pub(super) fn last_open_child(&self, index: usize) -> Option<usize> {
        let node = self.nodes.get(index)?;
        let &last = node.children.last()?;
        match self.nodes.get(last) {
            Some(child) if child.open => Some(last),
            _ => None,
        }
    }

    pub(super) fn close(&mut self, index: usize) {
        if let Some(node) = self.nodes.get_mut(index) {
            node.open = false;
        }
    }

    pub(super) fn append_child(&mut self, parent: usize, mut node: Node) -> usize {
        let index = self.nodes.len();
        node.parent = parent;
        self.nodes.push(node);
        if let Some(parent_node) = self.nodes.get_mut(parent) {
            parent_node.children.push(index);
        }
        index
    }

    pub(super) fn parent(&self, index: usize) -> usize {
        self.nodes.get(index).map_or(0, |node| node.parent)
    }

    /// Whether a block of `kind` may be a direct child of `container`.
    fn can_contain(&self, container: usize, kind: &Kind) -> bool {
        match self.kind(container) {
            Some(
                Kind::Document
                | Kind::BlockQuote { .. }
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
    pub(super) fn place(&mut self, mut container: usize, kind: &Kind) -> usize {
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
    /// line. Leaves terminate the descent: they are handled by [`Parser::process_line`], not
    /// entered as containers.
    fn descend_containers(&mut self, cursor: &mut Cursor) -> Descent {
        let mut container = 0;
        let mut all_matched = true;
        // Judge close fences at each div's own indent; deeper fences are content.
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

    #[allow(clippy::too_many_lines)]
    pub(super) fn process_line(
        &mut self,
        line: &str,
        following: &[&str],
        line_index: Option<usize>,
    ) {
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

        if self.close_fenced_div(container, &div_path) {
            return;
        }

        // The close tag sits at the element's own level; an unmatched inner block does not block it.
        if self.close_html_element(container, &cursor) {
            return;
        }

        if self.continue_raw_tex(container, all_matched, &cursor) {
            return;
        }

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

        // A folding fence absorbs lines verbatim (no opener fires) until its close fence or a blank.
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
                // The underline heads only what remains after leading link reference definitions.
                let text = self.node_text(para);
                let mut body = text.as_str();
                while let Some((_, _, rest)) = scan::parse_link_reference_definition(
                    body,
                    self.greedy_paragraphs,
                    self.extensions.contains(Extension::MmdLinkAttributes),
                ) {
                    body = rest;
                }
                // Markdown family: the underline heads a single-line paragraph only.
                let multiline_body = self.greedy_paragraphs && body.trim_end().contains('\n');
                if !multiline_body {
                    let only_definitions = body.trim().is_empty();
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

        // Open multi-line constructs claim continuation lines before the block openers misread them.
        if self.continue_open_construct(container, all_matched, blank, &cursor) {
            return;
        }

        // Deepest block open before this line; the unmatched-chain tip when phase 1 fell short.
        let matched = container;
        let old_tip = self.deepest_open(matched);

        // Attach blanks to the preceding content to classify list looseness.
        if blank {
            let target = if all_matched {
                self.blank_trails(old_tip)
            } else {
                self.nodes
                    .get(matched)
                    .and_then(|node| node.children.last().copied())
                    .unwrap_or(matched)
            };
            // A blank inside a still-open fenced div must not make the enclosing list loose.
            let internal_to_open_div = matches!(self.kind(target), Some(Kind::FencedDiv(..)))
                && self.nodes.get(target).is_some_and(|node| node.open);
            if !internal_to_open_div && let Some(node) = self.nodes.get_mut(target) {
                node.last_line_blank = true;
            }
        }

        let mut started_new = false;
        if !blank {
            loop {
                cursor.note_indent();
                if let Some(opened) = self.try_open(container, &mut cursor, following, line_index) {
                    started_new = true;
                    container = opened;
                    // Trailing spaces after a bare marker must not open an indented code block.
                    if self.is_container(container) && !cursor.is_blank() {
                        continue;
                    }
                }
                break;
            }
        }

        // Unmatched containers survive only for lazy paragraph continuation.
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

    /// Close `tip` and each ancestor up to (but not including) `until`.
    pub(super) fn close_chain(&mut self, mut tip: usize, until: usize) {
        while tip != until {
            self.close(tip);
            let parent = self.parent(tip);
            if parent == tip {
                break;
            }
            tip = parent;
        }
    }

    /// Find the block that a trailing blank line marks for list looseness.
    ///
    /// Descend through finalized list/item children, stopping at empty containers.
    /// Do not descend into closed quotes or divs: list classification cannot see marks there.
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

    pub(super) fn deepest_open(&self, mut index: usize) -> usize {
        while let Some(child) = self.last_open_child(index) {
            index = child;
        }
        index
    }

    /// Whether an open footnote definition sits in the chain strictly below `container`, that is,
    /// the paragraph a line would lazily continue is that definition's body. A definition marker then
    /// ends the open definition and starts a new one rather than folding into it.
    pub(super) fn footnote_def_open_below(&self, container: usize) -> bool {
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
    pub(super) fn tighten_interrupted_paragraph(&mut self, container: usize) {
        let leaf = self.deepest_open(container);
        if matches!(self.kind(leaf), Some(Kind::Paragraph))
            && let Some(node) = self.nodes.get_mut(leaf)
        {
            node.as_plain = true;
        }
    }

    fn is_container(&self, index: usize) -> bool {
        matches!(
            self.kind(index),
            Some(
                Kind::Document
                    | Kind::BlockQuote { .. }
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

    pub(super) fn append_text(&mut self, index: usize, text: &str) {
        if let Some(node) = self.nodes.get_mut(index) {
            node.text.push_str(text);
        }
    }

    pub(super) fn append_line(&mut self, index: usize, cursor: &Cursor) {
        if let Some(node) = self.nodes.get_mut(index) {
            node.text.push_str(cursor.remaining());
            node.text.push('\n');
        }
    }

    pub(super) fn node_text(&self, index: usize) -> String {
        self.nodes
            .get(index)
            .map(|node| node.text.clone())
            .unwrap_or_default()
    }

    /// Borrow a node's accumulated text without copying. Continuation checks that only inspect the
    /// buffer use this; callers that mutate the node afterward take the owned [`Self::node_text`].
    pub(super) fn node_text_ref(&self, index: usize) -> Option<&str> {
        self.nodes.get(index).map(|node| node.text.as_str())
    }

    /// Open a block-level HTML element when the cursor sits on a recognized open tag. A `<div>`
    /// becomes an [`IrBlock::Div`] when `native_divs` is on; any other block tag (and a `<div>` when
    /// only `markdown_in_html_blocks` is on) keeps its tags as raw HTML around the parsed content.
    /// The whole open tag is consumed; any same-line remainder is re-fed so its content (including a
    /// close tag on the same line) flows through the normal line handling.
    ///
    /// When the element directly interrupts an open paragraph with no blank line between, that
    /// preceding paragraph reads tight (`Plain` rather than `Para`) under `markdown_in_html_blocks`.
    /// A self-closing tag (`<div/>`) is read as an ordinary open and stays open until end of input.
    pub(super) fn open_html_element(
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
        // Parsed HTML needs `native_divs` or `markdown_in_html_blocks`.
        let as_div = is_div && native_divs;
        if !as_div && !markdown_in_html {
            return None;
        }
        // A paragraph still open here had no blank line before the element, so it reads tight.
        if markdown_in_html {
            self.tighten_interrupted_paragraph(container);
        }
        let raw_open = remaining.get(..open.len).unwrap_or(remaining).to_owned();
        let trailing = remaining.get(open.len..).unwrap_or("").to_owned();
        // The cursor moves in bytes, including over multibyte characters.
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
