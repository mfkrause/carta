//! Reader for Jira wiki markup — the line-oriented "text formatting notation" used in Jira
//! issue fields and comments.
//!
//! Blocks are recognised by a line prefix (`hN.`, `bq.`, list markers, table pipes, `----`) or a
//! paired brace macro (`{code}`, `{noformat}`, `{quote}`, `{panel}`). Inline markup — text effects
//! with flanking delimiters, links, images, monospaced and coloured spans, anchors, symbols, and
//! emoticons — is applied to the text of each line; markup does not span a line boundary.

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, Row, Table, TableBody, TableFoot, TableHead, Target,
    ToCompactString,
};
use carta_core::{Reader, ReaderOptions, Result};

/// Parses Jira wiki markup into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct JiraReader;

impl Reader for JiraReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        Ok(Document {
            blocks: parse_blocks_from_str(input),
            ..Document::default()
        })
    }
}

// ---------------------------------------------------------------------------
// Block layer
// ---------------------------------------------------------------------------

fn parse_blocks_from_str(input: &str) -> Vec<Block> {
    blocks_from_str(input, true, true)
}

/// Parses the content of a single list item as blocks. A list item carries full block structure —
/// headings, rules, tables, blockquotes, and brace macros — but the stand-alone colour Div is not a
/// list-item construct, so its marker lines stay literal text there.
fn parse_list_item_blocks(input: &str) -> Vec<Block> {
    blocks_from_str(input, false, true)
}

/// Parses the content of a single table cell as blocks. Lists and brace macros carry block
/// structure, but a line whose prefix names a heading, blockquote, or horizontal rule stays
/// paragraph text, and the stand-alone colour Div is not a cell construct.
fn parse_table_cell(input: &str) -> Vec<Block> {
    let mut blocks = blocks_from_str(input, false, false);
    // A cell's own paragraphs carry no surrounding whitespace — so the text that resumes after a
    // brace macro on the same line loses its leading space. The trim does not recurse: paragraphs
    // nested inside a list or blockquote keep their own leading whitespace.
    for block in &mut blocks {
        if let Block::Para(inlines) = block {
            trim_edge_whitespace(inlines);
        }
    }
    blocks
}

/// Drops leading and trailing whitespace inlines (spaces and line breaks) from `inlines`.
fn trim_edge_whitespace(inlines: &mut Vec<Inline>) {
    let is_ws = |inline: &Inline| matches!(inline, Inline::Space | Inline::LineBreak);
    while inlines.first().is_some_and(is_ws) {
        inlines.remove(0);
    }
    while inlines.last().is_some_and(is_ws) {
        inlines.pop();
    }
}

fn blocks_from_str(input: &str, color_block: bool, line_prefix_blocks: bool) -> Vec<Block> {
    // A carriage return is never a line separator or whitespace here: a `\r\n` pair collapses to a
    // single line break and a lone `\r` is dropped, so every carriage return is removed up front.
    let chars: Vec<char> = input.chars().filter(|&c| c != '\r').collect();
    BlockParser {
        chars: &chars,
        pos: 0,
        color_block,
        line_prefix_blocks,
    }
    .parse_blocks()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MacroKind {
    Code,
    Noformat,
    Quote,
    Panel,
}

struct BlockParser<'a> {
    chars: &'a [char],
    pos: usize,
    /// Whether a stand-alone `{color:…}`/`{color}` pair forms a block-level coloured `Div`. Disabled
    /// while parsing a list item's content, where those lines stay literal text.
    color_block: bool,
    /// Whether a line whose prefix names a block — a heading (`hN.`), a blockquote (`bq.`), or a
    /// horizontal rule (`----`) — is recognised as that block. Disabled inside a table cell, where
    /// only lists and brace macros carry block structure and such lines stay paragraph text.
    line_prefix_blocks: bool,
}

