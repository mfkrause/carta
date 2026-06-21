//! Typst writer: renders the document model to Typst markup.
//!
//! Block structure is conveyed through Typst's line-oriented markup; paragraph text is wrapped to a
//! fill column. Constructs that have no native markup form are emitted as code-mode function calls
//! (`#strong[..]`, `#link("..")[..]`, `#figure(..)`, `#table(..)`, …). Markup-significant characters
//! in literal text are backslash-escaped, some only where they could open a block or line marker.
//! Output carries no trailing newline; the caller appends one. The targeted syntax is described in
//! `vendor/typst/spec.md`.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColWidth, Document, Format, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MathType, QuoteType, Table, Target, to_plain_text,
};
use carta_core::{Result, WrapMode, Writer, WriterOptions};

use crate::common::{FILL_COLUMN, attribute_value, display_width};

/// Renders a document to Typst markup (no trailing newline).
#[derive(Debug, Default, Clone, Copy)]
pub struct TypstWriter;

impl Writer for TypstWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let body = blocks(&document.blocks, options.wrap);
        Ok(body.trim_end_matches('\n').to_owned())
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.typst"))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }
}

/// A fragment of rendered inline content awaiting line filling. Text fragments are breakable on the
/// spaces between them and may carry a marker character at their head that must be escaped should the
/// fragment open a physical line; atomic fragments (markup function calls) never break or escape.
#[derive(Debug, Clone)]
enum Fragment {
    /// A run of escaped literal text with no interior break point.
    Text(String),
    /// An atomic markup token (`#strong[..]`, a link, …) carried whole.
    Atom(String),
    /// A breakable space.
    Space,
    /// A soft line break from the source: a breakable reflow point under `Auto`, a space under
    /// `None`, and a kept physical line break under `Preserve`.
    Soft,
    /// A forced line break, rendered as ` \ ` and not breaking the physical line.
    LineBreak,
}

/// Render a top-level (or nested) block sequence. Every block is separated from the next by a blank
/// line, except that a header is followed by a single newline.
fn blocks(items: &[Block], wrap: WrapMode) -> String {
    let mut out = String::new();
    let mut previous_is_header = false;
    for item in items {
        let piece = block(item, wrap);
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if !previous_is_header {
                out.push('\n');
            }
        }
        out.push_str(&piece);
        previous_is_header = matches!(item, Block::Header(..));
    }
    out
}

fn block(value: &Block, wrap: WrapMode) -> String {
    match value {
        Block::Plain(items) | Block::Para(items) => fill_inlines(items, FILL_COLUMN, wrap),
        Block::Header(level, attr, items) => header(*level, attr, items, wrap),
        Block::CodeBlock(attr, text) => code_block(attr, text),
        Block::RawBlock(format, text) => raw_passthrough(format, text),
        Block::BlockQuote(items) => format!("#quote(block: true)[\n{}\n]", blocks(items, wrap)),
        Block::BulletList(items) => bullet_list(items, wrap),
        Block::OrderedList(list_attrs, items) => ordered_list(list_attrs, items, wrap),
        Block::DefinitionList(items) => definition_list(items, wrap),
        Block::HorizontalRule => "#horizontalrule".to_owned(),
        Block::LineBlock(lines) => line_block(lines, wrap),
        Block::Table(table) => render_table(table, wrap),
        Block::Figure(_, caption, items) => figure(caption, items, wrap),
        Block::Div(attr, items) => div(attr, items, wrap),
    }
}

fn header(level: i32, attr: &Attr, items: &[Inline], wrap: WrapMode) -> String {
    let text = inline_run(items, wrap);
    let heading = if attr.classes.iter().any(|class| class == "unnumbered") {
        format!("#heading(level: {level}, numbering: none)[{text}]")
    } else {
        let depth = usize::try_from(level.max(1)).unwrap_or(1);
        format!("{} {text}", "=".repeat(depth))
    };
    match label(&attr.id) {
        Some(rendered) => format!("{heading}\n{rendered}"),
        None => heading,
    }
}

/// A trailing label for a node carrying an id. Typst's `<name>` short form holds an identifier-like
/// id; an id containing whitespace is emitted through `#label("..")`, with interior whitespace
/// collapsed to single spaces.
fn label(id: &str) -> Option<String> {
    if id.is_empty() {
        return None;
    }
    if id.contains(char::is_whitespace) {
        let normalized: String = id.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(format!("#label(\"{}\")", escape_string(&normalized)))
    } else {
        Some(format!("<{id}>"))
    }
}

fn code_block(attr: &Attr, text: &str) -> String {
    let fence = backtick_fence(text);
    match attr.classes.first() {
        Some(language) => format!("{fence}{language}\n{text}\n{fence}"),
        None => format!("{fence}\n{text}\n{fence}"),
    }
}

