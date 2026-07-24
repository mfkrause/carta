//! Inline parsing of Jira wiki markup: text-effect spans, monospace, colour, citations, symbols, and breaks.

use carta_ast::{Attr, Inline, Target, ToCompactString};

use super::links::{match_bare_url, parse_image, parse_link};
use super::shared::{is_space, matches_at, slice_to_string};

const PAREN_SYMBOLS: &[(&str, char)] = &[
    ("(flagoff)", '\u{2690}'),
    ("(flag)", '\u{2691}'),
    ("(off)", '\u{1F319}'),
    ("(on)", '\u{1F4A1}'),
    ("(*r)", '\u{2B50}'),
    ("(*g)", '\u{2B50}'),
    ("(*b)", '\u{2B50}'),
    ("(*y)", '\u{2B50}'),
    ("(*)", '\u{2B50}'),
    ("(!)", '\u{2757}'),
    ("(x)", '\u{274C}'),
    ("(/)", '\u{2714}'),
    ("(i)", '\u{2139}'),
    ("(?)", '\u{2753}'),
    ("(y)", '\u{1F44D}'),
    ("(n)", '\u{1F44E}'),
    ("(+)", '\u{2795}'),
    ("(-)", '\u{2796}'),
];

const EMOTICONS: &[(&str, char)] = &[
    (":)", '\u{1F642}'),
    (":(", '\u{1F641}'),
    (":P", '\u{1F61B}'),
    (":D", '\u{1F603}'),
    (";)", '\u{1F609}'),
];

/// Tokenises `text` into inlines without interpreting markup: whitespace runs become
/// [`Inline::Space`] and every other run becomes an [`Inline::Str`]. Used for a panel title, whose
/// text is rendered verbatim inside its header.
pub(super) fn plain_inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in text.chars() {
        if is_space(ch) {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word).into()));
            }
            if out.last() != Some(&Inline::Space) {
                out.push(Inline::Space);
            }
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word.into()));
    }
    out
}

/// A unit of scanned inline content, before text-effect delimiters are paired up.
enum Tok {
    /// A run of literal text.
    Text(String),
    /// A single flanking delimiter that may open and/or close a text-effect span.
    Delim {
        marker: char,
        open: bool,
        close: bool,
    },
    /// A fully formed inline node: link, image, span, monospace, line break, or space.
    Atom(Inline),
}

/// Inline-nesting depth past which parsing stops descending. Monospace, colour, link-label and
/// citation spans each re-enter inline parsing on their inner text; a hard cap keeps adversarially
/// deep nesting off the call stack. It is far beyond any nesting real text uses.
const MAX_INLINE_DEPTH: usize = 32;

/// Parses the character range `lo..hi` into inline nodes: it scans the text into tokens, pairs the
/// flanking delimiters into spans, and folds the result into a flat list of inlines. Flanking
/// decisions consult the real neighbouring characters via absolute indices, so a range bounded to a
/// single line will not let markup escape that line.
pub(super) fn parse_inlines(chars: &[char], lo: usize, hi: usize) -> Vec<Inline> {
    inlines_with(chars, lo, hi, true, 0)
}

/// Parses inlines with control over bare-URL autolinking. A link label cannot contain another link,
/// so the text of one is parsed with `autolink` cleared. `depth` tracks how many nested spans deep
/// this call is; past the cap the remaining span is emitted as literal text without descending.
pub(super) fn inlines_with(
    chars: &[char],
    lo: usize,
    hi: usize,
    autolink: bool,
    depth: usize,
) -> Vec<Inline> {
    if depth > MAX_INLINE_DEPTH {
        let text = slice_to_string(chars, lo, hi);
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![Inline::Str(text.into())]
        };
    }
    finalize(resolve(scan_tokens(chars, lo, hi, autolink, depth)))
}

fn push_text(pending: &mut String, toks: &mut Vec<Tok>) {
    if !pending.is_empty() {
        toks.push(Tok::Text(std::mem::take(pending)));
    }
}

/// A forward-scan step budget proportional to the span. A dense run of unclosable openers would
/// otherwise make each failed construct re-scan the whole suffix, so the total cost grows
/// quadratically; charging one step per position examined keeps the scanning linear over the span.
/// It is far above what any genuine construct needs, so a real close is always found, while a
/// pathological run gives up and leaves the opener as literal text.
pub(super) fn scan_budget(lo: usize, hi: usize) -> usize {
    crate::inline_scan::scan_budget(hi.saturating_sub(lo))
}

