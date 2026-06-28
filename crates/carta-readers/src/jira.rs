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
    let chars: Vec<char> = input.chars().collect();
    BlockParser {
        chars: &chars,
        pos: 0,
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
        (start..end).all(|k| self.chars.get(k).is_some_and(|c| c.is_whitespace()))
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

    fn table_here(&self) -> bool {
        self.chars.get(self.pos) == Some(&'|')
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
        let (ts, te) = trim(self.chars, self.pos + 3, e);
        let inlines = parse_inlines(self.chars, ts, te);
        self.advance_line();
        Some(Block::Header(level, Attr::default(), inlines))
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
        let (ts, te) = trim(self.chars, self.pos + 3, e);
        let inlines = parse_inlines(self.chars, ts, te);
        self.advance_line();
        Some(Block::BlockQuote(vec![Block::Para(inlines)]))
    }

    fn parse_paragraph(&mut self) -> Block {
        let mut lines: Vec<Vec<Inline>> = Vec::new();
        // The first line is always part of the paragraph; this guarantees forward progress. Its
        // leading whitespace is kept (it collapses to a single leading space), while continuation
        // lines are trimmed on both sides.
        let e = self.line_end();
        let te = trim_end(self.chars, self.pos, e);
        lines.push(parse_inlines(self.chars, self.pos, te));
        self.advance_line();
        loop {
            if self.at_end() {
                break;
            }
            let e = self.line_end();
            if self.is_blank(self.pos, e) || self.line_starts_block() {
                break;
            }
            let (ts, te) = trim(self.chars, self.pos, e);
            lines.push(parse_inlines(self.chars, ts, te));
            self.advance_line();
        }
        Block::Para(join_lines(lines))
    }

    // --- tables ------------------------------------------------------------

    fn parse_table(&mut self) -> Block {
        let mut rows: Vec<Vec<(bool, String)>> = Vec::new();
        while !self.at_end() {
            let e = self.line_end();
            if self.is_blank(self.pos, e) || self.chars.get(self.pos) != Some(&'|') {
                break;
            }
            rows.push(parse_table_row(self.chars, self.pos, e));
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
            if self.is_blank(self.pos, e) {
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
                let marker: String = slice_to_string(self.chars, marker_start, k);
                // Exactly one space separates the marker from the item text; any further leading
                // whitespace is part of the content and kept. Trailing whitespace is trimmed.
                let content_start = k + 1;
                let te = trim_end(self.chars, content_start, e);
                items.push(ListItem {
                    marker,
                    lines: vec![parse_inlines(self.chars, content_start, te)],
                });
                self.advance_line();
            } else if self.line_starts_block() {
                break;
            } else if let Some(last) = items.last_mut() {
                let (ts, te) = trim(self.chars, self.pos, e);
                last.lines.push(parse_inlines(self.chars, ts, te));
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
        match kind {
            MacroKind::Code | MacroKind::Noformat => {
                Some(self.parse_verbatim_macro(kind, params.as_deref(), fence_end))
            }
            MacroKind::Quote => Some(self.parse_quote(fence_end)),
            MacroKind::Panel => Some(self.parse_panel(params.as_deref(), fence_end)),
        }
    }

    fn parse_verbatim_macro(
        &mut self,
        kind: MacroKind,
        params: Option<&str>,
        fence_end: usize,
    ) -> Vec<Block> {
        let close_token = if matches!(kind, MacroKind::Code) {
            "{code}"
        } else {
            "{noformat}"
        };
        let open_newline = (fence_end..self.len()).find(|&k| self.chars.get(k) == Some(&'\n'));
        let mut cur = match open_newline {
            Some(nl) => nl + 1,
            None => self.len(),
        };

        let mut content = String::new();
        let mut closed = false;
        while cur < self.len() {
            let le = self.line_end_from(cur);
            let (ts, _) = trim(self.chars, cur, le);
            if matches_at(self.chars, ts, close_token) {
                cur = if le < self.len() { le + 1 } else { le };
                closed = true;
                break;
            }
            content.push_str(&slice_to_string(self.chars, cur, le));
            content.push('\n');
            cur = if le < self.len() { le + 1 } else { le };
        }

        if !closed {
            // An unterminated verbatim macro consumes the rest of the input and yields no block.
            self.pos = self.len();
            return Vec::new();
        }
        self.pos = cur;

        let (classes, attributes) = verbatim_params(kind, params);
        vec![Block::CodeBlock(
            Attr {
                id: String::new(),
                classes,
                attributes,
            },
            content,
        )]
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

    fn parse_panel(&mut self, params: Option<&str>, fence_end: usize) -> Vec<Block> {
        let Some(content) = self.take_fenced(fence_end, "{panel}") else {
            return Vec::new();
        };
        let (title, attributes) = panel_params(params);
        let mut inner = Vec::new();
        if let Some(title) = title {
            inner.push(Block::Div(
                Attr {
                    id: String::new(),
                    classes: vec!["panelheader".to_string()],
                    attributes: Vec::new(),
                },
                vec![Block::Plain(vec![Inline::Strong(plain_inlines(&title))])],
            ));
        }
        inner.extend(parse_blocks_from_str(&content));
        vec![Block::Div(
            Attr {
                id: String::new(),
                classes: vec!["panel".to_string()],
                attributes,
            },
            inner,
        )]
    }
}

struct ListItem {
    marker: String,
    lines: Vec<Vec<Inline>>,
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
                    item_blocks.push(Block::Para(join_lines(item.lines.clone())));
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

fn join_lines(lines: Vec<Vec<Inline>>) -> Vec<Inline> {
    let mut out = Vec::new();
    for (idx, line) in lines.into_iter().enumerate() {
        if idx > 0 {
            out.push(Inline::LineBreak);
        }
        out.extend(line);
    }
    out
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
        // link or a brace span so a link's own `|` does not split the cell.
        let cell_start = i;
        let mut depth = 0i32;
        while i < end {
            match chars.get(i) {
                Some('[' | '{') => depth += 1,
                Some(']' | '}') => depth = depth.saturating_sub(1),
                Some('|') if depth == 0 => break,
                _ => {}
            }
            i += 1;
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
            Some((_, text)) if !text.is_empty() => vec![Block::Para(inlines_of(text))],
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

fn inlines_of(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    parse_inlines(&chars, 0, chars.len())
}

/// Tokenises `text` into inlines without interpreting markup: whitespace runs become
/// [`Inline::Space`] and every other run becomes an [`Inline::Str`]. Used for a panel title, whose
/// text is rendered verbatim inside its header.
fn plain_inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word)));
            }
            if out.last() != Some(&Inline::Space) {
                out.push(Inline::Space);
            }
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word));
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

