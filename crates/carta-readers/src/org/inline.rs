//! Inline scanning: emphasis, verbatim, sub/superscripts, links, footnotes, math, and quotes.

use std::cell::Cell as ScanBudget;
use std::collections::BTreeMap;
use std::mem;

use carta_ast::{Attr, Block, Format, Inline, MathType, QuoteType};
use carta_core::{Extension, Extensions};

use super::citations::parse_citation_items;
use super::entities::entity;
use super::inline_helpers::{
    collect_str, image, is_image_target, is_uri, is_url_boundary, link, plain_words, post_ok,
    pre_ok, process_target, verbatim_code, wrap_markup,
};

pub(super) fn parse_inlines(
    text: &str,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut scanner = Inlines {
        chars: &chars,
        ext,
        notes,
        out: Vec::new(),
        word: String::new(),
        budget: ScanBudget::new(crate::inline_scan::scan_budget(chars.len())),
    };
    scanner.run();
    scanner.finish()
}

struct Inlines<'a> {
    chars: &'a [char],
    ext: Extensions,
    notes: &'a BTreeMap<String, Vec<Block>>,
    out: Vec<Inline>,
    word: String,
    // Unclosable openers make failed forward scans quadratic; a span-proportional step budget
    // keeps them linear, far above any real construct, so only pathological runs give up.
    budget: ScanBudget<usize>,
}

