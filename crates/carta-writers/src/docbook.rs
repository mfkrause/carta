//! `DocBook` writer: renders the document model to `DocBook` 5 XML.
//!
//! Elements nest two columns per level and paragraph text is reflowed so a line's full width, its
//! indent included, stays within the fill column. Verbatim constructs (code listings, and
//! paragraphs carrying a hard break) are flushed to column zero instead, so their own line
//! structure survives untouched. Headings sectionize the block sequence: each one opens a
//! `<section>` holding the blocks that follow it up to the next heading of the same or shallower
//! level. Output carries no trailing newline; the caller appends one.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Format, Inline,
    ListAttributes, ListNumberStyle, MathType, Row, Table, Target, Text,
};
use carta_core::{Result, WrapMode, Writer, WriterOptions};

use crate::common::{
    Dimension, FILL_COLUMN, Piece, attribute_value, display_width, escape_xml, fill, fill_offset,
    format_length_dimension, format_percent_dimension, indent_block, parse_dimension, quote_marks,
};

/// Stack headroom below which the block and inline walks reserve another segment.
const STACK_RED_ZONE: usize = 128 * 1024;

/// Stack reserved when the walk runs low, sized so deeply nested input keeps recursing.
const STACK_SEGMENT: usize = 32 * 1024 * 1024;

/// Columns added per nesting level.
const STEP: usize = 2;

/// The namespaces a fragment's outermost sections declare, so the fragment stands alone as `DocBook`.
const NAMESPACES: &str =
    " xmlns=\"http://docbook.org/ns/docbook\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"";

/// The admonition elements a division selects through its leading class.
const ADMONITIONS: [&str; 6] = ["note", "warning", "tip", "caution", "important", "danger"];

/// The deepest nesting a `<section>` covers, counted from the shallowest level left to sections; a
/// heading below it opens a `<simplesect>` instead.
const DEEPEST_SECTION: i64 = 5;

/// The class on a heading that carries over as the section's role.
const UNNUMBERED: &str = "unnumbered";

/// Opens a line that must end up at column zero however deeply the surrounding layout is indented.
/// Document text cannot carry it: XML admits no such character, so escaping drops it.
const FLUSH: char = carta_core::FLUSH_LINE;

/// Renders a document to `DocBook` 5 XML.
#[derive(Debug, Default, Clone, Copy)]
pub struct DocbookWriter;

impl Writer for DocbookWriter {
    fn render_document(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        // A wrapping template seats the content inside an enclosing element, which already declares
        // the namespaces and indents the slot the body lands in.
        let wrapped = options.standalone || options.template.is_some();
        let slot = options.slot;
        let renderer = Renderer {
            // The slot's indent rides on every line, so it comes off the column the text reflows to.
            width: options
                .columns
                .unwrap_or(FILL_COLUMN)
                .saturating_sub(slot.indent),
            opening_offset: slot.column.saturating_sub(slot.indent),
            wrap: options.wrap,
            top_level: top_heading_level(&document.blocks),
            catalog: LanguageCatalog::new(options),
            outer_divisions: options.top_level_division.outer_divisions(),
            namespaced: !wrapped,
        };
        let body = renderer.blocks(&document.blocks, 0, Nesting::SECTIONED);
        // Nothing closes the document, so the last line the content opened is the one the newline
        // written after the render stands for, and a second break falls away with it.
        let body = closed_up(closed_up(&body));
        // A template resolves the marks itself, after adding the indentation they have to survive.
        Ok(if wrapped {
            body.to_string()
        } else {
            unmark_flushed(body)
        })
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.docbook"))
    }

    fn render_meta_inlines(&self, inlines: &[Inline], options: &WriterOptions) -> Result<String> {
        let document = Document {
            blocks: vec![Block::Plain(inlines.to_vec())],
            ..Document::default()
        };
        self.render_fragment(&document, options)
    }

    fn render_meta_blocks(&self, blocks: &[Block], options: &WriterOptions) -> Result<String> {
        let document = Document {
            blocks: blocks.to_vec(),
            ..Document::default()
        };
        self.render_fragment(&document, options)
    }
}

impl DocbookWriter {
    /// Renders a document as a bare fragment, whatever the caller asked of the document as a whole:
    /// a template variable lands inside the enclosing element rather than replacing it.
    fn render_fragment(self, document: &Document, options: &WriterOptions) -> Result<String> {
        let mut options = options.clone();
        options.standalone = false;
        Ok(self
            .write(document, &options)?
            .trim_end_matches('\n')
            .to_string())
    }
}

/// The catalog that answers whether a code block's class names a language, and under what spelling.
#[cfg(feature = "highlight")]
struct LanguageCatalog {
    /// The caller's catalog, which may know definitions loaded at run time.
    supplied: Option<std::sync::Arc<carta_highlight::Highlighter>>,
    /// Answers lookups when the caller supplied no catalog.
    bundled: carta_highlight::Registry,
}

#[cfg(feature = "highlight")]
impl LanguageCatalog {
    fn new(options: &WriterOptions) -> Self {
        Self {
            supplied: options.highlight.highlighter.clone(),
            bundled: carta_highlight::Registry::default(),
        }
    }

    /// The `language` a code listing declares for a class: the class as written when it already
    /// names a language, the language's own name when the class is one of its file extensions, and
    /// nothing when no language answers to it.
    fn language(&self, class: &str) -> Option<String> {
        let registry = self
            .supplied
            .as_ref()
            .map_or(&self.bundled, |highlighter| highlighter.registry());
        language_in(registry, class)
    }
}

#[cfg(not(feature = "highlight"))]
struct LanguageCatalog {
    bundled: carta_highlight::Registry,
}

#[cfg(not(feature = "highlight"))]
impl LanguageCatalog {
    fn new(_options: &WriterOptions) -> Self {
        Self {
            bundled: carta_highlight::Registry::default(),
        }
    }

    fn language(&self, class: &str) -> Option<String> {
        language_in(&self.bundled, class)
    }
}

fn language_in(registry: &carta_highlight::Registry, class: &str) -> Option<String> {
    if let Some(language) = registry.resolve(class) {
        let name = language.name.to_lowercase();
        return Some(if name == class.to_lowercase() {
            class.to_owned()
        } else {
            name
        });
    }
    fallback_language(class).map(str::to_owned)
}

fn fallback_language(class: &str) -> Option<&'static str> {
    match class.to_ascii_lowercase().as_str() {
        "py" | "python" => Some("python"),
        _ => None,
    }
}

/// The layout settings and lookups one rendering pass shares.
struct Renderer {
    /// The column paragraph text is reflowed to, counted from the left margin.
    width: usize,
    /// Columns the surrounding layout has already spent on the line the content opens on, which
    /// shortens that one line: a value interpolated after markup on its template line starts there.
    opening_offset: usize,
    wrap: WrapMode,
    /// The shallowest heading level in the document: the level whose sections carry the namespaces.
    top_level: i64,
    catalog: LanguageCatalog,
    /// The division names the outermost heading levels take, shallowest first.
    outer_divisions: &'static [&'static str],
    /// Whether the outermost sections declare the namespaces, as a bare fragment must.
    namespaced: bool,
}

