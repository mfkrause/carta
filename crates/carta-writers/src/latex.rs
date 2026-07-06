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
use carta_core::{Extension, MetaVarStyle, Result, TocStyle, WrapMode, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, Piece, attribute_value, clean_prefix_len, display_width, fill, indent_block,
    label_matches_url, list_is_tight, numeral, wrap_delim,
};
use crate::grid;

/// Renders a document to a LaTeX fragment.
#[derive(Debug, Default, Clone, Copy)]
pub struct LatexWriter;

impl Writer for LatexWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let smart = options.extensions.contains(Extension::Smart);
        let body = render_blocks(
            &document.blocks,
            options.columns.unwrap_or(FILL_COLUMN),
            0,
            Dialect::Article,
            options.wrap,
            smart,
        );
        Ok(body.trim_end_matches('\n').to_owned())
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.latex"))
    }

    fn meta_var_style(&self) -> MetaVarStyle {
        MetaVarStyle::Pdf
    }

    fn toc_style(&self) -> TocStyle {
        TocStyle::Native
    }

    fn numbers_sections_natively(&self) -> bool {
        true
    }
}

/// The LaTeX variant a block sequence is rendered for. The slide variant changes three constructs:
/// a footnote is anchored for incremental overlays, an ordered list states its label as an
/// `enumerate` template, and a long table places its closing rule after the body rather than in a
/// repeating footer. Its `incremental` flag, set inside a presentation's incremental region, gives
/// every list an `[<+->]` overlay so its items reveal one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    Article,
    Slide { incremental: bool },
}

impl Dialect {
    /// The slide dialect with item overlays disabled, the default for frame content.
    pub(crate) const SLIDE: Dialect = Dialect::Slide { incremental: false };

    fn is_slide(self) -> bool {
        matches!(self, Dialect::Slide { .. })
    }

    /// The same dialect with its incremental overlay set to `incremental`; a no-op for the article
    /// dialect, which has no overlays.
    pub(crate) fn with_incremental(self, incremental: bool) -> Dialect {
        match self {
            Dialect::Article => Dialect::Article,
            Dialect::Slide { .. } => Dialect::Slide { incremental },
        }
    }
}

/// Render a block sequence in a given dialect, returning the body without a trailing newline. Slide
/// writers render the content of each frame through this entry point, laid out under `wrap`.
pub(crate) fn render_fragment(
    blocks: &[Block],
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    render_blocks(blocks, FILL_COLUMN, 0, dialect, wrap, smart)
        .trim_end_matches('\n')
        .to_owned()
}

/// Render a header as a sectioning command, the form used for headers above a presentation's slide
/// level. Identical to a top-level header in the article dialect.
pub(crate) fn render_heading(
    level: i32,
    attr: &Attr,
    inlines: &[Inline],
    wrap: WrapMode,
    smart: bool,
) -> String {
    header(
        level,
        attr,
        inlines,
        FILL_COLUMN,
        Dialect::Article,
        wrap,
        smart,
    )
}

/// Render a titled environment opening: a `prefix` literal (such as `\begin{frame}{`), the title
/// inlines, and a closing `}`, all reflowed as one unit so the title wraps at the fill column with
/// the prefix counted against the first line. The closing brace glues to the final word.
pub(crate) fn render_titled_open(
    prefix: &str,
    inlines: &[Inline],
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let mut pieces = vec![Piece::text(prefix.to_owned())];
    pieces.extend(inline_pieces(inlines, FILL_COLUMN, dialect, wrap, smart));
    pieces.push(Piece::text("}"));
    fill(&pieces, FILL_COLUMN, wrap)
}

