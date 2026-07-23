//! Emphasis resolution: the delimiter-stack pass that pairs `*`/`_` (and extension delimiter)
//! runs into emphasis, strong, strikeout, superscript, subscript, highlight, and smart-quote
//! inlines after the inline scan has recorded them as [`Node::Delimiter`] entries.

use carta_ast::{Attr, Inline, QuoteType};
use carta_core::{Extension, Extensions};

use super::inline::{Delimiter, Node, collapse, flanking, quote_flanking};

/// A record in the delimiter list used by [`process_emphasis`].
///
/// Entries are held in a `Vec` whose indices are stable for the lifetime of one resolution pass:
/// an entry is never moved or removed, only unlinked. `prev`/`next` thread the still-active entries
/// into a doubly-linked list; consuming a matched pair unlinks the delimiters between and around it
/// in O(1), so the pass stays linear on delimiter-heavy input.
#[derive(Debug, Clone)]
struct DelimEntry {
    /// Index into `nodes` where this delimiter lives. Stable: nodes are never spliced during a pass.
    node_index: usize,
    ch: u8,
    count: usize,
    can_open: bool,
    can_close: bool,
    /// Previous still-active entry (a smaller `delims` index), or `None` at the list head.
    prev: Option<usize>,
    /// Next still-active entry (a larger `delims` index), or `None` at the list tail.
    next: Option<usize>,
}

