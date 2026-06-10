//! LaTeX writer: renders the document model to a LaTeX document fragment.
//!
//! Output is a body fragment (no preamble or `\begin{document}`) wrapped at a fill column of 72;
//! the wrap counts the literal LaTeX, markup included. Document metadata is not emitted. Syntax
//! highlighting is neutralized: a code block renders as a `verbatim` environment and inline code as
//! `\texttt{…}`, regardless of any language class. The result carries no trailing newline; the
//! caller appends one. This format has no public specification.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColWidth, Document, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MathType, QuoteType, Row, Table, Target, to_plain_text,
};
use carta_core::{Result, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, Piece, attribute_value, display_width, fill, indent_block, list_is_tight,
    wrap_delim,
};
use crate::grid;

/// Renders a document to a LaTeX fragment.
#[derive(Debug, Default, Clone, Copy)]
pub struct LatexWriter;

impl Writer for LatexWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let body = render_blocks(&document.blocks, FILL_COLUMN, 0);
        Ok(body.trim_end_matches('\n').to_owned())
    }
}

/// Selects the escaping policy for a run of literal text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeMode {
    /// Running prose.
    Text,
    /// Inside a `\texttt{…}` group, where spaces and a few extra glyphs gain escapes.
    Code,
}

/// Render a block sequence with a blank line between blocks, dropping those that produce no output.
fn render_blocks(blocks: &[Block], width: usize, enum_depth: usize) -> String {
    blocks
        .iter()
        .map(|block| block_to_string(block, width, enum_depth))
        .filter(|rendered| !rendered.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn block_to_string(block: &Block, width: usize, enum_depth: usize) -> String {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => inlines_to_string(inlines, width),
        Block::Header(level, attr, inlines) => header(*level, attr, inlines, width),
        Block::CodeBlock(attr, text) => code_block(attr, text),
        Block::RawBlock(format, text) => {
            if is_latex_format(&format.0) {
                text.strip_suffix('\n').unwrap_or(text).to_owned()
            } else {
                String::new()
            }
        }
        Block::BlockQuote(blocks) => format!(
            "\\begin{{quote}}\n{}\n\\end{{quote}}",
            render_blocks(blocks, width, enum_depth)
        ),
        Block::BulletList(items) => bullet_list(items, width, enum_depth),
        Block::OrderedList(attrs, items) => ordered_list(attrs, items, width, enum_depth),
        Block::DefinitionList(items) => definition_list(items, width, enum_depth),
        Block::HorizontalRule => {
            "\\begin{center}\\rule{0.5\\linewidth}{0.5pt}\\end{center}".to_owned()
        }
        Block::LineBlock(lines) => line_block(lines, width),
        Block::Div(attr, blocks) => {
            let body = render_blocks(blocks, width, enum_depth);
            if attr.id.is_empty() {
                body
            } else {
                format!("{}\n{body}", phantom_label(&attr.id))
            }
        }
        Block::Figure(attr, caption, blocks) => figure(attr, caption, blocks, width, enum_depth),
        Block::Table(table) => render_table(table, width),
    }
}

fn header(level: i32, attr: &Attr, inlines: &[Inline], width: usize) -> String {
    let command = match level {
        1 => "section",
        2 => "subsection",
        3 => "subsubsection",
        4 => "paragraph",
        5 => "subparagraph",
        _ => return inlines_to_string(inlines, width),
    };
    let unnumbered = attr.classes.iter().any(|class| class == "unnumbered");
    let star = if unnumbered { "*" } else { "" };
    let inner = inline_pieces(inlines);

    let mut content = vec![Piece::Text(format!("\\{command}{star}{{"))];
    if needs_texorpdfstring(inlines) {
        content.push(Piece::Text("\\texorpdfstring{".to_owned()));
        content.extend(inner.iter().cloned());
        let pdf = escape(&to_plain_text(inlines), EscapeMode::Text);
        content.push(Piece::Text(format!("}}{{{pdf}}}")));
    } else {
        content.extend(inner.iter().cloned());
    }
    content.push(Piece::Text("}".to_owned()));
    if !attr.id.is_empty() {
        content.push(Piece::Text(format!("\\label{{{}}}", attr.id)));
    }
    let heading = fill(&content, width);

    if unnumbered {
        let mut toc = vec![Piece::Text(format!(
            "\\addcontentsline{{toc}}{{{command}}}{{"
        ))];
        toc.extend(inner);
        toc.push(Piece::Text("}".to_owned()));
        format!("{heading}\n{}", fill(&toc, width))
    } else {
        heading
    }
}

fn code_block(attr: &Attr, text: &str) -> String {
    code_block_env(attr, text, "verbatim")
}

fn code_block_env(attr: &Attr, text: &str, environment: &str) -> String {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let verbatim = format!("\\begin{{{environment}}}\n{body}\n\\end{{{environment}}}");
    if attr.id.is_empty() {
        verbatim
    } else {
        format!("{}%\n{verbatim}", phantom_label(&attr.id))
    }
}

/// The anchor markup emitted for an element carrying an identifier.
fn phantom_label(id: &str) -> String {
    format!("\\protect\\phantomsection\\label{{{id}}}")
}

fn bullet_list(items: &[Vec<Block>], width: usize, enum_depth: usize) -> String {
    let mut lines = vec!["\\begin{itemize}".to_owned()];
    if list_is_tight(items) {
        lines.push("\\tightlist".to_owned());
    }
    for item in items {
        lines.push(list_item(item, width, enum_depth));
    }
    lines.push("\\end{itemize}".to_owned());
    lines.join("\n")
}

fn ordered_list(
    attrs: &ListAttributes,
    items: &[Vec<Block>],
    width: usize,
    enum_depth: usize,
) -> String {
    let depth = enum_depth + 1;
    let counter = enum_counter(depth);
    let mut lines = vec!["\\begin{enumerate}".to_owned()];
    if let Some(label) = label_definition(attrs, counter) {
        lines.push(label);
    }
    if attrs.start != 1 {
        lines.push(format!(
            "\\setcounter{{{counter}}}{{{}}}",
            attrs.start.saturating_sub(1)
        ));
    }
    if list_is_tight(items) {
        lines.push("\\tightlist".to_owned());
    }
    for item in items {
        lines.push(list_item(item, width, depth));
    }
    lines.push("\\end{enumerate}".to_owned());
    lines.join("\n")
}

/// Render one list item: its blocks indented two columns under an `\item` line.
fn list_item(item: &[Block], width: usize, enum_depth: usize) -> String {
    let body = render_blocks(item, width.saturating_sub(2), enum_depth);
    let content = indent_block(&body, "  ", "  ");
    if content.is_empty() {
        "\\item".to_owned()
    } else {
        format!("\\item\n{content}")
    }
}

fn definition_list(
    items: &[(Vec<Inline>, Vec<Vec<Block>>)],
    width: usize,
    enum_depth: usize,
) -> String {
    let mut lines = vec!["\\begin{description}".to_owned()];
    if is_tight_definitions(items) {
        lines.push("\\tightlist".to_owned());
    }
    for (term, definitions) in items {
        let header = format!("\\item[{}]", inlines_to_string(term, width));
        let bodies: Vec<String> = definitions
            .iter()
            .map(|definition| render_blocks(definition, width, enum_depth))
            .filter(|rendered| !rendered.is_empty())
            .collect();
        if bodies.is_empty() {
            lines.push(header);
        } else {
            lines.push(format!("{header}\n{}", bodies.join("\n\n")));
        }
    }
    lines.push("\\end{description}".to_owned());
    lines.join("\n")
}

fn line_block(lines: &[Vec<Inline>], width: usize) -> String {
    lines
        .iter()
        .map(|line| inlines_to_string(line, width))
        .collect::<Vec<_>>()
        .join("\\\\\n")
}

fn figure(
    attr: &Attr,
    caption: &Caption,
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
) -> String {
    let mut parts = vec![
        "\\begin{figure}".to_owned(),
        "\\centering".to_owned(),
        render_blocks(blocks, width, enum_depth),
    ];
    if !attr.id.is_empty() {
        parts.push(format!("\\label{{{}}}", attr.id));
    }
    let caption_inlines = caption_text(caption);
    if !caption_inlines.is_empty() {
        parts.push(format!(
            "\\caption{{{}}}",
            inlines_to_string(&caption_inlines, width)
        ));
    }
    parts.push("\\end{figure}".to_owned());
    parts.join("\n")
}

/// Collect a caption's inline content from its block-level body.
fn caption_text(caption: &Caption) -> Vec<Inline> {
    let mut out = Vec::new();
    for block in &caption.long {
        if let Block::Plain(inlines) | Block::Para(inlines) = block {
            out.extend(inlines.iter().cloned());
        }
    }
    out
}

/// Render a table as a `longtable` environment. A captionless table is wrapped so its float
/// counter is not advanced; a captioned one carries the caption and repeats its head for page
/// breaks. Spans become `\multicolumn`/`\multirow`; columns are letter classes unless explicit or
/// block-level cells call for sized `p{…}` columns with minipage cells.
fn render_table(table: &Table, width: usize) -> String {
    let plan = ColumnPlan::new(table);
    let head_rows: Vec<&Row> = table.head.rows.iter().collect();
    let body_rows: Vec<&Row> = table
        .bodies
        .iter()
        .flat_map(|body| body.head.iter().chain(body.body.iter()))
        .collect();
    let foot_rows: Vec<&Row> = table.foot.rows.iter().collect();

    let head_lines = render_section(&head_rows, &plan, true, width);
    let body_lines = render_section(&body_rows, &plan, false, width);
    let foot_lines = render_section(&foot_rows, &plan, false, width);
    let caption = table_caption(&table.caption, &table.attr, width);

    let mut parts = vec![format!("\\begin{{longtable}}[]{{{}}}", plan.colspec())];
    if let Some(caption) = &caption {
        parts.push(caption.clone());
        parts.push(head_block(&head_lines, "\\endfirsthead"));
        parts.push(head_block(&head_lines, "\\endhead"));
    } else {
        parts.push(head_block(&head_lines, "\\endhead"));
    }
    if !foot_lines.is_empty() {
        parts.push("\\midrule\\noalign{}".to_owned());
        parts.extend(foot_lines);
    }
    parts.push("\\bottomrule\\noalign{}".to_owned());
    parts.push("\\endlastfoot".to_owned());
    parts.extend(body_lines);
    parts.push("\\end{longtable}".to_owned());
    let body = parts.join("\n");

    if caption.is_some() {
        body
    } else {
        format!("{{\\def\\LTcaptype{{none}} % do not increment counter\n{body}\n}}")
    }
}

/// The head segment of a `longtable`: a top rule, the head rows, a closing rule when the head is
/// non-empty, and the terminating macro (`\endhead` or `\endfirsthead`).
fn head_block(head_lines: &[String], terminator: &str) -> String {
    let mut parts = vec!["\\toprule\\noalign{}".to_owned()];
    parts.extend(head_lines.iter().cloned());
    if !head_lines.is_empty() {
        parts.push("\\midrule\\noalign{}".to_owned());
    }
    parts.push(terminator.to_owned());
    parts.join("\n")
}

/// How a table's columns are sized and aligned.
struct ColumnPlan {
    columns: usize,
    aligns: Vec<Alignment>,
    /// At least one column carries an explicit fractional width.
    explicit: bool,
    /// Columns render as sized `p{…}` boxes (explicit widths, or block-level cell content).
    sized: bool,
    /// Each column's width fraction: the explicit fraction when present, else an equal share.
    fractions: Vec<f64>,
}

impl ColumnPlan {
    fn new(table: &Table) -> ColumnPlan {
        let columns = table.col_specs.len();
        let aligns: Vec<Alignment> = table
            .col_specs
            .iter()
            .map(|spec| spec.align.clone())
            .collect();
        let explicit = table
            .col_specs
            .iter()
            .any(|spec| matches!(spec.width, ColWidth::ColWidth(fraction) if fraction > 0.0));
        let block_cells =
            all_rows(table).any(|row| row.cells.iter().any(|cell| !is_simple_cell(cell)));
        let sized = explicit || block_cells;
        let fractions = if explicit {
            table
                .col_specs
                .iter()
                .map(|spec| match spec.width {
                    ColWidth::ColWidth(fraction) => fraction,
                    ColWidth::ColWidthDefault => 0.0,
                })
                .collect()
        } else if columns > 0 {
            vec![1.0 / columns as f64; columns]
        } else {
            Vec::new()
        };
        ColumnPlan {
            columns,
            aligns,
            explicit,
            sized,
            fractions,
        }
    }

    /// The column descriptor between the `{…}` of `\begin{longtable}`.
    fn colspec(&self) -> String {
        if self.sized {
            let pad = 2 * self.columns.saturating_sub(1);
            let columns: Vec<String> = (0..self.columns)
                .map(|index| {
                    let align = self.aligns.get(index).cloned().unwrap_or(Alignment::AlignDefault);
                    let fraction = self.fractions.get(index).copied().unwrap_or(0.0);
                    format!(
                        "  >{{{}\\arraybackslash}}p{{(\\linewidth - {pad}\\tabcolsep) * \\real{{{fraction:.4}}}}}",
                        column_command(&align)
                    )
                })
                .collect();
            format!("@{{}}\n{}@{{}}", columns.join("\n"))
        } else if self.aligns.is_empty() {
            "@{}l@{}".to_owned()
        } else {
            let letters: String = self.aligns.iter().map(column_letter).collect();
            format!("@{{}}{letters}@{{}}")
        }
    }
}

/// Iterate every row of a table: header, each body's intermediate-head and body rows, then footer.
fn all_rows(table: &Table) -> impl Iterator<Item = &Row> {
    table
        .head
        .rows
        .iter()
        .chain(
            table
                .bodies
                .iter()
                .flat_map(|body| body.head.iter().chain(body.body.iter())),
        )
        .chain(table.foot.rows.iter())
}

/// Render a section's rows to `longtable` lines, resolving spans against the section's own grid.
fn render_section(rows: &[&Row], plan: &ColumnPlan, head: bool, width: usize) -> Vec<String> {
    let placements = grid::place_columns(rows, plan.columns);
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let row_placements = placements.get(index).map_or(&[][..], Vec::as_slice);
            render_row(row, row_placements, plan, head, width)
        })
        .collect()
}

