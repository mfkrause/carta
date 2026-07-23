//! The greedy line-filling engine and hanging-indent helper shared by the text-oriented writers.

use super::display_width;
use carta_core::WrapMode;
use std::borrow::Cow;

/// Column at which inline content is wrapped: the default fill width.
pub(crate) const FILL_COLUMN: usize = 72;

/// A unit of inline content awaiting line filling: an unbreakable text run, a breakable space, a
/// soft line break from the source, or a forced line break.
#[cfg_attr(
    not(any(
        feature = "asciidoc",
        feature = "commonmark",
        feature = "gfm",
        feature = "latex",
        feature = "markdown",
        feature = "org",
        feature = "plain",
        feature = "rst"
    )),
    allow(dead_code)
)]
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
    #[cfg_attr(
        not(any(
            feature = "asciidoc",
            feature = "commonmark",
            feature = "gfm",
            feature = "latex",
            feature = "markdown",
            feature = "org",
            feature = "plain",
            feature = "rst"
        )),
        allow(dead_code)
    )]
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
#[cfg_attr(
    not(any(
        feature = "asciidoc",
        feature = "commonmark",
        feature = "gfm",
        feature = "latex",
        feature = "markdown",
        feature = "org",
        feature = "plain",
        feature = "rst"
    )),
    allow(dead_code)
)]
pub(crate) fn fill(pieces: &[Piece], width: usize, wrap: WrapMode) -> String {
    fill_offset(pieces, width, 0, wrap)
}

/// Like [`fill`], but the first line is laid out as if `initial` columns were already consumed (the
/// hanging-marker layout, where a leading marker shifts the first line's wrap point but leaves
/// continuation lines at the margin).
#[cfg_attr(
    not(any(
        feature = "asciidoc",
        feature = "commonmark",
        feature = "gfm",
        feature = "latex",
        feature = "markdown",
        feature = "org",
        feature = "plain",
        feature = "rst"
    )),
    allow(dead_code)
)]
pub(crate) fn fill_offset(
    pieces: &[Piece],
    width: usize,
    initial: usize,
    wrap: WrapMode,
) -> String {
    // Only Auto wraps on width; the other modes get a sentinel column no real line reaches.
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
#[cfg_attr(not(feature = "commonmark"), allow(dead_code))]
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
#[cfg_attr(
    not(any(feature = "org", feature = "plain", feature = "rst")),
    allow(dead_code)
)]
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
/// reflows to `width` under both [`WrapMode::Auto`] and [`WrapMode::Preserve`] (a bordered cell is
/// always bounded by its column) while [`WrapMode::None`] still renders the content on one line so
/// the column can instead grow to hold it. Under [`WrapMode::Preserve`] each source soft break stays
/// a forced line break, with the text between breaks reflowed to the field width.
#[cfg_attr(not(feature = "rst"), allow(dead_code))]
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

/// Like [`fill`], but streams the filled lines straight into `out` under a hanging indent: the first
/// line takes the `first` prefix, each non-empty continuation line the `rest` prefix, and blank
/// continuation lines stay unprefixed. Equivalent to appending [`indent_block`] of [`fill`]'s result,
/// without building and re-splitting that intermediate string.
#[cfg_attr(not(any(feature = "gfm", feature = "markdown")), allow(dead_code))]
pub(crate) fn fill_into(
    out: &mut String,
    pieces: &[Piece],
    width: usize,
    wrap: WrapMode,
    first: &str,
    rest: &str,
) {
    let width = match wrap {
        WrapMode::Auto => width.max(1),
        WrapMode::None | WrapMode::Preserve => usize::MAX,
    };
    let mut sink = LineSink::prefixed(out, first, rest);
    fill_pieces(
        &mut sink,
        pieces,
        width,
        0,
        matches!(wrap, WrapMode::Preserve),
        false,
        &[],
    );
}