/// Where a block sequence sits, and so how the blocks in it lay themselves out.
#[derive(Clone, Copy)]
struct Nesting {
    /// A bare [`Block::Plain`] renders as a paragraph, the shape a list item's content takes.
    promote: bool,
    /// Headings divide the sequence into sections.
    sections: bool,
}

impl Nesting {
    /// The document's own sequence, and every section within it.
    const SECTIONED: Self = Self {
        promote: false,
        sections: true,
    };

    /// A sequence held inside another element, which a heading cannot divide.
    const CONTAINED: Self = Self {
        promote: false,
        sections: false,
    };

    /// A contained sequence standing in for a list item's content.
    const ITEM: Self = Self {
        promote: true,
        sections: false,
    };

    fn promoted(self) -> Self {
        Self {
            promote: true,
            ..self
        }
    }
}

impl Renderer {
    /// Render a block sequence, one block per line, sectionizing on headings where its position
    /// admits sections. A block that produces no output contributes no separating line either.
    fn blocks(&self, blocks: &[Block], indent: usize, nesting: Nesting) -> String {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            let mut out = String::new();
            let mut index = 0;
            while index < blocks.len() {
                let rendered = match blocks.get(index) {
                    Some(Block::Header(level, attr, title)) if nesting.sections => {
                        let end = section_end(blocks, index, *level);
                        let inner = blocks.get(index + 1..end).unwrap_or_default();
                        index = end;
                        self.section(*level, attr, title, inner, indent)
                    }
                    Some(block) => {
                        index += 1;
                        self.block(block, indent, nesting, out.is_empty())
                    }
                    None => break,
                };
                if rendered.is_empty() {
                    continue;
                }
                // A block ending on a break of its own already opened the line the next one takes.
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&rendered);
            }
            out
        })
    }

    /// Render one block. `first` says the block opens its sequence, and so opens the line it starts
    /// on, which decides whether inline content laid out bare can break on its leading spacing.
    fn block(&self, block: &Block, indent: usize, nesting: Nesting, first: bool) -> String {
        match block {
            Block::Plain(inlines) if nesting.promote => self.paragraph("", inlines, indent),
            Block::Plain(inlines) => self.filled(inlines, indent, first),
            Block::Para(inlines) => self.paragraph("", inlines, indent),
            Block::LineBlock(lines) => self.line_block(lines, indent),
            Block::CodeBlock(attr, text) => self.code_block(attr, text, indent),
            Block::RawBlock(format, text) => raw_block(format, text, indent),
            Block::BlockQuote(blocks) => element(
                indent,
                "blockquote",
                "",
                &self.blocks(blocks, indent + STEP, Nesting::CONTAINED),
            ),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, indent),
            Block::BulletList(items) => self.bullet_list(items, indent),
            Block::DefinitionList(entries) => self.definition_list(entries, indent),
            // A heading where no section can open has nothing to divide, and no title of its own.
            Block::Header(_, _, _) | Block::HorizontalRule => String::new(),
            Block::Table(table) => self.table(table, indent),
            Block::Figure(attr, caption, body) => self.figure(attr, caption, body, indent),
            Block::Div(attr, blocks) => self.division(attr, blocks, indent, nesting),
        }
    }

    /// A heading and the blocks beneath it as a `<section>`, or a `<simplesect>` once the heading
    /// sits deeper than sections nest. A heading with no blocks beneath it keeps an empty
    /// paragraph, so the element stays valid.
    fn section(
        &self,
        level: i64,
        attr: &Attr,
        title: &[Inline],
        inner: &[Block],
        indent: usize,
    ) -> String {
        let mut attrs = String::new();
        if self.namespaced && level <= self.top_level {
            attrs.push_str(NAMESPACES);
        }
        push_attribute(&mut attrs, "xml:id", &identifier(&attr.id));
        if attr
            .classes
            .iter()
            .any(|class| class.as_str() == UNNUMBERED)
        {
            push_attribute(&mut attrs, "role", UNNUMBERED);
        }
        let name = self.division_name(level);
        let content_indent = indent + STEP;
        let mut content = self.title(title, content_indent);
        let body = if inner.is_empty() {
            element(content_indent, "para", "", "")
        } else {
            self.blocks(inner, content_indent, Nesting::SECTIONED)
        };
        if !body.is_empty() {
            content.push('\n');
            content.push_str(&body);
        }
        element(indent, name, &attrs, &content)
    }

    /// The element a heading at `level` opens: the named division its depth below the document's
    /// shallowest heading claims, then sections for as deep as they nest, then `<simplesect>`.
    fn division_name(&self, level: i64) -> &'static str {
        let depth = level.saturating_sub(self.top_level).max(0);
        let outer = i64::try_from(self.outer_divisions.len()).unwrap_or(i64::MAX);
        match usize::try_from(depth)
            .ok()
            .and_then(|depth| self.outer_divisions.get(depth))
        {
            Some(name) => name,
            None if depth.saturating_sub(outer) < DEEPEST_SECTION => "section",
            None => "simplesect",
        }
    }

    /// Content opening on the same line as the tags around it, with continuation lines back at the
    /// enclosing indent. The tags join the reflow, so a closing tag that would overrun the fill
    /// column moves down with the word it hangs off.
    fn enclosed(&self, open: &str, inlines: &[Inline], close: &str, indent: usize) -> String {
        self.enclosed_pieces(open, self.pieces(inlines, indent), close, indent)
    }

    fn enclosed_pieces(
        &self,
        open: &str,
        content: Vec<Piece>,
        close: &str,
        indent: usize,
    ) -> String {
        let mut pieces = vec![Piece::text(open)];
        pieces.extend(content);
        pieces.push(Piece::text(close));
        indented(&self.laid_out(&pieces, indent, false), indent)
    }

    /// Inline content reflowed to the fill column, one output line per hard break: a break says a
    /// line ends here, whether or not the line it ends says anything. `opening` says the content
    /// itself opens the line it starts on.
    fn laid_out(&self, pieces: &[Piece], indent: usize, opening: bool) -> String {
        let lines: Vec<String> = pieces
            .split(|piece| matches!(piece, Piece::Hard))
            .enumerate()
            .map(|(position, line)| self.fill_line(line, indent, opening && position == 0))
            .collect();
        lines.join("\n")
    }

    /// One reflowed line. Spacing opening a line is dropped, but where the content opens the line
    /// itself the spacing still offers the reflow a place to break: a first word that cannot follow
    /// it within the fill column moves down, leaving the line it opened empty. Content the line was
    /// already opened for, and every line a break within the content begins, has no such gap to keep.
    fn fill_line(&self, pieces: &[Piece], indent: usize, opening: bool) -> String {
        let spacing = pieces
            .iter()
            .take_while(|piece| matches!(piece, Piece::Space | Piece::Soft))
            .count();
        let rest = pieces.get(spacing..).unwrap_or_default();
        // Only content opening an unindented line picks up where the surrounding layout left off.
        let initial = if opening && indent == 0 {
            self.opening_offset
        } else {
            0
        };
        let body = fill_offset(rest, self.available(indent), initial, self.wrap);
        let broken = spacing > 0
            && opening
            && self.wrap != WrapMode::None
            && leading_word_width(rest).saturating_add(1) > self.width;
        if broken { format!("\n{body}") } else { body }
    }

    fn inline_element(&self, name: &str, inlines: &[Inline], indent: usize) -> String {
        self.enclosed(&format!("<{name}>"), inlines, &format!("</{name}>"), indent)
    }

    fn title(&self, inlines: &[Inline], indent: usize) -> String {
        self.inline_element("title", inlines, indent)
    }

    /// A paragraph. One carrying a hard break becomes a verbatim layout at the left margin, keeping
    /// the lines the breaks divide it into exactly as long as they are.
    fn paragraph(&self, attrs: &str, inlines: &[Inline], indent: usize) -> String {
        if has_line_break(inlines) {
            return format!(
                "{FLUSH}<literallayout{attrs}>{}</literallayout>",
                self.unwrapped(inlines)
            );
        }
        element(
            indent,
            "para",
            attrs,
            &self.filled(inlines, indent + STEP, true),
        )
    }

    /// A line block as a verbatim layout: one output line per given sequence, reflowed where a line
    /// overruns the fill column. A line saying nothing takes no line of its own, and a break already
    /// ending a line is the break that divides it from the next.
    fn line_block(&self, lines: &[Vec<Inline>], indent: usize) -> String {
        let mut layout: Vec<Vec<Piece>> = vec![vec![Piece::text("<literallayout>")]];
        let mut opened = false;
        for line in lines {
            let carried = self.pieces(line, indent);
            if !carries_anything(&carried) {
                continue;
            }
            if opened && layout.last().is_some_and(|current| carries_text(current)) {
                layout.push(Vec::new());
            }
            opened = true;
            for piece in carried {
                match piece {
                    Piece::Hard => layout.push(Vec::new()),
                    carried => {
                        if let Some(current) = layout.last_mut() {
                            current.push(carried);
                        }
                    }
                }
            }
        }
        if let Some(last) = layout.last_mut() {
            last.push(Piece::text("</literallayout>"));
        }
        // Spacing that opens a line here is dropped outright: a verbatim line already stands on its
        // own, so it never moves its first word down to keep the gap.
        let body: Vec<String> = layout
            .iter()
            .map(|line| fill(line, self.available(indent), self.wrap))
            .collect();
        indented(&body.join("\n"), indent)
    }

    /// Inline content reflowed to the fill column and indented to `indent`, `opening` saying the
    /// content opens the line it starts on.
    fn filled(&self, inlines: &[Inline], indent: usize, opening: bool) -> String {
        let body = self.laid_out(&self.pieces(inlines, indent), indent, opening);
        if body.is_empty() {
            return String::new();
        }
        indented(&body, indent)
    }

    /// Inline content on the lines the source gave it: no reflowing, hard breaks kept, every other
    /// break a space. Each line stands at column zero, spacing the document itself asked for
    /// included; only the indentation an element nested here would take is given up.
    fn unwrapped(&self, inlines: &[Inline]) -> String {
        let mut out = String::new();
        for piece in self.pieces(inlines, 0) {
            match piece {
                Piece::Text(text) => out.push_str(&flush_continuations(text.as_str())),
                Piece::Space | Piece::Soft => out.push(' '),
                Piece::Hard => {
                    out.push('\n');
                    out.push(FLUSH);
                }
            }
        }
        out
    }

    /// The columns left for content at `indent`, so an indented line still ends within the fill
    /// column.
    fn available(&self, indent: usize) -> usize {
        self.width.saturating_sub(indent)
    }

    fn code_block(&self, attr: &Attr, text: &str, indent: usize) -> String {
        let mut attrs = String::new();
        if let Some(language) = attr
            .classes
            .iter()
            .find_map(|class| self.catalog.language(class.as_str()))
        {
            push_attribute(&mut attrs, "language", &language);
        }
        let body = escaped(text);
        let close = if body.is_empty() || body.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        format!(
            "{}<programlisting{attrs}>\n{}",
            padding(indent),
            mark_flushed(&format!("{body}{close}</programlisting>"))
        )
    }

    fn bullet_list(&self, items: &[Vec<Block>], indent: usize) -> String {
        let attrs = spacing_attribute(items.iter().map(Vec::as_slice));
        element(
            indent,
            "itemizedlist",
            &attrs,
            &self.list_items(items, indent + STEP),
        )
    }

    fn ordered_list(&self, list: &ListAttributes, items: &[Vec<Block>], indent: usize) -> String {
        let mut attrs = String::new();
        if let Some(numeration) = numeration(list.style) {
            push_attribute(&mut attrs, "numeration", numeration);
        }
        attrs.push_str(&spacing_attribute(items.iter().map(Vec::as_slice)));
        if list.start != 1 {
            push_attribute(&mut attrs, "startingnumber", &list.start.to_string());
        }
        element(
            indent,
            "orderedlist",
            &attrs,
            &self.list_items(items, indent + STEP),
        )
    }

    fn list_items(&self, items: &[Vec<Block>], indent: usize) -> String {
        let rendered: Vec<String> = items
            .iter()
            .map(|item| {
                element(
                    indent,
                    "listitem",
                    "",
                    &self.blocks(item, indent + STEP, Nesting::ITEM),
                )
            })
            .collect();
        rendered.join("\n")
    }

    fn definition_list(&self, entries: &[(Vec<Inline>, Vec<Vec<Block>>)], indent: usize) -> String {
        let attrs = spacing_attribute(
            entries
                .iter()
                .flat_map(|(_, definitions)| definitions.iter().map(Vec::as_slice)),
        );
        let entry_indent = indent + STEP;
        let inner = entry_indent + STEP;
        let rendered: Vec<String> = entries
            .iter()
            .map(|(term, definitions)| {
                let mut content =
                    element(inner, "term", "", &self.filled(term, inner + STEP, true));
                let owned: Vec<Block> = definitions.iter().flatten().cloned().collect();
                content.push('\n');
                content.push_str(&element(
                    inner,
                    "listitem",
                    "",
                    &self.blocks(&owned, inner + STEP, Nesting::ITEM),
                ));
                element(entry_indent, "varlistentry", "", &content)
            })
            .collect();
        element(indent, "variablelist", &attrs, &rendered.join("\n"))
    }

    /// A division: the section its own leading heading opens, an admonition when its leading class
    /// names one, an identified paragraph when it holds exactly one, and otherwise its blocks behind
    /// a standalone anchor carrying the identifier.
    fn division(&self, attr: &Attr, blocks: &[Block], indent: usize, nesting: Nesting) -> String {
        if nesting.sections
            && let Some(Block::Header(level, heading, title)) = blocks.first()
            && (attr.id.is_empty() || heading.id.is_empty() || attr.id == heading.id)
            && section_end(blocks, 0, *level) == blocks.len()
        {
            return self.named_section(attr, *level, heading, title, blocks, indent);
        }
        if let Some(name) = attr
            .classes
            .first()
            .map(Text::as_str)
            .filter(|class| ADMONITIONS.contains(class))
        {
            let mut attrs = String::new();
            push_attribute(&mut attrs, "xml:id", &identifier(&attr.id));
            let inner = indent + STEP;
            let (heading, rest) = match blocks.split_first() {
                Some((Block::Div(first, title), rest)) if is_title_division(first) => {
                    (Some(self.admonition_title(title, inner)), rest)
                }
                _ => (None, blocks),
            };
            let mut content = heading.unwrap_or_default();
            let body = self.blocks(rest, inner, nesting.promoted());
            if !content.is_empty() && !body.is_empty() {
                content.push('\n');
            }
            content.push_str(&body);
            return element(indent, name, &attrs, &content);
        }
        if attr.id.is_empty() {
            return self.blocks(blocks, indent, nesting.promoted());
        }
        if let [Block::Para(inlines)] = blocks {
            let mut attrs = String::new();
            push_attribute(&mut attrs, "xml:id", &identifier(&attr.id));
            return self.paragraph(&attrs, inlines, indent);
        }
        let anchor = format!(
            "{}<anchor xml:id=\"{}\" />",
            padding(indent),
            escaped(&identifier(attr.id.as_str()))
        );
        let body = self.blocks(blocks, indent, nesting.promoted());
        if body.is_empty() {
            anchor
        } else {
            format!("{anchor}\n{body}")
        }
    }

    /// A division whose whole content is the section its own leading heading opens: no element of
    /// its own, only a name for that section, which the heading takes over when it has none.
    fn named_section(
        &self,
        division: &Attr,
        level: i64,
        heading: &Attr,
        title: &[Inline],
        blocks: &[Block],
        indent: usize,
    ) -> String {
        let mut named = heading.clone();
        if named.id.is_empty() {
            named.id.clone_from(&division.id);
        }
        let inner = blocks.get(1..).unwrap_or_default();
        self.section(level, &named, title, inner, indent)
    }

    /// An admonition's heading. A single bare paragraph becomes the title text itself; anything
    /// richer keeps its own block structure inside the title.
    fn admonition_title(&self, blocks: &[Block], indent: usize) -> String {
        match blocks {
            [] => self.enclosed("<title>", &[], "</title>", indent),
            [Block::Para(inlines) | Block::Plain(inlines)] => {
                self.enclosed("<title>", inlines, "</title>", indent)
            }
            _ => {
                let body = self.blocks(blocks, indent, Nesting::ITEM);
                let pad = padding(indent);
                // The title opens the line, so a block that would stand at column zero already does.
                format!(
                    "{pad}<title>{}</title>",
                    body.trim_start_matches([' ', FLUSH])
                )
            }
        }
    }

    /// A table: captioned as `<table>`, otherwise as `<informaltable>`. Only the first head row is a
    /// header, and one whose cells are all empty is left out entirely, since a column heading that
    /// says nothing is no heading. Column and row spans are not carried over; a covered column
    /// contributes an empty entry so the grid stays rectangular.
    fn table(&self, table: &Table, indent: usize) -> String {
        let columns = table.col_specs.len();
        let inner = indent + STEP;
        let group_inner = inner + STEP;
        let mut group = String::new();
        for spec in &table.col_specs {
            group.push_str(&colspec(spec, group_inner));
            group.push('\n');
        }
        let mut head = self.group_rows(&table.head.rows, columns, 0, group_inner + STEP);
        let mut body = if head.is_empty() {
            Vec::new()
        } else {
            head.split_off(1)
        };
        // A leading row that came out saying nothing heads nothing, so it goes without a heading.
        if let Some((true, row)) = head.first() {
            group.push_str(&element(group_inner, "thead", "", row));
            group.push('\n');
        }
        for section in &table.bodies {
            let head_columns = usize::try_from(section.row_head_columns).unwrap_or(0);
            body.extend(self.group_rows(&section.head, columns, 0, group_inner + STEP));
            body.extend(self.group_rows(&section.body, columns, head_columns, group_inner + STEP));
        }
        body.extend(self.group_rows(&table.foot.rows, columns, 0, group_inner + STEP));
        let rows: Vec<String> = body.into_iter().map(|(_, row)| row).collect();
        group.push_str(&element(group_inner, "tbody", "", &rows.join("\n")));
        let tgroup = element(inner, "tgroup", &format!(" cols=\"{columns}\""), &group);
        let caption = caption_inlines(&table.caption.long);
        if caption.is_empty() {
            return element(indent, "informaltable", "", &tgroup);
        }
        let title = self.title(&caption, inner);
        element(indent, "table", "", &format!("{title}\n{tgroup}"))
    }

    /// One group of rows, each laid out across exactly the table's declared columns: a column a
    /// span covers and a column no cell reached both contribute an empty entry. Each row is paired
    /// with whether any of its entries came out holding something.
    fn group_rows(
        &self,
        rows: &[Row],
        columns: usize,
        head_columns: usize,
        indent: usize,
    ) -> Vec<(bool, String)> {
        let mut grid = CellGrid::new(columns, head_columns);
        let inner = indent + STEP;
        let blank = element(inner, "entry", "", "");
        rows.iter()
            .map(|row| {
                let mut carries = false;
                let entries: Vec<String> = grid
                    .place(&row.cells)
                    .into_iter()
                    .map(|slot| match slot {
                        Some(cell) => {
                            carries |= !cell.content.is_empty();
                            element(
                                inner,
                                "entry",
                                "",
                                &self.blocks(&cell.content, inner + STEP, Nesting::CONTAINED),
                            )
                        }
                        None => blank.clone(),
                    })
                    .collect();
                (carries, element(indent, "row", "", &entries.join("\n")))
            })
            .collect()
    }

    /// A figure. A body that is nothing but one image becomes a media object; any other body is
    /// rendered as blocks beneath the caption. A figure showing nothing is left out, caption and
    /// all, since a caption alone captions nothing.
    fn figure(&self, attr: &Attr, caption: &Caption, body: &[Block], indent: usize) -> String {
        if holds_table_or_figure(body) {
            return self.figure_in_place(attr, caption, body, indent);
        }
        let inner = indent + STEP;
        let rendered = match figure_image(body) {
            Some((attr, alt, target)) => self.media_object(attr, alt, target, inner),
            None => self.blocks(body, inner, Nesting::CONTAINED),
        };
        if rendered.is_empty() {
            return String::new();
        }
        let mut content = self.title(&caption_inlines(&caption.long), inner);
        content.push('\n');
        content.push_str(&rendered);
        element(indent, "figure", "", &content)
    }

    /// A figure whose body a `<figure>` cannot hold: the body takes the figure's place, the caption
    /// follows it as blocks of its own, and an identifier becomes an anchor standing before both.
    fn figure_in_place(
        &self,
        attr: &Attr,
        caption: &Caption,
        body: &[Block],
        indent: usize,
    ) -> String {
        let anchor = if attr.id.is_empty() {
            String::new()
        } else {
            format!(
                "{}<anchor xml:id=\"{}\" />",
                padding(indent),
                escaped(attr.id.as_str())
            )
        };
        let parts = [
            anchor,
            self.blocks(body, indent, Nesting::ITEM),
            self.blocks(&caption.long, indent, Nesting::ITEM),
        ];
        parts
            .iter()
            .filter(|part| !part.is_empty())
            .cloned()
            .collect::<Vec<String>>()
            .join("\n")
    }

    /// A figure's picture: the image data, plus the alternative text as a one-line text object.
    fn media_object(&self, attr: &Attr, alt: &[Inline], target: &Target, indent: usize) -> String {
        let inner = indent + STEP;
        let mut content = element(
            inner,
            "imageobject",
            "",
            &image_data(attr, target, inner + STEP),
        );
        if !alt.is_empty() {
            content.push('\n');
            content.push_str(&self.enclosed(
                "<textobject><phrase>",
                alt,
                "</phrase></textobject>",
                inner,
            ));
        }
        element(indent, "mediaobject", "", &content)
    }

    /// A text object holding the alternative text on its own line. A picture's stand-in reads as
    /// bare text, so the markup around it drops away.
    fn text_object(alt: &[Inline], indent: usize) -> String {
        let pad = padding(indent + STEP);
        let phrase = format!("<phrase>{}</phrase>", plain_text(alt));
        element(indent, "textobject", "", &indent_block(&phrase, &pad, &pad))
    }

    /// An image within running text. The destination's title becomes the object's own title, and the
    /// alternative text a text object beside the image data.
    fn inline_media_object(
        &self,
        attr: &Attr,
        alt: &[Inline],
        target: &Target,
        indent: usize,
    ) -> String {
        let inner = indent + STEP;
        let data_indent = inner + STEP;
        let mut object = String::new();
        if !target.title.is_empty() {
            object.push_str(&element(
                data_indent,
                "objectinfo",
                "",
                &self.title(&[Inline::Str(target.title.clone())], data_indent + STEP),
            ));
            object.push('\n');
        }
        object.push_str(&image_data(attr, target, data_indent));
        let mut content = element(inner, "imageobject", "", &object);
        if !alt.is_empty() {
            content.push('\n');
            content.push_str(&Self::text_object(alt, inner));
        }
        element(indent, "inlinemediaobject", "", &content)
    }

    /// Inline content as fillable pieces. `indent` is the column the content is laid out at, which a
    /// footnote's own blocks nest below.
    fn pieces(&self, inlines: &[Inline], indent: usize) -> Vec<Piece> {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            let mut pieces = Vec::new();
            for inline in inlines {
                self.push_inline(&mut pieces, inline, indent);
            }
            pieces
        })
    }

    fn push_inline(&self, pieces: &mut Vec<Piece>, inline: &Inline, indent: usize) {
        match inline {
            // An empty run is no word at all, so the spaces around it stay one run of space.
            Inline::Str(text) if text.as_str().is_empty() => {}
            Inline::Str(text) => pieces.push(Piece::text(escaped(text.as_str()))),
            Inline::Emph(content) => self.wrap_inline(pieces, "emphasis", "", content, indent),
            Inline::Underline(content) => {
                self.wrap_inline(pieces, "emphasis", " role=\"underline\"", content, indent);
            }
            Inline::Strong(content) => {
                self.wrap_inline(pieces, "emphasis", " role=\"strong\"", content, indent);
            }
            Inline::Strikeout(content) => {
                self.wrap_inline(
                    pieces,
                    "emphasis",
                    " role=\"strikethrough\"",
                    content,
                    indent,
                );
            }
            Inline::Superscript(content) => {
                self.wrap_inline(pieces, "superscript", "", content, indent);
            }
            Inline::Subscript(content) => {
                self.wrap_inline(pieces, "subscript", "", content, indent);
            }
            Inline::SmallCaps(content) => {
                self.wrap_inline(pieces, "emphasis", " role=\"smallcaps\"", content, indent);
            }
            Inline::Quoted(_, content) => self.wrap_inline(pieces, "quote", "", content, indent),
            Inline::Cite(_, content) => {
                pieces.extend(self.pieces(content, indent));
            }
            Inline::Code(_, text) => pieces.push(Piece::text(format!(
                "<literal>{}</literal>",
                escaped(text.as_str())
            ))),
            Inline::Space => pieces.push(Piece::Space),
            Inline::SoftBreak => pieces.push(Piece::Soft),
            Inline::LineBreak => pieces.push(Piece::Hard),
            Inline::Math(kind, text) => self.push_math(pieces, kind, text, indent),
            Inline::RawInline(format, text) => {
                if matches!(
                    format.0.as_str().to_lowercase().as_str(),
                    "docbook" | "html"
                ) {
                    pieces.push(Piece::text(text.clone()));
                }
            }
            Inline::Link(attr, content, target) => {
                self.push_link(pieces, attr, content, target, indent);
            }
            Inline::Image(attr, alt, target) => {
                pieces.push(Piece::text(dedent(
                    &self.inline_media_object(attr, alt, target, indent),
                    indent,
                )));
            }
            Inline::Note(blocks) => {
                let body = self.blocks(blocks, indent + STEP, Nesting::CONTAINED);
                let note = opened_element("footnote", "", &body, &padding(indent));
                pieces.push(Piece::text(dedent(&note, indent)));
            }
            Inline::Span(attr, content) => {
                if !attr.id.is_empty() {
                    pieces.push(Piece::text(format!(
                        "<anchor xml:id=\"{}\" />",
                        escaped(&identifier(attr.id.as_str()))
                    )));
                }
                pieces.extend(self.pieces(content, indent));
            }
        }
    }

    /// A link: an address to write to, a cross-reference to somewhere in the document, or a pointer
    /// out of it.
    fn push_link(
        &self,
        pieces: &mut Vec<Piece>,
        attr: &Attr,
        content: &[Inline],
        target: &Target,
        indent: usize,
    ) {
        if let Some(address) = target.url.as_str().strip_prefix("mailto:") {
            let element = format!("<email>{}</email>", escaped(address));
            // The address written out stands for itself; any other wording keeps it beside.
            let speaks_for_itself = matches!(content, [Inline::Str(shown)]
                if percent_encoded(shown.as_str()) == address);
            if speaks_for_itself {
                pieces.push(Piece::text(element));
                return;
            }
            let shown = self.pieces(content, indent);
            // Wording that came out empty is no wording, so nothing stands beside it.
            let beside = carries_text(&shown);
            pieces.extend(shown);
            if beside {
                pieces.push(Piece::Space);
            }
            pieces.push(Piece::text(format!("({element})")));
            return;
        }
        let mut attrs = String::new();
        let anchor = target.url.as_str().strip_prefix('#');
        match anchor {
            Some(target) => push_pointer(&mut attrs, "linkend", &identifier(target)),
            None => push_pointer(&mut attrs, "xlink:href", target.url.as_str()),
        }
        push_attribute(&mut attrs, "id", &identifier(&attr.id));
        push_attribute(&mut attrs, "role", &role(attr));
        // A cross-reference within the document names its own destination when the document gave it
        // no wording of its own.
        let name = if anchor.is_some() && content.is_empty() {
            "xref"
        } else {
            "link"
        };
        self.wrap_inline(pieces, name, &attrs, content, indent);
    }

    fn wrap_inline(
        &self,
        pieces: &mut Vec<Piece>,
        name: &str,
        attrs: &str,
        content: &[Inline],
        indent: usize,
    ) {
        pieces.push(Piece::text(format!("<{name}{attrs}>")));
        pieces.extend(self.pieces(content, indent));
        pieces.push(Piece::text(format!("</{name}>")));
    }

    /// Math lowered to the inline markup `DocBook` offers. An expression the lowering cannot express
    /// keeps its source between the delimiters that identify it as math.
    fn push_math(&self, pieces: &mut Vec<Piece>, kind: &MathType, text: &Text, indent: usize) {
        if let Some(inlines) = crate::math::to_inlines(text.as_str()) {
            pieces.extend(self.pieces(&inlines, indent));
            return;
        }
        if text.as_str().trim().is_empty() {
            return;
        }
        let delimiter = if matches!(kind, MathType::DisplayMath) {
            "$$"
        } else {
            "$"
        };
        pieces.push(Piece::text(format!(
            "{delimiter}{}{delimiter}",
            escaped(text.as_str())
        )));
    }
}

