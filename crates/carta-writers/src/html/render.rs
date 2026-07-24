//! Block and inline serialization: the `State` walker that renders the document tree.

use std::fmt::Write as _;

#[cfg(feature = "highlight")]
use carta_ast::Text;
use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, Inline, ListAttributes, ListNumberStyle, MathType, Row,
    Table, TableBody, Target,
};
#[cfg(feature = "highlight")]
use carta_highlight::{SourceLine, Token};

use crate::common::{RowSpanGrid, quote_marks};
#[cfg(feature = "highlight")]
use crate::highlight::{is_number_lines_class, plain_source_lines, start_line};

use super::helpers::{
    alignment_style, append_trailing_newline, cell_attr, cell_attr_html4, checkbox_state, colgroup,
    header_tag, heading_attr_html4, image, is_implicit_figure, list_style_type, ordered_list_type,
    raw_passthrough, render_attr_into, table_width_style, title_attr,
};
#[cfg(feature = "highlight")]
use super::helpers::{render_id_into, render_keyvals_into};
use super::{
    AttrOrder, BREAK, FLUSH, Flavor, MathOutput, SEMANTIC_SPAN_TAGS, SOFT, STACK_RED_ZONE,
    STACK_SEGMENT, State, escape_attr, escape_attr_into, escape_text_into, fill_math,
    fragment_prefix,
};

