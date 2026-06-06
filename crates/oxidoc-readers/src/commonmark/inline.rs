//! Inline phase: parse the raw text of leaf blocks into inline nodes.
//!
//! Implements the spec's inline algorithm — a left-to-right scan that resolves code spans,
//! autolinks, raw HTML, entities and escapes immediately, records `*`/`_`/`[`/`![` runs on a
//! delimiter stack, resolves links/images at each `]`, and finally collapses emphasis. Shared
//! destination/title/label scanners back the block phase's link reference definitions too.

use oxidoc_ast::{Attr, Block, Inline, Target};

use super::{IrBlock, LinkDef, RefMap, para, plain};

pub(crate) fn resolve_blocks(ir: &[IrBlock], refs: &RefMap) -> Vec<Block> {
    ir.iter().map(|block| resolve_block(block, refs)).collect()
}

fn resolve_block(block: &IrBlock, refs: &RefMap) -> Block {
    match block {
        IrBlock::Para(text) => para(parse_inlines(text, refs)),
        IrBlock::Plain(text) => plain(parse_inlines(text, refs)),
        IrBlock::Heading(level, text) => {
            Block::Header(*level, Attr::default(), parse_inlines(text, refs))
        }
        IrBlock::CodeBlock(attr, text) => Block::CodeBlock(attr.clone(), text.clone()),
        IrBlock::RawHtml(text) => {
            Block::RawBlock(oxidoc_ast::Format("html".to_owned()), text.clone())
        }
        IrBlock::ThematicBreak => Block::HorizontalRule,
        IrBlock::BlockQuote(children) => Block::BlockQuote(resolve_blocks(children, refs)),
        IrBlock::BulletList(items) => {
            Block::BulletList(items.iter().map(|i| resolve_blocks(i, refs)).collect())
        }
        IrBlock::OrderedList(attrs, items) => Block::OrderedList(
            attrs.clone(),
            items.iter().map(|i| resolve_blocks(i, refs)).collect(),
        ),
    }
}

/// A node in the in-progress inline list. Delimiter runs stay as nodes until emphasis resolution.
#[derive(Debug, Clone)]
enum Node {
    Text(String),
    Inline(Inline),
    SoftBreak,
    LineBreak,
    Delimiter(Delimiter),
}

// The flags are independent properties of a delimiter run, not a state enum.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
struct Delimiter {
    ch: u8,
    count: usize,
    can_open: bool,
    can_close: bool,
    /// For `[` / `![` openers used by link resolution; inactive once consumed or deactivated.
    active: bool,
    /// Whether this is an image opener (`![`).
    image: bool,
}

fn parse_inlines(text: &str, refs: &RefMap) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut parser = InlineParser {
        chars: &chars,
        pos: 0,
        nodes: Vec::new(),
        refs,
    };
    parser.run();
    let mut nodes = parser.nodes;
    process_emphasis(&mut nodes, 0);
    collapse(nodes)
}

struct InlineParser<'a> {
    chars: &'a [char],
    pos: usize,
    nodes: Vec<Node>,
    refs: &'a RefMap,
}

