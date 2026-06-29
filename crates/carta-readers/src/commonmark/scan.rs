//! Shared raw-text scanners over character slices, used by both parsing phases.
//!
//! These are pure functions: given a `&[char]` (or `&str`) and a start index, each recognizes one
//! construct — an autolink, a raw inline HTML tag, a character reference, a link destination /
//! title / label, or a full link reference definition — and returns the parsed value together with
//! the index just past it. They hold no parser state. The inline phase drives most of them; the
//! block phase reuses the link-reference-definition and unescaping scanners while collecting
//! definitions.

use carta_ast::{Attr, Inline, Target};

use super::LinkDef;

/// Percent-encode the characters a link destination may not safely carry literally: ASCII
/// whitespace and the delimiters `< > | " { } [ ] ^` and the backtick. Every other byte passes
/// through unchanged — including a literal `%`, so an existing `%XX` sequence is preserved rather
/// than doubled — as does all non-ASCII text. Applying it twice yields the same result.
pub(crate) fn escape_uri(url: &str) -> String {
    fn hex(nibble: u8) -> char {
        char::from_digit(u32::from(nibble), 16)
            .unwrap_or('0')
            .to_ascii_uppercase()
    }
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        if ch.is_ascii_whitespace()
            || matches!(
                ch,
                '<' | '>' | '|' | '"' | '{' | '}' | '[' | ']' | '^' | '`'
            )
        {
            let byte = ch as u8;
            out.push('%');
            out.push(hex(byte >> 4));
            out.push(hex(byte & 0x0f));
        } else {
            out.push(ch);
        }
    }
    out
}

pub(crate) fn is_ascii_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '!' | '"'
            | '#'
            | '$'
            | '%'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | '-'
            | '.'
            | '/'
            | ':'
            | ';'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '_'
            | '`'
            | '{'
            | '|'
            | '}'
            | '~'
    )
}

pub(crate) fn scan_autolink(chars: &[char], start: usize) -> Option<(Inline, usize)> {
    if chars.get(start).copied() != Some('<') {
        return None;
    }
    let mut end = start + 1;
    let mut content = String::new();
    while let Some(&ch) = chars.get(end) {
        if ch == '>' {
            break;
        }
        if ch == '<' || ch.is_whitespace() {
            return None;
        }
        content.push(ch);
        end += 1;
    }
    if chars.get(end).copied() != Some('>') {
        return None;
    }
    let after = end + 1;
    if is_uri_autolink(&content) {
        let target = Target {
            url: content.clone(),
            title: String::new(),
        };
        return Some((
            Inline::Link(Attr::default(), vec![Inline::Str(content)], target),
            after,
        ));
    }
    if is_email_autolink(&content) {
        let url = format!("mailto:{content}");
        let target = Target {
            url,
            title: String::new(),
        };
        return Some((
            Inline::Link(Attr::default(), vec![Inline::Str(content)], target),
            after,
        ));
    }
    None
}

fn is_uri_autolink(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once(':') else {
        return false;
    };
    let scheme_ok = (2..=32).contains(&scheme.len())
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-');
    scheme_ok && !text.chars().any(|c| c.is_control() || c == ' ')
}