/// The anchor markup for an element carrying an identifier, exposed for slide-level scaffolding.
pub(crate) fn anchor(id: &str) -> String {
    phantom_label(id)
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
fn render_blocks(
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    blocks
        .iter()
        .map(|block| block_to_string(block, width, enum_depth, dialect, wrap, smart))
        .filter(|rendered| !rendered.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn block_to_string(
    block: &Block,
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => {
            inlines_to_string(inlines, width, dialect, wrap, smart)
        }
        Block::Header(level, attr, inlines) => {
            header(*level, attr, inlines, width, dialect, wrap, smart)
        }
        Block::CodeBlock(attr, text) => code_block(attr, text),
        Block::RawBlock(format, text) => {
            if is_latex_format(&format.0) {
                text.strip_suffix('\n').unwrap_or(text).to_owned()
            } else {
                String::new()
            }
        }
        Block::BlockQuote(blocks) => block_quote(blocks, width, enum_depth, dialect, wrap, smart),
        Block::BulletList(items) => bullet_list(items, width, enum_depth, dialect, wrap, smart),
        Block::OrderedList(attrs, items) => {
            ordered_list(attrs, items, width, enum_depth, dialect, wrap, smart)
        }
        Block::DefinitionList(items) => {
            definition_list(items, width, enum_depth, dialect, wrap, smart)
        }
        Block::HorizontalRule => {
            "\\begin{center}\\rule{0.5\\linewidth}{0.5pt}\\end{center}".to_owned()
        }
        Block::LineBlock(lines) => line_block(lines, width, dialect, wrap, smart),
        Block::Div(attr, blocks) => div(attr, blocks, width, enum_depth, dialect, wrap, smart),
        Block::Figure(attr, caption, blocks) => figure(
            attr, caption, blocks, width, enum_depth, dialect, wrap, smart,
        ),
        Block::Table(table) => render_table(table, width, dialect, wrap, smart),
    }
}

fn header(
    level: i32,
    attr: &Attr,
    inlines: &[Inline],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let command = match level {
        1 => "section",
        2 => "subsection",
        3 => "subsubsection",
        4 => "paragraph",
        5 => "subparagraph",
        _ => return inlines_to_string(inlines, width, dialect, wrap, smart),
    };
    let unnumbered = attr.classes.iter().any(|class| class == "unnumbered");
    let star = if unnumbered { "*" } else { "" };
    let inner = inline_pieces_in(inlines, width, dialect, wrap, smart, true);

    let mut content = vec![Piece::text(format!("\\{command}{star}"))];
    if let Some(short) = short_title(inlines, width, dialect, wrap, smart) {
        content.push(Piece::text(format!("[{short}]")));
    }
    content.push(Piece::text("{"));
    if needs_texorpdfstring(inlines) {
        content.push(Piece::text("\\texorpdfstring{"));
        content.extend(inner.iter().cloned());
        let pdf = escape_smart(&to_plain_text(inlines), EscapeMode::Text, smart);
        content.push(Piece::text(format!("}}{{{pdf}}}")));
    } else {
        content.extend(inner.iter().cloned());
    }
    content.push(Piece::text("}"));
    if !attr.id.is_empty() {
        content.push(Piece::text(format!("\\label{{{}}}", to_label(&attr.id))));
    }
    let heading = fill(&content, width, wrap);

    if unnumbered {
        let mut toc = vec![Piece::text(format!(
            "\\addcontentsline{{toc}}{{{command}}}{{"
        ))];
        toc.extend(inner);
        toc.push(Piece::text("}"));
        format!("{heading}\n{}", fill(&toc, width, wrap))
    } else {
        heading
    }
}

/// Render a `Div`. A plain div emits its body under an optional anchor; in the slide dialect a few
/// recognized classes map to presentation constructs: a column layout (`columns` with `column`
/// children), and incremental regions (`incremental` / `nonincremental`) that toggle whether the
/// lists inside reveal their items one at a time.
fn div(
    attr: &Attr,
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    if dialect.is_slide() {
        if has_class(attr, "columns") {
            return columns(attr, blocks, width, enum_depth, dialect, wrap, smart);
        }
        if has_class(attr, "incremental") {
            return render_blocks(
                blocks,
                width,
                enum_depth,
                dialect.with_incremental(true),
                wrap,
                smart,
            );
        }
        if has_class(attr, "nonincremental") {
            return render_blocks(
                blocks,
                width,
                enum_depth,
                dialect.with_incremental(false),
                wrap,
                smart,
            );
        }
    }
    let body = render_blocks(blocks, width, enum_depth, dialect, wrap, smart);
    if attr.id.is_empty() {
        body
    } else {
        format!("{}\n{body}", phantom_label(&attr.id))
    }
}

/// Render a `columns` div as a `columns` environment whose `column` children become sized `column`
/// boxes. The environment's vertical alignment comes from the div's `align` attribute (`top`→`T`,
/// `bottom`→`b`, `center`→`c`, defaulting to `T`); a `totalwidth` attribute is carried through.
fn columns(
    attr: &Attr,
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let align = match attribute_value(attr, "align") {
        Some("bottom") => "b",
        Some("center") => "c",
        _ => "T",
    };
    let total = attribute_value(attr, "totalwidth")
        .map(|value| format!(",totalwidth={value}"))
        .unwrap_or_default();
    let mut lines = Vec::new();
    if !attr.id.is_empty() {
        lines.push(phantom_label(&attr.id));
    }
    lines.push(format!("\\begin{{columns}}[{align}{total}]"));
    let boxes: Vec<String> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Div(column_attr, column_blocks) if has_class(column_attr, "column") => {
                Some(column(
                    column_attr,
                    column_blocks,
                    width,
                    enum_depth,
                    dialect,
                    wrap,
                    smart,
                ))
            }
            _ => None,
        })
        .collect();
    lines.push(boxes.join("\n\n"));
    lines.push("\\end{columns}".to_owned());
    lines.join("\n")
}

/// Render a single `column` div as a sized `column` box. Its fraction comes from a `width=NN%`
/// attribute, defaulting to `0.48` of the line width.
fn column(
    attr: &Attr,
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let fraction = attribute_value(attr, "width")
        .and_then(parse_percent)
        .unwrap_or(0.48);
    let mut lines = Vec::new();
    if !attr.id.is_empty() {
        lines.push(phantom_label(&attr.id));
    }
    lines.push(format!(
        "\\begin{{column}}{{{}\\linewidth}}",
        trim_number(fraction)
    ));
    let body = render_blocks(blocks, width, enum_depth, dialect, wrap, smart);
    if !body.is_empty() {
        lines.push(body);
    }
    lines.push("\\end{column}".to_owned());
    lines.join("\n")
}

/// Parse a percentage attribute (`50%`) to its fraction (`0.5`); `None` when it is not a percentage.
fn parse_percent(value: &str) -> Option<f64> {
    let number = value.strip_suffix('%')?;
    number
        .trim()
        .parse::<f64>()
        .ok()
        .map(|percent| percent / 100.0)
}

