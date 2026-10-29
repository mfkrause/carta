//! Emphasis resolution, smart quotes, and inline coalescing.

use carta_ast::{Inline, InlineParts, QuoteType};

use super::{Tok, raw_html};

/// A unit of the stream emphasis resolution works over: one apostrophe of a run, or a finished node.
enum Unit {
    Apostrophe,
    Node(Inline),
}

/// Resolve apostrophe emphasis, preferring strong spans and trimming their outer whitespace.
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
    let ends = emphasis_ends(&runs);
    let mut frames = Vec::new();
    let mut nodes = Vec::new();
    let mut units = units.into_iter().enumerate();
    while let Some((position, unit)) = units.next() {
        match unit {
            Unit::Node(inline) => nodes.push(inline),
            Unit::Apostrophe => {
                let closer = frames.last().map(|(strong, _)| *strong);
                match emphasis_step(&runs, &ends, position, closer) {
                    EmphasisStep::Open(strong, _) => {
                        for _ in 1..emphasis_width(strong) {
                            units.next();
                        }
                        frames.push((strong, std::mem::take(&mut nodes)));
                    }
                    EmphasisStep::Close(_) => {
                        if let Some((strong, mut parent)) = frames.pop() {
                            for _ in 1..emphasis_width(strong) {
                                units.next();
                            }
                            let body = strip_outer_whitespace(nodes);
                            parent.push(if strong {
                                Inline::Strong(body)
                            } else {
                                Inline::Emph(body)
                            });
                            nodes = parent;
                        }
                    }
                    EmphasisStep::Literal => nodes.push(Inline::Str("'".into())),
                }
            }
        }
    }
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

enum EmphasisStep {
    Open(bool, usize),
    Close(usize),
    Literal,
}

fn emphasis_step(
    runs: &[usize],
    ends: &[[Option<usize>; 2]],
    position: usize,
    closer: Option<bool>,
) -> EmphasisStep {
    let run = runs.get(position).copied().unwrap_or(0);
    let opening = |strong| {
        let width = emphasis_width(strong);
        let end = ends
            .get(position + width)?
            .get(usize::from(strong))
            .copied()
            .flatten()?;
        (run >= width && closer != Some(strong) && end > position + 2 * width).then_some(end)
    };
    if let Some(end) = opening(true) {
        return EmphasisStep::Open(true, end);
    }
    if let Some(strong) = closer
        && run >= emphasis_width(strong)
    {
        return EmphasisStep::Close(position + emphasis_width(strong));
    }
    if let Some(end) = opening(false) {
        return EmphasisStep::Open(false, end);
    }
    EmphasisStep::Literal
}

fn emphasis_ends(runs: &[usize]) -> Vec<[Option<usize>; 2]> {
    let mut ends = vec![[None; 2]; runs.len() + 1];
    for position in (0..runs.len()).rev() {
        for strong in [false, true] {
            let end = match emphasis_step(runs, &ends, position, Some(strong)) {
                EmphasisStep::Open(_, next) => ends
                    .get(next)
                    .and_then(|pair| pair.get(usize::from(strong)))
                    .copied()
                    .flatten(),
                EmphasisStep::Close(next) => Some(next),
                EmphasisStep::Literal => ends
                    .get(position + 1)
                    .and_then(|pair| pair.get(usize::from(strong)))
                    .copied()
                    .flatten(),
            };
            if let Some(slot) = ends
                .get_mut(position)
                .and_then(|pair| pair.get_mut(usize::from(strong)))
            {
                *slot = end;
            }
        }
    }
    ends
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
    carta_core::with_stack(|| apply_smart_quotes_nested(inlines))
}

fn apply_smart_quotes_nested(inlines: Vec<Inline>) -> Vec<Inline> {
    let recursed: Vec<Inline> = inlines.into_iter().map(smart_descend).collect();
    let mut units = flatten_smart(recursed);
    let length = units.len();
    resolve_double_quotes(&mut units, 0, length)
}

