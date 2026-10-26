//! Scanning and parsing of HTML-style tags embedded in wikitext.

use super::{HtmlTagRole, ScanBounds, Tok, at, collect_range, find_char};

/// If a verbatim tag opens at `i`, the index just past its closing tag (or end of input).
pub(super) fn verbatim_region_end(
    chars: &[char],
    i: usize,
    bounds: ScanBounds<'_>,
) -> Option<usize> {
    let (name, _raw, self_closing, after_open) = open_tag_bounded(chars, i, bounds)?;
    if !matches!(
        name.as_str(),
        "pre" | "nowiki" | "math" | "source" | "syntaxhighlight"
    ) {
        return None;
    }
    if self_closing {
        return Some(after_open);
    }
    match close_tag_bounded(chars, after_open, &name, bounds) {
        Some((_, after)) => Some(after),
        None => Some(chars.len()),
    }
}

/// `open_tag` guarded by `bounds`: yields `None` immediately when no `>` remains at or after `start`,
/// which is exactly when `open_tag` would scan to the end of input and fail.
pub(super) fn open_tag_bounded(
    chars: &[char],
    start: usize,
    bounds: ScanBounds<'_>,
) -> Option<(String, String, bool, usize)> {
    if !bounds.open_possible(start) {
        return None;
    }
    let end = if let Some((index, offset, limit)) = bounds.index {
        Some(index.tag_end(start + 1 + offset, limit)? - offset)
    } else {
        None
    };
    open_tag(chars, start, end)
}

/// `close_tag` guarded by `bounds`: yields `None` immediately when no `</` remains at or after
/// `start`, which is exactly when no matching closer can exist and `close_tag` would scan to the end
/// of input and fail.
pub(super) fn close_tag_bounded(
    chars: &[char],
    start: usize,
    name: &str,
    bounds: ScanBounds<'_>,
) -> Option<(usize, usize)> {
    if !bounds.close_possible(start) {
        return None;
    }
    if let Some((index, offset, end)) = bounds.index {
        return index
            .close(start + offset, end, name)
            .map(|(position, after)| (position - offset, after - offset));
    }
    close_tag(chars, start, name)
}

/// Reads an opening tag at `chars[start]`, returning its lowercased name, the raw `<…>` text,
/// whether it is self-closing, and the index just past the `>`. Attribute values in quotes may
/// contain `>`.
fn open_tag(
    chars: &[char],
    start: usize,
    end: Option<usize>,
) -> Option<(String, String, bool, usize)> {
    let mut cursor = start + 1;
    let mut name = String::new();
    while let Some(ch) = at(chars, cursor) {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
            cursor += 1;
        } else {
            break;
        }
    }
    if name.is_empty() {
        return None;
    }
    if let Some(end) = end {
        cursor = end;
    }
    let mut quote: Option<char> = None;
    let len = chars.len();
    while cursor < len {
        let Some(ch) = at(chars, cursor) else { break };
        match quote {
            Some(open_quote) => {
                if ch == open_quote {
                    quote = None;
                }
                cursor += 1;
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                    cursor += 1;
                } else if ch == '>' {
                    break;
                } else {
                    cursor += 1;
                }
            }
        }
    }
    if at(chars, cursor) != Some('>') {
        return None;
    }
    let self_closing = cursor > 0 && at(chars, cursor - 1) == Some('/');
    let raw = collect_range(chars, start, cursor + 1);
    Some((name, raw, self_closing, cursor + 1))
}

/// Finds the matching `</name>` for an element whose content begins at `start`, counting nested
/// same-named tags. Returns the index where the closing tag begins and the index just past its `>`.
pub(super) fn close_tag(chars: &[char], start: usize, name: &str) -> Option<(usize, usize)> {
    let mut depth = 0i32;
    let mut j = start;
    let n = chars.len();
    while j < n {
        if at(chars, j) == Some('<') {
            if at(chars, j + 1) == Some('/') {
                if tag_name_matches(chars, j + 2, name) {
                    if depth == 0 {
                        let gt = find_char(chars, j, '>')?;
                        return Some((j, gt + 1));
                    }
                    depth -= 1;
                }
            } else if tag_name_matches(chars, j + 1, name) {
                depth += 1;
            }
        }
        j += 1;
    }
    None
}