/// Parses the character range `lo..hi` into inline nodes: it scans the text into tokens, pairs the
/// flanking delimiters into spans, and folds the result into a flat list of inlines. Flanking
/// decisions consult the real neighbouring characters via absolute indices, so a range bounded to a
/// single line will not let markup escape that line.
fn parse_inlines(chars: &[char], lo: usize, hi: usize) -> Vec<Inline> {
    finalize(resolve(scan_tokens(chars, lo, hi)))
}

fn push_text(pending: &mut String, toks: &mut Vec<Tok>) {
    if !pending.is_empty() {
        toks.push(Tok::Text(std::mem::take(pending)));
    }
}

/// Scans `lo..hi` left to right into tokens: literal runs accumulate into [`Tok::Text`], a flanking
/// delimiter becomes a [`Tok::Delim`], and a self-contained construct (link, image, brace span,
/// citation, autolink, symbol) becomes a [`Tok::Atom`].
fn scan_tokens(chars: &[char], lo: usize, hi: usize) -> Vec<Tok> {
    let mut toks: Vec<Tok> = Vec::new();
    let mut pending = String::new();
    let mut i = lo;

    while i < hi {
        let Some(&c) = chars.get(i) else {
            break;
        };

        if c.is_whitespace() {
            push_text(&mut pending, &mut toks);
            toks.push(Tok::Atom(Inline::Space));
            i += 1;
            while i < hi && chars.get(i).is_some_and(|c| c.is_whitespace()) {
                i += 1;
            }
            continue;
        }

        let prev_alnum = i > 0 && chars.get(i - 1).is_some_and(|c| c.is_alphanumeric());

        if !prev_alnum && let Some(end) = match_bare_url(chars, i, hi) {
            push_text(&mut pending, &mut toks);
            let url = slice_to_string(chars, i, end);
            toks.push(Tok::Atom(Inline::Link(
                Attr::default(),
                vec![Inline::Str(url.clone())],
                Target {
                    url,
                    title: String::new(),
                },
            )));
            i = end;
            continue;
        }

        match c {
            '\\' => {
                i = scan_backslash(chars, i, hi, &mut pending, &mut toks);
            }
            '&' => {
                if let Some((text, next)) = read_entity(chars, i, hi) {
                    pending.push_str(&text);
                    i = next;
                } else {
                    pending.push('&');
                    i += 1;
                }
            }
            '?' => {
                if let Some((next, inner)) = parse_citation(chars, i, hi) {
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
                if let Some((node, next)) = scan_construct(c, chars, i, hi) {
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

/// Reads a character reference at `i` (which holds `&`). A named reference or a decimal numeric
/// reference resolves to its replacement text; a hexadecimal reference is not recognised and stays
/// literal. Returns the decoded text and the index just past the closing `;`.
fn read_entity(chars: &[char], i: usize, hi: usize) -> Option<(String, usize)> {
    if chars.get(i + 1) == Some(&'#') {
        let start = i + 2;
        let mut k = start;
        while k < hi && chars.get(k).is_some_and(char::is_ascii_digit) {
            k += 1;
        }
        if k > start && chars.get(k) == Some(&';') {
            let digits = slice_to_string(chars, start, k);
            if let Ok(code) = digits.parse::<u32>() {
                return Some((crate::entities::code_point(code).to_string(), k + 1));
            }
        }
        return None;
    }
    let start = i + 1;
    let mut k = start;
    while k < hi && chars.get(k).is_some_and(char::is_ascii_alphanumeric) {
        k += 1;
    }
    if k > start && chars.get(k) == Some(&';') {
        let name = slice_to_string(chars, start, k);
        if let Some(text) = crate::entities::lookup_named(&name) {
            return Some((text.to_string(), k + 1));
        }
    }
    None
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
fn scan_construct(c: char, chars: &[char], i: usize, hi: usize) -> Option<(Inline, usize)> {
    match c {
        '[' => parse_link(chars, i, hi),
        '!' => parse_image(chars, i, hi),
        _ => parse_brace_inline(chars, i, hi),
    }
}

/// Handles a backslash at `i`. A doubled backslash is a forced line break that absorbs the
/// whitespace around it; a backslash before one of a fixed set of punctuation marks escapes that mark
/// to a literal; before anything else the backslash itself stays literal. Returns the next position.
fn scan_backslash(
    chars: &[char],
    i: usize,
    hi: usize,
    pending: &mut String,
    toks: &mut Vec<Tok>,
) -> usize {
    if i + 1 < hi && chars.get(i + 1) == Some(&'\\') {
        push_text(pending, toks);
        if matches!(toks.last(), Some(Tok::Atom(Inline::Space))) {
            toks.pop();
        }
        toks.push(Tok::Atom(Inline::LineBreak));
        let mut j = i + 2;
        while j < hi && chars.get(j).is_some_and(|c| c.is_whitespace()) {
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

/// Index of the nearest preceding open delimiter that carries `marker`.
fn find_opener(acc: &[Tok], marker: char) -> Option<usize> {
    acc.iter()
        .rposition(|t| matches!(t, Tok::Delim { marker: m, open: true, .. } if *m == marker))
}

/// Pairs flanking delimiters into spans. A closing delimiter binds to the nearest open delimiter of
/// the same marker with non-empty content between them; same-marker spans nest at most two deep.
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
            && let Some(open_idx) = find_opener(&acc, marker)
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
            Tok::Text(s) => Inline::Str(s),
            Tok::Delim { marker, .. } => Inline::Str(marker.to_string()),
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
    chars.get(i).is_some_and(|c| !c.is_whitespace())
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

fn parse_citation(chars: &[char], i: usize, hi: usize) -> Option<(usize, Vec<Inline>)> {
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
            return Some((j + 2, parse_inlines(chars, i + 2, j)));
        }
        j += 1;
    }
    None
}

fn parse_brace_inline(chars: &[char], i: usize, hi: usize) -> Option<(Inline, usize)> {
    if chars.get(i + 1) == Some(&'{') {
        // Monospaced span: `{{ … }}`.
        let left_boundary = i == 0 || boundary(chars, i - 1);
        if !left_boundary || !non_space(chars, i + 2) {
            return None;
        }
        let mut j = i + 2;
        while j < hi {
            if chars.get(j) == Some(&'}')
                && chars.get(j + 1) == Some(&'}')
                && j > i + 2
                && non_space(chars, j - 1)
                && boundary(chars, j + 2)
            {
                let inner = parse_inlines(chars, i + 2, j);
                let text = carta_ast::to_plain_text(&inner);
                return Some((Inline::Code(Attr::default(), text), j + 2));
            }
            j += 1;
        }
        return None;
    }

    if matches_at(chars, i, "{color:") {
        let value_start = i + "{color:".len();
        let value_end = (value_start..hi).find(|&k| chars.get(k) == Some(&'}'))?;
        let value = slice_to_string(chars, value_start, value_end);
        if !is_color_value(&value) {
            return None;
        }
        let close = find_token(chars.get(..hi).unwrap_or(chars), value_end + 1, "{color}")?;
        let inner = parse_inlines(chars, value_end + 1, close);
        let attr = Attr {
            id: String::new(),
            classes: Vec::new(),
            attributes: vec![("color".to_string(), value)],
        };
        return Some((Inline::Span(attr, inner), close + "{color}".len()));
    }

    if matches_at(chars, i, "{anchor:") {
        let name_start = i + "{anchor:".len();
        let name_end = (name_start..hi).find(|&k| chars.get(k) == Some(&'}'))?;
        let name = slice_to_string(chars, name_start, name_end);
        let attr = Attr {
            id: name,
            classes: Vec::new(),
            attributes: Vec::new(),
        };
        return Some((Inline::Span(attr, Vec::new()), name_end + 1));
    }

    None
}

/// A colour span's value is either a name of ASCII letters or a `#` followed by exactly six
/// hexadecimal digits; anything else leaves the `{color:…}` markup as literal text.
fn is_color_value(value: &str) -> bool {
    if let Some(hex) = value.strip_prefix('#') {
        return hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphabetic())
}

fn parse_link(chars: &[char], i: usize, hi: usize) -> Option<(Inline, usize)> {
    let close = (i + 1..hi).find(|&k| chars.get(k) == Some(&']'))?;
    let pipes: Vec<usize> = (i + 1..close)
        .filter(|&k| chars.get(k) == Some(&'|'))
        .collect();
    if pipes.len() > 1 {
        return None;
    }
    let (label_range, target_start) = match pipes.first() {
        Some(&p) => (Some((i + 1, p)), p + 1),
        None => (None, i + 1),
    };
    let has_pipe = label_range.is_some();
    let target = slice_to_string(chars, target_start, close);

    let (url, class, default_label) = classify_link_target(&target, has_pipe)?;

    let label = match label_range {
        Some((ls, le)) if le > ls => parse_inlines(chars, ls, le),
        _ => vec![Inline::Str(default_label)],
    };
    let attr = Attr {
        id: String::new(),
        classes: class.into_iter().map(str::to_string).collect(),
        attributes: Vec::new(),
    };
    Some((
        Inline::Link(
            attr,
            label,
            Target {
                url,
                title: String::new(),
            },
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

    let mut classes = Vec::new();
    let mut attributes = Vec::new();
    if let Some(props) = props {
        let parts: Vec<&str> = props.split(',').map(str::trim).collect();
        if parts == ["thumbnail"] {
            classes.push("thumbnail".to_string());
        } else if !parts.is_empty() && parts.iter().all(|p| p.contains('=')) {
            for part in parts {
                if let Some((key, value)) = part.split_once('=') {
                    attributes.push((key.trim().to_string(), value.trim().to_string()));
                }
            }
        } else {
            return None;
        }
    }

    let attr = Attr {
        id: String::new(),
        classes,
        attributes,
    };
    Some((
        Inline::Image(
            attr,
            Vec::new(),
            Target {
                url: src,
                title: String::new(),
            },
        ),
        close + 1,
    ))
}

/// If a bare autolink starts at `i`, returns the index just past its URL run. The run extends to
/// the first whitespace or one of `|`, `]`, `}`.
fn match_bare_url(chars: &[char], i: usize, hi: usize) -> Option<usize> {
    if !BARE_URL_PREFIXES.iter().any(|p| matches_at_ci(chars, i, p)) {
        return None;
    }
    let mut end = i;
    while end < hi
        && chars
            .get(end)
            .is_some_and(|c| !c.is_whitespace() && !matches!(c, '|' | ']' | '}'))
    {
        end += 1;
    }
    Some(end)
}

/// Whether `s` begins with one of `prefixes`, ignoring ASCII case.
fn has_url_prefix(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| {
        s.get(..p.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(p))
    })
}

fn match_token_symbol(chars: &[char], i: usize, table: &[(&str, char)]) -> Option<(char, usize)> {
    for (token, glyph) in table {
        let len = token.chars().count();
        if matches_at(chars, i, token)
            && (i == 0 || boundary(chars, i - 1))
            && boundary(chars, i + len)
        {
            return Some((*glyph, len));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn matches_at(chars: &[char], pos: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, ch)| chars.get(pos + k) == Some(&ch))
}

fn matches_at_ci(chars: &[char], pos: usize, needle: &str) -> bool {
    needle.chars().enumerate().all(|(k, ch)| {
        chars
            .get(pos + k)
            .is_some_and(|c| c.eq_ignore_ascii_case(&ch))
    })
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
    while s < end && chars.get(s).is_some_and(|c| c.is_whitespace()) {
        s += 1;
    }
    let mut e = end;
    while e > s && chars.get(e - 1).is_some_and(|c| c.is_whitespace()) {
        e -= 1;
    }
    (s, e)
}

/// The end of `start..end` with trailing whitespace removed, leaving any leading whitespace in place.
fn trim_end(chars: &[char], start: usize, end: usize) -> usize {
    let mut e = end;
    while e > start && chars.get(e - 1).is_some_and(|c| c.is_whitespace()) {
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
        Inline::Str(text.to_string())
    }

    #[test]
    fn empty_input_yields_no_blocks() {
        assert!(blocks("").is_empty());
    }

    #[test]
    fn heading_levels() {
        assert_eq!(
            blocks("h2. Title"),
            vec![Block::Header(2, Attr::default(), vec![str_node("Title")])]
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
            vec![Inline::Code(Attr::default(), "a b c".to_string())]
        );
    }

    #[test]
    fn color_span() {
        assert_eq!(
            para("{color:red}x{color}"),
            vec![Inline::Span(
                Attr {
                    id: String::new(),
                    classes: Vec::new(),
                    attributes: vec![("color".to_string(), "red".to_string())],
                },
                vec![str_node("x")],
            )]
        );
    }

    #[test]
    fn anchor_span() {
        assert_eq!(
            para("{anchor:foo}bar"),
            vec![
                Inline::Span(
                    Attr {
                        id: "foo".to_string(),
                        classes: Vec::new(),
                        attributes: Vec::new(),
                    },
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
                Attr::default(),
                vec![str_node("home")],
                Target {
                    url: "http://example.com".to_string(),
                    title: String::new(),
                },
            )]
        );
    }

    #[test]
    fn link_bare_url_label() {
        assert_eq!(
            para("[http://example.com]"),
            vec![Inline::Link(
                Attr::default(),
                vec![str_node("http://example.com")],
                Target {
                    url: "http://example.com".to_string(),
                    title: String::new(),
                },
            )]
        );
    }

    #[test]
    fn attachment_link_carries_class() {
        assert_eq!(
            para("[^file.txt]"),
            vec![Inline::Link(
                Attr {
                    id: String::new(),
                    classes: vec!["attachment".to_string()],
                    attributes: Vec::new(),
                },
                vec![str_node("file.txt")],
                Target {
                    url: "file.txt".to_string(),
                    title: String::new(),
                },
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
                    Attr::default(),
                    vec![str_node("http://example.com")],
                    Target {
                        url: "http://example.com".to_string(),
                        title: String::new(),
                    },
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
                Attr {
                    id: String::new(),
                    classes: Vec::new(),
                    attributes: vec![
                        ("align".to_string(), "right".to_string()),
                        ("vspace".to_string(), "4".to_string()),
                    ],
                },
                Vec::new(),
                Target {
                    url: "pic.png".to_string(),
                    title: String::new(),
                },
            )]
        );
    }

    #[test]
    fn image_thumbnail() {
        assert_eq!(
            para("!pic.png|thumbnail!"),
            vec![Inline::Image(
                Attr {
                    id: String::new(),
                    classes: vec!["thumbnail".to_string()],
                    attributes: Vec::new(),
                },
                Vec::new(),
                Target {
                    url: "pic.png".to_string(),
                    title: String::new(),
                },
            )]
        );
    }

    #[test]
    fn symbols_and_emoticons() {
        assert_eq!(para("(!)"), vec![str_node("\u{2757}")]);
        assert_eq!(para("(y)"), vec![str_node("\u{1F44D}")]);
        assert_eq!(para(":)"), vec![str_node("\u{1F642}")]);
        // A symbol abutting a word is literal text.
        assert_eq!(para("a(!)"), vec![str_node("a(!)")]);
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
                Attr {
                    id: String::new(),
                    classes: vec!["java".to_string()],
                    attributes: Vec::new(),
                },
                "int x = 1;\n".to_string(),
            )]
        );
    }

    #[test]
    fn code_block_named_language() {
        assert_eq!(
            blocks("{code:python}\npass\n{code}"),
            vec![Block::CodeBlock(
                Attr {
                    id: String::new(),
                    classes: vec!["python".to_string()],
                    attributes: Vec::new(),
                },
                "pass\n".to_string(),
            )]
        );
    }

    #[test]
    fn noformat_has_no_language_class() {
        assert_eq!(
            blocks("{noformat}\nraw\n{noformat}"),
            vec![Block::CodeBlock(Attr::default(), "raw\n".to_string())]
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
                Attr {
                    id: String::new(),
                    classes: vec!["panel".to_string()],
                    attributes: Vec::new(),
                },
                vec![
                    Block::Div(
                        Attr {
                            id: String::new(),
                            classes: vec!["panelheader".to_string()],
                            attributes: Vec::new(),
                        },
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