fn is_email_autolink(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(c))
        && domain.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Recognize raw inline HTML at `start` (an open/closing tag, comment, processing instruction,
/// declaration, or CDATA section) per spec §6.6. Returns the verbatim text and the end position.
pub(crate) fn scan_html_tag(chars: &[char], start: usize) -> Option<(String, usize)> {
    let end = match_html(chars, start)?;
    let text: String = chars.get(start..end)?.iter().collect();
    Some((text, end))
}

fn match_html(chars: &[char], start: usize) -> Option<usize> {
    if chars.get(start).copied() != Some('<') {
        return None;
    }
    match chars.get(start + 1).copied()? {
        '/' => match_closing_tag(chars, start + 2),
        '?' => match_until(chars, start + 2, "?>"),
        '!' => match_declaration(chars, start),
        c if c.is_ascii_alphabetic() => match_open_tag(chars, start + 1),
        _ => None,
    }
}

fn match_open_tag(chars: &[char], mut index: usize) -> Option<usize> {
    index = match_tag_name(chars, index)?;
    while let Some(next) = match_attribute(chars, index) {
        index = next;
    }
    index = skip_html_whitespace(chars, index);
    if chars.get(index).copied() == Some('/') {
        index += 1;
    }
    (chars.get(index).copied() == Some('>')).then_some(index + 1)
}

fn match_closing_tag(chars: &[char], index: usize) -> Option<usize> {
    let index = skip_html_whitespace(chars, match_tag_name(chars, index)?);
    (chars.get(index).copied() == Some('>')).then_some(index + 1)
}

/// A comment (`<!-->`, `<!--->`, or `<!--` … `-->`), CDATA section, or declaration. `start` points
/// at `<`; the following `!` is already known.
fn match_declaration(chars: &[char], start: usize) -> Option<usize> {
    let body = start + 2;
    if chars.get(body).copied() == Some('-') && chars.get(body + 1).copied() == Some('-') {
        let after = body + 2;
        if chars.get(after).copied() == Some('>') {
            return Some(after + 1);
        }
        if chars.get(after).copied() == Some('-') && chars.get(after + 1).copied() == Some('>') {
            return Some(after + 2);
        }
        return match_until(chars, after, "-->");
    }
    if matches_at(chars, body, "[CDATA[") {
        return match_until(chars, body + 7, "]]>");
    }
    if chars
        .get(body)
        .copied()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return match_until_char(chars, body + 1, '>');
    }
    None
}

fn match_tag_name(chars: &[char], index: usize) -> Option<usize> {
    if !chars.get(index).copied()?.is_ascii_alphabetic() {
        return None;
    }
    let mut end = index + 1;
    while chars
        .get(end)
        .copied()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        end += 1;
    }
    Some(end)
}

/// An attribute: at least one whitespace, an attribute name, then an optional value specification.
fn match_attribute(chars: &[char], index: usize) -> Option<usize> {
    let after_space = skip_html_whitespace(chars, index);
    if after_space == index {
        return None;
    }
    let mut end = match_attribute_name(chars, after_space)?;
    if let Some(next) = match_attribute_value_spec(chars, end) {
        end = next;
    }
    Some(end)
}

fn match_attribute_name(chars: &[char], index: usize) -> Option<usize> {
    let first = chars.get(index).copied()?;
    if !(first.is_ascii_alphabetic() || first == '_' || first == ':') {
        return None;
    }
    let mut end = index + 1;
    while chars
        .get(end)
        .copied()
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
    {
        end += 1;
    }
    Some(end)
}

fn match_attribute_value_spec(chars: &[char], index: usize) -> Option<usize> {
    let equals = skip_html_whitespace(chars, index);
    if chars.get(equals).copied() != Some('=') {
        return None;
    }
    let value = skip_html_whitespace(chars, equals + 1);
    match_attribute_value(chars, value)
}

fn match_attribute_value(chars: &[char], index: usize) -> Option<usize> {
    match chars.get(index).copied()? {
        '\'' => match_until_char(chars, index + 1, '\''),
        '"' => match_until_char(chars, index + 1, '"'),
        _ => {
            let mut end = index;
            while chars.get(end).copied().is_some_and(|c| {
                !matches!(
                    c,
                    ' ' | '\t' | '\n' | '\r' | '"' | '\'' | '=' | '<' | '>' | '`'
                )
            }) {
                end += 1;
            }
            (end != index).then_some(end)
        }
    }
}

fn skip_html_whitespace(chars: &[char], mut index: usize) -> usize {
    while matches!(chars.get(index).copied(), Some(' ' | '\t' | '\n' | '\r')) {
        index += 1;
    }
    index
}

pub(crate) fn matches_at(chars: &[char], index: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(offset, c)| chars.get(index + offset).copied() == Some(c))
}

