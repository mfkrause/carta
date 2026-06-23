//! Section numbering and table-of-contents construction over the document model.
//!
//! [`number_sections`] splices a section number into each numbered heading: a leading
//! `header-section-number` span carrying the number, a following space, and a `number` key/value
//! attribute. [`build_toc`] produces a nested bullet list linking to the document's headings, which a
//! writer renders into the `toc` template variable. Both number headings the same way — a counter per
//! level, the result joined from the document's shallowest heading level — so a heading and its
//! contents entry always carry the same number.

use carta_ast::{Attr, Block, Inline, Target};

/// Headings range over levels 1 through 6.
const MAX_LEVEL: usize = 6;

/// A heading carrying this class keeps the counters unchanged and receives no number.
const UNNUMBERED: &str = "unnumbered";

/// Per-level section counters, advanced one heading at a time in document order. Numbers are joined
/// from `base`, the document's shallowest heading level.
struct Counters {
    levels: [u32; MAX_LEVEL],
    /// The shallowest heading level in the document (1-indexed), the level a number's first segment
    /// counts. Levels between this and a deeper heading that precedes its first appearance read as
    /// zero.
    base: usize,
}

impl Counters {
    fn new(base: usize) -> Self {
        Self {
            levels: [0; MAX_LEVEL],
            base: base.clamp(1, MAX_LEVEL),
        }
    }

    /// Advance the counters for a heading of `level` (clamped to 1..=6) and return its dotted
    /// number, or `None` when the heading is unnumbered (the counters are then left unchanged). A
    /// level's counter increments and every deeper level resets; the number joins the counters from
    /// the document's shallowest heading level up to this one. A document whose shallowest heading is
    /// level 2 still numbers from `1`; a skipped level reads as a zero (`1` then a level-3 heading is
    /// `1.0.1`); and a heading deeper than the base level appearing before any heading reaches that
    /// base reads with leading zeros (a level-2 heading before the first level-1 heading is `0.1`).
    fn advance(&mut self, level: i32, classes: &[String]) -> Option<String> {
        if classes.iter().any(|class| class == UNNUMBERED) {
            return None;
        }
        let level = usize::try_from(level).unwrap_or(1).clamp(1, MAX_LEVEL);
        if let Some(slot) = self.levels.get_mut(level - 1) {
            *slot += 1;
        }
        for slot in self.levels.iter_mut().skip(level) {
            *slot = 0;
        }
        let start = self.base.saturating_sub(1).min(level.saturating_sub(1));
        let number = self
            .levels
            .get(start..level)
            .unwrap_or(&[])
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".");
        Some(number)
    }
}

/// The shallowest heading level anywhere in `blocks` (recursing through divisions), or 1 when the
/// document has no headings. This anchors section numbering: it is the level a number's first segment
/// counts from.
fn min_heading_level(blocks: &[Block]) -> usize {
    fn walk(blocks: &[Block], min: &mut Option<usize>) {
        for block in blocks {
            match block {
                Block::Header(level, _, _) => {
                    let level = usize::try_from(*level).unwrap_or(1).clamp(1, MAX_LEVEL);
                    *min = Some(min.map_or(level, |current| current.min(level)));
                }
                Block::Div(_, inner) => walk(inner, min),
                _ => {}
            }
        }
    }
    let mut min = None;
    walk(blocks, &mut min);
    min.unwrap_or(1)
}

/// Splice section numbers into the headings of `blocks`, walking nested sections in document order. A
/// numbered heading gains a leading `header-section-number` span holding its number, a following
/// space, and a `number` key/value attribute; an unnumbered heading is left untouched.
pub fn number_sections(blocks: &mut [Block]) {
    let mut counters = Counters::new(min_heading_level(blocks));
    number_in(blocks, &mut counters);
}

fn number_in(blocks: &mut [Block], counters: &mut Counters) {
    for block in blocks {
        match block {
            Block::Header(level, attr, inlines) => {
                if let Some(number) = counters.advance(*level, &attr.classes) {
                    attr.attributes.push(("number".to_owned(), number.clone()));
                    let span = Inline::Span(
                        section_number_attr("header-section-number"),
                        vec![Inline::Str(number)],
                    );
                    inlines.splice(0..0, [span, Inline::Space]);
                }
            }
            Block::Div(_, inner) => number_in(inner, counters),
            _ => {}
        }
    }
}

