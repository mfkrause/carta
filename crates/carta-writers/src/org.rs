//! Render the document model as Org markup.

use std::borrow::Cow;

use carta_ast::{
    Attr, Block, Caption, Cell, Citation, CitationMode, Document, Format, Inline, ListAttributes,
    ListNumberDelim, MathType, QuoteType, Row, Table, Target, Text, to_plain_text,
};
use carta_core::{Extension, Result, WrapMode, Writer, WriterOptions, budget};

use crate::common::{
    FILL_COLUMN, Piece, append_notes, attribute_value, display_width, fill, fill_groups, fill_hang,
    indent_block, label_matches_url, offset_as_i64, per_character_width, quote_marks,
    simple_table_rows,
};

/// Number of dashes emitted for a horizontal rule.
const RULE_WIDTH: usize = 14;

/// Layout width for a table cell's content. Org table cells are never reflowed, so cell blocks are
/// rendered wide enough that filling never introduces a line break; the column is then sized to the
/// content's natural width.
const CELL_WIDTH: usize = 1_000_000;

/// Zero-width space used to keep a paragraph's leading character from being read as Org syntax.
const ZERO_WIDTH_SPACE: char = '\u{200b}';

/// Renders a document to Org text.
#[derive(Debug, Default, Clone, Copy)]
pub struct OrgWriter;

impl Writer for OrgWriter {
    fn render_document(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        budget::scope(document, || {
            let width = options.columns.unwrap_or(FILL_COLUMN);
            let mut state = State {
                wrap: options.wrap,
                width,
                smart: options.extensions.contains(Extension::Smart),
                task_lists: options.extensions.contains(Extension::TaskLists),
                citations: options.extensions.contains(Extension::Citations),
                notes: Vec::new(),
            };
            let body = state.blocks(&document.blocks, width, false, 0);
            Ok(append_notes(body, &state.notes))
        })
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.org"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }
}

/// Rendering state: the fill mode and column, whether smart punctuation and task-list checkboxes and
/// citation syntax are honored, and the footnote definitions gathered for the trailing section.
struct State {
    wrap: WrapMode,
    /// The document's fill column, used both for body layout and for footnote-definition bodies.
    width: usize,
    /// Whether quotation marks render as straight ASCII rather than curly glyphs.
    smart: bool,
    /// Whether a leading checkbox character becomes an Org `[ ]`/`[X]` marker.
    task_lists: bool,
    /// Whether a citation renders as Org `[cite:…]` syntax rather than its fallback inlines.
    citations: bool,
    /// Footnote bodies, indexed by note number minus one, emitted after the body.
    notes: Vec<String>,
}

impl State {
    /// Render a sequence of blocks, separating each from the previous by a blank line, except that a
    /// heading or plain block is followed by a single newline. When `hang_first` is set the first
    /// visible block is laid out for a hanging marker (its leading space is kept and it is not
    /// protected with a zero-width space).
    fn blocks(
        &mut self,
        blocks: &[Block],
        width: usize,
        hang_first: bool,
        initial_column: usize,
    ) -> String {
        let mut sequence = Vec::with_capacity(blocks.len());
        push_sequence(blocks, &mut sequence);
        let mut out = String::new();
        let mut previous: Option<&Block> = None;
        let mut first = true;
        for (block, cell_source) in sequence {
            let text = match block {
                Block::CodeBlock(attr, text) if cell_source => code_block(attr, text, "jupyter-"),
                _ => self.block(
                    block,
                    width,
                    hang_first && first,
                    if first { initial_column } else { 0 },
                ),
            };
            if text.is_empty() {
                continue;
            }
            if let Some(prev) = previous {
                out.push_str(block_separator(prev, block));
            }
            out.push_str(&text);
            previous = Some(block);
            first = false;
        }
        out
    }

