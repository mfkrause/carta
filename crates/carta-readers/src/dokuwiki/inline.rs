//! Inline scanning: emphasis, quotes, monospace, math, autolinks, and construct dispatch.

use carta_ast::{Inline, MathType, QuoteType, Target, to_plain_text};

use crate::entities;
use crate::inline_text::trim_inline_ends;
use crate::smart_fold::{
    QuoteCtx, can_close_quote, can_open_quote, fold_dash_run_greedy, fold_ellipsis_run, is_ws_opt,
    left_flanking,
};

use super::helpers::{boundary_before, find_subsequence, matches_at, run_length};
use super::links::{
    parse_angle, parse_footnote, parse_link, parse_macro, parse_media, parse_nowiki_pct,
};
use super::{Closer, Ctx, MAX_DEPTH};

/// The number of speculative delimiter openings an inline scan will attempt before it treats the
/// rest of its input as literal text. Each opener whose closer must be searched for costs one unit,
/// so this bounds the backtracking work an adversarial delimiter-dense run can provoke while staying
/// far above what any genuine document consumes.
fn inline_budget(len: usize) -> usize {
    len.saturating_mul(8).saturating_add(64).min(200_000)
}

/// Parse a block's inline content: scan it, then drop leading and trailing whitespace.
pub(super) fn inline_content(text: &str, ctx: Ctx, depth: usize) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    let mut budget = inline_budget(chars.len());
    let (mut inlines, _) = scan(
        &chars,
        &mut pos,
        None,
        ctx,
        QuoteCtx::default(),
        depth,
        &mut budget,
    );
    trim_inline_ends(&mut inlines);
    inlines
}

/// Scan a slice of characters as inline content with no surrounding-quote context.
pub(super) fn scan_slice(chars: &[char], ctx: Ctx, depth: usize) -> Vec<Inline> {
    let mut pos = 0;
    let mut budget = inline_budget(chars.len());
    let (inlines, _) = scan(
        chars,
        &mut pos,
        None,
        ctx,
        QuoteCtx::default(),
        depth,
        &mut budget,
    );
    inlines
}

/// Push the buffered text as a `Str` and clear the buffer.
fn flush(pending: &mut String, out: &mut Vec<Inline>) {
    if !pending.is_empty() {
        out.push(Inline::Str(std::mem::take(pending).into()));
    }
}