impl InlineParser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn run(&mut self) {
        while let Some(ch) = self.peek() {
            match ch {
                '\\' => self.backslash(),
                '`' => self.code_span(),
                '<' => self.left_angle(),
                '&' => self.entity(),
                '\n' => self.line_ending(),
                '*' | '_' => self.emphasis_run(ch as u8),
                '[' => {
                    self.pos += 1;
                    self.push_open_bracket(false);
                }
                '!' if self.at(1) == Some('[') => {
                    self.pos += 2;
                    self.push_open_bracket(true);
                }
                ']' => self.close_bracket(),
                _ => {
                    self.pos += 1;
                    self.push_text(ch);
                }
            }
        }
    }

    fn push_text(&mut self, ch: char) {
        if let Some(Node::Text(text)) = self.nodes.last_mut() {
            text.push(ch);
        } else {
            self.nodes.push(Node::Text(ch.to_string()));
        }
    }

    fn push_str(&mut self, value: &str) {
        if let Some(Node::Text(text)) = self.nodes.last_mut() {
            text.push_str(value);
        } else {
            self.nodes.push(Node::Text(value.to_owned()));
        }
    }

    fn backslash(&mut self) {
        self.pos += 1;
        match self.peek() {
            Some('\n') => {
                self.pos += 1;
                self.nodes.push(Node::LineBreak);
            }
            Some(ch) if is_ascii_punctuation(ch) => {
                self.pos += 1;
                self.push_text(ch);
            }
            _ => self.push_text('\\'),
        }
    }

    fn code_span(&mut self) {
        let start = self.pos;
        let mut open = 0;
        while self.peek() == Some('`') {
            self.pos += 1;
            open += 1;
        }
        // Find a closing run of exactly `open` backticks.
        let mut scan = self.pos;
        while scan < self.chars.len() {
            if self.chars.get(scan).copied() == Some('`') {
                let mut close = 0;
                while self.chars.get(scan + close).copied() == Some('`') {
                    close += 1;
                }
                if close == open {
                    let content: String = self
                        .chars
                        .get(self.pos..scan)
                        .map(|s| s.iter().collect())
                        .unwrap_or_default();
                    self.pos = scan + close;
                    self.nodes.push(Node::Inline(Inline::Code(
                        Attr::default(),
                        normalize_code(&content),
                    )));
                    return;
                }
                scan += close;
            } else {
                scan += 1;
            }
        }
        // No closing run: emit the opening backticks literally.
        let literal: String = self
            .chars
            .get(start..self.pos)
            .map(|s| s.iter().collect())
            .unwrap_or_default();
        self.push_str(&literal);
    }

    fn left_angle(&mut self) {
        if let Some((inline, next)) = scan_autolink(self.chars, self.pos) {
            self.pos = next;
            self.nodes.push(Node::Inline(inline));
            return;
        }
        if let Some((html, next)) = scan_html_tag(self.chars, self.pos) {
            self.pos = next;
            self.nodes.push(Node::Inline(Inline::RawInline(
                oxidoc_ast::Format("html".to_owned()),
                html,
            )));
            return;
        }
        self.pos += 1;
        self.push_text('<');
    }

    fn entity(&mut self) {
        if let Some((decoded, next)) = scan_entity(self.chars, self.pos) {
            self.pos = next;
            self.push_str(&decoded);
        } else {
            self.pos += 1;
            self.push_text('&');
        }
    }

    fn line_ending(&mut self) {
        // Trailing spaces before the newline determine hard vs soft break.
        let hard = matches!(self.nodes.last(), Some(Node::Text(t)) if t.ends_with("  "));
        let backslash_hard = matches!(self.nodes.last(), Some(Node::LineBreak));
        if let Some(Node::Text(text)) = self.nodes.last_mut() {
            let trimmed = text.trim_end_matches(' ').to_owned();
            *text = trimmed;
            if text.is_empty() {
                self.nodes.pop();
            }
        }
        self.pos += 1;
        // Skip leading spaces/tabs of the next line.
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.pos += 1;
        }
        if hard || backslash_hard {
            self.nodes.push(Node::LineBreak);
        } else {
            self.nodes.push(Node::SoftBreak);
        }
    }

    fn emphasis_run(&mut self, ch: u8) {
        let start = self.pos;
        while self.peek() == Some(ch as char) {
            self.pos += 1;
        }
        let count = self.pos - start;
        let before = if start == 0 {
            None
        } else {
            self.chars.get(start - 1).copied()
        };
        let after = self.peek();
        let (can_open, can_close) = flanking(ch, before, after);
        self.nodes.push(Node::Delimiter(Delimiter {
            ch,
            count,
            can_open,
            can_close,
            active: true,
            image: false,
        }));
    }

    fn push_open_bracket(&mut self, image: bool) {
        self.nodes.push(Node::Delimiter(Delimiter {
            ch: b'[',
            count: 1,
            can_open: true,
            can_close: false,
            active: true,
            image,
        }));
    }

    fn close_bracket(&mut self) {
        self.pos += 1;
        let Some(opener_index) = self.last_bracket_opener() else {
            self.push_text(']');
            return;
        };
        let is_image = matches!(self.nodes.get(opener_index), Some(Node::Delimiter(d)) if d.image);
        let active = matches!(self.nodes.get(opener_index), Some(Node::Delimiter(d)) if d.active);
        if !active {
            self.literalize_bracket(opener_index);
            self.push_text(']');
            return;
        }

        if let Some((target, next)) = self.try_link_target(opener_index) {
            self.pos = next;
            self.build_link(opener_index, is_image, target);
            if !is_image {
                self.deactivate_earlier_brackets(opener_index);
            }
            return;
        }
        // Not a valid link: the opener reverts to its literal `[` / `![`, and `]` stays literal.
        self.literalize_bracket(opener_index);
        self.push_text(']');
    }

    /// Turn an unmatched bracket opener back into the literal text it stands for.
    fn literalize_bracket(&mut self, opener_index: usize) {
        if let Some(node) = self.nodes.get_mut(opener_index)
            && let Node::Delimiter(d) = node
        {
            let literal = if d.image { "![" } else { "[" };
            *node = Node::Text(literal.to_owned());
        }
    }

    fn last_bracket_opener(&self) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, node)| matches!(node, Node::Delimiter(d) if d.ch == b'[').then_some(i))
    }

    fn deactivate_earlier_brackets(&mut self, before: usize) {
        for node in self.nodes.get_mut(..before).into_iter().flatten() {
            if let Node::Delimiter(d) = node
                && d.ch == b'['
            {
                d.active = false;
            }
        }
    }

    /// Attempt to parse what follows `]` as an inline `(...)`, reference, collapsed, or shortcut
    /// link, returning the target and the position after it.
    fn try_link_target(&mut self, opener_index: usize) -> Option<(Target, usize)> {
        if self.at(0) == Some('(')
            && let Some(result) = scan_inline_target(self.chars, self.pos)
        {
            return Some(result);
        }
        // Reference forms.
        let label_text = self.bracket_label_text(opener_index);
        if let Some((label, next)) = scan_following_label(self.chars, self.pos) {
            let key = if label.is_empty() {
                normalize_label(&label_text)
            } else {
                normalize_label(&label)
            };
            if let Some(def) = self.refs.get(&key) {
                return Some((def_target(def), next));
            }
            return None;
        }
        // Shortcut reference.
        let key = normalize_label(&label_text);
        if let Some(def) = self.refs.get(&key) {
            return Some((def_target(def), self.pos));
        }
        None
    }

    fn bracket_label_text(&self, opener_index: usize) -> String {
        let mut out = String::new();
        for node in self.nodes.get(opener_index + 1..).into_iter().flatten() {
            match node {
                Node::Text(t) | Node::Inline(Inline::Code(_, t)) => out.push_str(t),
                Node::SoftBreak | Node::LineBreak => out.push(' '),
                Node::Delimiter(d) => {
                    for _ in 0..d.count {
                        out.push(d.ch as char);
                    }
                }
                Node::Inline(_) => {}
            }
        }
        out
    }

    fn build_link(&mut self, opener_index: usize, is_image: bool, target: Target) {
        let mut inner: Vec<Node> = self.nodes.split_off(opener_index + 1);
        self.nodes.pop(); // remove the opener delimiter
        process_emphasis(&mut inner, 0);
        let content = collapse(inner);
        let inline = if is_image {
            Inline::Image(Attr::default(), content, target)
        } else {
            Inline::Link(Attr::default(), content, target)
        };
        self.nodes.push(Node::Inline(inline));
    }
}