impl BlockParser<'_> {
    fn len(&self) -> usize {
        self.chars.len()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.len()
    }

    /// Index of the newline at or after `from`, or the input length when none remains.
    fn line_end_from(&self, from: usize) -> usize {
        let mut j = from;
        while j < self.len() && self.chars.get(j) != Some(&'\n') {
            j += 1;
        }
        j
    }

    fn line_end(&self) -> usize {
        self.line_end_from(self.pos)
    }

    fn is_blank(&self, start: usize, end: usize) -> bool {
        (start..end).all(|k| self.chars.get(k).is_some_and(|&c| is_space(c)))
    }

    fn advance_line(&mut self) {
        let e = self.line_end();
        self.pos = if e < self.len() { e + 1 } else { e };
    }

    fn skip_blank_lines(&mut self) {
        while !self.at_end() {
            let e = self.line_end();
            if self.is_blank(self.pos, e) {
                self.advance_line();
            } else {
                break;
            }
        }
    }

    fn parse_blocks(&mut self) -> Vec<Block> {
        let mut blocks = Vec::new();
        loop {
            self.skip_blank_lines();
            if self.at_end() {
                break;
            }
            if let Some(macro_blocks) = self.try_macro() {
                blocks.extend(macro_blocks);
                continue;
            }
            if self.color_block
                && let Some(block) = self.try_color_block()
            {
                blocks.push(block);
                continue;
            }
            if self.line_prefix_blocks {
                if let Some(block) = self.try_heading() {
                    blocks.push(block);
                    continue;
                }
                if let Some(block) = self.try_horizontal_rule() {
                    blocks.push(block);
                    continue;
                }
                if let Some(block) = self.try_blockquote() {
                    blocks.push(block);
                    continue;
                }
            }
            if self.table_here() {
                blocks.push(self.parse_table());
                continue;
            }
            if self.list_here() {
                self.parse_list_group(&mut blocks);
                continue;
            }
            blocks.push(self.parse_paragraph());
        }
        blocks
    }

    // --- block-start predicates -------------------------------------------

    fn macro_here(&self) -> Option<MacroKind> {
        let p = self.pos;
        if matches_at(self.chars, p, "{code}") || matches_at(self.chars, p, "{code:") {
            Some(MacroKind::Code)
        } else if matches_at(self.chars, p, "{noformat}") || matches_at(self.chars, p, "{noformat:")
        {
            Some(MacroKind::Noformat)
        } else if matches_at(self.chars, p, "{quote}") {
            Some(MacroKind::Quote)
        } else if matches_at(self.chars, p, "{panel}") || matches_at(self.chars, p, "{panel:") {
            Some(MacroKind::Panel)
        } else {
            None
        }
    }

    fn heading_here(&self) -> Option<i32> {
        if self.chars.get(self.pos) != Some(&'h') || self.chars.get(self.pos + 2) != Some(&'.') {
            return None;
        }
        self.chars
            .get(self.pos + 1)
            .and_then(|c| c.to_digit(10))
            .filter(|d| (1..=6).contains(d))
            .and_then(|d| i32::try_from(d).ok())
    }

    fn horizontal_rule_here(&self) -> bool {
        // A rule is exactly four hyphens at the line start; only trailing whitespace is allowed, so
        // any leading indentation makes the line an ordinary paragraph instead.
        let e = trim_end(self.chars, self.pos, self.line_end());
        e - self.pos == 4 && (self.pos..e).all(|k| self.chars.get(k) == Some(&'-'))
    }

    fn blockquote_here(&self) -> bool {
        matches_at(self.chars, self.pos, "bq.")
    }

    /// A line beginning with a colour marker — an opening `{color:…}` or a closing `{color}` — starts
    /// a new block, so it ends any paragraph that runs into it.
    fn color_marker_line_here(&self) -> bool {
        matches_at(self.chars, self.pos, "{color:") || matches_at(self.chars, self.pos, "{color}")
    }

    fn table_here(&self) -> bool {
        // A line of only delimiters carries no cells, so it is ordinary text rather than a row.
        self.chars.get(self.pos) == Some(&'|')
            && !parse_table_row(self.chars, self.pos, self.line_end()).is_empty()
    }

    /// A run of one or more list-marker characters, optionally indented, followed by a space and at
    /// least one non-space character of item text. A marker with no content after it is ordinary text.
    fn list_here(&self) -> bool {
        let mut k = self.pos;
        while matches!(self.chars.get(k), Some(' ' | '\t')) {
            k += 1;
        }
        let marker_start = k;
        while matches!(self.chars.get(k), Some('*' | '-' | '#')) {
            k += 1;
        }
        if k == marker_start || self.chars.get(k) != Some(&' ') {
            return false;
        }
        let content_start = k + 1;
        trim_end(self.chars, content_start, self.line_end()) > content_start
    }

    fn line_starts_block(&self) -> bool {
        self.macro_here().is_some()
            || self.color_marker_line_here()
            || self.heading_here().is_some()
            || self.horizontal_rule_here()
            || self.blockquote_here()
            || self.table_here()
            || self.list_here()
    }

    // --- simple blocks -----------------------------------------------------

    fn try_heading(&mut self) -> Option<Block> {
        let level = self.heading_here()?;
        let e = self.line_end();
        // A bare block macro in the content makes the line a paragraph that the block layer then
        // splits at the macro, rather than a heading carrying the macro as literal text.
        if self.first_block_macro(self.pos + 3, e).is_some() {
            return None;
        }
        let (ts, te) = trim(self.chars, self.pos + 3, e);
        let inlines = drop_trailing_break(parse_inlines(self.chars, ts, te));
        self.advance_line();
        Some(Block::Header(level, Box::default(), inlines))
    }

    fn try_horizontal_rule(&mut self) -> Option<Block> {
        if !self.horizontal_rule_here() {
            return None;
        }
        self.advance_line();
        Some(Block::HorizontalRule)
    }

    fn try_blockquote(&mut self) -> Option<Block> {
        if !self.blockquote_here() {
            return None;
        }
        let e = self.line_end();
        // A bare block macro in the content makes the line a paragraph that the block layer then
        // splits at the macro, rather than a blockquote carrying the macro as literal text.
        if self.first_block_macro(self.pos + 3, e).is_some() {
            return None;
        }
        let (ts, te) = trim(self.chars, self.pos + 3, e);
        let inlines = drop_trailing_break(parse_inlines(self.chars, ts, te));
        self.advance_line();
        Some(Block::BlockQuote(vec![Block::Para(inlines)]))
    }

    fn parse_paragraph(&mut self) -> Block {
        let para_start = self.pos;
        // The first line is always part of the paragraph; this guarantees forward progress. Its
        // leading whitespace is kept (it collapses to a single leading space). Continuation lines
        // join across the newline, which the inline layer renders as a soft line break, absorbing
        // the whitespace around it.
        let mut content_end = self.line_end();
        self.advance_line();
        loop {
            if self.at_end() {
                break;
            }
            let e = self.line_end();
            if self.is_blank(self.pos, e) || self.line_starts_block() {
                break;
            }
            content_end = e;
            self.advance_line();
        }
        // A bare block macro that opens partway through the text ends the paragraph at that point;
        // the block layer processes the macro on the next pass.
        if let Some(macro_pos) = self.first_block_macro(para_start, content_end) {
            self.pos = macro_pos;
            content_end = macro_pos;
        }
        let para_end = trim_end(self.chars, para_start, content_end);
        Block::Para(drop_trailing_break(parse_inlines(
            self.chars, para_start, para_end,
        )))
    }

    /// Index of the first bare block macro (`{code}`, `{noformat}`, `{quote}`, `{panel}`) in
    /// `lo..hi`, skipping a token whose `{` is escaped by a preceding backslash. The parameterised
    /// forms (`{code:…}` and friends) are recognised only at the start of a block, so they are not
    /// reported here.
    fn first_block_macro(&self, lo: usize, hi: usize) -> Option<usize> {
        let mut k = lo;
        while k < hi {
            if self.chars.get(k) == Some(&'\\') {
                k += 2;
                continue;
            }
            if bare_block_macro_at(self.chars, k) {
                return Some(k);
            }
            k += 1;
        }
        None
    }

    /// A block-level colour span: an opening `{color:VALUE}` whose matching close is a line holding
    /// only `{color}`. The text between is parsed as blocks and wrapped in a `Div` carrying the
    /// colour. An unrecognised value, an absent or non-standalone close, a nested block construct, or
    /// empty content all leave the markup for the inline layer or as literal text.
    fn try_color_block(&mut self) -> Option<Block> {
        if !matches_at(self.chars, self.pos, "{color:") {
            return None;
        }
        let value_start = self.pos + "{color:".len();
        let open_line_end = self.line_end();
        let brace = (value_start..open_line_end).find(|&k| self.chars.get(k) == Some(&'}'))?;
        let value = color_value(&slice_to_string(self.chars, value_start, brace))?;
        let content_start = brace + 1;

        let mut ls = next_line_start(open_line_end, self.len());
        let close_line_start = loop {
            if ls >= self.len() {
                return None;
            }
            let le = self.line_end_from(ls);
            if matches_at(self.chars, ls, "{color}") && self.is_blank(ls + "{color}".len(), le) {
                break ls;
            }
            let probe = BlockParser {
                chars: self.chars,
                pos: ls,
                color_block: self.color_block,
                line_prefix_blocks: self.line_prefix_blocks,
            };
            if !self.is_blank(ls, le) && probe.line_starts_block() {
                return None;
            }
            ls = next_line_start(le, self.len());
        };

        let inner = parse_color_block_inner(self.chars.get(content_start..close_line_start)?);
        // Content that begins with a blank line yields no leading paragraph, so the markup does not
        // form a block.
        match inner.first() {
            None => return None,
            Some(Block::Para(inlines)) if inlines.is_empty() => return None,
            _ => {}
        }

        let close_line_end = self.line_end_from(close_line_start);
        self.pos = next_line_start(close_line_end, self.len());
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: vec![("color".into(), value.into())],
        };
        Some(Block::Div(Box::new(attr), inner))
    }

    // --- tables ------------------------------------------------------------

    fn parse_table(&mut self) -> Block {
        let mut rows: Vec<Vec<(bool, String)>> = Vec::new();
        while !self.at_end() {
            let e = self.line_end();
            if self.is_blank(self.pos, e) || self.chars.get(self.pos) != Some(&'|') {
                break;
            }
            let cells = parse_table_row(self.chars, self.pos, e);
            if cells.is_empty() {
                // A delimiter-only line has no cells; it ends the table and reparses as text.
                break;
            }
            rows.push(cells);
            self.advance_line();
        }

        let col_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut head_rows = Vec::new();
        let mut body_rows = Vec::new();
        let mut still_header = true;
        for cells in &rows {
            let all_header = !cells.is_empty() && cells.iter().all(|(is_header, _)| *is_header);
            let row = build_table_row(cells, col_count);
            if still_header && all_header {
                head_rows.push(row);
            } else {
                still_header = false;
                body_rows.push(row);
            }
        }

        let table = Table {
            attr: Attr::default(),
            caption: Caption {
                short: None,
                long: Vec::new(),
            },
            col_specs: vec![
                ColSpec {
                    align: Alignment::AlignDefault,
                    width: ColWidth::ColWidthDefault,
                };
                col_count
            ],
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
        };
        Block::Table(Box::new(table))
    }

    // --- lists -------------------------------------------------------------

    fn parse_list_group(&mut self, out: &mut Vec<Block>) {
        let mut items: Vec<ListItem> = Vec::new();
        loop {
            if self.at_end() {
                break;
            }
            let e = self.line_end();
            // A blank line and a horizontal rule both close the list; everything else that is not a
            // new marker is item content (a continuation line), so headings, tables, blockquotes, and
            // brace macros that follow an item are absorbed into it rather than ending the list.
            if self.is_blank(self.pos, e) || self.horizontal_rule_here() {
                break;
            }
            if self.list_here() {
                let mut k = self.pos;
                while matches!(self.chars.get(k), Some(' ' | '\t')) {
                    k += 1;
                }
                let marker_start = k;
                while matches!(self.chars.get(k), Some('*' | '-' | '#')) {
                    k += 1;
                }
                let marker = slice_to_string(self.chars, marker_start, k);
                // Exactly one space separates the marker from the item text; any further leading
                // whitespace is part of the content.
                let content_start = k + 1;
                items.push(ListItem {
                    marker,
                    text: slice_to_string(self.chars, content_start, e),
                });
                self.advance_line();
            } else if let Some(last) = items.last_mut() {
                last.text.push('\n');
                last.text
                    .push_str(&slice_to_string(self.chars, self.pos, e));
                self.advance_line();
            } else {
                break;
            }
        }
        build_lists(&items, 1, out);
    }

    // --- brace macros ------------------------------------------------------

    fn try_macro(&mut self) -> Option<Vec<Block>> {
        let kind = self.macro_here()?;
        let fence_end = (self.pos..self.len()).find(|&k| self.chars.get(k) == Some(&'}'))?;
        let inside = slice_to_string(self.chars, self.pos + 1, fence_end);
        let params = inside.split_once(':').map(|(_, p)| p.to_string());
        let has_params = params.is_some();
        let open_line_end = self.line_end_from(fence_end);
        let open_trailing_blank = self.is_blank(fence_end + 1, open_line_end);
        match kind {
            MacroKind::Code => self.parse_code(
                params.as_deref(),
                open_line_end,
                has_params,
                open_trailing_blank,
            ),
            MacroKind::Noformat => Some(self.parse_noformat(
                params.as_deref(),
                fence_end,
                open_line_end,
                open_trailing_blank,
            )),
            MacroKind::Quote => Some(self.parse_quote(fence_end)),
            MacroKind::Panel => self.parse_panel(
                params.as_deref(),
                fence_end,
                has_params,
                open_trailing_blank,
            ),
        }
    }

    /// Parses a `{code}` block. Its open fence must be alone on its line: any non-blank content
    /// after the closing brace disqualifies the block. With parameters present such an open line is
    /// not a code block at all (it reverts to text); a bare `{code}` with trailing content instead
    /// consumes the remainder of the input. The content begins on the next line and runs to a close
    /// line that ends with `{code}` (any text before the close on that line is kept).
    fn parse_code(
        &mut self,
        params: Option<&str>,
        open_line_end: usize,
        has_params: bool,
        open_trailing_blank: bool,
    ) -> Option<Vec<Block>> {
        if !open_trailing_blank {
            if has_params {
                return None;
            }
            self.pos = self.len();
            return Some(Vec::new());
        }
        let content_start = next_line_start(open_line_end, self.len());
        let (classes, attributes) = verbatim_params(MacroKind::Code, params);
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: classes.into_iter().map(Into::into).collect(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        };
        if let Some((content, resume)) = self.scan_code_content(content_start) {
            self.pos = resume;
            Some(vec![Block::CodeBlock(Box::new(attr), content.into())])
        } else {
            self.pos = self.len();
            Some(Vec::new())
        }
    }

    /// Collects the lines from `start` up to a `{code}` close. A close is a line that ends with the
    /// token once trailing whitespace is ignored; text before the token on that line is content, and
    /// parsing resumes on the following line. Returns the content and resume index, or `None` when no
    /// close is found.
    fn scan_code_content(&self, start: usize) -> Option<(String, usize)> {
        const CLOSE: &str = "{code}";
        let close_len = CLOSE.chars().count();
        let mut content = String::new();
        let mut cur = start;
        while cur < self.len() {
            let le = self.line_end_from(cur);
            let te = trim_end(self.chars, cur, le);
            if te >= cur + close_len && matches_at(self.chars, te - close_len, CLOSE) {
                content.push_str(&slice_to_string(self.chars, cur, te - close_len));
                return Some((content, next_line_start(le, self.len())));
            }
            content.push_str(&slice_to_string(self.chars, cur, le));
            content.push('\n');
            cur = next_line_start(le, self.len());
        }
        None
    }

    /// Parses a `{noformat}` block. Unlike `{code}`, content may begin on the open line: when the
    /// rest of that line is blank the content starts on the next line, otherwise it starts right
    /// after the closing brace. The block ends at the first `{noformat}`; any text after the close on
    /// its line continues as following content.
    fn parse_noformat(
        &mut self,
        params: Option<&str>,
        fence_end: usize,
        open_line_end: usize,
        open_trailing_blank: bool,
    ) -> Vec<Block> {
        const CLOSE: &str = "{noformat}";
        let content_start = if open_trailing_blank {
            next_line_start(open_line_end, self.len())
        } else {
            fence_end + 1
        };
        let (classes, attributes) = verbatim_params(MacroKind::Noformat, params);
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: classes.into_iter().map(Into::into).collect(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        };
        if let Some(close) = find_token(self.chars, content_start, CLOSE) {
            let content = slice_to_string(self.chars, content_start, close);
            self.pos = close + CLOSE.chars().count();
            vec![Block::CodeBlock(Box::new(attr), content.into())]
        } else {
            self.pos = self.len();
            Vec::new()
        }
    }

    /// Consume the text between the current fence and its closing `close_token`, advancing past the
    /// token. When the closing token is absent the whole remaining input is consumed and `None` is
    /// returned.
    fn take_fenced(&mut self, fence_end: usize, close_token: &str) -> Option<String> {
        match find_token(self.chars, fence_end + 1, close_token) {
            None => {
                self.pos = self.len();
                None
            }
            Some(close) => {
                let content = slice_to_string(self.chars, fence_end + 1, close);
                self.pos = close + close_token.len();
                Some(content)
            }
        }
    }

    fn parse_quote(&mut self, fence_end: usize) -> Vec<Block> {
        let Some(content) = self.take_fenced(fence_end, "{quote}") else {
            return Vec::new();
        };
        vec![Block::BlockQuote(parse_blocks_from_str(&content))]
    }

    /// Parses a `{panel}` block. Like `{code}`, its open fence must stand alone on its line: a
    /// parameterised open line with trailing content reverts to text, while a bare `{panel}` with
    /// trailing content consumes the remainder of the input.
    fn parse_panel(
        &mut self,
        params: Option<&str>,
        fence_end: usize,
        has_params: bool,
        open_trailing_blank: bool,
    ) -> Option<Vec<Block>> {
        if !open_trailing_blank {
            if has_params {
                return None;
            }
            self.pos = self.len();
            return Some(Vec::new());
        }
        let Some(content) = self.take_fenced(fence_end, "{panel}") else {
            return Some(Vec::new());
        };
        let (title, attributes) = panel_params(params);
        let mut inner = Vec::new();
        if let Some(title) = title {
            inner.push(Block::Div(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["panelheader".into()],
                    attributes: Vec::new(),
                }),
                vec![Block::Plain(vec![Inline::Strong(plain_inlines(&title))])],
            ));
        }
        inner.extend(parse_blocks_from_str(&content));
        Some(vec![Block::Div(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes: vec!["panel".into()],
                attributes: attributes
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
            }),
            inner,
        )])
    }
}

