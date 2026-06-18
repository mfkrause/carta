//! Block-structure phase: consume input line by line into a tree of [`IrBlock`]s, following the
//! `CommonMark` spec's open-block algorithm (spec appendix "A parsing strategy"). Leaf text is left
//! raw for the inline phase; link reference definitions are stripped from paragraph fronts and
//! collected into the [`RefMap`].

use carta_ast::{Attr, ListAttributes, ListNumberDelim, ListNumberStyle};
use carta_core::{Extension, Extensions};

use super::cursor::{Cursor, FenceInfo, ListMarkerParse};
use super::{FootnoteDefs, IrBlock, IrDefItem, RefMap, TAB_STOP, attr, html_block, scan, table};

/// Parse the normalized input into the block tree plus the collected link and footnote references.
pub(crate) fn parse(input: &str, extensions: Extensions) -> (Vec<IrBlock>, RefMap, FootnoteDefs) {
    let mut parser = Parser::new(extensions);
    for line in split_lines(input) {
        parser.process_line(line);
    }
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
    DefinitionItem { term: String, tight: bool },
    /// One definition body of a definition list. Its content continues under a four-column indent
    /// like a list item; `indent` is the column its content begins at.
    Definition { indent: usize },
    Heading(i32),
    IndentedCode,
    FencedCode(FenceInfo),
    HtmlBlock(u8),
    ThematicBreak,
}

#[derive(Debug, Clone)]
struct ListInfo {
    bullet: bool,
    marker: u8,
    delim: ListNumberDelim,
    start: i32,
}

