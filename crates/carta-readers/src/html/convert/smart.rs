//! Smart typography and inline-code text folding: quotes, dashes, ellipses, and math spans.

use carta_ast::{Attr, Inline, MathType, QuoteType};

use super::helpers::{push_str, push_text};
use crate::smart_fold::{fold_dash_run_greedy, fold_ellipsis_run};

/// Render the contents of a `<code>` element that carries inline markup: each run of text becomes a
/// [`Inline::Code`] carrying the element's attributes, while container inlines keep their structure
/// with their own text runs codified in turn.
pub(super) fn codify(out: &mut Vec<Inline>, inlines: Vec<Inline>, attr: &Attr) {
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Vec<Inline>| {
        if !run.is_empty() {
            out.push(Inline::Code(
                Box::new(attr.clone()),
                std::mem::take(run).into(),
            ));
        }
    };
    for inline in inlines {
        match inline {
            Inline::Str(text) => run.push_str(&text),
            Inline::Space | Inline::SoftBreak => run.push(' '),
            Inline::Emph(children) => {
                flush(&mut run, out);
                out.push(Inline::Emph(codified(children, attr)));
            }
            Inline::Strong(children) => {
                flush(&mut run, out);
                out.push(Inline::Strong(codified(children, attr)));
            }
            Inline::Strikeout(children) => {
                flush(&mut run, out);
                out.push(Inline::Strikeout(codified(children, attr)));
            }
            Inline::Underline(children) => {
                flush(&mut run, out);
                out.push(Inline::Underline(codified(children, attr)));
            }
            Inline::Superscript(children) => {
                flush(&mut run, out);
                out.push(Inline::Superscript(codified(children, attr)));
            }
            Inline::Subscript(children) => {
                flush(&mut run, out);
                out.push(Inline::Subscript(codified(children, attr)));
            }
            Inline::SmallCaps(children) => {
                flush(&mut run, out);
                out.push(Inline::SmallCaps(codified(children, attr)));
            }
            Inline::Span(span_attr, children) => {
                flush(&mut run, out);
                out.push(Inline::Span(span_attr, codified(children, attr)));
            }
            Inline::Link(link_attr, children, target) => {
                flush(&mut run, out);
                out.push(Inline::Link(link_attr, codified(children, attr), target));
            }
            other => {
                flush(&mut run, out);
                out.push(other);
            }
        }
    }
    flush(&mut run, out);
}

fn codified(inlines: Vec<Inline>, attr: &Attr) -> Vec<Inline> {
    let mut out = Vec::new();
    codify(&mut out, inlines, attr);
    out
}

/// A unit of a text node during the inline finishing pass: a literal character, or a math span
/// already lifted out of the surrounding text.
pub(super) enum Item {
    Lit(char),
    Math(MathType, String),
}

const LEFT_DOUBLE: char = '\u{201C}';
const RIGHT_DOUBLE: char = '\u{201D}';
const LEFT_SINGLE: char = '\u{2018}';
const APOSTROPHE: char = '\u{2019}';

/// Whether a quote at `i` may open: it must follow an opening context (the node start, a math span,
/// whitespace, or one of `.`, `-`, `\`, `"`, `'`, or a curly quote) and be followed by a
/// non-whitespace character. A quote glued to a letter, digit, or most punctuation cannot open.
fn can_open(items: &[Item], i: usize) -> bool {
    let opens_after = match i.checked_sub(1).and_then(|prev| items.get(prev)) {
        None | Some(Item::Math(..)) => true,
        Some(Item::Lit(c)) => {
            c.is_whitespace()
                || matches!(
                    *c,
                    '.' | '-'
                        | '\\'
                        | '"'
                        | '\''
                        | LEFT_SINGLE
                        | APOSTROPHE
                        | LEFT_DOUBLE
                        | RIGHT_DOUBLE
                )
        }
    };
    let followed_by_nonspace = match items.get(i + 1) {
        Some(Item::Math(..)) => true,
        Some(Item::Lit(c)) => !c.is_whitespace(),
        None => false,
    };
    opens_after && followed_by_nonspace
}

/// The index in `from..hi` of the next double quote, which closes a double-quoted span.
fn find_next_double(items: &[Item], from: usize, hi: usize) -> Option<usize> {
    (from..hi).find(|&j| matches!(items.get(j), Some(Item::Lit('"'))))
}