/// Build a nested bullet list linking to the document's headings down to `depth`, or `None` when no
/// heading qualifies. With `numbered`, each entry carries a leading `toc-section-number` span holding
/// the heading's number (computed exactly as [`number_sections`] does); without it, entries carry
/// only the heading text. With `anchors`, each entry's link carries its own `toc-`-prefixed id so it
/// can be linked back to; formats that cannot represent an inline identifier pass `false` to omit it.
/// Footnotes are dropped and links unwrapped, so an entry never nests an anchor or a note marker.
#[must_use]
pub fn build_toc(blocks: &[Block], depth: usize, numbered: bool, anchors: bool) -> Option<Block> {
    let mut counters = Counters::new(min_heading_level(blocks));
    let mut entries = Vec::new();
    collect_entries(
        blocks,
        depth,
        numbered,
        anchors,
        &mut counters,
        &mut entries,
    );
    if entries.is_empty() {
        None
    } else {
        Some(Block::BulletList(nest(&entries)))
    }
}

/// One contents entry: the heading's level (for nesting) and the prepared inlines (a link to the
/// heading, or plain text when the heading carries no id to target).
struct Entry {
    level: i32,
    content: Vec<Inline>,
}

fn collect_entries(
    blocks: &[Block],
    depth: usize,
    numbered: bool,
    anchors: bool,
    counters: &mut Counters,
    entries: &mut Vec<Entry>,
) {
    for block in blocks {
        match block {
            Block::Header(level, attr, inlines) => {
                // Every heading advances the counters so numbers stay consistent with the body, even
                // those deeper than `depth` that the contents list omits.
                let number = counters.advance(*level, &attr.classes);
                if (1..=depth).contains(&usize::try_from(*level).unwrap_or(0)) {
                    entries.push(Entry {
                        level: *level,
                        content: toc_entry(attr, inlines, numbered, number.as_deref(), anchors),
                    });
                }
            }
            Block::Div(_, inner) => {
                collect_entries(inner, depth, numbered, anchors, counters, entries);
            }
            _ => {}
        }
    }
}

/// Turn a flat, document-order list of entries into a nested bullet list: each entry's deeper-level
/// followers become its sub-list.
fn nest(entries: &[Entry]) -> Vec<Vec<Block>> {
    let mut items = Vec::new();
    let mut rest = entries;
    while let Some((first, tail)) = rest.split_first() {
        let child_count = tail
            .iter()
            .take_while(|entry| entry.level > first.level)
            .count();
        let (children, after) = tail.split_at(child_count);
        let mut blocks = vec![Block::Plain(first.content.clone())];
        if !children.is_empty() {
            blocks.push(Block::BulletList(nest(children)));
        }
        items.push(blocks);
        rest = after;
    }
    items
}

/// The inlines for one contents entry. A numbered entry leads with a `toc-section-number` span and a
/// space. When the heading carries an id, the entry is a link targeting it — and, with `anchors`, the
/// link also carries its own id (the heading id prefixed with `toc-`) so it can be linked back to.
/// When the heading has no id there is nothing to target, so the entry is plain text.
fn toc_entry(
    attr: &Attr,
    inlines: &[Inline],
    numbered: bool,
    number: Option<&str>,
    anchors: bool,
) -> Vec<Inline> {
    let mut content = Vec::new();
    if let Some(number) = number.filter(|_| numbered) {
        content.push(Inline::Span(
            section_number_attr("toc-section-number"),
            vec![Inline::Str(number.to_owned())],
        ));
        content.push(Inline::Space);
    }
    content.extend(clean_toc_inlines(inlines));
    if attr.id.is_empty() {
        return content;
    }
    let link_attr = Attr {
        id: if anchors {
            format!("toc-{}", attr.id)
        } else {
            String::new()
        },
        classes: Vec::new(),
        attributes: Vec::new(),
    };
    vec![Inline::Link(
        link_attr,
        content,
        Target {
            url: format!("#{}", attr.id),
            title: String::new(),
        },
    )]
}