fn def_target(def: &LinkDef) -> Target {
    Target {
        url: def.url.clone(),
        title: def.title.clone(),
    }
}

/// Resolve emphasis/strong delimiters in `nodes`, starting at `stack_bottom`.
fn process_emphasis(nodes: &mut Vec<Node>, stack_bottom: usize) {
    let mut closer = stack_bottom;
    while closer < nodes.len() {
        let is_closer = matches!(nodes.get(closer), Some(Node::Delimiter(d)) if d.can_close && (d.ch == b'*' || d.ch == b'_'));
        if !is_closer {
            closer += 1;
            continue;
        }
        let closer_ch = if let Some(Node::Delimiter(d)) = nodes.get(closer) {
            d.ch
        } else {
            closer += 1;
            continue;
        };
        // Find a matching opener below.
        let mut opener = None;
        let mut index = closer;
        while index > stack_bottom {
            index -= 1;
            if let Some(Node::Delimiter(d)) = nodes.get(index)
                && d.can_open
                && d.ch == closer_ch
                && emphasis_match(d, nodes, closer)
            {
                opener = Some(index);
                break;
            }
        }
        let Some(opener_index) = opener else {
            // No opener; if this delimiter also can't open, it's inert.
            if let Some(Node::Delimiter(d)) = nodes.get(closer)
                && !d.can_open
            {
                convert_delimiter_to_text(nodes, closer);
            }
            closer += 1;
            continue;
        };

        let use_count = {
            let opener_count = delimiter_count(nodes, opener_index);
            let closer_count = delimiter_count(nodes, closer);
            if opener_count >= 2 && closer_count >= 2 {
                2
            } else {
                1
            }
        };

        // Wrap the nodes strictly between opener and closer, then place the wrapped inline back
        // between the two (now adjacent) delimiters before trimming their counts.
        let inner: Vec<Node> = nodes.drain(opener_index + 1..closer).collect();
        let content = collapse(inner);
        let wrapped = if use_count == 2 {
            Inline::Strong(content)
        } else {
            Inline::Emph(content)
        };
        let emph_index = opener_index + 1;
        nodes.insert(emph_index, Node::Inline(wrapped));

        // Decrement counts and drop emptied delimiters, closer first so the opener index holds.
        let closer_index = emph_index + 1;
        decrement_delimiter(nodes, closer_index, use_count);
        decrement_delimiter(nodes, opener_index, use_count);
        let mut removable = [closer_index, opener_index];
        removable.sort_unstable_by(|a, b| b.cmp(a));
        for index in removable {
            if matches!(nodes.get(index), Some(Node::Delimiter(d)) if d.count == 0) {
                nodes.remove(index);
            }
        }

        closer = stack_bottom;
    }
    // Any leftover emphasis delimiters become literal text.
    for index in 0..nodes.len() {
        if matches!(nodes.get(index), Some(Node::Delimiter(d)) if d.ch == b'*' || d.ch == b'_') {
            convert_delimiter_to_text(nodes, index);
        }
    }
}

