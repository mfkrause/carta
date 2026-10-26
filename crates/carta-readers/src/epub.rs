//! Read EPUB packages, combining chapter content and embedded resources.

use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{
    Attr, Block, BlockParts, Document, Format, Inline, InlineParts, MetaValue, Target, Text,
};
use carta_core::container::zip;
use carta_core::walk::{for_each_image_target, for_each_link_target};
use carta_core::{
    BytesReader, DeepStack, Error, MediaBag, Reader, ReaderOptions, Result, on_deep_stack,
};

use crate::html::{HtmlReader, escape_uri};
use crate::xml::{self, Element, local_name};

/// The attribute name carrying an element's structural role within the publication.
const ROLE_ATTR: &str = "epub:type";
/// Roles whose `<section>`/`<aside>` container is flattened, its content promoted in place.
const FLATTEN_ROLES: [&str; 2] = ["chapter", "subchapter"];
/// Page roles whose `<section>`/`<div>` container is dropped from the block flow.
const PAGE_DROP_ROLES: [&str; 2] = ["titlepage", "halftitlepage"];
/// The navigation role dropped from the block flow whatever element carries it.
const NAV_DROP_ROLE: &str = "toc";
/// Roles marking a note whose content is lifted to the reference pointing to it.
const NOTE_ROLES: [&str; 2] = ["footnote", "rearnote"];
/// The role marking a reference whose same-file target note is inlined in its place.
const NOTEREF_ROLE: &str = "noteref";
/// The raw-inline format tag for a note reference left unresolved because it sits inside a note
/// body, where resolution does not recurse.
const NOTEREF_FORMAT: &str = "noteref";
/// The path of the archive entry that names the package document.
const CONTAINER_PATH: &str = "META-INF/container.xml";
/// Bound on XML nesting depth, so a pathologically deep package document cannot overflow the stack.
const MAX_XML_DEPTH: usize = 256;

/// Parses an EPUB package into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct EpubReader;

impl BytesReader for EpubReader {
    fn read(&self, input: &[u8], options: &ReaderOptions) -> Result<Document> {
        Ok(self.read_media(input, options)?.0)
    }

    fn read_media(&self, input: &[u8], options: &ReaderOptions) -> Result<(Document, MediaBag)> {
        // Nested markup recurses in decoding and stitching; a large-stack thread prevents overflow.
        match on_deep_stack(|| read_package(input, options)) {
            DeepStack::Completed(result) => result,
            DeepStack::Panicked => Err(Error::Container("worker thread failed".into())),
            DeepStack::NotSpawned => Err(Error::Container(
                "worker thread could not be spawned".into(),
            )),
        }
    }
}

/// Parses an EPUB package into the document model and its out-of-band media.
fn read_package(input: &[u8], options: &ReaderOptions) -> Result<(Document, MediaBag)> {
    let files = zip::read_map(input)?;

    let opf_path = locate_package(&files)?;
    let opf_dir = parent(&opf_path).to_string();
    let opf_bytes = files
        .get(&opf_path)
        .ok_or_else(|| Error::Container(format!("package document {opf_path} not found")))?;
    let package = xml::parse(opf_bytes, MAX_XML_DEPTH)
        .ok_or_else(|| Error::Container("package document is not well-formed XML".into()))?;

    let meta = build_meta(&package);
    let manifest = build_manifest(&package);
    let spine = resolve_spine(&package, &manifest, &opf_dir, &files);
    let known: BTreeSet<String> = spine.iter().map(|doc| doc.basename.clone()).collect();

    let mut media = MediaBag::new();
    let mut blocks = Vec::new();
    if let Some(href) = cover_image(&package, &manifest) {
        blocks.push(cover_block(&href, &opf_dir, &files, &manifest, &mut media));
    }
    for doc in &spine {
        let Some(bytes) = files.get(&doc.path) else {
            continue;
        };
        let text = String::from_utf8_lossy(bytes);
        let parsed = HtmlReader.read(text.as_ref(), options)?;
        let cleaned = drop_toc_nav(parsed.blocks);
        let mut notes = Notes::new();
        let stripped = collect_notes(cleaned, &mut notes);
        let ctx = TransformCtx {
            basename: &doc.basename,
            notes: &notes,
            resolve: true,
        };
        let mut body = transform_blocks(stripped, ctx);
        let doc_dir = parent(&doc.path).to_string();
        rewrite_links(&mut body, &doc.basename, &doc_dir, &known);
        rewrite_images(&mut body, &doc_dir, &opf_dir, &manifest, &files, &mut media);
        blocks.push(anchor(&doc.basename));
        blocks.append(&mut body);
    }

    let document = Document {
        meta,
        blocks,
        ..Document::default()
    };
    Ok((document, media))
}

