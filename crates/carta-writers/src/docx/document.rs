//! Rendering of the document body (blocks and inlines) into `word/document.xml`, along with the
//! footnote entries, list plan and embedded images the body gives rise to.

use self::comments::render_comment_entry;
use self::figures::{render_figure, render_footnote_entry};
use self::lists::{render_bullet_list, render_definition_list, render_ordered_list};
use self::runs::{
    RunProps, code_paragraph, horizontal_rule, line_block_paragraph, render_paragraph_runs,
    render_runs,
};
use self::tables::render_table;
use super::numbering::ListPlan;
use super::wml_root;
use carta_ast::{Attr, Block, Inline, MathType, MetaValue, Text};
use carta_core::container::xml::Element;
use carta_core::media::MediaBag;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

mod comments;
mod figures;
mod images;
mod lists;
mod runs;
mod tables;

/// The first free numeric identifier the body may hand out. The values `0` and `-1` are reserved for
/// the footnote separators, and `1` through `8` for the document part's fixed relationships, so every
/// footnote id, hyperlink relationship, image relationship and drawing id the body allocates begins
/// here and counts up. A bookmark takes one value of the sequence as well, when it closes.
const FIRST_FREE_ID: u32 = 9;

/// The nominal text width, in twips, a table's column fractions are measured against.
const TABLE_TEXT_WIDTH: f64 = 7920.0;

/// The indentation step, in twips, a table nested in a list item takes for each level of nesting.
const LIST_INDENT_STEP: u32 = 720;

/// One image the body embedded: its relationship id, the media file it is stored under, its MIME
/// type and its bytes.
pub(super) struct ImageMedia {
    pub(super) rel_id: u32,
    pub(super) file_name: String,
    pub(super) mime: String,
    pub(super) bytes: Vec<u8>,
}

/// The body render's products: the `word/document.xml` text, the list plan that drives the numbering
/// part, the footnote entries the notes part carries, the images the package must store, and the
/// external hyperlink destinations keyed by URL, each paired with the relationship id it was assigned.
pub(super) struct RenderedBody {
    pub(super) document_xml: String,
    pub(super) numbering: ListPlan,
    pub(super) footnotes: Vec<Element>,
    pub(super) comments: Vec<Element>,
    pub(super) images: Vec<ImageMedia>,
    pub(super) hyperlinks: BTreeMap<String, u32>,
}

/// A comment queued during the body walk: the identifier that ties its range markers to its entry,
/// the author metadata carried on the opening marker, and the inline text that becomes its body.
struct Comment {
    id: String,
    author: Option<String>,
    date: Option<String>,
    initials: Option<String>,
    body: Vec<Inline>,
    language: Option<Text>,
    right_to_left: Option<bool>,
}

/// The optional writer behaviors selected by the document's extension set.
#[derive(Clone, Copy, Default)]
pub(super) struct Features {
    /// Keep an empty paragraph in the output rather than dropping it.
    pub(super) keep_empty_paragraphs: bool,
    /// Number figures and tables with the target's own field mechanism and a caption label.
    pub(super) native_numbering: bool,
}

/// The syntax highlighter a body render draws on, or a zero-size placeholder when the feature is
/// compiled out. Threading it as one type keeps the body walk's signatures identical in both
/// configurations.
#[cfg(feature = "highlight")]
pub(super) type DocxHl = Option<Arc<carta_highlight::Highlighter>>;
#[cfg(not(feature = "highlight"))]
pub(super) type DocxHl = ();

/// State threaded through the body walk: the first/body paragraph alternation, the stack of open
/// heading bookmarks and the next free bookmark id, the list plan built up as lists appear, the
/// shared free-id counter, the queued footnote bodies, the embedded images, the media bag notes
/// and images resolve their bytes from, the selected optional behaviors, and the running
/// figure/table counts the native-numbering labels draw from.
struct Ctx {
    prev_paragraph: bool,
    bookmarks: Vec<Bookmark>,
    /// How many open heading bookmarks belong to enclosing containers and stay open inside this one.
    section_floor: usize,
    next_bookmark_id: u32,
    plan: ListPlan,
    /// The list item whose number the paragraphs being rendered bind to.
    item: Option<Item>,
    /// Whether the walk is inside a list item, however deep.
    in_list: bool,
    /// Whether the walk is inside a table's rows and no nested table or figure frame has closed since.
    in_table: bool,
    next_id: u32,
    notes: Vec<(u32, Vec<Block>, RunProps)>,
    run_properties: RunProps,
    comments: Vec<Comment>,
    next_insertion: u32,
    next_deletion: u32,
    images: Vec<ImageMedia>,
    image_indices: BTreeMap<Text, usize>,
    hyperlinks: BTreeMap<String, u32>,
    media: Arc<MediaBag>,
    features: Features,
    highlighter: DocxHl,
    figure_number: u32,
    table_number: u32,
}

