//! Opening new blocks: the block-opener dispatch, lists, definitions, and line placement.

use super::{
    Cursor, DivInfo, Extension, ItemInfo, Kind, ListMarkerParse, ListNumberDelim, ListNumberStyle,
    Node, Parser, TAB_STOP, continues_ordered, demote_lone_roman, div_open_fence, grid, html_block,
    list_info, scan, strip_atx_closing, table, texttable,
};

impl Parser {
    /// Try to open a new block at the current cursor position inside `container`.
    // A flat dispatch over the block openers, tried in precedence order; it reads best as one sequence.
    #[allow(clippy::too_many_lines)]
    pub(super) fn try_open(
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
            // Folds into a greedy paragraph unless it is an open definition's body, which it ends.
            if (!gates.foldable || self.footnote_def_open_below(container))
                && self.extensions.contains(Extension::Footnotes)
                && let Some(label) = cursor.footnote_def_marker()
            {
                let key = scan::normalize_label(&label);
                let parent = self.place(container, &Kind::FootnoteDef(key.clone()));
                return Some(self.append_child(parent, Node::new(Kind::FootnoteDef(key))));
            }
            // The whole fence line is consumed, so the div opens empty.
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
            // Markdown dialect: hashes at the left margin, and without `space_in_atx_header` a hash
            // run glued to text opens a heading; CommonMark allows indent <= 3 and needs the space.
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
                // Markdown family: a tilde fence folds into the paragraph, a backtick interrupts.
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
                    // No code block opens: the fence folds into a paragraph up to its close.
                    if self.greedy_paragraphs {
                        self.fence_fold = Some(fence);
                    }
                    cursor.reset_to(fence_checkpoint);
                }
            }
            // A parsed-content HTML element takes precedence over the raw HTML-block reading.
            if let Some(block) = self.open_html_element(container, indent, cursor) {
                return Some(block);
            }
            // Unparsed inner HTML: a block tag spans to its balanced close as one raw block.
            if let Some(block) =
                self.open_markdown_raw_html(container, indent, in_paragraph, cursor, following)
            {
                return Some(block);
            }
            // Kinds 6/7 were handled above; only container-agnostic kinds form a raw block here.
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
            // A dash ruling in a fresh block opens a header-less table candidate, preempting the
            // thematic break it would otherwise be (restored when no table forms).
            if self.text_tables_enabled()
                && !in_paragraph
                && texttable::opens_dash_table(cursor.remaining())
            {
                let parent = self.place(container, &Kind::TextTable);
                let index = self.append_child(parent, Node::new(Kind::TextTable));
                // Columns are positional: the ruling keeps its indent to share the rows' margin.
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
            // Turns the preceding paragraph into a term, or adds a definition to the open entry.
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
            // A definition, example, or enumerator shape breaks a greedy paragraph even when no
            // construct opens there; `2)`-style decimal enumerators stay prose.
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

    pub(super) fn last_open_leaf_kind(&self, container: usize) -> Option<&Kind> {
        let leaf = self.deepest_open(container);
        self.kind(leaf)
    }

    /// Open a definition body at a `:`/`~` marker, returning the new [`Kind::Definition`] container
    /// the enclosing loop then fills. The marker either starts a fresh definition of the open entry
    /// (when the cursor sits directly in a [`Kind::DefinitionItem`]) or, when the container's last
    /// child is a paragraph, turns that paragraph into a term, extending an immediately preceding
    /// definition list or beginning a new one. A marker in any other position is not consumed.
    pub(super) fn open_definition(
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

    /// Turn the container's last child (which must be a non-empty paragraph) into a definition
    /// term, returning the [`Kind::DefinitionItem`] that now holds it. The term joins an immediately
    /// preceding definition list, else opens a new one. Returns `None` when there is no term
    /// paragraph to consume, leaving the marker as ordinary text.
    pub(super) fn start_definition_item(&mut self, container: usize) -> Option<usize> {
        let &term_index = self.nodes.get(container)?.children.last()?;
        let term_node = self.nodes.get(term_index)?;
        if !matches!(term_node.kind, Kind::Paragraph) || term_node.text.trim().is_empty() {
            return None;
        }
        // A grid/pipe-table paragraph is not a term; a following `:` line is its caption.
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
    pub(super) fn reopen_definition_list(&mut self, list: usize) {
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
    /// its body begins one column past the marker: deferred indented lines join it as their own
    /// paragraph rather than continuing it.
    pub(super) fn consume_definition_marker(
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
    pub(super) fn list_marker(
        &mut self,
        container: usize,
        marker_indent: usize,
        cursor: &mut Cursor,
    ) -> Option<usize> {
        let fancy = self.extensions.contains(Extension::FancyLists);
        let example = self.extensions.contains(Extension::ExampleLists);
        // Greedy dialect without fancy lists: `#.` works, `)`-delimited enumerators stay prose.
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

        // Interrupting a bare paragraph (a direct child, not inside a list): no empty item, and
        // only a decimal `1.`/`1)` enumerator; other enumerators read too easily as prose.
        let in_paragraph = self
            .last_open_child(container)
            .is_some_and(|child| matches!(self.kind(child), Some(Kind::Paragraph)));
        let inside_list = matches!(self.kind(container), Some(Kind::List(_)));
        if in_paragraph && !inside_list {
            if parsed.blank_after {
                return None;
            }
            // Greedy mode reaches this only for a sublist marker, opening regardless of enumerator.
            let decimal_one = matches!(parsed.style, ListNumberStyle::Decimal) && parsed.start == 1;
            if !self.greedy_paragraphs && !parsed.bullet && !decimal_one {
                return None;
            }
        }

        cursor.advance_chars(parsed.marker_width);
        let after_marker = cursor.indent();
        // 1-4 spaces widen the content indent; 5+ collapse to one (the rest reads as indented code).
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
    pub(super) fn ensure_list(&mut self, container: usize, parsed: &ListMarkerParse) -> usize {
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
        // A lone `i`/`I` directly after another list reads as alphabetic, elsewhere as roman.
        let info = if self.preceding_is_list(parent) {
            demote_lone_roman(info)
        } else {
            info
        };
        self.append_child(parent, Node::new(Kind::List(info)))
    }

    /// Whether `parent`'s last child (open or closed) is a list, so a new sibling list abuts it.
    pub(super) fn preceding_is_list(&self, parent: usize) -> bool {
        self.nodes
            .get(parent)
            .and_then(|node| node.children.last().copied())
            .is_some_and(|child| matches!(self.kind(child), Some(Kind::List(_))))
    }

    pub(super) fn list_matches(&self, index: usize, parsed: &ListMarkerParse) -> bool {
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
        info.delim == parsed.delim && continues_ordered(info.style, parsed)
    }

    pub(super) fn add_line(
        &mut self,
        container: usize,
        started_new: bool,
        blank: bool,
        cursor: &mut Cursor,
    ) {
        if blank {
            let deepest = self.deepest_open(container);
            if matches!(self.kind(deepest), Some(Kind::Paragraph)) {
                self.close(deepest);
            }
            return;
        }

        if started_new {
            // Only a fresh paragraph or a container with trailing content still needs this text.
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
                    // A bare marker leaves its container empty, not seeding an empty paragraph.
                    if !cursor.remaining().trim().is_empty() {
                        let index = self.append_child(leaf, Node::new(Kind::Paragraph));
                        self.note_paragraph_indent(index, cursor);
                        self.append_line(index, cursor);
                    }
                }
            }
            return;
        }

        // Continue an open paragraph (lazily) or start a fresh one in the matched container.
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
    pub(super) fn note_paragraph_indent(&mut self, index: usize, cursor: &Cursor) {
        let indent = cursor.noted_indent();
        if let Some(node) = self.nodes.get_mut(index)
            && node.text.is_empty()
        {
            node.indent = indent;
        }
    }
}