/// The package document path named by the archive's container entry.
fn locate_package(files: &BTreeMap<String, Vec<u8>>) -> Result<String> {
    let bytes = files
        .get(CONTAINER_PATH)
        .ok_or_else(|| Error::Container(format!("{CONTAINER_PATH} not found")))?;
    let container = xml::parse(bytes, MAX_XML_DEPTH)
        .ok_or_else(|| Error::Container(format!("{CONTAINER_PATH} is not well-formed XML")))?;
    container
        .child("rootfiles")
        .and_then(|rootfiles| rootfiles.child("rootfile"))
        .and_then(|rootfile| rootfile.attr("full-path"))
        .map(str::to_string)
        .ok_or_else(|| Error::Container(format!("{CONTAINER_PATH} names no package document")))
}

/// One manifest entry: the file it points at and the media type declared for it.
struct ManifestItem {
    href: String,
    media_type: Option<String>,
}

/// The manifest, keyed by item id.
type Manifest = BTreeMap<String, ManifestItem>;

fn build_manifest(package: &Element) -> Manifest {
    let mut manifest = Manifest::new();
    if let Some(node) = package.child("manifest") {
        for item in node.elements().filter(|el| local_name(&el.name) == "item") {
            if let (Some(id), Some(href)) = (item.attr("id"), item.attr("href")) {
                manifest.insert(
                    id.to_string(),
                    ManifestItem {
                        href: href.to_string(),
                        media_type: item.attr("media-type").map(str::to_string),
                    },
                );
            }
        }
    }
    manifest
}

/// One resolved spine document, ready to decode. Its bytes are fetched from the archive at decode
/// time rather than held here, so the whole reading order is not copied up front.
struct SpineDoc {
    /// The document's path within the archive.
    path: String,
    /// The document's file name, used to namespace its identifiers and as its anchor.
    basename: String,
}

