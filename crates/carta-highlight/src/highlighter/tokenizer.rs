//! The line tokenizer: walks the context stack, applies rules, and emits classified tokens.

use std::rc::Rc;

use crate::grammar::{ContextSwitch, ContextTarget, Grammar, Matcher, Rule};
use crate::token::{SourceLine, Token, TokenKind};

use super::helpers::{
    build_regex, context_index, escape_regex, is_word_boundary, kind_for, match_c_char,
    match_c_string_char, match_decimal, match_float, match_hex, match_octal, normalize, plain_line,
    substitute,
};
use super::{Frame, Highlighter, RegexKey};

/// The maximum number of steps taken tokenizing a single line before the remainder is emitted as
/// ordinary text. Guards against a definition that could otherwise switch contexts without ever
/// consuming input.
const MAX_LINE_STEPS: usize = 100_000;

/// The maximum depth of nested rule inclusion, guarding against cyclic `IncludeRules`.
const MAX_INCLUDE_DEPTH: usize = 100;

/// The mutable state of an in-progress tokenization.
pub(super) struct Tokenizer<'a> {
    hl: &'a Highlighter,
    stack: Vec<Frame>,
    line: &'a str,
    pos: usize,
    col: usize,
    prev_char: char,
    first_nonspace: Option<usize>,
    line_continuation: bool,
    pending_captures: Vec<String>,
}

/// The result of running a matcher: the span it consumed.
#[derive(Clone, Copy)]
struct MatchSpan {
    bytes: usize,
    chars: usize,
}

impl<'a> Tokenizer<'a> {
    pub(super) fn new(hl: &'a Highlighter, start: Rc<Grammar>) -> Self {
        Tokenizer {
            hl,
            stack: vec![Frame {
                grammar: start,
                context: 0,
                captures: Vec::new(),
            }],
            line: "",
            pos: 0,
            col: 0,
            prev_char: '\n',
            first_nonspace: None,
            line_continuation: false,
            pending_captures: Vec::new(),
        }
    }

    pub(super) fn tokenize_line(&mut self, line: &'a str) -> SourceLine {
        self.line = line;
        self.pos = 0;
        self.prev_char = '\n';

        let (owner, begin_switch, empty_switch) = match self.stack.last() {
            Some(frame) => match frame.grammar.contexts.get(frame.context) {
                Some(ctx) => (
                    Rc::clone(&frame.grammar),
                    ctx.line_begin_context.clone(),
                    ctx.line_empty_context.clone(),
                ),
                None => return plain_line(line),
            },
            None => return plain_line(line),
        };

        if self.line_continuation {
            self.line_continuation = false;
        } else {
            self.col = 0;
            self.first_nonspace = line.chars().position(|c| !c.is_whitespace());
            self.apply_switch(&begin_switch, &owner);
        }
        if line.is_empty() {
            if let Some(sw) = &empty_switch {
                self.apply_switch(sw, &owner);
            }
        } else {
            self.apply_switch(&begin_switch, &owner);
        }

        let mut tokens = Vec::new();
        let mut steps = 0usize;
        while self.pos < self.line.len() {
            steps += 1;
            if steps > MAX_LINE_STEPS {
                let rest = self.remaining().to_string();
                let kind = self.current_attr_kind();
                tokens.push(Token::new(kind, rest));
                break;
            }
            match self.step() {
                Step::Emitted(Some(token)) => tokens.push(token),
                Step::Emitted(None) => {}
                Step::Stop => break,
            }
        }

        self.finish_line();
        normalize(tokens)
    }

    fn finish_line(&mut self) {
        if self.line_continuation {
            return;
        }
        // Apply the end-of-line switch, chaining while it keeps changing the context.
        for _ in 0..self.stack.len().saturating_add(1) {
            let (owner, switch) = match self.stack.last() {
                Some(frame) => match frame.grammar.contexts.get(frame.context) {
                    Some(ctx) => (Rc::clone(&frame.grammar), ctx.line_end_context.clone()),
                    None => break,
                },
                None => break,
            };
            if switch.pops == 0 && switch.push.is_none() {
                break;
            }
            let before = self.top_identity();
            self.apply_switch(&switch, &owner);
            if self.top_identity() == before {
                break;
            }
        }
    }