/// Position just past the first occurrence of `needle` at or after `from`, or `None` if absent.
fn match_until(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let pattern: Vec<char> = needle.chars().collect();
    let mut index = from;
    while index + pattern.len() <= chars.len() {
        if chars.get(index..index + pattern.len()) == Some(pattern.as_slice()) {
            return Some(index + pattern.len());
        }
        index += 1;
    }
    None
}

fn match_until_char(chars: &[char], from: usize, needle: char) -> Option<usize> {
    let mut index = from;
    while let Some(c) = chars.get(index).copied() {
        if c == needle {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

pub(crate) fn scan_entity(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start).copied() != Some('&') {
        return None;
    }
    let semi =
        (start + 1..(start + 33).min(chars.len())).find(|&i| chars.get(i).copied() == Some(';'))?;
    let body: String = chars
        .get(start + 1..semi)
        .map(|s| s.iter().collect())
        .unwrap_or_default();
    let decoded = decode_entity(&body)?;
    Some((decoded, semi + 1))
}

fn decode_entity(body: &str) -> Option<String> {
    if let Some(num) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
        // A hexadecimal reference is one to six digits.
        if num.is_empty() || num.len() > 6 || !num.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let code = u32::from_str_radix(num, 16).ok()?;
        return Some(crate::entities::code_point(code).to_string());
    }
    if let Some(num) = body.strip_prefix('#') {
        // A decimal reference is one to seven digits.
        if num.is_empty() || num.len() > 7 || !num.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let code: u32 = num.parse().ok()?;
        return Some(crate::entities::code_point(code).to_string());
    }
    crate::entities::lookup_named(body).map(str::to_owned)
}

/// Scan an inline link tail `(url "title")` beginning at `pos` (which points at `(`).
pub(crate) fn scan_inline_target(chars: &[char], pos: usize) -> Option<(Target, usize)> {
    let mut index = pos + 1;
    skip_inline_whitespace(chars, &mut index);
    let (url, next) = scan_destination(chars, index)?;
    index = next;
    skip_inline_whitespace(chars, &mut index);
    let mut title = String::new();
    if matches!(chars.get(index).copied(), Some('"' | '\'' | '(')) {
        let (parsed, after) = scan_title(chars, index)?;
        title = parsed;
        index = after;
        skip_inline_whitespace(chars, &mut index);
    }
    if chars.get(index).copied() != Some(')') {
        return None;
    }
    Some((
        Target {
            url: unescape_string(&url),
            title: unescape_string(&title),
        },
        index + 1,
    ))
}

fn scan_destination(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut index = start;
    if chars.get(index).copied() == Some('<') {
        index += 1;
        let mut out = String::new();
        while let Some(&ch) = chars.get(index) {
            match ch {
                '>' => return Some((out, index + 1)),
                '<' | '\n' => return None,
                '\\' if chars
                    .get(index + 1)
                    .is_some_and(|c| is_ascii_punctuation(*c)) =>
                {
                    if let Some(&next) = chars.get(index + 1) {
                        out.push(next);
                    }
                    index += 2;
                }
                _ => {
                    out.push(ch);
                    index += 1;
                }
            }
        }
        return None;
    }
    let mut out = String::new();
    let mut depth = 0;
    while let Some(&ch) = chars.get(index) {
        match ch {
            ' ' => break,
            c if c.is_control() => break,
            '(' => {
                depth += 1;
                out.push('(');
                index += 1;
            }
            ')' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                out.push(')');
                index += 1;
            }
            '\\' if chars
                .get(index + 1)
                .is_some_and(|c| is_ascii_punctuation(*c)) =>
            {
                if let Some(&next) = chars.get(index + 1) {
                    out.push(next);
                }
                index += 2;
            }
            _ => {
                out.push(ch);
                index += 1;
            }
        }
    }
    if out.is_empty() && depth == 0 {
        return Some((out, index));
    }
    if depth != 0 {
        return None;
    }
    Some((out, index))
}

