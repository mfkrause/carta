//! Reader for `MediaWiki`'s wikitext markup.
//!
//! The source is first cleared of comments (`<!-- … -->`), then parsed line by line into blocks.
//! A line opening with `=` runs is a heading, `*`/`#`/`:`/`;` runs start lists, four or more `-`
//! alone are a horizontal rule, a leading space marks preformatted text, `{{…}}` and `{|…|}` are
//! template and table markup, and `<pre>`/`<blockquote>`/`<syntaxhighlight>` are recognized block
//! tags; everything else is a paragraph. Inline markup — apostrophe emphasis, `[[internal]]` and
//! `[external]` links, bare URLs, entity references, and a fixed set of HTML tags — is scanned
//! within each block's text.
//!
//! Heading identifiers follow the enabled identifier scheme: with `gfm_auto_identifiers` the GitHub
//! algorithm (hyphen separators), otherwise `auto_identifiers` lowercases the text, keeps
//! alphanumerics together with `_` and `.`, turns spaces and `-` into single `_`, and drops a
//! leading run of non-letters; duplicates gain a numeric suffix and an empty result becomes
//! `section`. With neither enabled, headings carry no identifier.
//!
//! The scanner is panic-free on malformed input: unbalanced or unterminated constructs degrade to
//! literal text rather than being rejected.

use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{
    ApiVersion, Attr, Block, Caption, Document, Format, Inline, ListAttributes, ListNumberDelim,
    ListNumberStyle, MathType, Target, slug_gfm, to_plain_text,
};
use carta_core::{Extension, Extensions, Reader, ReaderOptions, Result};

use crate::entities;

/// Parses a wikitext document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct MediawikiReader;

impl Reader for MediawikiReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let source = strip_comments(input);
        let chars: Vec<char> = source.chars().collect();
        let mut parser = Parser::new(options);
        let blocks = parser.parse_blocks(&chars);
        Ok(Document {
            api_version: ApiVersion::default(),
            meta: BTreeMap::new(),
            blocks,
        })
    }
}

/// Carries the state that spans a whole document: the enabled extensions, the running counter for
/// unlabeled external links, and the set of heading identifiers already issued (for de-duplication).
struct Parser {
    extensions: Extensions,
    link_counter: usize,
    seen_ids: BTreeSet<String>,
}

/// One line of list markup: its leading marker run and the trimmed text that follows.
struct ListItem {
    markers: Vec<char>,
    content: String,
}

/// The list family a marker character opens.
#[derive(PartialEq, Eq, Clone, Copy)]
enum ListKind {
    Bullet,
    Ordered,
    Definition,
}

/// A lexical unit of inline text: either a finished inline node or a run of apostrophes whose
/// emphasis role is resolved once the surrounding run structure is known.
enum Tok {
    Inline(Inline),
    Apostrophes(usize),
}

/// An open emphasis span awaiting its closing run.
struct Frame {
    strong: bool,
    marker_len: usize,
    buffer: Vec<Inline>,
}

/// How an apostrophe run toggles emphasis.
enum Toggle {
    Emph,
    Strong,
    Both,
}

impl Parser {
    fn new(options: &ReaderOptions) -> Self {
        Self {
            extensions: options.extensions,
            link_counter: 0,
            seen_ids: BTreeSet::new(),
        }
    }

