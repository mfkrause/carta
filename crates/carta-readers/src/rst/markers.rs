//! Adornments, list and field markers, and explicit-markup block classification.

use super::{dedent, indent_of, is_blank, line_at};
use crate::roman::roman_value_loose_reverse;
use carta_ast::{ListNumberDelim, ListNumberStyle};

// --- adornments and markers --------------------------------------------------------------------

pub(super) const ADORNMENT_CHARS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

/// The single repeated punctuation character of a section adornment or transition line, or `None`
/// when the line is not a run of one such character.
pub(super) fn adornment_char(line: &str) -> Option<char> {
    let trimmed = line.trim();
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    if !ADORNMENT_CHARS.contains(first) {
        return None;
    }
    if chars.all(|c| c == first) {
        Some(first)
    } else {
        None
    }
}

const BULLETS: &str = "*+-\u{2022}\u{2023}\u{2043}";

/// For a bullet list item, the column at which its content begins.
pub(super) fn bullet_content_col(line: &str) -> Option<usize> {
    let mut chars = line.chars();
    let marker = chars.next()?;
    if !BULLETS.contains(marker) {
        return None;
    }
    match chars.next() {
        None => Some(1),
        Some(' ') => Some(2 + chars.take_while(|c| *c == ' ').count()),
        Some(_) => None,
    }
}

/// The parsed leading marker of an enumerated list item: its start value, numeral style, delimiter,
/// and the column at which its content begins.
pub(super) fn enumerator(line: &str) -> Option<(i32, ListNumberStyle, ListNumberDelim, usize)> {
    let bytes: Vec<char> = line.chars().collect();
    let (two_parens, numeral_start) = match bytes.first() {
        Some('(') => (true, 1),
        _ => (false, 0),
    };
    let mut end = numeral_start;
    while let Some(ch) = bytes.get(end) {
        if ch.is_ascii_alphanumeric() || *ch == '#' {
            end += 1;
        } else {
            break;
        }
    }
    let numeral: String = bytes.get(numeral_start..end)?.iter().collect();
    if numeral.is_empty() {
        return None;
    }
    let (style, start) = classify_numeral(&numeral)?;
    let delim = if two_parens {
        if bytes.get(end) != Some(&')') {
            return None;
        }
        end += 1;
        ListNumberDelim::TwoParens
    } else {
        match bytes.get(end) {
            Some('.') => {
                end += 1;
                ListNumberDelim::Period
            }
            Some(')') => {
                end += 1;
                ListNumberDelim::OneParen
            }
            _ => return None,
        }
    };
    // An auto-numbered (`#`) enumerator carries no concrete style or delimiter.
    let delim = if numeral == "#" {
        ListNumberDelim::DefaultDelim
    } else {
        delim
    };
    // An enumerator must be followed by whitespace; a marker that ends the line is ordinary text.
    match bytes.get(end) {
        Some(' ') => {
            let spaces = bytes
                .get(end + 1..)?
                .iter()
                .take_while(|c| **c == ' ')
                .count();
            Some((start, style, delim, end + 1 + spaces))
        }
        _ => None,
    }
}

fn classify_numeral(numeral: &str) -> Option<(ListNumberStyle, i32)> {
    if numeral == "#" {
        return Some((ListNumberStyle::DefaultStyle, 1));
    }
    if numeral.chars().all(|c| c.is_ascii_digit()) {
        return numeral
            .parse::<i32>()
            .ok()
            .map(|n| (ListNumberStyle::Decimal, n));
    }
    // A lone letter is alphabetic except `i`/`I`; a multi-letter valid Roman numeral (`iv`, `xii`) is Roman.
    let mut chars = numeral.chars();
    let single = chars.next()?;
    if chars.next().is_none() && single.is_ascii_alphabetic() && !matches!(single, 'i' | 'I') {
        let ordinal = i32::from((single.to_ascii_lowercase() as u8) - b'a' + 1);
        let style = if single.is_ascii_uppercase() {
            ListNumberStyle::UpperAlpha
        } else {
            ListNumberStyle::LowerAlpha
        };
        return Some((style, ordinal));
    }
    if let Some(value) = roman_value_loose_reverse(numeral) {
        let style = if numeral.chars().all(|c| c.is_ascii_uppercase()) {
            ListNumberStyle::UpperRoman
        } else {
            ListNumberStyle::LowerRoman
        };
        return Some((style, value));
    }
    None
}

