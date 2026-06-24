//! Conversion: walks the node tree into a [`Document`]'s blocks and inlines, and reads document
//! metadata from a `<head>` element.

use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Format, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MathType, MetaValue, QuoteType, Row, Table, TableBody,
    TableFoot, TableHead, Target, slug, to_plain_text,
};
use carta_core::{Extension, Extensions};

use super::classify::{BlockKind, InlineKind, block_kind, inline_kind, is_inline_element};
use super::table::{
    cell_alignment, column_alignments, column_widths, normalize_cell_style, row_elements,
    row_head_columns, span_attr, table_width,
};
use super::tree::{Element, Node, attr_value, collect_text, serialize_element, style_property};
use crate::inline_scan::{
    fold_ellipsis_run, scan_backslash_math, scan_display_math, scan_inline_math,
};

/// Build inline content from a node tree, with no surrounding block. Used to parse a string of HTML
/// inline markup into inlines: leading and trailing whitespace is trimmed, matching how inline
/// content sits inside a heading.
#[cfg(feature = "opml")]
pub(super) fn inlines_from_nodes(nodes: &[Node]) -> Vec<Inline> {
    let converter = Converter {
        preserve_unknown_tags: true,
        ..Converter::default()
    };
    trim_inlines(converter.build_inlines(nodes))
}

pub(super) fn extract_meta(head: &Element) -> BTreeMap<String, MetaValue> {
    let mut meta = BTreeMap::new();
    for child in &head.children {
        let Node::Element(e) = child else { continue };
        match e.name.as_str() {
            "title" => {
                meta.insert("title".to_string(), MetaValue::MetaInlines(text_inlines(e)));
            }
            "meta" => {
                let name = attr_value(e, "name");
                let content = attr_value(e, "content");
                if let (Some(name), Some(content)) = (name, content)
                    && !name.is_empty()
                {
                    meta.insert(name, MetaValue::MetaInlines(inlines_from_text(&content)));
                }
            }
            _ => {}
        }
    }
    meta
}

fn text_inlines(e: &Element) -> Vec<Inline> {
    let mut out = Vec::new();
    for child in &e.children {
        if let Node::Text(text) = child {
            push_text(&mut out, text);
        }
    }
    trim_inlines(out)
}

#[derive(Default)]
pub(super) struct Converter {
    used_ids: BTreeSet<String>,
    in_list_item: std::cell::Cell<bool>,
    /// When set, the inline finishing pass runs in code context: text becomes verbatim code, so
    /// `smart` emits curly glyphs in place of [`Inline::Quoted`] and the math forms are not scanned.
    in_code: std::cell::Cell<bool>,
    /// When set, an inline tag with no structural mapping is kept verbatim as a raw HTML inline
    /// (open tag, parsed inner content, close tag) instead of being unwrapped to its children. Used
    /// when parsing a standalone inline fragment, where an unknown tag carries meaning the consumer
    /// may want to round-trip.
    preserve_unknown_tags: bool,
    /// The enabled extension set. Structural extensions (`native_divs`, `native_spans`,
    /// `auto_identifiers`, `line_blocks`) gate how block and inline wrappers are emitted; the text
    /// extensions (`smart`, the TeX math forms) drive the inline finishing pass.
    ext: Extensions,
}

impl Converter {
    pub(super) fn new(ext: Extensions) -> Self {
        Self {
            ext,
            ..Self::default()
        }
    }

    pub(super) fn blocks(&mut self, nodes: &[&Node], in_list: bool) -> Vec<Block> {
        let mut out = Vec::new();
        let mut pending = Vec::new();
        self.process(nodes.iter().copied(), &mut out, &mut pending);
        flush(&mut pending, &mut out);
        fix_plains(out, in_list)
    }

    fn child_blocks(&mut self, children: &[Node], in_list: bool) -> Vec<Block> {
        let refs: Vec<&Node> = children.iter().collect();
        self.blocks(&refs, in_list)
    }

    fn line_block_lines(&self, children: &[Node]) -> Vec<Vec<Inline>> {
        let inlines = self.build_inlines(children);
        inlines
            .split(|inline| matches!(inline, Inline::LineBreak))
            .map(|line| trim_inlines(line.to_vec()))
            .collect()
    }