impl Inlines<'_> {
    fn finish(mut self) -> Vec<Inline> {
        self.flush();
        self.out
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
        self.chars.get(i).copied()
    }

    /// Charges one step against the shared forward-scan budget, returning `false` once it is spent so
    /// the caller abandons an over-long scan and leaves the opener as literal text.
    fn spend(&self) -> bool {
        let remaining = self.budget.get();
        if remaining == 0 {
            return false;
        }
        self.budget.set(remaining - 1);
        true
    }

    #[allow(clippy::too_many_lines)]
    fn run(&mut self) {
        let mut i = 0;
        while let Some(c) = self.at(i) {
            let prev = if i == 0 { None } else { self.at(i - 1) };

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
                        let inner = self.chars.get(i + 1..end).unwrap_or(&[]);
                        let content = parse_inlines(&collect_str(inner), self.ext, self.notes);
                        self.push_inline(wrap_markup(c, content));
                        i = end + 1;
                    } else {
                        self.word.push(c);
                        i += 1;
                    }
                }
                '_' => {
                    if let Some(end) = self.scan_emphasis(i, '_', prev) {
                        let inner = self.chars.get(i + 1..end).unwrap_or(&[]);
                        let content = parse_inlines(&collect_str(inner), self.ext, self.notes);
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

    // -- Emphasis ------------------------------------------------------------------------------

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
        let mut newlines = 0;
        let mut j = i + 1;
        while let Some(c) = self.at(j) {
            if !self.spend() {
                return None;
            }
            if c == '\n' {
                newlines += 1;
                if newlines > 1 {
                    return None;
                }
            }
            if c == marker
                && j > i + 1
                && !self.at(j - 1).is_some_and(char::is_whitespace)
                && post_ok(self.at(j + 1))
            {
                return Some(j);
            }
            j += 1;
        }
        None
    }

    // -- Sub/superscript -----------------------------------------------------------------------

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
            let close = self.match_brace(i + 1)?;
            let inner = self.chars.get(i + 2..close).unwrap_or(&[]);
            content = parse_inlines(&collect_str(inner), self.ext, self.notes);
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

    fn match_brace(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut j = open;
        while let Some(c) = self.at(j) {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                '\n' => return None,
                _ => {}
            }
            j += 1;
        }
        None
    }

    // -- Backslash: line break, math, entities -------------------------------------------------

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
            Some('(') => self.scan_tex_math(i + 2, "\\)", MathType::InlineMath, i),
            Some('[') => self.scan_tex_math(i + 2, "\\]", MathType::DisplayMath, i),
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
        close: &str,
        kind: MathType,
        fallback: usize,
    ) -> usize {
        let closing: Vec<char> = close.chars().collect();
        let mut j = start;
        while j < self.chars.len() {
            if !self.spend() {
                break;
            }
            if self.matches_at(j, &closing) {
                let inner: String = self.chars.get(start..j).unwrap_or(&[]).iter().collect();
                self.push_inline(Inline::Math(kind, inner.into()));
                return j + closing.len();
            }
            j += 1;
        }
        // Unterminated: emit the opening delimiter literally.
        self.word.push('\\');
        fallback + 1
    }

    fn scan_entity(&mut self, i: usize) -> usize {
        let mut j = i + 1;
        while matches!(self.at(j), Some(c) if c.is_ascii_alphabetic()) {
            j += 1;
        }
        let name: String = self.chars.get(i + 1..j).unwrap_or(&[]).iter().collect();
        // An optional `{}` terminates the entity and is consumed.
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

    // -- Brackets: links, footnotes, citations -------------------------------------------------

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
        // `[[` … `]]`, with an optional `][description]`.
        let inner_start = i + 2;
        let close = self.find_double_close(inner_start)?;
        let inner: String = self
            .chars
            .get(inner_start..close)
            .unwrap_or(&[])
            .iter()
            .collect();
        let (target_raw, desc_raw) = match inner.find("][") {
            Some(idx) => (
                inner.get(..idx).unwrap_or(""),
                Some(inner.get(idx + 2..).unwrap_or("")),
            ),
            None => (inner.as_str(), None),
        };
        let target = process_target(target_raw);
        let end = close + 2;
        match desc_raw {
            Some(desc) => Some((
                link(&target, parse_inlines(desc, self.ext, self.notes)),
                end,
            )),
            None => {
                if is_image_target(&target) {
                    Some((image(&target, Vec::new()), end))
                } else {
                    Some((link(&target, vec![Inline::Str(target_raw.into())]), end))
                }
            }
        }
    }

    /// Finds a `]]` starting at or after `from`.
    fn find_double_close(&self, from: usize) -> Option<usize> {
        let mut j = from;
        while j + 1 < self.chars.len() {
            if !self.spend() {
                return None;
            }
            if self.at(j) == Some(']') && self.at(j + 1) == Some(']') {
                return Some(j);
            }
            j += 1;
        }
        None
    }

    fn scan_footnote(&self, i: usize) -> Option<(Inline, usize)> {
        // `[fn:label]`, `[fn:label:text]`, or `[fn::text]`.
        let close = self.match_bracket(i)?;
        let inner: String = self.chars.get(i + 1..close).unwrap_or(&[]).iter().collect();
        let body = inner.strip_prefix("fn:")?;
        let end = close + 1;
        if let Some((label, text)) = body.split_once(':') {
            let note = vec![Block::Para(parse_inlines(
                text.trim(),
                self.ext,
                self.notes,
            ))];
            let _ = label;
            return Some((Inline::Note(note), end));
        }
        let blocks = self.notes.get(body).cloned().unwrap_or_default();
        Some((Inline::Note(blocks), end))
    }

    fn scan_citation(&self, i: usize) -> Option<(Inline, usize)> {
        let close = self.match_bracket(i)?;
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

    fn match_bracket(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut j = open;
        while let Some(c) = self.at(j) {
            if !self.spend() {
                return None;
            }
            match c {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
            j += 1;
        }
        None
    }

    // -- Angle brackets: targets and autolinks -------------------------------------------------

    fn scan_angle(&self, i: usize) -> Option<(Inline, usize)> {
        if self.at(i + 1) == Some('<') {
            // Target `<<name>>`.
            let name_start = i + 2;
            let mut j = name_start;
            while matches!(self.at(j), Some(c) if c != '<' && c != '>' && c != '\n') {
                j += 1;
            }
            if j > name_start && self.at(j) == Some('>') && self.at(j + 1) == Some('>') {
                let name: String = self
                    .chars
                    .get(name_start..j)
                    .unwrap_or(&[])
                    .iter()
                    .collect();
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
        // Autolink `<uri>`.
        let mut j = i + 1;
        while matches!(self.at(j), Some(c) if c != '>' && c != '\n') {
            j += 1;
        }
        if self.at(j) != Some('>') {
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

    // -- Dollar math ---------------------------------------------------------------------------

    fn scan_math_dollar(&self, i: usize, prev: Option<char>) -> Option<(Inline, usize)> {
        if self.at(i + 1) == Some('$') {
            // Display `$$…$$`.
            let start = i + 2;
            let mut j = start;
            while j + 1 < self.chars.len() {
                if !self.spend() {
                    return None;
                }
                if self.at(j) == Some('$') && self.at(j + 1) == Some('$') {
                    let inner: String = self.chars.get(start..j).unwrap_or(&[]).iter().collect();
                    return Some((Inline::Math(MathType::DisplayMath, inner.into()), j + 2));
                }
                j += 1;
            }
            return None;
        }
        // Inline `$…$` with word-boundary and border constraints.
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '$') {
            return None;
        }
        let first = self.at(i + 1)?;
        if first.is_whitespace() || first == '$' {
            return None;
        }
        let mut j = i + 1;
        while let Some(c) = self.at(j) {
            if c == '\n' {
                return None;
            }
            if c == '$'
                && !self.at(j - 1).is_some_and(char::is_whitespace)
                && !self.at(j + 1).is_some_and(char::is_alphanumeric)
            {
                let inner: String = self.chars.get(i + 1..j).unwrap_or(&[]).iter().collect();
                return Some((Inline::Math(MathType::InlineMath, inner.into()), j + 1));
            }
            j += 1;
        }
        None
    }

    // -- Inline export ---------------------------------------------------------------------------

    fn scan_export(&self, i: usize) -> Option<(Inline, usize)> {
        // `@@format:content@@`.
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
        let mut k = content_start;
        while k + 1 < self.chars.len() {
            if !self.spend() {
                return None;
            }
            if self.at(k) == Some('@') && self.at(k + 1) == Some('@') {
                let content: String = self
                    .chars
                    .get(content_start..k)
                    .unwrap_or(&[])
                    .iter()
                    .collect();
                return Some((Inline::RawInline(Format(fmt.into()), content.into()), k + 2));
            }
            k += 1;
        }
        None
    }

    // -- Smart quotes --------------------------------------------------------------------------

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
        let mut j = i + 1;
        while let Some(c) = self.at(j) {
            if c == close
                && !self.at(j - 1).is_some_and(char::is_whitespace)
                && post_ok(self.at(j + 1))
            {
                let inner = self.chars.get(i + 1..j).unwrap_or(&[]);
                let content = parse_inlines(&collect_str(inner), self.ext, self.notes);
                return (Some(Inline::Quoted(kind, content)), j + 1);
            }
            if c == '\n' {
                break;
            }
            j += 1;
        }
        (None, i + 1)
    }

    // -- Special strings -----------------------------------------------------------------------

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

    // -- Bare autolinks ------------------------------------------------------------------------

    fn scan_bare_url(&self, i: usize) -> Option<(String, usize)> {
        const SCHEMES: [&str; 3] = ["https://", "http://", "ftp://"];
        let scheme = SCHEMES.iter().find(|s| self.matches_str(i, s)).copied()?;
        let mut j = i + scheme.chars().count();
        while matches!(self.at(j), Some(c) if !c.is_whitespace() && !matches!(c, '<' | '>' | '(' | ')' | '[' | ']'))
        {
            j += 1;
        }
        // Trim trailing sentence punctuation.
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

    // -- Low-level matching --------------------------------------------------------------------

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