/// The start of the line after the one ending at `line_end`, or `len` when that line ends the input.
fn next_line_start(line_end: usize, len: usize) -> usize {
    if line_end < len {
        line_end + 1
    } else {
        line_end
    }
}

/// Parses the body of a block-level colour span. The newline that follows the opening marker leads
/// the first paragraph as a line break; blank lines beyond it separate the body into paragraphs.
fn parse_color_block_inner(content: &[char]) -> Vec<Block> {
    let mut parser = BlockParser {
        chars: content,
        pos: 0,
        color_block: true,
        line_prefix_blocks: true,
    };
    let mut blocks = Vec::new();
    let mut first = true;
    loop {
        if first {
            first = false;
        } else {
            parser.skip_blank_lines();
        }
        if parser.at_end() {
            break;
        }
        blocks.push(parser.parse_paragraph());
    }
    blocks
}

struct ListItem {
    marker: String,
    text: String,
}

/// Builds the nested list blocks for `items` at the given (1-based) depth, appending sibling lists
/// to `out`. A marker's length is its nesting depth: items are grouped by the marker character at
/// this depth, a different character starts a separate sibling list, and any item whose marker is
/// longer than this depth nests inside the current item — so a lone `*** x` produces three nested
/// lists even with no shallower item preceding it.
fn build_lists(items: &[ListItem], depth: usize, out: &mut Vec<Block>) {
    let mut idx = 0;
    while idx < items.len() {
        let Some(group_char) = marker_char(items, idx, depth) else {
            idx += 1;
            continue;
        };
        let mut list_items: Vec<Vec<Block>> = Vec::new();
        while marker_char(items, idx, depth) == Some(group_char) {
            let mut item_blocks = Vec::new();
            // An item whose marker length is exactly this depth owns the text; a longer marker is an
            // implicit parent that carries only its nested child list.
            let owns_content = marker_len(items, idx) == depth;
            let child_start = if owns_content {
                if let Some(item) = items.get(idx) {
                    item_blocks.extend(parse_list_item_blocks(&item.text));
                }
                idx + 1
            } else {
                idx
            };
            let mut child_end = child_start;
            while marker_len(items, child_end) > depth
                && marker_char(items, child_end, depth) == Some(group_char)
            {
                child_end += 1;
            }
            if let Some(children) = items.get(child_start..child_end) {
                build_lists(children, depth + 1, &mut item_blocks);
            }
            list_items.push(item_blocks);
            idx = child_end;
        }
        if group_char == '#' {
            out.push(Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::DefaultDelim,
                },
                list_items,
            ));
        } else {
            out.push(Block::BulletList(list_items));
        }
    }
}

fn marker_char(items: &[ListItem], idx: usize, depth: usize) -> Option<char> {
    items
        .get(idx)
        .and_then(|it| it.marker.chars().nth(depth - 1))
}

fn marker_len(items: &[ListItem], idx: usize) -> usize {
    items.get(idx).map_or(0, |it| it.marker.chars().count())
}

/// Removes a forced line break that ends a block's inline content. A line break with nothing after
/// it has no following line to separate, so it is dropped.
fn drop_trailing_break(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while matches!(inlines.last(), Some(Inline::LineBreak)) {
        inlines.pop();
    }
    inlines
}

fn parse_table_row(chars: &[char], start: usize, end: usize) -> Vec<(bool, String)> {
    let mut cells = Vec::new();
    let (mut i, _) = trim(chars, start, end);
    while i < end {
        if chars.get(i) != Some(&'|') {
            break;
        }
        let mut run = 0;
        while i < end && chars.get(i) == Some(&'|') {
            run += 1;
            i += 1;
        }
        let is_header = run >= 2;
        // Scan the cell content up to the next delimiter, ignoring pipes nested inside a bracketed
        // link, a brace span, or an image's property list so an inner `|` does not split the cell.
        let cell_start = i;
        let mut depth = 0i32;
        while i < end {
            match chars.get(i) {
                Some('[' | '{') => {
                    depth += 1;
                    i += 1;
                }
                Some(']' | '}') => {
                    depth = depth.saturating_sub(1);
                    i += 1;
                }
                Some('!') if depth == 0 => match parse_image(chars, i, end) {
                    Some((_, next)) => i = next,
                    None => i += 1,
                },
                Some('|') if depth == 0 => break,
                _ => i += 1,
            }
        }
        let (ts, te) = trim(chars, cell_start, i);
        let content = slice_to_string(chars, ts, te);
        if i >= end && content.is_empty() {
            // A trailing delimiter run closes the final cell; it introduces no new cell.
            break;
        }
        cells.push((is_header, content));
    }
    cells
}

fn build_table_row(cells: &[(bool, String)], col_count: usize) -> Row {
    let mut out_cells = Vec::with_capacity(col_count);
    for col in 0..col_count {
        let content = match cells.get(col) {
            Some((_, text)) if !text.is_empty() => parse_table_cell(text),
            _ => Vec::new(),
        };
        out_cells.push(Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content,
        });
    }
    Row {
        attr: Attr::default(),
        cells: out_cells,
    }
}

fn verbatim_params(kind: MacroKind, params: Option<&str>) -> (Vec<String>, Vec<(String, String)>) {
    let mut classes = Vec::new();
    let mut attributes = Vec::new();
    let tokens: Vec<&str> = match params {
        Some(p) if !p.is_empty() => p.split('|').collect(),
        _ => Vec::new(),
    };
    match kind {
        MacroKind::Code => {
            let mut language = "java".to_string();
            for (idx, token) in tokens.iter().enumerate() {
                let token = token.trim();
                if let Some((key, value)) = token.split_once('=') {
                    attributes.push((key.trim().to_string(), value.trim().to_string()));
                } else if idx == 0 && !token.is_empty() {
                    language = token.to_string();
                }
            }
            classes.push(language);
        }
        _ => {
            for token in tokens {
                if let Some((key, value)) = token.trim().split_once('=') {
                    attributes.push((key.trim().to_string(), value.trim().to_string()));
                }
            }
        }
    }
    (classes, attributes)
}

