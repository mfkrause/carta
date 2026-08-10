//! `DocBook` reader: parses a `DocBook` XML document into the document model.
//!
//! The vocabulary is large, so the converter is organized as three dispatch tables over an
//! element's local name: one for the block-level elements, one for the inline-level elements, and
//! one for the sectioning elements that establish heading levels. An element outside all three is
//! transparent: its children are converted in place, so an unrecognized wrapper degrades to its
//! content rather than disappearing.
//!
//! Heading levels come from two independent counters. Recursive divisions (`section`, `sect1` and
//! friends, `refsection`, `qandadiv`, `bibliodiv`) deepen with each nesting level, while the
//! book-level divisions (`part`, `chapter`) count only enclosing `part` elements and the
//! back-matter divisions (`appendix`, `preface`, `glossary`, `bibliography`) always sit at the top
//! level.
//!
//! XML is read by a hand-written scanner over the subset the format uses, with the full named
//! character-reference table (the vocabulary's prose relies on `&copy;`, `&mdash;`, and the rest).
//! It is panic-free on malformed input: an unterminated construct ends the scan, a stray close tag
//! is ignored, and an unresolvable reference is kept verbatim. Materialized nesting is capped, so
//! adversarially deep markup neither exhausts the stack nor deepens the conversion recursion.

use std::borrow::Cow;
use std::collections::BTreeMap;

use carta_ast::{
    Alignment, ApiVersion, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline,
    ListAttributes, ListNumberDelim, ListNumberStyle, MathType, MetaValue, QuoteType, Row, Table,
    TableBody, TableFoot, TableHead, Target, Text,
};
use carta_core::{Extension, Reader, ReaderOptions, Result};

use crate::entities::{code_point, lookup_named};
use crate::mathml::{MathTree, to_tex};
use crate::tabs::expand_tabs;

/// Parses a `DocBook` document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct DocbookReader;

impl Reader for DocbookReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        Ok(convert(input, options))
    }
}

/// Deepest element nesting the scanner materializes. Content below the ceiling is kept but hangs
/// off the deepest open element, which bounds the conversion recursion along with the tree.
const MAX_DEPTH: usize = 512;

/// Element names whose content is set verbatim in a monospaced face.
const CODE_ELEMENTS: &[&str] = &[
    "classname",
    "code",
    "command",
    "computeroutput",
    "constant",
    "envar",
    "filename",
    "function",
    "literal",
    "markup",
    "option",
    "parameter",
    "prompt",
    "symbol",
    "systemitem",
    "type",
    "userinput",
    "varname",
];

/// Elements that carry a title but contribute no heading and no content of their own.
const IGNORED_ELEMENTS: &[&str] = &["colspec", "spanspec", "subtitle", "title", "titleabbrev"];

/// Wrappers gathering a division's bibliographic material, which stands apart from its content. Any
/// other name ending in `info` names an ordinary element whose content belongs to the document.
const INFO_WRAPPERS: &[&str] = &[
    "appendixinfo",
    "articleinfo",
    "bookinfo",
    "chapterinfo",
    "glossaryinfo",
    "info",
    "partinfo",
    "refsect1info",
    "refsect2info",
    "refsect3info",
    "refsectioninfo",
    "sect1info",
    "sect2info",
    "sect3info",
    "sect4info",
    "sect5info",
    "sectioninfo",
];

/// Info wrappers whose fields furnish the document's metadata.
const METADATA_WRAPPERS: &[&str] = &["articleinfo", "bookinfo", "info"];

/// Targets a cross reference names by their own title. Every other target is named by a placeholder
/// token instead, whether or not it carries a title of its own.
const TITLED_TARGETS: &[&str] = &[
    "book", "chapter", "figure", "part", "sect1", "sect2", "sect3", "sect4", "sect5", "section",
    "table",
];

/// Admonitions, which always announce a title division even when the source states no title.
const ADMONITIONS: &[&str] = &["caution", "important", "note", "tip", "warning"];

/// Root elements whose bibliographic children furnish the document's metadata.
const METADATA_ROOTS: &[&str] = &["article", "book"];

/// Containers rendered as a classed division, with any title held in a division of its own.
const TITLED_DIVISIONS: &[&str] = &[
    "caution",
    "example",
    "formalpara",
    "important",
    "note",
    "sidebar",
    "tip",
    "warning",
];

fn convert(input: &str, options: &ReaderOptions) -> Document {
    let expanded = expand_source_tabs(input, options.tab_stop);
    let entities = declared_entities(&expanded);
    let mut budget = MAX_ENTITY_GROWTH;
    let document = scan(&substitute_entities(&expanded, &entities, &mut budget));
    let mut ids = BTreeMap::new();
    for root in document.elements() {
        index_ids(root, &mut ids);
    }
    let converter = Converter {
        ids,
        parted: divides_into_parts(&document),
    };
    let mut blocks = converter.blocks(&document.children, Context::default());
    let mut meta = converter.meta(&document);
    if options.extensions.contains(Extension::EastAsianLineBreaks) {
        drop_wide_breaks(&mut blocks);
        for value in meta.values_mut() {
            drop_wide_breaks_in_meta(value);
        }
    }
    Document {
        api_version: ApiVersion::default(),
        meta,
        blocks,
    }
}

/// Expands the input's tabs on the source column grid, the column restarting at each line break.
/// Verbatim content keeps whatever alignment the source lays out, so the grid has to be applied to
/// the document text before the markup around it is scanned.
fn expand_source_tabs(input: &str, tab_stop: usize) -> Cow<'_, str> {
    if tab_stop == 0 || !input.contains('\t') {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    for (index, line) in input.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&expand_tabs(line, tab_stop));
    }
    Cow::Owned(out)
}

/// The heading counters in force at a point in the tree.
#[derive(Debug, Clone, Copy, Default)]
struct Context {
    /// Depth of the recursive divisions enclosing this point; a `section` here is one deeper.
    level: i32,
    /// Where the innermost enclosing book division sits; a numbered `sect1` here is one deeper.
    component: i32,
}

impl Context {
    /// Whether the point stands at the head of the document, with no division opened around it.
    fn at_head(self) -> bool {
        self.level == 0 && self.component == 0
    }

    /// How many recursive section levels enclose the point, a part counting as one of them.
    fn section_depth(self, parted: bool) -> i32 {
        self.level
            .saturating_sub(self.component)
            .saturating_add(i32::from(parted))
    }
}

/// How an inline run treats the character data and the quotations it holds.
#[derive(Debug, Clone, Copy, Default)]
struct Style {
    /// How many quotations enclose this run, which alternates the marks a nested one is set in.
    quotes: u32,
    /// Character data stands exactly as written: a space holds its width and a line division opens
    /// a new line, rather than both folding into a word separator.
    verbatim: bool,
}

impl Style {
    /// The style inside a quotation, whose own quotations are set in the alternate marks.
    fn inside_quote(self) -> Self {
        Self {
            quotes: self.quotes.saturating_add(1),
            ..self
        }
    }

    /// The style of preformatted content, which keeps the spacing and line divisions it is given.
    fn verbatim() -> Self {
        Self {
            verbatim: true,
            ..Self::default()
        }
    }
}

/// How an inline run between block-level children is closed off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    /// Content standing alone, as in a list item or a table cell.
    Plain,
    /// Content of a paragraph split apart by block-level children.
    Para,
}

struct Converter<'a> {
    ids: BTreeMap<&'a str, &'a Element>,
    /// Whether the document divides into parts, which sets where book-level divisions sit.
    parted: bool,
}

// Document metadata

impl Converter<'_> {
    /// Metadata drawn from the whole-document elements the input states: their bibliographic
    /// children, whether written directly or gathered into an `info` wrapper. A division standing
    /// on its own (a chapter, a section, a set of books) speaks only for a part of a work, so it
    /// contributes nothing and its title stays a heading; a later whole document, including one
    /// nested inside another, overrides an earlier one field by field.
    fn meta(&self, document: &Element) -> BTreeMap<Text, MetaValue> {
        let mut meta = BTreeMap::new();
        self.gather_meta(document, &mut meta);
        meta
    }

    /// Adds the fields of every whole-document element under `element`, each one standing over the
    /// ones stated before it.
    fn gather_meta(&self, element: &Element, meta: &mut BTreeMap<Text, MetaValue>) {
        for child in element.elements() {
            if METADATA_ROOTS.contains(&local_name(&child.name)) {
                for field in child.elements() {
                    let name = local_name(&field.name);
                    if METADATA_WRAPPERS.contains(&name) {
                        for wrapped in field.elements() {
                            self.meta_field(wrapped, meta);
                        }
                    } else {
                        self.meta_field(field, meta);
                    }
                }
            }
            self.gather_meta(child, meta);
        }
    }

    fn meta_field(&self, field: &Element, meta: &mut BTreeMap<Text, MetaValue>) {
        let name = local_name(&field.name);
        let value = match name {
            "title" | "subtitle" | "date" | "releaseinfo" | "copyright" | "address" => {
                MetaValue::MetaInlines(self.inlines_trimmed(&field.children, Style::default()))
            }
            "author" => MetaValue::MetaInlines(person_name(field)),
            "authorgroup" => MetaValue::MetaList(
                field
                    .elements()
                    .filter(|person| local_name(&person.name) == "author")
                    .map(|person| MetaValue::MetaInlines(person_name(person)))
                    .collect(),
            ),
            "abstract" => {
                let blocks = self.blocks(&field.children, Context::default());
                match blocks.as_slice() {
                    [Block::Plain(inlines)] => MetaValue::MetaInlines(inlines.clone()),
                    _ => MetaValue::MetaBlocks(blocks),
                }
            }
            _ => return,
        };
        let key = if name == "authorgroup" {
            "author"
        } else {
            name
        };
        meta.insert(key.into(), value);
    }
}

