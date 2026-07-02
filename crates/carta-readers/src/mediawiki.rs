//! Reader for `MediaWiki`'s wikitext markup.
//!
//! The source is first cleared of comments (`<!-- … -->`), then parsed line by line into blocks.
//! A line opening with `=` runs is a heading, `*`/`#`/`:`/`;` runs start lists, four or more `-`
//! alone are a horizontal rule, a leading space marks preformatted text, `{{…}}` and `{|…|}` are
//! template and table markup, and `<pre>`/`<blockquote>`/`<syntaxhighlight>` are recognized block
//! tags; everything else is a paragraph. Inline markup — apostrophe emphasis, `[[internal]]` and
//! `[external]` links, bare URLs, entity references, and a fixed set of HTML tags — is scanned
//! within each block's text.
//!
//! Heading identifiers follow the enabled identifier scheme: with `gfm_auto_identifiers` the GitHub
//! algorithm (hyphen separators), otherwise `auto_identifiers` lowercases the text, keeps
//! alphanumerics together with `_` and `.`, turns spaces and `-` into single `_`, and drops a
//! leading run of non-letters; duplicates gain a numeric suffix and an empty result becomes
//! `section`. With neither enabled, headings carry no identifier.
//!
//! The scanner is panic-free on malformed input: unbalanced or unterminated constructs degrade to
//! literal text rather than being rejected.

use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{
    Alignment, ApiVersion, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Format, Inline,
    ListAttributes, ListNumberDelim, ListNumberStyle, MathType, MetaValue, QuoteType, Row, Table,
    TableBody, TableFoot, TableHead, Target, ToCompactString, slug_gfm, to_plain_text,
};
use carta_core::{Extension, Extensions, Reader, ReaderOptions, Result};

use crate::emoji;
use crate::entities;

/// Parses a wikitext document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct MediawikiReader;

impl Reader for MediawikiReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let stripped = strip_comments(&expand_tabs(input));
        let (source, behavior_switches) = extract_behavior_switches(&stripped);
        let chars: Vec<char> = source.chars().collect();
        let mut parser = Parser::new(options);
        let mut blocks = parser.parse_blocks(&chars);
        // Category memberships are pulled out of the inline flow as they are encountered and gathered
        // into a single trailing paragraph, one link per category in document order.
        if !parser.categories.is_empty() {
            let mut inlines: Vec<Inline> = Vec::new();
            for (index, category) in parser.categories.drain(..).enumerate() {
                if index > 0 {
                    inlines.push(Inline::Space);
                }
                inlines.push(category);
            }
            blocks.push(Block::Para(inlines));
        }
        let mut meta: BTreeMap<String, MetaValue> = BTreeMap::new();
        for switch in behavior_switches {
            meta.insert(switch, MetaValue::MetaBool(true));
        }
        Ok(Document {
            api_version: ApiVersion::default(),
            meta: meta.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            blocks,
        })
    }
}

/// Carries the state that spans a whole document: the enabled extensions, the running counter for
/// unlabeled external links, and the set of heading identifiers already issued (for de-duplication).
struct Parser {
    extensions: Extensions,
    link_counter: usize,
    seen_ids: BTreeSet<String>,
    /// Category links pulled out of the inline flow, to be emitted as one trailing paragraph.
    categories: Vec<Inline>,
    /// Current block-nesting depth, capped to keep adversarially deep input from exhausting the stack.
    depth: usize,
}

/// Block-nesting depth past which parsing stops descending: deeply stacked blockquotes, list levels,
/// notes, and table cells degrade to flat content rather than recursing without bound. The cap sits
/// far below the point where either parsing or serialization would overflow the stack.
const MAX_BLOCK_DEPTH: usize = 64;

/// One line of list markup: its leading marker run and the trimmed text that follows.
struct ListItem {
    markers: Vec<char>,
    content: String,
}

/// The list family a marker character opens.
#[derive(PartialEq, Eq, Clone, Copy)]
enum ListKind {
    Bullet,
    Ordered,
    Definition,
}

/// A table cell collected during the line scan, before its text is parsed into blocks. A `!`-marked
/// cell is a header cell; the spans and attributes come from the cell's leading attribute list.
struct RawCell {
    is_header: bool,
    align: Alignment,
    col_span: i32,
    row_span: i32,
    attr: Attr,
    content: String,
}

/// The alignment, spans, and attributes parsed from a cell's leading attribute list.
struct CellAttrs {
    align: Alignment,
    col_span: i32,
    row_span: i32,
    attr: Attr,
}

/// Which open construct a table continuation line extends.
#[derive(Clone, Copy)]
enum OpenTarget {
    None,
    Caption,
    Cell,
}

/// A lexical unit of inline text: a finished inline node, a run of apostrophes whose emphasis role is
/// resolved once the surrounding run structure is known, a block-level HTML tag that interrupts the
/// paragraph, or a paragraph break carried by a block-level tag that leaves no output.
enum Tok {
    Inline(Inline),
    Apostrophes(usize),
    BlockRaw(String),
    BlockBreak,
    /// A verbatim block element (`<pre>`, `<blockquote>`, `<syntaxhighlight>`) found mid-paragraph:
    /// it interrupts the paragraph and emerges as its own block.
    Block(Block),
}

/// The role a recognized HTML tag plays in the inline stream.
enum HtmlTagRole {
    /// An inline element: its opening and closing tags pass through as raw inline HTML.
    Inline,
    /// A block element: its tags interrupt the paragraph and pass through as raw block HTML.
    Block,
    /// A paragraph-only element (`p`, `gallery`): its tags interrupt the paragraph but leave no output.
    Break,
}

impl Parser {
    fn new(options: &ReaderOptions) -> Self {
        Self {
            extensions: options.extensions,
            link_counter: 0,
            seen_ids: BTreeSet::new(),
            categories: Vec::new(),
            depth: 0,
        }
    }

    /// Whether straight double quotes should fold into typographic quote runs.
    fn smart(&self) -> bool {
        self.extensions.contains(Extension::Smart)
    }

    fn parse_blocks(&mut self, chars: &[char]) -> Vec<Block> {
        self.depth += 1;
        if self.depth > MAX_BLOCK_DEPTH {
            self.depth -= 1;
            return degraded_blocks(chars);
        }
        let blocks = self.parse_blocks_inner(chars);
        self.depth -= 1;
        blocks
    }

    fn parse_blocks_inner(&mut self, chars: &[char]) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        let mut pos = 0;
        let mut line_start = true;
        let n = chars.len();
        // Heading-region lookahead memo, shared across every line-classification query over this
        // slice so each line's region is resolved at most once. `chars` is fixed for the whole
        // call, so positions stay valid throughout; nested slices (cells, blockquotes) get their
        // own memo via their own `parse_blocks_inner`.
        let mut scan = HeaderScan::default();
        while pos < n {
            if line_start {
                let le = line_end(chars, pos);
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
                    blocks.push(Block::Header(level, Box::new(attr), inlines));
                    let (np, ls) = finish_inline_block(chars, closer_end);
                    pos = np;
                    line_start = ls;
                    continue;
                }
                if c == '-' && is_hr_line(chars, pos) {
                    blocks.push(Block::HorizontalRule);
                    let le2 = line_end(chars, pos);
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
                    && let Some((block, after)) = self.parse_block_tag(chars, pos)
                {
                    blocks.push(block);
                    let (np, ls) = finish_inline_block(chars, after);
                    pos = np;
                    line_start = ls;
                    continue;
                }
            }
            let (mut para_blocks, after) = self.parse_paragraph(chars, pos, &mut scan);
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
        let le = line_end(chars, pos);
        let mut m = 0;
        while pos + m < le && at(chars, pos + m) == Some('=') {
            m += 1;
        }
        if m == 0 || m > 6 {
            return None;
        }
        let content_start = pos + m;
        // The closing run may sit several lines below: the heading text continues like a paragraph
        // until a blank line or a line that opens its own block, and the trailing `=` run anywhere in
        // that span closes it.
        let region_end = header_region_end_scan(chars, pos, scan);
        let closer = header_closer(chars, content_start, region_end, m)?;
        let content = collect_range(chars, content_start, closer);
        let inlines = self.parse_inlines(content.trim());
        Some((i32::try_from(m).unwrap_or(1), inlines, closer + m))
    }

    fn parse_list(&mut self, chars: &[char], pos: usize) -> (Vec<Block>, usize) {
        let mut items: Vec<ListItem> = Vec::new();
        let mut cursor = pos;
        let n = chars.len();
        while at(chars, cursor).is_some_and(is_list_marker) {
            let le = line_end(chars, cursor);
            let mut scan = cursor;
            let mut markers: Vec<char> = Vec::new();
            while scan < le && at(chars, scan).is_some_and(is_list_marker) {
                if let Some(marker) = at(chars, scan) {
                    markers.push(marker);
                }
                scan += 1;
            }
            let content = collect_range(chars, scan, le).trim().to_string();
            items.push(ListItem { markers, content });
            if le >= n {
                cursor = le;
                break;
            }
            cursor = le + 1;
        }
        (self.build_lists(&items, 0), cursor)
    }

    fn build_lists(&mut self, items: &[ListItem], level: usize) -> Vec<Block> {
        if level >= MAX_BLOCK_DEPTH {
            // Past the nesting cap, each item's text becomes a flat plain block with no deeper list
            // structure, so adversarially deep marker runs cannot exhaust the stack.
            let mut out: Vec<Block> = Vec::new();
            for item in items {
                let inlines = self.parse_inlines(&item.content);
                if !inlines.is_empty() {
                    out.push(Block::Plain(inlines));
                }
            }
            return out;
        }
        let mut out: Vec<Block> = Vec::new();
        let mut i = 0;
        while i < items.len() {
            let kind = if let Some(&m) = items.get(i).and_then(|it| it.markers.get(level)) {
                list_kind(m)
            } else {
                i += 1;
                continue;
            };
            let mut j = i;
            while j < items.len() {
                match items.get(j).and_then(|it| it.markers.get(level)) {
                    Some(&m) if list_kind(m) == kind => j += 1,
                    _ => break,
                }
            }
            let group = items.get(i..j).unwrap_or(&[]);
            match kind {
                ListKind::Bullet => out.push(Block::BulletList(self.build_simple(group, level))),
                ListKind::Ordered => {
                    out.push(Block::OrderedList(
                        default_list_attrs(),
                        self.build_simple(group, level),
                    ));
                }
                ListKind::Definition => out.push(self.build_definition(group, level)),
            }
            i = j;
        }
        out
    }

    fn build_simple(&mut self, group: &[ListItem], level: usize) -> Vec<Vec<Block>> {
        let mut entries: Vec<Vec<Block>> = Vec::new();
        let mut i = 0;
        while i < group.len() {
            let depth = group.get(i).map_or(0, |it| it.markers.len());
            if depth == level + 1 {
                let content = group.get(i).map_or("", |it| it.content.as_str());
                let mut blocks = vec![plain_or_figure(self.parse_inlines(content))];
                i += 1;
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                if let Some(sub) = group.get(start..i)
                    && !sub.is_empty()
                {
                    blocks.extend(self.build_lists(sub, level + 1));
                }
                entries.push(blocks);
            } else {
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                if i == start {
                    i += 1;
                }
                let blocks = group
                    .get(start..i)
                    .map(|sub| self.build_lists(sub, level + 1))
                    .unwrap_or_default();
                entries.push(blocks);
            }
        }
        entries
    }

    fn build_definition(&mut self, group: &[ListItem], level: usize) -> Block {
        let mut pairs: Vec<(Vec<Inline>, Vec<Vec<Block>>)> = Vec::new();
        let mut i = 0;
        while i < group.len() {
            let Some(item) = group.get(i) else { break };
            if item.markers.len() == level + 1 {
                let marker = item.markers.get(level).copied().unwrap_or(':');
                let content = item.content.clone();
                i += 1;
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                let nested = group
                    .get(start..i)
                    .map(|sub| self.build_lists(sub, level + 1))
                    .unwrap_or_default();
                if marker == ';' {
                    let (term_str, def_str) = split_term(&content);
                    let term = self.parse_inlines(&term_str);
                    let mut defs: Vec<Vec<Block>> = Vec::new();
                    if let Some(d) = def_str {
                        defs.push(vec![plain_or_figure(self.parse_inlines(&d))]);
                    }
                    if !nested.is_empty() {
                        match defs.last_mut() {
                            Some(last) => last.extend(nested),
                            None => defs.push(nested),
                        }
                    }
                    // Terms stacked with no definition between them share one entry, separated by a
                    // line break, until a definition arrives.
                    match pairs.last_mut() {
                        Some((last_term, last_defs)) if last_defs.is_empty() => {
                            last_term.push(Inline::LineBreak);
                            last_term.extend(term);
                            *last_defs = defs;
                        }
                        _ => pairs.push((term, defs)),
                    }
                } else {
                    let mut blocks = vec![plain_or_figure(self.parse_inlines(&content))];
                    blocks.extend(nested);
                    match pairs.last_mut() {
                        Some(last) => last.1.push(blocks),
                        None => pairs.push((Vec::new(), vec![blocks])),
                    }
                }
            } else {
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                if i == start {
                    i += 1;
                }
                let nested = group
                    .get(start..i)
                    .map(|sub| self.build_lists(sub, level + 1))
                    .unwrap_or_default();
                match pairs.last_mut() {
                    Some(last) => match last.1.last_mut() {
                        Some(d) => d.extend(nested),
                        None => last.1.push(nested),
                    },
                    None => pairs.push((Vec::new(), vec![nested])),
                }
            }
        }
        Block::DefinitionList(pairs)
    }

