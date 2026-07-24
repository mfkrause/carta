//! Inline text, autolinking, and typographic helper functions.

use super::escape_uri;
use crate::inline_text::trim_inline_ends;
use crate::smart_fold::{QuoteCtx, can_open_quote};
use carta_ast::{Inline, QuoteType, Target};

// --- inline text helpers -----------------------------------------------------------------------

/// Append raw text to an inline sequence, splitting on the regular space into words and single
/// spaces, with embedded newlines becoming soft breaks and space runs collapsing. Other whitespace
/// (such as a non-breaking space) stays part of its surrounding word.
pub(super) fn push_text(out: &mut Vec<Inline>, text: &str) {
    let mut word = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word).into()));
            }
            out.push(Inline::SoftBreak);
        } else if ch == ' ' || ch == '\t' {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word).into()));
            }
            // Keep a leading space: callers that must not start with one trim; interpreted text keeps it.
            if !matches!(out.last(), Some(Inline::Space | Inline::SoftBreak)) {
                out.push(Inline::Space);
            }
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word.into()));
    }
}

/// Convert verbatim text into inlines without interpreting further markup: emphasis and strong
/// spans do not nest, so their content is plain text with only backslash escapes resolved.
pub(super) fn literal_text(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut resolved = String::new();
    let mut pos = 0;
    while let Some(&ch) = chars.get(pos) {
        if ch == '\\' {
            match chars.get(pos + 1) {
                Some(next) if next.is_whitespace() => pos += 2,
                Some(next) => {
                    resolved.push(*next);
                    pos += 2;
                }
                None => {
                    resolved.push('\\');
                    pos += 1;
                }
            }
            continue;
        }
        resolved.push(ch);
        pos += 1;
    }
    let mut out = Vec::new();
    push_text(&mut out, &resolved);
    trim_inline_ends(&mut out);
    out
}

/// Whether markup is suppressed because it is wrapped in a matching pair of quoting characters: a
/// run opened by `"`, `'`, or `<` and closed by its partner keeps its contents as literal text.
pub(super) fn quote_suppresses(before: Option<char>, after: Option<char>) -> bool {
    matches!(
        (before, after),
        (Some('"'), Some('"')) | (Some('\''), Some('\'')) | (Some('<'), Some('>'))
    )
}

/// The trailing simple-reference name in accumulated text, with the character that precedes it. A
/// simple reference name is a run of alphanumerics joined by isolated internal punctuation drawn
/// from `-_.:+` (no two adjacent, none leading or trailing), so the name both starts and ends with
/// an alphanumeric. Returns `None` when the text does not end in such a name. The returned name is a
/// suffix of `pending`.
pub(super) fn trailing_reference_name(pending: &str) -> Option<(String, Option<char>)> {
    let chars: Vec<char> = pending.chars().collect();
    let last = chars.last()?;
    if !last.is_alphanumeric() {
        return None;
    }
    let mut start = chars.len() - 1;
    loop {
        if start == 0 {
            break;
        }
        let prev = chars.get(start - 1).copied();
        if prev.is_some_and(char::is_alphanumeric) {
            start -= 1;
            continue;
        }
        // Internal punctuation extends the name only after an alphanumeric, so it never leads the name.
        if prev.is_some_and(|c| matches!(c, '-' | '_' | '.' | ':' | '+'))
            && start
                .checked_sub(2)
                .and_then(|i| chars.get(i))
                .copied()
                .is_some_and(char::is_alphanumeric)
        {
            start -= 2;
            continue;
        }
        break;
    }
    let name: String = chars.get(start..)?.iter().collect();
    let before = start.checked_sub(1).and_then(|i| chars.get(i)).copied();
    Some((name, before))
}

/// Whether the character before a markup start string allows it to begin markup: a boundary, a
/// whitespace, or one of the opening punctuation characters.
pub(super) fn inline_start_ok(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => {
            c.is_whitespace() || matches!(c, '-' | ':' | '/' | '\'' | '"' | '<' | '(' | '[' | '{')
        }
    }
}