    fn block(&mut self, block: &Block, width: usize, hang: bool, initial_column: usize) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                self.leaf(inlines, width, hang, initial_column)
            }
            Block::LineBlock(lines) => self.line_block(lines),
            Block::CodeBlock(attr, text) => code_block(attr, text, ""),
            Block::RawBlock(format, text) => raw_block(format, text),
            Block::BlockQuote(blocks) => {
                let body = set_off(blocks, self.blocks(blocks, width, false, 0));
                fence("#+begin_quote", &body, "#+end_quote")
            }
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::BulletList(items) => self.bullet_list(items, width, initial_column),
            Block::DefinitionList(items) => self.definition_list(items, width),
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines),
            Block::HorizontalRule => "-".repeat(RULE_WIDTH),
            Block::Table(table) => self.table(table, width),
            Block::Figure(attr, caption, blocks) => self.figure(attr, caption, blocks, width),
            Block::Div(attr, blocks) => self.div(attr, blocks, width),
        }
    }

    /// Render a paragraph or plain block: fill its inlines to `width`, then, unless it is laid out for
    /// a hanging marker, guard a leading `*`, `#`, or `|` with a zero-width space so it is not read as
    /// a heading, keyword, or table. Only a character from literal text needs guarding: an emphasis or
    /// other markup delimiter that opens the line (e.g. the `*` of a bold span) is unambiguous.
    fn leaf(
        &mut self,
        inlines: &[Inline],
        width: usize,
        hang: bool,
        initial_column: usize,
    ) -> String {
        let pieces = separate_characters(self.pieces(inlines));
        if initial_column > 0 {
            fill_groups(&pieces, &[], width, initial_column, hang, self.wrap)
        } else if hang {
            fill_hang(&pieces, width, self.wrap)
        } else {
            let filled = fill(&pieces, width, self.wrap);
            match inlines.first() {
                Some(Inline::Str(text)) if !text.is_empty() => protect_leading(filled),
                _ => filled,
            }
        }
    }

    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| {
                let text = self.flat(line);
                if text.is_empty() {
                    String::new()
                } else {
                    format!("  {text}")
                }
            })
            .collect();
        fence("#+begin_verse", &rendered.join("\n"), "#+end_verse")
    }

    fn header(&mut self, level: i64, attr: &Attr, inlines: &[Inline]) -> String {
        let level = usize::try_from(level.max(1)).unwrap_or(usize::MAX);
        budget::charge(level);
        if budget::exhausted() {
            return String::new();
        }
        let stars = "*".repeat(level);
        let (title, tags) = heading_tags(inlines);
        let text = self.flat(title);
        let mut heading = format!("{stars} {text}");
        if !tags.is_empty() {
            let tags = format!(":{}:", tags.join(":"));
            heading.truncate(heading.trim_end().len());
            let used = display_width(&heading) + display_width(&tags);
            heading.push_str(&" ".repeat(self.width.saturating_sub(used).max(1)));
            heading.push_str(&tags);
        }
        let props = properties(attr);
        if props.is_empty() {
            heading
        } else {
            format!("{heading}\n{props}")
        }
    }

    fn bullet_list(&mut self, items: &[Vec<Block>], width: usize, initial_column: usize) -> String {
        let loose = !list_is_tight(items);
        let mut units = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let item = self.checkbox_item(item);
            let text = self.list_item(
                &item,
                width.saturating_sub(2),
                "- ",
                "  ",
                if index == 0 { initial_column } else { 0 },
            );
            units.push((text, loose || ends_blank(&item)));
        }
        join_items(units)
    }

    /// Replace a leading Org-ballot checkbox character in a task-list item with an Org checkbox
    /// marker, returning the item unchanged when task lists are off or the item does not open with a
    /// checkbox.
    fn checkbox_item<'a>(&self, item: &'a [Block]) -> Cow<'a, [Block]> {
        if !self.task_lists {
            return Cow::Borrowed(item);
        }
        let replacement = match item.first() {
            Some(Block::Plain(inlines) | Block::Para(inlines)) => match inlines.first() {
                Some(Inline::Str(text)) => match text.chars().next() {
                    Some('\u{2610}') => Some("[ ]"),
                    Some('\u{2612}') => Some("[X]"),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };
        let Some(marker) = replacement else {
            return Cow::Borrowed(item);
        };
        let mut blocks = item.to_vec();
        if let Some(Block::Plain(inlines) | Block::Para(inlines)) = blocks.first_mut()
            && let Some(Inline::Str(text)) = inlines.first_mut()
        {
            let rest: String = text.chars().skip(1).collect();
            *text = format!("{marker}{rest}").into();
        }
        Cow::Owned(blocks)
    }

    fn ordered_list(
        &mut self,
        attrs: &ListAttributes,
        items: &[Vec<Block>],
        width: usize,
    ) -> String {
        let loose = !list_is_tight(items);
        let delim = match attrs.delim {
            ListNumberDelim::OneParen | ListNumberDelim::TwoParens => ")",
            ListNumberDelim::Period | ListNumberDelim::DefaultDelim => ".",
        };
        let mut units = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let number = attrs.start.saturating_add(offset_as_i64(index));
            let marker = format!("{number}{delim}");
            let field = marker.chars().count() + 1;
            let cookie = if index == 0 && attrs.start != 1 {
                format!("[@{}] ", attrs.start)
            } else {
                String::new()
            };
            let first = format!("{marker} {cookie}");
            let rest = " ".repeat(field);
            let text = self.list_item(item, width.saturating_sub(field), &first, &rest, 0);
            units.push((text, loose || ends_blank(item)));
        }
        join_items(units)
    }

    fn list_item(
        &mut self,
        item: &[Block],
        width: usize,
        first: &str,
        rest: &str,
        initial_column: usize,
    ) -> String {
        let initial_column = if matches!(item.first(), Some(Block::Plain(_) | Block::Para(_))) {
            initial_column
        } else {
            0
        };
        let body = self.blocks(item, width, true, initial_column);
        if let Some(block) = item.first()
            && !body.is_empty()
            && !matches!(block, Block::Plain(_) | Block::Para(_))
        {
            let breaks = if leading_blank(block) { "\n\n" } else { "\n" };
            indent_block(&body, &format!("{first}{breaks}{rest}"), rest)
        } else {
            indent_block(&body, first, rest)
        }
    }

    fn definition_list(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        let mut units = Vec::new();
        for (term, definitions) in items {
            let loose = !list_is_tight(definitions);
            units.push((self.definition_item(term, definitions, width), loose));
        }
        join_items(units)
    }

    fn definition_item(
        &mut self,
        term: &[Inline],
        definitions: &[Vec<Block>],
        width: usize,
    ) -> String {
        let body_width = width.saturating_sub(2);
        let mut blocks: Vec<Block> = definitions.iter().flatten().cloned().collect();
        if let Some(Block::Plain(inlines) | Block::Para(inlines)) = blocks.first_mut()
            && !inlines.is_empty()
        {
            let mut fused = term.to_vec();
            fused.extend([Inline::Space, Inline::Str("::".into()), Inline::Space]);
            fused.append(inlines);
            *inlines = fused;
            let body = self.blocks(&blocks, body_width, true, 0);
            return indent_block(&body, "- ", "  ");
        }
        let mut lead = term.to_vec();
        // Keep the separator with the term's last word during wrapping.
        lead.push(Inline::RawInline(Format("org".into()), " ::".into()));
        let lead = self.leaf(&lead, body_width, true, 0);
        let first = blocks.iter().find(|block| !is_empty_leaf(block));
        let shared = first.is_none_or(shares_term_line);
        let column = if shared {
            lead.rsplit('\n').next().map_or(0, display_width) + 1
        } else {
            0
        };
        let body = self.blocks(&blocks, body_width, false, column);
        let text = if shared || body.is_empty() {
            format!("{lead} {body}")
        } else if first.is_some_and(leading_blank) {
            format!("{lead} \n\n{body}")
        } else {
            format!("{lead}\n{body}")
        };
        indent_block(&text, "- ", "  ")
    }

    fn table(&mut self, table: &Table, _width: usize) -> String {
        let caption = self.caption_line(&table.caption);
        let columns = table.col_specs.len();
        if columns == 0 {
            return caption;
        }

        let mut rows = Vec::new();
        let mut head_count = 0;
        for (row, header) in simple_table_rows(table) {
            rows.push(row);
            head_count += usize::from(header);
        }

        let grid = self.lay_grid(&rows, columns);
        let widths = column_widths(&grid, columns);

        let mut lines: Vec<String> = Vec::new();
        for (index, row) in grid.iter().enumerate() {
            let height = row
                .iter()
                .filter_map(|slot| slot.as_ref().map(|cell| cell.lines.len()))
                .max()
                .unwrap_or(1)
                .max(1);
            for line in 0..height {
                let fields: Vec<String> = row
                    .iter()
                    .map(|slot| {
                        slot.as_ref()
                            .and_then(|cell| cell.lines.get(line))
                            .map_or_else(String::new, String::clone)
                    })
                    .collect();
                lines.push(render_row(&fields, &widths));
            }
            if head_count > 0 && index + 1 == head_count {
                lines.push(render_rule(&widths));
            }
        }

        let table_text = lines.join("\n");
        if caption.is_empty() {
            table_text
        } else {
            format!("{table_text}\n#+caption: {caption}")
        }
    }

    /// Place each row's cells into a fixed-width grid, honoring row and column spans: a spanning cell's
    /// content sits in its top-left slot and the slots it covers below and to the right are left empty.
    /// Each occupied slot holds the cell's content as already-rendered lines.
    fn lay_grid(&mut self, rows: &[&Row], columns: usize) -> CellGrid {
        let mut grid: CellGrid = (0..rows.len())
            .map(|_| (0..columns).map(|_| None).collect())
            .collect();
        let mut occupied = vec![vec![false; columns]; rows.len()];

        for (index, row) in rows.iter().enumerate() {
            let mut column = 0usize;
            for cell in &row.cells {
                while column < columns
                    && matches!(
                        occupied.get(index).and_then(|line| line.get(column)),
                        Some(true)
                    )
                {
                    column += 1;
                }
                if column >= columns {
                    break;
                }
                // Clamp spans to real slots to avoid iterating over absent cells.
                let row_span = usize::try_from(cell.row_span.max(1))
                    .unwrap_or(1)
                    .min(rows.len() - index);
                let col_span = usize::try_from(cell.col_span.max(1))
                    .unwrap_or(1)
                    .min(columns - column);
                let rendered = self.render_cell(cell);
                if let Some(slot) = grid.get_mut(index).and_then(|line| line.get_mut(column)) {
                    *slot = Some(rendered);
                }
                for down in 0..row_span {
                    for across in 0..col_span {
                        if let Some(flag) = occupied
                            .get_mut(index + down)
                            .and_then(|line| line.get_mut(column + across))
                        {
                            *flag = true;
                        }
                    }
                }
                column += col_span;
            }
        }
        grid
    }

    /// Render a cell's blocks to lines. Cells are never reflowed, so a paragraph stays on one line and
    /// a nested list or several blocks expand the cell across several table lines. Boundary spaces of
    /// a sole-paragraph cell drop out of the rendered lines, but the sizing width still counts them so
    /// the column reserves their room.
    fn render_cell(&mut self, cell: &Cell) -> RenderedCell {
        let lines: Vec<String> = self
            .blocks(&cell.content, CELL_WIDTH, false, 0)
            .split('\n')
            .map(str::to_string)
            .collect();
        let widest = lines
            .iter()
            .map(|line| per_character_width(line))
            .max()
            .unwrap_or(0);
        RenderedCell {
            sizing_width: widest + cell_boundary_space_count(&cell.content),
            lines,
        }
    }

    fn figure(&mut self, attr: &Attr, caption: &Caption, blocks: &[Block], width: usize) -> String {
        let body = self.blocks(blocks, width, false, 0);
        let caption = self.caption_line(caption);
        let mut lines = Vec::new();
        if !caption.is_empty() {
            lines.push(format!("#+caption: {caption}"));
        }
        if !attr.id.is_empty() {
            lines.push(format!("<<{}>>", attr.id));
        }
        if lines.is_empty() {
            return body;
        }
        if !body.is_empty() {
            lines.push(set_off(blocks, body));
        }
        lines.join("\n")
    }

    fn caption_line(&mut self, caption: &Caption) -> String {
        let mut inlines: Vec<Inline> = Vec::new();
        for content in crate::common::caption_runs(&caption.long) {
            if !inlines.is_empty() {
                inlines.push(Inline::LineBreak);
            }
            inlines.extend(content.iter().cloned());
        }
        self.flat(&inlines)
    }

    fn div(&mut self, attr: &Attr, blocks: &[Block], width: usize) -> String {
        if let Some(result) = cell_result(attr, blocks) {
            let mut lines = vec!["#+RESULTS:".to_owned()];
            lines.extend(
                result
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(|line| format!(": {line}")),
            );
            return lines.join("\n");
        }
        let body = self.blocks(blocks, width, false, 0);
        if let Some(name) = drawer_name(&attr.classes) {
            let mut lines = vec![format!(":{name}:"), String::new()];
            if !body.is_empty() {
                lines.push(body);
                lines.push(String::new());
            }
            lines.push(":END:".to_owned());
            return lines.join("\n");
        }
        match special_div(&attr.classes) {
            Some(index) => {
                let name = attr.classes.get(index).map_or("", |class| class.as_str());
                let extra: Vec<&str> = attr
                    .classes
                    .iter()
                    .enumerate()
                    .filter(|(position, _)| *position != index)
                    .map(|(_, class)| class.as_str())
                    .collect();
                let mut lines: Vec<String> = Vec::new();
                if !attr.id.is_empty() {
                    lines.push(format!("#+name: {}", attr.id));
                }
                if let Some(line) = attr_html_line(&extra, &attr.attributes) {
                    lines.push(line);
                }
                lines.push(format!("#+begin_{name}"));
                if !body.is_empty() {
                    lines.push(set_off(blocks, body));
                }
                lines.push(format!("#+end_{name}"));
                lines.join("\n")
            }
            None => {
                if attr.id.is_empty() {
                    body
                } else if body.is_empty() {
                    format!("<<{}>>", attr.id)
                } else {
                    format!("<<{}>>\n{}", attr.id, set_off(blocks, body))
                }
            }
        }
    }

    /// Reserve a footnote slot, render the body offset under the `[fn:N]` marker, and return the
    /// inline marker. The slot is reserved before rendering so a nested footnote numbers after it.
    fn record_note(&mut self, blocks: &[Block]) -> String {
        let index = self.notes.len();
        self.notes.push(String::new());
        let marker = format!("[fn:{}]", index + 1);
        let field = marker.chars().count() + 1;
        let body = self.blocks(blocks, self.width.saturating_sub(field), false, 0);
        let definition = if body.is_empty() {
            format!("{marker} ")
        } else if blocks.first().is_some_and(leading_blank) {
            let rest = " ".repeat(field);
            indent_block(&body, &format!("{marker} \n\n{rest}"), &rest)
        } else {
            indent_block(&body, &format!("{marker} "), &" ".repeat(field))
        };
        if let Some(slot) = self.notes.get_mut(index) {
            *slot = definition;
        }
        marker
    }

    fn citation(&mut self, citations: &[Citation]) -> String {
        let variant = match citations {
            [first, ..] if matches!(first.mode, CitationMode::AuthorInText) => "/t",
            [only] if matches!(only.mode, CitationMode::SuppressAuthor) => "/na",
            _ => "",
        };
        let items: Vec<String> = citations
            .iter()
            .map(|citation| self.citation_item(citation))
            .collect();
        format!("[cite{variant}:{}]", items.join("; "))
    }

    fn citation_item(&mut self, citation: &Citation) -> String {
        let prefix = self.flat(&citation.prefix);
        let prefix = prefix.trim_end();
        let mut item = String::new();
        if !prefix.is_empty() {
            item.push_str(prefix);
            item.push(' ');
        }
        item.push('@');
        item.push_str(&citation.id);
        item.push_str(&self.citation_suffix(&citation.suffix));
        item
    }

    /// Render a citation's suffix after its key. A suffix that opens with a locator follows the key
    /// as plain text; any other suffix is set off by a comma unless it brings its own punctuation.
    fn citation_suffix(&mut self, suffix: &[Inline]) -> String {
        if let Some(locator) = split_locator(suffix) {
            let rest = self.flat(locator.rest);
            let gap = if locator.trailing.is_empty() { "" } else { " " };
            return format!(" {}{gap}{}{rest}", locator.text, locator.trailing);
        }
        let text = self.flat(suffix);
        match suffix.first() {
            None => String::new(),
            Some(Inline::Space) => format!(" {}", text.trim_start()),
            Some(Inline::Str(word)) if word.starts_with(|ch: char| ch.is_ascii_punctuation()) => {
                format!(" {text}")
            }
            Some(_) => format!(" , {}", text.trim_start()),
        }
    }

    /// Keep span and quote delimiters with their outer words while the interior wraps.
    fn pieces(&mut self, inlines: &[Inline]) -> Vec<Piece> {
        let mut out = Vec::new();
        for inline in inlines {
            self.piece(inline, &mut out);
        }
        out
    }

    fn piece(&mut self, inline: &Inline, out: &mut Vec<Piece>) {
        match inline {
            Inline::Str(text) => out.push(Piece::text(special_strings(text))),
            Inline::Space => out.push(Piece::Space),
            Inline::SoftBreak => out.push(Piece::Soft),
            Inline::LineBreak => {
                out.push(Piece::text("\\\\"));
                out.push(Piece::Hard);
            }
            Inline::Emph(content) => self.wrap_inlines(content, "/", "/", out),
            Inline::Strong(content) => self.wrap_inlines(content, "*", "*", out),
            Inline::Underline(content) => self.wrap_inlines(content, "_", "_", out),
            Inline::Strikeout(content) => self.wrap_inlines(content, "+", "+", out),
            Inline::Superscript(content) => self.wrap_inlines(content, "^{", "}", out),
            Inline::Subscript(content) => self.wrap_inlines(content, "_{", "}", out),
            Inline::Span(attr, content)
                if content.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty() =>
            {
                out.push(Piece::text(format!("<<{}>>", attr.id)));
            }
            Inline::SmallCaps(content) | Inline::Span(_, content) => {
                for inner in content {
                    self.piece(inner, out);
                }
            }
            Inline::Quoted(kind, content) => {
                let (open, close) = self.quote_glyphs(kind);
                out.push(Piece::text(open.to_string()));
                for inner in content {
                    self.piece(inner, out);
                }
                out.push(Piece::text(close.to_string()));
            }
            Inline::Code(_, text) => out.push(Piece::text(format!("={text}="))),
            Inline::Math(MathType::InlineMath, text) => {
                out.push(Piece::text(format!("\\({text}\\)")));
            }
            Inline::Math(MathType::DisplayMath, text) => {
                out.push(Piece::text(format!("\\[{text}\\]")));
            }
            Inline::RawInline(format, text) => {
                if is_raw_org(format) {
                    out.push(Piece::text(text.to_string()));
                }
            }
            Inline::Link(_, label, target) => out.push(Piece::text(self.link(label, target))),
            Inline::Image(_, _, target) => out.push(Piece::text(image(target))),
            Inline::Cite(citations, fallback) => {
                if self.citations {
                    out.push(Piece::text(self.citation(citations)));
                } else {
                    for inner in fallback {
                        self.piece(inner, out);
                    }
                }
            }
            Inline::Note(blocks) => out.push(Piece::text(self.record_note(blocks))),
        }
    }

    fn wrap_inlines(&mut self, content: &[Inline], open: &str, close: &str, out: &mut Vec<Piece>) {
        out.push(Piece::text(open));
        for inline in content {
            self.piece(inline, out);
        }
        out.push(Piece::text(close));
    }

    /// Render inlines to a flat, single-block string: the fill pieces joined with spaces for breaks.
    /// Used where inline content may not reflow: headings, list terms, captions, and table cells.
    fn flat(&mut self, inlines: &[Inline]) -> String {
        let pieces = self.pieces(inlines);
        let mut out = String::new();
        for piece in pieces {
            match piece {
                Piece::Text(text) => out.push_str(&text),
                Piece::Space | Piece::Soft => out.push(' '),
                Piece::Hard => out.push('\n'),
            }
        }
        out
    }

    fn link(&mut self, label: &[Inline], target: &Target) -> String {
        if let [Inline::Image(_, _, inner)] = label {
            return format!("[[{}][{}]]", org_path(&target.url), image(inner));
        }
        let plain = to_plain_text(label);
        if label_matches_url(&plain, &target.url) {
            format!("[[{}]]", org_path(&plain))
        } else {
            format!("[[{}][{}]]", org_path(&target.url), self.flat(label))
        }
    }

    fn quote_glyphs(&self, kind: &QuoteType) -> (char, char) {
        if self.smart {
            match kind {
                QuoteType::SingleQuote => ('\'', '\''),
                QuoteType::DoubleQuote => ('"', '"'),
            }
        } else {
            quote_marks(kind)
        }
    }
}

