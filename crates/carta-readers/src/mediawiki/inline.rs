//! Inline lexing and construction of the inline node stream.

use carta_ast::{Attr, Inline, MathType};
use carta_core::Extension;

use crate::entities;

use super::emphasis::{
    apply_smart_quotes, coalesce, drop_east_asian_breaks, preformat_transform, resolve_emphasis,
};
use super::links::bare_url;
use super::tags::{
    block_tag_token, close_tag_bounded, close_tag_parse, enclosed, html_tag_role, open_tag_bounded,
    starts_block_tag,
};
use super::{
    HtmlTagRole, Parser, ScanBounds, Tok, at, balanced_braces, collect_range, format_mediawiki,
    raw_html, template_opens,
};

impl Parser {
    pub(super) fn parse_inlines(&mut self, text: &str) -> Vec<Inline> {
        let chars: Vec<char> = text.chars().collect();
        let toks = self.lex(&chars, false, false);
        let mut inlines = coalesce(resolve_emphasis(toks));
        if self.extensions.contains(Extension::EastAsianLineBreaks) {
            inlines = drop_east_asian_breaks(inlines);
        }
        if self.smart() {
            inlines = apply_smart_quotes(inlines);
        }
        inlines
    }

    /// Parses one preformatted line: markup is honored, but literal text and its exact spacing are
    /// preserved as code spans rather than collapsed.
    pub(super) fn preformatted_line(&mut self, text: &str) -> Vec<Inline> {
        let chars: Vec<char> = text.chars().collect();
        let toks = self.lex(&chars, true, false);
        preformat_transform(resolve_emphasis(toks))
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn lex(
        &mut self,
        chars: &[char],
        preformatted: bool,
        block_context: bool,
    ) -> Vec<Tok> {
        let mut toks: Vec<Tok> = Vec::new();
        let mut word = String::new();
        let mut i = 0;
        let n = chars.len();
        let bounds = ScanBounds::of(chars);
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
                if let Some((decoded, next)) = entities::read_reference(chars, i, chars.len(), true)
                {
                    word.push_str(&decoded);
                    i = next;
                } else {
                    word.push('&');
                    i += 1;
                }
                continue;
            }
            if c == '<' {
                if let Some((inlines, next)) = self.handle_tag(chars, i, bounds) {
                    flush_word(&mut word, &mut toks);
                    for inline in inlines {
                        toks.push(Tok::Inline(inline));
                    }
                    i = next;
                    continue;
                }
                if block_context
                    && starts_block_tag(chars, i)
                    && let Some((block, next)) = self.parse_block_tag(chars, i, bounds)
                {
                    flush_word(&mut word, &mut toks);
                    toks.push(Tok::Block(block));
                    i = next;
                    continue;
                }
                if let Some((tok, next)) = block_tag_token(chars, i, bounds) {
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
                if template_opens(chars, i)
                    && let Some(after) = balanced_braces(chars, i)
                {
                    flush_word(&mut word, &mut toks);
                    let raw = collect_range(chars, i, after);
                    toks.push(Tok::Inline(Inline::RawInline(
                        format_mediawiki(),
                        raw.into(),
                    )));
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
                // A single `[` glued to a bare URL is a literal bracket followed by that URL.
                if at(chars, i + 1) != Some('[')
                    && let Some((inline, next)) = bare_url(chars, i + 1)
                {
                    word.push('[');
                    flush_word(&mut word, &mut toks);
                    toks.push(Tok::Inline(inline));
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
    fn handle_tag(
        &mut self,
        chars: &[char],
        i: usize,
        bounds: ScanBounds,
    ) -> Option<(Vec<Inline>, usize)> {
        if at(chars, i) != Some('<') {
            return None;
        }
        match at(chars, i + 1) {
            Some('/') => {
                let (name, raw, after) = close_tag_parse(chars, i, bounds)?;
                return match html_tag_role(&name) {
                    Some(HtmlTagRole::Inline) => Some((vec![raw_html(raw)], after)),
                    _ => None,
                };
            }
            Some(c) if c.is_ascii_alphabetic() => {}
            _ => return None,
        }
        let (name, raw_open, self_closing, after_open) = open_tag_bounded(chars, i, bounds)?;
        match name.as_str() {
            "br" => Some((vec![Inline::LineBreak], after_open)),
            "ref" => {
                if self_closing {
                    return Some((vec![Inline::Note(Vec::new())], after_open));
                }
                match close_tag_bounded(chars, after_open, "ref", bounds) {
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
                let (inner, after) = enclosed(chars, after_open, "nowiki", bounds);
                Some((plain_inlines(&inner), after))
            }
            "math" => {
                if self_closing {
                    return Some((Vec::new(), after_open));
                }
                match close_tag_bounded(chars, after_open, "math", bounds) {
                    Some((inner_end, after)) => {
                        let inner = collect_range(chars, after_open, inner_end);
                        Some((
                            vec![Inline::Math(MathType::InlineMath, inner.trim().into())],
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
                bounds,
            )),
            "var" => Some(verbatim_code(
                chars,
                "var",
                after_open,
                &raw_open,
                self_closing,
                &["variable"],
                bounds,
            )),
            "samp" => Some(verbatim_code(
                chars,
                "samp",
                after_open,
                &raw_open,
                self_closing,
                &["sample"],
                bounds,
            )),
            "sub" => Some(self.wrap(
                chars,
                "sub",
                after_open,
                &raw_open,
                self_closing,
                Inline::Subscript,
                bounds,
            )),
            "sup" => Some(self.wrap(
                chars,
                "sup",
                after_open,
                &raw_open,
                self_closing,
                Inline::Superscript,
                bounds,
            )),
            "del" | "strike" => Some(self.wrap(
                chars,
                &name,
                after_open,
                &raw_open,
                self_closing,
                Inline::Strikeout,
                bounds,
            )),
            "kbd" => Some(self.span(
                chars,
                "kbd",
                after_open,
                &raw_open,
                self_closing,
                "kbd",
                bounds,
            )),
            "mark" => Some(self.span(
                chars,
                "mark",
                after_open,
                &raw_open,
                self_closing,
                "mark",
                bounds,
            )),
            _ => match html_tag_role(&name) {
                Some(HtmlTagRole::Inline) => {
                    if self_closing {
                        return Some((vec![raw_html(raw_open)], after_open));
                    }
                    match close_tag_bounded(chars, after_open, &name, bounds) {
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
                // Recognized block tags become raw blocks at the paragraph level; unrecognized tags stay literal.
                _ => None,
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn wrap(
        &mut self,
        chars: &[char],
        name: &str,
        after_open: usize,
        raw_open: &str,
        self_closing: bool,
        ctor: fn(Vec<Inline>) -> Inline,
        bounds: ScanBounds,
    ) -> (Vec<Inline>, usize) {
        if self_closing {
            return (vec![raw_html(raw_open.to_string())], after_open);
        }
        match close_tag_bounded(chars, after_open, name, bounds) {
            Some((inner_end, after)) => {
                let inner = collect_range(chars, after_open, inner_end);
                (vec![ctor(self.parse_inlines(&inner))], after)
            }
            None => (vec![raw_html(raw_open.to_string())], after_open),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn span(
        &mut self,
        chars: &[char],
        name: &str,
        after_open: usize,
        raw_open: &str,
        self_closing: bool,
        class: &str,
        bounds: ScanBounds,
    ) -> (Vec<Inline>, usize) {
        if self_closing {
            return (vec![raw_html(raw_open.to_string())], after_open);
        }
        match close_tag_bounded(chars, after_open, name, bounds) {
            Some((inner_end, after)) => {
                let inner = collect_range(chars, after_open, inner_end);
                let attr = Attr {
                    id: carta_ast::Text::default(),
                    classes: vec![class.into()],
                    attributes: Vec::new(),
                };
                (
                    vec![Inline::Span(Box::new(attr), self.parse_inlines(&inner))],
                    after,
                )
            }
            None => (vec![raw_html(raw_open.to_string())], after_open),
        }
    }
}

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
                out.push(Inline::Str(std::mem::take(&mut word).into()));
            }
            let (token, next) = whitespace_token(&chars, i);
            out.push(token);
            i = next;
        } else if c == '&' {
            if let Some((decoded, next)) = entities::read_reference(&chars, i, chars.len(), true) {
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
        out.push(Inline::Str(word.into()));
    }
    out
}

fn decode_entities(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        if at(&chars, i) == Some('&')
            && let Some((decoded, next)) = entities::read_reference(&chars, i, chars.len(), true)
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

/// Parses `<code>`-family verbatim content into a [`Inline::Code`] node carrying `classes`, with
/// entity references decoded. An unterminated tag degrades to its literal opening as raw HTML.
fn verbatim_code(
    chars: &[char],
    name: &str,
    after_open: usize,
    raw_open: &str,
    self_closing: bool,
    classes: &[&str],
    bounds: ScanBounds,
) -> (Vec<Inline>, usize) {
    if self_closing {
        return (vec![raw_html(raw_open.to_string())], after_open);
    }
    match close_tag_bounded(chars, after_open, name, bounds) {
        Some((inner_end, after)) => {
            let inner = collect_range(chars, after_open, inner_end);
            let attr = Attr {
                id: carta_ast::Text::default(),
                classes: classes.iter().map(|s| (*s).into()).collect(),
                attributes: Vec::new(),
            };
            (
                vec![Inline::Code(Box::new(attr), decode_entities(&inner).into())],
                after,
            )
        }
        None => (vec![raw_html(raw_open.to_string())], after_open),
    }
}

fn flush_word(word: &mut String, toks: &mut Vec<Tok>) {
    if !word.is_empty() {
        toks.push(Tok::Inline(Inline::Str(std::mem::take(word).into())));
    }
}