/// Whether `ch` is one of the letters that form a Roman numeral.
fn is_roman_letter(ch: char) -> bool {
    matches!(
        ch.to_ascii_lowercase(),
        'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm'
    )
}

/// The leading enumerator numeral of `line` (the token before its delimiter) when `line` opens
/// with an enumerator. Used to reinterpret an ambiguous single-letter enumerator in the context of
/// an already-established list style.
fn enum_numeral(line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let start = usize::from(chars.first() == Some(&'('));
    let mut end = start;
    while chars
        .get(end)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '#')
    {
        end += 1;
    }
    let numeral: String = chars.get(start..end)?.iter().collect();
    if numeral.is_empty() {
        None
    } else {
        Some(numeral)
    }
}

/// Whether a single-letter `numeral` continues a list whose established `style` it does not match on
/// its own: any letter (of the style's case) continues an alphabetic list; only a Roman-numeral
/// letter continues a Roman list.
fn letter_continues(numeral: &str, style: ListNumberStyle) -> bool {
    let mut chars = numeral.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return false;
    };
    if !ch.is_ascii_alphabetic() {
        return false;
    }
    let upper = ch.is_ascii_uppercase();
    match style {
        ListNumberStyle::UpperAlpha => upper,
        ListNumberStyle::LowerAlpha => !upper,
        ListNumberStyle::UpperRoman => upper && is_roman_letter(ch),
        ListNumberStyle::LowerRoman => !upper && is_roman_letter(ch),
        _ => false,
    }
}

/// Whether the enumerator opening `line` can belong to a list whose first item established `style`
/// and `delim`. An auto-numbered (`#`) item joins any list and vice versa; otherwise the delimiter
/// must match and the style must match directly or by an ambiguous single letter adopting it.
pub(super) fn enum_compatible(line: &str, style: ListNumberStyle, delim: ListNumberDelim) -> bool {
    let Some((_, s, d, _)) = enumerator(line) else {
        return false;
    };
    let item_auto = s == ListNumberStyle::DefaultStyle && d == ListNumberDelim::DefaultDelim;
    let list_auto =
        style == ListNumberStyle::DefaultStyle && delim == ListNumberDelim::DefaultDelim;
    let style_ok = style == s || enum_numeral(line).is_some_and(|n| letter_continues(&n, style));
    item_auto || list_auto || (style_ok && delim == d)
}

/// Whether the enumerated-list item whose first line is `lines[idx]` (content column `col`) is a
/// well-formed item rather than the opening of an ordinary wrapped paragraph. The line after the
/// item's first line must be blank, indented into the item, or itself a matching sibling enumerator;
/// an under-indented line of ordinary text means the construct is a paragraph, not a list.
pub(super) fn item_well_formed(
    lines: &[String],
    idx: usize,
    col: usize,
    style: ListNumberStyle,
    delim: ListNumberDelim,
) -> bool {
    let next = line_at(lines, idx + 1);
    if is_blank(next) || indent_of(next) >= col {
        return true;
    }
    enum_compatible(next, style, delim)
}

/// A field marker `:name: value`: the field name and the column at which the value begins.
pub(super) fn field_marker(line: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    if chars.first() != Some(&':') {
        return None;
    }
    let mut idx = 1;
    while let Some(ch) = chars.get(idx) {
        if *ch == ':' && (chars.get(idx + 1).is_none() || chars.get(idx + 1) == Some(&' ')) {
            let name: String = chars.get(1..idx)?.iter().collect();
            if name.is_empty() {
                return None;
            }
            let value_col = if chars.get(idx + 1).is_some() {
                idx + 2
            } else {
                idx + 1
            };
            return Some((name, value_col));
        }
        if *ch == ':' && idx == 1 {
            return None;
        }
        idx += 1;
    }
    None
}

