//! Block-level parsing: paragraphs, headers, preformatted text, and block tags.

use std::collections::BTreeMap;

use carta_ast::{Attr, Block, Inline, ListAttributes, ListNumberDelim, ListNumberStyle};
use carta_core::Extension;

use super::emphasis::{
    apply_smart_quotes, coalesce, drop_east_asian_breaks, resolve_emphasis, strip_outer_whitespace,
};
use super::links::para_or_figure;
use super::tags::{
    close_tag_bounded, close_tag_parse, enclosed, html_li_content_bounds, open_ref_block_bodied,
    open_ref_depth, open_tag_bounded, starts_block_tag, tag_attribute, tag_name_matches,
};
use super::{
    Parser, ScanBounds, Tok, at, balanced_braces, collect_range, format_html, format_mediawiki,
    is_blank, line_end, skip_construct, template_opens,
};

impl Parser<'_> {
    pub(super) fn parse_blocks(&mut self, chars: &[char]) -> Vec<Block> {
        carta_core::with_stack(|| self.parse_blocks_inner(chars))
    }

    fn parse_blocks_inner(&mut self, chars: &[char]) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        let mut pos = 0;
        let mut line_start = true;
        let n = chars.len();
        let bounds = self.bounds(chars);
        // Shared heading-region memo: each line's region resolves at most once; nested slices get their own memo.
        let mut scan = HeaderScan::default();
        while pos < n {
            if line_start {
                let le = self.line_end(chars, pos);
                if is_blank(chars, pos, le) {
                    pos = if le < n { le + 1 } else { le };
                    continue;
                }
                let c = at(chars, pos).unwrap_or(' ');
                if c == '{'
                    && at(chars, pos + 1) == Some('{')
                    && template_opens(chars, pos)
                    && let Some(after) = balanced_braces(chars, pos)
                {
                    let raw = collect_range(chars, pos, after);
                    blocks.push(Block::RawBlock(format_mediawiki(), raw.into()));
                    let (np, ls) = finish_inline_block(chars, after);
                    pos = np;
                    line_start = ls;
                    continue;
                }
                if c == '{' && at(chars, pos + 1) == Some('|') {
                    let (block, after) = self.parse_table(chars, pos);
                    blocks.push(block);
                    let (np, ls) = finish_inline_block(chars, after);
                    pos = np;
                    line_start = ls;
                    continue;
                }
                if c == '='
                    && let Some((level, inlines, closer_end)) =
                        self.try_header(chars, pos, &mut scan)
                {
                    let id = self.make_id(&inlines);
                    let attr = Attr {
                        id: id.into(),
                        classes: Vec::new(),
                        attributes: Vec::new(),
                    };
                    blocks.push(Block::Header(i64::from(level), Box::new(attr), inlines));
                    let (np, ls) = finish_inline_block(chars, closer_end);
                    pos = np;
                    line_start = ls;
                    continue;
                }
                if c == '-' && is_hr_line(chars, pos) {
                    blocks.push(Block::HorizontalRule);
                    let le2 = self.line_end(chars, pos);
                    pos = if le2 < n { le2 + 1 } else { le2 };
                    line_start = true;
                    continue;
                }
                if matches!(c, '*' | '#' | ':' | ';') && list_run_uniform(chars, pos) {
                    let (list_blocks, after) = self.parse_list(chars, pos);
                    blocks.extend(list_blocks);
                    pos = after;
                    line_start = true;
                    continue;
                }
                if c == ' ' {
                    let (block, after) = self.parse_preformatted(chars, pos);
                    blocks.push(block);
                    pos = after;
                    line_start = true;
                    continue;
                }
                if c == '<'
                    && let Some((block, after)) = self.parse_block_tag(chars, pos, bounds)
                {
                    blocks.push(block);
                    let (np, ls) = finish_inline_block(chars, after);
                    pos = np;
                    line_start = ls;
                    continue;
                }
            }
            let (mut para_blocks, after) = self.parse_paragraph(chars, pos, &mut scan, bounds);
            blocks.append(&mut para_blocks);
            pos = after;
            line_start = true;
        }
        blocks
    }

    fn try_header(
        &mut self,
        chars: &[char],
        pos: usize,
        scan: &mut HeaderScan,
    ) -> Option<(i32, Vec<Inline>, usize)> {
        let le = self.line_end(chars, pos);
        let mut m = 0;
        while pos + m < le && at(chars, pos + m) == Some('=') {
            m += 1;
        }
        if m == 0 || m > 6 {
            return None;
        }
        let content_start = pos + m;
        // Multiline headings end at a closing `=` run, blank, or block opener.
        let region_end = header_region_end_scan(chars, pos, scan);
        let closer = header_closer(chars, content_start, region_end, m)?;
        let content = collect_range(chars, content_start, closer);
        let inlines = self.parse_inlines(content.trim());
        Some((i32::try_from(m).unwrap_or(1), inlines, closer + m))
    }

    fn parse_preformatted(&mut self, chars: &[char], pos: usize) -> (Block, usize) {
        let n = chars.len();
        let mut p = pos;
        let mut lines: Vec<Vec<Inline>> = Vec::new();
        while at(chars, p) == Some(' ') {
            let le = self.line_end(chars, p);
            let content = collect_range(chars, p + 1, le);
            lines.push(self.preformatted_line(&content));
            if le >= n {
                p = le;
                break;
            }
            p = le + 1;
        }
        let mut out: Vec<Inline> = Vec::new();
        for (idx, mut inlines) in lines.into_iter().enumerate() {
            if idx > 0 {
                out.push(Inline::LineBreak);
            }
            out.append(&mut inlines);
        }
        (Block::Para(out), p)
    }

    pub(super) fn parse_block_tag(
        &mut self,
        chars: &[char],
        pos: usize,
        bounds: ScanBounds<'_>,
    ) -> Option<(Block, usize)> {
        let (name, raw_open, self_closing, after_open) = open_tag_bounded(chars, pos, bounds)?;
        match name.as_str() {
            "blockquote" => {
                if self_closing {
                    return Some((Block::BlockQuote(Vec::new()), after_open));
                }
                let (end, after) = close_tag_bounded(chars, after_open, "blockquote", bounds)
                    .unwrap_or((chars.len(), chars.len()));
                let inner = chars.get(after_open..end).unwrap_or(&[]);
                Some((Block::BlockQuote(self.parse_blocks(inner)), after))
            }
            "pre" => {
                let (inner, after) = enclosed(chars, after_open, "pre", bounds);
                Some((
                    Block::CodeBlock(Box::default(), trim_code(&inner).into()),
                    after,
                ))
            }
            "source" | "syntaxhighlight" => {
                let (inner, after) = enclosed(chars, after_open, &name, bounds);
                let mut classes = Vec::new();
                if let Some(lang) = tag_attribute(&raw_open, "lang")
                    && !lang.is_empty()
                {
                    classes.push(lang.into());
                }
                let attr = Attr {
                    id: carta_ast::Text::default(),
                    classes,
                    attributes: Vec::new(),
                };
                Some((
                    Block::CodeBlock(Box::new(attr), trim_code(&inner).into()),
                    after,
                ))
            }
            "ul" => Some(self.parse_html_list(
                chars,
                after_open,
                false,
                &raw_open,
                self_closing,
                bounds,
            )),
            "ol" => {
                Some(self.parse_html_list(chars, after_open, true, &raw_open, self_closing, bounds))
            }
            _ => None,
        }
    }

    /// Parses an HTML `<ul>`/`<ol>` list into a native list block. Each `<li>` becomes one item whose
    /// content is parsed as blocks, with a leading paragraph rendered as plain text; nested `<ul>`/
    /// `<ol>` lists nest. For an ordered list, a `start` attribute sets the first number while `type`
    /// and any per-item `value` are ignored. Whitespace between items is skipped; the first stray
    /// (non-`<li>`) content ends the list, leaving the remainder to be parsed as ordinary blocks.
    fn parse_html_list(
        &mut self,
        chars: &[char],
        start: usize,
        ordered: bool,
        raw_open: &str,
        self_closing: bool,
        bounds: ScanBounds<'_>,
    ) -> (Block, usize) {
        let mut items: Vec<Vec<Block>> = Vec::new();
        let mut i = start;
        let close_name = if ordered { "ol" } else { "ul" };
        if !self_closing {
            loop {
                while at(chars, i).is_some_and(char::is_whitespace) {
                    i += 1;
                }
                if at(chars, i) == Some('<')
                    && at(chars, i + 1) == Some('/')
                    && tag_name_matches(chars, i + 2, close_name)
                    && let Some((_, _, after)) = close_tag_parse(chars, i, bounds)
                {
                    i = after;
                    break;
                }
                if at(chars, i) == Some('<')
                    && at(chars, i + 1) != Some('/')
                    && tag_name_matches(chars, i + 1, "li")
                    && let Some((_, _, _self_closing, after_li)) =
                        open_tag_bounded(chars, i, bounds)
                {
                    let (content_end, next) = html_li_content_bounds(chars, after_li, bounds);
                    let content = chars.get(after_li..content_end).unwrap_or(&[]);
                    let mut blocks = self.parse_blocks(content);
                    plain_first_paragraph(&mut blocks);
                    items.push(blocks);
                    i = next;
                    continue;
                }
                break;
            }
        }
        let block = if ordered {
            let start_num = tag_attribute(raw_open, "start")
                .and_then(|value| value.trim().parse::<i32>().ok())
                .unwrap_or(1);
            Block::OrderedList(
                ListAttributes {
                    start: i64::from(start_num),
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::DefaultDelim,
                },
                items,
            )
        } else {
            Block::BulletList(items)
        };
        (block, i)
    }

    fn parse_paragraph(
        &mut self,
        chars: &[char],
        pos: usize,
        scan: &mut HeaderScan,
        bounds: ScanBounds<'_>,
    ) -> (Vec<Block>, usize) {
        let n = chars.len();
        let mut cur = pos;
        let content_end = loop {
            let le = self.line_end(chars, cur);
            if le >= n {
                cur = le;
                break le;
            }
            let next = le + 1;
            if next >= n {
                cur = next;
                break le;
            }
            // Keep unclosed note bodies across blank lines; block openers join only block notes.
            let ref_open = open_ref_depth(chars, pos, next, bounds) > 0;
            let next_end = self.line_end(chars, next);
            if is_blank(chars, next, next_end) {
                if ref_open {
                    cur = next;
                    continue;
                }
                cur = if next_end < n { next_end + 1 } else { next_end };
                break le;
            }
            if line_starts_block_scan(chars, next, scan) {
                if ref_open && open_ref_block_bodied(chars, pos, next, bounds) {
                    cur = next;
                    continue;
                }
                cur = next;
                break le;
            }
            cur = next;
        };
        let trimmed = trim_chars(chars.get(pos..content_end).unwrap_or(&[]));
        if trimmed.is_empty() {
            return (Vec::new(), cur);
        }
        (self.parse_block_content(trimmed), cur)
    }

    /// Parses a paragraph's text into blocks. Recognized block-level HTML tags split the run: the
    /// text on either side becomes its own paragraph and each tag becomes a raw block, so a `<div>`
    /// embedded in prose interrupts the paragraph exactly where it appears.
    fn parse_block_content(&mut self, chars: &[char]) -> Vec<Block> {
        let toks = self.lex(chars, false, true);
        let smart = self.smart();
        let east_asian = self.extensions.contains(Extension::EastAsianLineBreaks);
        let mut blocks: Vec<Block> = Vec::new();
        let mut segment: Vec<Tok> = Vec::new();
        for tok in toks {
            match tok {
                Tok::BlockRaw(raw) => {
                    flush_para_segment(&mut segment, &mut blocks, smart, east_asian);
                    blocks.push(Block::RawBlock(format_html(), raw.into()));
                }
                Tok::Block(block) => {
                    flush_para_segment(&mut segment, &mut blocks, smart, east_asian);
                    blocks.push(block);
                }
                Tok::BlockBreak => flush_para_segment(&mut segment, &mut blocks, smart, east_asian),
                other => segment.push(other),
            }
        }
        flush_para_segment(&mut segment, &mut blocks, smart, east_asian);
        blocks
    }

    /// Parses a table cell's content. On the cell's first line the list and heading markers
    /// `* # ; =` are inert and read as plain paragraph text; from the second line on every marker is
    /// recognized again. Definition (`:`), horizontal rules, templates, and nested tables stay
    /// active even on the first line.
    pub(super) fn parse_cell_blocks(&mut self, chars: &[char]) -> Vec<Block> {
        let first = at(chars, 0);
        let suppressed = matches!(first, Some('*' | '#' | ';'))
            || (first == Some('=') && is_header_line_within(chars, 0));
        if !suppressed {
            return self.parse_blocks(chars);
        }
        let bounds = self.bounds(chars);
        let (mut blocks, after) =
            self.parse_paragraph(chars, 0, &mut HeaderScan::default(), bounds);
        if let Some(rest) = chars.get(after..) {
            blocks.extend(self.parse_blocks(rest));
        }
        blocks
    }

    /// Parses the content of a `<ref>` note as blocks; a lone paragraph becomes a [`Block::Plain`].
    pub(super) fn note_blocks(&mut self, chars: &[char]) -> Vec<Block> {
        let mut blocks = self.parse_blocks(chars);
        if blocks.len() == 1 {
            plain_first_paragraph(&mut blocks);
        }
        blocks
    }
}

