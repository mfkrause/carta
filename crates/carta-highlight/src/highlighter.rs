//! The tokenizer: walks a stack of contexts over source text, emitting classified [`Token`]s.
//!
//! For each line the current context's rules are tried in order; the first to match consumes text,
//! emits a token, and may switch contexts. When nothing matches, the context either falls through to
//! another context or consumes a run of ordinary text. Regular-expression rules match anchored at the
//! current position; compiled patterns and resolved keyword sets are cached across lines.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use fancy_regex::{Regex, RegexBuilder};

use crate::grammar::{ContextSwitch, ContextTarget, Grammar, Matcher, Rule};
use crate::registry::Registry;
use crate::token::{SourceLine, Token, TokenKind};

/// The maximum number of steps taken tokenizing a single line before the remainder is emitted as
/// ordinary text. Guards against a definition that could otherwise switch contexts without ever
/// consuming input.
const MAX_LINE_STEPS: usize = 100_000;

/// The maximum depth of nested rule inclusion, guarding against cyclic `IncludeRules`.
const MAX_INCLUDE_DEPTH: usize = 100;

/// The backtracking budget granted to each regular-expression match. On exhaustion the match fails,
/// keeping tokenization bounded on adversarial input.
const REGEX_BACKTRACK_LIMIT: usize = 1_000_000;

/// Tokenizes source code using a catalog of syntax definitions.
#[derive(Debug, Default)]
pub struct Highlighter {
    registry: Registry,
    regexes: RefCell<BTreeMap<RegexKey, Option<Rc<Regex>>>>,
    keyword_sets: RefCell<BTreeMap<String, BTreeMap<String, Rc<KeywordSet>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RegexKey {
    pattern: String,
    insensitive: bool,
    minimal: bool,
}

#[derive(Debug)]
pub(crate) struct KeywordSet {
    words: BTreeSet<String>,
    case_sensitive: bool,
}

impl KeywordSet {
    fn contains(&self, word: &str) -> bool {
        if self.case_sensitive {
            return self.words.contains(word);
        }
        // An ASCII word with no uppercase letters is already its own lowercase form.
        if word
            .bytes()
            .all(|b| b.is_ascii() && !b.is_ascii_uppercase())
        {
            return self.words.contains(word);
        }
        self.words.contains(&word.to_lowercase())
    }
}

/// One entry on the context stack: the definition and context it names, and any captures carried in
/// from the rule that entered it.
#[derive(Debug, Clone)]
struct Frame {
    grammar: Rc<Grammar>,
    context: usize,
    captures: Vec<String>,
}

impl Highlighter {
    /// A highlighter over the bundled syntax definitions.
    #[must_use]
    pub fn new() -> Self {
        Highlighter::default()
    }

    /// The catalog backing this highlighter.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The catalog backing this highlighter, mutably (to register user definitions).
    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.registry
    }

    /// Tokenize `code` as the given language, returning one [`SourceLine`] per line, or `None` if the
    /// language is unknown.
    pub fn highlight(&self, language: &str, code: &str) -> Option<Vec<SourceLine>> {
        let grammar = self.registry.resolve(language)?;
        Some(self.tokenize(grammar, code))
    }

    fn tokenize(&self, start: Rc<Grammar>, code: &str) -> Vec<SourceLine> {
        let mut state = Tokenizer::new(self, start);
        split_lines(code)
            .into_iter()
            .map(|line| state.tokenize_line(line))
            .collect()
    }

    fn compiled_regex(&self, key: &RegexKey) -> Option<Rc<Regex>> {
        if let Some(entry) = self.regexes.borrow().get(key) {
            return entry.clone();
        }
        let compiled = build_regex(key);
        self.regexes
            .borrow_mut()
            .insert(key.clone(), compiled.clone());
        compiled
    }

    fn keyword_set(&self, grammar: &Rc<Grammar>, list: &str) -> Rc<KeywordSet> {
        if let Some(set) = self
            .keyword_sets
            .borrow()
            .get(grammar.name.as_str())
            .and_then(|lists| lists.get(list))
        {
            return Rc::clone(set);
        }
        let mut words = BTreeSet::new();
        let case_sensitive = grammar.keywords.case_sensitive;
        self.collect_words(
            grammar,
            list,
            case_sensitive,
            &mut words,
            &mut BTreeSet::new(),
        );
        let set = Rc::new(KeywordSet {
            words,
            case_sensitive,
        });
        self.keyword_sets
            .borrow_mut()
            .entry(grammar.name.clone())
            .or_default()
            .insert(list.to_string(), Rc::clone(&set));
        set
    }

