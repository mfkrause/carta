//! Slide segmentation shared by the presentation writers.
//!
//! A presentation's block sequence is partitioned into a flat list of [`Slide`] units against a
//! computed *slide level*: headers shallower than that level are standalone sectioning markers,
//! headers at that level open a titled frame, and everything else gathers into the current frame.
//! A top-level horizontal rule forces a frame break. The segmentation is purely structural and
//! holds no target-format detail, so each presentation writer renders the same units its own way.
//!
//! Headers deeper than the slide level stay inside a frame's body for the writer to interpret;
//! [`group_headings`] offers the same hierarchical grouping over those inner headers for writers
//! that nest them (such as block environments).

use carta_ast::{Attr, Block, Inline};

/// The deepest header level, used as the slide level when no header introduces content.
pub(crate) const MAX_LEVEL: i32 = 6;

/// One unit of a segmented presentation.
pub(crate) enum Slide<'a> {
    /// A header shallower than the slide level: a sectioning marker carrying no frame body.
    Section {
        level: i32,
        attr: &'a Attr,
        title: &'a [Inline],
    },
    /// A frame: an optional title header at the slide level, then the contiguous block run that
    /// belongs to it. The body may contain headers deeper than the slide level.
    Frame {
        title: Option<FrameTitle<'a>>,
        body: &'a [Block],
    },
}

/// The title header of a frame. Its header level always equals the slide level passed to
/// [`segment`], so it is not repeated here.
pub(crate) struct FrameTitle<'a> {
    pub(crate) attr: &'a Attr,
    pub(crate) inlines: &'a [Inline],
}

/// The slide level for a block sequence: the shallowest level of any header immediately followed,
/// at the top level, by a block that is not a header. When no header introduces content, the level
/// is the deepest header level, so no header opens a frame on content grounds alone.
#[must_use]
pub(crate) fn slide_level(blocks: &[Block]) -> i32 {
    let mut level = MAX_LEVEL;
    let mut found = false;
    for pair in blocks.windows(2) {
        if let [Block::Header(header_level, _, _), next] = pair
            && !matches!(next, Block::Header(_, _, _))
        {
            level = level.min(*header_level);
            found = true;
        }
    }
    if found { level } else { MAX_LEVEL }
}

/// Partition a presentation's blocks into slide units against `level`.
#[must_use]
pub(crate) fn segment(blocks: &[Block], level: i32) -> Vec<Slide<'_>> {
    let mut slides = Vec::new();
    let mut index = 0;
    while index < blocks.len() {
        let Some(block) = blocks.get(index) else {
            break;
        };
        match block {
            Block::Header(header_level, attr, title) if *header_level < level => {
                slides.push(Slide::Section {
                    level: *header_level,
                    attr,
                    title,
                });
                index += 1;
            }
            Block::Header(header_level, attr, inlines) if *header_level == level => {
                let start = index + 1;
                let end = frame_end(blocks, start, level);
                slides.push(Slide::Frame {
                    title: Some(FrameTitle { attr, inlines }),
                    body: blocks.get(start..end).unwrap_or(&[]),
                });
                index = end;
            }
            Block::HorizontalRule => {
                index += 1;
            }
            _ => {
                let end = frame_end(blocks, index, level);
                slides.push(Slide::Frame {
                    title: None,
                    body: blocks.get(index..end).unwrap_or(&[]),
                });
                index = end;
            }
        }
    }
    slides
}

/// The end (exclusive) of a frame body starting at `start`: the first top-level horizontal rule or
/// header at or above the slide level.
fn frame_end(blocks: &[Block], start: usize, level: i32) -> usize {
    let mut index = start;
    while let Some(block) = blocks.get(index) {
        match block {
            Block::HorizontalRule => break,
            Block::Header(header_level, _, _) if *header_level <= level => break,
            _ => index += 1,
        }
    }
    index
}

/// A hierarchical grouping of blocks under headers at `level` and deeper. A writer that nests inner
/// headers (for example as block environments) walks this in place of the flat slide list.
pub(crate) enum Heading<'a> {
    /// A run of blocks that precedes any header at `level`.
    Loose(&'a [Block]),
    /// A header at `level` with the blocks that follow it, up to the next header at or above
    /// `level`. The nested blocks may themselves contain deeper headers.
    Section {
        attr: &'a Attr,
        title: &'a [Inline],
        body: &'a [Block],
    },
}