fn emphasis_match(opener: &Delimiter, nodes: &[Node], closer: usize) -> bool {
    let Some(Node::Delimiter(closer_delim)) = nodes.get(closer) else {
        return false;
    };
    // Rule of 3: when either run can both open and close, their combined length must not be a
    // multiple of 3 unless both lengths are themselves multiples of 3.
    let either_both =
        (opener.can_open && opener.can_close) || (closer_delim.can_open && closer_delim.can_close);
    if either_both {
        let sum = opener.count + closer_delim.count;
        if sum.is_multiple_of(3)
            && (!opener.count.is_multiple_of(3) || !closer_delim.count.is_multiple_of(3))
        {
            return false;
        }
    }
    true
}

fn delimiter_count(nodes: &[Node], index: usize) -> usize {
    match nodes.get(index) {
        Some(Node::Delimiter(d)) => d.count,
        _ => 0,
    }
}

fn decrement_delimiter(nodes: &mut [Node], index: usize, by: usize) {
    if let Some(Node::Delimiter(d)) = nodes.get_mut(index) {
        d.count = d.count.saturating_sub(by);
    }
}

fn convert_delimiter_to_text(nodes: &mut [Node], index: usize) {
    if let Some(node) = nodes.get_mut(index)
        && let Node::Delimiter(d) = node
        && (d.ch == b'*' || d.ch == b'_')
    {
        let literal: String = std::iter::repeat_n(d.ch as char, d.count).collect();
        *node = Node::Text(literal);
    }
}