/// Scan characters into inlines from `*pos`. When `end` is set, the scan stops and reports `true` on
/// the matching closing delimiter; otherwise it runs to the end and reports `false`.
#[allow(clippy::too_many_lines)]
fn scan(
    chars: &[char],
    pos: &mut usize,
    end: Option<Closer>,
    ctx: Ctx,
    qctx: QuoteCtx,
    depth: usize,
    budget: &mut usize,
) -> (Vec<Inline>, bool) {
    let start = *pos;
    let mut out: Vec<Inline> = Vec::new();
    let mut pending = String::new();
    while let Some(&c) = chars.get(*pos) {
        if let Some(closer) = end
            && at_closer(chars, *pos, start, closer)
        {
            flush(&mut pending, &mut out);
            *pos += closer_width(closer);
            return (coalesce(out), true);
        }
        if c.is_ascii_alphabetic()
            && boundary_before(chars, *pos)
            && let Some((link, end)) = try_autolink(chars, *pos)
        {
            flush(&mut pending, &mut out);
            out.push(link);
            *pos = end;
            continue;
        }
        match c {
            ' ' | '\t' | '\n' => scan_whitespace_run(chars, pos, &mut pending, &mut out),
            '&' => {
                if let Some((decoded, next)) =
                    entities::read_reference(chars, *pos, chars.len(), true)
                {
                    pending.push_str(&decoded);
                    *pos = next;
                } else {
                    pending.push('&');
                    *pos += 1;
                }
            }
            '\\' if chars.get(*pos + 1) == Some(&'\\') => {
                scan_hard_break(chars, pos, &mut pending, &mut out);
            }
            '\\' if ctx.math && chars.get(*pos + 1) == Some(&'$') => {
                // A backslash-escaped dollar is literal text, not a math delimiter.
                pending.push('\\');
                pending.push('$');
                *pos += 2;
            }
            '*' if chars.get(*pos + 1) == Some(&'*') && depth < MAX_DEPTH => {
                handle_delim(
                    chars,
                    pos,
                    '*',
                    ctx,
                    qctx,
                    depth,
                    budget,
                    &mut pending,
                    &mut out,
                    Inline::Strong,
                );
            }
            '/' if chars.get(*pos + 1) == Some(&'/') && depth < MAX_DEPTH => {
                handle_delim(
                    chars,
                    pos,
                    '/',
                    ctx,
                    qctx,
                    depth,
                    budget,
                    &mut pending,
                    &mut out,
                    Inline::Emph,
                );
            }
            '_' if chars.get(*pos + 1) == Some(&'_') && depth < MAX_DEPTH => {
                handle_delim(
                    chars,
                    pos,
                    '_',
                    ctx,
                    qctx,
                    depth,
                    budget,
                    &mut pending,
                    &mut out,
                    Inline::Underline,
                );
            }
            '\'' if chars.get(*pos + 1) == Some(&'\'') => {
                handle_mono_or_quote(chars, pos, ctx, qctx, depth, budget, &mut pending, &mut out);
            }
            '\'' | '"' if ctx.smart => {
                handle_quote(
                    chars,
                    pos,
                    c,
                    ctx,
                    qctx,
                    depth,
                    budget,
                    &mut pending,
                    &mut out,
                );
            }
            '$' if ctx.math => {
                handle_math(chars, pos, &mut pending, &mut out);
            }
            '-' if ctx.smart => {
                let run = run_length(chars, *pos, '-');
                pending.push_str(&fold_dash_run_greedy(run));
                *pos += run;
            }
            '.' if ctx.smart => {
                let run = run_length(chars, *pos, '.');
                pending.push_str(&fold_ellipsis_run(run));
                *pos += run;
            }
            '[' if chars.get(*pos + 1) == Some(&'[') && depth < MAX_DEPTH => {
                handle_construct(chars, pos, c, ctx, depth, budget, &mut pending, &mut out);
            }
            '{' if chars.get(*pos + 1) == Some(&'{') && depth < MAX_DEPTH => {
                handle_construct(chars, pos, c, ctx, depth, budget, &mut pending, &mut out);
            }
            '(' if chars.get(*pos + 1) == Some(&'(') && depth < MAX_DEPTH => {
                handle_construct(chars, pos, c, ctx, depth, budget, &mut pending, &mut out);
            }
            '%' if chars.get(*pos + 1) == Some(&'%') => {
                handle_construct(chars, pos, c, ctx, depth, budget, &mut pending, &mut out);
            }
            '<' if depth < MAX_DEPTH => {
                handle_construct(chars, pos, c, ctx, depth, budget, &mut pending, &mut out);
            }
            '~' if chars.get(*pos + 1) == Some(&'~') => {
                handle_construct(chars, pos, c, ctx, depth, budget, &mut pending, &mut out);
            }
            other => {
                pending.push(other);
                *pos += 1;
            }
        }
    }
    flush(&mut pending, &mut out);
    (coalesce(out), end.is_none())
}

/// Whether the scan's closing delimiter sits at `pos`. The two-character closers must follow at least
/// one character of content and lean against non-whitespace on their left.
fn at_closer(chars: &[char], pos: usize, start: usize, closer: Closer) -> bool {
    match closer {
        Closer::Quote(quote) => {
            chars.get(pos) == Some(&quote) && can_close_quote(chars, pos, quote)
        }
        Closer::Delim(delim) => {
            chars.get(pos) == Some(&delim)
                && chars.get(pos + 1) == Some(&delim)
                && pos > start
                && chars.get(pos - 1).is_some_and(|c| !c.is_whitespace())
        }
        Closer::Mono => {
            chars.get(pos) == Some(&'\'')
                && chars.get(pos + 1) == Some(&'\'')
                && pos > start
                && chars.get(pos - 1).is_some_and(|c| !c.is_whitespace())
        }
    }
}