/// Finds the first index in `range` whose character satisfies `pred`, charging one budget step per
/// position examined and abandoning the scan (returning `None`) once the budget is spent.
pub(super) fn find_within(
    chars: &[char],
    range: std::ops::Range<usize>,
    budget: &mut usize,
    pred: impl Fn(char) -> bool,
) -> Option<usize> {
    for k in range {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        if chars.get(k).copied().is_some_and(&pred) {
            return Some(k);
        }
    }
    None
}

/// Scans `lo..hi` left to right into tokens: literal runs accumulate into [`Tok::Text`], a flanking
/// delimiter becomes a [`Tok::Delim`], and a self-contained construct (link, image, brace span,
/// citation, autolink, symbol) becomes a [`Tok::Atom`].
#[allow(clippy::too_many_lines)]
fn scan_tokens(chars: &[char], lo: usize, hi: usize, autolink: bool, depth: usize) -> Vec<Tok> {
    let mut toks: Vec<Tok> = Vec::new();
    let mut pending = String::new();
    let mut i = lo;
    let mut budget = scan_budget(lo, hi);

    while i < hi {
        let Some(&c) = chars.get(i) else {
            break;
        };

        if is_space(c) {
            push_text(&mut pending, &mut toks);
            i = scan_whitespace_run(chars, i, hi, &mut toks);
            continue;
        }

        let prev_alnum = i > 0 && chars.get(i - 1).is_some_and(|c| c.is_alphanumeric());

        if autolink
            && !prev_alnum
            && let Some(end) = match_bare_url(chars, i, hi)
        {
            push_text(&mut pending, &mut toks);
            let url = slice_to_string(chars, i, end);
            toks.push(Tok::Atom(Inline::Link(
                Box::default(),
                vec![Inline::Str(url.clone().into())],
                Box::new(Target {
                    url: url.into(),
                    title: carta_ast::Text::default(),
                }),
            )));
            i = end;
            continue;
        }

        match c {
            '\\' => {
                i = scan_backslash(chars, i, hi, &mut pending, &mut toks);
            }
            '&' => {
                if let Some((text, next)) = crate::entities::read_reference(chars, i, hi, false) {
                    pending.push_str(&text);
                    i = next;
                } else {
                    pending.push('&');
                    i += 1;
                }
            }
            '?' => {
                if let Some((next, inner)) = parse_citation(chars, i, hi, autolink, depth) {
                    pending.push('\u{2014}');
                    push_text(&mut pending, &mut toks);
                    toks.push(Tok::Atom(Inline::Space));
                    toks.push(Tok::Atom(Inline::Emph(inner)));
                    i = next;
                } else {
                    pending.push('?');
                    i += 1;
                }
            }
            '*' | '_' | '+' | '^' | '~' => {
                push_delimiter(c, chars, i, &mut pending, &mut toks);
                i += 1;
            }
            '-' => {
                i = scan_dash(chars, i, hi, &mut pending, &mut toks);
            }
            '(' => {
                if let Some((glyph, len)) = match_token_symbol(chars, i, PAREN_SYMBOLS) {
                    pending.push(glyph);
                    i += len;
                } else {
                    pending.push('(');
                    i += 1;
                }
            }
            ':' | ';' => {
                if let Some((glyph, len)) = match_token_symbol(chars, i, EMOTICONS) {
                    pending.push(glyph);
                    i += len;
                } else {
                    pending.push(c);
                    i += 1;
                }
            }
            '[' | '!' | '{' => {
                if let Some((node, next)) =
                    scan_construct(c, chars, i, hi, autolink, depth, &mut budget)
                {
                    push_text(&mut pending, &mut toks);
                    toks.push(Tok::Atom(node));
                    i = next;
                } else {
                    pending.push(c);
                    i += 1;
                }
            }
            _ => {
                pending.push(c);
                i += 1;
            }
        }
    }

    push_text(&mut pending, &mut toks);
    toks
}