impl Ctx {
    /// The relationship id an external hyperlink destination is reached through, allocating a fresh
    /// id the first time a destination is seen and reusing it for every later link to the same URL,
    /// so identical destinations share one relationship.
    fn hyperlink_rel(&mut self, url: &str) -> u32 {
        if let Some(&id) = self.hyperlinks.get(url) {
            return id;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.hyperlinks.insert(url.to_owned(), id);
        id
    }

    /// The list binding the next paragraph of the current list item takes: the item's own number on
    /// its first paragraph, the continuation number on every later one. `None` outside a list item.
    fn take_number(&mut self) -> Option<(u32, u32)> {
        let continuation = self.plan.continuation_num();
        let item = self.item.as_mut()?;
        let number = if item.lead_used {
            continuation
        } else {
            item.num_id
        };
        item.lead_used = true;
        Some((number, item.depth))
    }
}

/// A list item being rendered: the number its first paragraph binds to, its nesting depth, and
/// whether that first paragraph has been emitted.
#[derive(Clone, Copy)]
struct Item {
    num_id: u32,
    depth: u32,
    lead_used: bool,
}

/// An open heading bookmark: the level of the heading that opened it and its numeric id.
struct Bookmark {
    level: i64,
    id: u32,
}

/// What a block inherits from the containers around it.
#[derive(Clone, Copy)]
struct Inherited<'a> {
    /// The paragraph style of the nearest styling container: a block quote, a definition, a caption
    /// or a custom-style div.
    style: Option<&'a str>,
    /// The alignment of the enclosing table cell or figure frame.
    jc: Option<&'a str>,
    /// Whether a heading opens a bookmark spanning its section, as it does in body flow only.
    sections: bool,
}

impl<'a> Inherited<'a> {
    /// Body flow: no inherited style or alignment, headings open sections.
    const BODY: Inherited<'static> = Inherited {
        style: None,
        jc: None,
        sections: true,
    };

    /// The environment inside a container that imposes `style` on its paragraphs.
    fn styled(self, style: &'a str) -> Self {
        Inherited {
            style: Some(style),
            sections: false,
            ..self
        }
    }

    /// The environment inside a container that keeps the inherited style.
    fn nested(self) -> Self {
        Inherited {
            sections: false,
            ..self
        }
    }
}

/// Renders the complete `word/document.xml` part and the footnote, numbering and image products the
/// body gives rise to.
pub(super) fn document_xml(
    blocks: &[Block],
    meta: &BTreeMap<Text, MetaValue>,
    media: Arc<MediaBag>,
    features: Features,
    highlighter: DocxHl,
) -> RenderedBody {
    let mut body = Element::new("w:body");
    let mut ctx = Ctx {
        prev_paragraph: false,
        bookmarks: Vec::new(),
        section_floor: 0,
        // Counts from 1, clear of the free-id range footnotes and images draw from.
        next_bookmark_id: 1,
        plan: ListPlan::default(),
        item: None,
        in_list: false,
        in_table: false,
        next_id: FIRST_FREE_ID,
        notes: Vec::new(),
        run_properties: RunProps::default(),
        comments: Vec::new(),
        next_insertion: 1,
        next_deletion: 1,
        images: Vec::new(),
        image_indices: BTreeMap::new(),
        hyperlinks: BTreeMap::new(),
        media,
        features,
        highlighter,
        figure_number: 0,
        table_number: 0,
    };
    title_block(meta, &mut body, &mut ctx);
    render_blocks(blocks, &mut body, &mut ctx, Inherited::BODY);
    close_bookmarks(&mut body, &mut ctx, i64::MIN);
    body.push(section_properties());

    // Rendered before footnotes so a footnote inside a comment joins the queue below.
    let queued_comments = std::mem::take(&mut ctx.comments);
    let mut comments = Vec::with_capacity(queued_comments.len());
    for comment in &queued_comments {
        comments.push(render_comment_entry(comment, &mut ctx));
    }

    // A note nested in a note appends mid-loop, so the growing queue is revisited until drained.
    let mut footnotes = Vec::new();
    let mut index = 0;
    while let Some((id, note_blocks, properties)) = ctx.notes.get(index).cloned() {
        let previous = std::mem::replace(&mut ctx.run_properties, properties);
        let entry = render_footnote_entry(id, &note_blocks, &mut ctx);
        ctx.run_properties = previous;
        footnotes.push(entry);
        index += 1;
    }

    let document_xml = wml_root("w:document").child(body).render_document();
    RenderedBody {
        document_xml,
        numbering: ctx.plan,
        footnotes,
        comments,
        images: ctx.images,
        hyperlinks: ctx.hyperlinks,
    }
}