/// A single layout token in a table row: an unbreakable word, a breakable space, or a forced line
/// break that survives reflow (it never collapses with an adjacent break).
enum Token {
    Word(String),
    Space,
    Break,
}

/// Render one row, reflowing it at `width` while preserving the hard breaks inside multi-line
/// cells. Fields are separated by ` & ` and the row ends with ` \\`; a column covered by a span
/// from an earlier or wider cell contributes an empty field.
fn render_row(
    row: &Row,
    placements: &[(usize, usize)],
    plan: &ColumnPlan,
    head: bool,
    width: usize,
) -> String {
    let mut tokens: Vec<Token> = Vec::new();
    let mut cells = row.cells.iter().zip(placements.iter());
    let mut next = cells.next();
    let mut column = 0usize;
    let mut first = true;
    while column < plan.columns {
        if !first {
            tokens.push(Token::Space);
            tokens.push(Token::Word("&".to_owned()));
            tokens.push(Token::Space);
        }
        first = false;
        match next {
            Some((cell, &(start, span))) if start == column => {
                render_field(&mut tokens, cell, start, span, plan, head, width);
                column += span.max(1);
                next = cells.next();
            }
            _ => column += 1,
        }
    }
    while matches!(tokens.last(), Some(Token::Space)) {
        tokens.pop();
    }
    glue_suffix(&mut tokens, " \\\\");
    layout_row(&tokens, width)
}