/// A person's name, built from the name parts written as element children, one space between
/// parts. A wrapping `personname` counts as a single part, so its own parts run together.
fn person_name(person: &Element) -> Vec<Inline> {
    let mut text = String::new();
    for part in person.elements() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(part.text().trim());
    }
    let mut inlines = Vec::new();
    push_text(&text, &mut inlines);
    trim(normalize(inlines))
}

// Block-level conversion

impl<'a> Converter<'a> {
    /// Converts a run of nodes into blocks. Every element child stands as a block of its own, so
    /// only the stretches of bare text between them accumulate into inline runs.
    fn blocks(&self, nodes: &'a [Node], context: Context) -> Vec<Block> {
        let mut out = Vec::new();
        let mut run = Vec::new();
        for node in nodes {
            match node {
                Node::Text(text) => push_text(text, &mut run),
                Node::Element(element) => {
                    flush(&mut run, Flow::Plain, &mut out);
                    self.block(element, context, &mut out);
                }
            }
        }
        flush(&mut run, Flow::Plain, &mut out);
        out
    }

    /// Converts the mixed content a paragraph or a table cell holds: inline markup accumulates
    /// into a run that a block-level child closes off, splitting the content around it.
    fn mixed(&self, nodes: &'a [Node], context: Context, flow: Flow) -> Vec<Block> {
        let mut out = Vec::new();
        let mut run = Vec::new();
        for node in nodes {
            match node {
                Node::Text(text) => push_text(text, &mut run),
                Node::Element(element) => {
                    if is_named_block(local_name(&element.name)) {
                        flush(&mut run, flow, &mut out);
                        self.block(element, context, &mut out);
                    } else {
                        self.inline(element, Style::default(), &mut run);
                    }
                }
            }
        }
        flush(&mut run, flow, &mut out);
        out
    }
}

/// Whether a name is one the block dispatch recognizes. An equation or a sidebar stands in the run
/// of text around it rather than breaking the paragraph it is written in.
fn is_named_block(name: &str) -> bool {
    if matches!(name, "equation" | "informalequation" | "sidebar") {
        return false;
    }
    section_level(name, Context::default(), false).is_some()
        || IGNORED_ELEMENTS.contains(&name)
        || TITLED_DIVISIONS.contains(&name)
        || INFO_WRAPPERS.contains(&name)
        || matches!(
            name,
            "abstract"
                | "answer"
                | "biblioentry"
                | "bibliomixed"
                | "blockquote"
                | "bridgehead"
                | "calloutlist"
                | "epigraph"
                | "equation"
                | "figure"
                | "glossdiv"
                | "glosslist"
                | "glosssee"
                | "glossseealso"
                | "index"
                | "informalequation"
                | "informalexample"
                | "informalfigure"
                | "informaltable"
                | "itemizedlist"
                | "literallayout"
                | "mediaobject"
                | "orderedlist"
                | "para"
                | "procedure"
                | "programlisting"
                | "question"
                | "screen"
                | "simpara"
                | "stepalternatives"
                | "substeps"
                | "table"
                | "variablelist"
        )
}

impl<'a> Converter<'a> {
    /// Converts one block-level element, appending its blocks to `out`.
    fn block(&self, element: &'a Element, context: Context, out: &mut Vec<Block>) {
        let name = local_name(&element.name);
        let attr = attr_of(element);
        if IGNORED_ELEMENTS.contains(&name)
            || name == "index"
            || name == "anchor"
            || INFO_WRAPPERS.contains(&name)
        {
            return;
        }
        if let Some((heading, inner)) = section_level(name, context, self.parted) {
            self.section(element, heading, inner, out);
            return;
        }
        if TITLED_DIVISIONS.contains(&name) {
            out.push(self.titled_division(element, name, attr, context));
            return;
        }
        match name {
            "para" | "simpara" => {
                let blocks = self.mixed(&element.children, context, Flow::Para);
                out.extend(wrap_blocks(&attr, blocks));
            }
            "programlisting" | "screen" | "literallayout" => {
                out.push(self.verbatim_block(element, name, attr));
            }
            "blockquote" if element.child("title").is_some() => {
                out.push(self.titled_quotation(element, attr, context));
            }
            "blockquote" | "epigraph" => {
                out.extend(wrap_blocks(&attr, vec![self.quotation(element, context)]));
            }
            "informalexample" => out.push(Block::Div(
                Box::new(classed(attr, name)),
                self.blocks(&element.children, context),
            )),
            "abstract" => out.extend(wrap_blocks(
                &attr,
                vec![Block::BlockQuote(self.blocks(&element.children, context))],
            )),
            "bridgehead" => out.extend(wrap_blocks(
                &attr,
                vec![Block::Para(vec![Inline::Strong(
                    self.inlines_trimmed(&element.children, Style::default()),
                )])],
            )),
            "itemizedlist" | "calloutlist" | "orderedlist" | "procedure" | "substeps"
            | "stepalternatives" | "variablelist" | "glossdiv" | "glosslist" => {
                self.list_block(element, name, &attr, context, out);
            }
            "question" | "answer" => {
                let label = if name == "question" { "Q:" } else { "A:" };
                let mut blocks = self.blocks(&element.children, context);
                label_first_paragraph(&mut blocks, label);
                out.extend(blocks);
            }
            "biblioentry" | "bibliomixed" => out.extend(wrap_blocks(
                &attr,
                self.mixed(&element.children, context, Flow::Para),
            )),
            "glosssee" | "glossseealso" => {
                let lead = if name == "glosssee" {
                    "See "
                } else {
                    "See also "
                };
                let mut inlines = Vec::new();
                push_text(lead, &mut inlines);
                inlines.extend(self.inlines_trimmed(&element.children, Style::default()));
                inlines.push(Inline::Str(".".into()));
                out.push(Block::Para(normalize(inlines)));
            }
            "equation" => out.extend(wrap_blocks(&attr, vec![Block::Para(display_math(element))])),
            "informalequation" => {
                let mut attr = attr;
                attr.classes.push(name.into());
                out.push(Block::Div(
                    Box::new(attr),
                    vec![Block::Para(display_math(element))],
                ));
            }
            "figure" | "informalfigure" => {
                let figure = Block::Figure(
                    Box::new(Attr {
                        id: attr.id.clone(),
                        ..Attr::default()
                    }),
                    Box::new(self.caption_of(element)),
                    self.figure_content(element, context),
                );
                out.extend(wrap_blocks(&attr, vec![figure]));
            }
            "mediaobject" => out.push(Block::Para(vec![self.image(element)])),
            "table" | "informaltable" => {
                out.push(Block::Table(Box::new(self.table(element, attr, context))));
            }
            _ => out.extend(self.blocks(&element.children, context)),
        }
    }

    /// Converts a division that may carry a title, as a division classed by its element name. An
    /// admonition keeps an empty title division so its label survives even when the source omits one.
    fn titled_division(
        &self,
        element: &'a Element,
        name: &str,
        attr: Attr,
        context: Context,
    ) -> Block {
        let mut attr = attr;
        attr.classes.push(name.into());
        let title = self.title_content(element);
        let mut children = Vec::new();
        if ADMONITIONS.contains(&name) || !title.is_empty() {
            children.push(Block::Div(Box::new(class_attr("title")), title));
        }
        children.extend(self.blocks(&element.children, context));
        Block::Div(Box::new(attr), children)
    }

    /// Converts an element whose content is laid out as written. A monospaced `literallayout` keeps
    /// the code-block shape of the listing elements; a plain one keeps its line breaks instead.
    fn verbatim_block(&self, element: &'a Element, name: &str, attr: Attr) -> Block {
        if name == "literallayout" && element.attr("class") != Some("monospaced") {
            return Block::LineBlock(split_lines(
                self.inlines(&element.children, Style::verbatim()),
            ));
        }
        Block::CodeBlock(
            Box::new(code_attr(element, attr)),
            verbatim(&element.text()).into(),
        )
    }

    /// Converts a `blockquote` that carries a title, as a division holding the title and the
    /// quotation.
    fn titled_quotation(&self, element: &'a Element, attr: Attr, context: Context) -> Block {
        let mut attr = attr;
        // The division answers for the quotation, so it states the quotation's attributes beside
        // the ones it holds in its own right.
        let own = attr.attributes.clone();
        attr.attributes.extend(own);
        Block::Div(
            Box::new(attr),
            vec![
                Block::Div(Box::new(class_attr("title")), self.title_content(element)),
                self.quotation(element, context),
            ],
        )
    }

    /// Converts a list-shaped element, whose flavor decides the item element it gathers and the
    /// list node it produces.
    fn list_block(
        &self,
        element: &'a Element,
        name: &str,
        attr: &Attr,
        context: Context,
        out: &mut Vec<Block>,
    ) {
        match name {
            "variablelist" => {
                let entries = self.variable_list(element, context);
                out.extend(wrap_blocks(attr, vec![Block::DefinitionList(entries)]));
            }
            "glossdiv" | "glosslist" => {
                let entries = self.glossary_entries(element, context);
                out.extend(wrap_blocks(attr, vec![Block::DefinitionList(entries)]));
            }
            "procedure" | "substeps" | "stepalternatives" => {
                let items = self.list_items(element, "step", context);
                let attributes = ListAttributes {
                    start: 1,
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::DefaultDelim,
                };
                out.extend(wrap_blocks(
                    attr,
                    vec![Block::OrderedList(attributes, items)],
                ));
            }
            "orderedlist" => {
                let items = self.list_items(element, "listitem", context);
                let list = Block::OrderedList(list_attributes(element), items);
                self.titled_list(element, attr, list, out);
            }
            _ => {
                let item = if name == "calloutlist" {
                    "callout"
                } else {
                    "listitem"
                };
                let items = self.list_items(element, item, context);
                self.titled_list(element, attr, Block::BulletList(items), out);
            }
        }
    }