    fn process<'a>(
        &mut self,
        nodes: impl Iterator<Item = &'a Node>,
        out: &mut Vec<Block>,
        pending: &mut Vec<Inline>,
    ) {
        for node in nodes {
            match node {
                Node::Text(text) => push_text(pending, text),
                Node::Element(e) => {
                    if e.name == "head" {
                        continue;
                    }
                    if e.name == "script" && !is_math_script(e) {
                        flush(pending, out);
                        continue;
                    }
                    if e.name == "style" && is_blank_run(pending) {
                        continue;
                    }
                    if let Some(kind) = block_kind(&e.name) {
                        flush(pending, out);
                        self.emit_block(kind, e, out);
                    } else if is_inline_element(&e.name) {
                        self.append_inline(pending, node);
                    } else {
                        self.process(e.children.iter(), out, pending);
                    }
                }
            }
        }
    }

    fn emit_block(&mut self, kind: BlockKind, e: &Element, out: &mut Vec<Block>) {
        match kind {
            BlockKind::Para => {
                let inlines = trim_inlines(self.build_inlines(&e.children));
                if inlines.is_empty() {
                    return;
                }
                if contains_checkbox(e) {
                    out.push(Block::Plain(inlines));
                } else {
                    out.push(Block::Para(inlines));
                }
            }
            BlockKind::Header(level) => {
                let inlines = trim_inlines(self.build_inlines(&e.children));
                let attr = self.header_attr(e, &inlines);
                out.push(Block::Header(level, attr, inlines));
            }
            BlockKind::BulletList => out.push(Block::BulletList(self.list_items(e))),
            BlockKind::OrderedList => {
                out.push(Block::OrderedList(list_attributes(e), self.list_items(e)));
            }
            BlockKind::BlockQuote => {
                out.push(Block::BlockQuote(self.child_blocks(&e.children, false)));
            }
            BlockKind::Pre => out.push(Self::code_block(e)),
            BlockKind::HorizontalRule => out.push(Block::HorizontalRule),
            BlockKind::Div { sectioning } => {
                if self.ext.contains(Extension::LineBlocks) && !sectioning && is_line_block_div(e) {
                    out.push(Block::LineBlock(self.line_block_lines(&e.children)));
                } else if self.ext.contains(Extension::NativeDivs) {
                    let attr = div_attr(e, sectioning);
                    out.push(Block::Div(attr, self.child_blocks(&e.children, false)));
                } else {
                    // `native_divs` off: the wrapper carries no document structure, so its content
                    // is spliced into the surrounding block flow.
                    out.extend(self.child_blocks(&e.children, false));
                }
            }
            BlockKind::DefinitionList => out.push(self.definition_list(e)),
            BlockKind::Table => out.push(self.table(e)),
            BlockKind::Figure => out.push(self.figure(e)),
        }
    }

    fn list_items(&mut self, e: &Element) -> Vec<Vec<Block>> {
        let mut items: Vec<Vec<&Node>> = Vec::new();
        for node in &e.children {
            match node {
                Node::Element(item) if item.name == "li" => {
                    items.push(item.children.iter().collect());
                }
                Node::Text(text) if text.trim().is_empty() => {}
                _ => match items.last_mut() {
                    Some(current) => current.push(node),
                    None => items.push(vec![node]),
                },
            }
        }
        items
            .into_iter()
            .map(|nodes| {
                let previous = self.in_list_item.replace(true);
                let blocks = self.blocks(&nodes, true);
                self.in_list_item.set(previous);
                blocks
            })
            .collect()
    }

    fn definition_list(&mut self, e: &Element) -> Block {
        let mut items: Vec<(Vec<Inline>, Vec<Vec<Block>>)> = Vec::new();
        let mut current: Option<(Vec<Inline>, Vec<Vec<Block>>)> = None;
        for child in &e.children {
            let Node::Element(item) = child else { continue };
            match item.name.as_str() {
                "dt" => {
                    let term = trim_inlines(self.build_inlines(&item.children));
                    match current.as_mut() {
                        Some((existing_term, definitions)) if definitions.is_empty() => {
                            existing_term.push(Inline::LineBreak);
                            existing_term.extend(term);
                        }
                        _ => {
                            if let Some(done) = current.take() {
                                items.push(done);
                            }
                            current = Some((term, Vec::new()));
                        }
                    }
                }
                "dd" => {
                    let definition = self.child_blocks(&item.children, true);
                    current
                        .get_or_insert_with(|| (Vec::new(), Vec::new()))
                        .1
                        .push(definition);
                }
                _ => {}
            }
        }
        if let Some(done) = current {
            items.push(done);
        }
        Block::DefinitionList(items)
    }

    fn figure(&mut self, e: &Element) -> Block {
        let attr = extract_attr(e, &[]);
        let mut caption = Caption::default();
        let mut content_nodes: Vec<&Node> = Vec::new();
        for child in &e.children {
            if let Node::Element(inner) = child
                && inner.name == "figcaption"
            {
                caption = Caption {
                    short: None,
                    long: self.child_blocks(&inner.children, false),
                };
                continue;
            }
            content_nodes.push(child);
        }
        Block::Figure(attr, caption, self.blocks(&content_nodes, true))
    }

    fn code_block(e: &Element) -> Block {
        let mut attr = extract_attr(e, &[]);
        let inner_code = e.children.iter().find_map(|node| match node {
            Node::Element(inner) if inner.name == "code" => Some(inner),
            _ => None,
        });
        let content_source = if let Some(code) = inner_code {
            let mut code_attr = extract_attr(code, &[]);
            for class in &mut code_attr.classes {
                if let Some(rest) = class.strip_prefix("language-") {
                    *class = rest.to_string();
                }
            }
            merge_attr(&mut attr, code_attr);
            code
        } else {
            e
        };
        let mut text = collect_text(content_source);
        if text.ends_with('\n') {
            text.pop();
        }
        Block::CodeBlock(attr, text)
    }

    fn table(&mut self, e: &Element) -> Block {
        let attr = extract_attr(e, &[]);
        let mut caption = Caption::default();
        let mut col_widths: Vec<ColWidth> = Vec::new();
        let mut head_rows = Vec::new();
        let mut body_rows = Vec::new();
        let mut foot_rows = Vec::new();
        let mut body_row_elements: Vec<&Element> = Vec::new();
        for child in &e.children {
            let Node::Element(section) = child else {
                continue;
            };
            match section.name.as_str() {
                "caption" => {
                    caption = Caption {
                        short: None,
                        long: self.child_blocks(&section.children, false),
                    };
                }
                "colgroup" => col_widths = column_widths(section),
                "thead" => head_rows.extend(self.rows(section)),
                "tbody" => {
                    body_rows.extend(self.rows(section));
                    body_row_elements.extend(row_elements(section));
                }
                "tfoot" => foot_rows.extend(self.rows(section)),
                "tr" => {
                    body_rows.push(self.row(section));
                    body_row_elements.push(section);
                }
                _ => {}
            }
        }
        let row_head_columns = row_head_columns(&body_row_elements);

        let columns = table_width(&head_rows, &body_rows, &foot_rows, col_widths.len());
        let aligns = column_alignments(body_rows.first().or_else(|| head_rows.first()), columns);
        let col_specs = (0..columns)
            .map(|i| ColSpec {
                align: aligns.get(i).cloned().unwrap_or(Alignment::AlignDefault),
                width: col_widths
                    .get(i)
                    .cloned()
                    .unwrap_or(ColWidth::ColWidthDefault),
            })
            .collect();

        Block::Table(Box::new(Table {
            attr,
            caption,
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: head_rows,
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns,
                head: Vec::new(),
                body: body_rows,
            }],
            foot: TableFoot {
                attr: Attr::default(),
                rows: foot_rows,
            },
        }))
    }

    fn rows(&mut self, section: &Element) -> Vec<Row> {
        section
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Element(tr) if tr.name == "tr" => Some(self.row(tr)),
                _ => None,
            })
            .collect()
    }

    fn row(&mut self, tr: &Element) -> Row {
        let cells = tr
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Element(cell) if cell.name == "td" || cell.name == "th" => {
                    Some(self.cell(cell))
                }
                _ => None,
            })
            .collect();
        Row {
            attr: Attr::default(),
            cells,
        }
    }

    fn cell(&mut self, cell: &Element) -> Cell {
        let mut attr = extract_attr(cell, &["align", "rowspan", "colspan"]);
        normalize_cell_style(&mut attr);
        Cell {
            attr,
            align: cell_alignment(cell),
            row_span: span_attr(cell, "rowspan"),
            col_span: span_attr(cell, "colspan"),
            content: self.child_blocks(&cell.children, false),
        }
    }

    fn header_attr(&mut self, e: &Element, inlines: &[Inline]) -> Attr {
        let mut attr = extract_attr(e, &[]);
        if !attr.id.is_empty() {
            self.used_ids.insert(attr.id.clone());
        } else if self.ext.contains(Extension::AutoIdentifiers) {
            let base = slug(&to_plain_text(inlines));
            let base = if base.is_empty() {
                "section".to_string()
            } else {
                base
            };
            attr.id = self.unique_id(base);
        }
        // `auto_identifiers` off: a heading without an explicit id keeps an empty one.
        attr
    }

    fn unique_id(&mut self, base: String) -> String {
        if self.used_ids.insert(base.clone()) {
            return base;
        }
        let mut n = 1;
        loop {
            let candidate = format!("{base}-{n}");
            if self.used_ids.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }

    fn build_inlines(&self, nodes: &[Node]) -> Vec<Inline> {
        let mut out = Vec::new();
        for node in nodes {
            self.append_inline(&mut out, node);
        }
        out
    }

    fn smart(&self) -> bool {
        self.ext.contains(Extension::Smart)
    }

    /// Whether any TeX math form is enabled.
    fn math_active(&self) -> bool {
        self.ext.contains(Extension::TexMathDollars)
            || self.ext.contains(Extension::TexMathSingleBackslash)
            || self.ext.contains(Extension::TexMathDoubleBackslash)
    }

    /// Append a text node, applying the inline finishing pass. Verbatim by default; with `smart` the
    /// quotes, dashes, and ellipses become typographic forms, and with a math form enabled the TeX
    /// delimiters become [`Inline::Math`]. In code context the text stays a code run, so `smart`
    /// emits curly glyphs rather than [`Inline::Quoted`] and math is never scanned.
    fn append_text(&self, out: &mut Vec<Inline>, text: &str) {
        if self.in_code.get() {
            if self.smart() || self.math_active() {
                let chars: Vec<char> = text.chars().collect();
                let items = self.scan_items(&chars);
                emit_code(&items, self.smart(), out);
            } else {
                push_text(out, text);
            }
        } else if self.smart() {
            let chars: Vec<char> = text.chars().collect();
            let items = self.scan_items(&chars);
            for inline in resolve_smart(&items, 0, items.len()) {
                absorb(out, inline);
            }
        } else if self.math_active() {
            let chars: Vec<char> = text.chars().collect();
            let items = self.scan_items(&chars);
            emit_math_only(&items, out);
        } else {
            push_text(out, text);
        }
    }

    /// Split a text node into literal characters and math spans. Math is scanned greedily from the
    /// left; a delimiter that opens no valid span stays a literal character.
    fn scan_items(&self, chars: &[char]) -> Vec<Item> {
        let mut items = Vec::new();
        let math = self.math_active();
        let mut i = 0;
        while let Some(&c) = chars.get(i) {
            if math && let Some((math_type, content, next)) = self.try_math(chars, i) {
                items.push(Item::Math(math_type, content));
                i = next;
                continue;
            }
            items.push(Item::Lit(c));
            i += 1;
        }
        items
    }

    /// Try to read a math span at `i`. A backslash opener prefers the double-backslash form; a
    /// `$$` opener is display math and a lone `$` is inline math, each only when its form is enabled.
    fn try_math(&self, chars: &[char], i: usize) -> Option<(MathType, String, usize)> {
        match chars.get(i)? {
            '\\' => {
                if self.ext.contains(Extension::TexMathDoubleBackslash)
                    && chars.get(i + 1) == Some(&'\\')
                    && let Some(found) = scan_backslash_math(chars, i, 2)
                {
                    return Some(found);
                }
                if self.ext.contains(Extension::TexMathSingleBackslash) {
                    return scan_backslash_math(chars, i, 1);
                }
                None
            }
            '$' if self.ext.contains(Extension::TexMathDollars) => {
                if chars.get(i + 1) == Some(&'$') {
                    scan_display_math(chars, i).map(|(c, n)| (MathType::DisplayMath, c, n))
                } else {
                    scan_inline_math(chars, i).map(|(c, n)| (MathType::InlineMath, c, n))
                }
            }
            _ => None,
        }
    }

    fn append_inline(&self, out: &mut Vec<Inline>, node: &Node) {
        let e = match node {
            Node::Text(text) => {
                self.append_text(out, text);
                return;
            }
            Node::Element(e) => e,
        };
        match inline_kind(&e.name) {
            InlineKind::Emph => out.push(Inline::Emph(self.build_inlines(&e.children))),
            InlineKind::Strong => out.push(Inline::Strong(self.build_inlines(&e.children))),
            InlineKind::Strikeout => out.push(Inline::Strikeout(self.build_inlines(&e.children))),
            InlineKind::Underline => out.push(Inline::Underline(self.build_inlines(&e.children))),
            InlineKind::Superscript => {
                out.push(Inline::Superscript(self.build_inlines(&e.children)));
            }
            InlineKind::Subscript => out.push(Inline::Subscript(self.build_inlines(&e.children))),
            InlineKind::Quoted => {
                out.push(Inline::Quoted(
                    QuoteType::DoubleQuote,
                    self.build_inlines(&e.children),
                ));
            }
            InlineKind::LineBreak => out.push(Inline::LineBreak),
            InlineKind::Span => {
                let inner = self.build_inlines(&e.children);
                if !self.ext.contains(Extension::NativeSpans) {
                    // `native_spans` off: a bare `<span>` carries no inline structure, so it
                    // unwraps to its content (the small-caps style is likewise dropped).
                    out.extend(inner);
                } else if is_small_caps(e) {
                    out.push(Inline::SmallCaps(inner));
                } else {
                    out.push(Inline::Span(extract_attr(e, &[]), inner));
                }
            }
            InlineKind::Bdo => {
                let inner = self.build_inlines(&e.children);
                if let Some(dir) = attr_value(e, "dir") {
                    let attr = Attr {
                        id: String::new(),
                        classes: Vec::new(),
                        attributes: vec![("dir".to_string(), dir)],
                    };
                    out.push(Inline::Span(attr, inner));
                } else {
                    out.extend(inner);
                }
            }
            InlineKind::SpanClass => {
                let mut attr = extract_attr(e, &[]);
                attr.classes.insert(0, e.name.clone());
                out.push(Inline::Span(attr, self.build_inlines(&e.children)));
            }
            InlineKind::Code(class) => self.code_inline(out, e, class),
            InlineKind::Anchor => self.anchor(out, e),
            InlineKind::Image => out.push(image(e)),
            InlineKind::Style => {
                out.push(Inline::RawInline(
                    Format("html".to_string()),
                    serialize_element(e),
                ));
            }
            InlineKind::Script => {
                if let Some(math_type) = math_script_type(e) {
                    out.push(Inline::Math(math_type, collect_text(e)));
                }
            }
            InlineKind::Input => {
                if is_checkbox(e) && self.in_list_item.get() {
                    let symbol = if attr_value(e, "checked").is_some() {
                        '\u{2612}'
                    } else {
                        '\u{2610}'
                    };
                    out.push(Inline::Str(symbol.to_string()));
                    out.push(Inline::Space);
                }
            }
            InlineKind::Transparent => {
                if self.preserve_unknown_tags && block_kind(&e.name).is_none() {
                    let format = Format("html".to_string());
                    if e.end_only {
                        out.push(Inline::RawInline(format, close_tag(&e.name)));
                    } else {
                        out.push(Inline::RawInline(format.clone(), open_tag(e)));
                        if !e.void {
                            for child in &e.children {
                                self.append_inline(out, child);
                            }
                            if e.closed {
                                out.push(Inline::RawInline(format, close_tag(&e.name)));
                            }
                        }
                    }
                } else {
                    for child in &e.children {
                        self.append_inline(out, child);
                    }
                }
            }
        }
    }

    fn code_inline(&self, out: &mut Vec<Inline>, e: &Element, forced_class: Option<&str>) {
        let mut attr = extract_attr(e, &[]);
        if let Some(class) = forced_class {
            attr.classes = vec![class.to_string()];
        }
        let has_elements = e
            .children
            .iter()
            .any(|node| matches!(node, Node::Element(_)));
        if has_elements || self.smart() || self.math_active() {
            let previous = self.in_code.replace(true);
            let inner = self.build_inlines(&e.children);
            self.in_code.set(previous);
            codify(out, inner, &attr);
        } else {
            out.push(Inline::Code(attr, collect_text(e)));
        }
    }

    fn anchor(&self, out: &mut Vec<Inline>, e: &Element) {
        let inner = self.build_inlines(&e.children);
        let (leading, trimmed, trailing) = hoist_edge_whitespace(inner);
        let mut attr = extract_attr(e, &["href", "title", "name"]);
        if attr.id.is_empty()
            && let Some(name) = attr_value(e, "name")
        {
            attr.id = name;
        }
        if let Some(lead) = leading {
            out.push(lead);
        }
        let anchor = if let Some(href) = attr_value(e, "href") {
            let title = attr_value(e, "title").unwrap_or_default();
            Inline::Link(attr, trimmed, Target { url: href, title })
        } else {
            Inline::Span(attr, trimmed)
        };
        out.push(anchor);
        if let Some(trail) = trailing {
            out.push(trail);
        }
    }
}