/// Split a rendered field into layout tokens: inter-word spaces become breakable, newlines forced
/// breaks. A line's leading indentation glues to its first word so block-level structure survives
/// reflow rather than collapsing at the line start.
fn push_field_tokens(tokens: &mut Vec<Token>, field: &str) {
    for (line_index, line) in field.split('\n').enumerate() {
        if line_index > 0 {
            tokens.push(Token::Break);
        }
        let trimmed = line.trim_start_matches(' ');
        let indent = &line[..line.len() - trimmed.len()];
        for (word_index, word) in trimmed.split(' ').enumerate() {
            if word_index > 0 {
                tokens.push(Token::Space);
            }
            if word.is_empty() {
                continue;
            }
            if word_index == 0 && !indent.is_empty() {
                tokens.push(Token::Word(format!("{indent}{word}")));
            } else {
                tokens.push(Token::Word(word.to_owned()));
            }
        }
    }
}

/// Greedily lay tokens out at `width`: place words separated by single spaces, break a line before
/// a word that would overflow, and honor forced breaks verbatim.
fn layout_row(tokens: &[Token], width: usize) -> String {
    let mut out = String::new();
    let mut column = 0usize;
    let mut line_start = true;
    let mut pending_space = false;
    for token in tokens {
        match token {
            Token::Word(word) => {
                let word_width = display_width(word);
                if line_start {
                    out.push_str(word);
                    column = word_width;
                    line_start = false;
                } else if pending_space && column + 1 + word_width > width {
                    out.push('\n');
                    out.push_str(word);
                    column = word_width;
                } else if pending_space {
                    out.push(' ');
                    out.push_str(word);
                    column += 1 + word_width;
                } else {
                    out.push_str(word);
                    column += word_width;
                }
                pending_space = false;
            }
            Token::Space => pending_space = true,
            Token::Break => {
                out.push('\n');
                column = 0;
                line_start = true;
                pending_space = false;
            }
        }
    }
    out
}