/// Resolve emphasis/strong (`*`/`_`) and format (`~`/`^`) delimiters in `nodes`, starting at
/// `stack_bottom`.
///
/// Implements the linear algorithm from the spec ("An algorithm for parsing nested emphasis and
/// links", `CommonMark` spec §A): a single left-to-right pass over closers, with per-bucket
/// `openers_bottom` lower bounds that prevent re-scanning already-rejected opener ranges.
///
/// All four delimiter kinds share one matching loop. They differ only in how a matched pair's
/// length maps to a node; see [`match_use_count`] and [`wrap_emphasis`].
// `opener_di` and `opener_ni` are intentionally similar: two indices into two distinct arrays.
#[allow(clippy::similar_names, clippy::too_many_lines)]
pub(super) fn process_emphasis(
    nodes: &mut [Node],
    stack_bottom: usize,
    ext: Extensions,
    markdown: bool,
) {
    // One entry per emphasis-class Node::Delimiter in [stack_bottom..] (bracket openers excluded).
    let mut delims: Vec<DelimEntry> = nodes
        .iter()
        .enumerate()
        .skip(stack_bottom)
        .filter_map(|(ni, node)| match node {
            Node::Delimiter(d) if is_delimiter_char(d.ch) => Some(DelimEntry {
                node_index: ni,
                ch: d.ch,
                count: d.count,
                can_open: d.can_open,
                can_close: d.can_close,
                prev: None,
                next: None,
            }),
            _ => None,
        })
        .collect();

    let count = delims.len();
    for (i, entry) in delims.iter_mut().enumerate() {
        entry.prev = i.checked_sub(1);
        entry.next = (i + 1 < count).then_some(i + 1);
    }
    // First still-active entry; used solely by the final sweep that literalizes leftovers.
    let mut head: Option<usize> = (count > 0).then_some(0);

    // `openers_bottom[bucket]`: minimum delimiter-list index to search for an opener. Key: the
    // spec §A triple plus `count >= 2`; closers share a slot only if every opener treats them alike.
    let mut openers_bottom = std::collections::BTreeMap::<(u8, usize, bool, bool), usize>::new();

    let mut current: Option<usize> = head;

    while let Some(cur) = current {
        let Some(current_entry) = delims.get(cur) else {
            break;
        };
        let (closer_ch, closer_count, closer_can_open, closer_can_close) = (
            current_entry.ch,
            current_entry.count,
            current_entry.can_open,
            current_entry.can_close,
        );
        let closer_ni = current_entry.node_index;
        let cur_next = current_entry.next;
        let scan_start = current_entry.prev;
        if !closer_can_close {
            current = cur_next;
            continue;
        }

        let bucket = (
            closer_ch,
            closer_count % 3,
            closer_can_open,
            closer_count >= 2,
        );
        let bottom = *openers_bottom.get(&bucket).unwrap_or(&0);

        // Scan backward through active openers, down to and including `bottom`, for a match.
        let mut found: Option<usize> = None; // delimiter-list index of the matched opener
        let mut scan = scan_start;
        while let Some(si) = scan {
            if si < bottom {
                break;
            }
            let Some(entry) = delims.get(si) else {
                break;
            };
            let scan_prev = entry.prev;
            if !entry.can_open || entry.ch != closer_ch {
                scan = scan_prev;
                continue;
            }
            // Markdown dialect: a run of four or more `*`/`_` opens nothing and stays literal.
            if markdown && markdown_opener_inert(closer_ch, entry.count) {
                scan = scan_prev;
                continue;
            }
            let Some(use_count) =
                match_use_count_md(entry.count, closer_count, closer_ch, ext, markdown)
            else {
                // Rejected opener: keep scanning; one rejection must not advance `openers_bottom`.
                scan = scan_prev;
                continue;
            };
            // Re-derive the Delimiter from `nodes` for the rule-of-3 check.
            let ni = entry.node_index;
            let rule_ok = match nodes.get(ni) {
                Some(Node::Delimiter(d)) => emphasis_match(d, nodes, closer_ni),
                _ => false,
            };
            if rule_ok {
                // Markdown forbids whitespace in super/subscript; keep looking for a tighter opener.
                if markdown
                    && rejects_inner_space(closer_ch, use_count)
                    && nodes.get(ni + 1..closer_ni).is_some_and(nodes_carry_break)
                {
                    scan = scan_prev;
                    continue;
                }
                // In markdown a lone `*`/`_` and a doubled run never pair; the run stays literal.
                if markdown && markdown_emphasis_runs_mismatch(closer_ch, entry.count, closer_count)
                {
                    scan = scan_prev;
                    continue;
                }
                found = Some(si);
                break;
            }
            scan = scan_prev;
        }

        let Some(opener_di) = found else {
            // No opener: exclude this closer's position from future searches for the same bucket.
            openers_bottom.insert(bucket, cur);
            // A delimiter that can't open is now known to be inert as a closer too.
            if !closer_can_open {
                convert_delimiter_to_text(nodes, closer_ni);
            }
            current = cur_next;
            continue;
        };

        // Match found: fold the inner span into a wrapping inline, leaving node indices stable.

        let Some(opener_entry) = delims.get(opener_di) else {
            break;
        };
        let (opener_ni, opener_count) = (opener_entry.node_index, opener_entry.count);

        // Already validated above.
        let use_count =
            match_use_count_md(opener_count, closer_count, closer_ch, ext, markdown).unwrap_or(1);

        // Tombstone the moved inner nodes so surviving delimiters keep their node_index.
        let mut inner: Vec<Node> = Vec::new();
        for slot in nodes
            .get_mut(opener_ni + 1..closer_ni)
            .into_iter()
            .flatten()
        {
            inner.push(std::mem::replace(slot, Node::Empty));
        }
        let content = collapse(inner);
        let wrapped = wrap_emphasis(closer_ch, use_count, content);
        // Opener and closer are separate runs, so this slot is never the closer's own node.
        if let Some(slot) = nodes.get_mut(opener_ni + 1) {
            *slot = Node::Inline(wrapped);
        }

        decrement_delimiter(nodes, closer_ni, use_count);
        decrement_delimiter(nodes, opener_ni, use_count);
        let new_closer_count = closer_count.saturating_sub(use_count);
        let new_opener_count = opener_count.saturating_sub(use_count);
        if let Some(e) = delims.get_mut(cur) {
            e.count = new_closer_count;
        }
        if let Some(e) = delims.get_mut(opener_di) {
            e.count = new_opener_count;
        }

        // The list entry following the closer: the resume point when both runs are spent.
        let after_closer = delims.get(cur).and_then(|e| e.next);

        // Delimiters between opener and closer were folded into the wrap and can never match again.
        if let Some(e) = delims.get_mut(opener_di) {
            e.next = Some(cur);
        }
        if let Some(e) = delims.get_mut(cur) {
            e.prev = Some(opener_di);
        }

        // Unlink a spent run and blank its node. Closer first (it is the higher node index).
        let closer_empty = new_closer_count == 0;
        let opener_empty = new_opener_count == 0;
        if closer_empty {
            unlink_delim(&mut delims, &mut head, cur);
            if let Some(slot) = nodes.get_mut(closer_ni) {
                *slot = Node::Empty;
            }
        }
        if opener_empty {
            unlink_delim(&mut delims, &mut head, opener_di);
            if let Some(slot) = nodes.get_mut(opener_ni) {
                *slot = Node::Empty;
            }
        }

        // Resume at the surviving opener if any, else the surviving closer, else past the closer.
        current = if !opener_empty {
            Some(opener_di)
        } else if !closer_empty {
            Some(cur)
        } else {
            after_closer
        };
    }

    // Any delimiter still on the active list never matched: it reverts to literal text.
    let mut leftover = head;
    while let Some(i) = leftover {
        let Some(entry) = delims.get(i) else {
            break;
        };
        let next = entry.next;
        convert_delimiter_to_text(nodes, entry.node_index);
        leftover = next;
    }
}

