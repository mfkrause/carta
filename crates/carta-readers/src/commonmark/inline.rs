//! Inline phase: parse the raw text of leaf blocks into inline nodes.
//!
//! Implements the spec's inline algorithm — a left-to-right scan that resolves code spans,
//! autolinks, raw HTML, entities and escapes immediately, records `*`/`_`/`[`/`![` runs on a
//! delimiter stack, resolves links/images at each `]`, and finally collapses emphasis. The raw
//! char-slice scanners it drives (autolinks, HTML tags, entities, link targets) live in `scan`.

use carta_ast::{Attr, Block, Inline, Target};

use super::scan::{
    is_ascii_punctuation, normalize_label, scan_autolink, scan_entity, scan_following_label,
    scan_html_tag, scan_inline_target,
};
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
            Block::RawBlock(carta_ast::Format("html".to_owned()), text.clone())
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
    /// Source index just past a bracket opener, where its raw label text begins. Unused otherwise.
    text_start: usize,
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
                while matches!(self.peek(), Some(' ' | '\t')) {
                    self.pos += 1;
                }
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
                carta_ast::Format("html".to_owned()),
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
            text_start: self.pos,
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
            text_start: self.pos,
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
                && !d.image
            {
                d.active = false;
            }
        }
    }

    /// Attempt to parse what follows `]` as an inline `(...)`, reference, collapsed, or shortcut
    /// link, returning the target and the position after it.
    fn try_link_target(&self, opener_index: usize) -> Option<(Target, usize)> {
        if self.at(0) == Some('(')
            && let Some(result) = scan_inline_target(self.chars, self.pos)
        {
            return Some(result);
        }
        // Reference forms. Labels match on their raw source text (the closing `]` sits at `pos - 1`).
        let label_text = self.raw_label(opener_index);
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

    /// The raw source between a bracket opener and the closing `]` just consumed.
    fn raw_label(&self, opener_index: usize) -> String {
        let start = match self.nodes.get(opener_index) {
            Some(Node::Delimiter(d)) => d.text_start,
            _ => return String::new(),
        };
        self.chars
            .get(start..self.pos.saturating_sub(1))
            .map(|s| s.iter().collect())
            .unwrap_or_default()
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

/// Split a text run into `Str` tokens separated by `Space` inlines, collapsing each run of
/// spaces to a single `Space`.
fn push_text_inlines(out: &mut Vec<Inline>, text: &str) {
    let mut chars = text.chars().peekable();
    let mut word = String::new();
    while let Some(ch) = chars.next() {
        if ch == ' ' {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word)));
            }
            while chars.peek() == Some(&' ') {
                chars.next();
            }
            out.push(Inline::Space);
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word));
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

/// A Unicode punctuation character per the spec: an ASCII punctuation character or anything in the
/// Unicode `P` (punctuation) or `S` (symbol) general categories.
fn is_punctuation(ch: char) -> bool {
    use unicode_general_category::GeneralCategory::{
        ClosePunctuation, ConnectorPunctuation, CurrencySymbol, DashPunctuation, FinalPunctuation,
        InitialPunctuation, MathSymbol, ModifierSymbol, OpenPunctuation, OtherPunctuation,
        OtherSymbol,
    };
    if ch.is_ascii() {
        return is_ascii_punctuation(ch);
    }
    matches!(
        unicode_general_category::get_general_category(ch),
        ConnectorPunctuation
            | DashPunctuation
            | OpenPunctuation
            | ClosePunctuation
            | InitialPunctuation
            | FinalPunctuation
            | OtherPunctuation
            | MathSymbol
            | CurrencySymbol
            | ModifierSymbol
            | OtherSymbol
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