/// Labels that open a citation locator, in lower case.
const LOCATOR_LABELS: [&str; 52] = [
    "bk.",
    "bks.",
    "book",
    "chap.",
    "chaps.",
    "chapter",
    "col.",
    "cols.",
    "column",
    "fig.",
    "figs.",
    "figure",
    "fol.",
    "fols.",
    "folio",
    "l.",
    "ll.",
    "line",
    "n.",
    "nn.",
    "no.",
    "nos.",
    "note",
    "number",
    "op.",
    "opp.",
    "opus",
    "p.",
    "pp.",
    "page",
    "para.",
    "paras.",
    "paragraph",
    "pt.",
    "pts.",
    "part",
    "s.v.",
    "s.vv.",
    "sec.",
    "secs.",
    "section",
    "sub verbo",
    "v.",
    "vv.",
    "verse",
    "vol.",
    "vols.",
    "volume",
    "\u{a7}",
    "\u{a7}\u{a7}",
    "\u{b6}",
    "\u{b6}\u{b6}",
];

/// A locator that opens a citation suffix.
struct Locator<'a> {
    /// The locator as plain text: its label, if any, and its value.
    text: String,
    /// Punctuation that closed the value and belongs to what follows.
    trailing: String,
    /// The suffix inlines after the locator.
    rest: &'a [Inline],
}