/// Renders the document's title block (its title, subtitle, authors, date and abstract) as the
/// styled paragraphs that open the body, in that order. Only the fields the document carries appear.
/// These paragraphs are laid down before the body's own blocks and leave the first/body alternation
/// untouched, so the first body paragraph still opens with the `FirstParagraph` style.
fn title_block(meta: &BTreeMap<Text, MetaValue>, body: &mut Element, ctx: &mut Ctx) {
    if let Some(value) = meta.get("title") {
        body.push(paragraph("Title", &meta_inlines(value), ctx));
    }
    if let Some(value) = meta.get("subtitle") {
        body.push(paragraph("Subtitle", &meta_inlines(value), ctx));
    }
    if let Some(value) = meta.get("author") {
        for author in author_list(value) {
            body.push(paragraph("Author", &author, ctx));
        }
    }
    if let Some(value) = meta.get("date") {
        body.push(paragraph("Date", &meta_inlines(value), ctx));
    }
    if let Some(value) = meta.get("abstract") {
        body.push(paragraph(
            "AbstractTitle",
            &[Inline::Str(Text::from("Abstract"))],
            ctx,
        ));
        match value {
            MetaValue::MetaBlocks(blocks) => {
                for block in blocks {
                    if let Block::Para(inlines) | Block::Plain(inlines) = block {
                        body.push(paragraph("Abstract", inlines, ctx));
                    }
                }
            }
            other => body.push(paragraph("Abstract", &meta_inlines(other), ctx)),
        }
    }
}

/// Flattens a metadata value to an inline sequence for a title-block paragraph. A list joins its
/// items with a comma-and-space separator; a map, which carries no directly renderable text, yields
/// nothing.
fn meta_inlines(value: &MetaValue) -> Vec<Inline> {
    match value {
        MetaValue::MetaInlines(inlines) => inlines.clone(),
        MetaValue::MetaString(text) => vec![Inline::Str(text.clone())],
        MetaValue::MetaBlocks(blocks) => carta_ast::single_block_inlines(blocks).to_vec(),
        MetaValue::MetaBool(flag) => vec![Inline::Str(Text::from(flag.to_string()))],
        MetaValue::MetaList(items) => {
            let mut out = Vec::new();
            for item in items {
                if !out.is_empty() {
                    out.push(Inline::Str(Text::from(", ")));
                }
                out.extend(meta_inlines(item));
            }
            out
        }
        MetaValue::MetaMap(_) => Vec::new(),
    }
}

/// The authors a metadata value names, each as its own inline sequence: a list yields one entry per
/// item, and any other value a single entry.
fn author_list(value: &MetaValue) -> Vec<Vec<Inline>> {
    match value {
        MetaValue::MetaList(items) => items.iter().map(meta_inlines).collect(),
        other => vec![meta_inlines(other)],
    }
}

/// The trailing section properties: a single section whose footnotes restart their numbering.
fn section_properties() -> Element {
    Element::new("w:sectPr").child(
        Element::new("w:footnotePr").child(Element::new("w:numRestart").attr("w:val", "eachSect")),
    )
}

