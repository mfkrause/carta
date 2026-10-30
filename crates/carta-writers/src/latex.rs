//! Render a LaTeX document fragment without a trailing newline.

use std::fmt::Write as _;
use std::sync::LazyLock;

use carta_ast::{
    Attr, Block, Caption, Document, Inline, ListAttributes, ListNumberDelim, ListNumberStyle,
    to_plain_text,
};
use carta_core::{Extension, MetaVarStyle, Result, TocStyle, WrapMode, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, Piece, attribute_value, checkbox_state, list_is_tight, numeral, strip_checkbox,
    wrap_delim,
};

mod code;
mod escaping;
mod inline;
mod tables;

use self::code::code_block;
pub(crate) use self::code::{Hl, code_highlighting};
use self::escaping::{EscapeMode, escape_smart, is_latex_format};
use self::inline::{InlineLayout, push_inlines, trim_number};
use self::tables::render_table;

/// Renders a document to a LaTeX fragment.
#[derive(Debug, Default, Clone, Copy)]
pub struct LatexWriter;

/// Insert the image-sizing definition shared by the article and slide templates.
pub(crate) fn with_image_bounds(template: &str) -> String {
    template.replace(
        "${image-bounds}$",
        r"\newcommand{\pandocbounded}[1]{%
  \begingroup
  \setbox0=\hbox{#1}%
  \ifdim\wd0>\linewidth
    \setbox0=\hbox{\resizebox{\linewidth}{!}{\box0}}%
  \fi
  \ifdim\dimexpr\ht0+\dp0\relax>0.9\textheight
    \resizebox*{!}{0.9\textheight}{\box0}%
  \else
    \box0
  \fi
  \endgroup
}",
    )
}

impl Writer for LatexWriter {
    fn render_document(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let smart = options.extensions.contains(Extension::Smart);
        let hl = code_highlighting(options);
        let body = render_blocks(
            &document.blocks,
            options.columns.unwrap_or(FILL_COLUMN),
            0,
            Dialect::Article,
            options.wrap,
            smart,
            hl,
        );
        Ok(body.trim_end_matches('\n').to_owned())
    }

    fn default_template(&self) -> Option<&'static str> {
        static TEMPLATE: LazyLock<String> =
            LazyLock::new(|| with_image_bounds(include_str!("templates/default.latex")));
        Some(&TEMPLATE)
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

/// Slides use presentation links, footnotes, lists, and tables.
/// `incremental` adds `[<+->]` overlays to lists.
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
    hl: Hl<'_>,
) -> String {
    render_blocks(blocks, FILL_COLUMN, 0, dialect, wrap, smart, hl)
        .trim_end_matches('\n')
        .to_owned()
}