    fn parse_preformatted(&mut self, chars: &[char], pos: usize) -> (Block, usize) {
        let n = chars.len();
        let mut p = pos;
        let mut lines: Vec<Vec<Inline>> = Vec::new();
        while at(chars, p) == Some(' ') {
            let le = line_end(chars, p);
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

    fn parse_block_tag(&mut self, chars: &[char], pos: usize) -> Option<(Block, usize)> {
        let (name, raw_open, self_closing, after_open) = open_tag(chars, pos)?;
        match name.as_str() {
            "blockquote" => {
                if self_closing {
                    return Some((Block::BlockQuote(Vec::new()), after_open));
                }
                let (inner, after) = enclosed(chars, after_open, "blockquote");
                let inner_chars: Vec<char> = inner.chars().collect();
                Some((Block::BlockQuote(self.parse_blocks(&inner_chars)), after))
            }
            "pre" => {
                let (inner, after) = enclosed(chars, after_open, "pre");
                Some((
                    Block::CodeBlock(Box::default(), trim_code(&inner).into()),
                    after,
                ))
            }
            "source" | "syntaxhighlight" => {
                let (inner, after) = enclosed(chars, after_open, &name);
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
            "ul" => Some(self.parse_html_list(chars, after_open, false, &raw_open, self_closing)),
            "ol" => Some(self.parse_html_list(chars, after_open, true, &raw_open, self_closing)),
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
                    && let Some((_, _, after)) = close_tag_parse(chars, i)
                {
                    i = after;
                    break;
                }
                if at(chars, i) == Some('<')
                    && at(chars, i + 1) != Some('/')
                    && tag_name_matches(chars, i + 1, "li")
                    && let Some((_, _, _self_closing, after_li)) = open_tag(chars, i)
                {
                    let (content_end, next) = html_li_content_bounds(chars, after_li);
                    let content: Vec<char> = collect_range(chars, after_li, content_end)
                        .chars()
                        .collect();
                    let mut blocks = self.parse_blocks(&content);
                    if let Some(Block::Para(inlines)) = blocks.first() {
                        let inlines = inlines.clone();
                        if let Some(first) = blocks.first_mut() {
                            *first = Block::Plain(inlines);
                        }
                    }
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
                    start: start_num,
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
    ) -> (Vec<Block>, usize) {
        let n = chars.len();
        let mut pieces: Vec<String> = Vec::new();
        let mut cur = pos;
        loop {
            let le = line_end(chars, cur);
            pieces.push(collect_range(chars, cur, le));
            if le >= n {
                cur = le;
                break;
            }
            let next = le + 1;
            if next >= n {
                cur = next;
                break;
            }
            // A `<ref>` whose `</ref>` has not yet been seen keeps the paragraph open across a blank
            // line so the note's body — including any internal paragraph breaks — is captured whole.
            // A line that would otherwise begin a block only stays attached when the open note reads
            // as block content (its body began on a fresh line); a note opened with text on the same
            // line reads inline and ends at such a line instead.
            let ref_open = open_ref_depth(chars, pos, next) > 0;
            let next_end = line_end(chars, next);
            if is_blank(chars, next, next_end) {
                if ref_open {
                    cur = next;
                    continue;
                }
                cur = if next_end < n { next_end + 1 } else { next_end };
                break;
            }
            if line_starts_block_scan(chars, next, scan) {
                if ref_open && open_ref_block_bodied(chars, pos, next) {
                    cur = next;
                    continue;
                }
                cur = next;
                break;
            }
            cur = next;
        }
        let raw = pieces.join("\n");
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return (Vec::new(), cur);
        }
        (self.parse_block_content(trimmed), cur)
    }

    /// Parses a paragraph's text into blocks. Recognized block-level HTML tags split the run: the
    /// text on either side becomes its own paragraph and each tag becomes a raw block, so a `<div>`
    /// embedded in prose interrupts the paragraph exactly where it appears.
    fn parse_block_content(&mut self, text: &str) -> Vec<Block> {
        let chars: Vec<char> = text.chars().collect();
        let toks = self.lex(&chars, false, true);
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

    /// Parses a `{|`-delimited table into a [`Block::Table`], returning the index past the closing
    /// `|}`. Table and row attribute lists are dropped; a cell's attribute list supplies its
    /// alignment, spans, identifier, and classes. The first row becomes the header when its first
    /// cell is a `!` header cell.
    fn parse_table(&mut self, chars: &[char], pos: usize) -> (Block, usize) {
        let after = table_block_end(chars, pos);
        let region = collect_range(chars, pos, after);
        (self.build_table(&region), after)
    }

    fn build_table(&mut self, region: &str) -> Block {
        let (mut rows, caption_text) = scan_table_region(region);
        // The first row may omit its leading `|-` separator, so a `|-` seen before any cell merely
        // opens the first row rather than closing an empty one: an empty leading segment is dropped.
        // Every later `|-` closes a row, so empty rows elsewhere are kept.
        if rows.first().is_some_and(Vec::is_empty) {
            rows.remove(0);
        }
        if rows.is_empty() {
            // A table with no cells still yields one empty row.
            rows.push(Vec::new());
        }

        let n_rows = rows.len();
        // The first row fixes the column count; cells that overflow it in later rows are dropped.
        let ncols = rows.first().map_or(0, |r| {
            r.iter().map(|c| col_count(c.col_span)).sum::<usize>()
        });
        let col_specs = column_specs(&rows, ncols);

        let is_header_first = rows
            .first()
            .and_then(|r| r.first())
            .is_some_and(|c| c.is_header);

        let ast_rows = self.lay_grid(&rows, ncols, n_rows);

        let (head_rows, body_rows) = if is_header_first {
            let mut iter = ast_rows.into_iter();
            let head: Vec<Row> = iter.next().into_iter().collect();
            (head, iter.collect::<Vec<Row>>())
        } else {
            (Vec::new(), ast_rows)
        };

        let caption = match caption_text {
            Some(text) => {
                let inlines = self.parse_inlines(text.trim());
                if inlines.is_empty() {
                    Caption::default()
                } else {
                    Caption {
                        short: None,
                        long: vec![Block::Plain(inlines)],
                    }
                }
            }
            None => Caption::default(),
        };

        Block::Table(Box::new(Table {
            attr: Attr::default(),
            caption,
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: head_rows,
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: body_rows,
            }],
            foot: TableFoot::default(),
        }))
    }

    /// Lays the parsed cells onto a fixed `ncols`-wide grid so spans stay in bounds: a `rowspan`
    /// cannot reach past the last row, a `colspan` cannot reach past the last column (an overflowing
    /// cell is dropped), a cell skips columns still covered by a `rowspan` from an earlier row, and
    /// any column a row leaves uncovered is filled with an empty cell.
    fn lay_grid(&mut self, rows: &[Vec<RawCell>], ncols: usize, n_rows: usize) -> Vec<Row> {
        let mut ast_rows: Vec<Row> = Vec::new();
        let mut occupied: Vec<i32> = vec![0; ncols];
        for (r, raw) in rows.iter().enumerate() {
            let available = i32::try_from(n_rows.saturating_sub(r)).unwrap_or(i32::MAX);
            let mut cells: Vec<Cell> = Vec::new();
            let mut col = 0usize;
            for c in raw {
                while col < ncols && occupied.get(col).copied().unwrap_or(0) > 0 {
                    col += 1;
                }
                if col >= ncols {
                    break;
                }
                let col_span = col_count(c.col_span).min(ncols - col);
                let row_span = c.row_span.max(1).min(available);
                let content_chars: Vec<char> = c.content.trim().chars().collect();
                let content = self.parse_cell_blocks(&content_chars);
                cells.push(Cell {
                    attr: c.attr.clone(),
                    align: c.align.clone(),
                    row_span,
                    col_span: i32::try_from(col_span).unwrap_or(1),
                    content,
                });
                for k in col..col + col_span {
                    if let Some(slot) = occupied.get_mut(k) {
                        *slot = row_span;
                    }
                }
                col += col_span;
            }
            while col < ncols {
                if occupied.get(col).copied().unwrap_or(0) == 0 {
                    cells.push(empty_cell());
                }
                col += 1;
            }
            for slot in &mut occupied {
                *slot = (*slot - 1).max(0);
            }
            ast_rows.push(Row {
                attr: Attr::default(),
                cells,
            });
        }
        ast_rows
    }

    /// Parses a table cell's content. On the cell's first line the list and heading markers
    /// `* # ; =` are inert and read as plain paragraph text; from the second line on every marker is
    /// recognized again. Definition (`:`), horizontal rules, templates, and nested tables stay
    /// active even on the first line.
    fn parse_cell_blocks(&mut self, chars: &[char]) -> Vec<Block> {
        let first = at(chars, 0);
        let suppressed = matches!(first, Some('*' | '#' | ';'))
            || (first == Some('=') && is_header_line_within(chars, 0));
        if !suppressed {
            return self.parse_blocks(chars);
        }
        let (mut blocks, after) = self.parse_paragraph(chars, 0, &mut HeaderScan::default());
        if let Some(rest) = chars.get(after..) {
            blocks.extend(self.parse_blocks(rest));
        }
        blocks
    }

    /// Parses the content of a `<ref>` note as blocks; a lone paragraph becomes a [`Block::Plain`].
    fn note_blocks(&mut self, chars: &[char]) -> Vec<Block> {
        let blocks = self.parse_blocks(chars);
        match blocks.as_slice() {
            [Block::Para(inlines)] => vec![Block::Plain(inlines.clone())],
            _ => blocks,
        }
    }

    fn parse_inlines(&mut self, text: &str) -> Vec<Inline> {
        let chars: Vec<char> = text.chars().collect();
        let toks = self.lex(&chars, false, false);
        let mut inlines = coalesce(resolve_emphasis(toks));
        if self.extensions.contains(Extension::EastAsianLineBreaks) {
            inlines = drop_east_asian_breaks(inlines);
        }
        if self.smart() {
            inlines = apply_smart_quotes(inlines);
        }
        inlines
    }

    /// Parses one preformatted line: markup is honored, but literal text and its exact spacing are
    /// preserved as code spans rather than collapsed.
    fn preformatted_line(&mut self, text: &str) -> Vec<Inline> {
        let chars: Vec<char> = text.chars().collect();
        let toks = self.lex(&chars, true, false);
        preformat_transform(resolve_emphasis(toks))
    }

    #[allow(clippy::too_many_lines)]
    fn lex(&mut self, chars: &[char], preformatted: bool, block_context: bool) -> Vec<Tok> {
        let mut toks: Vec<Tok> = Vec::new();
        let mut word = String::new();
        let mut i = 0;
        let n = chars.len();
        while i < n {
            let Some(c) = at(chars, i) else { break };
            if c == '\'' {
                let mut end = i;
                while at(chars, end) == Some('\'') {
                    end += 1;
                }
                let run = end - i;
                if run >= 2 {
                    flush_word(&mut word, &mut toks);
                    toks.push(Tok::Apostrophes(run));
                } else {
                    word.push('\'');
                }
                i = end;
                continue;
            }
            if c.is_whitespace() {
                if preformatted {
                    word.push(c);
                    i += 1;
                    continue;
                }
                flush_word(&mut word, &mut toks);
                let (token, next) = whitespace_token(chars, i);
                toks.push(Tok::Inline(token));
                i = next;
                continue;
            }
            if c == '&' {
                if let Some((decoded, next)) = read_entity(chars, i) {
                    word.push_str(&decoded);
                    i = next;
                } else {
                    word.push('&');
                    i += 1;
                }
                continue;
            }
            if c == '<' {
                if let Some((inlines, next)) = self.handle_tag(chars, i) {
                    flush_word(&mut word, &mut toks);
                    for inline in inlines {
                        toks.push(Tok::Inline(inline));
                    }
                    i = next;
                    continue;
                }
                if block_context
                    && starts_block_tag(chars, i)
                    && let Some((block, next)) = self.parse_block_tag(chars, i)
                {
                    flush_word(&mut word, &mut toks);
                    toks.push(Tok::Block(block));
                    i = next;
                    continue;
                }
                if let Some((tok, next)) = block_tag_token(chars, i) {
                    flush_word(&mut word, &mut toks);
                    toks.push(tok);
                    i = next;
                    continue;
                }
                word.push('<');
                i += 1;
                continue;
            }
            if c == '{' && at(chars, i + 1) == Some('{') {
                if template_opens(chars, i)
                    && let Some(after) = balanced_braces(chars, i)
                {
                    flush_word(&mut word, &mut toks);
                    let raw = collect_range(chars, i, after);
                    toks.push(Tok::Inline(Inline::RawInline(
                        format_mediawiki(),
                        raw.into(),
                    )));
                    i = after;
                    continue;
                }
                word.push('{');
                i += 1;
                continue;
            }
            if c == '[' {
                let handled = if at(chars, i + 1) == Some('[') {
                    self.internal_link(chars, i)
                } else {
                    self.external_link(chars, i)
                };
                if let Some((inlines, next)) = handled {
                    flush_word(&mut word, &mut toks);
                    for inline in inlines {
                        toks.push(Tok::Inline(inline));
                    }
                    i = next;
                    continue;
                }
                // A single `[` glued to a bare URL is a literal bracket followed by that URL.
                if at(chars, i + 1) != Some('[')
                    && let Some((inline, next)) = bare_url(chars, i + 1)
                {
                    word.push('[');
                    flush_word(&mut word, &mut toks);
                    toks.push(Tok::Inline(inline));
                    i = next;
                    continue;
                }
                word.push('[');
                i += 1;
                continue;
            }
            if word.is_empty()
                && let Some((inline, next)) = bare_url(chars, i)
            {
                toks.push(Tok::Inline(inline));
                i = next;
                continue;
            }
            word.push(c);
            i += 1;
        }
        flush_word(&mut word, &mut toks);
        toks
    }

    #[allow(clippy::too_many_lines)]
    fn handle_tag(&mut self, chars: &[char], i: usize) -> Option<(Vec<Inline>, usize)> {
        if at(chars, i) != Some('<') {
            return None;
        }
        match at(chars, i + 1) {
            Some('/') => {
                let (name, raw, after) = close_tag_parse(chars, i)?;
                return match html_tag_role(&name) {
                    Some(HtmlTagRole::Inline) => Some((vec![raw_html(raw)], after)),
                    _ => None,
                };
            }
            Some(c) if c.is_ascii_alphabetic() => {}
            _ => return None,
        }
        let (name, raw_open, self_closing, after_open) = open_tag(chars, i)?;
        match name.as_str() {
            "br" => Some((vec![Inline::LineBreak], after_open)),
            "ref" => {
                if self_closing {
                    return Some((vec![Inline::Note(Vec::new())], after_open));
                }
                match close_tag(chars, after_open, "ref") {
                    Some((inner_end, after)) => {
                        let inner = collect_range(chars, after_open, inner_end);
                        let inner_chars: Vec<char> = inner.chars().collect();
                        Some((vec![Inline::Note(self.note_blocks(&inner_chars))], after))
                    }
                    None => Some((vec![raw_html(raw_open)], after_open)),
                }
            }
            "nowiki" => {
                if self_closing {
                    return Some((Vec::new(), after_open));
                }
                let (inner, after) = enclosed(chars, after_open, "nowiki");
                Some((plain_inlines(&inner), after))
            }
            "math" => {
                if self_closing {
                    return Some((Vec::new(), after_open));
                }
                match close_tag(chars, after_open, "math") {
                    Some((inner_end, after)) => {
                        let inner = collect_range(chars, after_open, inner_end);
                        Some((
                            vec![Inline::Math(MathType::InlineMath, inner.trim().into())],
                            after,
                        ))
                    }
                    None => Some((vec![raw_html(raw_open)], after_open)),
                }
            }
            "code" | "tt" => Some(verbatim_code(
                chars,
                &name,
                after_open,
                &raw_open,
                self_closing,
                &[],
            )),
            "var" => Some(verbatim_code(
                chars,
                "var",
                after_open,
                &raw_open,
                self_closing,
                &["variable"],
            )),
            "samp" => Some(verbatim_code(
                chars,
                "samp",
                after_open,
                &raw_open,
                self_closing,
                &["sample"],
            )),
            "sub" => Some(self.wrap(
                chars,
                "sub",
                after_open,
                &raw_open,
                self_closing,
                Inline::Subscript,
            )),
            "sup" => Some(self.wrap(
                chars,
                "sup",
                after_open,
                &raw_open,
                self_closing,
                Inline::Superscript,
            )),
            "del" | "strike" => Some(self.wrap(
                chars,
                &name,
                after_open,
                &raw_open,
                self_closing,
                Inline::Strikeout,
            )),
            "kbd" => Some(self.span(chars, "kbd", after_open, &raw_open, self_closing, "kbd")),
            "mark" => Some(self.span(chars, "mark", after_open, &raw_open, self_closing, "mark")),
            _ => match html_tag_role(&name) {
                Some(HtmlTagRole::Inline) => {
                    if self_closing {
                        return Some((vec![raw_html(raw_open)], after_open));
                    }
                    match close_tag(chars, after_open, &name) {
                        Some((inner_end, after)) => {
                            let inner = collect_range(chars, after_open, inner_end);
                            let close_raw = collect_range(chars, inner_end, after);
                            let mut out = vec![raw_html(raw_open)];
                            out.extend(self.parse_inlines(&inner));
                            out.push(raw_html(close_raw));
                            Some((out, after))
                        }
                        None => Some((vec![raw_html(raw_open)], after_open)),
                    }
                }
                // Block-level and unrecognized tags are not inline output: a recognized block tag
                // becomes a raw block at the paragraph level, an unrecognized tag stays literal.
                _ => None,
            },
        }
    }

    fn wrap(
        &mut self,
        chars: &[char],
        name: &str,
        after_open: usize,
        raw_open: &str,
        self_closing: bool,
        ctor: fn(Vec<Inline>) -> Inline,
    ) -> (Vec<Inline>, usize) {
        if self_closing {
            return (vec![raw_html(raw_open.to_string())], after_open);
        }
        match close_tag(chars, after_open, name) {
            Some((inner_end, after)) => {
                let inner = collect_range(chars, after_open, inner_end);
                (vec![ctor(self.parse_inlines(&inner))], after)
            }
            None => (vec![raw_html(raw_open.to_string())], after_open),
        }
    }

    fn span(
        &mut self,
        chars: &[char],
        name: &str,
        after_open: usize,
        raw_open: &str,
        self_closing: bool,
        class: &str,
    ) -> (Vec<Inline>, usize) {
        if self_closing {
            return (vec![raw_html(raw_open.to_string())], after_open);
        }
        match close_tag(chars, after_open, name) {
            Some((inner_end, after)) => {
                let inner = collect_range(chars, after_open, inner_end);
                let attr = Attr {
                    id: carta_ast::Text::default(),
                    classes: vec![class.into()],
                    attributes: Vec::new(),
                };
                (
                    vec![Inline::Span(Box::new(attr), self.parse_inlines(&inner))],
                    after,
                )
            }
            None => (vec![raw_html(raw_open.to_string())], after_open),
        }
    }

    fn external_link(&mut self, chars: &[char], i: usize) -> Option<(Vec<Inline>, usize)> {
        let close = find_char(chars, i + 1, ']')?;
        let inner = collect_range(chars, i + 1, close);
        let (url, label) = match inner.split_once(|c: char| c.is_whitespace()) {
            Some((u, rest)) => (u.to_string(), rest.trim_start().to_string()),
            None => (inner.clone(), String::new()),
        };
        if !is_url(&url) {
            return None;
        }
        // A bracketed URL with no label that runs straight into a letter or digit is not a link: the
        // bracket stays literal and the URL continues past the `]` as a bare URL.
        if label.is_empty() && at(chars, close + 1).is_some_and(char::is_alphanumeric) {
            return None;
        }
        let text = if label.is_empty() {
            self.link_counter += 1;
            vec![Inline::Str(self.link_counter.to_compact_string())]
        } else {
            self.parse_inlines(&label)
        };
        Some((
            vec![Inline::Link(
                Box::default(),
                text,
                Box::new(Target {
                    url: encode_url_target(&url).into(),
                    title: carta_ast::Text::default(),
                }),
            )],
            close + 1,
        ))
    }

    fn internal_link(&mut self, chars: &[char], i: usize) -> Option<(Vec<Inline>, usize)> {
        // The target ends at the first `|` or the first `]]`, whichever comes first; nesting is not
        // tracked, so a `]]` from an inner link can close an unpiped target.
        let start = i + 2;
        let (target_end, has_pipe) = scan_link_target(chars, start)?;
        let target = collect_range(chars, start, target_end).trim().to_string();

        // With a pipe present, the label runs to the `]]` that closes this link, stepping over any
        // nested `[[ … ]]` so an inner link does not close the outer one.
        let (label_content, close) = if has_pipe {
            let label_start = target_end + 1;
            let close = find_link_close(chars, label_start)?;
            (Some(collect_range(chars, label_start, close)), close)
        } else {
            (None, target_end)
        };

        if let Some(ns) = namespace_of(&target) {
            if ns == "category" {
                let text = match &label_content {
                    Some(label) if !label.trim().is_empty() => self.parse_inlines(label),
                    _ => self.parse_inlines(&target),
                };
                let title = title_text(&text);
                let attr = Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["wikilink".into()],
                    attributes: Vec::new(),
                };
                self.categories.push(Inline::Link(
                    Box::new(attr),
                    text,
                    Box::new(Target {
                        url: wikilink_url(&target).into(),
                        title: title.into(),
                    }),
                ));
                return Some((Vec::new(), close + 2));
            }
            // A file or image embed may decline (a parameter it cannot represent as an image); when it
            // does, the markup falls through to the ordinary wikilink path below.
            if matches!(ns.as_str(), "file" | "image")
                && !strip_namespace(&target).is_empty()
                && let Some(image) = self.image_embed(&target, label_content.as_deref())
            {
                return Some((vec![image], close + 2));
            }
        }
        let mut after = close + 2;
        let mut trail = String::new();
        while let Some(c) = at(chars, after) {
            if c.is_ascii_alphabetic() {
                trail.push(c);
                after += 1;
            } else {
                break;
            }
        }
        let mut label = match &label_content {
            // An empty label invokes the pipe trick: the display text is derived from the target.
            Some(l) if l.trim().is_empty() => self.pipe_trick_label(&target),
            Some(l) => self.parse_inlines(l),
            None => self.parse_inlines(&target),
        };
        let title = title_text(&label);
        if !trail.is_empty() {
            label.push(Inline::Str(trail.into()));
            label = coalesce(label);
        }
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: vec!["wikilink".into()],
            attributes: Vec::new(),
        };
        let url = wikilink_url(&target);
        Some((
            vec![Inline::Link(
                Box::new(attr),
                label,
                Box::new(Target {
                    url: url.into(),
                    title: title.into(),
                }),
            )],
            after,
        ))
    }

    /// The display text the pipe trick derives from an empty-label link's target: the part after the
    /// first colon when the target is namespaced (so `Help:Contents` shows as `Contents`), otherwise
    /// no text at all.
    fn pipe_trick_label(&mut self, target: &str) -> Vec<Inline> {
        match target.split_once(':') {
            Some((_, rest)) => self.parse_inlines(rest),
            None => Vec::new(),
        }
    }

    /// Builds the image for a `[[File:…|…]]` / `[[Image:…|…]]` embed. The page name (with the
    /// namespace stripped) is the source; the `WxHpx` parameters set width/height; recognized
    /// placement and option keywords are dropped; the last remaining parameter is the caption,
    /// defaulting to the file name. A lone embed in its own paragraph later becomes a figure
    /// (see [`lone_image_figure`]).
    fn image_embed(&mut self, target: &str, params: Option<&str>) -> Option<Inline> {
        let url = wikilink_url(strip_namespace(target));
        let mut attributes: Vec<(String, String)> = Vec::new();
        let mut caption: Option<String> = None;
        if let Some(params) = params {
            for part in params.split('|') {
                let option = part.trim();
                if image_param_declines(option) {
                    return None;
                }
                if let Some((width, height)) = image_size(option) {
                    attributes.retain(|(key, _)| key != "width" && key != "height");
                    attributes.push(("width".to_string(), width));
                    if let Some(height) = height {
                        attributes.push(("height".to_string(), height));
                    }
                } else if is_image_keyword(option) || is_recognized_image_attr(option) {
                    // A placement or framing keyword, or a recognized `key=value` attribute, carries
                    // no caption text. An unrecognized `key=value` is treated as caption text.
                } else {
                    caption = Some(part.to_string());
                }
            }
        }
        let caption = caption.unwrap_or_else(|| url.clone());
        let alt = self.parse_inlines(&caption);
        let title = title_text(&alt);
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        };
        Some(Inline::Image(
            Box::new(attr),
            alt,
            Box::new(Target {
                url: url.into(),
                title: title.into(),
            }),
        ))
    }