/// Render the contents of a `<code>` element that carries inline markup: each run of text becomes a
/// [`Inline::Code`] carrying the element's attributes, while container inlines keep their structure
/// with their own text runs codified in turn.
fn codify(out: &mut Vec<Inline>, inlines: Vec<Inline>, attr: &Attr) {
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Vec<Inline>| {
        if !run.is_empty() {
            out.push(Inline::Code(attr.clone(), std::mem::take(run)));
        }
    };
    for inline in inlines {
        match inline {
            Inline::Str(text) => run.push_str(&text),
            Inline::Space | Inline::SoftBreak => run.push(' '),
            Inline::Emph(children) => {
                flush(&mut run, out);
                out.push(Inline::Emph(codified(children, attr)));
            }
            Inline::Strong(children) => {
                flush(&mut run, out);
                out.push(Inline::Strong(codified(children, attr)));
            }
            Inline::Strikeout(children) => {
                flush(&mut run, out);
                out.push(Inline::Strikeout(codified(children, attr)));
            }
            Inline::Underline(children) => {
                flush(&mut run, out);
                out.push(Inline::Underline(codified(children, attr)));
            }
            Inline::Superscript(children) => {
                flush(&mut run, out);
                out.push(Inline::Superscript(codified(children, attr)));
            }
            Inline::Subscript(children) => {
                flush(&mut run, out);
                out.push(Inline::Subscript(codified(children, attr)));
            }
            Inline::SmallCaps(children) => {
                flush(&mut run, out);
                out.push(Inline::SmallCaps(codified(children, attr)));
            }
            Inline::Span(span_attr, children) => {
                flush(&mut run, out);
                out.push(Inline::Span(span_attr, codified(children, attr)));
            }
            Inline::Link(link_attr, children, target) => {
                flush(&mut run, out);
                out.push(Inline::Link(link_attr, codified(children, attr), target));
            }
            other => {
                flush(&mut run, out);
                out.push(other);
            }
        }
    }
    flush(&mut run, out);
}