/// Resolves where each cell of a row starts, over one group of rows a span can reach across. The
/// columns a group keeps as row heads stand apart from the rest of the row: a column span reaches
/// only to the end of the run it starts in, so a head column and a body column never share a cell.
struct CellGrid {
    columns: usize,
    /// The leading columns holding row headings, never more than the table has.
    head_columns: usize,
    /// Per column, how many upcoming rows a span opened in an earlier row still covers.
    pending: Vec<i64>,
}

impl CellGrid {
    fn new(columns: usize, head_columns: usize) -> Self {
        Self {
            columns,
            head_columns: head_columns.min(columns),
            pending: vec![0; columns],
        }
    }

    /// One row laid out over the group's columns: each column carries the cell starting there, or
    /// nothing where a span covers it or no cell reached it. Cells past the last column are dropped.
    fn place<'cells>(&mut self, cells: &'cells [Cell]) -> Vec<Option<&'cells Cell>> {
        let held: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, rows)| **rows > 0)
            .map(|(column, _)| column)
            .collect();
        let mut slots: Vec<Option<&Cell>> = vec![None; self.columns];
        let mut column = 0_usize;
        for cell in cells {
            while self.pending.get(column).copied().unwrap_or(0) > 0 {
                column = column.saturating_add(1);
            }
            let Some(slot) = slots.get_mut(column) else {
                break;
            };
            *slot = Some(cell);
            let edge = if column < self.head_columns {
                self.head_columns
            } else {
                self.columns
            };
            let span = usize::try_from(cell.col_span)
                .unwrap_or(1)
                .clamp(1, edge.saturating_sub(column).max(1));
            let end = column.saturating_add(span).min(self.columns);
            for covered in self.pending.iter_mut().take(end).skip(column) {
                *covered = cell.row_span.saturating_sub(1).max(0);
            }
            column = end;
        }
        for column in held {
            if let Some(rows) = self.pending.get_mut(column) {
                *rows -= 1;
            }
        }
        slots
    }
}