/// Lay a table cell's inline `pieces` out to `width` (always reflowing, as a bordered field is a
/// hard layout constraint), collecting each wrapped line as its own string. Equivalent to splitting
/// `fill(pieces, width, WrapMode::Auto)` on line breaks, but the lines are built directly, without
/// the intermediate whole-cell string. An empty cell still yields a single empty line.
#[cfg_attr(
    not(any(feature = "gfm", feature = "markdown", feature = "plain")),
    allow(dead_code)
)]
pub(crate) fn fill_lines(pieces: &[Piece], width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut sink = LineVecSink::default();
    fill_pieces(&mut sink, pieces, width, 0, false, false, &[]);
    sink.finish()
}

/// A per-line sink: collects the filled content into one string per line rather than a single
/// buffer, so a table cell's wrapped lines are built without an intermediate whole-cell string.
#[derive(Default)]
struct LineVecSink {
    lines: Vec<String>,
    current: String,
}

impl LineVecSink {
    /// The collected lines, with trailing blank lines dropped (matching the trailing-break trim the
    /// string sink performs) but always at least one line.
    fn finish(mut self) -> Vec<String> {
        self.lines.push(self.current);
        while self.lines.len() > 1 && self.lines.last().is_some_and(String::is_empty) {
            self.lines.pop();
        }
        self.lines
    }
}

impl Sink for LineVecSink {
    fn newline(&mut self) {
        self.lines.push(std::mem::take(&mut self.current));
    }

    fn write_segment(&mut self, segment: &str) {
        self.current.push_str(segment);
    }
}