fn has_class(attr: &Attr, class: &str) -> bool {
    attr.classes.iter().any(|candidate| candidate == class)
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

/// The anchor markup emitted for an element carrying an identifier outside a movable argument.
fn phantom_label(id: &str) -> String {
    format!("\\protect\\phantomsection\\label{{{}}}", to_label(id))
}

/// The anchor markup for an identifier inside a heading. A `\label` placed in a section's movable
/// argument can resolve to the wrong location, so an empty `\hypertarget` names the spot instead.
fn header_anchor(id: &str) -> String {
    format!("\\protect\\hypertarget{{{}}}{{}}", to_label(id))
}

/// Rewrite an identifier into a single token safe as a `\label`/`\hypertarget` name: ASCII
/// alphanumerics and `_-+=:;.` are kept, every other character becomes `ux` followed by its
/// lowercase hexadecimal code point.
fn to_label(id: &str) -> String {
    let mut label = String::with_capacity(id.len());
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+' | '=' | ':' | ';' | '.') {
            label.push(ch);
        } else {
            label.push_str("ux");
            let _ = write!(label, "{:x}", ch as u32);
        }
    }
    label
}

/// The overlay specification appended to a list environment: `[<+->]` inside an incremental slide
/// region so items reveal one at a time, empty otherwise.
fn overlay(dialect: Dialect) -> &'static str {
    if matches!(dialect, Dialect::Slide { incremental: true }) {
        "[<+->]"
    } else {
        ""
    }
}

/// Render a block quote. In the slide dialect a quote whose sole content is a single list is the
/// idiom for toggling that list's incremental overlay: the quote wrapper is dropped and the list
/// renders with the surrounding incremental state flipped. Any other quote keeps its `quote`
/// environment, with the incremental overlay suppressed inside it.
fn block_quote(
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    if let (Dialect::Slide { incremental }, [list]) = (dialect, blocks)
        && matches!(
            list,
            Block::BulletList(_) | Block::OrderedList(_, _) | Block::DefinitionList(_)
        )
    {
        return block_to_string(
            list,
            width,
            enum_depth,
            dialect.with_incremental(!incremental),
            wrap,
            smart,
        );
    }
    format!(
        "\\begin{{quote}}\n{}\n\\end{{quote}}",
        render_blocks(
            blocks,
            width,
            enum_depth,
            dialect.with_incremental(false),
            wrap,
            smart
        )
    )
}

fn bullet_list(
    items: &[Vec<Block>],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let mut lines = vec![format!("\\begin{{itemize}}{}", overlay(dialect))];
    if list_is_tight(items) {
        lines.push("\\tightlist".to_owned());
    }
    for item in items {
        lines.push(list_item(item, width, enum_depth, dialect, wrap, smart));
    }
    lines.push("\\end{itemize}".to_owned());
    lines.join("\n")
}

fn ordered_list(
    attrs: &ListAttributes,
    items: &[Vec<Block>],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let depth = enum_depth + 1;
    let counter = enum_counter(depth);
    let mut lines = vec![format!("\\begin{{enumerate}}{}", overlay(dialect))];
    match dialect {
        Dialect::Article => {
            if let Some(label) = label_definition(attrs, counter) {
                lines.push(label);
            }
        }
        Dialect::Slide { .. } => {
            if let Some(template) = label_template(attrs) {
                lines.push(template);
            }
        }
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
        lines.push(list_item(item, width, depth, dialect, wrap, smart));
    }
    lines.push("\\end{enumerate}".to_owned());
    lines.join("\n")
}

/// Render one list item: its blocks indented two columns under an `\item` line.
fn list_item(
    item: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let body = render_blocks(
        item,
        width.saturating_sub(2),
        enum_depth,
        dialect,
        wrap,
        smart,
    );
    if body.is_empty() {
        "\\item".to_owned()
    } else {
        format!("\\item\n{}", indent_block(&body, "  ", "  "))
    }
}