/// The number of characters a closing delimiter occupies.
fn closer_width(closer: Closer) -> usize {
    match closer {
        Closer::Quote(_) => 1,
        Closer::Delim(_) | Closer::Mono => 2,
    }
}

/// Handle a `''` opener: a monospace run when both delimiters flank non-whitespace content,
/// otherwise, under smart typography, the two quotes fold individually, and otherwise the opener
/// stays literal.
#[allow(clippy::too_many_arguments)]
fn handle_mono_or_quote(
    chars: &[char],
    pos: &mut usize,
    ctx: Ctx,
    qctx: QuoteCtx,
    depth: usize,
    budget: &mut usize,
    pending: &mut String,
    out: &mut Vec<Inline>,
) {
    if depth < MAX_DEPTH
        && let Some((node, end)) = parse_mono(chars, *pos, ctx, depth, budget)
    {
        flush(pending, out);
        out.push(node);
        *pos = end;
    } else if ctx.smart {
        handle_quote(chars, pos, '\'', ctx, qctx, depth, budget, pending, out);
    } else {
        pending.push('\'');
        *pos += 1;
    }
}

/// Consume a run of spaces, tabs, and newlines at `*pos`, emitting a single break: a soft break
/// when the run contains a newline, an ordinary space otherwise.
fn scan_whitespace_run(
    chars: &[char],
    pos: &mut usize,
    pending: &mut String,
    out: &mut Vec<Inline>,
) {
    flush(pending, out);
    let mut has_newline = false;
    while let Some(&w) = chars.get(*pos) {
        match w {
            '\n' => {
                has_newline = true;
                *pos += 1;
            }
            ' ' | '\t' => *pos += 1,
            _ => break,
        }
    }
    out.push(if has_newline {
        Inline::SoftBreak
    } else {
        Inline::Space
    });
}

/// Handle a `\\` sequence at `*pos`: a hard line break when followed by whitespace or the line end,
/// and two literal backslashes otherwise.
fn scan_hard_break(chars: &[char], pos: &mut usize, pending: &mut String, out: &mut Vec<Inline>) {
    let after = chars.get(*pos + 2);
    if after.is_none_or(|c| c.is_whitespace()) {
        flush(pending, out);
        out.push(Inline::LineBreak);
        *pos += 2;
        if after.is_some() {
            *pos += 1;
        }
    } else {
        pending.push('\\');
        pending.push('\\');
        *pos += 2;
    }
}

/// Try to parse the inline construct introduced by `c` at `pos`: a link (`[[`), media (`{{`), a
/// footnote (`((`), a verbatim span (`%%`), an angle-bracket construct (`<`), or a dropped macro
/// (`~~`). Returns the produced nodes and the index past the construct.
fn scan_construct(
    chars: &[char],
    pos: usize,
    c: char,
    ctx: Ctx,
    depth: usize,
) -> Option<(Vec<Inline>, usize)> {
    match c {
        '[' => parse_link(chars, pos).map(|(node, end)| (vec![node], end)),
        '{' => parse_media(chars, pos).map(|(node, end)| (vec![node], end)),
        '(' => parse_footnote(chars, pos, ctx, depth).map(|(node, end)| (vec![node], end)),
        '%' => parse_nowiki_pct(chars, pos),
        '<' => parse_angle(chars, pos, ctx, depth),
        '~' => parse_macro(chars, pos).map(|end| (Vec::new(), end)),
        _ => None,
    }
}

/// Dispatch an inline construct opener at `*pos`: on a successful parse the produced nodes are
/// appended and `*pos` advances past the construct; otherwise the opener is buffered literally and
/// `*pos` advances one character.
#[allow(clippy::too_many_arguments)]
fn handle_construct(
    chars: &[char],
    pos: &mut usize,
    c: char,
    ctx: Ctx,
    depth: usize,
    budget: &mut usize,
    pending: &mut String,
    out: &mut Vec<Inline>,
) {
    // Failed emphasis closes re-scan the same span, so a construct can be parsed many times over;
    // charging the shared backtracking budget by consumed span keeps total work linear.
    if *budget > 0
        && let Some((mut nodes, end)) = scan_construct(chars, *pos, c, ctx, depth)
    {
        *budget = budget.saturating_sub((end - *pos).max(1));
        flush(pending, out);
        out.append(&mut nodes);
        *pos = end;
    } else {
        pending.push(c);
        *pos += 1;
    }
}