    fn make_id(&mut self, inlines: &[Inline]) -> String {
        let plain = to_plain_text(inlines);
        if self.extensions.contains(Extension::GfmAutoIdentifiers) {
            let base = self.finish_id(slug_gfm, &emoji_to_aliases(&plain));
            self.dedup(base, '-')
        } else if self.extensions.contains(Extension::AutoIdentifiers) {
            let base = self.finish_id(mediawiki_slug, &plain);
            self.dedup(base, '_')
        } else {
            String::new()
        }
    }

    /// Builds an identifier with `slug`, then — when `ascii_identifiers` is on — folds the finished
    /// slug to pure ASCII (accents stripped, non-Latin letters dropped) and re-slugs it, so a dropped
    /// letter leaves its separators intact while a now-leading separator is trimmed. An empty result
    /// becomes a placeholder.
    fn finish_id(&self, slug: fn(&str) -> String, source: &str) -> String {
        let mut base = slug(source);
        if self.extensions.contains(Extension::AsciiIdentifiers) {
            base = slug(&transliterate_ascii(&base));
        }
        if base.is_empty() {
            "section".to_string()
        } else {
            base
        }
    }

    fn dedup(&mut self, base: String, sep: char) -> String {
        if !self.seen_ids.contains(&base) {
            self.seen_ids.insert(base.clone());
            return base;
        }
        let mut k = 1usize;
        loop {
            let candidate = format!("{base}{sep}{k}");
            if !self.seen_ids.contains(&candidate) {
                self.seen_ids.insert(candidate.clone());
                return candidate;
            }
            k += 1;
        }
    }
}

// --- preprocessing ------------------------------------------------------------------------------

/// Expands tab characters to spaces on a four-column grid, with the column resetting at each line
/// break. Wikitext markup is column-sensitive — a leading space marks preformatted text — so tabs
/// are normalized before any block scanning runs.
fn expand_tabs(input: &str) -> String {
    if !input.contains('\t') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut col = 0usize;
    for ch in input.chars() {
        match ch {
            '\t' => {
                let spaces = 4 - (col % 4);
                for _ in 0..spaces {
                    out.push(' ');
                }
                col += spaces;
            }
            '\n' => {
                out.push('\n');
                col = 0;
            }
            other => {
                out.push(other);
                col += 1;
            }
        }
    }
    out
}

// --- comment stripping --------------------------------------------------------------------------

/// Removes wikitext comments. A comment that is the whole line (preceded by a line start and
/// followed by a line end) is dropped together with its trailing newline; one embedded in other
/// text collapses to a single space. Verbatim regions (`pre`, `nowiki`, `math`, `source`,
/// `syntaxhighlight`) are copied unchanged so comment-like text inside them survives. An
/// unterminated `<!--` is left as literal text.
fn strip_comments(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let Some(c) = at(&chars, i) else { break };
        if c == '<' {
            if let Some(after) = verbatim_region_end(&chars, i) {
                out.push_str(&collect_range(&chars, i, after));
                i = after;
                continue;
            }
            if matches_prefix_ci(&chars, i, "<!--") {
                if let Some(dash) = find_seq(&chars, i + 4, &['-', '-', '>']) {
                    let comment_end = dash + 3;
                    let preceded = i == 0 || at(&chars, i - 1) == Some('\n');
                    let followed = comment_end >= n || at(&chars, comment_end) == Some('\n');
                    if preceded && followed {
                        i = if comment_end < n {
                            comment_end + 1
                        } else {
                            comment_end
                        };
                    } else if preceded || followed {
                        // Adjacent to a line boundary, the comment leaves nothing behind, so the
                        // line neither gains a leading space (which would make it preformatted) nor
                        // a trailing one.
                        i = comment_end;
                    } else {
                        // Between text, the comment collapses to a single space.
                        out.push(' ');
                        i = comment_end;
                    }
                    continue;
                }
                out.push('<');
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// If a verbatim tag opens at `i`, the index just past its closing tag (or end of input).
fn verbatim_region_end(chars: &[char], i: usize) -> Option<usize> {
    let (name, _raw, self_closing, after_open) = open_tag(chars, i)?;
    if !matches!(
        name.as_str(),
        "pre" | "nowiki" | "math" | "source" | "syntaxhighlight"
    ) {
        return None;
    }
    if self_closing {
        return Some(after_open);
    }
    match close_tag(chars, after_open, &name) {
        Some((_, after)) => Some(after),
        None => Some(chars.len()),
    }
}

/// Behavior switches recognized in `__WORD__` form. A matched switch is removed from the text and
/// recorded as a boolean metadata entry under its lowercased name; the comparison is case-sensitive,
/// so only the uppercase spelling is a switch.
const BEHAVIOR_SWITCHES: &[&str] = &[
    "ARCHIVEDTALK",
    "DISAMBIG",
    "EXPECTUNUSEDCATEGORY",
    "EXPECTUNUSEDTEMPLATE",
    "FORCETOC",
    "HIDDENCAT",
    "INDEX",
    "NEWSECTIONLINK",
    "NOCC",
    "NOCONTENTCONVERT",
    "NOEDITSECTION",
    "NOGALLERY",
    "NOGLOBAL",
    "NOINDEX",
    "NONEWSECTIONLINK",
    "NOTC",
    "NOTITLECONVERT",
    "NOTOC",
    "STATICREDIRECT",
    "TOC",
];

/// Removes every recognized `__WORD__` behavior switch from the text, returning the cleaned text and
/// the lowercased names of the switches found in document order. Switches inside verbatim regions
/// (`<nowiki>`, `<pre>`, …) are left untouched as literal text.
fn extract_behavior_switches(input: &str) -> (String, Vec<String>) {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut found: Vec<String> = Vec::new();
    let mut i = 0;
    while i < n {
        if at(&chars, i) == Some('<')
            && let Some(after) = verbatim_region_end(&chars, i)
        {
            out.push_str(&collect_range(&chars, i, after));
            i = after;
            continue;
        }
        if at(&chars, i) == Some('_')
            && at(&chars, i + 1) == Some('_')
            && let Some((word, after)) = behavior_switch_at(&chars, i)
        {
            let key = word.to_ascii_lowercase();
            if !found.contains(&key) {
                found.push(key);
            }
            i = after;
            // A switch that begins a line is removed together with the spaces and tabs that follow
            // it on that line, so the line does not gain a leading space that would mark it as
            // preformatted text; the line break itself is left in place.
            if out.is_empty() || out.ends_with('\n') {
                while matches!(at(&chars, i), Some(' ' | '\t')) {
                    i += 1;
                }
            }
            continue;
        }
        if let Some(c) = at(&chars, i) {
            out.push(c);
        }
        i += 1;
    }
    (out, found)
}

/// Reads a `__WORD__` behavior switch at `i`, returning the uppercase word and the index past it.
fn behavior_switch_at(chars: &[char], i: usize) -> Option<(String, usize)> {
    let start = i + 2;
    let mut j = start;
    while at(chars, j).is_some_and(|c| c.is_ascii_uppercase()) {
        j += 1;
    }
    let word = collect_range(chars, start, j);
    if word.is_empty()
        || at(chars, j) != Some('_')
        || at(chars, j + 1) != Some('_')
        || !BEHAVIOR_SWITCHES.contains(&word.as_str())
    {
        return None;
    }
    Some((word, j + 2))
}

// --- emphasis resolution ------------------------------------------------------------------------

/// A unit of the stream emphasis resolution works over: one apostrophe of a run, or a finished node.
enum Unit {
    Apostrophe,
    Node(Inline),
}

/// Resolves apostrophe emphasis. Runs of two apostrophes open and close `Emph`, three open and close
/// `Strong`. The structure is found by recursive descent with backtracking: at each run the parser
/// tries to open the span whose width fits, parses its content up to a matching closing run, and
/// falls back to a literal apostrophe when no span can be formed. A span is never reopened by its
/// immediate parent of the same kind, and a span's content has its outer whitespace removed.
fn resolve_emphasis(toks: Vec<Tok>) -> Vec<Inline> {
    let mut units: Vec<Unit> = Vec::new();
    for tok in toks {
        match tok {
            Tok::Inline(inline) => units.push(Unit::Node(inline)),
            Tok::Apostrophes(n) => units.extend((0..n).map(|_| Unit::Apostrophe)),
            Tok::BlockRaw(raw) => units.push(Unit::Node(raw_html(raw))),
            Tok::BlockBreak | Tok::Block(_) => {}
        }
    }
    let runs = apostrophe_runs(&units);
    // Bound the backtracking work so adversarial apostrophe-dense input cannot blow up.
    let mut budget = units
        .len()
        .saturating_mul(8)
        .saturating_add(64)
        .min(200_000);
    let (nodes, _, _) = parse_runs(&units, &runs, 0, None, &mut budget);
    nodes
}

/// For each position, the length of the apostrophe run starting there (zero at a non-apostrophe).
fn apostrophe_runs(units: &[Unit]) -> Vec<usize> {
    let mut runs = vec![0usize; units.len()];
    for i in (0..units.len()).rev() {
        if matches!(units.get(i), Some(Unit::Apostrophe)) {
            let next = runs.get(i + 1).copied().unwrap_or(0);
            if let Some(slot) = runs.get_mut(i) {
                *slot = 1 + next;
            }
        }
    }
    runs
}

/// The apostrophe width an emphasis kind consumes: three for `Strong`, two for `Emph`.
fn emphasis_width(strong: bool) -> usize {
    if strong { 3 } else { 2 }
}

/// Tries to open an emphasis span of the given kind at the apostrophe run starting at `i`. Returns
/// the span node and the index just past its closing run, or `None` if no matching closer is found
/// or the span would be empty.
fn try_open(
    units: &[Unit],
    runs: &[usize],
    i: usize,
    strong: bool,
    budget: &mut usize,
) -> Option<(Inline, usize)> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let width = emphasis_width(strong);
    let (body, next, closed) = parse_runs(units, runs, i + width, Some(strong), budget);
    if !closed || body.is_empty() {
        return None;
    }
    let body = strip_outer_whitespace(body);
    Some((
        if strong {
            Inline::Strong(body)
        } else {
            Inline::Emph(body)
        },
        next,
    ))
}

/// Parses content until the run that closes `closer` (or end of input when `closer` is `None`).
/// Returns the collected nodes, the index reached, and whether a closer was found.
///
/// At each apostrophe run, a wider `'''…'''` strong span is preferred over a `''…''` emphasis span,
/// and closing the enclosing span takes precedence over opening a same-kind span. A run that opens
/// nothing and closes nothing is emitted as literal apostrophes.
fn parse_runs(
    units: &[Unit],
    runs: &[usize],
    start: usize,
    closer: Option<bool>,
    budget: &mut usize,
) -> (Vec<Inline>, usize, bool) {
    let mut nodes: Vec<Inline> = Vec::new();
    let mut pos = start;
    while let Some(unit) = units.get(pos) {
        match unit {
            Unit::Node(inline) => {
                nodes.push(inline.clone());
                pos += 1;
            }
            Unit::Apostrophe => {
                let run = runs.get(pos).copied().unwrap_or(0);
                if run >= emphasis_width(true)
                    && closer != Some(true)
                    && let Some((span, next)) = try_open(units, runs, pos, true, budget)
                {
                    nodes.push(span);
                    pos = next;
                    continue;
                }
                if let Some(strong) = closer
                    && run >= emphasis_width(strong)
                {
                    return (nodes, pos + emphasis_width(strong), true);
                }
                if run >= emphasis_width(false)
                    && closer != Some(false)
                    && let Some((span, next)) = try_open(units, runs, pos, false, budget)
                {
                    nodes.push(span);
                    pos = next;
                    continue;
                }
                nodes.push(Inline::Str("'".into()));
                pos += 1;
            }
        }
    }
    (nodes, pos, closer.is_none())
}

/// Removes leading and trailing spaces and soft breaks from a span's content.
fn strip_outer_whitespace(mut inlines: Vec<Inline>) -> Vec<Inline> {
    let lead = inlines
        .iter()
        .take_while(|x| matches!(x, Inline::Space | Inline::SoftBreak))
        .count();
    inlines.drain(0..lead);
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
    inlines
}

/// A flattened unit used while pairing smart double quotes: a `"` awaiting a partner, an ordinary
/// character, a whitespace inline (which cannot follow an opening quote), or an opaque inline node
/// carried through unchanged.
enum SmartUnit {
    Quote,
    Ch(char),
    Space(Inline),
    Node(Inline),
}

/// Folds straight double quotes into [`Inline::Quoted`] runs. A double quote followed by
/// non-whitespace content opens a run that the next double quote closes; an unpaired quote stays a
/// literal `"`. Single quotes, which mark emphasis, are left untouched. The fold also descends into
/// the children of container inlines.
fn apply_smart_quotes(inlines: Vec<Inline>) -> Vec<Inline> {
    let recursed: Vec<Inline> = inlines.into_iter().map(smart_descend).collect();
    let units = flatten_smart(recursed);
    resolve_double_quotes(&units, 0, units.len())
}

/// Applies the double-quote fold to the inline children of a container, leaving leaf and opaque
/// inlines (text, code, math, raw passthrough, notes) untouched.
fn smart_descend(inline: Inline) -> Inline {
    match inline {
        Inline::Emph(v) => Inline::Emph(apply_smart_quotes(v)),
        Inline::Underline(v) => Inline::Underline(apply_smart_quotes(v)),
        Inline::Strong(v) => Inline::Strong(apply_smart_quotes(v)),
        Inline::Strikeout(v) => Inline::Strikeout(apply_smart_quotes(v)),
        Inline::Superscript(v) => Inline::Superscript(apply_smart_quotes(v)),
        Inline::Subscript(v) => Inline::Subscript(apply_smart_quotes(v)),
        Inline::SmallCaps(v) => Inline::SmallCaps(apply_smart_quotes(v)),
        Inline::Quoted(quote_type, v) => Inline::Quoted(quote_type, apply_smart_quotes(v)),
        Inline::Span(attr, v) => Inline::Span(attr, apply_smart_quotes(v)),
        Inline::Link(attr, v, target) => Inline::Link(attr, apply_smart_quotes(v), target),
        Inline::Image(attr, v, target) => Inline::Image(attr, apply_smart_quotes(v), target),
        other => other,
    }
}

fn flatten_smart(inlines: Vec<Inline>) -> Vec<SmartUnit> {
    let mut units: Vec<SmartUnit> = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Str(text) => {
                for c in text.chars() {
                    if c == '"' {
                        units.push(SmartUnit::Quote);
                    } else {
                        units.push(SmartUnit::Ch(c));
                    }
                }
            }
            space @ (Inline::Space | Inline::SoftBreak | Inline::LineBreak) => {
                units.push(SmartUnit::Space(space));
            }
            other => units.push(SmartUnit::Node(other)),
        }
    }
    units
}

