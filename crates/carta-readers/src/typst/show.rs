//! Show rules: what `#show` matches, and what it puts in its place.
//!
//! A rule governs the content from its declaration to the end of the region holding it, so the
//! statement reads that content as blocks and rewrites the parts its selector picks out. Rewritten
//! content is not offered to the same rule again, which is what keeps a rule that reintroduces what
//! it matched from running forever.

use carta_ast::{Block, Inline};
use fancy_regex::Regex;

use super::{Arg, Function, MAX_DEPTH, MAX_ITERATIONS, Parser, Value, positional_text, table_rows};

/// What a show rule picks out.
enum Selector {
    /// A literal run of characters inside a text run.
    Text(String),
    /// A pattern over the characters of a text run.
    Pattern(Box<Regex>),
    /// Elements of one kind, narrowed by the fields a `where` call fixes.
    Element(String, Vec<(String, Value)>),
}

/// What a show rule puts in place of what it picked out.
enum Transform {
    /// A closure, or a function bound by `#let`, called with the match.
    Function(Function),
    /// An element function, which rebuilds the match as the element it names.
    Element(String),
    /// A value that stands in for the match whatever the match was.
    Fixed(Value),
    /// A restyling with no counterpart in the document model, which leaves the match as it is.
    Keep,
}

/// A parsed `#show` statement.
struct Rule {
    /// What to pick out, or the whole governed content when the statement names no selector.
    selector: Option<Selector>,
    /// What to put in its place.
    transform: Transform,
}

/// The indentation width the spaces opening a line stand for.
fn indent_width(opening: &str) -> usize {
    opening
        .chars()
        .map(|c| match c {
            ' ' => 1,
            '\t' => 2,
            _ => 0,
        })
        .sum()
}

impl Parser {
    /// Read a `#show` statement and rewrite the content it governs.
    ///
    /// A form this reader cannot carry out leaves the statement's line behind and nothing else, so
    /// the content that follows is read as it stands.
    pub(super) fn show_statement(&mut self) -> Value {
        self.skip_spaces();
        let selector = if self.peek() == Some(':') {
            None
        } else {
            let Some(parsed) = self.selector() else {
                self.skip_line_comment();
                return Value::Nothing;
            };
            Some(parsed)
        };
        self.skip_spaces();
        if !self.eat(':') {
            self.skip_line_comment();
            return Value::Nothing;
        }
        self.skip_spaces();
        let rule = Rule {
            selector,
            transform: self.transform(),
        };
        let governed = self.governed_content();
        if rule.selector.is_some() {
            return Value::Content(self.rewrite_blocks(&rule, governed));
        }
        if matches!(rule.transform, Transform::Keep) {
            return Value::Content(governed);
        }
        self.substitute(&rule.transform, Value::Content(governed))
    }

    /// Read the content a rule governs: the rest of the region holding it.
    ///
    /// The break between the statement and that content still separates them, so it opens the
    /// content and is dropped again wherever the content stands on its own as blocks.
    fn governed_content(&mut self) -> Vec<Block> {
        let mut whitespace = String::new();
        let mut index = self.pos;
        while let Some(c) = self.at(index).filter(char::is_ascii_whitespace) {
            whitespace.push(c);
            index = index.saturating_add(1);
        }
        // Content that falls back out of the region has left the rule behind.
        if let Some((_, opening)) = whitespace.rsplit_once('\n')
            && indent_width(opening) < self.indent
        {
            return Vec::new();
        }
        let mut governed = self.blocks(self.indent);
        if let Some(separator) = super::edge_separator(&whitespace)
            && let Some(inlines) = super::first_edge_inlines(&mut governed)
        {
            inlines.insert(0, separator);
        }
        governed
    }

    /// Read the selector before a show rule's colon.
    fn selector(&mut self) -> Option<Selector> {
        if self.peek() == Some('"') {
            return Some(Selector::Text(self.string_literal()));
        }
        if !self.peek().is_some_and(|c| c.is_alphabetic() || c == '_') {
            return None;
        }
        let name = self.read_path();
        if name == "regex" {
            let args = self.call_arguments()?;
            let pattern = Regex::new(&positional_text(&args)).ok()?;
            return Some(Selector::Pattern(Box::new(pattern)));
        }
        let Some(element) = name.strip_suffix(".where") else {
            return Some(Selector::Element(name, Vec::new()));
        };
        let fields = self
            .call_arguments()?
            .into_iter()
            .filter_map(|arg| Some((arg.name?, arg.value)))
            .collect();
        Some(Selector::Element(element.to_string(), fields))
    }