/// Consumes the whitespace run beginning at `start`, pushing the single token it collapses to: a
/// line break when the run crosses a newline, otherwise a space. The spaces around a soft break are
/// absorbed into it. Returns the index just past the run.
fn scan_whitespace_run(chars: &[char], start: usize, hi: usize, toks: &mut Vec<Tok>) -> usize {
    let mut has_newline = chars.get(start) == Some(&'\n');
    let mut i = start + 1;
    while i < hi && chars.get(i).is_some_and(|&c| is_space(c)) {
        has_newline |= chars.get(i) == Some(&'\n');
        i += 1;
    }
    toks.push(Tok::Atom(if has_newline {
        Inline::LineBreak
    } else {
        Inline::Space
    }));
    i
}

/// The punctuation a backslash removes itself before, leaving the character as literal text. Any
/// character outside this set keeps its backslash.
fn is_escapable(c: char) -> bool {
    matches!(
        c,
        '!' | '"'
            | '#'
            | '%'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | ','
            | '-'
            | '.'
            | '/'
            | ':'
            | ';'
            | '?'
            | '@'
            | '['
            | ']'
            | '_'
            | '{'
            | '}'
    )
}

/// Emits a flanking-delimiter token for one of the emphasis markers at `i`, or buffers the marker as
/// literal text when it can neither open nor close a span.
fn push_delimiter(
    marker: char,
    chars: &[char],
    i: usize,
    pending: &mut String,
    toks: &mut Vec<Tok>,
) {
    let open = can_open(chars, i);
    let close = can_close(chars, i);
    if open || close {
        push_text(pending, toks);
        toks.push(Tok::Delim {
            marker,
            open,
            close,
        });
    } else {
        pending.push(marker);
    }
}

/// Parses a self-contained construct introduced by `c` at `i`: `[` starts a link, `!` an image, and
/// `{` a brace span. Returns the resulting node and the index just past it, or `None` when the text
/// does not form that construct.
fn scan_construct(
    c: char,
    chars: &[char],
    i: usize,
    hi: usize,
    autolink: bool,
    depth: usize,
    budget: &mut usize,
) -> Option<(Inline, usize)> {
    match c {
        '[' => parse_link(chars, i, hi, depth, budget),
        '!' => parse_image(chars, i, hi, budget),
        _ => parse_brace_inline(chars, i, hi, autolink, depth, budget),
    }
}

/// Handles a backslash at `i`. A backslash pair `\\` is a forced line break that absorbs the
/// whitespace around it, unless a third backslash follows, in which case the pair is an escaped
/// backslash producing one literal `\` and the scan continues at the third. A backslash before one
/// of a fixed set of punctuation marks escapes that mark to a literal; before anything else the
/// backslash itself stays literal. Returns the next position.
fn scan_backslash(
    chars: &[char],
    i: usize,
    hi: usize,
    pending: &mut String,
    toks: &mut Vec<Tok>,
) -> usize {
    if i + 1 < hi && chars.get(i + 1) == Some(&'\\') {
        if i + 2 < hi && chars.get(i + 2) == Some(&'\\') {
            pending.push('\\');
            return i + 2;
        }
        push_text(pending, toks);
        if matches!(toks.last(), Some(Tok::Atom(Inline::Space))) {
            toks.pop();
        }
        toks.push(Tok::Atom(Inline::LineBreak));
        let mut j = i + 2;
        while j < hi && chars.get(j).is_some_and(|&c| is_space(c)) {
            j += 1;
        }
        return j;
    }
    if let Some(&next) = chars.get(i + 1).filter(|_| i + 1 < hi)
        && is_escapable(next)
    {
        pending.push(next);
        return i + 2;
    }
    pending.push('\\');
    i + 1
}