/// The index in `from..hi` of the single quote that closes a single-quoted span: the first one not
/// glued to a following letter or digit, so a contraction's apostrophe is skipped over.
fn find_single_close(items: &[Item], from: usize, hi: usize) -> Option<usize> {
    (from..hi).find(|&j| {
        matches!(items.get(j), Some(Item::Lit('\'')))
            && !matches!(items.get(j + 1), Some(Item::Lit(c)) if c.is_alphanumeric())
    })
}

/// Resolve a span of items into smart inlines: quotes pair into [`Inline::Quoted`], an unpaired
/// quote reverts to its curly glyph, math spans pass through, and the literal runs between them fold
/// dashes and ellipses before collapsing whitespace.
pub(super) fn resolve_smart(items: &[Item], lo: usize, hi: usize) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = lo;
    while i < hi {
        match items.get(i) {
            Some(Item::Math(math_type, content)) => {
                flush_run(&mut buf, &mut out);
                out.push(Inline::Math(math_type.clone(), content.clone().into()));
                i += 1;
            }
            Some(Item::Lit('"')) => {
                if can_open(items, i)
                    && let Some(j) = find_next_double(items, i + 1, hi)
                {
                    flush_run(&mut buf, &mut out);
                    out.push(Inline::Quoted(
                        QuoteType::DoubleQuote,
                        resolve_smart(items, i + 1, j),
                    ));
                    i = j + 1;
                } else {
                    // An opener with no closer is a left quote; one that cannot open is a right quote.
                    buf.push(if can_open(items, i) {
                        LEFT_DOUBLE
                    } else {
                        RIGHT_DOUBLE
                    });
                    i += 1;
                }
            }
            Some(Item::Lit('\'')) => {
                if can_open(items, i)
                    && let Some(j) = find_single_close(items, i + 1, hi)
                {
                    flush_run(&mut buf, &mut out);
                    out.push(Inline::Quoted(
                        QuoteType::SingleQuote,
                        resolve_smart(items, i + 1, j),
                    ));
                    i = j + 1;
                } else {
                    // An unpaired single quote is always an apostrophe.
                    buf.push(APOSTROPHE);
                    i += 1;
                }
            }
            Some(Item::Lit(c)) => {
                buf.push(*c);
                i += 1;
            }
            None => break,
        }
    }
    flush_run(&mut buf, &mut out);
    out
}

/// Emit a text node when math forms are enabled but `smart` is not: literal runs stay verbatim
/// (subject only to whitespace collapse) and math spans pass through.
pub(super) fn emit_math_only(items: &[Item], out: &mut Vec<Inline>) {
    let mut buf = String::new();
    for item in items {
        match item {
            Item::Math(math_type, content) => {
                if !buf.is_empty() {
                    push_text(out, &buf);
                    buf.clear();
                }
                out.push(Inline::Math(math_type.clone(), content.clone().into()));
            }
            Item::Lit(c) => buf.push(*c),
        }
    }
    if !buf.is_empty() {
        push_text(out, &buf);
    }
}

/// Flush a literal run into `out`: fold its dashes and ellipses, then collapse whitespace into the
/// surrounding inline breaks.
fn flush_run(buf: &mut String, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        push_text(out, &fold_smart_punct(buf));
        buf.clear();
    }
}