/// Render one field into the row's token stream, wrapping the cell content in `\multirow` and
/// `\multicolumn` for its spans. A span's structural markup (the `\multicolumn` opening with its
/// sized column spec) is emitted as unbreakable words so reflow never splits it.
fn render_field(
    tokens: &mut Vec<Token>,
    cell: &Cell,
    start: usize,
    span: usize,
    plan: &ColumnPlan,
    head: bool,
    width: usize,
) {
    let inner = render_cell(cell, start, plan, head, width);
    let mut field: Vec<Token> = Vec::new();
    push_field_tokens(&mut field, &inner);

    let row_span = cell.row_span.max(1);
    if row_span > 1 {
        let prefix = multirow_prefix(&resolved_align(cell, start, plan));
        glue_prefix(
            &mut field,
            &format!("\\multirow{{{row_span}}}{{*}}{{{prefix}"),
        );
        glue_suffix(&mut field, "}");
    }
    if span > 1 {
        let spec = multicolumn_spec(cell, start, span, plan);
        glue_suffix(&mut field, "}");
        let mut wrapped = vec![
            Token::Word(format!("\\multicolumn{{{span}}}{{{spec}}}{{%")),
            Token::Break,
        ];
        wrapped.append(&mut field);
        field = wrapped;
    }
    tokens.append(&mut field);
}

/// Glue a literal prefix onto a field's first word so it stays unbreakable with the content.
fn glue_prefix(tokens: &mut Vec<Token>, prefix: &str) {
    match tokens.first_mut() {
        Some(Token::Word(word)) => *word = format!("{prefix}{word}"),
        _ => tokens.insert(0, Token::Word(prefix.to_owned())),
    }
}

/// Glue a literal suffix onto a field's last word.
fn glue_suffix(tokens: &mut Vec<Token>, suffix: &str) {
    match tokens.last_mut() {
        Some(Token::Word(word)) => word.push_str(suffix),
        _ => tokens.push(Token::Word(suffix.to_owned())),
    }
}

/// Render a cell's content, wrapping it in a `minipage` when the columns are explicitly sized and
/// the cell is a header cell or carries block-level content.
fn render_cell(cell: &Cell, start: usize, plan: &ColumnPlan, head: bool, width: usize) -> String {
    let content = cell_content(cell, width);
    if plan.explicit && (head || !is_simple_cell(cell)) {
        minipage(
            head,
            column_command(&resolved_align(cell, start, plan)),
            &content,
        )
    } else {
        content
    }
}

/// A `minipage` cell box. Header cells sit on the bottom baseline, body cells on the top; a hard
/// line break in the content appends a `\strut` so the final line keeps its full height.
fn minipage(head: bool, align: &str, content: &str) -> String {
    let position = if head { "b" } else { "t" };
    let mut lines = vec![format!(
        "\\begin{{minipage}}[{position}]{{\\linewidth}}{align}"
    )];
    if !content.is_empty() {
        let mut content_lines: Vec<String> = content.split('\n').map(str::to_owned).collect();
        if content.contains("\\\\")
            && let Some(last) = content_lines.last_mut()
        {
            last.push_str("\\strut");
        }
        lines.extend(content_lines);
    }
    lines.push("\\end{minipage}".to_owned());
    lines.join("\n")
}

/// The column specification inside a `\multicolumn`: a letter class or a sized `p{…}` box, bounded
/// by `@{}` on the table's outer edges.
fn multicolumn_spec(cell: &Cell, start: usize, span: usize, plan: &ColumnPlan) -> String {
    let align = resolved_align(cell, start, plan);
    let lead = if start == 0 { "@{}" } else { "" };
    let trail = if start + span == plan.columns {
        "@{}"
    } else {
        ""
    };
    if plan.sized {
        let pad = 2 * plan.columns.saturating_sub(1);
        let fraction: f64 = plan.fractions.iter().skip(start).take(span).sum();
        let extra = 2 * span.saturating_sub(1);
        format!(
            "{lead}>{{{}\\arraybackslash}}p{{(\\linewidth - {pad}\\tabcolsep) * \\real{{{fraction:.4}}} + {extra}\\tabcolsep}}{trail}",
            column_command(&align)
        )
    } else {
        format!("{lead}{}{trail}", column_letter(&align))
    }
}

/// Render a cell's content to a string: simple cells flatten to one logical line, block-level cells
/// render as stacked blocks.
fn cell_content(cell: &Cell, width: usize) -> String {
    if is_simple_cell(cell) {
        match cell.content.first() {
            Some(Block::Plain(inlines) | Block::Para(inlines)) => {
                flatten_pieces(&inline_pieces(inlines))
            }
            _ => String::new(),
        }
    } else {
        render_blocks(&cell.content, width, 0)
    }
}

/// Whether a cell holds at most a single paragraph, so it can render without a minipage box.
fn is_simple_cell(cell: &Cell) -> bool {
    match cell.content.as_slice() {
        [] => true,
        [block] => matches!(block, Block::Plain(_) | Block::Para(_)),
        _ => false,
    }
}

/// Flatten layout pieces to a string, keeping hard breaks as newlines.
fn flatten_pieces(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Space => out.push(' '),
            Piece::Hard => out.push('\n'),
        }
    }
    out
}

/// A cell's effective alignment: its own when set, otherwise its starting column's.
fn resolved_align(cell: &Cell, column: usize, plan: &ColumnPlan) -> Alignment {
    if cell.align == Alignment::AlignDefault {
        plan.aligns
            .get(column)
            .cloned()
            .unwrap_or(Alignment::AlignDefault)
    } else {
        cell.align.clone()
    }
}

/// The column-class letter for an alignment (a default column is left-aligned).
fn column_letter(align: &Alignment) -> &'static str {
    match align {
        Alignment::AlignRight => "r",
        Alignment::AlignCenter => "c",
        _ => "l",
    }
}

/// The paragraph-shaping command for a sized column.
fn column_command(align: &Alignment) -> &'static str {
    match align {
        Alignment::AlignRight => "\\raggedleft",
        Alignment::AlignCenter => "\\centering",
        _ => "\\raggedright",
    }
}

