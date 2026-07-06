//! The greedy line-filling engine and hanging-indent helper shared by the text-oriented writers.
//! Consumed by whichever text writers are enabled, so unused-item warnings are allowed here rather
//! than gated per item.
#![allow(dead_code)]

use super::display_width;
use carta_core::WrapMode;
use std::borrow::Cow;

/// Column at which inline content is wrapped: the default fill width.
pub(crate) const FILL_COLUMN: usize = 72;

/// A unit of inline content awaiting line filling: an unbreakable text run, a breakable space, a
/// soft line break from the source, or a forced line break.
#[derive(Debug, Clone)]
pub(crate) enum Piece {
    Text(Cow<'static, str>),
    Space,
    /// A soft line break in the source. Under [`WrapMode::Preserve`] it stays a line break; under
    /// [`WrapMode::Auto`] and [`WrapMode::None`] it is inter-word space like [`Piece::Space`].
    Soft,
    Hard,
}

impl Piece {
    /// A text piece from a static literal (borrowed, allocation-free) or an owned string (moved in),
    /// without an intervening copy.
    pub(crate) fn text(value: impl Into<Cow<'static, str>>) -> Self {
        Piece::Text(value.into())
    }
}

/// Greedily fill inline pieces to `width` columns: a breakable space becomes a line break when
/// keeping the next word on the current line would exceed the fill column. Consecutive text runs (no
/// intervening space) stay together; runs of spaces collapse; leading and trailing spaces on a line
/// are dropped.
///
/// The `wrap` mode governs line layout: [`WrapMode::Auto`] reflows to `width`; [`WrapMode::None`]
/// never wraps (the whole paragraph is one line, soft breaks becoming spaces); [`WrapMode::Preserve`]
/// does not reflow but keeps each soft break from the source as a line break.
pub(crate) fn fill(pieces: &[Piece], width: usize, wrap: WrapMode) -> String {
    fill_offset(pieces, width, 0, wrap)
}

/// Like [`fill`], but the first line is laid out as if `initial` columns were already consumed (the
/// hanging-marker layout, where a leading marker shifts the first line's wrap point but leaves
/// continuation lines at the margin).
pub(crate) fn fill_offset(
    pieces: &[Piece],
    width: usize,
    initial: usize,
    wrap: WrapMode,
) -> String {
    // Auto reflows to the fill column; the other modes never wrap on width, so a sentinel column
    // wide enough that no real line reaches it stands in. Soft breaks are the only line splits then.
    let width = match wrap {
        WrapMode::Auto => width.max(1),
        WrapMode::None | WrapMode::Preserve => usize::MAX,
    };
    fill_core(
        pieces,
        width,
        initial,
        matches!(wrap, WrapMode::Preserve),
        false,
        &[],
    )
}

/// Like [`fill_offset`], but `groups` names half-open index ranges into `pieces` that must be laid
/// out as a single atom: a group is measured at its full single-line width for the decision of
/// whether to begin it on a fresh line, then its interior is filled from there (folding across
/// lines only when the group on its own is wider than the column). The ranges must be disjoint and
/// listed in ascending order. With `keep_leading` (see [`fill_hang`]) a space that opens the content
/// is emitted before the first word instead of being dropped.
pub(crate) fn fill_groups(
    pieces: &[Piece],
    groups: &[(usize, usize)],
    width: usize,
    initial: usize,
    keep_leading: bool,
    wrap: WrapMode,
) -> String {
    let width = match wrap {
        WrapMode::Auto => width.max(1),
        WrapMode::None | WrapMode::Preserve => usize::MAX,
    };
    fill_core(
        pieces,
        width,
        initial,
        matches!(wrap, WrapMode::Preserve),
        keep_leading,
        groups,
    )
}

/// Like [`fill`], but the first line keeps a leading space rather than dropping it: the content is
/// laid out as hanging text that a caller will prefix with a marker, so a space that opens the first
/// block sits between the marker and the first word instead of being swallowed.
pub(crate) fn fill_hang(pieces: &[Piece], width: usize, wrap: WrapMode) -> String {
    let width = match wrap {
        WrapMode::Auto => width.max(1),
        WrapMode::None | WrapMode::Preserve => usize::MAX,
    };
    fill_core(
        pieces,
        width,
        0,
        matches!(wrap, WrapMode::Preserve),
        true,
        &[],
    )
}

/// Lay out a table cell's inline content to a fixed-width column field. Unlike [`fill`], the field
/// reflows to `width` under both [`WrapMode::Auto`] and [`WrapMode::Preserve`] — a bordered cell is
/// always bounded by its column — while [`WrapMode::None`] still renders the content on one line so
/// the column can instead grow to hold it. Under [`WrapMode::Preserve`] each source soft break stays
/// a forced line break, with the text between breaks reflowed to the field width.
pub(crate) fn fill_cell(pieces: &[Piece], width: usize, wrap: WrapMode) -> String {
    let width = match wrap {
        WrapMode::None => usize::MAX,
        WrapMode::Auto | WrapMode::Preserve => width.max(1),
    };
    fill_core(
        pieces,
        width,
        0,
        matches!(wrap, WrapMode::Preserve),
        false,
        &[],
    )
}

/// The shared line-filling engine behind [`fill_offset`], [`fill_cell`], [`fill_groups`], and
/// [`fill_hang`]: lay `pieces` out into lines no wider than `width` (already resolved to a sentinel
/// when the caller wants no width wrap), starting `initial` columns into the first line, breaking on
/// each source soft break only when `preserve_softs` is set. With `keep_leading`, a space that opens
/// the content is emitted before the first word instead of being dropped, so hanging content laid
/// out under a marker keeps the gap the source put between the marker position and its first word.
/// `groups` names disjoint, ascending half-open index ranges that are placed atomically (see
/// [`fill_groups`]).
// A cohesive line-layout state machine: the per-piece arms and group handling share one running
// cursor, so keeping them in one body is clearer than threading the cursor through callees.
#[allow(clippy::too_many_lines)]
fn fill_core(
    pieces: &[Piece],
    width: usize,
    initial: usize,
    preserve_softs: bool,
    keep_leading: bool,
    groups: &[(usize, usize)],
) -> String {
    let mut out = String::new();
    let mut column = initial;
    let mut at_line_start = initial == 0 && !keep_leading;
    let mut pending_space = false;
    // Consecutive text pieces (no intervening space or break) form one unbreakable word, gathered
    // here as borrowed runs and placed only once its full width is known.
    let mut word: Vec<&str> = Vec::new();
    let mut word_width = 0;
    let mut next_group = 0;
    let mut index = 0;
    while index < pieces.len() {
        if let Some(&(start, end)) = groups.get(next_group) {
            if index >= end {
                // A stale range that the cursor has already passed; advance past it.
                next_group += 1;
                continue;
            }
            if index == start && end > start && end <= pieces.len() {
                // Flush the pending word; the group joins it with no space when no space split them.
                let abuts = !word.is_empty();
                place_word(
                    &mut out,
                    &mut column,
                    &mut at_line_start,
                    pending_space,
                    &word,
                    word_width,
                    width,
                );
                word.clear();
                word_width = 0;
                place_group(
                    &mut out,
                    &mut column,
                    &mut at_line_start,
                    pending_space && !abuts,
                    pieces.get(start..end).unwrap_or(&[]),
                    preserve_softs,
                    width,
                );
                pending_space = false;
                index = end;
                next_group += 1;
                continue;
            }
        }
        match pieces.get(index) {
            Some(Piece::Text(text)) => {
                let text = text.as_ref();
                word.push(text);
                word_width += display_width(text);
            }
            // A soft break forces a line break only when preserving the source's own breaks;
            // otherwise it is just inter-word space (and may become a reflow point under Auto).
            Some(Piece::Soft) if preserve_softs => {
                place_word(
                    &mut out,
                    &mut column,
                    &mut at_line_start,
                    pending_space,
                    &word,
                    word_width,
                    width,
                );
                word.clear();
                word_width = 0;
                if !at_line_start {
                    out.push('\n');
                    column = 0;
                    at_line_start = true;
                }
                pending_space = false;
            }
            Some(Piece::Space | Piece::Soft) => {
                place_word(
                    &mut out,
                    &mut column,
                    &mut at_line_start,
                    pending_space,
                    &word,
                    word_width,
                    width,
                );
                word.clear();
                word_width = 0;
                pending_space = true;
            }
            Some(Piece::Hard) => {
                place_word(
                    &mut out,
                    &mut column,
                    &mut at_line_start,
                    pending_space,
                    &word,
                    word_width,
                    width,
                );
                word.clear();
                word_width = 0;
                if !at_line_start {
                    out.push('\n');
                    column = 0;
                    at_line_start = true;
                }
                pending_space = false;
            }
            None => {}
        }
        index += 1;
    }
    place_word(
        &mut out,
        &mut column,
        &mut at_line_start,
        pending_space,
        &word,
        word_width,
        width,
    );
    out.trim_end_matches('\n').to_owned()
}

/// Place an atomic group's interior into `out`, deciding first whether it begins on a fresh line.
/// The group is sized at its full single-line width: when `lead_space` (a breakable space precedes
/// it) and the whole group would overflow the current line, it starts a new line; either way its
/// interior is then filled from the resulting column, folding across lines only when the group
/// alone is wider than the column.
fn place_group(
    out: &mut String,
    column: &mut usize,
    at_line_start: &mut bool,
    lead_space: bool,
    inner: &[Piece],
    preserve_softs: bool,
    width: usize,
) {
    let flat = flat_width(inner);
    if *at_line_start {
        *at_line_start = false;
    } else if lead_space && *column + 1 + flat > width {
        out.push('\n');
        *column = 0;
    } else if lead_space {
        out.push(' ');
        *column += 1;
    }
    let rendered = fill_core(inner, width, *column, preserve_softs, false, &[]);
    out.push_str(&rendered);
    *column = line_end_column(&rendered, *column);
}

/// The natural single-line width of a piece run: each text run's display width, each space or break
/// counted as one column.
fn flat_width(pieces: &[Piece]) -> usize {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(text) => display_width(text),
            Piece::Space | Piece::Soft | Piece::Hard => 1,
        })
        .sum()
}