/// A backtick fence at least one tick longer than the longest backtick run in the payload, so the
/// fence cannot be closed early by the content.
fn backtick_fence(text: &str) -> String {
    let mut longest = 0usize;
    let mut current = 0usize;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

fn bullet_list(items: &[Vec<Block>], wrap: WrapMode) -> String {
    let loose = !list_is_tight(items);
    let mut lines = Vec::new();
    for item in items {
        lines.push(list_item("- ", item, wrap));
    }
    lines.join(if loose { "\n\n" } else { "\n" })
}

fn ordered_list(attrs: &ListAttributes, items: &[Vec<Block>], wrap: WrapMode) -> String {
    let loose = !list_is_tight(items);
    let mut lines = Vec::new();
    for item in items {
        lines.push(list_item("+ ", item, wrap));
    }
    let body = lines.join(if loose { "\n\n" } else { "\n" });
    if is_default_enum(attrs) {
        body
    } else {
        format!(
            "#block[\n#set enum(numbering: \"{}\", start: {})\n{body}\n]",
            enum_numbering(attrs),
            attrs.start,
        )
    }
}

/// Whether every item is empty or opens with a [`Block::Plain`]; such a list renders tight.
fn list_is_tight(items: &[Vec<Block>]) -> bool {
    items
        .iter()
        .all(|item| matches!(item.first(), None | Some(Block::Plain(_))))
}

/// Whether an ordered list uses Typst's implicit `+` numbering: decimal style, period delimiter, and
/// a start of one. Anything else is rendered through an explicit `#set enum` rule.
fn is_default_enum(attrs: &ListAttributes) -> bool {
    attrs.start == 1
        && matches!(
            attrs.style,
            ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal
        )
        && matches!(
            attrs.delim,
            ListNumberDelim::DefaultDelim | ListNumberDelim::Period
        )
}

/// The `numbering` pattern string for an explicit enumeration: the style's sample numeral wrapped in
/// the delimiter (`1.`, `a)`, `(I)`, …).
fn enum_numbering(attrs: &ListAttributes) -> String {
    let numeral = match attrs.style {
        ListNumberStyle::LowerAlpha => "a",
        ListNumberStyle::UpperAlpha => "A",
        ListNumberStyle::LowerRoman => "i",
        ListNumberStyle::UpperRoman => "I",
        ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal | ListNumberStyle::Example => "1",
    };
    match attrs.delim {
        ListNumberDelim::OneParen => format!("{numeral})"),
        ListNumberDelim::TwoParens => format!("({numeral})"),
        ListNumberDelim::Period | ListNumberDelim::DefaultDelim => format!("{numeral}."),
    }
}

/// Render one list item: the marker on its first line, with every continuation line indented to
/// align under the marker's text column.
fn list_item(marker: &str, item: &[Block], wrap: WrapMode) -> String {
    let body = blocks(item, wrap);
    let indent = " ".repeat(marker.len());
    let mut out = String::new();
    for (index, line) in body.lines().enumerate() {
        if index > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(&indent);
            }
        }
        out.push_str(line);
    }
    format!("{marker}{out}")
}

fn definition_list(items: &[(Vec<Inline>, Vec<Vec<Block>>)], wrap: WrapMode) -> String {
    let mut lines = Vec::new();
    for (term, definitions) in items {
        let body = blocks(
            &definitions.iter().flatten().cloned().collect::<Vec<_>>(),
            wrap,
        );
        lines.push(format!(
            "/ {}: #block[\n{body}\n]",
            escape_term_colons(&inline_run(term, wrap))
        ));
    }
    lines.join("\n")
}

/// In a definition term, a colon would close the term, so escape every colon in the rendered markup
/// — but not those inside a string-literal argument such as a link URL.
fn escape_term_colons(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in term.chars() {
        if !escaped && ch == '"' {
            in_string = !in_string;
        }
        if ch == ':' && !in_string && !escaped {
            out.push('\\');
        }
        escaped = ch == '\\' && !escaped;
        out.push(ch);
    }
    out
}

fn line_block(lines: &[Vec<Inline>], wrap: WrapMode) -> String {
    let rendered: Vec<String> = lines.iter().map(|line| inline_run(line, wrap)).collect();
    rendered.join(" \\ ")
}

fn div(attr: &Attr, items: &[Block], wrap: WrapMode) -> String {
    let body = blocks(items, wrap);
    let trailing = match items.last() {
        Some(Block::Plain(_)) => "",
        _ => "\n",
    };
    let block = format!("#block[\n{body}{trailing}\n]");
    match label(&attr.id) {
        Some(rendered) => format!("{block} {rendered}"),
        None => block,
    }
}

fn figure(caption: &Caption, items: &[Block], wrap: WrapMode) -> String {
    let inner = match figure_image(items) {
        Some(image) => image,
        None => format!("[{}]", blocks(items, wrap)),
    };
    format!(
        "#figure({inner},\n  caption: [\n    {}\n  ]\n)",
        blocks(&caption.long, wrap).trim_end_matches('\n')
    )
}

/// When a figure's body is a single image, the bare `image(..)` call carried directly as the
/// figure's content.
fn figure_image(items: &[Block]) -> Option<String> {
    match items {
        [Block::Plain(inlines) | Block::Para(inlines)] => match inlines.as_slice() {
            [Inline::Image(attr, alt, target)] => Some(image_call(attr, alt, target)),
            _ => None,
        },
        _ => None,
    }
}