/// Whether the character after a markup end string allows it to end markup: a boundary, a
/// whitespace, or one of the closing punctuation characters.
pub(super) fn inline_end_ok(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || matches!(
                    c,
                    '-' | '.'
                        | ','
                        | ':'
                        | ';'
                        | '!'
                        | '?'
                        | '\\'
                        | '/'
                        | '\''
                        | '"'
                        | ')'
                        | ']'
                        | '}'
                        | '>'
                )
        }
    }
}

fn matches_at(chars: &[char], at: usize, delim: &[char]) -> bool {
    delim
        .iter()
        .enumerate()
        .all(|(k, d)| chars.get(at + k) == Some(d))
}

/// Find the closing delimiter of a verbatim span (inline literal, interpreted text, substitution,
/// bracketed label): the next occurrence of the delimiter. Returns the verbatim inner text and the
/// index past the closing delimiter.
pub(super) fn find_close_literal(
    chars: &[char],
    start: usize,
    delim: &str,
) -> Option<(String, usize)> {
    let dchars: Vec<char> = delim.chars().collect();
    let mut i = start;
    while i < chars.len() {
        if matches_at(chars, i, &dchars) {
            let content: String = chars.get(start..i)?.iter().collect();
            return Some((content, i + dchars.len()));
        }
        i += 1;
    }
    None
}

/// Parse a role token `:name:` beginning at `pos` (which must be the opening colon), returning the
/// role name and the index past the closing colon.
pub(super) fn parse_role(chars: &[char], pos: usize) -> Option<(String, usize)> {
    if chars.get(pos) != Some(&':') {
        return None;
    }
    let mut name = String::new();
    let mut i = pos + 1;
    while let Some(&c) = chars.get(i) {
        if c == ':' {
            if name.is_empty() {
                return None;
            }
            return Some((name, i + 1));
        }
        if c.is_alphanumeric() || matches!(c, '-' | '_' | '+' | '.') {
            name.push(c);
            i += 1;
        } else {
            return None;
        }
    }
    None
}

/// Split interpreted-text content into its display label and an optional embedded destination
/// `<uri>`.
pub(super) fn split_embedded_uri(text: &str) -> (String, Option<String>) {
    let trimmed = text.trim_end();
    if trimmed.ends_with('>')
        && let Some(open) = text.rfind('<')
        && let Some(close) = text.rfind('>')
        && open < close
    {
        let url = text.get(open + 1..close).unwrap_or("").trim().to_string();
        let label = text.get(..open).unwrap_or("").trim().to_string();
        return (label, Some(url));
    }
    (text.to_string(), None)
}

// --- bare URI and email autolinking ------------------------------------------------------------

/// Whether the character before a candidate autolink permits it to begin: a boundary, whitespace, or
/// an opening bracket. This keeps an address that is already part of larger markup (an angle-bracket
/// URI, a word fragment) from being linked twice.
pub(super) fn autolink_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '(' | '[' | '{'),
    }
}

/// Attempt to auto-link a bare URI or email address beginning at `pos`.
pub(super) fn autolink(chars: &[char], pos: usize) -> Option<(Inline, usize)> {
    try_uri_autolink(chars, pos).or_else(|| try_email_autolink(chars, pos))
}