fn scan_title(chars: &[char], start: usize) -> Option<(String, usize)> {
    let open = chars.get(start).copied()?;
    let close = match open {
        '"' => '"',
        '\'' => '\'',
        '(' => ')',
        _ => return None,
    };
    let mut index = start + 1;
    let mut out = String::new();
    while let Some(&ch) = chars.get(index) {
        if ch == close {
            return Some((out, index + 1));
        }
        if ch == '\\'
            && chars
                .get(index + 1)
                .is_some_and(|c| is_ascii_punctuation(*c))
        {
            if let Some(&next) = chars.get(index + 1) {
                out.push(next);
            }
            index += 2;
            continue;
        }
        out.push(ch);
        index += 1;
    }
    None
}

/// Scan a `[label]` immediately following a `]`, returning the raw label and the next position.
pub(crate) fn scan_following_label(chars: &[char], pos: usize) -> Option<(String, usize)> {
    if chars.get(pos).copied() != Some('[') {
        return None;
    }
    let mut index = pos + 1;
    let mut out = String::new();
    while let Some(&ch) = chars.get(index) {
        match ch {
            ']' => return Some((out, index + 1)),
            '[' => return None,
            '\\' if chars
                .get(index + 1)
                .is_some_and(|c| is_ascii_punctuation(*c)) =>
            {
                out.push('\\');
                if let Some(&next) = chars.get(index + 1) {
                    out.push(next);
                }
                index += 2;
            }
            _ => {
                out.push(ch);
                index += 1;
            }
        }
    }
    None
}

fn skip_inline_whitespace(chars: &[char], index: &mut usize) {
    while matches!(chars.get(*index).copied(), Some(' ' | '\t' | '\n')) {
        *index += 1;
    }
}

/// Normalize a link label per the spec: trim, collapse internal whitespace to single spaces, and
/// apply Unicode case folding (so e.g. `ẞ` and `SS` match).
pub(crate) fn normalize_label(label: &str) -> String {
    let collapsed = label.split_whitespace().collect::<Vec<_>>().join(" ");
    caseless::default_case_fold_str(&collapsed)
}

/// Remove backslash escapes of ASCII punctuation from a string, leaving other backslashes intact.
pub(crate) fn unescape_string(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while let Some(&ch) = chars.get(index) {
        if ch == '\\'
            && let Some(&next) = chars.get(index + 1)
            && is_ascii_punctuation(next)
        {
            out.push(next);
            index += 2;
            continue;
        }
        if ch == '&'
            && let Some((decoded, next)) = scan_entity(&chars, index)
        {
            out.push_str(&decoded);
            index = next;
            continue;
        }
        out.push(ch);
        index += 1;
    }
    out
}

/// Parse a leading link reference definition from `text`. Returns the normalized label, the
/// resolved definition, and the unconsumed remainder of `text`. In the markdown dialect
/// (`markdown`), an unbracketed destination may carry internal spaces (each whitespace run joining
/// its words with a single space) and may be empty; the bare `CommonMark` form ends a destination at
/// the first space and requires a non-empty unbracketed destination.
pub(crate) fn parse_link_reference_definition(
    text: &str,
    markdown: bool,
) -> Option<(String, LinkDef, &str)> {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    skip_spaces_up_to_three(&chars, &mut index);
    if chars.get(index).copied() != Some('[') {
        return None;
    }
    index += 1;
    let mut label = String::new();
    let mut closed = false;
    while let Some(&ch) = chars.get(index) {
        match ch {
            ']' => {
                closed = true;
                index += 1;
                break;
            }
            '[' => return None,
            '\\' if chars
                .get(index + 1)
                .is_some_and(|c| is_ascii_punctuation(*c)) =>
            {
                label.push('\\');
                if let Some(&next) = chars.get(index + 1) {
                    label.push(next);
                }
                index += 2;
            }
            _ => {
                label.push(ch);
                index += 1;
            }
        }
    }
    if !closed || chars.get(index).copied() != Some(':') {
        return None;
    }
    index += 1;
    skip_inline_whitespace_no_double_newline(&chars, &mut index)?;
    let angle = chars.get(index).copied() == Some('<');

    let (url, title) = if markdown && !angle {
        let (url, same_line_title, line_end) = scan_markdown_reference_body(&chars, index)?;
        index = line_end;
        if let Some(title) = same_line_title {
            (url, title)
        } else {
            let (title, end) = scan_following_title(&chars, index)?;
            index = end;
            (url, title)
        }
    } else {
        let (url, next) = scan_destination(&chars, index)?;
        // A bare (non-angle) destination must be non-empty; `<>` is a valid empty destination.
        if url.is_empty() && !angle {
            return None;
        }
        let (title, end) = scan_following_title(&chars, next)?;
        index = end;
        (url, title)
    };

    // Consume the trailing newline.
    if chars.get(index).copied() == Some('\n') {
        index += 1;
    }

    let normalized = normalize_label(&label);
    if normalized.is_empty() {
        return None;
    }
    let def = LinkDef {
        url: unescape_string(&url),
        title: unescape_string(&title),
    };
    let consumed_bytes: usize = chars
        .get(..index)
        .map_or(0, |s| s.iter().map(|c| c.len_utf8()).sum());
    let rest = text.get(consumed_bytes..).unwrap_or("");
    Some((normalized, def, rest))
}

