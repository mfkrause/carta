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
    ListAttributes, ListNumberDelim, ListNumberStyle, MathType, QuoteType, Row, Table, TableBody,
    TableFoot, TableHead, Target, slug_gfm, to_plain_text,
};
use carta_core::{Extension, Extensions, Reader, ReaderOptions, Result};

use crate::entities;

/// Parses a wikitext document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct MediawikiReader;

impl Reader for MediawikiReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let source = strip_comments(input);
        let chars: Vec<char> = source.chars().collect();
        let mut parser = Parser::new(options);
        let blocks = parser.parse_blocks(&chars);
        Ok(Document {
            api_version: ApiVersion::default(),
            meta: BTreeMap::new(),
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
}

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
        }
    }

    /// Whether straight double quotes should fold into typographic quote runs.
    fn smart(&self) -> bool {
        self.extensions.contains(Extension::Smart)
    }

    fn parse_blocks(&mut self, chars: &[char]) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        let mut pos = 0;
        let mut line_start = true;
        let n = chars.len();
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
                    && let Some(after) = balanced_braces(chars, pos)
                {
                    let raw = collect_range(chars, pos, after);
                    blocks.push(Block::RawBlock(format_mediawiki(), raw));
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
                    && let Some((level, inlines, closer_end)) = self.try_header(chars, pos)
                {
                    let id = self.make_id(&inlines);
                    let attr = Attr {
                        id,
                        classes: Vec::new(),
                        attributes: Vec::new(),
                    };
                    blocks.push(Block::Header(level, attr, inlines));
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
            let (mut para_blocks, after) = self.parse_paragraph(chars, pos);
            blocks.append(&mut para_blocks);
            pos = after;
            line_start = true;
        }
        blocks
    }

    fn try_header(&mut self, chars: &[char], pos: usize) -> Option<(i32, Vec<Inline>, usize)> {
        let le = line_end(chars, pos);
        let mut m = 0;
        while pos + m < le && at(chars, pos + m) == Some('=') {
            m += 1;
        }
        if m == 0 || m > 6 {
            return None;
        }
        let content_start = pos + m;
        let closer = header_closer(chars, content_start, le, m)?;
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
                    pairs.push((term, defs));
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
                Some((Block::CodeBlock(Attr::default(), trim_code(&inner)), after))
            }
            "source" | "syntaxhighlight" => {
                let (inner, after) = enclosed(chars, after_open, &name);
                let mut classes = Vec::new();
                if let Some(lang) = tag_attribute(&raw_open, "lang")
                    && !lang.is_empty()
                {
                    classes.push(lang);
                }
                let attr = Attr {
                    id: String::new(),
                    classes,
                    attributes: Vec::new(),
                };
                Some((Block::CodeBlock(attr, trim_code(&inner)), after))
            }
            _ => None,
        }
    }

    fn parse_paragraph(&mut self, chars: &[char], pos: usize) -> (Vec<Block>, usize) {
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
            let next_end = line_end(chars, next);
            if is_blank(chars, next, next_end) {
                cur = if next_end < n { next_end + 1 } else { next_end };
                break;
            }
            if line_starts_block(chars, next) {
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
        let toks = self.lex(&chars, false);
        let smart = self.smart();
        let mut blocks: Vec<Block> = Vec::new();
        let mut segment: Vec<Tok> = Vec::new();
        for tok in toks {
            match tok {
                Tok::BlockRaw(raw) => {
                    flush_para_segment(&mut segment, &mut blocks, smart);
                    blocks.push(Block::RawBlock(format_html(), raw));
                }
                Tok::BlockBreak => flush_para_segment(&mut segment, &mut blocks, smart),
                other => segment.push(other),
            }
        }
        flush_para_segment(&mut segment, &mut blocks, smart);
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
                let content = self.parse_blocks(&content_chars);
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
        let toks = self.lex(&chars, false);
        let inlines = coalesce(resolve_emphasis(toks));
        if self.smart() {
            apply_smart_quotes(inlines)
        } else {
            inlines
        }
    }

    /// Parses one preformatted line: markup is honored, but literal text and its exact spacing are
    /// preserved as code spans rather than collapsed.
    fn preformatted_line(&mut self, text: &str) -> Vec<Inline> {
        let chars: Vec<char> = text.chars().collect();
        let toks = self.lex(&chars, true);
        preformat_transform(resolve_emphasis(toks))
    }

    fn lex(&mut self, chars: &[char], preformatted: bool) -> Vec<Tok> {
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
                if let Some(after) = balanced_braces(chars, i) {
                    flush_word(&mut word, &mut toks);
                    let raw = collect_range(chars, i, after);
                    toks.push(Tok::Inline(Inline::RawInline(format_mediawiki(), raw)));
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
                            vec![Inline::Math(MathType::InlineMath, inner.trim().to_string())],
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
                    id: String::new(),
                    classes: vec![class.to_string()],
                    attributes: Vec::new(),
                };
                (vec![Inline::Span(attr, self.parse_inlines(&inner))], after)
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
        let text = if label.is_empty() {
            self.link_counter += 1;
            vec![Inline::Str(self.link_counter.to_string())]
        } else {
            self.parse_inlines(&label)
        };
        Some((
            vec![Inline::Link(
                Attr::default(),
                text,
                Target {
                    url,
                    title: String::new(),
                },
            )],
            close + 1,
        ))
    }

    fn internal_link(&mut self, chars: &[char], i: usize) -> Option<(Vec<Inline>, usize)> {
        let close = find_seq(chars, i + 2, &[']', ']'])?;
        let inner = collect_range(chars, i + 2, close);
        let (target_part, label_part) = match inner.split_once('|') {
            Some((t, l)) => (t.to_string(), Some(l.to_string())),
            None => (inner.clone(), None),
        };
        let target = target_part.trim().to_string();
        if let Some(ns) = namespace_of(&target)
            && matches!(ns.as_str(), "file" | "image")
            && !strip_namespace(&target).is_empty()
        {
            let image = self.image_embed(&target, label_part.as_deref());
            return Some((vec![image], close + 2));
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
        let mut label = match &label_part {
            Some(l) => self.parse_inlines(l),
            None => self.parse_inlines(&target),
        };
        let title = title_text(&label);
        if !trail.is_empty() {
            label.push(Inline::Str(trail));
            label = coalesce(label);
        }
        let attr = Attr {
            id: String::new(),
            classes: vec!["wikilink".to_string()],
            attributes: Vec::new(),
        };
        let url = wikilink_url(&target);
        Some((
            vec![Inline::Link(attr, label, Target { url, title })],
            after,
        ))
    }

    /// Builds the image for a `[[File:…|…]]` / `[[Image:…|…]]` embed. The page name (with the
    /// namespace stripped) is the source; the `WxHpx` parameters set width/height; recognized
    /// placement and option keywords are dropped; the last remaining parameter is the caption,
    /// defaulting to the file name. A lone embed in its own paragraph later becomes a figure
    /// (see [`lone_image_figure`]).
    fn image_embed(&mut self, target: &str, params: Option<&str>) -> Inline {
        let url = wikilink_url(strip_namespace(target));
        let mut attributes: Vec<(String, String)> = Vec::new();
        let mut caption: Option<String> = None;
        if let Some(params) = params {
            for part in params.split('|') {
                let option = part.trim();
                if let Some((width, height)) = image_size(option) {
                    attributes.retain(|(key, _)| key != "width" && key != "height");
                    attributes.push(("width".to_string(), width));
                    if let Some(height) = height {
                        attributes.push(("height".to_string(), height));
                    }
                } else if is_image_keyword(option) || option.contains('=') {
                    // A placement, framing, or `key=value` option carries no caption text.
                } else {
                    caption = Some(part.to_string());
                }
            }
        }
        let caption = caption.unwrap_or_else(|| url.clone());
        let alt = self.parse_inlines(&caption);
        let title = title_text(&alt);
        let attr = Attr {
            id: String::new(),
            classes: Vec::new(),
            attributes,
        };
        Inline::Image(attr, alt, Target { url, title })
    }

    fn make_id(&mut self, inlines: &[Inline]) -> String {
        let plain = to_plain_text(inlines);
        if self.extensions.contains(Extension::GfmAutoIdentifiers) {
            let base = slug_gfm(&plain);
            let base = if base.is_empty() {
                "section".to_string()
            } else {
                base
            };
            self.dedup(base, '-')
        } else if self.extensions.contains(Extension::AutoIdentifiers) {
            let base = mediawiki_slug(&plain);
            let base = if base.is_empty() {
                "section".to_string()
            } else {
                base
            };
            self.dedup(base, '_')
        } else {
            String::new()
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
            Tok::BlockBreak => {}
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

/// Tries to open an emphasis span at the apostrophe run starting at `i`, given the kind of the
/// immediately enclosing span. Returns the span node and the index just past its closing run.
fn open_emphasis(
    units: &[Unit],
    runs: &[usize],
    i: usize,
    parent: Option<bool>,
    budget: &mut usize,
) -> Option<(Inline, usize)> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let run = runs.get(i).copied().unwrap_or(0);
    if run < 2 {
        return None;
    }
    // Short runs prefer the wider span; longer runs prefer the narrower one.
    let order = if run <= 5 {
        [true, false]
    } else {
        [false, true]
    };
    for strong in order {
        let width = emphasis_width(strong);
        if run < width || parent == Some(strong) {
            continue;
        }
        let (body, next, closed) = parse_runs(units, runs, i + width, Some(strong), budget);
        if !closed || body.is_empty() {
            continue;
        }
        let body = strip_outer_whitespace(body);
        return Some((
            if strong {
                Inline::Strong(body)
            } else {
                Inline::Emph(body)
            },
            next,
        ));
    }
    None
}

/// Parses content until the run that closes `closer` (or end of input when `closer` is `None`).
/// Returns the collected nodes, the index reached, and whether a closer was found.
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
                if let Some((span, next)) = open_emphasis(units, runs, pos, closer, budget) {
                    nodes.push(span);
                    pos = next;
                    continue;
                }
                if let Some(strong) = closer {
                    let width = emphasis_width(strong);
                    if runs.get(pos).copied().unwrap_or(0) >= width {
                        return (nodes, pos + width, true);
                    }
                }
                nodes.push(Inline::Str("'".to_string()));
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
        out.push(Inline::Str(std::mem::take(buf)));
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
        if let (Some(Inline::Str(prev)), Inline::Str(next)) = (out.last_mut(), &inline) {
            prev.push_str(next);
        } else {
            out.push(inline);
        }
    }
    out
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
                    out.push(Inline::Code(Attr::default(), std::mem::take(&mut run)));
                }
                out.push(preformat_descend(other));
            }
        }
    }
    if !run.is_empty() {
        out.push(Inline::Code(Attr::default(), run));
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
                out.push(Inline::Str(std::mem::take(&mut word)));
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
        out.push(Inline::Str(word));
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

const URL_SCHEMES: &[&str] = &[
    "https://",
    "http://",
    "ftps://",
    "ftp://",
    "ircs://",
    "irc://",
    "gopher://",
    "telnet://",
    "nntp://",
    "mailto:",
    "news:",
    "tel:",
];

fn is_url(text: &str) -> bool {
    let lower = text.to_lowercase();
    URL_SCHEMES.iter().any(|scheme| lower.starts_with(scheme))
}

fn url_scheme_len(chars: &[char], i: usize) -> Option<usize> {
    URL_SCHEMES
        .iter()
        .find(|scheme| matches_prefix_ci(chars, i, scheme))
        .map(|scheme| scheme.chars().count())
}

/// Reads a bare URL beginning at a word boundary, trimming trailing sentence punctuation and an
/// unmatched closing parenthesis. Returns the autolink and the index just past the consumed URL.
fn bare_url(chars: &[char], i: usize) -> Option<(Inline, usize)> {
    let scheme_len = url_scheme_len(chars, i)?;
    let mut j = i + scheme_len;
    while let Some(c) = at(chars, j) {
        if c.is_whitespace() || matches!(c, '<' | '>' | '[' | ']' | '{' | '}' | '|' | '"') {
            break;
        }
        j += 1;
    }
    if j <= i + scheme_len {
        return None;
    }
    let mut url = collect_range(chars, i, j);
    while let Some(last) = url.chars().last() {
        let trailing_punctuation = matches!(last, '.' | ',' | ';' | ':' | '!' | '?');
        let unmatched_paren = last == ')' && !url.contains('(');
        if trailing_punctuation || unmatched_paren {
            url.pop();
        } else {
            break;
        }
    }
    if url.is_empty() {
        return None;
    }
    let consumed = url.chars().count();
    Some((
        Inline::Link(
            Attr::default(),
            vec![Inline::Str(url.clone())],
            Target {
                url,
                title: String::new(),
            },
        ),
        i + consumed,
    ))
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
        Attr::default(),
        caption,
        vec![Block::Plain(vec![image])],
    ))
}