    fn section(&self, element: &'a Element, heading: i32, inner: Context, out: &mut Vec<Block>) {
        let mut attr = attr_of(element);
        if local_name(&element.name) == "simplesect" {
            attr.classes.push("unnumbered".into());
        }
        let title = section_title_of(element)
            .map(|title| self.inlines_trimmed(&title.children, Style::default()))
            .unwrap_or_default();
        // A whole document names the work rather than a division of it, so it opens no heading.
        if !METADATA_ROOTS.contains(&local_name(&element.name)) {
            out.push(Block::Header(i64::from(heading), Box::new(attr), title));
        }
        out.extend(self.blocks(&element.children, inner));
    }

    /// A quotation's body, with any `attribution` set as a closing paragraph.
    fn quotation(&self, element: &'a Element, context: Context) -> Block {
        let mut blocks = Vec::new();
        let mut run = Vec::new();
        for node in &element.children {
            match node {
                Node::Text(text) => push_text(text, &mut run),
                Node::Element(child) if local_name(&child.name) == "attribution" => {}
                Node::Element(child) => {
                    flush(&mut run, Flow::Plain, &mut blocks);
                    self.block(child, context, &mut blocks);
                }
            }
        }
        flush(&mut run, Flow::Plain, &mut blocks);
        if let Some(attribution) = element.child("attribution") {
            let mut inlines = vec![Inline::Str("\u{2014} ".into())];
            inlines.extend(self.inlines_trimmed(&attribution.children, Style::default()));
            blocks.push(Block::Para(normalize(inlines)));
        }
        Block::BlockQuote(blocks)
    }

    /// A list wrapped in a division when it carries a title, so the title survives alongside it.
    fn titled_list(&self, element: &'a Element, attr: &Attr, list: Block, out: &mut Vec<Block>) {
        let Some(title) = element.child("title") else {
            out.extend(wrap_blocks(attr, vec![list]));
            return;
        };
        let heading = Block::Div(
            Box::new(class_attr("title")),
            vec![Block::Plain(
                self.inlines_trimmed(&title.children, Style::default()),
            )],
        );
        out.extend(wrap_blocks(
            attr,
            vec![Block::Div(Box::default(), vec![heading, list])],
        ));
    }

    fn list_items(&self, element: &'a Element, item: &str, context: Context) -> Vec<Vec<Block>> {
        let compact = element.attr("spacing") == Some("compact");
        element
            .elements()
            .filter(|child| local_name(&child.name) == item)
            .map(|child| {
                let mut blocks = self.blocks(&child.children, context);
                if compact {
                    tighten(&mut blocks);
                }
                blocks
            })
            .collect()
    }

    fn variable_list(
        &self,
        element: &'a Element,
        context: Context,
    ) -> Vec<(Vec<Inline>, Vec<Vec<Block>>)> {
        element
            .elements()
            .filter(|entry| local_name(&entry.name) == "varlistentry")
            .map(|entry| {
                let mut terms: Vec<Inline> = Vec::new();
                for term in entry.elements().filter(|e| local_name(&e.name) == "term") {
                    if !terms.is_empty() {
                        terms.push(Inline::Str("; ".into()));
                    }
                    terms.extend(self.inlines_trimmed(&term.children, Style::default()));
                }
                let definitions = entry
                    .elements()
                    .filter(|e| local_name(&e.name) == "listitem")
                    .map(|item| self.blocks(&item.children, context))
                    .collect();
                (normalize(terms), definitions)
            })
            .collect()
    }

    fn glossary_entries(
        &self,
        element: &'a Element,
        context: Context,
    ) -> Vec<(Vec<Inline>, Vec<Vec<Block>>)> {
        element
            .elements()
            .filter(|entry| local_name(&entry.name) == "glossentry")
            .map(|entry| {
                let term = entry
                    .child("glossterm")
                    .map(|term| self.inlines_trimmed(&term.children, Style::default()))
                    .unwrap_or_default();
                let definitions = entry
                    .elements()
                    .filter(|e| local_name(&e.name) == "glossdef")
                    .map(|def| self.blocks(&def.children, context))
                    .collect();
                (term, definitions)
            })
            .collect()
    }

    fn caption_of(&self, element: &'a Element) -> Caption {
        let source = element.child("title").or_else(|| element.child("caption"));
        Caption {
            short: None,
            long: source
                .map(|title| {
                    vec![Block::Plain(
                        self.inlines_trimmed(&title.children, Style::default()),
                    )]
                })
                .unwrap_or_default(),
        }
    }

    /// A figure's body. A media object stands alone as the figure's content rather than opening a
    /// paragraph of its own.
    fn figure_content(&self, element: &'a Element, context: Context) -> Vec<Block> {
        let mut out = Vec::new();
        for child in element.elements() {
            match local_name(&child.name) {
                "title" | "caption" | "titleabbrev" | "info" => {}
                "mediaobject" | "inlinemediaobject" => {
                    out.push(Block::Plain(vec![self.image(child)]));
                }
                _ => self.block(child, context, &mut out),
            }
        }
        out
    }

    fn title_content(&self, element: &'a Element) -> Vec<Block> {
        element
            .child("title")
            .map(|title| {
                vec![Block::Plain(
                    self.inlines_trimmed(&title.children, Style::default()),
                )]
            })
            .unwrap_or_default()
    }
}

// Tables

impl<'a> Converter<'a> {
    /// A table, in whichever of the two vocabularies it is written: a column-group model whose rows
    /// live in a `tgroup`, or a row-and-cell model whose rows stand in the table itself.
    fn table(&self, element: &'a Element, attr: Attr, context: Context) -> Table {
        let caption = self.caption_of(element);
        match element.child("tgroup") {
            Some(group) => self.column_group_table(group, attr, caption, context),
            None => self.row_table(element, attr, caption, context),
        }
    }

    /// A table whose rows carry `td` and `th` cells directly. The columns are as many as the widest
    /// row holds, capped by the `col` elements the table declares, and a cell spans exactly one of
    /// them however many it claims.
    fn row_table(
        &self,
        element: &'a Element,
        attr: Attr,
        caption: Caption,
        context: Context,
    ) -> Table {
        let mut columns: Vec<&Element> = Vec::new();
        for child in element.elements() {
            match local_name(&child.name) {
                "col" => columns.push(child),
                "colgroup" => columns.extend(
                    child
                        .elements()
                        .filter(|column| local_name(&column.name) == "col"),
                ),
                _ => {}
            }
        }
        let rows_of = |section: &str| -> Vec<&Element> {
            element
                .child(section)
                .into_iter()
                .flat_map(Element::elements)
                .filter(|row| local_name(&row.name) == "tr")
                .collect()
        };
        let head_rows = rows_of("thead");
        // Rows stated outside a body section only stand for the body when the table states none.
        let body_rows = if element.child("tbody").is_some() {
            rows_of("tbody")
        } else {
            element
                .elements()
                .filter(|row| local_name(&row.name) == "tr")
                .collect()
        };
        let widest = head_rows
            .iter()
            .chain(body_rows.iter())
            .map(|row| row.elements().filter(|cell| is_row_cell(cell)).count())
            .max()
            .unwrap_or(0);
        let count = if columns.is_empty() {
            widest
        } else {
            widest.min(columns.len())
        };
        let col_specs = (0..count)
            .map(|index| ColSpec {
                align: columns
                    .get(index)
                    .and_then(|column| column.attr("align"))
                    .map_or(Alignment::AlignDefault, alignment),
                width: ColWidth::ColWidthDefault,
            })
            .collect();
        Table {
            attr,
            caption,
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: self.cell_rows(&head_rows, count, context),
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: self.cell_rows(&body_rows, count, context),
            }],
            foot: TableFoot::default(),
        }
    }

    /// Converts rows of `td` and `th` cells, each row cut or filled out to `columns` cells.
    fn cell_rows(&self, rows: &[&'a Element], columns: usize, context: Context) -> Vec<Row> {
        rows.iter()
            .map(|row| {
                let mut cells: Vec<Cell> = row
                    .elements()
                    .filter(|cell| is_row_cell(cell))
                    .take(columns)
                    .map(|cell| Cell {
                        attr: Attr::default(),
                        align: cell
                            .attr("align")
                            .map_or(Alignment::AlignDefault, alignment),
                        row_span: 1,
                        col_span: 1,
                        content: self.mixed(&cell.children, context, Flow::Plain),
                    })
                    .collect();
                cells.resize_with(columns, || Cell {
                    attr: Attr::default(),
                    align: Alignment::AlignDefault,
                    row_span: 1,
                    col_span: 1,
                    content: Vec::new(),
                });
                Row {
                    attr: Attr::default(),
                    cells,
                }
            })
            .collect()
    }

    /// A table whose rows stand in a `tgroup` beside the specifications of its columns.
    fn column_group_table(
        &self,
        group: &'a Element,
        attr: Attr,
        caption: Caption,
        context: Context,
    ) -> Table {
        let columns: Vec<&Element> = group
            .elements()
            .filter(|child| local_name(&child.name) == "colspec")
            .collect();
        let names: Vec<&str> = columns
            .iter()
            .map(|column| column.attr("colname").unwrap_or_default())
            .collect();
        let rows_of = |section: &str| -> Vec<&Element> {
            group
                .child(section)
                .into_iter()
                .flat_map(Element::elements)
                .filter(|row| local_name(&row.name) == "row")
                .collect()
        };
        let head_rows = rows_of("thead");
        let body_rows = rows_of("tbody");
        let widest = head_rows
            .iter()
            .chain(body_rows.iter())
            .map(|row| {
                row.elements()
                    .filter(|cell| local_name(&cell.name) == "entry")
                    .count()
            })
            .max()
            .unwrap_or(0);
        let widths = proportional_widths(&columns);
        let count = if widths.is_empty() {
            widest
        } else {
            widths.len()
        };
        let col_specs = (0..count)
            .map(|index| ColSpec {
                align: columns
                    .get(index)
                    .and_then(|column| column.attr("align"))
                    .map_or(Alignment::AlignDefault, alignment),
                width: widths
                    .get(index)
                    .copied()
                    .map_or(ColWidth::ColWidthDefault, ColWidth::ColWidth),
            })
            .collect();
        let head = self.table_section(&head_rows, &names, count, context);
        let body = self.table_section(&body_rows, &names, count, context);
        Table {
            attr,
            caption,
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: head,
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body,
            }],
            foot: TableFoot::default(),
        }
    }

    /// Converts a header or body section, resolving each cell's placement: a vertical span covers
    /// the cells below it, and a row that runs out of free columns drops the entries that overflow.
    fn table_section(
        &self,
        rows: &[&'a Element],
        names: &[&str],
        columns: usize,
        context: Context,
    ) -> Vec<Row> {
        let mut covered = vec![0usize; columns];
        let mut out = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            let remaining = rows.len() - index;
            let mut column = 0;
            let mut cells = Vec::new();
            for entry in row
                .elements()
                .filter(|cell| local_name(&cell.name) == "entry")
            {
                while covered.get(column).copied().unwrap_or(0) > 0 {
                    column += 1;
                }
                if column >= columns {
                    break;
                }
                let col_span = column_span(entry, names).min(columns - column);
                let row_span = row_span(entry).min(remaining);
                for offset in 0..col_span {
                    if let Some(slot) = covered.get_mut(column + offset) {
                        *slot = row_span;
                    }
                }
                column += col_span;
                cells.push(Cell {
                    attr: Attr::default(),
                    align: entry
                        .attr("align")
                        .map_or(Alignment::AlignDefault, alignment),
                    row_span: i64::try_from(row_span).unwrap_or(1),
                    col_span: i64::try_from(col_span).unwrap_or(1),
                    content: self.mixed(&entry.children, context, Flow::Plain),
                });
            }
            loop {
                while covered.get(column).copied().unwrap_or(0) > 0 {
                    column += 1;
                }
                if column >= columns {
                    break;
                }
                column += 1;
                cells.push(Cell {
                    attr: Attr::default(),
                    align: Alignment::AlignDefault,
                    row_span: 1,
                    col_span: 1,
                    content: Vec::new(),
                });
            }
            for slot in &mut covered {
                *slot = slot.saturating_sub(1);
            }
            out.push(Row {
                attr: Attr::default(),
                cells,
            });
        }
        out
    }
}

