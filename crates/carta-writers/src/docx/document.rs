//! Rendering of the document body — blocks and inlines — into `word/document.xml`, along with the
//! footnote entries, list plan and embedded images the body gives rise to.
//!
//! A top-level paragraph opens the body with the `FirstParagraph` style and switches to `BodyText`
//! once another paragraph precedes it; a heading, list, table, figure, block quote or other
//! structural block resets that run so the next paragraph opens fresh again, while a raw-passthrough
//! block is transparent to it. A heading carrying an identifier opens a bookmark spanning its whole
//! outline section: the `bookmarkEnd` is deferred until a heading of the same or a shallower level
//! begins, or the body ends, and nested sections close from the inside out. A code block carrying an
//! identifier is wrapped in a bookmark too, but one that closes immediately after the block.
//!
//! A bulleted or ordered list binds each item's lead paragraph to a concrete list number and its
//! continuation paragraphs to the scaffold number, deepening the indent level for nested lists. A
//! table lays out a grid: columns take their width from the column fractions, header rows are marked
//! as repeating, and a cell that spans rows or columns leaves merge-continuation cells behind it. A
//! footnote reference drops a numbered mark into the runs and queues its block content for the
//! footnotes part. An image resolves to a drawing sized from the picture's pixel dimensions when its
//! bytes are on hand, and degrades to its descriptive text otherwise.
//!
//! Inline content becomes a sequence of runs: consecutive text joined by single spaces collapses
//! into one run, a space at a formatting boundary and every soft break stand as their own run, and
//! each formatted span (emphasis, strong, code, …) contributes its own run carrying the accumulated
//! run properties. Math lowers to an Office Math fragment placed directly in the paragraph, or to a
//! delimited literal when its source cannot be rendered; `openxml` raw passthrough is emitted
//! verbatim and raw passthrough in any other format contributes nothing.

use super::numbering::ListPlan;
use super::wml_root;
use crate::image_size::{image_dimensions, image_dpi};
use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Inline, ListAttributes, MathType,
    MetaValue, QuoteType, Row, Table, Target, Text,
};
use carta_core::container::xml::Element;
use carta_core::media::{MediaBag, extension_for_mime};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The first free numeric identifier the body may hand out. The values `0` and `-1` are reserved for
/// the footnote separators, and `1` through `8` for the document part's fixed relationships, so every
/// footnote id, hyperlink relationship, image relationship and drawing id the body allocates begins
/// here and counts up.
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
    next_bookmark_id: u32,
    plan: ListPlan,
    next_id: u32,
    notes: Vec<(u32, Vec<Block>)>,
    comments: Vec<Comment>,
    next_insertion: u32,
    next_deletion: u32,
    images: Vec<ImageMedia>,
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
}

/// An open heading bookmark: the level of the heading that opened it and its numeric id.
struct Bookmark {
    level: i32,
    id: u32,
}

/// The paragraph styles a nested container renders its `Para` and `Plain` blocks under. The styles
/// are usually fixed names, but a custom-style div supplies borrowed ones, hence the lifetime.
/// `list_ambient` is the style a nested list's loose item paragraphs inherit from the container — a
/// block quote and a definition pass their own style down to the lists they hold, whereas a table
/// cell imposes none — mirroring the `ambient` a custom-style div threads through the top level.
#[derive(Clone, Copy)]
struct FlowStyle<'a> {
    para: &'a str,
    plain: &'a str,
    list_ambient: Option<&'a str>,
}

impl FlowStyle<'_> {
    /// The style a block quote's content takes.
    fn blockquote() -> FlowStyle<'static> {
        FlowStyle {
            para: "BlockText",
            plain: "BlockText",
            list_ambient: Some("BlockText"),
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
        // Bookmark ids count up from 1, staying clear of the free-id range the footnotes and images
        // draw from so every emitted id is distinct.
        next_bookmark_id: 1,
        plan: ListPlan::default(),
        next_id: FIRST_FREE_ID,
        notes: Vec::new(),
        comments: Vec::new(),
        // Insertions and deletions carry independent change ids, each counting up from one.
        next_insertion: 1,
        next_deletion: 1,
        images: Vec::new(),
        hyperlinks: BTreeMap::new(),
        media,
        features,
        highlighter,
        figure_number: 0,
        table_number: 0,
    };
    title_block(meta, &mut body, &mut ctx);
    let mut previous = None;
    for block in blocks {
        separate_adjacent_tables(previous, block, &mut body);
        render_top_block(block, &mut body, &mut ctx, None);
        previous = Some(block);
    }
    close_bookmarks(&mut body, &mut ctx.bookmarks, i32::MIN);
    body.push(section_properties());

    // Comment bodies are rendered after the main walk, in the order their ranges opened. Rendering
    // them first lets any footnote a comment carries join the footnote queue below.
    let queued_comments = std::mem::take(&mut ctx.comments);
    let mut comments = Vec::with_capacity(queued_comments.len());
    for comment in &queued_comments {
        comments.push(render_comment_entry(comment, &mut ctx));
    }

    // Footnote bodies are rendered after the main walk, in the order their references appeared. A
    // note nested inside another note is appended as it is discovered, so the loop revisits the
    // growing queue until it is drained.
    let mut footnotes = Vec::new();
    let mut index = 0;
    while let Some((id, note_blocks)) = ctx.notes.get(index).cloned() {
        let entry = render_footnote_entry(id, &note_blocks, &mut ctx);
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

/// Renders the document's title block — its title, subtitle, authors, date and abstract — as the
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

/// Renders one top-level block, applying the first/body paragraph alternation and heading anchors.
/// `ambient` carries the paragraph style a surrounding custom-style div imposes on its direct
/// paragraphs; it is `None` in ordinary body flow.
fn render_top_block(block: &Block, body: &mut Element, ctx: &mut Ctx, ambient: Option<&str>) {
    match block {
        Block::Para(inlines) | Block::Plain(inlines) => {
            // A display equation set among text is lifted onto its own centred line, so a paragraph
            // that carries one is split around it rather than emitted whole.
            if ambient.is_none() && inlines.iter().any(is_display_equation) {
                render_split_paragraph(inlines, body, ctx);
                return;
            }
            // An empty paragraph carries nothing and is dropped rather than emitted as a blank line,
            // unless empty paragraphs are being preserved.
            if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                return;
            }
            let style = ambient.unwrap_or_else(|| body_style(ctx.prev_paragraph));
            body.push(paragraph(style, inlines, ctx));
            ctx.prev_paragraph = true;
        }
        Block::LineBlock(lines) => {
            let style = ambient.unwrap_or_else(|| body_style(ctx.prev_paragraph));
            body.push(line_block_paragraph(style, lines, ctx));
            ctx.prev_paragraph = true;
        }
        // A div is transparent to the first/body alternation: its blocks join the body as though
        // written directly. A `custom-style` div additionally imposes its named paragraph style on
        // its direct paragraphs, which a nested custom-style div overrides in turn.
        Block::Div(attr, blocks) => {
            let mark = open_bookmark(attr.id.as_str(), body, ctx);
            let child_ambient = custom_style(attr)
                .or_else(|| bibliography_style(attr))
                .or(ambient);
            let mut previous = None;
            for inner in blocks {
                separate_adjacent_tables(previous, inner, body);
                render_top_block(inner, body, ctx, child_ambient);
                previous = Some(inner);
            }
            close_bookmark(mark, body);
        }
        Block::Header(level, attr, inlines) => {
            close_bookmarks(body, &mut ctx.bookmarks, *level);
            open_heading(*level, attr.id.as_str(), inlines, body, ctx);
            ctx.prev_paragraph = false;
        }
        // A code block anchors a bookmark around just itself when it carries an identifier.
        Block::CodeBlock(attr, code) => {
            let id = attr.id.clone();
            let para = code_paragraph(attr, code, None, &ctx.highlighter);
            push_anchored(body, ctx, id.as_str(), para);
            ctx.prev_paragraph = false;
        }
        // Raw passthrough carries markup, not paragraph flow, so it neither opens nor closes the
        // first/body alternation: an `openxml` payload is emitted verbatim, any other format drops.
        Block::RawBlock(format, payload) => {
            if format.0.as_str() == "openxml" {
                body.push_raw(payload);
            }
        }
        Block::BlockQuote(blocks) => {
            let mut previous = None;
            for inner in blocks {
                separate_adjacent_tables(previous, inner, body);
                render_flow(inner, body, ctx, FlowStyle::blockquote());
                previous = Some(inner);
            }
            ctx.prev_paragraph = false;
        }
        Block::BulletList(items) => {
            render_bullet_list(items, body, ctx, 0, ambient);
            ctx.prev_paragraph = false;
        }
        Block::OrderedList(attrs, items) => {
            render_ordered_list(attrs, items, body, ctx, 0, ambient);
            ctx.prev_paragraph = false;
        }
        Block::DefinitionList(items) => {
            render_definition_list(items, body, ctx);
            ctx.prev_paragraph = false;
        }
        Block::Table(table) => {
            render_table(table, body, ctx, None);
            ctx.prev_paragraph = false;
        }
        Block::Figure(attr, caption, blocks) => {
            render_figure(attr.id.as_str(), caption, blocks, body, ctx);
            ctx.prev_paragraph = false;
        }
        Block::HorizontalRule => {
            body.push(horizontal_rule());
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

/// Renders a body paragraph that carries a display equation, splitting it around each one: a display
/// equation becomes its own centred, block-level paragraph and the text on either side becomes its own
/// paragraph, so the equation never sits inline among text. A display equation resets the first/body
/// alternation the way any other block-level construct does, so the text after it opens fresh.
fn render_split_paragraph(inlines: &[Inline], body: &mut Element, ctx: &mut Ctx) {
    let mut segment: Vec<Inline> = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Math(MathType::DisplayMath, source) => {
                match crate::math::to_omml(source, true) {
                    Some(fragment) => {
                        flush_text_segment(&mut segment, body, ctx);
                        let style = body_style(ctx.prev_paragraph);
                        let mut para = Element::new("w:p");
                        para.push(paragraph_props(Some(style), None, None));
                        para.push_raw(&fragment);
                        body.push(para);
                        ctx.prev_paragraph = false;
                    }
                    None => segment.push(inline.clone()),
                }
            }
            other => segment.push(other.clone()),
        }
    }
    flush_text_segment(&mut segment, body, ctx);
}

/// Emits the text collected between display equations as one body paragraph, unless it carries no
/// visible text. The whitespace that had separated the text from an adjacent equation is dropped, so
/// the text paragraph carries no leading or trailing space. Clears the buffer either way.
fn flush_text_segment(segment: &mut Vec<Inline>, body: &mut Element, ctx: &mut Ctx) {
    let trimmed = trim_blank_inlines(segment);
    if !trimmed.is_empty() {
        let style = body_style(ctx.prev_paragraph);
        body.push(paragraph(style, trimmed, ctx));
        ctx.prev_paragraph = true;
    }
    segment.clear();
}

/// Emits a nested container's paragraph under one style, lifting each display equation onto its own
/// centred paragraph in that style so a display equation never sits inline among text; the text on
/// either side of an equation becomes its own paragraph. Returns whether any paragraph was emitted.
fn styled_flow_with_display(
    style: &str,
    jc: Option<&str>,
    inlines: &[Inline],
    ctx: &mut Ctx,
    out: &mut Element,
) -> bool {
    let mut emitted = false;
    let mut segment: Vec<Inline> = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Math(MathType::DisplayMath, source) => match crate::math::to_omml(source, true)
            {
                Some(fragment) => {
                    flush_styled_segment(style, jc, &mut segment, ctx, out);
                    let mut para = Element::new("w:p");
                    para.push(paragraph_props(Some(style), None, jc));
                    para.push_raw(&fragment);
                    out.push(para);
                    emitted = true;
                }
                None => segment.push(inline.clone()),
            },
            other => segment.push(other.clone()),
        }
    }
    emitted | flush_styled_segment(style, jc, &mut segment, ctx, out)
}