impl State {
    /// Render a block sequence into `out`, one block per line. A block that renders to nothing (such
    /// as an empty paragraph) contributes neither output nor a separating newline.
    pub(super) fn blocks(&mut self, out: &mut String, blocks: &[Block]) {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            let mut wrote_any = false;
            for block in blocks {
                let checkpoint = out.len();
                if wrote_any {
                    out.push('\n');
                }
                let body_start = out.len();
                self.block(out, block);
                if out.len() == body_start {
                    out.truncate(checkpoint);
                } else {
                    wrote_any = true;
                }
            }
        });
    }

    fn block(&mut self, out: &mut String, block: &Block) {
        match block {
            Block::Plain(inlines) => self.inlines(out, inlines),
            Block::Para(inlines) => {
                if inlines.is_empty() {
                    return;
                }
                out.push_str("<p>");
                self.inlines(out, inlines);
                out.push_str("</p>");
            }
            Block::Header(level, attr, inlines) => {
                let tag = header_tag(*level);
                let _ = write!(out, "<{tag}");
                if self.flavor.is_html5_family() {
                    render_attr_into(out, attr, AttrOrder::Header, self.flavor);
                } else {
                    render_attr_into(
                        out,
                        &heading_attr_html4(attr),
                        AttrOrder::Header,
                        self.flavor,
                    );
                }
                out.push('>');
                self.inlines(out, inlines);
                let _ = write!(out, "</{tag}>");
            }
            Block::CodeBlock(attr, text) => self.code_block(out, attr, text),
            Block::RawBlock(format, text) => out.push_str(&raw_passthrough(&format.0, text)),
            Block::BlockQuote(blocks) => {
                out.push_str("<blockquote>\n");
                self.blocks(out, blocks);
                out.push_str("\n</blockquote>");
            }
            Block::BulletList(items) => self.bullet_list(out, items),
            Block::OrderedList(attrs, items) => self.ordered_list(out, attrs, items),
            Block::DefinitionList(items) => self.definition_list(out, items),
            Block::Div(attr, blocks) => {
                // EPUB 3 promotes a section-class div to `<section>`, consuming the marker class
                let section = self.flavor == Flavor::Epub3
                    && attr.classes.iter().any(|class| class == "section");
                if section {
                    let stripped = Attr {
                        id: attr.id.clone(),
                        classes: attr
                            .classes
                            .iter()
                            .filter(|class| class.as_str() != "section")
                            .cloned()
                            .collect(),
                        attributes: attr.attributes.clone(),
                    };
                    out.push_str("<section");
                    render_attr_into(out, &stripped, AttrOrder::Standard, self.flavor);
                    out.push_str(">\n");
                    self.blocks(out, blocks);
                    out.push_str("\n</section>");
                } else {
                    out.push_str("<div");
                    render_attr_into(out, attr, AttrOrder::Standard, self.flavor);
                    out.push_str(">\n");
                    self.blocks(out, blocks);
                    out.push_str("\n</div>");
                }
            }
            Block::Figure(attr, caption, blocks) => self.figure(out, attr, caption, blocks),
            Block::HorizontalRule => out.push_str("<hr />"),
            Block::LineBlock(lines) => self.line_block(out, lines),
            Block::Table(table) => self.table(out, table),
        }
    }

    /// Render a code block. A block whose class names a known syntax definition (or that requests
    /// line numbering) is colorized inside the `div.sourceCode` scaffolding; anything else stays a
    /// plain `<pre><code>`. Every code block advances the sequence counter so a colorized block
    /// without its own identifier gets a stable `cbN` one, whatever plain blocks precede it.
    fn code_block(&mut self, out: &mut String, attr: &Attr, text: &str) {
        self.code_block_id += 1;
        #[cfg(feature = "highlight")]
        if self.code_block_highlighted(out, attr, text) {
            return;
        }
        out.push_str("<pre");
        render_attr_into(out, attr, AttrOrder::Standard, self.flavor);
        out.push_str("><code>");
        out.push(FLUSH);
        escape_attr_into(out, text);
        out.push_str("</code></pre>");
    }

    /// Emit the colorized form of a code block, returning whether it applied. It does not when
    /// highlighting is off, or when the block neither names a known language nor numbers its lines;
    /// the caller then renders the plain form.
    #[cfg(feature = "highlight")]
    fn code_block_highlighted(&self, out: &mut String, attr: &Attr, text: &str) -> bool {
        let Some(highlighter) = self.highlighter.clone() else {
            return false;
        };
        let numbered = attr.classes.iter().any(is_number_lines_class);
        let language = attr
            .classes
            .iter()
            .find(|class| highlighter.registry().is_known(class.as_str()));
        if language.is_none() && !numbered {
            return false;
        }
        let lines = match language {
            Some(language) => highlighter
                .highlight(language.as_str(), text)
                .unwrap_or_default(),
            None => plain_source_lines(text),
        };
        self.emit_source_block(out, attr, language.map(Text::as_str), numbered, &lines);
        true
    }

    /// Emit the colorized form of inline code, returning whether it applied. It does not when
    /// highlighting is off or the span names no known language, leaving the caller to render the
    /// plain `<code>`. The class list leads with the `sourceCode` marker and the resolved language,
    /// then carries the span's remaining classes; the id and key/value pairs follow.
    #[cfg(feature = "highlight")]
    fn code_inline_highlighted(&self, out: &mut String, attr: &Attr, text: &str) -> bool {
        let Some(highlighter) = self.highlighter.clone() else {
            return false;
        };
        let Some(language) = attr
            .classes
            .iter()
            .find(|class| highlighter.registry().is_known(class.as_str()))
        else {
            return false;
        };
        let lines = highlighter
            .highlight(language.as_str(), text)
            .unwrap_or_default();

        out.push_str("<code");
        out.push(BREAK);
        out.push_str("class=\"sourceCode ");
        escape_attr_into(out, language.as_str());
        for class in &attr.classes {
            if class.is_empty() || std::ptr::eq(class, language) {
                continue;
            }
            out.push(' ');
            escape_attr_into(out, class.as_str());
        }
        out.push('"');
        render_id_into(out, &attr.id);
        render_keyvals_into(out, &attr.attributes, self.flavor);
        out.push('>');

        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            for token in line {
                emit_token(out, token);
            }
        }
        out.push_str("</code>");
        true
    }

    /// Write the `div.sourceCode` / `pre` / `code` scaffolding and the per-line, per-token spans.
    #[cfg(feature = "highlight")]
    fn emit_source_block(
        &self,
        out: &mut String,
        attr: &Attr,
        language: Option<&str>,
        numbered: bool,
        lines: &[SourceLine],
    ) {
        let block_id = if attr.id.is_empty() {
            format!("cb{}", self.code_block_id)
        } else {
            attr.id.as_str().to_owned()
        };
        let block_id_attr = escape_attr(&block_id);
        let start = if numbered { start_line(attr) } else { 1 };

        // the wrapping div carries only the sourceCode class, block id, and key/value pairs
        out.push_str("<div");
        out.push(BREAK);
        out.push_str("class=\"sourceCode\"");
        out.push(BREAK);
        let _ = write!(out, "id=\"{block_id_attr}\"");
        render_keyvals_into(out, &attr.attributes, self.flavor);
        out.push('>');

        // one break point lets the `<pre>` tag wrap; everything after is one unbroken run
        out.push_str("<pre");
        out.push(BREAK);
        out.push_str("class=\"sourceCode");
        if numbered {
            out.push_str(" numberSource");
        }
        for class in &attr.classes {
            if class.is_empty() {
                continue;
            }
            out.push(' ');
            escape_attr_into(out, class.as_str());
        }
        out.push_str("\">");

        out.push_str("<code class=\"sourceCode");
        if let Some(language) = language {
            out.push(' ');
            escape_attr_into(out, language);
        }
        out.push('"');
        if numbered && start != 1 {
            let _ = write!(
                out,
                " style=\"counter-reset: source-line {};\"",
                start.saturating_sub(1)
            );
        }
        out.push('>');

        let anchor = source_anchor_attrs(self.flavor, numbered);
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let number = start.saturating_add(i64::try_from(index).unwrap_or(i64::MAX));
            let _ = write!(
                out,
                "<span id=\"{block_id_attr}-{number}\"><a href=\"#{block_id_attr}-{number}\"{anchor}></a>"
            );
            for token in line {
                emit_token(out, token);
            }
            out.push_str("</span>");
        }

        out.push_str("</code></pre></div>");
    }

    fn bullet_list(&mut self, out: &mut String, items: &[Vec<Block>]) {
        if !items.is_empty() && items.iter().all(|item| checkbox_state(item).is_some()) {
            out.push_str("<ul class=\"task-list\">\n");
        } else {
            out.push_str("<ul>\n");
        }
        self.list_items(out, items);
        out.push_str("\n</ul>");
    }

    fn ordered_list(&mut self, out: &mut String, attrs: &ListAttributes, items: &[Vec<Block>]) {
        out.push_str("<ol");
        if attrs.start != 1 {
            let _ = write!(out, " start=\"{}\"", attrs.start);
        }
        if matches!(attrs.style, ListNumberStyle::Example) {
            out.push_str(" class=\"example\"");
        }
        if self.flavor.is_html5_family() {
            if let Some(kind) = ordered_list_type(attrs.style) {
                let _ = write!(out, " type=\"{kind}\"");
            }
        } else if let Some(name) = list_style_type(attrs.style) {
            let _ = write!(out, " style=\"list-style-type: {name}\"");
        }
        out.push_str(">\n");
        self.list_items(out, items);
        out.push_str("\n</ol>");
    }

    /// Render each list item's blocks (newline-joined, no surrounding padding) wrapped in `<li>`.
    fn list_items(&mut self, out: &mut String, items: &[Vec<Block>]) {
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str("<li>");
            match checkbox_state(item) {
                Some(checked) => self.checkbox_item(out, item, checked),
                None => self.blocks(out, item),
            }
            out.push_str("</li>");
        }
    }

    fn checkbox_item(&mut self, out: &mut String, item: &[Block], checked: bool) {
        let input = if checked {
            "<label><input type=\"checkbox\" checked=\"\" />"
        } else {
            "<label><input type=\"checkbox\" />"
        };
        for (index, block) in item.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            match (index, block) {
                (0, Block::Plain(inlines)) => {
                    out.push_str(input);
                    self.inlines(out, inlines.get(2..).unwrap_or_default());
                    out.push_str("</label>");
                }
                (0, Block::Para(inlines)) => {
                    out.push_str("<p>");
                    out.push_str(input);
                    self.inlines(out, inlines.get(2..).unwrap_or_default());
                    out.push_str("</label></p>");
                }
                _ => self.block(out, block),
            }
        }
    }

    fn definition_list(&mut self, out: &mut String, items: &[(Vec<Inline>, Vec<Vec<Block>>)]) {
        out.push_str("<dl>");
        for (term, definitions) in items {
            out.push_str("\n<dt>");
            self.inlines(out, term);
            out.push_str("</dt>");
            for definition in definitions {
                out.push_str("\n<dd>\n");
                self.blocks(out, definition);
                out.push_str("\n</dd>");
            }
        }
        out.push_str("\n</dl>");
    }

    fn figure(&mut self, out: &mut String, attr: &Attr, caption: &Caption, blocks: &[Block]) {
        if self.flavor.is_html5_family() {
            self.figure_html5(out, attr, caption, blocks);
        } else {
            self.figure_html4(out, attr, caption, blocks);
        }
    }

    fn figure_html5(&mut self, out: &mut String, attr: &Attr, caption: &Caption, blocks: &[Block]) {
        out.push_str("<figure");
        render_attr_into(out, attr, AttrOrder::Standard, self.flavor);
        out.push_str(">\n");
        self.blocks(out, blocks);
        if !caption.long.is_empty() {
            let hidden = if is_implicit_figure(caption, blocks) {
                " aria-hidden=\"true\""
            } else {
                ""
            };
            let _ = write!(out, "\n<figcaption{hidden}>");
            self.blocks(out, &caption.long);
            out.push_str("</figcaption>");
        }
        out.push_str("\n</figure>");
    }

    fn figure_html4(&mut self, out: &mut String, attr: &Attr, caption: &Caption, blocks: &[Block]) {
        out.push_str("<div class=\"float\"");
        render_attr_into(out, attr, AttrOrder::Standard, self.flavor);
        out.push_str(">\n");
        self.blocks(out, blocks);
        if !caption.long.is_empty() {
            out.push_str("\n<div class=\"figcaption\">");
            self.blocks(out, &caption.long);
            out.push_str("</div>");
        }
        out.push_str("\n</div>");
    }

    fn line_block(&mut self, out: &mut String, lines: &[Vec<Inline>]) {
        out.push_str("<div class=\"line-block\">");
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                out.push_str("<br />\n");
            }
            self.inlines(out, line);
        }
        out.push_str("</div>");
    }

    fn table(&mut self, out: &mut String, table: &Table) {
        out.push_str("<table");
        render_attr_into(out, &table.attr, AttrOrder::Standard, self.flavor);
        out.push_str(&table_width_style(&table.col_specs));
        out.push('>');
        if !table.caption.long.is_empty() {
            out.push_str("\n<caption>");
            self.blocks(out, &table.caption.long);
            out.push_str("</caption>");
        }
        let aligns: Vec<Alignment> = table
            .col_specs
            .iter()
            .map(|spec| spec.align.clone())
            .collect();
        out.push_str(&colgroup(&table.col_specs, self.flavor));
        if !table.head.rows.is_empty() {
            out.push_str("\n<thead");
            render_attr_into(out, &table.head.attr, AttrOrder::Standard, self.flavor);
            out.push('>');
            out.push('\n');
            self.rows(out, &table.head.rows, &aligns, true);
            out.push_str("\n</thead>");
        }
        for body in &table.bodies {
            self.table_body(out, body, &aligns);
        }
        if !table.foot.rows.is_empty() {
            // the foot opens directly after `</tbody>`; only a bodiless foot gets its own line
            if table.bodies.is_empty() {
                out.push('\n');
            }
            out.push_str("<tfoot");
            render_attr_into(out, &table.foot.attr, AttrOrder::Standard, self.flavor);
            out.push('>');
            out.push('\n');
            self.rows(out, &table.foot.rows, &aligns, false);
            out.push_str("\n</tfoot>");
        }
        // a table ending without body rows closes after a blank line
        if table.bodies.is_empty() || !table.foot.rows.is_empty() {
            out.push('\n');
        }
        out.push_str("\n</table>");
    }

    fn table_body(&mut self, out: &mut String, body: &TableBody, aligns: &[Alignment]) {
        out.push_str("\n<tbody");
        render_attr_into(out, &body.attr, AttrOrder::Standard, self.flavor);
        out.push('>');
        let mut head_grid = RowSpanGrid::new(aligns.len());
        for row in &body.head {
            out.push('\n');
            self.row(out, row, aligns, true, 0, &mut head_grid);
        }
        // A blank line separates a body's own header rows from the rows that follow.
        if !body.head.is_empty() {
            out.push('\n');
        }
        let mut body_grid = RowSpanGrid::new(aligns.len());
        for row in &body.body {
            out.push('\n');
            self.row(
                out,
                row,
                aligns,
                false,
                body.row_head_columns,
                &mut body_grid,
            );
        }
        out.push_str("\n</tbody>");
    }

    fn rows(&mut self, out: &mut String, rows: &[Row], aligns: &[Alignment], header: bool) {
        let mut grid = RowSpanGrid::new(aligns.len());
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            self.row(out, row, aligns, header, 0, &mut grid);
        }
    }

    fn row(
        &mut self,
        out: &mut String,
        row: &Row,
        aligns: &[Alignment],
        header: bool,
        head_columns: i32,
        grid: &mut RowSpanGrid,
    ) {
        out.push_str("<tr");
        render_attr_into(out, &row.attr, AttrOrder::Standard, self.flavor);
        out.push('>');
        out.push('\n');
        let head_columns = usize::try_from(head_columns).unwrap_or(0);
        for (index, (column, cell)) in grid.place(&row.cells).into_iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            self.cell(
                out,
                cell,
                aligns.get(column),
                header || column < head_columns,
            );
        }
        out.push_str("\n</tr>");
    }

    fn cell(&mut self, out: &mut String, cell: &Cell, col_align: Option<&Alignment>, header: bool) {
        let tag = if header { "th" } else { "td" };
        let effective = match &cell.align {
            Alignment::AlignDefault => col_align.unwrap_or(&Alignment::AlignDefault),
            explicit => explicit,
        };
        let _ = write!(out, "<{tag}");
        if cell.col_span != 1 {
            let _ = write!(out, "{BREAK}colspan=\"{}\"", cell.col_span);
        }
        if cell.row_span != 1 {
            let _ = write!(out, "{BREAK}rowspan=\"{}\"", cell.row_span);
        }
        if self.flavor.is_html5_family() {
            out.push_str(&cell_attr(&cell.attr, alignment_style(effective)));
        } else {
            out.push_str(&cell_attr_html4(&cell.attr, effective, self.flavor));
        }
        out.push('>');
        self.blocks(out, &cell.content);
        let _ = write!(out, "</{tag}>");
    }

    pub(super) fn inlines(&mut self, out: &mut String, inlines: &[Inline]) {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            for inline in inlines {
                self.inline(out, inline);
            }
        });
    }

    fn inline(&mut self, out: &mut String, inline: &Inline) {
        match inline {
            Inline::Str(text) => escape_text_into(out, text),
            Inline::Emph(inlines) => self.wrap(out, "em", inlines),
            Inline::Strong(inlines) => self.wrap(out, "strong", inlines),
            Inline::Strikeout(inlines) => self.wrap(out, "del", inlines),
            Inline::Superscript(inlines) => self.wrap(out, "sup", inlines),
            Inline::Subscript(inlines) => self.wrap(out, "sub", inlines),
            Inline::Underline(inlines) => self.wrap(out, "u", inlines),
            Inline::SmallCaps(inlines) => {
                let _ = write!(out, "<span{BREAK}class=\"smallcaps\">");
                self.inlines(out, inlines);
                out.push_str("</span>");
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_marks(kind);
                out.push(open);
                self.inlines(out, inlines);
                out.push(close);
            }
            Inline::Code(attr, text) => {
                #[cfg(feature = "highlight")]
                if self.code_inline_highlighted(out, attr, text) {
                    return;
                }
                out.push_str("<code");
                render_attr_into(out, attr, AttrOrder::Standard, self.flavor);
                out.push('>');
                escape_text_into(out, text);
                out.push_str("</code>");
            }
            Inline::Space => out.push(BREAK),
            Inline::SoftBreak => out.push(SOFT),
            Inline::LineBreak => out.push_str("<br />\n"),
            Inline::Math(kind, text) => {
                let (class, delimiters) = match kind {
                    MathType::InlineMath => ("inline", ("\\(", "\\)")),
                    MathType::DisplayMath => ("display", ("\\[", "\\]")),
                };
                let (open, close) = match self.math {
                    MathOutput::Delimited => delimiters,
                    MathOutput::Raw => ("", ""),
                };
                let _ = write!(
                    out,
                    "<span{BREAK}class=\"math {class}\">{open}{}{close}</span>",
                    fill_math(text)
                );
            }
            Inline::RawInline(format, text) => out.push_str(&raw_passthrough(&format.0, text)),
            Inline::Link(attr, inlines, target) => self.link(out, attr, inlines, target),
            Inline::Image(attr, inlines, target) => {
                out.push_str(&image(attr, inlines, target, self.flavor));
            }
            Inline::Span(attr, inlines) => self.span(out, attr, inlines),
            Inline::Cite(citations, inlines) => {
                if self.flavor.is_html5_family() {
                    let ids: Vec<&str> = citations
                        .iter()
                        .map(|citation| citation.id.as_str())
                        .collect();
                    let _ = write!(
                        out,
                        "<span class=\"citation\"{BREAK}data-cites=\"{}\">",
                        escape_attr(&ids.join(" "))
                    );
                } else {
                    out.push_str("<span class=\"citation\">");
                }
                self.inlines(out, inlines);
                out.push_str("</span>");
            }
            Inline::Note(blocks) => self.note(out, blocks),
        }
    }

    fn wrap(&mut self, out: &mut String, tag: &str, inlines: &[Inline]) {
        let _ = write!(out, "<{tag}>");
        self.inlines(out, inlines);
        let _ = write!(out, "</{tag}>");
    }

    /// Render a span. A class naming a dedicated HTML element (see [`SEMANTIC_SPAN_TAGS`]) promotes
    /// the span to that element: the first such class becomes the outermost tag and carries the id,
    /// key/value attributes, and any non-semantic classes following it; further semantic classes
    /// nest inside it as bare elements. Classes preceding the first semantic one are dropped. With no
    /// semantic class the span renders as a generic `<span>`.
    fn span(&mut self, out: &mut String, attr: &Attr, inlines: &[Inline]) {
        // the underline class becomes a bare innermost `<u>`; remaining attributes fall to the enclosing element
        let underline = attr.classes.iter().any(|class| class == "underline");
        let stripped;
        let attr = if underline {
            stripped = Attr {
                id: attr.id.clone(),
                classes: attr
                    .classes
                    .iter()
                    .filter(|class| class.as_str() != "underline")
                    .cloned()
                    .collect(),
                attributes: attr.attributes.clone(),
            };
            &stripped
        } else {
            attr
        };

        let first = attr
            .classes
            .iter()
            .position(|class| SEMANTIC_SPAN_TAGS.contains(&class.as_str()));
        let Some(first) = first else {
            // generic `<span>`, unless the consumed underline left nothing to carry: then the bare `<u>` stands alone
            let bare_underline = underline
                && attr.id.is_empty()
                && attr.classes.is_empty()
                && attr.attributes.is_empty();
            if !bare_underline {
                out.push_str("<span");
                render_attr_into(out, attr, AttrOrder::Standard, self.flavor);
                out.push('>');
            }
            if underline {
                out.push_str("<u>");
            }
            self.inlines(out, inlines);
            if underline {
                out.push_str("</u>");
            }
            if !bare_underline {
                out.push_str("</span>");
            }
            return;
        };
        let mut tags = Vec::new();
        let mut remaining = Vec::new();
        for class in attr.classes.iter().skip(first) {
            if SEMANTIC_SPAN_TAGS.contains(&class.as_str()) {
                tags.push(class.as_str());
            } else {
                remaining.insert(0, class.clone());
            }
        }
        let outer = Attr {
            id: attr.id.clone(),
            classes: remaining,
            attributes: attr.attributes.clone(),
        };
        for (index, tag) in tags.iter().enumerate() {
            if index == 0 {
                let _ = write!(out, "<{tag}");
                render_attr_into(out, &outer, AttrOrder::Standard, self.flavor);
                out.push('>');
            } else {
                let _ = write!(out, "<{tag}>");
            }
        }
        if underline {
            out.push_str("<u>");
        }
        self.inlines(out, inlines);
        if underline {
            out.push_str("</u>");
        }
        for tag in tags.iter().rev() {
            let _ = write!(out, "</{tag}>");
        }
    }

    fn link(&mut self, out: &mut String, attr: &Attr, inlines: &[Inline], target: &Target) {
        if self.in_anchor {
            out.push_str("<span");
            render_attr_into(out, attr, AttrOrder::Standard, self.flavor);
            out.push('>');
            self.inlines(out, inlines);
            out.push_str("</span>");
            return;
        }
        out.push_str("<a");
        out.push(BREAK);
        out.push_str("href=\"");
        escape_attr_into(out, &target.url);
        out.push('"');
        render_attr_into(out, attr, AttrOrder::Standard, self.flavor);
        out.push_str(&title_attr(&target.title));
        out.push('>');
        self.in_anchor = true;
        self.inlines(out, inlines);
        self.in_anchor = false;
        out.push_str("</a>");
    }

    fn note(&mut self, out: &mut String, blocks: &[Block]) {
        let number = self.footnotes.len() + 1;
        let prefix = fragment_prefix(self.flavor);
        match self.flavor {
            Flavor::Epub3 => {
                // note: an `<aside>` in the trailing section; reference: a plain noteref link, no superscript
                let mut body = String::new();
                self.blocks(&mut body, blocks);
                self.footnotes.push(format!(
                    "<aside{BREAK}epub:type=\"footnote\"{BREAK}role=\"doc-footnote\"{BREAK}id=\"fn{number}\">\n{body}\n</aside>"
                ));
                let _ = write!(
                    out,
                    "<a{BREAK}href=\"{prefix}fn{number}\"{BREAK}class=\"footnote-ref\"{BREAK}id=\"fnref{number}\"{BREAK}epub:type=\"noteref\"{BREAK}role=\"doc-noteref\">{number}</a>"
                );
            }
            Flavor::Epub2 => {
                // note: a `<div>` opening with a numbered back-reference; reference: a plain link, no superscript
                let backlink = format!(
                    "<a{BREAK}href=\"{prefix}fnref{number}\"{BREAK}class=\"footnote-back\">{number}</a>. "
                );
                let body = self.note_body_epub2(blocks, &backlink);
                self.footnotes
                    .push(format!("<div{BREAK}id=\"fn{number}\">\n{body}\n</div>"));
                let _ = write!(
                    out,
                    "<a{BREAK}href=\"{prefix}fn{number}\"{BREAK}class=\"footnote-ref\"{BREAK}id=\"fnref{number}\">{number}</a>"
                );
            }
            Flavor::Html5 | Flavor::Slides | Flavor::Html4 => {
                let backlink_role = if self.flavor.is_html5_family() {
                    format!("{BREAK}role=\"doc-backlink\"")
                } else {
                    String::new()
                };
                let backlink = format!(
                    "<a{BREAK}href=\"{prefix}fnref{number}\"{BREAK}class=\"footnote-back\"{backlink_role}>\u{21a9}\u{fe0e}</a>"
                );
                let body = self.note_body(blocks, &backlink);
                self.footnotes
                    .push(format!("<li{BREAK}id=\"fn{number}\">{body}</li>"));
                let ref_role = if self.flavor.is_html5_family() {
                    format!("{BREAK}role=\"doc-noteref\"")
                } else {
                    String::new()
                };
                let _ = write!(
                    out,
                    "<a{BREAK}href=\"{prefix}fn{number}\"{BREAK}class=\"footnote-ref\"{BREAK}id=\"fnref{number}\"{ref_role}><sup>{number}</sup></a>"
                );
            }
        }
    }

    /// Render a footnote's blocks, appending the backlink inline after the final block's content
    /// when that block is a paragraph (wrapped in `<p>`) or an unwrapped `Plain`; for any other
    /// trailing block the backlink follows on its own line. The body is returned as its own value
    /// because notes are gathered for a trailing section.
    fn note_body(&mut self, blocks: &[Block], backlink: &str) -> String {
        let mut body = String::new();
        match blocks.split_last() {
            Some((Block::Para(inlines), rest)) => {
                self.blocks(&mut body, rest);
                append_trailing_newline(&mut body);
                body.push_str("<p>");
                self.inlines(&mut body, inlines);
                body.push_str(backlink);
                body.push_str("</p>");
            }
            Some((Block::Plain(inlines), rest)) => {
                self.blocks(&mut body, rest);
                append_trailing_newline(&mut body);
                self.inlines(&mut body, inlines);
                body.push_str(backlink);
            }
            _ => {
                self.blocks(&mut body, blocks);
                append_trailing_newline(&mut body);
                body.push_str(backlink);
            }
        }
        body
    }

    /// Render an EPUB 2 footnote's blocks, opening the first paragraph (or plain block) with the
    /// numbered back-reference link; any further blocks follow unchanged. A note that does not begin
    /// with a paragraph gets the back-reference on a line of its own.
    fn note_body_epub2(&mut self, blocks: &[Block], backlink: &str) -> String {
        let mut body = String::new();
        match blocks.split_first() {
            Some((Block::Para(inlines), rest)) => {
                body.push_str("<p>");
                body.push_str(backlink);
                self.inlines(&mut body, inlines);
                body.push_str("</p>");
                if !rest.is_empty() {
                    body.push('\n');
                    self.blocks(&mut body, rest);
                }
            }
            Some((Block::Plain(inlines), rest)) => {
                body.push_str(backlink);
                self.inlines(&mut body, inlines);
                if !rest.is_empty() {
                    body.push('\n');
                    self.blocks(&mut body, rest);
                }
            }
            _ => {
                let _ = writeln!(body, "<p>{}</p>", backlink.trim_end());
                self.blocks(&mut body, blocks);
            }
        }
        body
    }

    pub(super) fn push_footnote_section(&self, out: &mut String) {
        if self.footnotes.is_empty() {
            return;
        }
        match self.flavor {
            Flavor::Html5 | Flavor::Slides => {
                let _ = write!(
                    out,
                    "\n<section{BREAK}id=\"footnotes\"{BREAK}class=\"footnotes footnotes-end-of-document\"{BREAK}role=\"doc-endnotes\">\n<hr />\n<ol>\n"
                );
            }
            Flavor::Html4 => {
                let _ = write!(
                    out,
                    "\n<div{BREAK}class=\"footnotes footnotes-end-of-document\">\n<hr />\n<ol>\n"
                );
            }
            Flavor::Epub3 => {
                let _ = write!(
                    out,
                    "\n<section{BREAK}id=\"footnotes\"{BREAK}class=\"footnotes footnotes-end-of-document\"{BREAK}epub:type=\"footnotes\">\n<hr />\n"
                );
            }
            Flavor::Epub2 => {
                let _ = write!(
                    out,
                    "\n<div{BREAK}class=\"footnotes footnotes-end-of-document\">\n<hr />\n"
                );
            }
        }
        for (index, note) in self.footnotes.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(note);
        }
        let close = match self.flavor {
            Flavor::Html5 | Flavor::Slides => "\n</ol>\n</section>",
            Flavor::Html4 => "\n</ol>\n</div>",
            Flavor::Epub3 => "\n</section>",
            Flavor::Epub2 => "\n</div>",
        };
        out.push_str(close);
    }
}

/// The attributes on a line's anchor. A numbered line's anchor carries none (its number is drawn by
/// the stylesheet); an unnumbered line's anchor is hidden from assistive technology and taken out of
/// the tab order, dropping the `aria-hidden` half in the presentational dialect that lacks it.
#[cfg(feature = "highlight")]
fn source_anchor_attrs(flavor: Flavor, numbered: bool) -> &'static str {
    if numbered {
        ""
    } else if flavor.is_html5_family() {
        " aria-hidden=\"true\" tabindex=\"-1\""
    } else {
        " tabindex=\"-1\""
    }
}

/// Write one classified token: an unclassified run as bare escaped text, any other kind wrapped in a
/// class-tagged span the stylesheet colors.
#[cfg(feature = "highlight")]
fn emit_token(out: &mut String, token: &Token) {
    let class = token.kind.html_class();
    if class.is_empty() {
        escape_attr_into(out, &token.text);
    } else {
        let _ = write!(out, "<span class=\"{class}\">");
        escape_attr_into(out, &token.text);
        out.push_str("</span>");
    }
}