/// The reading-order documents, in order, skipping non-linear items and any that cannot be resolved.
fn resolve_spine(
    package: &Element,
    manifest: &Manifest,
    opf_dir: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Vec<SpineDoc> {
    let mut docs = Vec::new();
    let Some(spine) = package.child("spine") else {
        return docs;
    };
    for itemref in spine
        .elements()
        .filter(|el| local_name(&el.name) == "itemref")
    {
        if itemref.attr("linear") == Some("no") {
            continue;
        }
        let Some(idref) = itemref.attr("idref") else {
            continue;
        };
        let Some(item) = manifest.get(idref) else {
            continue;
        };
        // `%XX` escapes decode before the href names an archive entry; the anchor keeps its spelling.
        let path = join_norm(opf_dir, &percent_decode(&item.href));
        if !files.contains_key(&path) {
            continue;
        }
        docs.push(SpineDoc {
            basename: file_name(&item.href).to_string(),
            path,
        });
    }
    docs
}

/// The anchor block placed before a document's content so cross-file links can target the file.
fn anchor(basename: &str) -> Block {
    Block::Para(vec![Inline::Span(
        Box::new(Attr {
            id: basename.into(),
            ..Attr::default()
        }),
        Vec::new(),
    )])
}

/// Reads the package metadata into the document metadata map.
///
/// Each Dublin Core element contributes its text under its local name (a creator under `author`)
/// with values for a repeated field held newest-first. A field with one value is inline text; a
/// field with several is a list.
fn build_meta(package: &Element) -> BTreeMap<Text, MetaValue> {
    let mut collected: BTreeMap<String, Vec<Vec<Inline>>> = BTreeMap::new();
    if let Some(metadata) = package.child("metadata") {
        for element in metadata.elements() {
            let Some(local) = element.name.strip_prefix("dc:") else {
                continue;
            };
            let key = if local == "creator" { "author" } else { local };
            let value = vec![Inline::Str(element.text().into())];
            collected.entry(key.to_string()).or_default().push(value);
        }
    }

    let mut meta = BTreeMap::new();
    for (key, mut values) in collected {
        values.reverse();
        let value = if values.len() == 1 {
            MetaValue::MetaInlines(values.into_iter().next().unwrap_or_default())
        } else {
            MetaValue::MetaList(values.into_iter().map(MetaValue::MetaInlines).collect())
        };
        meta.insert(key.into(), value);
    }
    meta
}

/// The cover image's package-relative href, if the publication declares one. An EPUB3 publication
/// flags the manifest item with `properties="cover-image"`; an EPUB2 publication names the cover
/// item by id in a `<meta name="cover">`. When both are present they name the same file.
fn cover_image(package: &Element, manifest: &Manifest) -> Option<String> {
    if let Some(node) = package.child("manifest") {
        for item in node.elements().filter(|el| local_name(&el.name) == "item") {
            let flagged = item
                .attr("properties")
                .is_some_and(|props| props.split_whitespace().any(|prop| prop == "cover-image"));
            if flagged && let Some(href) = item.attr("href") {
                return Some(href.to_string());
            }
        }
    }
    let cover_id = package
        .child("metadata")?
        .elements()
        .filter(|el| local_name(&el.name) == "meta")
        .find(|meta| meta.attr("name") == Some("cover"))
        .and_then(|meta| meta.attr("content"))?;
    manifest.get(cover_id).map(|item| item.href.clone())
}

/// The leading block for a cover image: a paragraph wrapping the image, referenced by its
/// package-relative href. When the file is present in the archive its bytes are carried into the
/// media bag under the same name.
fn cover_block(
    href: &str,
    opf_dir: &str,
    files: &BTreeMap<String, Vec<u8>>,
    manifest: &Manifest,
    media: &mut MediaBag,
) -> Block {
    let path = join_norm(opf_dir, href);
    if let Some(bytes) = files.get(&path) {
        let media_type = manifest
            .values()
            .find(|item| item.href == href)
            .and_then(|item| item.media_type.clone());
        media.insert(href.to_string(), media_type, bytes.clone());
    }
    Block::Para(vec![Inline::Image(
        Box::default(),
        Vec::new(),
        Box::new(Target {
            url: href.into(),
            title: Text::default(),
        }),
    )])
}

/// The container role handling for a `Div`, decided from its `epub:type` and source element.
enum DivKind {
    /// Keep the container, folding its role into classes.
    Keep,
    /// Remove the container and its content from the block flow.
    Drop,
    /// Remove the container but promote its content in place.
    Flatten,
}

/// Same-file notes lifted from their containers, keyed by the identifier a reference targets.
type Notes = BTreeMap<String, Vec<Block>>;

/// Remove the table-of-contents navigation from a spine document's blocks. A `<nav epub:type="toc">`
/// has no structural mapping, so the XHTML decoder emits it as a raw start-tag block, its list, and a
/// raw end-tag block; that whole run is the generated contents page and is dropped so it does not
/// appear inline. Other navigation (landmarks, page lists) is left untouched, and containers are
/// searched recursively in case the navigation is nested inside one.
fn drop_toc_nav(blocks: Vec<Block>) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    // While positive, the number of open `<nav>` start tags whose run is being dropped.
    let mut dropping = 0usize;
    for block in blocks {
        if dropping > 0 {
            if is_nav_open(&block) {
                dropping += 1;
            } else if is_nav_close(&block) {
                dropping -= 1;
            }
            continue;
        }
        if is_toc_nav_open(&block) {
            dropping = 1;
            continue;
        }
        out.push(descend_toc_nav(block));
    }
    out
}

/// Apply [`drop_toc_nav`] to a block's own children, so a navigation nested inside a container is
/// removed too.
fn descend_toc_nav(block: Block) -> Block {
    match block.into_parts() {
        BlockParts::Div(attr, inner) => Block::Div(attr, drop_toc_nav(inner)),
        BlockParts::BlockQuote(inner) => Block::BlockQuote(drop_toc_nav(inner)),
        BlockParts::Figure(attr, caption, inner) => {
            Block::Figure(attr, caption, drop_toc_nav(inner))
        }
        BlockParts::BulletList(items) => {
            Block::BulletList(items.into_iter().map(drop_toc_nav).collect())
        }
        BlockParts::OrderedList(attr, items) => {
            Block::OrderedList(attr, items.into_iter().map(drop_toc_nav).collect())
        }
        other => other.into(),
    }
}