/// An option-list marker: an option group (a comma-separated run of `-a`, `-fARG`, `-f ARG`,
/// `--word`, `--word=ARG`, or `/S` options) that fully fills the line up to the first run of two
/// or more spaces (or the end of line). Returns the option-group text and the column at which the
/// description body begins. The group must consume its entire candidate span: a trailing token
/// after a single-space gap (e.g. `-f FILE extra`) is ordinary prose, not an option list.
pub(super) fn option_marker(line: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let gap = chars.windows(2).position(|pair| pair == [' ', ' ']);
    let candidate_end = gap.unwrap_or(chars.len());
    let candidate: String = chars.get(..candidate_end)?.iter().collect();
    let candidate = candidate.trim_end();
    if !valid_option_group(candidate) {
        return None;
    }
    let value_col = match gap {
        Some(g) => {
            let mut v = g;
            while chars.get(v) == Some(&' ') {
                v += 1;
            }
            v
        }
        None => candidate.chars().count(),
    };
    Some((candidate.to_string(), value_col))
}

/// Whether `text` is a complete, comma-separated group of option specifiers with nothing left over.
fn valid_option_group(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return false;
    }
    let mut i = 0;
    loop {
        let Some(next) = parse_one_option(&chars, i) else {
            return false;
        };
        i = next;
        if i == chars.len() {
            return true;
        }
        // Options are joined by a comma and a single space.
        if chars.get(i) == Some(&',') && chars.get(i + 1) == Some(&' ') {
            i += 2;
        } else {
            return false;
        }
    }
}

/// Parse a single option specifier starting at `i`, returning the index just past it (and any
/// argument). Recognizes long options (`--word`, `--word=ARG`, `--word ARG`), short options
/// (`-a`, `-aARG`, `-a ARG`), and DOS-style options (`/S`, `/S ARG`). Returns `None` if no valid
/// specifier begins at `i`.
fn parse_one_option(chars: &[char], i: usize) -> Option<usize> {
    match chars.get(i) {
        Some('-') if chars.get(i + 1) == Some(&'-') => {
            // Long option: a name of letters, digits, and hyphens.
            let mut j = i + 2;
            let name_start = j;
            while chars
                .get(j)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '-')
            {
                j += 1;
            }
            if j == name_start {
                return None;
            }
            parse_optional_arg(chars, j)
        }
        Some('-') => {
            // Short option: a hyphen and exactly one alphanumeric character.
            let ch = chars.get(i + 1)?;
            if !ch.is_ascii_alphanumeric() {
                return None;
            }
            parse_optional_arg(chars, i + 2)
        }
        Some('/') => {
            // DOS/VMS-style option: a slash and exactly one alphanumeric character.
            let ch = chars.get(i + 1)?;
            if !ch.is_ascii_alphanumeric() {
                return None;
            }
            parse_optional_arg(chars, i + 2)
        }
        _ => None,
    }
}

/// Parse an optional argument that follows an option specifier at `i`: an `=`-delimited argument,
/// a single-space-delimited argument, or an argument attached directly to a short option. Returns
/// the index just past the option (and argument, if present).
fn parse_optional_arg(chars: &[char], i: usize) -> Option<usize> {
    let delim = chars.get(i);
    if delim == Some(&'=') || delim == Some(&' ') {
        let arg_start = i + 1;
        let mut j = arg_start;
        while chars
            .get(j)
            .is_some_and(|c| !c.is_whitespace() && *c != ',')
        {
            j += 1;
        }
        if j == arg_start {
            return None;
        }
        return Some(j);
    }
    // An argument attached directly to a short option (e.g. `-fARG`).
    let mut j = i;
    while chars
        .get(j)
        .is_some_and(|c| !c.is_whitespace() && *c != ',')
    {
        j += 1;
    }
    Some(j)
}