fn codified(inlines: Vec<Inline>, attr: &Attr) -> Vec<Inline> {
    let mut out = Vec::new();
    codify(&mut out, inlines, attr);
    out
}

/// A unit of a text node during the inline finishing pass: a literal character, or a math span
/// already lifted out of the surrounding text.
enum Item {
    Lit(char),
    Math(MathType, String),
}

/// The curly quote glyphs the smart pass produces.
const LEFT_DOUBLE: char = '\u{201C}';
const RIGHT_DOUBLE: char = '\u{201D}';
const LEFT_SINGLE: char = '\u{2018}';
const APOSTROPHE: char = '\u{2019}';

/// Whether a quote at `i` may open: it must follow an opening context — the node start, a math span,
/// whitespace, or one of `.`, `-`, `\`, `"`, `'`, or a curly quote — and be followed by a
/// non-whitespace character. A quote glued to a letter, digit, or most punctuation cannot open.
fn can_open(items: &[Item], i: usize) -> bool {
    let opens_after = match i.checked_sub(1).and_then(|prev| items.get(prev)) {
        // The node start and a preceding math span are both opening contexts.
        None | Some(Item::Math(..)) => true,
        Some(Item::Lit(c)) => {
            c.is_whitespace()
                || matches!(
                    *c,
                    '.' | '-'
                        | '\\'
                        | '"'
                        | '\''
                        | LEFT_SINGLE
                        | APOSTROPHE
                        | LEFT_DOUBLE
                        | RIGHT_DOUBLE
                )
        }
    };
    let followed_by_nonspace = match items.get(i + 1) {
        Some(Item::Math(..)) => true,
        Some(Item::Lit(c)) => !c.is_whitespace(),
        None => false,
    };
    opens_after && followed_by_nonspace
}