fn resolve_double_quotes(units: &[SmartUnit], lo: usize, hi: usize) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut buf = String::new();
    let mut i = lo;
    while i < hi {
        match units.get(i) {
            Some(SmartUnit::Quote) => {
                if smart_quote_opens(units, i, hi)
                    && let Some(j) = next_smart_quote(units, i + 1, hi)
                {
                    flush_smart_buf(&mut buf, &mut out);
                    out.push(Inline::Quoted(
                        QuoteType::DoubleQuote,
                        strip_outer_whitespace(resolve_double_quotes(units, i + 1, j)),
                    ));
                    i = j + 1;
                } else {
                    buf.push('"');
                    i += 1;
                }
            }
            Some(SmartUnit::Ch(c)) => {
                buf.push(*c);
                i += 1;
            }
            Some(SmartUnit::Space(inline) | SmartUnit::Node(inline)) => {
                flush_smart_buf(&mut buf, &mut out);
                out.push(inline.clone());
                i += 1;
            }
            None => break,
        }
    }
    flush_smart_buf(&mut buf, &mut out);
    out
}

fn flush_smart_buf(buf: &mut String, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        out.push(Inline::Str(std::mem::take(buf).into()));
    }
}

/// A double quote opens a run when the unit immediately after it, within the same span, is
/// non-whitespace content.
fn smart_quote_opens(units: &[SmartUnit], i: usize, hi: usize) -> bool {
    if i + 1 >= hi {
        return false;
    }
    match units.get(i + 1) {
        Some(SmartUnit::Ch(c)) => !c.is_whitespace(),
        Some(SmartUnit::Quote | SmartUnit::Node(_)) => true,
        Some(SmartUnit::Space(_)) | None => false,
    }
}

fn next_smart_quote(units: &[SmartUnit], from: usize, hi: usize) -> Option<usize> {
    (from..hi).find(|&j| matches!(units.get(j), Some(SmartUnit::Quote)))
}

/// Merges adjacent string runs so a span never holds two consecutive [`Inline::Str`] nodes,
/// descending into the markup wrappers a reader produces.
fn coalesce(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for inline in inlines {
        let inline = match inline {
            Inline::Emph(xs) => Inline::Emph(coalesce(xs)),
            Inline::Strong(xs) => Inline::Strong(coalesce(xs)),
            Inline::Strikeout(xs) => Inline::Strikeout(coalesce(xs)),
            Inline::Superscript(xs) => Inline::Superscript(coalesce(xs)),
            Inline::Subscript(xs) => Inline::Subscript(coalesce(xs)),
            Inline::Underline(xs) => Inline::Underline(coalesce(xs)),
            Inline::SmallCaps(xs) => Inline::SmallCaps(coalesce(xs)),
            Inline::Span(attr, xs) => Inline::Span(attr, coalesce(xs)),
            other => other,
        };
        match (out.last_mut(), &inline) {
            (Some(Inline::Str(prev)), Inline::Str(next)) => prev.push_str(next),
            // Two whitespace tokens land next to each other only where a zero-width construct (a
            // category, an empty element) was removed between them; collapse them to one, keeping a
            // soft break if either side carried one.
            (
                Some(slot @ (Inline::Space | Inline::SoftBreak)),
                Inline::Space | Inline::SoftBreak,
            ) => {
                if matches!(inline, Inline::SoftBreak) {
                    *slot = Inline::SoftBreak;
                }
            }
            _ => out.push(inline),
        }
    }
    out
}

/// Removes a soft line break that falls between two East Asian wide characters, so wrapped CJK text
/// rejoins with no intervening space. A break next to a non-wide character, or an explicit space, is
/// left as is.
fn drop_east_asian_breaks(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    let mut iter = inlines.into_iter().peekable();
    while let Some(inline) = iter.next() {
        if matches!(inline, Inline::SoftBreak) {
            let prev_wide = out.last().and_then(trailing_char).is_some_and(is_wide_char);
            let next_wide = iter.peek().and_then(leading_char).is_some_and(is_wide_char);
            if prev_wide && next_wide {
                continue;
            }
        }
        out.push(inline);
    }
    out
}

/// The last rendered character of an inline, descending into wrapper inlines, or `None` for one that
/// renders no character at the boundary (a break, image, or note).
fn trailing_char(inline: &Inline) -> Option<char> {
    match inline {
        Inline::Str(s) | Inline::Code(_, s) | Inline::Math(_, s) | Inline::RawInline(_, s) => {
            s.chars().next_back()
        }
        Inline::Emph(xs)
        | Inline::Underline(xs)
        | Inline::Strong(xs)
        | Inline::Strikeout(xs)
        | Inline::Superscript(xs)
        | Inline::Subscript(xs)
        | Inline::SmallCaps(xs)
        | Inline::Quoted(_, xs)
        | Inline::Span(_, xs)
        | Inline::Link(_, xs, _)
        | Inline::Cite(_, xs) => xs.iter().rev().find_map(trailing_char),
        _ => None,
    }
}

/// The first rendered character of an inline, descending into wrapper inlines, or `None` for one
/// that renders no character at the boundary.
fn leading_char(inline: &Inline) -> Option<char> {
    match inline {
        Inline::Str(s) | Inline::Code(_, s) | Inline::Math(_, s) | Inline::RawInline(_, s) => {
            s.chars().next()
        }
        Inline::Emph(xs)
        | Inline::Underline(xs)
        | Inline::Strong(xs)
        | Inline::Strikeout(xs)
        | Inline::Superscript(xs)
        | Inline::Subscript(xs)
        | Inline::SmallCaps(xs)
        | Inline::Quoted(_, xs)
        | Inline::Span(_, xs)
        | Inline::Link(_, xs, _)
        | Inline::Cite(_, xs) => xs.iter().find_map(leading_char),
        _ => None,
    }
}

/// Whether `c` is an East Asian wide or fullwidth character, the class of characters that wrap
/// without a separating space.
fn is_wide_char(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x1100..=0x115F
        | 0x2329 | 0x232A
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B2FF
        | 0x1F200..=0x1F2FF
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD
    )
}

// --- preformatted text --------------------------------------------------------------------------

/// Turns a parsed preformatted line into code spans: runs of literal text and spaces become
/// [`Inline::Code`] while markup wrappers keep their structure with code interiors. A space inside a
/// code run is held as a non-breaking space so the rendered width is preserved.
fn preformat_transform(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut run = String::new();
    for inline in inlines {
        match inline {
            Inline::Str(s) => run.push_str(&s.replace(' ', "\u{a0}")),
            Inline::Space | Inline::SoftBreak => run.push('\u{a0}'),
            other => {
                if !run.is_empty() {
                    out.push(Inline::Code(
                        Box::default(),
                        std::mem::take(&mut run).into(),
                    ));
                }
                out.push(preformat_descend(other));
            }
        }
    }
    if !run.is_empty() {
        out.push(Inline::Code(Box::default(), run.into()));
    }
    out
}

/// Recurses preformatting into a wrapper inline, leaving leaf inlines (code, math, breaks, raw)
/// untouched.
fn preformat_descend(inline: Inline) -> Inline {
    match inline {
        Inline::Emph(xs) => Inline::Emph(preformat_transform(xs)),
        Inline::Strong(xs) => Inline::Strong(preformat_transform(xs)),
        Inline::Strikeout(xs) => Inline::Strikeout(preformat_transform(xs)),
        Inline::Superscript(xs) => Inline::Superscript(preformat_transform(xs)),
        Inline::Subscript(xs) => Inline::Subscript(preformat_transform(xs)),
        Inline::Underline(xs) => Inline::Underline(preformat_transform(xs)),
        Inline::SmallCaps(xs) => Inline::SmallCaps(preformat_transform(xs)),
        Inline::Span(attr, xs) => Inline::Span(attr, preformat_transform(xs)),
        Inline::Link(attr, xs, target) => Inline::Link(attr, preformat_transform(xs), target),
        other => other,
    }
}

// --- plain text & entities ----------------------------------------------------------------------

/// Tokenizes literal text (used for `nowiki`): entity references are decoded, whitespace runs become
/// a single [`Inline::Space`] or [`Inline::SoftBreak`], and no other markup is recognized.
fn plain_inlines(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out: Vec<Inline> = Vec::new();
    let mut word = String::new();
    let mut i = 0;
    while i < n {
        let Some(c) = at(&chars, i) else { break };
        if c.is_whitespace() {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word).into()));
            }
            let (token, next) = whitespace_token(&chars, i);
            out.push(token);
            i = next;
        } else if c == '&' {
            if let Some((decoded, next)) = read_entity(&chars, i) {
                word.push_str(&decoded);
                i = next;
            } else {
                word.push('&');
                i += 1;
            }
        } else {
            word.push(c);
            i += 1;
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word.into()));
    }
    out
}

/// Decodes every entity reference in a string, leaving other characters untouched.
fn decode_entities(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        if at(&chars, i) == Some('&')
            && let Some((decoded, next)) = read_entity(&chars, i)
        {
            out.push_str(&decoded);
            i = next;
            continue;
        }
        if let Some(c) = at(&chars, i) {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Reads one entity reference starting at the `&` in `chars[i]`, returning its decoded text and the
/// index just past the closing `;`. Named, decimal, and hexadecimal forms are recognized.
fn read_entity(chars: &[char], i: usize) -> Option<(String, usize)> {
    let mut j = i + 1;
    if at(chars, j) == Some('#') {
        j += 1;
        let hex = matches!(at(chars, j), Some('x' | 'X'));
        if hex {
            j += 1;
        }
        let start = j;
        while let Some(c) = at(chars, j) {
            let digit = if hex {
                c.is_ascii_hexdigit()
            } else {
                c.is_ascii_digit()
            };
            if digit {
                j += 1;
            } else {
                break;
            }
        }
        if j == start || at(chars, j) != Some(';') {
            return None;
        }
        let digits = collect_range(chars, start, j);
        let code = u32::from_str_radix(&digits, if hex { 16 } else { 10 }).ok()?;
        Some((entities::code_point(code).to_string(), j + 1))
    } else {
        let start = j;
        while let Some(c) = at(chars, j) {
            if c.is_ascii_alphanumeric() {
                j += 1;
            } else {
                break;
            }
        }
        if j == start || at(chars, j) != Some(';') {
            return None;
        }
        let name = collect_range(chars, start, j);
        let decoded = entities::lookup_named(&name)?;
        Some((decoded.to_string(), j + 1))
    }
}

// --- bare URLs & namespaces ---------------------------------------------------------------------

/// Whether `name` (compared case-insensitively) is a recognized URL scheme. Beyond the shared
/// registry, this format additionally autolinks the `doi` and `javascript` schemes.
fn is_scheme(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    crate::url_schemes::is_scheme(&lower) || lower == "doi" || lower == "javascript"
}

/// Whether `text` begins with a recognized scheme followed by a colon — the test a bracketed
/// `[url label]` target must pass to be a link.
fn is_url(text: &str) -> bool {
    match text.split_once(':') {
        Some((scheme, _)) => {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                && is_scheme(scheme)
        }
        None => false,
    }
}

/// The length of a `scheme:` prefix at `i` (the scheme name plus its colon) when the name is a
/// recognized scheme, else `None`. The scheme name is the run of letters, digits, `+`, `-`, and `.`
/// before the colon.
fn url_scheme_len(chars: &[char], i: usize) -> Option<usize> {
    let mut j = i;
    let mut name = String::new();
    while let Some(c) = at(chars, j) {
        if c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.') {
            name.push(c);
            j += 1;
        } else {
            break;
        }
    }
    if name.is_empty() || at(chars, j) != Some(':') || !is_scheme(&name) {
        return None;
    }
    Some(j - i + 1)
}

/// Reads a bare URL beginning at a word boundary. The URL runs to the next space or angle bracket,
/// after which trailing punctuation and unbalanced brackets are trimmed back. The displayed text
/// keeps the characters literally while the link target percent-encodes the unsafe ones. Returns the
/// autolink and the index just past the consumed URL.
fn bare_url(chars: &[char], i: usize) -> Option<(Inline, usize)> {
    let scheme_len = url_scheme_len(chars, i)?;
    let mut j = i + scheme_len;
    while let Some(c) = at(chars, j) {
        if c.is_whitespace() || matches!(c, '<' | '>') {
            break;
        }
        // A run of two or more apostrophes opens emphasis, so it also ends the URL.
        if c == '\'' && at(chars, j + 1) == Some('\'') {
            break;
        }
        j += 1;
    }
    if j <= i + scheme_len {
        return None;
    }
    let mut display = collect_range(chars, i, j);
    trim_url_trailing(&mut display);
    if display.is_empty() {
        return None;
    }
    let consumed = display.chars().count();
    let target = encode_url_target(&display);
    Some((
        Inline::Link(
            Box::default(),
            vec![Inline::Str(display.into())],
            Box::new(Target {
                url: target.into(),
                title: carta_ast::Text::default(),
            }),
        ),
        i + consumed,
    ))
}

/// Trims a URL's trailing characters that read as sentence punctuation or unbalanced brackets: the
/// always-trimmed set never legitimately ends a URL, and a closing bracket is trimmed only when it
/// outnumbers its opener so a balanced `(a)` or `[a]` survives.
fn trim_url_trailing(url: &mut String) {
    while let Some(last) = url.chars().last() {
        let always = matches!(
            last,
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '*' | '~' | '\'' | '|'
        );
        let unbalanced = match last {
            ')' => url.matches(')').count() > url.matches('(').count(),
            ']' => url.matches(']').count() > url.matches('[').count(),
            '}' => url.matches('}').count() > url.matches('{').count(),
            _ => false,
        };
        if always || unbalanced {
            url.pop();
        } else {
            break;
        }
    }
}

/// Percent-encodes the characters a wikitext link target escapes, leaving the rest intact.
fn encode_url_target(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        match ch {
            ' ' => out.push_str("%20"),
            '"' => out.push_str("%22"),
            '`' => out.push_str("%60"),
            '^' => out.push_str("%5E"),
            '[' => out.push_str("%5B"),
            ']' => out.push_str("%5D"),
            '{' => out.push_str("%7B"),
            '}' => out.push_str("%7D"),
            '|' => out.push_str("%7C"),
            other => out.push(other),
        }
    }
    out
}