/// The index one past the last block belonging to the section a heading at `start` opens: the next
/// heading of the same or a shallower level, or the end of the sequence. A division that leads with
/// such a heading closes the section too, since the heading it carries opens the next one.
fn section_end(blocks: &[Block], start: usize, level: i64) -> usize {
    let mut end = start + 1;
    while let Some(block) = blocks.get(end) {
        let next = match block {
            Block::Header(next, _, _) => Some(*next),
            Block::Div(_, inner) => leading_heading(inner),
            _ => None,
        };
        if next.is_some_and(|next| next <= level) {
            break;
        }
        end += 1;
    }
    end
}

/// The level of the heading a block sequence opens with, looking through the divisions it opens
/// with in turn, or `None` when it opens with anything else.
fn leading_heading(blocks: &[Block]) -> Option<i64> {
    let mut current = blocks;
    loop {
        match current.first() {
            Some(Block::Header(level, _, _)) => return Some(*level),
            Some(Block::Div(_, inner)) => current = inner,
            _ => return None,
        }
    }
}

/// The shallowest heading level anywhere in the document, or 1 when it carries no heading. Every
/// place a block can sit counts, a footnote and a table cell included, whether or not a heading
/// there would open a section of its own.
fn top_heading_level(blocks: &[Block]) -> i64 {
    let mut shallowest: Option<i64> = None;
    visit_blocks(blocks, &mut |block| {
        if let Block::Header(level, _, _) = block {
            shallowest = Some(shallowest.map_or(*level, |current| current.min(*level)));
        }
    });
    shallowest.unwrap_or(1)
}