/// Collapse the node list into final inlines: leftover delimiters become text, adjacent text is
/// merged, and text is split into `Str`/`Space` runs.
fn collapse(nodes: Vec<Node>) -> Vec<Inline> {
    let mut text = String::new();
    let mut out: Vec<Inline> = Vec::new();
    let flush = |text: &mut String, out: &mut Vec<Inline>| {
        if !text.is_empty() {
            push_text_inlines(out, text);
            text.clear();
        }
    };
    for node in nodes {
        match node {
            Node::Text(t) => text.push_str(&t),
            Node::Delimiter(d) => {
                for _ in 0..d.count {
                    text.push(d.ch as char);
                }
            }
            Node::Inline(inline) => {
                flush(&mut text, &mut out);
                out.push(inline);
            }
            Node::SoftBreak => {
                flush(&mut text, &mut out);
                out.push(Inline::SoftBreak);
            }
            Node::LineBreak => {
                flush(&mut text, &mut out);
                out.push(Inline::LineBreak);
            }
        }
    }
    flush(&mut text, &mut out);
    out
}

/// Split a text run into `Str` tokens separated by single `Space` inlines.
fn push_text_inlines(out: &mut Vec<Inline>, text: &str) {
    let mut first = true;
    for piece in text.split(' ') {
        if !first {
            out.push(Inline::Space);
        }
        first = false;
        if !piece.is_empty() {
            out.push(Inline::Str(piece.to_owned()));
        }
    }
}

fn flanking(ch: u8, before: Option<char>, after: Option<char>) -> (bool, bool) {
    let before_ws = before.is_none_or(is_unicode_whitespace);
    let after_ws = after.is_none_or(is_unicode_whitespace);
    let before_punct = before.is_some_and(is_punctuation);
    let after_punct = after.is_some_and(is_punctuation);

    let left_flanking = !after_ws && (!after_punct || before_ws || before_punct);
    let right_flanking = !before_ws && (!before_punct || after_ws || after_punct);

    if ch == b'_' {
        let can_open = left_flanking && (!right_flanking || before_punct);
        let can_close = right_flanking && (!left_flanking || after_punct);
        (can_open, can_close)
    } else {
        (left_flanking, right_flanking)
    }
}

fn is_unicode_whitespace(ch: char) -> bool {
    ch == ' '
        || ch == '\t'
        || ch == '\n'
        || ch == '\u{0c}'
        || ch == '\u{0b}'
        || ch == '\r'
        || ch.is_whitespace()
}

fn is_punctuation(ch: char) -> bool {
    is_ascii_punctuation(ch) || ch.is_ascii_punctuation()
}

fn is_ascii_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '!' | '"'
            | '#'
            | '$'
            | '%'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | '-'
            | '.'
            | '/'
            | ':'
            | ';'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '_'
            | '`'
            | '{'
            | '|'
            | '}'
            | '~'
    )
}

/// Normalize the interior of a code span: line endings to spaces, and if it both begins and ends
/// with a space (and is not all spaces), strip one space from each end.
fn normalize_code(content: &str) -> String {
    let collapsed: String = content
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let bytes = collapsed.as_bytes();
    if collapsed.len() >= 2
        && bytes.first() == Some(&b' ')
        && bytes.last() == Some(&b' ')
        && !collapsed.chars().all(|c| c == ' ')
    {
        collapsed
            .get(1..collapsed.len() - 1)
            .unwrap_or("")
            .to_owned()
    } else {
        collapsed
    }
}

// --- Shared scanners (also used by link reference definitions) ---