/// The column reached at the end of an already-filled run that began `start_col` columns into its
/// first line: the width of the text after the last line break, or `start_col` plus the whole run's
/// width when it stayed on one line.
fn line_end_column(rendered: &str, start_col: usize) -> usize {
    match rendered.rsplit_once('\n') {
        Some((_, last)) => display_width(last),
        None => start_col + display_width(rendered),
    }
}

/// Place a gathered word onto the current line, inserting a line break in place of the preceding
/// space when keeping the word would overflow `width`. A no-op for an empty word.
///
/// A word usually has no embedded line break, but a multi-line literal — a footnote body set over
/// several paragraphs — does. Such a word's first line is what must fit after the preceding space,
/// and its last line sets the column the following text continues from; only its first line shares
/// the line it lands on, so the rest cannot push later words off the column.
fn place_word(
    out: &mut String,
    column: &mut usize,
    at_line_start: &mut bool,
    pending_space: bool,
    word: &[&str],
    word_width: usize,
    width: usize,
) {
    if word.is_empty() {
        return;
    }
    let (first_line, multiline, last_line) = word_line_metrics(word, word_width);
    if *at_line_start {
        *at_line_start = false;
    } else if pending_space && *column > 0 && *column + 1 + first_line > width {
        out.push('\n');
        *column = 0;
        *at_line_start = false;
    } else if pending_space {
        out.push(' ');
        *column += 1;
    }
    for part in word {
        out.push_str(part);
    }
    *column = if multiline {
        last_line
    } else {
        *column + word_width
    };
}

