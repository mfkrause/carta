//! Structuring a document into the nested sections and separate chapter files an EPUB uses.
//!
//! An EPUB body is not a flat run of blocks: each heading and the content beneath it forms a
//! `<section>`, and the book is broken into separate XHTML files at a chosen heading level. This
//! module performs both transforms on the document model, before the HTML writer renders each
//! chapter.

use carta_ast::{Attr, Block, Inline, Text, slug, to_plain_text};
use std::collections::{BTreeMap, BTreeSet};

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
    // Track the identifiers assigned so far, so a section whose heading carries none is given a
    // unique one rather than an empty string that no navigation link could target. Seed it with
    // every identifier the document already carries explicitly — on a heading or any other element —
    // so a derived slug is disambiguated against them and cannot silently duplicate an author's own.
    let mut seen = BTreeSet::new();
    let mut explicit = BTreeMap::new();
    super::record_ids(blocks, "", &mut explicit);
    seen.extend(explicit.into_keys());
    if !preamble.is_empty() {
        out.push(synthetic_section(title, preamble, &mut seen));
    }
    out.extend(build_sections(rest, &mut seen));
    // A document with a title but no body still yields one chapter: an unnumbered leading section
    // holding just the title, so the book has a first page rather than none.
    if out.is_empty() && !title.is_empty() {
        out.push(synthetic_section(title, &[], &mut seen));
    }
    out
}

/// Group a header-led block sequence into nested section `Div`s. A block that is not a header (the
/// content directly under a section, before any sub-heading) is carried through unchanged.
fn build_sections(blocks: &[Block], seen: &mut BTreeSet<String>) -> Vec<Block> {
    let mut out = Vec::new();
    let mut remaining = blocks;
    while let Some((first, rest)) = remaining.split_first() {
        if let Block::Header(level, attr, inlines) = first {
            let end = rest
                .iter()
                .position(|block| matches!(block, Block::Header(inner, ..) if *inner <= *level))
                .unwrap_or(rest.len());
            let (body, tail) = rest.split_at(end);
            // Resolve this section's identifier before its descendants', so a derived slug is made
            // unique in document order.
            let id = section_id(attr, inlines, seen);
            let inner = build_sections(body, seen);
            out.push(section_div(*level, attr, inlines, id, inner));
            remaining = tail;
        } else {
            out.push(first.clone());
            remaining = rest;
        }
    }
    out
}

