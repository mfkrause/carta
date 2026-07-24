//! Block-level rendering for the ODT writer.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MetaValue, Row, Table, Text,
};
use carta_core::container::xml::escape_attribute;

use super::helpers::{
    collect_caption_runs, column_letter, custom_style, inlines_are_empty, is_block_math,
    is_opendocument, is_tight, leads_with_plain, table_column_count, trim_flanking_spacing,
};
use super::meta::{abstract_paragraphs, meta_authors, meta_inlines};
use super::styles::{delim_fixes, num_format};
use super::{AlignKind, Builder, ParaStyleKey, STACK_RED_ZONE, STACK_SEGMENT};

impl Builder<'_> {
    /// Renders a block sequence. `fixed` names a paragraph style imposed on the sequence's direct
    /// paragraphs (a list item, a cell, a blockquote); `None` selects the flowing body styles.
    pub(super) fn render_blocks(&mut self, blocks: &[Block], fixed: Option<&str>) {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            for block in blocks {
                self.render_block(block, fixed);
            }
        });
    }

    fn render_block(&mut self, block: &Block, fixed: Option<&str>) {
        match block {
            Block::Para(inlines) | Block::Plain(inlines) => self.paragraph_like(inlines, fixed),
            Block::LineBlock(lines) => self.line_block_like(lines, fixed),
            Block::Div(attr, inner) => self.div(attr, inner, fixed),
            other => {
                self.block_other(other);
                self.first_para = true;
            }
        }
    }

    /// A paragraph node: styled with the flowing first/body distinction, or with an imposed style.
    fn paragraph_like(&mut self, inlines: &[Inline], fixed: Option<&str>) {
        if inlines.iter().any(is_block_math) {
            self.paragraphs_split_on_block_math(inlines);
            return;
        }
        if let Some(style) = fixed {
            self.paragraph(style, inlines);
        } else if !inlines_are_empty(inlines) || self.keep_empty {
            self.flowing_paragraph(inlines);
        }
    }

    /// A paragraph in the flowing body styles: `First_20_paragraph` when it opens a run of body text,
    /// `Text_20_body` otherwise. The style is read after the content renders, so a footnote whose body
    /// carries a block element can mark this as the paragraph that anchors it.
    fn flowing_paragraph(&mut self, inlines: &[Inline]) {
        let start = self.body.len();
        self.inlines(inlines);
        let style = self.flowing_style();
        self.body
            .insert_str(start, &format!("<text:p text:style-name=\"{style}\">"));
        self.body.push_str("</text:p>");
    }

    /// The flowing body style for the paragraph now opening: `First_20_paragraph` when it leads a run
    /// of body text, `Text_20_body` after. Reading it clears the first-of-run flag.
    fn flowing_style(&mut self) -> &'static str {
        let style = if self.first_para {
            "First_20_paragraph"
        } else {
            "Text_20_body"
        };
        self.first_para = false;
        style
    }

    /// Renders a paragraph whose inlines carry a display formula. A display formula stands alone in
    /// the text flow, so the run is broken at every formula boundary: the text on either side becomes
    /// its own paragraph (with the spacing that flanked the formula trimmed away, and an all-spacing
    /// remainder dropped) and each formula, or a cluster of formulas set directly against one
    /// another, its own. Every piece takes the flowing body styles regardless of the surrounding
    /// block style.
    fn paragraphs_split_on_block_math(&mut self, inlines: &[Inline]) {
        let mut index = 0;
        while index < inlines.len() {
            let math_run = inlines.get(index).is_some_and(is_block_math);
            let start = index;
            while inlines
                .get(index)
                .is_some_and(|inline| is_block_math(inline) == math_run)
            {
                index += 1;
            }
            let segment = inlines.get(start..index).unwrap_or_default();
            if math_run {
                self.flowing_paragraph(segment);
            } else {
                let text = trim_flanking_spacing(segment);
                if !inlines_are_empty(text) {
                    self.flowing_paragraph(text);
                }
            }
        }
    }

    /// A line block: one paragraph whose source line divisions become hard breaks.
    fn line_block_like(&mut self, lines: &[Vec<Inline>], fixed: Option<&str>) {
        let style = match fixed {
            Some(style) => style,
            None => self.flowing_style(),
        };
        self.body.push_str("<text:p text:style-name=\"");
        self.body.push_str(style);
        self.body.push_str("\">");
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                self.body.push_str("<text:line-break />");
            }
            self.inlines(line);
        }
        self.body.push_str("</text:p>");
    }

    /// A generic container: transparent to the flow, wrapped in a section when it carries an id, and
    /// re-styling its direct paragraphs when it carries a recognized `custom-style`.
    fn div(&mut self, attr: &Attr, inner: &[Block], fixed: Option<&str>) {
        let section = !attr.id.is_empty();
        if section {
            self.body.push_str("<text:section text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\">");
        }
        match custom_style(attr) {
            Some(name) => self.render_blocks(inner, Some(name)),
            None => self.render_blocks(inner, fixed),
        }
        if section {
            self.body.push_str("</text:section>");
        }
    }

    /// Renders a block that is neither a paragraph nor a transparent container.
    fn block_other(&mut self, block: &Block) {
        match block {
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines),
            Block::CodeBlock(_, text) => self.code_block(text),
            Block::BlockQuote(blocks) => {
                self.first_para = true;
                self.render_blocks(blocks, Some("Quotations"));
            }
            Block::BulletList(items) => self.bullet_list(items),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items),
            Block::DefinitionList(items) => self.definition_list(items),
            Block::HorizontalRule => {
                self.body
                    .push_str("<text:p text:style-name=\"Horizontal_20_Line\" />");
            }
            Block::Table(table) => self.table(table),
            Block::Figure(_, caption, content) => self.figure(caption, content),
            Block::RawBlock(format, text) => {
                if is_opendocument(format) {
                    self.body.push_str(text);
                }
            }
            // Already handled before dispatch; these arms keep the match total without a panicking
            // fallback.
            Block::Para(inlines) | Block::Plain(inlines) => self.paragraph_like(inlines, None),
            Block::LineBlock(lines) => self.line_block_like(lines, None),
            Block::Div(attr, inner) => self.div(attr, inner, None),
        }
    }

    fn header(&mut self, level: i32, attr: &Attr, inlines: &[Inline]) {
        let style_level = level.clamp(1, 6);
        let outline = level.max(1);
        let _ = write!(
            self.body,
            "<text:h text:style-name=\"Heading_20_{style_level}\" text:outline-level=\"{outline}\">"
        );
        if !attr.id.is_empty() {
            self.body.push_str("<text:bookmark-start text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\" />");
        }
        self.inlines(inlines);
        if !attr.id.is_empty() {
            self.body.push_str("<text:bookmark-end text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\" />");
        }
        self.body.push_str("</text:h>");
    }

    fn code_block(&mut self, text: &str) {
        let trimmed = text.strip_suffix('\n').unwrap_or(text);
        for line in trimmed.split('\n') {
            self.body
                .push_str("<text:p text:style-name=\"Preformatted_20_Text\">");
            self.push_verbatim(line);
            self.body.push_str("</text:p>");
        }
    }

    fn bullet_list(&mut self, items: &[Vec<Block>]) {
        let item_style = if is_tight(items) {
            "List_20_Bullet_20_Tight"
        } else {
            "List_20_Bullet"
        };
        self.body
            .push_str("<text:list text:style-name=\"List_20_1\">");
        self.first_para = true;
        self.list_items(items, item_style);
        self.body.push_str("</text:list>");
    }

    fn ordered_list(&mut self, attrs: &ListAttributes, items: &[Vec<Block>]) {
        let item_style = if is_tight(items) {
            "List_20_Number_20_Tight"
        } else {
            "List_20_Number"
        };
        let use_builtin = matches!(
            attrs.style,
            ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal
        ) && matches!(
            attrs.delim,
            ListNumberDelim::DefaultDelim | ListNumberDelim::Period
        );
        let list_style = if use_builtin {
            "Numbering_20_1".to_string()
        } else {
            self.auto_list_style(attrs.style, attrs.delim)
        };
        self.body.push_str("<text:list text:style-name=\"");
        escape_attribute(&list_style, &mut self.body);
        if attrs.start != 1 {
            let _ = write!(self.body, "\" text:start-value=\"{}", attrs.start);
        }
        self.body.push_str("\">");
        self.first_para = true;
        self.list_items(items, item_style);
        self.body.push_str("</text:list>");
    }

    fn list_items(&mut self, items: &[Vec<Block>], item_style: &str) {
        for item in items {
            self.body.push_str("<text:list-item>");
            let checkpoint = self.body.len();
            self.render_blocks(item, Some(item_style));
            self.close_or_self_close(checkpoint, "</text:list-item>");
        }
    }

    /// Closes the element whose start tag ends just before `checkpoint`: when nothing was written
    /// since, the start tag's `>` is rewritten to `/>` for an empty element; otherwise `close_tag`
    /// ends it. `checkpoint` is the body length captured immediately after that `>`.
    fn close_or_self_close(&mut self, checkpoint: usize, close_tag: &str) {
        if self.body.len() == checkpoint {
            self.body.truncate(checkpoint - 1);
            self.body.push_str("/>");
        } else {
            self.body.push_str(close_tag);
        }
    }

    fn definition_list(&mut self, items: &[(Vec<Inline>, Vec<Vec<Block>>)]) {
        let tight = items
            .first()
            .and_then(|(_, defs)| defs.first())
            .map(Vec::as_slice)
            .is_none_or(leads_with_plain);
        let (term_style, def_style) = if tight {
            (
                "Definition_20_Term_20_Tight",
                "Definition_20_Definition_20_Tight",
            )
        } else {
            ("Definition_20_Term", "Definition_20_Definition")
        };
        for (term, defs) in items {
            self.paragraph(term_style, term);
            for def in defs {
                let checkpoint = self.body.len();
                self.render_blocks(def, Some(def_style));
                if self.body.len() == checkpoint {
                    self.paragraph(def_style, &[]);
                }
            }
        }
    }

    fn figure(&mut self, caption: &Caption, content: &[Block]) {
        self.render_blocks(content, Some("FigureWithCaption"));
        if !caption.long.is_empty() {
            self.caption_block("FigureCaption", &caption.long);
        }
    }

    /// Renders a caption's blocks as a single paragraph, joining its paragraph-level runs with hard
    /// line breaks so a multi-paragraph caption reads as one caption line.
    fn caption_block(&mut self, style: &str, blocks: &[Block]) {
        let mut runs: Vec<&[Inline]> = Vec::new();
        collect_caption_runs(blocks, &mut runs);
        self.body.push_str("<text:p text:style-name=\"");
        self.body.push_str(style);
        self.body.push_str("\">");
        for (index, inlines) in runs.iter().enumerate() {
            if index > 0 {
                self.body.push_str("<text:line-break />");
            }
            self.inlines(inlines);
        }
        self.body.push_str("</text:p>");
    }

    fn table(&mut self, table: &Table) {
        if !table.caption.long.is_empty() {
            self.caption_block("TableCaption", &table.caption.long);
        }
        self.table_index += 1;
        let n = self.table_index;
        let columns = table_column_count(table);
        let has_widths = table
            .col_specs
            .iter()
            .any(|spec| matches!(spec.width, ColWidth::ColWidth(_)));
        self.push_table_style(n, columns, &table.col_specs, has_widths);

        let _ = write!(
            self.body,
            "<table:table table:name=\"Table{n}\" table:style-name=\"Table{n}\">"
        );
        for column in 0..columns {
            let _ = write!(
                self.body,
                "<table:table-column table:style-name=\"Table{n}.{}\" />",
                column_letter(column)
            );
        }

        let mut covered = vec![0usize; columns];
        if !table.head.rows.is_empty() {
            self.body.push_str("<table:table-header-rows>");
            self.table_rows(
                &table.head.rows,
                true,
                columns,
                columns,
                &table.col_specs,
                &mut covered,
            );
            self.body.push_str("</table:table-header-rows>");
        }
        for section in &table.bodies {
            let head_columns = usize::try_from(section.row_head_columns).unwrap_or(0);
            self.table_rows(
                &section.head,
                false,
                columns,
                columns,
                &table.col_specs,
                &mut covered,
            );
            self.table_rows(
                &section.body,
                false,
                head_columns,
                columns,
                &table.col_specs,
                &mut covered,
            );
        }
        self.table_rows(
            &table.foot.rows,
            false,
            0,
            columns,
            &table.col_specs,
            &mut covered,
        );
        self.body.push_str("</table:table>");
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn push_table_style(&mut self, n: usize, columns: usize, specs: &[ColSpec], has_widths: bool) {
        let _ = write!(
            self.table_styles,
            "<style:style style:name=\"Table{n}\" style:family=\"table\">\
             <style:table-properties table:align=\"center\""
        );
        if has_widths {
            let total: f64 = specs
                .iter()
                .map(|spec| match spec.width {
                    ColWidth::ColWidth(fraction) => fraction,
                    ColWidth::ColWidthDefault => 0.0,
                })
                .sum();
            let percent = (total * 100.0).round() as i64;
            let _ = write!(self.table_styles, " style:rel-width=\"{percent}%\"");
        }
        self.table_styles.push_str(" /></style:style>");

        for column in 0..columns {
            let letter = column_letter(column);
            if has_widths {
                let fraction = match specs.get(column).map(|spec| &spec.width) {
                    Some(ColWidth::ColWidth(value)) => *value,
                    _ => 0.0,
                };
                let relative = (fraction * 65535.0) as u64;
                let _ = write!(
                    self.table_styles,
                    "<style:style style:name=\"Table{n}.{letter}\" style:family=\"table-column\">\
                     <style:table-column-properties style:rel-column-width=\"{relative}*\" /></style:style>"
                );
            } else {
                let _ = write!(
                    self.table_styles,
                    "<style:style style:name=\"Table{n}.{letter}\" style:family=\"table-column\" />"
                );
            }
        }
    }

    #[allow(clippy::cast_sign_loss)]
    fn table_rows(
        &mut self,
        rows: &[Row],
        cell_header: bool,
        head_columns: usize,
        columns: usize,
        specs: &[ColSpec],
        covered: &mut [usize],
    ) {
        for row in rows {
            self.body.push_str("<table:table-row>");
            let mut cells = row.cells.iter();
            let mut column = 0usize;
            while column < columns {
                if let Some(remaining) = covered.get_mut(column)
                    && *remaining > 0
                {
                    *remaining -= 1;
                    column += 1;
                    continue;
                }
                if let Some(cell) = cells.next() {
                    let span = (cell.col_span.max(1) as usize).min(columns - column);
                    let rows_spanned = cell.row_span.max(1) as usize;
                    let para_header = column < head_columns;
                    self.emit_cell(
                        cell,
                        cell_header,
                        para_header,
                        column,
                        specs,
                        span,
                        rows_spanned,
                    );
                    if rows_spanned > 1 {
                        for offset in 0..span {
                            if let Some(slot) = covered.get_mut(column + offset) {
                                *slot = rows_spanned - 1;
                            }
                        }
                    }
                    column += span;
                } else {
                    self.emit_empty_cell(cell_header);
                    column += 1;
                }
            }
            self.body.push_str("</table:table-row>");
        }
    }

    // All distinct inputs to one emission; bundling them would only add indirection.
    #[allow(clippy::too_many_arguments)]
    fn emit_cell(
        &mut self,
        cell: &Cell,
        cell_header: bool,
        para_header: bool,
        column: usize,
        specs: &[ColSpec],
        span: usize,
        rows_spanned: usize,
    ) {
        let para_style = self.cell_paragraph_style(para_header, column, specs, &cell.align);
        let cell_style = if cell_header {
            "TableHeaderRowCell"
        } else {
            "TableRowCell"
        };
        let _ = write!(
            self.body,
            "<table:table-cell table:style-name=\"{cell_style}\" office:value-type=\"string\""
        );
        if span > 1 {
            let _ = write!(self.body, " table:number-columns-spanned=\"{span}\"");
        }
        if rows_spanned > 1 {
            let _ = write!(self.body, " table:number-rows-spanned=\"{rows_spanned}\"");
        }
        self.body.push('>');
        let checkpoint = self.body.len();
        self.render_blocks(&cell.content, Some(&para_style));
        self.close_or_self_close(checkpoint, "</table:table-cell>");
    }

    fn emit_empty_cell(&mut self, cell_header: bool) {
        let cell_style = if cell_header {
            "TableHeaderRowCell"
        } else {
            "TableRowCell"
        };
        let _ = write!(
            self.body,
            "<table:table-cell table:style-name=\"{cell_style}\" office:value-type=\"string\" />"
        );
    }

    /// The paragraph style a cell's content takes, folding the cell's own alignment over the
    /// column's default and mapping centered and trailing alignment to automatic styles.
    fn cell_paragraph_style(
        &mut self,
        para_header: bool,
        column: usize,
        specs: &[ColSpec],
        cell_align: &Alignment,
    ) -> String {
        let column_default = Alignment::AlignDefault;
        let effective = match cell_align {
            Alignment::AlignDefault => specs
                .get(column)
                .map_or(&column_default, |spec| &spec.align),
            other => other,
        };
        let base = if para_header {
            "Table_20_Heading"
        } else {
            "Table_20_Contents"
        };
        match effective {
            Alignment::AlignCenter => self.align_style(para_header, AlignKind::Center),
            Alignment::AlignRight => self.align_style(para_header, AlignKind::Right),
            _ => base.to_string(),
        }
    }

    /// The name of the automatic paragraph style realizing an alignment over a table base style,
    /// registering it on first use.
    fn align_style(&mut self, para_header: bool, kind: AlignKind) -> String {
        let key = ParaStyleKey {
            header: para_header,
            align: kind,
        };
        if let Some(index) = self
            .para_styles
            .iter()
            .position(|existing| *existing == key)
        {
            return format!("P{}", index + 1);
        }
        self.para_styles.push(key);
        format!("P{}", self.para_styles.len())
    }

    /// Builds and records an automatic numbered-list style for a non-default numbering, returning
    /// its name.
    fn auto_list_style(&mut self, style: ListNumberStyle, delim: ListNumberDelim) -> String {
        self.list_auto_index += 1;
        let name = format!("L{}", self.list_auto_index);
        let format = num_format(style);
        let (prefix, suffix) = delim_fixes(delim);
        let mut out = format!("<text:list-style style:name=\"{name}\">");
        for level in 1..=10 {
            let space = format!("{:.4}in", f64::from(level - 1) * 0.1972);
            let _ = write!(
                out,
                "<text:list-level-style-number text:level=\"{level}\" \
                 text:style-name=\"Numbering_20_Symbols\" style:num-format=\"{format}\""
            );
            if let Some(prefix) = prefix {
                let _ = write!(out, " style:num-prefix=\"{prefix}\"");
            }
            let _ = write!(
                out,
                " style:num-suffix=\"{suffix}\">\
                 <style:list-level-properties text:space-before=\"{space}\" \
                 text:min-label-width=\"0.1965in\" text:min-label-distance=\"0.1in\" />\
                 </text:list-level-style-number>"
            );
        }
        out.push_str("</text:list-style>");
        self.list_styles.push_str(&out);
        name
    }

    pub(super) fn title_block(&mut self, meta: &BTreeMap<Text, MetaValue>) {
        if let Some(inlines) = meta_inlines(meta, "title") {
            self.paragraph("Title", &inlines);
        }
        if let Some(inlines) = meta_inlines(meta, "subtitle") {
            self.paragraph("Subtitle", &inlines);
        }
        for author in meta_authors(meta) {
            self.paragraph("Author", &author);
        }
        if let Some(inlines) = meta_inlines(meta, "date") {
            self.paragraph("Date", &inlines);
        }
    }

    /// The abstract, a bare run standing after the title block, its paragraphs separated by line
    /// breaks. The run is a text node directly in the body, so the surrounding newlines are part of
    /// its adjacent text and set it off on its own line.
    pub(super) fn abstract_block(&mut self, meta: &BTreeMap<Text, MetaValue>) {
        let Some(value) = meta.get("abstract") else {
            return;
        };
        let paragraphs = abstract_paragraphs(value);
        if paragraphs.is_empty() {
            return;
        }
        self.body.push('\n');
        for (index, inlines) in paragraphs.iter().enumerate() {
            if index > 0 {
                self.body.push_str("<text:line-break />");
            }
            self.inlines(inlines);
        }
        self.body.push('\n');
    }

    /// A table-of-contents field whose entries the reader regenerates on open.
    pub(super) fn table_of_contents(&mut self) {
        let depth = self.options.toc_depth.unwrap_or(3);
        self.body
            .push_str("<text:table-of-content text:name=\"Table of Contents1\">");
        let _ = write!(
            self.body,
            "<text:table-of-content-source text:outline-level=\"{depth}\">"
        );
        self.body.push_str(
            "<text:index-title-template text:style-name=\"Contents_20_Heading\"></text:index-title-template>",
        );
        for level in 1..=10 {
            let _ = write!(
                self.body,
                "<text:table-of-content-entry-template text:outline-level=\"{level}\" \
                 text:style-name=\"Contents_20_{level}\">\
                 <text:index-entry-link-start text:style-name=\"Internet_20_link\" />\
                 <text:index-entry-chapter />\
                 <text:index-entry-text />\
                 <text:index-entry-link-end />\
                 <text:index-entry-tab-stop style:type=\"right\" style:leader-char=\".\" />\
                 <text:index-entry-link-start text:style-name=\"Internet_20_link\" />\
                 <text:index-entry-page-number />\
                 <text:index-entry-link-end />\
                 </text:table-of-content-entry-template>"
            );
        }
        self.body.push_str("</text:table-of-content-source>");
        self.body.push_str("</text:table-of-content>");
    }
}