/// Whether a table or a figure sits anywhere within a block sequence: content a `<figure>` has no
/// room for, however deeply it is buried.
fn holds_table_or_figure(blocks: &[Block]) -> bool {
    let mut found = false;
    visit_blocks(blocks, &mut |block| {
        found |= matches!(block, Block::Table(_) | Block::Figure(_, _, _));
    });
    found
}

/// Visit every block the sequence holds, however deeply: a caption's blocks, a table cell's, and a
/// footnote's all count.
fn visit_blocks(blocks: &[Block], visit: &mut impl FnMut(&Block)) {
    stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
        for block in blocks {
            visit(block);
            match block {
                Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
                    visit_inlines(inlines, visit);
                }
                Block::LineBlock(lines) => {
                    for line in lines {
                        visit_inlines(line, visit);
                    }
                }
                Block::Div(_, inner) | Block::BlockQuote(inner) => visit_blocks(inner, visit),
                Block::Figure(_, caption, inner) => {
                    visit_blocks(&caption.long, visit);
                    visit_blocks(inner, visit);
                }
                Block::BulletList(items) | Block::OrderedList(_, items) => {
                    for item in items {
                        visit_blocks(item, visit);
                    }
                }
                Block::DefinitionList(entries) => {
                    for (term, definitions) in entries {
                        visit_inlines(term, visit);
                        for definition in definitions {
                            visit_blocks(definition, visit);
                        }
                    }
                }
                Block::Table(table) => visit_table(table, visit),
                Block::CodeBlock(_, _) | Block::RawBlock(_, _) | Block::HorizontalRule => {}
            }
        }
    });
}