/// The index in `from..hi` of the next double quote, which closes a double-quoted span.
fn find_next_double(items: &[Item], from: usize, hi: usize) -> Option<usize> {
    (from..hi).find(|&j| matches!(items.get(j), Some(Item::Lit('"'))))
}

/// The index in `from..hi` of the single quote that closes a single-quoted span: the first one not
/// glued to a following letter or digit, so a contraction's apostrophe is skipped over.
fn find_single_close(items: &[Item], from: usize, hi: usize) -> Option<usize> {
    (from..hi).find(|&j| {
        matches!(items.get(j), Some(Item::Lit('\'')))
            && !matches!(items.get(j + 1), Some(Item::Lit(c)) if c.is_alphanumeric())
    })
}

/// Resolve a span of items into smart inlines: quotes pair into [`Inline::Quoted`], an unpaired
/// quote reverts to its curly glyph, math spans pass through, and the literal runs between them fold
/// dashes and ellipses before collapsing whitespace.
fn resolve_smart(items: &[Item], lo: usize, hi: usize) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = lo;
    while i < hi {
        match items.get(i) {
            Some(Item::Math(math_type, content)) => {
                flush_run(&mut buf, &mut out);
                out.push(Inline::Math(math_type.clone(), content.clone()));
                i += 1;
            }
            Some(Item::Lit('"')) => {
                if can_open(items, i)
                    && let Some(j) = find_next_double(items, i + 1, hi)
                {
                    flush_run(&mut buf, &mut out);
                    out.push(Inline::Quoted(
                        QuoteType::DoubleQuote,
                        resolve_smart(items, i + 1, j),
                    ));
                    i = j + 1;
                } else {
                    // An opener with no closer is a left quote; a quote that cannot open is a right
                    // quote.
                    buf.push(if can_open(items, i) {
                        LEFT_DOUBLE
                    } else {
                        RIGHT_DOUBLE
                    });
                    i += 1;
                }
            }
            Some(Item::Lit('\'')) => {
                if can_open(items, i)
                    && let Some(j) = find_single_close(items, i + 1, hi)
                {
                    flush_run(&mut buf, &mut out);
                    out.push(Inline::Quoted(
                        QuoteType::SingleQuote,
                        resolve_smart(items, i + 1, j),
                    ));
                    i = j + 1;
                } else {
                    // An unpaired single quote is always an apostrophe.
                    buf.push(APOSTROPHE);
                    i += 1;
                }
            }
            Some(Item::Lit(c)) => {
                buf.push(*c);
                i += 1;
            }
            None => break,
        }
    }
    flush_run(&mut buf, &mut out);
    out
}