/// A gathered word's first-line width, whether it spans more than one line, and its last-line width.
/// Without an embedded line break the first and last lines are the whole word.
fn word_line_metrics(word: &[&str], word_width: usize) -> (usize, bool, usize) {
    if !word.iter().any(|part| part.contains('\n')) {
        return (word_width, false, word_width);
    }
    let joined = word.concat();
    let first = joined.split('\n').next().unwrap_or("");
    let last = joined.rsplit('\n').next().unwrap_or("");
    (display_width(first), true, display_width(last))
}

/// Apply `first` to the first line and `rest` to each non-empty later line, leaving blank lines
/// (block separators) unprefixed. This produces a hanging indent: a list marker plus continuation
/// indent, or a uniform block-quote / code prefix.
pub(crate) fn indent_block(body: &str, first: &str, rest: &str) -> String {
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if index == 0 {
            out.push_str(first);
            out.push_str(line);
        } else if !line.is_empty() {
            out.push_str(rest);
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_wraps_at_column_boundary() {
        let pieces = vec![
            Piece::Text("hello".into()),
            Piece::Space,
            Piece::Text("world".into()),
        ];
        assert_eq!(fill(&pieces, 72, WrapMode::Auto), "hello world");
        assert_eq!(fill(&pieces, 8, WrapMode::Auto), "hello\nworld");
    }

    #[test]
    fn fill_collapses_spaces_and_keeps_runs_together() {
        let pieces = vec![
            Piece::Space,
            Piece::Text("ab".into()),
            Piece::Text("cd".into()),
            Piece::Space,
            Piece::Space,
            Piece::Text("ef".into()),
            Piece::Space,
        ];
        assert_eq!(fill(&pieces, 72, WrapMode::Auto), "abcd ef");
    }

    #[test]
    fn fill_honors_hard_break() {
        let pieces = vec![
            Piece::Text("a".into()),
            Piece::Hard,
            Piece::Text("b".into()),
        ];
        assert_eq!(fill(&pieces, 72, WrapMode::Auto), "a\nb");
    }

    #[test]
    fn fill_offset_shifts_first_line_wrap() {
        let pieces = vec![
            Piece::Text("aa".into()),
            Piece::Space,
            Piece::Text("bb".into()),
        ];
        assert_eq!(fill_offset(&pieces, 6, 3, WrapMode::Auto), "aa\nbb");
        assert_eq!(fill_offset(&pieces, 8, 3, WrapMode::Auto), "aa bb");
    }

    #[test]
    fn fill_none_never_wraps_and_softens_breaks() {
        let pieces = vec![
            Piece::Text("hello".into()),
            Piece::Soft,
            Piece::Text("world".into()),
            Piece::Space,
            Piece::Text("again".into()),
        ];
        // No wrapping despite the tiny width, and the soft break is just a space.
        assert_eq!(fill(&pieces, 4, WrapMode::None), "hello world again");
    }

    #[test]
    fn fill_preserve_keeps_soft_breaks_but_does_not_reflow() {
        let pieces = vec![
            Piece::Text("hello".into()),
            Piece::Soft,
            Piece::Text("world".into()),
            Piece::Space,
            Piece::Text("again".into()),
        ];
        // The soft break stays a line break; the long second line is not reflowed.
        assert_eq!(fill(&pieces, 4, WrapMode::Preserve), "hello\nworld again");
    }

    #[test]
    fn fill_auto_treats_soft_break_as_a_reflow_point() {
        let pieces = vec![
            Piece::Text("hello".into()),
            Piece::Soft,
            Piece::Text("world".into()),
        ];
        // A soft break reflows exactly like a space under Auto.
        assert_eq!(fill(&pieces, 72, WrapMode::Auto), "hello world");
        assert_eq!(fill(&pieces, 8, WrapMode::Auto), "hello\nworld");
    }

    #[test]
    fn indent_block_applies_hanging_prefixes() {
        assert_eq!(indent_block("a\nb\n\nc", "- ", "  "), "- a\n  b\n\n  c");
    }
}
