//! Smart-typography folds shared by the readers that curl straight punctuation runs.

/// Fold a run of `len` dots into one ellipsis (`…`) per group of three, leaving the remaining one or
/// two dots literal.
pub(crate) fn fold_ellipsis_run(len: usize) -> String {
    let mut out = String::with_capacity(len / 3 * 3 + len % 3);
    out.extend(std::iter::repeat_n('\u{2026}', len / 3));
    out.extend(std::iter::repeat_n('.', len % 3));
    out
}

/// Fold a run of `n` hyphens into em and en dashes, greedily preferring em dashes: every three
/// become an em dash (`—`), a remaining two a single en dash (`–`), a remaining one a literal
/// hyphen.
#[cfg(any(
    feature = "rst",
    feature = "dokuwiki",
    feature = "opml",
    feature = "html"
))]
pub(crate) fn fold_dash_run_greedy(n: usize) -> String {
    let mut s = "\u{2014}".repeat(n / 3);
    match n % 3 {
        2 => s.push('\u{2013}'),
        1 => s.push('-'),
        _ => {}
    }
    s
}

/// Fold a run of `len` hyphens (`len >= 2`) into the fewest em (`—`) and en (`–`) dashes that sum to
/// its length: a multiple of three is all em dashes, an even length is all en dashes, and an odd
/// length that is not a multiple of three takes one or two en dashes — whichever leaves a multiple of
/// three — with the rest em dashes.
///
/// The other smart readers fold dash runs greedily instead ([`fold_dash_run_greedy`]), so this
/// minimal decomposition is `CommonMark`-only.
#[cfg(feature = "commonmark")]
pub(crate) fn fold_dash_run_thirds(len: usize) -> String {
    let (em, en) = if len.is_multiple_of(3) {
        (len / 3, 0)
    } else if len.is_multiple_of(2) {
        (0, len / 2)
    } else {
        let en = if len % 3 == 1 { 2 } else { 1 };
        ((len - 2 * en) / 3, en)
    };
    let mut out = String::with_capacity((em + en) * 3);
    out.extend(std::iter::repeat_n('\u{2014}', em));
    out.extend(std::iter::repeat_n('\u{2013}', en));
    out
}

// --- quote flanking ---

/// Which quote kinds already enclose the current scan. A quote of a kind already open does not open
/// again; the straight quote folds to its apostrophe or curly glyph instead. Readers without nested
/// quote tracking pass the default (nothing open), which never suppresses an opener.
#[cfg(any(feature = "rst", feature = "dokuwiki"))]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct QuoteCtx {
    pub(crate) in_single: bool,
    pub(crate) in_double: bool,
}

/// The character before `pos`, if any.
#[cfg(any(feature = "rst", feature = "dokuwiki"))]
fn before_char(chars: &[char], pos: usize) -> Option<char> {
    pos.checked_sub(1).and_then(|p| chars.get(p)).copied()
}

/// Whether an optional character is whitespace, treating a missing character (a boundary) as
/// whitespace.
#[cfg(any(feature = "rst", feature = "dokuwiki"))]
pub(crate) fn is_ws_opt(opt: Option<char>) -> bool {
    opt.is_none_or(char::is_whitespace)
}

/// Whether a character counts as punctuation for flanking: ASCII punctuation, or any other
/// non-alphanumeric, non-whitespace character.
#[cfg(any(feature = "rst", feature = "dokuwiki"))]
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace())
}

/// Whether an optional character is punctuation, treating a missing character as not punctuation.
#[cfg(any(feature = "rst", feature = "dokuwiki"))]
fn is_punct_opt(opt: Option<char>) -> bool {
    opt.is_some_and(is_punct)
}

/// Whether the single character at `pos` is left-flanking (it leans against following content).
#[cfg(any(feature = "rst", feature = "dokuwiki"))]
pub(crate) fn left_flanking(chars: &[char], pos: usize) -> bool {
    let before = before_char(chars, pos);
    let after = chars.get(pos + 1).copied();
    !is_ws_opt(after) && (!is_punct_opt(after) || is_ws_opt(before) || is_punct_opt(before))
}

/// Whether the single character at `pos` is right-flanking (it leans against preceding content).
#[cfg(any(feature = "rst", feature = "dokuwiki"))]
fn right_flanking(chars: &[char], pos: usize) -> bool {
    let before = before_char(chars, pos);
    let after = chars.get(pos + 1).copied();
    !is_ws_opt(before) && (!is_punct_opt(before) || is_ws_opt(after) || is_punct_opt(after))
}

/// Whether a straight quote at `pos` may open a quoted run. A quote whose kind already encloses the
/// position may not open again, so nested same-kind quotation never forms.
#[cfg(any(feature = "rst", feature = "dokuwiki"))]
pub(crate) fn can_open_quote(chars: &[char], pos: usize, quote: char, qctx: QuoteCtx) -> bool {
    if (quote == '\'' && qctx.in_single) || (quote == '"' && qctx.in_double) {
        return false;
    }
    left_flanking(chars, pos)
}

/// Whether a straight quote at `pos` may close a quoted run. A single quote may not close against a
/// following alphanumeric, so a word-internal apostrophe never ends a quotation.
#[cfg(any(feature = "rst", feature = "dokuwiki"))]
pub(crate) fn can_close_quote(chars: &[char], pos: usize, quote: char) -> bool {
    if !right_flanking(chars, pos) {
        return false;
    }
    if quote == '\'' {
        !chars.get(pos + 1).is_some_and(|c| c.is_alphanumeric())
    } else {
        true
    }
}