/// Match a bare URI `scheme://…` whose scheme is registered, returning the link and the end index.
fn try_uri_autolink(chars: &[char], pos: usize) -> Option<(Inline, usize)> {
    if !chars.get(pos).is_some_and(char::is_ascii_alphabetic) {
        return None;
    }
    let mut k = pos;
    while chars
        .get(k)
        .is_some_and(|&c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        k += 1;
    }
    if !(chars.get(k) == Some(&':')
        && chars.get(k + 1) == Some(&'/')
        && chars.get(k + 2) == Some(&'/'))
    {
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
    // The link text shows the URL as written; the destination is percent-encoded.
    Some((
        Inline::Link(
            Box::default(),
            vec![Inline::Str(url.clone().into())],
            Box::new(Target {
                url: escape_uri(&url).into(),
                title: carta_ast::Text::default(),
            }),
        ),
        end,
    ))
}

/// Match a bare email address `local@domain`, returning a `mailto:` link and the end index.
fn try_email_autolink(chars: &[char], pos: usize) -> Option<(Inline, usize)> {
    let mut i = pos;
    while chars.get(i).is_some_and(|&c| is_email_local(c)) {
        i += 1;
    }
    if i == pos || chars.get(i) != Some(&'@') {
        return None;
    }
    i += 1;
    let domain_start = i;
    let mut dots = 0usize;
    let mut end = i;
    loop {
        let label_start = i;
        if !chars.get(i).is_some_and(char::is_ascii_alphanumeric) {
            break;
        }
        while chars
            .get(i)
            .is_some_and(|&c| c.is_ascii_alphanumeric() || c == '-')
        {
            i += 1;
        }
        let mut label_end = i;
        while label_end > label_start && chars.get(label_end - 1) == Some(&'-') {
            label_end -= 1;
        }
        end = label_end;
        i = label_end;
        if chars.get(i) == Some(&'.') {
            dots += 1;
            i += 1;
        } else {
            break;
        }
    }
    if dots == 0 || end <= domain_start {
        return None;
    }
    let address: String = chars.get(pos..end)?.iter().collect();
    Some((
        Inline::Link(
            Box::default(),
            vec![Inline::Str(address.clone().into())],
            Box::new(Target {
                url: format!("mailto:{address}").into(),
                title: carta_ast::Text::default(),
            }),
        ),
        end,
    ))
}

/// Whether a character may appear in an email address's local part.
fn is_email_local(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '.' | '!'
                | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '/'
                | '='
                | '?'
                | '^'
                | '_'
                | '`'
                | '{'
                | '|'
                | '}'
                | '~'
                | '-'
        )
}

/// Walk a URL run forward to its raw extent, stopping only at whitespace or an angle bracket
/// (`<` or `>`). Brackets are taken in; whether a trailing one belongs to the URL is decided by
/// [`trim_trailing`] from the run's bracket balance.
fn forward_scan(chars: &[char], from: usize) -> usize {
    let mut j = from;
    while let Some(&c) = chars.get(j) {
        if c.is_whitespace() || matches!(c, '<' | '>') {
            break;
        }
        j += 1;
    }
    j
}

/// The number of occurrences of `target` in `chars[min..end]`.
fn count_char(chars: &[char], min: usize, end: usize, target: char) -> usize {
    chars
        .get(min..end)
        .map_or(0, |run| run.iter().filter(|&&c| c == target).count())
}

/// Drop trailing punctuation from a URL run, never below `min`. A trailing `;` takes a preceding
/// `&entity;` with it. A trailing closing bracket is dropped only when it is unbalanced within the
/// run (there are more of it than its matching opener), so a bracketed path stays whole.
fn trim_trailing(chars: &[char], min: usize, mut end: usize) -> usize {
    while end > min {
        match chars.get(end - 1) {
            Some('!' | '"' | '\'' | '*' | ',' | '.' | ':' | '?' | '_' | '~') => end -= 1,
            Some(&close @ (')' | ']' | '}')) => {
                let open = match close {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                if count_char(chars, min, end, close) > count_char(chars, min, end, open) {
                    end -= 1;
                } else {
                    break;
                }
            }
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

// --- typographic punctuation (smart) -----------------------------------------------------------

/// The number of consecutive `ch` at `pos`.
pub(super) fn run_length(chars: &[char], pos: usize, ch: char) -> usize {
    let mut n = 0;
    while chars.get(pos + n) == Some(&ch) {
        n += 1;
    }
    n
}

/// The quote-node kind for a straight quote character.
pub(super) fn quote_type(quote: char) -> QuoteType {
    if quote == '\'' {
        QuoteType::SingleQuote
    } else {
        QuoteType::DoubleQuote
    }
}

/// The curly glyph a non-paired straight quote folds into: an apostrophe for `'`, and an opening or
/// closing double quote depending on which side it leans.
pub(super) fn quote_glyph(chars: &[char], pos: usize, quote: char) -> char {
    if quote == '\'' {
        '\u{2019}'
    } else if can_open_quote(chars, pos, quote, QuoteCtx::default()) {
        '\u{201c}'
    } else {
        '\u{201d}'
    }
}
