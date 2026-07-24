//! Table rendering to a `longtable` environment.

use std::fmt::Write as _;

use carta_ast::{Alignment, Attr, Block, Caption, Cell, ColWidth, Inline, Row, Table};
use carta_core::WrapMode;

use crate::common::{Piece, display_width, fill};
use crate::grid;

use super::{Dialect, Hl, inline_pieces, render_blocks, to_label};

/// Render a table as a `longtable` environment. A captionless table is wrapped so its float
/// counter is not advanced; a captioned one carries the caption and repeats its head for page
/// breaks. Spans become `\multicolumn`/`\multirow`; columns are letter classes unless explicit or
/// block-level cells call for sized `p{…}` columns with minipage cells.
pub(super) fn render_table(
    table: &Table,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    let plan = ColumnPlan::new(table);
    let head_rows: Vec<&Row> = table.head.rows.iter().collect();
    let body_rows: Vec<&Row> = table
        .bodies
        .iter()
        .flat_map(|body| body.head.iter().chain(body.body.iter()))
        .collect();
    let foot_rows: Vec<&Row> = table.foot.rows.iter().collect();

    let head_lines = render_section(&head_rows, &plan, true, width, dialect, wrap, smart, hl);
    let body_lines = render_section(&body_rows, &plan, false, width, dialect, wrap, smart, hl);
    let foot_lines = render_section(&foot_rows, &plan, false, width, dialect, wrap, smart, hl);
    let caption = table_caption(&table.caption, &table.attr, width, dialect, wrap, smart, hl);

    let mut parts = vec![format!("\\begin{{longtable}}[]{{{}}}", plan.colspec())];
    if let Some(caption) = &caption {
        parts.push(caption.clone());
        parts.push(head_block(&head_lines, "\\endfirsthead"));
        parts.push(head_block(&head_lines, "\\endhead"));
    } else {
        parts.push(head_block(&head_lines, "\\endhead"));
    }
    match dialect {
        Dialect::Article => {
            if !foot_lines.is_empty() {
                parts.push("\\midrule\\noalign{}".to_owned());
                parts.extend(foot_lines);
            }
            parts.push("\\bottomrule\\noalign{}".to_owned());
            parts.push("\\endlastfoot".to_owned());
            parts.extend(body_lines);
        }
        Dialect::Slide { .. } => {
            parts.extend(body_lines);
            if !foot_lines.is_empty() {
                parts.push("\\midrule\\noalign{}".to_owned());
                parts.extend(foot_lines);
            }
            parts.push("\\bottomrule\\noalign{}".to_owned());
        }
    }
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
        let block_cells = all_rows(table).any(|row| {
            row.cells
                .iter()
                .any(|cell| !is_simple_cell(cell) || simple_cell_has_break(cell))
        });
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
            // A table's column count is tiny; the reciprocal loses no meaningful precision.
            #[allow(clippy::cast_precision_loss)]
            let equal = 1.0 / columns as f64;
            vec![equal; columns]
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

/// Shared layout context for a table section's rows: the column plan, whether these are header
/// rows, the reflow width, the output dialect, and the text-layout mode for cell prose.
struct TableContext<'a> {
    plan: &'a ColumnPlan,
    head: bool,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'a>,
}

