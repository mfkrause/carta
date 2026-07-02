//! Org-mode writer: renders the document model to Org text.
//!
//! Block structure is conveyed through Org's `#+begin_…`/`#+end_…` blocks, heading asterisks, and
//! list markers; inline emphasis maps to `/…/`, `*…*`, `_…_`, and `+…+`, inline code to `=…=`, and
//! sub/superscripts and math to their brace and `\(…\)` forms. Footnotes are collected while
//! rendering and emitted as a trailing section. Output carries no trailing newline; the caller
//! appends one. Content is wrapped at a fill column of 72.

use carta_ast::{
    Attr, Block, Caption, Cell, Citation, CitationMode, Document, Format, Inline, ListAttributes,
    ListNumberDelim, MathType, QuoteType, Row, Table, Target, to_plain_text,
};
use carta_core::{Extension, Result, WrapMode, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, Piece, append_notes, attribute_value, display_width, fill, fill_hang,
    indent_block, item_separator, offset_as_i32, quote_marks,
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
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let width = options.columns.unwrap_or(FILL_COLUMN);
        let mut state = State {
            wrap: options.wrap,
            width,
            smart: options.extensions.contains(Extension::Smart),
            task_lists: options.extensions.contains(Extension::TaskLists),
            citations: options.extensions.contains(Extension::Citations),
            notes: Vec::new(),
        };
        let body = state.blocks(&document.blocks, width, false);
        Ok(append_notes(body, &state.notes))
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
    fn blocks(&mut self, blocks: &[Block], width: usize, hang_first: bool) -> String {
        let mut out = String::new();
        let mut previous: Option<&Block> = None;
        let mut first = true;
        for block in blocks {
            let text = self.block(block, width, hang_first && first);
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

    fn block(&mut self, block: &Block, width: usize, hang: bool) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => self.leaf(inlines, width, hang),
            Block::LineBlock(lines) => self.line_block(lines),
            Block::CodeBlock(attr, text) => code_block(attr, text),
            Block::RawBlock(format, text) => raw_block(format, text),
            Block::BlockQuote(blocks) => {
                let body = self.blocks(blocks, width, false);
                fence("#+begin_quote", &body, "#+end_quote")
            }
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::BulletList(items) => self.bullet_list(items, width),
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
    /// a heading, keyword, or table.
    fn leaf(&mut self, inlines: &[Inline], width: usize, hang: bool) -> String {
        let pieces = self.pieces(inlines);
        if hang {
            fill_hang(&pieces, width, self.wrap)
        } else {
            protect_leading(fill(&pieces, width, self.wrap))
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

    fn header(&mut self, level: i32, attr: &Attr, inlines: &[Inline]) -> String {
        let stars = "*".repeat(usize::try_from(level.max(1)).unwrap_or(1));
        let text = self.flat(inlines);
        let heading = format!("{stars} {text}");
        let props = properties(attr);
        if props.is_empty() {
            heading
        } else {
            format!("{heading}\n{props}")
        }
    }

    fn bullet_list(&mut self, items: &[Vec<Block>], width: usize) -> String {
        let loose = !list_is_tight(items);
        let mut units = Vec::new();
        for item in items {
            let item = self.checkbox_item(item);
            let body = self.blocks(&item, width.saturating_sub(2), true);
            units.push(indent_block(&body, "- ", "  "));
        }
        join_items(units, item_separator(loose))
    }

    /// Replace a leading Org-ballot checkbox character in a task-list item with an Org checkbox
    /// marker, returning the item unchanged when task lists are off or the item does not open with a
    /// checkbox.
    fn checkbox_item(&self, item: &[Block]) -> Vec<Block> {
        if !self.task_lists {
            return item.to_vec();
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
            return item.to_vec();
        };
        let mut blocks = item.to_vec();
        if let Some(Block::Plain(inlines) | Block::Para(inlines)) = blocks.first_mut()
            && let Some(Inline::Str(text)) = inlines.first_mut()
        {
            let rest: String = text.chars().skip(1).collect();
            *text = format!("{marker}{rest}");
        }
        blocks
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
            let number = attrs.start.saturating_add(offset_as_i32(index));
            let marker = format!("{number}{delim}");
            let field = marker.chars().count() + 1;
            let cookie = if index == 0 && attrs.start != 1 {
                format!("[@{}] ", attrs.start)
            } else {
                String::new()
            };
            let first = format!("{marker} {cookie}");
            let rest = " ".repeat(field);
            let body = self.blocks(item, width.saturating_sub(field), true);
            units.push(indent_block(&body, &first, &rest));
        }
        join_items(units, item_separator(loose))
    }

    fn definition_list(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        let loose = items.iter().any(|(_, definitions)| {
            definitions
                .iter()
                .any(|blocks| matches!(blocks.first(), Some(Block::Para(_))))
        });
        let mut units = Vec::new();
        for (term, definitions) in items {
            units.push(self.definition_item(term, definitions, width));
        }
        join_items(units, item_separator(loose))
    }

    fn definition_item(
        &mut self,
        term: &[Inline],
        definitions: &[Vec<Block>],
        width: usize,
    ) -> String {
        let mut blocks: Vec<Block> = definitions.iter().flatten().cloned().collect();
        if blocks.is_empty() {
            let term_text = self.flat(term);
            return format!("- {term_text} :: ");
        }
        fuse_term(term, &mut blocks);
        let body = self.blocks(&blocks, width.saturating_sub(2), true);
        indent_block(&body, "- ", "  ")
    }

    fn table(&mut self, table: &Table, _width: usize) -> String {
        let caption = self.caption_line(&table.caption);
        let columns = table.col_specs.len();
        if columns == 0 {
            return caption;
        }

        let mut rows: Vec<&Row> = table.head.rows.iter().collect();
        let head_count = rows.len();
        for body in &table.bodies {
            rows.extend(body.head.iter().chain(body.body.iter()));
        }
        rows.extend(table.foot.rows.iter());

        let grid = self.lay_grid(&rows, columns);
        let widths = column_widths(&grid, columns);

        let mut lines: Vec<String> = Vec::new();
        for (index, row) in grid.iter().enumerate() {
            let height = row
                .iter()
                .filter_map(|slot| slot.as_ref().map(Vec::len))
                .max()
                .unwrap_or(1)
                .max(1);
            for line in 0..height {
                let fields: Vec<String> = row
                    .iter()
                    .map(|slot| {
                        slot.as_ref()
                            .and_then(|cell_lines| cell_lines.get(line))
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
        let mut grid: CellGrid = vec![vec![None; columns]; rows.len()];
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
                let row_span = usize::try_from(cell.row_span.max(1)).unwrap_or(1);
                let col_span = usize::try_from(cell.col_span.max(1)).unwrap_or(1);
                let cell_lines = self.cell_lines(cell);
                if let Some(slot) = grid.get_mut(index).and_then(|line| line.get_mut(column)) {
                    *slot = Some(cell_lines);
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
    /// a nested list or several blocks expand the cell across several table lines.
    fn cell_lines(&mut self, cell: &Cell) -> Vec<String> {
        self.blocks(&cell.content, CELL_WIDTH, false)
            .split('\n')
            .map(str::to_string)
            .collect()
    }

    fn figure(&mut self, attr: &Attr, caption: &Caption, blocks: &[Block], width: usize) -> String {
        let mut body = self.blocks(blocks, width, false);
        if !attr.id.is_empty() {
            body = format!("<<{}>>\n{body}", attr.id);
        }
        let caption = self.caption_line(caption);
        if caption.is_empty() {
            body
        } else {
            format!("#+caption: {caption}\n{body}")
        }
    }

    fn caption_line(&mut self, caption: &Caption) -> String {
        let mut inlines: Vec<Inline> = Vec::new();
        for block in &caption.long {
            if let Block::Plain(content) | Block::Para(content) = block {
                if !inlines.is_empty() {
                    inlines.push(Inline::LineBreak);
                }
                inlines.extend(content.iter().cloned());
            }
        }
        self.flat(&inlines)
    }

    fn div(&mut self, attr: &Attr, blocks: &[Block], width: usize) -> String {
        let body = self.blocks(blocks, width, false);
        match special_div(&attr.classes) {
            Some(index) => {
                let name = attr.classes.get(index).map_or("", String::as_str);
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
                    lines.push(body);
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
                    format!("<<{}>>\n{body}", attr.id)
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
        let body = self.blocks(blocks, self.width.saturating_sub(field), false);
        let definition = if body.is_empty() {
            marker.clone()
        } else {
            indent_block(&body, &format!("{marker} "), &" ".repeat(field))
        };
        if let Some(slot) = self.notes.get_mut(index) {
            *slot = definition;
        }
        marker
    }

    fn citation(&mut self, citations: &[Citation]) -> String {
        let variant = match citations.first().map(|citation| &citation.mode) {
            Some(CitationMode::AuthorInText) => "/t",
            Some(CitationMode::SuppressAuthor) => "/na",
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
        let suffix_text = self.flat(&citation.suffix);
        let suffix = suffix_text.strip_prefix(',').unwrap_or(&suffix_text);
        let mut item = String::new();
        if !prefix.is_empty() {
            item.push_str(&prefix);
            item.push(' ');
        }
        item.push('@');
        item.push_str(&citation.id);
        item.push_str(suffix);
        item
    }

    /// Render inlines to a breakable stream of fill pieces. Markup spans become a single unbreakable
    /// run so they are never reflowed; small caps and generic spans are transparent; a quotation glues
    /// its glyphs to its content's outer pieces so the interior can still wrap.
    fn pieces(&mut self, inlines: &[Inline]) -> Vec<Piece> {
        let mut out = Vec::new();
        for inline in inlines {
            self.piece(inline, &mut out);
        }
        out
    }

    fn piece(&mut self, inline: &Inline, out: &mut Vec<Piece>) {
        match inline {
            Inline::Str(text) => out.push(Piece::Text(special_strings(text))),
            Inline::Space => out.push(Piece::Space),
            Inline::SoftBreak => out.push(Piece::Soft),
            Inline::LineBreak => {
                out.push(Piece::Text("\\\\".to_string()));
                out.push(Piece::Hard);
            }
            Inline::Emph(content) => out.push(Piece::Text(format!("/{}/", self.flat(content)))),
            Inline::Strong(content) => out.push(Piece::Text(format!("*{}*", self.flat(content)))),
            Inline::Underline(content) => {
                out.push(Piece::Text(format!("_{}_", self.flat(content))));
            }
            Inline::Strikeout(content) => {
                out.push(Piece::Text(format!("+{}+", self.flat(content))));
            }
            Inline::Superscript(content) => {
                out.push(Piece::Text(format!("^{{{}}}", self.flat(content))));
            }
            Inline::Subscript(content) => {
                out.push(Piece::Text(format!("_{{{}}}", self.flat(content))));
            }
            Inline::SmallCaps(content) | Inline::Span(_, content) => {
                for inner in content {
                    self.piece(inner, out);
                }
            }
            Inline::Quoted(kind, content) => {
                let (open, close) = self.quote_glyphs(kind);
                out.push(Piece::Text(open.to_string()));
                for inner in content {
                    self.piece(inner, out);
                }
                out.push(Piece::Text(close.to_string()));
            }
            Inline::Code(_, text) => out.push(Piece::Text(format!("={text}="))),
            Inline::Math(MathType::InlineMath, text) => {
                out.push(Piece::Text(format!("\\({text}\\)")));
            }
            Inline::Math(MathType::DisplayMath, text) => {
                out.push(Piece::Text(format!("\\[{text}\\]")));
            }
            Inline::RawInline(format, text) => {
                if is_raw_org(format) {
                    out.push(Piece::Text(text.clone()));
                }
            }
            Inline::Link(_, label, target) => out.push(Piece::Text(self.link(label, target))),
            Inline::Image(_, _, target) => out.push(Piece::Text(image(target))),
            Inline::Cite(citations, fallback) => {
                if self.citations {
                    out.push(Piece::Text(self.citation(citations)));
                } else {
                    for inner in fallback {
                        self.piece(inner, out);
                    }
                }
            }
            Inline::Note(blocks) => out.push(Piece::Text(self.record_note(blocks))),
        }
    }

    /// Render inlines to a flat, single-block string: the fill pieces joined with spaces for breaks.
    /// Used where inline content may not reflow — headings, list terms, captions, table cells, and the
    /// interior of a markup span.
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
        let path = org_path(&target.url);
        if to_plain_text(label) == target.url {
            format!("[[{path}]]")
        } else {
            format!("[[{path}][{}]]", self.flat(label))
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

/// The separator between two consecutive blocks: a blank line when the earlier block wants trailing
/// space or the later block wants leading space, a single newline otherwise. A plain block, heading,
/// or raw Org block attaches tightly to what follows; a block quote, div, verse, rule, table, or raw
/// HTML block is set off from what precedes it.
fn block_separator(previous: &Block, next: &Block) -> &'static str {
    if trailing_blank(previous) || leading_blank(next) {
        "\n\n"
    } else {
        "\n"
    }
}

/// Whether a block is followed by a blank line: everything except a plain block, a heading, and a raw
/// Org block, which attach to what follows.
fn trailing_blank(block: &Block) -> bool {
    !matches!(block, Block::Plain(_) | Block::Header(..)) && !is_raw_org_block(block)
}

/// Whether a block is preceded by a blank line: the block-level constructs that stand apart from what
/// comes before them.
fn leading_blank(block: &Block) -> bool {
    match block {
        Block::BlockQuote(_)
        | Block::Div(..)
        | Block::LineBlock(_)
        | Block::HorizontalRule
        | Block::Table(_) => true,
        Block::RawBlock(format, _) => format.0.eq_ignore_ascii_case("html"),
        _ => false,
    }
}

fn is_raw_org_block(block: &Block) -> bool {
    matches!(block, Block::RawBlock(format, _) if format.0.eq_ignore_ascii_case("org"))
}

/// Whether a list is tight — every item's blocks are plain text or a nested tight list, so items are
/// separated by a single newline. Any paragraph, quote, code, or loose nested list makes it loose.
fn list_is_tight(items: &[Vec<Block>]) -> bool {
    items.iter().all(|item| item.iter().all(tight_block))
}

fn tight_block(block: &Block) -> bool {
    match block {
        Block::Plain(_) => true,
        Block::BulletList(items) | Block::OrderedList(_, items) => list_is_tight(items),
        _ => false,
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

/// Join rendered list items with `separator`, dropping any that produced no output.
fn join_items(units: Vec<String>, separator: &str) -> String {
    units
        .into_iter()
        .filter(|unit| !unit.is_empty())
        .collect::<Vec<_>>()
        .join(separator)
}

/// Guard a paragraph whose first character would open Org syntax with a zero-width space.
fn protect_leading(text: String) -> String {
    match text.chars().next() {
        Some('*' | '#' | '|') => format!("{ZERO_WIDTH_SPACE}{text}"),
        _ => text,
    }
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

/// Prepend a definition term to its first definition block so the term and its `::` marker share the
/// first line, inserting a plain block when the first definition block carries no inline content.
fn fuse_term(term: &[Inline], blocks: &mut Vec<Block>) {
    if let Some(Block::Plain(inlines) | Block::Para(inlines)) = blocks.first_mut() {
        let mut fused = term.to_vec();
        fused.push(Inline::Space);
        fused.push(Inline::Str("::".to_string()));
        fused.push(Inline::Space);
        fused.append(inlines);
        *inlines = fused;
    } else {
        let mut lead = term.to_vec();
        lead.push(Inline::Space);
        lead.push(Inline::Str("::".to_string()));
        blocks.insert(0, Block::Plain(lead));
    }
}

/// Render an Org code block: a plain `example` block when it carries no class, otherwise a `src` block
/// tagged with its language and line-numbering switch. A leading identifier becomes a `#+name:` line.
fn code_block(attr: &Attr, text: &str) -> String {
    let content = escape_code(text);
    let block = if let Some(class) = attr.classes.first() {
        let language = translate_language(class);
        let numbered = attr.classes.iter().any(|class| {
            matches!(
                class.as_str(),
                "numberLines" | "number-lines" | "numberlines"
            )
        });
        let mut begin = format!("#+begin_src {language}");
        if numbered {
            match attribute_value(attr, "startFrom") {
                Some(start) => {
                    begin.push_str(" -n ");
                    begin.push_str(start);
                }
                None => begin.push_str(" -n"),
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

/// The index of the first class naming a special Org block, matched case-insensitively.
fn special_div(classes: &[String]) -> Option<usize> {
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
fn attr_html_line(extra: &[&str], attributes: &[(String, String)]) -> Option<String> {
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

/// Whether a URL opens with a `scheme:` prefix — a non-empty run of scheme characters before a colon.
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

/// Render a table body row, padding each cell to its column width and framing it with pipes.
/// A table laid out as rows of column slots. An occupied slot holds a cell's rendered lines; a slot
/// left empty by a span or a short row is `None`.
type CellGrid = Vec<Vec<Option<Vec<String>>>>;

/// The display width of each column: the widest rendered line whose cell begins in that column. A
/// cell that spans several columns contributes only to the leftmost.
fn column_widths(grid: &CellGrid, columns: usize) -> Vec<usize> {
    let mut widths = vec![0usize; columns];
    for row in grid {
        for (column, slot) in row.iter().enumerate() {
            if let Some(cell_lines) = slot {
                let width = cell_lines
                    .iter()
                    .map(|line| display_width(line))
                    .max()
                    .unwrap_or(0);
                if let Some(current) = widths.get_mut(column) {
                    *current = (*current).max(width);
                }
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
