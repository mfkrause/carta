//! Block-level rendering for the ODT writer.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MetaValue, Row, Table, Text,
};
use carta_core::container::xml::escape_attribute;

use super::helpers::{
    attr_value, collect_caption_runs, column_letter, custom_style, inlines_are_empty,
    is_block_math, is_opendocument, is_tight, table_column_count, trim_flanking_spacing,
};
use super::meta::{abstract_paragraphs, meta_authors, meta_inlines};
use super::styles::{delim_fixes, num_format};
use super::{
    AlignKind, Builder, CaptionKind, Language, ListStyle, ParaKind, ParaStyle, STACK_RED_ZONE,
    STACK_SEGMENT,
};

impl Builder<'_> {
    /// Renders a block sequence. `fixed` names a paragraph style imposed on the sequence's direct
    /// paragraphs (a list item, a cell, a blockquote); `None` selects the flowing body styles.
    pub(super) fn render_blocks(&mut self, blocks: &[Block], fixed: Option<&str>) {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            for block in blocks {
                if carta_core::budget::exhausted() {
                    break;
                }
                self.render_block(block, fixed);
            }
        });
    }

    fn render_block(&mut self, block: &Block, fixed: Option<&str>) {
        let quote_depth = self.quote_depth;
        if matches!(
            block,
            Block::Div(..)
                | Block::BulletList(..)
                | Block::OrderedList(..)
                | Block::Table(..)
                | Block::Figure(..)
        ) {
            self.quote_depth = 0;
        }
        match block {
            Block::Plain(inlines) if inlines.is_empty() && fixed.is_none() => {}
            Block::Para(inlines) | Block::Plain(inlines) => self.paragraph_like(inlines, fixed),
            Block::LineBlock(lines) => self.line_block_like(lines, fixed),
            Block::Div(attr, inner) => self.div(attr, inner, fixed),
            Block::RawBlock(..) | Block::Figure(..) => self.block_other(block, fixed),
            other => {
                self.first_para = true;
                self.block_other(other, fixed);
            }
        }
        self.quote_depth = quote_depth;
    }

    /// A paragraph node: styled with the flowing first/body distinction, or with an imposed style.
    fn paragraph_like(&mut self, inlines: &[Inline], fixed: Option<&str>) {
        if inlines.iter().any(is_block_math) {
            self.paragraphs_split_on_block_math(inlines);
            return;
        }
        if let Some(style) = fixed {
            self.paragraph(style, inlines);
        } else if !inlines.is_empty() || self.keep_empty {
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
        if !self.keep_empty && lines.len() <= 1 && lines.iter().all(Vec::is_empty) {
            return;
        }
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
        let language = attr_value(attr, "lang")
            .and_then(Language::parse)
            .filter(|language| !self.languages.contains(language));
        if let Some(language) = &language {
            carta_core::budget::charge(
                std::mem::size_of::<Language>().saturating_add(language.clone_size()),
            );
            if carta_core::budget::exhausted() {
                return;
            }
            self.languages.insert(language.clone());
        }
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
        if let Some(language) = &language {
            self.languages.remove(language);
        }
    }

    /// Renders a block that is neither a paragraph nor a transparent container.
    fn block_other(&mut self, block: &Block, fixed: Option<&str>) {
        match block {
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines),
            Block::CodeBlock(attr, text) => self.code_block(attr, text),
            Block::BlockQuote(blocks) => {
                self.first_para = true;
                let indentation = if matches!(
                    fixed,
                    Some("Definition_20_Definition" | "Definition_20_Definition_20_Tight")
                ) {
                    self.quote_depth = self.quote_depth.max(1);
                    Some(self.quote_depth.saturating_sub(1).max(1))
                } else {
                    self.quote_depth += 1;
                    (self.quote_depth > 1).then_some(self.quote_depth - 1)
                };
                let style = if let Some(half_inches) = indentation {
                    self.para_styles.push(ParaStyle {
                        parent: "Quotations".to_string(),
                        kind: ParaKind::Quote(half_inches),
                    });
                    format!("P{}", self.para_styles.len())
                } else {
                    "Quotations".to_string()
                };
                self.render_blocks(blocks, Some(&style));
            }
            Block::BulletList(items) => self.bullet_list(items),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items),
            Block::DefinitionList(items) => self.definition_list(items),
            Block::HorizontalRule => {
                self.body
                    .push_str("<text:p text:style-name=\"Horizontal_20_Line\" />");
            }
            Block::Table(table) => self.table(table),
            Block::Figure(attr, caption, content) => self.figure(attr, caption, content),
            Block::RawBlock(format, text) => {
                if is_opendocument(format) {
                    self.body.push_str(text);
                }
            }
            Block::Para(inlines) | Block::Plain(inlines) => self.paragraph_like(inlines, None),
            Block::LineBlock(lines) => self.line_block_like(lines, None),
            Block::Div(attr, inner) => self.div(attr, inner, None),
        }
    }

    fn header(&mut self, level: i64, attr: &Attr, inlines: &[Inline]) {
        let _ = write!(
            self.body,
            "<text:h text:style-name=\"Heading_20_{level}\" text:outline-level=\"{level}\">"
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

    #[cfg_attr(not(feature = "highlight"), allow(unused_variables))]
    fn code_block(&mut self, attr: &Attr, text: &str) {
        if text.is_empty() {
            return;
        }
        #[cfg(feature = "highlight")]
        if self.highlighted_code(attr, text) {
            return;
        }
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
        self.list_items(items, item_style, false);
        self.body.push_str("\n</text:list>");
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
        self.list_items(items, item_style, true);
        self.body.push_str("\n</text:list>");
    }

    fn list_items(&mut self, items: &[Vec<Block>], item_style: &str, separate_blocks: bool) {
        for item in items {
            if carta_core::budget::exhausted() {
                break;
            }
            self.body.push_str("\n  <text:list-item>");
            let checkpoint = self.body.len();
            for block in item {
                let start = self.body.len();
                self.render_blocks(std::slice::from_ref(block), Some(item_style));
                if separate_blocks
                    && start > checkpoint
                    && self.body.len() > start
                    && !self
                        .body
                        .get(..start)
                        .is_some_and(|text| text.ends_with('\n'))
                {
                    self.body.insert(start, '\n');
                }
            }
            self.indent_since(checkpoint, "    ");
            self.body.push_str("\n  </text:list-item>");
        }
    }

    fn indent_since(&mut self, checkpoint: usize, prefix: &str) {
        let content = self.body.get(checkpoint..).unwrap_or_default();
        if content.is_empty() {
            return;
        }
        if content.bytes().all(|byte| byte == b'\n') {
            // The closing tag supplies the last line break.
            let _ = self.body.pop();
            return;
        }
        let content = content.trim_end_matches('\n');
        carta_core::budget::charge(
            content
                .len()
                .saturating_add(content.split('\n').count().saturating_mul(prefix.len())),
        );
        if carta_core::budget::exhausted() {
            return;
        }
        let first = if content.starts_with('\n') {
            ""
        } else {
            prefix
        };
        let indented = crate::common::indent_block(content, first, prefix);
        self.body.truncate(checkpoint);
        self.body.push('\n');
        self.body.push_str(&indented);
    }

    fn definition_list(&mut self, items: &[(Vec<Inline>, Vec<Vec<Block>>)]) {
        for (term, defs) in items {
            let (term_style, def_style) = if is_tight(defs) {
                (
                    "Definition_20_Term_20_Tight",
                    "Definition_20_Definition_20_Tight",
                )
            } else {
                ("Definition_20_Term", "Definition_20_Definition")
            };
            self.paragraph(term_style, term);
            for def in defs {
                self.render_blocks(def, Some(def_style));
            }
        }
    }

    fn figure(&mut self, attr: &Attr, caption: &Caption, content: &[Block]) {
        let mut runs = Vec::new();
        collect_caption_runs(&caption.long, &mut runs);
        let has_caption = runs.len() > 1 || runs.first().is_some_and(|run| !run.is_empty());
        let style = if has_caption {
            "FigureWithCaption"
        } else {
            "Figure"
        };
        self.render_blocks(content, Some(style));
        if has_caption {
            self.caption(CaptionKind::Figure, attr.id.as_str(), &runs);
        }
    }

    fn caption(&mut self, kind: CaptionKind, identifier: &str, runs: &[Cow<'_, [Inline]>]) {
        self.caption_start(kind, identifier);
        for (index, inlines) in runs.iter().enumerate() {
            if index > 0 {
                self.body.push_str("<text:line-break />");
            }
            self.inlines(inlines);
        }
        self.body.push_str("</text:p>");
    }

    fn caption_start(&mut self, kind: CaptionKind, identifier: &str) {
        let (style, sequence, label, counter) = match kind {
            CaptionKind::Figure => (
                "FigureCaption",
                "Illustration",
                self.caption_labels.0,
                &mut self.figure_number,
            ),
            CaptionKind::Table => (
                "TableCaption",
                "Table",
                self.caption_labels.1,
                &mut self.table_number,
            ),
        };
        let _ = write!(self.body, "<text:p text:style-name=\"{style}\">");
        if !self
            .options
            .extensions
            .contains(carta_core::Extension::NativeNumbering)
        {
            return;
        }
        let index = *counter;
        *counter = counter.saturating_add(1);
        let number = *counter;
        self.push_verbatim(label);
        self.body.push_str(" <text:sequence text:ref-name=\"");
        if identifier.is_empty() {
            let _ = write!(self.body, "ref{sequence}{index}");
        } else {
            escape_attribute(identifier, &mut self.body);
        }
        let _ = write!(
            self.body,
            "\" text:name=\"{sequence}\" text:formula=\"ooow:{sequence}+1\" style:num-format=\"1\">{number}</text:sequence>: "
        );
    }

    fn table(&mut self, table: &Table) {
        if !table.caption.long.is_empty() {
            let mut runs = Vec::new();
            collect_caption_runs(&table.caption.long, &mut runs);
            self.caption(CaptionKind::Table, table.attr.id.as_str(), &runs);
        }
        self.table_index += 1;
        let n = self.table_index;
        let columns = table_column_count(table);
        self.push_table_style(n, columns, &table.col_specs);
        let headings: Vec<_> = (0..columns)
            .map(|column| self.column_paragraph_style(true, column, &table.col_specs))
            .collect();
        let styles: Vec<_> = headings
            .into_iter()
            .enumerate()
            .map(|(column, heading)| {
                (
                    heading,
                    self.column_paragraph_style(false, column, &table.col_specs),
                )
            })
            .collect();

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
                &styles,
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
                &styles,
                &mut covered,
            );
            self.table_rows(
                &section.body,
                false,
                head_columns,
                columns,
                &styles,
                &mut covered,
            );
        }
        self.table_rows(&table.foot.rows, false, 0, columns, &styles, &mut covered);
        self.body.push_str("</table:table>");
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn push_table_style(&mut self, n: usize, columns: usize, specs: &[ColSpec]) {
        let _ = write!(
            self.table_styles,
            "<style:style style:name=\"Table{n}\" style:family=\"table\">\
             <style:table-properties table:align=\"center\""
        );
        let relative_width = |spec: &ColSpec| match spec.width {
            ColWidth::ColWidth(fraction) if fraction > 0.0 => Some(fraction),
            _ => None,
        };
        let total: f64 = specs.iter().filter_map(relative_width).sum();
        if total > 0.0 && total <= 1.0 {
            let percent = (total * 100.0).round_ties_even() as i64;
            let _ = write!(self.table_styles, " style:rel-width=\"{percent}%\"");
        }
        self.table_styles.push_str(" /></style:style>");

        for column in 0..columns {
            let letter = column_letter(column);
            if let Some(fraction) = specs.get(column).and_then(relative_width) {
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
        styles: &[(String, String)],
        covered: &mut [usize],
    ) {
        for (row_index, row) in rows.iter().enumerate() {
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
                    let span = usize::try_from(cell.col_span.max(1))
                        .unwrap_or(usize::MAX)
                        .min(columns - column);
                    let rows_spanned = usize::try_from(cell.row_span.max(1))
                        .unwrap_or(usize::MAX)
                        .min(rows.len() - row_index);
                    let para_header = column < head_columns;
                    self.emit_cell(
                        cell,
                        cell_header,
                        para_header,
                        column,
                        styles,
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

    #[allow(clippy::too_many_arguments)]
    fn emit_cell(
        &mut self,
        cell: &Cell,
        cell_header: bool,
        para_header: bool,
        column: usize,
        styles: &[(String, String)],
        span: usize,
        rows_spanned: usize,
    ) {
        let para_style = self.cell_paragraph_style(para_header, column, styles, &cell.align);
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
        self.render_blocks(&cell.content, Some(&para_style));
        self.body.push_str("</table:table-cell>");
    }

    fn emit_empty_cell(&mut self, cell_header: bool) {
        let cell_style = if cell_header {
            "TableHeaderRowCell"
        } else {
            "TableRowCell"
        };
        let _ = write!(
            self.body,
            "<table:table-cell table:style-name=\"{cell_style}\" office:value-type=\"string\"></table:table-cell>"
        );
    }

    /// The column's paragraph style, with a fresh child style for an explicit cell alignment.
    fn cell_paragraph_style(
        &mut self,
        para_header: bool,
        column: usize,
        styles: &[(String, String)],
        cell_align: &Alignment,
    ) -> String {
        let default = if para_header {
            "Table_20_Heading"
        } else {
            "Table_20_Contents"
        };
        let base = styles.get(column).map_or(default, |(heading, contents)| {
            if para_header {
                heading.as_str()
            } else {
                contents.as_str()
            }
        });
        match cell_align {
            Alignment::AlignCenter => self.align_style(base, AlignKind::Center),
            Alignment::AlignRight => self.align_style(base, AlignKind::Right),
            _ => base.to_string(),
        }
    }

    fn column_paragraph_style(&mut self, header: bool, column: usize, specs: &[ColSpec]) -> String {
        let base = if header {
            "Table_20_Heading"
        } else {
            "Table_20_Contents"
        };
        match specs.get(column).map(|spec| &spec.align) {
            Some(Alignment::AlignCenter) => self.align_style(base, AlignKind::Center),
            Some(Alignment::AlignRight) => self.align_style(base, AlignKind::Right),
            _ => base.to_owned(),
        }
    }

    fn align_style(&mut self, parent: &str, kind: AlignKind) -> String {
        self.para_styles.push(ParaStyle {
            parent: parent.to_owned(),
            kind: ParaKind::Align(kind),
        });
        format!("P{}", self.para_styles.len())
    }

    /// Returns a numbered-list style name, creating the style on first use.
    fn auto_list_style(&mut self, style: ListNumberStyle, delim: ListNumberDelim) -> String {
        let index = self
            .list_styles
            .iter()
            .position(|existing| existing.numbering == style && existing.delimiter == delim)
            .unwrap_or(self.list_styles.len());
        let name = format!("Numbering_5f_{}", index + 1);
        if index < self.list_styles.len() {
            return name;
        }
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
        self.list_styles.push(ListStyle {
            numbering: style,
            delimiter: delim,
            xml: out,
        });
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
        self.body.push_str("<text:table-of-content>");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_lists_indent_items_and_keep_empty_items_explicit() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        builder.bullet_list(&[
            vec![
                Block::Para(vec![Inline::Str("one".into())]),
                Block::BulletList(vec![vec![Block::Para(vec![Inline::Str("inner".into())])]]),
            ],
            Vec::new(),
        ]);
        assert_eq!(
            builder.body,
            concat!(
                "<text:list text:style-name=\"List_20_1\">\n",
                "  <text:list-item>\n",
                "    <text:p text:style-name=\"List_20_Bullet\">one</text:p><text:list text:style-name=\"List_20_1\">\n",
                "      <text:list-item>\n",
                "        <text:p text:style-name=\"List_20_Bullet\">inner</text:p>\n",
                "      </text:list-item>\n",
                "    </text:list>\n",
                "  </text:list-item>\n",
                "  <text:list-item>\n",
                "  </text:list-item>\n",
                "</text:list>"
            )
        );
    }

    #[test]
    fn nested_list_indentation_stops_at_the_output_limit() {
        let mut block = Block::Para(vec![Inline::Str("text".into())]);
        for _ in 0..256 {
            block = Block::BulletList(vec![vec![block]]);
        }
        let document = carta_ast::Document {
            blocks: vec![block],
            ..Default::default()
        };
        assert!(matches!(
            super::super::write_odt(&document, &carta_core::WriterOptions::default()),
            Err(carta_core::Error::OutputTooLarge)
        ));
    }

    #[test]
    fn numbered_list_paragraphs_use_separate_lines() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        builder.ordered_list(
            &ListAttributes {
                start: 1,
                style: ListNumberStyle::Decimal,
                delim: ListNumberDelim::Period,
            },
            &[vec![
                Block::Para(vec![Inline::Str("one".into())]),
                Block::Para(vec![Inline::Str("two".into())]),
            ]],
        );
        assert!(
            builder
                .body
                .contains("one</text:p>\n    <text:p text:style-name=\"List_20_Number\">two")
        );
    }

    #[test]
    fn numbered_captions_keep_ids_and_independent_counters() {
        let mut options = carta_core::WriterOptions::default();
        options
            .extensions
            .insert(carta_core::Extension::NativeNumbering);
        let caption = Caption {
            long: vec![
                Block::Para(vec![Inline::Str("Caption".into())]),
                Block::RawBlock(
                    carta_ast::Format("opendocument".into()),
                    "<text:span>Raw</text:span>".into(),
                ),
                Block::LineBlock(vec![
                    vec![Inline::Str("One".into())],
                    vec![Inline::Str("Two".into())],
                ]),
            ],
            ..Default::default()
        };
        let document = carta_ast::Document {
            meta: BTreeMap::from([("lang".into(), MetaValue::MetaString("de".into()))]),
            blocks: vec![
                Block::Figure(Box::default(), Box::default(), Vec::new()),
                Block::Figure(
                    Box::new(Attr {
                        id: "chart&one".into(),
                        ..Default::default()
                    }),
                    Box::new(caption.clone()),
                    Vec::new(),
                ),
                Block::Table(Box::new(Table {
                    caption: caption.clone(),
                    ..Default::default()
                })),
                Block::Figure(Box::default(), Box::new(caption), Vec::new()),
            ],
            ..Default::default()
        };
        let mut builder = Builder::new(&options);
        builder.document(&document);
        let xml = builder.finish_content();
        assert_eq!((builder.figure_number, builder.table_number), (2, 1));
        assert_eq!(
            xml.matches("Raw</text:span><text:line-break />One<text:line-break />Two</text:p>")
                .count(),
            3
        );
        assert!(xml.contains("xmlns:ooow=\"http://openoffice.org/2004/writer\""));
        assert!(xml.contains("Abbildung <text:sequence text:ref-name=\"chart&amp;one\""));
        assert!(xml.contains("Tabelle <text:sequence text:ref-name=\"refTable0\""));
        assert!(xml.contains("text:ref-name=\"refIllustration1\" text:name=\"Illustration\" text:formula=\"ooow:Illustration+1\" style:num-format=\"1\">2</text:sequence>: Caption"));
    }

    #[test]
    fn relative_widths_keep_automatic_columns_and_even_percentage_ties() {
        let options = carta_core::WriterOptions::default();
        for (widths, percent) in [
            ([0.0, 0.025], Some("2%")),
            ([-0.25, 0.5], Some("50%")),
            ([0.0, 0.0], None),
            ([0.0, 1.5], None),
        ] {
            let mut builder = Builder::new(&options);
            let specs: Vec<_> = widths
                .into_iter()
                .map(|width| ColSpec {
                    align: Alignment::AlignDefault,
                    width: ColWidth::ColWidth(width),
                })
                .collect();
            builder.push_table_style(1, 2, &specs);
            assert!(
                builder.table_styles.contains(
                    "<style:style style:name=\"Table1.A\" style:family=\"table-column\" />"
                )
            );
            if let Some(percent) = percent {
                assert!(
                    builder
                        .table_styles
                        .contains(&format!("style:rel-width=\"{percent}\""))
                );
            } else {
                assert!(!builder.table_styles.contains("style:rel-width="));
            }
        }
    }

    #[test]
    fn nested_quotes_indent_and_restart_inside_lists_and_notes() {
        let paragraph = |text: &str| Block::Para(vec![Inline::Str(text.into())]);
        let blocks = vec![Block::BlockQuote(vec![
            paragraph("Outer"),
            Block::BlockQuote(vec![
                paragraph("Inner"),
                Block::BlockQuote(vec![paragraph("Deep")]),
            ]),
            Block::BulletList(vec![vec![Block::BlockQuote(vec![paragraph("List")])]]),
            Block::Para(vec![Inline::Note(vec![Block::BlockQuote(vec![
                paragraph("Note"),
            ])])]),
            Block::BlockQuote(vec![paragraph("Last")]),
        ])];
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        builder.render_blocks(&blocks, None);
        assert_eq!(builder.quote_depth, 0);
        assert_eq!(builder.para_styles.len(), 3);
        let xml = builder.finish_content();
        assert_eq!(xml.matches("fo:margin-left=\"0.5in\"").count(), 2);
        assert_eq!(xml.matches("fo:margin-left=\"1.0in\"").count(), 1);
        for text in ["List", "Note"] {
            assert!(xml.contains(&format!(
                "<text:p text:style-name=\"Quotations\">{text}</text:p>"
            )));
        }
        assert!(xml.contains("<text:p text:style-name=\"P3\">Last</text:p>"));
    }

    #[test]
    fn definitions_supply_the_first_quote_indent() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        let blocks = vec![Block::DefinitionList(vec![(
            vec![Inline::Str("Term".into())],
            vec![vec![Block::BlockQuote(vec![Block::BlockQuote(vec![
                Block::Para(vec![Inline::Str("Text".into())]),
            ])])]],
        )])];
        builder.render_blocks(&blocks, None);
        let xml = builder.finish_content();
        assert_eq!(xml.matches("fo:margin-left=\"0.5in\"").count(), 2);
        assert!(!xml.contains("fo:margin-left=\"1.0in\""));
        assert!(xml.contains("<text:p text:style-name=\"P2\">Text</text:p>"));
    }

    #[test]
    fn container_flow_state_starts_before_rendering_its_content() {
        let options = carta_core::WriterOptions::default();
        let paragraph = Block::Para(vec![
            Inline::Str("Before".into()),
            Inline::Math(carta_ast::MathType::DisplayMath, "x".into()),
            Inline::Str("After".into()),
        ]);
        let table = Table {
            col_specs: vec![
                ColSpec {
                    align: Alignment::AlignDefault,
                    width: ColWidth::ColWidthDefault
                };
                2
            ],
            bodies: vec![carta_ast::TableBody {
                body: vec![Row {
                    cells: vec![
                        Cell {
                            row_span: 1,
                            col_span: 1,
                            content: vec![paragraph.clone()],
                            attr: Attr::default(),
                            align: Alignment::AlignDefault
                        };
                        2
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        for block in [
            Block::Table(Box::new(table)),
            Block::BlockQuote(vec![paragraph.clone(), paragraph.clone()]),
            Block::DefinitionList(vec![(
                vec![Inline::Str("Term".into())],
                vec![vec![paragraph.clone(), paragraph]],
            )]),
        ] {
            let mut builder = Builder::new(&options);
            builder.render_blocks(
                &[block, Block::Para(vec![Inline::Str("Suffix".into())])],
                None,
            );
            assert_eq!(
                builder
                    .body
                    .matches("text:style-name=\"First_20_paragraph\"")
                    .count(),
                1
            );
            assert!(
                builder
                    .body
                    .ends_with("<text:p text:style-name=\"Text_20_body\">Suffix</text:p>")
            );
        }
    }

    #[test]
    fn raw_blocks_and_figures_preserve_the_flowing_paragraph_state() {
        let options = carta_core::WriterOptions::default();
        let blocks = [
            Block::RawBlock(carta_ast::Format("html".into()), "<p>Drop</p>".into()),
            Block::RawBlock(
                carta_ast::Format("opendocument".into()),
                "<text:p>Raw</text:p>".into(),
            ),
            Block::Figure(
                Box::default(),
                Box::default(),
                vec![Block::Para(vec![Inline::Str("Figure".into())])],
            ),
        ];
        for block in blocks {
            for first in [false, true] {
                let mut builder = Builder::new(&options);
                builder.first_para = first;
                builder.render_blocks(std::slice::from_ref(&block), None);
                builder.render_blocks(&[Block::Para(vec![Inline::Str("After".into())])], None);
                let style = if first {
                    "First_20_paragraph"
                } else {
                    "Text_20_body"
                };
                assert!(builder.body.ends_with(&format!(
                    "<text:p text:style-name=\"{style}\">After</text:p>"
                )));
            }
        }
    }

    #[test]
    fn list_definitions_follow_numbering_and_delimiter_order() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        let first =
            builder.auto_list_style(ListNumberStyle::LowerRoman, ListNumberDelim::TwoParens);
        let second =
            builder.auto_list_style(ListNumberStyle::UpperAlpha, ListNumberDelim::TwoParens);
        let third = builder.auto_list_style(ListNumberStyle::LowerRoman, ListNumberDelim::OneParen);
        let xml = builder.finish_content();
        let position = |name: &str| {
            xml.find(&format!("<text:list-style style:name=\"{name}\">"))
                .expect("list definition")
        };
        assert!(position(&third) < position(&first));
        assert!(position(&first) < position(&second));
    }

    #[test]
    fn repeated_numbering_settings_share_a_style() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        let first =
            builder.auto_list_style(ListNumberStyle::LowerRoman, ListNumberDelim::DefaultDelim);
        assert_eq!(
            first,
            builder.auto_list_style(ListNumberStyle::LowerRoman, ListNumberDelim::DefaultDelim)
        );
        assert_ne!(
            first,
            builder.auto_list_style(ListNumberStyle::LowerRoman, ListNumberDelim::Period)
        );
        assert_ne!(
            first,
            builder.auto_list_style(ListNumberStyle::UpperRoman, ListNumberDelim::DefaultDelim)
        );
        assert_eq!(builder.list_styles.len(), 3);
    }

    #[test]
    fn aligned_columns_keep_distinct_styles_without_header_content() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        let table = Table {
            col_specs: vec![
                ColSpec {
                    align: Alignment::AlignCenter,
                    width: ColWidth::ColWidthDefault
                };
                2
            ],
            head: carta_ast::TableHead {
                rows: vec![Row::default()],
                ..Default::default()
            },
            bodies: vec![carta_ast::TableBody {
                body: vec![Row {
                    cells: ["A", "B"]
                        .into_iter()
                        .map(|label| Cell {
                            attr: Attr::default(),
                            align: Alignment::AlignDefault,
                            row_span: 1,
                            col_span: 1,
                            content: vec![Block::Plain(vec![Inline::Str(label.into())])],
                        })
                        .collect(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        builder.table(&table);
        assert_eq!(
            builder
                .para_styles
                .iter()
                .map(|style| style.parent.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Table_20_Heading",
                "Table_20_Heading",
                "Table_20_Contents",
                "Table_20_Contents"
            ]
        );
        assert!(
            builder
                .body
                .contains("<text:p text:style-name=\"P3\">A</text:p>")
        );
        assert!(
            builder
                .body
                .contains("<text:p text:style-name=\"P4\">B</text:p>")
        );
    }

    #[test]
    fn row_spans_stop_at_the_last_row() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        let rows: Vec<Row> = [1, i64::MAX]
            .into_iter()
            .map(|row_span| Row {
                attr: Attr::default(),
                cells: vec![Cell {
                    attr: Attr::default(),
                    align: Alignment::AlignDefault,
                    row_span,
                    col_span: 1,
                    content: Vec::new(),
                }],
            })
            .collect();
        let mut covered = [0];
        builder.table_rows(&rows, false, 0, 1, &[], &mut covered);
        assert!(!builder.body.contains("table:number-rows-spanned"));
        assert_eq!(covered, [0]);
    }
}