/// Whether an element is a cell of a row-and-cell table.
fn is_row_cell(element: &Element) -> bool {
    matches!(local_name(&element.name), "td" | "th")
}

/// How many rows a cell claims, from the extra rows it declares below its own.
fn row_span(cell: &Element) -> usize {
    cell.attr("morerows")
        .and_then(|more| more.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .saturating_add(1)
}

/// How many columns a cell spans, from the named first and last columns it covers.
fn column_span(cell: &Element, names: &[&str]) -> usize {
    let index = |key: &str| -> Option<usize> {
        let name = cell.attr(key)?;
        names.iter().position(|candidate| *candidate == name)
    };
    match (index("namest"), index("nameend")) {
        (Some(start), Some(end)) if end >= start => end - start + 1,
        _ => 1,
    }
}

fn alignment(value: &str) -> Alignment {
    match value {
        "left" => Alignment::AlignLeft,
        "right" => Alignment::AlignRight,
        "center" => Alignment::AlignCenter,
        _ => Alignment::AlignDefault,
    }
}

/// Each column's share of the table's width, or nothing when any column leaves its width unstated.
/// The stated widths are read as bare proportions, whatever unit they are written in.
fn proportional_widths(columns: &[&Element]) -> Vec<f64> {
    let mut shares = Vec::with_capacity(columns.len());
    for column in columns {
        let Some(share) = column.attr("colwidth").and_then(leading_number) else {
            return Vec::new();
        };
        shares.push(share);
    }
    let total: f64 = shares.iter().sum();
    if total <= 0.0 {
        return Vec::new();
    }
    shares.iter().map(|share| share / total).collect()
}

/// The number a measurement starts with, ignoring whatever unit or marker follows it.
fn leading_number(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    let digits = trimmed
        .find(|ch: char| !ch.is_ascii_digit() && ch != '.' && ch != '-' && ch != '+')
        .unwrap_or(trimmed.len());
    trimmed.get(..digits)?.parse().ok()
}

// Inline-level conversion

impl<'a> Converter<'a> {
    fn inlines(&self, nodes: &'a [Node], style: Style) -> Vec<Inline> {
        let mut out = Vec::new();
        for node in nodes {
            match node {
                Node::Text(text) if style.verbatim => push_verbatim(text, &mut out),
                Node::Text(text) => push_text(text, &mut out),
                Node::Element(element) => self.inline(element, style, &mut out),
            }
        }
        normalize(out)
    }

    fn inlines_trimmed(&self, nodes: &'a [Node], style: Style) -> Vec<Inline> {
        trim(self.inlines(nodes, style))
    }

    /// Converts one inline-level element, appending its inlines to `out`. An element outside the
    /// table is transparent, contributing its children in place.
    fn inline(&self, element: &'a Element, style: Style, out: &mut Vec<Inline>) {
        let name = local_name(&element.name);
        let attr = attr_of(element);
        if CODE_ELEMENTS.contains(&name) {
            let text = element.text();
            let text = if style.verbatim {
                text
            } else {
                collapse_spaces(&text)
            };
            out.push(Inline::Code(Box::new(attr), text.as_str().into()));
            return;
        }
        match name {
            "title" | "subtitle" | "titleabbrev" | "info" | "index" => {}
            "citerefentry" => out.push(Inline::Code(
                Box::new(classed(attr, "citerefentry")),
                manual_page(element).as_str().into(),
            )),
            "emphasis" => {
                let content = self.inlines(&element.children, style);
                out.push(match element.attr("role") {
                    Some("strong" | "bold") => Inline::Strong(content),
                    Some("underline") => Inline::Underline(content),
                    Some("strikethrough") => Inline::Strikeout(content),
                    _ => Inline::Emph(content),
                });
            }
            "foreignphrase" | "wordasword" => {
                out.push(Inline::Emph(self.inlines(&element.children, style)));
            }
            "superscript" => {
                out.push(Inline::Superscript(self.inlines(&element.children, style)));
            }
            "subscript" => {
                out.push(Inline::Subscript(self.inlines(&element.children, style)));
            }
            "quote" => {
                let mark = if style.quotes.is_multiple_of(2) {
                    QuoteType::DoubleQuote
                } else {
                    QuoteType::SingleQuote
                };
                let content = self.inlines(&element.children, style.inside_quote());
                out.extend(wrap_inlines(&attr, vec![Inline::Quoted(mark, content)]));
            }
            "varargs" => out.push(Inline::Code(Box::new(attr), "(...)".into())),
            "replaceable" | "optional" => {
                let (open, close) = if name == "optional" {
                    ("[", "]")
                } else {
                    ("<", ">")
                };
                out.push(Inline::Str(open.into()));
                out.extend(self.inlines(&element.children, style));
                out.push(Inline::Str(close.into()));
            }
            "anchor" | "indexterm" | "phrase" | "menuchoice" | "keycombo" => {
                out.extend(self.inline_span(element, name, attr, style));
            }
            "link" | "ulink" | "uri" | "email" | "xref" => {
                out.push(self.inline_link(element, name, attr, style));
            }
            "footnote" => {
                out.push(Inline::Note(
                    self.blocks(&element.children, Context::default()),
                ));
            }
            "inlineequation" => out.extend(
                math_source(element).map(|tex| Inline::Math(MathType::InlineMath, tex.into())),
            ),
            "equation" | "informalequation" => out.extend(display_math(element)),
            "inlinemediaobject" | "mediaobject" => out.push(self.image(element)),
            _ => {
                for node in &element.children {
                    match node {
                        Node::Text(text) if style.verbatim => push_verbatim(text, out),
                        Node::Text(text) => push_text(text, out),
                        Node::Element(child) => self.inline(child, style, out),
                    }
                }
            }
        }
    }

    /// Converts one of the elements that annotate their surroundings rather than style them, each
    /// becoming a span that carries its own kind.
    fn inline_span(
        &self,
        element: &'a Element,
        name: &str,
        attr: Attr,
        style: Style,
    ) -> Vec<Inline> {
        let mut attr = attr;
        match name {
            "anchor" => vec![Inline::Span(Box::new(attr), Vec::new())],
            "indexterm" => {
                attr.classes.push("indexterm".into());
                if let Some(role) = element.attr("role") {
                    attr.classes.push(role.into());
                }
                for key in ["primary", "secondary", "tertiary", "see", "seealso"] {
                    if let Some(term) = element.child(key) {
                        attr.attributes.push((key.into(), term.text().into()));
                    }
                }
                for key in ["significance", "startref", "scope", "class"] {
                    if let Some(value) = element.attr(key) {
                        attr.attributes.push((key.into(), value.into()));
                    }
                }
                vec![Inline::Span(Box::new(attr), Vec::new())]
            }
            "menuchoice" | "keycombo" => {
                let separator: Vec<Inline> = if name == "keycombo" {
                    vec![Inline::Str("+".into())]
                } else {
                    vec![Inline::Space, Inline::Str(">".into()), Inline::Space]
                };
                attr.classes.push(name.into());
                let mut content = Vec::new();
                for child in element
                    .elements()
                    .filter(|child| local_name(&child.name) != "shortcut")
                {
                    if !content.is_empty() {
                        content.extend(separator.iter().cloned());
                    }
                    self.inline(child, style, &mut content);
                }
                vec![Inline::Span(Box::new(attr), normalize(content))]
            }
            _ => {
                if let Some(role) = element.attr("role") {
                    attr.classes.push(role.into());
                }
                let content = self.inlines(&element.children, style);
                if attr == Attr::default() {
                    content
                } else {
                    vec![Inline::Span(Box::new(attr), content)]
                }
            }
        }
    }

    /// Converts one of the linking elements, each of which names its destination differently.
    fn inline_link(&self, element: &'a Element, name: &str, attr: Attr, style: Style) -> Inline {
        let mut attr = attr;
        let url = match name {
            "ulink" => element.attr("url").unwrap_or_default().to_owned(),
            "uri" => element.text(),
            "email" => format!("mailto:{}", element.text()),
            "xref" => format!("#{}", element.attr("linkend").unwrap_or_default()),
            _ => {
                if let Some(role) = element.attr("role") {
                    attr.classes.push(role.into());
                }
                match element.attr("href") {
                    Some(href) => href.to_owned(),
                    None => format!("#{}", element.attr("linkend").unwrap_or_default()),
                }
            }
        };
        let mut content = if name == "xref" {
            self.cross_reference(element)
        } else {
            self.inlines(&element.children, style)
        };
        if content.is_empty() && name == "link" {
            content.push(Inline::Str(url.as_str().into()));
        }
        link(attr, content, &url)
    }

    /// The text a cross reference stands in for: the wording named by `endterm`, else the wording
    /// the target states for itself in `xreflabel`, else the target's title, else a placeholder
    /// naming what kind of target it is. A target that would be cited by its number has no such
    /// placeholder, so an untitled one is left unnamed.
    fn cross_reference(&self, element: &'a Element) -> Vec<Inline> {
        if let Some(source) = element.attr("endterm").and_then(|id| self.ids.get(id)) {
            return direct_text(source);
        }
        let Some(target) = self.ids.get(element.attr("linkend").unwrap_or_default()) else {
            return vec![Inline::Str("???".into())];
        };
        if let Some(label) = target.attr("xreflabel").filter(|label| !label.is_empty()) {
            let mut out = Vec::new();
            push_text(label, &mut out);
            return normalize(out);
        }
        let name = local_name(&target.name);
        if !TITLED_TARGETS.contains(&name) {
            return vec![Inline::Str(format!("{name}_title").into())];
        }
        match section_title_of(target) {
            Some(title) => direct_text(title),
            None => vec![Inline::Str("???".into())],
        }
    }

    /// The image a media object stands for: the first `imagedata` it holds, described by the
    /// object's alternative text, its text object, or its caption, whichever it states first.
    fn image(&self, element: &'a Element) -> Inline {
        let mut attr = attr_of(element);
        let data = element
            .child("imageobject")
            .and_then(|object| object.child("imagedata"));
        if let Some(role) = data.and_then(|data| data.attr("role")) {
            attr.classes.push(role.into());
        }
        for (source, key) in [("width", "width"), ("depth", "height")] {
            if let Some(value) = data.and_then(|data| data.attr(source)) {
                attr.attributes.push((key.into(), value.into()));
            }
        }
        let alt = element
            .child("alt")
            .or_else(|| element.child("textobject"))
            .or_else(|| element.child("caption"))
            .map(|source| self.inlines_trimmed(&source.children, Style::default()))
            .unwrap_or_default();
        // The image object states the title, in the character data of its own bibliographic title.
        let title = element
            .child("imageobject")
            .and_then(|object| object.child("objectinfo"))
            .and_then(|info| info.child("title"))
            .map(character_data)
            .unwrap_or_default();
        Inline::Image(
            Box::new(attr),
            alt,
            Box::new(Target {
                url: data
                    .and_then(|data| data.attr("fileref"))
                    .unwrap_or_default()
                    .into(),
                title: title.as_str().into(),
            }),
        )
    }
}

/// An equation set on its own line, or nothing when it carries no math this reader can render.
fn display_math(element: &Element) -> Vec<Inline> {
    math_source(element)
        .map(|tex| Inline::Math(MathType::DisplayMath, tex.into()))
        .into_iter()
        .collect()
}

/// The TeX an equation carries, written either as a `mathphrase` or as embedded `MathML`.
fn math_source(element: &Element) -> Option<String> {
    element
        .elements()
        .find_map(|child| match local_name(&child.name) {
            "mathphrase" => Some(child.text()),
            "math" => Some(to_tex(child)),
            _ => None,
        })
}

fn link(attr: Attr, content: Vec<Inline>, url: &str) -> Inline {
    Inline::Link(
        Box::new(attr),
        content,
        Box::new(Target {
            url: url.into(),
            title: Text::default(),
        }),
    )
}

// Attributes

/// The identifier and role an element carries. Every other `DocBook` attribute describes profiling
/// or presentation the document model has no place for.
/// The character data written directly inside an element, leaving out whatever its markup holds.
fn character_data(element: &Element) -> String {
    element
        .children
        .iter()
        .filter_map(|node| match node {
            Node::Text(text) => Some(text.as_str()),
            Node::Element(_) => None,
        })
        .collect()
}

fn attr_of(element: &Element) -> Attr {
    Attr {
        id: element.attr("id").unwrap_or_default().into(),
        classes: Vec::new(),
        attributes: element
            .attr("role")
            .map(|role| vec![(Text::from("role"), Text::from(role))])
            .unwrap_or_default(),
    }
}

/// The attributes of an element named by its kind rather than by an identifier of its own.
fn classed(attr: Attr, class: &str) -> Attr {
    Attr {
        id: Text::default(),
        classes: vec![class.into()],
        ..attr
    }
}

fn class_attr(class: &str) -> Attr {
    Attr {
        classes: vec![class.into()],
        ..Attr::default()
    }
}

/// A verbatim block's attributes: the element's own, plus the source language and a request for
/// line numbers as classes.
fn code_attr(element: &Element, attr: Attr) -> Attr {
    let mut attr = attr;
    if let Some(language) = element.attr("language") {
        attr.classes.push(language.into());
    }
    if element.attr("linenumbering") == Some("numbered") {
        attr.classes.push("numberLines".into());
    }
    attr
}

/// Wraps blocks in a division carrying attributes the blocks themselves cannot hold.
fn wrap_blocks(attr: &Attr, blocks: Vec<Block>) -> Vec<Block> {
    if attr.attributes.is_empty() {
        return blocks;
    }
    vec![Block::Div(Box::new(wrapper_attr(attr)), blocks)]
}

fn wrap_inlines(attr: &Attr, inlines: Vec<Inline>) -> Vec<Inline> {
    if attr.attributes.is_empty() {
        return inlines;
    }
    vec![Inline::Span(Box::new(wrapper_attr(attr)), inlines)]
}

fn wrapper_attr(attr: &Attr) -> Attr {
    let mut attributes = vec![(Text::from("wrapper"), Text::from("1"))];
    attributes.extend(attr.attributes.iter().cloned());
    Attr {
        attributes,
        ..Attr::default()
    }
}

// Sectioning

/// The heading level an element introduces and the counters its content sees, or `None` when the
/// element opens no section.
///
/// `parted` says the document divides into parts, which pushes every book-level division one level
/// down wherever it stands, since `part` claims the top level for the whole document.
fn section_level(name: &str, context: Context, parted: bool) -> Option<(i32, Context)> {
    let deeper = context.level.saturating_add(1);
    let component = 1 + i32::from(parted);
    // A whole document written inside another restarts the heading count at the depth of sections it
    // stands in, rather than carrying on from the divisions around it.
    if METADATA_ROOTS.contains(&name) && !context.at_head() {
        let base = context.section_depth(parted);
        return Some(if name == "book" {
            let deeper = base.saturating_add(1);
            (
                deeper,
                Context {
                    level: deeper,
                    component,
                },
            )
        } else {
            (
                base,
                Context {
                    level: base,
                    component: i32::from(parted),
                },
            )
        });
    }
    match name {
        // A bibliography division titles a shelf rather than opening a level, so it never deepens.
        "bibliodiv" => Some((1, context)),
        "section" | "simplesect" | "refsection" | "qandadiv" => Some((
            deeper,
            Context {
                level: deeper,
                ..context
            },
        )),
        "book" => Some((
            deeper,
            Context {
                level: deeper,
                component,
            },
        )),
        "chapter" => Some((
            component,
            Context {
                level: component,
                component,
            },
        )),
        // Back matter stands beside the components around it, so it hands its content the depth its
        // enclosing component sits at rather than the one its own heading takes.
        "appendix" | "preface" | "glossary" | "bibliography" => Some((
            component,
            Context {
                level: context.component,
                component: context.component,
            },
        )),
        "part" => Some((
            1,
            Context {
                level: 2,
                component: 2,
            },
        )),
        _ => {
            let numbered = name
                .strip_prefix("sect")
                .or_else(|| name.strip_prefix("refsect"))
                .and_then(|digit| digit.parse::<i32>().ok())
                .filter(|depth| (1..=5).contains(depth))?;
            let heading = context.component.saturating_add(numbered);
            Some((
                heading,
                Context {
                    level: heading,
                    ..context
                },
            ))
        }
    }
}

/// The title of a sectioning division, written either directly or inside its `info` wrapper. Only a
/// division names itself through `info`; elsewhere a title has to stand as a child of its own.
fn section_title_of(element: &Element) -> Option<&Element> {
    element
        .child("title")
        .or_else(|| element.child("info").and_then(|info| info.child("title")))
}

/// A manual page reference, written as the page's name followed by its section in parentheses.
fn manual_page(element: &Element) -> String {
    let mut out = element
        .child("refentrytitle")
        .map(Element::text)
        .unwrap_or_default();
    if let Some(volume) = element.child("manvolnum") {
        out.push('(');
        out.push_str(&volume.text());
        out.push(')');
    }
    out
}

// Inline sequence shaping

/// Splits text into words and the breaks between them. Only XML whitespace separates; the space
/// characters a document spells out as references stay part of their word.
fn push_text(text: &str, out: &mut Vec<Inline>) {
    let mut word = String::new();
    let mut pending: Option<bool> = None;
    for ch in text.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\r') {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word).as_str().into()));
            }
            let newline = ch == '\n' || ch == '\r';
            pending = Some(pending.unwrap_or(false) || newline);
        } else {
            if let Some(newline) = pending.take() {
                out.push(if newline {
                    Inline::SoftBreak
                } else {
                    Inline::Space
                });
            }
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word.as_str().into()));
    }
    if let Some(newline) = pending {
        out.push(if newline {
            Inline::SoftBreak
        } else {
            Inline::Space
        });
    }
}

