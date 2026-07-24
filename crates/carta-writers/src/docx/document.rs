//! Rendering of the document body (blocks and inlines) into `word/document.xml`, along with the
//! footnote entries, list plan and embedded images the body gives rise to.

use self::comments::render_comment_entry;
use self::figures::{render_figure, render_footnote_entry};
use self::lists::{render_bullet_list, render_definition_list, render_ordered_list};
use self::runs::{RunProps, code_paragraph, horizontal_rule, line_block_paragraph, render_runs};
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
/// `list_ambient` is the style a nested list's loose item paragraphs inherit from the container (a
/// block quote and a definition pass their own style down to the lists they hold, whereas a table
/// cell imposes none), mirroring the `ambient` a custom-style div threads through the top level.
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
        // Counts from 1, clear of the free-id range footnotes and images draw from.
        next_bookmark_id: 1,
        plan: ListPlan::default(),
        next_id: FIRST_FREE_ID,
        notes: Vec::new(),
        comments: Vec::new(),
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

    // Rendered before footnotes so a footnote inside a comment joins the queue below.
    let queued_comments = std::mem::take(&mut ctx.comments);
    let mut comments = Vec::with_capacity(queued_comments.len());
    for comment in &queued_comments {
        comments.push(render_comment_entry(comment, &mut ctx));
    }

    // A note nested in a note appends mid-loop, so the growing queue is revisited until drained.
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

/// Renders one top-level block, applying the first/body paragraph alternation and heading anchors.
/// `ambient` carries the paragraph style a surrounding custom-style div imposes on its direct
/// paragraphs; it is `None` in ordinary body flow.
fn render_top_block(block: &Block, body: &mut Element, ctx: &mut Ctx, ambient: Option<&str>) {
    match block {
        Block::Para(inlines) | Block::Plain(inlines) => {
            // A display equation lifts onto its own line; the paragraph splits around it.
            if ambient.is_none() && inlines.iter().any(is_display_equation) {
                render_split_paragraph(inlines, body, ctx);
                return;
            }
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
        // A div is transparent to the first/body alternation; a custom-style div restyles its
        // direct paragraphs (a nested one overrides).
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
        Block::CodeBlock(attr, code) => {
            let id = attr.id.clone();
            let para = code_paragraph(attr, code, None, &ctx.highlighter);
            push_anchored(body, ctx, id.as_str(), para);
            ctx.prev_paragraph = false;
        }
        // Raw markup neither opens nor closes the first/body alternation; only openxml passes through.
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
        // Display equations lift onto their own centred line, as at the top level.
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
        // A nested heading is not an outline section: styled paragraph, no bookmark.
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
        // A custom-style div restyles its paragraphs and loose lists; a plain one is transparent.
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