/// Whether the list line at `pos` may begin a list: its marker run must be a single repeated marker
/// character. A run that mixes marker characters (`*#`, `:;`, …) has no parent item to anchor its
/// deeper level, so it is not a list.
fn list_run_uniform(chars: &[char], pos: usize) -> bool {
    let first = at(chars, pos);
    let le = line_end(chars, pos);
    let mut p = pos;
    while p < le && at(chars, p).is_some_and(is_list_marker) {
        if at(chars, p) != first {
            return false;
        }
        p += 1;
    }
    true
}

/// Resolves a buffered run of inline tokens into a paragraph (or figure) block, dropping it when it
/// holds only whitespace. Used between block-level tags while splitting a paragraph.
fn flush_para_segment(
    segment: &mut Vec<Tok>,
    blocks: &mut Vec<Block>,
    smart: bool,
    east_asian: bool,
) {
    if segment.is_empty() {
        return;
    }
    let toks = std::mem::take(segment);
    let mut inlines = coalesce(strip_outer_whitespace(resolve_emphasis(toks)));
    if east_asian {
        inlines = drop_east_asian_breaks(inlines);
    }
    if smart {
        inlines = apply_smart_quotes(inlines);
    }
    if !inlines.is_empty() {
        blocks.push(para_or_figure(inlines));
    }
}