/// The content of an element starting at `start` together with the index just past its closing tag;
/// an unterminated element runs to the end of input.
pub(super) fn enclosed(
    chars: &[char],
    start: usize,
    name: &str,
    bounds: ScanBounds<'_>,
) -> (String, usize) {
    match close_tag_bounded(chars, start, name, bounds) {
        Some((inner_end, after)) => (collect_range(chars, start, inner_end), after),
        None => (collect_range(chars, start, chars.len()), chars.len()),
    }
}

pub(super) fn tag_name_matches(chars: &[char], pos: usize, name: &str) -> bool {
    let mut count = 0;
    for (k, nc) in name.chars().enumerate() {
        match at(chars, pos + k) {
            Some(c) if c.eq_ignore_ascii_case(&nc) => count += 1,
            _ => return false,
        }
    }
    match at(chars, pos + count) {
        Some(c) => c.is_whitespace() || c == '>' || c == '/',
        None => false,
    }
}

pub(super) fn starts_block_tag(chars: &[char], pos: usize) -> bool {
    if at(chars, pos) != Some('<') {
        return false;
    }
    ["pre", "source", "syntaxhighlight", "blockquote", "ul", "ol"]
        .iter()
        .any(|name| tag_name_matches(chars, pos + 1, name))
}

pub(super) struct ReferenceScan {
    position: usize,
    open: Vec<bool>,
}

impl ReferenceScan {
    pub(super) fn new(position: usize) -> Self {
        Self {
            position,
            open: Vec::new(),
        }
    }

    /// Return whether the innermost open note starts with a block body.
    pub(super) fn until(
        &mut self,
        chars: &[char],
        end: usize,
        bounds: ScanBounds<'_>,
    ) -> Option<bool> {
        while self.position < end {
            let position = self.position;
            if at(chars, position) == Some('<') {
                if let Some(after) = verbatim_region_end(chars, position, bounds) {
                    self.position = after;
                    continue;
                }
                if at(chars, position + 1) == Some('/') {
                    if tag_name_matches(chars, position + 2, "ref") {
                        self.open.pop();
                    }
                } else if tag_name_matches(chars, position + 1, "ref")
                    && let Some((_, _, self_closing, after)) =
                        open_tag_bounded(chars, position, bounds)
                {
                    if !self_closing {
                        let mut content = after;
                        while matches!(at(chars, content), Some(' ' | '\t')) {
                            content += 1;
                        }
                        self.open
                            .push(matches!(at(chars, content), None | Some('\n')));
                    }
                    self.position = after;
                    continue;
                }
            }
            self.position += 1;
        }
        self.open.last().copied()
    }
}

/// The role of a recognized HTML element, or `None` when the name is not a recognized HTML tag (in
/// which case the surrounding `<…>` stays literal text).
pub(super) fn html_tag_role(name: &str) -> Option<HtmlTagRole> {
    const INLINE: &[&str] = &[
        "abbr", "b", "bdi", "bdo", "big", "cite", "data", "dfn", "em", "font", "i", "ins", "q",
        "rb", "rt", "rtc", "ruby", "s", "small", "span", "strong", "u", "wbr",
    ];
    const BLOCK: &[&str] = &[
        "caption",
        "center",
        "col",
        "colgroup",
        "dd",
        "div",
        "dl",
        "dt",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "hr",
        "li",
        "ol",
        "references",
        "rp",
        "table",
        "td",
        "th",
        "time",
        "tr",
        "ul",
    ];
    const PARAGRAPH: &[&str] = &["gallery", "p"];
    if INLINE.contains(&name) {
        Some(HtmlTagRole::Inline)
    } else if BLOCK.contains(&name) {
        Some(HtmlTagRole::Block)
    } else if PARAGRAPH.contains(&name) {
        Some(HtmlTagRole::Break)
    } else {
        None
    }
}

/// Reads a closing tag `</name…>` at `i`, returning its lowercased name, raw text, and the index
/// just past `>`.
pub(super) fn close_tag_parse(
    chars: &[char],
    i: usize,
    bounds: ScanBounds<'_>,
) -> Option<(String, String, usize)> {
    if at(chars, i) != Some('<') || at(chars, i + 1) != Some('/') || !bounds.open_possible(i) {
        return None;
    }
    let mut cursor = i + 2;
    let mut name = String::new();
    while let Some(ch) = at(chars, cursor) {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
            cursor += 1;
        } else {
            break;
        }
    }
    if name.is_empty() {
        return None;
    }
    let gt = find_char(chars, cursor, '>')?;
    Some((name, collect_range(chars, i, gt + 1), gt + 1))
}

