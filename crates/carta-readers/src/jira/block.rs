//! Block-level parsing of Jira wiki markup: line-prefixed blocks, brace macros, tables, and lists.

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, Row, Table, TableBody, TableFoot, TableHead,
};

use super::inline::{color_value, parse_inlines, plain_inlines, scan_budget};
use super::links::parse_image;
use super::shared::{
    bare_block_macro_at, find_token, is_space, matches_at, slice_to_string, trim, trim_end,
};

pub(super) fn parse_blocks_from_str(input: &str) -> Vec<Block> {
    blocks_from_str(input, true, true)
}

/// Parses the content of a single list item as blocks. A list item carries full block structure
/// (headings, rules, tables, blockquotes, and brace macros), but the stand-alone colour Div is not a
/// list-item construct, so its marker lines stay literal text there.
fn parse_list_item_blocks(input: &str) -> Vec<Block> {
    blocks_from_str(input, false, true)
}

/// Parses the content of a single table cell as blocks. Lists and brace macros carry block
/// structure, but a line whose prefix names a heading, blockquote, or horizontal rule stays
/// paragraph text, and the stand-alone colour Div is not a cell construct.
fn parse_table_cell(input: &str) -> Vec<Block> {
    let mut blocks = blocks_from_str(input, false, false);
    // Trim only top-level paragraphs; ones nested in a list or blockquote keep their whitespace.
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
    // `\r\n` collapses to `\n` and a lone `\r` is dropped, so every CR is removed up front.
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
    /// Whether a line whose prefix names a block (a heading `hN.`, a blockquote `bq.`, or a
    /// horizontal rule `----`) is recognised as that block. Disabled inside a table cell, where
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
        // Exactly four hyphens at line start; any leading indentation makes it a paragraph.
        let e = trim_end(self.chars, self.pos, self.line_end());
        e - self.pos == 4 && (self.pos..e).all(|k| self.chars.get(k) == Some(&'-'))
    }

    fn blockquote_here(&self) -> bool {
        matches_at(self.chars, self.pos, "bq.")
    }

    /// A line beginning with a colour marker (an opening `{color:…}` or a closing `{color}`) starts
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
        // A bare block macro in the content demotes the line to a paragraph, split at the macro.
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
        // A bare block macro in the content demotes the line to a paragraph, split at the macro.
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
        // The first line always joins (guarantees progress); continuation lines join across the
        // newline, which the inline layer renders as a break absorbing surrounding whitespace.
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
        // A bare block macro ends the paragraph; the block layer handles it on the next pass.
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
        // Content opening with a blank line yields no leading paragraph, so no block forms.
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
            // A blank line or rule closes the list; any other non-marker line is item continuation.
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
                // One space separates marker and text; further whitespace is content.
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
/// longer than this depth nests inside the current item, so a lone `*** x` produces three nested
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
            // A longer marker is an implicit parent carrying only its nested child list.
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
        // Pipes nested in brackets, braces, or an image's properties do not split the cell.
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
                Some('!') if depth == 0 => {
                    let mut budget = scan_budget(i, end);
                    match parse_image(chars, i, end, &mut budget) {
                        Some((_, next)) => i = next,
                        None => i += 1,
                    }
                }
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