/// Memo tables for the heading-region lookahead, keyed by the starting char index within one
/// `chars` slice. A heading's text runs until the next line that opens a block, and deciding
/// whether a `=`-prefixed line opens its own heading needs that same lookahead, so region end and
/// header-ness are mutually recursive. Every recursive step advances to a strictly later line, so
/// the recursion always terminates on its own, but without memoization each line's region would be
/// recomputed once per enclosing region, which is exponential in the number of consecutive
/// `=`-prefixed lines. Caching each result by position collapses that to linear work per line.
#[derive(Default)]
struct HeaderScan {
    region_end: BTreeMap<usize, usize>,
    is_header: BTreeMap<usize, bool>,
}

fn line_starts_block_scan(chars: &[char], ls: usize, scan: &mut HeaderScan) -> bool {
    match at(chars, ls) {
        Some('*' | '#' | ':' | ';' | ' ') => true,
        Some('=') => is_header_line(chars, ls, scan),
        Some('-') => is_hr_line(chars, ls),
        Some('{') => matches!(at(chars, ls + 1), Some('{' | '|')),
        Some('<') => starts_block_tag(chars, ls),
        _ => false,
    }
}

fn is_header_line_within(chars: &[char], pos: usize) -> bool {
    is_header_line(chars, pos, &mut HeaderScan::default())
}