/// Split a locator off the front of a citation suffix, after an optional comma: a label and its
/// value, or a bare value that holds a digit, with any further values that hold a digit.
fn split_locator(suffix: &[Inline]) -> Option<Locator<'_>> {
    let has_digit = |word: &str| word.chars().any(|ch| ch.is_ascii_digit());
    let mut words: Vec<(usize, String)> = Vec::new();
    let mut open = false;
    for (index, inline) in suffix.iter().enumerate() {
        if matches!(inline, Inline::Space | Inline::SoftBreak) {
            open = false;
            continue;
        }
        let text = to_plain_text(std::slice::from_ref(inline)).replace('\u{a0}', " ");
        match words.last_mut() {
            Some((end, word)) if open => {
                word.push_str(&text);
                *end = index + 1;
            }
            _ => words.push((index + 1, text)),
        }
        open = true;
    }
    if let Some((_, first)) = words.first_mut()
        && let Some(stripped) = first.strip_prefix(',')
    {
        *first = stripped.to_owned();
        if first.is_empty() {
            words.remove(0);
        }
    }
    let lowered: Vec<String> = words
        .iter()
        .take(2)
        .map(|(_, word)| word.to_lowercase())
        .collect();
    let label_words = match lowered.as_slice() {
        [first, second] if LOCATOR_LABELS.contains(&format!("{first} {second}").as_str()) => 2,
        [first, ..] if LOCATOR_LABELS.contains(&first.as_str()) => 1,
        _ => 0,
    };
    let (_, value) = words.get(label_words)?;
    if label_words == 0 && !has_digit(value) {
        return None;
    }
    let mut count = label_words + 1;
    while words.get(count).is_some_and(|(_, word)| has_digit(word)) {
        count += 1;
    }
    let mut text = words
        .iter()
        .take(count)
        .map(|(_, word)| word.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let mut trailing = String::new();
    let core = value.trim_end_matches([',', ';', '.', '!', '?', ')']);
    if count == label_words + 1 && !core.is_empty() {
        trailing = text.split_off(text.len() - (value.len() - core.len()));
    }
    let end = words.get(count - 1).map_or(suffix.len(), |(end, _)| *end);
    Some(Locator {
        text,
        trailing,
        rest: suffix.get(end..).unwrap_or_default(),
    })
}