/// Wrap a generic two-character emphasis run, or, when no valid closer exists, emit the opener
/// literally and resume scanning right after it. The run's content is scanned recursively, so a
/// would-be inner marker that cannot close is taken as text and an outer marker that the inner run
/// consumed past never pairs.
#[allow(clippy::too_many_arguments)]
fn handle_delim(
    chars: &[char],
    pos: &mut usize,
    delim: char,
    ctx: Ctx,
    qctx: QuoteCtx,
    depth: usize,
    budget: &mut usize,
    pending: &mut String,
    out: &mut Vec<Inline>,
    wrap: fn(Vec<Inline>) -> Inline,
) {
    let begin = *pos;
    // The opener must lean against following non-whitespace; the closer search stays in budget.
    if !is_ws_opt(chars.get(begin + 2).copied()) && *budget > 0 {
        *budget -= 1;
        let mut scan_pos = begin + 2;
        let (inner, closed) = scan(
            chars,
            &mut scan_pos,
            Some(Closer::Delim(delim)),
            ctx,
            qctx,
            depth + 1,
            budget,
        );
        if closed {
            flush(pending, out);
            out.push(wrap(inner));
            *pos = scan_pos;
            return;
        }
        // No closer: the opener stays literal, but each following opener would re-scan the same
        // span; charge the budget by span scanned so failed opens stay linear, not quadratic.
        *budget = budget.saturating_sub(scan_pos - begin);
    }
    pending.push(delim);
    pending.push(delim);
    *pos = begin + 2;
}

/// Try to open a curly-quote run at `*pos`; on a missing closer, leave the opener as the apt quote
/// glyph and let the scan reprocess what follows. An empty run is kept for double quotes but folds to
/// apostrophes for single quotes.
#[allow(clippy::too_many_arguments)]
fn handle_quote(
    chars: &[char],
    pos: &mut usize,
    quote: char,
    ctx: Ctx,
    qctx: QuoteCtx,
    depth: usize,
    budget: &mut usize,
    pending: &mut String,
    out: &mut Vec<Inline>,
) {
    let begin = *pos;
    if can_open_quote(chars, begin, quote, qctx) && depth < MAX_DEPTH && *budget > 0 {
        *budget -= 1;
        *pos = begin + 1;
        let mut inner_qctx = qctx;
        if quote == '\'' {
            inner_qctx.in_single = true;
        } else {
            inner_qctx.in_double = true;
        }
        let (inner, closed) = scan(
            chars,
            pos,
            Some(Closer::Quote(quote)),
            ctx,
            inner_qctx,
            depth + 1,
            budget,
        );
        if closed && (quote == '"' || !inner.is_empty()) {
            flush(pending, out);
            out.push(Inline::Quoted(quote_type(quote), inner));
            return;
        }
        // As in `handle_delim`: charge the failed scan's span so a quote-dense run stays linear.
        *budget = budget.saturating_sub(pos.saturating_sub(begin));
        *pos = begin + 1;
    } else {
        *pos = begin + 1;
    }
    pending.push(quote_glyph(chars, begin, quote));
}

/// The quote-node kind for a straight quote character.
fn quote_type(quote: char) -> QuoteType {
    if quote == '\'' {
        QuoteType::SingleQuote
    } else {
        QuoteType::DoubleQuote
    }
}

/// The curly glyph a non-paired straight quote folds into: an apostrophe for `'`, and an opening or
/// closing double quote depending on which side it leans.
fn quote_glyph(chars: &[char], pos: usize, quote: char) -> char {
    if quote == '\'' {
        '\u{2019}'
    } else if left_flanking(chars, pos) {
        '\u{201c}'
    } else {
        '\u{201d}'
    }
}