/// Render a section heading above a presentation's slide level.
pub(crate) fn render_heading(
    level: i64,
    attr: &Attr,
    inlines: &[Inline],
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    header(
        level,
        attr,
        inlines,
        FILL_COLUMN,
        Dialect::SLIDE,
        wrap,
        smart,
        hl,
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
    hl: Hl<'_>,
) -> String {
    let mut pieces = InlineLayout::default();
    pieces.push(Piece::text(prefix.to_owned()));
    pieces.append(inline_pieces(
        inlines,
        FILL_COLUMN,
        dialect,
        wrap,
        smart,
        hl,
    ));
    pieces.push(Piece::text("}"));
    pieces.render(FILL_COLUMN, wrap)
}

/// The anchor markup for an element carrying an identifier, exposed for slide-level scaffolding.
pub(crate) fn anchor(id: &str) -> String {
    phantom_label(id)
}

/// Render a block sequence with a blank line between blocks, dropping those that produce no output.
pub(crate) fn render_blocks(
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    blocks
        .iter()
        .map(|block| block_to_string(block, width, enum_depth, dialect, wrap, smart, hl))
        .filter(|rendered| !rendered.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[allow(clippy::too_many_arguments)]
fn block_to_string(
    block: &Block,
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => {
            inlines_to_string(inlines, width, dialect, wrap, smart, hl)
        }
        Block::Header(level, attr, inlines) => {
            header(*level, attr, inlines, width, dialect, wrap, smart, hl)
        }
        Block::CodeBlock(attr, text) => code_block(attr, text, hl),
        Block::RawBlock(format, text) => {
            if is_latex_format(&format.0) {
                text.strip_suffix('\n').unwrap_or(text).to_owned()
            } else {
                String::new()
            }
        }
        Block::BlockQuote(blocks) => {
            block_quote(blocks, width, enum_depth, dialect, wrap, smart, hl)
        }
        Block::BulletList(items) => bullet_list(items, width, enum_depth, dialect, wrap, smart, hl),
        Block::OrderedList(attrs, items) => {
            ordered_list(attrs, items, width, enum_depth, dialect, wrap, smart, hl)
        }
        Block::DefinitionList(items) => {
            definition_list(items, width, enum_depth, dialect, wrap, smart, hl)
        }
        Block::HorizontalRule => {
            "\\begin{center}\\rule{0.5\\linewidth}{0.5pt}\\end{center}".to_owned()
        }
        Block::LineBlock(lines) => line_block(lines, width, dialect, wrap, smart, hl),
        Block::Div(attr, blocks) => div(attr, blocks, width, enum_depth, dialect, wrap, smart, hl),
        Block::Figure(attr, caption, blocks) => figure(
            attr, caption, blocks, width, enum_depth, dialect, wrap, smart, hl,
        ),
        Block::Table(table) => render_table(table, width, dialect, wrap, smart, hl),
    }
}

#[allow(clippy::too_many_arguments)]
fn header(
    level: i64,
    attr: &Attr,
    inlines: &[Inline],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    let command = match level {
        1 => "section",
        2 => "subsection",
        3 => "subsubsection",
        4 => "paragraph",
        5 => "subparagraph",
        _ => return inlines_to_string(inlines, width, dialect, wrap, smart, hl),
    };
    let unnumbered = attr.classes.iter().any(|class| class == "unnumbered");
    let star = if unnumbered { "*" } else { "" };
    let inner = inline_pieces_in(inlines, width, dialect, wrap, smart, true, hl);

    let mut content = InlineLayout::default();
    content.push(Piece::text(format!("\\{command}{star}")));
    if let Some(short) = short_title(inlines, width, dialect, wrap, smart, hl) {
        content.push(Piece::text(format!("[{short}]")));
    }
    content.push(Piece::text("{"));
    if needs_texorpdfstring(inlines) {
        content.push(Piece::text("\\texorpdfstring{"));
        content.append(inner.clone());
        let pdf = escape_smart(&to_plain_text(inlines), EscapeMode::Text, smart);
        content.push(Piece::text(format!("}}{{{pdf}}}")));
    } else {
        content.append(inner.clone());
    }
    content.push(Piece::text("}"));
    if !attr.id.is_empty() {
        content.push(Piece::text(format!("\\label{{{}}}", to_label(&attr.id))));
    }
    let heading = content.render(width, wrap);

    if unnumbered {
        let mut toc = InlineLayout::default();
        toc.push(Piece::text(format!(
            "\\addcontentsline{{toc}}{{{command}}}{{"
        )));
        toc.append(inner);
        toc.push(Piece::text("}"));
        format!("{heading}\n{}", toc.render(width, wrap))
    } else {
        heading
    }
}

/// Render a `Div`. A plain div emits its body under an optional anchor; in the slide dialect a few
/// recognized classes map to presentation constructs: a column layout (`columns` with `column`
/// children), and incremental regions (`incremental` / `nonincremental`) that toggle whether the
/// lists inside reveal their items one at a time.
#[allow(clippy::too_many_arguments)]
fn div(
    attr: &Attr,
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    if dialect.is_slide() {
        if has_class(attr, "columns") {
            return columns(attr, blocks, width, enum_depth, dialect, wrap, smart, hl);
        }
        if has_class(attr, "incremental") {
            return render_blocks(
                blocks,
                width,
                enum_depth,
                dialect.with_incremental(true),
                wrap,
                smart,
                hl,
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
                hl,
            );
        }
    }
    let body = render_blocks(blocks, width, enum_depth, dialect, wrap, smart, hl);
    if attr.id.is_empty() {
        body
    } else {
        format!("{}\n{body}", phantom_label(&attr.id))
    }
}

/// Render a `columns` div as a `columns` environment whose `column` children become sized `column`
/// boxes. The environment's vertical alignment comes from the div's `align` attribute (`top`→`T`,
/// `bottom`→`b`, `center`→`c`, defaulting to `T`); a `totalwidth` attribute is carried through.
#[allow(clippy::too_many_arguments)]
fn columns(
    attr: &Attr,
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
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
                    hl,
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
#[allow(clippy::too_many_arguments)]
fn column(
    attr: &Attr,
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
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
    let body = render_blocks(blocks, width, enum_depth, dialect, wrap, smart, hl);
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
#[allow(clippy::too_many_arguments)]
fn block_quote(
    blocks: &[Block],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
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
            hl,
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
            smart,
            hl
        )
    )
}

#[allow(clippy::too_many_arguments)]
fn bullet_list(
    items: &[Vec<Block>],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    let mut lines = vec![format!("\\begin{{itemize}}{}", overlay(dialect))];
    if list_is_tight(items) {
        lines.push("\\tightlist".to_owned());
    }
    for item in items {
        let checked = checkbox_state(item);
        let stripped = checked.map(|_| strip_checkbox(item));
        let marker = match checked {
            Some(false) => "\\item[$\\square$]",
            Some(true) => "\\item[$\\boxtimes$]",
            None => "\\item",
        };
        lines.push(list_item(
            stripped.as_deref().unwrap_or(item),
            marker,
            width,
            enum_depth,
            dialect,
            wrap,
            smart,
            hl,
        ));
    }
    lines.push("\\end{itemize}".to_owned());
    lines.join("\n")
}

#[allow(clippy::too_many_arguments)]
fn ordered_list(
    attrs: &ListAttributes,
    items: &[Vec<Block>],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
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
        lines.push(list_item(
            item, "\\item", width, depth, dialect, wrap, smart, hl,
        ));
    }
    lines.push("\\end{enumerate}".to_owned());
    lines.join("\n")
}

fn indent_list_body(body: &str) -> String {
    const LITERAL_ENVIRONMENTS: [(&str, &str); 5] = [
        ("\\begin{code}", "\\end{code}"),
        ("\\begin{verbatim}", "\\end{verbatim}"),
        ("\\begin{Verbatim}", "\\end{Verbatim}"),
        ("\\begin{lstlisting}", "\\end{lstlisting}"),
        ("\\begin{Highlighting}", "\\end{Highlighting}"),
    ];
    let mut closing = None;
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if closing.is_none() {
            closing = LITERAL_ENVIRONMENTS
                .iter()
                .find_map(|(open, close)| trimmed.starts_with(open).then_some(*close));
        }
        if closing.is_none() && (index == 0 || !line.is_empty()) {
            out.push_str("  ");
        }
        out.push_str(line);
        if closing.is_some_and(|close| trimmed.starts_with(close)) {
            closing = None;
        }
    }
    out
}