/// After a destination ending at `after_dest`, scan an optional title separated from it by
/// whitespace and at most one newline. Returns the title (empty when absent) together with the
/// index at the end of the definition's last line. Returns `None` when non-whitespace other than a
/// well-formed title follows the destination, which invalidates the whole definition.
fn scan_following_title(chars: &[char], after_dest: usize) -> Option<(String, usize)> {
    let mut probe = after_dest;
    let mut newlines = 0;
    while let Some(ch) = chars.get(probe).copied() {
        match ch {
            ' ' | '\t' => probe += 1,
            '\n' if newlines == 0 => {
                newlines += 1;
                probe += 1;
            }
            _ => break,
        }
    }
    let separated = probe > after_dest;
    if separated
        && matches!(chars.get(probe).copied(), Some('"' | '\'' | '('))
        && let Some((parsed, after)) = scan_title(chars, probe)
    {
        let mut tail = after;
        skip_blanks_to_line_end(chars, &mut tail);
        if at_line_end(chars, tail) {
            return Some((parsed, tail));
        }
    }
    let mut index = after_dest;
    skip_blanks_to_line_end(chars, &mut index);
    if !at_line_end(chars, index) {
        return None;
    }
    Some((String::new(), index))
}

/// The outcome of testing whether a title begins at a given position in a markdown reference body.
enum TitleScan {
    /// A title that occupies the rest of the line. Carries the raw title and the index at line end.
    Title(String, usize),
    /// A parenthesized title that parses but is not the line's last element — invalid here.
    Reject,
    /// A title delimiter that does not form a line-ending title; its characters are literal text.
    Literal,
    /// No title delimiter at this position.
    Absent,
}

/// Test whether a `"..."`, `'...'`, or `(...)` title begins at `at` and ends the line. A title token
/// requires its closing delimiter to be followed by whitespace or the line's end; with trailing
/// non-whitespace after that, the definition is invalid. A quote whose closing delimiter abuts more
/// text is not a title token at all and reverts to literal destination text, whereas a parenthesized
/// run in that position still invalidates the definition.
fn try_reference_title(chars: &[char], at: usize) -> TitleScan {
    let Some(opener @ ('"' | '\'' | '(')) = chars.get(at).copied() else {
        return TitleScan::Absent;
    };
    match scan_title(chars, at) {
        Some((parsed, after)) => {
            if at_line_end(chars, after) || matches!(chars.get(after).copied(), Some(' ' | '\t')) {
                let mut tail = after;
                skip_blanks_to_line_end(chars, &mut tail);
                if at_line_end(chars, tail) {
                    TitleScan::Title(parsed, tail)
                } else {
                    TitleScan::Reject
                }
            } else if opener == '(' {
                TitleScan::Reject
            } else {
                TitleScan::Literal
            }
        }
        None => TitleScan::Literal,
    }
}

