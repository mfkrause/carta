//! Inline markup parsing and deferred-reference resolution.

use super::definitions::{RoleChain, Substitution};
use super::directives::{class_attr, normalize_inline_literal};
use super::inline_helpers::{
    autolink, autolink_boundary, find_close_literal, inline_end_ok, inline_start_ok, literal_text,
    parse_role, push_text, quote_glyph, quote_suppresses, quote_type, run_length,
    split_embedded_uri, trailing_reference_name,
};
use super::markers::is_citation_label;
use super::{Parser, REF_SENTINEL, defer_reference, indirect_referent, normalize_name};
use crate::inline_text::trim_inline_ends;
use crate::smart_fold::{
    QuoteCtx, can_close_quote, can_open_quote, fold_dash_run_greedy, fold_ellipsis_run,
};
use carta_ast::{Attr, Block, Format, Inline, MathType, Target};
use carta_core::Extension;

impl Parser<'_> {
    // inline parsing

    pub(super) fn inlines(&mut self, text: &str) -> Vec<Inline> {
        let mut out = self.inlines_no_trim(text);
        trim_inline_ends(&mut out);
        out
    }

    /// Parse inline markup without trimming the leading and trailing whitespace nodes. Interpreted
    /// text keeps the spacing around its content, so role content is parsed through this entry.
    fn inlines_no_trim(&mut self, text: &str) -> Vec<Inline> {
        carta_core::with_stack(|| self.inlines_no_trim_nested(text))
    }

    fn inlines_no_trim_nested(&mut self, text: &str) -> Vec<Inline> {
        let chars: Vec<char> = text.chars().collect();
        let smart = self.ext.contains(Extension::Smart);
        let mut out = Vec::new();
        let mut pending = String::new();
        let mut pos = 0;
        while pos < chars.len() {
            let ch = chars.get(pos).copied().unwrap_or(' ');
            let prev = pos.checked_sub(1).and_then(|p| chars.get(p)).copied();
            if ch == '\\' {
                match chars.get(pos + 1) {
                    Some(next) if next.is_whitespace() => pos += 2,
                    Some(next) => {
                        pending.push(*next);
                        pos += 2;
                    }
                    None => {
                        pending.push('\\');
                        pos += 1;
                    }
                }
                continue;
            }
            if ch == '_'
                && chars.get(pos + 1) == Some(&'`')
                && inline_start_ok(prev)
                && let Some((span, next)) = self.inline_target(&chars, pos)
            {
                push_text(&mut out, &pending);
                pending.clear();
                out.push(span);
                pos = next;
                continue;
            }
            // A trailing underscore closes a simple reference named by the just-accumulated name chars.
            if ch == '_'
                && let Some((link, next)) = self.simple_reference(&chars, pos, &mut pending)
            {
                push_text(&mut out, &pending);
                pending.clear();
                out.push(link);
                pos = next;
                continue;
            }
            if let Some((inline, drop_space, next)) = self.try_markup(&chars, pos) {
                push_text(&mut out, &pending);
                pending.clear();
                if drop_space && matches!(out.last(), Some(Inline::Space)) {
                    out.pop();
                }
                out.extend(inline);
                pos = next;
                continue;
            }
            if autolink_boundary(prev)
                && let Some((link, next)) = autolink(&chars, pos)
            {
                push_text(&mut out, &pending);
                pending.clear();
                out.push(link);
                pos = next;
                continue;
            }
            // smart: paired quotes become quotation nodes, lone quotes curl, hyphen runs dashes, dot runs ellipses.
            if smart {
                match ch {
                    '"' | '\'' => {
                        if let Some((quoted, next)) = self.smart_quote(&chars, pos, ch) {
                            push_text(&mut out, &pending);
                            pending.clear();
                            out.push(quoted);
                            pos = next;
                            continue;
                        }
                        pending.push(quote_glyph(&chars, pos, ch));
                        pos += 1;
                        continue;
                    }
                    '-' => {
                        let n = run_length(&chars, pos, '-');
                        pending.push_str(&fold_dash_run_greedy(n));
                        pos += n;
                        continue;
                    }
                    '.' => {
                        let n = run_length(&chars, pos, '.');
                        pending.push_str(&fold_ellipsis_run(n));
                        pos += n;
                        continue;
                    }
                    _ => {}
                }
            }
            pending.push(ch);
            pos += 1;
        }
        push_text(&mut out, &pending);
        out
    }

    /// An inline internal hyperlink target (written `` _`name` `` in source): a span whose
    /// identifier is the slug of its text, marking a location elsewhere markup can link to.
    fn inline_target(&mut self, chars: &[char], pos: usize) -> Option<(Inline, usize)> {
        let (name, end) = find_close_literal(chars, pos + 2, "`")?;
        if name.trim().is_empty() {
            return None;
        }
        let inner = self.inlines(&name);
        let id = carta_ast::slug(&carta_ast::to_plain_text(&inner));
        Some((
            Inline::Span(
                Box::new(Attr {
                    id: id.into(),
                    classes: Vec::new(),
                    attributes: Vec::new(),
                }),
                inner,
            ),
            end,
        ))
    }

    /// A quoted run opened by a straight quote: scan for a matching closer and, on success, parse the
    /// interior recursively into a quotation node. Returns `None` when the quote cannot open a run or
    /// has no closer, leaving the caller to fold it into a lone glyph.
    fn smart_quote(&mut self, chars: &[char], pos: usize, quote: char) -> Option<(Inline, usize)> {
        if !can_open_quote(chars, pos, quote, QuoteCtx::default()) {
            return None;
        }
        // A single quote after a letter or digit is a word-internal apostrophe, not an opener.
        if quote == '\'' {
            let before = pos.checked_sub(1).and_then(|p| chars.get(p)).copied();
            if before.is_some_and(char::is_alphanumeric) {
                return None;
            }
        }
        let mut j = pos + 1;
        while j < chars.len() {
            match chars.get(j).copied() {
                Some('\\') => j += 2,
                Some(c) if c == quote && can_close_quote(chars, j, quote) => {
                    let content: String = chars.get(pos + 1..j)?.iter().collect();
                    let inner = self.inlines(&content);
                    return Some((Inline::Quoted(quote_type(quote), inner), j + 1));
                }
                Some(_) => j += 1,
                None => break,
            }
        }
        None
    }

    /// Attempt to parse inline markup at `pos`. On success returns the produced inlines, whether a
    /// directly preceding space should be dropped (footnotes), and the index past the construct.
    fn try_markup(&mut self, chars: &[char], pos: usize) -> Option<(Vec<Inline>, bool, usize)> {
        let ch = chars.get(pos).copied()?;
        let prev = pos.checked_sub(1).and_then(|p| chars.get(p)).copied();
        match ch {
            '`' => self.backtick(chars, pos, prev),
            '*' => Self::emphasis(chars, pos, prev),
            '|' => self.substitution(chars, pos, prev),
            '[' => self.note_reference(chars, pos, prev),
            ':' => self.role_prefix(chars, pos, prev),
            _ => None,
        }
    }

    fn emphasis(
        chars: &[char],
        pos: usize,
        prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        if !inline_start_ok(prev) {
            return None;
        }
        if chars.get(pos + 1) == Some(&'*') {
            if chars.get(pos + 2).is_none_or(|c| c.is_whitespace()) {
                return None;
            }
            let (inner, end) = Self::scan_strong(chars, pos)?;
            if quote_suppresses(prev, chars.get(end).copied()) {
                return None;
            }
            return Some((vec![Inline::Strong(inner)], false, end));
        }
        if chars.get(pos + 1).is_none_or(|c| c.is_whitespace()) {
            return None;
        }
        let (inner, end) = Self::scan_emphasis(chars, pos)?;
        if quote_suppresses(prev, chars.get(end).copied()) {
            return None;
        }
        Some((vec![Inline::Emph(inner)], false, end))
    }

    /// Scan a strong span opened by `**` at `pos`. Its content is verbatim text in which a single `*`
    /// is an ordinary character; the first later run of two or more `*` closes the span. Returns the
    /// parsed content and the index past the closing delimiter, or `None` when no closer is found.
    fn scan_strong(chars: &[char], pos: usize) -> Option<(Vec<Inline>, usize)> {
        let mut pending = String::new();
        let mut i = pos + 2;
        while i < chars.len() {
            match chars.get(i).copied() {
                Some('\\') => {
                    pending.push('\\');
                    if let Some(&next) = chars.get(i + 1) {
                        pending.push(next);
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                Some('*') if run_length(chars, i, '*') >= 2 => {
                    return Some((literal_text(&pending), i + 2));
                }
                Some(c) => {
                    pending.push(c);
                    i += 1;
                }
                None => break,
            }
        }
        None
    }

    /// Scan an emphasis span opened by a single `*` at `pos`. A later single `*`, or a `**` run that
    /// is followed by whitespace, closes the span (consuming one `*`); a `**` run followed by content
    /// is an inner strong start-string that is stripped, flushing the text gathered so far as its own
    /// segment. Returns the content segments and the index past the closing `*`, or `None` with no
    /// closer.
    fn scan_emphasis(chars: &[char], pos: usize) -> Option<(Vec<Inline>, usize)> {
        let mut result = Vec::new();
        let mut pending = String::new();
        let mut i = pos + 1;
        while i < chars.len() {
            match chars.get(i).copied() {
                Some('\\') => {
                    pending.push('\\');
                    if let Some(&next) = chars.get(i + 1) {
                        pending.push(next);
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                Some('*') => {
                    let run = run_length(chars, i, '*');
                    let after = chars.get(i + run).copied();
                    if run >= 2 && after.is_some_and(|c| !c.is_whitespace()) {
                        result.extend(literal_text(&pending));
                        pending.clear();
                        i += run;
                    } else {
                        result.extend(literal_text(&pending));
                        return Some((result, i + 1));
                    }
                }
                Some(c) => {
                    pending.push(c);
                    i += 1;
                }
                None => break,
            }
        }
        None
    }

    fn backtick(
        &mut self,
        chars: &[char],
        pos: usize,
        prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        // Inline literals match even mid-word; other backtick constructs need a preceding boundary.
        if chars.get(pos + 1) == Some(&'`') {
            let (content, end) = find_close_literal(chars, pos + 2, "``")?;
            return Some((
                vec![Inline::Code(
                    Box::default(),
                    normalize_inline_literal(&content).into(),
                )],
                false,
                end,
            ));
        }
        if !inline_start_ok(prev) {
            return None;
        }
        let (content, mut end) = find_close_literal(chars, pos + 1, "`")?;
        // A trailing underscore turns interpreted text into a hyperlink reference.
        if chars.get(end) == Some(&'_') {
            let anonymous = chars.get(end + 1) == Some(&'_');
            end += if anonymous { 2 } else { 1 };
            if quote_suppresses(prev, chars.get(end).copied()) {
                return None;
            }
            return Some((vec![self.phrase_reference(&content, anonymous)], false, end));
        }
        if chars.get(end) == Some(&':')
            && let Some((role, role_end)) = parse_role(chars, end)
        {
            if quote_suppresses(prev, chars.get(role_end).copied()) {
                return None;
            }
            let inline = self.apply_role(&role, &content);
            return Some((vec![inline], false, role_end));
        }
        if quote_suppresses(prev, chars.get(end).copied()) {
            return None;
        }
        let role = self.default_role.clone();
        Some((vec![self.apply_role(&role, &content)], false, end))
    }

    fn role_prefix(
        &mut self,
        chars: &[char],
        pos: usize,
        prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        if !inline_start_ok(prev) {
            return None;
        }
        let (role, after) = parse_role(chars, pos)?;
        if chars.get(after) != Some(&'`') {
            return None;
        }
        let (content, end) = find_close_literal(chars, after + 1, "`")?;
        Some((vec![self.apply_role(&role, &content)], false, end))
    }

    fn apply_role(&mut self, role: &str, content: &str) -> Inline {
        let chain = self.resolve_role(role);
        match chain.base.as_str() {
            "emphasis" => Inline::Emph(self.inlines_no_trim(content)),
            "strong" => Inline::Strong(self.inlines_no_trim(content)),
            "subscript" | "sub" => Inline::Subscript(self.inlines_no_trim(content)),
            "superscript" | "sup" => Inline::Superscript(self.inlines_no_trim(content)),
            "math" => Inline::Math(MathType::InlineMath, content.into()),
            // Raw role: content verbatim under the chain's format (may be empty); classes do not apply.
            "raw" => Inline::RawInline(
                Format(chain.format.unwrap_or_default().into()),
                content.into(),
            ),
            // A code/literal role's content is verbatim; a chain's classes lead, then the language.
            "literal" | "code" => {
                let mut classes = chain.classes;
                if let Some(language) = chain.language {
                    classes.push(language);
                }
                Inline::Code(Box::new(class_attr(classes)), content.into())
            }
            "title-reference" | "title" | "t" => {
                let mut classes = chain.classes;
                classes.push("title-ref".to_string());
                Inline::Span(Box::new(class_attr(classes)), self.inlines_no_trim(content))
            }
            "" => Inline::Span(
                Box::new(class_attr(chain.classes)),
                self.inlines_no_trim(content),
            ),
            // Unrecognized role: content verbatim, tagged with the role name to survive a round-trip.
            other => Inline::Code(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["interpreted-text".into()],
                    attributes: vec![("role".into(), other.into())],
                }),
                content.into(),
            ),
        }
    }

    /// Follow a custom-role chain to the builtin role that supplies its rendering, accumulating the
    /// classes each role in the chain contributes (its `:class:` list, or its own name when it sets
    /// none) outermost-first, along with the first `:format:` and `:language:` the chain declares.
    /// `base` is the builtin role name, an unknown role name, or empty for a plain (baseless) role.
    fn resolve_role(&self, role: &str) -> RoleChain {
        let mut chain = RoleChain::default();
        let mut current = role.to_string();
        let mut seen = std::collections::BTreeSet::new();
        loop {
            if !seen.insert(current.clone()) {
                return chain;
            }
            let Some(def) = self.custom_roles.get(&current) else {
                chain.base = current;
                return chain;
            };
            if def.classes.is_empty() {
                chain.classes.push(current.clone());
            } else {
                chain.classes.extend(def.classes.iter().cloned());
            }
            if chain.format.is_none() {
                chain.format.clone_from(&def.format);
            }
            if chain.language.is_none() {
                chain.language.clone_from(&def.language);
            }
            match &def.base {
                Some(base) => current.clone_from(base),
                None => return chain,
            }
        }
    }

    fn substitution(
        &mut self,
        chars: &[char],
        pos: usize,
        _prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        if chars.get(pos + 1).is_some_and(|c| c.is_whitespace()) {
            return None;
        }
        let (name, mut end) = find_close_literal(chars, pos + 1, "|")?;
        let referenced = chars.get(end) == Some(&'_');
        if referenced {
            end += 1;
        }
        let key = normalize_name(&name);
        // Keep cyclic substitutions unresolved to prevent unbounded recursion.
        if self.active_substitutions.contains(&key) {
            let mut display = Vec::new();
            push_text(&mut display, &format!("|{name}|"));
            return Some((
                vec![Inline::Link(
                    Box::default(),
                    display,
                    Box::new(Target {
                        url: format!("##SUBST##|{name}|").into(),
                        title: carta_ast::Text::default(),
                    }),
                )],
                false,
                end,
            ));
        }
        let definition = self.defs.substitutions.get(&key);
        if let Some(definition) = definition
            && !self.spend(definition.weight())
        {
            return None;
        }
        let expansion = match definition.cloned() {
            Some(Substitution::Replace(text)) => {
                self.active_substitutions.insert(key.clone());
                let inlines = self.inlines(&text);
                self.active_substitutions.remove(&key);
                // A multi-inline replacement is kept together as one unit.
                match inlines.len() {
                    1 => inlines,
                    _ => vec![Inline::Span(Box::default(), inlines)],
                }
            }
            Some(Substitution::Image(url, attr, alt)) => vec![Inline::Image(
                Box::new(attr),
                alt,
                Box::new(Target {
                    url: url.into(),
                    title: carta_ast::Text::default(),
                }),
            )],
            None => {
                let mut display = Vec::new();
                push_text(&mut display, &format!("|{name}|"));
                return Some((
                    vec![Inline::Link(
                        Box::default(),
                        display,
                        Box::new(Target {
                            url: format!("##SUBST##|{name}|").into(),
                            title: carta_ast::Text::default(),
                        }),
                    )],
                    false,
                    end,
                ));
            }
        };
        let result = if referenced {
            vec![Inline::Link(
                Box::default(),
                expansion,
                Box::new(Target {
                    url: defer_reference(&name).into(),
                    title: carta_ast::Text::default(),
                }),
            )]
        } else {
            expansion
        };
        Some((result, false, end))
    }

    fn note_reference(
        &mut self,
        chars: &[char],
        pos: usize,
        prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        if !inline_start_ok(prev) {
            return None;
        }
        let (label, after) = find_close_literal(chars, pos + 1, "]")?;
        if chars.get(after) != Some(&'_') {
            return None;
        }
        let end = after + 1;
        if !inline_end_ok(chars.get(end).copied()) {
            return None;
        }
        if is_citation_label(&label) {
            let url = format!("#{label}");
            let link = Inline::Link(
                Box::new(Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["citation".into()],
                    attributes: Vec::new(),
                }),
                vec![Inline::Str(format!("[{label}]").into())],
                Box::new(Target {
                    url: url.into(),
                    title: carta_ast::Text::default(),
                }),
            );
            return Some((vec![link], false, end));
        }
        let named = !matches!(label.as_str(), "#" | "*");
        if named && self.active_footnotes.contains(&label) {
            return None;
        }
        let body = self.footnote_body_for(&label)?;
        let weight = body.iter().fold(0usize, |weight, line| {
            weight.saturating_add(line.len()).saturating_add(1)
        });
        if !self.spend(weight) {
            return None;
        }
        if named {
            self.active_footnotes.insert(label.clone());
        }
        let blocks = self.blocks(&body);
        if named {
            self.active_footnotes.remove(&label);
        }
        Some((vec![Inline::Note(blocks)], true, end))
    }

    fn footnote_body_for(&mut self, label: &str) -> Option<Vec<String>> {
        if label == "#" {
            let body = self.defs.auto_footnotes.get(self.auto_footnote)?.clone();
            self.auto_footnote += 1;
            Some(body)
        } else if label == "*" {
            let body = self
                .defs
                .symbol_footnotes
                .get(self.symbol_footnote)?
                .clone();
            self.symbol_footnote += 1;
            Some(body)
        } else {
            self.defs.footnotes.get(label).cloned()
        }
    }

    fn phrase_reference(&mut self, text: &str, anonymous: bool) -> Inline {
        let (label, url) = split_embedded_uri(text);
        let display = if label.trim().is_empty() {
            url.clone().unwrap_or_default()
        } else {
            label.clone()
        };
        let target = match url {
            // An embedded destination naming another target (`<other_>`) resolves through the reference table.
            Some(url) => match indirect_referent(&url) {
                Some(referent) => defer_reference(&referent),
                None => url,
            },
            None if anonymous => self.next_anonymous(),
            None => defer_reference(&label),
        };
        // A named phrase reference with an embedded destination also defines the label as a target.
        if !anonymous && !label.trim().is_empty() && !target.starts_with(REF_SENTINEL) {
            self.deferred.insert(normalize_name(&label), target.clone());
        }
        Inline::Link(
            Box::default(),
            self.inlines(&display),
            Box::new(Target {
                url: target.into(),
                title: carta_ast::Text::default(),
            }),
        )
    }

    /// Close a simple reference `name_` (or anonymous `name__`) whose name is the trailing run of
    /// name characters already accumulated in `pending`. The name is removed from `pending` and the
    /// link returned, with the index past the closing underscore(s).
    fn simple_reference(
        &mut self,
        chars: &[char],
        pos: usize,
        pending: &mut String,
    ) -> Option<(Inline, usize)> {
        let anonymous = chars.get(pos + 1) == Some(&'_');
        let after = pos + if anonymous { 2 } else { 1 };
        if !inline_end_ok(chars.get(after).copied()) {
            return None;
        }
        let (name, before_name) = trailing_reference_name(pending)?;
        // Name must start at a word boundary (not the trailing run of `__init__` or `b` in `a __b__ c`).
        if !inline_start_ok(before_name) {
            return None;
        }
        // Matching quotes suppress the reference: quotes and underscore stay literal.
        if quote_suppresses(before_name, chars.get(after).copied()) {
            return None;
        }
        let keep = pending.len().saturating_sub(name.len());
        pending.truncate(keep);
        let url = if anonymous {
            self.next_anonymous()
        } else {
            defer_reference(&name)
        };
        let link = Inline::Link(
            Box::default(),
            vec![Inline::Str(name.into())],
            Box::new(Target {
                url: url.into(),
                title: carta_ast::Text::default(),
            }),
        );
        Some((link, after))
    }

    /// Resolve a normalized reference name to its destination, following an indirect chain (a target
    /// whose destination is another target's name) to a concrete URL. Returns an empty string when
    /// the name is undefined or the chain forms a cycle.
    fn lookup_url(&self, name: &str) -> String {
        let mut current = name.to_string();
        let mut seen = std::collections::BTreeSet::new();
        while seen.insert(current.clone()) {
            let Some(url) = self.deferred.get(&current) else {
                return String::new();
            };
            let referent = indirect_referent(url)
                .map(|r| normalize_name(&r))
                .filter(|key| self.deferred.contains_key(key));
            match referent {
                Some(next) => current = next,
                None => return url.clone(),
            }
        }
        String::new()
    }

    /// Fill in every link and image destination left deferred during tree construction, now that all
    /// targets, sections, and phrase-reference labels have been registered.
    pub(super) fn resolve_deferred(&self, blocks: &mut [Block]) {
        let mut resolve = |target: &mut Target| {
            if let Some(name) = target.url.strip_prefix(REF_SENTINEL) {
                target.url = self.lookup_url(name).into();
            }
        };
        carta_core::walk::for_each_link_target(blocks, &mut resolve);
        carta_core::walk::for_each_image_target(blocks, &mut resolve);
    }

    fn next_anonymous(&mut self) -> String {
        let idx = self.anonymous;
        self.anonymous += 1;
        self.defs.anonymous.get(idx).cloned().unwrap_or_default()
    }
}