fn visit_table(table: &Table, visit: &mut impl FnMut(&Block)) {
    visit_blocks(&table.caption.long, visit);
    let bodies = table
        .bodies
        .iter()
        .flat_map(|body| body.head.iter().chain(body.body.iter()));
    for row in table
        .head
        .rows
        .iter()
        .chain(bodies)
        .chain(table.foot.rows.iter())
    {
        for cell in &row.cells {
            visit_blocks(&cell.content, visit);
        }
    }
}

fn visit_inlines(inlines: &[Inline], visit: &mut impl FnMut(&Block)) {
    stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
        for inline in inlines {
            match inline {
                Inline::Note(blocks) => visit_blocks(blocks, visit),
                Inline::Emph(content)
                | Inline::Underline(content)
                | Inline::Strong(content)
                | Inline::Strikeout(content)
                | Inline::Superscript(content)
                | Inline::Subscript(content)
                | Inline::SmallCaps(content)
                | Inline::Quoted(_, content)
                | Inline::Cite(_, content)
                | Inline::Link(_, content, _)
                | Inline::Image(_, content, _)
                | Inline::Span(_, content) => visit_inlines(content, visit),
                _ => {}
            }
        }
    });
}

/// An element whose content occupies its own indented lines. Empty content leaves the tags adjacent
/// on consecutive lines rather than wrapping a blank one.
fn element(indent: usize, name: &str, attrs: &str, content: &str) -> String {
    let pad = padding(indent);
    format!("{pad}{}", opened_element(name, attrs, content, &pad))
}

/// An element opening where the line already stands, its content on the lines below it and its
/// closing tag back at `pad`.
fn opened_element(name: &str, attrs: &str, content: &str, pad: &str) -> String {
    let content = closed_up(content);
    if content.is_empty() {
        return format!("<{name}{attrs}>\n{pad}</{name}>");
    }
    let opener = if content.ends_with('\n') { "" } else { "\n" };
    format!("<{name}{attrs}>\n{content}{opener}{pad}</{name}>")
}