fn render_table(table: &Table, wrap: WrapMode) -> String {
    let columns = table_columns(table);
    let aligns = table_aligns(table);
    let mut grid = String::new();
    let _ = writeln!(grid, "    columns: {columns},");
    let _ = writeln!(grid, "    align: {aligns},");

    let head_rows = collect_rows(&table.head.rows);
    if !head_rows.is_empty() {
        let cells: Vec<&Cell> = head_rows.into_iter().flatten().collect();
        let _ = writeln!(
            grid,
            "    table.header({},),",
            render_row(&cells, "    table.header(", 6, wrap)
        );
        grid.push_str("    table.hline(),\n");
    }

    for body in &table.bodies {
        let head = collect_rows(&body.head);
        emit_rows(&mut grid, &head, wrap);
        if !head.is_empty() {
            grid.push_str("    table.hline(),\n");
        }
        emit_rows(&mut grid, &collect_rows(&body.body), wrap);
    }

    let foot_rows = collect_rows(&table.foot.rows);
    if !foot_rows.is_empty() {
        grid.push_str("    table.hline(),\n");
        let cells: Vec<&Cell> = foot_rows.into_iter().flatten().collect();
        let _ = writeln!(
            grid,
            "    table.footer({},),",
            render_row(&cells, "    table.footer(", 6, wrap)
        );
    }

    let mut out = format!("#figure(\n  align(center)[#table(\n{grid}  )]\n");
    if !table.caption.long.is_empty() {
        let _ = writeln!(out, "  , caption: {}", table_caption(&table.caption.long));
    }
    out.push_str("  , kind: table\n  )");
    match label(&table.attr.id) {
        Some(rendered) => format!("{out}\n{rendered}"),
        None => out,
    }
}

/// Render a table caption within `[..]`. A single inline block stays on one line; richer content is
/// laid out as an indented block, two columns in.
fn table_caption(content: &[Block]) -> String {
    if let [Block::Plain(inlines) | Block::Para(inlines)] = content {
        format!("[{}]", inline_run(inlines, WrapMode::Auto))
    } else {
        let mut body = blocks(content, WrapMode::Auto);
        if !matches!(content.last(), Some(Block::Plain(_))) {
            body.push('\n');
        }
        format!("[{}\n  ]", indent_continuation(&body, "  "))
    }
}

fn emit_rows(grid: &mut String, rows: &[Vec<&Cell>], wrap: WrapMode) {
    for row in rows {
        let _ = writeln!(grid, "    {},", render_row(row, "    ", 4, wrap));
    }
}

/// Render a row's cells as a `, `-joined sequence. Each cell's content is laid out from the column
/// where its opening bracket falls (so a long cell wraps against the fill column), which depends on
/// the `prefix` that opens the row line and the widths of the cells before it.
fn render_row(row: &[&Cell], prefix: &str, indent: usize, wrap: WrapMode) -> String {
    let mut out = String::new();
    let mut column = display_width(prefix);
    for (index, cell) in row.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
            column += 2;
        }
        let rendered = table_cell(cell, column, indent, wrap);
        match rendered.rfind('\n') {
            Some(position) => column = display_width(&rendered[position + 1..]),
            None => column += display_width(&rendered),
        }
        out.push_str(&rendered);
    }
    out
}

fn collect_rows(rows: &[carta_ast::Row]) -> Vec<Vec<&Cell>> {
    rows.iter().map(|row| row.cells.iter().collect()).collect()
}

/// The `columns:` argument: a bare count when every column takes the default width, or a tuple of
/// percentages when any column carries an explicit fractional width.
fn table_columns(table: &Table) -> String {
    let has_explicit = table
        .col_specs
        .iter()
        .any(|spec| matches!(spec.width, ColWidth::ColWidth(_)));
    if has_explicit {
        let widths: Vec<String> = table
            .col_specs
            .iter()
            .map(|spec| match spec.width {
                ColWidth::ColWidth(fraction) => format!("{}%", trim_percent(fraction * 100.0)),
                ColWidth::ColWidthDefault => "auto".to_owned(),
            })
            .collect();
        format!("({})", widths.join(", "))
    } else {
        table.col_specs.len().to_string()
    }
}

fn table_aligns(table: &Table) -> String {
    let mut out = String::from("(");
    for spec in &table.col_specs {
        out.push_str(alignment(&spec.align));
        out.push(',');
    }
    out.push(')');
    out
}

fn alignment(value: &Alignment) -> &'static str {
    match value {
        Alignment::AlignLeft => "left",
        Alignment::AlignRight => "right",
        Alignment::AlignCenter => "center",
        Alignment::AlignDefault => "auto",
    }
}

fn table_cell(cell: &Cell, column: usize, indent: usize, wrap: WrapMode) -> String {
    let mut spans = Vec::new();
    if cell.col_span != 1 {
        spans.push(format!("colspan: {}", cell.col_span));
    }
    if cell.row_span != 1 {
        spans.push(format!("rowspan: {}", cell.row_span));
    }
    let mut prefix = String::new();
    if !spans.is_empty() {
        prefix = format!("table.cell({})", spans.join(", "));
    }
    let bracket_column = column + display_width(&prefix);
    format!(
        "{prefix}{}",
        cell_content(&cell.content, bracket_column, indent, wrap)
    )
}

/// Render a cell's content within `[..]`. A single block of inline content fills against the column
/// where its opening bracket sits; richer content is laid out as an indented block. Wrapped lines sit
/// `indent` columns in.
fn cell_content(content: &[Block], bracket_column: usize, indent: usize, wrap: WrapMode) -> String {
    let pad = " ".repeat(indent);
    match content {
        [Block::Plain(inlines) | Block::Para(inlines)] => {
            let filled = fill_cell(&fragments(inlines, wrap), bracket_column + 1, indent, wrap);
            format!("[{}]", indent_continuation(&filled, &pad))
        }
        [] => "[]".to_owned(),
        blocks_value => {
            let mut body = blocks(blocks_value, wrap);
            if !matches!(blocks_value.last(), Some(Block::Plain(_))) {
                body.push('\n');
            }
            format!("[{}\n{pad}]", indent_continuation(&body, &pad))
        }
    }
}