/// Monospace run `''…''`: its interior is parsed and then flattened to text. The run forms only when
/// the opener is followed by a non-space, the closer preceded by a non-space, and the interior is
/// non-empty; otherwise the opener is not a monospace marker.
///
/// Under smart typography the interior is scanned with quote folding active: any typographic quotes
/// that pair within the run are rendered as their glyphs, but if a straight quote inside disrupts the
/// closing `''` so the run never closes, the opener is not a monospace marker after all.
fn parse_mono(
    chars: &[char],
    begin: usize,
    ctx: Ctx,
    depth: usize,
    budget: &mut usize,
) -> Option<(Inline, usize)> {
    if is_ws_opt(chars.get(begin + 2).copied()) {
        return None;
    }
    if ctx.smart {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let mut pos = begin + 2;
        let (inner, closed) = scan(
            chars,
            &mut pos,
            Some(Closer::Mono),
            ctx,
            QuoteCtx::default(),
            depth + 1,
            budget,
        );
        if !closed {
            return None;
        }
        return Some((
            Inline::Code(Box::default(), flatten_mono(&inner).into()),
            pos,
        ));
    }
    let close = find_subsequence(chars, begin + 2, "''")?;
    if close <= begin + 2 || is_ws_opt(chars.get(close - 1).copied()) {
        return None;
    }
    let content = chars.get(begin + 2..close).unwrap_or(&[]);
    let inner = scan_slice(content, ctx, depth + 1);
    Some((
        Inline::Code(Box::default(), to_plain_text(&inner).into()),
        close + 2,
    ))
}

/// Flatten monospace interior inlines to text, rendering a quoted run as its curly quote glyphs so
/// folded quotation survives inside the code span.
fn flatten_mono(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_mono_text(inlines, &mut out);
    out
}

fn push_mono_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => out.push_str(text),
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => out.push(' '),
            Inline::Quoted(QuoteType::SingleQuote, xs) => {
                out.push('\u{2018}');
                push_mono_text(xs, out);
                out.push('\u{2019}');
            }
            Inline::Quoted(QuoteType::DoubleQuote, xs) => {
                out.push('\u{201c}');
                push_mono_text(xs, out);
                out.push('\u{201d}');
            }
            Inline::Emph(xs)
            | Inline::Underline(xs)
            | Inline::Strong(xs)
            | Inline::Strikeout(xs)
            | Inline::Superscript(xs)
            | Inline::Subscript(xs)
            | Inline::SmallCaps(xs)
            | Inline::Cite(_, xs)
            | Inline::Link(_, xs, _)
            | Inline::Image(_, xs, _)
            | Inline::Span(_, xs) => push_mono_text(xs, out),
            Inline::RawInline(..) | Inline::Note(_) => {}
        }
    }
}

/// Handle a `$` opener under dollar-math: a `$$…$$` display span when the next character is also `$`,
/// otherwise a `$…$` inline span. A failed attempt emits a single literal `$` and resumes scanning at
/// the following character, so an unmatched dollar is taken as text.
fn handle_math(chars: &[char], pos: &mut usize, pending: &mut String, out: &mut Vec<Inline>) {
    let begin = *pos;
    let parsed = if chars.get(begin + 1) == Some(&'$') {
        parse_display_math(chars, begin)
    } else {
        parse_inline_math(chars, begin)
    };
    if let Some((node, end)) = parsed {
        flush(pending, out);
        out.push(node);
        *pos = end;
    } else {
        pending.push('$');
        *pos = begin + 1;
    }
}

/// A `$$…$$` display-math span: its interior is taken verbatim. `None` when the span has no closer or
/// encloses nothing.
fn parse_display_math(chars: &[char], begin: usize) -> Option<(Inline, usize)> {
    let close = find_subsequence(chars, begin + 2, "$$")?;
    if close <= begin + 2 {
        return None;
    }
    let content: String = chars.get(begin + 2..close).unwrap_or(&[]).iter().collect();
    Some((
        Inline::Math(MathType::DisplayMath, content.into()),
        close + 2,
    ))
}