/// Finds where one `<li>` item's content ends, given the index just past its `<li>` open tag.
/// Returns the index where the content ends and the index to resume the enclosing list scan from.
/// The item ends at its own `</li>` (consumed), at a sibling `<li>` (left in place), or at the
/// enclosing list's `</ul>`/`</ol>` (left in place); nested `<ul>`/`<ol>` lists are stepped over so
/// their markers do not end the item.
pub(super) fn html_li_content_bounds(
    chars: &[char],
    start: usize,
    bounds: ScanBounds<'_>,
) -> (usize, usize) {
    let n = chars.len();
    let mut list_depth = 0i32;
    let mut j = start;
    while j < n {
        if at(chars, j) == Some('<') {
            if at(chars, j + 1) == Some('/') {
                if tag_name_matches(chars, j + 2, "ul") || tag_name_matches(chars, j + 2, "ol") {
                    if list_depth == 0 {
                        return (j, j);
                    }
                    list_depth -= 1;
                    if let Some((_, _, after)) = close_tag_parse(chars, j, bounds) {
                        j = after;
                        continue;
                    }
                } else if list_depth == 0
                    && tag_name_matches(chars, j + 2, "li")
                    && let Some((_, _, after)) = close_tag_parse(chars, j, bounds)
                {
                    return (j, after);
                }
            } else if tag_name_matches(chars, j + 1, "ul") || tag_name_matches(chars, j + 1, "ol") {
                if let Some((_, _, self_closing, after)) = open_tag_bounded(chars, j, bounds) {
                    if !self_closing {
                        if let Some((index, offset, end)) = bounds.index {
                            match index.close(after + offset, end, "") {
                                Some((_, after)) => {
                                    j = after - offset;
                                    continue;
                                }
                                None => return (n, n),
                            }
                        }
                        list_depth += 1;
                    }
                    j = after;
                    continue;
                }
            } else if list_depth == 0 && tag_name_matches(chars, j + 1, "li") {
                return (j, j);
            }
        }
        j += 1;
    }
    (n, n)
}

/// Reads a recognized block-level HTML tag (opening, closing, or self-closing) at `i`, returning the
/// token it contributes to the paragraph stream and the index just past it. Inline and unrecognized
/// tags yield `None`.
pub(super) fn block_tag_token(
    chars: &[char],
    i: usize,
    bounds: ScanBounds<'_>,
) -> Option<(Tok, usize)> {
    let (name, raw, after) = if at(chars, i + 1) == Some('/') {
        close_tag_parse(chars, i, bounds)?
    } else {
        let (name, raw, _self_closing, after) = open_tag_bounded(chars, i, bounds)?;
        (name, raw, after)
    };
    match html_tag_role(&name)? {
        HtmlTagRole::Block => Some((Tok::BlockRaw(raw), after)),
        HtmlTagRole::Break => Some((Tok::BlockBreak, after)),
        HtmlTagRole::Inline => None,
    }
}

/// Reads the value of `key` from a raw tag string, accepting quoted or bare values.
pub(super) fn tag_attribute(raw: &str, key: &str) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        match at(&chars, i) {
            Some(c) if c.is_ascii_alphabetic() => {
                let start = i;
                while let Some(c) = at(&chars, i) {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let name = collect_range(&chars, start, i).to_lowercase();
                while at(&chars, i).is_some_and(char::is_whitespace) {
                    i += 1;
                }
                if at(&chars, i) == Some('=') {
                    i += 1;
                    while at(&chars, i).is_some_and(char::is_whitespace) {
                        i += 1;
                    }
                    let value = if let Some(q @ ('"' | '\'')) = at(&chars, i) {
                        i += 1;
                        let vs = i;
                        while at(&chars, i).is_some_and(|c| c != q) {
                            i += 1;
                        }
                        let v = collect_range(&chars, vs, i);
                        i += 1;
                        v
                    } else {
                        let vs = i;
                        while at(&chars, i)
                            .is_some_and(|c| !c.is_whitespace() && c != '>' && c != '/')
                        {
                            i += 1;
                        }
                        collect_range(&chars, vs, i)
                    };
                    if name == key {
                        return Some(value);
                    }
                }
            }
            _ => i += 1,
        }
    }
    None
}