/// Scan a markdown-dialect unbracketed reference destination starting at `start`, where the
/// destination may hold spaces and balanced parentheses. The destination runs to the end of the
/// line, save for a trailing `"..."`, `'...'`, or `(...)` title separated by whitespace (or one at
/// the very start, which leaves the destination empty). Returns the raw destination, a same-line
/// title when one ends the line, and the index at line end. Returns `None` when the line cannot form
/// a valid definition (a parenthesized title not at the line's end).
fn scan_markdown_reference_body(
    chars: &[char],
    start: usize,
) -> Option<(String, Option<String>, usize)> {
    let mut index = start;
    match try_reference_title(chars, index) {
        TitleScan::Title(parsed, end) => return Some((String::new(), Some(parsed), end)),
        TitleScan::Reject => return None,
        TitleScan::Literal | TitleScan::Absent => {}
    }
    let mut url = String::new();
    let mut depth: usize = 0;
    loop {
        match chars.get(index).copied() {
            None | Some('\n') => break,
            Some('(') => {
                depth += 1;
                url.push('(');
                index += 1;
            }
            Some(')') => {
                depth = depth.saturating_sub(1);
                url.push(')');
                index += 1;
            }
            Some('\\') if matches!(chars.get(index + 1).copied(), Some(' ' | '\t')) => {
                url.push(' ');
                index += 2;
            }
            Some('\\')
                if chars
                    .get(index + 1)
                    .copied()
                    .is_some_and(is_ascii_punctuation) =>
            {
                url.push('\\');
                if let Some(&next) = chars.get(index + 1) {
                    url.push(next);
                }
                index += 2;
            }
            Some(' ' | '\t') => {
                let mut after = index;
                skip_blanks_to_line_end(chars, &mut after);
                if at_line_end(chars, after) {
                    index = after;
                    break;
                }
                if depth == 0 {
                    match try_reference_title(chars, after) {
                        TitleScan::Title(parsed, end) => return Some((url, Some(parsed), end)),
                        TitleScan::Reject => return None,
                        TitleScan::Literal | TitleScan::Absent => {
                            url.push(' ');
                            index = after;
                        }
                    }
                } else {
                    url.push(' ');
                    index = after;
                }
            }
            Some(ch) => {
                url.push(ch);
                index += 1;
            }
        }
    }
    Some((url, None, index))
}

fn skip_spaces_up_to_three(chars: &[char], index: &mut usize) {
    let mut count = 0;
    while count < 3 && chars.get(*index).copied() == Some(' ') {
        *index += 1;
        count += 1;
    }
}

fn skip_inline_whitespace_no_double_newline(chars: &[char], index: &mut usize) -> Option<()> {
    let mut newlines = 0;
    while let Some(&ch) = chars.get(*index) {
        match ch {
            ' ' | '\t' => *index += 1,
            '\n' => {
                newlines += 1;
                if newlines > 1 {
                    return None;
                }
                *index += 1;
            }
            _ => break,
        }
    }
    Some(())
}

fn skip_blanks_to_line_end(chars: &[char], index: &mut usize) {
    while matches!(chars.get(*index).copied(), Some(' ' | '\t')) {
        *index += 1;
    }
}

fn at_line_end(chars: &[char], index: usize) -> bool {
    matches!(chars.get(index).copied(), None | Some('\n'))
}

#[cfg(test)]
mod tests {
    use super::escape_uri;

    #[test]
    fn unsafe_characters_become_uppercase_percent_escapes() {
        assert_eq!(escape_uri("two words"), "two%20words");
        assert_eq!(escape_uri("a{b}c"), "a%7Bb%7Dc");
        assert_eq!(escape_uri("p^q"), "p%5Eq");
        assert_eq!(
            escape_uri("a<b>c|d\"e[f]g`h"),
            "a%3Cb%3Ec%7Cd%22e%5Bf%5Dg%60h"
        );
    }

    #[test]
    fn a_literal_percent_is_never_encoded_so_the_pass_is_idempotent() {
        assert_eq!(escape_uri("a%20b"), "a%20b");
        let once = escape_uri("two words {x}");
        assert_eq!(escape_uri(&once), once);
    }

    #[test]
    fn backslashes_and_non_ascii_text_pass_through() {
        assert_eq!(escape_uri("a\\b/café"), "a\\b/café");
    }
}