/// Prefix every line after the first with `indent`, leaving blank lines bare.
fn indent_continuation(body: &str, indent: &str) -> String {
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
    }
    out
}

/// Render inline content laid out per the document's wrap mode (paragraph context).
fn fill_inlines(items: &[Inline], width: usize, wrap: WrapMode) -> String {
    fill(&fragments(items, wrap), width, wrap)
}

/// Render inline content without wrapping, single-spacing the breakable units (nested markup
/// context, where the surrounding construct controls layout). A nested footnote's body still
/// reflows per the document `wrap`.
fn inline_run(items: &[Inline], wrap: WrapMode) -> String {
    let mut out = String::new();
    for fragment in fragments(items, wrap) {
        match fragment {
            Fragment::Text(text) | Fragment::Atom(text) => out.push_str(&text),
            Fragment::Space | Fragment::Soft => out.push(' '),
            Fragment::LineBreak => out.push_str(" \\ "),
        }
    }
    escape_open_marker(&out)
}

/// Build the fragment stream for an inline sequence. A leading `(` on the very first text fragment is
/// escaped here (the only character whose escape depends on opening the whole sequence rather than a
/// physical line).
fn fragments(items: &[Inline], wrap: WrapMode) -> Vec<Fragment> {
    let mut out = Vec::new();
    let mut after_space = true;
    let mut first_inline = true;
    for item in items {
        let next_after_space = matches!(item, Inline::Space | Inline::SoftBreak);
        match item {
            Inline::Str(text) => {
                out.push(Fragment::Text(escape_text(text, after_space, first_inline)));
                first_inline = false;
            }
            Inline::Space => out.push(Fragment::Space),
            Inline::SoftBreak => out.push(Fragment::Soft),
            Inline::LineBreak => out.push(Fragment::LineBreak),
            Inline::Emph(inner) => {
                extend_wrapped(&mut out, inner, "#emph[", "]", wrap);
                first_inline = false;
            }
            Inline::Strong(inner) => {
                extend_wrapped(&mut out, inner, "#strong[", "]", wrap);
                first_inline = false;
            }
            Inline::Strikeout(inner) => {
                extend_wrapped(&mut out, inner, "#strike[", "]", wrap);
                first_inline = false;
            }
            Inline::Underline(inner) => {
                extend_wrapped(&mut out, inner, "#underline[", "]", wrap);
                first_inline = false;
            }
            Inline::SmallCaps(inner) => {
                extend_wrapped(&mut out, inner, "#smallcaps[", "]", wrap);
                first_inline = false;
            }
            Inline::Quoted(kind, inner) => {
                let (open, close) = quote_marks(kind);
                extend_wrapped(&mut out, inner, &open.to_string(), &close.to_string(), wrap);
                first_inline = false;
            }
            Inline::Span(attr, inner) if attr.id.is_empty() => {
                let (open, close) = span_wrapper(attr);
                extend_wrapped(&mut out, inner, open, close, wrap);
                first_inline = false;
            }
            Inline::Span(attr, inner) => {
                out.push(Fragment::Atom(span(attr, inner, first_inline, wrap)));
                first_inline = false;
            }
            other => {
                let rendered = inline(other, wrap);
                if !rendered.is_empty() {
                    out.push(Fragment::Atom(rendered));
                    first_inline = false;
                }
            }
        }
        after_space = next_after_space;
    }
    out
}

/// Splice an inline sequence into `out` wrapped in `open`/`close` delimiters, keeping its internal
/// spaces as wrap points: the opening fuses to the first word and the closing to the last, so a long
/// run can break across physical lines with the delimiters staying attached to their boundary words.
/// Both boundary words remain escapable so a leading line-marker character is still guarded when a
/// word opens a physical line.
fn extend_wrapped(
    out: &mut Vec<Fragment>,
    items: &[Inline],
    open: &str,
    close: &str,
    wrap: WrapMode,
) {
    let mut inner = fragments(items, wrap);
    let is_textual =
        |fragment: &Fragment| matches!(fragment, Fragment::Text(_) | Fragment::Atom(_));
    match inner.iter().position(is_textual) {
        None => out.push(Fragment::Atom(format!("{open}{close}"))),
        Some(first) => {
            let last = inner.iter().rposition(is_textual).unwrap_or(first);
            if let Some(fragment) = inner.get_mut(first) {
                prepend_fragment(fragment, open);
            }
            if let Some(fragment) = inner.get_mut(last) {
                append_fragment(fragment, close);
            }
            out.append(&mut inner);
        }
    }
}

/// Prepend `prefix` to a textual fragment's text, leaving its variant (and thus its escapability) intact.
fn prepend_fragment(fragment: &mut Fragment, prefix: &str) {
    if let Fragment::Text(text) | Fragment::Atom(text) = fragment {
        *text = format!("{prefix}{text}");
    }
}

/// Append `suffix` to a textual fragment's text, leaving its variant (and thus its escapability) intact.
fn append_fragment(fragment: &mut Fragment, suffix: &str) {
    if let Fragment::Text(text) | Fragment::Atom(text) = fragment {
        text.push_str(suffix);
    }
}