/// Render a section's rows to `longtable` lines, resolving spans against the section's own grid.
#[allow(clippy::too_many_arguments)]
fn render_section<'a>(
    rows: &[&Row],
    plan: &'a ColumnPlan,
    head: bool,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'a>,
) -> Vec<String> {
    let context = TableContext {
        plan,
        head,
        width,
        dialect,
        wrap,
        smart,
        hl,
    };
    let placements = grid::place_columns(rows, plan.columns);
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let row_placements = placements.get(index).map_or(&[][..], Vec::as_slice);
            render_row(row, row_placements, &context)
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
/// cells. Fields are separated by ` & ` and the row ends with ` \\`. A column covered by a span
/// from an earlier or wider cell that still precedes a later cell contributes an empty field;
/// columns trailing the row's last cell (covered by a multi-row cell stacked above) are dropped,
/// ending the row early.
fn render_row(row: &Row, placements: &[(usize, usize)], context: &TableContext) -> String {
    let mut tokens: Vec<Token> = Vec::new();
    let mut cells = row.cells.iter().zip(placements.iter());
    let mut next = cells.next();
    let mut column = 0usize;
    let mut first = true;
    let mut trailing_cell_space = false;
    let last_column = placements
        .iter()
        .map(|&(start, span)| start + span.max(1))
        .max()
        .unwrap_or(0);
    while column < last_column {
        if !first {
            tokens.push(Token::Space);
            tokens.push(Token::Word("&".to_owned()));
            tokens.push(Token::Space);
        }
        first = false;
        match next {
            Some((cell, &(start, span))) if start == column => {
                let before = tokens.len();
                render_field(&mut tokens, cell, start, span, context);
                trailing_cell_space =
                    tokens.len() > before && matches!(tokens.last(), Some(Token::Space));
                column += span.max(1);
                next = cells.next();
            }
            _ => {
                trailing_cell_space = false;
                column += 1;
            }
        }
    }
    while matches!(tokens.last(), Some(Token::Space)) {
        tokens.pop();
    }
    // The final cell's trailing space is visible; a bare separator from an empty final cell is not.
    if trailing_cell_space {
        tokens.push(Token::Space);
    }
    glue_suffix(&mut tokens, " \\\\");
    layout_row(&tokens, context.width, context.wrap)
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
/// a word that would overflow, and honor forced breaks verbatim. Soft reflow applies only under
/// [`WrapMode::Auto`]; otherwise the row stays on one logical line and only its forced breaks split
/// it.
fn layout_row(tokens: &[Token], width: usize, wrap: WrapMode) -> String {
    let width = match wrap {
        WrapMode::Auto => width,
        WrapMode::None | WrapMode::Preserve => usize::MAX,
    };
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
    context: &TableContext,
) {
    let inner = render_cell(cell, start, context);
    let mut field: Vec<Token> = Vec::new();
    push_field_tokens(&mut field, &inner);

    let row_span = cell.row_span.max(1);
    if row_span > 1 {
        let prefix = multirow_prefix(&resolved_align(cell, start, context.plan));
        // `=` sizes the stacked cell to an explicit column width; `*` takes the content's own width.
        let sizing = if context.plan.explicit { "=" } else { "*" };
        glue_prefix(
            &mut field,
            &format!("\\multirow{{{row_span}}}{{{sizing}}}{{{prefix}"),
        );
        glue_suffix(&mut field, "}");
    }
    if span > 1 {
        let spec = multicolumn_spec(cell, start, span, context.plan);
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
fn render_cell(cell: &Cell, start: usize, context: &TableContext) -> String {
    let stacked_lines = context.plan.sized && !context.plan.explicit;
    let text = cell_content(
        cell,
        stacked_lines,
        context.width,
        context.dialect,
        context.wrap,
        context.smart,
        context.hl,
    );
    if context.plan.explicit && (context.head || !is_simple_cell(cell)) {
        minipage(
            context.head,
            column_command(&resolved_align(cell, start, context.plan)),
            &text,
        )
    } else {
        text
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
fn cell_content(
    cell: &Cell,
    stacked_lines: bool,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    if is_simple_cell(cell) {
        match cell.content.first() {
            Some(Block::Plain(inlines) | Block::Para(inlines)) => {
                if stacked_lines
                    && inlines
                        .iter()
                        .any(|inline| matches!(inline, Inline::LineBreak))
                {
                    stacked_cell(inlines, width, dialect, wrap, smart, hl)
                } else {
                    let text =
                        flatten_pieces(&inline_pieces(inlines, width, dialect, wrap, smart, hl));
                    text.trim_start_matches(' ').to_owned()
                }
            }
            _ => String::new(),
        }
    } else {
        render_blocks(&cell.content, width, 0, dialect, wrap, smart, hl)
    }
}

/// Render a single paragraph that carries hard line breaks as a stack of struts: one
/// `\hbox{\strut …}` per break-delimited segment, wrapped in a `\vtop` so the cell grows downward.
fn stacked_cell(
    inlines: &[Inline],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    let mut boxes = String::new();
    for segment in inlines.split(|inline| matches!(inline, Inline::LineBreak)) {
        let text = flatten_pieces(&inline_pieces(segment, width, dialect, wrap, smart, hl));
        boxes.push_str("\\hbox{\\strut ");
        boxes.push_str(&text);
        boxes.push('}');
    }
    format!("\\vtop{{{boxes}}}")
}

/// Whether a cell holds at most a single paragraph, so it can render without a minipage box.
fn is_simple_cell(cell: &Cell) -> bool {
    match cell.content.as_slice() {
        [] => true,
        [block] => matches!(block, Block::Plain(_) | Block::Para(_)),
        _ => false,
    }
}

/// Whether a cell is a single paragraph split by a hard line break; such a cell forces the sized
/// table form so its lines can stack.
fn simple_cell_has_break(cell: &Cell) -> bool {
    matches!(
        cell.content.as_slice(),
        [Block::Plain(inlines) | Block::Para(inlines)]
            if inlines.iter().any(|inline| matches!(inline, Inline::LineBreak))
    )
}

/// Flatten layout pieces to a string, keeping hard breaks as newlines.
pub(super) fn flatten_pieces(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Space | Piece::Soft => out.push(' '),
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
#[allow(clippy::too_many_arguments)]
fn table_caption(
    caption: &Caption,
    attr: &Attr,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> Option<String> {
    if caption.long.is_empty() {
        return None;
    }
    let short = caption
        .short
        .as_ref()
        .map(|inlines| {
            format!(
                "[{}]",
                flatten_pieces(&inline_pieces(inlines, width, dialect, wrap, smart, hl))
            )
        })
        .unwrap_or_default();
    let mut pieces = vec![Piece::text(format!("\\caption{short}{{"))];
    let mut first = true;
    for block in &caption.long {
        if let Block::Plain(inlines) | Block::Para(inlines) = block {
            if !first {
                pieces.push(Piece::text("\\\\"));
                pieces.push(Piece::Hard);
            }
            first = false;
            pieces.extend(inline_pieces(inlines, width, dialect, wrap, smart, hl));
        }
    }
    let mut close = String::from("}");
    if !attr.id.is_empty() {
        let _ = write!(close, "\\label{{{}}}", to_label(&attr.id));
    }
    close.push_str("\\tabularnewline");
    pieces.push(Piece::text(close));
    Some(fill(&pieces, width, wrap))
}
