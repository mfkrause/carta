//! Block-structure phase: consume input line by line into a tree of [`IrBlock`]s, following the
//! `CommonMark` spec's open-block algorithm (spec appendix "A parsing strategy"). Leaf text is left
//! raw for the inline phase; link reference definitions are stripped from paragraph fronts and
//! collected into the [`RefMap`].

use carta_ast::{Attr, ListAttributes, ListNumberDelim, ListNumberStyle};
use carta_core::{Extension, Extensions};

use super::cursor::{Cursor, FenceInfo, ListMarkerParse};
use super::{IrBlock, RefMap, TAB_STOP, attr, html_block, scan};

/// Parse the normalized input into the block tree plus the collected link references.
pub(crate) fn parse(input: &str, extensions: Extensions) -> (Vec<IrBlock>, RefMap) {
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
    Paragraph,
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
            Some(Kind::Document | Kind::BlockQuote | Kind::Item(_)) => {
                !matches!(kind, Kind::Item(_))
            }
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

    fn process_line(&mut self, line: &str) {
        let mut cursor = Cursor::new(line);

        // Phase 1: descend through open *containers*, matching each one's continuation marker.
        // Leaves terminate the descent — they are handled below, not entered as containers.
        let mut container = 0;
        let mut all_matched = true;
        loop {
            let Some(child) = self.last_open_child(container) else {
                break;
            };
            if !self.is_container(child) {
                break;
            }
            match self.try_continue(child, &mut cursor) {
                Continue::Matched => container = child,
                Continue::NotMatched => {
                    all_matched = false;
                    break;
                }
                Continue::MatchedLeaf => return,
            }
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

        // The deepest block open before this line opens any new ones. When a container went
        // unmatched in phase 1, this is the tip of the unmatched chain hanging below `matched`.
        let matched = container;
        let old_tip = self.deepest_open(matched);

        // Record the blank against the block it trails — the deepest already-finalized block, or
        // the still-open container if it has no content yet. This drives loose-list classification:
        // a blank after an item's last block (or after an empty item) marks the line blank even
        // though the block was closed before this line was read.
        if blank {
            let target = self.blank_trails(old_tip);
            if let Some(node) = self.nodes.get_mut(target) {
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
                    if self.is_container(container) {
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
    fn blank_trails(&self, mut index: usize) -> usize {
        while let Some(&last) = self.nodes.get(index).and_then(|node| node.children.last()) {
            if self.nodes.get(last).is_some_and(|node| !node.open) {
                index = last;
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

    fn is_container(&self, index: usize) -> bool {
        matches!(
            self.kind(index),
            Some(Kind::Document | Kind::BlockQuote | Kind::List(_) | Kind::Item(_))
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
            // A list is a transparent container: it consumes nothing and defers to its items.
            Some(Kind::List(_)) => Continue::Matched,
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
                if cursor.is_blank() {
                    // A blank line continues the item only if it already has content.
                    if self.nodes.get(index).is_some_and(|n| n.children.is_empty()) {
                        Continue::NotMatched
                    } else {
                        Continue::Matched
                    }
                } else if cursor.indent() >= info.indent {
                    cursor.advance_columns(info.indent);
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
        let in_paragraph = matches!(self.last_open_leaf_kind(container), Some(Kind::Paragraph));
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

    fn finish(mut self) -> (Vec<IrBlock>, RefMap) {
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
        let blocks = self.build_children(0);
        (blocks, self.refs)
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
            Kind::Document | Kind::Item(_) => return None,
            Kind::Paragraph => {
                let trimmed = node.text.trim();
                if trimmed.is_empty() {
                    return None;
                }
                IrBlock::Para(trimmed.to_owned())
            }
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
            Kind::List(info) => self.build_list(index, info),
        };
        Some(block)
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

fn strip_one_trailing_newline(text: &str) -> String {
    text.strip_suffix('\n').unwrap_or(text).to_owned()
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