/// Emits the text collected between display equations as one styled paragraph, unless it carries no
/// visible text; the whitespace that had abutted an equation is trimmed away. Clears the buffer and
/// returns whether a paragraph was emitted.
fn flush_styled_segment(
    style: &str,
    jc: Option<&str>,
    segment: &mut Vec<Inline>,
    ctx: &mut Ctx,
    out: &mut Element,
) -> bool {
    let trimmed = trim_blank_inlines(segment);
    let emitted = if trimmed.is_empty() {
        false
    } else {
        out.push(styled_paragraph(Some(style), None, jc, trimmed, ctx));
        true
    };
    segment.clear();
    emitted
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
fn open_heading(level: i32, id: &str, inlines: &[Inline], body: &mut Element, ctx: &mut Ctx) {
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
    body.push(paragraph(style, inlines, ctx));
}

/// Pushes a block, wrapping it in a bookmark that spans just that block when it carries an
/// identifier.
fn push_anchored(body: &mut Element, ctx: &mut Ctx, id: &str, element: Element) {
    let mark = open_bookmark(id, body, ctx);
    body.push(element);
    close_bookmark(mark, body);
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

/// Closes a bookmark opened by [`open_bookmark`], if one was opened.
fn close_bookmark(mark: Option<u32>, out: &mut Element) {
    if let Some(mark) = mark {
        out.push(Element::new("w:bookmarkEnd").attr("w:id", &mark.to_string()));
    }
}

/// Emits the `bookmarkEnd` for every open heading bookmark at or deeper than `level`, most-recent
/// first, so nested sections close from the inside out. `i32::MIN` closes every open bookmark.
fn close_bookmarks(body: &mut Element, bookmarks: &mut Vec<Bookmark>, level: i32) {
    while let Some(bookmark) = bookmarks.last() {
        if bookmark.level < level {
            break;
        }
        let id = bookmark.id;
        bookmarks.pop();
        body.push(Element::new("w:bookmarkEnd").attr("w:id", &id.to_string()));
    }
}

/// Clamps a heading level to one of the nine defined heading styles.
fn heading_style(level: i32) -> &'static str {
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

/// Renders a block nested inside a container, its `Para`/`Plain` blocks taking the caller's styles
/// and every other block taking its own natural shape.
fn render_flow(block: &Block, out: &mut Element, ctx: &mut Ctx, style: FlowStyle) {
    match block {
        // A display equation set among a nested paragraph's text is lifted onto its own centred line,
        // just as at the top level, so the paragraph is split around each one rather than left inline.
        Block::Para(inlines) if inlines.iter().any(is_display_equation) => {
            styled_flow_with_display(style.para, None, inlines, ctx, out);
        }
        Block::Para(inlines) => {
            if !inlines.is_empty() || ctx.features.keep_empty_paragraphs {
                out.push(paragraph(style.para, inlines, ctx));
            }
        }
        Block::Plain(inlines) => {
            if !inlines.is_empty() || ctx.features.keep_empty_paragraphs {
                out.push(paragraph(style.plain, inlines, ctx));
            }
        }
        // A heading nested inside a container is not a document-outline section, so it degrades to a
        // styled paragraph without the section-spanning bookmark the top-level walk would open.
        Block::Header(level, _, inlines) => {
            out.push(paragraph(heading_style(*level), inlines, ctx));
        }
        Block::BlockQuote(blocks) => {
            let mut previous = None;
            for inner in blocks {
                separate_adjacent_tables(previous, inner, out);
                render_flow(inner, out, ctx, FlowStyle::blockquote());
                previous = Some(inner);
            }
        }
        Block::BulletList(items) => render_bullet_list(items, out, ctx, 0, style.list_ambient),
        Block::OrderedList(attrs, items) => {
            render_ordered_list(attrs, items, out, ctx, 0, style.list_ambient);
        }
        Block::DefinitionList(items) => render_definition_list(items, out, ctx),
        Block::CodeBlock(attr, code) => {
            out.push(code_paragraph(attr, code, None, &ctx.highlighter));
        }
        Block::LineBlock(lines) => out.push(line_block_paragraph(style.para, lines, ctx)),
        Block::HorizontalRule => out.push(horizontal_rule()),
        Block::Figure(attr, caption, blocks) => {
            render_figure(attr.id.as_str(), caption, blocks, out, ctx);
        }
        Block::Table(table) => render_table(table, out, ctx, None),
        // A custom-style div re-styles its direct paragraphs and imposes that style on the loose lists
        // it holds; a plain one is transparent, passing the surrounding style through.
        Block::Div(attr, blocks) => {
            let inner = match custom_style(attr).or_else(|| bibliography_style(attr)) {
                Some(name) => FlowStyle {
                    para: name,
                    plain: name,
                    list_ambient: Some(name),
                },
                None => style,
            };
            let mut previous = None;
            for block in blocks {
                separate_adjacent_tables(previous, block, out);
                render_flow(block, out, ctx, inner);
                previous = Some(block);
            }
        }
        Block::RawBlock(format, payload) => {
            if format.0.as_str() == "openxml" {
                out.push_raw(payload);
            }
        }
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

/// Whether an attribute set names the given class.
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

/// The identifier that ties a comment's range markers to its entry, drawn from the marker's `id`.
fn comment_id(attr: &Attr) -> String {
    attr_value(attr, "id").unwrap_or("0").to_owned()
}

/// Opens a comment range: drops the range-start boundary into the flow and queues the comment's text
/// and author metadata for the comments part. The opening marker's inline content is the comment's
/// own text, so it is held back from the body rather than rendered where the range begins.
fn open_comment(attr: &Attr, body: &[Inline], out: &mut Element, ctx: &mut Ctx) {
    let id = comment_id(attr);
    out.push(Element::new("w:commentRangeStart").attr("w:id", &id));
    ctx.comments.push(Comment {
        id,
        author: attr_value(attr, "author").map(str::to_owned),
        date: attr_value(attr, "date").map(str::to_owned),
        initials: attr_value(attr, "initials").map(str::to_owned),
        body: body.to_vec(),
    });
}

/// Closes a comment range: drops the range-end boundary and the reference mark that ties the range
/// back to its entry in the comments part.
fn close_comment(attr: &Attr, props: &RunProps, out: &mut Element) {
    let id = comment_id(attr);
    out.push(Element::new("w:commentRangeEnd").attr("w:id", &id));
    let mut run = run_with_props(&props.with_style("CommentReference"));
    run.push(Element::new("w:commentReference").attr("w:id", &id));
    out.push(run);
}

/// Renders one comment's entry for the comments part: its identifier and author metadata, then a
/// single paragraph in the comment style that opens with the annotation mark and carries the text.
fn render_comment_entry(comment: &Comment, ctx: &mut Ctx) -> Element {
    let mut element = Element::new("w:comment").attr("w:id", &comment.id);
    if let Some(author) = &comment.author {
        element = element.attr("w:author", author);
    }
    if let Some(date) = &comment.date {
        element = element.attr("w:date", date);
    }
    if let Some(initials) = &comment.initials {
        element = element.attr("w:initials", initials);
    }
    let mut para = Element::new("w:p");
    para.push(paragraph_props(Some("CommentText"), None, None));
    para.push(annotation_reference_run());
    render_runs(&comment.body, &RunProps::default(), ctx, &mut para);
    element.child(para)
}

/// A tracked-change wrapper (`w:ins` or `w:del`) carrying its change id and the author and date the
/// marker records. An unattributed change is credited to an unknown author; a change with no date
/// records none.
fn tracked_change(tag: &str, id: u32, attr: &Attr) -> Element {
    let mut element = Element::new(tag)
        .attr("w:id", &id.to_string())
        .attr("w:author", attr_value(attr, "author").unwrap_or("unknown"));
    if let Some(date) = attr_value(attr, "date") {
        element = element.attr("w:date", date);
    }
    element
}

/// The run that opens a comment entry with its annotation reference mark.
fn annotation_reference_run() -> Element {
    Element::new("w:r")
        .child(
            Element::new("w:rPr").child(Element::new("w:rStyle").attr("w:val", "CommentReference")),
        )
        .child(Element::new("w:annotationRef"))
}

/// Renders a bulleted list, binding every item to one concrete list number at the given depth.
/// `ambient` is the paragraph style a surrounding custom-style div imposes on the items' loose
/// paragraphs; it is `None` in ordinary flow.
fn render_bullet_list(
    items: &[Vec<Block>],
    out: &mut Element,
    ctx: &mut Ctx,
    depth: u32,
    ambient: Option<&str>,
) {
    // A list whose every item leads with a checkbox is a task list: each item binds to a checkbox
    // number its state selects, with the leading glyph and the space after it dropped from the text,
    // so the box itself stands as the item's marker. Each item takes its own number so an unchecked
    // item and a checked one can carry different boxes.
    if let Some(states) = task_list_states(items) {
        for (item, checked) in items.iter().zip(states) {
            let num_id = ctx.plan.checkbox(checked);
            let stripped = strip_checkbox(item);
            render_list_item(&stripped, num_id, depth, out, ctx, ambient);
        }
        return;
    }
    let num_id = ctx.plan.bullet();
    for item in items {
        render_list_item(item, num_id, depth, out, ctx, ambient);
    }
}

/// The checked state of every item when the list is a task list — one whose every item leads with a
/// ballot-box marker — else `None`. An empty list is never a task list.
fn task_list_states(items: &[Vec<Block>]) -> Option<Vec<bool>> {
    if items.is_empty() {
        return None;
    }
    items.iter().map(|item| checkbox_state(item)).collect()
}

/// Whether an item leads with a task-list checkbox, and if so whether it is ticked. An item qualifies
/// when its first block is a `Plain` or `Para` whose first inline is the empty or ticked ballot box
/// followed by a space.
fn checkbox_state(item: &[Block]) -> Option<bool> {
    let (Block::Plain(inlines) | Block::Para(inlines)) = item.first()? else {
        return None;
    };
    let checked = match inlines.first()? {
        Inline::Str(text) if text.as_str() == "\u{2610}" => false,
        Inline::Str(text) if text.as_str() == "\u{2612}" => true,
        _ => return None,
    };
    matches!(inlines.get(1), Some(Inline::Space)).then_some(checked)
}

/// A task item's blocks with the leading ballot-box glyph and the space after it removed from the
/// first paragraph, so the checkbox does not double as both the marker and inline text.
fn strip_checkbox(item: &[Block]) -> Vec<Block> {
    let mut blocks = item.to_vec();
    if let Some(Block::Plain(inlines) | Block::Para(inlines)) = blocks.first_mut() {
        let cut = inlines.len().min(2);
        inlines.drain(..cut);
    }
    blocks
}

/// Renders an ordered list, binding every item to the concrete number its marker style, delimiter
/// and start select.
fn render_ordered_list(
    attrs: &ListAttributes,
    items: &[Vec<Block>],
    out: &mut Element,
    ctx: &mut Ctx,
    depth: u32,
    ambient: Option<&str>,
) {
    let num_id = ctx.plan.ordered(attrs);
    for item in items {
        render_list_item(item, num_id, depth, out, ctx, ambient);
    }
}

/// Renders one list item: its lead paragraph binds to the list's number, every later block that
/// yields a paragraph binds to the scaffold number so it reads as a continuation line, a nested table
/// is indented to the item's level, and a nested list deepens the level.
fn render_list_item(
    item: &[Block],
    num_id: u32,
    depth: u32,
    out: &mut Element,
    ctx: &mut Ctx,
    ambient: Option<&str>,
) {
    let mut lead_used = false;
    for block in item {
        render_item_block(block, num_id, depth, &mut lead_used, out, ctx, ambient);
    }
}

/// Renders one block of a list item. Each paragraph the block yields binds to the item's own number
/// on the first paragraph and to the continuation number thereafter, keeping the whole item indented
/// under one marker; a nested table takes the item's indent instead; and a nested list — when it
/// leads the item, with no paragraph ahead of it to carry the marker — is preceded by an empty
/// numbered paragraph that holds the item's marker.
fn render_item_block(
    block: &Block,
    num_id: u32,
    depth: u32,
    lead_used: &mut bool,
    out: &mut Element,
    ctx: &mut Ctx,
    ambient: Option<&str>,
) {
    match block {
        Block::Plain(inlines) => {
            if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                return;
            }
            let number = list_number(lead_used, num_id, ctx);
            out.push(styled_paragraph(
                Some("Compact"),
                Some((number, depth)),
                None,
                inlines,
                ctx,
            ));
        }
        // A loose item's paragraph takes the style a surrounding custom-style div imposes, alongside
        // the item's own numbering; without such a div it carries no explicit style.
        Block::Para(inlines) => {
            if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                return;
            }
            let number = list_number(lead_used, num_id, ctx);
            out.push(styled_paragraph(
                ambient,
                Some((number, depth)),
                None,
                inlines,
                ctx,
            ));
        }
        // A heading nested in a list item is not an outline section, so it takes its level's style
        // without opening a bookmark, and joins the item's numbering like any other paragraph.
        Block::Header(level, _, inlines) => {
            let style = heading_style(*level);
            let number = list_number(lead_used, num_id, ctx);
            out.push(styled_paragraph(
                Some(style),
                Some((number, depth)),
                None,
                inlines,
                ctx,
            ));
        }
        Block::CodeBlock(attr, code) => {
            let number = list_number(lead_used, num_id, ctx);
            out.push(code_paragraph(
                attr,
                code,
                Some((number, depth)),
                &ctx.highlighter,
            ));
        }
        // A block quote's own paragraphs each take the block-quote style and join the item's
        // numbering; any other block it holds renders as it would directly in the item.
        Block::BlockQuote(blocks) => {
            for inner in blocks {
                match inner {
                    Block::Para(inlines) | Block::Plain(inlines) => {
                        if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                            continue;
                        }
                        let number = list_number(lead_used, num_id, ctx);
                        out.push(styled_paragraph(
                            Some("BlockText"),
                            Some((number, depth)),
                            None,
                            inlines,
                            ctx,
                        ));
                    }
                    other => render_item_block(other, num_id, depth, lead_used, out, ctx, ambient),
                }
            }
        }
        Block::Table(table) => render_table(table, out, ctx, Some(depth)),
        Block::BulletList(items) => {
            lead_empty_paragraph(num_id, depth, lead_used, out, ctx);
            render_bullet_list(items, out, ctx, depth + 1, ambient);
        }
        Block::OrderedList(attrs, items) => {
            lead_empty_paragraph(num_id, depth, lead_used, out, ctx);
            render_ordered_list(attrs, items, out, ctx, depth + 1, ambient);
        }
        // A transparent div contributes its blocks to the item directly, so its paragraphs number
        // like the item's own.
        Block::Div(attr, blocks) if custom_style(attr).is_none() => {
            for inner in blocks {
                render_item_block(inner, num_id, depth, lead_used, out, ctx, ambient);
            }
        }
        other => render_flow(
            other,
            out,
            ctx,
            FlowStyle {
                para: "BodyText",
                plain: "Compact",
                list_ambient: None,
            },
        ),
    }
}

/// Emits the empty paragraph that carries a list item's marker when a nested list leads the item, so
/// the outer item still shows its own number or bullet. Does nothing once the item's lead is spent.
fn lead_empty_paragraph(
    num_id: u32,
    depth: u32,
    lead_used: &mut bool,
    out: &mut Element,
    ctx: &mut Ctx,
) {
    if *lead_used {
        return;
    }
    *lead_used = true;
    out.push(styled_paragraph(
        Some("Compact"),
        Some((num_id, depth)),
        None,
        &[],
        ctx,
    ));
}

/// The number a list item's next paragraph binds to: the item's own on its first paragraph, the
/// scaffold continuation number thereafter.
fn list_number(lead_used: &mut bool, num_id: u32, ctx: &Ctx) -> u32 {
    let number = if *lead_used {
        ctx.plan.continuation_num()
    } else {
        num_id
    };
    *lead_used = true;
    number
}

/// Renders a definition list: each term as a `DefinitionTerm` paragraph and each definition's blocks
/// under the `Definition` style.
fn render_definition_list(
    items: &[(Vec<Inline>, Vec<Vec<Block>>)],
    out: &mut Element,
    ctx: &mut Ctx,
) {
    for (term, definitions) in items {
        if !term.is_empty() {
            out.push(paragraph("DefinitionTerm", term, ctx));
        }
        for definition in definitions {
            for block in definition {
                render_definition_block(block, out, ctx);
            }
        }
    }
}

/// Renders one block of a definition's body under the `Definition` style.
fn render_definition_block(block: &Block, out: &mut Element, ctx: &mut Ctx) {
    match block {
        Block::Para(inlines) | Block::Plain(inlines) => {
            if !inlines.is_empty() || ctx.features.keep_empty_paragraphs {
                out.push(paragraph("Definition", inlines, ctx));
            }
        }
        other => render_flow(
            other,
            out,
            ctx,
            FlowStyle {
                para: "Definition",
                plain: "Definition",
                list_ambient: Some("Definition"),
            },
        ),
    }
}

/// Renders a table: its caption paragraphs, then the grid itself with header rows marked and merged
/// cells laid out. `indent`, when set, is the list-nesting depth a table inside a list item is shifted
/// by, so it aligns under its item's text.
fn render_table(table: &Table, out: &mut Element, ctx: &mut Ctx, indent: Option<u32>) {
    let columns = table.col_specs.len();
    let has_head = !table.head.rows.is_empty();
    let has_foot = !table.foot.rows.is_empty();

    let mark = open_bookmark(table.attr.id.as_str(), out, ctx);
    // A table opens a fresh first/body paragraph run: its first cell paragraph is a `FirstParagraph`,
    // unless a caption precedes it and takes that opening slot, leaving the cells to continue as body.
    ctx.prev_paragraph = false;
    render_caption(
        &table.caption.long,
        out,
        ctx,
        "TableCaption",
        Numbered::Table,
    );
    if !blocks_plain_text(&table.caption.long).is_empty() {
        ctx.prev_paragraph = true;
    }

    let mut tbl = Element::new("w:tbl");
    tbl.push(table_properties(table, has_head, has_foot, indent));
    tbl.push(table_grid(&table.col_specs));

    // The remaining rows each cell of a row spans are tracked across rows so a row-spanning cell
    // leaves a merge-continuation cell in every row below it, for the width it covers.
    let mut carried = vec![0u32; columns];
    let mut carried_span = vec![1u32; columns];

    for row in &table.head.rows {
        tbl.push(render_row(
            row,
            &table.col_specs,
            &mut carried,
            &mut carried_span,
            columns,
            true,
            ctx,
        ));
    }
    for section in &table.bodies {
        for row in section.head.iter().chain(section.body.iter()) {
            tbl.push(render_row(
                row,
                &table.col_specs,
                &mut carried,
                &mut carried_span,
                columns,
                false,
                ctx,
            ));
        }
    }
    for row in &table.foot.rows {
        tbl.push(render_row(
            row,
            &table.col_specs,
            &mut carried,
            &mut carried_span,
            columns,
            false,
            ctx,
        ));
    }
    out.push(tbl);
    close_bookmark(mark, out);
}

/// The table's properties: its style, width, list indent, header/footer look and caption text.
fn table_properties(table: &Table, has_head: bool, has_foot: bool, indent: Option<u32>) -> Element {
    let mut properties = Element::new("w:tblPr");
    properties.push(Element::new("w:tblStyle").attr("w:val", "Table"));

    let sized: f64 = table
        .col_specs
        .iter()
        .filter_map(|spec| match &spec.width {
            ColWidth::ColWidth(fraction) => Some(*fraction),
            ColWidth::ColWidthDefault => None,
        })
        .sum();
    if sized > 0.0 {
        let percent = (sized * 5000.0).round();
        properties.push(
            Element::new("w:tblW")
                .attr("w:type", "pct")
                .attr("w:w", &percent.to_string()),
        );
        properties.push(Element::new("w:tblLayout").attr("w:type", "fixed"));
    } else {
        properties.push(
            Element::new("w:tblW")
                .attr("w:type", "auto")
                .attr("w:w", "0"),
        );
    }

    // A table nested in a list item is left-aligned and shifted one indent step per nesting level so
    // it sits under its item's text rather than at the page margin.
    if let Some(depth) = indent {
        properties.push(Element::new("w:jc").attr("w:val", "left"));
        properties.push(
            Element::new("w:tblInd")
                .attr("w:w", &(LIST_INDENT_STEP * (depth + 1)).to_string())
                .attr("w:type", "dxa"),
        );
    }

    let (first_row, look) = if has_head {
        ("1", "0020")
    } else {
        ("0", "0000")
    };
    let last_row = if has_foot { "1" } else { "0" };
    properties.push(
        Element::new("w:tblLook")
            .attr("w:firstRow", first_row)
            .attr("w:lastRow", last_row)
            .attr("w:firstColumn", "0")
            .attr("w:lastColumn", "0")
            .attr("w:noHBand", "0")
            .attr("w:noVBand", "0")
            .attr("w:val", look),
    );

    let caption = blocks_plain_text(&table.caption.long);
    if !caption.is_empty() {
        properties.push(Element::new("w:tblCaption").attr("w:val", &caption));
    }
    properties
}

/// The column grid: each column's width from its fraction of the text width, or an equal share when
/// no fractions are given.
#[allow(clippy::cast_precision_loss)] // Column counts are tiny, far inside f64's exact range.
fn table_grid(col_specs: &[ColSpec]) -> Element {
    let mut grid = Element::new("w:tblGrid");
    let columns = col_specs.len();
    for spec in col_specs {
        let width = match &spec.width {
            ColWidth::ColWidth(fraction) => (fraction * TABLE_TEXT_WIDTH).round(),
            ColWidth::ColWidthDefault if columns > 0 => (TABLE_TEXT_WIDTH / columns as f64).round(),
            ColWidth::ColWidthDefault => 0.0,
        };
        grid.push(Element::new("w:gridCol").attr("w:w", &width.to_string()));
    }
    grid
}

/// Renders one table row, filling merge-continuation cells for any row-spans carried down from above
/// before laying out the row's own cells.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Column spans are small counts.
fn render_row(
    row: &Row,
    col_specs: &[ColSpec],
    carried: &mut [u32],
    carried_span: &mut [u32],
    columns: usize,
    is_header: bool,
    ctx: &mut Ctx,
) -> Element {
    let mut tr = Element::new("w:tr");
    if is_header {
        tr.push(Element::new("w:trPr").child(Element::new("w:tblHeader").attr("w:val", "on")));
    }

    let mut column = 0usize;
    let mut cells = row.cells.iter();
    while column < columns {
        if carried.get(column).copied().unwrap_or(0) > 0 {
            let span = carried_span.get(column).copied().unwrap_or(1).max(1) as usize;
            tr.push(continuation_cell(span as u32));
            decrement_carried(carried, column, span, columns);
            column = column.saturating_add(span).max(column + 1);
            continue;
        }
        let Some(cell) = cells.next() else {
            break;
        };
        let span = cell.col_span.max(1) as usize;
        let rows = cell.row_span.max(1);
        let jc = effective_jc(cell, col_specs, column);
        tr.push(render_normal_cell(cell, span as u32, rows, jc, ctx));
        if rows > 1 {
            let remaining = (rows - 1).max(0) as u32;
            let end = column.saturating_add(span).min(columns);
            for slot in column..end {
                if let Some(value) = carried.get_mut(slot) {
                    *value = remaining;
                }
                if let Some(value) = carried_span.get_mut(slot) {
                    *value = span.max(1) as u32;
                }
            }
        }
        column = column.saturating_add(span).max(column + 1);
    }
    tr
}

/// Decrements the carried row-span count for the columns a continuation cell just covered.
fn decrement_carried(carried: &mut [u32], column: usize, span: usize, columns: usize) {
    let end = column.saturating_add(span).min(columns);
    for slot in column..end {
        if let Some(value) = carried.get_mut(slot) {
            *value = value.saturating_sub(1);
        }
    }
}

/// A merge-continuation cell: an empty cell that continues the vertical merge above it, always
/// carrying an explicit grid span so it lines up under the cell it continues.
fn continuation_cell(span: u32) -> Element {
    let properties = Element::new("w:tcPr")
        .child(Element::new("w:gridSpan").attr("w:val", &span.to_string()))
        .child(Element::new("w:vMerge").attr("w:val", "continue"));
    Element::new("w:tc")
        .child(properties)
        .child(Element::new("w:p").child(Element::new("w:pPr")))
}

/// Renders a cell that begins its own content: its grid span and merge start when it spans, then its
/// block content laid out under the cell's effective alignment.
fn render_normal_cell(
    cell: &Cell,
    span: u32,
    rows: i32,
    jc: Option<&str>,
    ctx: &mut Ctx,
) -> Element {
    let mut tc = Element::new("w:tc");
    let mut properties = Element::new("w:tcPr");
    if span > 1 {
        properties.push(Element::new("w:gridSpan").attr("w:val", &span.to_string()));
    }
    if rows > 1 {
        properties.push(Element::new("w:vMerge").attr("w:val", "restart"));
    }
    tc.push(properties);

    let mut wrote = false;
    let mut previous = None;
    for block in &cell.content {
        separate_adjacent_tables(previous, block, &mut tc);
        wrote |= render_cell_block(block, &mut tc, jc, ctx);
        previous = Some(block);
    }
    // A cell must hold at least one paragraph and must not end on a table. A cell with no content
    // takes a compact filler paragraph; one whose content is present but renders to nothing (a
    // comment, a raw block for another target, a list of empty items) takes a bare filler paragraph,
    // as does a cell whose content ends on a nested table, so the table is not the cell's final child.
    if !wrote {
        if cell.content.is_empty() {
            tc.push(Element::new("w:p").child(paragraph_props(Some("Compact"), None, None)));
        } else {
            tc.push(Element::new("w:p"));
        }
    } else if tc.last_child_element_name() == Some("w:tbl") {
        tc.push(Element::new("w:p"));
    }
    tc
}

/// Renders one block of a cell's content, applying the cell's alignment to its direct paragraphs.
/// Returns whether it emitted anything.
fn render_cell_block(block: &Block, tc: &mut Element, jc: Option<&str>, ctx: &mut Ctx) -> bool {
    match block {
        // A cell's paragraphs join the table's first/body run: the very first paragraph in the table
        // opens as `FirstParagraph` and every one after it continues as `BodyText`. A display equation
        // among the text is lifted onto its own centred paragraph in that same style.
        Block::Para(inlines) => {
            if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                return false;
            }
            let style = body_style(ctx.prev_paragraph);
            let emitted = if inlines.iter().any(is_display_equation) {
                styled_flow_with_display(style, jc, inlines, ctx, tc)
            } else {
                tc.push(styled_paragraph(Some(style), None, jc, inlines, ctx));
                true
            };
            ctx.prev_paragraph = true;
            emitted
        }
        Block::Plain(inlines) => {
            if inlines.is_empty() && !ctx.features.keep_empty_paragraphs {
                return false;
            }
            tc.push(styled_paragraph(Some("Compact"), None, jc, inlines, ctx));
            // A cell's compact paragraph still advances the table's first/body run, so the next
            // paragraph in the table continues as body text rather than reopening as the first.
            ctx.prev_paragraph = true;
            true
        }
        Block::CodeBlock(attr, code) => {
            tc.push(code_paragraph(attr, code, None, &ctx.highlighter));
            true
        }
        other => {
            let before = tc.child_count();
            render_flow(
                other,
                tc,
                ctx,
                FlowStyle {
                    para: "BodyText",
                    plain: "Compact",
                    list_ambient: None,
                },
            );
            tc.child_count() > before
        }
    }
}