fn definition_list(
    items: &[(Vec<Inline>, Vec<Vec<Block>>)],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let mut lines = vec![format!("\\begin{{description}}{}", overlay(dialect))];
    if is_tight_definitions(items) {
        lines.push("\\tightlist".to_owned());
    }
    for (term, definitions) in items {
        let header = format!(
            "\\item[{}]",
            inlines_to_string(term, width, dialect, wrap, smart)
        );
        let bodies: Vec<String> = definitions
            .iter()
            .map(|definition| render_blocks(definition, width, enum_depth, dialect, wrap, smart))
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

/// Render a line block, breaking each line with `\\`. An empty line needs a placeholder so the break
/// has content to act on: while only empty lines have been seen it is `\hfill\break` (which both fills
/// the line and breaks it, so no `\\` follows); once real content has appeared it is `\strut`, which
/// gives the otherwise-blank line its height ahead of the `\\`. A trailing empty line contributes
/// nothing beyond the break already closing the previous line.
fn line_block(
    lines: &[Vec<Inline>],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let mut out = String::new();
    let mut only_empty_so_far = true;
    for (index, line) in lines.iter().enumerate() {
        let is_last = index + 1 == lines.len();
        let mut breaks = true;
        if !line.is_empty() {
            out.push_str(&inlines_to_string(line, width, dialect, wrap, smart));
            only_empty_so_far = false;
        } else if is_last {
            // The break ending the previous line already stands in for this one.
        } else if only_empty_so_far {
            out.push_str("\\hfill\\break");
            breaks = false;
        } else {
            out.push_str("\\strut ");
        }
        if !is_last {
            out.push_str(if breaks { "\\\\\n" } else { "\n" });
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn figure(
    attr: &Attr,
    caption: &Caption,
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let mut parts = vec![
        "\\begin{figure}".to_owned(),
        "\\centering".to_owned(),
        render_blocks(blocks, width, enum_depth, dialect, wrap, smart),
    ];
    let caption_body = caption_body(caption, width, dialect, wrap, smart);
    if !caption_body.is_empty() || !attr.id.is_empty() {
        let label = if attr.id.is_empty() {
            String::new()
        } else {
            format!("\\label{{{}}}", to_label(&attr.id))
        };
        parts.push(format!("\\caption{{{caption_body}}}{label}"));
    }
    parts.push("\\end{figure}".to_owned());
    parts.join("\n")
}

/// Render a caption's block body to the content of a `\caption{…}`. Each leaf block becomes its own
/// line, separated by `\\` breaks, so a multi-paragraph legend keeps its paragraph boundaries.
fn caption_body(
    caption: &Caption,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    caption
        .long
        .iter()
        .filter_map(|block| match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                Some(inlines_to_string(inlines, width, dialect, wrap, smart))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\\\\\n")
}

/// Render a table as a `longtable` environment. A captionless table is wrapped so its float
/// counter is not advanced; a captioned one carries the caption and repeats its head for page
/// breaks. Spans become `\multicolumn`/`\multirow`; columns are letter classes unless explicit or
/// block-level cells call for sized `p{…}` columns with minipage cells.
fn render_table(
    table: &Table,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let plan = ColumnPlan::new(table);
    let head_rows: Vec<&Row> = table.head.rows.iter().collect();
    let body_rows: Vec<&Row> = table
        .bodies
        .iter()
        .flat_map(|body| body.head.iter().chain(body.body.iter()))
        .collect();
    let foot_rows: Vec<&Row> = table.foot.rows.iter().collect();

    let head_lines = render_section(&head_rows, &plan, true, width, dialect, wrap, smart);
    let body_lines = render_section(&body_rows, &plan, false, width, dialect, wrap, smart);
    let foot_lines = render_section(&foot_rows, &plan, false, width, dialect, wrap, smart);
    let caption = table_caption(&table.caption, &table.attr, width, dialect, wrap, smart);

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
}

/// Render a section's rows to `longtable` lines, resolving spans against the section's own grid.
fn render_section(
    rows: &[&Row],
    plan: &ColumnPlan,
    head: bool,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> Vec<String> {
    let context = TableContext {
        plan,
        head,
        width,
        dialect,
        wrap,
        smart,
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
/// columns trailing the row's last cell — covered by a multi-row cell stacked above — are dropped,
/// ending the row early.
fn render_row(row: &Row, placements: &[(usize, usize)], context: &TableContext) -> String {
    let mut tokens: Vec<Token> = Vec::new();
    let mut cells = row.cells.iter().zip(placements.iter());
    let mut next = cells.next();
    let mut column = 0usize;
    let mut first = true;
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
                render_field(&mut tokens, cell, start, span, context);
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
        // Explicitly sized columns size the stacked cell to the column width (`=`); columns left at
        // their natural width take the content's own width (`*`).
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
    let text = cell_content(
        cell,
        context.width,
        context.dialect,
        context.wrap,
        context.smart,
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
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    if is_simple_cell(cell) {
        match cell.content.first() {
            Some(Block::Plain(inlines) | Block::Para(inlines)) => {
                flatten_pieces(&inline_pieces(inlines, width, dialect, wrap, smart))
            }
            _ => String::new(),
        }
    } else {
        render_blocks(&cell.content, width, 0, dialect, wrap, smart)
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
fn table_caption(
    caption: &Caption,
    attr: &Attr,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
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
                flatten_pieces(&inline_pieces(inlines, width, dialect, wrap, smart))
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
            pieces.extend(inline_pieces(inlines, width, dialect, wrap, smart));
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

/// The leading `\def\labelenum…` an ordered list carries, or `None` when both numeral style and
/// delimiter are the renderer defaults (where the built-in label suffices).
fn label_definition(attrs: &ListAttributes, counter: &str) -> Option<String> {
    if matches!(attrs.style, ListNumberStyle::DefaultStyle)
        && matches!(attrs.delim, ListNumberDelim::DefaultDelim)
    {
        return None;
    }
    let numeral = numeral_command(attrs.style, counter);
    let label = wrap_delim(&numeral, attrs.delim);
    Some(format!("\\def\\label{counter}{{{label}}}"))
}

/// The `[…]` template line stating an ordered list's label inside a slide `enumerate`, or `None`
/// for the two spellings of the plain default — `(DefaultStyle, DefaultDelim)` and
/// `(Decimal, Period)` — where the environment's built-in `1.` label already applies. The template
/// renders the first numeral in the list's style, wrapped in its delimiter.
fn label_template(attrs: &ListAttributes) -> Option<String> {
    let is_default = matches!(attrs.style, ListNumberStyle::DefaultStyle)
        && matches!(attrs.delim, ListNumberDelim::DefaultDelim);
    let is_plain_decimal = matches!(attrs.style, ListNumberStyle::Decimal)
        && matches!(attrs.delim, ListNumberDelim::Period);
    if is_default || is_plain_decimal {
        return None;
    }
    let first = numeral(1, attrs.style);
    Some(format!("[{}]", wrap_delim(&first, attrs.delim)))
}

fn numeral_command(style: ListNumberStyle, counter: &str) -> String {
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

/// The optional running-head argument for a heading. When the heading carries an inline that cannot
/// survive a section's movable argument — a footnote, an image, or an element with an identifier
/// anchor — those inlines are dropped and the remainder rendered as a short title. `None` when the
/// heading carries no such inline and so needs no short title.
fn short_title(
    inlines: &[Inline],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> Option<String> {
    let visible: Vec<Inline> = inlines
        .iter()
        .filter(|inline| !contains_fragile(inline))
        .cloned()
        .collect();
    if visible.len() == inlines.len() || visible.is_empty() {
        return None;
    }
    Some(flatten_pieces(&inline_pieces(
        &visible, width, dialect, wrap, smart,
    )))
}

/// Whether an inline expands to a construct invalid in a heading's movable short-title argument: a
/// footnote, an image, or an element (itself or a descendant) carrying an identifier anchor.
fn contains_fragile(inline: &Inline) -> bool {
    match inline {
        Inline::Note(_) | Inline::Image(..) => true,
        Inline::Span(attr, inlines) => !attr.id.is_empty() || inlines.iter().any(contains_fragile),
        Inline::Emph(inlines)
        | Inline::Strong(inlines)
        | Inline::Underline(inlines)
        | Inline::Strikeout(inlines)
        | Inline::Superscript(inlines)
        | Inline::Subscript(inlines)
        | Inline::SmallCaps(inlines)
        | Inline::Quoted(_, inlines)
        | Inline::Cite(_, inlines)
        | Inline::Link(_, inlines, _) => inlines.iter().any(contains_fragile),
        _ => false,
    }
}

fn inlines_to_string(
    inlines: &[Inline],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    fill(
        &inline_pieces(inlines, width, dialect, wrap, smart),
        width,
        wrap,
    )
}

fn inline_pieces(
    inlines: &[Inline],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> Vec<Piece> {
    inline_pieces_in(inlines, width, dialect, wrap, smart, false)
}

/// Render an inline list to pieces. `in_header` is set while rendering a heading's content, where an
/// identifier anchor is emitted as a `\hypertarget` rather than a `\phantomsection\label`.
fn inline_pieces_in(
    inlines: &[Inline],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
) -> Vec<Piece> {
    let mut out = Vec::new();
    push_inlines(inlines, &mut out, width, dialect, wrap, smart, in_header);
    out
}

/// Render an inline list. After a quote span, a thin space separates its closing delimiter from a
/// following quotation mark so the two marks do not run together into one glyph.
fn push_inlines(
    inlines: &[Inline],
    out: &mut Vec<Piece>,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
) {
    let mut remaining = inlines.iter().peekable();
    while let Some(inline) = remaining.next() {
        push_inline(inline, out, width, dialect, wrap, smart, in_header);
        if matches!(inline, Inline::Quoted(..))
            && let Some(Inline::Str(text)) = remaining.peek()
            && text.chars().next().is_some_and(is_quotation_mark)
        {
            out.push(Piece::text("\\,"));
        }
    }
}

/// Whether a character is a quotation mark that would visually merge with a preceding quote span's
/// closing delimiter. The grave accent is not a quotation mark and is excluded.
fn is_quotation_mark(ch: char) -> bool {
    matches!(ch, '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}' | '\'')
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn push_inline(
    inline: &Inline,
    out: &mut Vec<Piece>,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
) {
    match inline {
        Inline::Str(text) => out.push(Piece::text(escape_smart(text, EscapeMode::Text, smart))),
        Inline::Emph(inlines) => {
            wrap_command(
                "\\emph{", inlines, out, width, dialect, wrap, smart, in_header,
            );
        }
        Inline::Strong(inlines) => {
            wrap_command(
                "\\textbf{",
                inlines,
                out,
                width,
                dialect,
                wrap,
                smart,
                in_header,
            );
        }
        Inline::Underline(inlines) => {
            wrap_command(
                "\\ul{", inlines, out, width, dialect, wrap, smart, in_header,
            );
        }
        Inline::Strikeout(inlines) => {
            wrap_command(
                "\\st{", inlines, out, width, dialect, wrap, smart, in_header,
            );
        }
        Inline::Superscript(inlines) => {
            wrap_command(
                "\\textsuperscript{",
                inlines,
                out,
                width,
                dialect,
                wrap,
                smart,
                in_header,
            );
        }
        Inline::Subscript(inlines) => {
            wrap_command(
                "\\textsubscript{",
                inlines,
                out,
                width,
                dialect,
                wrap,
                smart,
                in_header,
            );
        }
        Inline::SmallCaps(inlines) => {
            wrap_command(
                "\\textsc{",
                inlines,
                out,
                width,
                dialect,
                wrap,
                smart,
                in_header,
            );
        }
        Inline::Quoted(kind, inlines) => {
            let (open, close) = match kind {
                QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
                QuoteType::DoubleQuote => ('\u{201C}', '\u{201D}'),
            };
            out.push(Piece::text(escape_smart(
                &open.to_string(),
                EscapeMode::Text,
                smart,
            )));
            push_inlines(inlines, out, width, dialect, wrap, smart, in_header);
            out.push(Piece::text(escape_smart(
                &close.to_string(),
                EscapeMode::Text,
                smart,
            )));
        }
        Inline::Cite(_, inlines) => {
            push_inlines(inlines, out, width, dialect, wrap, smart, in_header);
        }
        Inline::Code(_, text) => {
            out.push(Piece::text(format!(
                "\\texttt{{{}}}",
                escape(text, EscapeMode::Code)
            )));
        }
        Inline::Space => out.push(Piece::Space),
        Inline::SoftBreak => out.push(Piece::Soft),
        Inline::LineBreak => {
            out.push(Piece::text("\\\\"));
            out.push(Piece::Hard);
        }
        Inline::Math(kind, text) => {
            let rendered = match kind {
                MathType::InlineMath => format!("\\({text}\\)"),
                MathType::DisplayMath => format!("\\[{text}\\]"),
            };
            out.push(Piece::text(rendered));
        }
        Inline::RawInline(format, text) => {
            if is_latex_format(&format.0) {
                out.push(Piece::text(text.to_string()));
            }
        }
        Inline::Link(attr, inlines, target) => {
            push_link(
                attr, inlines, target, out, width, dialect, wrap, smart, in_header,
            );
        }
        Inline::Image(attr, inlines, target) => {
            out.push(Piece::text(image(attr, inlines, target, smart)));
        }
        Inline::Span(attr, inlines) => {
            let mut open = if attr.id.is_empty() {
                String::new()
            } else if in_header {
                header_anchor(&attr.id)
            } else {
                phantom_label(&attr.id)
            };
            open.push('{');
            out.push(Piece::text(open));
            push_inlines(inlines, out, width, dialect, wrap, smart, in_header);
            out.push(Piece::text("}"));
        }
        Inline::Note(blocks) => out.push(Piece::text(note(blocks, width, dialect, wrap, smart))),
    }
}

#[allow(clippy::too_many_arguments)]
fn wrap_command(
    open: &str,
    inlines: &[Inline],
    out: &mut Vec<Piece>,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
) {
    out.push(Piece::text(open.to_owned()));
    push_inlines(inlines, out, width, dialect, wrap, smart, in_header);
    out.push(Piece::text("}"));
}

#[allow(clippy::too_many_arguments)]
fn push_link(
    attr: &Attr,
    inlines: &[Inline],
    target: &Target,
    out: &mut Vec<Piece>,
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
) {
    if !attr.id.is_empty() {
        out.push(Piece::text(if in_header {
            header_anchor(&attr.id)
        } else {
            phantom_label(&attr.id)
        }));
    }
    // A target into the document itself is a cross-reference, not an external location: the
    // fragment names a label and the link resolves through `\hyperref` rather than `\href`.
    if let Some(reference) = target.url.strip_prefix('#') {
        out.push(Piece::text(format!(
            "\\hyperref[{}]{{",
            cross_reference_label(reference)
        )));
        for inline in inlines {
            push_inline(inline, out, width, dialect, wrap, smart, in_header);
        }
        out.push(Piece::text("}"));
        return;
    }
    let url = escape_url(&target.url);
    if let [Inline::Str(text)] = inlines
        && label_matches_url(text, &target.url)
    {
        out.push(Piece::text(format!("\\url{{{url}}}")));
        return;
    }
    // A mailto link whose visible text is the bare address renders the address verbatim, with no
    // hyperlink styling applied to the text.
    if let [Inline::Str(text)] = inlines
        && let Some(address) = target.url.strip_prefix("mailto:")
        && text == address
    {
        let address = escape_url(address);
        out.push(Piece::text(format!(
            "\\href{{{url}}}{{\\nolinkurl{{{address}}}}}"
        )));
        return;
    }
    out.push(Piece::text(format!("\\href{{{url}}}{{")));
    push_inlines(inlines, out, width, dialect, wrap, smart, in_header);
    out.push(Piece::text("}"));
}

fn image(attr: &Attr, inlines: &[Inline], target: &Target, smart: bool) -> String {
    let svg = is_svg(&target.url);
    // The SVG include command carries no alternate-text key; for every other image the alt key is
    // emitted whenever a description is present, even one that renders to empty text.
    let alt_option = if svg || inlines.is_empty() {
        String::new()
    } else {
        format!(
            ",alt={{{}}}",
            escape_smart(&to_plain_text(inlines), EscapeMode::Text, smart)
        )
    };
    let command = if svg { "includesvg" } else { "includegraphics" };
    let url = escape_url(&target.url);

    let width = attribute_value(attr, "width").and_then(Dimension::parse);
    let height = attribute_value(attr, "height").and_then(Dimension::parse);
    if width.is_none() && height.is_none() {
        return format!("\\pandocbounded{{\\{command}[keepaspectratio{alt_option}]{{{url}}}}}");
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
    format!("\\{command}[width={width_option},height={height_option}{aspect}{alt_option}]{{{url}}}")
}

/// Whether an image URL names an SVG file, i.e. its path's final extension is `svg`. The extension
/// is the text after the last `.` in the last `/`-delimited segment, so a trailing query string
/// (which is part of the extension under this rule) means the URL is not treated as an SVG.
fn is_svg(url: &str) -> bool {
    let segment = url.rsplit('/').next().unwrap_or(url);
    matches!(segment.rsplit_once('.'), Some((_, ext)) if ext.eq_ignore_ascii_case("svg"))
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
fn note(
    blocks: &[Block],
    base_width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let width = base_width.saturating_sub(2);
    let mut parts: Vec<String> = Vec::new();
    let mut ends_with_code = false;
    for block in blocks {
        let (rendered, is_code) = match block {
            Block::CodeBlock(attr, text) => (code_block_env(attr, text, "Verbatim"), true),
            _ => (
                block_to_string(block, width, 0, dialect, wrap, smart),
                false,
            ),
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
    let opening = match dialect {
        Dialect::Article => "\\footnote{",
        Dialect::Slide { .. } => "\\footnote<\\value{beamerpauses}->[frame]{",
    };
    format!("{opening}{body}{closing}")
}

fn is_latex_format(format: &str) -> bool {
    format.eq_ignore_ascii_case("latex") || format.eq_ignore_ascii_case("tex")
}

/// Escape a run of literal text for the given context.
fn escape(text: &str, mode: EscapeMode) -> String {
    escape_smart(text, mode, true)
}

/// Escape text for LaTeX. With `smart`, Unicode smart punctuation (curly quotes, en/em dashes, the
/// ellipsis) renders as its TeX ligature; otherwise it passes through as the literal Unicode
/// character. The non-breaking space and the `--` ligature guard are structural and are emitted
/// regardless of `smart`.
fn escape_smart(text: &str, mode: EscapeMode, smart: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let code = mode == EscapeMode::Code;
    let is_trigger = |byte: u8| {
        matches!(
            byte,
            b'&' | b'%'
                | b'#'
                | b'_'
                | b'$'
                | b'{'
                | b'}'
                | b'^'
                | b'['
                | b']'
                | b'~'
                | b'\\'
                | b'<'
                | b'>'
                | b'|'
                | b'\''
                | b'-'
        ) || byte >= 0x80
            || (code && matches!(byte, b' ' | b'`'))
    };
    let mut rest = text;
    loop {
        let clean = clean_prefix_len(rest, is_trigger);
        let Some((head, tail)) = rest.split_at_checked(clean) else {
            out.push_str(rest);
            break;
        };
        out.push_str(head);
        let mut chars = tail.chars();
        let Some(ch) = chars.next() else { break };
        let next = chars.clone().next();
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
            // A literal hyphen abutting a smart en/em dash would merge with the dash's leading
            // hyphens into a longer ligature; the same guard keeps the hyphen distinct.
            '-' if mode == EscapeMode::Text
                && smart
                && matches!(next, Some('\u{2013}' | '\u{2014}')) =>
            {
                out.push_str("-\\/");
            }
            ' ' if mode == EscapeMode::Code => out.push_str("\\ "),
            '`' if mode == EscapeMode::Code => out.push_str("\\textasciigrave{}"),
            '\u{a0}' => out.push('~'),
            '\u{2026}' if mode == EscapeMode::Text && smart => {
                push_control_word(&mut out, "\\ldots", next, mode);
            }
            '\u{2013}' if mode == EscapeMode::Text && smart => out.push_str("--"),
            '\u{2014}' if mode == EscapeMode::Text && smart => out.push_str("---"),
            '\u{2018}' if mode == EscapeMode::Text && smart => {
                out.push('`');
                guard_quote_ligature(&mut out, next);
            }
            '\u{2019}' if mode == EscapeMode::Text && smart => {
                out.push('\'');
                guard_quote_ligature(&mut out, next);
            }
            '\u{201C}' if mode == EscapeMode::Text && smart => {
                out.push_str("``");
                guard_quote_ligature(&mut out, next);
            }
            '\u{201D}' if mode == EscapeMode::Text && smart => {
                out.push_str("''");
                guard_quote_ligature(&mut out, next);
            }
            other => out.push(other),
        }
        rest = chars.as_str();
    }
    out
}

/// Insert a thin-space ligature guard after a smart-quote glyph when the next character also opens
/// with a quote glyph (another smart quote, or a literal backtick). Without it, adjacent quotes such
/// as the two apostrophes of `’’` would fuse into a single closing double quote.
fn guard_quote_ligature(out: &mut String, next: Option<char>) {
    if matches!(
        next,
        Some('\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}' | '`')
    ) {
        out.push_str("\\,");
    }
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

/// The label naming an internal cross-reference, derived from a link's fragment. The fragment is
/// first escaped as a URL, then reduced to a single `\hyperref`-safe token by [`to_label`].
fn cross_reference_label(reference: &str) -> String {
    let mut escaped = String::with_capacity(reference.len());
    for ch in reference.chars() {
        match ch {
            '\\' => escaped.push('/'),
            '#' => escaped.push_str("\\#"),
            '%' => escaped.push_str("\\%"),
            '[' | ']' | '^' | '`' | '{' | '|' | '}' => percent_encode(ch, &mut escaped),
            other => escaped.push(other),
        }
    }
    to_label(&escaped)
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
        vec![Inline::Str(text.to_owned().into())]
    }

    fn render_columns(blocks: Vec<Block>, columns: usize) -> String {
        let document = Document {
            blocks,
            ..Document::default()
        };
        let mut options = WriterOptions::default();
        options.columns = Some(columns);
        LatexWriter.write(&document, &options).unwrap()
    }

    fn long_paragraph() -> Vec<Block> {
        let words: Vec<Inline> =
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi"
                .split(' ')
                .flat_map(|word| [Inline::Str(word.to_owned().into()), Inline::Space])
                .collect();
        vec![Block::Para(words)]
    }

    #[test]
    fn custom_columns_change_paragraph_wrapping() {
        let narrow = render_columns(long_paragraph(), 20);
        let wide = render_columns(long_paragraph(), 70);
        assert!(narrow.lines().count() > wide.lines().count());
        assert!(narrow.lines().all(|line| line.chars().count() <= 20));
    }

    #[test]
    fn omitted_columns_uses_the_default_fill_width() {
        assert_eq!(
            render(long_paragraph()),
            render_columns(long_paragraph(), 72)
        );
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
        // A literal hyphen before a smart en/em dash is guarded so the run is not read as one
        // longer ligature; a dash that itself renders to hyphens needs no such guard after it.
        assert_eq!(escape("-\u{2013}", EscapeMode::Text), "-\\/--");
        assert_eq!(escape("-\u{2014}", EscapeMode::Text), "-\\/---");
        assert_eq!(escape("\u{2013}-", EscapeMode::Text), "---");
        assert_eq!(escape("\u{2018}x\u{2019}", EscapeMode::Text), "`x'");
        assert_eq!(escape("\u{201C}x\u{201D}", EscapeMode::Text), "``x''");
    }

    #[test]
    fn escape_copies_clean_runs_verbatim() {
        assert_eq!(escape("hello world", EscapeMode::Text), "hello world");
        assert_eq!(
            escape("caf\u{e9} au lait", EscapeMode::Text),
            "caf\u{e9} au lait"
        );
        assert_eq!(escape("a & b", EscapeMode::Text), "a \\& b");
        assert_eq!(escape("100%", EscapeMode::Text), "100\\%");
    }

    #[test]
    fn escape_handles_triggers_at_run_edges() {
        assert_eq!(escape("&x", EscapeMode::Text), "\\&x");
        assert_eq!(escape("x&", EscapeMode::Text), "x\\&");
        assert_eq!(escape("&&", EscapeMode::Text), "\\&\\&");
        assert_eq!(escape("caf\u{e9}&", EscapeMode::Text), "caf\u{e9}\\&");
        assert_eq!(escape("&\u{e9}x", EscapeMode::Text), "\\&\u{e9}x");
    }

    #[test]
    fn escape_lookahead_sees_past_a_verbatim_prefix() {
        // The `--` ligature guard and the control-word separator both peek at the character after
        // the trigger; a prefix copied verbatim before the trigger must not hide that lookahead.
        assert_eq!(escape("abc--def", EscapeMode::Text), "abc-\\/-def");
        assert_eq!(escape("x~y", EscapeMode::Text), "x\\textasciitilde y");
        assert_eq!(escape("abc-\u{2013}", EscapeMode::Text), "abc-\\/--");
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
            render(vec![Block::Header(1, Box::default(), str_inlines("T"))]),
            "\\section{T}"
        );
        assert_eq!(
            render(vec![Block::Header(4, Box::default(), str_inlines("T"))]),
            "\\paragraph{T}"
        );
        assert_eq!(
            render(vec![Block::Header(5, Box::default(), str_inlines("T"))]),
            "\\subparagraph{T}"
        );
        // A level beyond the mapped range degrades to plain text.
        assert_eq!(
            render(vec![Block::Header(7, Box::default(), str_inlines("T"))]),
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
        let out = render(vec![Block::Header(1, Box::new(attr), str_inlines("Title"))]);
        assert!(out.contains("\\section*{Title}\\label{sec}"));
        assert!(out.contains("\\addcontentsline{toc}{section}{Title}"));
    }

    #[test]
    fn header_with_markup_wraps_texorpdfstring() {
        let inlines = vec![Inline::Emph(str_inlines("x"))];
        let out = render(vec![Block::Header(1, Box::default(), inlines)]);
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
            Box::new(attr),
            Box::new(caption),
            vec![Block::Plain(str_inlines("body"))],
        )]);
        assert!(out.contains("\\caption{Cap}\\label{fig}"));
    }

    #[test]
    fn figure_with_id_no_caption_emits_empty_caption() {
        let attr = Attr {
            id: "fig".into(),
            ..Attr::default()
        };
        let out = render(vec![Block::Figure(
            Box::new(attr),
            Box::default(),
            vec![Block::Plain(str_inlines("body"))],
        )]);
        assert!(out.contains("\\caption{}\\label{fig}"));
    }

    #[test]
    fn span_with_id_emits_phantom_label() {
        let span = Inline::Span(
            Box::new(Attr {
                id: "s".into(),
                ..Attr::default()
            }),
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
            Box::new(attr),
            str_inlines("alt"),
            Box::new(Target {
                url: "img.png".into(),
                title: String::new().into(),
            }),
        );
        let out = render(vec![Block::Para(vec![image])]);
        assert!(out.contains("width=0.5\\linewidth"));
        assert!(out.contains("height=2in"));
        assert!(out.contains("alt={alt}"));
    }

    #[test]
    fn footnote_with_code_block_closes_on_own_line() {
        let note = Inline::Note(vec![Block::CodeBlock(Box::default(), "x\n".into())]);
        let out = render(vec![Block::Para(vec![Inline::Str("a".into()), note])]);
        assert!(out.contains("\\begin{Verbatim}"));
        assert!(out.contains("\n}"));
    }
}