/// The separator between two consecutive blocks: a blank line when the earlier block wants trailing
/// space or the later block wants leading space, a single newline otherwise. A plain block, heading,
/// or raw Org block attaches tightly to what follows; a block quote, div, verse, rule, or raw HTML
/// block is set off from what precedes it. A drawer opening a heading's section attaches to the
/// headline like the section's other leading content.
fn block_separator(previous: &Block, next: &Block) -> &'static str {
    if matches!(previous, Block::Header(..)) && is_drawer(next) {
        return "\n";
    }
    if trailing_blank(previous) || leading_blank(next) {
        "\n\n"
    } else {
        "\n"
    }
}

/// Whether a block is a drawer: a div carrying the `drawer` marker class and a name.
fn is_drawer(block: &Block) -> bool {
    matches!(block, Block::Div(attr, _) if drawer_name(&attr.classes).is_some())
}

/// Whether a block is followed by a blank line: everything except a plain block, a heading, and a raw
/// Org block, which attach to what follows.
fn trailing_blank(block: &Block) -> bool {
    !matches!(block, Block::Plain(_) | Block::Header(..))
        && !is_raw_org_block(block)
        && !is_cell_result(block)
}

/// Whether a block is preceded by a blank line: the block-level constructs that stand apart from what
/// comes before them.
fn leading_blank(block: &Block) -> bool {
    match block {
        Block::Div(..) => !is_cell_result(block) && cell_blocks(block).is_none(),
        Block::BlockQuote(_) | Block::LineBlock(_) | Block::HorizontalRule => true,
        Block::RawBlock(format, _) => format.0.eq_ignore_ascii_case("html"),
        _ => false,
    }
}