/// Emit the text of an inline `<code>` element under `smart` and/or a math form. Top-level math spans
/// lift out as bare [`Inline::Math`]; the verbatim text between them becomes [`Inline::Str`] runs
/// (which [`codify`] then wraps as code), with whitespace collapsed to single spaces and, under
/// `smart`, dashes, ellipses, and paired quotes rendered as their typographic glyphs.
pub(super) fn emit_code(items: &[Item], smart: bool, out: &mut Vec<Inline>) {
    let hi = items.len();
    let mut result = String::new();
    let mut run = String::new();
    let mut i = 0;
    while i < hi {
        match items.get(i) {
            Some(Item::Math(math_type, content)) => {
                finalize_run(&mut run, &mut result, smart);
                if !result.is_empty() {
                    push_str(out, &result);
                    result.clear();
                }
                out.push(Inline::Math(math_type.clone(), content.clone().into()));
                i += 1;
            }
            Some(Item::Lit('"')) if smart => {
                if can_open(items, i)
                    && let Some(j) = find_next_double(items, i + 1, hi)
                {
                    finalize_run(&mut run, &mut result, smart);
                    result.push(LEFT_DOUBLE);
                    result.push_str(&code_build(items, i + 1, j));
                    result.push(RIGHT_DOUBLE);
                    i = j + 1;
                } else {
                    run.push(if can_open(items, i) {
                        LEFT_DOUBLE
                    } else {
                        RIGHT_DOUBLE
                    });
                    i += 1;
                }
            }
            Some(Item::Lit('\'')) if smart => {
                if can_open(items, i)
                    && let Some(j) = find_single_close(items, i + 1, hi)
                {
                    finalize_run(&mut run, &mut result, smart);
                    result.push(LEFT_SINGLE);
                    result.push_str(&code_build(items, i + 1, j));
                    result.push(APOSTROPHE);
                    i = j + 1;
                } else {
                    run.push(APOSTROPHE);
                    i += 1;
                }
            }
            Some(Item::Lit(c)) => {
                run.push(*c);
                i += 1;
            }
            None => break,
        }
    }
    finalize_run(&mut run, &mut result, smart);
    if !result.is_empty() {
        push_str(out, &result);
    }
}

/// Build the flat code text of a quote-delimited span: nested quotes become glyphs and math flattens
/// to its content, since the whole span renders as one code string. The result is already finalized,
/// so the caller appends it verbatim.
fn code_build(items: &[Item], lo: usize, hi: usize) -> String {
    let mut result = String::new();
    let mut run = String::new();
    let mut i = lo;
    while i < hi {
        match items.get(i) {
            Some(Item::Math(_, content)) => {
                finalize_run(&mut run, &mut result, true);
                result.push_str(content);
                i += 1;
            }
            Some(Item::Lit('"')) => {
                if can_open(items, i)
                    && let Some(j) = find_next_double(items, i + 1, hi)
                {
                    finalize_run(&mut run, &mut result, true);
                    result.push(LEFT_DOUBLE);
                    result.push_str(&code_build(items, i + 1, j));
                    result.push(RIGHT_DOUBLE);
                    i = j + 1;
                } else {
                    run.push(if can_open(items, i) {
                        LEFT_DOUBLE
                    } else {
                        RIGHT_DOUBLE
                    });
                    i += 1;
                }
            }
            Some(Item::Lit('\'')) => {
                if can_open(items, i)
                    && let Some(j) = find_single_close(items, i + 1, hi)
                {
                    finalize_run(&mut run, &mut result, true);
                    result.push(LEFT_SINGLE);
                    result.push_str(&code_build(items, i + 1, j));
                    result.push(APOSTROPHE);
                    i = j + 1;
                } else {
                    run.push(APOSTROPHE);
                    i += 1;
                }
            }
            Some(Item::Lit(c)) => {
                run.push(*c);
                i += 1;
            }
            None => break,
        }
    }
    finalize_run(&mut run, &mut result, true);
    result
}

/// Finalize a literal code run into `result`: fold its dashes and ellipses (under `smart`), then
/// collapse each whitespace span to a single space, joining cleanly with any text already there.
fn finalize_run(run: &mut String, result: &mut String, smart: bool) {
    if run.is_empty() {
        return;
    }
    let folded = if smart {
        fold_smart_punct(run)
    } else {
        std::mem::take(run)
    };
    run.clear();
    let mut prev_space = result.ends_with(' ');
    for ch in folded.chars() {
        if ch.is_ascii_whitespace() {
            if !prev_space {
                result.push(' ');
                prev_space = true;
            }
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
}

/// Fold a literal run's typography: a run of hyphens becomes em and en dashes, and a run of dots
/// becomes ellipses with up to two trailing dots.
fn fold_smart_punct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '-' => {
                let mut n = 0;
                while chars.peek() == Some(&'-') {
                    chars.next();
                    n += 1;
                }
                out.push_str(&fold_dash_run_greedy(n));
            }
            '.' => {
                let mut n = 0;
                while chars.peek() == Some(&'.') {
                    chars.next();
                    n += 1;
                }
                out.push_str(&fold_ellipsis_run(n));
            }
            _ => {
                out.push(c);
                chars.next();
            }
        }
    }
    out
}