/// Handles a run of `-` at `i`. A run of two or more hyphens followed by a space or tab folds into
/// typographic dashes: a word character on its left keeps the first hyphen attached to that word,
/// then the remaining hyphens fold (two into an en dash, three or more into an em dash preceded by
/// the surplus hyphens). Otherwise a single `-` is scanned as a strikeout delimiter (or literal text).
/// Returns the next scan position. The character following the run is read from the full input rather
/// than the line-content bound, so a hyphen run that ends a line still sees the space trimmed from it.
fn scan_dash(
    chars: &[char],
    i: usize,
    hi: usize,
    pending: &mut String,
    toks: &mut Vec<Tok>,
) -> usize {
    let mut run = 0;
    while i + run < hi && chars.get(i + run) == Some(&'-') {
        run += 1;
    }
    let left_word = i > 0 && chars.get(i - 1).is_some_and(|c| c.is_alphanumeric());
    let right_space = matches!(chars.get(i + run), Some(' ' | '\t'));
    // A word on the left keeps its first hyphen attached, so only the remainder folds.
    let fold_run = if left_word {
        run.saturating_sub(1)
    } else {
        run
    };
    // A lone leftover hyphen stays a strikeout delimiter so `--x--` before a space can still pair.
    if right_space && fold_run >= 2 {
        if left_word {
            pending.push('-');
        }
        if fold_run == 2 {
            pending.push('\u{2013}');
        } else {
            for _ in 0..fold_run.saturating_sub(3) {
                pending.push('-');
            }
            pending.push('\u{2014}');
        }
        return i + run;
    }

    let open = can_open(chars, i);
    let close = can_close(chars, i);
    if open || close {
        push_text(pending, toks);
        toks.push(Tok::Delim {
            marker: '-',
            open,
            close,
        });
    } else {
        pending.push('-');
    }
    i + 1
}

/// Index of the innermost open delimiter still awaiting a close, regardless of its marker.
fn top_opener(acc: &[Tok]) -> Option<usize> {
    acc.iter()
        .rposition(|t| matches!(t, Tok::Delim { open: true, .. }))
}

/// Pairs flanking delimiters into spans. A closing delimiter binds only to the innermost open
/// delimiter; it forms a span when that opener carries the same marker and they enclose non-empty
/// content, and is otherwise left literal. Binding only to the innermost opener keeps spans strictly
/// nested, so two different markers that interleave cannot both form a span. Same-marker spans nest
/// at most two deep.
fn resolve(toks: Vec<Tok>) -> Vec<Tok> {
    let mut acc: Vec<Tok> = Vec::new();
    for tok in toks {
        let Tok::Delim {
            marker,
            open,
            close,
        } = tok
        else {
            acc.push(tok);
            continue;
        };
        if close
            && let Some(open_idx) = top_opener(&acc)
            && matches!(acc.get(open_idx), Some(Tok::Delim { marker: m, .. }) if *m == marker)
            && acc.len() > open_idx + 1
        {
            let inner = finalize(acc.split_off(open_idx + 1));
            if same_marker_depth(&inner, marker) < 2 {
                acc.pop();
                acc.push(Tok::Atom(make_span(marker, inner)));
                continue;
            }
            // Nesting cap reached: the opener stays unmatched; resolved content returns to the stack.
            acc.extend(inner.into_iter().map(Tok::Atom));
        }
        acc.push(Tok::Delim {
            marker,
            open,
            close,
        });
    }
    acc
}

/// Lowers resolved tokens into inlines: an unmatched delimiter becomes its literal marker character,
/// adjacent text merges into one string, and adjacent spans of the same kind merge into one.
fn finalize(toks: Vec<Tok>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for tok in toks {
        let inline = match tok {
            Tok::Text(s) => Inline::Str(s.into()),
            Tok::Delim { marker, .. } => Inline::Str(marker.to_compact_string()),
            Tok::Atom(node) => node,
        };
        let inline = match out.last_mut() {
            Some(last) => match merge_adjacent(last, inline) {
                None => continue,
                Some(unmerged) => unmerged,
            },
            None => inline,
        };
        out.push(inline);
    }
    out
}

/// Merges `next` into `last` when they are two strings or two spans of the same kind, returning
/// `None` on success and `Some(next)` when they do not combine.
fn merge_adjacent(last: &mut Inline, next: Inline) -> Option<Inline> {
    match (last, next) {
        (Inline::Str(a), Inline::Str(b)) => {
            a.push_str(&b);
            None
        }
        (Inline::Strong(a), Inline::Strong(b))
        | (Inline::Emph(a), Inline::Emph(b))
        | (Inline::Underline(a), Inline::Underline(b))
        | (Inline::Superscript(a), Inline::Superscript(b))
        | (Inline::Subscript(a), Inline::Subscript(b))
        | (Inline::Strikeout(a), Inline::Strikeout(b)) => {
            a.extend(b);
            None
        }
        (_, other) => Some(other),
    }
}

fn make_span(marker: char, inner: Vec<Inline>) -> Inline {
    match marker {
        '*' => Inline::Strong(inner),
        '_' => Inline::Emph(inner),
        '+' => Inline::Underline(inner),
        '^' => Inline::Superscript(inner),
        '~' => Inline::Subscript(inner),
        _ => Inline::Strikeout(inner),
    }
}