/// Renders a sequence of sibling blocks, keeping adjacent tables apart.
fn render_blocks(blocks: &[Block], out: &mut Element, ctx: &mut Ctx, inherited: Inherited) {
    let mut previous = None;
    for block in blocks {
        separate_adjacent_tables(previous, block, out);
        render_block(block, out, ctx, inherited);
        previous = Some(block);
    }
}

/// Renders one block under the style, alignment and list binding it inherits, and advances the
/// first/body paragraph alternation: body text continues after a paragraph and starts afresh after
/// every other construct. A table or figure starts it afresh for its own content.
fn render_block(block: &Block, out: &mut Element, ctx: &mut Ctx, inherited: Inherited) {
    match block {
        Block::Para(inlines) => render_paragraph(inlines, false, out, ctx, inherited),
        Block::Plain(inlines) => render_paragraph(inlines, true, out, ctx, inherited),
        Block::LineBlock(lines) => {
            let style = flow_style(inherited, false, ctx.item.is_some(), ctx);
            let numbering = ctx.take_number();
            let properties = paragraph_props(
                style,
                numbering,
                inherited.jc,
                ctx.run_properties.right_to_left,
            );
            out.push(line_block_paragraph(properties, lines, ctx));
            ctx.prev_paragraph = !lines.iter().flatten().any(is_display_equation);
        }
        // A section opened inside a div ends with it; sections of enclosing containers stay open.
        Block::Div(attr, blocks) => {
            let previous_properties = ctx.run_properties.clone();
            ctx.run_properties = ctx.run_properties.with_attributes(attr);
            let mark = open_bookmark(attr.id.as_str(), out, ctx);
            let floor = std::mem::replace(&mut ctx.section_floor, ctx.bookmarks.len());
            let style = custom_style(attr)
                .or_else(|| bibliography_style(attr))
                .or(inherited.style);
            render_blocks(blocks, out, ctx, Inherited { style, ..inherited });
            if inherited.sections {
                close_bookmarks(out, ctx, i64::MIN);
            }
            ctx.section_floor = floor;
            close_bookmark(mark, out, ctx);
            ctx.run_properties = previous_properties;
        }
        Block::Header(level, attr, inlines) => {
            if inherited.sections {
                close_bookmarks(out, ctx, *level);
                open_heading(*level, attr.id.as_str(), inlines, out, ctx);
            } else {
                let numbering = ctx.take_number();
                let style = Some(heading_style(*level));
                let para = styled_paragraph(style, numbering, inherited.jc, inlines, ctx);
                push_anchored(out, ctx, attr.id.as_str(), para);
            }
            ctx.prev_paragraph = false;
        }
        Block::CodeBlock(attr, code) => {
            let numbering = ctx.take_number();
            let para = code_paragraph(
                attr,
                code,
                numbering,
                inherited.jc,
                &ctx.highlighter,
                &ctx.run_properties,
            );
            push_anchored(out, ctx, attr.id.as_str(), para);
            ctx.prev_paragraph = false;
        }
        // Raw markup neither opens nor closes the first/body alternation; only openxml passes through.
        Block::RawBlock(format, payload) => {
            if format.0.as_str() == "openxml" {
                out.push_raw(payload);
            }
        }
        Block::BlockQuote(blocks) => {
            render_blocks(blocks, out, ctx, inherited.styled("BlockText"));
            ctx.prev_paragraph = false;
        }
        Block::BulletList(items) => {
            render_bullet_list(items, out, ctx, inherited.nested());
            ctx.prev_paragraph = false;
        }
        Block::OrderedList(attrs, items) => {
            render_ordered_list(attrs, items, out, ctx, inherited.nested());
            ctx.prev_paragraph = false;
        }
        Block::DefinitionList(items) => {
            render_definition_list(items, out, ctx, inherited);
            ctx.prev_paragraph = false;
        }
        Block::Table(table) => render_table(table, out, ctx, inherited),
        Block::Figure(attr, caption, blocks) => {
            render_figure(attr.id.as_str(), caption, blocks, out, ctx, inherited);
        }
        Block::HorizontalRule => {
            out.push(horizontal_rule(ctx.run_properties.right_to_left));
            ctx.prev_paragraph = false;
        }
    }
}

