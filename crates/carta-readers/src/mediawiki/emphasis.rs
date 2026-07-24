//! Emphasis resolution, smart quotes, and inline coalescing.

use carta_ast::{Inline, QuoteType};

use super::{Tok, raw_html};

/// A unit of the stream emphasis resolution works over: one apostrophe of a run, or a finished node.
enum Unit {
    Apostrophe,
    Node(Inline),
}

/// Resolves apostrophe emphasis. Runs of two apostrophes open and close `Emph`, three open and close
/// `Strong`. The structure is found by recursive descent with backtracking: at each run the parser
/// tries to open the span whose width fits, parses its content up to a matching closing run, and
/// falls back to a literal apostrophe when no span can be formed. A span is never reopened by its
/// immediate parent of the same kind, and a span's content has its outer whitespace removed.
pub(super) fn resolve_emphasis(toks: Vec<Tok>) -> Vec<Inline> {
    let mut units: Vec<Unit> = Vec::new();
    for tok in toks {
        match tok {
            Tok::Inline(inline) => units.push(Unit::Node(inline)),
            Tok::Apostrophes(n) => units.extend((0..n).map(|_| Unit::Apostrophe)),
            Tok::BlockRaw(raw) => units.push(Unit::Node(raw_html(raw))),
            Tok::BlockBreak | Tok::Block(_) => {}
        }
    }
    let runs = apostrophe_runs(&units);
    // Bound the backtracking work so adversarial apostrophe-dense input cannot blow up.
    let mut budget = crate::inline_scan::scan_budget(units.len());
    let (nodes, _, _) = parse_runs(&units, &runs, 0, None, &mut budget);
    nodes
}

/// For each position, the length of the apostrophe run starting there (zero at a non-apostrophe).
fn apostrophe_runs(units: &[Unit]) -> Vec<usize> {
    let mut runs = vec![0usize; units.len()];
    for i in (0..units.len()).rev() {
        if matches!(units.get(i), Some(Unit::Apostrophe)) {
            let next = runs.get(i + 1).copied().unwrap_or(0);
            if let Some(slot) = runs.get_mut(i) {
                *slot = 1 + next;
            }
        }
    }
    runs
}

fn emphasis_width(strong: bool) -> usize {
    if strong { 3 } else { 2 }
}

/// Tries to open an emphasis span of the given kind at the apostrophe run starting at `i`. Returns
/// the span node and the index just past its closing run, or `None` if no matching closer is found
/// or the span would be empty.
fn try_open(
    units: &[Unit],
    runs: &[usize],
    i: usize,
    strong: bool,
    budget: &mut usize,
) -> Option<(Inline, usize)> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let width = emphasis_width(strong);
    let (body, next, closed) = parse_runs(units, runs, i + width, Some(strong), budget);
    if !closed || body.is_empty() {
        return None;
    }
    let body = strip_outer_whitespace(body);
    Some((
        if strong {
            Inline::Strong(body)
        } else {
            Inline::Emph(body)
        },
        next,
    ))
}

/// Parses content until the run that closes `closer` (or end of input when `closer` is `None`).
/// Returns the collected nodes, the index reached, and whether a closer was found.
///
/// At each apostrophe run, a wider `'''…'''` strong span is preferred over a `''…''` emphasis span,
/// and closing the enclosing span takes precedence over opening a same-kind span. A run that opens
/// nothing and closes nothing is emitted as literal apostrophes.
fn parse_runs(
    units: &[Unit],
    runs: &[usize],
    start: usize,
    closer: Option<bool>,
    budget: &mut usize,
) -> (Vec<Inline>, usize, bool) {
    let mut nodes: Vec<Inline> = Vec::new();
    let mut pos = start;
    while let Some(unit) = units.get(pos) {
        match unit {
            Unit::Node(inline) => {
                nodes.push(inline.clone());
                pos += 1;
            }
            Unit::Apostrophe => {
                let run = runs.get(pos).copied().unwrap_or(0);
                if run >= emphasis_width(true)
                    && closer != Some(true)
                    && let Some((span, next)) = try_open(units, runs, pos, true, budget)
                {
                    nodes.push(span);
                    pos = next;
                    continue;
                }
                if let Some(strong) = closer
                    && run >= emphasis_width(strong)
                {
                    return (nodes, pos + emphasis_width(strong), true);
                }
                if run >= emphasis_width(false)
                    && closer != Some(false)
                    && let Some((span, next)) = try_open(units, runs, pos, false, budget)
                {
                    nodes.push(span);
                    pos = next;
                    continue;
                }
                nodes.push(Inline::Str("'".into()));
                pos += 1;
            }
        }
    }
    (nodes, pos, closer.is_none())
}