/// Lays out preformatted character data: every space holds its width and every line division opens
/// a new line.
fn push_verbatim(text: &str, out: &mut Vec<Inline>) {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push(Inline::LineBreak);
        }
        out.push(Inline::Str(line.replace(' ', "\u{a0}").as_str().into()));
    }
}

/// Reduces every run of whitespace to a single space and drops the ones at either end.
fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Merges adjacent text runs and collapses each run of breaks into one, a line break winning over
/// a plain space.
fn normalize(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Str(text) => match out.last_mut() {
                Some(Inline::Str(previous)) => previous.push_str(&text),
                _ => out.push(Inline::Str(text)),
            },
            Inline::Space => {
                if !matches!(out.last(), Some(Inline::Space | Inline::SoftBreak)) {
                    out.push(Inline::Space);
                }
            }
            Inline::SoftBreak => match out.last_mut() {
                Some(last @ (Inline::Space | Inline::SoftBreak)) => *last = Inline::SoftBreak,
                _ => out.push(Inline::SoftBreak),
            },
            other => match (out.last_mut(), other) {
                (Some(last), other) if merges(last, &other) => extend_wrapper(last, other),
                (_, other) => out.push(other),
            },
        }
    }
    out
}

/// Whether two neighbouring inlines are the same kind of wrapper, and so read as one.
fn merges(left: &Inline, right: &Inline) -> bool {
    matches!(
        (left, right),
        (Inline::Emph(_), Inline::Emph(_))
            | (Inline::Underline(_), Inline::Underline(_))
            | (Inline::Strong(_), Inline::Strong(_))
            | (Inline::Strikeout(_), Inline::Strikeout(_))
            | (Inline::Superscript(_), Inline::Superscript(_))
            | (Inline::Subscript(_), Inline::Subscript(_))
            | (Inline::SmallCaps(_), Inline::SmallCaps(_))
    )
}