/// Emit a text node when math forms are enabled but `smart` is not: literal runs stay verbatim
/// (subject only to whitespace collapse) and math spans pass through.
fn emit_math_only(items: &[Item], out: &mut Vec<Inline>) {
    let mut buf = String::new();
    for item in items {
        match item {
            Item::Math(math_type, content) => {
                if !buf.is_empty() {
                    push_text(out, &buf);
                    buf.clear();
                }
                out.push(Inline::Math(math_type.clone(), content.clone()));
            }
            Item::Lit(c) => buf.push(*c),
        }
    }
    if !buf.is_empty() {
        push_text(out, &buf);
    }
}

/// Flush a literal run into `out`: fold its dashes and ellipses, then collapse whitespace into the
/// surrounding inline breaks.
fn flush_run(buf: &mut String, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        push_text(out, &fold_smart_punct(buf));
        buf.clear();
    }
}

/// Emit the text of an inline `<code>` element under `smart` and/or a math form. Top-level math spans
/// lift out as bare [`Inline::Math`]; the verbatim text between them becomes [`Inline::Str`] runs
/// (which [`codify`] then wraps as code), with whitespace collapsed to single spaces and — under
/// `smart` — dashes, ellipses, and paired quotes rendered as their typographic glyphs.
fn emit_code(items: &[Item], smart: bool, out: &mut Vec<Inline>) {
    let hi = items.len();
    let mut result = String::new();
    let mut run = String::new();
    let mut i = 0;
    while i < hi {
        match items.get(i) {
            Some(Item::Math(math_type, content)) => {
                finalize_run(&mut run, &mut result, smart);
                if !result.is_empty() {
                    push_str(out, &result);
                    result.clear();
                }
                out.push(Inline::Math(math_type.clone(), content.clone()));
                i += 1;
            }
            Some(Item::Lit('"')) if smart => {
                if can_open(items, i)
                    && let Some(j) = find_next_double(items, i + 1, hi)
                {
                    finalize_run(&mut run, &mut result, smart);
                    result.push(LEFT_DOUBLE);
                    result.push_str(&code_build(items, i + 1, j));
                    result.push(RIGHT_DOUBLE);
                    i = j + 1;
                } else {
                    run.push(if can_open(items, i) {
                        LEFT_DOUBLE
                    } else {
                        RIGHT_DOUBLE
                    });
                    i += 1;
                }
            }
            Some(Item::Lit('\'')) if smart => {
                if can_open(items, i)
                    && let Some(j) = find_single_close(items, i + 1, hi)
                {
                    finalize_run(&mut run, &mut result, smart);
                    result.push(LEFT_SINGLE);
                    result.push_str(&code_build(items, i + 1, j));
                    result.push(APOSTROPHE);
                    i = j + 1;
                } else {
                    run.push(APOSTROPHE);
                    i += 1;
                }
            }
            Some(Item::Lit(c)) => {
                run.push(*c);
                i += 1;
            }
            None => break,
        }
    }
    finalize_run(&mut run, &mut result, smart);
    if !result.is_empty() {
        push_str(out, &result);
    }
}