/// Builds a wikilink target URL from a page name: each run of whitespace collapses to a single
/// underscore, every other character is kept as written.
fn wikilink_url(target: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for ch in target.chars() {
        if ch.is_whitespace() {
            pending = true;
        } else {
            if pending {
                out.push('_');
                pending = false;
            }
            out.push(ch);
        }
    }
    out
}

/// Flatten inline content into the plain string stored as a link or image title. Markup wrappers
/// unwrap to their contents and breaks collapse to a space, as for any plain-text flattening, but a
/// [`Inline::Quoted`] node renders the matching curly quote glyphs around its contents so a curled
/// quotation survives into the title text.
fn title_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_title_text(inlines, &mut out);
    out
}

fn push_title_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => out.push_str(text),
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => out.push(' '),
            Inline::Quoted(QuoteType::SingleQuote, xs) => {
                out.push('\u{2018}');
                push_title_text(xs, out);
                out.push('\u{2019}');
            }
            Inline::Quoted(QuoteType::DoubleQuote, xs) => {
                out.push('\u{201c}');
                push_title_text(xs, out);
                out.push('\u{201d}');
            }
            Inline::Emph(xs)
            | Inline::Underline(xs)
            | Inline::Strong(xs)
            | Inline::Strikeout(xs)
            | Inline::Superscript(xs)
            | Inline::Subscript(xs)
            | Inline::SmallCaps(xs)
            | Inline::Cite(_, xs)
            | Inline::Link(_, xs, _)
            | Inline::Image(_, xs, _)
            | Inline::Span(_, xs) => push_title_text(xs, out),
            Inline::RawInline(..) | Inline::Note(_) => {}
        }
    }
}

fn namespace_of(target: &str) -> Option<String> {
    if target.starts_with(':') {
        return None;
    }
    let (before, _) = target.split_once(':')?;
    Some(before.trim().to_lowercase())
}

// --- image embeds -------------------------------------------------------------------------------

/// The page name with a leading `namespace:` prefix removed.
fn strip_namespace(target: &str) -> &str {
    match target.split_once(':') {
        Some((_, rest)) => rest.trim(),
        None => target,
    }
}

/// Parses an image size parameter — `<w>px`, `x<h>px`, or `<w>x<h>px` — into its width and optional
/// height. The width is the digits before an `x` (empty when the form is `x<h>px`); the height is
/// the digits after it. Returns `None` for any parameter that is not a pixel size.
fn image_size(param: &str) -> Option<(String, Option<String>)> {
    let digits = param.strip_suffix("px")?;
    match digits.split_once('x') {
        Some((width, height)) => {
            let valid = width.chars().all(|c| c.is_ascii_digit())
                && !height.is_empty()
                && height.chars().all(|c| c.is_ascii_digit());
            valid.then(|| (width.to_string(), Some(height.to_string())))
        }
        None => (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
            .then(|| (digits.to_string(), None)),
    }
}

/// Whether an image parameter forces the embed to decline, so the markup becomes an ordinary
/// wikilink instead of an image. A `thumbtime` parameter (with or without a value) and an `upright`
/// parameter that carries an explicit value have no image representation; a bare `upright` keyword
/// is a normal sizing hint and does not decline.
fn image_param_declines(param: &str) -> bool {
    match param.split_once('=') {
        Some((key, _)) => {
            let key = key.trim().to_ascii_lowercase();
            key == "thumbtime" || key == "upright"
        }
        None => param.trim().eq_ignore_ascii_case("thumbtime"),
    }
}

/// Whether an image parameter is a recognized `key=value` attribute (`alt`, `link`, `class`,
/// `page`) that is consumed without contributing caption text. Any other `key=value` becomes
/// caption text.
fn is_recognized_image_attr(param: &str) -> bool {
    match param.split_once('=') {
        Some((key, _)) => matches!(
            key.trim().to_ascii_lowercase().as_str(),
            "alt" | "link" | "class" | "page"
        ),
        None => false,
    }
}

/// Whether an image parameter is a recognized placement, framing, or alignment keyword that
/// carries no caption text.
fn is_image_keyword(param: &str) -> bool {
    matches!(
        param.to_ascii_lowercase().as_str(),
        "thumb"
            | "thumbnail"
            | "frame"
            | "framed"
            | "frameless"
            | "border"
            | "left"
            | "right"
            | "center"
            | "centre"
            | "none"
            | "upright"
            | "baseline"
            | "sub"
            | "super"
            | "top"
            | "text-top"
            | "middle"
            | "bottom"
            | "text-bottom"
    )
}

/// Wraps a paragraph whose only content is an image in a figure, moving the image's description to
/// the figure caption; any other paragraph is returned unchanged.
fn para_or_figure(inlines: Vec<Inline>) -> Block {
    match lone_image_figure(&inlines) {
        Some(figure) => figure,
        None => Block::Para(inlines),
    }
}

/// As [`para_or_figure`], for a context (a list item) whose tight content is a [`Block::Plain`].
fn plain_or_figure(inlines: Vec<Inline>) -> Block {
    match lone_image_figure(&inlines) {
        Some(figure) => figure,
        None => Block::Plain(inlines),
    }
}

/// Builds a figure from a paragraph that holds a single image (ignoring surrounding whitespace),
/// or `None` when the paragraph is anything else.
fn lone_image_figure(inlines: &[Inline]) -> Option<Block> {
    let mut significant = inlines.iter().filter(|inline| {
        !matches!(
            inline,
            Inline::Space | Inline::SoftBreak | Inline::LineBreak
        )
    });
    let Inline::Image(attr, alt, target) = significant.next()? else {
        return None;
    };
    if significant.next().is_some() {
        return None;
    }
    let caption = Caption {
        short: None,
        long: vec![Block::Plain(alt.clone())],
    };
    let image = Inline::Image(attr.clone(), Vec::new(), target.clone());
    Some(Block::Figure(
        Box::default(),
        Box::new(caption),
        vec![Block::Plain(vec![image])],
    ))
}

// --- identifiers --------------------------------------------------------------------------------

/// Under the `gfm_auto_identifiers` scheme each emoji that has a known shortname contributes that
/// name to the identifier in place of the raw character. Spans of text with no emoji pass through
/// unchanged; the shortname is spliced in directly, without inserting word boundaries.
fn emoji_to_aliases(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some((alias, len)) = emoji::alias_at(rest) {
            out.push_str(alias);
            rest = rest.get(len..).unwrap_or("");
        } else if let Some(ch) = rest.chars().next() {
            out.push(ch);
            rest = rest.get(ch.len_utf8()..).unwrap_or("");
        } else {
            break;
        }
    }
    out
}

/// Builds a heading identifier under the `auto_identifiers` scheme: lowercase, keep alphanumerics
/// with `_` and `.`, collapse each whitespace run to a single `_`, turn each hyphen into its own
/// `_`, drop other punctuation without breaking an adjacent whitespace run, and strip a leading run
/// of non-letters.
fn mediawiki_slug(text: &str) -> String {
    let mut out = String::new();
    let mut in_ws = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push('_');
                in_ws = true;
            }
        } else if ch == '-' {
            out.push('_');
            in_ws = false;
        } else if ch.is_alphanumeric() || ch == '_' || ch == '.' {
            out.extend(ch.to_lowercase());
            in_ws = false;
        }
        // Other punctuation is transparent: it emits nothing and leaves a running whitespace
        // collapse intact, so `Foo : Bar` and `Foo  Bar` both yield a single separating `_`.
    }
    out.chars().skip_while(|c| !c.is_alphabetic()).collect()
}

/// Transliterates text to ASCII for `ascii_identifiers`: an ASCII character is kept as is, a
/// character whose canonical decomposition begins with an ASCII letter or digit folds to that
/// character (so `é` becomes `e`), and any other non-ASCII character is dropped (so `Œ`, `ß`, and
/// `½` vanish). The result is then slugged like any other identifier.
fn transliterate_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else if let Ok(index) = ASCII_FOLD.binary_search_by(|&(cp, _)| cp.cmp(&(ch as u32)))
            && let Some(&(_, byte)) = ASCII_FOLD.get(index)
        {
            out.push(byte as char);
        }
    }
    out
}