fn scan_autolink(chars: &[char], start: usize) -> Option<(Inline, usize)> {
    if chars.get(start).copied() != Some('<') {
        return None;
    }
    let mut end = start + 1;
    let mut content = String::new();
    while let Some(&ch) = chars.get(end) {
        if ch == '>' {
            break;
        }
        if ch == '<' || ch.is_whitespace() {
            return None;
        }
        content.push(ch);
        end += 1;
    }
    if chars.get(end).copied() != Some('>') {
        return None;
    }
    let after = end + 1;
    if is_uri_autolink(&content) {
        let target = Target {
            url: content.clone(),
            title: String::new(),
        };
        return Some((
            Inline::Link(Attr::default(), vec![Inline::Str(content)], target),
            after,
        ));
    }
    if is_email_autolink(&content) {
        let url = format!("mailto:{content}");
        let target = Target {
            url,
            title: String::new(),
        };
        return Some((
            Inline::Link(Attr::default(), vec![Inline::Str(content)], target),
            after,
        ));
    }
    None
}

fn is_uri_autolink(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once(':') else {
        return false;
    };
    let scheme_ok = (2..=32).contains(&scheme.len())
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-');
    scheme_ok && !text.chars().any(|c| c.is_control() || c == ' ')
}

fn is_email_autolink(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(c))
        && domain.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

fn scan_html_tag(chars: &[char], start: usize) -> Option<(String, usize)> {
    // Minimal recognizer: an open/close tag, comment, processing instruction, declaration, or
    // CDATA. Returns the verbatim text and the position after `>`.
    let rest: String = chars
        .get(start..)
        .map(|s| s.iter().collect())
        .unwrap_or_default();
    let tag = match_html_tag(&rest)?;
    let len = tag.chars().count();
    Some((tag, start + len))
}

fn match_html_tag(rest: &str) -> Option<String> {
    if !rest.starts_with('<') {
        return None;
    }
    let end = rest.find('>')?;
    let candidate = rest.get(..=end)?;
    let inner = candidate.get(1..candidate.len() - 1)?;
    let valid = is_html_tag_inner(inner);
    valid.then(|| candidate.to_owned())
}

fn is_html_tag_inner(inner: &str) -> bool {
    if inner.is_empty() {
        return false;
    }
    let first = inner.chars().next();
    match first {
        Some('/') => inner.get(1..).is_some_and(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric())
        }),
        Some(c) if c.is_ascii_alphabetic() => true,
        Some('!' | '?') => true,
        _ => false,
    }
}

fn scan_entity(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start).copied() != Some('&') {
        return None;
    }
    let semi =
        (start + 1..(start + 33).min(chars.len())).find(|&i| chars.get(i).copied() == Some(';'))?;
    let body: String = chars
        .get(start + 1..semi)
        .map(|s| s.iter().collect())
        .unwrap_or_default();
    let decoded = decode_entity(&body)?;
    Some((decoded, semi + 1))
}

fn decode_entity(body: &str) -> Option<String> {
    if let Some(num) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
        let code = u32::from_str_radix(num, 16).ok()?;
        return Some(code_point_char(code));
    }
    if let Some(num) = body.strip_prefix('#') {
        let code: u32 = num.parse().ok()?;
        return Some(code_point_char(code));
    }
    named_entity(body)
}

fn code_point_char(code: u32) -> String {
    if code == 0 {
        return '\u{fffd}'.to_string();
    }
    char::from_u32(code).unwrap_or('\u{fffd}').to_string()
}

fn named_entity(name: &str) -> Option<String> {
    let value = match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "copy" => "\u{a9}",
        "reg" => "\u{ae}",
        "hellip" => "\u{2026}",
        "mdash" => "\u{2014}",
        "ndash" => "\u{2013}",
        "auml" => "\u{e4}",
        "ouml" => "\u{f6}",
        "uuml" => "\u{fc}",
        _ => return None,
    };
    Some(value.to_owned())
}

/// Scan an inline link tail `(url "title")` beginning at `pos` (which points at `(`).
fn scan_inline_target(chars: &[char], pos: usize) -> Option<(Target, usize)> {
    let mut index = pos + 1;
    skip_inline_whitespace(chars, &mut index);
    let (url, next) = scan_destination(chars, index)?;
    index = next;
    skip_inline_whitespace(chars, &mut index);
    let mut title = String::new();
    if matches!(chars.get(index).copied(), Some('"' | '\'' | '(')) {
        let (parsed, after) = scan_title(chars, index)?;
        title = parsed;
        index = after;
        skip_inline_whitespace(chars, &mut index);
    }
    if chars.get(index).copied() != Some(')') {
        return None;
    }
    Some((
        Target {
            url: unescape_string(&url),
            title: unescape_string(&title),
        },
        index + 1,
    ))
}