/// Word coalesces two tables with nothing between them into a single table, so an empty separator
/// paragraph is placed between adjacent tables to keep them distinct. Emits it only when `previous` and
/// `current` are both tables.
fn separate_adjacent_tables(previous: Option<&Block>, current: &Block, out: &mut Element) {
    if matches!(previous, Some(Block::Table(_))) && matches!(current, Block::Table(_)) {
        out.push(Element::new("w:p"));
    }
}

/// Whether an inline is a display equation, which is set on its own line rather than among text.
fn is_display_equation(inline: &Inline) -> bool {
    matches!(inline, Inline::Math(MathType::DisplayMath, source) if crate::math::to_omml(source, true).is_some())
}

/// Whether an inline carries no visible text, so a run of them makes no paragraph of its own.
fn is_blank_inline(inline: &Inline) -> bool {
    matches!(inline, Inline::Space | Inline::SoftBreak)
}

/// The inline slice with its leading and trailing blank inlines (spaces and soft breaks) removed;
/// empty when every inline is blank.
fn trim_blank_inlines(inlines: &[Inline]) -> &[Inline] {
    let start = inlines.iter().take_while(|i| is_blank_inline(i)).count();
    let end = inlines.len()
        - inlines
            .iter()
            .rev()
            .take_while(|i| is_blank_inline(i))
            .count();
    inlines.get(start..end).unwrap_or(&[])
}

/// The style of a flow paragraph. A tight paragraph inside a list item or a table is compact.
/// Otherwise the enclosing container's style applies; failing that, a paragraph bound to a list
/// number or aligned by its cell carries no style, and body flow alternates between the first and
/// the continuing body style.
fn flow_style<'a>(
    inherited: Inherited<'a>,
    tight: bool,
    numbered: bool,
    ctx: &Ctx,
) -> Option<&'a str> {
    if tight && (ctx.in_list || ctx.in_table) {
        Some("Compact")
    } else if inherited.style.is_some() {
        inherited.style
    } else if numbered || inherited.jc.is_some() {
        None
    } else {
        Some(body_style(ctx.prev_paragraph))
    }
}

/// Renders a `Para` (loose) or `Plain` (tight) block. A display equation is lifted onto a paragraph
/// of its own, outside any list numbering, and the text on either side becomes its own paragraph;
/// body text starts afresh after an equation.
fn render_paragraph(
    inlines: &[Inline],
    tight: bool,
    out: &mut Element,
    ctx: &mut Ctx,
    inherited: Inherited,
) {
    if !inlines.iter().any(is_display_equation) {
        if !inlines.is_empty() || ctx.features.keep_empty_paragraphs {
            push_text_paragraph(inlines, tight, out, ctx, inherited);
        }
        return;
    }
    let mut segment: Vec<Inline> = Vec::new();
    for inline in inlines {
        let fragment = match inline {
            Inline::Math(MathType::DisplayMath, source) => crate::math::to_omml(source, true),
            _ => None,
        };
        let Some(fragment) = fragment else {
            segment.push(inline.clone());
            continue;
        };
        flush_text_segment(&mut segment, tight, out, ctx, inherited);
        let style = flow_style(inherited, tight, false, ctx);
        let mut para = Element::new("w:p");
        para.push(paragraph_props(
            style,
            None,
            inherited.jc,
            ctx.run_properties.right_to_left,
        ));
        para.push_raw(&fragment);
        out.push(para);
        ctx.prev_paragraph = false;
    }
    flush_text_segment(&mut segment, tight, out, ctx, inherited);
}

/// Emits the text collected between display equations as one paragraph, unless it carries no
/// visible text. The whitespace that had separated the text from an adjacent equation is dropped.
/// Clears the buffer either way.
fn flush_text_segment(
    segment: &mut Vec<Inline>,
    tight: bool,
    out: &mut Element,
    ctx: &mut Ctx,
    inherited: Inherited,
) {
    let trimmed = trim_blank_inlines(segment);
    if !trimmed.is_empty() {
        push_text_paragraph(trimmed, tight, out, ctx, inherited);
    }
    segment.clear();
}