/// Render a list item with two-column indentation outside literal code environments.
#[allow(clippy::too_many_arguments)]
fn list_item(
    item: &[Block],
    marker: &str,
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    let body = render_blocks(
        item,
        width.saturating_sub(2),
        enum_depth,
        dialect,
        wrap,
        smart,
        hl,
    );
    if body.is_empty() {
        marker.to_owned()
    } else {
        format!("{marker}\n{}", indent_list_body(&body))
    }
}

#[allow(clippy::too_many_arguments)]
fn definition_list(
    items: &[(Vec<Inline>, Vec<Vec<Block>>)],
    width: usize,
    enum_depth: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    let mut lines = vec![format!("\\begin{{description}}{}", overlay(dialect))];
    if is_tight_definitions(items) {
        lines.push("\\tightlist".to_owned());
    }
    for (term, definitions) in items {
        let header = format!(
            "\\item[{}]",
            inlines_to_string(term, width, dialect, wrap, smart, hl)
        );
        let mut bodies: Vec<String> = definitions
            .iter()
            .map(|definition| {
                render_blocks(definition, width, enum_depth, dialect, wrap, smart, hl)
            })
            .filter(|rendered| !rendered.is_empty())
            .collect();
        if let Some(environment) = definition_list_environment(definitions)
            && let Some(body) = bodies.first_mut()
        {
            *body = body.replacen(environment, &format!("\\hfill\n{environment}"), 1);
        }
        if bodies.is_empty() {
            lines.push(header);
        } else {
            lines.push(format!("{header}\n{}", bodies.join("\n\n")));
        }
    }
    lines.push("\\end{description}".to_owned());
    lines.join("\n")
}

fn definition_list_environment(definitions: &[Vec<Block>]) -> Option<&'static str> {
    let mut pending: Vec<_> = definitions.iter().flatten().rev().collect();
    while let Some(block) = pending.pop() {
        match block {
            Block::BulletList(_) => return Some("\\begin{itemize}"),
            Block::OrderedList(..) => return Some("\\begin{enumerate}"),
            Block::BlockQuote(children) | Block::Div(_, children) => {
                pending.extend(children.iter().rev());
            }
            Block::Plain(inlines) | Block::Para(inlines) if inlines.is_empty() => {}
            Block::RawBlock(format, text) if !is_latex_format(&format.0) || text.is_empty() => {}
            _ => return None,
        }
    }
    None
}