/// The shaping prefix for a `\multirow` cell; left and default rows take none.
fn multirow_prefix(align: &Alignment) -> &'static str {
    match align {
        Alignment::AlignCenter => "\\centering\\arraybackslash ",
        Alignment::AlignRight => "\\raggedright\\arraybackslash ",
        _ => "",
    }
}

/// Render a table caption to a `\caption[short]{long}…\tabularnewline` line, reflowed at `width`.
/// Returns `None` for an empty caption. The closing brace, optional label, and `\tabularnewline`
/// stay glued to the final word so they reflow as one unit.
fn table_caption(caption: &Caption, attr: &Attr, width: usize) -> Option<String> {
    if caption.long.is_empty() {
        return None;
    }
    let short = caption
        .short
        .as_ref()
        .map(|inlines| format!("[{}]", flatten_pieces(&inline_pieces(inlines))))
        .unwrap_or_default();
    let mut pieces = vec![Piece::Text(format!("\\caption{short}{{"))];
    let mut first = true;
    for block in &caption.long {
        if let Block::Plain(inlines) | Block::Para(inlines) = block {
            if !first {
                pieces.push(Piece::Text("\\\\".to_owned()));
                pieces.push(Piece::Hard);
            }
            first = false;
            pieces.extend(inline_pieces(inlines));
        }
    }
    let mut close = String::from("}");
    if !attr.id.is_empty() {
        let _ = write!(close, "\\label{{{}}}", attr.id);
    }
    close.push_str("\\tabularnewline");
    pieces.push(Piece::Text(close));
    Some(fill(&pieces, width))
}

/// The leading `\def\labelenum…` an ordered list carries, or `None` when both numeral style and
/// delimiter are the renderer defaults (where the built-in label suffices).
fn label_definition(attrs: &ListAttributes, counter: &str) -> Option<String> {
    if matches!(attrs.style, ListNumberStyle::DefaultStyle)
        && matches!(attrs.delim, ListNumberDelim::DefaultDelim)
    {
        return None;
    }
    let numeral = numeral_command(&attrs.style, counter);
    let label = wrap_delim(&numeral, &attrs.delim);
    Some(format!("\\def\\label{counter}{{{label}}}"))
}

fn numeral_command(style: &ListNumberStyle, counter: &str) -> String {
    let command = match style {
        ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal | ListNumberStyle::Example => {
            "arabic"
        }
        ListNumberStyle::LowerAlpha => "alph",
        ListNumberStyle::UpperAlpha => "Alph",
        ListNumberStyle::LowerRoman => "roman",
        ListNumberStyle::UpperRoman => "Roman",
    };
    format!("\\{command}{{{counter}}}")
}

/// The LaTeX enumerate counter name for a nesting depth (`enumi`, `enumii`, …), capped at the four
/// levels LaTeX provides.
fn enum_counter(depth: usize) -> &'static str {
    match depth {
        0 | 1 => "enumi",
        2 => "enumii",
        3 => "enumiii",
        _ => "enumiv",
    }
}

fn is_tight_definitions(items: &[(Vec<Inline>, Vec<Vec<Block>>)]) -> bool {
    items
        .iter()
        .all(|(_, definitions)| list_is_tight(definitions))
}

/// Whether a heading needs a `\texorpdfstring` wrapper: it carries an inline that produces no plain
/// PDF-bookmark text on its own (anything beyond literal text and spaces).
fn needs_texorpdfstring(inlines: &[Inline]) -> bool {
    inlines
        .iter()
        .any(|inline| !matches!(inline, Inline::Str(_) | Inline::Space | Inline::SoftBreak))
}

fn inlines_to_string(inlines: &[Inline], width: usize) -> String {
    fill(&inline_pieces(inlines), width)
}

fn inline_pieces(inlines: &[Inline]) -> Vec<Piece> {
    let mut out = Vec::new();
    for inline in inlines {
        push_inline(inline, &mut out);
    }
    out
}

fn push_inline(inline: &Inline, out: &mut Vec<Piece>) {
    match inline {
        Inline::Str(text) => out.push(Piece::Text(escape(text, EscapeMode::Text))),
        Inline::Emph(inlines) => wrap_command("\\emph{", inlines, out),
        Inline::Strong(inlines) => wrap_command("\\textbf{", inlines, out),
        Inline::Underline(inlines) => wrap_command("\\ul{", inlines, out),
        Inline::Strikeout(inlines) => wrap_command("\\st{", inlines, out),
        Inline::Superscript(inlines) => wrap_command("\\textsuperscript{", inlines, out),
        Inline::Subscript(inlines) => wrap_command("\\textsubscript{", inlines, out),
        Inline::SmallCaps(inlines) => wrap_command("\\textsc{", inlines, out),
        Inline::Quoted(kind, inlines) => {
            let (open, close) = quote_marks(kind);
            out.push(Piece::Text(open.to_owned()));
            for inline in inlines {
                push_inline(inline, out);
            }
            out.push(Piece::Text(close.to_owned()));
        }
        Inline::Cite(_, inlines) => {
            for inline in inlines {
                push_inline(inline, out);
            }
        }
        Inline::Code(_, text) => {
            out.push(Piece::Text(format!(
                "\\texttt{{{}}}",
                escape(text, EscapeMode::Code)
            )));
        }
        Inline::Space | Inline::SoftBreak => out.push(Piece::Space),
        Inline::LineBreak => {
            out.push(Piece::Text("\\\\".to_owned()));
            out.push(Piece::Hard);
        }
        Inline::Math(kind, text) => {
            let rendered = match kind {
                MathType::InlineMath => format!("\\({text}\\)"),
                MathType::DisplayMath => format!("\\[{text}\\]"),
            };
            out.push(Piece::Text(rendered));
        }
        Inline::RawInline(format, text) => {
            if is_latex_format(&format.0) {
                out.push(Piece::Text(text.clone()));
            }
        }
        Inline::Link(attr, inlines, target) => push_link(attr, inlines, target, out),
        Inline::Image(attr, inlines, target) => out.push(Piece::Text(image(attr, inlines, target))),
        Inline::Span(attr, inlines) => {
            let mut open = if attr.id.is_empty() {
                String::new()
            } else {
                phantom_label(&attr.id)
            };
            open.push('{');
            out.push(Piece::Text(open));
            for inline in inlines {
                push_inline(inline, out);
            }
            out.push(Piece::Text("}".to_owned()));
        }
        Inline::Note(blocks) => out.push(Piece::Text(note(blocks))),
    }
}