/// Emits one text paragraph under its flow style, binding it to the current list item.
fn push_text_paragraph(
    inlines: &[Inline],
    tight: bool,
    out: &mut Element,
    ctx: &mut Ctx,
    inherited: Inherited,
) {
    let style = flow_style(inherited, tight, ctx.item.is_some(), ctx);
    let numbering = ctx.take_number();
    out.push(styled_paragraph(
        style,
        numbering,
        inherited.jc,
        inlines,
        ctx,
    ));
    ctx.prev_paragraph = true;
}

/// The paragraph style for body flow: `FirstParagraph` opens the body and each fresh section,
/// `BodyText` continues one.
fn body_style(prev_paragraph: bool) -> &'static str {
    if prev_paragraph {
        "BodyText"
    } else {
        "FirstParagraph"
    }
}

/// Emits a heading paragraph, opening a section-spanning bookmark when it carries an identifier.
fn open_heading(level: i64, id: &str, inlines: &[Inline], body: &mut Element, ctx: &mut Ctx) {
    let style = heading_style(level);
    if !id.is_empty() {
        let mark = ctx.next_bookmark_id;
        ctx.next_bookmark_id = ctx.next_bookmark_id.wrapping_add(1);
        body.push(
            Element::new("w:bookmarkStart")
                .attr("w:id", &mark.to_string())
                .attr("w:name", clean_bookmark_name(id).as_ref()),
        );
        ctx.bookmarks.push(Bookmark { level, id: mark });
    }
    let mut para = Element::new("w:p");
    para.push(paragraph_props(
        Some(style),
        None,
        None,
        ctx.run_properties.right_to_left,
    ));
    let properties = ctx.run_properties.clone();
    render_runs(inlines, &properties, ctx, &mut para);
    body.push(para);
}

/// Pushes a block, wrapping it in a bookmark that spans just that block when it carries an
/// identifier.
fn push_anchored(body: &mut Element, ctx: &mut Ctx, id: &str, element: Element) {
    let mark = open_bookmark(id, body, ctx);
    body.push(element);
    close_bookmark(mark, body, ctx);
}

/// A bookmark name Word accepts. An identifier that begins with a letter and is at most forty
/// characters long is used unchanged; any other is replaced by a fixed-length name computed by
/// hashing its bytes, so the name still begins with a letter and stays within the length limit while
/// staying stable for the links that target the same identifier.
fn clean_bookmark_name(id: &str) -> Cow<'_, str> {
    let acceptable = id.chars().next().is_some_and(char::is_alphabetic) && id.chars().count() <= 40;
    if acceptable {
        return Cow::Borrowed(id);
    }
    let hex = carta_core::media::sha1_hex(id.as_bytes());
    Cow::Owned(format!("X{}", hex.get(1..).unwrap_or(hex.as_str())))
}

/// Opens a bookmark spanning content that carries an identifier and returns the mark to close it with;
/// an empty identifier opens nothing.
fn open_bookmark(id: &str, out: &mut Element, ctx: &mut Ctx) -> Option<u32> {
    if id.is_empty() {
        return None;
    }
    let mark = ctx.next_bookmark_id;
    ctx.next_bookmark_id = ctx.next_bookmark_id.wrapping_add(1);
    out.push(
        Element::new("w:bookmarkStart")
            .attr("w:id", &mark.to_string())
            .attr("w:name", clean_bookmark_name(id).as_ref()),
    );
    Some(mark)
}

/// Closes a bookmark, which takes its value of the shared identifier sequence at this point.
fn close_bookmark(mark: Option<u32>, out: &mut Element, ctx: &mut Ctx) {
    if let Some(mark) = mark {
        out.push(Element::new("w:bookmarkEnd").attr("w:id", &mark.to_string()));
        ctx.next_id = ctx.next_id.saturating_add(1);
    }
}

/// Closes every heading bookmark of the current container at or deeper than `level`, most-recent
/// first, so nested sections close from the inside out. `i64::MIN` closes all of them.
fn close_bookmarks(body: &mut Element, ctx: &mut Ctx, level: i64) {
    while ctx.bookmarks.len() > ctx.section_floor {
        let Some(bookmark) = ctx.bookmarks.last() else {
            break;
        };
        if bookmark.level < level {
            break;
        }
        let id = bookmark.id;
        ctx.bookmarks.pop();
        close_bookmark(Some(id), body, ctx);
    }
}

