//! Inline scanning: emphasis, verbatim, sub/superscripts, links, footnotes, math, and quotes.

use std::collections::BTreeMap;
use std::mem;

use carta_ast::{Attr, Block, Format, Inline, MathType, QuoteType};
use carta_core::{Extension, Extensions, with_stack};

use super::citations::parse_citation_items;
use super::entities::entity;
use super::inline_helpers::{
    image, is_image_target, is_internal_target, is_uri, is_url_boundary, link, plain_words,
    post_ok, pre_ok, process_target, verbatim_code, wrap_markup,
};

pub(super) fn parse_inlines(
    text: &str,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut closes: BTreeMap<(char, Option<char>), Vec<usize>> = BTreeMap::new();
    let mut groups = BTreeMap::new();
    let mut brackets = Vec::new();
    let mut braces = Vec::new();
    let mut newlines = Vec::new();
    for (index, &character) in chars.iter().enumerate() {
        let previous = index
            .checked_sub(1)
            .and_then(|index| chars.get(index))
            .copied();
        let next = chars.get(index + 1).copied();
        if closes_alone(character, previous, next) {
            closes.entry((character, None)).or_default().push(index);
        }
        if matches!(
            (character, next),
            (']', Some(']')) | ('$', Some('$')) | ('@', Some('@')) | ('\\', Some(')' | ']'))
        ) {
            closes.entry((character, next)).or_default().push(index);
        }
        match character {
            '[' => brackets.push(index),
            ']' => {
                if let Some(open) = brackets.pop() {
                    groups.insert(open, index);
                }
            }
            '{' => braces.push(index),
            '}' => {
                if let Some(open) = braces.pop() {
                    groups.insert(open, index);
                }
            }
            '\n' => {
                braces.clear();
                newlines.push(index);
            }
            _ => {}
        }
    }
    Inlines {
        chars: &chars,
        ext,
        notes,
        out: Vec::new(),
        word: String::new(),
        closes: &closes,
        groups: &groups,
        newlines: &newlines,
        start: 0,
        end: chars.len(),
    }
    .parse()
}

/// Whether `character` closes a span by itself between these neighbors.
fn closes_alone(character: char, previous: Option<char>, next: Option<char>) -> bool {
    matches!(character, '<' | '>')
        || previous.is_some_and(|character| !character.is_whitespace())
            && (matches!(character, '*' | '/' | '+' | '_' | '=' | '~' | '\'' | '"')
                && post_ok(next)
                || character == '$' && !next.is_some_and(char::is_alphanumeric))
}

/// A scanner over the window `start..end` of an inline run. Nested content is a narrower window
/// over the same characters and position tables, read as if nothing lay outside it.
struct Inlines<'a> {
    chars: &'a [char],
    ext: Extensions,
    notes: &'a BTreeMap<String, Vec<Block>>,
    out: Vec<Inline>,
    word: String,
    closes: &'a BTreeMap<(char, Option<char>), Vec<usize>>,
    groups: &'a BTreeMap<usize, usize>,
    newlines: &'a [usize],
    start: usize,
    end: usize,
}