fn wrap_command(open: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
    out.push(Piece::Text(open.to_owned()));
    for inline in inlines {
        push_inline(inline, out);
    }
    out.push(Piece::Text("}".to_owned()));
}

fn push_link(attr: &Attr, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
    if !attr.id.is_empty() {
        out.push(Piece::Text(phantom_label(&attr.id)));
    }
    let url = escape_url(&target.url);
    if let [Inline::Str(text)] = inlines
        && *text == target.url
    {
        out.push(Piece::Text(format!("\\url{{{url}}}")));
        return;
    }
    out.push(Piece::Text(format!("\\href{{{url}}}{{")));
    for inline in inlines {
        push_inline(inline, out);
    }
    out.push(Piece::Text("}".to_owned()));
}

fn image(attr: &Attr, inlines: &[Inline], target: &Target) -> String {
    let alt = to_plain_text(inlines);
    let alt_option = if alt.is_empty() {
        String::new()
    } else {
        format!(",alt={{{}}}", escape(&alt, EscapeMode::Text))
    };
    let url = escape_url(&target.url);

    let width = attribute_value(attr, "width").and_then(Dimension::parse);
    let height = attribute_value(attr, "height").and_then(Dimension::parse);
    if width.is_none() && height.is_none() {
        return format!(
            "\\pandocbounded{{\\includegraphics[keepaspectratio{alt_option}]{{{url}}}}}"
        );
    }

    let width_option = match &width {
        Some(dimension) => dimension.render("\\linewidth"),
        None => "\\linewidth".to_owned(),
    };
    let height_option = match &height {
        Some(dimension) => dimension.render("\\textheight"),
        None => "\\textheight".to_owned(),
    };
    let aspect = if width.is_some() && height.is_some() {
        ""
    } else {
        ",keepaspectratio"
    };
    format!(
        "\\includegraphics[width={width_option},height={height_option}{aspect}{alt_option}]{{{url}}}"
    )
}

/// A parsed image dimension. A pixel or bare number is expressed in inches at 96 pixels per inch; a
/// percentage is expressed as a fraction of a reference length; any other recognized unit is kept
/// verbatim.
enum Dimension {
    Length(String),
    Percent(f64),
}

impl Dimension {
    fn parse(value: &str) -> Option<Dimension> {
        let value = value.trim();
        let split = value
            .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
            .unwrap_or(value.len());
        let (number, unit) = value.split_at(split);
        let number: f64 = number.parse().ok()?;
        match unit.to_ascii_lowercase().as_str() {
            "" | "px" => Some(Dimension::Length(format!(
                "{}in",
                trim_number(number / 96.0)
            ))),
            "%" => Some(Dimension::Percent(number)),
            "in" | "cm" | "mm" | "pt" | "pc" | "em" => {
                Some(Dimension::Length(format!("{}{unit}", trim_number(number))))
            }
            _ => None,
        }
    }

    fn render(&self, reference: &str) -> String {
        match self {
            Dimension::Length(rendered) => rendered.clone(),
            Dimension::Percent(percent) => format!("{}{reference}", trim_number(percent / 100.0)),
        }
    }
}

/// Format a number to at most five fractional digits, dropping trailing zeros.
fn trim_number(value: f64) -> String {
    let formatted = format!("{value:.5}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

/// Render a footnote as an inline `\footnote{…}`. Its blocks hang two columns under the opening so
/// continuation paragraphs align with the first; a code block instead sits flush against the margin
/// in a `Verbatim` environment, since verbatim content cannot be indented, and pushes the closing
/// brace onto its own line.
fn note(blocks: &[Block]) -> String {
    let width = FILL_COLUMN.saturating_sub(2);
    let mut parts: Vec<String> = Vec::new();
    let mut ends_with_code = false;
    for block in blocks {
        let (rendered, is_code) = match block {
            Block::CodeBlock(attr, text) => (code_block_env(attr, text, "Verbatim"), true),
            _ => (block_to_string(block, width, 0), false),
        };
        if rendered.is_empty() {
            continue;
        }
        ends_with_code = is_code;
        let indented = if is_code {
            rendered
        } else if parts.is_empty() {
            indent_block(&rendered, "", "  ")
        } else {
            indent_block(&rendered, "  ", "  ")
        };
        parts.push(indented);
    }
    let body = parts.join("\n\n");
    let closing = if ends_with_code { "\n}" } else { "}" };
    format!("\\footnote{{{body}{closing}")
}

fn quote_marks(kind: &QuoteType) -> (&'static str, &'static str) {
    match kind {
        QuoteType::SingleQuote => ("`", "'"),
        QuoteType::DoubleQuote => ("``", "''"),
    }
}

fn is_latex_format(format: &str) -> bool {
    format.eq_ignore_ascii_case("latex") || format.eq_ignore_ascii_case("tex")
}

/// Escape a run of literal text for the given context.
fn escape(text: &str, mode: EscapeMode) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let next = chars.peek().copied();
        match ch {
            '&' | '%' | '#' | '_' | '$' | '{' | '}' => {
                out.push('\\');
                out.push(ch);
            }
            '^' => out.push_str("\\^{}"),
            '[' => out.push_str("{[}"),
            ']' => out.push_str("{]}"),
            '~' => push_control_word(&mut out, "\\textasciitilde", next, mode),
            '\\' => push_control_word(&mut out, "\\textbackslash", next, mode),
            '<' => push_control_word(&mut out, "\\textless", next, mode),
            '>' => push_control_word(&mut out, "\\textgreater", next, mode),
            '|' => push_control_word(&mut out, "\\textbar", next, mode),
            '\'' => push_control_word(&mut out, "\\textquotesingle", next, mode),
            '-' if next == Some('-') => out.push_str("-\\/"),
            ' ' if mode == EscapeMode::Code => out.push_str("\\ "),
            '`' if mode == EscapeMode::Code => out.push_str("\\textasciigrave{}"),
            '\u{a0}' if mode == EscapeMode::Text => out.push('~'),
            '\u{2026}' if mode == EscapeMode::Text => {
                push_control_word(&mut out, "\\ldots", next, mode);
            }
            '\u{2013}' if mode == EscapeMode::Text => out.push_str("--"),
            '\u{2014}' if mode == EscapeMode::Text => out.push_str("---"),
            '\u{2018}' if mode == EscapeMode::Text => out.push('`'),
            '\u{2019}' if mode == EscapeMode::Text => out.push('\''),
            '\u{201C}' if mode == EscapeMode::Text => out.push_str("``"),
            '\u{201D}' if mode == EscapeMode::Text => out.push_str("''"),
            other => out.push(other),
        }
    }
    out
}