fn panel_params(params: Option<&str>) -> (Option<String>, Vec<(String, String)>) {
    let mut title = None;
    let mut attributes = Vec::new();
    let tokens: Vec<&str> = match params {
        Some(p) if !p.is_empty() => p.split('|').collect(),
        _ => Vec::new(),
    };
    for token in tokens {
        if let Some((key, value)) = token.trim().split_once('=') {
            if key.trim() == "title" {
                title = Some(value.trim().to_string());
            } else {
                attributes.push((key.trim().to_string(), value.trim().to_string()));
            }
        }
    }
    (title, attributes)
}

// ---------------------------------------------------------------------------
// Inline layer
// ---------------------------------------------------------------------------

/// URL prefixes accepted as a bracketed link target. Schemes other than `mailto:` require `://`.
const LINK_URL_PREFIXES: &[&str] = &[
    "https://", "http://", "ftp://", "file://", "news://", "nntp://", "irc://", "mailto:",
];
/// URL prefixes accepted as a bare (unbracketed) autolink. `file://` is not autolinked.
const BARE_URL_PREFIXES: &[&str] = &[
    "https://", "http://", "ftp://", "news://", "nntp://", "irc://", "mailto:",
];

const PAREN_SYMBOLS: &[(&str, char)] = &[
    ("(flagoff)", '\u{2690}'),
    ("(flag)", '\u{2691}'),
    ("(off)", '\u{1F319}'),
    ("(on)", '\u{1F4A1}'),
    ("(*r)", '\u{2B50}'),
    ("(*g)", '\u{2B50}'),
    ("(*b)", '\u{2B50}'),
    ("(*y)", '\u{2B50}'),
    ("(*)", '\u{2B50}'),
    ("(!)", '\u{2757}'),
    ("(x)", '\u{274C}'),
    ("(/)", '\u{2714}'),
    ("(i)", '\u{2139}'),
    ("(?)", '\u{2753}'),
    ("(y)", '\u{1F44D}'),
    ("(n)", '\u{1F44E}'),
    ("(+)", '\u{2795}'),
    ("(-)", '\u{2796}'),
];

const EMOTICONS: &[(&str, char)] = &[
    (":)", '\u{1F642}'),
    (":(", '\u{1F641}'),
    (":P", '\u{1F61B}'),
    (":D", '\u{1F603}'),
    (";)", '\u{1F609}'),
];

/// Tokenises `text` into inlines without interpreting markup: whitespace runs become
/// [`Inline::Space`] and every other run becomes an [`Inline::Str`]. Used for a panel title, whose
/// text is rendered verbatim inside its header.
fn plain_inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in text.chars() {
        if is_space(ch) {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word).into()));
            }
            if out.last() != Some(&Inline::Space) {
                out.push(Inline::Space);
            }
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word.into()));
    }
    out
}

/// A unit of scanned inline content, before text-effect delimiters are paired up.
enum Tok {
    /// A run of literal text.
    Text(String),
    /// A single flanking delimiter that may open and/or close a text-effect span.
    Delim {
        marker: char,
        open: bool,
        close: bool,
    },
    /// A fully formed inline node — link, image, span, monospace, line break, or space.
    Atom(Inline),
}

/// Inline-nesting depth past which parsing stops descending. Monospace, colour, link-label and
/// citation spans each re-enter inline parsing on their inner text; a hard cap keeps adversarially
/// deep nesting off the call stack. It is far beyond any nesting real text uses.
const MAX_INLINE_DEPTH: usize = 32;

/// Parses the character range `lo..hi` into inline nodes: it scans the text into tokens, pairs the
/// flanking delimiters into spans, and folds the result into a flat list of inlines. Flanking
/// decisions consult the real neighbouring characters via absolute indices, so a range bounded to a
/// single line will not let markup escape that line.
fn parse_inlines(chars: &[char], lo: usize, hi: usize) -> Vec<Inline> {
    inlines_with(chars, lo, hi, true, 0)
}

/// Parses inlines with control over bare-URL autolinking. A link label cannot contain another link,
/// so the text of one is parsed with `autolink` cleared. `depth` tracks how many nested spans deep
/// this call is; past the cap the remaining span is emitted as literal text without descending.
fn inlines_with(chars: &[char], lo: usize, hi: usize, autolink: bool, depth: usize) -> Vec<Inline> {
    if depth > MAX_INLINE_DEPTH {
        let text = slice_to_string(chars, lo, hi);
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![Inline::Str(text.into())]
        };
    }
    finalize(resolve(scan_tokens(chars, lo, hi, autolink, depth)))
}

fn push_text(pending: &mut String, toks: &mut Vec<Tok>) {
    if !pending.is_empty() {
        toks.push(Tok::Text(std::mem::take(pending)));
    }
}

/// Scans `lo..hi` left to right into tokens: literal runs accumulate into [`Tok::Text`], a flanking
/// delimiter becomes a [`Tok::Delim`], and a self-contained construct (link, image, brace span,
/// citation, autolink, symbol) becomes a [`Tok::Atom`].
fn scan_tokens(chars: &[char], lo: usize, hi: usize, autolink: bool, depth: usize) -> Vec<Tok> {
    let mut toks: Vec<Tok> = Vec::new();
    let mut pending = String::new();
    let mut i = lo;

    while i < hi {
        let Some(&c) = chars.get(i) else {
            break;
        };

        if is_space(c) {
            push_text(&mut pending, &mut toks);
            i = scan_whitespace_run(chars, i, hi, &mut toks);
            continue;
        }

        let prev_alnum = i > 0 && chars.get(i - 1).is_some_and(|c| c.is_alphanumeric());

        if autolink
            && !prev_alnum
            && let Some(end) = match_bare_url(chars, i, hi)
        {
            push_text(&mut pending, &mut toks);
            let url = slice_to_string(chars, i, end);
            toks.push(Tok::Atom(Inline::Link(
                Box::default(),
                vec![Inline::Str(url.clone().into())],
                Box::new(Target {
                    url: url.into(),
                    title: carta_ast::Text::default(),
                }),
            )));
            i = end;
            continue;
        }

        match c {
            '\\' => {
                i = scan_backslash(chars, i, hi, &mut pending, &mut toks);
            }
            '&' => {
                if let Some((text, next)) = crate::entities::read_reference(chars, i, hi, false) {
                    pending.push_str(&text);
                    i = next;
                } else {
                    pending.push('&');
                    i += 1;
                }
            }
            '?' => {
                if let Some((next, inner)) = parse_citation(chars, i, hi, autolink, depth) {
                    pending.push('\u{2014}');
                    push_text(&mut pending, &mut toks);
                    toks.push(Tok::Atom(Inline::Space));
                    toks.push(Tok::Atom(Inline::Emph(inner)));
                    i = next;
                } else {
                    pending.push('?');
                    i += 1;
                }
            }
            '*' | '_' | '+' | '^' | '~' => {
                push_delimiter(c, chars, i, &mut pending, &mut toks);
                i += 1;
            }
            '-' => {
                i = scan_dash(chars, i, hi, &mut pending, &mut toks);
            }
            '(' => {
                if let Some((glyph, len)) = match_token_symbol(chars, i, PAREN_SYMBOLS) {
                    pending.push(glyph);
                    i += len;
                } else {
                    pending.push('(');
                    i += 1;
                }
            }
            ':' | ';' => {
                if let Some((glyph, len)) = match_token_symbol(chars, i, EMOTICONS) {
                    pending.push(glyph);
                    i += len;
                } else {
                    pending.push(c);
                    i += 1;
                }
            }
            '[' | '!' | '{' => {
                if let Some((node, next)) = scan_construct(c, chars, i, hi, autolink, depth) {
                    push_text(&mut pending, &mut toks);
                    toks.push(Tok::Atom(node));
                    i = next;
                } else {
                    pending.push(c);
                    i += 1;
                }
            }
            _ => {
                pending.push(c);
                i += 1;
            }
        }
    }

    push_text(&mut pending, &mut toks);
    toks
}

/// Consumes the whitespace run beginning at `start`, pushing the single token it collapses to: a
/// line break when the run crosses a newline, otherwise a space. The spaces around a soft break are
/// absorbed into it. Returns the index just past the run.
fn scan_whitespace_run(chars: &[char], start: usize, hi: usize, toks: &mut Vec<Tok>) -> usize {
    let mut has_newline = chars.get(start) == Some(&'\n');
    let mut i = start + 1;
    while i < hi && chars.get(i).is_some_and(|&c| is_space(c)) {
        has_newline |= chars.get(i) == Some(&'\n');
        i += 1;
    }
    toks.push(Tok::Atom(if has_newline {
        Inline::LineBreak
    } else {
        Inline::Space
    }));
    i
}

/// The punctuation a backslash removes itself before, leaving the character as literal text. Any
/// character outside this set keeps its backslash.
fn is_escapable(c: char) -> bool {
    matches!(
        c,
        '!' | '"'
            | '#'
            | '%'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | ','
            | '-'
            | '.'
            | '/'
            | ':'
            | ';'
            | '?'
            | '@'
            | '['
            | ']'
            | '_'
            | '{'
            | '}'
    )
}