/// The cell's effective horizontal alignment: its own if set, otherwise its column's.
fn effective_jc(cell: &Cell, col_specs: &[ColSpec], column: usize) -> Option<&'static str> {
    let align = match cell.align {
        Alignment::AlignDefault => col_specs.get(column).map(|spec| &spec.align),
        ref own => Some(own),
    };
    match align {
        Some(Alignment::AlignLeft) => Some("left"),
        Some(Alignment::AlignRight) => Some("right"),
        Some(Alignment::AlignCenter) => Some("center"),
        _ => None,
    }
}

/// Renders a figure: a single embedded image as a captioned drawing when its bytes resolve,
/// otherwise the figure's content boxed in a centered frame, with the caption following in either
/// case.
fn render_figure(id: &str, caption: &Caption, body: &[Block], out: &mut Element, ctx: &mut Ctx) {
    let mark = open_bookmark(id, out, ctx);
    if let Some((attr, alt, target)) = figure_single_image(body)
        && let Some(drawing) = image_drawing_for(attr, target, alt, ctx)
    {
        out.push(
            Element::new("w:p")
                .child(paragraph_props(Some("CaptionedFigure"), None, None))
                .child(Element::new("w:r").child(drawing)),
        );
        render_figure_caption(caption, out, ctx);
        close_bookmark(mark, out);
        return;
    }
    render_figure_frame(body, out, ctx);
    render_figure_caption(caption, out, ctx);
    close_bookmark(mark, out);
}