/// Group blocks under headers at exactly `level`, gathering each header's following blocks (up to
/// the next header at or above `level`) as its body. Blocks before the first such header form a
/// leading [`Heading::Loose`] run. Unlike [`segment`], horizontal rules are ordinary content here.
#[must_use]
pub(crate) fn group_headings(blocks: &[Block], level: i32) -> Vec<Heading<'_>> {
    let mut groups = Vec::new();
    let first = blocks
        .iter()
        .position(
            |block| matches!(block, Block::Header(header_level, _, _) if *header_level <= level),
        )
        .unwrap_or(blocks.len());
    if first > 0 {
        groups.push(Heading::Loose(blocks.get(..first).unwrap_or(&[])));
    }
    let mut index = first;
    while let Some(block) = blocks.get(index) {
        if let Block::Header(header_level, attr, title) = block
            && *header_level == level
        {
            let start = index + 1;
            let end = heading_end(blocks, start, level);
            groups.push(Heading::Section {
                attr,
                title,
                body: blocks.get(start..end).unwrap_or(&[]),
            });
            index = end;
        } else {
            // A header shallower than `level` (or other stray block) is left as a loose run so no
            // content is dropped; deeper headers are folded into the preceding section's body.
            let start = index;
            let end = heading_end(blocks, start + 1, level);
            groups.push(Heading::Loose(blocks.get(start..end).unwrap_or(&[])));
            index = end;
        }
    }
    groups
}

/// The end (exclusive) of a heading body starting at `start`: the first header at or above `level`.
fn heading_end(blocks: &[Block], start: usize, level: i32) -> usize {
    let mut index = start;
    while let Some(block) = blocks.get(index) {
        match block {
            Block::Header(header_level, _, _) if *header_level <= level => break,
            _ => index += 1,
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(level: i32, id: &str) -> Block {
        Block::Header(
            level,
            Box::new(Attr {
                id: id.to_owned().into(),
                ..Attr::default()
            }),
            vec![Inline::Str(id.to_owned().into())],
        )
    }

    fn para(text: &str) -> Block {
        Block::Para(vec![Inline::Str(text.to_owned().into())])
    }

    #[test]
    fn slide_level_defaults_to_max_without_content() {
        assert_eq!(slide_level(&[header(1, "a"), header(2, "b")]), MAX_LEVEL);
    }

    #[test]
    fn slide_level_is_shallowest_content_bearing_header() {
        let blocks = vec![
            header(1, "a"),
            header(2, "b"),
            para("y"),
            header(3, "c"),
            para("z"),
        ];
        assert_eq!(slide_level(&blocks), 2);
    }

    #[test]
    fn slide_level_drops_to_one_for_content_under_top_header() {
        let blocks = vec![header(1, "a"), para("x"), header(2, "b"), para("y")];
        assert_eq!(slide_level(&blocks), 1);
    }

    #[test]
    fn segment_splits_sections_and_frames() {
        let blocks = vec![
            header(1, "a"),
            header(2, "b"),
            para("y"),
            header(1, "d"),
            header(2, "e"),
            para("w"),
        ];
        let level = slide_level(&blocks);
        assert_eq!(level, 2);
        let slides = segment(&blocks, level);
        assert_eq!(slides.len(), 4);
        assert!(matches!(
            slides.first(),
            Some(Slide::Section { level: 1, .. })
        ));
        assert!(matches!(
            slides.get(1),
            Some(Slide::Frame { title: Some(_), body }) if body.len() == 1
        ));
        assert!(matches!(
            slides.get(2),
            Some(Slide::Section { level: 1, .. })
        ));
        assert!(matches!(
            slides.get(3),
            Some(Slide::Frame { title: Some(_), body }) if body.len() == 1
        ));
    }

    #[test]
    fn horizontal_rule_breaks_into_titleless_frames() {
        let blocks = vec![para("above"), Block::HorizontalRule, para("below")];
        let slides = segment(&blocks, slide_level(&blocks));
        assert_eq!(slides.len(), 2);
        for slide in &slides {
            assert!(matches!(slide, Slide::Frame { title: None, body } if body.len() == 1));
        }
    }

    #[test]
    fn deep_header_without_title_opens_titleless_frame() {
        let blocks = vec![
            header(3, "c"),
            para("x"),
            header(1, "a"),
            header(2, "b"),
            para("y"),
        ];
        let level = slide_level(&blocks);
        assert_eq!(level, 2);
        let slides = segment(&blocks, level);
        // Frame(C body), Section(A), Frame(B body).
        assert!(
            matches!(slides.first(), Some(Slide::Frame { title: None, body }) if body.len() == 2)
        );
    }

    #[test]
    fn group_headings_nests_deeper_content() {
        let blocks = vec![
            para("y"),
            header(3, "c"),
            para("z"),
            header(4, "e"),
            para("q"),
            header(3, "d"),
            para("w"),
        ];
        let groups = group_headings(&blocks, 3);
        assert_eq!(groups.len(), 3);
        assert!(matches!(groups.first(), Some(Heading::Loose(run)) if run.len() == 1));
        assert!(matches!(
            groups.get(1),
            Some(Heading::Section { body, .. }) if body.len() == 3
        ));
        assert!(matches!(
            groups.get(2),
            Some(Heading::Section { body, .. }) if body.len() == 1
        ));
    }
}
