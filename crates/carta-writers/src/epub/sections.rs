//! Structuring a document into the nested sections and separate chapter files an EPUB uses.
//!
//! An EPUB body is not a flat run of blocks: each heading and the content beneath it forms a
//! `<section>`, and the book is broken into separate XHTML files at a chosen heading level. This
//! module performs both transforms on the document model, before the HTML writer renders each
//! chapter.

use carta_ast::{Attr, Block, Inline, Text, slug, to_plain_text};

/// The `section` marker class an EPUB section wrapper carries; the HTML writer keys its
/// `<section>` rendering off it.
const SECTION_CLASS: &str = "section";

/// Transform a flat block sequence into the nested section structure an EPUB body uses. Each heading
/// and the blocks beneath it — up to the next heading of the same or a shallower level — become a
/// `Div` marked with the [`SECTION_CLASS`], a `level{N}` class, and the heading's own classes; the
/// heading's identifier moves onto that wrapper, and the heading keeps its classes and key/value
/// pairs. Content preceding the first heading is gathered into a leading section headed by the
/// document title (an `unnumbered` section whose identifier derives from that title).
pub(crate) fn make_sections(blocks: &[Block], title: &[Inline]) -> Vec<Block> {
    let boundary = blocks
        .iter()
        .position(|block| matches!(block, Block::Header(..)))
        .unwrap_or(blocks.len());
    let (preamble, rest) = blocks.split_at(boundary);
    let mut out = Vec::new();
    if !preamble.is_empty() {
        out.push(synthetic_section(title, preamble));
    }
    out.extend(build_sections(rest));
    // A document with a title but no body still yields one chapter: an unnumbered leading section
    // holding just the title, so the book has a first page rather than none.
    if out.is_empty() && !title.is_empty() {
        out.push(synthetic_section(title, &[]));
    }
    out
}

/// Group a header-led block sequence into nested section `Div`s. A block that is not a header (the
/// content directly under a section, before any sub-heading) is carried through unchanged.
fn build_sections(blocks: &[Block]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut remaining = blocks;
    while let Some((first, rest)) = remaining.split_first() {
        if let Block::Header(level, attr, inlines) = first {
            let end = rest
                .iter()
                .position(|block| matches!(block, Block::Header(inner, ..) if *inner <= *level))
                .unwrap_or(rest.len());
            let (body, tail) = rest.split_at(end);
            out.push(section_div(*level, attr, inlines, build_sections(body)));
            remaining = tail;
        } else {
            out.push(first.clone());
            remaining = rest;
        }
    }
    out
}

/// Build one section `Div` from a heading and its already-sectioned body.
fn section_div(level: i32, attr: &Attr, inlines: &[Inline], body: Vec<Block>) -> Block {
    let mut classes = vec![
        Text::from(SECTION_CLASS),
        Text::from(format!("level{level}")),
    ];
    classes.extend(attr.classes.iter().cloned());
    let section_attr = Attr {
        id: attr.id.clone(),
        classes,
        attributes: attr.attributes.clone(),
    };
    let heading = Block::Header(
        level,
        Box::new(Attr {
            id: Text::default(),
            classes: attr.classes.clone(),
            attributes: attr.attributes.clone(),
        }),
        inlines.to_vec(),
    );
    let mut children = Vec::with_capacity(body.len() + 1);
    children.push(heading);
    children.extend(body);
    Block::Div(Box::new(section_attr), children)
}

/// The leading section that carries content appearing before the document's first heading. It is an
/// `unnumbered` level-one section headed by the document title; with no title the heading is empty
/// and the identifier falls back to `section`.
fn synthetic_section(title: &[Inline], preamble: &[Block]) -> Block {
    let derived = slug(&to_plain_text(title));
    let id = if derived.is_empty() {
        Text::from("section")
    } else {
        Text::from(derived)
    };
    let section_attr = Attr {
        id,
        classes: vec![
            Text::from(SECTION_CLASS),
            Text::from("level1"),
            Text::from("unnumbered"),
        ],
        attributes: Vec::new(),
    };
    let heading = Block::Header(
        1,
        Box::new(Attr {
            id: Text::default(),
            classes: vec![Text::from("unnumbered")],
            attributes: Vec::new(),
        }),
        title.to_vec(),
    );
    let mut children = Vec::with_capacity(preamble.len() + 1);
    children.push(heading);
    children.extend(preamble.iter().cloned());
    Block::Div(Box::new(section_attr), children)
}

/// The nesting level a section `Div` records in its `level{N}` class, if any.
fn section_level(attr: &Attr) -> Option<i32> {
    attr.classes
        .iter()
        .find_map(|class| class.strip_prefix("level").and_then(|n| n.parse().ok()))
}

/// Whether a block is a section `Div` at the given nesting level.
fn is_section_at(block: &Block, level: i32) -> bool {
    matches!(block, Block::Div(attr, _)
        if attr.classes.iter().any(|class| class == SECTION_CLASS)
            && section_level(attr) == Some(level))
}

/// Break the sectioned blocks into chapters, one block list per output file. A section at a level up
/// to `split_level` starts a new file; deeper sections stay within their ancestor's file. With the
/// default `split_level` of one, each top-level section becomes its own chapter.
pub(crate) fn split_chapters(blocks: Vec<Block>, split_level: i32) -> Vec<Vec<Block>> {
    let mut chapters = Vec::new();
    for block in blocks {
        push_chapter(block, split_level, &mut chapters);
    }
    chapters
}

/// Append the chapters a single top-level section contributes. When the section sits above the split
/// level and holds sub-sections, those sub-sections are lifted into their own chapters and the
/// section keeps only its own leading content.
fn push_chapter(block: Block, split_level: i32, chapters: &mut Vec<Vec<Block>>) {
    if let Block::Div(attr, children) = &block
        && let Some(level) = section_level(attr)
        && level < split_level
        && children.iter().any(|c| is_section_at(c, level + 1))
    {
        let mut own = Vec::new();
        let mut nested = Vec::new();
        for child in children {
            if is_section_at(child, level + 1) {
                nested.push(child.clone());
            } else {
                own.push(child.clone());
            }
        }
        chapters.push(vec![Block::Div(attr.clone(), own)]);
        for child in nested {
            push_chapter(child, split_level, chapters);
        }
        return;
    }
    chapters.push(vec![block]);
}