/// The blocks of a notebook code cell: a div classed `cell code` that opens with its source block.
fn cell_blocks(block: &Block) -> Option<(&Block, &[Block])> {
    match block {
        Block::Div(attr, blocks) if attr.classes.iter().map(Text::as_str).eq(["cell", "code"]) => {
            match blocks.split_first() {
                Some((source @ Block::CodeBlock(..), rest)) => Some((source, rest)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// List `blocks` in layout order, each with whether it is the source block of a notebook code cell.
/// A code cell lays out as the blocks it holds, so it adds no spacing of its own.
fn push_sequence<'a>(blocks: &'a [Block], out: &mut Vec<(&'a Block, bool)>) {
    for block in blocks {
        match cell_blocks(block) {
            Some((source, rest)) => {
                out.push((source, true));
                push_sequence(rest, out);
            }
            None => out.push((block, false)),
        }
    }
}

/// The text of a notebook cell's result: a div classed `output execute_result` holding one code block.
fn cell_result<'a>(attr: &Attr, blocks: &'a [Block]) -> Option<&'a str> {
    match blocks {
        [Block::CodeBlock(_, text)]
            if attr
                .classes
                .iter()
                .map(Text::as_str)
                .eq(["output", "execute_result"]) =>
        {
            Some(text)
        }
        _ => None,
    }
}

fn is_cell_result(block: &Block) -> bool {
    matches!(block, Block::Div(attr, blocks) if cell_result(attr, blocks).is_some())
}

fn is_raw_org_block(block: &Block) -> bool {
    matches!(block, Block::RawBlock(format, _) if format.0.eq_ignore_ascii_case("org"))
}

/// Whether a list is tight: every item is empty, opens with a plain block, or holds only a nested
/// tight list. A loose list sets a blank line after each item.
fn list_is_tight(items: &[Vec<Block>]) -> bool {
    items.iter().all(|item| match item.as_slice() {
        [] | [Block::Plain(_), ..] => true,
        [Block::BulletList(nested) | Block::OrderedList(_, nested)] => list_is_tight(nested),
        _ => false,
    })
}

/// Whether an item of a tight list is followed by a blank line: its last block wants trailing
/// space, where a nested list counts only when it is loose or its own last item ends that way.
fn ends_blank(item: &[Block]) -> bool {
    match item.last() {
        None => false,
        Some(Block::BulletList(nested) | Block::OrderedList(_, nested)) => {
            !list_is_tight(nested) || nested.last().is_some_and(|last| ends_blank(last))
        }
        Some(block) => trailing_blank(block),
    }
}

/// Whether a block is a paragraph or plain block without content, which renders nothing.
fn is_empty_leaf(block: &Block) -> bool {
    matches!(block, Block::Plain(inlines) | Block::Para(inlines) if inlines.is_empty())
}

/// Whether a definition's opening block continues the term line: running text and a nested bullet
/// list do, while a block that Org recognizes only at the start of a line opens its own line.
fn shares_term_line(block: &Block) -> bool {
    match block {
        Block::Plain(_) | Block::Para(_) | Block::BulletList(_) => true,
        Block::Figure(attr, caption, blocks) => {
            attr.id.is_empty()
                && caption.long.is_empty()
                && blocks.first().is_none_or(shares_term_line)
        }
        _ => false,
    }
}

/// Open a container's body with a blank line when its first block stands apart from what precedes.
fn set_off(blocks: &[Block], body: String) -> String {
    if !body.is_empty() && blocks.first().is_some_and(leading_blank) {
        format!("\n{body}")
    } else {
        body
    }
}

/// Wrap body content in an Org block, collapsing to adjacent delimiters when the body is empty.
fn fence(begin: &str, body: &str, end: &str) -> String {
    if body.is_empty() {
        format!("{begin}\n{end}")
    } else {
        format!("{begin}\n{body}\n{end}")
    }
}

/// Join rendered list items, dropping any that produced no output. An item whose flag is set is
/// followed by a blank line.
fn join_items(units: Vec<(String, bool)>) -> String {
    let mut out = String::new();
    let mut blank = false;
    for (unit, spaced) in units {
        if unit.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank { "\n\n" } else { "\n" });
        }
        out.push_str(&unit);
        blank = spaced;
    }
    out
}

/// Split every text run that reaches beyond ASCII into one piece per character. Org sets a line one
/// character at a time, so each character claims its columns alone: a cluster spread over several
/// characters is not drawn as one glyph, and a combining mark keeps a column of its own.
fn separate_characters(pieces: Vec<Piece>) -> Vec<Piece> {
    if pieces.iter().all(|piece| match piece {
        Piece::Text(text) => text.is_ascii(),
        _ => true,
    }) {
        return pieces;
    }
    let mut out = Vec::with_capacity(pieces.len());
    for piece in pieces {
        match piece {
            Piece::Text(text) if !text.is_ascii() => {
                let mut buffer = [0u8; 4];
                out.extend(
                    text.chars()
                        .map(|ch| Piece::text(&*ch.encode_utf8(&mut buffer))),
                );
            }
            other => out.push(other),
        }
    }
    out
}

/// Guard a paragraph whose first character would open Org syntax with a zero-width space.
fn protect_leading(text: String) -> String {
    match text.chars().next() {
        Some('*' | '#' | '|') => format!("{ZERO_WIDTH_SPACE}{text}"),
        _ => text,
    }
}

/// Split a heading at its first tag span: the title before it, and the name of every tag from there
/// on. Tags close a headline, so the title ends where they begin.
fn heading_tags(inlines: &[Inline]) -> (&[Inline], Vec<&str>) {
    fn tag_name(inline: &Inline) -> Option<&str> {
        match inline {
            Inline::Span(attr, _) => attribute_value(attr, "tag-name"),
            _ => None,
        }
    }
    let start = inlines
        .iter()
        .position(|inline| tag_name(inline).is_some())
        .unwrap_or(inlines.len());
    let (title, rest) = inlines.split_at(start);
    (title, rest.iter().filter_map(tag_name).collect())
}

/// The property drawer for a heading's attributes, or an empty string when it carries none.
fn properties(attr: &Attr) -> String {
    if attr.id.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty() {
        return String::new();
    }
    let mut lines = vec![":PROPERTIES:".to_string()];
    if !attr.id.is_empty() {
        lines.push(format!(":CUSTOM_ID: {}", attr.id));
    }
    if !attr.classes.is_empty() {
        lines.push(format!(":CLASS: {}", attr.classes.join(" ")));
    }
    for (key, value) in &attr.attributes {
        lines.push(format!(":{key}: {value}"));
    }
    lines.push(":END:".to_string());
    lines.join("\n")
}

/// Render an Org code block: a plain `example` block when it carries no class, otherwise a `src` block
/// tagged with its language (after `language_prefix`), its line-numbering switch, and its key/value
/// attributes as header arguments. A leading identifier becomes a `#+name:` line.
fn code_block(attr: &Attr, text: &str, language_prefix: &str) -> String {
    let content = escape_code(text);
    let block = if let Some(class) = attr.classes.first() {
        let language = translate_language(class);
        let numbered = attr.classes.iter().any(|class| {
            matches!(
                class.as_str(),
                "numberLines" | "number-lines" | "numberlines"
            )
        });
        let mut begin = format!("#+begin_src {language_prefix}{language}");
        if numbered {
            match attribute_value(attr, "startFrom") {
                Some(start) => {
                    begin.push_str(" -n ");
                    begin.push_str(start);
                }
                None => begin.push_str(" -n"),
            }
        }
        for (key, value) in &attr.attributes {
            if key != "startFrom" {
                begin.push_str(" :");
                begin.push_str(key);
                begin.push(' ');
                begin.push_str(value);
            }
        }
        fence(&begin, &content, "#+end_src")
    } else {
        fence("#+begin_example", &content, "#+end_example")
    };
    if attr.id.is_empty() {
        block
    } else {
        format!("#+name: {}\n{block}", attr.id)
    }
}

/// Guard each code line that would open Org syntax: after any leading whitespace, a `*` or `#+` is
/// prefixed with a comma so Org treats it as literal content rather than markup.
fn escape_code(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.trim_end_matches('\n').split('\n') {
        let indent: String = line
            .chars()
            .take_while(|ch| matches!(ch, ' ' | '\t'))
            .collect();
        let rest = line.strip_prefix(indent.as_str()).unwrap_or(line);
        if rest.starts_with('*') || rest.starts_with("#+") {
            lines.push(format!("{indent},{rest}"));
        } else {
            lines.push(line.to_string());
        }
    }
    lines.join("\n")
}

/// Map a language class to the name Org's source blocks use for it, passing others through.
fn translate_language(language: &str) -> &str {
    match language {
        "c" => "C",
        "bash" => "sh",
        "r" => "R",
        "commonlisp" => "lisp",
        other => other,
    }
}

/// Render a raw block: Org, LaTeX, and TeX pass through verbatim; HTML is wrapped in an export block;
/// other formats are dropped.
fn raw_block(format: &Format, text: &str) -> String {
    let name = format.0.to_ascii_lowercase();
    match name.as_str() {
        "org" | "latex" | "tex" => text.trim_end_matches('\n').to_string(),
        "html" => {
            let body = indent_block(text.trim_end_matches('\n'), "  ", "  ");
            fence("#+begin_html", &body, "#+end_html")
        }
        _ => String::new(),
    }
}

/// Whether a raw-passthrough format is one Org emits verbatim inline.
fn is_raw_org(format: &Format) -> bool {
    matches!(
        format.0.to_ascii_lowercase().as_str(),
        "org" | "latex" | "tex"
    )
}

/// The drawer name of a `Div` marked as a drawer: the first class other than the `drawer` marker,
/// or `None` when the div is not a drawer or carries no name.
fn drawer_name(classes: &[Text]) -> Option<&str> {
    if !classes.iter().any(|class| class == "drawer") {
        return None;
    }
    classes
        .iter()
        .map(Text::as_str)
        .find(|&class| class != "drawer")
}

fn special_div(classes: &[Text]) -> Option<usize> {
    const SPECIAL: [&str; 7] = [
        "center",
        "quote",
        "note",
        "warning",
        "tip",
        "important",
        "caution",
    ];
    classes.iter().position(|class| {
        let lowered = class.to_ascii_lowercase();
        SPECIAL.contains(&lowered.as_str())
    })
}

/// The `#+attr_html:` line for a special block's remaining classes and key/value attributes, or
/// `None` when it carries neither.
fn attr_html_line(extra: &[&str], attributes: &[(Text, Text)]) -> Option<String> {
    if extra.is_empty() && attributes.is_empty() {
        return None;
    }
    let mut line = String::from("#+attr_html:");
    if !extra.is_empty() {
        line.push_str(" :class ");
        line.push_str(&extra.join(" "));
    }
    for (key, value) in attributes {
        line.push_str(" :");
        line.push_str(key);
        line.push(' ');
        line.push_str(value);
    }
    Some(line)
}

/// The `[[…]]` body for an image: its destination as an Org path. Alt text and title are dropped.
fn image(target: &Target) -> String {
    format!("[[{}]]", org_path(&target.url))
}

/// Map a link or image destination to an Org path: fragments and absolute or relative filesystem
/// paths and full URLs pass through; a bare filename is prefixed with `file:`.
fn org_path(url: &str) -> String {
    if url.is_empty() {
        return String::new();
    }
    if url.starts_with('#')
        || url.starts_with('/')
        || url.starts_with("./")
        || url.starts_with("../")
        || has_scheme(url)
    {
        return url.to_string();
    }
    format!("file:{url}")
}

/// Whether a URL opens with a `scheme:` prefix: a non-empty run of scheme characters before a colon.
fn has_scheme(url: &str) -> bool {
    match url.split_once(':') {
        Some((scheme, _)) => {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+'))
        }
        None => false,
    }
}

/// Collapse Unicode dashes and the ellipsis to their ASCII forms so the text reads plainly in Org.
fn special_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\u{2014}' => out.push_str("---"),
            '\u{2013}' => out.push_str("--"),
            '\u{2026}' => out.push_str("..."),
            _ => out.push(ch),
        }
    }
    out
}