/// The lone image a figure wraps, when its body is exactly one paragraph holding exactly one image.
fn figure_single_image(body: &[Block]) -> Option<(&Attr, &[Inline], &Target)> {
    let [only] = body else {
        return None;
    };
    let inlines = match only {
        Block::Plain(inlines) | Block::Para(inlines) => inlines.as_slice(),
        _ => return None,
    };
    let [Inline::Image(attr, alt, target)] = inlines else {
        return None;
    };
    Some((&**attr, alt.as_slice(), &**target))
}

/// Renders a figure's content as a single centered, full-width frame.
fn render_figure_frame(body: &[Block], out: &mut Element, ctx: &mut Ctx) {
    let mut tbl = Element::new("w:tbl");
    tbl.push(
        Element::new("w:tblPr")
            .child(Element::new("w:tblStyle").attr("w:val", "FigureTable"))
            .child(
                Element::new("w:tblW")
                    .attr("w:type", "auto")
                    .attr("w:w", "0"),
            )
            .child(Element::new("w:jc").attr("w:val", "center"))
            .child(
                Element::new("w:tblLook")
                    .attr("w:firstRow", "0")
                    .attr("w:lastRow", "0")
                    .attr("w:firstColumn", "0")
                    .attr("w:lastColumn", "0"),
            ),
    );
    tbl.push(
        Element::new("w:tblGrid")
            .child(Element::new("w:gridCol").attr("w:w", &TABLE_TEXT_WIDTH.to_string())),
    );

    let mut tc = Element::new("w:tc");
    tc.push(Element::new("w:tcPr"));
    let mut wrote = false;
    for block in body {
        match block {
            Block::Para(inlines) | Block::Plain(inlines) => {
                if !inlines.is_empty() || ctx.features.keep_empty_paragraphs {
                    tc.push(styled_paragraph(
                        Some("Compact"),
                        None,
                        Some("center"),
                        inlines,
                        ctx,
                    ));
                    wrote = true;
                }
            }
            other => {
                render_flow(
                    other,
                    &mut tc,
                    ctx,
                    FlowStyle {
                        para: "Compact",
                        plain: "Compact",
                        list_ambient: None,
                    },
                );
                wrote = true;
            }
        }
    }
    if !wrote {
        tc.push(Element::new("w:p").child(Element::new("w:pPr")));
    }
    tbl.push(Element::new("w:tr").child(tc));
    out.push(tbl);
}