/// The delimiters wrapping a span's content: a semantic class selects a Typst function, otherwise the
/// content renders bare. Spans carrying an id label are handled separately and never reach here.
fn span_wrapper(attr: &Attr) -> (&'static str, &'static str) {
    if attr.classes.iter().any(|class| class == "mark") {
        ("#highlight[", "]")
    } else if attr.classes.iter().any(|class| class == "underline") {
        ("#underline[", "]")
    } else if attr.classes.iter().any(|class| class == "smallcaps") {
        ("#smallcaps[", "]")
    } else {
        ("", "")
    }
}

/// Greedily fill fragments to the fill column at a physical line start (paragraph context), laid out
/// per the document's wrap mode.
fn fill(fragments: &[Fragment], width: usize, wrap: WrapMode) -> String {
    // Outside a cell only Auto reflows to a width; the other modes lay everything on one line and
    // split solely on source soft breaks.
    let width = if matches!(wrap, WrapMode::Auto) {
        width.max(1)
    } else {
        usize::MAX
    };
    fill_with(fragments, width, 0, 0, matches!(wrap, WrapMode::Preserve))
}

/// Fill fragments for a table cell whose first line begins at `first` columns in (after the opening
/// bracket) and whose wrapped continuation lines sit at `indent` columns. Continuation lines are
/// emitted at column zero; the caller applies the indent. The cell content is `#table` source rather
/// than a bordered field, so it follows the document wrap mode: only `Auto` reflows to the fill
/// column, while `None` and `Preserve` keep it on physical lines split solely on source soft breaks.
fn fill_cell(fragments: &[Fragment], first: usize, indent: usize, wrap: WrapMode) -> String {
    let width = if matches!(wrap, WrapMode::Auto) {
        FILL_COLUMN
    } else {
        usize::MAX
    };
    fill_with(
        fragments,
        width,
        first,
        indent,
        matches!(wrap, WrapMode::Preserve),
    )
}

/// Lay fragments out into lines no wider than `width` (already resolved to a sentinel when no width
/// wrap is wanted). The first line is laid out as if `first` columns are already consumed; each
/// continuation line reserves `indent` columns. A line-opening `- + = /` is escaped only when it
/// begins a true physical line (`first == 0`). A source soft break forces a fresh physical line when
/// `preserve_softs` is set, and is otherwise inter-word space.
fn fill_with(
    fragments: &[Fragment],
    width: usize,
    first: usize,
    indent: usize,
    preserve_softs: bool,
) -> String {
    let mut out = String::new();
    let mut column = first;
    let mut at_line_start = first == 0;
    let mut physical_line_start = first == 0;
    let mut pending_space = false;
    for fragment in fragments {
        match fragment {
            Fragment::Space => pending_space = true,
            Fragment::Soft if preserve_softs => {
                out.push('\n');
                column = indent;
                pending_space = false;
                at_line_start = true;
                physical_line_start = true;
            }
            Fragment::Soft => pending_space = true,
            Fragment::LineBreak => {
                out.push_str(" \\ ");
                column += 3;
                pending_space = false;
                at_line_start = false;
                physical_line_start = false;
            }
            Fragment::Text(text) | Fragment::Atom(text) => {
                let escapable = matches!(fragment, Fragment::Text(_));
                let word_width = display_width(text);
                if at_line_start {
                    push_word(&mut out, text, escapable, physical_line_start);
                    column += word_width;
                    at_line_start = false;
                } else if pending_space && column + 1 + word_width > width {
                    out.push('\n');
                    push_word(&mut out, text, escapable, true);
                    column = indent + word_width;
                    physical_line_start = true;
                } else {
                    if pending_space {
                        out.push(' ');
                        column += 1;
                    }
                    push_word(&mut out, text, escapable, false);
                    column += word_width;
                }
                pending_space = false;
            }
        }
    }
    out
}

/// Append a word; when it opens a physical line, escape a leading `- + = /` that would otherwise be
/// read as a list or line marker.
fn push_word(out: &mut String, word: &str, escapable: bool, line_start: bool) {
    if line_start && escapable {
        out.push_str(&escape_open_marker(word));
    } else {
        out.push_str(word);
    }
}

/// Escape a leading line-marker character (`- + = /`) so it does not open a Typst list or rule.
fn escape_open_marker(word: &str) -> String {
    match word.chars().next() {
        Some('-' | '+' | '=' | '/') => format!("\\{word}"),
        _ => word.to_owned(),
    }
}

fn inline(value: &Inline, wrap: WrapMode) -> String {
    match value {
        Inline::Str(text) => escape_text(text, true, false),
        Inline::Emph(items) => format!("#emph[{}]", inline_run(items, wrap)),
        Inline::Strong(items) => format!("#strong[{}]", inline_run(items, wrap)),
        Inline::Underline(items) => format!("#underline[{}]", inline_run(items, wrap)),
        Inline::Strikeout(items) => format!("#strike[{}]", inline_run(items, wrap)),
        Inline::Superscript(items) => format!("#super[{}]", inline_run(items, wrap)),
        Inline::Subscript(items) => format!("#sub[{}]", inline_run(items, wrap)),
        Inline::SmallCaps(items) => format!("#smallcaps[{}]", inline_run(items, wrap)),
        Inline::Quoted(kind, items) => {
            let (open, close) = quote_marks(kind);
            format!("{open}{}{close}", inline_run(items, wrap))
        }
        Inline::Cite(citations, _) => cite(citations),
        Inline::Code(_, text) => inline_code(text),
        Inline::Space | Inline::SoftBreak => " ".to_owned(),
        Inline::LineBreak => " \\ ".to_owned(),
        Inline::Math(kind, text) => math(kind, text),
        Inline::RawInline(format, text) => raw_inline_passthrough(format, text),
        Inline::Link(_, items, target) => link(items, target, wrap),
        Inline::Image(attr, alt, target) => format!("#box({})", image_call(attr, alt, target)),
        Inline::Note(blocks) => format!("#footnote[{}]", self_blocks(blocks, wrap)),
        Inline::Span(attr, items) => span(attr, items, false, wrap),
    }
}