/// The text of a raw `html` block, if `block` is one.
fn raw_html_block(block: &Block) -> Option<&str> {
    match block {
        Block::RawBlock(format, text) if format.0 == "html" => Some(text.as_str()),
        _ => None,
    }
}

/// Whether `block` is a raw `<nav …>` start tag.
fn is_nav_open(block: &Block) -> bool {
    raw_html_block(block).is_some_and(is_nav_open_tag)
}

/// Whether `block` is a raw `</nav>` end tag.
fn is_nav_close(block: &Block) -> bool {
    raw_html_block(block).is_some_and(|tag| tag.trim().eq_ignore_ascii_case("</nav>"))
}

/// Whether `block` is a raw `<nav …>` start tag whose `epub:type` marks it as the contents page.
fn is_toc_nav_open(block: &Block) -> bool {
    raw_html_block(block).is_some_and(|tag| is_nav_open_tag(tag) && nav_type_is_toc(tag))
}

fn is_nav_open_tag(tag: &str) -> bool {
    tag.strip_prefix("<nav").is_some_and(
        |rest| matches!(rest.chars().next(), Some(c) if c.is_ascii_whitespace() || c == '>'),
    )
}

/// Whether a serialized `<nav>` start tag's `epub:type` is exactly the contents-page role. The match
/// is exact: a value that merely lists the role among others, or differs in case or surrounding
/// space, marks a different kind of navigation that stays in the flow.
fn nav_type_is_toc(tag: &str) -> bool {
    tag.split_once("epub:type=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .is_some_and(|(value, _)| value == NAV_DROP_ROLE)
}

/// Transform decoded EPUB content, resolving notes and namespacing identifiers.
///
/// Clear `resolve` inside note bodies: nested references remain raw and cycles terminate.
#[derive(Clone, Copy)]
struct TransformCtx<'a> {
    basename: &'a str,
    notes: &'a Notes,
    resolve: bool,
}

impl TransformCtx<'_> {
    /// The same context with note-reference resolution set to `resolve`.
    fn with_resolve(self, resolve: bool) -> Self {
        Self { resolve, ..self }
    }
}

fn transform_blocks(blocks: Vec<Block>, ctx: TransformCtx) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        transform_into(block, ctx, &mut out);
    }
    out
}

fn transform_into(block: Block, ctx: TransformCtx, out: &mut Vec<Block>) {
    match block.into_parts() {
        BlockParts::Div(attr, inner) => match classify_div(&attr) {
            DivKind::Drop => {}
            DivKind::Flatten => {
                for child in inner {
                    transform_into(child, ctx, out);
                }
            }
            DivKind::Keep => {
                let mut attr = attr;
                normalize(&mut attr, ctx.basename);
                out.push(Block::Div(attr, transform_blocks(inner, ctx)));
            }
        },
        BlockParts::Header(level, mut attr, inlines) => {
            normalize(&mut attr, ctx.basename);
            out.push(Block::Header(level, attr, transform_inlines(inlines, ctx)));
        }
        BlockParts::CodeBlock(mut attr, code) => {
            normalize(&mut attr, ctx.basename);
            out.push(Block::CodeBlock(attr, code));
        }
        BlockParts::Figure(mut attr, mut caption, inner) => {
            normalize(&mut attr, ctx.basename);
            caption.long = transform_blocks(std::mem::take(&mut caption.long), ctx);
            caption.short = caption
                .short
                .take()
                .map(|short| transform_inlines(short, ctx));
            out.push(Block::Figure(attr, caption, transform_blocks(inner, ctx)));
        }
        BlockParts::Table(mut table) => {
            normalize(&mut table.attr, ctx.basename);
            transform_table(table.as_mut(), ctx);
            out.push(Block::Table(table));
        }
        BlockParts::Plain(inlines) => {
            out.push(Block::Plain(transform_inlines(inlines, ctx)));
        }
        BlockParts::Para(inlines) => {
            out.push(Block::Para(transform_inlines(inlines, ctx)));
        }
        BlockParts::LineBlock(lines) => out.push(Block::LineBlock(
            lines
                .into_iter()
                .map(|line| transform_inlines(line, ctx))
                .collect(),
        )),
        BlockParts::BlockQuote(inner) => {
            out.push(Block::BlockQuote(transform_blocks(inner, ctx)));
        }
        BlockParts::OrderedList(attrs, items) => {
            out.push(Block::OrderedList(attrs, transform_item_lists(items, ctx)));
        }
        BlockParts::BulletList(items) => {
            out.push(Block::BulletList(transform_item_lists(items, ctx)));
        }
        BlockParts::DefinitionList(items) => out.push(Block::DefinitionList(
            items
                .into_iter()
                .map(|(term, defs)| {
                    (
                        transform_inlines(term, ctx),
                        transform_item_lists(defs, ctx),
                    )
                })
                .collect(),
        )),
        other @ (BlockParts::RawBlock(..) | BlockParts::HorizontalRule) => out.push(other.into()),
    }
}