/// Folds a wrapper's content into the matching wrapper before it.
fn extend_wrapper(left: &mut Inline, right: Inline) {
    let (Inline::Emph(target)
    | Inline::Underline(target)
    | Inline::Strong(target)
    | Inline::Strikeout(target)
    | Inline::Superscript(target)
    | Inline::Subscript(target)
    | Inline::SmallCaps(target)) = left
    else {
        return;
    };
    let (Inline::Emph(source)
    | Inline::Underline(source)
    | Inline::Strong(source)
    | Inline::Strikeout(source)
    | Inline::Superscript(source)
    | Inline::Subscript(source)
    | Inline::SmallCaps(source)) = right
    else {
        return;
    };
    target.extend(source);
}

fn trim(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut inlines = inlines;
    while matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.remove(0);
    }
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
    inlines
}

fn flush(run: &mut Vec<Inline>, flow: Flow, out: &mut Vec<Block>) {
    let inlines = trim(normalize(std::mem::take(run)));
    if inlines.is_empty() {
        return;
    }
    out.push(match flow {
        Flow::Plain => Block::Plain(inlines),
        Flow::Para => Block::Para(inlines),
    });
}

/// Marks the opening paragraph of a question or answer with its label.
fn label_first_paragraph(blocks: &mut [Block], label: &str) {
    if let Some(Block::Para(inlines)) = blocks.first_mut() {
        let mut labelled = vec![
            Inline::Strong(vec![Inline::Str(label.into())]),
            Inline::Str(" ".into()),
        ];
        labelled.append(inlines);
        *inlines = normalize(labelled);
    }
}

/// Divides an inline run into the lines its explicit breaks mark out. A run holding no break is one
/// line, and an empty run is a single empty line.
fn split_lines(inlines: Vec<Inline>) -> Vec<Vec<Inline>> {
    let mut lines = vec![Vec::new()];
    for inline in inlines {
        if matches!(inline, Inline::LineBreak) {
            lines.push(Vec::new());
        } else if let Some(line) = lines.last_mut() {
            line.push(inline);
        }
    }
    lines
}

/// A numbered list's marker configuration. A numeration that is not a recognized numeral system,
/// or none at all, numbers the list in decimal.
fn list_attributes(element: &Element) -> ListAttributes {
    ListAttributes {
        start: element
            .attr("startingnumber")
            .and_then(|start| start.trim().parse().ok())
            .unwrap_or(1),
        style: match element.attr("numeration") {
            Some("loweralpha") => ListNumberStyle::LowerAlpha,
            Some("upperalpha") => ListNumberStyle::UpperAlpha,
            Some("lowerroman") => ListNumberStyle::LowerRoman,
            Some("upperroman") => ListNumberStyle::UpperRoman,
            _ => ListNumberStyle::Decimal,
        },
        delim: ListNumberDelim::DefaultDelim,
    }
}

/// Turns a compact item's paragraphs into plain content, so it sets without vertical space.
fn tighten(blocks: &mut [Block]) {
    for block in blocks {
        if let Block::Para(inlines) = block {
            *block = Block::Plain(std::mem::take(inlines));
        }
    }
}

/// Strips a code payload's framing newlines, leaving the indentation of the first and last lines.
fn verbatim(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    normalized.trim_matches('\n').to_owned()
}

// East Asian line breaks

/// Drops a soft line break falling between two East Asian wide characters, so wrapped text rejoins
/// with no intervening space.
fn drop_wide_breaks(blocks: &mut [Block]) {
    for block in blocks.iter_mut() {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
                drop_wide_breaks_in(inlines);
            }
            Block::LineBlock(lines) => lines.iter_mut().for_each(drop_wide_breaks_in),
            Block::BlockQuote(children) | Block::Div(_, children) => drop_wide_breaks(children),
            Block::Figure(_, caption, children) => {
                drop_wide_breaks(&mut caption.long);
                drop_wide_breaks(children);
            }
            Block::OrderedList(_, items) | Block::BulletList(items) => {
                for item in items {
                    drop_wide_breaks(item);
                }
            }
            Block::DefinitionList(entries) => {
                for (term, definitions) in entries {
                    drop_wide_breaks_in(term);
                    for definition in definitions {
                        drop_wide_breaks(definition);
                    }
                }
            }
            Block::Table(table) => {
                drop_wide_breaks(&mut table.caption.long);
                for row in table
                    .head
                    .rows
                    .iter_mut()
                    .chain(
                        table
                            .bodies
                            .iter_mut()
                            .flat_map(|body| body.body.iter_mut()),
                    )
                    .chain(table.foot.rows.iter_mut())
                {
                    for cell in &mut row.cells {
                        drop_wide_breaks(&mut cell.content);
                    }
                }
            }
            Block::CodeBlock(_, _) | Block::RawBlock(_, _) | Block::HorizontalRule => {}
        }
    }
}

/// Applies the same rejoining to a metadata value, which carries the same wording as the body.
fn drop_wide_breaks_in_meta(value: &mut MetaValue) {
    match value {
        MetaValue::MetaInlines(inlines) => drop_wide_breaks_in(inlines),
        MetaValue::MetaBlocks(blocks) => drop_wide_breaks(blocks),
        MetaValue::MetaList(values) => values.iter_mut().for_each(drop_wide_breaks_in_meta),
        MetaValue::MetaMap(entries) => {
            for entry in entries.values_mut() {
                drop_wide_breaks_in_meta(entry);
            }
        }
        MetaValue::MetaString(_) | MetaValue::MetaBool(_) => {}
    }
}

fn drop_wide_breaks_in(inlines: &mut Vec<Inline>) {
    for inline in inlines.iter_mut() {
        match inline {
            Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Cite(_, children)
            | Inline::Link(_, children, _)
            | Inline::Image(_, children, _)
            | Inline::Span(_, children) => drop_wide_breaks_in(children),
            Inline::Note(children) => drop_wide_breaks(children),
            _ => {}
        }
    }
    let mut index = 0;
    while index < inlines.len() {
        let joins = matches!(inlines.get(index), Some(Inline::SoftBreak))
            && index
                .checked_sub(1)
                .and_then(|previous| inlines.get(previous))
                .and_then(edge_char::<true>)
                .is_some_and(is_wide)
            && inlines
                .get(index + 1)
                .and_then(edge_char::<false>)
                .is_some_and(is_wide);
        if joins {
            inlines.remove(index);
        } else {
            index += 1;
        }
    }
}

/// The character an inline presents at one of its edges: its last when `LAST`, else its first. A
/// quotation presents the mark it is set in rather than its content, and so never a wide one.
fn edge_char<const LAST: bool>(inline: &Inline) -> Option<char> {
    match inline {
        Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => {
            if LAST {
                text.chars().next_back()
            } else {
                text.chars().next()
            }
        }
        Inline::Emph(children)
        | Inline::Underline(children)
        | Inline::Strong(children)
        | Inline::Strikeout(children)
        | Inline::Superscript(children)
        | Inline::Subscript(children)
        | Inline::SmallCaps(children)
        | Inline::Cite(_, children)
        | Inline::Link(_, children, _)
        | Inline::Image(_, children, _)
        | Inline::Span(_, children) => {
            if LAST {
                children.iter().rev().find_map(edge_char::<LAST>)
            } else {
                children.iter().find_map(edge_char::<LAST>)
            }
        }
        _ => None,
    }
}

/// Whether a character occupies a wide cell in East Asian text (Unicode East Asian Width Wide or
/// Fullwidth). Halfwidth and ambiguous-width characters are excluded.
fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1_7000..=0x1_87FF
        | 0x1_B000..=0x1_B2FF
        | 0x1_F200..=0x1_F2FF
        | 0x2_0000..=0x3_FFFD)
}

// XML scanning

/// A node inside an element: a child element or a run of character data.
#[derive(Debug)]
enum Node {
    Text(String),
    Element(Element),
}

/// A parsed element: its qualified name, its attributes in source order, and its child nodes.
#[derive(Debug, Default)]
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