impl Inlines<'_> {
    fn parse(mut self) -> Vec<Inline> {
        with_stack(|| {
            self.run();
            self.flush();
            self.out
        })
    }

    /// Parse the window `start..end` as inline content of its own.
    fn nested(&self, start: usize, end: usize) -> Vec<Inline> {
        Inlines {
            chars: self.chars,
            ext: self.ext,
            notes: self.notes,
            out: Vec::new(),
            word: String::new(),
            closes: self.closes,
            groups: self.groups,
            newlines: self.newlines,
            start,
            end,
        }
        .parse()
    }

    fn flush(&mut self) {
        if !self.word.is_empty() {
            self.out.push(Inline::Str(mem::take(&mut self.word).into()));
        }
    }

    fn push_inline(&mut self, inline: Inline) {
        self.flush();
        self.out.push(inline);
    }

    fn at(&self, i: usize) -> Option<char> {
        if i < self.end {
            self.chars.get(i).copied()
        } else {
            None
        }
    }

    fn before(&self, i: usize) -> Option<char> {
        if i > self.start { self.at(i - 1) } else { None }
    }

    /// The first closing delimiter at or after `from`, which lies past the window's first
    /// character. The tables record each character between its neighbors in the whole run, so the
    /// window's last character, which has no successor here, is judged apart.
    fn next_close(&self, from: usize, first: char, second: Option<char>) -> Option<usize> {
        let last = self.end.checked_sub(1)?;
        let indexed = self.closes.get(&(first, second)).and_then(|positions| {
            positions
                .get(positions.partition_point(|&position| position < from))
                .copied()
        });
        if let Some(position) = indexed.filter(|&position| position < last) {
            return Some(position);
        }
        (second.is_none()
            && from <= last
            && self.at(last) == Some(first)
            && closes_alone(first, self.before(last), None))
        .then_some(last)
    }

    /// The closing bracket or brace paired with the one at `open`, when the window holds it.
    fn group_end(&self, open: usize) -> Option<usize> {
        self.groups
            .get(&open)
            .copied()
            .filter(|&close| close < self.end)
    }

    fn within_lines(&self, from: usize, to: usize, count: usize) -> bool {
        let start = self.newlines.partition_point(|&position| position < from);
        self.newlines
            .get(start + count)
            .is_none_or(|&position| position >= to)
    }

    #[allow(clippy::too_many_lines)]
    fn run(&mut self) {
        let mut i = self.start;
        while let Some(c) = self.at(i) {
            let prev = self.before(i);

            if is_url_boundary(prev)
                && let Some((url, end)) = self.scan_bare_url(i)
            {
                self.push_inline(link(&url, vec![Inline::Str(url.clone().into())]));
                i = end;
                continue;
            }

            match c {
                ' ' | '\t' => {
                    self.flush();
                    while matches!(self.at(i), Some(' ' | '\t')) {
                        i += 1;
                    }
                    self.out.push(Inline::Space);
                }
                '\n' => {
                    self.flush();
                    self.out.push(Inline::SoftBreak);
                    i += 1;
                }
                '\\' => i = self.scan_backslash(i),
                '*' | '/' | '+' => {
                    if let Some(end) = self.scan_emphasis(i, c, prev) {
                        let content = self.nested(i + 1, end);
                        self.push_inline(wrap_markup(c, content));
                        i = end + 1;
                    } else {
                        self.word.push(c);
                        i += 1;
                    }
                }
                '_' => {
                    if let Some(end) = self.scan_emphasis(i, '_', prev) {
                        let content = self.nested(i + 1, end);
                        self.push_inline(Inline::Underline(content));
                        i = end + 1;
                    } else if let Some((inline, end)) = self.scan_subsup(i, prev, false) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('_');
                        i += 1;
                    }
                }
                '^' => {
                    if let Some((inline, end)) = self.scan_subsup(i, prev, true) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('^');
                        i += 1;
                    }
                }
                '=' | '~' => {
                    // Verbatim shares markup border rules but takes its body literally.
                    if let Some(end) = self.scan_emphasis(i, c, prev) {
                        let inner = self.chars.get(i + 1..end).unwrap_or(&[]);
                        self.push_inline(verbatim_code(c, inner));
                        i = end + 1;
                    } else {
                        self.word.push(c);
                        i += 1;
                    }
                }
                '[' => {
                    if let Some((inline, end)) = self.scan_bracket(i) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('[');
                        i += 1;
                    }
                }
                '<' => {
                    if let Some((inline, end)) = self.scan_angle(i) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('<');
                        i += 1;
                    }
                }
                '$' => {
                    if let Some((inline, end)) = self.scan_math_dollar(i, prev) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('$');
                        i += 1;
                    }
                }
                '@' => {
                    if let Some((inline, end)) = self.scan_export(i) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('@');
                        i += 1;
                    }
                }
                '-' | '.' => {
                    // The typographic dash and ellipsis replacements are always active.
                    if let Some((text, end)) = self.scan_special_string(i) {
                        self.word.push_str(text);
                        i = end;
                    } else {
                        self.word.push(c);
                        i += 1;
                    }
                }
                '\'' if self.ext.contains(Extension::Smart)
                    && prev.is_some_and(char::is_alphanumeric) =>
                {
                    // A word-internal or trailing apostrophe becomes a right single quotation mark.
                    self.word.push('\u{2019}');
                    i += 1;
                }
                '"' | '\'' if self.ext.contains(Extension::Smart) => {
                    let (inline, end) = self.scan_quote(i, c);
                    if let Some(q) = inline {
                        self.push_inline(q);
                        i = end;
                    } else {
                        self.word.push(c);
                        i += 1;
                    }
                }
                _ => {
                    self.word.push(c);
                    i += 1;
                }
            }
        }
    }

    // Emphasis

    /// Finds the closing marker for markup emphasis opened at `i`, honoring the pre/post border
    /// rules and the single-newline body limit.
    fn scan_emphasis(&self, i: usize, marker: char, prev: Option<char>) -> Option<usize> {
        if !pre_ok(prev) {
            return None;
        }
        let first = self.at(i + 1)?;
        if first.is_whitespace() {
            return None;
        }
        let close = self.next_close(i + 2, marker, None)?;
        self.within_lines(i + 1, close, 1).then_some(close)
    }

    // Sub/superscript

    /// Parses a subscript (`_`) or superscript (`^`) at `i`. Requires a preceding non-space base and
    /// accepts either a `{…}` group or a bare token ending in an alphanumeric.
    fn scan_subsup(&self, i: usize, prev: Option<char>, sup: bool) -> Option<(Inline, usize)> {
        // Base must be non-space and not `_`: `a__b` is a literal double underscore, not a subscript.
        if prev.is_none_or(|c| c.is_whitespace() || c == '_') {
            return None;
        }
        let content;
        let end;
        if self.at(i + 1) == Some('{') {
            let close = self.group_end(i + 1)?;
            content = self.nested(i + 2, close);
            end = close + 1;
        } else {
            let (text, stop) = self.scan_bare_script(i + 1)?;
            content = vec![Inline::Str(text.into())];
            end = stop;
        }
        let inline = if sup {
            Inline::Superscript(content)
        } else {
            Inline::Subscript(content)
        };
        Some((inline, end))
    }

    /// Scans a bare sub/superscript token: an optional sign then alphanumerics, dots, and commas,
    /// which must end in an alphanumeric.
    fn scan_bare_script(&self, start: usize) -> Option<(String, usize)> {
        let mut j = start;
        if matches!(self.at(j), Some('-' | '+')) {
            j += 1;
        }
        let body_start = j;
        while matches!(self.at(j), Some(c) if c.is_alphanumeric() || matches!(c, '.' | ',' | '\\'))
        {
            j += 1;
        }
        // Trim trailing non-alphanumerics; require at least one alphanumeric in the body.
        let mut last = j;
        while last > body_start && !self.at(last - 1).is_some_and(char::is_alphanumeric) {
            last -= 1;
        }
        if last <= body_start {
            return None;
        }
        let text: String = self.chars.get(start..last).unwrap_or(&[]).iter().collect();
        Some((text, last))
    }

    // Backslash: line break, math, entities

    fn scan_backslash(&mut self, i: usize) -> usize {
        match self.at(i + 1) {
            Some('\\') => {
                self.push_inline(Inline::LineBreak);
                let mut j = i + 2;
                while matches!(self.at(j), Some(' ' | '\t')) {
                    j += 1;
                }
                if self.at(j) == Some('\n') {
                    j += 1;
                }
                j
            }
            Some('(') => self.scan_tex_math(i + 2, ')', MathType::InlineMath, i),
            Some('[') => self.scan_tex_math(i + 2, ']', MathType::DisplayMath, i),
            Some(c) if c.is_ascii_alphabetic() => self.scan_entity(i),
            _ => {
                self.word.push('\\');
                i + 1
            }
        }
    }

    fn scan_tex_math(
        &mut self,
        start: usize,
        close: char,
        kind: MathType,
        fallback: usize,
    ) -> usize {
        if let Some(end) = self.next_close(start, '\\', Some(close)) {
            let inner: String = self.chars.get(start..end).unwrap_or(&[]).iter().collect();
            self.push_inline(Inline::Math(kind, inner.into()));
            return end + 2;
        }
        self.word.push('\\');
        fallback + 1
    }

    fn scan_entity(&mut self, i: usize) -> usize {
        let mut j = i + 1;
        while matches!(self.at(j), Some(c) if c.is_ascii_alphabetic()) {
            j += 1;
        }
        let name: String = self.chars.get(i + 1..j).unwrap_or(&[]).iter().collect();
        let mut end = j;
        if self.at(j) == Some('{') && self.at(j + 1) == Some('}') {
            end = j + 2;
        }
        if let Some(replacement) = entity(&name) {
            self.word.push_str(replacement);
        } else {
            self.push_inline(Inline::RawInline(
                Format("latex".into()),
                format!("\\{name}").into(),
            ));
        }
        end
    }

    // Brackets: links, footnotes, citations

    fn scan_bracket(&self, i: usize) -> Option<(Inline, usize)> {
        if self.at(i + 1) == Some('[') {
            return self.scan_link(i);
        }
        if self.matches_at(i + 1, &['f', 'n', ':']) {
            return self.scan_footnote(i);
        }
        if self.ext.contains(Extension::Citations)
            && (self.matches_at(i + 1, &['c', 'i', 't', 'e', ':'])
                || self.matches_at(i + 1, &['c', 'i', 't', 'e', '/']))
        {
            return self.scan_citation(i);
        }
        None
    }

    fn scan_link(&self, i: usize) -> Option<(Inline, usize)> {
        let inner_start = i + 2;
        let close = self.next_close(inner_start, ']', Some(']'))?;
        let inner = self.chars.get(inner_start..close).unwrap_or(&[]);
        let split = inner.windows(2).position(|pair| pair == [']', '[']);
        let description = split.map(|offset| inner_start + offset + 2);
        let target_raw: String = inner.iter().take(split.unwrap_or(inner.len())).collect();
        let target_raw = target_raw.as_str();
        if target_raw.is_empty() || description == Some(close) {
            return None;
        }
        let target = process_target(target_raw);
        let end = close + 2;
        let internal = is_internal_target(target_raw);
        if !internal && description.is_none() && is_image_target(&target) {
            return Some((image(&target, Vec::new()), end));
        }
        let target = if internal {
            format!("{}{target_raw}", super::links::INTERNAL_TARGET)
        } else {
            target
        };
        let label = match description {
            Some(start) => self.nested(start, close),
            None => vec![Inline::Str(target_raw.into())],
        };
        Some((link(&target, label), end))
    }

    fn scan_footnote(&self, i: usize) -> Option<(Inline, usize)> {
        let close = self.group_end(i)?;
        let body_start = i + "[fn:".len();
        let body = self.chars.get(body_start..close)?;
        let end = close + 1;
        if let Some(colon) = body.iter().position(|&character| character == ':') {
            let mut start = body_start + colon + 1;
            let mut stop = close;
            while start < stop && self.at(start).is_some_and(char::is_whitespace) {
                start += 1;
            }
            while stop > start && self.at(stop - 1).is_some_and(char::is_whitespace) {
                stop -= 1;
            }
            let note = vec![Block::Para(self.nested(start, stop))];
            return Some((Inline::Note(note), end));
        }
        let label: String = body.iter().collect();
        let blocks = self.notes.get(&label).cloned().unwrap_or_default();
        Some((Inline::Note(blocks), end))
    }

    fn scan_citation(&self, i: usize) -> Option<(Inline, usize)> {
        let close = self.group_end(i)?;
        let inner: String = self.chars.get(i + 1..close).unwrap_or(&[]).iter().collect();
        let raw: String = self.chars.get(i..close + 1).unwrap_or(&[]).iter().collect();
        let rest = inner.strip_prefix("cite")?;
        let (style, payload) = match rest.strip_prefix('/') {
            Some(after) => {
                let (sty, pay) = after.split_once(':')?;
                (Some(sty), pay)
            }
            None => (None, rest.strip_prefix(':')?),
        };
        let citations = parse_citation_items(payload, style, self.ext, self.notes)?;
        Some((Inline::Cite(citations, plain_words(&raw)), close + 1))
    }

    // Angle brackets: targets and autolinks

    fn scan_angle(&self, i: usize) -> Option<(Inline, usize)> {
        if self.at(i + 1) == Some('<') {
            let name_start = i + 2;
            let j = self.next_close(name_start, '>', None)?;
            if !self.within_lines(name_start, j, 0)
                || self
                    .next_close(name_start, '<', None)
                    .is_some_and(|open| open < j)
            {
                return None;
            }
            if j > name_start && self.at(j) == Some('>') && self.at(j + 1) == Some('>') {
                let name: String = self
                    .chars
                    .get(name_start..j)
                    .unwrap_or(&[])
                    .iter()
                    .collect();
                if name
                    .chars()
                    .any(|character| matches!(character, ' ' | '\t' | '\n' | '\'' | '"'))
                {
                    return None;
                }
                let attr = Attr {
                    id: name.into(),
                    ..Attr::default()
                };
                // A target absorbs the whitespace that follows it.
                let mut end = j + 2;
                while matches!(self.at(end), Some(' ' | '\t')) {
                    end += 1;
                }
                return Some((Inline::Span(Box::new(attr), Vec::new()), end));
            }
            return None;
        }
        let j = self.next_close(i + 1, '>', None)?;
        if !self.within_lines(i + 1, j, 0) {
            return None;
        }
        let content: String = self.chars.get(i + 1..j).unwrap_or(&[]).iter().collect();
        if is_uri(&content) {
            return Some((
                link(&content, vec![Inline::Str(content.clone().into())]),
                j + 1,
            ));
        }
        None
    }

    // Dollar math

    fn scan_math_dollar(&self, i: usize, prev: Option<char>) -> Option<(Inline, usize)> {
        if self.at(i + 1) == Some('$') {
            let start = i + 2;
            let end = self.next_close(start, '$', Some('$'))?;
            let inner: String = self.chars.get(start..end).unwrap_or(&[]).iter().collect();
            return Some((Inline::Math(MathType::DisplayMath, inner.into()), end + 2));
        }
        // Inline `$…$` with word-boundary and border constraints.
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '$') {
            return None;
        }
        let first = self.at(i + 1)?;
        if first.is_whitespace() || first == '$' {
            return None;
        }
        let end = self.next_close(i + 1, '$', None)?;
        if !self.within_lines(i + 1, end, 0) {
            return None;
        }
        let inner: String = self.chars.get(i + 1..end).unwrap_or(&[]).iter().collect();
        Some((Inline::Math(MathType::InlineMath, inner.into()), end + 1))
    }

    // Inline export

    fn scan_export(&self, i: usize) -> Option<(Inline, usize)> {
        if self.at(i + 1) != Some('@') {
            return None;
        }
        let fmt_start = i + 2;
        let mut j = fmt_start;
        while matches!(self.at(j), Some(c) if c.is_ascii_alphanumeric() || c == '-') {
            j += 1;
        }
        if self.at(j) != Some(':') || j == fmt_start {
            return None;
        }
        let fmt: String = self.chars.get(fmt_start..j).unwrap_or(&[]).iter().collect();
        let content_start = j + 1;
        let end = self.next_close(content_start, '@', Some('@'))?;
        let content: String = self
            .chars
            .get(content_start..end)
            .unwrap_or(&[])
            .iter()
            .collect();
        Some((
            Inline::RawInline(Format(fmt.into()), content.into()),
            end + 2,
        ))
    }

    // Smart quotes

    fn scan_quote(&self, i: usize, quote: char) -> (Option<Inline>, usize) {
        let (kind, close) = if quote == '"' {
            (QuoteType::DoubleQuote, '"')
        } else {
            (QuoteType::SingleQuote, '\'')
        };
        // The opening quote must be followed immediately by a non-space character.
        if !matches!(self.at(i + 1), Some(c) if !c.is_whitespace()) {
            return (None, i + 1);
        }
        if let Some(end) = self.next_close(i + 1, close, None)
            && self.within_lines(i + 1, end, 0)
        {
            let content = self.nested(i + 1, end);
            return (Some(Inline::Quoted(kind, content)), end + 1);
        }
        (None, i + 1)
    }

    // Special strings

    /// Replaces `---`/`--` with em/en dashes and `...` with an ellipsis.
    fn scan_special_string(&self, i: usize) -> Option<(&'static str, usize)> {
        if self.at(i) == Some('.') {
            if self.at(i + 1) == Some('.') && self.at(i + 2) == Some('.') {
                return Some(("\u{2026}", i + 3));
            }
            return None;
        }
        if self.at(i + 1) == Some('-') {
            if self.at(i + 2) == Some('-') {
                return Some(("\u{2014}", i + 3));
            }
            return Some(("\u{2013}", i + 2));
        }
        None
    }

    // Bare autolinks

    fn scan_bare_url(&self, i: usize) -> Option<(String, usize)> {
        const SCHEMES: [&str; 3] = ["https://", "http://", "ftp://"];
        let scheme = SCHEMES.iter().find(|s| self.matches_str(i, s)).copied()?;
        let mut j = i + scheme.chars().count();
        while matches!(self.at(j), Some(c) if !c.is_whitespace() && !matches!(c, '<' | '>' | '(' | ')' | '[' | ']'))
        {
            j += 1;
        }
        while j > i + scheme.chars().count()
            && self
                .at(j - 1)
                .is_some_and(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"'))
        {
            j -= 1;
        }
        let url: String = self.chars.get(i..j).unwrap_or(&[]).iter().collect();
        Some((url, j))
    }

    // Low-level matching

    fn matches_at(&self, i: usize, pat: &[char]) -> bool {
        pat.iter()
            .enumerate()
            .all(|(k, &c)| self.at(i + k) == Some(c))
    }

    fn matches_str(&self, i: usize, pat: &str) -> bool {
        pat.chars()
            .enumerate()
            .all(|(k, c)| self.at(i + k) == Some(c))
    }
}