/// The deepest nesting of spans carrying `marker` anywhere within `nodes`.
fn same_marker_depth(nodes: &[Inline], marker: char) -> usize {
    nodes
        .iter()
        .map(|n| node_marker_depth(n, marker))
        .max()
        .unwrap_or(0)
}

fn node_marker_depth(node: &Inline, marker: char) -> usize {
    let (is_match, children) = match node {
        Inline::Strong(k) => (marker == '*', Some(k)),
        Inline::Emph(k) => (marker == '_', Some(k)),
        Inline::Underline(k) => (marker == '+', Some(k)),
        Inline::Superscript(k) => (marker == '^', Some(k)),
        Inline::Subscript(k) => (marker == '~', Some(k)),
        Inline::Strikeout(k) => (marker == '-', Some(k)),
        _ => (false, None),
    };
    match children {
        Some(k) => same_marker_depth(k, marker) + usize::from(is_match),
        None => 0,
    }
}

/// True when the character at `i` is absent (start/end of input) or not alphanumeric.
fn boundary(chars: &[char], i: usize) -> bool {
    chars.get(i).is_none_or(|c| !c.is_alphanumeric())
}

pub(super) fn non_space(chars: &[char], i: usize) -> bool {
    chars.get(i).is_some_and(|&c| !is_space(c))
}

/// A delimiter at `i` may open a span when its left neighbour is a boundary and the next character
/// is not whitespace.
fn can_open(chars: &[char], i: usize) -> bool {
    let left_boundary = i == 0 || boundary(chars, i - 1);
    left_boundary && non_space(chars, i + 1)
}

/// A delimiter at `j` may close a span when the previous character is not whitespace and the right
/// neighbour is a boundary.
fn can_close(chars: &[char], j: usize) -> bool {
    j > 0 && non_space(chars, j - 1) && boundary(chars, j + 1)
}

fn parse_citation(
    chars: &[char],
    i: usize,
    hi: usize,
    autolink: bool,
    depth: usize,
) -> Option<(usize, Vec<Inline>)> {
    if chars.get(i + 1) != Some(&'?') {
        return None;
    }
    let left_boundary = i == 0 || boundary(chars, i - 1);
    if !left_boundary || !non_space(chars, i + 2) {
        return None;
    }
    let mut j = i + 2;
    while j < hi {
        if chars.get(j) == Some(&'?')
            && chars.get(j + 1) == Some(&'?')
            && j > i + 2
            && non_space(chars, j - 1)
            && boundary(chars, j + 2)
        {
            return Some((j + 2, inlines_with(chars, i + 2, j, autolink, depth + 1)));
        }
        j += 1;
    }
    None
}

/// A monospaced span opens at `i` (which holds the first `{` of `{{`) when its left neighbour is a
/// boundary and the character after `{{` is not whitespace.
fn can_open_monospace(chars: &[char], i: usize) -> bool {
    let left_boundary = i == 0 || boundary(chars, i - 1);
    left_boundary && non_space(chars, i + 2)
}

/// A monospaced span closes at `j` (holding the first `}` of `}}`) when the close is non-empty, its
/// left neighbour is not whitespace, and the character after `}}` is a boundary.
fn closes_monospace(chars: &[char], open: usize, j: usize) -> bool {
    j > open + 2 && non_space(chars, j - 1) && boundary(chars, j + 2)
}

/// Finds the `}}` that closes the monospaced span opened at `i`, scanning across nested `{{ … }}`
/// pairs so an inner span does not end the outer one. Returns the index of the closing `}}`, or
/// `None` when the span is never closed.
fn match_monospace_close(chars: &[char], i: usize, hi: usize) -> Option<usize> {
    // The shared budget keeps nested-open scanning linear; a pathological run stays literal.
    let mut budget = scan_budget(i, hi);
    match_monospace_close_within(chars, i, hi, &mut budget, 0)
}