/// Build one section `Div` from a heading, its resolved identifier, and its already-sectioned body.
fn section_div(level: i32, attr: &Attr, inlines: &[Inline], id: Text, body: Vec<Block>) -> Block {
    let mut classes = vec![
        Text::from(SECTION_CLASS),
        Text::from(format!("level{level}")),
    ];
    classes.extend(attr.classes.iter().cloned());
    let section_attr = Attr {
        id,
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

/// The identifier a section wrapper carries. A heading's own identifier is kept verbatim; a heading
/// without one is given a slug derived from its text (falling back to `section` when that is empty),
/// made unique against the identifiers already assigned so every navigation link has a live target.
fn section_id(attr: &Attr, inlines: &[Inline], seen: &mut BTreeSet<String>) -> Text {
    if !attr.id.is_empty() {
        seen.insert(attr.id.to_string());
        return attr.id.clone();
    }
    let derived = slug(&to_plain_text(inlines));
    let base = if derived.is_empty() {
        String::from("section")
    } else {
        derived
    };
    Text::from(unique_id(base, seen))
}

/// Return `base` if it is unused, otherwise `base-1`, `base-2`, … until one is free, recording the
/// chosen identifier so later calls avoid it.
fn unique_id(base: String, seen: &mut BTreeSet<String>) -> String {
    if seen.insert(base.clone()) {
        return base;
    }
    let mut suffix = 1u32;
    loop {
        let candidate = format!("{base}-{suffix}");
        if seen.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

/// The leading section that carries content appearing before the document's first heading. It is an
/// `unnumbered` level-one section headed by the document title; with no title the heading is empty
/// and the identifier falls back to `section`.
fn synthetic_section(title: &[Inline], preamble: &[Block], seen: &mut BTreeSet<String>) -> Block {
    let derived = slug(&to_plain_text(title));
    let base = if derived.is_empty() {
        String::from("section")
    } else {
        derived
    };
    let id = Text::from(unique_id(base, seen));
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

/// Whether a block is a section `Div` shallow enough to begin its own chapter file: one whose level
/// is at or above the split level. Testing the level directly, rather than expecting the next level
/// down, lifts a section out even where the heading levels jump (an `H1` straight to an `H3`).
fn is_promotable_section(block: &Block, split_level: i32) -> bool {
    matches!(block, Block::Div(attr, _)
        if attr.classes.iter().any(|class| class == SECTION_CLASS)
            && section_level(attr).is_some_and(|level| level <= split_level))
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
        && children
            .iter()
            .any(|c| is_promotable_section(c, split_level))
    {
        let mut own = Vec::new();
        let mut nested = Vec::new();
        for child in children {
            if is_promotable_section(child, split_level) {
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

#[cfg(test)]
mod tests {
    use super::{make_sections, section_id, unique_id};
    use carta_ast::{Attr, Block, Inline, Text};
    use std::collections::BTreeSet;

    fn heading(text: &str) -> Vec<Inline> {
        vec![Inline::Str(Text::from(text))]
    }

    fn header(level: i32, id: &str, text: &str) -> Block {
        Block::Header(
            level,
            Box::new(Attr {
                id: Text::from(id),
                ..Attr::default()
            }),
            heading(text),
        )
    }

    /// The identifiers of the top-level section wrappers, in document order.
    fn section_ids(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|block| match block {
                Block::Div(attr, _) => Some(attr.id.to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn unique_id_suffixes_repeated_bases() {
        let mut seen = BTreeSet::new();
        assert_eq!(unique_id(String::from("intro"), &mut seen), "intro");
        assert_eq!(unique_id(String::from("intro"), &mut seen), "intro-1");
        assert_eq!(unique_id(String::from("intro"), &mut seen), "intro-2");
        assert_eq!(unique_id(String::from("outro"), &mut seen), "outro");
    }

    #[test]
    fn section_id_prefers_explicit_then_derives() {
        let mut seen = BTreeSet::new();
        let explicit = Attr {
            id: Text::from("kept"),
            ..Attr::default()
        };
        assert_eq!(
            section_id(&explicit, &heading("Any Title"), &mut seen),
            "kept"
        );
        let bare = Attr::default();
        assert_eq!(
            section_id(&bare, &heading("Getting Started"), &mut seen),
            "getting-started"
        );
        assert_eq!(
            section_id(&bare, &heading("Getting Started"), &mut seen),
            "getting-started-1"
        );
        // A heading whose text yields no slug falls back to the generic identifier.
        assert_eq!(section_id(&bare, &heading("!!!"), &mut seen), "section");
    }

    #[test]
    fn make_sections_generates_and_disambiguates_ids() {
        let blocks = vec![
            header(1, "", "One"),
            header(1, "", "One"),
            header(1, "kept", "Three"),
        ];
        assert_eq!(
            section_ids(&make_sections(&blocks, &[])),
            ["one", "one-1", "kept"]
        );
    }

    #[test]
    fn derived_slug_avoids_a_later_explicit_id() {
        // The second heading's explicit identifier is what the first heading's text would slug to;
        // seeding it up front pushes the derived one aside instead of duplicating it.
        let blocks = vec![
            header(1, "", "Installation"),
            header(1, "installation", "Setup"),
        ];
        assert_eq!(
            section_ids(&make_sections(&blocks, &[])),
            ["installation-1", "installation"]
        );
    }

    #[test]
    fn split_promotes_a_section_even_when_heading_levels_jump() {
        use super::split_chapters;
        // An H1 followed straight by an H3, with no H2 between them.
        let blocks = vec![header(1, "", "Chapter"), header(3, "", "Deep")];
        let sectioned = make_sections(&blocks, &[]);
        // A split level deep enough to include the H3 still lifts it into its own chapter.
        assert_eq!(split_chapters(sectioned.clone(), 3).len(), 2);
        // A shallower split keeps the H3 within the H1's single file.
        assert_eq!(split_chapters(sectioned, 1).len(), 1);
    }
}