    fn collect_words(
        &self,
        grammar: &Rc<Grammar>,
        list: &str,
        case_sensitive: bool,
        out: &mut BTreeSet<String>,
        visited: &mut BTreeSet<(String, String)>,
    ) {
        let key = (grammar.name.clone(), list.to_string());
        if !visited.insert(key) {
            return;
        }
        if let Some(words) = grammar.keyword_lists.get(list) {
            for word in words {
                if word.is_empty() {
                    continue;
                }
                out.insert(if case_sensitive {
                    word.clone()
                } else {
                    word.to_lowercase()
                });
            }
        }
        for include in &grammar.keyword_includes {
            if include.target_list != list {
                continue;
            }
            if include.source_language == grammar.name {
                self.collect_words(grammar, &include.source_list, case_sensitive, out, visited);
            } else if let Some(source) = self.registry.resolve_reference(&include.source_language) {
                self.collect_words(&source, &include.source_list, case_sensitive, out, visited);
            }
        }
    }
}

/// The mutable state of an in-progress tokenization.
struct Tokenizer<'a> {
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
    fn new(hl: &'a Highlighter, start: Rc<Grammar>) -> Self {
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

    fn tokenize_line(&mut self, line: &'a str) -> SourceLine {
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
        // A regular expression that matches the empty string (e.g. one ending in an empty
        // alternative) does not count as a match unless the rule only looks ahead; otherwise it would
        // switch contexts without consuming, pre-empting the fall-through the definition intends.
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
        // Locate the match first; the far costlier capture extraction runs only on a hit, and only
        // when the pattern has capture groups a dynamic rule could reference.
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

    // --- state helpers -------------------------------------------------------

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
    /// referencing rule — which is not necessarily the grammar on top of the stack once foreign rules
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

// --- free helpers ------------------------------------------------------------

fn context_index(grammar: &Grammar, name: &str) -> Option<usize> {
    grammar.contexts.iter().position(|c| c.name == name)
}

fn kind_for(grammar: &Grammar, attr_name: &str) -> TokenKind {
    grammar
        .item_styles
        .get(attr_name)
        .copied()
        .unwrap_or(TokenKind::Normal)
}

fn plain_line(line: &str) -> SourceLine {
    if line.is_empty() {
        Vec::new()
    } else {
        vec![Token::new(TokenKind::Normal, line.to_string())]
    }
}

/// Drop empty tokens and merge runs of the same kind, matching how a line is finally rendered.
fn normalize(tokens: Vec<Token>) -> SourceLine {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if token.text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.kind == token.kind => last.text.push_str(&token.text),
            _ => out.push(token),
        }
    }
    out
}

fn split_lines(code: &str) -> Vec<&str> {
    if code.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = code.split('\n').collect();
    if code.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_word_boundary(c: char, d: char) -> bool {
    is_word_char(c) != is_word_char(d)
}

fn substitute<F>(template: &str, lookup: F) -> String
where
    F: Fn(usize) -> Option<String>,
{
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%'
            && let Some(d) = chars.peek().copied().filter(char::is_ascii_digit)
        {
            chars.next();
            let idx = (d as usize) - ('0' as usize);
            out.push_str(&lookup(idx).unwrap_or_default());
            continue;
        }
        out.push(c);
    }
    out
}

fn escape_regex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn build_regex(key: &RegexKey) -> Option<Rc<Regex>> {
    let mut pattern = String::new();
    if key.insensitive {
        pattern.push_str("(?i)");
    }
    if key.minimal {
        pattern.push_str("(?U)");
    }
    pattern.push_str("\\A(?:");
    pattern.push_str(&key.pattern);
    pattern.push(')');
    RegexBuilder::new(&pattern)
        .backtrack_limit(REGEX_BACKTRACK_LIMIT)
        .build()
        .ok()
        .map(Rc::new)
}

// --- number and C-literal matchers ------------------------------------------

fn digits_len(s: &str, valid: impl Fn(char) -> bool) -> usize {
    s.chars()
        .take_while(|c| valid(*c))
        .map(char::len_utf8)
        .sum()
}