#[derive(Debug, Clone)]
struct ItemInfo {
    /// Column indent required for a continuation line to belong to this item.
    indent: usize,
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

#[derive(Debug)]
struct Node {
    kind: Kind,
    open: bool,
    parent: usize,
    children: Vec<usize>,
    text: String,
    /// Whether the line that followed this block (while it was the deepest open block) was blank.
    /// Drives loose-vs-tight list classification.
    last_line_blank: bool,
}

impl Node {
    fn new(kind: Kind) -> Self {
        Node {
            kind,
            open: true,
            parent: 0,
            children: Vec::new(),
            text: String::new(),
            last_line_blank: false,
        }
    }
}

struct Parser {
    nodes: Vec<Node>,
    refs: RefMap,
    extensions: Extensions,
}

impl Parser {
    fn new(extensions: Extensions) -> Self {
        Parser {
            nodes: vec![Node::new(Kind::Document)],
            refs: RefMap::new(),
            extensions,
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

    fn process_line(&mut self, line: &str) {
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

        // An open code/html leaf (reachable only when every container matched) consumes the line.
        if all_matched
            && let Some(leaf) = self
                .last_open_child(container)
                .filter(|&c| !self.is_container(c))
            && matches!(
                self.kind(leaf),
                Some(Kind::FencedCode(_) | Kind::IndentedCode | Kind::HtmlBlock(_))
            )
        {
            match self.try_continue(leaf, &mut cursor) {
                Continue::MatchedLeaf | Continue::Matched => return,
                Continue::NotMatched => self.close(leaf),
            }
        }

        let blank = cursor.is_blank();

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
                // underline. Pull them out first; the underline forms a heading only over what
                // remains. If nothing remains, the paragraph was pure definitions — it is consumed
                // and this line is reparsed as ordinary content (not an underline).
                let text = self.node_text(para);
                let remaining = self.extract_refs(&text);
                let only_definitions = remaining.trim().is_empty();
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

        // A pipe table that has begun in an open paragraph claims its rows here, before the block
        // openers below could read a row's leading `-`, `>`, `#`, etc. as the start of a new block.
        if self.continue_pipe_table(container, all_matched, blank, &cursor) {
            return;
        }

        // An open line block claims its `|`-led and indented continuation lines here, before the
        // openers below could read a line's leading content as the start of a new block.
        if self.continue_line_block(container, all_matched, &cursor) {
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
            if !internal_to_open_div
                && let Some(node) = self.nodes.get_mut(target)
            {
                node.last_line_blank = true;
            }
        }

        // Phase 2: open new blocks at the matched container.
        let mut started_new = false;
        if !blank {
            loop {
                cursor.note_indent();
                if let Some(opened) = self.try_open(container, &mut cursor) {
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
        let rest = cursor.rest();
        let header = self.node_text(leaf);
        // A line block becomes a table only on its very first line: a delimiter row directly under
        // the opening `|` line reinterprets it as a table header. Past the first line the block is
        // committed to being a line block.
        if matches!(self.kind(leaf), Some(Kind::LineBlock)) {
            if !single_line(&header) || !table::opens_table(header.trim_end(), &rest) {
                return false;
            }
            if let Some(node) = self.nodes.get_mut(leaf) {
                node.kind = Kind::Paragraph;
            }
            self.append_text(leaf, &rest);
            self.append_text(leaf, "\n");
            return true;
        }
        match table::classify_continuation(&header, &rest) {
            table::Continuation::Absorb => {
                self.append_text(leaf, &rest);
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
    fn continue_line_block(&mut self, container: usize, all_matched: bool, cursor: &Cursor) -> bool {
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
            || (remaining.starts_with(' ') && !last_entry_is_empty(&self.node_text(block)));
        if absorb {
            self.append_text(block, &cursor.rest());
            self.append_text(block, "\n");
            true
        } else {
            self.close(block);
            false
        }
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
        match self.kind(index).cloned() {
            // A list (and the two grouping levels of a definition list) is a transparent container:
            // it consumes nothing and defers to its items.
            Some(Kind::List(_) | Kind::DefinitionList | Kind::DefinitionItem { .. }) => {
                Continue::Matched
            }
            // A definition body continues under its content indent, like a list item — except that
            // an as-yet-empty body survives a blank line, so a deferred indented paragraph still
            // joins it.
            Some(Kind::Definition { indent }) => {
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
            Some(Kind::Item(info)) => self.continue_item_like(index, info.indent, false, cursor),
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
                self.continue_fenced(index, &fence, cursor);
                Continue::MatchedLeaf
            }
            Some(Kind::IndentedCode) => {
                if cursor.is_blank() {
                    cursor.advance_up_to_columns(TAB_STOP);
                    self.append_text(index, &cursor.rest_with_newline());
                    Continue::MatchedLeaf
                } else if cursor.indent() >= TAB_STOP {
                    cursor.advance_columns(TAB_STOP);
                    self.append_text(index, &cursor.rest_with_newline());
                    Continue::MatchedLeaf
                } else {
                    Continue::NotMatched
                }
            }
            Some(Kind::HtmlBlock(kind)) => {
                self.continue_html(index, kind, cursor);
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

    fn continue_fenced(&mut self, index: usize, fence: &FenceInfo, cursor: &mut Cursor) {
        let indent = cursor.indent();
        if indent <= 3 && cursor.is_closing_fence(fence.marker, fence.length) {
            if let Some(node) = self.nodes.get_mut(index) {
                node.open = false;
            }
            return;
        }
        cursor.advance_up_to_columns(fence.indent);
        let content = cursor.rest_with_newline();
        self.append_text(index, &content);
    }

    fn continue_html(&mut self, index: usize, kind: u8, cursor: &mut Cursor) {
        // Types 6 and 7 are terminated by a blank line, which is not part of the block.
        if matches!(kind, 6 | 7) && cursor.is_blank() {
            self.close(index);
            return;
        }
        let line = cursor.rest();
        self.append_text(index, &line);
        self.append_text(index, "\n");
        if html_block::closes(kind, &line) {
            self.close(index);
        }
    }

    /// Try to open a new block at the current cursor position inside `container`.
    fn try_open(&mut self, container: usize, cursor: &mut Cursor) -> Option<usize> {
        let indent = cursor.indent();
        let in_paragraph = matches!(self.last_open_leaf_kind(container), Some(Kind::Paragraph));

        if indent >= TAB_STOP && !in_paragraph {
            cursor.advance_columns(TAB_STOP);
            let parent = self.place(container, &Kind::IndentedCode);
            let index = self.append_child(parent, Node::new(Kind::IndentedCode));
            self.append_text(index, &cursor.rest_with_newline());
            return Some(index);
        }

        if indent < TAB_STOP {
            cursor.skip_indent();
            if cursor.peek() == Some(b'>') {
                cursor.advance_one();
                cursor.consume_optional_space();
                let parent = self.place(container, &Kind::BlockQuote);
                return Some(self.append_child(parent, Node::new(Kind::BlockQuote)));
            }
            // A footnote definition opens here, interrupting any open paragraph; its content is then
            // gathered by the enclosing container loop, the same as a block quote's.
            if self.extensions.contains(Extension::Footnotes)
                && let Some(label) = cursor.footnote_def_marker()
            {
                let key = scan::normalize_label(&label);
                let parent = self.place(container, &Kind::FootnoteDef(key.clone()));
                return Some(self.append_child(parent, Node::new(Kind::FootnoteDef(key))));
            }
            // A fenced div opens on a colon-run line carrying a valid attribute spec; it may
            // interrupt a paragraph. The whole fence line is consumed, so the div opens empty.
            if self.extensions.contains(Extension::FencedDivs) {
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
            if let Some(level) = cursor.atx_heading() {
                let parent = self.place(container, &Kind::Heading(level));
                let index = self.append_child(parent, Node::new(Kind::Heading(level)));
                self.append_text(index, &strip_atx_closing(&cursor.rest()));
                self.close(index);
                return Some(index);
            }
            if let Some(fence) = cursor.fenced_code_start() {
                let kind = Kind::FencedCode(fence);
                let parent = self.place(container, &kind);
                return Some(self.append_child(parent, Node::new(kind)));
            }
            if let Some(kind) = html_block::classify(cursor.remaining(), !in_paragraph) {
                let parent = self.place(container, &Kind::HtmlBlock(kind));
                let index = self.append_child(parent, Node::new(Kind::HtmlBlock(kind)));
                // The start line keeps its leading indentation (always spaces after normalization).
                let line = format!("{}{}", " ".repeat(indent), cursor.rest());
                self.append_text(index, &line);
                self.append_text(index, "\n");
                if html_block::closes(kind, &line) {
                    self.close(index);
                }
                return Some(index);
            }
            if cursor.thematic_break() {
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
            if let Some(list) = self.list_marker(container, indent, cursor) {
                return Some(list);
            }
        }
        None
    }

    fn last_open_leaf_kind(&self, container: usize) -> Option<Kind> {
        let leaf = self.deepest_open(container);
        self.kind(leaf).cloned()
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
        let parsed = cursor.list_marker_at()?;

        // These restrictions apply only when the marker would interrupt a *bare* paragraph (one not
        // already inside a list): an empty item cannot interrupt, and an ordered marker must start
        // at 1. Inside an open list any marker is allowed — a matching one continues the list, a
        // differing one ends it and begins a new sibling list.
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
            if !parsed.bullet && parsed.start != 1 {
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
        self.append_child(parent, Node::new(Kind::List(info)))
    }

    fn list_matches(&self, index: usize, parsed: &ListMarkerParse) -> bool {
        matches!(
            self.kind(index),
            Some(Kind::List(info)) if info.bullet == parsed.bullet && info.marker == parsed.marker
        )
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
            match self.kind(leaf).cloned() {
                Some(Kind::Paragraph) => {
                    self.append_text(leaf, &cursor.rest());
                    self.append_text(leaf, "\n");
                }
                Some(
                    Kind::Heading(_)
                    | Kind::ThematicBreak
                    | Kind::LineBlock
                    | Kind::IndentedCode
                    | Kind::FencedCode(_)
                    | Kind::HtmlBlock(_),
                ) => {}
                _ => {
                    // An opener whose own line carries no content (a bare marker) leaves its
                    // container empty rather than seeding an empty paragraph.
                    let rest = cursor.rest();
                    if !rest.trim().is_empty() {
                        let index = self.append_child(leaf, Node::new(Kind::Paragraph));
                        self.append_text(index, &rest);
                        self.append_text(index, "\n");
                    }
                }
            }
            return;
        }

        // No new block opened: continue an open paragraph, lazily crossing any unmatched
        // containers, or start a fresh paragraph in the matched container.
        let deepest = self.deepest_open(0);
        if matches!(self.kind(deepest), Some(Kind::Paragraph)) {
            self.append_text(deepest, &cursor.rest());
            self.append_text(deepest, "\n");
            return;
        }

        let parent = self.place(container, &Kind::Paragraph);
        let index = self.append_child(parent, Node::new(Kind::Paragraph));
        self.append_text(index, &cursor.rest());
        self.append_text(index, "\n");
    }

    fn append_text(&mut self, index: usize, text: &str) {
        if let Some(node) = self.nodes.get_mut(index) {
            node.text.push_str(text);
        }
    }

    fn finish(mut self) -> (Vec<IrBlock>, RefMap, FootnoteDefs) {
        // Pre-pass: pull link reference definitions out of every paragraph.
        for index in 0..self.nodes.len() {
            if matches!(self.kind(index), Some(Kind::Paragraph)) {
                let text = self.node_text(index);
                let stripped = self.extract_refs(&text);
                if let Some(node) = self.nodes.get_mut(index) {
                    node.text = stripped;
                }
            }
        }
        let footnotes = self.collect_footnotes();
        let blocks = self.build_children(0);
        (blocks, self.refs, footnotes)
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

    fn extract_refs(&mut self, text: &str) -> String {
        let mut remaining = text;
        while let Some((label, def, rest)) = scan::parse_link_reference_definition(remaining) {
            self.refs.entry(label).or_insert(def);
            remaining = rest;
        }
        remaining.to_owned()
    }

    fn build_children(&self, index: usize) -> Vec<IrBlock> {
        let Some(node) = self.nodes.get(index) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &child in &node.children {
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
                if self.extensions.contains(Extension::PipeTables)
                    && let Some((alignments, header, rows)) = table::try_parse(trimmed)
                {
                    IrBlock::Table {
                        alignments,
                        header,
                        rows,
                    }
                } else {
                    IrBlock::Para(trimmed.to_owned())
                }
            }
            Kind::LineBlock => IrBlock::LineBlock(line_block_lines(&node.text)),
            Kind::DefinitionList => self.build_definition_list(index),
            Kind::Heading(level) => IrBlock::Heading(*level, node.text.trim().to_owned()),
            Kind::ThematicBreak => IrBlock::ThematicBreak,
            Kind::IndentedCode => {
                IrBlock::CodeBlock(Attr::default(), strip_trailing_blank_lines(&node.text))
            }
            Kind::FencedCode(fence) => {
                let attr = fence_attr(&fence.info, self.extensions);
                // A closing fence drops the final newline; a block ended by end-of-input (still
                // open) keeps it.
                let text = if node.open {
                    node.text.clone()
                } else {
                    strip_one_trailing_newline(&node.text)
                };
                IrBlock::CodeBlock(attr, text)
            }
            Kind::HtmlBlock(kind) => {
                let mut text = node.text.clone();
                // Kinds 1–5 close only on an explicit end tag; reaching end-of-input with the block
                // still open leaves it unterminated, which surfaces as a trailing blank line in the output.
                if node.open && matches!(kind, 1..=5) {
                    text.push('\n');
                }
                IrBlock::RawHtml(text)
            }
            Kind::BlockQuote => IrBlock::BlockQuote(self.build_children(index)),
            Kind::FencedDiv(info) => IrBlock::Div(info.attr.clone(), self.build_children(index)),
            Kind::List(info) => self.build_list(index, info),
        };
        Some(block)
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
            let attrs = ListAttributes {
                start: info.start,
                style: ListNumberStyle::Decimal,
                delim: info.delim.clone(),
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

/// In a tight list, item paragraphs render as `Plain` rather than `Para`.
fn demote_loose_paragraphs(blocks: &mut [IrBlock]) {
    for block in blocks {
        if let IrBlock::Para(text) = block {
            *block = IrBlock::Plain(std::mem::take(text));
        }
    }
}

enum Continue {
    Matched,
    MatchedLeaf,
    NotMatched,
}

fn list_info(parsed: &ListMarkerParse) -> ListInfo {
    ListInfo {
        bullet: parsed.bullet,
        marker: parsed.marker,
        delim: parsed.delim.clone(),
        start: parsed.start,
    }
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
        && info.get(consumed..).is_some_and(|rest| rest.trim().is_empty())
    {
        return parsed;
    }
    let language = info.split_whitespace().next().unwrap_or("");
    Attr {
        id: String::new(),
        classes: vec![language.to_owned()],
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
        && spec.get(consumed..).is_some_and(|rest| rest.trim().is_empty())
    {
        return Some(attr);
    }
    // Bare-word form: a single whitespace-free token becomes the sole class, kept verbatim (a
    // leading dot is not stripped).
    if spec.chars().any(char::is_whitespace) {
        return None;
    }
    Some(Attr {
        id: String::new(),
        classes: vec![spec.to_owned()],
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

/// Whether a line block's current (final) entry is empty: its last line is a `|` marker carrying no
/// content. A content-bearing line stays non-empty once written, so checking the final line alone is
/// enough — an empty entry is only ever followed by another marker line, never folded into.
fn last_entry_is_empty(text: &str) -> bool {
    let last = text.trim_end_matches('\n').rsplit('\n').next().unwrap_or("");
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
fn strip_atx_closing(content: &str) -> String {
    let trimmed = content.trim_matches([' ', '\t']);
    let without_hashes = trimmed.trim_end_matches('#');
    if without_hashes.len() == trimmed.len() {
        return trimmed.to_owned();
    }
    if without_hashes.is_empty() || without_hashes.ends_with([' ', '\t']) {
        without_hashes.trim_end_matches([' ', '\t']).to_owned()
    } else {
        trimmed.to_owned()
    }
}
