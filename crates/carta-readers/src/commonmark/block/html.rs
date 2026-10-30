//! Block-level HTML: parsed-content elements, raw spans, and their opening and closing.

use super::{Cursor, Extension, Kind, Node, Parser, html_block, html_element};

impl Parser {
    /// Raw HTML reading for elements without parsed inner content.
    pub(super) fn markdown_raw_html(&self) -> bool {
        self.greedy_paragraphs && !self.extensions.contains(Extension::MarkdownInHtmlBlocks)
    }

    /// In the Markdown family reading raw HTML, a block-level HTML tag that starts the line opens a
    /// raw block. A non-self-closing open tag with a balanced matching close is a span running to that
    /// close (nested same-name tags and blank lines included); every other block tag (self-closing,
    /// void with no close ahead, or a bare close tag) is a single-line raw block. With
    /// `markdown_attribute` the block reads like the other block openers: it accepts up to three
    /// columns of indentation and interrupts an open paragraph (which then reads tight); without it
    /// the tag must stand at column zero and folds into an open paragraph as ordinary text instead.
    pub(super) fn open_markdown_raw_html(
        &mut self,
        container: usize,
        indent: usize,
        in_paragraph: bool,
        cursor: &mut Cursor,
        following: &[&str],
        line_index: Option<usize>,
    ) -> Option<usize> {
        enum Opener {
            /// A single-line raw block holding `line[..end]`.
            Single(usize),
            /// A multi-line span with the given tag name and open-nesting depth after its first line.
            Span(String, usize, Option<html_element::HtmlLiteral>),
        }
        let interrupts = self.extensions.contains(Extension::MarkdownAttribute);
        let max_indent = if interrupts { 3 } else { 0 };
        if !self.markdown_raw_html() || indent > max_indent || (in_paragraph && !interrupts) {
            return None;
        }
        let line = cursor.remaining().to_owned();
        let opener = if let Some(open) = html_element::parse_open_tag(&line) {
            let after_tag = line.get(open.len..).unwrap_or("");
            let mut literal = None;
            let (line_depth, same_line) = if open.self_closing {
                (0, None)
            } else {
                html_element::scan_depth(after_tag, &open.tag, 1, &mut literal)
            };
            match same_line {
                Some(offset) => Opener::Single(open.len + offset),
                // Self-closing/void, or no balanced close ahead: the tag alone is the raw block.
                None if line_depth == 0
                    || !self.raw_html_span_closes(
                        container, line_depth, &open.tag, following, literal, line_index,
                    ) =>
                {
                    Opener::Single(open.len)
                }
                None => Opener::Span(open.tag, line_depth, literal),
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
            Opener::Span(tag, depth, literal) => {
                let kind = Kind::RawHtmlSpan {
                    tag,
                    depth,
                    literal,
                };
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
    pub(super) fn emit_raw_html_leaf(&mut self, container: usize, line: &str, end: usize) -> usize {
        let no_close = self
            .last_html_close_line
            .is_none_or(|last| last <= self.source_line)
            && line.get(end..).is_none_or(|rest| !rest.contains("</"));
        let first = self.nodes.len();
        let mut line = line;
        let mut end = end;
        loop {
            let kind = Kind::RawHtmlSpan {
                tag: String::new(),
                depth: 0,
                literal: None,
            };
            let parent = self.place(container, &kind);
            let index = self.append_child(parent, Node::new(kind));
            self.append_text(index, line.get(..end).unwrap_or(line));
            self.close(index);
            let rest = line.get(end..).unwrap_or("");
            if no_close && rest.starts_with('<') {
                let raw_text = html_block::raw_text_tag(rest).is_some();
                let raw_element = self.markdown_raw_html()
                    && html_element::parse_open_tag(rest).is_some_and(|open| {
                        open.tag != "div" || !self.extensions.contains(Extension::NativeDivs)
                    });
                if (raw_text || raw_element)
                    && let Some(next) = super::scan::scan_html_tag_end(rest, 0)
                {
                    line = rest;
                    end = next;
                    continue;
                }
            }
            if !rest.trim().is_empty() {
                self.process_line(rest, &[], None);
            }
            return first;
        }
    }

    /// Whether a raw HTML span opened at `depth` finds its balanced close somewhere in the following
    /// lines (read at the container's own indentation). A span cannot outlast its container.
    pub(super) fn raw_html_span_closes(
        &self,
        container: usize,
        depth: usize,
        tag: &str,
        following: &[&str],
        mut literal: Option<html_element::HtmlLiteral>,
        line_index: Option<usize>,
    ) -> bool {
        if line_index
            .is_some_and(|current| self.last_html_close_line.is_none_or(|last| last <= current))
        {
            return false;
        }
        let path = self.container_path(container);
        let mut depth = depth;
        for line in following {
            let mut cursor = Cursor::new(line);
            if !self.strip_container_path(&path, &mut cursor) {
                return false;
            }
            let (next, close) =
                html_element::scan_depth(cursor.remaining(), tag, depth, &mut literal);
            if close.is_some() {
                return true;
            }
            depth = next;
        }
        false
    }

    /// Checks closing candidates without rescanning the remaining document for each opener.
    pub(super) fn html_reaches_close(
        &self,
        kind: u8,
        line: &str,
        following: &[&str],
        line_index: Option<usize>,
    ) -> bool {
        if html_block::closes(kind, line) {
            return true;
        }
        if line_index.is_none() && !following.is_empty() {
            return following.iter().any(|line| html_block::closes(kind, line));
        }
        let current = line_index.unwrap_or(self.source_line);
        self.html_closing_lines
            .get(usize::from(kind))
            .copied()
            .flatten()
            .is_some_and(|last| last > current)
    }

    /// If this line carries the matching close tag of the innermost open HTML element, close that
    /// element and return `true`. Content preceding the tag on its line is fed as the element's final
    /// content (which is then tightened to `Plain`); content after the tag is re-fed as a fresh line.
    pub(super) fn close_html_element(&mut self, container: usize, cursor: &Cursor) -> bool {
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
        // Divs tighten at a content-trailed close; raw elements tighten without a preceding blank.
        let tighten = if as_div {
            trails
        } else {
            matches!(self.kind(self.deepest_open(element)), Some(Kind::Paragraph))
        };
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
    pub(super) fn innermost_open_html_element(&self) -> Option<usize> {
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

    pub(super) fn continue_html(&mut self, index: usize, kind: u8, cursor: &mut Cursor) {
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
    pub(super) fn continue_raw_html_span(
        &mut self,
        index: usize,
        tag: &str,
        depth: usize,
        mut literal: Option<html_element::HtmlLiteral>,
        cursor: &mut Cursor,
    ) {
        let line = cursor.remaining();
        let (new_depth, close) = html_element::scan_depth(line, tag, depth, &mut literal);
        if let Some(offset) = close {
            let kept = line.get(..offset).unwrap_or(line).to_owned();
            let rest = line.get(offset..).unwrap_or("").to_owned();
            self.append_text(index, &kept);
            self.set_raw_html_state(index, 0, literal);
            self.close(index);
            if !rest.trim().is_empty() {
                self.process_line(&rest, &[], None);
            }
        } else {
            self.append_text(index, line);
            self.append_text(index, "\n");
            self.set_raw_html_state(index, new_depth, literal);
        }
    }

    pub(super) fn set_raw_html_state(
        &mut self,
        index: usize,
        value: usize,
        content: Option<html_element::HtmlLiteral>,
    ) {
        if let Some(node) = self.nodes.get_mut(index)
            && let Kind::RawHtmlSpan { depth, literal, .. } = &mut node.kind
        {
            *depth = value;
            *literal = content;
        }
    }
}