/// Renders a figure's caption as `ImageCaption` paragraphs.
fn render_figure_caption(caption: &Caption, out: &mut Element, ctx: &mut Ctx) {
    render_caption(&caption.long, out, ctx, "ImageCaption", Numbered::Figure);
}

/// A figure or table whose caption can carry an auto-incrementing number.
#[derive(Clone, Copy)]
enum Numbered {
    Figure,
    Table,
}

impl Numbered {
    /// The word that opens the caption label.
    fn label(self) -> &'static str {
        match self {
            Numbered::Figure => "Figure",
            Numbered::Table => "Table",
        }
    }

    /// The field instruction that draws and advances the running count for this kind.
    fn field_instruction(self) -> &'static str {
        match self {
            Numbered::Figure => "SEQ Figure \\* ARABIC ",
            Numbered::Table => "SEQ Table \\* ARABIC ",
        }
    }

    /// The prefix of the bookmark name a caption anchors for cross-references.
    fn bookmark_prefix(self) -> &'static str {
        match self {
            Numbered::Figure => "ref_fig",
            Numbered::Table => "table",
        }
    }
}

/// Renders a caption's blocks under `style`. With native numbering on, the first paragraph gains a
/// "Figure N: " (or "Table N: ") label drawn from the running count for its kind; otherwise the
/// blocks render plainly.
fn render_caption(
    blocks: &[Block],
    out: &mut Element,
    ctx: &mut Ctx,
    style: &'static str,
    kind: Numbered,
) {
    let mut rest = blocks.iter();
    if let Some(first) = rest.clone().next() {
        let leading = match first {
            Block::Para(inlines) | Block::Plain(inlines)
                if ctx.features.native_numbering && !inlines.is_empty() =>
            {
                Some(inlines)
            }
            _ => None,
        };
        if let Some(inlines) = leading {
            out.push(numbered_caption(style, kind, inlines, ctx));
            rest.next();
        }
    }
    for block in rest {
        render_styled_block(block, out, ctx, style);
    }
}

/// A caption paragraph led by an auto-number: the label word, the sequence field, an anchoring
/// bookmark, then the caption text after a `": "` separator.
fn numbered_caption(
    style: &'static str,
    kind: Numbered,
    inlines: &[Inline],
    ctx: &mut Ctx,
) -> Element {
    let number = match kind {
        Numbered::Figure => {
            ctx.figure_number = ctx.figure_number.saturating_add(1);
            ctx.figure_number
        }
        Numbered::Table => {
            ctx.table_number = ctx.table_number.saturating_add(1);
            ctx.table_number
        }
    };
    let mark = ctx.next_bookmark_id;
    ctx.next_bookmark_id = ctx.next_bookmark_id.wrapping_add(1);
    let name = format!("{}{number}", kind.bookmark_prefix());

    let mut para = Element::new("w:p");
    para.push(paragraph_props(Some(style), None, None));
    para.push(
        Element::new("w:bookmarkStart")
            .attr("w:id", &mark.to_string())
            .attr("w:name", &name),
    );
    // The label word and its number are joined by a non-breaking space so they never wrap apart.
    para.push(text_run(
        &RunProps::default(),
        &format!("{}\u{a0}", kind.label()),
    ));
    para.push(sequence_field(kind.field_instruction(), number));
    para.push(Element::new("w:bookmarkEnd").attr("w:id", &mark.to_string()));

    let mut prefixed = Vec::with_capacity(inlines.len() + 1);
    prefixed.push(Inline::Str(": ".into()));
    prefixed.extend(inlines.iter().cloned());
    render_runs(&prefixed, &RunProps::default(), ctx, &mut para);
    para
}

/// A simple field drawing one running sequence number; its number run carries no space handling so
/// it stays a bare digit.
fn sequence_field(instruction: &str, number: u32) -> Element {
    Element::new("w:fldSimple")
        .attr("w:instr", instruction)
        .child(Element::new("w:r").child(Element::new("w:t").text(&number.to_string())))
}

/// Renders a block whose `Para`/`Plain` shape takes one shared paragraph style.
fn render_styled_block(block: &Block, out: &mut Element, ctx: &mut Ctx, style: &'static str) {
    match block {
        Block::Para(inlines) if inlines.iter().any(is_display_equation) => {
            styled_flow_with_display(style, None, inlines, ctx, out);
        }
        Block::Para(inlines) | Block::Plain(inlines) => {
            if !inlines.is_empty() || ctx.features.keep_empty_paragraphs {
                out.push(paragraph(style, inlines, ctx));
            }
        }
        other => render_flow(
            other,
            out,
            ctx,
            FlowStyle {
                para: style,
                plain: style,
                list_ambient: None,
            },
        ),
    }
}