    /// Read the parenthesized arguments at the cursor.
    fn call_arguments(&mut self) -> Option<Vec<Arg>> {
        if self.peek() != Some('(') {
            return None;
        }
        let close = self.balanced('(', ')')?;
        let args = self.arguments(self.pos.saturating_add(1), close);
        self.pos = close.saturating_add(1);
        Some(args)
    }

    /// Read the transform after a show rule's colon.
    fn transform(&mut self) -> Transform {
        if let Some(closure) = self.closure_signature() {
            return Transform::Function(closure);
        }
        if !self.peek().is_some_and(|c| c.is_alphabetic() || c == '_') {
            return Transform::Fixed(self.argument_value());
        }
        let start = self.pos;
        let name = self.read_path();
        if name == "set" {
            self.skip_line_comment();
            return Transform::Keep;
        }
        if name == "none" {
            return Transform::Fixed(Value::Nothing);
        }
        if let Some(function) = self.functions.get(&name).cloned() {
            return Transform::Function(function);
        }
        // A bare name is the function itself; anything longer is an expression to evaluate.
        if matches!(self.peek(), Some('(' | '[')) {
            self.pos = start;
            return Transform::Fixed(self.argument_value());
        }
        Transform::Element(name)
    }

    /// The value a transform yields for one match.
    fn substitute(&mut self, transform: &Transform, matched: Value) -> Value {
        if self.depth >= MAX_DEPTH {
            return matched;
        }
        self.depth = self.depth.saturating_add(1);
        let value = match transform {
            Transform::Keep => matched,
            Transform::Fixed(fixed) => fixed.clone(),
            Transform::Element(name) => {
                let args = vec![Arg {
                    name: None,
                    value: matched.clone(),
                }];
                match self.call(name, args) {
                    Value::Nothing => matched,
                    built => built,
                }
            }
            Transform::Function(function) => {
                let args = vec![Arg {
                    name: None,
                    value: matched,
                }];
                self.call_bound(&function.clone(), &args)
            }
        };
        self.depth = self.depth.saturating_sub(1);
        value
    }

    /// Rewrite a block sequence, replacing what the rule picks out and descending into the rest.
    fn rewrite_blocks(&mut self, rule: &Rule, blocks: Vec<Block>) -> Vec<Block> {
        let mut out = Vec::with_capacity(blocks.len());
        for block in blocks {
            if block_selected(rule, &block) {
                let value = self.substitute(&rule.transform, Value::Content(vec![block]));
                out.extend(value.into_blocks());
            } else {
                out.push(self.rewrite_within(rule, block));
            }
        }
        out
    }

    /// Rewrite the content a block holds, leaving the block itself in place.
    fn rewrite_within(&mut self, rule: &Rule, block: Block) -> Block {
        match block {
            Block::Para(inlines) => Block::Para(self.rewrite_inlines(rule, inlines)),
            Block::Plain(inlines) => Block::Plain(self.rewrite_inlines(rule, inlines)),
            Block::Header(level, attr, inlines) => {
                Block::Header(level, attr, self.rewrite_inlines(rule, inlines))
            }
            Block::LineBlock(lines) => Block::LineBlock(
                lines
                    .into_iter()
                    .map(|line| self.rewrite_inlines(rule, line))
                    .collect(),
            ),
            Block::BlockQuote(children) => Block::BlockQuote(self.rewrite_blocks(rule, children)),
            Block::Div(attr, children) => Block::Div(attr, self.rewrite_blocks(rule, children)),
            Block::BulletList(items) => Block::BulletList(self.rewrite_items(rule, items)),
            Block::OrderedList(attributes, items) => {
                Block::OrderedList(attributes, self.rewrite_items(rule, items))
            }
            Block::DefinitionList(entries) => Block::DefinitionList(
                entries
                    .into_iter()
                    .map(|(term, definitions)| {
                        (
                            self.rewrite_inlines(rule, term),
                            self.rewrite_items(rule, definitions),
                        )
                    })
                    .collect(),
            ),
            Block::Figure(attr, mut caption, children) => {
                if let Some(short) = caption.short.take() {
                    caption.short = Some(self.rewrite_inlines(rule, short));
                }
                caption.long = self.rewrite_blocks(rule, caption.long);
                Block::Figure(attr, caption, self.rewrite_blocks(rule, children))
            }
            Block::Table(mut table) => {
                if let Some(short) = table.caption.short.take() {
                    table.caption.short = Some(self.rewrite_inlines(rule, short));
                }
                let long = std::mem::take(&mut table.caption.long);
                table.caption.long = self.rewrite_blocks(rule, long);
                for row in table_rows(&mut table) {
                    for cell in row {
                        let content = std::mem::take(cell);
                        *cell = self.rewrite_blocks(rule, content);
                    }
                }
                Block::Table(table)
            }
            other => other,
        }
    }