fn scan_destination(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut index = start;
    if chars.get(index).copied() == Some('<') {
        index += 1;
        let mut out = String::new();
        while let Some(&ch) = chars.get(index) {
            match ch {
                '>' => return Some((out, index + 1)),
                '<' | '\n' => return None,
                '\\' if chars
                    .get(index + 1)
                    .is_some_and(|c| is_ascii_punctuation(*c)) =>
                {
                    if let Some(&next) = chars.get(index + 1) {
                        out.push(next);
                    }
                    index += 2;
                }
                _ => {
                    out.push(ch);
                    index += 1;
                }
            }
        }
        return None;
    }
    let mut out = String::new();
    let mut depth = 0;
    while let Some(&ch) = chars.get(index) {
        match ch {
            c if c.is_whitespace() => break,
            c if c.is_control() => break,
            '(' => {
                depth += 1;
                out.push('(');
                index += 1;
            }
            ')' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                out.push(')');
                index += 1;
            }
            '\\' if chars
                .get(index + 1)
                .is_some_and(|c| is_ascii_punctuation(*c)) =>
            {
                if let Some(&next) = chars.get(index + 1) {
                    out.push(next);
                }
                index += 2;
            }
            _ => {
                out.push(ch);
                index += 1;
            }
        }
    }
    if out.is_empty() && depth == 0 {
        return Some((out, index));
    }
    if depth != 0 {
        return None;
    }
    Some((out, index))
}

fn scan_title(chars: &[char], start: usize) -> Option<(String, usize)> {
    let open = chars.get(start).copied()?;
    let close = match open {
        '"' => '"',
        '\'' => '\'',
        '(' => ')',
        _ => return None,
    };
    let mut index = start + 1;
    let mut out = String::new();
    while let Some(&ch) = chars.get(index) {
        if ch == close {
            return Some((out, index + 1));
        }
        if ch == '\\'
            && chars
                .get(index + 1)
                .is_some_and(|c| is_ascii_punctuation(*c))
        {
            if let Some(&next) = chars.get(index + 1) {
                out.push(next);
            }
            index += 2;
            continue;
        }
        out.push(ch);
        index += 1;
    }
    None
}

/// Scan a `[label]` immediately following a `]`, returning the raw label and the next position.
fn scan_following_label(chars: &[char], pos: usize) -> Option<(String, usize)> {
    if chars.get(pos).copied() != Some('[') {
        return None;
    }
    let mut index = pos + 1;
    let mut out = String::new();
    while let Some(&ch) = chars.get(index) {
        match ch {
            ']' => return Some((out, index + 1)),
            '[' => return None,
            '\\' if chars
                .get(index + 1)
                .is_some_and(|c| is_ascii_punctuation(*c)) =>
            {
                out.push('\\');
                if let Some(&next) = chars.get(index + 1) {
                    out.push(next);
                }
                index += 2;
            }
            _ => {
                out.push(ch);
                index += 1;
            }
        }
    }
    None
}

fn skip_inline_whitespace(chars: &[char], index: &mut usize) {
    while matches!(chars.get(*index).copied(), Some(' ' | '\t' | '\n')) {
        *index += 1;
    }
}

/// Normalize a link label per the spec: trim, collapse internal whitespace to single spaces, and
/// case-fold (here, lowercase).
pub(crate) fn normalize_label(label: &str) -> String {
    let collapsed: Vec<&str> = label.split_whitespace().collect();
    collapsed.join(" ").to_lowercase()
}

/// Remove backslash escapes of ASCII punctuation from a string, leaving other backslashes intact.
pub(crate) fn unescape_string(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while let Some(&ch) = chars.get(index) {
        if ch == '\\'
            && let Some(&next) = chars.get(index + 1)
            && is_ascii_punctuation(next)
        {
            out.push(next);
            index += 2;
            continue;
        }
        if ch == '&'
            && let Some((decoded, next)) = scan_entity(&chars, index)
        {
            out.push_str(&decoded);
            index = next;
            continue;
        }
        out.push(ch);
        index += 1;
    }
    out
}

