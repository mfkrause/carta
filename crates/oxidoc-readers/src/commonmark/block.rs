//! Block-structure phase: consume input line by line into a tree of [`IrBlock`]s, following the
//! `CommonMark` spec's open-block algorithm (spec appendix "A parsing strategy"). Leaf text is left
//! raw for the inline phase; link reference definitions are stripped from paragraph fronts and
//! collected into the [`RefMap`].

use oxidoc_ast::{Attr, ListAttributes, ListNumberDelim, ListNumberStyle};

use super::{IrBlock, RefMap, inline};

const TAB_STOP: usize = 4;

/// Parse the normalized input into the block tree plus the collected link references.
pub(crate) fn parse(input: &str) -> (Vec<IrBlock>, RefMap) {
    let mut parser = Parser::new();
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

#[derive(Debug, Clone)]
struct FenceInfo {
    marker: u8,
    length: usize,
    indent: usize,
    info: String,
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
}

impl Parser {
    fn new() -> Self {
        Parser {
            nodes: vec![Node::new(Kind::Document)],
            refs: RefMap::new(),
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
                self.convert_paragraph_to_heading(para, level);
                return;
            }
        }

        // The deepest block open before this line opens any new ones. When a container went
        // unmatched in phase 1, this is the tip of the unmatched chain hanging below `matched`.
        let matched = container;
        let old_tip = self.deepest_open(matched);

        // Phase 2: open new blocks at the matched container.
        let mut started_new = false;
        if !blank {
            loop {
                cursor.note_indent();
                if let Some(opened) = self.try_open(container, &mut cursor) {
                    started_new = true;
                    container = opened;
                    if self.accepts_children(container) {
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

    fn accepts_children(&self, index: usize) -> bool {
        matches!(
            self.kind(index),
            Some(Kind::Document | Kind::BlockQuote | Kind::List(_) | Kind::Item(_))
        )
    }

    /// Try to continue an open container (block quote / list item) or open leaf on this line.
    fn try_continue(&mut self, index: usize, cursor: &mut Cursor) -> Continue {
        match self.kind(index).cloned() {
            // A list is a transparent container: it consumes nothing and defers to its items.
            Some(Kind::List(_)) => Continue::Matched,
            Some(Kind::BlockQuote) => {
                cursor.skip_up_to_three_spaces();
                if cursor.peek() == Some(b'>') {
                    cursor.advance_one();
                    cursor.consume_optional_space();
                    Continue::Matched
                } else {
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
                    self.append_text(index, "\n");
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
        let line = cursor.rest_raw();
        self.append_text(index, &line);
        self.append_text(index, "\n");
        if html_block_closes(kind, &line) {
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
            if let Some(kind) = cursor.html_block_start(!in_paragraph) {
                let parent = self.place(container, &Kind::HtmlBlock(kind));
                let index = self.append_child(parent, Node::new(Kind::HtmlBlock(kind)));
                let line = cursor.rest_raw();
                self.append_text(index, &line);
                self.append_text(index, "\n");
                if html_block_closes(kind, &line) {
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
            if let Some(list) = self.list_marker(container, cursor) {
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
    fn list_marker(&mut self, container: usize, cursor: &mut Cursor) -> Option<usize> {
        let marker_indent = cursor.indent();
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
        let content_offset = if parsed.blank_after || after_marker >= TAB_STOP {
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
            // Record the blank against the deepest open block (unless it is an item still on its
            // own marker line), then close any open paragraph.
            let deepest = self.deepest_open(container);
            let empty_item = matches!(self.kind(deepest), Some(Kind::Item(_)))
                && self
                    .nodes
                    .get(deepest)
                    .is_some_and(|n| n.children.is_empty());
            if !empty_item && let Some(node) = self.nodes.get_mut(deepest) {
                node.last_line_blank = true;
            }
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
                    let index = self.append_child(leaf, Node::new(Kind::Paragraph));
                    self.append_text(index, &cursor.rest());
                    self.append_text(index, "\n");
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
                let text = self
                    .nodes
                    .get(index)
                    .map(|n| n.text.clone())
                    .unwrap_or_default();
                let stripped = self.extract_refs(&text);
                if let Some(node) = self.nodes.get_mut(index) {
                    node.text = stripped;
                }
            }
        }
        let blocks = self.build_children(0);
        (blocks, self.refs)
    }

    fn extract_refs(&mut self, text: &str) -> String {
        let mut remaining = text;
        while let Some((label, def, rest)) = inline::parse_link_reference_definition(remaining) {
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
                let text = node.text.trim_end_matches('\n').to_owned();
                IrBlock::CodeBlock(Attr::default(), text)
            }
            Kind::FencedCode(fence) => {
                let attr = fence_attr(&fence.info);
                IrBlock::CodeBlock(attr, strip_one_trailing_newline(&node.text))
            }
            Kind::HtmlBlock(_) => IrBlock::RawHtml(node.text.clone()),
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

#[derive(Debug)]
struct ListMarkerParse {
    bullet: bool,
    marker: u8,
    delim: ListNumberDelim,
    start: i32,
    marker_width: usize,
    blank_after: bool,
}

fn list_info(parsed: &ListMarkerParse) -> ListInfo {
    ListInfo {
        bullet: parsed.bullet,
        marker: parsed.marker,
        delim: parsed.delim.clone(),
        start: parsed.start,
    }
}

fn fence_attr(info: &str) -> Attr {
    let info = info.trim();
    if info.is_empty() {
        return Attr::default();
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

/// Tag names that begin an HTML block of type 6 (terminated by a blank line).
const HTML_BLOCK_TAGS: &[&str] = &[
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

/// Whether `line` satisfies the end condition for an HTML block of the given type. Types 6 and 7
/// end at a blank line instead and are handled by the caller.
fn html_block_closes(kind: u8, line: &str) -> bool {
    match kind {
        1 => {
            let lower = line.to_ascii_lowercase();
            ["</script>", "</pre>", "</style>", "</textarea>"]
                .iter()
                .any(|needle| lower.contains(needle))
        }
        2 => line.contains("-->"),
        3 => line.contains("?>"),
        4 => line.contains('>'),
        5 => line.contains("]]>"),
        _ => false,
    }
}

/// Whether the bytes at `s` begin an HTML block of type 6 (`<`/`</` + block tag name + boundary).
fn html_type6_start(s: &str) -> bool {
    let after = s
        .strip_prefix("</")
        .or_else(|| s.strip_prefix('<'))
        .unwrap_or("");
    let name_len = after.bytes().take_while(u8::is_ascii_alphanumeric).count();
    let Some(name) = after.get(..name_len) else {
        return false;
    };
    if name.is_empty() {
        return false;
    }
    let tail = after.get(name_len..).unwrap_or("");
    let boundary = tail.is_empty() || tail.starts_with([' ', '\t', '>']) || tail.starts_with("/>");
    boundary && HTML_BLOCK_TAGS.contains(&name.to_ascii_lowercase().as_str())
}

/// Length in bytes of a complete HTML open or closing tag at the start of `s`, if any. Used for
/// HTML block type 7, which requires a complete tag spanning the rest of the line.
fn scan_complete_tag(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    if bytes.get(1) == Some(&b'/') {
        let mut index = 2;
        if !bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
            return None;
        }
        index += 1;
        while bytes
            .get(index)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            index += 1;
        }
        while bytes.get(index).is_some_and(|b| matches!(b, b' ' | b'\t')) {
            index += 1;
        }
        return (bytes.get(index) == Some(&b'>')).then_some(index + 1);
    }
    let mut index = 1;
    if !bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    index += 1;
    while bytes
        .get(index)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
    {
        index += 1;
    }
    loop {
        let mut whitespace = 0;
        while bytes.get(index).is_some_and(|b| matches!(b, b' ' | b'\t')) {
            index += 1;
            whitespace += 1;
        }
        let name_ok = bytes
            .get(index)
            .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'_' | b':'));
        if whitespace == 0 || !name_ok {
            index -= whitespace;
            break;
        }
        index += 1;
        while bytes
            .get(index)
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
        {
            index += 1;
        }
        index = scan_optional_attribute_value(bytes, index)?;
    }
    while bytes.get(index).is_some_and(|b| matches!(b, b' ' | b'\t')) {
        index += 1;
    }
    if bytes.get(index) == Some(&b'/') {
        index += 1;
    }
    (bytes.get(index) == Some(&b'>')).then_some(index + 1)
}

/// Consume an optional `= value` attribute tail; returns the new index, or `None` if a value is
/// started but malformed (unterminated quote / empty unquoted value).
fn scan_optional_attribute_value(bytes: &[u8], start: usize) -> Option<usize> {
    let mut probe = start;
    while bytes.get(probe).is_some_and(|b| matches!(b, b' ' | b'\t')) {
        probe += 1;
    }
    if bytes.get(probe) != Some(&b'=') {
        return Some(start);
    }
    probe += 1;
    while bytes.get(probe).is_some_and(|b| matches!(b, b' ' | b'\t')) {
        probe += 1;
    }
    match bytes.get(probe) {
        Some(quote @ (b'"' | b'\'')) => {
            let quote = *quote;
            probe += 1;
            while bytes.get(probe).is_some_and(|b| *b != quote) {
                probe += 1;
            }
            (bytes.get(probe) == Some(&quote)).then(|| probe + 1)
        }
        Some(_) => {
            let value_start = probe;
            while bytes.get(probe).is_some_and(|b| {
                !matches!(b, b' ' | b'\t' | b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')
            }) {
                probe += 1;
            }
            (probe > value_start).then_some(probe)
        }
        None => None,
    }
}

/// A tab-aware cursor over a single input line.
struct Cursor<'a> {
    bytes: &'a [u8],
    line: &'a str,
    offset: usize,
    column: usize,
    indent_mark: usize,
}

impl<'a> Cursor<'a> {
    fn new(line: &'a str) -> Self {
        Cursor {
            bytes: line.as_bytes(),
            line,
            offset: 0,
            column: 0,
            indent_mark: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    fn advance_one(&mut self) {
        if let Some(byte) = self.peek() {
            self.offset += 1;
            self.column += if byte == b'\t' {
                TAB_STOP - (self.column % TAB_STOP)
            } else {
                1
            };
        }
    }

    fn consume_optional_space(&mut self) {
        match self.peek() {
            Some(b' ') => {
                self.offset += 1;
                self.column += 1;
            }
            Some(b'\t') => {
                self.offset += 1;
                self.column += TAB_STOP - (self.column % TAB_STOP);
            }
            _ => {}
        }
    }

    /// Visual columns of whitespace from the current position to the first non-space.
    fn indent(&self) -> usize {
        let mut column = self.column;
        let mut offset = self.offset;
        while let Some(byte) = self.bytes.get(offset) {
            match byte {
                b' ' => column += 1,
                b'\t' => column += TAB_STOP - (column % TAB_STOP),
                _ => break,
            }
            offset += 1;
        }
        column - self.column
    }

    fn note_indent(&mut self) {
        self.indent_mark = self.indent();
    }

    fn is_blank(&self) -> bool {
        self.bytes
            .get(self.offset..)
            .unwrap_or(&[])
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t'))
    }

    fn skip_up_to_three_spaces(&mut self) {
        let mut consumed = 0;
        while consumed < 3 {
            match self.peek() {
                Some(b' ') => {
                    self.offset += 1;
                    self.column += 1;
                    consumed += 1;
                }
                _ => break,
            }
        }
    }

    fn skip_indent(&mut self) {
        while let Some(byte) = self.peek() {
            match byte {
                b' ' => {
                    self.offset += 1;
                    self.column += 1;
                }
                b'\t' => {
                    self.offset += 1;
                    self.column += TAB_STOP - (self.column % TAB_STOP);
                }
                _ => break,
            }
        }
    }

    /// Advance past `count` characters regardless of kind, used to consume a list marker.
    fn advance_chars(&mut self, count: usize) {
        for _ in 0..count {
            self.advance_one();
        }
    }

    /// Advance by up to `count` visual columns of leading whitespace, stopping at non-whitespace.
    fn advance_columns(&mut self, count: usize) {
        let target = self.column + count;
        while self.column < target {
            match self.peek() {
                Some(b' ') => {
                    self.offset += 1;
                    self.column += 1;
                }
                Some(b'\t') => {
                    let width = TAB_STOP - (self.column % TAB_STOP);
                    if self.column + width > target {
                        // Partial tab: consume it but overshoot is acceptable for indentation.
                        self.offset += 1;
                        self.column += width;
                    } else {
                        self.offset += 1;
                        self.column += width;
                    }
                }
                _ => break,
            }
        }
    }

    fn advance_up_to_columns(&mut self, count: usize) {
        let target = self.column + count;
        while self.column < target {
            match self.peek() {
                Some(b' ') => {
                    self.offset += 1;
                    self.column += 1;
                }
                Some(b'\t') => {
                    let width = TAB_STOP - (self.column % TAB_STOP);
                    if self.column + width > target {
                        break;
                    }
                    self.offset += 1;
                    self.column += width;
                }
                _ => break,
            }
        }
    }

    /// The remaining line content from the cursor, as-is.
    fn rest(&self) -> String {
        self.line.get(self.offset..).unwrap_or("").to_owned()
    }

    fn rest_raw(&self) -> String {
        self.rest()
    }

    fn rest_with_newline(&self) -> String {
        let mut out = self.rest();
        out.push('\n');
        out
    }

    fn atx_heading(&mut self) -> Option<i32> {
        let start = self.offset;
        let start_col = self.column;
        let mut hashes = 0;
        while self.peek() == Some(b'#') {
            self.advance_one();
            hashes += 1;
        }
        if hashes == 0 || hashes > 6 {
            self.offset = start;
            self.column = start_col;
            return None;
        }
        match self.peek() {
            None => Some(hashes),
            Some(b' ' | b'\t') => {
                self.consume_optional_space();
                Some(hashes)
            }
            _ => {
                self.offset = start;
                self.column = start_col;
                None
            }
        }
    }

    /// If the remaining line begins an HTML block, return its type (1–7) per `CommonMark` §4.6. The
    /// cursor is assumed positioned at the first non-space. Type 7 cannot interrupt a paragraph.
    fn html_block_start(&self, can_interrupt_paragraph: bool) -> Option<u8> {
        let rest = self.line.get(self.offset..)?;
        if !rest.starts_with('<') {
            return None;
        }
        if rest.starts_with("<!--") {
            return Some(2);
        }
        if rest.starts_with("<?") {
            return Some(3);
        }
        if rest.starts_with("<![CDATA[") {
            return Some(5);
        }
        if rest
            .strip_prefix("<!")
            .is_some_and(|after| after.starts_with(|c: char| c.is_ascii_alphabetic()))
        {
            return Some(4);
        }
        let lower = rest.to_ascii_lowercase();
        for tag in ["script", "pre", "style", "textarea"] {
            if let Some(after) = lower.strip_prefix('<').and_then(|r| r.strip_prefix(tag))
                && (after.is_empty() || after.starts_with([' ', '\t', '>']))
            {
                return Some(1);
            }
        }
        if html_type6_start(rest) {
            return Some(6);
        }
        if can_interrupt_paragraph
            && let Some(len) = scan_complete_tag(rest)
            && rest.get(len..).is_some_and(|tail| tail.trim().is_empty())
        {
            return Some(7);
        }
        None
    }

    /// If the remaining line is a setext heading underline, return its level (1 for `=`, 2 for
    /// `-`). The caller has already ensured the leading indent is under four columns.
    fn setext_underline(&self) -> Option<i32> {
        let rest = self.bytes.get(self.offset..).unwrap_or(&[]);
        let mut index = 0;
        while rest.get(index) == Some(&b' ') {
            index += 1;
        }
        let marker = *rest.get(index)?;
        if marker != b'=' && marker != b'-' {
            return None;
        }
        let mut count = 0;
        while rest.get(index) == Some(&marker) {
            index += 1;
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let trailing_ok = rest
            .get(index..)
            .is_some_and(|tail| tail.iter().all(|byte| matches!(byte, b' ' | b'\t')));
        if !trailing_ok {
            return None;
        }
        Some(if marker == b'=' { 1 } else { 2 })
    }

    fn thematic_break(&self) -> bool {
        let rest = self.bytes.get(self.offset..).unwrap_or(&[]);
        let mut marker = None;
        let mut count = 0;
        for &byte in rest {
            match byte {
                b' ' | b'\t' => {}
                b'-' | b'_' | b'*' => {
                    if let Some(existing) = marker {
                        if existing != byte {
                            return false;
                        }
                    } else {
                        marker = Some(byte);
                    }
                    count += 1;
                }
                _ => return false,
            }
        }
        marker.is_some() && count >= 3
    }

    fn fenced_code_start(&mut self) -> Option<FenceInfo> {
        let indent = self.indent_mark;
        let marker = self.peek()?;
        if marker != b'`' && marker != b'~' {
            return None;
        }
        let start = self.offset;
        let start_col = self.column;
        let mut length = 0;
        while self.peek() == Some(marker) {
            self.advance_one();
            length += 1;
        }
        if length < 3 {
            self.offset = start;
            self.column = start_col;
            return None;
        }
        let info = self.rest();
        // A backtick fence's info string may not contain a backtick.
        if marker == b'`' && info.contains('`') {
            self.offset = start;
            self.column = start_col;
            return None;
        }
        Some(FenceInfo {
            marker,
            length,
            indent,
            info: unescape_info(info.trim()),
        })
    }

    fn is_closing_fence(&self, marker: u8, min_length: usize) -> bool {
        let rest = self.bytes.get(self.offset..).unwrap_or(&[]);
        let mut count = 0;
        let mut index = 0;
        // Skip leading indentation already handled by caller via indent() check.
        while rest.get(index).copied() == Some(b' ') {
            index += 1;
        }
        while rest.get(index).copied() == Some(marker) {
            count += 1;
            index += 1;
        }
        if count < min_length {
            return false;
        }
        rest.get(index..)
            .is_some_and(|tail| tail.iter().all(|byte| matches!(byte, b' ' | b'\t')))
    }

    fn list_marker_at(&mut self) -> Option<ListMarkerParse> {
        let byte = self.peek()?;
        match byte {
            b'-' | b'+' | b'*' => {
                // Distinguish from a thematic break.
                if self.thematic_break() {
                    return None;
                }
                let blank_after = self.bytes.get(self.offset + 1).is_none_or(|b| *b == b'\n');
                let followed_ok =
                    matches!(self.bytes.get(self.offset + 1), None | Some(b' ' | b'\t'));
                if !followed_ok {
                    return None;
                }
                Some(ListMarkerParse {
                    bullet: true,
                    marker: byte,
                    delim: ListNumberDelim::DefaultDelim,
                    start: 1,
                    marker_width: 1,
                    blank_after,
                })
            }
            b'0'..=b'9' => self.ordered_marker_at(),
            _ => None,
        }
    }

    fn ordered_marker_at(&mut self) -> Option<ListMarkerParse> {
        let mut digits = 0;
        let mut value: i64 = 0;
        while let Some(byte) = self.bytes.get(self.offset + digits) {
            if byte.is_ascii_digit() {
                value = value * 10 + i64::from(byte - b'0');
                digits += 1;
            } else {
                break;
            }
        }
        if digits == 0 || digits > 9 {
            return None;
        }
        let delim_byte = self.bytes.get(self.offset + digits).copied();
        let delim = match delim_byte {
            Some(b'.') => ListNumberDelim::Period,
            Some(b')') => ListNumberDelim::OneParen,
            _ => return None,
        };
        let after = self.bytes.get(self.offset + digits + 1);
        let blank_after = after.is_none_or(|b| *b == b'\n');
        if !matches!(after, None | Some(b' ' | b'\t')) {
            return None;
        }
        let start = i32::try_from(value).unwrap_or(1);
        Some(ListMarkerParse {
            bullet: false,
            marker: delim_byte.unwrap_or(b'.'),
            delim,
            start,
            marker_width: digits + 1,
            blank_after,
        })
    }
}

fn unescape_info(info: &str) -> String {
    inline::unescape_string(info)
}

/// Parse a leading link reference definition from `text`, returning the normalized label, the
/// definition, and the unconsumed remainder. Defined in the inline module to share its escaping
/// and destination scanners.
mod refs {}