/// Strip a heading's inlines for use in a contents entry: drop footnotes and replace each link with
/// its own content (an anchor cannot nest inside the entry's anchor), recursing through styled spans.
fn clean_toc_inlines(inlines: &[Inline]) -> Vec<Inline> {
    let mut out = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Note(_) => {}
            Inline::Link(_, inner, _) => out.extend(clean_toc_inlines(inner)),
            Inline::Emph(inner) => out.push(Inline::Emph(clean_toc_inlines(inner))),
            Inline::Underline(inner) => out.push(Inline::Underline(clean_toc_inlines(inner))),
            Inline::Strong(inner) => out.push(Inline::Strong(clean_toc_inlines(inner))),
            Inline::Strikeout(inner) => out.push(Inline::Strikeout(clean_toc_inlines(inner))),
            Inline::Superscript(inner) => out.push(Inline::Superscript(clean_toc_inlines(inner))),
            Inline::Subscript(inner) => out.push(Inline::Subscript(clean_toc_inlines(inner))),
            Inline::SmallCaps(inner) => out.push(Inline::SmallCaps(clean_toc_inlines(inner))),
            Inline::Quoted(quote, inner) => {
                out.push(Inline::Quoted(quote.clone(), clean_toc_inlines(inner)));
            }
            Inline::Cite(citations, inner) => {
                out.push(Inline::Cite(citations.clone(), clean_toc_inlines(inner)));
            }
            Inline::Span(attr, inner) => {
                out.push(Inline::Span(attr.clone(), clean_toc_inlines(inner)));
            }
            other => out.push(other.clone()),
        }
    }
    out
}