/// Build the flat code text of a quote-delimited span: nested quotes become glyphs and math flattens
/// to its content, since the whole span renders as one code string. The result is already finalized,
/// so the caller appends it verbatim.
fn code_build(items: &[Item], lo: usize, hi: usize) -> String {
    let mut result = String::new();
    let mut run = String::new();
    let mut i = lo;
    while i < hi {
        match items.get(i) {
            Some(Item::Math(_, content)) => {
                finalize_run(&mut run, &mut result, true);
                result.push_str(content);
                i += 1;
            }
            Some(Item::Lit('"')) => {
                if can_open(items, i)
                    && let Some(j) = find_next_double(items, i + 1, hi)
                {
                    finalize_run(&mut run, &mut result, true);
                    result.push(LEFT_DOUBLE);
                    result.push_str(&code_build(items, i + 1, j));
                    result.push(RIGHT_DOUBLE);
                    i = j + 1;
                } else {
                    run.push(if can_open(items, i) {
                        LEFT_DOUBLE
                    } else {
                        RIGHT_DOUBLE
                    });
                    i += 1;
                }
            }
            Some(Item::Lit('\'')) => {
                if can_open(items, i)
                    && let Some(j) = find_single_close(items, i + 1, hi)
                {
                    finalize_run(&mut run, &mut result, true);
                    result.push(LEFT_SINGLE);
                    result.push_str(&code_build(items, i + 1, j));
                    result.push(APOSTROPHE);
                    i = j + 1;
                } else {
                    run.push(APOSTROPHE);
                    i += 1;
                }
            }
            Some(Item::Lit(c)) => {
                run.push(*c);
                i += 1;
            }
            None => break,
        }
    }
    finalize_run(&mut run, &mut result, true);
    result
}

/// Finalize a literal code run into `result`: fold its dashes and ellipses (under `smart`), then
/// collapse each whitespace span to a single space, joining cleanly with any text already there.
fn finalize_run(run: &mut String, result: &mut String, smart: bool) {
    if run.is_empty() {
        return;
    }
    let folded = if smart {
        fold_smart_punct(run)
    } else {
        std::mem::take(run)
    };
    run.clear();
    let mut prev_space = result.ends_with(' ');
    for ch in folded.chars() {
        if ch.is_ascii_whitespace() {
            if !prev_space {
                result.push(' ');
                prev_space = true;
            }
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
}

/// Fold a run of `n` hyphens left to right: each group of three becomes an em dash (`—`), a final
/// pair becomes an en dash (`–`), and a single leftover hyphen stays literal.
fn fold_dashes(n: usize, out: &mut String) {
    let mut n = n;
    while n >= 3 {
        out.push('\u{2014}');
        n -= 3;
    }
    if n == 2 {
        out.push('\u{2013}');
    } else if n == 1 {
        out.push('-');
    }
}

/// Fold a literal run's typography: a run of hyphens becomes em and en dashes, and a run of dots
/// becomes ellipses with up to two trailing dots.
fn fold_smart_punct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '-' => {
                let mut n = 0;
                while chars.peek() == Some(&'-') {
                    chars.next();
                    n += 1;
                }
                fold_dashes(n, &mut out);
            }
            '.' => {
                let mut n = 0;
                while chars.peek() == Some(&'.') {
                    chars.next();
                    n += 1;
                }
                out.push_str(&fold_ellipsis_run(n));
            }
            _ => {
                out.push(c);
                chars.next();
            }
        }
    }
    out
}

/// Merge a finished inline into a run, joining adjacent strings and collapsing adjacent breaks the
/// way [`push_text`] does, so a smart pass over one text node fuses cleanly with its neighbors.
fn absorb(out: &mut Vec<Inline>, inline: Inline) {
    match inline {
        Inline::Str(text) => push_str(out, &text),
        Inline::Space => push_break(out, false),
        Inline::SoftBreak => push_break(out, true),
        other => out.push(other),
    }
}

/// Whether a `<span>` requests small-caps rendering, either through the `smallcaps` class or a
/// `font-variant: small-caps` style declaration.
fn is_small_caps(e: &Element) -> bool {
    if e.attrs
        .iter()
        .any(|(key, value)| key == "class" && value.split_whitespace().any(|c| c == "smallcaps"))
    {
        return true;
    }
    attr_value(e, "style")
        .and_then(|style| style_property(&style, "font-variant"))
        .is_some_and(|value| value.eq_ignore_ascii_case("small-caps"))
}

/// Split an anchor's content into the whitespace at its edges and the trimmed middle. Leading and
/// trailing breaks belong outside the anchor in HTML rendering, so they are returned to the caller to
/// place around it.
fn hoist_edge_whitespace(
    mut inlines: Vec<Inline>,
) -> (Option<Inline>, Vec<Inline>, Option<Inline>) {
    let leading = if matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        Some(inlines.remove(0))
    } else {
        None
    };
    let trailing = if matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop()
    } else {
        None
    };
    (leading, inlines, trailing)
}

/// Serialize an element's start tag, e.g. `<cite id="1">`. Attribute names are emitted in source
/// order; a value-less attribute is written bare, and special characters in values are escaped.
fn open_tag(e: &Element) -> String {
    let mut out = String::from("<");
    out.push_str(&e.name);
    for (key, value) in &e.attrs {
        out.push(' ');
        out.push_str(key);
        if !value.is_empty() {
            out.push_str("=\"");
            push_escaped_attr_value(&mut out, value);
            out.push('"');
        }
    }
    out.push('>');
    out
}

/// Serialize an element's end tag, e.g. `</cite>`.
fn close_tag(name: &str) -> String {
    let mut out = String::from("</");
    out.push_str(name);
    out.push('>');
    out
}