/// Parse a leading link reference definition from `text`. Returns the normalized label, the
/// resolved definition, and the unconsumed remainder of `text`.
pub(crate) fn parse_link_reference_definition(text: &str) -> Option<(String, LinkDef, &str)> {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    skip_spaces_up_to_three(&chars, &mut index);
    if chars.get(index).copied() != Some('[') {
        return None;
    }
    index += 1;
    let mut label = String::new();
    let mut closed = false;
    while let Some(&ch) = chars.get(index) {
        match ch {
            ']' => {
                closed = true;
                index += 1;
                break;
            }
            '[' => return None,
            '\\' if chars
                .get(index + 1)
                .is_some_and(|c| is_ascii_punctuation(*c)) =>
            {
                label.push('\\');
                if let Some(&next) = chars.get(index + 1) {
                    label.push(next);
                }
                index += 2;
            }
            _ => {
                label.push(ch);
                index += 1;
            }
        }
    }
    if !closed || chars.get(index).copied() != Some(':') {
        return None;
    }
    index += 1;
    skip_inline_whitespace_no_double_newline(&chars, &mut index)?;
    let (url, next) = scan_destination(&chars, index)?;
    if url.is_empty() {
        return None;
    }
    index = next;

    // Optional title; if parsing the title fails, the definition still stands without it as long
    // as the line ends after the destination.
    let saved = index;
    let mut title = String::new();
    let mut has_title = false;
    let mut spaces = index;
    let mut saw_newline = false;
    while matches!(chars.get(spaces).copied(), Some(' ' | '\t' | '\n')) {
        if chars.get(spaces).copied() == Some('\n') {
            saw_newline = true;
        }
        spaces += 1;
    }
    if matches!(chars.get(spaces).copied(), Some('"' | '\'' | '('))
        && let Some((parsed, after)) = scan_title(&chars, spaces)
    {
        let mut tail = after;
        skip_blanks_to_line_end(&chars, &mut tail);
        if at_line_end(&chars, tail) {
            title = parsed;
            has_title = true;
            index = tail;
        }
    }
    if !has_title {
        index = saved;
        skip_blanks_to_line_end(&chars, &mut index);
        if !at_line_end(&chars, index) {
            return None;
        }
        let _ = saw_newline;
    }
    // Consume the trailing newline.
    if chars.get(index).copied() == Some('\n') {
        index += 1;
    }

    let normalized = normalize_label(&label);
    if normalized.is_empty() {
        return None;
    }
    let def = LinkDef {
        url: unescape_string(&url),
        title: unescape_string(&title),
    };
    let consumed_bytes: usize = chars
        .get(..index)
        .map_or(0, |s| s.iter().map(|c| c.len_utf8()).sum());
    let rest = text.get(consumed_bytes..).unwrap_or("");
    Some((normalized, def, rest))
}

fn skip_spaces_up_to_three(chars: &[char], index: &mut usize) {
    let mut count = 0;
    while count < 3 && chars.get(*index).copied() == Some(' ') {
        *index += 1;
        count += 1;
    }
}

fn skip_inline_whitespace_no_double_newline(chars: &[char], index: &mut usize) -> Option<()> {
    let mut newlines = 0;
    while let Some(&ch) = chars.get(*index) {
        match ch {
            ' ' | '\t' => *index += 1,
            '\n' => {
                newlines += 1;
                if newlines > 1 {
                    return None;
                }
                *index += 1;
            }
            _ => break,
        }
    }
    Some(())
}

fn skip_blanks_to_line_end(chars: &[char], index: &mut usize) {
    while matches!(chars.get(*index).copied(), Some(' ' | '\t')) {
        *index += 1;
    }
}

fn at_line_end(chars: &[char], index: usize) -> bool {
    matches!(chars.get(index).copied(), None | Some('\n'))
}