impl Element {
    /// The value of the attribute whose local name is `key`.
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| local_name(name) == key)
            .map(|(_, value)| value.as_str())
    }

    fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            Node::Text(_) => None,
        })
    }

    /// The first child element whose local name is `key`.
    fn child(&self, key: &str) -> Option<&Element> {
        self.elements()
            .find(|element| local_name(&element.name) == key)
    }

    /// The concatenated character data of this element and its descendants.
    fn text(&self) -> String {
        let mut out = String::new();
        let mut stack: Vec<&Node> = self.children.iter().rev().collect();
        while let Some(node) = stack.pop() {
            match node {
                Node::Text(text) => out.push_str(text),
                Node::Element(element) => stack.extend(element.children.iter().rev()),
            }
        }
        out
    }
}

impl MathTree for Element {
    fn tag(&self) -> &str {
        local_name(&self.name)
    }
    fn attribute(&self, key: &str) -> Option<String> {
        self.attr(key).map(str::to_owned)
    }
    fn inner_text(&self) -> String {
        self.text()
    }
    fn element_children(&self) -> Vec<&Self> {
        self.elements().collect()
    }
    fn nth_element_child(&self, index: usize) -> Option<&Self> {
        self.elements().nth(index)
    }
}

/// The local part of a qualified name (`mml:math` becomes `math`, `xml:id` becomes `id`).
fn local_name(name: &str) -> &str {
    match name.rsplit_once(':') {
        Some((_, tail)) => tail,
        None => name,
    }
}

/// The wording an element carries in its own right, with any nested markup left out.
fn direct_text(element: &Element) -> Vec<Inline> {
    let mut joined = String::new();
    for node in &element.children {
        if let Node::Text(text) = node {
            joined.push_str(text);
        }
    }
    let mut out = Vec::new();
    push_text(&joined, &mut out);
    normalize(out)
}

/// Whether a `part` stands anywhere in the tree, wherever it is nested.
fn divides_into_parts(element: &Element) -> bool {
    element
        .elements()
        .any(|child| local_name(&child.name) == "part" || divides_into_parts(child))
}

/// Records every element under the identifier it claims, so cross references can resolve their
/// targets. The first claimant keeps the identifier, and an element that names itself not at all
/// answers to the empty one.
fn index_ids<'a>(element: &'a Element, ids: &mut BTreeMap<&'a str, &'a Element>) {
    ids.entry(element.attr("id").unwrap_or_default())
        .or_insert(element);
    for child in element.elements() {
        index_ids(child, ids);
    }
}

/// Parses a document into a synthetic root whose children are its top-level nodes. Never fails:
/// an unterminated construct ends the scan, a stray close tag is ignored, and elements left open
/// fold back into the root.
fn scan(input: &str) -> Element {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let bytes = input.as_bytes();
    let mut stack: Vec<Element> = vec![Element::default()];
    let mut index = 0;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'<') {
            if starts_with(bytes, index, b"<!--") {
                index = find(bytes, index + 4, b"-->").map_or(bytes.len(), |end| end + 3);
            } else if starts_with(bytes, index, b"<![CDATA[") {
                let end = find(bytes, index + 9, b"]]>").unwrap_or(bytes.len());
                if let Some(text) = input.get(index + 9..end) {
                    attach(&mut stack, Node::Text(text.to_owned()));
                }
                index = (end + 3).min(bytes.len());
            } else if starts_with(bytes, index, b"<!DOCTYPE") {
                index = doctype_end(bytes, index);
            } else if starts_with(bytes, index, b"<!") || starts_with(bytes, index, b"<?") {
                index = find_byte(bytes, index + 2, b'>').map_or(bytes.len(), |end| end + 1);
            } else if starts_with(bytes, index, b"</") {
                let end = find_byte(bytes, index, b'>').unwrap_or(bytes.len());
                close(&mut stack);
                index = (end + 1).min(bytes.len());
            } else {
                index = start_tag(input, index, &mut stack);
            }
        } else {
            let end = find_byte(bytes, index, b'<').unwrap_or(bytes.len());
            if let Some(text) = input.get(index..end)
                && !text.is_empty()
            {
                attach(&mut stack, Node::Text(decode_entities(text)));
            }
            index = end;
        }
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    stack.into_iter().next().unwrap_or_default()
}

/// Parses the start tag beginning at `start`, returning the index just past it.
fn start_tag(input: &str, start: usize, stack: &mut Vec<Element>) -> usize {
    let bytes = input.as_bytes();
    let mut index = start + 1;
    let name_start = index;
    while let Some(&byte) = bytes.get(index) {
        if matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/') {
            break;
        }
        index += 1;
    }
    let mut element = Element {
        name: input.get(name_start..index).unwrap_or_default().to_owned(),
        ..Element::default()
    };
    let mut self_closing = false;
    loop {
        index = skip_space(bytes, index);
        match bytes.get(index) {
            None => break,
            Some(&b'>') => {
                index += 1;
                break;
            }
            Some(&b'/') => {
                self_closing = true;
                index += 1;
            }
            Some(_) => {
                let key_start = index;
                while let Some(&byte) = bytes.get(index) {
                    if matches!(byte, b'=' | b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/') {
                        break;
                    }
                    index += 1;
                }
                let key = input.get(key_start..index).unwrap_or_default().to_owned();
                index = skip_space(bytes, index);
                let mut value = String::new();
                if bytes.get(index) == Some(&b'=') {
                    index = skip_space(bytes, index + 1);
                    if let Some(&quote) = bytes.get(index)
                        && (quote == b'"' || quote == b'\'')
                    {
                        let value_start = index + 1;
                        let value_end = find_byte(bytes, value_start, quote).unwrap_or(bytes.len());
                        value = input
                            .get(value_start..value_end)
                            .map(decode_entities)
                            .unwrap_or_default();
                        index = (value_end + 1).min(bytes.len());
                    }
                }
                if !key.is_empty() {
                    element.attrs.push((key, value));
                }
            }
        }
    }
    if self_closing || stack.len() >= MAX_DEPTH {
        attach(stack, Node::Element(element));
    } else {
        stack.push(element);
    }
    index
}

fn close(stack: &mut Vec<Element>) {
    if stack.len() <= 1 {
        return;
    }
    if let Some(element) = stack.pop() {
        attach(stack, Node::Element(element));
    }
}

fn attach(stack: &mut [Element], node: Node) {
    if let Some(top) = stack.last_mut() {
        top.children.push(node);
    }
}

/// The general entities a document declares for itself, by the name each answers to.
type Entities = BTreeMap<String, String>;

/// How many times a declaration's text is re-read for further references before it stands as it is.
const MAX_ENTITY_ROUNDS: usize = 8;

/// Ceiling on the text one substitution pass may add. Past it a reference stands as written, so a
/// declaration that nests inside itself cannot exhaust memory.
const MAX_ENTITY_GROWTH: usize = 1 << 22;

/// The general entities the internal declaration subset defines. A name the character reference
/// table already answers to keeps that answer, so a declaration cannot redefine it.
fn declared_entities(input: &str) -> Entities {
    let mut entities = Entities::new();
    let Some(subset) = internal_subset(input) else {
        return entities;
    };
    let bytes = subset.as_bytes();
    let mut index = 0;
    while let Some(start) = find(bytes, index, b"<!ENTITY") {
        index = start + 8;
        let mut cursor = skip_space(bytes, index);
        let name_start = cursor;
        while matches!(bytes.get(cursor), Some(byte) if !byte.is_ascii_whitespace() && *byte != b'>')
        {
            cursor += 1;
        }
        let name = subset.get(name_start..cursor).unwrap_or_default();
        cursor = skip_space(bytes, cursor);
        let Some(&quote) = bytes.get(cursor) else {
            continue;
        };
        if quote != b'"' && quote != b'\'' {
            continue;
        }
        let Some(end) = find_byte(bytes, cursor + 1, quote) else {
            continue;
        };
        index = end + 1;
        if name.is_empty() || lookup_named(name).is_some() {
            continue;
        }
        entities
            .entry(name.to_owned())
            .or_insert_with(|| subset.get(cursor + 1..end).unwrap_or_default().to_owned());
    }
    resolve_declarations(&mut entities);
    entities
}

/// Reads each declaration's text for references to the others, until none is left or the rounds run
/// out. A declaration that still names something after that stands with the reference in place.
fn resolve_declarations(entities: &mut Entities) {
    for _ in 0..MAX_ENTITY_ROUNDS {
        let snapshot = entities.clone();
        let mut budget = MAX_ENTITY_GROWTH;
        let mut changed = false;
        for value in entities.values_mut() {
            let expanded = substitute_entities(value, &snapshot, &mut budget).into_owned();
            changed |= expanded != *value;
            *value = expanded;
        }
        if !changed {
            return;
        }
    }
}

/// The text between the brackets of the document type declaration, when it states one.
fn internal_subset(input: &str) -> Option<&str> {
    let bytes = input.as_bytes();
    let start = find(bytes, 0, b"<!DOCTYPE")?;
    let end = doctype_end(bytes, start);
    let open = find_byte(bytes, start, b'[')?;
    let body = input.get(open + 1..end)?;
    body.rsplit_once(']').map(|(subset, _)| subset)
}

/// The index just past the document type declaration beginning at `start`, taking in its internal
/// subset and the quoted literals inside it.
fn doctype_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 2;
    let mut quote = None;
    let mut subset = false;
    while let Some(&byte) = bytes.get(index) {
        index += 1;
        match quote {
            Some(mark) if byte == mark => quote = None,
            Some(_) => {}
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'[' => subset = true,
                b']' => subset = false,
                b'>' if !subset => return index,
                _ => {}
            },
        }
    }
    bytes.len()
}