/// Emits a flanking-delimiter token for one of the emphasis markers at `i`, or buffers the marker as
/// literal text when it can neither open nor close a span.
fn push_delimiter(
    marker: char,
    chars: &[char],
    i: usize,
    pending: &mut String,
    toks: &mut Vec<Tok>,
) {
    let open = can_open(chars, i);
    let close = can_close(chars, i);
    if open || close {
        push_text(pending, toks);
        toks.push(Tok::Delim {
            marker,
            open,
            close,
        });
    } else {
        pending.push(marker);
    }
}

/// Parses a self-contained construct introduced by `c` at `i`: `[` starts a link, `!` an image, and
/// `{` a brace span. Returns the resulting node and the index just past it, or `None` when the text
/// does not form that construct.
fn scan_construct(
    c: char,
    chars: &[char],
    i: usize,
    hi: usize,
    autolink: bool,
    depth: usize,
) -> Option<(Inline, usize)> {
    match c {
        '[' => parse_link(chars, i, hi, depth),
        '!' => parse_image(chars, i, hi),
        _ => parse_brace_inline(chars, i, hi, autolink, depth),
    }
}

/// Handles a backslash at `i`. A backslash pair `\\` is a forced line break that absorbs the
/// whitespace around it — unless a third backslash follows, in which case the pair is an escaped
/// backslash producing one literal `\` and the scan continues at the third. A backslash before one
/// of a fixed set of punctuation marks escapes that mark to a literal; before anything else the
/// backslash itself stays literal. Returns the next position.
fn scan_backslash(
    chars: &[char],
    i: usize,
    hi: usize,
    pending: &mut String,
    toks: &mut Vec<Tok>,
) -> usize {
    if i + 1 < hi && chars.get(i + 1) == Some(&'\\') {
        if i + 2 < hi && chars.get(i + 2) == Some(&'\\') {
            pending.push('\\');
            return i + 2;
        }
        push_text(pending, toks);
        if matches!(toks.last(), Some(Tok::Atom(Inline::Space))) {
            toks.pop();
        }
        toks.push(Tok::Atom(Inline::LineBreak));
        let mut j = i + 2;
        while j < hi && chars.get(j).is_some_and(|&c| is_space(c)) {
            j += 1;
        }
        return j;
    }
    if let Some(&next) = chars.get(i + 1).filter(|_| i + 1 < hi)
        && is_escapable(next)
    {
        pending.push(next);
        return i + 2;
    }
    pending.push('\\');
    i + 1
}

/// Handles a run of `-` at `i`. A run of two or more hyphens followed by a space or tab folds into
/// typographic dashes: a word character on its left keeps the first hyphen attached to that word,
/// then the remaining hyphens fold — two into an en dash, three or more into an em dash preceded by
/// the surplus hyphens. Otherwise a single `-` is scanned as a strikeout delimiter (or literal text).
/// Returns the next scan position. The character following the run is read from the full input rather
/// than the line-content bound, so a hyphen run that ends a line still sees the space trimmed from it.
fn scan_dash(
    chars: &[char],
    i: usize,
    hi: usize,
    pending: &mut String,
    toks: &mut Vec<Tok>,
) -> usize {
    let mut run = 0;
    while i + run < hi && chars.get(i + run) == Some(&'-') {
        run += 1;
    }
    let left_word = i > 0 && chars.get(i - 1).is_some_and(|c| c.is_alphanumeric());
    let right_space = matches!(chars.get(i + run), Some(' ' | '\t'));
    // A word on the left keeps its first hyphen attached, so only the remainder folds.
    let fold_run = if left_word {
        run.saturating_sub(1)
    } else {
        run
    };
    // Fold only when at least two hyphens remain to fold into a typographic dash. A lone leftover
    // hyphen would render identically to literal text, so it is left as a strikeout delimiter instead
    // — that way a `--…--` pair whose closing run is followed by a space can still form a span.
    if right_space && fold_run >= 2 {
        if left_word {
            pending.push('-');
        }
        if fold_run == 2 {
            pending.push('\u{2013}');
        } else {
            for _ in 0..fold_run.saturating_sub(3) {
                pending.push('-');
            }
            pending.push('\u{2014}');
        }
        return i + run;
    }

    let open = can_open(chars, i);
    let close = can_close(chars, i);
    if open || close {
        push_text(pending, toks);
        toks.push(Tok::Delim {
            marker: '-',
            open,
            close,
        });
    } else {
        pending.push('-');
    }
    i + 1
}

/// Index of the innermost open delimiter still awaiting a close, regardless of its marker.
fn top_opener(acc: &[Tok]) -> Option<usize> {
    acc.iter()
        .rposition(|t| matches!(t, Tok::Delim { open: true, .. }))
}

/// Pairs flanking delimiters into spans. A closing delimiter binds only to the innermost open
/// delimiter; it forms a span when that opener carries the same marker and they enclose non-empty
/// content, and is otherwise left literal. Binding only to the innermost opener keeps spans strictly
/// nested, so two different markers that interleave cannot both form a span. Same-marker spans nest
/// at most two deep.
fn resolve(toks: Vec<Tok>) -> Vec<Tok> {
    let mut acc: Vec<Tok> = Vec::new();
    for tok in toks {
        let Tok::Delim {
            marker,
            open,
            close,
        } = tok
        else {
            acc.push(tok);
            continue;
        };
        if close
            && let Some(open_idx) = top_opener(&acc)
            && matches!(acc.get(open_idx), Some(Tok::Delim { marker: m, .. }) if *m == marker)
            && acc.len() > open_idx + 1
        {
            let inner = finalize(acc.split_off(open_idx + 1));
            if same_marker_depth(&inner, marker) < 2 {
                acc.pop();
                acc.push(Tok::Atom(make_span(marker, inner)));
                continue;
            }
            // The nesting cap is reached: the opener stays unmatched and its already-resolved
            // content returns to the stack.
            acc.extend(inner.into_iter().map(Tok::Atom));
        }
        acc.push(Tok::Delim {
            marker,
            open,
            close,
        });
    }
    acc
}

/// Lowers resolved tokens into inlines: an unmatched delimiter becomes its literal marker character,
/// adjacent text merges into one string, and adjacent spans of the same kind merge into one.
fn finalize(toks: Vec<Tok>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for tok in toks {
        let inline = match tok {
            Tok::Text(s) => Inline::Str(s.into()),
            Tok::Delim { marker, .. } => Inline::Str(marker.to_compact_string()),
            Tok::Atom(node) => node,
        };
        let inline = match out.last_mut() {
            Some(last) => match merge_adjacent(last, inline) {
                None => continue,
                Some(unmerged) => unmerged,
            },
            None => inline,
        };
        out.push(inline);
    }
    out
}

/// Merges `next` into `last` when they are two strings or two spans of the same kind, returning
/// `None` on success and `Some(next)` when they do not combine.
fn merge_adjacent(last: &mut Inline, next: Inline) -> Option<Inline> {
    match (last, next) {
        (Inline::Str(a), Inline::Str(b)) => {
            a.push_str(&b);
            None
        }
        (Inline::Strong(a), Inline::Strong(b))
        | (Inline::Emph(a), Inline::Emph(b))
        | (Inline::Underline(a), Inline::Underline(b))
        | (Inline::Superscript(a), Inline::Superscript(b))
        | (Inline::Subscript(a), Inline::Subscript(b))
        | (Inline::Strikeout(a), Inline::Strikeout(b)) => {
            a.extend(b);
            None
        }
        (_, other) => Some(other),
    }
}

fn make_span(marker: char, inner: Vec<Inline>) -> Inline {
    match marker {
        '*' => Inline::Strong(inner),
        '_' => Inline::Emph(inner),
        '+' => Inline::Underline(inner),
        '^' => Inline::Superscript(inner),
        '~' => Inline::Subscript(inner),
        _ => Inline::Strikeout(inner),
    }
}

/// The deepest nesting of spans carrying `marker` anywhere within `nodes`.
fn same_marker_depth(nodes: &[Inline], marker: char) -> usize {
    nodes
        .iter()
        .map(|n| node_marker_depth(n, marker))
        .max()
        .unwrap_or(0)
}

fn node_marker_depth(node: &Inline, marker: char) -> usize {
    let (is_match, children) = match node {
        Inline::Strong(k) => (marker == '*', Some(k)),
        Inline::Emph(k) => (marker == '_', Some(k)),
        Inline::Underline(k) => (marker == '+', Some(k)),
        Inline::Superscript(k) => (marker == '^', Some(k)),
        Inline::Subscript(k) => (marker == '~', Some(k)),
        Inline::Strikeout(k) => (marker == '-', Some(k)),
        _ => (false, None),
    };
    match children {
        Some(k) => same_marker_depth(k, marker) + usize::from(is_match),
        None => 0,
    }
}

/// True when the character at `i` is absent (start/end of input) or not alphanumeric.
fn boundary(chars: &[char], i: usize) -> bool {
    chars.get(i).is_none_or(|c| !c.is_alphanumeric())
}

fn non_space(chars: &[char], i: usize) -> bool {
    chars.get(i).is_some_and(|&c| !is_space(c))
}

/// A delimiter at `i` may open a span when its left neighbour is a boundary and the next character
/// is not whitespace.
fn can_open(chars: &[char], i: usize) -> bool {
    let left_boundary = i == 0 || boundary(chars, i - 1);
    left_boundary && non_space(chars, i + 1)
}

/// A delimiter at `j` may close a span when the previous character is not whitespace and the right
/// neighbour is a boundary.
fn can_close(chars: &[char], j: usize) -> bool {
    j > 0 && non_space(chars, j - 1) && boundary(chars, j + 1)
}

