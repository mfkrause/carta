//! Body conversion for the docx reader: blocks, inlines, runs, tables, and identifiers.

use carta_ast::{
    Alignment, Attr, Block, Caption, ColSpec, ColWidth, Inline, ListAttributes, MathType,
    MetaValue, Table, TableBody, TableFoot, TableHead, Target, Text, slug, slug_gfm,
};

use crate::transliterate::docx_asciify;
use crate::xml::{Element, local_name};

use super::blocks::{BlockSink, ListEntry};
use super::helpers::{
    DEFAULT_TEXT_WIDTH_TWIPS, INTER_COLUMN_TWIPS, alignment, canonical_style, custom_style_attr,
    image_attr, is_builtin_style, mime_for, net_left_indent, normalize_target, parse_int, ratio,
    read_toggles, single_image, style_class, table_look_first_row, tbl_header_on, tokenize_inlines,
};
use super::inline::{InlineBuilder, code_paragraph_text};
use super::omml::omml_to_tex;
use super::symbols::{SymbolFont, symbol_font};
use super::tables::{CellRaw, RowRaw, VMerge, build_rows, column_alignment};
use super::{Converter, DOC_DIR, IdMode, MAX_INLINE_DEPTH, ParaRole, RunFmt};

impl Converter<'_> {
    /// Walks a block container (`w:body`, `w:tc`, or a note), reconstructing lists, block quotes, and
    /// code blocks from consecutive same-styled paragraphs.
    pub(super) fn convert_blocks(&mut self, container: &Element) -> Vec<Block> {
        let mut sink = BlockSink::default();
        for element in container.elements() {
            match local_name(&element.name) {
                "p" => self.convert_paragraph(element, &mut sink),
                "tbl" => {
                    self.in_title_block = false;
                    if let Some(table) = self.convert_table(element) {
                        sink.emit_table(table);
                    }
                }
                "sdt" => {
                    if let Some(content) = element.child("sdtContent") {
                        let inner = self.convert_blocks(content);
                        for block in inner {
                            sink.emit(block);
                        }
                    }
                }
                _ => {}
            }
        }
        sink.finish()
    }

    #[allow(clippy::too_many_lines)]
    fn convert_paragraph(&mut self, paragraph: &Element, sink: &mut BlockSink) {
        let properties = paragraph.child("pPr");
        let style_id = properties
            .and_then(|pr| pr.child("pStyle"))
            .and_then(|element| element.attr("val"));
        let style_name = style_id
            .and_then(|id| self.style_name(id))
            .map(str::to_owned)
            .unwrap_or_default();
        let canonical = canonical_style(&style_name);
        // The compact style suppresses block spacing, so the body renders as a bare line.
        let compact = canonical == "compact";

        let inlines = self.convert_inlines(paragraph);

        // Metadata styles consume paragraphs only inside the leading title block; later ones are body.
        let metadata_style = matches!(
            canonical.as_str(),
            "title" | "subtitle" | "author" | "date" | "abstract"
        );
        if self.in_title_block && metadata_style {
            sink.flush();
            match canonical.as_str() {
                "author" => self.authors.push(inlines),
                "abstract" => self.abstract_paras.push(inlines),
                key => {
                    self.meta
                        .insert(key.into(), MetaValue::MetaInlines(inlines));
                }
            }
            return;
        }
        self.in_title_block = false;

        // The role resolves through the `basedOn` chain, so a custom style inherits its base's role.
        let role = style_id.map_or(ParaRole::Normal, |id| self.paragraph_role(id));
        let custom = !style_name.is_empty() && !is_builtin_style(&canonical);
        // `styles` extension: a non-builtin style becomes a `custom-style` container; its role still
        // shapes the inner block.
        let wrap_custom = self.styles_ext && custom;

        // A heading is never a custom-style container; a custom heading style adds its name as a class.
        if let ParaRole::Heading(level) = role {
            let id = self.heading_id(&inlines);
            let classes = if custom {
                vec![style_class(&style_name)]
            } else {
                Vec::new()
            };
            sink.emit(Block::Header(
                level,
                Box::new(Attr {
                    id: id.into(),
                    classes,
                    attributes: Vec::new(),
                }),
                inlines,
            ));
            return;
        }

        // Remaining roles sink into the flow unless a custom style defers them to a container below.
        if !wrap_custom {
            match role {
                // A caption folds into an adjacent image or table; alone it stays a paragraph.
                ParaRole::Caption if !inlines.is_empty() => {
                    sink.push_caption(vec![Block::Para(inlines)]);
                    return;
                }
                ParaRole::Code => {
                    sink.push_code(code_paragraph_text(paragraph));
                    return;
                }
                ParaRole::Quote => {
                    sink.push_quote(Block::Para(inlines));
                    return;
                }
                _ => {}
            }
        }

        // The paragraph's own `numPr` wins; otherwise the style contributes numbering via `basedOn`.
        let direct_num = properties.and_then(|pr| pr.child("numPr"));
        let direct_num_id = direct_num
            .and_then(|np| np.child("numId"))
            .and_then(|element| element.attr("val"))
            .and_then(parse_int);
        let direct_ilvl = direct_num
            .and_then(|np| np.child("ilvl"))
            .and_then(|element| element.attr("val"))
            .and_then(parse_int);
        let (num_id, ilvl) = match direct_num_id {
            Some(id) => (Some(id), direct_ilvl.unwrap_or(0)),
            None => match style_id.and_then(|id| self.style_num_pr(id)) {
                Some((id, style_ilvl)) => (Some(id), direct_ilvl.or(style_ilvl).unwrap_or(0)),
                None => (None, 0),
            },
        };
        if let Some(num_id) = num_id {
            let numbering = self.list_numbering(num_id, ilvl);
            let item = if compact {
                Block::Plain(inlines)
            } else {
                Block::Para(inlines)
            };
            // `styles` extension: each custom-styled item gets its own `custom-style` container.
            let block = if wrap_custom {
                Block::Div(Box::new(custom_style_attr(&style_name)), vec![item])
            } else {
                item
            };
            sink.push_list(ListEntry {
                num_id,
                level: ilvl,
                numbering,
                block,
            });
            return;
        }

        if inlines.is_empty() {
            if self.empty_paragraphs {
                sink.emit(Block::Para(Vec::new()));
            } else {
                sink.interrupt();
            }
            return;
        }

        // A lone-image paragraph becomes a figure when a caption adjoins it on either side.
        if single_image(&inlines) {
            sink.emit_image(inlines);
            return;
        }

        // Consecutive indented paragraphs fold into one block quote regardless of depth.
        let indented = net_left_indent(properties) > 0;

        if wrap_custom {
            let inner = match role {
                ParaRole::Quote => vec![Block::BlockQuote(vec![Block::Para(inlines)])],
                ParaRole::Code => vec![Block::CodeBlock(
                    Box::default(),
                    code_paragraph_text(paragraph).into(),
                )],
                _ => {
                    let para = Block::Para(inlines);
                    if indented {
                        vec![Block::BlockQuote(vec![para])]
                    } else {
                        vec![para]
                    }
                }
            };
            sink.emit(Block::Div(Box::new(custom_style_attr(&style_name)), inner));
            return;
        }

        let body = if compact {
            Block::Plain(inlines)
        } else {
            Block::Para(inlines)
        };
        if indented {
            sink.push_quote(body);
        } else {
            sink.emit(body);
        }
    }

    /// The marker configuration for a list paragraph, or `None` when its level is a bullet or the
    /// numbering id is unknown.
    fn list_numbering(&self, num_id: i32, ilvl: i32) -> Option<ListAttributes> {
        let level = self.lists.get(&num_id)?.get(&ilvl)?;
        level.style.map(|style| ListAttributes {
            start: level.start,
            style,
            delim: level.delim,
        })
    }

    /// Builds the inline content of a paragraph or paragraph-like container from its runs.
    fn convert_inlines(&mut self, container: &Element) -> Vec<Inline> {
        let mut builder = InlineBuilder::default();
        self.walk_inlines(container, &mut builder, true, 0);
        builder.finish()
    }

    /// Recursively emits inline leaves from a container's children. `top` marks the paragraph level,
    /// where `pPr` is skipped. `depth` counts nested inline containers and is capped by
    /// [`MAX_INLINE_DEPTH`] so unbounded nesting cannot exhaust the stack.
    fn walk_inlines(
        &mut self,
        container: &Element,
        builder: &mut InlineBuilder,
        top: bool,
        depth: usize,
    ) {
        for element in container.elements() {
            match local_name(&element.name) {
                "pPr" if top => {}
                "r" => self.convert_run(element, builder),
                "hyperlink" => self.convert_hyperlink(element, builder, depth),
                "ins" | "moveTo" | "smartTag" if depth < MAX_INLINE_DEPTH => {
                    self.walk_inlines(element, builder, false, depth + 1);
                }
                "sdt" if depth < MAX_INLINE_DEPTH => {
                    if let Some(content) = element.child("sdtContent") {
                        self.walk_inlines(content, builder, false, depth + 1);
                    }
                }
                "oMath" => {
                    let tex = omml_to_tex(element);
                    builder.push_node(
                        RunFmt::default(),
                        Inline::Math(MathType::InlineMath, tex.into()),
                    );
                }
                "oMathPara" => {
                    if let Some(math) = element.child("oMath") {
                        let tex = omml_to_tex(math);
                        builder.push_node(
                            RunFmt::default(),
                            Inline::Math(MathType::DisplayMath, tex.into()),
                        );
                    }
                }
                // Deletions, comment anchors, bookmarks, and proofing marks carry no body content.
                _ => {}
            }
        }
    }

    fn convert_run(&mut self, run: &Element, builder: &mut InlineBuilder) {
        let mut fmt = RunFmt::default();
        let mut is_code = false;
        let properties = run.child("rPr");
        if let Some(properties) = properties {
            if let Some(style_id) = properties
                .child("rStyle")
                .and_then(|element| element.attr("val"))
            {
                is_code = self.is_code_style(style_id);
                let custom_name = self.styles_ext.then(|| self.style_name(style_id)).flatten();
                match custom_name
                    .filter(|name| !name.is_empty() && !is_builtin_style(&canonical_style(name)))
                {
                    // The custom-style span carries the style's formatting; not reapplied as toggles.
                    Some(name) => fmt.custom.push(name.into()),
                    None => self.style_fmt(style_id, &mut fmt),
                }
            }
            read_toggles(properties).apply(&mut fmt);
        }
        if is_code {
            // A verbatim-styled run is one inline code span; internal whitespace is preserved.
            let mut text = String::new();
            for child in run.elements() {
                match local_name(&child.name) {
                    "t" => text.push_str(&child.text()),
                    "tab" => text.push('\t'),
                    "br" | "cr" => text.push('\n'),
                    "noBreakHyphen" => text.push('\u{2011}'),
                    _ => {}
                }
            }
            builder.push_node(RunFmt::default(), Inline::Code(Box::default(), text.into()));
            return;
        }
        // Legacy pictorial fonts (Symbol, Wingdings): each letter maps to the Unicode glyph it renders.
        let sub = properties.and_then(symbol_font);
        for child in run.elements() {
            if local_name(&child.name) == "AlternateContent" {
                // Markup-compatibility container: render the Fallback, ignore the Choice.
                if let Some(fallback) = child.child("Fallback") {
                    for leaf in fallback.elements() {
                        self.run_child(leaf, &fmt, sub, builder);
                    }
                }
                continue;
            }
            self.run_child(child, &fmt, sub, builder);
        }
    }

    /// Emits one run-content child (text, break, image, math, note reference, or field marker).
    fn run_child(
        &mut self,
        child: &Element,
        fmt: &RunFmt,
        sub: Option<SymbolFont>,
        builder: &mut InlineBuilder,
    ) {
        match local_name(&child.name) {
            "t" => match sub {
                Some(font) => builder.push_text(fmt, &font.substitute(&child.text())),
                None => builder.push_text(fmt, &child.text()),
            },
            "tab" => builder.push_space(fmt),
            "br" => {
                // A page or column break renders no line of its own and carries no content.
                let kind = child.attr("type").unwrap_or("textWrapping");
                if kind == "textWrapping" {
                    builder.push_break(fmt);
                }
            }
            "cr" => builder.push_break(fmt),
            "noBreakHyphen" => builder.push_text(fmt, "\u{2011}"),
            "sym" => {
                if let Some(ch) = child
                    .attr("char")
                    .and_then(|code| u32::from_str_radix(code, 16).ok())
                    .and_then(char::from_u32)
                {
                    builder.push_text(fmt, &ch.to_string());
                }
            }
            "drawing" | "pict" => {
                if let Some(image) = self.convert_drawing(child) {
                    // An image takes no character formatting: emitted unformatted, splitting any span.
                    builder.push_node(RunFmt::default(), image);
                }
            }
            "oMath" => {
                let tex = omml_to_tex(child);
                builder.push_node(fmt.clone(), Inline::Math(MathType::InlineMath, tex.into()));
            }
            "footnoteReference" => {
                if let Some(note) = self.convert_note(child, false) {
                    // The superscript reference styling is presentational; no inline wrappers.
                    builder.push_node(RunFmt::default(), note);
                }
            }
            "endnoteReference" => {
                if let Some(note) = self.convert_note(child, true) {
                    builder.push_node(RunFmt::default(), note);
                }
            }
            "fldChar" => match child.attr("fldCharType") {
                Some("begin") => builder.field_begin(),
                Some("separate") => builder.field_separate(),
                Some("end") => builder.field_end(),
                _ => {}
            },
            "instrText" => builder.field_instr(&child.text()),
            _ => {}
        }
    }

    fn convert_hyperlink(&mut self, link: &Element, builder: &mut InlineBuilder, depth: usize) {
        if depth >= MAX_INLINE_DEPTH {
            return;
        }
        let mut inner = InlineBuilder::default();
        self.walk_inlines(link, &mut inner, false, depth + 1);
        let content = inner.finish();
        if content.is_empty() {
            return;
        }
        let url = if let Some(anchor) = link.attr("anchor") {
            format!("#{anchor}")
        } else if let Some(id) = link.attr_qualified("r:id", "id") {
            self.rels
                .get(id)
                .map(|rel| rel.target.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        builder.push_node(
            RunFmt::default(),
            Inline::Link(
                Box::default(),
                content,
                Box::new(Target {
                    url: url.into(),
                    title: Text::default(),
                }),
            ),
        );
    }

    fn convert_note(&mut self, reference: &Element, endnote: bool) -> Option<Inline> {
        if self.note_depth >= 8 {
            return None;
        }
        let id = reference.attr("id")?;
        let note = if endnote {
            self.endnotes.get(id)
        } else {
            self.footnotes.get(id)
        }?
        .clone();
        self.note_depth += 1;
        // Title-block metadata comes only from the main document, so suspend extraction inside notes.
        let outer_title_block = self.in_title_block;
        self.in_title_block = false;
        let blocks = self.convert_blocks(&note);
        self.in_title_block = outer_title_block;
        self.note_depth -= 1;
        Some(Inline::Note(blocks))
    }

    fn convert_drawing(&mut self, drawing: &Element) -> Option<Inline> {
        // DrawingML carries the relationship on `a:blip`; legacy VML and OLE previews on `v:imagedata`.
        let rel_id = drawing
            .descendant("blip")
            .and_then(|blip| {
                blip.attr_qualified("r:embed", "embed")
                    .or_else(|| blip.attr_qualified("r:link", "link"))
            })
            .or_else(|| {
                drawing.descendant("imagedata").and_then(|data| {
                    data.attr_qualified("r:id", "id")
                        .or_else(|| data.attr_qualified("r:link", "link"))
                })
            })?;
        let rel = self.rels.get(rel_id)?;
        let alt = drawing
            .descendant("docPr")
            .and_then(|doc_pr| doc_pr.attr("descr"))
            .map(tokenize_inlines)
            .unwrap_or_default();
        if rel.external {
            // An externally linked image is referenced by its URL and carries no packaged bytes.
            return Some(Inline::Image(
                Box::new(image_attr(drawing)),
                alt,
                Box::new(Target {
                    url: rel.target.clone().into(),
                    title: Text::default(),
                }),
            ));
        }
        let url = normalize_target(&rel.target);
        // Bytes are fetched only when the image is new to the bag; a repeat skips the copy.
        if !self.media.contains(&url) {
            let part_name = format!("{DOC_DIR}{url}");
            if let Some(bytes) = self.part(&part_name).map(<[u8]>::to_vec) {
                self.media.insert(url.clone(), Some(mime_for(&url)), bytes);
            }
        }
        Some(Inline::Image(
            Box::new(image_attr(drawing)),
            alt,
            Box::new(Target {
                url: url.into(),
                title: Text::default(),
            }),
        ))
    }

    #[allow(clippy::too_many_lines)]
    fn convert_table(&mut self, table: &Element) -> Option<Block> {
        let grid: Vec<i64> = table
            .child("tblGrid")
            .into_iter()
            .flat_map(Element::elements)
            .filter(|element| local_name(&element.name) == "gridCol")
            .map(|element| element.attr("w").and_then(parse_int).map_or(0, i64::from))
            .collect();
        let total: i64 = grid.iter().sum();
        // Column width is a fraction of a default page's printable width minus a per-boundary
        // allowance, floored at the grid's own total so an over-wide table stays normalized to itself.
        let column_count = i64::try_from(grid.len()).unwrap_or(i64::MAX);
        let reference_width =
            (DEFAULT_TEXT_WIDTH_TWIPS - INTER_COLUMN_TWIPS * (column_count - 1).max(0)).max(total);

        // A firstRow table look promotes the first row to header when no row carries its own marker.
        let look_first_row = table
            .child("tblPr")
            .and_then(|pr| pr.child("tblLook"))
            .is_some_and(table_look_first_row);

        // Column count is fixed by the grid (widest row absent one); spans are validated against it,
        // so a malformed `gridSpan` cannot overflow a column position or inflate the width.
        let columns = if grid.is_empty() {
            table
                .elements()
                .filter(|tr| local_name(&tr.name) == "tr")
                .map(|tr| {
                    tr.elements()
                        .filter(|tc| local_name(&tc.name) == "tc")
                        .count()
                })
                .max()
                .unwrap_or(0)
        } else {
            grid.len()
        };

        // Parse each row into positioned cells so vertical merges can be resolved by grid column.
        let mut rows: Vec<RowRaw> = Vec::new();
        for tr in table.elements() {
            if local_name(&tr.name) != "tr" {
                continue;
            }
            let header = tr
                .child("trPr")
                .and_then(|pr| pr.child("tblHeader"))
                .is_some_and(tbl_header_on);
            let mut cells: Vec<CellRaw> = Vec::new();
            let mut col = 0usize;
            for tc in tr.elements() {
                if local_name(&tc.name) != "tc" {
                    continue;
                }
                if col >= columns {
                    // No column remains: a surplus cell is dropped, never widening the table.
                    continue;
                }
                let properties = tc.child("tcPr");
                let requested = properties
                    .and_then(|pr| pr.child("gridSpan"))
                    .and_then(|element| element.attr("val"))
                    .and_then(parse_int)
                    .unwrap_or(1)
                    .max(1);
                let remaining = columns - col;
                let span = usize::try_from(requested)
                    .unwrap_or(remaining)
                    .min(remaining);
                let vmerge = properties.and_then(|pr| pr.child("vMerge")).map(|element| {
                    match element.attr("val") {
                        Some("restart") => VMerge::Restart,
                        _ => VMerge::Continue,
                    }
                });
                let align = tc
                    .child("p")
                    .and_then(|p| p.child("pPr"))
                    .and_then(|pr| pr.child("jc"))
                    .and_then(|element| element.attr("val"))
                    .map_or(Alignment::AlignDefault, alignment);
                let content = self.convert_cell(tc);
                cells.push(CellRaw {
                    start_col: col,
                    span,
                    vmerge,
                    align,
                    content,
                });
                col += span;
            }
            rows.push(RowRaw { header, cells });
        }

        if rows.is_empty() {
            return None;
        }

        // A row is a header when flagged; otherwise a `firstRow` table look marks the first row.
        let any_header = rows.iter().any(|row| row.header);
        let head_count = if any_header {
            rows.iter().take_while(|row| row.header).count()
        } else {
            usize::from(look_first_row)
        };

        let col_specs = (0..columns)
            .map(|index| {
                let width = match grid.get(index) {
                    Some(&value) if total > 0 => ColWidth::ColWidth(ratio(value, reference_width)),
                    _ => ColWidth::ColWidthDefault,
                };
                let align = column_alignment(&rows, head_count, index);
                ColSpec { align, width }
            })
            .collect();

        let mut head_rows = Vec::new();
        let mut body_rows = Vec::new();
        for (index, row) in build_rows(rows).into_iter().enumerate() {
            if index < head_count {
                head_rows.push(row);
            } else {
                body_rows.push(row);
            }
        }

        Some(Block::Table(Box::new(Table {
            attr: Attr::default(),
            caption: Caption::default(),
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: head_rows,
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: body_rows,
            }],
            foot: TableFoot::default(),
        })))
    }

    /// Cell content: a single paragraph becomes a `Plain` block; anything richer keeps its blocks.
    fn convert_cell(&mut self, cell: &Element) -> Vec<Block> {
        // Cell content is a nested block flow, never the document's leading title block.
        let outer_title_block = self.in_title_block;
        self.in_title_block = false;
        let mut blocks = self.convert_blocks(cell);
        self.in_title_block = outer_title_block;
        if matches!(blocks.as_slice(), [Block::Para(_)])
            && let Some(Block::Para(inlines)) = blocks.pop()
        {
            return vec![Block::Plain(inlines)];
        }
        blocks
    }

    /// Derives a unique heading identifier from heading text, honoring the ASCII- and GitHub-slug
    /// extensions and disambiguating repeats with an incrementing numeric suffix.
    fn heading_id(&mut self, inlines: &[Inline]) -> String {
        let text = carta_ast::to_plain_text(inlines);
        let source = if self.ascii_ids {
            docx_asciify(&text)
        } else {
            text
        };
        let base = match self.id_mode {
            IdMode::Plain => slug(&source),
            IdMode::Gfm => slug_gfm(&source),
        };
        self.ids.assign_with_separator(base, '-')
    }

    pub(super) fn finish_authors(&mut self) {
        let entries = std::mem::take(&mut self.authors);
        let value = match entries.len() {
            0 => return,
            1 => MetaValue::MetaInlines(entries.into_iter().next().unwrap_or_default()),
            _ => MetaValue::MetaList(entries.into_iter().map(MetaValue::MetaInlines).collect()),
        };
        self.meta.insert("author".into(), value);
    }

    pub(super) fn finish_abstract(&mut self) {
        let paras = std::mem::take(&mut self.abstract_paras);
        let value = match paras.len() {
            0 => return,
            1 => MetaValue::MetaInlines(paras.into_iter().next().unwrap_or_default()),
            _ => MetaValue::MetaBlocks(paras.into_iter().map(Block::Para).collect()),
        };
        self.meta.insert("abstract".into(), value);
    }
}