/// Renders one footnote entry: the marker paragraph joined to the note's first paragraph, then the
/// note's remaining blocks.
fn render_footnote_entry(id: u32, blocks: &[Block], ctx: &mut Ctx) -> Element {
    let mut footnote = Element::new("w:footnote").attr("w:id", &id.to_string());
    let rest: &[Block] = if let Some(Block::Para(inlines) | Block::Plain(inlines)) = blocks.first()
    {
        let mut para = Element::new("w:p");
        para.push(paragraph_props(Some("FootnoteText"), None, None));
        para.push(footnote_marker_run());
        para.push(text_run(&RunProps::default(), " "));
        render_runs(inlines, &RunProps::default(), ctx, &mut para);
        footnote.push(para);
        blocks.get(1..).unwrap_or(&[])
    } else {
        // A note whose first block is not a paragraph gets a standalone marker paragraph, and
        // every block then follows as continuation content.
        footnote.push(
            Element::new("w:p")
                .child(paragraph_props(Some("FootnoteText"), None, None))
                .child(footnote_marker_run()),
        );
        blocks
    };
    for block in rest {
        render_styled_block(block, &mut footnote, ctx, "FootnoteText");
    }
    footnote
}

/// The run that draws a footnote's own back-reference mark inside its entry.
fn footnote_marker_run() -> Element {
    Element::new("w:r")
        .child(
            Element::new("w:rPr")
                .child(Element::new("w:rStyle").attr("w:val", "FootnoteReference")),
        )
        .child(Element::new("w:footnoteRef"))
}

/// The run that references a footnote from the body, carrying the surrounding formatting plus the
/// footnote-reference character style.
fn footnote_reference_run(id: u32, props: &RunProps) -> Element {
    let mut run = run_with_props(&props.with_style("FootnoteReference"));
    run.push(Element::new("w:footnoteReference").attr("w:id", &id.to_string()));
    run
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
    para.push(paragraph_props(style, numbering, jc));
    render_runs(inlines, &RunProps::default(), ctx, &mut para);
    para
}

/// The paragraph properties element for a style, list binding and alignment. Emitted even when empty
/// so a paragraph always carries a `w:pPr`.
fn paragraph_props(
    style: Option<&str>,
    numbering: Option<(u32, u32)>,
    jc: Option<&str>,
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
    if let Some(jc) = jc {
        properties.push(Element::new("w:jc").attr("w:val", jc));
    }
    properties
}

/// Builds a paragraph with a paragraph style and the runs its inline content yields.
fn paragraph(style: &str, inlines: &[Inline], ctx: &mut Ctx) -> Element {
    styled_paragraph(Some(style), None, None, inlines, ctx)
}

/// A code block: one paragraph in the source-code style. When a highlighter classifies the block's
/// language, each token becomes a run carrying its token style; otherwise each line becomes a run in
/// the plain code character style. Lines are separated by breaks. `numbering` binds the paragraph to
/// a list number when the block sits inside a list item.
#[cfg_attr(not(feature = "highlight"), allow(unused_variables))]
fn code_paragraph(attr: &Attr, code: &str, numbering: Option<(u32, u32)>, hl: &DocxHl) -> Element {
    let mut para = Element::new("w:p");
    para.push(paragraph_props(Some("SourceCode"), numbering, None));
    #[cfg(feature = "highlight")]
    if let Some(runs) = highlighted_code_runs(attr, code, hl) {
        for run in runs {
            para.push(run);
        }
        return para;
    }
    let props = RunProps {
        style: Some(Cow::Borrowed("VerbatimChar")),
        ..RunProps::default()
    };
    let mut first = true;
    for line in code.split('\n') {
        if !first {
            para.push(break_run(&RunProps::default()));
        }
        first = false;
        para.push(text_run(&props, line));
    }
    para
}

/// The token runs for a code block whose language a highlighter recognizes, or `None` when the block
/// carries no recognized language class and should fall back to the plain code style. Every token —
/// including plain and whitespace-only ones — is wrapped in its own styled run, and consecutive
/// source lines are joined by break runs.
#[cfg(feature = "highlight")]
fn highlighted_code_runs(attr: &Attr, code: &str, hl: &DocxHl) -> Option<Vec<Element>> {
    let highlighter = hl.as_ref()?;
    let language = attr
        .classes
        .iter()
        .find(|class| highlighter.registry().is_known(class.as_str()))?;
    let source = code.strip_suffix('\n').unwrap_or(code);
    let lines = highlighter
        .highlight(language.as_str(), source)
        .unwrap_or_default();
    let mut runs = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            runs.push(break_run(&RunProps::default()));
        }
        for token in line {
            let props = RunProps {
                style: Some(Cow::Owned(format!("{}Tok", token.kind.style_key()))),
                ..RunProps::default()
            };
            runs.push(text_run(&props, &token.text));
        }
    }
    Some(runs)
}

/// The token runs for inline code whose language a highlighter recognizes, or `None` when the span
/// carries no recognized language class and should fall back to the plain verbatim run. Each token
/// becomes its own styled run; a deletion context is carried onto every run.
#[cfg(feature = "highlight")]
fn highlighted_inline_runs(
    attr: &Attr,
    text: &str,
    hl: &DocxHl,
    deletion: bool,
) -> Option<Vec<Element>> {
    let highlighter = hl.as_ref()?;
    let language = attr
        .classes
        .iter()
        .find(|class| highlighter.registry().is_known(class.as_str()))?;
    let lines = highlighter
        .highlight(language.as_str(), text)
        .unwrap_or_default();
    let mut runs = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            runs.push(break_run(&RunProps {
                deletion,
                ..RunProps::default()
            }));
        }
        for token in line {
            let props = RunProps {
                style: Some(Cow::Owned(format!("{}Tok", token.kind.style_key()))),
                deletion,
                ..RunProps::default()
            };
            runs.push(text_run(&props, &token.text));
        }
    }
    Some(runs)
}

/// A line block: one paragraph whose lines are separated by breaks, each line's inlines lowered to
/// runs in the surrounding paragraph style.
fn line_block_paragraph(style: &str, lines: &[Vec<Inline>], ctx: &mut Ctx) -> Element {
    let mut para = Element::new("w:p");
    para.push(paragraph_props(Some(style), None, None));
    let mut first = true;
    for line in lines {
        if !first {
            para.push(break_run(&RunProps::default()));
        }
        first = false;
        render_runs(line, &RunProps::default(), ctx, &mut para);
    }
    para
}

/// A thematic break, rendered as a paragraph holding a full-width horizontal-rule drawing.
fn horizontal_rule() -> Element {
    let rect = Element::new("v:rect")
        .attr("style", "width:0;height:1.5pt")
        .attr("o:hralign", "center")
        .attr("o:hrstd", "t")
        .attr("o:hr", "t");
    let run = Element::new("w:r").child(Element::new("w:pict").child(rect));
    Element::new("w:p").child(run)
}

/// The run properties accumulated down a chain of nested inline formatting. Rendered in the fixed
/// schema order so output stays stable regardless of nesting order.
// Each flag is an independent on/off run-property toggle, so a flat set of bools is the natural shape.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Default)]
struct RunProps {
    style: Option<Cow<'static, str>>,
    bold: bool,
    italic: bool,
    smallcaps: bool,
    strike: bool,
    underline: bool,
    east_asian: bool,
    vert_align: Option<&'static str>,
    /// Whether these runs sit inside a deletion, so their text is emitted as deleted text rather
    /// than visible text. This is a run-context flag, not a `w:rPr` toggle, so it stays out of the
    /// property element.
    deletion: bool,
}

impl RunProps {
    fn is_empty(&self) -> bool {
        self.style.is_none()
            && !self.bold
            && !self.italic
            && !self.smallcaps
            && !self.strike
            && !self.underline
            && !self.east_asian
            && self.vert_align.is_none()
    }

    fn with_bold(&self) -> Self {
        Self {
            bold: true,
            ..self.clone()
        }
    }

    fn with_italic(&self) -> Self {
        Self {
            italic: true,
            ..self.clone()
        }
    }

    fn with_smallcaps(&self) -> Self {
        Self {
            smallcaps: true,
            ..self.clone()
        }
    }

    fn with_strike(&self) -> Self {
        Self {
            strike: true,
            ..self.clone()
        }
    }

    fn with_underline(&self) -> Self {
        Self {
            underline: true,
            ..self.clone()
        }
    }

    fn with_vert_align(&self, value: &'static str) -> Self {
        Self {
            vert_align: Some(value),
            ..self.clone()
        }
    }

    fn with_style(&self, value: impl Into<Cow<'static, str>>) -> Self {
        Self {
            style: Some(value.into()),
            ..self.clone()
        }
    }

    fn with_east_asian(&self, value: bool) -> Self {
        Self {
            east_asian: value,
            ..self.clone()
        }
    }

    fn with_deletion(&self) -> Self {
        Self {
            deletion: true,
            ..self.clone()
        }
    }

    /// The `w:rPr` element for these properties, or `None` when no property is set.
    fn element(&self) -> Option<Element> {
        if self.is_empty() {
            return None;
        }
        let mut rpr = Element::new("w:rPr");
        if let Some(style) = &self.style {
            rpr.push(Element::new("w:rStyle").attr("w:val", style_id(style.as_ref()).as_ref()));
        }
        // The East Asian font hint follows any character style but precedes the weight and slant
        // toggles, keeping the property order the schema fixes.
        if self.east_asian {
            rpr.push(Element::new("w:rFonts").attr("w:hint", "eastAsia"));
        }
        if self.bold {
            rpr.push(Element::new("w:b"));
            rpr.push(Element::new("w:bCs"));
        }
        if self.italic {
            rpr.push(Element::new("w:i"));
            rpr.push(Element::new("w:iCs"));
        }
        if self.smallcaps {
            rpr.push(Element::new("w:smallCaps"));
        }
        if self.strike {
            rpr.push(Element::new("w:strike"));
        }
        if self.underline {
            rpr.push(Element::new("w:u").attr("w:val", "single"));
        }
        if let Some(value) = self.vert_align {
            rpr.push(Element::new("w:vertAlign").attr("w:val", value));
        }
        Some(rpr)
    }
}