    /// Rewrite each item of a list.
    fn rewrite_items(&mut self, rule: &Rule, items: Vec<Vec<Block>>) -> Vec<Vec<Block>> {
        items
            .into_iter()
            .map(|item| self.rewrite_blocks(rule, item))
            .collect()
    }

    /// Rewrite an inline sequence, replacing what the rule picks out and descending into the rest.
    fn rewrite_inlines(&mut self, rule: &Rule, inlines: Vec<Inline>) -> Vec<Inline> {
        let mut out = Vec::with_capacity(inlines.len());
        for inline in inlines {
            if let Some(matched) = inline_selected(rule, &inline) {
                let value = self.substitute(&rule.transform, matched);
                out.extend(value.into_inlines());
                continue;
            }
            if let Inline::Str(text) = &inline
                && let Some(pieces) = self.rewrite_text(rule, text.as_str())
            {
                out.extend(pieces);
                continue;
            }
            out.push(self.rewrite_inline_within(rule, inline));
        }
        out
    }

    /// Rewrite the runs of a text inline a text or pattern selector picks out, or `None` when the
    /// selector picks out none of it.
    fn rewrite_text(&mut self, rule: &Rule, text: &str) -> Option<Vec<Inline>> {
        let spans = match rule.selector.as_ref()? {
            Selector::Text(needle) => literal_spans(text, needle),
            Selector::Pattern(pattern) => pattern_spans(pattern, text),
            Selector::Element(..) => return None,
        };
        if spans.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        let mut cursor = 0;
        for (start, end) in spans {
            if let Some(head) = text.get(cursor..start).filter(|head| !head.is_empty()) {
                out.push(Inline::Str(head.into()));
            }
            let matched = Value::Str(text.get(start..end).unwrap_or_default().to_string());
            let value = self.substitute(&rule.transform, matched);
            out.extend(value.into_inlines());
            cursor = end;
        }
        if let Some(tail) = text.get(cursor..).filter(|tail| !tail.is_empty()) {
            out.push(Inline::Str(tail.into()));
        }
        Some(out)
    }

    /// Rewrite the content an inline holds, leaving the inline itself in place.
    fn rewrite_inline_within(&mut self, rule: &Rule, inline: Inline) -> Inline {
        match inline {
            Inline::Emph(children) => Inline::Emph(self.rewrite_inlines(rule, children)),
            Inline::Strong(children) => Inline::Strong(self.rewrite_inlines(rule, children)),
            Inline::Underline(children) => Inline::Underline(self.rewrite_inlines(rule, children)),
            Inline::Strikeout(children) => Inline::Strikeout(self.rewrite_inlines(rule, children)),
            Inline::Superscript(children) => {
                Inline::Superscript(self.rewrite_inlines(rule, children))
            }
            Inline::Subscript(children) => Inline::Subscript(self.rewrite_inlines(rule, children)),
            Inline::SmallCaps(children) => Inline::SmallCaps(self.rewrite_inlines(rule, children)),
            Inline::Quoted(kind, children) => {
                Inline::Quoted(kind, self.rewrite_inlines(rule, children))
            }
            Inline::Cite(citations, children) => {
                Inline::Cite(citations, self.rewrite_inlines(rule, children))
            }
            Inline::Link(attr, children, target) => {
                Inline::Link(attr, self.rewrite_inlines(rule, children), target)
            }
            Inline::Image(attr, children, target) => {
                Inline::Image(attr, self.rewrite_inlines(rule, children), target)
            }
            Inline::Span(attr, children) => {
                Inline::Span(attr, self.rewrite_inlines(rule, children))
            }
            Inline::Note(blocks) => Inline::Note(self.rewrite_blocks(rule, blocks)),
            other => other,
        }
    }
}