/// Clamps a heading level to one of the nine defined heading styles.
fn heading_style(level: i64) -> &'static str {
    match level.clamp(1, 9) {
        1 => "Heading1",
        2 => "Heading2",
        3 => "Heading3",
        4 => "Heading4",
        5 => "Heading5",
        6 => "Heading6",
        7 => "Heading7",
        8 => "Heading8",
        _ => "Heading9",
    }
}

/// A Word style id: the style name with its whitespace removed, since a style id admits no spaces
/// while its display name may (a `Intense Quote` style is referred to by the id `IntenseQuote`). A
/// name that is already whitespace-free is borrowed unchanged.
fn style_id(name: &str) -> Cow<'_, str> {
    if name.chars().any(char::is_whitespace) {
        Cow::Owned(name.chars().filter(|c| !c.is_whitespace()).collect())
    } else {
        Cow::Borrowed(name)
    }
}

/// The paragraph style a bibliography container imposes on its entries. A generated citation
/// bibliography is a div identified as `refs`; its entry paragraphs take the `Bibliography` style.
fn bibliography_style(attr: &Attr) -> Option<&'static str> {
    (attr.id.as_str() == "refs").then_some("Bibliography")
}

/// The `custom-style` attribute value carried by a div or span, when present and non-empty.
fn custom_style(attr: &Attr) -> Option<&str> {
    attr.attributes
        .iter()
        .find(|(key, _)| key.as_str() == "custom-style")
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

fn has_class(attr: &Attr, class: &str) -> bool {
    attr.classes.iter().any(|name| name.as_str() == class)
}

/// The value a key carries in an attribute set, when present and not empty.
fn attr_value<'a>(attr: &'a Attr, key: &str) -> Option<&'a str> {
    attr.attributes
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

/// Builds a paragraph with an optional style, list binding and alignment, then its runs.
fn styled_paragraph(
    style: Option<&str>,
    numbering: Option<(u32, u32)>,
    jc: Option<&str>,
    inlines: &[Inline],
    ctx: &mut Ctx,
) -> Element {
    let mut para = Element::new("w:p");
    para.push(paragraph_props(
        style,
        numbering,
        jc,
        ctx.run_properties.right_to_left,
    ));
    let properties = ctx.run_properties.clone();
    render_paragraph_runs(inlines, &properties, ctx, &mut para);
    para
}

/// The paragraph properties element for a style, list binding and alignment. Emitted even when empty
/// so a paragraph always carries a `w:pPr`.
fn paragraph_props(
    style: Option<&str>,
    numbering: Option<(u32, u32)>,
    jc: Option<&str>,
    right_to_left: Option<bool>,
) -> Element {
    let mut properties = Element::new("w:pPr");
    if let Some(style) = style {
        properties.push(Element::new("w:pStyle").attr("w:val", style_id(style).as_ref()));
    }
    if let Some((num_id, level)) = numbering {
        properties.push(
            Element::new("w:numPr")
                .child(Element::new("w:ilvl").attr("w:val", &level.to_string()))
                .child(Element::new("w:numId").attr("w:val", &num_id.to_string())),
        );
    }
    if let Some(right_to_left) = right_to_left {
        let mut property = Element::new("w:bidi");
        if !right_to_left {
            property = property.attr("w:val", "0");
        }
        properties.push(property);
    }
    if let Some(jc) = jc {
        properties.push(Element::new("w:jc").attr("w:val", jc));
    }
    properties
}

fn paragraph(style: &str, inlines: &[Inline], ctx: &mut Ctx) -> Element {
    styled_paragraph(Some(style), None, None, inlines, ctx)
}