fn match_decimal(s: &str) -> Option<usize> {
    let mut len = 0;
    if s.starts_with('-') {
        len += 1;
    }
    let digits = digits_len(&s[len..], |c| c.is_ascii_digit());
    if digits == 0 {
        return None;
    }
    Some(len + digits)
}

fn match_hex(s: &str) -> Option<usize> {
    let mut len = 0;
    if s.starts_with('-') {
        len += 1;
    }
    let rest = &s[len..];
    let mut chars = rest.char_indices();
    if chars.next()?.1 != '0' {
        return None;
    }
    match chars.next()?.1 {
        'x' | 'X' => {}
        _ => return None,
    }
    let digits = digits_len(&rest[2..], |c| c.is_ascii_hexdigit());
    if digits == 0 {
        return None;
    }
    Some(len + 2 + digits)
}

fn match_octal(s: &str) -> Option<usize> {
    let mut len = 0;
    if s.starts_with('-') {
        len += 1;
    }
    let rest = s.get(len..).unwrap_or("");
    if !rest.starts_with('0') {
        return None;
    }
    let digits = digits_len(rest.get(1..).unwrap_or(""), |c| ('0'..='7').contains(&c));
    if digits == 0 {
        return None;
    }
    Some(len + 1 + digits)
}

// The float shapes are clearer enumerated as independent cases than as a single minimized predicate.
#[allow(clippy::nonminimal_bool)]
fn match_float(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    if bytes.first() == Some(&b'+') || bytes.first() == Some(&b'-') {
        i += 1;
    }
    let before = {
        let n = digits_len(&s[i..], |c| c.is_ascii_digit());
        i += n;
        n > 0
    };
    let dot = if bytes.get(i) == Some(&b'.') {
        i += 1;
        true
    } else {
        false
    };
    let after = {
        let n = digits_len(&s[i..], |c| c.is_ascii_digit());
        i += n;
        n > 0
    };
    let exponent = {
        if matches!(bytes.get(i), Some(&b'e' | &b'E')) {
            let mut j = i + 1;
            if matches!(bytes.get(j), Some(&b'+' | &b'-')) {
                j += 1;
            }
            let n = digits_len(&s[j..], |c| c.is_ascii_digit());
            if n > 0 {
                i = j + n;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if matches!(s[i..].chars().next(), Some('.')) {
        return None;
    }
    let valid = (before && !dot && exponent)
        || (before && dot && (after || !exponent))
        || (!before && dot && after);
    valid.then_some(i)
}

fn match_c_string_char(s: &str) -> Option<usize> {
    let mut chars = s.char_indices();
    if chars.next()?.1 != '\\' {
        return None;
    }
    let (_, next) = chars.next()?;
    match next {
        'x' | 'X' => {
            let digits = digits_len(&s[2..], |c| c.is_ascii_hexdigit());
            if digits == 0 {
                return None;
            }
            Some(2 + digits)
        }
        '0' => {
            let digits = digits_len(&s[2..], |c| ('0'..='7').contains(&c));
            Some(2 + digits)
        }
        c if "abefnrtv\"'?\\".contains(c) => Some(1 + c.len_utf8()),
        _ => None,
    }
}

fn match_c_char(s: &str) -> Option<usize> {
    let mut i = 0;
    if s.get(i..).and_then(|r| r.chars().next()) != Some('\'') {
        return None;
    }
    i += 1;
    let rest = s.get(i..)?;
    let inner = if let Some(len) = match_c_string_char(rest) {
        len
    } else {
        let c = rest.chars().next()?;
        if c == '\'' || c == '\\' {
            return None;
        }
        c.len_utf8()
    };
    i += inner;
    if s.get(i..).and_then(|r| r.chars().next()) != Some('\'') {
        return None;
    }
    Some(i + 1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    const RUST_SNIPPET: &str = r#"use std::collections::HashMap;

/// A small example struct.
pub struct Point {
    x: f64,
    y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }

    fn magnitude(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

fn main() {
    let mut counts: HashMap<String, u32> = HashMap::new();
    let p = Point::new(3.0, 4.0);
    let label = "distance";
    let n = 0xFF;
    counts.insert(label.to_string(), n);
    // Print the magnitude.
    println!("{} = {}", label, p.magnitude());
    for i in 0..10 {
        counts.insert(format!("k{}", i), i as u32);
    }
}
"#;

    fn kinds(line: &SourceLine) -> Vec<(TokenKind, &str)> {
        line.iter().map(|t| (t.kind, t.text.as_str())).collect()
    }

    #[test]
    fn highlights_c_keyword_and_number() {
        let hl = Highlighter::new();
        let lines = hl.highlight("c", "int x = 42;").expect("c is known");
        assert_eq!(lines.len(), 1);
        let toks = kinds(&lines[0]);
        assert!(
            toks.iter()
                .any(|(k, t)| *k == TokenKind::DataType && *t == "int")
        );
        assert!(
            toks.iter()
                .any(|(k, t)| *k == TokenKind::DecVal && *t == "42")
        );
    }

    #[test]
    fn unknown_language_returns_none() {
        let hl = Highlighter::new();
        assert!(hl.highlight("no-such-lang", "x").is_none());
    }

    #[test]
    fn normalizes_adjacent_same_kind() {
        let merged = normalize(vec![
            Token::new(TokenKind::Normal, "a"),
            Token::new(TokenKind::Normal, "b"),
            Token::new(TokenKind::Keyword, ""),
            Token::new(TokenKind::Keyword, "if"),
        ]);
        assert_eq!(
            merged,
            vec![
                Token::new(TokenKind::Normal, "ab"),
                Token::new(TokenKind::Keyword, "if"),
            ]
        );
    }

    #[test]
    fn splits_lines_like_the_spec() {
        assert_eq!(split_lines(""), Vec::<&str>::new());
        assert_eq!(split_lines("a\n"), vec!["a"]);
        assert_eq!(split_lines("a\nb"), vec!["a", "b"]);
        assert_eq!(split_lines("a\n\n"), vec!["a", ""]);
    }

    #[test]
    fn float_matcher_matches_expected_forms() {
        assert_eq!(match_float("5e2"), Some(3));
        assert_eq!(match_float("5.2"), Some(3));
        assert_eq!(match_float(".23"), Some(3));
        assert_eq!(match_float("5"), None);
        assert_eq!(match_float("5.2.3"), None);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn rust_snippet_token_stream_is_stable() {
        use TokenKind::{
            Comment, ControlFlow, DataType, DecVal, Keyword, Normal, Operator, Preprocessor, String,
        };
        let hl = Highlighter::new();
        let lines = hl.highlight("rust", RUST_SNIPPET).expect("rust is known");
        let actual: Vec<Vec<(TokenKind, &str)>> = lines.iter().map(kinds).collect();
        let expected: Vec<Vec<(TokenKind, &str)>> = vec![
            vec![
                (Keyword, "use"),
                (Normal, " "),
                (Preprocessor, "std::collections::"),
                (Normal, "HashMap"),
                (Operator, ";"),
            ],
            vec![],
            vec![(Comment, "/// A small example struct.")],
            vec![
                (Keyword, "pub"),
                (Normal, " "),
                (Keyword, "struct"),
                (Normal, " Point "),
                (Operator, "{"),
            ],
            vec![
                (Normal, "    x"),
                (Operator, ":"),
                (Normal, " "),
                (DataType, "f64"),
                (Operator, ","),
            ],
            vec![
                (Normal, "    y"),
                (Operator, ":"),
                (Normal, " "),
                (DataType, "f64"),
                (Operator, ","),
            ],
            vec![(Operator, "}")],
            vec![],
            vec![(Keyword, "impl"), (Normal, " Point "), (Operator, "{")],
            vec![
                (Normal, "    "),
                (Keyword, "pub"),
                (Normal, " "),
                (Keyword, "fn"),
                (Normal, " new(x"),
                (Operator, ":"),
                (Normal, " "),
                (DataType, "f64"),
                (Operator, ","),
                (Normal, " y"),
                (Operator, ":"),
                (Normal, " "),
                (DataType, "f64"),
                (Normal, ") "),
                (Operator, "->"),
                (Normal, " "),
                (DataType, "Self"),
                (Normal, " "),
                (Operator, "{"),
            ],
            vec![
                (Normal, "        Point "),
                (Operator, "{"),
                (Normal, " x"),
                (Operator, ","),
                (Normal, " y "),
                (Operator, "}"),
            ],
            vec![(Normal, "    "), (Operator, "}")],
            vec![],
            vec![
                (Normal, "    "),
                (Keyword, "fn"),
                (Normal, " magnitude("),
                (Operator, "&"),
                (Keyword, "self"),
                (Normal, ") "),
                (Operator, "->"),
                (Normal, " "),
                (DataType, "f64"),
                (Normal, " "),
                (Operator, "{"),
            ],
            vec![
                (Normal, "        ("),
                (Keyword, "self"),
                (Operator, "."),
                (Normal, "x "),
                (Operator, "*"),
                (Normal, " "),
                (Keyword, "self"),
                (Operator, "."),
                (Normal, "x "),
                (Operator, "+"),
                (Normal, " "),
                (Keyword, "self"),
                (Operator, "."),
                (Normal, "y "),
                (Operator, "*"),
                (Normal, " "),
                (Keyword, "self"),
                (Operator, "."),
                (Normal, "y)"),
                (Operator, "."),
                (Normal, "sqrt()"),
            ],
            vec![(Normal, "    "), (Operator, "}")],
            vec![(Operator, "}")],
            vec![],
            vec![(Keyword, "fn"), (Normal, " main() "), (Operator, "{")],
            vec![
                (Normal, "    "),
                (Keyword, "let"),
                (Normal, " "),
                (Keyword, "mut"),
                (Normal, " counts"),
                (Operator, ":"),
                (Normal, " HashMap"),
                (Operator, "<"),
                (DataType, "String"),
                (Operator, ","),
                (Normal, " "),
                (DataType, "u32"),
                (Operator, ">"),
                (Normal, " "),
                (Operator, "="),
                (Normal, " "),
                (Preprocessor, "HashMap::"),
                (Normal, "new()"),
                (Operator, ";"),
            ],
            vec![
                (Normal, "    "),
                (Keyword, "let"),
                (Normal, " p "),
                (Operator, "="),
                (Normal, " "),
                (Preprocessor, "Point::"),
                (Normal, "new("),
                (DecVal, "3.0"),
                (Operator, ","),
                (Normal, " "),
                (DecVal, "4.0"),
                (Normal, ")"),
                (Operator, ";"),
            ],
            vec![
                (Normal, "    "),
                (Keyword, "let"),
                (Normal, " label "),
                (Operator, "="),
                (Normal, " "),
                (String, "\"distance\""),
                (Operator, ";"),
            ],
            vec![
                (Normal, "    "),
                (Keyword, "let"),
                (Normal, " n "),
                (Operator, "="),
                (Normal, " "),
                (DecVal, "0xFF"),
                (Operator, ";"),
            ],
            vec![
                (Normal, "    counts"),
                (Operator, "."),
                (Normal, "insert(label"),
                (Operator, "."),
                (Normal, "to_string()"),
                (Operator, ","),
                (Normal, " n)"),
                (Operator, ";"),
            ],
            vec![(Normal, "    "), (Comment, "// Print the magnitude.")],
            vec![
                (Normal, "    "),
                (Preprocessor, "println!"),
                (Normal, "("),
                (String, "\"{} = {}\""),
                (Operator, ","),
                (Normal, " label"),
                (Operator, ","),
                (Normal, " p"),
                (Operator, "."),
                (Normal, "magnitude())"),
                (Operator, ";"),
            ],
            vec![
                (Normal, "    "),
                (ControlFlow, "for"),
                (Normal, " i "),
                (Keyword, "in"),
                (Normal, " "),
                (DecVal, "0"),
                (Operator, ".."),
                (DecVal, "10"),
                (Normal, " "),
                (Operator, "{"),
            ],
            vec![
                (Normal, "        counts"),
                (Operator, "."),
                (Normal, "insert("),
                (Preprocessor, "format!"),
                (Normal, "("),
                (String, "\"k{}\""),
                (Operator, ","),
                (Normal, " i)"),
                (Operator, ","),
                (Normal, " i "),
                (Keyword, "as"),
                (Normal, " "),
                (DataType, "u32"),
                (Normal, ")"),
                (Operator, ";"),
            ],
            vec![(Normal, "    "), (Operator, "}")],
            vec![(Operator, "}")],
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn hex_and_decimal() {
        assert_eq!(match_hex("0xFF"), Some(4));
        assert_eq!(match_hex("0x"), None);
        assert_eq!(match_decimal("42abc"), Some(2));
        assert_eq!(match_decimal("abc"), None);
    }
}