    fn parse_blocks(&mut self, chars: &[char]) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        let mut pos = 0;
        let mut line_start = true;
        let n = chars.len();
        while pos < n {
            if line_start {
                let le = line_end(chars, pos);
                if is_blank(chars, pos, le) {
                    pos = if le < n { le + 1 } else { le };
                    continue;
                }
                let c = at(chars, pos).unwrap_or(' ');
                if c == '{'
                    && at(chars, pos + 1) == Some('{')
                    && let Some(after) = balanced_braces(chars, pos)
                {
                    let raw = collect_range(chars, pos, after);
                    blocks.push(Block::RawBlock(format_mediawiki(), raw));
                    let (np, ls) = finish_inline_block(chars, after);
                    pos = np;
                    line_start = ls;
                    continue;
                }
                if c == '{' && at(chars, pos + 1) == Some('|') {
                    let after = table_block_end(chars, pos);
                    let raw = collect_range(chars, pos, after);
                    blocks.push(Block::RawBlock(format_mediawiki(), raw));
                    let (np, ls) = finish_inline_block(chars, after);
                    pos = np;
                    line_start = ls;
                    continue;
                }
                if c == '='
                    && let Some((level, inlines, closer_end)) = self.try_header(chars, pos)
                {
                    let id = self.make_id(&inlines);
                    let attr = Attr {
                        id,
                        classes: Vec::new(),
                        attributes: Vec::new(),
                    };
                    blocks.push(Block::Header(level, attr, inlines));
                    let (np, ls) = finish_inline_block(chars, closer_end);
                    pos = np;
                    line_start = ls;
                    continue;
                }
                if c == '-' && is_hr_line(chars, pos) {
                    blocks.push(Block::HorizontalRule);
                    let le2 = line_end(chars, pos);
                    pos = if le2 < n { le2 + 1 } else { le2 };
                    line_start = true;
                    continue;
                }
                if matches!(c, '*' | '#' | ':' | ';') {
                    let (list_blocks, after) = self.parse_list(chars, pos);
                    blocks.extend(list_blocks);
                    pos = after;
                    line_start = true;
                    continue;
                }
                if c == ' ' {
                    let (block, after) = self.parse_preformatted(chars, pos);
                    blocks.push(block);
                    pos = after;
                    line_start = true;
                    continue;
                }
                if c == '<'
                    && let Some((block, after)) = self.parse_block_tag(chars, pos)
                {
                    blocks.push(block);
                    let (np, ls) = finish_inline_block(chars, after);
                    pos = np;
                    line_start = ls;
                    continue;
                }
            }
            let (block, after) = self.parse_paragraph(chars, pos);
            if let Some(block) = block {
                blocks.push(block);
            }
            pos = after;
            line_start = true;
        }
        blocks
    }

    fn try_header(&mut self, chars: &[char], pos: usize) -> Option<(i32, Vec<Inline>, usize)> {
        let le = line_end(chars, pos);
        let mut m = 0;
        while pos + m < le && at(chars, pos + m) == Some('=') {
            m += 1;
        }
        if m == 0 || m > 6 {
            return None;
        }
        let content_start = pos + m;
        let closer = header_closer(chars, content_start, le, m)?;
        let content = collect_range(chars, content_start, closer);
        let inlines = self.parse_inlines(content.trim());
        Some((i32::try_from(m).unwrap_or(1), inlines, closer + m))
    }

    fn parse_list(&mut self, chars: &[char], pos: usize) -> (Vec<Block>, usize) {
        let mut items: Vec<ListItem> = Vec::new();
        let mut cursor = pos;
        let n = chars.len();
        while at(chars, cursor).is_some_and(is_list_marker) {
            let le = line_end(chars, cursor);
            let mut scan = cursor;
            let mut markers: Vec<char> = Vec::new();
            while scan < le && at(chars, scan).is_some_and(is_list_marker) {
                if let Some(marker) = at(chars, scan) {
                    markers.push(marker);
                }
                scan += 1;
            }
            let content = collect_range(chars, scan, le).trim().to_string();
            items.push(ListItem { markers, content });
            if le >= n {
                cursor = le;
                break;
            }
            cursor = le + 1;
        }
        (self.build_lists(&items, 0), cursor)
    }

    fn build_lists(&mut self, items: &[ListItem], level: usize) -> Vec<Block> {
        let mut out: Vec<Block> = Vec::new();
        let mut i = 0;
        while i < items.len() {
            let kind = if let Some(&m) = items.get(i).and_then(|it| it.markers.get(level)) {
                list_kind(m)
            } else {
                i += 1;
                continue;
            };
            let mut j = i;
            while j < items.len() {
                match items.get(j).and_then(|it| it.markers.get(level)) {
                    Some(&m) if list_kind(m) == kind => j += 1,
                    _ => break,
                }
            }
            let group = items.get(i..j).unwrap_or(&[]);
            match kind {
                ListKind::Bullet => out.push(Block::BulletList(self.build_simple(group, level))),
                ListKind::Ordered => {
                    out.push(Block::OrderedList(
                        default_list_attrs(),
                        self.build_simple(group, level),
                    ));
                }
                ListKind::Definition => out.push(self.build_definition(group, level)),
            }
            i = j;
        }
        out
    }

    fn build_simple(&mut self, group: &[ListItem], level: usize) -> Vec<Vec<Block>> {
        let mut entries: Vec<Vec<Block>> = Vec::new();
        let mut i = 0;
        while i < group.len() {
            let depth = group.get(i).map_or(0, |it| it.markers.len());
            if depth == level + 1 {
                let content = group.get(i).map_or("", |it| it.content.as_str());
                let mut blocks = vec![plain_or_figure(self.parse_inlines(content))];
                i += 1;
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                if let Some(sub) = group.get(start..i)
                    && !sub.is_empty()
                {
                    blocks.extend(self.build_lists(sub, level + 1));
                }
                entries.push(blocks);
            } else {
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                if i == start {
                    i += 1;
                }
                let blocks = group
                    .get(start..i)
                    .map(|sub| self.build_lists(sub, level + 1))
                    .unwrap_or_default();
                entries.push(blocks);
            }
        }
        entries
    }

    fn build_definition(&mut self, group: &[ListItem], level: usize) -> Block {
        let mut pairs: Vec<(Vec<Inline>, Vec<Vec<Block>>)> = Vec::new();
        let mut i = 0;
        while i < group.len() {
            let Some(item) = group.get(i) else { break };
            if item.markers.len() == level + 1 {
                let marker = item.markers.get(level).copied().unwrap_or(':');
                let content = item.content.clone();
                i += 1;
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                let nested = group
                    .get(start..i)
                    .map(|sub| self.build_lists(sub, level + 1))
                    .unwrap_or_default();
                if marker == ';' {
                    let (term_str, def_str) = split_term(&content);
                    let term = self.parse_inlines(&term_str);
                    let mut defs: Vec<Vec<Block>> = Vec::new();
                    if let Some(d) = def_str {
                        defs.push(vec![plain_or_figure(self.parse_inlines(&d))]);
                    }
                    if !nested.is_empty() {
                        match defs.last_mut() {
                            Some(last) => last.extend(nested),
                            None => defs.push(nested),
                        }
                    }
                    pairs.push((term, defs));
                } else {
                    let mut blocks = vec![plain_or_figure(self.parse_inlines(&content))];
                    blocks.extend(nested);
                    match pairs.last_mut() {
                        Some(last) => last.1.push(blocks),
                        None => pairs.push((Vec::new(), vec![blocks])),
                    }
                }
            } else {
                let start = i;
                while i < group.len() && group.get(i).map_or(0, |it| it.markers.len()) > level + 1 {
                    i += 1;
                }
                if i == start {
                    i += 1;
                }
                let nested = group
                    .get(start..i)
                    .map(|sub| self.build_lists(sub, level + 1))
                    .unwrap_or_default();
                match pairs.last_mut() {
                    Some(last) => match last.1.last_mut() {
                        Some(d) => d.extend(nested),
                        None => last.1.push(nested),
                    },
                    None => pairs.push((Vec::new(), vec![nested])),
                }
            }
        }
        Block::DefinitionList(pairs)
    }

    fn parse_preformatted(&mut self, chars: &[char], pos: usize) -> (Block, usize) {
        let n = chars.len();
        let mut p = pos;
        let mut lines: Vec<Vec<Inline>> = Vec::new();
        while at(chars, p) == Some(' ') {
            let le = line_end(chars, p);
            let content = collect_range(chars, p + 1, le);
            lines.push(self.preformatted_line(&content));
            if le >= n {
                p = le;
                break;
            }
            p = le + 1;
        }
        let mut out: Vec<Inline> = Vec::new();
        for (idx, mut inlines) in lines.into_iter().enumerate() {
            if idx > 0 {
                out.push(Inline::LineBreak);
            }
            out.append(&mut inlines);
        }
        (Block::Para(out), p)
    }

    fn parse_block_tag(&mut self, chars: &[char], pos: usize) -> Option<(Block, usize)> {
        let (name, raw_open, self_closing, after_open) = open_tag(chars, pos)?;
        match name.as_str() {
            "blockquote" => {
                if self_closing {
                    return Some((Block::BlockQuote(Vec::new()), after_open));
                }
                let (inner, after) = enclosed(chars, after_open, "blockquote");
                let inner_chars: Vec<char> = inner.chars().collect();
                Some((Block::BlockQuote(self.parse_blocks(&inner_chars)), after))
            }
            "pre" => {
                let (inner, after) = enclosed(chars, after_open, "pre");
                Some((Block::CodeBlock(Attr::default(), trim_code(&inner)), after))
            }
            "source" | "syntaxhighlight" => {
                let (inner, after) = enclosed(chars, after_open, &name);
                let mut classes = Vec::new();
                if let Some(lang) = tag_attribute(&raw_open, "lang")
                    && !lang.is_empty()
                {
                    classes.push(lang);
                }
                let attr = Attr {
                    id: String::new(),
                    classes,
                    attributes: Vec::new(),
                };
                Some((Block::CodeBlock(attr, trim_code(&inner)), after))
            }
            _ => None,
        }
    }

    fn parse_paragraph(&mut self, chars: &[char], pos: usize) -> (Option<Block>, usize) {
        let n = chars.len();
        let mut pieces: Vec<String> = Vec::new();
        let mut cur = pos;
        loop {
            let le = line_end(chars, cur);
            pieces.push(collect_range(chars, cur, le));
            if le >= n {
                cur = le;
                break;
            }
            let next = le + 1;
            if next >= n {
                cur = next;
                break;
            }
            let next_end = line_end(chars, next);
            if is_blank(chars, next, next_end) {
                cur = if next_end < n { next_end + 1 } else { next_end };
                break;
            }
            if line_starts_block(chars, next) {
                cur = next;
                break;
            }
            cur = next;
        }
        let raw = pieces.join("\n");
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return (None, cur);
        }
        (Some(para_or_figure(self.parse_inlines(trimmed))), cur)
    }

    fn parse_inlines(&mut self, text: &str) -> Vec<Inline> {
        let chars: Vec<char> = text.chars().collect();
        let toks = self.lex(&chars, false);
        coalesce(resolve_emphasis(toks))
    }

    /// Parses one preformatted line: markup is honored, but literal text and its exact spacing are
    /// preserved as code spans rather than collapsed.
    fn preformatted_line(&mut self, text: &str) -> Vec<Inline> {
        let chars: Vec<char> = text.chars().collect();
        let toks = self.lex(&chars, true);
        preformat_transform(resolve_emphasis(toks))
    }

    fn lex(&mut self, chars: &[char], preformatted: bool) -> Vec<Tok> {
        let mut toks: Vec<Tok> = Vec::new();
        let mut word = String::new();
        let mut i = 0;
        let n = chars.len();
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
                if let Some((decoded, next)) = read_entity(chars, i) {
                    word.push_str(&decoded);
                    i = next;
                } else {
                    word.push('&');
                    i += 1;
                }
                continue;
            }
            if c == '<' {
                if let Some((inlines, next)) = self.handle_tag(chars, i) {
                    flush_word(&mut word, &mut toks);
                    for inline in inlines {
                        toks.push(Tok::Inline(inline));
                    }
                    i = next;
                    continue;
                }
                word.push('<');
                i += 1;
                continue;
            }
            if c == '{' && at(chars, i + 1) == Some('{') {
                if let Some(after) = balanced_braces(chars, i) {
                    flush_word(&mut word, &mut toks);
                    let raw = collect_range(chars, i, after);
                    toks.push(Tok::Inline(Inline::RawInline(format_mediawiki(), raw)));
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
    fn handle_tag(&mut self, chars: &[char], i: usize) -> Option<(Vec<Inline>, usize)> {
        if at(chars, i) != Some('<') {
            return None;
        }
        match at(chars, i + 1) {
            Some('/') => {
                let gt = find_char(chars, i, '>')?;
                let raw = collect_range(chars, i, gt + 1);
                return Some((vec![raw_html(raw)], gt + 1));
            }
            Some(c) if c.is_ascii_alphabetic() => {}
            _ => return None,
        }
        let (name, raw_open, self_closing, after_open) = open_tag(chars, i)?;
        match name.as_str() {
            "br" => Some((vec![Inline::LineBreak], after_open)),
            "ref" => {
                if self_closing {
                    return Some((vec![Inline::Note(Vec::new())], after_open));
                }
                match close_tag(chars, after_open, "ref") {
                    Some((inner_end, after)) => {
                        let inner = collect_range(chars, after_open, inner_end);
                        let inlines = self.parse_inlines(&inner);
                        Some((vec![Inline::Note(vec![Block::Plain(inlines)])], after))
                    }
                    None => Some((vec![raw_html(raw_open)], after_open)),
                }
            }
            "nowiki" => {
                if self_closing {
                    return Some((Vec::new(), after_open));
                }
                let (inner, after) = enclosed(chars, after_open, "nowiki");
                Some((plain_inlines(&inner), after))
            }
            "math" => {
                if self_closing {
                    return Some((Vec::new(), after_open));
                }
                match close_tag(chars, after_open, "math") {
                    Some((inner_end, after)) => {
                        let inner = collect_range(chars, after_open, inner_end);
                        Some((
                            vec![Inline::Math(MathType::InlineMath, inner.trim().to_string())],
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
            )),
            "var" => Some(verbatim_code(
                chars,
                "var",
                after_open,
                &raw_open,
                self_closing,
                &["variable"],
            )),
            "samp" => Some(verbatim_code(
                chars,
                "samp",
                after_open,
                &raw_open,
                self_closing,
                &["sample"],
            )),
            "sub" => Some(self.wrap(
                chars,
                "sub",
                after_open,
                &raw_open,
                self_closing,
                Inline::Subscript,
            )),
            "sup" => Some(self.wrap(
                chars,
                "sup",
                after_open,
                &raw_open,
                self_closing,
                Inline::Superscript,
            )),
            "del" | "strike" => Some(self.wrap(
                chars,
                &name,
                after_open,
                &raw_open,
                self_closing,
                Inline::Strikeout,
            )),
            "kbd" => Some(self.span(chars, "kbd", after_open, &raw_open, self_closing, "kbd")),
            "mark" => Some(self.span(chars, "mark", after_open, &raw_open, self_closing, "mark")),
            _ => {
                if self_closing {
                    return Some((vec![raw_html(raw_open)], after_open));
                }
                match close_tag(chars, after_open, &name) {
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
        }
    }

    fn wrap(
        &mut self,
        chars: &[char],
        name: &str,
        after_open: usize,
        raw_open: &str,
        self_closing: bool,
        ctor: fn(Vec<Inline>) -> Inline,
    ) -> (Vec<Inline>, usize) {
        if self_closing {
            return (vec![raw_html(raw_open.to_string())], after_open);
        }
        match close_tag(chars, after_open, name) {
            Some((inner_end, after)) => {
                let inner = collect_range(chars, after_open, inner_end);
                (vec![ctor(self.parse_inlines(&inner))], after)
            }
            None => (vec![raw_html(raw_open.to_string())], after_open),
        }
    }

    fn span(
        &mut self,
        chars: &[char],
        name: &str,
        after_open: usize,
        raw_open: &str,
        self_closing: bool,
        class: &str,
    ) -> (Vec<Inline>, usize) {
        if self_closing {
            return (vec![raw_html(raw_open.to_string())], after_open);
        }
        match close_tag(chars, after_open, name) {
            Some((inner_end, after)) => {
                let inner = collect_range(chars, after_open, inner_end);
                let attr = Attr {
                    id: String::new(),
                    classes: vec![class.to_string()],
                    attributes: Vec::new(),
                };
                (vec![Inline::Span(attr, self.parse_inlines(&inner))], after)
            }
            None => (vec![raw_html(raw_open.to_string())], after_open),
        }
    }

    fn external_link(&mut self, chars: &[char], i: usize) -> Option<(Vec<Inline>, usize)> {
        let close = find_char(chars, i + 1, ']')?;
        let inner = collect_range(chars, i + 1, close);
        let (url, label) = match inner.split_once(|c: char| c.is_whitespace()) {
            Some((u, rest)) => (u.to_string(), rest.trim_start().to_string()),
            None => (inner.clone(), String::new()),
        };
        if !is_url(&url) {
            return None;
        }
        let text = if label.is_empty() {
            self.link_counter += 1;
            vec![Inline::Str(self.link_counter.to_string())]
        } else {
            self.parse_inlines(&label)
        };
        Some((
            vec![Inline::Link(
                Attr::default(),
                text,
                Target {
                    url,
                    title: String::new(),
                },
            )],
            close + 1,
        ))
    }

    fn internal_link(&mut self, chars: &[char], i: usize) -> Option<(Vec<Inline>, usize)> {
        let close = find_seq(chars, i + 2, &[']', ']'])?;
        let inner = collect_range(chars, i + 2, close);
        let (target_part, label_part) = match inner.split_once('|') {
            Some((t, l)) => (t.to_string(), Some(l.to_string())),
            None => (inner.clone(), None),
        };
        let target = target_part.trim().to_string();
        if let Some(ns) = namespace_of(&target)
            && matches!(ns.as_str(), "file" | "image")
            && !strip_namespace(&target).is_empty()
        {
            let image = self.image_embed(&target, label_part.as_deref());
            return Some((vec![image], close + 2));
        }
        let mut after = close + 2;
        let mut trail = String::new();
        while let Some(c) = at(chars, after) {
            if c.is_ascii_alphabetic() {
                trail.push(c);
                after += 1;
            } else {
                break;
            }
        }
        let mut label = match &label_part {
            Some(l) => self.parse_inlines(l),
            None => self.parse_inlines(&target),
        };
        let title = to_plain_text(&label);
        if !trail.is_empty() {
            label.push(Inline::Str(trail));
            label = coalesce(label);
        }
        let attr = Attr {
            id: String::new(),
            classes: vec!["wikilink".to_string()],
            attributes: Vec::new(),
        };
        let url = wikilink_url(&target);
        Some((
            vec![Inline::Link(attr, label, Target { url, title })],
            after,
        ))
    }

    /// Builds the image for a `[[File:…|…]]` / `[[Image:…|…]]` embed. The page name (with the
    /// namespace stripped) is the source; the `WxHpx` parameters set width/height; recognized
    /// placement and option keywords are dropped; the last remaining parameter is the caption,
    /// defaulting to the file name. A lone embed in its own paragraph later becomes a figure
    /// (see [`lone_image_figure`]).
    fn image_embed(&mut self, target: &str, params: Option<&str>) -> Inline {
        let url = wikilink_url(strip_namespace(target));
        let mut attributes: Vec<(String, String)> = Vec::new();
        let mut caption: Option<String> = None;
        if let Some(params) = params {
            for part in params.split('|') {
                let option = part.trim();
                if let Some((width, height)) = image_size(option) {
                    attributes.retain(|(key, _)| key != "width" && key != "height");
                    attributes.push(("width".to_string(), width));
                    if let Some(height) = height {
                        attributes.push(("height".to_string(), height));
                    }
                } else if is_image_keyword(option) || option.contains('=') {
                    // A placement, framing, or `key=value` option carries no caption text.
                } else {
                    caption = Some(part.to_string());
                }
            }
        }
        let caption = caption.unwrap_or_else(|| url.clone());
        let alt = self.parse_inlines(&caption);
        let title = to_plain_text(&alt);
        let attr = Attr {
            id: String::new(),
            classes: Vec::new(),
            attributes,
        };
        Inline::Image(attr, alt, Target { url, title })
    }

    fn make_id(&mut self, inlines: &[Inline]) -> String {
        let plain = to_plain_text(inlines);
        if self.extensions.contains(Extension::GfmAutoIdentifiers) {
            let base = slug_gfm(&plain);
            let base = if base.is_empty() {
                "section".to_string()
            } else {
                base
            };
            self.dedup(base, '-')
        } else if self.extensions.contains(Extension::AutoIdentifiers) {
            let base = mediawiki_slug(&plain);
            let base = if base.is_empty() {
                "section".to_string()
            } else {
                base
            };
            self.dedup(base, '_')
        } else {
            String::new()
        }
    }

    fn dedup(&mut self, base: String, sep: char) -> String {
        if !self.seen_ids.contains(&base) {
            self.seen_ids.insert(base.clone());
            return base;
        }
        let mut k = 1usize;
        loop {
            let candidate = format!("{base}{sep}{k}");
            if !self.seen_ids.contains(&candidate) {
                self.seen_ids.insert(candidate.clone());
                return candidate;
            }
            k += 1;
        }
    }
}

// --- comment stripping --------------------------------------------------------------------------

/// Removes wikitext comments. A comment that is the whole line (preceded by a line start and
/// followed by a line end) is dropped together with its trailing newline; one embedded in other
/// text collapses to a single space. Verbatim regions (`pre`, `nowiki`, `math`, `source`,
/// `syntaxhighlight`) are copied unchanged so comment-like text inside them survives. An
/// unterminated `<!--` is left as literal text.
fn strip_comments(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let Some(c) = at(&chars, i) else { break };
        if c == '<' {
            if let Some(after) = verbatim_region_end(&chars, i) {
                out.push_str(&collect_range(&chars, i, after));
                i = after;
                continue;
            }
            if matches_prefix_ci(&chars, i, "<!--") {
                if let Some(dash) = find_seq(&chars, i + 4, &['-', '-', '>']) {
                    let comment_end = dash + 3;
                    let preceded = i == 0 || at(&chars, i - 1) == Some('\n');
                    let followed = comment_end >= n || at(&chars, comment_end) == Some('\n');
                    if preceded && followed {
                        i = if comment_end < n {
                            comment_end + 1
                        } else {
                            comment_end
                        };
                    } else {
                        out.push(' ');
                        i = comment_end;
                    }
                    continue;
                }
                out.push('<');
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// If a verbatim tag opens at `i`, the index just past its closing tag (or end of input).
fn verbatim_region_end(chars: &[char], i: usize) -> Option<usize> {
    let (name, _raw, self_closing, after_open) = open_tag(chars, i)?;
    if !matches!(
        name.as_str(),
        "pre" | "nowiki" | "math" | "source" | "syntaxhighlight"
    ) {
        return None;
    }
    if self_closing {
        return Some(after_open);
    }
    match close_tag(chars, after_open, &name) {
        Some((_, after)) => Some(after),
        None => Some(chars.len()),
    }
}

// --- emphasis resolution ------------------------------------------------------------------------

fn resolve_emphasis(toks: Vec<Tok>) -> Vec<Inline> {
    let mut root: Vec<Inline> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    for tok in toks {
        match tok {
            Tok::Inline(inline) => push_inline(&mut root, &mut stack, inline),
            Tok::Apostrophes(len) => apply_run(&mut root, &mut stack, len),
        }
    }
    while let Some(frame) = stack.pop() {
        push_inline(
            &mut root,
            &mut stack,
            Inline::Str("'".repeat(frame.marker_len)),
        );
        for inline in frame.buffer {
            push_inline(&mut root, &mut stack, inline);
        }
    }
    root
}

fn push_inline(root: &mut Vec<Inline>, stack: &mut [Frame], inline: Inline) {
    match stack.last_mut() {
        Some(frame) => frame.buffer.push(inline),
        None => root.push(inline),
    }
}

fn is_open(stack: &[Frame], strong: bool) -> bool {
    stack.iter().any(|f| f.strong == strong)
}

fn open_kind(stack: &mut Vec<Frame>, strong: bool) {
    stack.push(Frame {
        strong,
        marker_len: if strong { 3 } else { 2 },
        buffer: Vec::new(),
    });
}

fn close_kind(root: &mut Vec<Inline>, stack: &mut Vec<Frame>, strong: bool) {
    let Some(open_at) = stack.iter().rposition(|f| f.strong == strong) else {
        return;
    };
    while stack.len() > open_at {
        if let Some(frame) = stack.pop() {
            let wrapped = if frame.strong {
                Inline::Strong(frame.buffer)
            } else {
                Inline::Emph(frame.buffer)
            };
            push_inline(root, stack, wrapped);
        } else {
            break;
        }
    }
}

fn apply_run(root: &mut Vec<Inline>, stack: &mut Vec<Frame>, len: usize) {
    let (toggle, literal) = decompose_run(len);
    match toggle {
        Toggle::Emph => toggle_one(root, stack, false, literal),
        Toggle::Strong => toggle_one(root, stack, true, literal),
        Toggle::Both => {
            if is_open(stack, false) && is_open(stack, true) {
                close_kind(root, stack, false);
                close_kind(root, stack, true);
            } else {
                open_kind(stack, true);
                open_kind(stack, false);
            }
            if literal > 0 {
                push_inline(root, stack, Inline::Str("'".repeat(literal)));
            }
        }
    }
}

fn toggle_one(root: &mut Vec<Inline>, stack: &mut Vec<Frame>, strong: bool, literal: usize) {
    if is_open(stack, strong) {
        close_kind(root, stack, strong);
    } else {
        open_kind(stack, strong);
    }
    if literal > 0 {
        push_inline(root, stack, Inline::Str("'".repeat(literal)));
    }
}

fn decompose_run(len: usize) -> (Toggle, usize) {
    match len {
        2 => (Toggle::Emph, 0),
        3 => (Toggle::Strong, 0),
        4 => (Toggle::Strong, 1),
        5 => (Toggle::Both, 0),
        _ => (Toggle::Both, len.saturating_sub(5)),
    }
}

/// Merges adjacent string runs so a span never holds two consecutive [`Inline::Str`] nodes,
/// descending into the markup wrappers a reader produces.
fn coalesce(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for inline in inlines {
        let inline = match inline {
            Inline::Emph(xs) => Inline::Emph(coalesce(xs)),
            Inline::Strong(xs) => Inline::Strong(coalesce(xs)),
            Inline::Strikeout(xs) => Inline::Strikeout(coalesce(xs)),
            Inline::Superscript(xs) => Inline::Superscript(coalesce(xs)),
            Inline::Subscript(xs) => Inline::Subscript(coalesce(xs)),
            Inline::Underline(xs) => Inline::Underline(coalesce(xs)),
            Inline::SmallCaps(xs) => Inline::SmallCaps(coalesce(xs)),
            Inline::Span(attr, xs) => Inline::Span(attr, coalesce(xs)),
            other => other,
        };
        if let (Some(Inline::Str(prev)), Inline::Str(next)) = (out.last_mut(), &inline) {
            prev.push_str(next);
        } else {
            out.push(inline);
        }
    }
    out
}

// --- preformatted text --------------------------------------------------------------------------

/// Turns a parsed preformatted line into code spans: runs of literal text and spaces become
/// [`Inline::Code`] while markup wrappers keep their structure with code interiors. A space inside a
/// code run is held as a non-breaking space so the rendered width is preserved.
fn preformat_transform(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut run = String::new();
    for inline in inlines {
        match inline {
            Inline::Str(s) => run.push_str(&s.replace(' ', "\u{a0}")),
            Inline::Space | Inline::SoftBreak => run.push('\u{a0}'),
            other => {
                if !run.is_empty() {
                    out.push(Inline::Code(Attr::default(), std::mem::take(&mut run)));
                }
                out.push(preformat_descend(other));
            }
        }
    }
    if !run.is_empty() {
        out.push(Inline::Code(Attr::default(), run));
    }
    out
}

/// Recurses preformatting into a wrapper inline, leaving leaf inlines (code, math, breaks, raw)
/// untouched.
fn preformat_descend(inline: Inline) -> Inline {
    match inline {
        Inline::Emph(xs) => Inline::Emph(preformat_transform(xs)),
        Inline::Strong(xs) => Inline::Strong(preformat_transform(xs)),
        Inline::Strikeout(xs) => Inline::Strikeout(preformat_transform(xs)),
        Inline::Superscript(xs) => Inline::Superscript(preformat_transform(xs)),
        Inline::Subscript(xs) => Inline::Subscript(preformat_transform(xs)),
        Inline::Underline(xs) => Inline::Underline(preformat_transform(xs)),
        Inline::SmallCaps(xs) => Inline::SmallCaps(preformat_transform(xs)),
        Inline::Span(attr, xs) => Inline::Span(attr, preformat_transform(xs)),
        Inline::Link(attr, xs, target) => Inline::Link(attr, preformat_transform(xs), target),
        other => other,
    }
}

// --- plain text & entities ----------------------------------------------------------------------

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
                out.push(Inline::Str(std::mem::take(&mut word)));
            }
            let (token, next) = whitespace_token(&chars, i);
            out.push(token);
            i = next;
        } else if c == '&' {
            if let Some((decoded, next)) = read_entity(&chars, i) {
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
        out.push(Inline::Str(word));
    }
    out
}

/// Decodes every entity reference in a string, leaving other characters untouched.
fn decode_entities(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        if at(&chars, i) == Some('&')
            && let Some((decoded, next)) = read_entity(&chars, i)
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

/// Reads one entity reference starting at the `&` in `chars[i]`, returning its decoded text and the
/// index just past the closing `;`. Named, decimal, and hexadecimal forms are recognized.
fn read_entity(chars: &[char], i: usize) -> Option<(String, usize)> {
    let mut j = i + 1;
    if at(chars, j) == Some('#') {
        j += 1;
        let hex = matches!(at(chars, j), Some('x' | 'X'));
        if hex {
            j += 1;
        }
        let start = j;
        while let Some(c) = at(chars, j) {
            let digit = if hex {
                c.is_ascii_hexdigit()
            } else {
                c.is_ascii_digit()
            };
            if digit {
                j += 1;
            } else {
                break;
            }
        }
        if j == start || at(chars, j) != Some(';') {
            return None;
        }
        let digits = collect_range(chars, start, j);
        let code = u32::from_str_radix(&digits, if hex { 16 } else { 10 }).ok()?;
        Some((entities::code_point(code).to_string(), j + 1))
    } else {
        let start = j;
        while let Some(c) = at(chars, j) {
            if c.is_ascii_alphanumeric() {
                j += 1;
            } else {
                break;
            }
        }
        if j == start || at(chars, j) != Some(';') {
            return None;
        }
        let name = collect_range(chars, start, j);
        let decoded = entities::lookup_named(&name)?;
        Some((decoded.to_string(), j + 1))
    }
}

// --- bare URLs & namespaces ---------------------------------------------------------------------

const URL_SCHEMES: &[&str] = &[
    "https://",
    "http://",
    "ftps://",
    "ftp://",
    "ircs://",
    "irc://",
    "gopher://",
    "telnet://",
    "nntp://",
    "mailto:",
    "news:",
    "tel:",
];

fn is_url(text: &str) -> bool {
    let lower = text.to_lowercase();
    URL_SCHEMES.iter().any(|scheme| lower.starts_with(scheme))
}

fn url_scheme_len(chars: &[char], i: usize) -> Option<usize> {
    URL_SCHEMES
        .iter()
        .find(|scheme| matches_prefix_ci(chars, i, scheme))
        .map(|scheme| scheme.chars().count())
}

/// Reads a bare URL beginning at a word boundary, trimming trailing sentence punctuation and an
/// unmatched closing parenthesis. Returns the autolink and the index just past the consumed URL.
fn bare_url(chars: &[char], i: usize) -> Option<(Inline, usize)> {
    let scheme_len = url_scheme_len(chars, i)?;
    let mut j = i + scheme_len;
    while let Some(c) = at(chars, j) {
        if c.is_whitespace() || matches!(c, '<' | '>' | '[' | ']' | '{' | '}' | '|' | '"') {
            break;
        }
        j += 1;
    }
    if j <= i + scheme_len {
        return None;
    }
    let mut url = collect_range(chars, i, j);
    while let Some(last) = url.chars().last() {
        let trailing_punctuation = matches!(last, '.' | ',' | ';' | ':' | '!' | '?');
        let unmatched_paren = last == ')' && !url.contains('(');
        if trailing_punctuation || unmatched_paren {
            url.pop();
        } else {
            break;
        }
    }
    if url.is_empty() {
        return None;
    }
    let consumed = url.chars().count();
    Some((
        Inline::Link(
            Attr::default(),
            vec![Inline::Str(url.clone())],
            Target {
                url,
                title: String::new(),
            },
        ),
        i + consumed,
    ))
}

/// Builds a wikilink target URL from a page name: each run of whitespace collapses to a single
/// underscore, every other character is kept as written.
fn wikilink_url(target: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for ch in target.chars() {
        if ch.is_whitespace() {
            pending = true;
        } else {
            if pending {
                out.push('_');
                pending = false;
            }
            out.push(ch);
        }
    }
    out
}

fn namespace_of(target: &str) -> Option<String> {
    if target.starts_with(':') {
        return None;
    }
    let (before, _) = target.split_once(':')?;
    Some(before.trim().to_lowercase())
}

// --- image embeds -------------------------------------------------------------------------------

/// The page name with a leading `namespace:` prefix removed.
fn strip_namespace(target: &str) -> &str {
    match target.split_once(':') {
        Some((_, rest)) => rest.trim(),
        None => target,
    }
}

/// Parses an image size parameter — `<w>px`, `x<h>px`, or `<w>x<h>px` — into its width and optional
/// height. The width is the digits before an `x` (empty when the form is `x<h>px`); the height is
/// the digits after it. Returns `None` for any parameter that is not a pixel size.
fn image_size(param: &str) -> Option<(String, Option<String>)> {
    let digits = param.strip_suffix("px")?;
    match digits.split_once('x') {
        Some((width, height)) => {
            let valid = width.chars().all(|c| c.is_ascii_digit())
                && !height.is_empty()
                && height.chars().all(|c| c.is_ascii_digit());
            valid.then(|| (width.to_string(), Some(height.to_string())))
        }
        None => (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
            .then(|| (digits.to_string(), None)),
    }
}

/// Whether an image parameter is a recognized placement, framing, or alignment keyword that
/// carries no caption text.
fn is_image_keyword(param: &str) -> bool {
    matches!(
        param.to_ascii_lowercase().as_str(),
        "thumb"
            | "thumbnail"
            | "frame"
            | "framed"
            | "frameless"
            | "border"
            | "left"
            | "right"
            | "center"
            | "centre"
            | "none"
            | "upright"
            | "baseline"
            | "sub"
            | "super"
            | "top"
            | "text-top"
            | "middle"
            | "bottom"
            | "text-bottom"
    )
}

/// Wraps a paragraph whose only content is an image in a figure, moving the image's description to
/// the figure caption; any other paragraph is returned unchanged.
fn para_or_figure(inlines: Vec<Inline>) -> Block {
    match lone_image_figure(&inlines) {
        Some(figure) => figure,
        None => Block::Para(inlines),
    }
}

/// As [`para_or_figure`], for a context (a list item) whose tight content is a [`Block::Plain`].
fn plain_or_figure(inlines: Vec<Inline>) -> Block {
    match lone_image_figure(&inlines) {
        Some(figure) => figure,
        None => Block::Plain(inlines),
    }
}

/// Builds a figure from a paragraph that holds a single image (ignoring surrounding whitespace),
/// or `None` when the paragraph is anything else.
fn lone_image_figure(inlines: &[Inline]) -> Option<Block> {
    let mut significant = inlines.iter().filter(|inline| {
        !matches!(
            inline,
            Inline::Space | Inline::SoftBreak | Inline::LineBreak
        )
    });
    let Inline::Image(attr, alt, target) = significant.next()? else {
        return None;
    };
    if significant.next().is_some() {
        return None;
    }
    let caption = Caption {
        short: None,
        long: vec![Block::Plain(alt.clone())],
    };
    let image = Inline::Image(attr.clone(), Vec::new(), target.clone());
    Some(Block::Figure(
        Attr::default(),
        caption,
        vec![Block::Plain(vec![image])],
    ))
}

// --- identifiers --------------------------------------------------------------------------------

/// Builds a heading identifier under the `auto_identifiers` scheme: lowercase, keep alphanumerics
/// with `_` and `.`, collapse whitespace and `-` runs to a single `_`, drop other punctuation, and
/// strip a leading run of non-letters.
fn mediawiki_slug(text: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for ch in text.chars() {
        if ch.is_whitespace() || ch == '-' {
            pending = true;
        } else if ch.is_alphanumeric() || ch == '_' || ch == '.' {
            if pending && !out.is_empty() {
                out.push('_');
            }
            pending = false;
            out.extend(ch.to_lowercase());
        }
    }
    out.chars().skip_while(|c| !c.is_alphabetic()).collect()
}

// --- tag scanning -------------------------------------------------------------------------------

/// Reads an opening tag at `chars[i]`, returning its lowercased name, the raw `<…>` text, whether it
/// is self-closing, and the index just past the `>`. Attribute values in quotes may contain `>`.
fn open_tag(chars: &[char], start: usize) -> Option<(String, String, bool, usize)> {
    let mut cursor = start + 1;
    let mut name = String::new();
    while let Some(ch) = at(chars, cursor) {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
            cursor += 1;
        } else {
            break;
        }
    }
    if name.is_empty() {
        return None;
    }
    let mut quote: Option<char> = None;
    let len = chars.len();
    while cursor < len {
        let Some(ch) = at(chars, cursor) else { break };
        match quote {
            Some(open_quote) => {
                if ch == open_quote {
                    quote = None;
                }
                cursor += 1;
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                    cursor += 1;
                } else if ch == '>' {
                    break;
                } else {
                    cursor += 1;
                }
            }
        }
    }
    if at(chars, cursor) != Some('>') {
        return None;
    }
    let self_closing = cursor > 0 && at(chars, cursor - 1) == Some('/');
    let raw = collect_range(chars, start, cursor + 1);
    Some((name, raw, self_closing, cursor + 1))
}

/// Finds the matching `</name>` for an element whose content begins at `start`, counting nested
/// same-named tags. Returns the index where the closing tag begins and the index just past its `>`.
fn close_tag(chars: &[char], start: usize, name: &str) -> Option<(usize, usize)> {
    let mut depth = 0i32;
    let mut j = start;
    let n = chars.len();
    while j < n {
        if at(chars, j) == Some('<') {
            if at(chars, j + 1) == Some('/') {
                if tag_name_matches(chars, j + 2, name) {
                    if depth == 0 {
                        let gt = find_char(chars, j, '>')?;
                        return Some((j, gt + 1));
                    }
                    depth -= 1;
                }
            } else if tag_name_matches(chars, j + 1, name) {
                depth += 1;
            }
        }
        j += 1;
    }
    None
}

/// The content of an element starting at `start` together with the index just past its closing tag;
/// an unterminated element runs to the end of input.
fn enclosed(chars: &[char], start: usize, name: &str) -> (String, usize) {
    match close_tag(chars, start, name) {
        Some((inner_end, after)) => (collect_range(chars, start, inner_end), after),
        None => (collect_range(chars, start, chars.len()), chars.len()),
    }
}

fn tag_name_matches(chars: &[char], pos: usize, name: &str) -> bool {
    let mut count = 0;
    for (k, nc) in name.chars().enumerate() {
        match at(chars, pos + k) {
            Some(c) if c.eq_ignore_ascii_case(&nc) => count += 1,
            _ => return false,
        }
    }
    match at(chars, pos + count) {
        Some(c) => c.is_whitespace() || c == '>' || c == '/',
        None => false,
    }
}

fn starts_block_tag(chars: &[char], pos: usize) -> bool {
    if at(chars, pos) != Some('<') {
        return false;
    }
    ["pre", "source", "syntaxhighlight", "blockquote"]
        .iter()
        .any(|name| tag_name_matches(chars, pos + 1, name))
}

/// Reads the value of `key` from a raw tag string, accepting quoted or bare values.
fn tag_attribute(raw: &str, key: &str) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        match at(&chars, i) {
            Some(c) if c.is_ascii_alphabetic() => {
                let start = i;
                while let Some(c) = at(&chars, i) {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let name = collect_range(&chars, start, i).to_lowercase();
                while at(&chars, i).is_some_and(char::is_whitespace) {
                    i += 1;
                }
                if at(&chars, i) == Some('=') {
                    i += 1;
                    while at(&chars, i).is_some_and(char::is_whitespace) {
                        i += 1;
                    }
                    let value = if let Some(q @ ('"' | '\'')) = at(&chars, i) {
                        i += 1;
                        let vs = i;
                        while at(&chars, i).is_some_and(|c| c != q) {
                            i += 1;
                        }
                        let v = collect_range(&chars, vs, i);
                        i += 1;
                        v
                    } else {
                        let vs = i;
                        while at(&chars, i)
                            .is_some_and(|c| !c.is_whitespace() && c != '>' && c != '/')
                        {
                            i += 1;
                        }
                        collect_range(&chars, vs, i)
                    };
                    if name == key {
                        return Some(value);
                    }
                }
            }
            _ => i += 1,
        }
    }
    None
}

// --- line classification ------------------------------------------------------------------------

fn line_starts_block(chars: &[char], ls: usize) -> bool {
    match at(chars, ls) {
        Some('*' | '#' | ':' | ';' | ' ') => true,
        Some('=') => is_header_line(chars, ls),
        Some('-') => is_hr_line(chars, ls),
        Some('{') => matches!(at(chars, ls + 1), Some('{' | '|')),
        Some('<') => starts_block_tag(chars, ls),
        _ => false,
    }
}

fn is_header_line(chars: &[char], pos: usize) -> bool {
    let le = line_end(chars, pos);
    let mut m = 0;
    while pos + m < le && at(chars, pos + m) == Some('=') {
        m += 1;
    }
    if m == 0 || m > 6 {
        return false;
    }
    header_closer(chars, pos + m, le, m).is_some()
}

/// The index of the first bare `=` run after the heading text, when that run is at least `m` long;
/// otherwise no valid closer. Constructs (templates, links, tags) are skipped so an `=` inside them
/// is not mistaken for the closer.
fn header_closer(chars: &[char], content_start: usize, line_end: usize, m: usize) -> Option<usize> {
    let mut i = content_start;
    while i < line_end {
        if let Some(next) = skip_construct(chars, i)
            && next > i
        {
            i = next.min(line_end);
            continue;
        }
        if at(chars, i) == Some('=') {
            let mut j = i;
            while j < line_end && at(chars, j) == Some('=') {
                j += 1;
            }
            return if j - i >= m { Some(i) } else { None };
        }
        i += 1;
    }
    None
}

fn is_hr_line(chars: &[char], pos: usize) -> bool {
    let le = line_end(chars, pos);
    let mut k = pos;
    while k < le && at(chars, k) == Some('-') {
        k += 1;
    }
    k - pos >= 4 && is_blank(chars, k, le)
}

/// Splits a definition term at the first top-level `:`, skipping constructs so a `:` inside a link
/// or template is not treated as the separator.
fn split_term(content: &str) -> (String, Option<String>) {
    let chars: Vec<char> = content.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if let Some(next) = skip_construct(&chars, i)
            && next > i
        {
            i = next;
            continue;
        }
        if at(&chars, i) == Some(':') {
            let before = collect_range(&chars, 0, i).trim().to_string();
            let after = collect_range(&chars, i + 1, n).trim().to_string();
            return (before, Some(after));
        }
        i += 1;
    }
    (content.trim().to_string(), None)
}

/// If an inline construct opens at `i`, the index just past it: `{{…}}`, `[[…]]`, `[…]`, or `<…>`.
fn skip_construct(chars: &[char], i: usize) -> Option<usize> {
    match at(chars, i) {
        Some('{') if at(chars, i + 1) == Some('{') => balanced_braces(chars, i),
        Some('[') if at(chars, i + 1) == Some('[') => {
            find_seq(chars, i + 2, &[']', ']']).map(|c| c + 2)
        }
        Some('[') => find_char(chars, i + 1, ']').map(|c| c + 1),
        Some('<') => find_char(chars, i, '>').map(|c| c + 1),
        _ => None,
    }
}

/// The index just past the `}}` that balances the `{{` at `i`, accounting for nesting.
fn balanced_braces(chars: &[char], i: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut j = i;
    let n = chars.len();
    while j < n {
        if at(chars, j) == Some('{') && at(chars, j + 1) == Some('{') {
            depth += 1;
            j += 2;
        } else if at(chars, j) == Some('}') && at(chars, j + 1) == Some('}') {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return Some(j);
            }
        } else {
            j += 1;
        }
    }
    None
}

// --- small helpers ------------------------------------------------------------------------------

fn is_list_marker(c: char) -> bool {
    matches!(c, '*' | '#' | ':' | ';')
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

fn list_kind(marker: char) -> ListKind {
    match marker {
        '#' => ListKind::Ordered,
        ';' | ':' => ListKind::Definition,
        _ => ListKind::Bullet,
    }
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
) -> (Vec<Inline>, usize) {
    if self_closing {
        return (vec![raw_html(raw_open.to_string())], after_open);
    }
    match close_tag(chars, after_open, name) {
        Some((inner_end, after)) => {
            let inner = collect_range(chars, after_open, inner_end);
            let attr = Attr {
                id: String::new(),
                classes: classes.iter().map(|s| (*s).to_string()).collect(),
                attributes: Vec::new(),
            };
            (vec![Inline::Code(attr, decode_entities(&inner))], after)
        }
        None => (vec![raw_html(raw_open.to_string())], after_open),
    }
}

fn default_list_attrs() -> ListAttributes {
    ListAttributes {
        start: 1,
        style: ListNumberStyle::DefaultStyle,
        delim: ListNumberDelim::DefaultDelim,
    }
}

fn finish_inline_block(chars: &[char], pos: usize) -> (usize, bool) {
    let le = line_end(chars, pos);
    if is_blank(chars, pos, le) {
        let next = if le < chars.len() { le + 1 } else { le };
        (next, true)
    } else {
        (pos, false)
    }
}

fn trim_code(inner: &str) -> String {
    let stripped = inner
        .strip_prefix("\r\n")
        .or_else(|| inner.strip_prefix('\n'))
        .unwrap_or(inner);
    stripped
        .strip_suffix("\r\n")
        .or_else(|| stripped.strip_suffix('\n'))
        .unwrap_or(stripped)
        .to_string()
}

fn flush_word(word: &mut String, toks: &mut Vec<Tok>) {
    if !word.is_empty() {
        toks.push(Tok::Inline(Inline::Str(std::mem::take(word))));
    }
}

fn raw_html(text: String) -> Inline {
    Inline::RawInline(Format("html".to_string()), text)
}

fn format_mediawiki() -> Format {
    Format("mediawiki".to_string())
}

fn at(chars: &[char], i: usize) -> Option<char> {
    chars.get(i).copied()
}

fn collect_range(chars: &[char], start: usize, end: usize) -> String {
    if end <= start {
        return String::new();
    }
    chars.iter().skip(start).take(end - start).collect()
}

/// Finds the index one past the end of a table block opening with `{|` at `pos`. Opening (`{|`) and
/// closing (`|}`) markers are matched by depth, scanning whole lines, so a nested table does not
/// close the outer one early; an unterminated table runs to the end of input.
fn table_block_end(chars: &[char], pos: usize) -> usize {
    let n = chars.len();
    let mut depth = 0usize;
    let mut line = pos;
    loop {
        let mut content = line;
        while matches!(at(chars, content), Some(' ' | '\t')) {
            content += 1;
        }
        if at(chars, content) == Some('{') && at(chars, content + 1) == Some('|') {
            depth += 1;
        } else if at(chars, content) == Some('|') && at(chars, content + 1) == Some('}') {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return content + 2;
            }
        }
        let le = line_end(chars, line);
        if le >= n {
            return n;
        }
        line = le + 1;
    }
}

fn line_end(chars: &[char], pos: usize) -> usize {
    find_char(chars, pos, '\n').unwrap_or(chars.len())
}

fn is_blank(chars: &[char], start: usize, end: usize) -> bool {
    (start..end).all(|j| at(chars, j).is_none_or(char::is_whitespace))
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&j| at(chars, j) == Some(target))
}

fn find_seq(chars: &[char], from: usize, seq: &[char]) -> Option<usize> {
    let n = chars.len();
    let m = seq.len();
    if m == 0 || n < m {
        return None;
    }
    (from..=n - m).find(|&j| (0..m).all(|k| at(chars, j + k) == seq.get(k).copied()))
}

fn matches_prefix_ci(chars: &[char], i: usize, prefix: &str) -> bool {
    prefix
        .chars()
        .enumerate()
        .all(|(k, pc)| match at(chars, i + k) {
            Some(c) => c.eq_ignore_ascii_case(&pc),
            None => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Vec<Block> {
        let mut options = ReaderOptions::default();
        options.extensions = Extensions::from_list(&[Extension::AutoIdentifiers]);
        MediawikiReader
            .read(input, &options)
            .expect("read should not fail")
            .blocks
    }

    fn parse_gfm(input: &str) -> Vec<Block> {
        let mut options = ReaderOptions::default();
        options.extensions = Extensions::from_list(&[Extension::GfmAutoIdentifiers]);
        MediawikiReader.read(input, &options).expect("read").blocks
    }

    #[test]
    fn table_markup_is_kept_as_a_raw_block() {
        assert_eq!(
            parse("{|\n! Header\n|-\n| Cell\n|}\nafter"),
            vec![
                Block::RawBlock(format_mediawiki(), "{|\n! Header\n|-\n| Cell\n|}".into()),
                Block::Para(vec![Inline::Str("after".into())]),
            ]
        );
    }

    #[test]
    fn unterminated_table_markup_does_not_panic() {
        assert_eq!(
            parse("{|"),
            vec![Block::RawBlock(format_mediawiki(), "{|".into())]
        );
    }

    #[test]
    fn nested_table_markup_closes_at_the_outer_marker() {
        let source = "{|\n|\n{|\n| inner\n|}\n|}";
        assert_eq!(
            parse(source),
            vec![Block::RawBlock(format_mediawiki(), source.into())]
        );
    }

    #[test]
    fn paragraph_joins_lines_with_soft_breaks() {
        assert_eq!(
            parse("one two\nthree"),
            vec![Block::Para(vec![
                Inline::Str("one".into()),
                Inline::Space,
                Inline::Str("two".into()),
                Inline::SoftBreak,
                Inline::Str("three".into()),
            ])]
        );
    }

    #[test]
    fn emphasis_runs_decompose() {
        assert_eq!(
            parse("''i'' '''b''' '''''both'''''"),
            vec![Block::Para(vec![
                Inline::Emph(vec![Inline::Str("i".into())]),
                Inline::Space,
                Inline::Strong(vec![Inline::Str("b".into())]),
                Inline::Space,
                Inline::Strong(vec![Inline::Emph(vec![Inline::Str("both".into())])]),
            ])]
        );
    }

    #[test]
    fn header_carries_mediawiki_identifier() {
        assert_eq!(
            parse("== Hello World =="),
            vec![Block::Header(
                2,
                Attr {
                    id: "hello_world".into(),
                    classes: vec![],
                    attributes: vec![],
                },
                vec![
                    Inline::Str("Hello".into()),
                    Inline::Space,
                    Inline::Str("World".into()),
                ],
            )]
        );
    }

    #[test]
    fn duplicate_identifiers_are_suffixed() {
        let blocks = parse("== Dup ==\n== Dup ==");
        let ids: Vec<String> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Header(_, attr, _) => Some(attr.id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["dup".to_string(), "dup_1".to_string()]);
    }

    #[test]
    fn gfm_identifier_scheme_uses_hyphens() {
        let blocks = parse_gfm("== Hello World ==");
        match blocks.first() {
            Some(Block::Header(_, attr, _)) => assert_eq!(attr.id, "hello-world"),
            other => panic!("expected header, got {other:?}"),
        }
    }

    #[test]
    fn empty_identifier_falls_back_to_section() {
        let blocks = parse("== !!! ==\n== ??? ==");
        let ids: Vec<String> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Header(_, attr, _) => Some(attr.id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["section".to_string(), "section_1".to_string()]);
    }

    #[test]
    fn malformed_header_is_a_paragraph() {
        assert_eq!(
            parse("== a=b =="),
            vec![Block::Para(vec![
                Inline::Str("==".into()),
                Inline::Space,
                Inline::Str("a=b".into()),
                Inline::Space,
                Inline::Str("==".into()),
            ])]
        );
    }

    #[test]
    fn header_leftover_becomes_paragraph() {
        assert_eq!(
            parse("== H ==="),
            vec![
                Block::Header(
                    2,
                    Attr {
                        id: "h".into(),
                        classes: vec![],
                        attributes: vec![],
                    },
                    vec![Inline::Str("H".into())],
                ),
                Block::Para(vec![Inline::Str("=".into())]),
            ]
        );
    }

    #[test]
    fn nested_bullets_and_ordered() {
        assert_eq!(
            parse("* a\n** b\n*# c"),
            vec![Block::BulletList(vec![vec![
                Block::Plain(vec![Inline::Str("a".into())]),
                Block::BulletList(vec![vec![Block::Plain(vec![Inline::Str("b".into())])]]),
                Block::OrderedList(
                    default_list_attrs(),
                    vec![vec![Block::Plain(vec![Inline::Str("c".into())])]]
                ),
            ]])]
        );
    }

    #[test]
    fn definition_list_splits_inline_definition() {
        assert_eq!(
            parse("; term : def"),
            vec![Block::DefinitionList(vec![(
                vec![Inline::Str("term".into())],
                vec![vec![Block::Plain(vec![Inline::Str("def".into())])]],
            )])]
        );
    }

    #[test]
    fn internal_link_with_trail() {
        assert_eq!(
            parse("[[Page]]s"),
            vec![Block::Para(vec![Inline::Link(
                Attr {
                    id: String::new(),
                    classes: vec!["wikilink".into()],
                    attributes: vec![],
                },
                vec![Inline::Str("Pages".into())],
                Target {
                    url: "Page".into(),
                    title: "Page".into(),
                },
            )])]
        );
    }

    #[test]
    fn lone_file_embed_becomes_a_figure() {
        assert_eq!(
            parse("[[File:Foo.jpg|thumb|A caption]]"),
            vec![Block::Figure(
                Attr::default(),
                Caption {
                    short: None,
                    long: vec![Block::Plain(vec![
                        Inline::Str("A".into()),
                        Inline::Space,
                        Inline::Str("caption".into()),
                    ])],
                },
                vec![Block::Plain(vec![Inline::Image(
                    Attr::default(),
                    vec![],
                    Target {
                        url: "Foo.jpg".into(),
                        title: "A caption".into(),
                    },
                )])],
            )]
        );
    }

    #[test]
    fn embed_without_caption_defaults_to_the_file_name() {
        assert_eq!(
            parse("[[Image:My Photo.jpg]]"),
            vec![Block::Figure(
                Attr::default(),
                Caption {
                    short: None,
                    long: vec![Block::Plain(vec![Inline::Str("My_Photo.jpg".into())])],
                },
                vec![Block::Plain(vec![Inline::Image(
                    Attr::default(),
                    vec![],
                    Target {
                        url: "My_Photo.jpg".into(),
                        title: "My_Photo.jpg".into(),
                    },
                )])],
            )]
        );
    }

    #[test]
    fn embed_size_parameters_set_width_and_height() {
        assert_eq!(
            parse("[[File:Foo.jpg|100x200px|cap]]"),
            vec![Block::Figure(
                Attr::default(),
                Caption {
                    short: None,
                    long: vec![Block::Plain(vec![Inline::Str("cap".into())])],
                },
                vec![Block::Plain(vec![Inline::Image(
                    Attr {
                        id: String::new(),
                        classes: vec![],
                        attributes: vec![
                            ("width".into(), "100".into()),
                            ("height".into(), "200".into()),
                        ],
                    },
                    vec![],
                    Target {
                        url: "Foo.jpg".into(),
                        title: "cap".into(),
                    },
                )])],
            )]
        );
    }

    #[test]
    fn inline_embed_stays_an_image_not_a_figure() {
        assert_eq!(
            parse("x [[File:Foo.jpg|cap]]"),
            vec![Block::Para(vec![
                Inline::Str("x".into()),
                Inline::Space,
                Inline::Image(
                    Attr::default(),
                    vec![Inline::Str("cap".into())],
                    Target {
                        url: "Foo.jpg".into(),
                        title: "cap".into(),
                    },
                ),
            ])]
        );
    }

    #[test]
    fn empty_file_embed_is_an_ordinary_wikilink() {
        assert_eq!(
            parse("[[File:]]"),
            vec![Block::Para(vec![Inline::Link(
                Attr {
                    id: String::new(),
                    classes: vec!["wikilink".into()],
                    attributes: vec![],
                },
                vec![Inline::Str("File:".into())],
                Target {
                    url: "File:".into(),
                    title: "File:".into(),
                },
            )])]
        );
    }

    #[test]
    fn external_links_number_and_label() {
        assert_eq!(
            parse("[http://x.com lbl] [http://y.com]"),
            vec![Block::Para(vec![
                Inline::Link(
                    Attr::default(),
                    vec![Inline::Str("lbl".into())],
                    Target {
                        url: "http://x.com".into(),
                        title: String::new(),
                    },
                ),
                Inline::Space,
                Inline::Link(
                    Attr::default(),
                    vec![Inline::Str("1".into())],
                    Target {
                        url: "http://y.com".into(),
                        title: String::new(),
                    },
                ),
            ])]
        );
    }

    #[test]
    fn bare_url_trims_trailing_punctuation() {
        assert_eq!(
            parse("see http://x.com."),
            vec![Block::Para(vec![
                Inline::Str("see".into()),
                Inline::Space,
                Inline::Link(
                    Attr::default(),
                    vec![Inline::Str("http://x.com".into())],
                    Target {
                        url: "http://x.com".into(),
                        title: String::new(),
                    },
                ),
                Inline::Str(".".into()),
            ])]
        );
    }

    #[test]
    fn entities_are_decoded_in_text() {
        assert_eq!(
            parse("AT&amp;T &copy;"),
            vec![Block::Para(vec![
                Inline::Str("AT&T".into()),
                Inline::Space,
                Inline::Str("\u{a9}".into()),
            ])]
        );
    }

    #[test]
    fn nowiki_is_literal_text() {
        assert_eq!(
            parse("<nowiki>'''raw'''</nowiki>"),
            vec![Block::Para(vec![Inline::Str("'''raw'''".into())])]
        );
    }

    #[test]
    fn reference_becomes_a_note() {
        assert_eq!(
            parse("x<ref>note</ref>"),
            vec![Block::Para(vec![
                Inline::Str("x".into()),
                Inline::Note(vec![Block::Plain(vec![Inline::Str("note".into())])]),
            ])]
        );
    }

    #[test]
    fn code_tag_decodes_entities() {
        assert_eq!(
            parse("<code>a &amp; b</code>"),
            vec![Block::Para(vec![Inline::Code(
                Attr::default(),
                "a & b".into()
            )])]
        );
    }

    #[test]
    fn unknown_tag_passes_through_as_raw_html() {
        assert_eq!(
            parse("<b>x</b>"),
            vec![Block::Para(vec![
                raw_html("<b>".into()),
                Inline::Str("x".into()),
                raw_html("</b>".into()),
            ])]
        );
    }

    #[test]
    fn whole_line_comment_is_removed_with_its_newline() {
        assert_eq!(
            parse("x\n<!--c-->\ny"),
            vec![Block::Para(vec![
                Inline::Str("x".into()),
                Inline::SoftBreak,
                Inline::Str("y".into()),
            ])]
        );
    }

    #[test]
    fn inline_comment_becomes_a_space() {
        assert_eq!(
            parse("a<!--c-->b"),
            vec![Block::Para(vec![
                Inline::Str("a".into()),
                Inline::Space,
                Inline::Str("b".into()),
            ])]
        );
    }

    #[test]
    fn syntax_highlight_block_keeps_language_and_content() {
        assert_eq!(
            parse("<syntaxhighlight lang=\"rust\">\nfn main(){}\n</syntaxhighlight>"),
            vec![Block::CodeBlock(
                Attr {
                    id: String::new(),
                    classes: vec!["rust".into()],
                    attributes: vec![],
                },
                "fn main(){}".into(),
            )]
        );
    }

    #[test]
    fn horizontal_rule_requires_a_dashes_only_line() {
        assert_eq!(parse("----"), vec![Block::HorizontalRule]);
        assert_eq!(
            parse("----foo"),
            vec![Block::Para(vec![Inline::Str("----foo".into())])]
        );
    }

    #[test]
    fn preformatted_lines_become_code() {
        assert_eq!(
            parse(" indented  line"),
            vec![Block::Para(vec![Inline::Code(
                Attr::default(),
                "indented\u{a0}\u{a0}line".into()
            )])]
        );
    }

    #[test]
    fn preformatted_preserves_markup_and_spacing() {
        assert_eq!(
            parse(" a '''b''' c"),
            vec![Block::Para(vec![
                Inline::Code(Attr::default(), "a\u{a0}".into()),
                Inline::Strong(vec![Inline::Code(Attr::default(), "b".into())]),
                Inline::Code(Attr::default(), "\u{a0}c".into()),
            ])]
        );
    }

    #[test]
    fn block_template_is_raw_then_trailing_paragraph() {
        assert_eq!(
            parse("{{tpl}} trailing"),
            vec![
                Block::RawBlock(format_mediawiki(), "{{tpl}}".into()),
                Block::Para(vec![Inline::Str("trailing".into())]),
            ]
        );
    }
}