pub(super) fn strip_outer_whitespace(mut inlines: Vec<Inline>) -> Vec<Inline> {
    let lead = inlines
        .iter()
        .take_while(|x| matches!(x, Inline::Space | Inline::SoftBreak))
        .count();
    inlines.drain(0..lead);
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
    inlines
}

/// A flattened unit used while pairing smart double quotes: a `"` awaiting a partner, an ordinary
/// character, a whitespace inline (which cannot follow an opening quote), or an opaque inline node
/// carried through unchanged.
enum SmartUnit {
    Quote,
    Ch(char),
    Space(Inline),
    Node(Inline),
}

/// Folds straight double quotes into [`Inline::Quoted`] runs. A double quote followed by
/// non-whitespace content opens a run that the next double quote closes; an unpaired quote stays a
/// literal `"`. Single quotes, which mark emphasis, are left untouched. The fold also descends into
/// the children of container inlines.
pub(super) fn apply_smart_quotes(inlines: Vec<Inline>) -> Vec<Inline> {
    let recursed: Vec<Inline> = inlines.into_iter().map(smart_descend).collect();
    let units = flatten_smart(recursed);
    resolve_double_quotes(&units, 0, units.len())
}

/// Applies the double-quote fold to the inline children of a container, leaving leaf and opaque
/// inlines (text, code, math, raw passthrough, notes) untouched.
fn smart_descend(inline: Inline) -> Inline {
    match inline {
        Inline::Emph(v) => Inline::Emph(apply_smart_quotes(v)),
        Inline::Underline(v) => Inline::Underline(apply_smart_quotes(v)),
        Inline::Strong(v) => Inline::Strong(apply_smart_quotes(v)),
        Inline::Strikeout(v) => Inline::Strikeout(apply_smart_quotes(v)),
        Inline::Superscript(v) => Inline::Superscript(apply_smart_quotes(v)),
        Inline::Subscript(v) => Inline::Subscript(apply_smart_quotes(v)),
        Inline::SmallCaps(v) => Inline::SmallCaps(apply_smart_quotes(v)),
        Inline::Quoted(quote_type, v) => Inline::Quoted(quote_type, apply_smart_quotes(v)),
        Inline::Span(attr, v) => Inline::Span(attr, apply_smart_quotes(v)),
        Inline::Link(attr, v, target) => Inline::Link(attr, apply_smart_quotes(v), target),
        Inline::Image(attr, v, target) => Inline::Image(attr, apply_smart_quotes(v), target),
        other => other,
    }
}

fn flatten_smart(inlines: Vec<Inline>) -> Vec<SmartUnit> {
    let mut units: Vec<SmartUnit> = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Str(text) => {
                for c in text.chars() {
                    if c == '"' {
                        units.push(SmartUnit::Quote);
                    } else {
                        units.push(SmartUnit::Ch(c));
                    }
                }
            }
            space @ (Inline::Space | Inline::SoftBreak | Inline::LineBreak) => {
                units.push(SmartUnit::Space(space));
            }
            other => units.push(SmartUnit::Node(other)),
        }
    }
    units
}