/// The concatenated plain text of a sequence of blocks, used for a table's caption description.
fn blocks_plain_text(blocks: &[Block]) -> String {
    let mut text = String::new();
    for block in blocks {
        match block {
            Block::Para(inlines) | Block::Plain(inlines) | Block::Header(_, _, inlines) => {
                text.push_str(&carta_ast::to_plain_text(inlines));
            }
            Block::LineBlock(lines) => {
                for line in lines {
                    text.push_str(&carta_ast::to_plain_text(line));
                }
            }
            Block::BlockQuote(blocks) | Block::Div(_, blocks) | Block::Figure(_, _, blocks) => {
                text.push_str(&blocks_plain_text(blocks));
            }
            _ => {}
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::{Alignment, Cell, ColSpec, ColWidth, Row, Table, TableBody};

    fn body(blocks: &[Block]) -> String {
        let media = Arc::new(MediaBag::new());
        let features = Features::default();
        document_xml(blocks, &BTreeMap::new(), media, features, DocxHl::default()).document_xml
    }

    /// The style and list number of every paragraph that has content, in document order.
    fn outline(blocks: &[Block]) -> Vec<String> {
        let xml = body(blocks);
        let value = |paragraph: &str, tag: &str| {
            let (_, rest) = paragraph.split_once(&format!("<{tag} w:val=\""))?;
            Some(rest.split_once('"')?.0.to_owned())
        };
        xml.split("<w:p>")
            .skip(1)
            .map(|paragraph| {
                let style = value(paragraph, "w:pStyle").unwrap_or_else(|| "-".to_owned());
                let number = value(paragraph, "w:numId").unwrap_or_else(|| "-".to_owned());
                format!("{style} {number}")
            })
            .collect()
    }

    fn text(content: &str) -> Vec<Inline> {
        vec![Inline::Str(content.into())]
    }

    fn para(content: &str) -> Block {
        Block::Para(text(content))
    }

    fn plain(content: &str) -> Block {
        Block::Plain(text(content))
    }

    /// A one-column table holding one cell.
    fn table(content: Vec<Block>) -> Block {
        let cell = Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content,
        };
        Block::Table(Box::new(Table {
            col_specs: vec![ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            }],
            bodies: vec![TableBody {
                body: vec![Row {
                    cells: vec![cell],
                    ..Row::default()
                }],
                ..TableBody::default()
            }],
            ..Table::default()
        }))
    }

    #[test]
    fn every_paragraph_of_a_list_item_binds_to_the_list() {
        let definitions =
            Block::DefinitionList(vec![(text("term"), vec![vec![para("definition")]])]);
        let list = Block::BulletList(vec![vec![para("lead"), definitions]]);
        assert_eq!(
            outline(&[list]),
            ["- 1001", "DefinitionTerm 1000", "Definition 1000"]
        );
    }

    #[test]
    fn cell_paragraphs_take_the_container_style_or_the_body_alternation() {
        let heading = Block::Header(2, Box::default(), text("heading"));
        let cell = vec![para("first"), heading, para("fresh"), para("next")];
        assert_eq!(
            outline(&[table(cell)]),
            [
                "FirstParagraph -",
                "Heading2 -",
                "FirstParagraph -",
                "BodyText -"
            ]
        );
        let quoted = Block::BlockQuote(vec![table(vec![para("cell")])]);
        assert_eq!(outline(&[quoted]), ["BlockText -"]);
    }

    #[test]
    fn tight_cell_paragraphs_are_compact_until_a_nested_table_closes() {
        let nested = table(vec![plain("inner")]);
        let outer = table(vec![plain("before"), nested, plain("after")]);
        assert_eq!(outline(&[outer]), ["Compact -", "Compact -", "BodyText -"]);
    }

    #[test]
    fn a_closed_bookmark_takes_the_next_shared_identifier() {
        let attr = Attr {
            id: "here".into(),
            ..Attr::default()
        };
        let anchor = Inline::Span(Box::new(attr), Vec::new());
        let note = Inline::Note(vec![para("note")]);
        let xml = body(&[Block::Para(vec![anchor, note])]);
        assert!(xml.contains("<w:footnoteReference w:id=\"10\" />"));
    }

    #[test]
    fn nested_toggles_repeat_and_breaks_carry_no_formatting() {
        let inner = Inline::Emph(vec![Inline::Str("text".into()), Inline::LineBreak]);
        let xml = body(&[Block::Para(vec![Inline::Emph(vec![inner])])]);
        assert!(xml.contains("<w:rPr><w:i /><w:i /><w:iCs /><w:iCs /></w:rPr>"));
        assert!(xml.contains("<w:r><w:br /></w:r>"));
    }
}