/// Render a line block, breaking each line with `\\`. An empty line needs a placeholder so the break
/// has content to act on: while only empty lines have been seen it is `\hfill\break` (which both fills
/// the line and breaks it, so no `\\` follows); once real content has appeared it is `\strut`, which
/// gives the otherwise-blank line its height ahead of the `\\`. A trailing empty line contributes
/// nothing beyond the break already closing the previous line. A line whose own content opens on a
/// non-breaking space takes the same strut ahead of it.
fn line_block(
    lines: &[Vec<Inline>],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> String {
    let mut out = String::new();
    let mut only_empty_so_far = true;
    for (index, line) in lines.iter().enumerate() {
        let is_last = index + 1 == lines.len();
        if !line.is_empty() {
            let mut pieces = inline_pieces(line, width, dialect, wrap, smart, hl);
            if !is_last {
                if line_needs_strut(line) {
                    pieces.push(Piece::text("\\strut "));
                }
                pieces.push(Piece::text("\\\\"));
            }
            let rendered = pieces.render(width, wrap);
            // A line opening on a non-breaking space carries no glyph to set its height.
            if !only_empty_so_far && rendered.starts_with('~') {
                out.push_str("\\strut ");
            }
            out.push_str(&rendered);
            only_empty_so_far = false;
        } else if is_last {
        } else if only_empty_so_far {
            out.push_str("\\hfill\\break");
        } else {
            out.push_str("\\strut \\\\");
        }
        if !is_last {
            out.push('\n');
        }
    }
    out
}

fn line_needs_strut(line: &[Inline]) -> bool {
    let mut pending = vec![line];
    while let Some(inlines) = pending.pop() {
        for inline in inlines {
            match inline {
                Inline::Code(..) | Inline::Space | Inline::SoftBreak => {}
                Inline::Emph(content)
                | Inline::Strong(content)
                | Inline::Underline(content)
                | Inline::Strikeout(content)
                | Inline::Superscript(content)
                | Inline::Subscript(content)
                | Inline::SmallCaps(content)
                | Inline::Quoted(_, content)
                | Inline::Cite(_, content)
                | Inline::Link(_, content, _)
                | Inline::Span(_, content) => pending.push(content),
                Inline::RawInline(format, _) if !is_latex_format(&format.0) => {}
                _ => return false,
            }
        }
    }
    true
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
    hl: Hl<'_>,
) -> String {
    let mut parts = vec![
        "\\begin{figure}".to_owned(),
        "\\centering".to_owned(),
        render_blocks(blocks, width, enum_depth, dialect, wrap, smart, hl),
    ];
    let caption_body = caption_body(caption, width, dialect, wrap, smart, hl);
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
    hl: Hl<'_>,
) -> String {
    caption
        .long
        .iter()
        .filter_map(|block| match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                Some(inlines_to_string(inlines, width, dialect, wrap, smart, hl))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\\\\\n")
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
/// for the two spellings of the plain default, `(DefaultStyle, DefaultDelim)` and
/// `(Decimal, Period)`, where the environment's built-in `1.` label already applies. The template
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
/// survive a section's movable argument (a footnote, an image, or an element with an identifier
/// anchor), those inlines are dropped and the remainder rendered as a short title. `None` when the
/// heading carries no such inline and so needs no short title.
fn short_title(
    inlines: &[Inline],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> Option<String> {
    let visible: Vec<Inline> = inlines
        .iter()
        .filter(|inline| !contains_fragile(inline))
        .cloned()
        .collect();
    if visible.len() == inlines.len() || visible.is_empty() {
        return None;
    }
    Some(inline_pieces(&visible, width, dialect, wrap, smart, hl).flatten())
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
    hl: Hl<'_>,
) -> String {
    inline_pieces(inlines, width, dialect, wrap, smart, hl).render(width, wrap)
}

fn inline_pieces(
    inlines: &[Inline],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    hl: Hl<'_>,
) -> InlineLayout {
    inline_pieces_in(inlines, width, dialect, wrap, smart, false, hl)
}

/// Render an inline list to pieces. `in_header` is set while rendering a heading's content, where an
/// identifier anchor is emitted as a `\hypertarget` rather than a `\phantomsection\label`.
#[allow(clippy::too_many_arguments)]
fn inline_pieces_in(
    inlines: &[Inline],
    width: usize,
    dialect: Dialect,
    wrap: WrapMode,
    smart: bool,
    in_header: bool,
    hl: Hl<'_>,
) -> InlineLayout {
    let mut out = InlineLayout::default();
    push_inlines(
        inlines, &mut out, width, dialect, wrap, smart, in_header, false, hl,
    );
    out
}

#[cfg(test)]
mod tests;