/// Applies the double-quote fold to the inline children of a container, leaving leaf and opaque
/// inlines (text, code, math, raw passthrough, notes) untouched.
/// Link labels use their own text rules and are already parsed.
fn smart_descend(inline: Inline) -> Inline {
    match inline.into_parts() {
        InlineParts::Emph(v) => Inline::Emph(apply_smart_quotes(v)),
        InlineParts::Underline(v) => Inline::Underline(apply_smart_quotes(v)),
        InlineParts::Strong(v) => Inline::Strong(apply_smart_quotes(v)),
        InlineParts::Strikeout(v) => Inline::Strikeout(apply_smart_quotes(v)),
        InlineParts::Superscript(v) => Inline::Superscript(apply_smart_quotes(v)),
        InlineParts::Subscript(v) => Inline::Subscript(apply_smart_quotes(v)),
        InlineParts::SmallCaps(v) => Inline::SmallCaps(apply_smart_quotes(v)),
        InlineParts::Quoted(quote_type, v) => Inline::Quoted(quote_type, apply_smart_quotes(v)),
        InlineParts::Span(attr, v) => Inline::Span(attr, apply_smart_quotes(v)),
        InlineParts::Image(attr, v, target) => Inline::Image(attr, apply_smart_quotes(v), target),
        other => other.into(),
    }
}

fn flatten_smart(inlines: Vec<Inline>) -> Vec<SmartUnit> {
    let mut units: Vec<SmartUnit> = Vec::new();
    for inline in inlines {
        match inline.into_parts() {
            InlineParts::Str(text) => {
                for c in text.chars() {
                    if c == '"' {
                        units.push(SmartUnit::Quote);
                    } else {
                        units.push(SmartUnit::Ch(c));
                    }
                }
            }
            space @ (InlineParts::Space | InlineParts::SoftBreak | InlineParts::LineBreak) => {
                units.push(SmartUnit::Space(space.into()));
            }
            other => units.push(SmartUnit::Node(other.into())),
        }
    }
    units
}

fn resolve_double_quotes(units: &mut [SmartUnit], lo: usize, hi: usize) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut buf = String::new();
    let mut i = lo;
    while i < hi {
        match units.get_mut(i) {
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
                out.push(std::mem::replace(inline, Inline::Space));
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
    carta_core::with_stack(|| coalesce_nested(inlines))
}

fn coalesce_nested(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for inline in inlines {
        let inline = match inline.into_parts() {
            InlineParts::Emph(xs) => Inline::Emph(coalesce(xs)),
            InlineParts::Strong(xs) => Inline::Strong(coalesce(xs)),
            InlineParts::Strikeout(xs) => Inline::Strikeout(coalesce(xs)),
            InlineParts::Superscript(xs) => Inline::Superscript(coalesce(xs)),
            InlineParts::Subscript(xs) => Inline::Subscript(coalesce(xs)),
            InlineParts::Underline(xs) => Inline::Underline(coalesce(xs)),
            InlineParts::SmallCaps(xs) => Inline::SmallCaps(coalesce(xs)),
            InlineParts::Span(attr, xs) => Inline::Span(attr, coalesce(xs)),
            other => other.into(),
        };
        match (out.last_mut(), &inline) {
            (Some(Inline::Str(prev)), Inline::Str(next)) => prev.push_str(next),
            // Merge adjacent whitespace after removed content, retaining any soft break.
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
    carta_core::with_stack(|| trailing_char_nested(inline))
}

fn trailing_char_nested(inline: &Inline) -> Option<char> {
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
    carta_core::with_stack(|| leading_char_nested(inline))
}

fn leading_char_nested(inline: &Inline) -> Option<char> {
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
    carta_core::with_stack(|| preformat_transform_nested(inlines))
}

fn preformat_transform_nested(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut run = String::new();
    for inline in inlines {
        match inline.into_parts() {
            InlineParts::Str(s) => run.push_str(&s.replace(' ', "\u{a0}")),
            InlineParts::Space | InlineParts::SoftBreak => run.push('\u{a0}'),
            other => {
                if !run.is_empty() {
                    out.push(Inline::Code(
                        Box::default(),
                        std::mem::take(&mut run).into(),
                    ));
                }
                out.push(preformat_descend(other.into()));
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
    match inline.into_parts() {
        InlineParts::Emph(xs) => Inline::Emph(preformat_transform(xs)),
        InlineParts::Strong(xs) => Inline::Strong(preformat_transform(xs)),
        InlineParts::Strikeout(xs) => Inline::Strikeout(preformat_transform(xs)),
        InlineParts::Superscript(xs) => Inline::Superscript(preformat_transform(xs)),
        InlineParts::Subscript(xs) => Inline::Subscript(preformat_transform(xs)),
        InlineParts::Underline(xs) => Inline::Underline(preformat_transform(xs)),
        InlineParts::SmallCaps(xs) => Inline::SmallCaps(preformat_transform(xs)),
        InlineParts::Span(attr, xs) => Inline::Span(attr, preformat_transform(xs)),
        InlineParts::Link(attr, xs, target) => Inline::Link(attr, preformat_transform(xs), target),
        other => other.into(),
    }
}