fn self_blocks(items: &[Block], wrap: WrapMode) -> String {
    blocks(items, wrap).trim_end_matches('\n').to_owned()
}

fn cite(citations: &[carta_ast::Citation]) -> String {
    let mut out = String::new();
    for citation in citations {
        let _ = write!(out, "@{}", citation.id);
    }
    out
}

fn link(items: &[Inline], target: &Target, wrap: WrapMode) -> String {
    let plain = to_plain_text(items);
    let url = escape_string(&target.url);
    if plain == target.url {
        format!("#link(\"{url}\")")
    } else {
        // The label is laid out at unbounded width so it is never reflowed (the link is one unit for
        // wrapping), yet a source line break inside it is still kept under `Preserve`.
        let label = fill(&fragments(items, wrap), usize::MAX, wrap);
        format!("#link(\"{url}\")[{label}]")
    }
}

/// Render a span: its semantic classes select a wrapper, then a trailing id label. A label opening
/// the inline sequence is anchored with a leading zero-width space so it does not attach to the
/// preceding markup.
fn span(attr: &Attr, items: &[Inline], at_start: bool, wrap: WrapMode) -> String {
    let content = if attr.classes.iter().any(|class| class == "mark") {
        format!("#highlight[{}]", inline_run(items, wrap))
    } else if attr.classes.iter().any(|class| class == "underline") {
        format!("#underline[{}]", inline_run(items, wrap))
    } else if attr.classes.iter().any(|class| class == "smallcaps") {
        format!("#smallcaps[{}]", inline_run(items, wrap))
    } else {
        inline_run(items, wrap)
    };
    match label(&attr.id) {
        Some(rendered) if at_start => format!("\u{200b}{content}{rendered}"),
        Some(rendered) => format!("{content}{rendered}"),
        None => content,
    }
}

/// Render an `image(..)` call: the path, then any `height`/`width` from the attributes, then the alt
/// text.
fn image_call(attr: &Attr, alt: &[Inline], target: &Target) -> String {
    let mut args = vec![format!("\"{}\"", escape_string(&target.url))];
    if let Some(height) = attribute_value(attr, "height") {
        args.push(format!("height: {}", dimension(height)));
    }
    if let Some(width) = attribute_value(attr, "width") {
        args.push(format!("width: {}", dimension(width)));
    }
    let alt_text = to_plain_text(alt);
    if !alt_text.is_empty() {
        args.push(format!("alt: \"{}\"", escape_string(&alt_text)));
    }
    format!("image({})", args.join(", "))
}

/// A Typst length argument from an attribute value: a percentage carries a `.0` for a whole number; a
/// bare number or pixel count converts to inches at 96 pixels per inch; any other unit passes
/// through.
fn dimension(value: &str) -> String {
    if let Some(percent) = value.strip_suffix('%') {
        if percent.contains('.') {
            return format!("{percent}%");
        }
        return format!("{percent}.0%");
    }
    let pixels = value.strip_suffix("px").unwrap_or(value);
    if let Ok(number) = pixels.parse::<f64>() {
        format!("{}in", trim_number(number / 96.0))
    } else {
        value.to_owned()
    }
}

/// Format a number to at most five decimal places, dropping trailing zeros and a bare decimal point.
fn trim_number(value: f64) -> String {
    trim_decimals(&format!("{value:.5}"))
}

/// Format a column-width percentage to at most two decimal places.
fn trim_percent(value: f64) -> String {
    trim_decimals(&format!("{value:.2}"))
}

fn trim_decimals(text: &str) -> String {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text.to_owned()
    }
}

/// Render a math expression as Typst. The source is translated to Typst's native math markup when
/// possible; an expression with no Typst equivalent is emitted verbatim, with its TeX delimiters
/// reconstructed and the whole run escaped as ordinary markup text.
fn math(kind: &MathType, text: &str) -> String {
    let display = matches!(kind, MathType::DisplayMath);
    let Some(math) = crate::math::to_typst_labeled(text, display) else {
        let verbatim = match kind {
            MathType::InlineMath => format!("${text}$"),
            MathType::DisplayMath => format!("$${text}$$"),
        };
        return escape_text(&verbatim, false, true);
    };
    let crate::math::TypstMath { body, label } = math;
    // An equation `\label` is set as a Typst reference label immediately after the closing `$`.
    let label = label.as_deref().unwrap_or("");
    match kind {
        MathType::InlineMath => format!("${body}${label}"),
        MathType::DisplayMath => format!("$ {body} ${label}"),
    }
}

/// Inline code: backtick-delimited raw markup, falling back to `#raw(..)` when the content contains a
/// backtick that the delimiter could not contain.
fn inline_code(text: &str) -> String {
    if text.contains('`') {
        format!("#raw(\"{}\")", escape_string(text))
    } else {
        format!("`{text}`")
    }
}

