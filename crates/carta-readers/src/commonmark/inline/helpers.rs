//! Free helpers for the inline scanners: citation keys, link targets, backtick runs, and escapes.

use carta_ast::{Inline, Target};

use super::super::LinkDef;
use super::super::scan::{
    char_at, escape_uri, is_ascii_punctuation, scan_inline_target, unescape_string,
};
use super::char_before;

/// A character that may appear directly before `@` and block a bare citation: an alphanumeric
/// glues the `@` to a preceding word (`foo@bar`, an email-like run), so no citation forms there.
pub(super) fn is_citation_word(ch: char) -> bool {
    ch.is_alphanumeric()
}

/// Tokenize raw citation source into the literal inlines that stand in for the citation: whitespace
/// runs become `SoftBreak` (when they hold a newline) or `Space`, and every other run becomes a
/// `Str`. No inline markup is interpreted, so the source reads back verbatim word by word.
pub(super) fn citation_fallback_inlines(raw: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut had_space = false;
    let mut had_newline = false;
    let flush_word = |out: &mut Vec<Inline>, word: &mut String| {
        if !word.is_empty() {
            out.push(Inline::Str(std::mem::take(word).into()));
        }
    };
    for ch in raw.chars() {
        if ch.is_whitespace() {
            flush_word(&mut out, &mut word);
            had_space = true;
            had_newline |= ch == '\n';
        } else {
            if had_space {
                out.push(if had_newline {
                    Inline::SoftBreak
                } else {
                    Inline::Space
                });
                had_space = false;
                had_newline = false;
            }
            word.push(ch);
        }
    }
    flush_word(&mut out, &mut word);
    if had_space {
        out.push(if had_newline {
            Inline::SoftBreak
        } else {
            Inline::Space
        });
    }
    out
}