fn parse_citation(
    chars: &[char],
    i: usize,
    hi: usize,
    autolink: bool,
    depth: usize,
) -> Option<(usize, Vec<Inline>)> {
    if chars.get(i + 1) != Some(&'?') {
        return None;
    }
    let left_boundary = i == 0 || boundary(chars, i - 1);
    if !left_boundary || !non_space(chars, i + 2) {
        return None;
    }
    let mut j = i + 2;
    while j < hi {
        if chars.get(j) == Some(&'?')
            && chars.get(j + 1) == Some(&'?')
            && j > i + 2
            && non_space(chars, j - 1)
            && boundary(chars, j + 2)
        {
            return Some((j + 2, inlines_with(chars, i + 2, j, autolink, depth + 1)));
        }
        j += 1;
    }
    None
}

/// A monospaced span opens at `i` (which holds the first `{` of `{{`) when its left neighbour is a
/// boundary and the character after `{{` is not whitespace.
fn can_open_monospace(chars: &[char], i: usize) -> bool {
    let left_boundary = i == 0 || boundary(chars, i - 1);
    left_boundary && non_space(chars, i + 2)
}

/// A monospaced span closes at `j` (holding the first `}` of `}}`) when the close is non-empty, its
/// left neighbour is not whitespace, and the character after `}}` is a boundary.
fn closes_monospace(chars: &[char], open: usize, j: usize) -> bool {
    j > open + 2 && non_space(chars, j - 1) && boundary(chars, j + 2)
}

/// Finds the `}}` that closes the monospaced span opened at `i`, scanning across nested `{{ … }}`
/// pairs so an inner span does not end the outer one. Returns the index of the closing `}}`, or
/// `None` when the span is never closed.
fn match_monospace_close(chars: &[char], i: usize, hi: usize) -> Option<usize> {
    // A dense run of unbalanced `{` would otherwise make each failed nested open re-scan the whole
    // suffix, so the search cost grows exponentially. A step budget proportional to the span keeps
    // it linear per span: it is far above what any genuine span needs, so a real close is always
    // found, while a pathological run gives up and leaves the braces as literal text.
    let mut budget = hi
        .saturating_sub(i)
        .saturating_mul(8)
        .saturating_add(64)
        .min(200_000);
    match_monospace_close_within(chars, i, hi, &mut budget, 0)
}

fn match_monospace_close_within(
    chars: &[char],
    i: usize,
    hi: usize,
    budget: &mut usize,
    depth: usize,
) -> Option<usize> {
    // Each nested `{{` recurses, so cap the nesting to keep deeply stacked braces off the call stack.
    if depth > MAX_INLINE_DEPTH {
        return None;
    }
    let mut j = i + 2;
    while j < hi {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        if chars.get(j) == Some(&'{')
            && chars.get(j + 1) == Some(&'{')
            && can_open_monospace(chars, j)
            && let Some(nested) = match_monospace_close_within(chars, j, hi, budget, depth + 1)
        {
            j = nested + 2;
            continue;
        }
        if chars.get(j) == Some(&'}')
            && chars.get(j + 1) == Some(&'}')
            && closes_monospace(chars, i, j)
        {
            return Some(j);
        }
        j += 1;
    }
    None
}

fn parse_brace_inline(
    chars: &[char],
    i: usize,
    hi: usize,
    autolink: bool,
    depth: usize,
) -> Option<(Inline, usize)> {
    if chars.get(i + 1) == Some(&'{') {
        // Monospaced span: `{{ … }}`. The close is the `}}` that balances this open, so a nested
        // `{{ … }}` inside is skipped over rather than ending the span early.
        if !can_open_monospace(chars, i) {
            return None;
        }
        let close = match_monospace_close(chars, i, hi)?;
        let inner = inlines_with(chars, i + 2, close, autolink, depth + 1);
        let text = carta_ast::to_plain_text(&inner);
        return Some((Inline::Code(Box::default(), text.into()), close + 2));
    }

    if matches_at(chars, i, "{color:") {
        let value_start = i + "{color:".len();
        let value_end = (value_start..hi).find(|&k| chars.get(k) == Some(&'}'))?;
        let value = color_value(&slice_to_string(chars, value_start, value_end))?;
        let close = match_color_close(chars, value_end + 1, hi)?;
        let inner = inlines_with(chars, value_end + 1, close, autolink, depth + 1);
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: vec![("color".into(), value.into())],
        };
        return Some((Inline::Span(Box::new(attr), inner), close + "{color}".len()));
    }

    if matches_at(chars, i, "{anchor:") {
        let name_start = i + "{anchor:".len();
        let name_end = (name_start..hi).find(|&k| chars.get(k) == Some(&'}'))?;
        let name: String = chars
            .get(name_start..name_end)
            .unwrap_or_default()
            .iter()
            .filter(|c| !is_space(**c))
            .collect();
        let attr = Attr {
            id: name.into(),
            classes: Vec::new(),
            attributes: Vec::new(),
        };
        return Some((Inline::Span(Box::new(attr), Vec::new()), name_end + 1));
    }

    None
}

/// Validates and normalises a colour value. A recognised value is one of: a name of letters (any
/// Unicode letters, not only ASCII); a `#` followed by exactly six hexadecimal digits; or six
/// hexadecimal digits with a leading decimal digit, which is normalised by prepending `#`. Anything
/// else leaves the `{color:…}` markup as literal text.
fn color_value(value: &str) -> Option<String> {
    if let Some(hex) = value.strip_prefix('#') {
        return (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| value.to_string());
    }
    if !value.is_empty() && value.chars().all(char::is_alphabetic) {
        return Some(value.to_string());
    }
    if value.len() == 6
        && value.bytes().all(|b| b.is_ascii_hexdigit())
        && value.bytes().next().is_some_and(|b| b.is_ascii_digit())
    {
        return Some(format!("#{value}"));
    }
    None
}

/// Finds the `{color}` that closes the inline colour span whose content begins at `from`, balancing
/// nested `{color:…}` opens so an inner close does not end the outer span early. Returns the index of
/// the closing token, or `None` when the span is never closed within `from..hi`.
fn match_color_close(chars: &[char], from: usize, hi: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut k = from;
    while k < hi {
        if matches_at(chars, k, "{color:") {
            depth += 1;
            k += "{color:".len();
        } else if matches_at(chars, k, "{color}") {
            depth -= 1;
            if depth == 0 {
                return Some(k);
            }
            k += "{color}".len();
        } else {
            k += 1;
        }
    }
    None
}

fn parse_link(chars: &[char], i: usize, hi: usize, depth: usize) -> Option<(Inline, usize)> {
    let close = (i + 1..hi).find(|&k| chars.get(k) == Some(&']'))?;
    let pipes: Vec<usize> = (i + 1..close)
        .filter(|&k| chars.get(k) == Some(&'|'))
        .collect();
    // A third `|`-segment is allowed only when it names a smart-link style, which becomes a class on
    // the link; any other third segment, or a fourth, is not a link.
    let (label_range, target_start, target_end, smart_class) = match pipes.as_slice() {
        [] => (None, i + 1, close, None),
        [p] => (Some((i + 1, *p)), p + 1, close, None),
        [p1, p2] => {
            let third = slice_to_string(chars, p2 + 1, close);
            if third != "smart-link" && third != "smart-card" {
                return None;
            }
            (Some((i + 1, *p1)), p1 + 1, *p2, Some(third))
        }
        _ => return None,
    };
    let has_pipe = label_range.is_some();
    let target = slice_to_string(chars, target_start, target_end);

    let (url, class, default_label) = classify_link_target(&target, has_pipe)?;

    let label = match label_range {
        Some((ls, le)) if le > ls => inlines_with(chars, ls, le, false, depth + 1),
        _ => vec![Inline::Str(default_label.into())],
    };
    let mut classes: Vec<String> = class.into_iter().map(str::to_string).collect();
    classes.extend(smart_class);
    let attr = Attr {
        id: carta_ast::Text::default(),
        classes: classes.into_iter().map(Into::into).collect(),
        attributes: Vec::new(),
    };
    Some((
        Inline::Link(
            Box::new(attr),
            label,
            Box::new(Target {
                url: url.into(),
                title: carta_ast::Text::default(),
            }),
        ),
        close + 1,
    ))
}

fn classify_link_target(
    target: &str,
    has_pipe: bool,
) -> Option<(String, Option<&'static str>, String)> {
    if target.starts_with('#') {
        return Some((target.to_string(), None, target.to_string()));
    }
    if target.starts_with('~') {
        return Some((target.to_string(), Some("user-account"), target.to_string()));
    }
    if let Some(rest) = target.strip_prefix('^') {
        if has_pipe {
            return None;
        }
        return Some((rest.to_string(), Some("attachment"), rest.to_string()));
    }
    if has_url_prefix(target, LINK_URL_PREFIXES) {
        let label = target
            .strip_prefix("mailto:")
            .map_or_else(|| target.to_string(), str::to_string);
        return Some((target.to_string(), None, label));
    }
    None
}

fn parse_image(chars: &[char], i: usize, hi: usize) -> Option<(Inline, usize)> {
    // The character immediately after the opening `!` must not be whitespace.
    if !non_space(chars, i + 1) {
        return None;
    }
    let close = (i + 1..hi).find(|&k| chars.get(k) == Some(&'!'))?;
    let content = slice_to_string(chars, i + 1, close);
    let (src, props) = match content.split_once('|') {
        Some((s, p)) => (s.to_string(), Some(p.to_string())),
        None => (content, None),
    };
    if src.is_empty() {
        return None;
    }

    let (attr, title) = match props {
        Some(props) => image_properties(&props)?,
        None => (Attr::default(), String::new()),
    };
    Some((
        Inline::Image(
            Box::new(attr),
            Vec::new(),
            Box::new(Target {
                url: src.into(),
                title: title.into(),
            }),
        ),
        close + 1,
    ))
}