/// Whether an element selector picks out a block.
fn block_selected(rule: &Rule, block: &Block) -> bool {
    let Some(Selector::Element(name, fields)) = rule.selector.as_ref() else {
        return false;
    };
    match (name.as_str(), block) {
        ("heading", Block::Header(level, ..)) => fields.iter().all(|(key, want)| {
            matches!(key.as_str(), "level" | "depth")
                && want.as_number() == i32::try_from(*level).ok().map(f64::from)
        }),
        ("raw", Block::CodeBlock(..)) => fields
            .iter()
            .all(|(key, want)| key == "block" && want.is_truthy()),
        ("par", Block::Para(_))
        | ("list", Block::BulletList(_))
        | ("enum", Block::OrderedList(..))
        | ("terms", Block::DefinitionList(_))
        | ("table", Block::Table(_))
        | ("figure", Block::Figure(..))
        | ("quote", Block::BlockQuote(_))
        | ("line", Block::HorizontalRule) => fields.is_empty(),
        _ => false,
    }
}

/// The value an element selector binds for an inline it picks out, or `None` when it picks out
/// neither the inline nor its text.
fn inline_selected(rule: &Rule, inline: &Inline) -> Option<Value> {
    let Some(Selector::Element(name, fields)) = rule.selector.as_ref() else {
        return None;
    };
    if name == "text" {
        // A text element carries its characters, not child content, so the match is that text.
        return match inline {
            Inline::Str(text) if fields.is_empty() => Some(Value::Str(text.to_string())),
            _ => None,
        };
    }
    let selected = match (name.as_str(), inline) {
        ("emph", Inline::Emph(_))
        | ("strong", Inline::Strong(_))
        | ("underline", Inline::Underline(_))
        | ("strike", Inline::Strikeout(_))
        | ("super", Inline::Superscript(_))
        | ("sub", Inline::Subscript(_))
        | ("smallcaps", Inline::SmallCaps(_))
        | ("quote", Inline::Quoted(..))
        | ("image", Inline::Image(..))
        | ("footnote", Inline::Note(_))
        | ("math.equation", Inline::Math(..))
        | ("cite" | "ref", Inline::Cite(..)) => fields.is_empty(),
        ("link", Inline::Link(attr, ..)) => {
            fields.is_empty() && !attr.classes.iter().any(|class| class == "ref")
        }
        ("ref", Inline::Link(attr, ..)) => {
            fields.is_empty() && attr.classes.iter().any(|class| class == "ref")
        }
        ("raw", Inline::Code(..)) => fields
            .iter()
            .all(|(key, want)| key == "block" && !want.is_truthy()),
        _ => false,
    };
    selected.then(|| Value::Inlines(vec![inline.clone()]))
}

/// Where a literal run occurs in a text, left to right and without overlap.
fn literal_spans(text: &str, needle: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    if needle.is_empty() {
        return spans;
    }
    let mut cursor = 0;
    while let Some(found) = text.get(cursor..).and_then(|rest| rest.find(needle)) {
        let start = cursor.saturating_add(found);
        cursor = start.saturating_add(needle.len());
        spans.push((start, cursor));
        if spans.len() >= MAX_ITERATIONS {
            break;
        }
    }
    spans
}

/// Where a pattern matches in a text, left to right and without overlap.
fn pattern_spans(pattern: &Regex, text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut cursor = 0;
    while cursor <= text.len() && spans.len() < MAX_ITERATIONS {
        let Ok(Some(found)) = pattern.find_from_pos(text, cursor) else {
            break;
        };
        spans.push((found.start(), found.end()));
        // An empty match would otherwise be found again at the same place forever.
        cursor = if found.end() > found.start() {
            found.end()
        } else {
            next_boundary(text, found.end())
        };
    }
    spans
}

/// The next character boundary past an offset.
fn next_boundary(text: &str, from: usize) -> usize {
    let mut next = from.saturating_add(1);
    while next < text.len() && !text.is_char_boundary(next) {
        next = next.saturating_add(1);
    }
    next
}