/// The shared line-filling engine behind [`fill_offset`], [`fill_cell`], [`fill_groups`], and
/// [`fill_hang`]: lay `pieces` out into lines no wider than `width` (already resolved to a sentinel
/// when the caller wants no width wrap), starting `initial` columns into the first line, breaking on
/// each source soft break only when `preserve_softs` is set. With `keep_leading`, a space that opens
/// the content is emitted before the first word instead of being dropped, so hanging content laid
/// out under a marker keeps the gap the source put between the marker position and its first word.
/// `groups` names disjoint, ascending half-open index ranges that are placed atomically (see
/// [`fill_groups`]).
// One body: the per-piece arms and group handling share one running cursor.
#[allow(clippy::too_many_lines)]
fn fill_pieces<S: Sink>(
    sink: &mut S,
    pieces: &[Piece],
    width: usize,
    initial: usize,
    preserve_softs: bool,
    keep_leading: bool,
    groups: &[(usize, usize)],
) {
    let mut column = initial;
    let mut at_line_start = initial == 0 && !keep_leading;
    let mut pending_space = false;
    // Consecutive text pieces form one unbreakable word, placed once its full width is known.
    let mut word: Vec<&str> = Vec::new();
    let mut word_width = 0;
    let mut next_group = 0;
    let mut index = 0;
    while index < pieces.len() {
        if let Some(&(start, end)) = groups.get(next_group) {
            if index >= end {
                next_group += 1;
                continue;
            }
            if index == start && end > start && end <= pieces.len() {
                // Flush the pending word; the group joins it with no space when no space split them.
                let abuts = !word.is_empty();
                place_word(
                    sink,
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
                    sink,
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
            // A soft break splits lines only under Preserve; otherwise it is inter-word space.
            Some(Piece::Soft) if preserve_softs => {
                place_word(
                    sink,
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
                    sink.newline();
                    column = 0;
                    at_line_start = true;
                }
                pending_space = false;
            }
            Some(Piece::Space | Piece::Soft) => {
                place_word(
                    sink,
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
                    sink,
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
                    sink.newline();
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
        sink,
        &mut column,
        &mut at_line_start,
        pending_space,
        &word,
        word_width,
        width,
    );
}

/// A line-oriented output target: appends filled content into a caller's buffer, applying a hanging
/// indent as it goes. The first line takes the `first` prefix, each non-empty continuation line the
/// `rest` prefix, and blank continuation lines stay unprefixed: the same rule [`indent_block`]
/// applies to an already-rendered body, but streamed so no whole-block string is built and re-split.
/// Trailing line breaks are dropped (they are only emitted once real content follows them), matching
/// the trim the string-returning entry points perform.
struct LineSink<'a> {
    out: &'a mut String,
    rest: &'a str,
    at_line_start: bool,
    on_first_line: bool,
    pending_newlines: usize,
}

/// The line-oriented output operations the fill engine drives: end a line, or append content that
/// may itself contain line breaks. Implemented by the string sink ([`LineSink`]) and the per-line
/// vector sink ([`LineVecSink`]).
trait Sink {
    /// End the current line.
    fn newline(&mut self);

    /// Append content, treating any embedded line break as a line end.
    fn write(&mut self, text: &str) {
        let mut segments = text.split('\n');
        if let Some(first) = segments.next() {
            self.write_segment(first);
        }
        for segment in segments {
            self.newline();
            self.write_segment(segment);
        }
    }

    /// Append a single break-free segment.
    fn write_segment(&mut self, segment: &str);
}

impl<'a> LineSink<'a> {
    /// A prefixing sink: emits `first` eagerly so an empty first line still carries its marker (as
    /// [`indent_block`] does), then `rest` before each non-empty continuation line.
    fn prefixed(out: &'a mut String, first: &'a str, rest: &'a str) -> Self {
        out.push_str(first);
        LineSink {
            out,
            rest,
            at_line_start: true,
            on_first_line: true,
            pending_newlines: 0,
        }
    }

    /// A pass-through sink with no prefixes, for the string-returning entry points and for laying out
    /// a group's interior for measurement.
    fn plain(out: &'a mut String) -> Self {
        LineSink {
            out,
            rest: "",
            at_line_start: true,
            on_first_line: true,
            pending_newlines: 0,
        }
    }
}

impl Sink for LineSink<'_> {
    /// End the current line. The break is deferred until the next content arrives, so trailing breaks
    /// leave no dangling newline (and no continuation prefix).
    fn newline(&mut self) {
        self.pending_newlines += 1;
        self.at_line_start = true;
        self.on_first_line = false;
    }

    fn write_segment(&mut self, segment: &str) {
        if segment.is_empty() {
            return;
        }
        for _ in 0..self.pending_newlines {
            self.out.push('\n');
        }
        self.pending_newlines = 0;
        if self.at_line_start && !self.on_first_line {
            self.out.push_str(self.rest);
        }
        self.at_line_start = false;
        self.out.push_str(segment);
    }
}

/// The shared string-returning engine: lay `pieces` out into a fresh, prefix-free string, trimming
/// trailing line breaks. The line-prefixing entry points ([`fill_into`], [`fill_groups_into`]) stream
/// the same layout through a prefixing [`LineSink`] instead.
fn fill_core(
    pieces: &[Piece],
    width: usize,
    initial: usize,
    preserve_softs: bool,
    keep_leading: bool,
    groups: &[(usize, usize)],
) -> String {
    let mut out = String::new();
    let mut sink = LineSink::plain(&mut out);
    fill_pieces(
        &mut sink,
        pieces,
        width,
        initial,
        preserve_softs,
        keep_leading,
        groups,
    );
    out
}

/// Place an atomic group's interior into `out`, deciding first whether it begins on a fresh line.
/// The decision weighs the group's first line (the run before its own first fold): when `lead_space`
/// (a breakable space precedes it) and that first line would overflow the current line, the group
/// starts a new line. Either way its interior is then filled from the resulting column, folding
/// across lines only when the group alone is wider than the column.
///
/// The interior is laid out once from the margin. That single render is the final one whenever the
/// group lands at column 0 (a fresh line, or the whole content on one line, which is
/// column-independent); only a group that both stays on the current line *and* folds across lines is
/// re-laid out from that column, since its interior fold points then shift.
fn place_group<S: Sink>(
    sink: &mut S,
    column: &mut usize,
    at_line_start: &mut bool,
    lead_space: bool,
    inner: &[Piece],
    preserve_softs: bool,
    width: usize,
) {
    let rendered = fill_core(inner, width, 0, preserve_softs, false, &[]);
    let first_line = rendered.split('\n').next().map_or(0, display_width);
    let multiline = rendered.contains('\n');
    let start_col = if *at_line_start {
        *at_line_start = false;
        0
    } else if lead_space && *column + 1 + first_line > width {
        sink.newline();
        *column = 0;
        0
    } else if lead_space {
        sink.write(" ");
        *column += 1;
        *column
    } else {
        *column
    };
    if start_col == 0 || !multiline {
        sink.write(&rendered);
        *column = line_end_column(&rendered, start_col);
    } else {
        let refolded = fill_core(inner, width, start_col, preserve_softs, false, &[]);
        sink.write(&refolded);
        *column = line_end_column(&refolded, start_col);
    }
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
/// A word usually has no embedded line break, but a multi-line literal (a footnote body set over
/// several paragraphs) does. Such a word's first line is what must fit after the preceding space,
/// and its last line sets the column the following text continues from; only its first line shares
/// the line it lands on, so the rest cannot push later words off the column.
fn place_word<S: Sink>(
    sink: &mut S,
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
        sink.newline();
        *column = 0;
        *at_line_start = false;
    } else if pending_space {
        sink.write(" ");
        *column += 1;
    }
    for part in word {
        sink.write(part);
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
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "gfm",
        feature = "latex",
        feature = "markdown",
        feature = "org",
        feature = "plain",
        feature = "rst"
    )),
    allow(dead_code)
)]
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

    #[test]
    fn fill_into_applies_first_and_continuation_prefixes() {
        let pieces = vec![
            Piece::Text("alpha".into()),
            Piece::Space,
            Piece::Text("beta".into()),
            Piece::Space,
            Piece::Text("gamma".into()),
        ];
        let mut out = String::new();
        fill_into(&mut out, &pieces, 8, WrapMode::Auto, "- ", "  ");
        assert_eq!(out, "- alpha\n  beta\n  gamma");
    }

    #[test]
    fn fill_into_leaves_blank_continuation_lines_unprefixed() {
        // A multi-line literal word carries its embedded blank line through unchanged.
        let pieces = vec![Piece::Text("a\n\nb".into())];
        let mut out = String::new();
        fill_into(&mut out, &pieces, 72, WrapMode::Auto, "> ", "> ");
        assert_eq!(out, "> a\n\n> b");
    }

    #[test]
    fn fill_into_empty_pieces_with_empty_prefixes_produce_nothing() {
        let mut out = String::new();
        fill_into(&mut out, &[], 72, WrapMode::Auto, "", "");
        assert!(out.is_empty());
    }

    #[test]
    fn fill_into_appends_to_existing_buffer() {
        let mut out = String::from("head\n");
        fill_into(
            &mut out,
            &[Piece::Text("body".into())],
            72,
            WrapMode::Auto,
            "- ",
            "  ",
        );
        assert_eq!(out, "head\n- body");
    }

    #[test]
    fn fill_into_with_empty_prefixes_equals_fill_across_shapes() {
        let shapes: Vec<Vec<Piece>> = vec![
            vec![],
            vec![Piece::Text("one".into())],
            vec![
                Piece::Text("one".into()),
                Piece::Space,
                Piece::Text("two".into()),
            ],
            vec![
                Piece::Text("a".into()),
                Piece::Hard,
                Piece::Text("b".into()),
            ],
            vec![
                Piece::Text("xx".into()),
                Piece::Soft,
                Piece::Text("yy".into()),
                Piece::Space,
                Piece::Text("zzzzzz".into()),
            ],
        ];
        for shape in &shapes {
            for &width in &[1usize, 3, 5, 8, 72] {
                for wrap in [WrapMode::Auto, WrapMode::None, WrapMode::Preserve] {
                    let mut out = String::new();
                    fill_into(&mut out, shape, width, wrap, "", "");
                    assert_eq!(out, fill(shape, width, wrap));
                }
            }
        }
    }

    #[test]
    fn fill_into_reproduces_grouped_layout() {
        // With empty prefixes the streamed layout must equal `fill_groups`.
        let pieces = vec![
            Piece::Text("see".into()),
            Piece::Space,
            Piece::Text("a-very-long-link-label".into()),
            Piece::Text("(target)".into()),
            Piece::Space,
            Piece::Text("end".into()),
        ];
        let groups = [(2usize, 4usize)];
        for &width in &[6usize, 10, 20, 72] {
            let expected = fill_groups(&pieces, &groups, width, 0, false, WrapMode::Auto);
            let mut out = String::new();
            let mut sink = LineSink::plain(&mut out);
            fill_pieces(&mut sink, &pieces, width.max(1), 0, false, false, &groups);
            assert_eq!(out, expected);
        }
    }
}