fn transform_item_lists(items: Vec<Vec<Block>>, ctx: TransformCtx) -> Vec<Vec<Block>> {
    items
        .into_iter()
        .map(|item| transform_blocks(item, ctx))
        .collect()
}

/// A mutable reference to every cell's block content across a table's head, bodies, and foot.
fn table_cell_contents(table: &mut carta_ast::Table) -> impl Iterator<Item = &mut Vec<Block>> {
    let bodies = table
        .bodies
        .iter_mut()
        .flat_map(|body| std::iter::once(&mut body.head).chain(std::iter::once(&mut body.body)));
    std::iter::once(&mut table.head.rows)
        .chain(bodies)
        .chain(std::iter::once(&mut table.foot.rows))
        .flatten()
        .flat_map(|row| row.cells.iter_mut())
        .map(|cell| &mut cell.content)
}

fn transform_table(table: &mut carta_ast::Table, ctx: TransformCtx) {
    for content in table_cell_contents(table) {
        *content = transform_blocks(std::mem::take(content), ctx);
    }
}

fn transform_inlines(inlines: Vec<Inline>, ctx: TransformCtx) -> Vec<Inline> {
    inlines
        .into_iter()
        .map(|inline| transform_inline(inline, ctx))
        .collect()
}

fn transform_inline(inline: Inline, ctx: TransformCtx) -> Inline {
    match inline.into_parts() {
        InlineParts::Span(mut attr, children) => {
            normalize(&mut attr, ctx.basename);
            Inline::Span(attr, transform_inlines(children, ctx))
        }
        InlineParts::Link(mut attr, children, target) => {
            if is_noteref(&attr)
                && let Some(id) = target.url.strip_prefix('#')
            {
                if ctx.resolve {
                    let content = ctx.notes.get(id).cloned().unwrap_or_default();
                    return Inline::Note(transform_blocks(content, ctx.with_resolve(false)));
                }
                return Inline::RawInline(Format(NOTEREF_FORMAT.into()), id.into());
            }
            normalize(&mut attr, ctx.basename);
            Inline::Link(attr, transform_inlines(children, ctx), target)
        }
        InlineParts::Image(mut attr, alt, target) => {
            normalize(&mut attr, ctx.basename);
            Inline::Image(attr, transform_inlines(alt, ctx), target)
        }
        InlineParts::Code(mut attr, code) => {
            normalize(&mut attr, ctx.basename);
            Inline::Code(attr, code)
        }
        InlineParts::Emph(children) => Inline::Emph(transform_inlines(children, ctx)),
        InlineParts::Underline(children) => Inline::Underline(transform_inlines(children, ctx)),
        InlineParts::Strong(children) => Inline::Strong(transform_inlines(children, ctx)),
        InlineParts::Strikeout(children) => Inline::Strikeout(transform_inlines(children, ctx)),
        InlineParts::Superscript(children) => Inline::Superscript(transform_inlines(children, ctx)),
        InlineParts::Subscript(children) => Inline::Subscript(transform_inlines(children, ctx)),
        InlineParts::SmallCaps(children) => Inline::SmallCaps(transform_inlines(children, ctx)),
        InlineParts::Quoted(kind, children) => {
            Inline::Quoted(kind, transform_inlines(children, ctx))
        }
        InlineParts::Cite(mut citations, children) => {
            for citation in &mut citations {
                let prefix = std::mem::take(&mut citation.prefix);
                let suffix = std::mem::take(&mut citation.suffix);
                citation.prefix = transform_inlines(prefix, ctx);
                citation.suffix = transform_inlines(suffix, ctx);
            }
            Inline::Cite(citations, transform_inlines(children, ctx))
        }
        InlineParts::Note(blocks) => {
            Inline::Note(transform_blocks(blocks, ctx.with_resolve(false)))
        }
        other @ (InlineParts::Str(_)
        | InlineParts::Space
        | InlineParts::SoftBreak
        | InlineParts::LineBreak
        | InlineParts::Math(..)
        | InlineParts::RawInline(..)) => other.into(),
    }
}