/// A `w:r` run carrying `props`' `w:rPr`, if any, ready for its content to be pushed.
fn run_with_props(props: &RunProps) -> Element {
    let mut run = Element::new("w:r");
    if let Some(rpr) = props.element() {
        run.push(rpr);
    }
    run
}

/// A text run carrying the given properties, its whitespace preserved.
fn text_run(props: &RunProps, text: &str) -> Element {
    let mut run = run_with_props(props);
    let tag = if props.deletion { "w:delText" } else { "w:t" };
    run.push(Element::new(tag).attr("xml:space", "preserve").text(text));
    run
}

/// A run holding a single line break.
fn break_run(props: &RunProps) -> Element {
    let mut run = run_with_props(props);
    run.push(Element::new("w:br"));
    run
}

/// Flushes an accumulated text buffer as one run, if it holds anything, carrying the East Asian hint
/// its content called for. The hint is reset ready for the next buffer.
fn flush_text(buffer: &mut String, hint: &mut bool, props: &RunProps, out: &mut Element) {
    if !buffer.is_empty() {
        out.push(text_run(&props.with_east_asian(*hint), buffer));
        buffer.clear();
    }
    *hint = false;
}

/// Whether a character calls for the East Asian font hint on its run. The recognized ranges are the
/// Han ideographs and the Yi, compatibility, half- and full-width, and supplementary ideographic
/// blocks; the kana, Hangul and bopomofo scripts are left to the default font.
fn is_east_asian(c: char) -> bool {
    matches!(
        u32::from(c),
        0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFFEE
            | 0x2_0000..=0x2_A6DF
            | 0x2_A700..=0x2_EBEF
            | 0x2_F800..=0x2_FA1F
            | 0x3_0000..=0x3_134A
    )
}

/// Lowers an inline sequence to runs. Consecutive text pieces gather into one run, split where the
/// text crosses into or out of an East Asian script so that only the East Asian stretch carries the
/// font hint; a single space between two text pieces joins the non-East-Asian side, while a space at
/// a formatting boundary, one between two East Asian stretches, and every soft break become their own
/// run; a formatted span recurses with the corresponding property set; a footnote drops a numbered
/// mark and queues its body; an image resolves to a drawing or degrades to its text; and constructs
/// without a run form degrade to the text they carry.
#[allow(clippy::too_many_lines)]
fn render_runs(inlines: &[Inline], props: &RunProps, ctx: &mut Ctx, out: &mut Element) {
    let mut buffer = String::new();
    // Whether the buffered text so far is East Asian, so a run of one kind flushes before the other
    // begins.
    let mut buffer_hint = false;
    let mut index = 0;
    while let Some(inline) = inlines.get(index) {
        match inline {
            // Consecutive text pieces with nothing between them share a run; the run is East Asian
            // when any of its characters call for the hint. A change of kind ends the buffered run.
            Inline::Str(_) => {
                let mut text = String::new();
                while let Some(Inline::Str(piece)) = inlines.get(index) {
                    text.push_str(piece);
                    index += 1;
                }
                let hint = text.chars().any(is_east_asian);
                if !buffer.is_empty() && buffer_hint != hint {
                    flush_text(&mut buffer, &mut buffer_hint, props, out);
                }
                if buffer.is_empty() {
                    buffer_hint = hint;
                }
                buffer.push_str(&text);
                continue;
            }
            // A space flanked on both sides by text joins the non-East-Asian side of it: it stays
            // with a non-East-Asian run before it, else opens the non-East-Asian run after it, and
            // when East Asian text sits on both sides it stands alone. Against a formatting boundary
            // (or with nothing before it) it stands alone too.
            Inline::Space => {
                let next_hint = match inlines.get(index + 1) {
                    Some(Inline::Str(piece)) => Some(piece.chars().any(is_east_asian)),
                    _ => None,
                };
                if !buffer.is_empty() && next_hint.is_some() {
                    if !buffer_hint {
                        buffer.push(' ');
                    } else if next_hint == Some(false) {
                        flush_text(&mut buffer, &mut buffer_hint, props, out);
                        buffer.push(' ');
                    } else {
                        flush_text(&mut buffer, &mut buffer_hint, props, out);
                        out.push(text_run(props, " "));
                    }
                } else {
                    flush_text(&mut buffer, &mut buffer_hint, props, out);
                    out.push(text_run(props, " "));
                }
            }
            Inline::SoftBreak => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                out.push(text_run(props, " "));
            }
            Inline::LineBreak => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                out.push(break_run(props));
            }
            Inline::Emph(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_italic(), ctx, out);
            }
            Inline::Strong(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_bold(), ctx, out);
            }
            Inline::Underline(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_underline(), ctx, out);
            }
            Inline::Strikeout(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_strike(), ctx, out);
            }
            Inline::SmallCaps(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_smallcaps(), ctx, out);
            }
            Inline::Superscript(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_vert_align("superscript"), ctx, out);
            }
            Inline::Subscript(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_vert_align("subscript"), ctx, out);
            }
            Inline::Code(attr, text) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                #[cfg(feature = "highlight")]
                let highlighted =
                    highlighted_inline_runs(attr, text, &ctx.highlighter, props.deletion);
                #[cfg(not(feature = "highlight"))]
                let highlighted: Option<Vec<Element>> = {
                    let _ = attr;
                    None
                };
                if let Some(runs) = highlighted {
                    for run in runs {
                        out.push(run);
                    }
                } else {
                    let hint = text.chars().any(is_east_asian);
                    out.push(text_run(
                        &props.with_style("VerbatimChar").with_east_asian(hint),
                        text,
                    ));
                }
            }
            // The quotation glyphs join their inner text so a quoted word renders as one run.
            Inline::Quoted(kind, children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                let (open, close) = quotation_marks(kind);
                let mut quoted = Vec::with_capacity(children.len() + 2);
                quoted.push(Inline::Str(open.into()));
                quoted.extend(children.iter().cloned());
                quoted.push(Inline::Str(close.into()));
                render_runs(&quoted, props, ctx, out);
            }
            // Without citation processing a citation renders as the source text it was written as.
            Inline::Cite(_, source) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(source, props, ctx, out);
            }
            // A span contributes its inline content, drawing a `custom-style` attribute as the
            // character style on its runs. A comment range is a span pair: the opening marker holds
            // the comment's text and author, which move to the comments part, and both markers drop
            // range boundaries into the flow rather than any visible text.
            Inline::Span(attr, children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                if has_class(attr, "comment-start") {
                    open_comment(attr, children, out, ctx);
                } else if has_class(attr, "comment-end") {
                    close_comment(attr, props, out);
                } else if has_class(attr, "insertion") {
                    let id = ctx.next_insertion;
                    ctx.next_insertion = ctx.next_insertion.saturating_add(1);
                    let mut insertion = tracked_change("w:ins", id, attr);
                    render_runs(children, props, ctx, &mut insertion);
                    out.push(insertion);
                } else if has_class(attr, "deletion") {
                    let id = ctx.next_deletion;
                    ctx.next_deletion = ctx.next_deletion.saturating_add(1);
                    let mut deletion = tracked_change("w:del", id, attr);
                    render_runs(children, &props.with_deletion(), ctx, &mut deletion);
                    out.push(deletion);
                } else {
                    let mark = open_bookmark(attr.id.as_str(), out, ctx);
                    match custom_style(attr) {
                        Some(name) => {
                            render_runs(children, &props.with_style(name.to_owned()), ctx, out);
                        }
                        None => render_runs(children, props, ctx, out),
                    }
                    close_bookmark(mark, out);
                }
            }
            // A link wraps its content in a hyperlink whose runs carry the link character style. A
            // destination beginning with '#' points at the in-document bookmark of that name; any
            // other destination is reached through an external relationship. Nested content is
            // rendered before this link claims its own relationship id, so an inner link is numbered
            // ahead of the outer one it sits inside.
            Inline::Link(_, children, target) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                let mut hyperlink = Element::new("w:hyperlink");
                render_runs(
                    children,
                    &props.with_style("Hyperlink"),
                    ctx,
                    &mut hyperlink,
                );
                hyperlink = if let Some(anchor) = target.url.strip_prefix('#') {
                    hyperlink.attr("w:anchor", clean_bookmark_name(anchor).as_ref())
                } else {
                    let rel_id = ctx.hyperlink_rel(target.url.as_str());
                    hyperlink.attr("r:id", &format!("rId{rel_id}"))
                };
                out.push(hyperlink);
            }
            // An image resolves to a sized drawing when its bytes are on hand, and degrades to its
            // descriptive text otherwise.
            Inline::Image(attr, alt, target) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                match image_drawing_for(attr, target, alt, ctx) {
                    Some(drawing) => {
                        let mut run = run_with_props(props);
                        run.push(drawing);
                        out.push(run);
                    }
                    None => render_runs(alt, props, ctx, out),
                }
            }
            // Math lowers to an Office Math fragment set directly among the runs; when its source
            // has no renderable form it degrades to the delimited literal source.
            Inline::Math(kind, source) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                let display = matches!(kind, MathType::DisplayMath);
                if let Some(fragment) = crate::math::to_omml(source, display) {
                    out.push_raw(&fragment);
                } else {
                    let delimiter = if display { "$$" } else { "$" };
                    out.push(text_run(props, &format!("{delimiter}{source}{delimiter}")));
                }
            }
            // An `openxml` raw payload is emitted verbatim; passthrough in any other format has no
            // run form and contributes nothing. Either way it bounds the surrounding runs.
            Inline::RawInline(format, payload) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                if format.0.as_str() == "openxml" {
                    out.push_raw(payload);
                }
            }
            // A footnote drops a numbered mark here and queues its block content for the notes part.
            Inline::Note(blocks) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                let id = ctx.next_id;
                ctx.next_id = ctx.next_id.saturating_add(1);
                ctx.notes.push((id, blocks.clone()));
                out.push(footnote_reference_run(id, props));
            }
        }
        index += 1;
    }
    flush_text(&mut buffer, &mut buffer_hint, props, out);
}