/// Unlink `i` from the doubly-linked delimiter list, advancing `head` if `i` was at the front.
fn unlink_delim(delims: &mut [DelimEntry], head: &mut Option<usize>, i: usize) {
    let (prev, next) = match delims.get(i) {
        Some(entry) => (entry.prev, entry.next),
        None => return,
    };
    match prev {
        Some(pi) => {
            if let Some(prev_entry) = delims.get_mut(pi) {
                prev_entry.next = next;
            }
        }
        None => *head = next,
    }
    if let Some(ni) = next
        && let Some(next_entry) = delims.get_mut(ni)
    {
        next_entry.prev = prev;
    }
}

/// Resolve `==`-delimited highlight runs into `Span` inlines carrying the `mark` class.
///
/// A run is delimited by two `=` on each side. Scanning left to right, each `=` closer pairs with
/// the nearest preceding `=` opener; the pair consumes exactly two `=` from each side and the inner
/// nodes (with their own emphasis resolved) become the span's content. Any `=` left over on either
/// side, or a lone `=`, stays literal text. Resolving here, ahead of the shared emphasis pass, keeps
/// each run to a single span: leftover `=` do not re-pair into nested marks.
pub(super) fn resolve_mark(nodes: &mut Vec<Node>, ext: Extensions, markdown: bool) {
    let mut current = 0usize;
    while current < nodes.len() {
        let is_closer = matches!(
            nodes.get(current),
            Some(Node::Delimiter(d)) if d.ch == b'=' && d.can_close && d.count >= 2
        );
        if !is_closer {
            current += 1;
            continue;
        }
        let mut opener = None;
        for i in (0..current).rev() {
            if matches!(
                nodes.get(i),
                Some(Node::Delimiter(d)) if d.ch == b'=' && d.can_open && d.count >= 2
            ) {
                opener = Some(i);
                break;
            }
        }
        let Some(opener_ni) = opener else {
            current += 1;
            continue;
        };

        let inner: Vec<Node> = nodes.drain(opener_ni + 1..current).collect();
        let mut inner = inner;
        process_emphasis(&mut inner, 0, ext, markdown);
        let content = collapse(inner);
        let span = Inline::Span(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["mark".into()],
                attributes: Vec::new(),
            }),
            content,
        );
        // After the drain, the closer sits directly after the opener.
        let closer_ni = opener_ni + 1;
        nodes.insert(closer_ni, Node::Inline(span));
        // The closer has shifted one further along by the insert.
        let closer_ni = opener_ni + 2;

        consume_mark_side(nodes, closer_ni);
        consume_mark_side(nodes, opener_ni);

        // Resume scanning from the opener position: nodes there are now resolved, so re-derive.
        current = opener_ni;
    }

    // Any `=` delimiter that never formed a span reverts to literal text.
    for i in 0..nodes.len() {
        if matches!(nodes.get(i), Some(Node::Delimiter(d)) if d.ch == b'=') {
            convert_delimiter_to_text(nodes, i);
        }
    }
}

/// Take two `=` off the delimiter node at `index`: a remainder of zero removes the node, otherwise
/// it becomes literal text of the remaining `=`. Returns nothing; callers index high-to-low so the
/// node positions they still hold stay valid (the opener is below the closer).
fn consume_mark_side(nodes: &mut Vec<Node>, index: usize) {
    let remainder = match nodes.get(index) {
        Some(Node::Delimiter(d)) => d.count.saturating_sub(2),
        _ => return,
    };
    if remainder == 0 {
        nodes.remove(index);
    } else if let Some(node) = nodes.get_mut(index) {
        *node = Node::Text("=".repeat(remainder));
    }
}