/// A character that always belongs to a citation key: an alphanumeric or `_`. A key begins with one
/// of these.
fn is_citation_key_start(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Scan a citation key beginning at `start` (the index just past `@`). A key opens with an
/// alphanumeric or `_` and runs over further such characters; the internal punctuation `-`, `.`,
/// `:`, and `/` extend it only when another key character follows, so a trailing `-key.` keeps the
/// `key` but drops the `.`. Returns the key text and the index just past it, or `None` when no key
/// begins at `start`.
pub(super) fn scan_citation_id(text: &str, start: usize) -> Option<(String, usize)> {
    let first = char_at(text, start)?;
    if !is_citation_key_start(first) {
        return None;
    }
    let mut end = start + first.len_utf8();
    while let Some(ch) = char_at(text, end) {
        if is_citation_key_start(ch) {
            end += ch.len_utf8();
        } else if matches!(ch, '-' | '.' | ':' | '/')
            // Punctuation is ASCII: the next key char begins one byte on.
            && matches!(char_at(text, end + 1), Some(next) if is_citation_key_start(next))
        {
            end += 1 + char_at(text, end + 1).map_or(0, char::len_utf8);
        } else {
            break;
        }
    }
    let id = text.get(start..end)?.to_owned();
    Some((id, end))
}

/// Advance past one escape, backtick code span, or bracket at `index`, updating bracket `depth` and
/// returning the next index. Returns `None` when the character is none of those; the caller then
/// inspects it for a top-level delimiter (`;` or `@`) and advances itself.
fn step_citation_scan(text: &str, index: usize, depth: &mut usize) -> Option<usize> {
    match char_at(text, index) {
        Some('\\') => Some(index + 1 + char_at(text, index + 1).map_or(0, char::len_utf8)),
        Some('`') => {
            let run = backtick_run_len(text, index);
            Some(skip_code_span(text, index, run))
        }
        Some('[') => {
            *depth += 1;
            Some(index + 1)
        }
        Some(']') => {
            *depth = depth.saturating_sub(1);
            Some(index + 1)
        }
        _ => None,
    }
}

/// Split a bracket's raw content into citation segments on top-level semicolons. A semicolon inside
/// a nested `[...]` or a backtick code span does not split. Returns `None` when the content is not a
/// citation list: it has no `@` at all, or any segment is empty (including a leading or trailing
/// empty segment from a stray semicolon).
pub(super) fn split_citation_segments(text: &str) -> Option<Vec<std::ops::Range<usize>>> {
    if !text.contains('@') {
        return None;
    }
    let mut segments = Vec::new();
    let mut start = 0;
    let mut index = 0;
    let mut depth = 0usize;
    while index < text.len() {
        if let Some(next) = step_citation_scan(text, index, &mut depth) {
            index = next;
        } else if char_at(text, index) == Some(';') && depth == 0 {
            segments.push(start..index);
            start = index + 1;
            index += 1;
        } else {
            index += char_at(text, index).map_or(1, char::len_utf8);
        }
    }
    segments.push(start..text.len());
    // An empty segment (a stray `;`, or whitespace-only between separators) is not a citation list.
    for segment in &segments {
        if text
            .get(segment.clone())
            .is_none_or(|s| s.chars().all(char::is_whitespace))
        {
            return None;
        }
    }
    Some(segments)
}

/// The length in bytes of the backtick run starting at byte offset `index`.
pub(super) fn backtick_run_len(text: &str, index: usize) -> usize {
    let bytes = text.as_bytes();
    let mut len = 0;
    while bytes.get(index + len) == Some(&b'`') {
        len += 1;
    }
    len
}

/// Skip past a code span opened by `run` backticks at `index`, returning the index just past its
/// closing run. With no matching closer the backticks are not a code span and only the opening run
/// is skipped.
fn skip_code_span(text: &str, index: usize, run: usize) -> usize {
    let bytes = text.as_bytes();
    let mut scan = index + run;
    while scan < bytes.len() {
        if bytes.get(scan) == Some(&b'`') {
            let closer = backtick_run_len(text, scan);
            if closer == run {
                return scan + closer;
            }
            scan += closer;
        } else {
            scan += 1;
        }
    }
    index + run
}

/// The located citation key of one segment: the index of `@`, the key text and the index past it,
/// whether a `-` author-suppression marker precedes the `@`, and that marker's index.
pub(super) struct CitationKey {
    pub(super) at: usize,
    pub(super) dash: usize,
    pub(super) id: String,
    pub(super) id_end: usize,
    pub(super) suppress: bool,
}

/// Find the first top-level `@key` within `chars[range]`: an `@` not inside a nested `[...]` or a
/// backtick code span, immediately followed by a key. A `-` directly before the `@`, itself at the
/// segment start or preceded by whitespace, marks author suppression. Returns `None` when no such
/// key is present.
pub(super) fn find_citation_key(text: &str, range: std::ops::Range<usize>) -> Option<CitationKey> {
    let mut index = range.start;
    let mut depth = 0usize;
    while index < range.end {
        if let Some(next) = step_citation_scan(text, index, &mut depth) {
            index = next;
            continue;
        }
        if depth == 0
            && char_at(text, index) == Some('@')
            && let Some((id, id_end)) = scan_citation_id(text, index + 1)
        {
            // `-` is ASCII: when it precedes the `@` it sits at byte `index - 1`.
            let dash_before = index > range.start && char_before(text, index) == Some('-');
            let dash_anchored = dash_before
                && (index - 1 == range.start
                    || char_before(text, index - 1).is_some_and(char::is_whitespace));
            let suppress = dash_anchored;
            return Some(CitationKey {
                at: index,
                dash: if suppress { index - 1 } else { index },
                id,
                id_end,
                suppress,
            });
        }
        index += char_at(text, index).map_or(1, char::len_utf8);
    }
    None
}

pub(super) fn def_target(def: &LinkDef) -> Target {
    Target {
        url: def.url.clone().into(),
        title: def.title.clone().into(),
    }
}

/// Scan an inline link tail `(destination "title")` in the markdown dialect, where the unbracketed
/// destination may hold spaces (percent-encoded to `%20`) and balanced parentheses. The destination
/// runs until the parenthesis that balances the link's opener, save for a trailing quoted title
/// separated by whitespace. Returns `None` when the parentheses are unbalanced or a quoted title is
/// not immediately followed by the closing parenthesis. `pos` points at the opening `(`.
pub(super) fn scan_markdown_inline_target(text: &str, pos: usize) -> Option<(Target, usize)> {
    let mut index = pos + 1;
    skip_target_whitespace(text, &mut index);
    if char_at(text, index) == Some('<') {
        // Angle-bracketed form has no special space handling; defer to the shared scanner.
        return scan_inline_target(text, pos);
    }
    let mut url = String::new();
    let mut title = String::new();
    let mut depth: usize = 0;
    loop {
        match char_at(text, index) {
            None => return None,
            Some(')') if depth == 0 => {
                index += 1;
                break;
            }
            Some(')') => {
                depth -= 1;
                url.push(')');
                index += 1;
            }
            Some('(') => {
                depth += 1;
                url.push('(');
                index += 1;
            }
            // An escaped space is destination content (never a title separator), encoded `%20`.
            Some('\\') if matches!(char_at(text, index + 1), Some(' ' | '\t')) => {
                url.push_str("%20");
                index += 2;
            }
            Some('\\') if char_at(text, index + 1).is_some_and(is_ascii_punctuation) => {
                if let Some(next) = char_at(text, index + 1) {
                    url.push('\\');
                    url.push(next);
                    index += 1 + next.len_utf8();
                }
            }
            Some(ch) if ch == ' ' || ch == '\t' => {
                let mut after = index;
                skip_target_whitespace(text, &mut after);
                match char_at(text, after) {
                    // Trailing whitespace before the closing parenthesis ends the destination.
                    Some(')') if depth == 0 => {
                        index = after;
                    }
                    // A quoted title must be the last element before `)`, else the tail fails.
                    Some('"' | '\'') if depth == 0 => {
                        let (parsed, mut close) = scan_target_title(text, after)?;
                        title = parsed;
                        skip_target_whitespace(text, &mut close);
                        if char_at(text, close) != Some(')') {
                            return None;
                        }
                        index = close + 1;
                        break;
                    }
                    // More destination follows: the whitespace run joins it as a single `%20`.
                    Some(_) => {
                        url.push_str("%20");
                        index = after;
                    }
                    None => return None,
                }
            }
            Some(ch) => {
                url.push(ch);
                index += ch.len_utf8();
            }
        }
    }
    Some((
        Target {
            url: unescape_string(&url).into(),
            title: unescape_string(&title).into(),
        },
        index,
    ))
}

/// Advance `index` past a run of spaces and tabs.
fn skip_target_whitespace(text: &str, index: &mut usize) {
    while matches!(char_at(text, *index), Some(' ' | '\t')) {
        *index += 1;
    }
}

/// Scan a quoted link title starting at `start` (a `"` or `'`), returning its raw content and the
/// index just past the closing quote. A backslash escapes the following punctuation character.
fn scan_target_title(text: &str, start: usize) -> Option<(String, usize)> {
    let close = char_at(text, start)?;
    if close != '"' && close != '\'' {
        return None;
    }
    let mut index = start + 1;
    let mut out = String::new();
    while let Some(ch) = char_at(text, index) {
        if ch == close {
            return Some((out, index + 1));
        }
        if ch == '\\'
            && let Some(next) = char_at(text, index + 1)
            && is_ascii_punctuation(next)
        {
            out.push('\\');
            out.push(next);
            index += 1 + next.len_utf8();
            continue;
        }
        out.push(ch);
        index += ch.len_utf8();
    }
    None
}

/// Tag an explicit angle-bracket autolink with its kind. A link whose visible text differs from its
/// destination is an email autolink (the destination gained a `mailto:` scheme), so it carries the
/// `email` class; every other angle autolink is a URI autolink and carries `uri`. Bare URLs linked
/// by the autolink post-pass keep an empty class list and never reach here.
pub(super) fn classify_angle_autolink(inline: Inline) -> Inline {
    let Inline::Link(mut attr, text, target) = inline else {
        return inline;
    };
    let is_email = matches!(text.first(), Some(Inline::Str(shown)) if *shown != target.url);
    attr.classes
        .push(if is_email { "email" } else { "uri" }.into());
    Inline::Link(attr, text, target)
}

/// Percent-encode the destination of a link, leaving its shown text untouched.
pub(super) fn escape_link_destination(inline: Inline) -> Inline {
    let Inline::Link(attr, text, mut target) = inline else {
        return inline;
    };
    target.url = escape_uri(&target.url).into();
    Inline::Link(attr, text, target)
}

/// The characters an original-Markdown backslash escape recognizes regardless of
/// `all_symbols_escapable`: the inline delimiters and block markers that carry syntactic weight. A
/// dialect without the broad escape set drops the backslash before only these, leaving every other
/// backslash literal.
pub(super) fn is_classic_markdown_escapable(ch: char) -> bool {
    matches!(
        ch,
        '\\' | '`'
            | '*'
            | '_'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | '>'
            | '#'
            | '+'
            | '-'
            | '.'
            | '!'
    )
}

/// Normalize the interior of a code span: line endings to spaces, and if it both begins and ends
/// with a space (and is not all spaces), strip one space from each end.
pub(super) fn normalize_code(content: &str, markdown: bool) -> String {
    let collapsed: String = content
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    // Markdown dialect strips all surrounding whitespace; strict removes one space from each end
    // and only when the content is not all spaces.
    if markdown {
        return collapsed.trim().to_owned();
    }
    let bytes = collapsed.as_bytes();
    if collapsed.len() >= 2
        && bytes.first() == Some(&b' ')
        && bytes.last() == Some(&b' ')
        && !collapsed.chars().all(|c| c == ' ')
    {
        collapsed
            .get(1..collapsed.len() - 1)
            .unwrap_or("")
            .to_owned()
    } else {
        collapsed
    }
}