/// The value of the `epub:type` role attribute, if the element carries one.
fn role_value(attr: &Attr) -> Option<&str> {
    attr.attributes
        .iter()
        .find(|(key, _)| key.as_str() == ROLE_ATTR)
        .map(|(_, value)| value.as_str())
}

/// Whether any of an element's roles appears in `set`.
fn has_any_role(attr: &Attr, set: &[&str]) -> bool {
    role_value(attr).is_some_and(|value| value.split_whitespace().any(|role| set.contains(&role)))
}

/// Decides how a role-tagged `Div` is folded into the block flow. Flattening applies only to the
/// sectioning elements (`<section>`, `<aside>`), whose source name the reader records as the leading
/// class; page drops spare a sidebar `<aside>`, while a contents role is dropped whatever carries it.
fn classify_div(attr: &Attr) -> DivKind {
    let element = attr.classes.first().map(carta_ast::Text::as_str);
    if has_any_role(attr, &FLATTEN_ROLES) {
        return match element {
            Some("section" | "aside") => DivKind::Flatten,
            _ => DivKind::Keep,
        };
    }
    if has_any_role(attr, &[NAV_DROP_ROLE]) {
        return DivKind::Drop;
    }
    if has_any_role(attr, &PAGE_DROP_ROLES) {
        return match element {
            Some("aside") => DivKind::Keep,
            _ => DivKind::Drop,
        };
    }
    DivKind::Keep
}

/// Whether an element is a note container whose content is lifted to its reference.
fn is_note(attr: &Attr) -> bool {
    has_any_role(attr, &NOTE_ROLES)
}

/// Whether an element is a reference to a note.
fn is_noteref(attr: &Attr) -> bool {
    has_any_role(attr, &[NOTEREF_ROLE])
}

/// Lifts every note container out of the block flow, keyed by identifier so a reference can inline
/// it. A note without an identifier is unreferenceable, so it is dropped with its content.
fn collect_notes(blocks: Vec<Block>, notes: &mut Notes) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block.into_parts() {
            BlockParts::Div(attr, inner) if is_note(&attr) => {
                if !attr.id.is_empty() {
                    notes.entry(attr.id.to_string()).or_insert(inner);
                }
            }
            BlockParts::Div(attr, inner) => out.push(Block::Div(attr, collect_notes(inner, notes))),
            BlockParts::BlockQuote(inner) => {
                out.push(Block::BlockQuote(collect_notes(inner, notes)));
            }
            BlockParts::OrderedList(list_attr, items) => {
                out.push(Block::OrderedList(
                    list_attr,
                    collect_note_items(items, notes),
                ));
            }
            BlockParts::BulletList(items) => {
                out.push(Block::BulletList(collect_note_items(items, notes)));
            }
            BlockParts::Figure(attr, caption, inner) => {
                out.push(Block::Figure(attr, caption, collect_notes(inner, notes)));
            }
            BlockParts::DefinitionList(items) => {
                out.push(Block::DefinitionList(
                    items
                        .into_iter()
                        .map(|(term, defs)| (term, collect_note_items(defs, notes)))
                        .collect(),
                ));
            }
            BlockParts::Table(mut table) => {
                collect_notes_table(table.as_mut(), notes);
                out.push(Block::Table(table));
            }
            other => out.push(other.into()),
        }
    }
    out
}

fn collect_note_items(items: Vec<Vec<Block>>, notes: &mut Notes) -> Vec<Vec<Block>> {
    items
        .into_iter()
        .map(|item| collect_notes(item, notes))
        .collect()
}

/// Lifts note containers out of every cell of a table, so a footnote defined inside a cell is
/// collected just as one in the block flow is.
fn collect_notes_table(table: &mut carta_ast::Table, notes: &mut Notes) {
    for content in table_cell_contents(table) {
        *content = collect_notes(std::mem::take(content), notes);
    }
}

/// Folds a role attribute into classes and namespaces the identifier with `basename`.
fn normalize(attr: &mut Attr, basename: &str) {
    if let Some(position) = attr
        .attributes
        .iter()
        .position(|(key, _)| key.as_str() == ROLE_ATTR)
    {
        let (_, roles) = attr.attributes.remove(position);
        for role in roles.split_whitespace() {
            attr.classes.push(role.into());
        }
    }
    if !attr.id.is_empty() {
        attr.id = format!("{basename}_{}", attr.id).into();
    }
}