fn section_number_attr(class: &str) -> Attr {
    Attr {
        id: String::new(),
        classes: vec![class.to_owned()],
        attributes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::Inline;

    fn header(level: i32, classes: &[&str], text: &str) -> Block {
        Block::Header(
            level,
            Attr {
                id: text.to_lowercase().replace(' ', "-"),
                classes: classes.iter().map(|class| (*class).to_owned()).collect(),
                attributes: Vec::new(),
            },
            vec![Inline::Str(text.to_owned())],
        )
    }

    fn number_of(block: &Block) -> Option<String> {
        if let Block::Header(_, attr, _) = block {
            attr.attributes
                .iter()
                .find(|(key, _)| key == "number")
                .map(|(_, value)| value.clone())
        } else {
            None
        }
    }

    fn number_at(blocks: &[Block], index: usize) -> Option<String> {
        blocks.get(index).and_then(number_of)
    }

    #[test]
    fn numbers_increment_and_reset_by_level() {
        let mut blocks = vec![
            header(1, &[], "One"),
            header(2, &[], "One A"),
            header(2, &[], "One B"),
            header(3, &[], "Deep"),
            header(1, &[], "Two"),
        ];
        number_sections(&mut blocks);
        let numbers: Vec<_> = blocks.iter().map(number_of).collect();
        assert_eq!(
            numbers,
            vec![
                Some("1".to_owned()),
                Some("1.1".to_owned()),
                Some("1.2".to_owned()),
                Some("1.2.1".to_owned()),
                Some("2".to_owned()),
            ]
        );
    }

    #[test]
    fn document_opening_at_level_two_numbers_from_one() {
        let mut blocks = vec![header(2, &[], "Start"), header(3, &[], "Child")];
        number_sections(&mut blocks);
        assert_eq!(number_at(&blocks, 0), Some("1".to_owned()));
        assert_eq!(number_at(&blocks, 1), Some("1.1".to_owned()));
    }

    #[test]
    fn skipped_level_reads_as_zero() {
        let mut blocks = vec![header(1, &[], "One"), header(3, &[], "Jump")];
        number_sections(&mut blocks);
        assert_eq!(number_at(&blocks, 1), Some("1.0.1".to_owned()));
    }

    #[test]
    fn deep_heading_before_its_base_level_reads_with_zero_ancestors() {
        let mut blocks = vec![
            header(2, &[], "Deep"),
            header(3, &[], "Deeper"),
            header(1, &[], "Top"),
        ];
        number_sections(&mut blocks);
        assert_eq!(number_at(&blocks, 0), Some("0.1".to_owned()));
        assert_eq!(number_at(&blocks, 1), Some("0.1.1".to_owned()));
        assert_eq!(number_at(&blocks, 2), Some("1".to_owned()));
    }

    #[test]
    fn shallowest_level_anchors_numbering_even_when_levels_only_deepen() {
        let mut blocks = vec![header(6, &[], "Six"), header(5, &[], "Five")];
        number_sections(&mut blocks);
        assert_eq!(number_at(&blocks, 0), Some("0.1".to_owned()));
        assert_eq!(number_at(&blocks, 1), Some("1".to_owned()));
    }

    #[test]
    fn unnumbered_heading_keeps_counter() {
        let mut blocks = vec![
            header(1, &[], "One"),
            header(2, &["unnumbered"], "Hidden"),
            header(2, &[], "Listed"),
        ];
        number_sections(&mut blocks);
        assert_eq!(number_at(&blocks, 1), None);
        assert_eq!(number_at(&blocks, 2), Some("1.1".to_owned()));
    }

    #[test]
    fn numbered_heading_leads_with_a_span() {
        let mut blocks = vec![header(1, &[], "One")];
        number_sections(&mut blocks);
        let Some(Block::Header(_, _, inlines)) = blocks.first() else {
            panic!("expected a header");
        };
        assert!(matches!(
            inlines.first(),
            Some(Inline::Span(attr, _)) if attr.classes == ["header-section-number"]
        ));
        assert!(matches!(inlines.get(1), Some(Inline::Space)));
    }

    #[test]
    fn toc_omits_headings_beyond_depth() {
        let blocks = vec![
            header(1, &[], "One"),
            header(2, &[], "Two"),
            header(3, &[], "Three"),
        ];
        let Some(Block::BulletList(items)) = build_toc(&blocks, 2, false, true) else {
            panic!("expected a contents list");
        };
        // One top-level item ("One") with a single nested item ("Two"); "Three" is past depth 2.
        assert_eq!(items.len(), 1);
        let Some(Block::BulletList(children)) = items.first().and_then(|item| item.get(1)) else {
            panic!("expected a nested list");
        };
        assert_eq!(children.len(), 1);
        let Some(child) = children.first() else {
            panic!("expected a child item");
        };
        assert!(child.get(1).is_none());
    }

    #[test]
    fn empty_document_has_no_toc() {
        assert!(
            build_toc(
                &[Block::Para(vec![Inline::Str("hi".to_owned())])],
                3,
                false,
                true
            )
            .is_none()
        );
    }

    #[test]
    fn toc_entry_links_to_heading() {
        let blocks = vec![header(1, &[], "One")];
        let Some(Block::BulletList(items)) = build_toc(&blocks, 3, true, true) else {
            panic!("expected a contents list");
        };
        let Some(Block::Plain(inlines)) = items.first().and_then(|item| item.first()) else {
            panic!("expected a plain item");
        };
        let Some(Inline::Link(attr, content, target)) = inlines.first() else {
            panic!("expected a link");
        };
        assert_eq!(attr.id, "toc-one");
        assert_eq!(target.url, "#one");
        assert!(matches!(
            content.first(),
            Some(Inline::Span(span_attr, _)) if span_attr.classes == ["toc-section-number"]
        ));
    }

    #[test]
    fn toc_drops_notes_and_unwraps_links() {
        let heading = Block::Header(
            1,
            Attr {
                id: "h".to_owned(),
                ..Attr::default()
            },
            vec![
                Inline::Link(
                    Attr::default(),
                    vec![Inline::Str("text".to_owned())],
                    Target {
                        url: "x".to_owned(),
                        title: String::new(),
                    },
                ),
                Inline::Note(vec![Block::Para(vec![Inline::Str("note".to_owned())])]),
            ],
        );
        let Some(Block::BulletList(items)) = build_toc(&[heading], 3, false, true) else {
            panic!("expected a contents list");
        };
        let Some(Block::Plain(inlines)) = items.first().and_then(|item| item.first()) else {
            panic!("expected a plain item");
        };
        let Some(Inline::Link(_, content, _)) = inlines.first() else {
            panic!("expected a link");
        };
        assert_eq!(content, &vec![Inline::Str("text".to_owned())]);
    }

    #[test]
    fn toc_without_anchors_omits_entry_ids() {
        let blocks = vec![header(1, &[], "One")];
        let Some(Block::BulletList(items)) = build_toc(&blocks, 3, false, false) else {
            panic!("expected a contents list");
        };
        let Some(Block::Plain(inlines)) = items.first().and_then(|item| item.first()) else {
            panic!("expected a plain item");
        };
        let Some(Inline::Link(attr, _, target)) = inlines.first() else {
            panic!("expected a link");
        };
        assert!(attr.id.is_empty());
        assert_eq!(target.url, "#one");
    }

    #[test]
    fn toc_entry_without_an_id_is_plain_text() {
        let heading = Block::Header(1, Attr::default(), vec![Inline::Str("Untitled".to_owned())]);
        let Some(Block::BulletList(items)) = build_toc(&[heading], 3, false, true) else {
            panic!("expected a contents list");
        };
        let Some(Block::Plain(inlines)) = items.first().and_then(|item| item.first()) else {
            panic!("expected a plain item");
        };
        assert_eq!(inlines, &vec![Inline::Str("Untitled".to_owned())]);
    }
}