/// The ASCII fold for `ascii_identifiers`, keyed by Unicode code point and kept sorted for binary
/// search. Each entry maps a precomposed character to the ASCII letter or digit its canonical
/// decomposition begins with; characters with no ASCII base are absent and are dropped instead.
const ASCII_FOLD: &[(u32, u8)] = &[
    (0x00C0, b'a'),
    (0x00C1, b'a'),
    (0x00C2, b'a'),
    (0x00C3, b'a'),
    (0x00C4, b'a'),
    (0x00C5, b'a'),
    (0x00C7, b'c'),
    (0x00C8, b'e'),
    (0x00C9, b'e'),
    (0x00CA, b'e'),
    (0x00CB, b'e'),
    (0x00CC, b'i'),
    (0x00CD, b'i'),
    (0x00CE, b'i'),
    (0x00CF, b'i'),
    (0x00D1, b'n'),
    (0x00D2, b'o'),
    (0x00D3, b'o'),
    (0x00D4, b'o'),
    (0x00D5, b'o'),
    (0x00D6, b'o'),
    (0x00D9, b'u'),
    (0x00DA, b'u'),
    (0x00DB, b'u'),
    (0x00DC, b'u'),
    (0x00DD, b'y'),
    (0x00E0, b'a'),
    (0x00E1, b'a'),
    (0x00E2, b'a'),
    (0x00E3, b'a'),
    (0x00E4, b'a'),
    (0x00E5, b'a'),
    (0x00E7, b'c'),
    (0x00E8, b'e'),
    (0x00E9, b'e'),
    (0x00EA, b'e'),
    (0x00EB, b'e'),
    (0x00EC, b'i'),
    (0x00ED, b'i'),
    (0x00EE, b'i'),
    (0x00EF, b'i'),
    (0x00F1, b'n'),
    (0x00F2, b'o'),
    (0x00F3, b'o'),
    (0x00F4, b'o'),
    (0x00F5, b'o'),
    (0x00F6, b'o'),
    (0x00F9, b'u'),
    (0x00FA, b'u'),
    (0x00FB, b'u'),
    (0x00FC, b'u'),
    (0x00FD, b'y'),
    (0x00FF, b'y'),
    (0x0100, b'a'),
    (0x0101, b'a'),
    (0x0102, b'a'),
    (0x0103, b'a'),
    (0x0104, b'a'),
    (0x0105, b'a'),
    (0x0106, b'c'),
    (0x0107, b'c'),
    (0x0108, b'c'),
    (0x0109, b'c'),
    (0x010A, b'c'),
    (0x010B, b'c'),
    (0x010C, b'c'),
    (0x010D, b'c'),
    (0x010E, b'd'),
    (0x010F, b'd'),
    (0x0112, b'e'),
    (0x0113, b'e'),
    (0x0114, b'e'),
    (0x0115, b'e'),
    (0x0116, b'e'),
    (0x0117, b'e'),
    (0x0118, b'e'),
    (0x0119, b'e'),
    (0x011A, b'e'),
    (0x011B, b'e'),
    (0x011C, b'g'),
    (0x011D, b'g'),
    (0x011E, b'g'),
    (0x011F, b'g'),
    (0x0120, b'g'),
    (0x0121, b'g'),
    (0x0122, b'g'),
    (0x0123, b'g'),
    (0x0124, b'h'),
    (0x0125, b'h'),
    (0x0128, b'i'),
    (0x0129, b'i'),
    (0x012A, b'i'),
    (0x012B, b'i'),
    (0x012C, b'i'),
    (0x012D, b'i'),
    (0x012E, b'i'),
    (0x012F, b'i'),
    (0x0130, b'i'),
    (0x0134, b'j'),
    (0x0135, b'j'),
    (0x0136, b'k'),
    (0x0137, b'k'),
    (0x0139, b'l'),
    (0x013A, b'l'),
    (0x013B, b'l'),
    (0x013C, b'l'),
    (0x013D, b'l'),
    (0x013E, b'l'),
    (0x0143, b'n'),
    (0x0144, b'n'),
    (0x0145, b'n'),
    (0x0146, b'n'),
    (0x0147, b'n'),
    (0x0148, b'n'),
    (0x014C, b'o'),
    (0x014D, b'o'),
    (0x014E, b'o'),
    (0x014F, b'o'),
    (0x0150, b'o'),
    (0x0151, b'o'),
    (0x0154, b'r'),
    (0x0155, b'r'),
    (0x0156, b'r'),
    (0x0157, b'r'),
    (0x0158, b'r'),
    (0x0159, b'r'),
    (0x015A, b's'),
    (0x015B, b's'),
    (0x015C, b's'),
    (0x015D, b's'),
    (0x015E, b's'),
    (0x015F, b's'),
    (0x0160, b's'),
    (0x0161, b's'),
    (0x0162, b't'),
    (0x0163, b't'),
    (0x0164, b't'),
    (0x0165, b't'),
    (0x0168, b'u'),
    (0x0169, b'u'),
    (0x016A, b'u'),
    (0x016B, b'u'),
    (0x016C, b'u'),
    (0x016D, b'u'),
    (0x016E, b'u'),
    (0x016F, b'u'),
    (0x0170, b'u'),
    (0x0171, b'u'),
    (0x0172, b'u'),
    (0x0173, b'u'),
    (0x0174, b'w'),
    (0x0175, b'w'),
    (0x0176, b'y'),
    (0x0177, b'y'),
    (0x0178, b'y'),
    (0x0179, b'z'),
    (0x017A, b'z'),
    (0x017B, b'z'),
    (0x017C, b'z'),
    (0x017D, b'z'),
    (0x017E, b'z'),
    (0x01A0, b'o'),
    (0x01A1, b'o'),
    (0x01AF, b'u'),
    (0x01B0, b'u'),
    (0x01CD, b'a'),
    (0x01CE, b'a'),
    (0x01CF, b'i'),
    (0x01D0, b'i'),
    (0x01D1, b'o'),
    (0x01D2, b'o'),
    (0x01D3, b'u'),
    (0x01D4, b'u'),
    (0x01D5, b'u'),
    (0x01D6, b'u'),
    (0x01D7, b'u'),
    (0x01D8, b'u'),
    (0x01D9, b'u'),
    (0x01DA, b'u'),
    (0x01DB, b'u'),
    (0x01DC, b'u'),
    (0x01DE, b'a'),
    (0x01DF, b'a'),
    (0x01E0, b'a'),
    (0x01E1, b'a'),
    (0x01E6, b'g'),
    (0x01E7, b'g'),
    (0x01E8, b'k'),
    (0x01E9, b'k'),
    (0x01EA, b'o'),
    (0x01EB, b'o'),
    (0x01EC, b'o'),
    (0x01ED, b'o'),
    (0x01F0, b'j'),
    (0x01F4, b'g'),
    (0x01F5, b'g'),
    (0x01F8, b'n'),
    (0x01F9, b'n'),
    (0x01FA, b'a'),
    (0x01FB, b'a'),
    (0x0200, b'a'),
    (0x0201, b'a'),
    (0x0202, b'a'),
    (0x0203, b'a'),
    (0x0204, b'e'),
    (0x0205, b'e'),
    (0x0206, b'e'),
    (0x0207, b'e'),
    (0x0208, b'i'),
    (0x0209, b'i'),
    (0x020A, b'i'),
    (0x020B, b'i'),
    (0x020C, b'o'),
    (0x020D, b'o'),
    (0x020E, b'o'),
    (0x020F, b'o'),
    (0x0210, b'r'),
    (0x0211, b'r'),
    (0x0212, b'r'),
    (0x0213, b'r'),
    (0x0214, b'u'),
    (0x0215, b'u'),
    (0x0216, b'u'),
    (0x0217, b'u'),
    (0x0218, b's'),
    (0x0219, b's'),
    (0x021A, b't'),
    (0x021B, b't'),
    (0x021E, b'h'),
    (0x021F, b'h'),
    (0x0226, b'a'),
    (0x0227, b'a'),
    (0x0228, b'e'),
    (0x0229, b'e'),
    (0x022A, b'o'),
    (0x022B, b'o'),
    (0x022C, b'o'),
    (0x022D, b'o'),
    (0x022E, b'o'),
    (0x022F, b'o'),
    (0x0230, b'o'),
    (0x0231, b'o'),
    (0x0232, b'y'),
    (0x0233, b'y'),
    (0x1E00, b'a'),
    (0x1E01, b'a'),
    (0x1E02, b'b'),
    (0x1E03, b'b'),
    (0x1E04, b'b'),
    (0x1E05, b'b'),
    (0x1E06, b'b'),
    (0x1E07, b'b'),
    (0x1E08, b'c'),
    (0x1E09, b'c'),
    (0x1E0A, b'd'),
    (0x1E0B, b'd'),
    (0x1E0C, b'd'),
    (0x1E0D, b'd'),
    (0x1E0E, b'd'),
    (0x1E0F, b'd'),
    (0x1E10, b'd'),
    (0x1E11, b'd'),
    (0x1E12, b'd'),
    (0x1E13, b'd'),
    (0x1E14, b'e'),
    (0x1E15, b'e'),
    (0x1E16, b'e'),
    (0x1E17, b'e'),
    (0x1E18, b'e'),
    (0x1E19, b'e'),
    (0x1E1A, b'e'),
    (0x1E1B, b'e'),
    (0x1E1C, b'e'),
    (0x1E1D, b'e'),
    (0x1E1E, b'f'),
    (0x1E1F, b'f'),
    (0x1E20, b'g'),
    (0x1E21, b'g'),
    (0x1E22, b'h'),
    (0x1E23, b'h'),
    (0x1E24, b'h'),
    (0x1E25, b'h'),
    (0x1E26, b'h'),
    (0x1E27, b'h'),
    (0x1E28, b'h'),
    (0x1E29, b'h'),
    (0x1E2A, b'h'),
    (0x1E2B, b'h'),
    (0x1E2C, b'i'),
    (0x1E2D, b'i'),
    (0x1E2E, b'i'),
    (0x1E2F, b'i'),
    (0x1E30, b'k'),
    (0x1E31, b'k'),
    (0x1E32, b'k'),
    (0x1E33, b'k'),
    (0x1E34, b'k'),
    (0x1E35, b'k'),
    (0x1E36, b'l'),
    (0x1E37, b'l'),
    (0x1E38, b'l'),
    (0x1E39, b'l'),
    (0x1E3A, b'l'),
    (0x1E3B, b'l'),
    (0x1E3C, b'l'),
    (0x1E3D, b'l'),
    (0x1E3E, b'm'),
    (0x1E3F, b'm'),
    (0x1E40, b'm'),
    (0x1E41, b'm'),
    (0x1E42, b'm'),
    (0x1E43, b'm'),
    (0x1E44, b'n'),
    (0x1E45, b'n'),
    (0x1E46, b'n'),
    (0x1E47, b'n'),
    (0x1E48, b'n'),
    (0x1E49, b'n'),
    (0x1E4A, b'n'),
    (0x1E4B, b'n'),
    (0x1E4C, b'o'),
    (0x1E4D, b'o'),
    (0x1E4E, b'o'),
    (0x1E4F, b'o'),
    (0x1E50, b'o'),
    (0x1E51, b'o'),
    (0x1E52, b'o'),
    (0x1E53, b'o'),
    (0x1E54, b'p'),
    (0x1E55, b'p'),
    (0x1E56, b'p'),
    (0x1E57, b'p'),
    (0x1E58, b'r'),
    (0x1E59, b'r'),
    (0x1E5A, b'r'),
    (0x1E5B, b'r'),
    (0x1E5C, b'r'),
    (0x1E5D, b'r'),
    (0x1E5E, b'r'),
    (0x1E5F, b'r'),
    (0x1E60, b's'),
    (0x1E61, b's'),
    (0x1E62, b's'),
    (0x1E63, b's'),
    (0x1E64, b's'),
    (0x1E65, b's'),
    (0x1E66, b's'),
    (0x1E67, b's'),
    (0x1E68, b's'),
    (0x1E69, b's'),
    (0x1E6A, b't'),
    (0x1E6B, b't'),
    (0x1E6C, b't'),
    (0x1E6D, b't'),
    (0x1E6E, b't'),
    (0x1E6F, b't'),
    (0x1E70, b't'),
    (0x1E71, b't'),
    (0x1E72, b'u'),
    (0x1E73, b'u'),
    (0x1E74, b'u'),
    (0x1E75, b'u'),
    (0x1E76, b'u'),
    (0x1E77, b'u'),
    (0x1E78, b'u'),
    (0x1E79, b'u'),
    (0x1E7A, b'u'),
    (0x1E7B, b'u'),
    (0x1E7C, b'v'),
    (0x1E7D, b'v'),
    (0x1E7E, b'v'),
    (0x1E7F, b'v'),
    (0x1E80, b'w'),
    (0x1E81, b'w'),
    (0x1E82, b'w'),
    (0x1E83, b'w'),
    (0x1E84, b'w'),
    (0x1E85, b'w'),
    (0x1E86, b'w'),
    (0x1E87, b'w'),
    (0x1E88, b'w'),
    (0x1E89, b'w'),
    (0x1E8A, b'x'),
    (0x1E8B, b'x'),
    (0x1E8C, b'x'),
    (0x1E8D, b'x'),
    (0x1E8E, b'y'),
    (0x1E8F, b'y'),
    (0x1E90, b'z'),
    (0x1E91, b'z'),
    (0x1E92, b'z'),
    (0x1E93, b'z'),
    (0x1E94, b'z'),
    (0x1E95, b'z'),
    (0x1E96, b'h'),
    (0x1E97, b't'),
    (0x1E98, b'w'),
    (0x1E99, b'y'),
    (0x1EA0, b'a'),
    (0x1EA1, b'a'),
    (0x1EA2, b'a'),
    (0x1EA3, b'a'),
    (0x1EA4, b'a'),
    (0x1EA5, b'a'),
    (0x1EA6, b'a'),
    (0x1EA7, b'a'),
    (0x1EA8, b'a'),
    (0x1EA9, b'a'),
    (0x1EAA, b'a'),
    (0x1EAB, b'a'),
    (0x1EAC, b'a'),
    (0x1EAD, b'a'),
    (0x1EAE, b'a'),
    (0x1EAF, b'a'),
    (0x1EB0, b'a'),
    (0x1EB1, b'a'),
    (0x1EB2, b'a'),
    (0x1EB3, b'a'),
    (0x1EB4, b'a'),
    (0x1EB5, b'a'),
    (0x1EB6, b'a'),
    (0x1EB7, b'a'),
    (0x1EB8, b'e'),
    (0x1EB9, b'e'),
    (0x1EBA, b'e'),
    (0x1EBB, b'e'),
    (0x1EBC, b'e'),
    (0x1EBD, b'e'),
    (0x1EBE, b'e'),
    (0x1EBF, b'e'),
    (0x1EC0, b'e'),
    (0x1EC1, b'e'),
    (0x1EC2, b'e'),
    (0x1EC3, b'e'),
    (0x1EC4, b'e'),
    (0x1EC5, b'e'),
    (0x1EC6, b'e'),
    (0x1EC7, b'e'),
    (0x1EC8, b'i'),
    (0x1EC9, b'i'),
    (0x1ECA, b'i'),
    (0x1ECB, b'i'),
    (0x1ECC, b'o'),
    (0x1ECD, b'o'),
    (0x1ECE, b'o'),
    (0x1ECF, b'o'),
    (0x1ED0, b'o'),
    (0x1ED1, b'o'),
    (0x1ED2, b'o'),
    (0x1ED3, b'o'),
    (0x1ED4, b'o'),
    (0x1ED5, b'o'),
    (0x1ED6, b'o'),
    (0x1ED7, b'o'),
    (0x1ED8, b'o'),
    (0x1ED9, b'o'),
    (0x1EDA, b'o'),
    (0x1EDB, b'o'),
    (0x1EDC, b'o'),
    (0x1EDD, b'o'),
    (0x1EDE, b'o'),
    (0x1EDF, b'o'),
    (0x1EE0, b'o'),
    (0x1EE1, b'o'),
    (0x1EE2, b'o'),
    (0x1EE3, b'o'),
    (0x1EE4, b'u'),
    (0x1EE5, b'u'),
    (0x1EE6, b'u'),
    (0x1EE7, b'u'),
    (0x1EE8, b'u'),
    (0x1EE9, b'u'),
    (0x1EEA, b'u'),
    (0x1EEB, b'u'),
    (0x1EEC, b'u'),
    (0x1EED, b'u'),
    (0x1EEE, b'u'),
    (0x1EEF, b'u'),
    (0x1EF0, b'u'),
    (0x1EF1, b'u'),
    (0x1EF2, b'y'),
    (0x1EF3, b'y'),
    (0x1EF4, b'y'),
    (0x1EF5, b'y'),
    (0x1EF6, b'y'),
    (0x1EF7, b'y'),
    (0x1EF8, b'y'),
    (0x1EF9, b'y'),
    (0x212A, b'k'),
    (0x212B, b'a'),
];

// --- tag scanning -------------------------------------------------------------------------------