// --- identifiers --------------------------------------------------------------------------------

/// Builds a heading identifier under the `auto_identifiers` scheme: lowercase, keep alphanumerics
/// with `_` and `.`, collapse whitespace and `-` runs to a single `_`, drop other punctuation, and
/// strip a leading run of non-letters.
fn mediawiki_slug(text: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for ch in text.chars() {
        if ch.is_whitespace() || ch == '-' {
            pending = true;
        } else if ch.is_alphanumeric() || ch == '_' || ch == '.' {
            if pending && !out.is_empty() {
                out.push('_');
            }
            pending = false;
            out.extend(ch.to_lowercase());
        }
    }
    out.chars().skip_while(|c| !c.is_alphabetic()).collect()
}

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
    ["pre", "source", "syntaxhighlight", "blockquote"]
        .iter()
        .any(|name| tag_name_matches(chars, pos + 1, name))
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
fn flush_para_segment(segment: &mut Vec<Tok>, blocks: &mut Vec<Block>, smart: bool) {
    if segment.is_empty() {
        return;
    }
    let toks = std::mem::take(segment);
    let mut inlines = coalesce(strip_outer_whitespace(resolve_emphasis(toks)));
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

fn line_starts_block(chars: &[char], ls: usize) -> bool {
    match at(chars, ls) {
        Some('*' | '#' | ':' | ';' | ' ') => true,
        Some('=') => is_header_line(chars, ls),
        Some('-') => is_hr_line(chars, ls),
        Some('{') => matches!(at(chars, ls + 1), Some('{' | '|')),
        Some('<') => starts_block_tag(chars, ls),
        _ => false,
    }
}

fn is_header_line(chars: &[char], pos: usize) -> bool {
    let le = line_end(chars, pos);
    let mut m = 0;
    while pos + m < le && at(chars, pos + m) == Some('=') {
        m += 1;
    }
    if m == 0 || m > 6 {
        return false;
    }
    header_closer(chars, pos + m, le, m).is_some()
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
                id: String::new(),
                classes: classes.iter().map(|s| (*s).to_string()).collect(),
                attributes: Vec::new(),
            };
            (vec![Inline::Code(attr, decode_entities(&inner))], after)
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
        toks.push(Tok::Inline(Inline::Str(std::mem::take(word))));
    }
}