fn match_monospace_close_within(
    chars: &[char],
    i: usize,
    hi: usize,
    budget: &mut usize,
    depth: usize,
) -> Option<usize> {
    // Nesting cap keeps deeply stacked braces off the call stack.
    if depth > MAX_INLINE_DEPTH {
        return None;
    }
    let mut j = i + 2;
    while j < hi {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        if chars.get(j) == Some(&'{')
            && chars.get(j + 1) == Some(&'{')
            && can_open_monospace(chars, j)
            && let Some(nested) = match_monospace_close_within(chars, j, hi, budget, depth + 1)
        {
            j = nested + 2;
            continue;
        }
        if chars.get(j) == Some(&'}')
            && chars.get(j + 1) == Some(&'}')
            && closes_monospace(chars, i, j)
        {
            return Some(j);
        }
        j += 1;
    }
    None
}

fn parse_brace_inline(
    chars: &[char],
    i: usize,
    hi: usize,
    autolink: bool,
    depth: usize,
    budget: &mut usize,
) -> Option<(Inline, usize)> {
    if chars.get(i + 1) == Some(&'{') {
        // Monospaced `{{ … }}`: a nested pair is skipped rather than ending the span early.
        if !can_open_monospace(chars, i) {
            return None;
        }
        let close = match_monospace_close(chars, i, hi)?;
        let inner = inlines_with(chars, i + 2, close, autolink, depth + 1);
        let text = carta_ast::to_plain_text(&inner);
        return Some((Inline::Code(Box::default(), text.into()), close + 2));
    }

    if matches_at(chars, i, "{color:") {
        let value_start = i + "{color:".len();
        let value_end = find_within(chars, value_start..hi, budget, |c| c == '}')?;
        let value = color_value(&slice_to_string(chars, value_start, value_end))?;
        let close = match_color_close(chars, value_end + 1, hi, budget)?;
        let inner = inlines_with(chars, value_end + 1, close, autolink, depth + 1);
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: vec![("color".into(), value.into())],
        };
        return Some((Inline::Span(Box::new(attr), inner), close + "{color}".len()));
    }

    if matches_at(chars, i, "{anchor:") {
        let name_start = i + "{anchor:".len();
        let name_end = find_within(chars, name_start..hi, budget, |c| c == '}')?;
        let name: String = chars
            .get(name_start..name_end)
            .unwrap_or_default()
            .iter()
            .filter(|c| !is_space(**c))
            .collect();
        let attr = Attr {
            id: name.into(),
            classes: Vec::new(),
            attributes: Vec::new(),
        };
        return Some((Inline::Span(Box::new(attr), Vec::new()), name_end + 1));
    }

    None
}

/// Validates and normalises a colour value. A recognised value is one of: a name of letters (any
/// Unicode letters, not only ASCII); a `#` followed by exactly six hexadecimal digits; or six
/// hexadecimal digits with a leading decimal digit, which is normalised by prepending `#`. Anything
/// else leaves the `{color:…}` markup as literal text.
pub(super) fn color_value(value: &str) -> Option<String> {
    if let Some(hex) = value.strip_prefix('#') {
        return (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| value.to_string());
    }
    if !value.is_empty() && value.chars().all(char::is_alphabetic) {
        return Some(value.to_string());
    }
    if value.len() == 6
        && value.bytes().all(|b| b.is_ascii_hexdigit())
        && value.bytes().next().is_some_and(|b| b.is_ascii_digit())
    {
        return Some(format!("#{value}"));
    }
    None
}

/// Finds the `{color}` that closes the inline colour span whose content begins at `from`, balancing
/// nested `{color:…}` opens so an inner close does not end the outer span early. Returns the index of
/// the closing token, or `None` when the span is never closed within `from..hi`.
fn match_color_close(chars: &[char], from: usize, hi: usize, budget: &mut usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut k = from;
    while k < hi {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        if matches_at(chars, k, "{color:") {
            depth += 1;
            k += "{color:".len();
        } else if matches_at(chars, k, "{color}") {
            depth -= 1;
            if depth == 0 {
                return Some(k);
            }
            k += "{color}".len();
        } else {
            k += 1;
        }
    }
    None
}

/// Matches a symbol or emoticon token at `i`. The token is recognised wherever the character that
/// follows it is a boundary (end of input or a non-alphanumeric character); the character before it
/// is irrelevant, so a symbol may abut the end of a preceding word.
fn match_token_symbol(chars: &[char], i: usize, table: &[(&str, char)]) -> Option<(char, usize)> {
    for (token, glyph) in table {
        let len = token.chars().count();
        if matches_at(chars, i, token) && boundary(chars, i + len) {
            return Some((*glyph, len));
        }
    }
    None
}