    fn step(&mut self) -> Step {
        if self.pos >= self.line.len() {
            return Step::Stop;
        }
        let (grammar, context) = match self.stack.last() {
            Some(frame) => (Rc::clone(&frame.grammar), frame.context),
            None => return Step::Stop,
        };
        let Some(ctx) = grammar.contexts.get(context) else {
            return Step::Stop;
        };
        let ctx_attr: &str = &ctx.attribute;
        for rule in &ctx.rules {
            if let Some(result) = self.try_rule(rule, &grammar, ctx_attr, 0) {
                return Step::Emitted(result);
            }
        }

        // Nothing matched: fall through or consume ordinary text.
        let fallthrough = ctx.fallthrough;
        let fallthrough_ctx = &ctx.fallthrough_context;
        let active = fallthrough && !(fallthrough_ctx.pops == 0 && fallthrough_ctx.push.is_none());
        if active {
            self.apply_switch(fallthrough_ctx, &grammar);
            Step::Emitted(None)
        } else if fallthrough {
            self.apply_switch(
                &ContextSwitch {
                    pops: 1,
                    push: None,
                },
                &grammar,
            );
            Step::Emitted(None)
        } else {
            let span = self.normal_chunk();
            let text = self.consume(span);
            let kind = kind_for(&grammar, ctx_attr);
            Step::Emitted(Some(Token::new(kind, text)))
        }
    }

    /// Try one rule at the current position. Returns `None` if the rule does not match; otherwise the
    /// rule has been applied (text consumed, context switched) and the emitted token, if any, is
    /// returned. The outer `Option` distinguishes "no match" from a match that emits no token.
    #[allow(clippy::option_option)]
    fn try_rule(
        &mut self,
        rule: &Rule,
        grammar: &Rc<Grammar>,
        ctx_attr: &str,
        depth: usize,
    ) -> Option<Option<Token>> {
        if let Some(col) = rule.column
            && self.col != col
        {
            return None;
        }
        if rule.first_non_space && self.first_nonspace != Some(self.col) {
            return None;
        }

        if let Matcher::IncludeRules {
            target,
            include_attribute,
        } = &rule.matcher
        {
            return self.include_rules(target, *include_attribute, ctx_attr, grammar, depth);
        }

        let saved = (self.pos, self.col, self.prev_char);
        self.pending_captures.clear();

        let matched = self.run_matcher(rule, grammar)?;
        // A zero-width match (unless look-ahead only) would switch contexts without consuming,
        // pre-empting the intended fall-through.
        if matched.bytes == 0 && !rule.look_ahead && matches!(rule.matcher, Matcher::RegExpr { .. })
        {
            return None;
        }
        let mut text = self.consume(matched);

        // Children extend the match, keeping the parent's classification.
        if !rule.children.is_empty() {
            for child in &rule.children {
                if let Some(child_tok) = self.try_rule(child, grammar, ctx_attr, depth) {
                    if let Some(tok) = child_tok {
                        text.push_str(&tok.text);
                    }
                    break;
                }
            }
        }

        let attr_name = rule.attribute.as_deref().unwrap_or(ctx_attr);
        let kind = kind_for(grammar, attr_name);

        let token = if rule.look_ahead {
            self.pos = saved.0;
            self.col = saved.1;
            self.prev_char = saved.2;
            None
        } else if text.is_empty() {
            None
        } else {
            Some(Token::new(kind, text))
        };

        self.apply_switch(&rule.context, grammar);
        self.attach_captures();
        Some(token)
    }

    #[allow(clippy::option_option)]
    fn include_rules(
        &mut self,
        target: &ContextTarget,
        include_attribute: bool,
        ctx_attr: &str,
        owner: &Rc<Grammar>,
        depth: usize,
    ) -> Option<Option<Token>> {
        if depth >= MAX_INCLUDE_DEPTH {
            return None;
        }
        let (grammar, context) = self.resolve_target(target, owner)?;
        let ctx = grammar.contexts.get(context)?;
        let inner_attr: &str = &ctx.attribute;
        for rule in &ctx.rules {
            if let Some(result) = self.try_rule(rule, &grammar, inner_attr, depth + 1) {
                if include_attribute
                    && let Some(tok) = &result
                    && tok.kind == TokenKind::Normal
                {
                    let kind = kind_for(owner, ctx_attr);
                    return Some(Some(Token::new(kind, tok.text.clone())));
                }
                return Some(result);
            }
        }
        None
    }

