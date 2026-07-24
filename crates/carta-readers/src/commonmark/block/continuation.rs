//! Container and leaf continuation: matching open blocks against each line and fence gating.

use super::{
    Continue, Cursor, Extension, FenceInfo, GreedyGates, ItemInfo, Kind, Parser, TAB_STOP,
};

impl Parser {
    /// Try to continue an open container (block quote / list item) or open leaf on this line.
    pub(super) fn try_continue(&mut self, index: usize, cursor: &mut Cursor) -> Continue {
        match self.kind(index) {
            // Transparent containers: consume nothing, defer to their items.
            Some(Kind::List(_) | Kind::DefinitionList | Kind::DefinitionItem { .. }) => {
                Continue::Matched
            }
            // Transparent; its close tag is detected separately in `process_line`.
            Some(Kind::HtmlElement(_)) => Continue::Matched,
            // Like a list item, except an empty body survives a blank line (deferred paragraph).
            Some(Kind::Definition { indent }) => {
                let indent = *indent;
                self.continue_item_like(index, indent, true, cursor)
            }
            // Re-bases content to its opening column; closed by `process_line` or end of input.
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
                    // Restore speculative indent so phase 2 sees the line's true indent.
                    cursor.reset_to(checkpoint);
                    Continue::NotMatched
                }
            }
            Some(Kind::Item(info)) => {
                let indent = info.indent;
                self.continue_item_like(index, indent, false, cursor)
            }
            // Continues under a four-column indent; an unindented non-blank line ends it.
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
    /// `blank_keeps_empty` decides: a list item ends, while a definition body waits for a deferred
    /// indented paragraph.
    pub(super) fn continue_item_like(
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

    pub(super) fn continue_fenced(
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

    /// How a greedy paragraph (the markdown dialect) absorbs a following block opener. The foldable
    /// openers (a block quote, heading, thematic break, fenced div, or footnote definition)
    /// continue an open paragraph as a lazy line rather than interrupting it, even across a container
    /// the line did not match (`> a` then `# b`); only a blank line, a fenced code block, or an HTML
    /// block ends it. The block-quote and heading folds are gated further on the
    /// `blank_before_blockquote` and `blank_before_header` toggles, so dropping a toggle lets that
    /// opener interrupt again. A list marker is structural: it still opens a sibling item in an open
    /// list or a sublist inside an item, and folds only where it would otherwise *start* a fresh list:
    /// when the paragraph is the container's own last child and the container is not itself a list
    /// item or other indented item body. The `lists_without_preceding_blankline` toggle drops that
    /// last fold, so a fresh list interrupts the paragraph instead.
    pub(super) fn greedy_gates(&self, container: usize, in_paragraph: bool) -> GreedyGates {
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
    /// extension (a backtick fence on `backtick_code_blocks`, a tilde fence on `fenced_code_blocks`)
    /// and, lacking any extension that gives a richer info string meaning, requires the info string
    /// to be a single bare language token: an info string carrying inner whitespace or a brace then
    /// names no language and the fence is left to fold into a paragraph.
    pub(super) fn fence_opener_accepted(&self, fence: &FenceInfo) -> bool {
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
        // Attribute or raw-output extensions give non-bare info strings meaning; else only a bare
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
    /// In the Markdown dialect a fenced code block must be closed: an unclosed fence (one that would
    /// run to the container's end) does not open, and its lines fold into a paragraph instead. Pure
    /// `CommonMark` lets an unclosed fence run to the end, so there a fence always opens.
    ///
    /// The closing fence is judged at the fence's own container level, so each look-ahead line first
    /// replays the open containers' continuation markers; a line that breaks the chain (a block quote
    /// losing its `>`, a list item losing its indent) cannot carry the close.
    pub(super) fn fence_reaches_close(
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
        // At the root no prefix breaks the chain, so the candidate index answers in O(log n);
        // nested containers keep the linear replay.
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
    pub(super) fn container_path(&self, container: usize) -> Vec<usize> {
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
    pub(super) fn strip_container_path(&self, path: &[usize], cursor: &mut Cursor) -> bool {
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
}