fn raw_html(text: String) -> Inline {
    Inline::RawInline(Format("html".to_string()), text)
}

fn format_mediawiki() -> Format {
    Format("mediawiki".to_string())
}

fn format_html() -> Format {
    Format("html".to_string())
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
    for (i, ch) in s.char_indices() {
        match ch {
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
        let value = if let Some(quote @ ('"' | '\'')) = at(&chars, i) {
            i += 1;
            let value_start = i;
            while at(&chars, i).is_some_and(|c| c != quote) {
                i += 1;
            }
            let value = collect_range(&chars, value_start, i);
            if at(&chars, i) == Some(quote) {
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
            id,
            classes,
            attributes,
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
                Attr {
                    id: "hello_world".into(),
                    classes: vec![],
                    attributes: vec![],
                },
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
                Block::Header(_, attr, _) => Some(attr.id.clone()),
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
                Block::Header(_, attr, _) => Some(attr.id.clone()),
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
                    Attr {
                        id: "h".into(),
                        classes: vec![],
                        attributes: vec![],
                    },
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
                Attr {
                    id: String::new(),
                    classes: vec!["wikilink".into()],
                    attributes: vec![],
                },
                vec![Inline::Str("Pages".into())],
                Target {
                    url: "Page".into(),
                    title: "Page".into(),
                },
            )])]
        );
    }

    #[test]
    fn lone_file_embed_becomes_a_figure() {
        assert_eq!(
            parse("[[File:Foo.jpg|thumb|A caption]]"),
            vec![Block::Figure(
                Attr::default(),
                Caption {
                    short: None,
                    long: vec![Block::Plain(vec![
                        Inline::Str("A".into()),
                        Inline::Space,
                        Inline::Str("caption".into()),
                    ])],
                },
                vec![Block::Plain(vec![Inline::Image(
                    Attr::default(),
                    vec![],
                    Target {
                        url: "Foo.jpg".into(),
                        title: "A caption".into(),
                    },
                )])],
            )]
        );
    }

    #[test]
    fn embed_without_caption_defaults_to_the_file_name() {
        assert_eq!(
            parse("[[Image:My Photo.jpg]]"),
            vec![Block::Figure(
                Attr::default(),
                Caption {
                    short: None,
                    long: vec![Block::Plain(vec![Inline::Str("My_Photo.jpg".into())])],
                },
                vec![Block::Plain(vec![Inline::Image(
                    Attr::default(),
                    vec![],
                    Target {
                        url: "My_Photo.jpg".into(),
                        title: "My_Photo.jpg".into(),
                    },
                )])],
            )]
        );
    }

    #[test]
    fn embed_size_parameters_set_width_and_height() {
        assert_eq!(
            parse("[[File:Foo.jpg|100x200px|cap]]"),
            vec![Block::Figure(
                Attr::default(),
                Caption {
                    short: None,
                    long: vec![Block::Plain(vec![Inline::Str("cap".into())])],
                },
                vec![Block::Plain(vec![Inline::Image(
                    Attr {
                        id: String::new(),
                        classes: vec![],
                        attributes: vec![
                            ("width".into(), "100".into()),
                            ("height".into(), "200".into()),
                        ],
                    },
                    vec![],
                    Target {
                        url: "Foo.jpg".into(),
                        title: "cap".into(),
                    },
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
                    Attr::default(),
                    vec![Inline::Str("cap".into())],
                    Target {
                        url: "Foo.jpg".into(),
                        title: "cap".into(),
                    },
                ),
            ])]
        );
    }

    #[test]
    fn empty_file_embed_is_an_ordinary_wikilink() {
        assert_eq!(
            parse("[[File:]]"),
            vec![Block::Para(vec![Inline::Link(
                Attr {
                    id: String::new(),
                    classes: vec!["wikilink".into()],
                    attributes: vec![],
                },
                vec![Inline::Str("File:".into())],
                Target {
                    url: "File:".into(),
                    title: "File:".into(),
                },
            )])]
        );
    }

    #[test]
    fn external_links_number_and_label() {
        assert_eq!(
            parse("[http://x.com lbl] [http://y.com]"),
            vec![Block::Para(vec![
                Inline::Link(
                    Attr::default(),
                    vec![Inline::Str("lbl".into())],
                    Target {
                        url: "http://x.com".into(),
                        title: String::new(),
                    },
                ),
                Inline::Space,
                Inline::Link(
                    Attr::default(),
                    vec![Inline::Str("1".into())],
                    Target {
                        url: "http://y.com".into(),
                        title: String::new(),
                    },
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
                    Attr::default(),
                    vec![Inline::Str("http://x.com".into())],
                    Target {
                        url: "http://x.com".into(),
                        title: String::new(),
                    },
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
                Attr::default(),
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
                Attr {
                    id: String::new(),
                    classes: vec!["rust".into()],
                    attributes: vec![],
                },
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
                Attr::default(),
                "indented\u{a0}\u{a0}line".into()
            )])]
        );
    }

    #[test]
    fn preformatted_preserves_markup_and_spacing() {
        assert_eq!(
            parse(" a '''b''' c"),
            vec![Block::Para(vec![
                Inline::Code(Attr::default(), "a\u{a0}".into()),
                Inline::Strong(vec![Inline::Code(Attr::default(), "b".into())]),
                Inline::Code(Attr::default(), "\u{a0}c".into()),
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
}