/// Reads an opening tag at `chars[i]`, returning its lowercased name, the raw `<…>` text, whether it
/// is self-closing, and the index just past the `>`. Attribute values in quotes may contain `>`.
fn open_tag(chars: &[char], start: usize) -> Option<(String, String, bool, usize)> {
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
fn close_tag(chars: &[char], start: usize, name: &str) -> Option<(usize, usize)> {
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
fn enclosed(chars: &[char], start: usize, name: &str) -> (String, usize) {
    match close_tag(chars, start, name) {
        Some((inner_end, after)) => (collect_range(chars, start, inner_end), after),
        None => (collect_range(chars, start, chars.len()), chars.len()),
    }
}

fn tag_name_matches(chars: &[char], pos: usize, name: &str) -> bool {
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

fn starts_block_tag(chars: &[char], pos: usize) -> bool {
    if at(chars, pos) != Some('<') {
        return false;
    }
    ["pre", "source", "syntaxhighlight", "blockquote", "ul", "ol"]
        .iter()
        .any(|name| tag_name_matches(chars, pos + 1, name))
}

/// The count of `<ref>` tags opened but not yet closed within `chars[start..end]`. A self-closing
/// `<ref … />` opens nothing; verbatim regions are stepped over so a `<ref>` inside `<nowiki>` does
/// not count. Used to keep a paragraph open until a `<ref>` note's body is complete.
fn open_ref_depth(chars: &[char], start: usize, end: usize) -> i32 {
    let mut depth = 0i32;
    let mut i = start;
    while i < end {
        if at(chars, i) == Some('<') {
            if let Some(after) = verbatim_region_end(chars, i) {
                i = after;
                continue;
            }
            if at(chars, i + 1) == Some('/') {
                if tag_name_matches(chars, i + 2, "ref") {
                    depth = (depth - 1).max(0);
                }
            } else if tag_name_matches(chars, i + 1, "ref")
                && let Some((_, _, self_closing, after)) = open_tag(chars, i)
            {
                if !self_closing {
                    depth += 1;
                }
                i = after;
                continue;
            }
        }
        i += 1;
    }
    depth
}

/// Whether the innermost `<ref>` still open at `end` has a body that begins on a fresh line — its
/// open tag is the last non-blank thing on its line. Such a note is read as block content, so its
/// body may hold lists and other block constructs; a note opened with text on the same line reads as
/// inline content and a following block-level line ends it instead of joining it.
fn open_ref_block_bodied(chars: &[char], start: usize, end: usize) -> bool {
    let mut stack: Vec<bool> = Vec::new();
    let mut i = start;
    while i < end {
        if at(chars, i) == Some('<') {
            if let Some(after) = verbatim_region_end(chars, i) {
                i = after;
                continue;
            }
            if at(chars, i + 1) == Some('/') {
                if tag_name_matches(chars, i + 2, "ref") {
                    stack.pop();
                }
            } else if tag_name_matches(chars, i + 1, "ref")
                && let Some((_, _, self_closing, after)) = open_tag(chars, i)
            {
                if !self_closing {
                    let mut j = after;
                    while matches!(at(chars, j), Some(' ' | '\t')) {
                        j += 1;
                    }
                    stack.push(matches!(at(chars, j), None | Some('\n')));
                }
                i = after;
                continue;
            }
        }
        i += 1;
    }
    stack.last().copied().unwrap_or(false)
}

/// The role of a recognized HTML element, or `None` when the name is not a recognized HTML tag (in
/// which case the surrounding `<…>` stays literal text).
fn html_tag_role(name: &str) -> Option<HtmlTagRole> {
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
fn close_tag_parse(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    if at(chars, i) != Some('<') || at(chars, i + 1) != Some('/') {
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
fn html_li_content_bounds(chars: &[char], start: usize) -> (usize, usize) {
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
                    if let Some((_, _, after)) = close_tag_parse(chars, j) {
                        j = after;
                        continue;
                    }
                } else if list_depth == 0
                    && tag_name_matches(chars, j + 2, "li")
                    && let Some((_, _, after)) = close_tag_parse(chars, j)
                {
                    return (j, after);
                }
            } else if tag_name_matches(chars, j + 1, "ul") || tag_name_matches(chars, j + 1, "ol") {
                if let Some((_, _, self_closing, after)) = open_tag(chars, j) {
                    if !self_closing {
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
fn block_tag_token(chars: &[char], i: usize) -> Option<(Tok, usize)> {
    let (name, raw, after) = if at(chars, i + 1) == Some('/') {
        close_tag_parse(chars, i)?
    } else {
        let (name, raw, _self_closing, after) = open_tag(chars, i)?;
        (name, raw, after)
    };
    match html_tag_role(&name)? {
        HtmlTagRole::Block => Some((Tok::BlockRaw(raw), after)),
        HtmlTagRole::Break => Some((Tok::BlockBreak, after)),
        HtmlTagRole::Inline => None,
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

/// Reads the value of `key` from a raw tag string, accepting quoted or bare values.
fn tag_attribute(raw: &str, key: &str) -> Option<String> {
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

// --- line classification ------------------------------------------------------------------------

/// Memo tables for the heading-region lookahead, keyed by the starting char index within one
/// `chars` slice. A heading's text runs until the next line that opens a block, and deciding
/// whether a `=`-prefixed line opens its own heading needs that same lookahead, so region end and
/// header-ness are mutually recursive. Every recursive step advances to a strictly later line, so
/// the recursion always terminates on its own — but without memoization each line's region would be
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
    // The region end of a line depends only on lines that come after it, so resolve them
    // back-to-front. First gather the forward run of consecutive non-blank line starts beginning at
    // `pos` (the run stops at the first blank line or the end of input); then fill the memo from the
    // last line to the first. Resolving bottom-up keeps the mutual recursion between region-end and
    // header-ness at constant stack depth no matter how many `=`-prefixed lines are stacked — a
    // naive recursive walk would instead recurse once per line and overflow the stack on adversarial
    // input.
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
                // `next` is the following run element (already resolved) or a blank/EOF line, so
                // `line_starts_block_scan` and the recursive lookup below both hit the memo.
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

/// Splits a definition term at the first top-level `:`, skipping constructs so a `:` inside a link
/// or template is not treated as the separator.
fn split_term(content: &str) -> (String, Option<String>) {
    let chars: Vec<char> = content.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if let Some(next) = skip_construct(&chars, i)
            && next > i
        {
            i = next;
            continue;
        }
        // A bare URL is stepped over whole so the `:` in its scheme is not read as the separator.
        if let Some((_, next)) = bare_url(&chars, i)
            && next > i
        {
            i = next;
            continue;
        }
        if at(&chars, i) == Some(':') {
            let before = collect_range(&chars, 0, i).trim().to_string();
            let after = collect_range(&chars, i + 1, n).trim().to_string();
            return (before, Some(after));
        }
        i += 1;
    }
    (content.trim().to_string(), None)
}

/// If an inline construct opens at `i`, the index just past it: `{{…}}`, `[[…]]`, `[…]`, or `<…>`.
fn skip_construct(chars: &[char], i: usize) -> Option<usize> {
    match at(chars, i) {
        Some('{') if at(chars, i + 1) == Some('{') => balanced_braces(chars, i),
        Some('[') if at(chars, i + 1) == Some('[') => {
            find_seq(chars, i + 2, &[']', ']']).map(|c| c + 2)
        }
        Some('[') => find_char(chars, i + 1, ']').map(|c| c + 1),
        Some('<') => find_char(chars, i, '>').map(|c| c + 1),
        _ => None,
    }
}

/// The index just past the `}}` that balances the `{{` at `i`, accounting for nesting.
/// Whether the `{{` at `i` opens a template transclusion. A template name begins with a letter, a
/// digit, or a `:` (a leading-colon main-namespace reference); a `{{` followed by anything else —
/// whitespace, a parser-function `#`, a pipe, or `}}` — is literal braces, not a template.
fn template_opens(chars: &[char], i: usize) -> bool {
    matches!(at(chars, i + 2), Some(c) if c.is_alphanumeric() || c == ':')
}

fn balanced_braces(chars: &[char], i: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut j = i;
    let n = chars.len();
    while j < n {
        if at(chars, j) == Some('{') && at(chars, j + 1) == Some('{') {
            depth += 1;
            j += 2;
        } else if at(chars, j) == Some('}') && at(chars, j + 1) == Some('}') {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return Some(j);
            }
        } else {
            j += 1;
        }
    }
    None
}

// --- small helpers ------------------------------------------------------------------------------

fn is_list_marker(c: char) -> bool {
    matches!(c, '*' | '#' | ':' | ';')
}

/// Flat fallback used when block nesting reaches [`MAX_BLOCK_DEPTH`]: the remaining text becomes a
/// single paragraph of its literal content, with no further block structure parsed, so deeply
/// stacked constructs cannot exhaust the stack during parsing or serialization.
fn degraded_blocks(chars: &[char]) -> Vec<Block> {
    let text = collect_range(chars, 0, chars.len());
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Vec::new()
    } else {
        vec![Block::Para(vec![Inline::Str(trimmed.into())])]
    }
}

/// Consumes the whitespace run beginning at `from`, returning a single break token (soft when the
/// run spans a newline, otherwise a space) and the index just past the run.
fn whitespace_token(chars: &[char], from: usize) -> (Inline, usize) {
    let mut i = from;
    let mut has_newline = false;
    while let Some(w) = at(chars, i) {
        if w.is_whitespace() {
            if w == '\n' {
                has_newline = true;
            }
            i += 1;
        } else {
            break;
        }
    }
    let token = if has_newline {
        Inline::SoftBreak
    } else {
        Inline::Space
    };
    (token, i)
}

fn list_kind(marker: char) -> ListKind {
    match marker {
        '#' => ListKind::Ordered,
        ';' | ':' => ListKind::Definition,
        _ => ListKind::Bullet,
    }
}

/// Parses `<code>`-family verbatim content into a [`Inline::Code`] node carrying `classes`, with
/// entity references decoded. An unterminated tag degrades to its literal opening as raw HTML.
fn verbatim_code(
    chars: &[char],
    name: &str,
    after_open: usize,
    raw_open: &str,
    self_closing: bool,
    classes: &[&str],
) -> (Vec<Inline>, usize) {
    if self_closing {
        return (vec![raw_html(raw_open.to_string())], after_open);
    }
    match close_tag(chars, after_open, name) {
        Some((inner_end, after)) => {
            let inner = collect_range(chars, after_open, inner_end);
            let attr = Attr {
                id: carta_ast::Text::default(),
                classes: classes.iter().map(|s| (*s).into()).collect(),
                attributes: Vec::new(),
            };
            (
                vec![Inline::Code(Box::new(attr), decode_entities(&inner).into())],
                after,
            )
        }
        None => (vec![raw_html(raw_open.to_string())], after_open),
    }
}

fn default_list_attrs() -> ListAttributes {
    ListAttributes {
        start: 1,
        style: ListNumberStyle::DefaultStyle,
        delim: ListNumberDelim::DefaultDelim,
    }
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

fn flush_word(word: &mut String, toks: &mut Vec<Tok>) {
    if !word.is_empty() {
        toks.push(Tok::Inline(Inline::Str(std::mem::take(word).into())));
    }
}

fn raw_html(text: String) -> Inline {
    Inline::RawInline(Format("html".into()), text.into())
}

fn format_mediawiki() -> Format {
    Format("mediawiki".into())
}

fn format_html() -> Format {
    Format("html".into())
}

fn at(chars: &[char], i: usize) -> Option<char> {
    chars.get(i).copied()
}

fn collect_range(chars: &[char], start: usize, end: usize) -> String {
    if end <= start {
        return String::new();
    }
    chars.iter().skip(start).take(end - start).collect()
}

/// Finds the index one past the end of a table block opening with `{|` at `pos`. Opening (`{|`) and
/// closing (`|}`) markers are matched by depth, scanning whole lines, so a nested table does not
/// close the outer one early; an unterminated table runs to the end of input.
fn table_block_end(chars: &[char], pos: usize) -> usize {
    let n = chars.len();
    let mut depth = 0usize;
    let mut line = pos;
    loop {
        let mut content = line;
        while matches!(at(chars, content), Some(' ' | '\t')) {
            content += 1;
        }
        if at(chars, content) == Some('{') && at(chars, content + 1) == Some('|') {
            depth += 1;
        } else if at(chars, content) == Some('|') && at(chars, content + 1) == Some('}') {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return content + 2;
            }
        }
        let le = line_end(chars, line);
        if le >= n {
            return n;
        }
        line = le + 1;
    }
}

/// The number of grid columns a cell spans, never less than one.
/// Scans the body of a `{|…|}` region into its rows of raw cells and an optional caption.
/// Each `|-` closes the current row; nested tables are passed through verbatim as cell content.
fn scan_table_region(region: &str) -> (Vec<Vec<RawCell>>, Option<String>) {
    let mut caption_text: Option<String> = None;
    let mut rows: Vec<Vec<RawCell>> = Vec::new();
    let mut cur: Vec<RawCell> = Vec::new();
    let mut open = OpenTarget::None;
    let mut nest = 0i32;

    let mut lines = region.lines();
    lines.next(); // The opening `{|` line; any table attribute list it carries is dropped.
    for line in lines {
        let trimmed = line.trim_start();
        if nest > 0 {
            if trimmed.starts_with("{|") {
                nest += 1;
            } else if trimmed.starts_with("|}") {
                nest -= 1;
            }
            append_continuation(open, &mut cur, &mut caption_text, line);
            continue;
        }
        if trimmed.starts_with("|}") {
            break;
        }
        if trimmed.starts_with("{|") {
            nest += 1;
            append_continuation(open, &mut cur, &mut caption_text, line);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("|+") {
            caption_text = Some(rest.to_string());
            open = OpenTarget::Caption;
            continue;
        }
        if trimmed.starts_with("|-") {
            rows.push(std::mem::take(&mut cur));
            open = OpenTarget::None;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('|') {
            cur.extend(parse_cell_line(false, rest));
            open = OpenTarget::Cell;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('!') {
            cur.extend(parse_cell_line(true, rest));
            open = OpenTarget::Cell;
            continue;
        }
        append_continuation(open, &mut cur, &mut caption_text, line);
    }
    rows.push(cur);
    (rows, caption_text)
}

/// Builds the column specifications from the first row, taking each column's alignment from the
/// cell that opens it and defaulting every column's width.
fn column_specs(rows: &[Vec<RawCell>], ncols: usize) -> Vec<ColSpec> {
    let mut aligns: Vec<Alignment> = Vec::new();
    if let Some(first) = rows.first() {
        for cell in first {
            for _ in 0..col_count(cell.col_span) {
                aligns.push(cell.align.clone());
            }
        }
    }
    aligns.resize(ncols, Alignment::AlignDefault);
    aligns
        .into_iter()
        .map(|align| ColSpec {
            align,
            width: ColWidth::ColWidthDefault,
        })
        .collect()
}

fn col_count(col_span: i32) -> usize {
    usize::try_from(col_span.max(1)).unwrap_or(1)
}

/// A blank single-column cell used to fill a row that covers fewer columns than the table is wide.
fn empty_cell() -> Cell {
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content: Vec::new(),
    }
}

/// Appends a table continuation line to whichever construct is currently open.
fn append_continuation(
    open: OpenTarget,
    cur: &mut [RawCell],
    caption: &mut Option<String>,
    line: &str,
) {
    match open {
        OpenTarget::Cell => {
            if let Some(cell) = cur.last_mut() {
                cell.content.push('\n');
                cell.content.push_str(line);
            }
        }
        OpenTarget::Caption => {
            if let Some(text) = caption {
                text.push('\n');
                text.push_str(line);
            }
        }
        OpenTarget::None => {}
    }
}

/// Splits one cell-marker line into its cells. A `|` data line separates cells with `||`; a `!`
/// header line additionally separates them with `!!`.
fn parse_cell_line(is_header: bool, rest: &str) -> Vec<RawCell> {
    split_cells(rest, is_header)
        .iter()
        .map(|chunk| parse_cell_chunk(is_header, chunk))
        .collect()
}

/// Splits a marker line's text into per-cell chunks at top-level `||` (and, for a header line, `!!`)
/// separators, leaving separators inside `[…]` or `{…}` groups untouched.
fn split_cells(s: &str, header: bool) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut square = 0i32;
    let mut curly = 0i32;
    let mut i = 0usize;
    while i < n {
        match at(&chars, i) {
            Some('[') => square += 1,
            Some(']') => square = (square - 1).max(0),
            Some('{') => curly += 1,
            Some('}') => curly = (curly - 1).max(0),
            _ => {}
        }
        if square == 0 && curly == 0 {
            let pipe = at(&chars, i) == Some('|') && at(&chars, i + 1) == Some('|');
            let bang = header && at(&chars, i) == Some('!') && at(&chars, i + 1) == Some('!');
            if pipe || bang {
                out.push(collect_range(&chars, start, i));
                i += 2;
                start = i;
                continue;
            }
        }
        i += 1;
    }
    out.push(collect_range(&chars, start, n));
    out
}

/// Parses one cell chunk into a [`RawCell`], splitting a leading attribute list from the content at
/// the first top-level `|` when the text before it is a valid attribute list.
fn parse_cell_chunk(is_header: bool, chunk: &str) -> RawCell {
    if let Some(idx) = find_attr_pipe(chunk)
        && let Some(attrs) = parse_cell_attrs(chunk.get(..idx).unwrap_or(""))
    {
        return RawCell {
            is_header,
            align: attrs.align,
            col_span: attrs.col_span,
            row_span: attrs.row_span,
            attr: attrs.attr,
            content: chunk.get(idx + 1..).unwrap_or("").to_string(),
        };
    }
    RawCell {
        is_header,
        align: Alignment::AlignDefault,
        col_span: 1,
        row_span: 1,
        attr: Attr::default(),
        content: chunk.to_string(),
    }
}

/// Finds the byte offset of the first top-level `|` in a cell chunk — the boundary between a leading
/// attribute list and the cell content — skipping any `|` inside `[…]` or `{…}` groups.
fn find_attr_pipe(s: &str) -> Option<usize> {
    let mut square = 0i32;
    let mut curly = 0i32;
    let mut in_quote = false;
    for (i, ch) in s.char_indices() {
        if in_quote {
            if ch == '"' {
                in_quote = false;
            }
            continue;
        }
        match ch {
            '"' => in_quote = true,
            '[' => square += 1,
            ']' => square = (square - 1).max(0),
            '{' => curly += 1,
            '}' => curly = (curly - 1).max(0),
            '|' if square == 0 && curly == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Parses a cell's leading attribute list. `align` maps to a column alignment, `colspan`/`rowspan`
/// to spans, `id`/`class` to the cell's identifier and classes, and everything else to a key/value
/// attribute. A bare token without a value is not a valid attribute list, so the whole text is
/// content instead — signalled by [`None`].
fn parse_cell_attrs(s: &str) -> Option<CellAttrs> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0usize;
    let mut id = String::new();
    let mut classes: Vec<String> = Vec::new();
    let mut attributes: Vec<(String, String)> = Vec::new();
    let mut align = Alignment::AlignDefault;
    let mut col_span = 1i32;
    let mut row_span = 1i32;
    let mut any = false;
    while i < n {
        while at(&chars, i).is_some_and(char::is_whitespace) {
            i += 1;
        }
        if i >= n {
            break;
        }
        let name_start = i;
        while at(&chars, i).is_some_and(|c| !c.is_whitespace() && c != '=') {
            i += 1;
        }
        let name = collect_range(&chars, name_start, i);
        if name.is_empty() || at(&chars, i) != Some('=') {
            return None;
        }
        i += 1;
        let value = if at(&chars, i) == Some('"') {
            i += 1;
            let value_start = i;
            while at(&chars, i).is_some_and(|c| c != '"') {
                i += 1;
            }
            let value = collect_range(&chars, value_start, i);
            if at(&chars, i) == Some('"') {
                i += 1;
            }
            value
        } else {
            let value_start = i;
            while at(&chars, i).is_some_and(|c| !c.is_whitespace()) {
                i += 1;
            }
            collect_range(&chars, value_start, i)
        };
        any = true;
        match name.to_ascii_lowercase().as_str() {
            "id" => id = value,
            "class" => classes.extend(value.split_whitespace().map(str::to_string)),
            "align" => match value.to_ascii_lowercase().as_str() {
                "left" => align = Alignment::AlignLeft,
                "right" => align = Alignment::AlignRight,
                "center" => align = Alignment::AlignCenter,
                _ => attributes.push(("align".to_string(), value)),
            },
            "colspan" => match value.trim().parse::<i32>() {
                Ok(v) if v >= 1 => col_span = v,
                _ => attributes.push(("colspan".to_string(), value)),
            },
            "rowspan" => match value.trim().parse::<i32>() {
                Ok(v) if v >= 1 => row_span = v,
                _ => attributes.push(("rowspan".to_string(), value)),
            },
            _ => attributes.push((name, value)),
        }
    }
    if !any {
        return None;
    }
    Some(CellAttrs {
        align,
        col_span,
        row_span,
        attr: Attr {
            id: id.into(),
            classes: classes.into_iter().map(Into::into).collect(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        },
    })
}

fn line_end(chars: &[char], pos: usize) -> usize {
    find_char(chars, pos, '\n').unwrap_or(chars.len())
}

fn is_blank(chars: &[char], start: usize, end: usize) -> bool {
    (start..end).all(|j| at(chars, j).is_none_or(char::is_whitespace))
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&j| at(chars, j) == Some(target))
}

fn find_seq(chars: &[char], from: usize, seq: &[char]) -> Option<usize> {
    let n = chars.len();
    let m = seq.len();
    if m == 0 || n < m {
        return None;
    }
    (from..=n - m).find(|&j| (0..m).all(|k| at(chars, j + k) == seq.get(k).copied()))
}

/// Scans an internal link's target from `start`: it ends at the first `|` or the first `]]`,
/// whichever comes first, with no nesting tracked. Returns the end index and whether a `|` (rather
/// than `]]`) was the delimiter, or `None` if neither appears.
fn scan_link_target(chars: &[char], start: usize) -> Option<(usize, bool)> {
    let mut i = start;
    while let Some(c) = at(chars, i) {
        if c == '|' {
            return Some((i, true));
        }
        if c == ']' && at(chars, i + 1) == Some(']') {
            return Some((i, false));
        }
        i += 1;
    }
    None
}

/// Finds the `]]` that closes an internal link whose label may hold nested `[[ … ]]` links, stepping
/// over each balanced inner pair so only the outer close is returned.
fn find_link_close(chars: &[char], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = start;
    while let Some(c) = at(chars, i) {
        if c == '[' && at(chars, i + 1) == Some('[') {
            depth += 1;
            i += 2;
        } else if c == ']' && at(chars, i + 1) == Some(']') {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}

fn matches_prefix_ci(chars: &[char], i: usize, prefix: &str) -> bool {
    prefix
        .chars()
        .enumerate()
        .all(|(k, pc)| match at(chars, i + k) {
            Some(c) => c.eq_ignore_ascii_case(&pc),
            None => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Vec<Block> {
        let mut options = ReaderOptions::default();
        options.extensions = Extensions::from_list(&[Extension::AutoIdentifiers]);
        MediawikiReader
            .read(input, &options)
            .expect("read should not fail")
            .blocks
    }

    fn parse_gfm(input: &str) -> Vec<Block> {
        let mut options = ReaderOptions::default();
        options.extensions = Extensions::from_list(&[Extension::GfmAutoIdentifiers]);
        MediawikiReader.read(input, &options).expect("read").blocks
    }

    #[test]
    fn doi_and_javascript_are_recognized_schemes() {
        assert!(is_scheme("doi"));
        assert!(is_scheme("javascript"));
        assert!(is_scheme("DOI"));
        assert!(is_scheme("http"));
        assert!(!is_scheme("notascheme"));
    }

    fn cell_with(content: Vec<Block>) -> Cell {
        Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content,
        }
    }

    fn data_cell(text: &str) -> Cell {
        cell_with(vec![Block::Para(vec![Inline::Str(text.into())])])
    }

    fn table_row(cells: Vec<Cell>) -> Row {
        Row {
            attr: Attr::default(),
            cells,
        }
    }

    fn default_col() -> ColSpec {
        ColSpec {
            align: Alignment::AlignDefault,
            width: ColWidth::ColWidthDefault,
        }
    }

    #[test]
    fn table_markup_becomes_a_table() {
        assert_eq!(
            parse("{|\n! Header\n|-\n| Cell\n|}\nafter"),
            vec![
                Block::Table(Box::new(Table {
                    col_specs: vec![default_col()],
                    head: TableHead {
                        rows: vec![table_row(vec![data_cell("Header")])],
                        ..Default::default()
                    },
                    bodies: vec![TableBody {
                        body: vec![table_row(vec![data_cell("Cell")])],
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                Block::Para(vec![Inline::Str("after".into())]),
            ]
        );
    }

    #[test]
    fn unterminated_table_markup_does_not_panic() {
        assert_eq!(
            parse("{|"),
            vec![Block::Table(Box::new(Table {
                bodies: vec![TableBody {
                    body: vec![table_row(Vec::new())],
                    ..Default::default()
                }],
                ..Default::default()
            }))]
        );
    }

    #[test]
    fn nested_table_markup_closes_at_the_outer_marker() {
        let inner = Block::Table(Box::new(Table {
            col_specs: vec![default_col()],
            bodies: vec![TableBody {
                body: vec![table_row(vec![data_cell("inner")])],
                ..Default::default()
            }],
            ..Default::default()
        }));
        assert_eq!(
            parse("{|\n|\n{|\n| inner\n|}\n|}"),
            vec![Block::Table(Box::new(Table {
                col_specs: vec![default_col()],
                bodies: vec![TableBody {
                    body: vec![table_row(vec![cell_with(vec![inner])])],
                    ..Default::default()
                }],
                ..Default::default()
            }))]
        );
    }

    #[test]
    fn paragraph_joins_lines_with_soft_breaks() {
        assert_eq!(
            parse("one two\nthree"),
            vec![Block::Para(vec![
                Inline::Str("one".into()),
                Inline::Space,
                Inline::Str("two".into()),
                Inline::SoftBreak,
                Inline::Str("three".into()),
            ])]
        );
    }

    #[test]
    fn emphasis_runs_decompose() {
        assert_eq!(
            parse("''i'' '''b''' '''''both'''''"),
            vec![Block::Para(vec![
                Inline::Emph(vec![Inline::Str("i".into())]),
                Inline::Space,
                Inline::Strong(vec![Inline::Str("b".into())]),
                Inline::Space,
                Inline::Strong(vec![Inline::Emph(vec![Inline::Str("both".into())])]),
            ])]
        );
    }

    #[test]
    fn header_carries_mediawiki_identifier() {
        assert_eq!(
            parse("== Hello World =="),
            vec![Block::Header(
                2,
                Box::new(Attr {
                    id: "hello_world".into(),
                    classes: vec![],
                    attributes: vec![],
                }),
                vec![
                    Inline::Str("Hello".into()),
                    Inline::Space,
                    Inline::Str("World".into()),
                ],
            )]
        );
    }

    #[test]
    fn duplicate_identifiers_are_suffixed() {
        let blocks = parse("== Dup ==\n== Dup ==");
        let ids: Vec<String> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Header(_, attr, _) => Some(attr.id.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["dup".to_string(), "dup_1".to_string()]);
    }

    #[test]
    fn gfm_identifier_scheme_uses_hyphens() {
        let blocks = parse_gfm("== Hello World ==");
        match blocks.first() {
            Some(Block::Header(_, attr, _)) => assert_eq!(attr.id, "hello-world"),
            other => panic!("expected header, got {other:?}"),
        }
    }

    #[test]
    fn empty_identifier_falls_back_to_section() {
        let blocks = parse("== !!! ==\n== ??? ==");
        let ids: Vec<String> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Header(_, attr, _) => Some(attr.id.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["section".to_string(), "section_1".to_string()]);
    }

    #[test]
    fn malformed_header_is_a_paragraph() {
        assert_eq!(
            parse("== a=b =="),
            vec![Block::Para(vec![
                Inline::Str("==".into()),
                Inline::Space,
                Inline::Str("a=b".into()),
                Inline::Space,
                Inline::Str("==".into()),
            ])]
        );
    }

    #[test]
    fn header_leftover_becomes_paragraph() {
        assert_eq!(
            parse("== H ==="),
            vec![
                Block::Header(
                    2,
                    Box::new(Attr {
                        id: "h".into(),
                        classes: vec![],
                        attributes: vec![],
                    }),
                    vec![Inline::Str("H".into())],
                ),
                Block::Para(vec![Inline::Str("=".into())]),
            ]
        );
    }

    #[test]
    fn nested_bullets_and_ordered() {
        assert_eq!(
            parse("* a\n** b\n*# c"),
            vec![Block::BulletList(vec![vec![
                Block::Plain(vec![Inline::Str("a".into())]),
                Block::BulletList(vec![vec![Block::Plain(vec![Inline::Str("b".into())])]]),
                Block::OrderedList(
                    default_list_attrs(),
                    vec![vec![Block::Plain(vec![Inline::Str("c".into())])]]
                ),
            ]])]
        );
    }

    #[test]
    fn definition_list_splits_inline_definition() {
        assert_eq!(
            parse("; term : def"),
            vec![Block::DefinitionList(vec![(
                vec![Inline::Str("term".into())],
                vec![vec![Block::Plain(vec![Inline::Str("def".into())])]],
            )])]
        );
    }

    #[test]
    fn internal_link_with_trail() {
        assert_eq!(
            parse("[[Page]]s"),
            vec![Block::Para(vec![Inline::Link(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["wikilink".into()],
                    attributes: vec![],
                }),
                vec![Inline::Str("Pages".into())],
                Box::new(Target {
                    url: "Page".into(),
                    title: "Page".into(),
                }),
            )])]
        );
    }

    #[test]
    fn lone_file_embed_becomes_a_figure() {
        assert_eq!(
            parse("[[File:Foo.jpg|thumb|A caption]]"),
            vec![Block::Figure(
                Box::default(),
                Box::new(Caption {
                    short: None,
                    long: vec![Block::Plain(vec![
                        Inline::Str("A".into()),
                        Inline::Space,
                        Inline::Str("caption".into()),
                    ])],
                }),
                vec![Block::Plain(vec![Inline::Image(
                    Box::default(),
                    vec![],
                    Box::new(Target {
                        url: "Foo.jpg".into(),
                        title: "A caption".into(),
                    }),
                )])],
            )]
        );
    }

    #[test]
    fn embed_without_caption_defaults_to_the_file_name() {
        assert_eq!(
            parse("[[Image:My Photo.jpg]]"),
            vec![Block::Figure(
                Box::default(),
                Box::new(Caption {
                    short: None,
                    long: vec![Block::Plain(vec![Inline::Str("My_Photo.jpg".into())])],
                }),
                vec![Block::Plain(vec![Inline::Image(
                    Box::default(),
                    vec![],
                    Box::new(Target {
                        url: "My_Photo.jpg".into(),
                        title: "My_Photo.jpg".into(),
                    }),
                )])],
            )]
        );
    }

    #[test]
    fn embed_size_parameters_set_width_and_height() {
        assert_eq!(
            parse("[[File:Foo.jpg|100x200px|cap]]"),
            vec![Block::Figure(
                Box::default(),
                Box::new(Caption {
                    short: None,
                    long: vec![Block::Plain(vec![Inline::Str("cap".into())])],
                }),
                vec![Block::Plain(vec![Inline::Image(
                    Box::new(Attr {
                        id: carta_ast::Text::default(),
                        classes: vec![],
                        attributes: vec![
                            ("width".into(), "100".into()),
                            ("height".into(), "200".into()),
                        ],
                    }),
                    vec![],
                    Box::new(Target {
                        url: "Foo.jpg".into(),
                        title: "cap".into(),
                    }),
                )])],
            )]
        );
    }

    #[test]
    fn inline_embed_stays_an_image_not_a_figure() {
        assert_eq!(
            parse("x [[File:Foo.jpg|cap]]"),
            vec![Block::Para(vec![
                Inline::Str("x".into()),
                Inline::Space,
                Inline::Image(
                    Box::default(),
                    vec![Inline::Str("cap".into())],
                    Box::new(Target {
                        url: "Foo.jpg".into(),
                        title: "cap".into(),
                    }),
                ),
            ])]
        );
    }

    #[test]
    fn empty_file_embed_is_an_ordinary_wikilink() {
        assert_eq!(
            parse("[[File:]]"),
            vec![Block::Para(vec![Inline::Link(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["wikilink".into()],
                    attributes: vec![],
                }),
                vec![Inline::Str("File:".into())],
                Box::new(Target {
                    url: "File:".into(),
                    title: "File:".into(),
                }),
            )])]
        );
    }

    #[test]
    fn external_links_number_and_label() {
        assert_eq!(
            parse("[http://x.com lbl] [http://y.com]"),
            vec![Block::Para(vec![
                Inline::Link(
                    Box::default(),
                    vec![Inline::Str("lbl".into())],
                    Box::new(Target {
                        url: "http://x.com".into(),
                        title: carta_ast::Text::default(),
                    }),
                ),
                Inline::Space,
                Inline::Link(
                    Box::default(),
                    vec![Inline::Str("1".into())],
                    Box::new(Target {
                        url: "http://y.com".into(),
                        title: carta_ast::Text::default(),
                    }),
                ),
            ])]
        );
    }

    #[test]
    fn bare_url_trims_trailing_punctuation() {
        assert_eq!(
            parse("see http://x.com."),
            vec![Block::Para(vec![
                Inline::Str("see".into()),
                Inline::Space,
                Inline::Link(
                    Box::default(),
                    vec![Inline::Str("http://x.com".into())],
                    Box::new(Target {
                        url: "http://x.com".into(),
                        title: carta_ast::Text::default(),
                    }),
                ),
                Inline::Str(".".into()),
            ])]
        );
    }

    #[test]
    fn entities_are_decoded_in_text() {
        assert_eq!(
            parse("AT&amp;T &copy;"),
            vec![Block::Para(vec![
                Inline::Str("AT&T".into()),
                Inline::Space,
                Inline::Str("\u{a9}".into()),
            ])]
        );
    }

    #[test]
    fn nowiki_is_literal_text() {
        assert_eq!(
            parse("<nowiki>'''raw'''</nowiki>"),
            vec![Block::Para(vec![Inline::Str("'''raw'''".into())])]
        );
    }

    #[test]
    fn reference_becomes_a_note() {
        assert_eq!(
            parse("x<ref>note</ref>"),
            vec![Block::Para(vec![
                Inline::Str("x".into()),
                Inline::Note(vec![Block::Plain(vec![Inline::Str("note".into())])]),
            ])]
        );
    }

    #[test]
    fn code_tag_decodes_entities() {
        assert_eq!(
            parse("<code>a &amp; b</code>"),
            vec![Block::Para(vec![Inline::Code(
                Box::default(),
                "a & b".into()
            )])]
        );
    }

    #[test]
    fn unknown_tag_passes_through_as_raw_html() {
        assert_eq!(
            parse("<b>x</b>"),
            vec![Block::Para(vec![
                raw_html("<b>".into()),
                Inline::Str("x".into()),
                raw_html("</b>".into()),
            ])]
        );
    }

    #[test]
    fn whole_line_comment_is_removed_with_its_newline() {
        assert_eq!(
            parse("x\n<!--c-->\ny"),
            vec![Block::Para(vec![
                Inline::Str("x".into()),
                Inline::SoftBreak,
                Inline::Str("y".into()),
            ])]
        );
    }

    #[test]
    fn inline_comment_becomes_a_space() {
        assert_eq!(
            parse("a<!--c-->b"),
            vec![Block::Para(vec![
                Inline::Str("a".into()),
                Inline::Space,
                Inline::Str("b".into()),
            ])]
        );
    }

    #[test]
    fn syntax_highlight_block_keeps_language_and_content() {
        assert_eq!(
            parse("<syntaxhighlight lang=\"rust\">\nfn main(){}\n</syntaxhighlight>"),
            vec![Block::CodeBlock(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["rust".into()],
                    attributes: vec![],
                }),
                "fn main(){}".into(),
            )]
        );
    }

    #[test]
    fn horizontal_rule_requires_a_dashes_only_line() {
        assert_eq!(parse("----"), vec![Block::HorizontalRule]);
        assert_eq!(
            parse("----foo"),
            vec![Block::Para(vec![Inline::Str("----foo".into())])]
        );
    }

    #[test]
    fn preformatted_lines_become_code() {
        assert_eq!(
            parse(" indented  line"),
            vec![Block::Para(vec![Inline::Code(
                Box::default(),
                "indented\u{a0}\u{a0}line".into()
            )])]
        );
    }

    #[test]
    fn preformatted_preserves_markup_and_spacing() {
        assert_eq!(
            parse(" a '''b''' c"),
            vec![Block::Para(vec![
                Inline::Code(Box::default(), "a\u{a0}".into()),
                Inline::Strong(vec![Inline::Code(Box::default(), "b".into())]),
                Inline::Code(Box::default(), "\u{a0}c".into()),
            ])]
        );
    }

    #[test]
    fn block_template_is_raw_then_trailing_paragraph() {
        assert_eq!(
            parse("{{tpl}} trailing"),
            vec![
                Block::RawBlock(format_mediawiki(), "{{tpl}}".into()),
                Block::Para(vec![Inline::Str("trailing".into())]),
            ]
        );
    }

    /// Reads with the default option set and reports only whether the read completed without error,
    /// so a deeply nested input can be checked for graceful (non-panicking) handling.
    fn reads_ok(input: &str) -> bool {
        MediawikiReader
            .read(input, &ReaderOptions::default())
            .is_ok()
    }

    #[test]
    fn adversarially_nested_wiki_list_does_not_panic() {
        let mut input = String::new();
        for n in 1..4000 {
            input.push_str(&"*".repeat(n));
            input.push_str(" item\n");
        }
        assert!(reads_ok(&input));
        let single = format!("{} item", "*".repeat(20_000));
        assert!(reads_ok(&single));
    }

    #[test]
    fn adversarially_nested_tables_do_not_panic() {
        let input = format!("{}| x\n{}", "{|\n".repeat(4000), "|}\n".repeat(4000));
        assert!(reads_ok(&input));
    }

    #[test]
    fn adversarially_nested_html_list_does_not_panic() {
        let input = format!("{}x{}", "<ul><li>".repeat(4000), "</li></ul>".repeat(4000));
        assert!(reads_ok(&input));
    }

    #[test]
    fn adversarially_nested_refs_do_not_panic() {
        let input = format!("{}x{}", "a<ref>".repeat(4000), "</ref>".repeat(4000));
        assert!(reads_ok(&input));
    }

    #[test]
    fn stacked_header_lines_do_not_blow_up() {
        // A run of consecutive `=`-prefixed lines with no blank separators and no same-line closer
        // once forced the heading-region lookahead to recompute each line's region for every
        // enclosing region — exponential in the number of stacked lines, which a nightly fuzz run
        // hit as a timeout. Memoizing the region scan makes it linear; a run this long would never
        // finish under the old code.
        let input = "== ~iT\n= w e\n= J".repeat(4000);
        assert!(reads_ok(&input));
    }
}