/// Whether `ch` names a delimiter run resolved by [`process_emphasis`].
fn is_delimiter_char(ch: u8) -> bool {
    matches!(ch, b'*' | b'_' | b'~' | b'^' | b'\'' | b'"' | b'=')
}

/// Whether `ch` is a smart-quote delimiter (`'` or `"`).
fn is_quote(ch: u8) -> bool {
    matches!(ch, b'\'' | b'"')
}

/// Open/close eligibility for a delimiter run, dispatching to the smart-quote rule for `'`/`"` and
/// to the emphasis rule for everything else.
pub(super) fn run_flanking(
    ch: u8,
    before: Option<char>,
    after: Option<char>,
    relax_underscore: bool,
) -> (bool, bool) {
    if is_quote(ch) {
        quote_flanking(ch, before, after)
    } else if ch == b'_' && relax_underscore {
        // Pair underscores by the same boundary rule as `*`, which permits intraword emphasis.
        flanking(b'*', before, after)
    } else {
        flanking(ch, before, after)
    }
}

/// How many delimiters a matched opener/closer pair consumes, or `None` when the enabled extensions
/// give the pair no meaning (so the search must look further or leave the run literal).
///
/// `*`/`_` consume two when both runs can (strong) else one (emphasis). `^` consumes one per layer
/// (superscript). `~` consumes two for a strikeout when both runs allow it and `strikeout` is on,
/// otherwise one for a subscript when `subscript` is on; with neither it is not a delimiter.
pub(super) fn match_use_count(
    opener_count: usize,
    closer_count: usize,
    ch: u8,
    ext: Extensions,
) -> Option<usize> {
    let both_at_least_two = opener_count >= 2 && closer_count >= 2;
    match ch {
        b'*' | b'_' => Some(if both_at_least_two { 2 } else { 1 }),
        b'^' | b'\'' | b'"' => Some(1),
        b'~' => {
            if both_at_least_two && ext.contains(Extension::Strikeout) {
                Some(2)
            } else if ext.contains(Extension::Subscript) {
                Some(1)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Whether a `*`/`_` run is too long to open emphasis in the markdown dialect. A run there denotes
/// at most a strong wrapping an emphasis (three delimiters); four or more open nothing and the run
/// stays literal.
fn markdown_opener_inert(ch: u8, count: usize) -> bool {
    matches!(ch, b'*' | b'_') && count > 3
}

/// Whether a `*`/`_` opener and closer have run lengths that cannot pair in the markdown dialect.
/// A run of one delimiter (an emphasis marker) and a run of two (a strong marker) never match each
/// other: a lone delimiter cannot close against a strong marker, nor a strong marker against a lone
/// one, so the pairing fails and both runs stay literal.
fn markdown_emphasis_runs_mismatch(ch: u8, opener_count: usize, closer_count: usize) -> bool {
    matches!(ch, b'*' | b'_')
        && ((opener_count == 1 && closer_count == 2) || (opener_count == 2 && closer_count == 1))
}

/// How many delimiters a matched pair consumes, accounting for the markdown dialect's emphasis
/// rule. For `*`/`_` in markdown, a pair whose opener and closer both still have three or more
/// delimiters consumes a single one first, so the emphasis it forms nests inside the strong that
/// the remaining pair forms: a triple run resolves to a strong wrapping an emphasis. Every other
/// pairing defers to [`match_use_count`].
fn match_use_count_md(
    opener_count: usize,
    closer_count: usize,
    ch: u8,
    ext: Extensions,
    markdown: bool,
) -> Option<usize> {
    if markdown && matches!(ch, b'*' | b'_') && opener_count >= 3 && closer_count >= 3 {
        return Some(1);
    }
    // An odd symmetric run of 3+ tildes is one subscript; no strikeout nests inside.
    if markdown
        && ch == b'~'
        && ext.contains(Extension::Subscript)
        && opener_count == closer_count
        && opener_count >= 3
        && opener_count % 2 == 1
    {
        return Some(opener_count);
    }
    match_use_count(opener_count, closer_count, ch, ext)
}

/// Wrap `content` in the inline a matched delimiter pair denotes, given its character and the number
/// of delimiters consumed.
fn wrap_emphasis(ch: u8, use_count: usize, content: Vec<Inline>) -> Inline {
    match (ch, use_count) {
        (b'\'', _) => Inline::Quoted(QuoteType::SingleQuote, content),
        (b'"', _) => Inline::Quoted(QuoteType::DoubleQuote, content),
        (b'~', 2) => Inline::Strikeout(content),
        (b'~', _) => Inline::Subscript(content),
        (b'^', _) => Inline::Superscript(content),
        (_, 2) => Inline::Strong(content),
        (_, _) => Inline::Emph(content),
    }
}

/// Whether a matched delimiter pair forms a superscript or a subscript, the spans the markdown
/// dialect forbids from holding whitespace. A double tilde is a strikeout, which may, so only a
/// single tilde counts.
fn rejects_inner_space(ch: u8, use_count: usize) -> bool {
    ch == b'^' || (ch == b'~' && use_count == 1)
}

/// Whether any node in the slice carries whitespace that, in the markdown dialect, ends a
/// superscript or subscript: a space or tab in text, or a soft or hard line break. A non-breaking
/// space (what an escaped space becomes) does not count, so an escaped space keeps the span open.
fn nodes_carry_break(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::Text(text) => text.chars().any(|c| c == ' ' || c == '\t'),
        Node::SoftBreak | Node::LineBreak => true,
        Node::Inline(inline) => inline_carries_break(inline),
        Node::Delimiter(_) | Node::Empty => false,
    })
}

/// The [`nodes_carry_break`] test for an already-built inline, recursing through the inline
/// containers a superscript or subscript may nest.
fn inline_carries_break(inline: &Inline) -> bool {
    match inline {
        Inline::Space | Inline::SoftBreak | Inline::LineBreak => true,
        Inline::Str(text) => text.chars().any(|c| c == ' ' || c == '\t'),
        Inline::Emph(content)
        | Inline::Underline(content)
        | Inline::Strong(content)
        | Inline::Strikeout(content)
        | Inline::Superscript(content)
        | Inline::Subscript(content)
        | Inline::SmallCaps(content)
        | Inline::Quoted(_, content)
        | Inline::Cite(_, content)
        | Inline::Link(_, content, _)
        | Inline::Image(_, content, _)
        | Inline::Span(_, content) => content.iter().any(inline_carries_break),
        _ => false,
    }
}

fn emphasis_match(opener: &Delimiter, nodes: &[Node], closer: usize) -> bool {
    let Some(Node::Delimiter(closer_delim)) = nodes.get(closer) else {
        return false;
    };
    // Rule of 3: a sum divisible by 3 rejects unless both counts are.
    let either_both =
        (opener.can_open && opener.can_close) || (closer_delim.can_open && closer_delim.can_close);
    if either_both {
        let sum = opener.count + closer_delim.count;
        if sum.is_multiple_of(3)
            && (!opener.count.is_multiple_of(3) || !closer_delim.count.is_multiple_of(3))
        {
            return false;
        }
    }
    true
}

/// The literal text an unmatched delimiter run reverts to. An unmatched smart quote becomes a curly
/// quote: a single quote closes (`’`) and a double quote opens (`“`); every other delimiter is its
/// own character repeated.
pub(super) fn delimiter_literal(ch: u8, count: usize) -> String {
    match ch {
        b'\'' => "\u{2019}".repeat(count),
        b'"' => "\u{201c}".repeat(count),
        _ => std::iter::repeat_n(ch as char, count).collect(),
    }
}

fn decrement_delimiter(nodes: &mut [Node], index: usize, by: usize) {
    if let Some(Node::Delimiter(d)) = nodes.get_mut(index) {
        d.count = d.count.saturating_sub(by);
    }
}

fn convert_delimiter_to_text(nodes: &mut [Node], index: usize) {
    if let Some(node) = nodes.get_mut(index)
        && let Node::Delimiter(d) = node
        && is_delimiter_char(d.ch)
    {
        *node = Node::Text(delimiter_literal(d.ch, d.count));
    }
}