/// Content trimmed where it meets what closes it: the line the content's last break opened is the
/// line the close goes on.
fn closed_up(content: &str) -> &str {
    content.strip_suffix('\n').unwrap_or(content)
}

/// Move an element carried inside a verbatim layout to column zero: every line but the first gives
/// up the indentation its nesting gave it, while a line already marked keeps the spaces it holds,
/// being text whose shape is the point.
fn flush_continuations(text: &str) -> String {
    let mut out = String::new();
    for (position, line) in text.split('\n').enumerate() {
        if position == 0 {
            out.push_str(line);
            continue;
        }
        out.push('\n');
        let stripped = line.trim_start_matches(' ');
        if !stripped.starts_with(FLUSH) {
            out.push(FLUSH);
        }
        out.push_str(stripped);
    }
    out
}

/// Mark every line of a verbatim construct for column zero.
fn mark_flushed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (position, line) in text.split('\n').enumerate() {
        if position > 0 {
            out.push('\n');
        }
        out.push(FLUSH);
        out.push_str(line);
    }
    out
}

/// Return every marked line to column zero, discarding the indentation the surrounding layout gave
/// it. A mark anywhere but under a line's own indentation is left alone, since it did not come from
/// here.
fn unmark_flushed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (position, line) in text.split('\n').enumerate() {
        if position > 0 {
            out.push('\n');
        }
        match line.split_once(FLUSH) {
            Some((leading, rest)) if leading.bytes().all(|byte| byte == b' ') => {
                out.push_str(rest);
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// Laid-out content moved to its own indent. A line the reflow left empty stays empty: indentation
/// is written only where a line carries something.
fn indented(body: &str, indent: usize) -> String {
    let pad = padding(indent);
    let first = if body.starts_with('\n') {
        ""
    } else {
        pad.as_str()
    };
    indent_block(body, first, &pad)
}

/// The width of the first word a piece sequence opens with, up to the first break within it.
fn leading_word_width(pieces: &[Piece]) -> usize {
    let word: String = pieces
        .iter()
        .map_while(|piece| match piece {
            Piece::Text(text) => Some(text.as_str()),
            Piece::Space | Piece::Soft | Piece::Hard => None,
        })
        .collect();
    display_width(word.split('\n').next().unwrap_or_default())
}

/// Whether a laid-out line puts anything at all on the page: text, spacing, or a break. A line
/// carrying nothing is passed over entirely, taking no line and no separator.
fn carries_anything(line: &[Piece]) -> bool {
    line.iter()
        .any(|piece| !matches!(piece, Piece::Text(text) if text.as_str().is_empty()))
}

/// Whether a laid-out line has any text of its own, as against nothing but spacing.
fn carries_text(line: &[Piece]) -> bool {
    line.iter()
        .any(|piece| matches!(piece, Piece::Text(text) if !text.as_str().is_empty()))
}

fn padding(indent: usize) -> String {
    " ".repeat(indent)
}

/// Append ` name="value"` unless the value is empty.
fn push_attribute(attrs: &mut String, name: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    attrs.push(' ');
    attrs.push_str(name);
    attrs.push_str("=\"");
    attrs.push_str(&escaped(value));
    attrs.push('"');
}

/// Whether a division is an admonition's heading rather than part of its body: it says it is a
/// title and nothing else about itself.
fn is_title_division(attr: &Attr) -> bool {
    matches!(attr.classes.as_slice(), [class] if class.as_str() == "title")
}

/// Append ` name="value"` even when the value is empty: an attribute that points somewhere is what
/// gives its element meaning, so it is written whether or not the document supplied a destination.
fn push_pointer(attrs: &mut String, name: &str, value: &str) {
    attrs.push(' ');
    attrs.push_str(name);
    attrs.push_str("=\"");
    attrs.push_str(&escaped(value));
    attrs.push('"');
}

/// Text as XML content: the markup delimiters replaced by references, and any character XML cannot
/// carry dropped so the output stays parseable.
fn escaped(value: &str) -> String {
    let carried: String = value
        .chars()
        .filter(|character| representable(*character))
        .collect();
    escape_xml(&carried, true)
}

/// Whether XML can carry a character at all. Tab, newline, and carriage return are the only control
/// characters it admits, and the two non-characters closing the basic plane are excluded as well.
fn representable(character: char) -> bool {
    matches!(character,
        '\t' | '\n' | '\r'
        | ' '..='\u{d7ff}'
        | '\u{e000}'..='\u{fffd}'
        | '\u{10000}'..='\u{10ffff}')
}

/// The role an element takes from the classes its attributes carry, all of them in order.
fn role(attr: &Attr) -> String {
    attr.classes
        .iter()
        .map(Text::as_str)
        .collect::<Vec<&str>>()
        .join(" ")
}

/// Text as a URI: every ASCII character a URI cannot spell out becomes a percent-encoded byte.
/// Characters beyond ASCII are left as they stand.
fn percent_encoded(text: &str) -> String {
    const SPELLED_OUT: &str = "-_.~!*'();:@&=+$,/?#[]%";
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if !character.is_ascii()
            || character.is_ascii_alphanumeric()
            || SPELLED_OUT.contains(character)
        {
            out.push(character);
            continue;
        }
        let mut encoded = [0_u8; 4];
        for byte in character.encode_utf8(&mut encoded).as_bytes() {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// An identifier as an XML name: one that does not open with a letter takes a prefix, since XML
/// admits no other opening character.
fn identifier(id: &str) -> String {
    match id.chars().next() {
        None => String::new(),
        Some(first) if first.is_alphabetic() => id.to_owned(),
        Some(_) => format!("id_{id}"),
    }
}

/// Remove up to `indent` leading spaces from every line, so already-indented block content can be
/// carried inside a filled line and re-indented as one unit.
fn dedent(text: &str, indent: usize) -> String {
    let mut out = String::with_capacity(text.len());
    for (position, line) in text.split('\n').enumerate() {
        if position > 0 {
            out.push('\n');
        }
        let keep = line.len() - line.trim_start_matches(' ').len();
        out.push_str(line.get(keep.min(indent)..).unwrap_or(line));
    }
    out
}

/// Whether a hard break occurs anywhere in the inline tree, which switches a paragraph to a verbatim
/// layout.
fn has_line_break(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::LineBreak => true,
        Inline::Emph(content)
        | Inline::Underline(content)
        | Inline::Strong(content)
        | Inline::Strikeout(content)
        | Inline::Superscript(content)
        | Inline::Subscript(content)
        | Inline::SmallCaps(content)
        | Inline::Quoted(_, content)
        | Inline::Cite(_, content)
        | Inline::Link(_, content, _)
        | Inline::Image(_, content, _)
        | Inline::Span(_, content) => has_line_break(content),
        _ => false,
    })
}

/// Inline content as bare text, for a slot that admits no markup: styling, links and citations
/// contribute their content alone, code and math their literal payload, any break a space, and a
/// note or a raw fragment nothing at all.
fn plain_text(inlines: &[Inline]) -> String {
    stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
        let mut out = String::new();
        for inline in inlines {
            match inline {
                Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => {
                    out.push_str(text.as_str());
                }
                Inline::Emph(content)
                | Inline::Underline(content)
                | Inline::Strong(content)
                | Inline::Strikeout(content)
                | Inline::Superscript(content)
                | Inline::Subscript(content)
                | Inline::SmallCaps(content)
                | Inline::Cite(_, content)
                | Inline::Link(_, content, _)
                | Inline::Image(_, content, _)
                | Inline::Span(_, content) => out.push_str(&plain_text(content)),
                Inline::Quoted(kind, content) => {
                    let (open, close) = quote_marks(kind);
                    out.push(open);
                    out.push_str(&plain_text(content));
                    out.push(close);
                }
                Inline::Space | Inline::SoftBreak | Inline::LineBreak => out.push(' '),
                Inline::RawInline(_, _) | Inline::Note(_) => {}
            }
        }
        out
    })
}

/// A raw block reaches the output only when it is already written in this format, indented into the
/// surrounding layout without otherwise touching its own line structure.
fn raw_block(format: &Format, text: &str, indent: usize) -> String {
    let body = text.trim_end_matches('\n');
    if body.is_empty() || format.0.as_str().to_lowercase() != "docbook" {
        return String::new();
    }
    let pad = padding(indent);
    indent_block(body, &pad, &pad)
}

/// ` spacing="compact"` for a list every item of which is laid out tightly.
fn spacing_attribute<'items>(mut items: impl Iterator<Item = &'items [Block]>) -> String {
    if items.all(item_is_tight) {
        " spacing=\"compact\"".to_owned()
    } else {
        String::new()
    }
}