/// Emit a control-word command and the separator that stops it from absorbing the following
/// character. In code context the command always closes with an empty group; in text context the
/// separator depends on what follows: a space before a letter, an empty group before whitespace or
/// the end of the run, and nothing before other glyphs (which already terminate the command).
fn push_control_word(out: &mut String, command: &str, next: Option<char>, mode: EscapeMode) {
    out.push_str(command);
    match mode {
        EscapeMode::Code => out.push_str("{}"),
        EscapeMode::Text => match next {
            Some(following) if following.is_alphabetic() => out.push(' '),
            Some(following) if following.is_whitespace() => out.push_str("{}"),
            None => out.push_str("{}"),
            Some(_) => {}
        },
    }
}

/// Escape a URL for `\href`/`\url`/`\includegraphics`: percent-encode the bytes LaTeX cannot carry
/// in a URL argument, map a backslash to a forward slash, and escape the surviving `#` and `%`.
fn escape_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        match ch {
            '\\' => out.push('/'),
            '#' => out.push_str("\\#"),
            '%' => out.push_str("\\%"),
            ' ' | '"' | '<' | '>' | '[' | ']' | '^' | '`' | '{' | '|' | '}' => {
                percent_encode(ch, &mut out);
            }
            other if !other.is_ascii() || (other as u32) < 0x20 => percent_encode(other, &mut out),
            other => out.push(other),
        }
    }
    out
}