fn is_header_line(chars: &[char], pos: usize, scan: &mut HeaderScan) -> bool {
    if let Some(&cached) = scan.is_header.get(&pos) {
        return cached;
    }
    let le = line_end(chars, pos);
    let mut m = 0;
    while pos + m < le && at(chars, pos + m) == Some('=') {
        m += 1;
    }
    let result = if m == 0 || m > 6 {
        false
    } else {
        let region_end = header_region_end_scan(chars, pos, scan);
        header_closer(chars, pos + m, region_end, m).is_some()
    };
    scan.is_header.insert(pos, result);
    result
}

/// The end index of the span a heading's text may cover: the heading continues across lines like a
/// paragraph until a blank line or a line that opens its own block, and the result is the line end
/// of the last line still part of that span.
fn header_region_end_scan(chars: &[char], pos: usize, scan: &mut HeaderScan) -> usize {
    if let Some(&cached) = scan.region_end.get(&pos) {
        return cached;
    }
    let n = chars.len();
    // Fill the region memo backwards to avoid repeated scans and recursive stack growth.
    let mut starts = Vec::new();
    let mut cur = pos;
    loop {
        starts.push(cur);
        let le = line_end(chars, cur);
        if le >= n {
            break;
        }
        let next = le + 1;
        if next >= n {
            break;
        }
        let next_end = line_end(chars, next);
        if is_blank(chars, next, next_end) {
            break;
        }
        cur = next;
    }
    for &s in starts.iter().rev() {
        if scan.region_end.contains_key(&s) {
            continue;
        }
        let le = line_end(chars, s);
        let region = if le >= n {
            le
        } else {
            let next = le + 1;
            if next >= n {
                le
            } else {
                let next_end = line_end(chars, next);
                if is_blank(chars, next, next_end) || line_starts_block_scan(chars, next, scan) {
                    le
                } else {
                    header_region_end_scan(chars, next, scan)
                }
            }
        };
        scan.region_end.insert(s, region);
    }
    scan.region_end
        .get(&pos)
        .copied()
        .unwrap_or_else(|| line_end(chars, pos))
}