    #[allow(clippy::too_many_lines)]
    fn run_matcher(&mut self, rule: &Rule, grammar: &Rc<Grammar>) -> Option<MatchSpan> {
        let dynamic = rule.dynamic;
        let remaining = self.remaining();
        match &rule.matcher {
            Matcher::DetectChar(c) => {
                let target = if dynamic { self.dynamic_char(*c)? } else { *c };
                let first = remaining.chars().next()?;
                (first == target).then(|| MatchSpan {
                    bytes: first.len_utf8(),
                    chars: 1,
                })
            }
            Matcher::Detect2Chars(a, b) => {
                let a = if dynamic { self.dynamic_char(*a)? } else { *a };
                let b = if dynamic { self.dynamic_char(*b)? } else { *b };
                let mut chars = remaining.chars();
                (chars.next() == Some(a) && chars.next() == Some(b)).then(|| MatchSpan {
                    bytes: a.len_utf8() + b.len_utf8(),
                    chars: 2,
                })
            }
            Matcher::AnyChar(set) => {
                let first = remaining.chars().next()?;
                set.contains(first).then(|| MatchSpan {
                    bytes: first.len_utf8(),
                    chars: 1,
                })
            }
            Matcher::StringDetect { text, insensitive } => {
                let target = if dynamic {
                    self.substitute_text(text)
                } else {
                    text.clone()
                };
                self.match_literal(&target, *insensitive)
            }
            Matcher::WordDetect { text, insensitive } => {
                self.match_word_detect(text, *insensitive, grammar)
            }
            Matcher::RegExpr {
                pattern,
                insensitive,
                minimal,
            } => self.match_regex(rule, pattern, *insensitive, *minimal),
            Matcher::Keyword(list) => self.match_keyword(rule, list, grammar),
            Matcher::Int => self.match_number(NumberKind::Int),
            Matcher::Float => self.match_number(NumberKind::Float),
            Matcher::HlCOct => self.match_number(NumberKind::Oct),
            Matcher::HlCHex => self.match_number(NumberKind::Hex),
            Matcher::HlCStringChar => self.match_c_string_char(),
            Matcher::HlCChar => self.match_c_char(),
            Matcher::RangeDetect { start, end } => {
                let mut chars = remaining.chars();
                if chars.next()? != *start {
                    return None;
                }
                let rest = &remaining[start.len_utf8()..];
                let end_idx = rest.find(*end)?;
                let inner = &rest[..end_idx];
                let bytes = start.len_utf8() + inner.len() + end.len_utf8();
                let chars = 1 + inner.chars().count() + 1;
                Some(MatchSpan { bytes, chars })
            }
            Matcher::DetectSpaces => {
                let count: usize = remaining
                    .chars()
                    .take_while(char::is_ascii_whitespace)
                    .map(char::len_utf8)
                    .sum();
                (count > 0).then(|| MatchSpan {
                    bytes: count,
                    chars: remaining[..count].chars().count(),
                })
            }
            Matcher::DetectIdentifier => {
                let mut chars = remaining.char_indices();
                let (_, first) = chars.next()?;
                if !((first.is_ascii_alphabetic()) || first == '_') {
                    return None;
                }
                let mut end = first.len_utf8();
                for (i, c) in chars {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        end = i + c.len_utf8();
                    } else {
                        break;
                    }
                }
                Some(MatchSpan {
                    bytes: end,
                    chars: remaining[..end].chars().count(),
                })
            }
            Matcher::LineContinue(c) => {
                let mut chars = remaining.chars();
                (chars.next() == Some(*c) && chars.next().is_none()).then(|| {
                    self.line_continuation = true;
                    MatchSpan {
                        bytes: c.len_utf8(),
                        chars: 1,
                    }
                })
            }
            Matcher::IncludeRules { .. } | Matcher::Unsupported => None,
        }
    }

    fn match_literal(&self, target: &str, insensitive: bool) -> Option<MatchSpan> {
        if target.is_empty() {
            return None;
        }
        let remaining = self.remaining();
        if !insensitive {
            return remaining.starts_with(target).then(|| MatchSpan {
                bytes: target.len(),
                chars: target.chars().count(),
            });
        }
        let take: String = remaining.chars().take(target.chars().count()).collect();
        (take.to_lowercase() == target.to_lowercase()).then(|| MatchSpan {
            bytes: take.len(),
            chars: take.chars().count(),
        })
    }

    fn match_word_detect(
        &self,
        text: &str,
        insensitive: bool,
        _grammar: &Rc<Grammar>,
    ) -> Option<MatchSpan> {
        let span = self.match_literal(text, insensitive)?;
        let remaining = self.remaining();
        let matched = &remaining[..span.bytes];
        let first = matched.chars().next()?;
        if !is_word_boundary(self.prev_char, first) {
            return None;
        }
        let last = matched.chars().last()?;
        let next = remaining[span.bytes..].chars().next().unwrap_or('\n');
        is_word_boundary(last, next).then_some(span)
    }

    fn match_keyword(&self, rule: &Rule, list: &str, grammar: &Rc<Grammar>) -> Option<MatchSpan> {
        if !grammar.keywords.is_delimiter(self.prev_char) {
            return None;
        }
        let remaining = self.remaining();
        let end = remaining
            .char_indices()
            .find(|(_, c)| grammar.keywords.is_delimiter(*c))
            .map_or(remaining.len(), |(i, _)| i);
        if end == 0 {
            return None;
        }
        let word = &remaining[..end];
        let set = rule
            .keyword_set
            .get_or_init(|| self.hl.keyword_set(grammar, list));
        set.contains(word).then(|| MatchSpan {
            bytes: end,
            chars: word.chars().count(),
        })
    }

    fn match_regex(
        &mut self,
        rule: &Rule,
        pattern: &str,
        insensitive: bool,
        minimal: bool,
    ) -> Option<MatchSpan> {
        let remaining = self.remaining();
        if pattern.starts_with("\\b") {
            let d = remaining.chars().next().unwrap_or('\n');
            if !is_word_boundary(self.prev_char, d) {
                return None;
            }
        }
        let regex = if rule.dynamic {
            let key = RegexKey {
                pattern: self.substitute_regex(pattern),
                insensitive,
                minimal,
            };
            self.hl.compiled_regex(&key)?
        } else {
            rule.compiled_regex
                .get_or_init(|| {
                    build_regex(&RegexKey {
                        pattern: pattern.to_string(),
                        insensitive,
                        minimal,
                    })
                })
                .clone()?
        };
        // Find first; the far costlier capture extraction runs only on a hit with capture groups.
        let end = match regex.find(remaining) {
            Ok(Some(m)) if m.start() == 0 => m.end(),
            _ => return None,
        };
        if regex.captures_len() > 1 {
            let Ok(Some(caps)) = regex.captures(remaining) else {
                return None;
            };
            let whole = caps.get(0)?;
            if whole.start() != 0 {
                return None;
            }
            let mut captures = Vec::with_capacity(caps.len());
            for i in 0..caps.len() {
                captures.push(
                    caps.get(i)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default(),
                );
            }
            self.pending_captures = captures;
            let bytes = whole.end();
            return Some(MatchSpan {
                bytes,
                chars: remaining[..bytes].chars().count(),
            });
        }
        Some(MatchSpan {
            bytes: end,
            chars: remaining[..end].chars().count(),
        })
    }

    fn match_number(&self, kind: NumberKind) -> Option<MatchSpan> {
        // These require a word boundary before the number.
        let d = self.remaining().chars().next().unwrap_or('\n');
        if !is_word_boundary(self.prev_char, d) {
            return None;
        }
        let remaining = self.remaining();
        let len = match kind {
            NumberKind::Int => match_hex(remaining)
                .or_else(|| match_octal(remaining))
                .or_else(|| match_decimal(remaining)),
            NumberKind::Hex => match_hex(remaining),
            NumberKind::Oct => match_octal(remaining),
            NumberKind::Float => match_float(remaining),
        }?;
        (len > 0).then(|| MatchSpan {
            bytes: len,
            chars: remaining[..len].chars().count(),
        })
    }

    fn match_c_string_char(&self) -> Option<MatchSpan> {
        let len = match_c_string_char(self.remaining())?;
        Some(MatchSpan {
            bytes: len,
            chars: self.remaining()[..len].chars().count(),
        })
    }

    fn match_c_char(&self) -> Option<MatchSpan> {
        let len = match_c_char(self.remaining())?;
        Some(MatchSpan {
            bytes: len,
            chars: self.remaining()[..len].chars().count(),
        })
    }

    fn remaining(&self) -> &'a str {
        self.line.get(self.pos..).unwrap_or("")
    }

    fn consume(&mut self, span: MatchSpan) -> String {
        let end = self.pos + span.bytes;
        let text = self.line.get(self.pos..end).unwrap_or("").to_string();
        if let Some(last) = text.chars().last() {
            self.prev_char = last;
        }
        self.pos = end;
        self.col += span.chars;
        text
    }

    fn normal_chunk(&self) -> MatchSpan {
        let remaining = self.remaining();
        let Some(first) = remaining.chars().next() else {
            return MatchSpan { bytes: 0, chars: 0 };
        };
        if first == ' ' {
            let bytes = remaining.chars().take_while(|c| *c == ' ').count();
            return MatchSpan {
                bytes,
                chars: bytes,
            };
        }
        if first.is_ascii_alphanumeric() {
            let mut bytes = 0;
            let mut chars = 0;
            for c in remaining.chars() {
                if c.is_alphanumeric() {
                    bytes += c.len_utf8();
                    chars += 1;
                } else {
                    break;
                }
            }
            return MatchSpan { bytes, chars };
        }
        MatchSpan {
            bytes: first.len_utf8(),
            chars: 1,
        }
    }

    fn current_attr_kind(&self) -> TokenKind {
        match self.stack.last() {
            Some(frame) => match frame.grammar.contexts.get(frame.context) {
                Some(ctx) => kind_for(&frame.grammar, &ctx.attribute),
                None => TokenKind::Normal,
            },
            None => TokenKind::Normal,
        }
    }

    fn top_identity(&self) -> Option<(usize, usize)> {
        self.stack
            .last()
            .map(|f| (Rc::as_ptr(&f.grammar) as usize, f.context))
    }

    fn apply_switch(&mut self, switch: &ContextSwitch, owner: &Rc<Grammar>) {
        for _ in 0..switch.pops {
            if self.stack.len() > 1 {
                self.stack.pop();
            }
        }
        if let Some(target) = &switch.push
            && let Some((grammar, context)) = self.resolve_target(target, owner)
        {
            self.stack.push(Frame {
                grammar,
                context,
                captures: Vec::new(),
            });
        }
    }

    /// Resolve a context reference. Local names resolve within `owner`, the grammar that defines the
    /// referencing rule, which is not necessarily the grammar on top of the stack once foreign rules
    /// have been spliced in.
    fn resolve_target(
        &self,
        target: &ContextTarget,
        owner: &Rc<Grammar>,
    ) -> Option<(Rc<Grammar>, usize)> {
        match target {
            ContextTarget::Local(name) => {
                let idx = context_index(owner, name)?;
                Some((Rc::clone(owner), idx))
            }
            ContextTarget::Foreign { language, context } => {
                let grammar = self.hl.registry.resolve_reference(language)?;
                let idx = match context {
                    Some(name) => context_index(&grammar, name)?,
                    None => 0,
                };
                Some((grammar, idx))
            }
        }
    }

    fn attach_captures(&mut self) {
        if self.pending_captures.is_empty() {
            return;
        }
        if let Some(frame) = self.stack.last_mut() {
            frame.captures = std::mem::take(&mut self.pending_captures);
        }
    }

    fn capture(&self, index: usize) -> Option<&str> {
        self.stack.last()?.captures.get(index).map(String::as_str)
    }

    fn dynamic_char(&self, c: char) -> Option<char> {
        if c.is_ascii_digit() {
            let idx = (c as usize) - ('0' as usize);
            self.capture(idx)?.chars().next()
        } else {
            Some(c)
        }
    }

    fn substitute_text(&self, template: &str) -> String {
        substitute(template, |n| self.capture(n).map(str::to_string))
    }

    fn substitute_regex(&self, template: &str) -> String {
        substitute(template, |n| self.capture(n).map(escape_regex))
    }
}

enum Step {
    Emitted(Option<Token>),
    Stop,
}

#[derive(Clone, Copy)]
enum NumberKind {
    Int,
    Hex,
    Oct,
    Float,
}