/// Parses the property list that follows the `|` in an image, returning its attributes and title, or
/// `None` when the list is malformed (which disqualifies the image). Leading whitespace on the whole
/// list disqualifies it; `thumbnail` is accepted only as the sole property and only with no
/// surrounding whitespace. Otherwise every property is `key=value`: a key carries no whitespace and
/// loses only the whitespace introduced after a separating comma, while a value is kept verbatim so
/// its surrounding whitespace is preserved. A `title` property is the image's title rather than an
/// attribute.
fn image_properties(props: &str) -> Option<(Attr, String)> {
    if props.starts_with(is_space) {
        return None;
    }
    if props == "thumbnail" {
        return Some((
            Attr {
                id: carta_ast::Text::default(),
                classes: vec!["thumbnail".into()],
                attributes: Vec::new(),
            },
            String::new(),
        ));
    }
    let mut attributes = Vec::new();
    let mut title = String::new();
    for (idx, raw) in props.split(',').enumerate() {
        let part = if idx == 0 {
            raw
        } else {
            raw.trim_start_matches(is_space)
        };
        let (key, value) = part.split_once('=')?;
        if key.is_empty() || key.contains(is_space) {
            return None;
        }
        if key == "title" {
            title = value.to_string();
        } else {
            attributes.push((key.to_string(), value.to_string()));
        }
    }
    Some((
        Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        },
        title,
    ))
}

/// If a bare autolink starts at `i`, returns the index just past its URL run. A scheme matches
/// only in lower case. The run extends to the first whitespace or URL terminator.
fn match_bare_url(chars: &[char], i: usize, hi: usize) -> Option<usize> {
    if !BARE_URL_PREFIXES.iter().any(|p| matches_at(chars, i, p)) {
        return None;
    }
    let mut end = i;
    while end < hi
        && chars
            .get(end)
            .is_some_and(|&c| !is_space(c) && !is_url_terminator(c))
    {
        end += 1;
    }
    Some(end)
}

/// Characters that end a bare autolink run.
fn is_url_terminator(c: char) -> bool {
    matches!(c, '|' | ']' | '}' | '<' | '>' | '"' | '[' | '{' | '`')
}

/// Whether `s` begins with one of `prefixes`. A scheme matches only in lower case.
fn has_url_prefix(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}

/// Matches a symbol or emoticon token at `i`. The token is recognised wherever the character that
/// follows it is a boundary (end of input or a non-alphanumeric character); the character before it
/// is irrelevant, so a symbol may abut the end of a preceding word.
fn match_token_symbol(chars: &[char], i: usize, table: &[(&str, char)]) -> Option<(char, usize)> {
    for (token, glyph) in table {
        let len = token.chars().count();
        if matches_at(chars, i, token) && boundary(chars, i + len) {
            return Some((*glyph, len));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The separators this format recognises: ASCII space, tab, and line feed. Code points that Unicode
/// classes as whitespace — a no-break space, em space, form feed, vertical tab, and the like — are
/// ordinary characters here, kept inside the surrounding word rather than splitting it.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n')
}

fn matches_at(chars: &[char], pos: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, ch)| chars.get(pos + k) == Some(&ch))
}

/// Whether a parameterless block macro begins at `pos`. These tokens introduce a block wherever they
/// occur, so they end any paragraph that runs into them.
fn bare_block_macro_at(chars: &[char], pos: usize) -> bool {
    matches_at(chars, pos, "{code}")
        || matches_at(chars, pos, "{noformat}")
        || matches_at(chars, pos, "{quote}")
        || matches_at(chars, pos, "{panel}")
}

fn find_token(chars: &[char], from: usize, token: &str) -> Option<usize> {
    let token_len = token.chars().count();
    let upper = chars.len().saturating_sub(token_len);
    (from..=upper).find(|&k| matches_at(chars, k, token))
}

fn slice_to_string(chars: &[char], start: usize, end: usize) -> String {
    chars.get(start..end).unwrap_or_default().iter().collect()
}

/// Trims leading and trailing whitespace from `start..end`, returning the narrowed range.
fn trim(chars: &[char], start: usize, end: usize) -> (usize, usize) {
    let mut s = start;
    while s < end && chars.get(s).is_some_and(|&c| is_space(c)) {
        s += 1;
    }
    let mut e = end;
    while e > s && chars.get(e - 1).is_some_and(|&c| is_space(c)) {
        e -= 1;
    }
    (s, e)
}