/// The index of the first bare `=` run after the heading text, when that run is at least `m` long;
/// otherwise no valid closer. Constructs (templates, links, tags) are skipped so an `=` inside them
/// is not mistaken for the closer.
fn header_closer(chars: &[char], content_start: usize, line_end: usize, m: usize) -> Option<usize> {
    let mut i = content_start;
    while i < line_end {
        if let Some(next) = skip_construct(chars, i)
            && next > i
        {
            i = next.min(line_end);
            continue;
        }
        if at(chars, i) == Some('=') {
            let mut j = i;
            while j < line_end && at(chars, j) == Some('=') {
                j += 1;
            }
            return if j - i >= m { Some(i) } else { None };
        }
        i += 1;
    }
    None
}

fn is_hr_line(chars: &[char], pos: usize) -> bool {
    let le = line_end(chars, pos);
    let mut k = pos;
    while k < le && at(chars, k) == Some('-') {
        k += 1;
    }
    k - pos >= 4 && is_blank(chars, k, le)
}

pub(super) fn is_list_marker(c: char) -> bool {
    matches!(c, '*' | '#' | ':' | ';')
}

fn finish_inline_block(chars: &[char], pos: usize) -> (usize, bool) {
    let le = line_end(chars, pos);
    if is_blank(chars, pos, le) {
        let next = if le < chars.len() { le + 1 } else { le };
        (next, true)
    } else {
        (pos, false)
    }
}

fn trim_code(inner: &str) -> String {
    let stripped = inner
        .strip_prefix("\r\n")
        .or_else(|| inner.strip_prefix('\n'))
        .unwrap_or(inner);
    stripped
        .strip_suffix("\r\n")
        .or_else(|| stripped.strip_suffix('\n'))
        .unwrap_or(stripped)
        .to_string()
}

fn plain_first_paragraph(blocks: &mut [Block]) {
    if let Some(first) = blocks.first_mut()
        && let Block::Para(inlines) = first
    {
        *first = Block::Plain(std::mem::take(inlines));
    }
}

pub(super) fn trim_chars(chars: &[char]) -> &[char] {
    let start = chars
        .iter()
        .position(|ch| !ch.is_whitespace())
        .unwrap_or(chars.len());
    let end = chars
        .iter()
        .rposition(|ch| !ch.is_whitespace())
        .map_or(start, |position| position + 1);
    chars.get(start..end).unwrap_or(&[])
}