/// Emit a raw-passthrough inline verbatim when its format is Typst; drop it otherwise.
fn raw_inline_passthrough(format: &Format, text: &str) -> String {
    if format.0 == "typst" {
        text.to_owned()
    } else {
        String::new()
    }
}

/// Emit a raw-passthrough block verbatim when its format is Typst; drop it otherwise.
fn raw_passthrough(format: &Format, text: &str) -> String {
    if format.0 == "typst" {
        text.trim_end_matches('\n').to_owned()
    } else {
        String::new()
    }
}

fn quote_marks(kind: &QuoteType) -> (char, char) {
    match kind {
        QuoteType::SingleQuote => ('\'', '\''),
        QuoteType::DoubleQuote => ('"', '"'),
    }
}

/// Escape a literal-text token for markup mode.
///
/// Always-escaped characters are markup-significant anywhere. The remaining cases key off position:
/// `.` and `;` are escaped when they open a token that continues with more text; `(` is escaped when
/// it opens a token that is not preceded by a space; a `-` or `/` directly following one of its own
/// kind is escaped. En/em dashes are spelled `--`/`---`. The leading `- + = /` line markers are left
/// for the fill pass, which escapes them only at a physical line start.
fn escape_text(text: &str, after_space: bool, first_text: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &ch) in chars.iter().enumerate() {
        let previous = if index == 0 {
            None
        } else {
            chars.get(index - 1)
        };
        let has_more = index + 1 < chars.len();
        let escape = match ch {
            '*' | '_' | '`' | '\\' | '#' | '$' | '@' | '<' | '>' | '~' | '[' | ']' | '"' | '\'' => {
                true
            }
            '.' | ';' => index == 0 && has_more,
            '(' => index == 0 && has_more && (!after_space || first_text),
            '-' | '/' => previous == Some(&ch),
            _ => false,
        };
        if let Some(replacement) = smart_replacement(ch) {
            out.push_str(replacement);
            continue;
        }
        if escape {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// The literal spelling Typst markup uses for a punctuation character that has a typographic form:
/// dashes become their hyphen runs, smart quotes their straight equivalents, and a non-breaking
/// space the `~` shortcut.
fn smart_replacement(ch: char) -> Option<&'static str> {
    match ch {
        '\u{2013}' => Some("--"),
        '\u{2014}' => Some("---"),
        '\u{2018}' | '\u{2019}' => Some("'"),
        '\u{201C}' | '\u{201D}' => Some("\""),
        '\u{00A0}' => Some("~"),
        _ => None,
    }
}

/// Escape a string for a double-quoted Typst string literal: backslash and double-quote only.
fn escape_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\\' || ch == '"' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::Document;

    fn render(blocks: Vec<Block>) -> String {
        let document = Document {
            blocks,
            ..Document::default()
        };
        TypstWriter
            .write(&document, &WriterOptions::default())
            .unwrap()
    }

    fn para(inlines: Vec<Inline>) -> Block {
        Block::Para(inlines)
    }

    fn str_inline(text: &str) -> Inline {
        Inline::Str(text.to_owned())
    }

    #[test]
    fn empty_document() {
        assert_eq!(render(vec![]), "");
    }

    #[test]
    fn paragraph_with_emphasis() {
        assert_eq!(
            render(vec![para(vec![
                Inline::Strong(vec![str_inline("bold")]),
                Inline::Space,
                Inline::Emph(vec![str_inline("italic")]),
            ])]),
            "#strong[bold] #emph[italic]"
        );
    }

    #[test]
    fn heading_with_label() {
        assert_eq!(
            render(vec![Block::Header(
                2,
                Attr {
                    id: "intro".into(),
                    ..Attr::default()
                },
                vec![str_inline("H")],
            )]),
            "== H\n<intro>"
        );
    }

    #[test]
    fn heading_unnumbered_uses_function() {
        assert_eq!(
            render(vec![Block::Header(
                1,
                Attr {
                    id: "hidden".into(),
                    classes: vec!["unnumbered".into()],
                    ..Attr::default()
                },
                vec![str_inline("Hidden")],
            )]),
            "#heading(level: 1, numbering: none)[Hidden]\n<hidden>"
        );
    }

    #[test]
    fn default_ordered_list_uses_plus() {
        let attrs = ListAttributes {
            start: 1,
            style: ListNumberStyle::Decimal,
            delim: ListNumberDelim::Period,
        };
        let items = vec![vec![Block::Plain(vec![str_inline("a")])]];
        assert_eq!(render(vec![Block::OrderedList(attrs, items)]), "+ a");
    }

    #[test]
    fn loose_bullet_list_separates_items() {
        let items = vec![
            vec![Block::Para(vec![str_inline("a")])],
            vec![Block::Para(vec![str_inline("b")])],
        ];
        assert_eq!(render(vec![Block::BulletList(items)]), "- a\n\n- b");
    }

    #[test]
    fn inline_code_uses_backticks() {
        assert_eq!(
            render(vec![para(vec![Inline::Code(
                Attr::default(),
                "let x = 1;".into()
            )])]),
            "`let x = 1;`"
        );
    }

    #[test]
    fn inline_code_with_backtick_falls_back() {
        assert_eq!(
            render(vec![para(vec![Inline::Code(
                Attr::default(),
                "a`b".into()
            )])]),
            "#raw(\"a`b\")"
        );
    }

    #[test]
    fn paragraph_wraps_at_fill_column() {
        let words: Vec<Inline> = std::iter::repeat_n(
            [str_inline("word"), Inline::Space]
                .into_iter()
                .collect::<Vec<_>>(),
            15,
        )
        .flatten()
        .chain(std::iter::once(str_inline("end")))
        .collect();
        let rendered = render(vec![para(words)]);
        assert!(rendered.contains('\n'));
        assert!(rendered.lines().all(|line| line.len() <= FILL_COLUMN));
    }

    #[test]
    fn span_label_at_start_anchors_with_zwsp() {
        assert_eq!(
            render(vec![para(vec![Inline::Span(
                Attr {
                    id: "sid".into(),
                    ..Attr::default()
                },
                vec![str_inline("a")],
            )])]),
            "\u{200b}a<sid>"
        );
    }

    #[test]
    fn span_label_mid_text_has_no_zwsp() {
        assert_eq!(
            render(vec![para(vec![
                str_inline("a"),
                Inline::Space,
                Inline::Span(
                    Attr {
                        id: "s".into(),
                        ..Attr::default()
                    },
                    vec![str_inline("styled")],
                ),
                Inline::Space,
                str_inline("word"),
            ])]),
            "a styled<s> word"
        );
    }

    #[test]
    fn mark_span_highlights() {
        assert_eq!(
            render(vec![para(vec![Inline::Span(
                Attr {
                    classes: vec!["mark".into()],
                    ..Attr::default()
                },
                vec![str_inline("x")],
            )])]),
            "#highlight[x]"
        );
    }

    #[test]
    fn image_pixel_width_converts_to_inches() {
        assert_eq!(
            render(vec![para(vec![Inline::Image(
                Attr {
                    attributes: vec![("width".into(), "200".into())],
                    ..Attr::default()
                },
                vec![str_inline("alt")],
                Target {
                    url: "i.png".into(),
                    title: String::new(),
                },
            )])]),
            "#box(image(\"i.png\", width: 2.08333in, alt: \"alt\"))"
        );
    }

    #[test]
    fn markup_escaping() {
        assert_eq!(render(vec![para(vec![str_inline("a*b_c")])]), "a\\*b\\_c");
        assert_eq!(render(vec![para(vec![str_inline("-x")])]), "\\-x");
        assert_eq!(render(vec![para(vec![str_inline("a-b")])]), "a-b");
        assert_eq!(render(vec![para(vec![str_inline("a---b")])]), "a-\\-\\-b");
        assert_eq!(
            render(vec![para(vec![str_inline("http://a")])]),
            "http:/\\/a"
        );
        assert_eq!(render(vec![para(vec![str_inline("a.b")])]), "a.b");
        assert_eq!(render(vec![para(vec![str_inline(".x")])]), "\\.x");
        assert_eq!(render(vec![para(vec![str_inline("(ab)")])]), "\\(ab)");
    }

    #[test]
    fn period_token_alone_is_not_escaped() {
        assert_eq!(
            render(vec![para(vec![
                Inline::Quoted(QuoteType::SingleQuote, vec![str_inline("hi")]),
                str_inline("."),
            ])]),
            "'hi'."
        );
    }

    #[test]
    fn smart_dashes_spelled_out() {
        assert_eq!(
            render(vec![para(vec![str_inline(
                "en\u{2013}dash em\u{2014}dash"
            )])]),
            "en--dash em---dash"
        );
    }

    #[test]
    fn code_block_with_language() {
        assert_eq!(
            render(vec![Block::CodeBlock(
                Attr {
                    classes: vec!["rust".into()],
                    ..Attr::default()
                },
                "fn x() {}".into(),
            )]),
            "```rust\nfn x() {}\n```"
        );
    }

    fn inline_math(text: &str) -> String {
        render(vec![para(vec![Inline::Math(
            MathType::InlineMath,
            text.into(),
        )])])
    }

    fn display_math(text: &str) -> String {
        render(vec![para(vec![Inline::Math(
            MathType::DisplayMath,
            text.into(),
        )])])
    }

    #[test]
    fn inline_math_translates_to_native_markup() {
        assert_eq!(inline_math("a^2 + b^2 = c^2"), "$a^2 + b^2 = c^2$");
        assert_eq!(inline_math("\\alpha + \\beta"), "$alpha + beta$");
        assert_eq!(inline_math("\\frac{1}{2}"), "$1 / 2$");
        assert_eq!(inline_math("\\mathbb{R}"), "$bb(R)$");
    }

    #[test]
    fn display_math_uses_spaced_delimiters() {
        assert_eq!(
            display_math("\\int_0^1 x \\, dx"),
            "$ integral_0^1 x thin d x $"
        );
    }

    #[test]
    fn untranslatable_inline_math_falls_back_to_escaped_verbatim() {
        // A command with no native form is kept as its source, the TeX delimiters reconstructed and
        // the whole run escaped as ordinary markup text.
        assert_eq!(inline_math("\\unknowncmd"), "\\$\\\\unknowncmd\\$");
        assert_eq!(
            inline_math("\\foo #h _u *s"),
            "\\$\\\\foo \\#h \\_u \\*s\\$"
        );
    }

    #[test]
    fn untranslatable_display_math_uses_double_dollar_verbatim() {
        assert_eq!(display_math("\\unknowncmd"), "\\$\\$\\\\unknowncmd\\$\\$");
    }
}