fn resolve_double_quotes(units: &[SmartUnit], lo: usize, hi: usize) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut buf = String::new();
    let mut i = lo;
    while i < hi {
        match units.get(i) {
            Some(SmartUnit::Quote) => {
                if smart_quote_opens(units, i, hi)
                    && let Some(j) = next_smart_quote(units, i + 1, hi)
                {
                    flush_smart_buf(&mut buf, &mut out);
                    out.push(Inline::Quoted(
                        QuoteType::DoubleQuote,
                        strip_outer_whitespace(resolve_double_quotes(units, i + 1, j)),
                    ));
                    i = j + 1;
                } else {
                    buf.push('"');
                    i += 1;
                }
            }
            Some(SmartUnit::Ch(c)) => {
                buf.push(*c);
                i += 1;
            }
            Some(SmartUnit::Space(inline) | SmartUnit::Node(inline)) => {
                flush_smart_buf(&mut buf, &mut out);
                out.push(inline.clone());
                i += 1;
            }
            None => break,
        }
    }
    flush_smart_buf(&mut buf, &mut out);
    out
}

fn flush_smart_buf(buf: &mut String, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        out.push(Inline::Str(std::mem::take(buf).into()));
    }
}

/// A double quote opens a run when the unit immediately after it, within the same span, is
/// non-whitespace content.
fn smart_quote_opens(units: &[SmartUnit], i: usize, hi: usize) -> bool {
    if i + 1 >= hi {
        return false;
    }
    match units.get(i + 1) {
        Some(SmartUnit::Ch(c)) => !c.is_whitespace(),
        Some(SmartUnit::Quote | SmartUnit::Node(_)) => true,
        Some(SmartUnit::Space(_)) | None => false,
    }
}

fn next_smart_quote(units: &[SmartUnit], from: usize, hi: usize) -> Option<usize> {
    (from..hi).find(|&j| matches!(units.get(j), Some(SmartUnit::Quote)))
}

/// Merges adjacent string runs so a span never holds two consecutive [`Inline::Str`] nodes,
/// descending into the markup wrappers a reader produces.
pub(super) fn coalesce(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for inline in inlines {
        let inline = match inline {
            Inline::Emph(xs) => Inline::Emph(coalesce(xs)),
            Inline::Strong(xs) => Inline::Strong(coalesce(xs)),
            Inline::Strikeout(xs) => Inline::Strikeout(coalesce(xs)),
            Inline::Superscript(xs) => Inline::Superscript(coalesce(xs)),
            Inline::Subscript(xs) => Inline::Subscript(coalesce(xs)),
            Inline::Underline(xs) => Inline::Underline(coalesce(xs)),
            Inline::SmallCaps(xs) => Inline::SmallCaps(coalesce(xs)),
            Inline::Span(attr, xs) => Inline::Span(attr, coalesce(xs)),
            other => other,
        };
        match (out.last_mut(), &inline) {
            (Some(Inline::Str(prev)), Inline::Str(next)) => prev.push_str(next),
            // Adjacent whitespace only occurs where a zero-width construct was removed; collapse to
            // one token, keeping a soft break if either side carried one.
            (
                Some(slot @ (Inline::Space | Inline::SoftBreak)),
                Inline::Space | Inline::SoftBreak,
            ) => {
                if matches!(inline, Inline::SoftBreak) {
                    *slot = Inline::SoftBreak;
                }
            }
            _ => out.push(inline),
        }
    }
    out
}

/// Removes a soft line break that falls between two East Asian wide characters, so wrapped CJK text
/// rejoins with no intervening space. A break next to a non-wide character, or an explicit space, is
/// left as is.
pub(super) fn drop_east_asian_breaks(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    let mut iter = inlines.into_iter().peekable();
    while let Some(inline) = iter.next() {
        if matches!(inline, Inline::SoftBreak) {
            let prev_wide = out.last().and_then(trailing_char).is_some_and(is_wide_char);
            let next_wide = iter.peek().and_then(leading_char).is_some_and(is_wide_char);
            if prev_wide && next_wide {
                continue;
            }
        }
        out.push(inline);
    }
    out
}