// --- explicit markup blocks --------------------------------------------------------------------

/// The extent of an explicit-markup block (a `..` or `__` construct, a directive, or a comment):
/// the index one past its last content line. The block runs over its first line plus all following
/// blank or further-indented lines, up to but not including the next line indented no more than the
/// marker.
pub(super) fn explicit_extent(lines: &[String], start: usize, marker_indent: usize) -> usize {
    let mut last_content = start;
    let mut i = start + 1;
    while let Some(line) = lines.get(i) {
        if is_blank(line) {
            i += 1;
        } else if indent_of(line) > marker_indent {
            last_content = i;
            i += 1;
        } else {
            break;
        }
    }
    last_content + 1
}

/// The body region of an explicit-markup block: the first line's text after `prefix_len` columns,
/// followed by the continuation lines dedented by their shared minimum indentation. A leading empty
/// first-line remainder is dropped.
pub(super) fn explicit_body(
    lines: &[String],
    start: usize,
    end: usize,
    prefix_len: usize,
) -> Vec<String> {
    let mut body = Vec::new();
    let first = line_at(lines, start);
    let remainder: String = first.chars().skip(prefix_len).collect();
    if !remainder.trim().is_empty() {
        body.push(remainder.trim_start().to_string());
    }
    let continuation: Vec<&String> = (start + 1..end).filter_map(|i| lines.get(i)).collect();
    let min_indent = continuation
        .iter()
        .filter(|l| !is_blank(l))
        .map(|l| indent_of(l))
        .min()
        .unwrap_or(0);
    for line in continuation {
        if is_blank(line) {
            body.push(String::new());
        } else {
            body.push(dedent(line, min_indent));
        }
    }
    while body.last().is_some_and(std::string::String::is_empty) {
        body.pop();
    }
    body
}

/// A classified explicit-markup construct, by the first non-`..` token on its line.
pub(super) enum Explicit {
    Target,
    AnonymousTarget,
    Footnote(String),
    Citation(String),
    Substitution,
    Directive(String),
    Comment,
}

pub(super) fn classify_explicit(line: &str) -> Option<Explicit> {
    let trimmed = line.trim_start();
    if trimmed == "__" || trimmed.starts_with("__ ") {
        return Some(Explicit::AnonymousTarget);
    }
    if trimmed != ".." && !trimmed.starts_with(".. ") {
        return None;
    }
    let rest = trimmed.strip_prefix("..").unwrap_or("").trim_start();
    if rest.is_empty() {
        return Some(Explicit::Comment);
    }
    if rest.starts_with("__") {
        return Some(Explicit::AnonymousTarget);
    }
    if rest.starts_with('_') {
        return Some(Explicit::Target);
    }
    if let Some(after) = rest.strip_prefix('[') {
        if let Some(close) = after.find(']') {
            let label = after.get(..close).unwrap_or("");
            if !label.is_empty() {
                return Some(if is_citation_label(label) {
                    Explicit::Citation(label.to_string())
                } else {
                    Explicit::Footnote(label.to_string())
                });
            }
        }
        return Some(Explicit::Comment);
    }
    if rest.starts_with('|') {
        return Some(Explicit::Substitution);
    }
    if let Some(name) = directive_name(rest) {
        return Some(Explicit::Directive(name));
    }
    Some(Explicit::Comment)
}

/// A footnote label is a number, `#`, `#name`, or `*`; any other bracket label is a citation.
pub(super) fn is_citation_label(label: &str) -> bool {
    !(label.chars().all(|c| c.is_ascii_digit())
        || label == "*"
        || label == "#"
        || label.starts_with('#'))
}

/// The lowercased name of a directive (`name::`), or `None` when the text is not a directive.
fn directive_name(rest: &str) -> Option<String> {
    let end = rest.find("::")?;
    let name = rest.get(..end)?;
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.' | ':'))
    {
        return None;
    }
    Some(name.to_lowercase())
}