/// Resolves an image reference to a sized drawing, recording the picture in the media set. The bytes
/// come from the media set when the reference names an entry there, or from the reference itself when
/// it is a `data:` URI carrying an embedded payload; any other reference whose bytes are not on hand
/// returns `None`.
fn image_drawing_for(
    attr: &Attr,
    target: &Target,
    alt: &[Inline],
    ctx: &mut Ctx,
) -> Option<Element> {
    let (bytes, mime) = match ctx.media.get(target.url.as_str()) {
        Some(item) => {
            let mime = item
                .mime
                .clone()
                .unwrap_or_else(|| mime_from_url(target.url.as_str()));
            (item.bytes.clone(), mime)
        }
        None => carta_core::media::decode_data_uri(target.url.as_str())?,
    };
    let (cx, cy) = image_extent(&bytes, &attr.attributes);

    let rel_id = ctx.next_id;
    ctx.next_id = ctx.next_id.saturating_add(3);
    let extension = extension_for_mime(&mime);
    let file_name = format!("rId{rel_id}.{extension}");
    let alt_text = carta_ast::to_plain_text(alt);
    let drawing = image_drawing(
        rel_id,
        cx,
        cy,
        target.url.as_str(),
        &alt_text,
        target.title.as_str(),
    );
    ctx.images.push(ImageMedia {
        rel_id,
        file_name,
        mime,
        bytes,
    });
    Some(drawing)
}

/// The drawn size of an image in English metric units. A requested width and height, given in pixels
/// or an absolute unit, map through a 96-dpi baseline; when only one is given the other follows from
/// the natural aspect ratio. With neither given, the natural pixel size maps through the image's own
/// resolution, so a picture that records a higher dpi draws correspondingly smaller.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // Image dimensions stay well inside range.
fn image_extent(bytes: &[u8], attributes: &[(carta_ast::Text, carta_ast::Text)]) -> (i64, i64) {
    const EMU_PER_PX: f64 = 9525.0;
    const EMU_PER_INCH: i64 = 914_400;
    // The page's text width: the width a picture requested as a fraction of the page is measured
    // against, and never drawn wider than.
    const DEFAULT_WIDTH: f64 = 5_334_000.0;
    let (natural_px_w, natural_px_h) = image_dimensions(bytes);
    let natural_w = f64::from(natural_px_w);
    let natural_h = f64::from(natural_px_h);
    // A percentage width scales the page's text width, capped at the picture's own natural width, and
    // the height follows the natural aspect ratio; any height request is disregarded in this case.
    if let Some(fraction) = dimension_fraction(attributes, "width") {
        let (dpi_x, _) = image_dpi(bytes);
        let intrinsic_w = i64::from(natural_px_w) * EMU_PER_INCH / i64::from(dpi_x.max(1));
        let scaled = (fraction * DEFAULT_WIDTH).round() as i64;
        let cx = if natural_px_w > 0 {
            intrinsic_w.min(scaled)
        } else {
            scaled
        };
        let cy = if natural_w > 0.0 {
            (cx as f64 * natural_h / natural_w).round() as i64
        } else {
            scaled
        };
        return (cx, cy);
    }
    let requested_w = dimension_px(attributes, "width");
    let requested_h = dimension_px(attributes, "height");
    let px_to_emu = |value: f64| (value * EMU_PER_PX).round() as i64;
    match (requested_w, requested_h) {
        (Some(w), Some(h)) => (px_to_emu(w), px_to_emu(h)),
        (Some(w), None) if natural_w > 0.0 => (px_to_emu(w), px_to_emu(w * natural_h / natural_w)),
        (Some(w), None) => (px_to_emu(w), px_to_emu(natural_h)),
        (None, Some(h)) if natural_h > 0.0 => (px_to_emu(h * natural_w / natural_h), px_to_emu(h)),
        (None, Some(h)) => (px_to_emu(natural_w), px_to_emu(h)),
        (None, None) => {
            let (dpi_x, dpi_y) = image_dpi(bytes);
            let dpi_x = i64::from(dpi_x.max(1));
            let dpi_y = i64::from(dpi_y.max(1));
            (
                i64::from(natural_px_w) * EMU_PER_INCH / dpi_x,
                i64::from(natural_px_h) * EMU_PER_INCH / dpi_y,
            )
        }
    }
}

/// A pixel dimension read from an attribute, when it is given in pixels or an absolute unit. A
/// percentage or unknown unit yields `None`, leaving the natural size to stand.
fn dimension_px(attributes: &[(carta_ast::Text, carta_ast::Text)], key: &str) -> Option<f64> {
    let raw = attributes
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, value)| value.as_str())?;
    let trimmed = raw.trim();
    let split = trimmed
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let value: f64 = number.parse().ok()?;
    match unit.trim() {
        "" | "px" => Some(value),
        "in" => Some(value * 96.0),
        "cm" => Some(value * 96.0 / 2.54),
        "mm" => Some(value * 96.0 / 25.4),
        "pt" => Some(value * 96.0 / 72.0),
        "pc" => Some(value * 16.0),
        _ => None,
    }
}

/// A dimension given as a percentage, returned as a fraction (so `50%` yields `0.5`). Any value that
/// is not a percentage yields `None`.
fn dimension_fraction(attributes: &[(carta_ast::Text, carta_ast::Text)], key: &str) -> Option<f64> {
    let raw = attributes
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, value)| value.as_str())?;
    let number = raw.trim().strip_suffix('%')?;
    let value: f64 = number.trim().parse().ok()?;
    Some(value / 100.0)
}

/// A MIME type guessed from a URL's file extension, for a media entry that recorded none.
fn mime_from_url(url: &str) -> String {
    carta_core::media::image_mime_for_extension(url)
        .unwrap_or("application/octet-stream")
        .to_owned()
}

/// The inline drawing for an embedded picture: an anchored image sized by `cx`×`cy` EMU, referencing
/// the relationship the media entry was stored under. The picture's own description records the
/// `source` the reference was written as, while the drawing's description carries the alt text.
fn image_drawing(
    rel_id: u32,
    cx: i64,
    cy: i64,
    source: &str,
    description: &str,
    title: &str,
) -> Element {
    let doc_id = rel_id.saturating_add(1);
    let picture_id = rel_id.saturating_add(2);
    let cx = cx.to_string();
    let cy = cy.to_string();

    let blip_fill = Element::new("pic:blipFill")
        .child(Element::new("a:blip").attr("r:embed", &format!("rId{rel_id}")))
        .child(Element::new("a:stretch").child(Element::new("a:fillRect")));

    let shape_props = Element::new("pic:spPr")
        .attr("bwMode", "auto")
        .child(
            Element::new("a:xfrm")
                .child(Element::new("a:off").attr("x", "0").attr("y", "0"))
                .child(Element::new("a:ext").attr("cx", &cx).attr("cy", &cy)),
        )
        .child(Element::new("a:prstGeom").attr("prst", "rect").child(Element::new("a:avLst")))
        .child(Element::new("a:noFill"))
        // An explicit empty outline bounds the picture's shape so a viewer draws no stray border.
        .child(
            Element::new("a:ln")
                .attr("w", "9525")
                .child(Element::new("a:noFill"))
                .child(Element::new("a:headEnd"))
                .child(Element::new("a:tailEnd")),
        );

    let picture = Element::new("pic:pic")
        .child(
            Element::new("pic:nvPicPr")
                .child(
                    Element::new("pic:cNvPr")
                        .attr("id", &picture_id.to_string())
                        .attr("name", "Picture")
                        .attr("descr", source),
                )
                .child(
                    Element::new("pic:cNvPicPr").child(
                        Element::new("a:picLocks")
                            .attr("noChangeArrowheads", "1")
                            .attr("noChangeAspect", "1"),
                    ),
                ),
        )
        .child(blip_fill)
        .child(shape_props);

    let graphic = Element::new("a:graphic").child(
        Element::new("a:graphicData")
            .attr(
                "uri",
                "http://schemas.openxmlformats.org/drawingml/2006/picture",
            )
            .child(picture),
    );

    let inline = Element::new("wp:inline")
        .child(Element::new("wp:extent").attr("cx", &cx).attr("cy", &cy))
        // A zero effect extent declares the drawing claims no space beyond its own bounds.
        .child(
            Element::new("wp:effectExtent")
                .attr("l", "0")
                .attr("t", "0")
                .attr("r", "0")
                .attr("b", "0"),
        )
        .child(
            Element::new("wp:docPr")
                .attr("id", &doc_id.to_string())
                .attr("name", "Picture")
                .attr("descr", description)
                .attr("title", title),
        )
        .child(graphic);

    Element::new("w:drawing").child(inline)
}

/// The opening and closing glyphs for a quotation kind.
fn quotation_marks(kind: &QuoteType) -> (&'static str, &'static str) {
    match kind {
        QuoteType::SingleQuote => ("\u{2018}", "\u{2019}"),
        QuoteType::DoubleQuote => ("\u{201c}", "\u{201d}"),
    }
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