/// The last rendered character of an inline, descending into wrapper inlines, or `None` for one that
/// renders no character at the boundary (a break, image, or note).
fn trailing_char(inline: &Inline) -> Option<char> {
    match inline {
        Inline::Str(s) | Inline::Code(_, s) | Inline::Math(_, s) | Inline::RawInline(_, s) => {
            s.chars().next_back()
        }
        Inline::Emph(xs)
        | Inline::Underline(xs)
        | Inline::Strong(xs)
        | Inline::Strikeout(xs)
        | Inline::Superscript(xs)
        | Inline::Subscript(xs)
        | Inline::SmallCaps(xs)
        | Inline::Quoted(_, xs)
        | Inline::Span(_, xs)
        | Inline::Link(_, xs, _)
        | Inline::Cite(_, xs) => xs.iter().rev().find_map(trailing_char),
        _ => None,
    }
}

/// The first rendered character of an inline, descending into wrapper inlines, or `None` for one
/// that renders no character at the boundary.
fn leading_char(inline: &Inline) -> Option<char> {
    match inline {
        Inline::Str(s) | Inline::Code(_, s) | Inline::Math(_, s) | Inline::RawInline(_, s) => {
            s.chars().next()
        }
        Inline::Emph(xs)
        | Inline::Underline(xs)
        | Inline::Strong(xs)
        | Inline::Strikeout(xs)
        | Inline::Superscript(xs)
        | Inline::Subscript(xs)
        | Inline::SmallCaps(xs)
        | Inline::Quoted(_, xs)
        | Inline::Span(_, xs)
        | Inline::Link(_, xs, _)
        | Inline::Cite(_, xs) => xs.iter().find_map(leading_char),
        _ => None,
    }
}

/// Whether `c` is an East Asian wide or fullwidth character, the class of characters that wrap
/// without a separating space.
fn is_wide_char(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x1100..=0x115F
        | 0x2329 | 0x232A
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B2FF
        | 0x1F200..=0x1F2FF
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD
    )
}

/// Turns a parsed preformatted line into code spans: runs of literal text and spaces become
/// [`Inline::Code`] while markup wrappers keep their structure with code interiors. A space inside a
/// code run is held as a non-breaking space so the rendered width is preserved.
pub(super) fn preformat_transform(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut run = String::new();
    for inline in inlines {
        match inline {
            Inline::Str(s) => run.push_str(&s.replace(' ', "\u{a0}")),
            Inline::Space | Inline::SoftBreak => run.push('\u{a0}'),
            other => {
                if !run.is_empty() {
                    out.push(Inline::Code(
                        Box::default(),
                        std::mem::take(&mut run).into(),
                    ));
                }
                out.push(preformat_descend(other));
            }
        }
    }
    if !run.is_empty() {
        out.push(Inline::Code(Box::default(), run.into()));
    }
    out
}

/// Recurses preformatting into a wrapper inline, leaving leaf inlines (code, math, breaks, raw)
/// untouched.
fn preformat_descend(inline: Inline) -> Inline {
    match inline {
        Inline::Emph(xs) => Inline::Emph(preformat_transform(xs)),
        Inline::Strong(xs) => Inline::Strong(preformat_transform(xs)),
        Inline::Strikeout(xs) => Inline::Strikeout(preformat_transform(xs)),
        Inline::Superscript(xs) => Inline::Superscript(preformat_transform(xs)),
        Inline::Subscript(xs) => Inline::Subscript(preformat_transform(xs)),
        Inline::Underline(xs) => Inline::Underline(preformat_transform(xs)),
        Inline::SmallCaps(xs) => Inline::SmallCaps(preformat_transform(xs)),
        Inline::Span(attr, xs) => Inline::Span(attr, preformat_transform(xs)),
        Inline::Link(attr, xs, target) => Inline::Link(attr, preformat_transform(xs), target),
        other => other,
    }
}