fn percent_encode(ch: char, out: &mut String) {
    let mut buffer = [0u8; 4];
    for byte in ch.encode_utf8(&mut buffer).bytes() {
        out.push_str("\\%");
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0x0f));
    }
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'A' + value - 10) as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::Format;

    fn render(blocks: Vec<Block>) -> String {
        LatexWriter
            .write(
                &Document {
                    blocks,
                    ..Document::default()
                },
                &WriterOptions::default(),
            )
            .unwrap()
    }

    fn str_inlines(text: &str) -> Vec<Inline> {
        vec![Inline::Str(text.to_owned())]
    }

    #[test]
    fn dimension_parses_units() {
        assert!(matches!(Dimension::parse("96px"), Some(Dimension::Length(s)) if s == "1in"));
        assert!(matches!(Dimension::parse("96"), Some(Dimension::Length(s)) if s == "1in"));
        assert!(
            matches!(Dimension::parse("50%"), Some(Dimension::Percent(p)) if (p - 50.0).abs() < 1e-9)
        );
        assert!(matches!(Dimension::parse("2in"), Some(Dimension::Length(s)) if s == "2in"));
        assert!(matches!(Dimension::parse("3cm"), Some(Dimension::Length(s)) if s == "3cm"));
        // The unit's match key is case-folded, but the original spelling is preserved verbatim.
        assert!(matches!(Dimension::parse("3CM"), Some(Dimension::Length(s)) if s == "3CM"));
        for unit in ["mm", "pt", "pc", "em"] {
            assert!(matches!(
                Dimension::parse(&format!("4{unit}")),
                Some(Dimension::Length(_))
            ));
        }
        assert!(Dimension::parse("5xyz").is_none());
        assert!(Dimension::parse("notanumber").is_none());
    }

    #[test]
    fn dimension_renders_against_reference() {
        assert_eq!(Dimension::Length("2in".into()).render("\\linewidth"), "2in");
        assert_eq!(
            Dimension::Percent(50.0).render("\\linewidth"),
            "0.5\\linewidth"
        );
    }

    #[test]
    fn trim_number_drops_trailing_zeros() {
        assert_eq!(trim_number(1.0), "1");
        assert_eq!(trim_number(0.5), "0.5");
        assert_eq!(trim_number(1.230_00), "1.23");
    }

    #[test]
    fn escape_text_metacharacters_and_glyphs() {
        assert_eq!(
            escape("a&b%c#d_e$f{g}", EscapeMode::Text),
            "a\\&b\\%c\\#d\\_e\\$f\\{g\\}"
        );
        assert_eq!(escape("a^b", EscapeMode::Text), "a\\^{}b");
        assert_eq!(escape("[x]", EscapeMode::Text), "{[}x{]}");
        assert_eq!(escape("--", EscapeMode::Text), "-\\/-");
        assert_eq!(escape("\u{a0}", EscapeMode::Text), "~");
        assert_eq!(escape("\u{2026}", EscapeMode::Text), "\\ldots{}");
        assert_eq!(escape("\u{2013}\u{2014}", EscapeMode::Text), "-----");
        assert_eq!(escape("\u{2018}x\u{2019}", EscapeMode::Text), "`x'");
        assert_eq!(escape("\u{201C}x\u{201D}", EscapeMode::Text), "``x''");
    }

    #[test]
    fn escape_control_words_pick_separator() {
        assert_eq!(escape("~x", EscapeMode::Text), "\\textasciitilde x");
        assert_eq!(escape("~ ", EscapeMode::Text), "\\textasciitilde{} ");
        assert_eq!(escape("~!", EscapeMode::Text), "\\textasciitilde!");
        assert_eq!(escape("~", EscapeMode::Text), "\\textasciitilde{}");
        assert_eq!(
            escape("<>|\\", EscapeMode::Text),
            "\\textless\\textgreater\\textbar\\textbackslash{}"
        );
    }

    #[test]
    fn escape_code_mode_handles_space_and_backtick() {
        assert_eq!(escape("a b", EscapeMode::Code), "a\\ b");
        assert_eq!(escape("`", EscapeMode::Code), "\\textasciigrave{}");
        assert_eq!(escape("~", EscapeMode::Code), "\\textasciitilde{}");
    }

    #[test]
    fn escape_url_encodes_specials() {
        assert_eq!(escape_url("a\\b"), "a/b");
        assert_eq!(escape_url("a#b%c"), "a\\#b\\%c");
        assert_eq!(escape_url("a b"), "a\\%20b");
        assert_eq!(escape_url("café"), "caf\\%C3\\%A9");
    }

    #[test]
    fn header_levels_and_anchors() {
        assert_eq!(
            render(vec![Block::Header(1, Attr::default(), str_inlines("T"))]),
            "\\section{T}"
        );
        assert_eq!(
            render(vec![Block::Header(4, Attr::default(), str_inlines("T"))]),
            "\\paragraph{T}"
        );
        assert_eq!(
            render(vec![Block::Header(5, Attr::default(), str_inlines("T"))]),
            "\\subparagraph{T}"
        );
        // A level beyond the mapped range degrades to plain text.
        assert_eq!(
            render(vec![Block::Header(7, Attr::default(), str_inlines("T"))]),
            "T"
        );
    }

    #[test]
    fn header_unnumbered_adds_toc_line() {
        let attr = Attr {
            id: "sec".into(),
            classes: vec!["unnumbered".into()],
            ..Attr::default()
        };
        let out = render(vec![Block::Header(1, attr, str_inlines("Title"))]);
        assert!(out.contains("\\section*{Title}\\label{sec}"));
        assert!(out.contains("\\addcontentsline{toc}{section}{Title}"));
    }

    #[test]
    fn header_with_markup_wraps_texorpdfstring() {
        let inlines = vec![Inline::Emph(str_inlines("x"))];
        let out = render(vec![Block::Header(1, Attr::default(), inlines)]);
        assert!(out.contains("\\texorpdfstring{\\emph{x}}{x}"));
    }

    #[test]
    fn raw_block_kept_only_for_latex() {
        assert_eq!(
            render(vec![Block::RawBlock(
                Format("latex".into()),
                "\\foo\n".into()
            )]),
            "\\foo"
        );
        assert_eq!(
            render(vec![Block::RawBlock(Format("html".into()), "<b>".into())]),
            ""
        );
    }

    #[test]
    fn empty_item_and_term_render_bare() {
        let out = render(vec![Block::BulletList(vec![vec![]])]);
        assert!(out.contains("\\item"));
        let def = render(vec![Block::DefinitionList(vec![(
            str_inlines("term"),
            vec![],
        )])]);
        assert!(def.contains("\\item[term]"));
    }

    #[test]
    fn ordered_list_styles_set_label_and_counter() {
        let attrs = ListAttributes {
            start: 3,
            style: ListNumberStyle::UpperRoman,
            delim: ListNumberDelim::OneParen,
        };
        let out = render(vec![Block::OrderedList(
            attrs,
            vec![vec![Block::Plain(str_inlines("a"))]],
        )]);
        assert!(out.contains("\\def\\labelenumi{\\Roman{enumi})}"));
        assert!(out.contains("\\setcounter{enumi}{2}"));
    }

    #[test]
    fn nested_ordered_lists_use_deeper_counters() {
        let inner = Block::OrderedList(
            ListAttributes {
                start: 1,
                style: ListNumberStyle::LowerAlpha,
                delim: ListNumberDelim::Period,
            },
            vec![vec![Block::Plain(str_inlines("x"))]],
        );
        let out = render(vec![Block::OrderedList(
            ListAttributes {
                start: 1,
                style: ListNumberStyle::LowerAlpha,
                delim: ListNumberDelim::Period,
            },
            vec![vec![inner]],
        )]);
        assert!(out.contains("\\alph{enumi}"));
        assert!(out.contains("\\alph{enumii}"));
    }

    #[test]
    fn figure_with_id_and_caption() {
        let caption = Caption {
            short: None,
            long: vec![Block::Plain(str_inlines("Cap"))],
        };
        let attr = Attr {
            id: "fig".into(),
            ..Attr::default()
        };
        let out = render(vec![Block::Figure(
            attr,
            caption,
            vec![Block::Plain(str_inlines("body"))],
        )]);
        assert!(out.contains("\\label{fig}"));
        assert!(out.contains("\\caption{Cap}"));
    }

    #[test]
    fn span_with_id_emits_phantom_label() {
        let span = Inline::Span(
            Attr {
                id: "s".into(),
                ..Attr::default()
            },
            str_inlines("x"),
        );
        let out = render(vec![Block::Para(vec![span])]);
        assert!(out.contains("\\protect\\phantomsection\\label{s}{x}"));
    }

    #[test]
    fn image_with_dimensions_renders_options() {
        let attr = Attr {
            attributes: vec![
                ("width".into(), "50%".into()),
                ("height".into(), "2in".into()),
            ],
            ..Attr::default()
        };
        let image = Inline::Image(
            attr,
            str_inlines("alt"),
            Target {
                url: "img.png".into(),
                title: String::new(),
            },
        );
        let out = render(vec![Block::Para(vec![image])]);
        assert!(out.contains("width=0.5\\linewidth"));
        assert!(out.contains("height=2in"));
        assert!(out.contains("alt={alt}"));
    }

    #[test]
    fn footnote_with_code_block_closes_on_own_line() {
        let note = Inline::Note(vec![Block::CodeBlock(Attr::default(), "x\n".into())]);
        let out = render(vec![Block::Para(vec![Inline::Str("a".into()), note])]);
        assert!(out.contains("\\begin{Verbatim}"));
        assert!(out.contains("\n}"));
    }
}