/// The end of `start..end` with trailing whitespace removed, leaving any leading whitespace in place.
fn trim_end(chars: &[char], start: usize, end: usize) -> usize {
    let mut e = end;
    while e > start && chars.get(e - 1).is_some_and(|&c| is_space(c)) {
        e -= 1;
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(input: &str) -> Vec<Block> {
        JiraReader
            .read(input, &ReaderOptions::default())
            .expect("jira reader should not fail")
            .blocks
    }

    fn para(input: &str) -> Vec<Inline> {
        match blocks(input).into_iter().next() {
            Some(Block::Para(inlines)) => inlines,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    fn str_node(text: &str) -> Inline {
        Inline::Str(text.to_string().into())
    }

    #[test]
    fn empty_input_yields_no_blocks() {
        assert!(blocks("").is_empty());
    }

    #[test]
    fn heading_levels() {
        assert_eq!(
            blocks("h2. Title"),
            vec![Block::Header(2, Box::default(), vec![str_node("Title")])]
        );
        // Level seven is not a heading.
        assert!(matches!(blocks("h7. Title").as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn text_effects() {
        assert_eq!(para("*bold*"), vec![Inline::Strong(vec![str_node("bold")])]);
        assert_eq!(para("_em_"), vec![Inline::Emph(vec![str_node("em")])]);
        assert_eq!(
            para("+ins+"),
            vec![Inline::Underline(vec![str_node("ins")])]
        );
        assert_eq!(
            para("^sup^"),
            vec![Inline::Superscript(vec![str_node("sup")])]
        );
        assert_eq!(
            para("~sub~"),
            vec![Inline::Subscript(vec![str_node("sub")])]
        );
    }

    #[test]
    fn nested_effects() {
        assert_eq!(
            para("*_both_*"),
            vec![Inline::Strong(vec![Inline::Emph(vec![str_node("both")])])]
        );
    }

    #[test]
    fn intraword_underscore_is_literal() {
        assert_eq!(para("snake_case_here"), vec![str_node("snake_case_here")]);
    }

    #[test]
    fn monospace_stringifies_inner_markup() {
        assert_eq!(
            para("{{a *b* c}}"),
            vec![Inline::Code(Box::default(), "a b c".to_string().into())]
        );
    }

    #[test]
    fn color_span() {
        assert_eq!(
            para("{color:red}x{color}"),
            vec![Inline::Span(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: Vec::new(),
                    attributes: vec![("color".to_string().into(), "red".to_string().into())],
                }),
                vec![str_node("x")],
            )]
        );
    }

    #[test]
    fn color_block_wraps_in_div() {
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: vec![("color".to_string().into(), "red".to_string().into())],
        };
        assert_eq!(
            blocks("{color:red}\nstuff\n{color}"),
            vec![Block::Div(
                Box::new(attr),
                vec![Block::Para(vec![Inline::LineBreak, str_node("stuff")])],
            )]
        );
        // A close that is not alone on its line keeps the colour inline.
        assert!(matches!(
            blocks("{color:red}a\nb{color}").as_slice(),
            [Block::Para(_)]
        ));
    }

    #[test]
    fn anchor_span() {
        assert_eq!(
            para("{anchor:foo}bar"),
            vec![
                Inline::Span(
                    Box::new(Attr {
                        id: "foo".to_string().into(),
                        classes: Vec::new(),
                        attributes: Vec::new(),
                    }),
                    Vec::new(),
                ),
                str_node("bar"),
            ]
        );
    }

    #[test]
    fn citation_renders_with_em_dash_prefix() {
        assert_eq!(
            para("??cited??"),
            vec![
                str_node("\u{2014}"),
                Inline::Space,
                Inline::Emph(vec![str_node("cited")]),
            ]
        );
    }

    #[test]
    fn dash_folding() {
        assert_eq!(
            para("a -- b"),
            vec![
                str_node("a"),
                Inline::Space,
                str_node("\u{2013}"),
                Inline::Space,
                str_node("b"),
            ]
        );
        assert_eq!(
            para("a --- b"),
            vec![
                str_node("a"),
                Inline::Space,
                str_node("\u{2014}"),
                Inline::Space,
                str_node("b"),
            ]
        );
    }

    #[test]
    fn strikeout_span() {
        assert_eq!(
            para("-gone-"),
            vec![Inline::Strikeout(vec![str_node("gone")])]
        );
    }

    #[test]
    fn escape_emits_literal() {
        assert_eq!(
            para("\\*not bold\\*"),
            vec![str_node("*not"), Inline::Space, str_node("bold*")]
        );
    }

    #[test]
    fn forced_line_break() {
        assert_eq!(
            para("one\\\\two"),
            vec![str_node("one"), Inline::LineBreak, str_node("two")]
        );
    }

    #[test]
    fn newline_within_paragraph_is_hard_break() {
        assert_eq!(
            para("one\ntwo"),
            vec![str_node("one"), Inline::LineBreak, str_node("two")]
        );
    }

    #[test]
    fn horizontal_rule() {
        assert_eq!(blocks("----"), vec![Block::HorizontalRule]);
    }

    #[test]
    fn blockquote_prefix() {
        assert_eq!(
            blocks("bq. quoted"),
            vec![Block::BlockQuote(vec![Block::Para(vec![str_node(
                "quoted"
            )])])]
        );
    }

    #[test]
    fn link_with_label() {
        assert_eq!(
            para("[home|http://example.com]"),
            vec![Inline::Link(
                Box::default(),
                vec![str_node("home")],
                Box::new(Target {
                    url: "http://example.com".to_string().into(),
                    title: carta_ast::Text::default(),
                }),
            )]
        );
    }

    #[test]
    fn link_bare_url_label() {
        assert_eq!(
            para("[http://example.com]"),
            vec![Inline::Link(
                Box::default(),
                vec![str_node("http://example.com")],
                Box::new(Target {
                    url: "http://example.com".to_string().into(),
                    title: carta_ast::Text::default(),
                }),
            )]
        );
    }

    #[test]
    fn attachment_link_carries_class() {
        assert_eq!(
            para("[^file.txt]"),
            vec![Inline::Link(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["attachment".to_string().into()],
                    attributes: Vec::new(),
                }),
                vec![str_node("file.txt")],
                Box::new(Target {
                    url: "file.txt".to_string().into(),
                    title: carta_ast::Text::default(),
                }),
            )]
        );
    }

    #[test]
    fn bare_autolink() {
        assert_eq!(
            para("see http://example.com here"),
            vec![
                str_node("see"),
                Inline::Space,
                Inline::Link(
                    Box::default(),
                    vec![str_node("http://example.com")],
                    Box::new(Target {
                        url: "http://example.com".to_string().into(),
                        title: carta_ast::Text::default(),
                    }),
                ),
                Inline::Space,
                str_node("here"),
            ]
        );
    }

    #[test]
    fn image_with_properties() {
        assert_eq!(
            para("!pic.png|align=right, vspace=4!"),
            vec![Inline::Image(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: Vec::new(),
                    attributes: vec![
                        ("align".to_string().into(), "right".to_string().into()),
                        ("vspace".to_string().into(), "4".to_string().into()),
                    ],
                }),
                Vec::new(),
                Box::new(Target {
                    url: "pic.png".to_string().into(),
                    title: carta_ast::Text::default(),
                }),
            )]
        );
    }

    #[test]
    fn image_thumbnail() {
        assert_eq!(
            para("!pic.png|thumbnail!"),
            vec![Inline::Image(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["thumbnail".to_string().into()],
                    attributes: Vec::new(),
                }),
                Vec::new(),
                Box::new(Target {
                    url: "pic.png".to_string().into(),
                    title: carta_ast::Text::default(),
                }),
            )]
        );
    }

    #[test]
    fn symbols_and_emoticons() {
        assert_eq!(para("(!)"), vec![str_node("\u{2757}")]);
        assert_eq!(para("(y)"), vec![str_node("\u{1F44D}")]);
        assert_eq!(para(":)"), vec![str_node("\u{1F642}")]);
        // A symbol is recognised even when it abuts a preceding word.
        assert_eq!(para("a(!)"), vec![str_node("a\u{2757}")]);
    }

    #[test]
    fn bullet_list_nesting() {
        assert_eq!(
            blocks("* a\n** b"),
            vec![Block::BulletList(vec![vec![
                Block::Para(vec![str_node("a")]),
                Block::BulletList(vec![vec![Block::Para(vec![str_node("b")])]]),
            ]])]
        );
    }

    #[test]
    fn ordered_list_attributes() {
        assert_eq!(
            blocks("# one\n# two"),
            vec![Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::DefaultDelim,
                },
                vec![
                    vec![Block::Para(vec![str_node("one")])],
                    vec![Block::Para(vec![str_node("two")])],
                ],
            )]
        );
    }

    #[test]
    fn distinct_markers_split_lists() {
        assert_eq!(
            blocks("* a\n- b"),
            vec![
                Block::BulletList(vec![vec![Block::Para(vec![str_node("a")])]]),
                Block::BulletList(vec![vec![Block::Para(vec![str_node("b")])]]),
            ]
        );
    }

    #[test]
    fn table_header_and_body() {
        let blocks = blocks("||h1||h2||\n|a|b|");
        let table = match blocks.first() {
            Some(Block::Table(table)) => table,
            other => panic!("expected a table, got {other:?}"),
        };
        assert_eq!(table.col_specs.len(), 2);
        assert_eq!(table.head.rows.len(), 1);
        assert_eq!(table.bodies.len(), 1);
        assert_eq!(table.bodies.first().map(|b| b.body.len()), Some(1));
    }

    #[test]
    fn code_block_default_language() {
        assert_eq!(
            blocks("{code}\nint x = 1;\n{code}"),
            vec![Block::CodeBlock(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["java".to_string().into()],
                    attributes: Vec::new(),
                }),
                "int x = 1;\n".to_string().into(),
            )]
        );
    }

    #[test]
    fn code_block_named_language() {
        assert_eq!(
            blocks("{code:python}\npass\n{code}"),
            vec![Block::CodeBlock(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["python".to_string().into()],
                    attributes: Vec::new(),
                }),
                "pass\n".to_string().into(),
            )]
        );
    }

    #[test]
    fn noformat_has_no_language_class() {
        assert_eq!(
            blocks("{noformat}\nraw\n{noformat}"),
            vec![Block::CodeBlock(Box::default(), "raw\n".to_string().into())]
        );
    }

    #[test]
    fn unterminated_code_block_is_dropped() {
        assert!(blocks("{code}\nno close").is_empty());
    }

    #[test]
    fn quote_macro_holds_blocks() {
        assert_eq!(
            blocks("{quote}\ninside\n{quote}"),
            vec![Block::BlockQuote(vec![Block::Para(vec![str_node(
                "inside"
            )])])]
        );
    }

    #[test]
    fn panel_with_title() {
        assert_eq!(
            blocks("{panel:title=Note}\nbody\n{panel}"),
            vec![Block::Div(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["panel".to_string().into()],
                    attributes: Vec::new(),
                }),
                vec![
                    Block::Div(
                        Box::new(Attr {
                            id: carta_ast::Text::default(),
                            classes: vec!["panelheader".to_string().into()],
                            attributes: Vec::new(),
                        }),
                        vec![Block::Plain(vec![Inline::Strong(vec![str_node("Note")])])],
                    ),
                    Block::Para(vec![str_node("body")]),
                ],
            )]
        );
    }

    #[test]
    fn paragraph_separation() {
        assert_eq!(
            blocks("one\n\ntwo"),
            vec![
                Block::Para(vec![str_node("one")]),
                Block::Para(vec![str_node("two")]),
            ]
        );
    }

    #[test]
    fn leading_space_opens_paragraph() {
        assert_eq!(para(" hello"), vec![Inline::Space, str_node("hello")]);
        assert_eq!(
            para("   indented"),
            vec![Inline::Space, str_node("indented")]
        );
    }

    #[test]
    fn backslash_before_non_escapable_stays_literal() {
        assert_eq!(para("a\\1b"), vec![str_node("a\\1b")]);
    }

    #[test]
    fn named_and_decimal_entities_decode_but_hex_does_not() {
        assert_eq!(
            para("&copy; &#169; &#x41;"),
            vec![
                str_node("\u{a9}"),
                Inline::Space,
                str_node("\u{a9}"),
                Inline::Space,
                str_node("&#x41;"),
            ]
        );
    }

    #[test]
    fn empty_color_macro_is_literal() {
        assert_eq!(para("{color:}x"), vec![str_node("{color:}x")]);
    }

    #[test]
    fn four_dash_run_folds_to_hyphen_and_em_dash() {
        assert_eq!(
            para("a ---- b"),
            vec![
                str_node("a"),
                Inline::Space,
                str_node("-\u{2014}"),
                Inline::Space,
                str_node("b"),
            ]
        );
    }

    #[test]
    fn dash_run_at_line_end_stays_literal() {
        assert_eq!(
            para("x --"),
            vec![str_node("x"), Inline::Space, str_node("--")]
        );
    }

    #[test]
    fn repeated_markers_nest_bullet_lists() {
        assert_eq!(
            blocks("*** x"),
            vec![Block::BulletList(vec![vec![Block::BulletList(vec![
                vec![Block::BulletList(vec![vec![Block::Para(vec![str_node(
                    "x"
                )])]]),]
            ])]])]
        );
    }

    #[test]
    fn indented_marker_still_opens_list() {
        assert_eq!(
            blocks(" * x"),
            vec![Block::BulletList(vec![vec![Block::Para(vec![str_node(
                "x"
            )])]])]
        );
    }

    #[test]
    fn indented_dash_run_is_paragraph_not_rule() {
        assert_eq!(
            blocks("  ----"),
            vec![Block::Para(vec![Inline::Space, str_node("----")])]
        );
    }

    #[test]
    fn same_marker_nesting_caps_at_two() {
        assert_eq!(
            para("*a**b*"),
            vec![Inline::Strong(vec![str_node("a"), str_node("b")])]
        );
        assert_eq!(
            para("**x**"),
            vec![Inline::Strong(vec![Inline::Strong(vec![str_node("x")])])]
        );
    }

    #[test]
    fn strikeout_nests() {
        assert_eq!(
            para("--x--"),
            vec![Inline::Strikeout(vec![Inline::Strikeout(vec![str_node(
                "x"
            )])])]
        );
    }
}