/// Whether a list item is laid out tightly: it is empty, it opens with bare inline content, or it
/// holds nothing but a nested list that is itself tight.
fn item_is_tight(item: &[Block]) -> bool {
    stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || match item {
        [] | [Block::Plain(_), ..] => true,
        [Block::BulletList(nested) | Block::OrderedList(_, nested)] => {
            nested.iter().all(|nested_item| item_is_tight(nested_item))
        }
        _ => false,
    })
}

/// The numeral style of an ordered list, or `None` when the list takes the renderer's own.
fn numeration(style: ListNumberStyle) -> Option<&'static str> {
    match style {
        ListNumberStyle::DefaultStyle => None,
        ListNumberStyle::Decimal | ListNumberStyle::Example => Some("arabic"),
        ListNumberStyle::LowerAlpha => Some("loweralpha"),
        ListNumberStyle::UpperAlpha => Some("upperalpha"),
        ListNumberStyle::LowerRoman => Some("lowerroman"),
        ListNumberStyle::UpperRoman => Some("upperroman"),
    }
}

/// One column's specification: its share of the table width as a relative measure, and its
/// alignment.
fn colspec(spec: &ColSpec, indent: usize) -> String {
    let mut attrs = String::new();
    if let ColWidth::ColWidth(fraction) = spec.width {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let percent = (fraction * 100.0).trunc().max(0.0) as u64;
        push_attribute(&mut attrs, "colwidth", &format!("{percent}*"));
    }
    push_attribute(&mut attrs, "align", alignment(&spec.align));
    format!("{}<colspec{attrs} />", padding(indent))
}

fn alignment(align: &Alignment) -> &'static str {
    match align {
        Alignment::AlignRight => "right",
        Alignment::AlignCenter => "center",
        Alignment::AlignLeft | Alignment::AlignDefault => "left",
    }
}

/// A block-level caption flattened to inline content: consecutive blocks are divided by a hard
/// break, whether or not either of them says anything.
fn caption_inlines(blocks: &[Block]) -> Vec<Inline> {
    stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
        let mut out = Vec::new();
        for (position, block) in blocks.iter().enumerate() {
            if position > 0 {
                out.push(Inline::LineBreak);
            }
            out.extend(caption_block(block));
        }
        out
    })
}

/// One caption block as inline content. A grouping block contributes its own blocks flattened the
/// same way; the parts of a list run together, since the layout that told them apart is gone.
fn caption_block(block: &Block) -> Vec<Inline> {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
            inlines.clone()
        }
        Block::LineBlock(lines) => {
            let mut joined = Vec::new();
            for (position, line) in lines.iter().enumerate() {
                if position > 0 {
                    joined.push(Inline::LineBreak);
                }
                joined.extend(line.iter().cloned());
            }
            joined
        }
        Block::CodeBlock(attr, text) => vec![Inline::Code(attr.clone(), text.clone())],
        Block::RawBlock(format, text) => vec![Inline::RawInline(format.clone(), text.clone())],
        Block::BlockQuote(inner) | Block::Div(_, inner) | Block::Figure(_, _, inner) => {
            caption_inlines(inner)
        }
        Block::BulletList(items) | Block::OrderedList(_, items) => items
            .iter()
            .flat_map(|item| caption_inlines(item))
            .collect(),
        Block::DefinitionList(entries) => entries
            .iter()
            .flat_map(|(term, definitions)| {
                let mut entry = term.clone();
                entry.push(Inline::Str(Text::from(":")));
                entry.push(Inline::Space);
                for definition in definitions {
                    entry.extend(caption_inlines(definition));
                }
                entry
            })
            .collect(),
        Block::Table(table) => caption_table(table),
        Block::HorizontalRule => Vec::new(),
    }
}

/// A table flattened to inline content: one row after another, divided by a hard break, each row
/// the content of its cells run together. The table's own caption says nothing here, being a
/// caption to a caption.
fn caption_table(table: &Table) -> Vec<Inline> {
    let rows = table
        .head
        .rows
        .iter()
        .chain(
            table
                .bodies
                .iter()
                .flat_map(|section| section.head.iter().chain(section.body.iter())),
        )
        .chain(table.foot.rows.iter());
    let mut out = Vec::new();
    for (position, row) in rows.enumerate() {
        if position > 0 {
            out.push(Inline::LineBreak);
        }
        for cell in &row.cells {
            out.extend(caption_inlines(&cell.content));
        }
    }
    out
}

/// The lone image a figure's body consists of, if that is all it holds.
fn figure_image(body: &[Block]) -> Option<(&Attr, &[Inline], &Target)> {
    let [Block::Plain(inlines)] = body else {
        return None;
    };
    let [Inline::Image(attr, alt, target)] = inlines.as_slice() else {
        return None;
    };
    Some((attr, alt, target))
}

/// An `<imagedata>` element: where the picture lives, and how large it is drawn.
fn image_data(attr: &Attr, target: &Target, indent: usize) -> String {
    let mut attrs = String::new();
    push_pointer(&mut attrs, "fileref", target.url.as_str());
    push_attribute(&mut attrs, "id", &identifier(&attr.id));
    push_attribute(&mut attrs, "role", &role(attr));
    if let Some(width) = dimension(attr, "width") {
        push_attribute(&mut attrs, "width", &width);
    }
    if let Some(depth) = dimension(attr, "height") {
        push_attribute(&mut attrs, "depth", &depth);
    }
    format!("{}<imagedata{attrs} />", padding(indent))
}

/// An image dimension in the spelling `DocBook` takes, or `None` when the attribute is absent or not a
/// dimension.
fn dimension(attr: &Attr, key: &str) -> Option<String> {
    match parse_dimension(attribute_value(attr, key)?)? {
        Dimension::Pixels(pixels) => Some(format!("{pixels}px")),
        Dimension::Percent(magnitude) => Some(format_percent_dimension(magnitude)),
        Dimension::Length(magnitude, unit) => Some(format_length_dimension(magnitude, unit)),
    }
}
