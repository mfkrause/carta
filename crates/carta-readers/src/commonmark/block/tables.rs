//! Table and line-block continuation: pipe, grid, and dash-ruled tables plus line blocks.

use super::{
    Cursor, Extension, Kind, Node, Parser, grid, is_line_block_marker, is_thematic_dash_line,
    last_entry_is_empty, last_nonempty_line, owned_lines, single_line, split_table_lines, table,
    texttable,
};

impl Parser {
    /// Let an in-progress grid table claim its `+`/`|` continuation lines before the block openers
    /// run. A paragraph whose first line is a grid top border is a candidate; each following grid
    /// line is absorbed into it. A non-grid line ends the candidate: if the lines so far already
    /// form a complete table the paragraph closes (and builds as a table) so the new line starts
    /// fresh, otherwise the paragraph stays open to take the line as a lazy continuation. Returns
    /// `true` when the line was absorbed.
    pub(super) fn continue_grid_table(
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
    pub(super) fn continue_pipe_table(
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
        // A line block reinterprets as a table header only on its very first line.
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
    pub(super) fn continue_line_block(
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

    pub(super) fn text_tables_enabled(&self) -> bool {
        self.extensions.contains(Extension::SimpleTables)
            || self.extensions.contains(Extension::MultilineTables)
    }

    /// Let a dash-ruled table claim its lines before the block openers run. A single-line paragraph
    /// directly above a dash ruling is the header of a new table: the paragraph is retyped and the
    /// ruling folded onto it, so the rows below gather into one leaf. An already-open table leaf
    /// absorbs each further line, and a blank line settles it (see [`Parser::finalize_text_table`]).
    /// Returns `true` when the line was absorbed.
    pub(super) fn continue_text_table(
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
                // Columns are positional: the de-indented header is restored to its original
                // column to share the ruling's coordinates.
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
                    // Header-led tables end at a blank; dash-led only after their closing ruling.
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
    pub(super) fn finalize_text_table(&mut self, leaf: usize) {
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
    pub(super) fn refeed_lines(&mut self, lines: &[String]) {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        for index in 0..refs.len() {
            let line = refs.get(index).copied().unwrap_or("");
            let following = refs.get(index + 1..).unwrap_or(&[]);
            self.process_line(line, following, None);
        }
    }

    /// Settle every dash-ruled table leaf still open at end of input. Re-feeding surplus lines may
    /// open a fresh candidate, which the next pass settles; each pass strictly shrinks the work.
    pub(super) fn finalize_open_text_tables(&mut self) {
        while let Some(leaf) = self.open_text_table_leaf() {
            self.finalize_text_table(leaf);
        }
    }

    pub(super) fn open_text_table_leaf(&self) -> Option<usize> {
        (0..self.nodes.len()).find(|&index| {
            matches!(self.kind(index), Some(Kind::TextTable))
                && self.nodes.get(index).is_some_and(|node| node.open)
        })
    }

    /// Open a line block on a `|` flush at the line start. A line block never interrupts a paragraph
    /// and never carries leading indentation; its whole line is its first entry, with later lines
    /// claimed by `continue_line_block` before the openers run.
    pub(super) fn open_line_block(
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
}