/// Replaces references to the declared entities with the text each stands for, leaving comments and
/// character data sections as written. What a declaration stands for is itself markup, so this runs
/// over the document text before the scan reads it.
fn substitute_entities<'a>(
    input: &'a str,
    entities: &Entities,
    budget: &mut usize,
) -> Cow<'a, str> {
    if entities.is_empty() || !input.contains('&') {
        return Cow::Borrowed(input);
    }
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut copied = 0;
    let mut index = 0;
    while let Some(offset) = bytes
        .get(index..)
        .and_then(|rest| rest.iter().position(|byte| matches!(byte, b'&' | b'<')))
    {
        index += offset;
        if starts_with(bytes, index, b"<!--") {
            index = find(bytes, index + 4, b"-->").map_or(bytes.len(), |end| end + 3);
            continue;
        }
        if starts_with(bytes, index, b"<![CDATA[") {
            index = find(bytes, index + 9, b"]]>").map_or(bytes.len(), |end| end + 3);
            continue;
        }
        if bytes.get(index) != Some(&b'&') {
            index += 1;
            continue;
        }
        let replacement = find_byte(bytes, index, b';')
            .and_then(|semi| Some((input.get(index + 1..semi)?, semi)))
            .and_then(|(name, semi)| Some((entities.get(name)?, semi)))
            .filter(|(text, _)| text.len() <= *budget);
        match replacement {
            Some((text, semi)) => {
                out.push_str(input.get(copied..index).unwrap_or_default());
                out.push_str(text);
                *budget -= text.len();
                index = semi + 1;
                copied = index;
            }
            None => index += 1,
        }
    }
    if copied == 0 {
        return Cow::Borrowed(input);
    }
    out.push_str(input.get(copied..).unwrap_or_default());
    Cow::Owned(out)
}

/// Replaces character references with the characters they name. A reference that names nothing, or
/// whose code point is out of range, is kept verbatim.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(rest.get(..amp).unwrap_or_default());
        let tail = rest.get(amp..).unwrap_or_default();
        if let Some(semi) = tail.find(';')
            && let Some(body) = tail.get(1..semi)
            && let Some(decoded) = resolve_reference(body)
        {
            out.push_str(&decoded);
            rest = tail.get(semi + 1..).unwrap_or_default();
            continue;
        }
        out.push('&');
        rest = tail.get(1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

fn resolve_reference(body: &str) -> Option<String> {
    let Some(digits) = body.strip_prefix('#') else {
        return lookup_named(body).map(str::to_owned);
    };
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    Some(code_point(code).to_string())
}

fn starts_with(bytes: &[u8], at: usize, prefix: &[u8]) -> bool {
    bytes.get(at..at + prefix.len()) == Some(prefix)
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let mut index = from;
    while index + needle.len() <= bytes.len() {
        if bytes.get(index..index + needle.len()) == Some(needle) {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn find_byte(bytes: &[u8], from: usize, needle: u8) -> Option<usize> {
    let mut index = from;
    while let Some(&byte) = bytes.get(index) {
        if byte == needle {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn skip_space(bytes: &[u8], from: usize) -> usize {
    let mut index = from;
    while matches!(bytes.get(index), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::{DocbookReader, Element, MAX_DEPTH, Node, decode_entities, scan};
    use carta_ast::{Attr, Block, Inline};
    use carta_core::{Extension, Extensions, Reader, ReaderOptions};

    fn read(input: &str) -> Vec<Block> {
        DocbookReader
            .read(input, &ReaderOptions::default())
            .expect("reader is infallible")
            .blocks
    }

    fn depth(element: &Element) -> usize {
        1 + element
            .children
            .iter()
            .map(|node| match node {
                Node::Element(child) => depth(child),
                Node::Text(_) => 0,
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn empty_input_yields_an_empty_document() {
        assert!(read("").is_empty());
    }

    #[test]
    fn paragraph_text_is_split_into_words_and_breaks() {
        assert_eq!(
            read("<article><para>a b\nc</para></article>"),
            vec![Block::Para(vec![
                Inline::Str("a".into()),
                Inline::Space,
                Inline::Str("b".into()),
                Inline::SoftBreak,
                Inline::Str("c".into()),
            ])]
        );
    }

    #[test]
    fn sections_nest_their_heading_levels() {
        let blocks = read(
            "<article><section><title>A</title><section><title>B</title></section></section></article>",
        );
        assert!(matches!(
            blocks.as_slice(),
            [Block::Header(1, _, _), Block::Header(2, _, _)]
        ));
    }

    #[test]
    fn chapters_sit_below_the_parts_enclosing_them() {
        let blocks =
            read("<book><part><title>P</title><chapter><title>C</title></chapter></part></book>");
        assert!(matches!(
            blocks.as_slice(),
            [Block::Header(1, _, _), Block::Header(2, _, _)]
        ));
    }

    #[test]
    fn a_block_child_splits_the_paragraph_around_it() {
        let blocks = read(
            "<article><para>a<itemizedlist><listitem><para>x</para></listitem></itemizedlist>b</para></article>",
        );
        assert!(matches!(
            blocks.as_slice(),
            [Block::Para(_), Block::BulletList(_), Block::Para(_)]
        ));
    }

    #[test]
    fn a_verbatim_block_loses_only_its_framing_newlines() {
        let blocks = read("<article><programlisting>\n  x \n</programlisting></article>");
        assert_eq!(
            blocks,
            vec![Block::CodeBlock(Box::default(), "  x ".into())]
        );
    }

    #[test]
    fn an_unknown_element_contributes_its_content() {
        assert_eq!(
            read("<article><nonesuch><para>a</para></nonesuch></article>"),
            vec![Block::Para(vec![Inline::Str("a".into())])]
        );
    }

    #[test]
    fn character_references_resolve_and_unknown_ones_stay_verbatim() {
        assert_eq!(decode_entities("&copy;&#65;&#x42;&nope;"), "\u{a9}AB&nope;");
    }

    #[test]
    fn character_data_is_not_taken_for_markup() {
        assert_eq!(
            read("<article><para><![CDATA[a &amp; <b>]]></para></article>"),
            vec![Block::Para(vec![
                Inline::Str("a".into()),
                Inline::Space,
                Inline::Str("&amp;".into()),
                Inline::Space,
                Inline::Str("<b>".into()),
            ])]
        );
    }

    #[test]
    fn an_index_term_leaves_only_a_marker_behind() {
        assert_eq!(
            read("<article><para>a<indexterm><primary>k</primary></indexterm></para></article>"),
            vec![Block::Para(vec![
                Inline::Str("a".into()),
                Inline::Span(
                    Box::new(Attr {
                        id: "".into(),
                        classes: vec!["indexterm".into()],
                        attributes: vec![("primary".into(), "k".into())],
                    }),
                    Vec::new(),
                ),
            ])]
        );
    }

    #[test]
    fn a_short_row_is_filled_out_to_the_table_width() {
        let blocks = read(concat!(
            "<article><informaltable><tgroup cols=\"2\"><tbody>",
            "<row><entry>a</entry><entry>b</entry></row><row><entry>c</entry></row>",
            "</tbody></tgroup></informaltable></article>"
        ));
        let [Block::Table(table)] = blocks.as_slice() else {
            panic!("expected a single table");
        };
        assert_eq!(table.col_specs.len(), 2);
        let widths: Vec<usize> = table
            .bodies
            .iter()
            .flat_map(|body| body.body.iter().map(|row| row.cells.len()))
            .collect();
        assert_eq!(widths, vec![2, 2]);
    }

    #[test]
    fn a_target_cited_by_number_stays_unnamed_without_a_title() {
        let blocks = read(concat!(
            "<article><sect1 id=\"s1\"><para>a</para></sect1>",
            "<note id=\"n1\"><para>b</para></note>",
            "<para><xref linkend=\"s1\"/><xref linkend=\"n1\"/></para></article>"
        ));
        let names: Vec<String> = blocks
            .iter()
            .flat_map(|block| match block {
                Block::Para(inlines) => inlines.clone(),
                _ => Vec::new(),
            })
            .filter_map(|inline| match inline {
                Inline::Link(_, content, _) => Some(format!("{content:?}")),
                _ => None,
            })
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.first().is_some_and(|name| name.contains("???")));
        assert!(names.get(1).is_some_and(|name| name.contains("note_title")));
    }

    #[test]
    fn a_cross_reference_falls_back_to_a_placeholder() {
        let blocks = read("<article><para><xref linkend=\"nowhere\"/></para></article>");
        assert_eq!(
            blocks,
            vec![Block::Para(vec![Inline::Link(
                Box::default(),
                vec![Inline::Str("???".into())],
                Box::new(carta_ast::Target {
                    url: "#nowhere".into(),
                    title: carta_ast::Text::default(),
                }),
            )])]
        );
    }

    #[test]
    fn wide_line_breaks_are_dropped_only_when_the_extension_asks() {
        let input = "<article><para>\u{4e00}\n\u{4e8c}</para></article>";
        let mut options = ReaderOptions::default();
        assert!(read(input).contains(&Block::Para(vec![
            Inline::Str("\u{4e00}".into()),
            Inline::SoftBreak,
            Inline::Str("\u{4e8c}".into()),
        ])));
        options.extensions = Extensions::from_list(&[Extension::EastAsianLineBreaks]);
        let blocks = DocbookReader
            .read(input, &options)
            .expect("reader is infallible")
            .blocks;
        assert_eq!(
            blocks,
            vec![Block::Para(vec![
                Inline::Str("\u{4e00}".into()),
                Inline::Str("\u{4e8c}".into()),
            ])]
        );
    }

    #[test]
    fn malformed_markup_is_scanned_without_panicking() {
        for input in [
            "<para>",
            "</para>",
            "<para attr=",
            "<!-- unterminated",
            "<![CDATA[unterminated",
            "<?pi",
            "<para>&#xZZ;&#999999999;</para>",
            "<a><b></a></b>",
        ] {
            let _ = read(input);
        }
    }

    #[test]
    fn nesting_beyond_the_ceiling_is_kept_but_not_deepened() {
        let input = "<x>".repeat(MAX_DEPTH * 2);
        let tree = scan(&input);
        assert!(depth(&tree) <= MAX_DEPTH + 1);
    }
}