/// A rendered table cell: its lines, and the width its column must reserve (the widest line plus
/// any boundary spaces of a sole-paragraph cell that the rendering drops).
struct RenderedCell {
    lines: Vec<String>,
    sizing_width: usize,
}

/// A table laid out as rows of column slots. An occupied slot holds a rendered cell; a slot left
/// empty by a span or a short row is `None`.
type CellGrid = Vec<Vec<Option<RenderedCell>>>;

/// How many boundary spaces a sole-paragraph cell opens or closes with: one leading and/or one
/// trailing space inline. Richer cell content counts none.
fn cell_boundary_space_count(content: &[Block]) -> usize {
    let [Block::Plain(inlines) | Block::Para(inlines)] = content else {
        return 0;
    };
    let mut interior = inlines.as_slice();
    if let [Inline::Space, rest @ ..] = interior {
        interior = rest;
    }
    if let [rest @ .., Inline::Space] = interior {
        interior = rest;
    }
    inlines.len() - interior.len()
}

/// The display width of each column: the widest sizing width among cells that begin in that column.
/// A cell that spans several columns contributes only to the leftmost.
fn column_widths(grid: &CellGrid, columns: usize) -> Vec<usize> {
    let mut widths = vec![0usize; columns];
    for row in grid {
        for (column, slot) in row.iter().enumerate() {
            if let Some(cell) = slot
                && let Some(current) = widths.get_mut(column)
            {
                *current = (*current).max(cell.sizing_width);
            }
        }
    }
    widths
}

fn render_row(cells: &[String], widths: &[usize]) -> String {
    let mut fields = Vec::with_capacity(widths.len());
    for (column, width) in widths.iter().enumerate() {
        let text = cells.get(column).map_or("", String::as_str);
        let padding = width.saturating_sub(display_width(text));
        fields.push(format!(" {text}{} ", " ".repeat(padding)));
    }
    format!("|{}|", fields.join("|"))
}

/// Render the rule separating a table's header from its body.
fn render_rule(widths: &[usize]) -> String {
    let parts: Vec<String> = widths.iter().map(|width| "-".repeat(width + 2)).collect();
    format!("|{}|", parts.join("+"))
}

#[cfg(test)]
mod tests;