/// Rewrites intra-publication fragment links to the anchors the target files carry.
///
/// A same-file `#name` reference points at the current file's namespaced identifier; a
/// `file#name` reference at another reading-order file's. References outside the publication
/// (absolute URLs and links to files not in the reading order) are left untouched.
fn rewrite_links(blocks: &mut [Block], basename: &str, doc_dir: &str, known: &BTreeSet<String>) {
    for_each_link_target(blocks, &mut |target: &mut Target| {
        let url = target.url.as_str();
        if url.is_empty() || has_scheme(url) {
            return;
        }
        // Whole-file links use the file anchor; fragments use namespaced identifiers.
        let (path, fragment) = match url.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment)),
            None => (url, None),
        };
        let file = if path.is_empty() {
            basename.to_string()
        } else {
            file_name(&join_norm(doc_dir, path)).to_string()
        };
        if known.contains(&file) {
            target.url = match fragment {
                Some(fragment) => format!("#{file}_{fragment}"),
                None => format!("#{file}"),
            }
            .into();
        }
    });
}

/// Resolves image references against the archive, carrying found bytes into `media` and rewriting
/// each reference to its package-relative path.
fn rewrite_images(
    blocks: &mut [Block],
    doc_dir: &str,
    opf_dir: &str,
    manifest: &Manifest,
    files: &BTreeMap<String, Vec<u8>>,
    media: &mut MediaBag,
) {
    // Index the manifest by href for one-lookup media types; the first item per href wins.
    let mut media_types: BTreeMap<&str, Option<String>> = BTreeMap::new();
    for item in manifest.values() {
        media_types
            .entry(item.href.as_str())
            .or_insert_with(|| item.media_type.clone());
    }
    for_each_image_target(blocks, &mut |target: &mut Target| {
        let url = target.url.as_str();
        if url.is_empty() || has_scheme(url) {
            return;
        }
        // `%XX` decodes before naming the entry and bag key; the rewrite re-escapes to stay a valid URL.
        let path = join_norm(doc_dir, &percent_decode(url));
        let name = strip_prefix_dir(&path, opf_dir);
        // Bytes enter the bag only on first sight; repeats reuse the stored copy.
        if !media.contains(&name)
            && let Some(bytes) = files.get(&path)
        {
            let media_type = media_types.get(name.as_str()).cloned().flatten();
            media.insert(name.clone(), media_type, bytes.clone());
        }
        target.url = escape_uri(&name).into();
    });
}

/// Whether a URL carries an explicit scheme (`http:`, `mailto:`, `data:`), marking it as a reference
/// outside the archive that is passed through unchanged.
fn has_scheme(url: &str) -> bool {
    let mut chars = url.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for ch in chars {
        if ch == ':' {
            return true;
        }
        if !(ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-')) {
            return false;
        }
    }
    false
}

/// The final path segment.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Everything before the final path segment, or the empty string when there is none.
fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => path.get(..index).unwrap_or(""),
        None => "",
    }
}

/// Joins a base directory and a relative reference, then normalizes away `.` and `..` segments.
fn join_norm(dir: &str, rel: &str) -> String {
    let combined = if dir.is_empty() {
        rel.to_string()
    } else {
        format!("{dir}/{rel}")
    };
    let mut stack: Vec<&str> = Vec::new();
    for segment in combined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    stack.join("/")
}

/// Decodes the `%XX` escapes in a URL reference so it can be matched against an archive entry name,
/// which holds the unescaped path. Decoded bytes are read back as UTF-8; a `%` not followed by two
/// hexadecimal digits is left as written.
fn percent_decode(reference: &str) -> String {
    let bytes = reference.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte == b'%'
            && let Some(high) = bytes.get(i + 1).copied().and_then(hex_value)
            && let Some(low) = bytes.get(i + 2).copied().and_then(hex_value)
        {
            out.push(high * 16 + low);
            i += 3;
        } else {
            out.push(byte);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The value of a single hexadecimal digit byte.
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// A path made relative to `dir`, or returned unchanged when it does not lie under `dir`.
fn strip_prefix_dir(path: &str, dir: &str) -> String {
    if dir.is_empty() {
        return path.to_string();
    }
    path.strip_prefix(dir)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests;