fn push_escaped_attr_value(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

fn image(e: &Element) -> Inline {
    let url = attr_value(e, "src").unwrap_or_default();
    let title = attr_value(e, "title").unwrap_or_default();
    let alt = attr_value(e, "alt").unwrap_or_default();
    let attr = extract_attr(e, &["src", "title", "alt"]);
    Inline::Image(attr, inlines_from_text(&alt), Target { url, title })
}

fn extract_attr(e: &Element, exclude: &[&str]) -> Attr {
    let mut attr = Attr::default();
    for (key, value) in &e.attrs {
        if exclude.contains(&key.as_str()) {
            continue;
        }
        match key.as_str() {
            "id" => attr.id.clone_from(value),
            "class" => attr.classes = value.split_whitespace().map(str::to_string).collect(),
            _ => {
                let name = key.strip_prefix("data-").unwrap_or(key).to_string();
                attr.attributes.push((name, value.clone()));
            }
        }
    }
    attr
}

fn is_line_block_div(e: &Element) -> bool {
    let attr = extract_attr(e, &[]);
    attr.id.is_empty() && attr.attributes.is_empty() && attr.classes == ["line-block"]
}

fn div_attr(e: &Element, sectioning: bool) -> Attr {
    let mut attr = extract_attr(e, &[]);
    if sectioning {
        attr.classes.insert(0, e.name.clone());
    }
    attr
}

fn merge_attr(base: &mut Attr, other: Attr) {
    if base.id.is_empty() {
        base.id = other.id;
    }
    base.classes.extend(other.classes);
    base.attributes.extend(other.attributes);
}

fn list_attributes(e: &Element) -> ListAttributes {
    let start = attr_value(e, "start")
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(1);
    let style = match attr_value(e, "type").as_deref() {
        Some("1") => ListNumberStyle::Decimal,
        Some("a") => ListNumberStyle::LowerAlpha,
        Some("A") => ListNumberStyle::UpperAlpha,
        Some("i") => ListNumberStyle::LowerRoman,
        Some("I") => ListNumberStyle::UpperRoman,
        _ => ListNumberStyle::DefaultStyle,
    };
    ListAttributes {
        start,
        style,
        delim: ListNumberDelim::DefaultDelim,
    }
}

fn inlines_from_text(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    push_text(&mut out, text);
    out
}

/// Append `text` to an inline run, collapsing each whitespace span to a single break: a span that
/// spans a line is a soft break, otherwise a space. Non-whitespace merges into the trailing string.
fn push_text(out: &mut Vec<Inline>, text: &str) {
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_whitespace() {
            let mut newline = false;
            while let Some(&w) = chars.peek() {
                if !w.is_ascii_whitespace() {
                    break;
                }
                newline |= w == '\n';
                chars.next();
            }
            push_break(out, newline);
        } else {
            let mut word = String::new();
            while let Some(&w) = chars.peek() {
                if w.is_ascii_whitespace() {
                    break;
                }
                word.push(w);
                chars.next();
            }
            push_str(out, &word);
        }
    }
}

fn push_str(out: &mut Vec<Inline>, word: &str) {
    if let Some(Inline::Str(existing)) = out.last_mut() {
        existing.push_str(word);
    } else {
        out.push(Inline::Str(word.to_string()));
    }
}

fn push_break(out: &mut Vec<Inline>, newline: bool) {
    match out.last() {
        Some(Inline::SoftBreak) => {}
        Some(Inline::Space) => {
            if newline {
                out.pop();
                out.push(Inline::SoftBreak);
            }
        }
        _ => out.push(if newline {
            Inline::SoftBreak
        } else {
            Inline::Space
        }),
    }
}

fn trim_inlines(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.remove(0);
    }
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
    inlines
}

/// Flush a loose inline run as a `Plain` block, dropping it when only whitespace remains.
fn flush(pending: &mut Vec<Inline>, out: &mut Vec<Block>) {
    let trimmed = trim_inlines(std::mem::take(pending));
    if !trimmed.is_empty() {
        out.push(Block::Plain(trimmed));
    }
}

/// A loose inline run is a `Plain` block until a paragraph-like sibling promotes the whole group to
/// `Para`. Nested lists do not promote runs inside an enclosing list item.
fn fix_plains(blocks: Vec<Block>, in_list: bool) -> Vec<Block> {
    if !blocks.iter().any(|block| is_paraish(block, in_list)) {
        return blocks;
    }
    blocks
        .into_iter()
        .map(|block| match block {
            Block::Plain(inlines) => Block::Para(inlines),
            other => other,
        })
        .collect()
}

fn is_paraish(block: &Block, in_list: bool) -> bool {
    match block {
        Block::Para(_) | Block::Header(..) | Block::BlockQuote(_) | Block::CodeBlock(..) => true,
        Block::BulletList(_) | Block::OrderedList(..) => !in_list,
        _ => false,
    }
}

/// A `<script type="math/tex">` carries TeX in its body; `mode=display` selects display math.
fn math_script_type(e: &Element) -> Option<MathType> {
    let value = attr_value(e, "type")?;
    let value = value.to_ascii_lowercase();
    if !value.starts_with("math/tex") {
        return None;
    }
    if value.contains("mode=display") {
        Some(MathType::DisplayMath)
    } else {
        Some(MathType::InlineMath)
    }
}

fn is_math_script(e: &Element) -> bool {
    math_script_type(e).is_some()
}

fn is_blank_run(inlines: &[Inline]) -> bool {
    inlines.iter().all(|inline| {
        matches!(
            inline,
            Inline::Space | Inline::SoftBreak | Inline::LineBreak
        )
    })
}

fn is_checkbox(e: &Element) -> bool {
    e.name == "input"
        && attr_value(e, "type").is_some_and(|kind| kind.eq_ignore_ascii_case("checkbox"))
}

fn contains_checkbox(e: &Element) -> bool {
    e.children.iter().any(|node| match node {
        Node::Element(child) => is_checkbox(child) || contains_checkbox(child),
        Node::Text(_) => false,
    })
}