/// A `$…$` inline-math span: the opener must be followed by a non-space, the closer preceded by a
/// non-space and not followed by a digit. Its interior is taken verbatim.
fn parse_inline_math(chars: &[char], begin: usize) -> Option<(Inline, usize)> {
    if is_ws_opt(chars.get(begin + 1).copied()) {
        return None;
    }
    let mut j = begin + 1;
    while j < chars.len() {
        if chars.get(j) == Some(&'$')
            && j > begin + 1
            && chars.get(j - 1).is_some_and(|c| !c.is_whitespace())
            && !chars.get(j + 1).is_some_and(char::is_ascii_digit)
        {
            let content: String = chars.get(begin + 1..j).unwrap_or(&[]).iter().collect();
            return Some((Inline::Math(MathType::InlineMath, content.into()), j + 1));
        }
        j += 1;
    }
    None
}

/// Match a bare URL beginning at `pos` (`scheme://…`), returning the link and the end index.
fn try_autolink(chars: &[char], pos: usize) -> Option<(Inline, usize)> {
    let mut k = pos;
    while chars
        .get(k)
        .is_some_and(|&c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        k += 1;
    }
    if !matches_at(chars, k, "://") {
        return None;
    }
    let scheme: String = chars.get(pos..k)?.iter().collect::<String>().to_lowercase();
    if !crate::url_schemes::is_scheme(&scheme) {
        return None;
    }
    let content_start = k + 3;
    let scan_end = forward_scan(chars, pos);
    let end = trim_trailing(chars, content_start, scan_end);
    if end <= content_start {
        return None;
    }
    let url: String = chars.get(pos..end)?.iter().collect();
    Some((
        Inline::Link(
            Box::default(),
            vec![Inline::Str(url.clone().into())],
            Box::new(Target {
                url: url.into(),
                title: carta_ast::Text::default(),
            }),
        ),
        end,
    ))
}

/// Walk a URL run forward, stopping at whitespace or `<`, balancing parentheses, and ending at an
/// unbalanced `)` or a `]` outside any parenthesis.
fn forward_scan(chars: &[char], from: usize) -> usize {
    let mut depth: i32 = 0;
    let mut j = from;
    while let Some(&c) = chars.get(j) {
        if c.is_whitespace() || c == '<' {
            break;
        }
        match c {
            '(' => depth += 1,
            ')' | ']' if depth == 0 => break,
            ')' => depth -= 1,
            _ => {}
        }
        j += 1;
    }
    j
}

/// Drop trailing punctuation from a URL run, never below `min`. A trailing `;` takes a preceding
/// `&entity;` with it.
fn trim_trailing(chars: &[char], min: usize, mut end: usize) -> usize {
    while end > min {
        match chars.get(end - 1) {
            Some('!' | '"' | '\'' | '*' | ',' | '.' | ':' | '?' | '_' | '~') => end -= 1,
            Some(';') => {
                let mut j = end - 1;
                while j > min
                    && chars
                        .get(j - 1)
                        .is_some_and(|&c| c.is_ascii_alphanumeric() || c == '#')
                {
                    j -= 1;
                }
                end = if j > min && chars.get(j - 1) == Some(&'&') {
                    j - 1
                } else {
                    end - 1
                };
            }
            _ => break,
        }
    }
    end
}

/// Merge adjacent text runs and collapse adjacent whitespace into a single token (preferring a hard
/// space), so dropped macros and split apostrophes leave no doubled spacing or fragmented words.
fn coalesce(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Str(s) => {
                if let Some(Inline::Str(prev)) = out.last_mut() {
                    prev.push_str(&s);
                } else if !s.is_empty() {
                    out.push(Inline::Str(s));
                }
            }
            Inline::Space | Inline::SoftBreak => match out.last() {
                Some(Inline::Space) => {}
                Some(Inline::SoftBreak) => {
                    if matches!(inline, Inline::Space)
                        && let Some(slot) = out.last_mut()
                    {
                        *slot = Inline::Space;
                    }
                }
                _ => out.push(inline),
            },
            other => out.push(other),
        }
    }
    out
}
