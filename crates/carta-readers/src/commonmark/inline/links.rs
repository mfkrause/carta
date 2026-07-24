//! Bracket resolution: links, images, spans, footnotes, inline notes, and bracketed citations.

use carta_ast::{Attr, Citation, CitationMode, Inline, Target};
use carta_core::Extension;

use super::super::attr;
use super::super::para;
use super::super::postprocess::is_format_name_char;
use super::super::scan::{
    char_at, escape_uri, normalize_label, scan_following_label, scan_inline_target,
};
use super::helpers::{
    citation_fallback_inlines, def_target, find_citation_key, scan_markdown_inline_target,
    split_citation_segments,
};
use super::{Delimiter, InlineParser, MAX_LABEL_BYTES, Node, parse_inlines, resolve_inline_nodes};

/// Outcome of resolving an explicit link target after a closing `]`.
enum Explicit {
    /// An inline or reference target resolved to this destination, ending at the given position.
    Target(Target, usize),
    /// An explicit reference was present but its label is undefined: not a link.
    Failed,
    /// No explicit target syntax follows; a span or shortcut reference may still apply.
    None,
}

impl<'a> InlineParser<'a> {
    pub(super) fn push_open_bracket(&mut self, image: bool) {
        let node_index = self.nodes.len();
        self.bracket_stack.push(node_index);
        self.nodes.push(Node::Delimiter(Delimiter {
            ch: b'[',
            count: 1,
            can_open: true,
            can_close: false,
            image,
            text_start: self.pos,
            active: true,
            cite_count_at_open: self.notes.cite_count.get(),
        }));
    }

    pub(super) fn close_bracket(&mut self) {
        self.pos += 1;
        let Some(&opener_index) = self.bracket_stack.last() else {
            self.push_text(']');
            return;
        };
        let (is_image, is_active) = match self.nodes.get(opener_index) {
            Some(Node::Delimiter(d)) => (d.image, d.active),
            _ => (false, false),
        };

        // A defined `[^label]` wins over every other bracket use and consumes nothing past `]`.
        if is_active
            && self.ext.contains(Extension::Footnotes)
            && self.try_footnote(opener_index, is_image)
        {
            return;
        }

        // An inactive `[` cannot form a link (spec 6.3 rule 6) but may still open a span.
        if is_active {
            match self.resolve_explicit(opener_index) {
                Explicit::Target(target, next) => {
                    self.finish_link(opener_index, is_image, target, next);
                    return;
                }
                // Undefined explicit reference: brackets stay literal, no span/shortcut fallback.
                Explicit::Failed => {
                    self.bracket_stack.pop();
                    self.literalize_bracket(opener_index);
                    self.push_text(']');
                    return;
                }
                Explicit::None => {}
            }
        }

        // A non-empty attribute block makes a span, winning over a same-label shortcut reference.
        if !is_image
            && self.ext.contains(Extension::BracketedSpans)
            && let Some((attr, next)) = self.scan_attr_block()
        {
            self.bracket_stack.pop();
            self.pos = next;
            self.build_span(opener_index, attr);
            return;
        }

        // Shortcut reference: skip the lookup with no definitions or a span past the label limit.
        if is_active && !self.refs.is_empty() {
            let raw = self.raw_label(opener_index);
            if raw.len() <= MAX_LABEL_BYTES
                && let Some(target) = self.refs.get(&normalize_label(raw)).map(def_target)
            {
                self.finish_link(opener_index, is_image, target, self.pos);
                return;
            }
        }

        // A well-formed citation list becomes a `Cite`; an image's `!` survives as literal text.
        if self.ext.contains(Extension::Citations)
            && self.try_bracket_citation(opener_index, is_image)
        {
            return;
        }

        // Otherwise the opener reverts to its literal `[` / `![`, and `]` stays literal.
        self.bracket_stack.pop();
        self.literalize_bracket(opener_index);
        self.push_text(']');
    }

    /// If the bracket opener encloses a well-formed citation list `[ ... @key ... ]`, emit a `Cite`
    /// and return `true`. The content is split on top-level semicolons into entries; every entry
    /// must hold one top-level `@key`, and no entry may be empty. Each entry's text before the key
    /// is its prefix and the text after is its suffix (both parsed as inlines, so a nested bare
    /// `@key` there becomes its own citation); a `-` glued to the front of the key suppresses the
    /// author. The whole group shares one citation number, raised to cover any nested citation. The
    /// fallback field is the raw bracket source parsed as ordinary inlines. Returns `false` (leaving
    /// the brackets for literal handling) when the content is not a citation list.
    fn try_bracket_citation(&mut self, opener_index: usize, is_image: bool) -> bool {
        let raw = self.raw_label(opener_index);
        // No `@` means no citation list; skip the segment scan.
        if !raw.as_bytes().contains(&b'@') {
            return false;
        }
        let Some(segments) = split_citation_segments(raw) else {
            return false;
        };
        // Interior bare citations are discarded with their nodes: rewind to the count at bracket
        // open before numbering (for `![@key]` the discarded count stays off, numbering one low).
        if let Some(Node::Delimiter(d)) = self.nodes.get(opener_index) {
            self.notes.cite_count.set(d.cite_count_at_open);
        }
        // Reserve the group's number before parsing affixes so nested citations count after it.
        self.bump_cite_count();
        let mut citations = Vec::with_capacity(segments.len());
        for segment in &segments {
            let Some(entry) = self.parse_citation_entry(raw, segment.clone()) else {
                return false;
            };
            citations.push(entry);
        }
        let group_num = self.notes.cite_count.get();
        for citation in &mut citations {
            citation.note_num = group_num;
        }
        let fallback = citation_fallback_inlines(&format!("[{raw}]"));
        self.nodes.truncate(opener_index);
        self.bracket_stack.retain(|&ni| ni < opener_index);
        if is_image {
            self.push_text('!');
        }
        self.nodes
            .push(Node::Inline(Inline::Cite(citations, fallback)));
        true
    }

    /// Parse one citation entry from `chars[range]`: locate the first top-level `@key`, taking the
    /// text before it as the prefix and the text after as the suffix. A `-` directly before the key
    /// (itself at the segment start or preceded by whitespace) suppresses the author. Returns `None`
    /// when the segment holds no top-level key.
    fn parse_citation_entry(&self, raw: &str, range: std::ops::Range<usize>) -> Option<Citation> {
        let key = find_citation_key(raw, range.clone())?;
        let prefix_end = if key.suppress { key.dash } else { key.at };
        let prefix_src = raw.get(range.start..prefix_end)?;
        let suffix_src = raw.get(key.id_end..range.end)?;
        let mode = if key.suppress {
            CitationMode::SuppressAuthor
        } else {
            CitationMode::NormalCitation
        };
        // Prefix is whitespace-trimmed; suffix keeps its leading space, drops trailing. Locator
        // joins (`p.\u{a0}5`) are not applied: the suffix stays ordinary inlines.
        Some(Citation {
            id: key.id.into(),
            prefix: parse_inlines(prefix_src.trim(), self.refs, self.notes, self.ext),
            suffix: parse_inlines(suffix_src.trim_end(), self.refs, self.notes, self.ext),
            mode,
            note_num: 0,
            hash: 0,
        })
    }

    /// Pop the opener, consume an optional trailing attribute block, and emit the link or image.
    fn finish_link(&mut self, opener_index: usize, is_image: bool, target: Target, next: usize) {
        self.bracket_stack.pop();
        self.pos = next;
        let attr = self.take_link_attr();
        self.build_link(opener_index, is_image, target, attr);
        if !is_image {
            self.deactivate_earlier_brackets(opener_index);
        }
    }

    /// Parse one or more consecutive non-empty attribute blocks at the cursor, merged into a single
    /// [`Attr`], with the position past the last block. An empty block (`{}`) alone is not consumed;
    /// a space between blocks ends the run.
    fn scan_attr_block(&self) -> Option<(Attr, usize)> {
        let (mut merged, mut next) = attr::parse_attributes_bytes(self.text, self.pos)?;
        while let Some((more, after)) = attr::parse_attributes_bytes(self.text, next) {
            attr::merge(&mut merged, more);
            next = after;
        }
        attr::is_non_empty(&merged).then_some((merged, next))
    }

    /// Scan a raw-format marker `{=FORMAT}` at the cursor, returning the format name and the index
    /// past the closing brace. The braces may hold surrounding whitespace (`{ =html }`), but no
    /// space may sit between `=` and the format, and the format may carry nothing but the marker:
    /// any further content (`{=html .foo}`) is not a raw marker. The format token is one or more
    /// ASCII alphanumerics, `-`, or `_`. Active only when `raw_attribute` is enabled.
    pub(super) fn scan_raw_format(&self) -> Option<(String, usize)> {
        if !self.ext.contains(Extension::RawAttribute) {
            return None;
        }
        if char_at(self.text, self.pos) != Some('{') {
            return None;
        }
        let mut index = self.pos + 1;
        while let Some(ch) = char_at(self.text, index) {
            if ch == ' ' || ch == '\t' {
                index += 1;
            } else {
                break;
            }
        }
        if char_at(self.text, index) != Some('=') {
            return None;
        }
        index += 1;
        let format_start = index;
        while let Some(ch) = char_at(self.text, index) {
            if is_format_name_char(ch) {
                index += ch.len_utf8();
            } else {
                break;
            }
        }
        if index == format_start {
            return None;
        }
        let format = self.text.get(format_start..index)?.to_owned();
        while let Some(ch) = char_at(self.text, index) {
            if ch == ' ' || ch == '\t' {
                index += 1;
            } else {
                break;
            }
        }
        if char_at(self.text, index) != Some('}') {
            return None;
        }
        Some((format, index + 1))
    }

    /// Consume an attribute block following an inline code span when the relevant extension is on,
    /// advancing the cursor; otherwise the default attribute.
    pub(super) fn take_code_attr(&mut self) -> Attr {
        if (self.ext.contains(Extension::InlineCodeAttributes)
            || self.ext.contains(Extension::Attributes))
            && let Some((parsed, next)) = self.scan_attr_block()
        {
            self.pos = next;
            return parsed;
        }
        Attr::default()
    }

    /// Consume an attribute block following a link or image when the relevant extension is on,
    /// advancing the cursor; otherwise the default attribute.
    fn take_link_attr(&mut self) -> Attr {
        if (self.ext.contains(Extension::LinkAttributes)
            || self.ext.contains(Extension::Attributes))
            && let Some((parsed, next)) = self.scan_attr_block()
        {
            self.pos = next;
            return parsed;
        }
        Attr::default()
    }

    /// Build a span from a non-image bracket opener and its inner content.
    fn build_span(&mut self, opener_index: usize, attr: Attr) {
        let inner: Vec<Node> = self.nodes.split_off(opener_index + 1);
        self.nodes.pop();
        self.bracket_stack.retain(|&ni| ni < opener_index);
        let content = resolve_inline_nodes(inner, self.ext, self.notes.markdown);
        self.nodes
            .push(Node::Inline(Inline::Span(Box::new(attr), content)));
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

    /// Mark all non-image `[` openers that appear before `before` in the node list as inactive,
    /// preventing them from forming links that would contain the link just built. Inactive openers
    /// remain on the bracket stack so that a later `]` can consume them one at a time (spec §6.3,
    /// rule 6): each `]` pops the top inactive entry, literalizes it, and emits `]` as text.
    fn deactivate_earlier_brackets(&mut self, before: usize) {
        for &ni in &self.bracket_stack {
            if ni >= before {
                continue;
            }
            if let Some(Node::Delimiter(d)) = self.nodes.get_mut(ni)
                && !d.image
            {
                d.active = false;
            }
        }
    }

    /// Resolve an explicit link target following `]`: an inline `(...)` destination or an explicit
    /// `[label]`/`[]` reference. Shortcut references (the bracket's own text) are handled separately
    /// so a bracketed span can take precedence over them.
    fn resolve_explicit(&self, opener_index: usize) -> Explicit {
        if self.at(0) == Some('(') {
            // Markdown dialect allows spaces and balanced parens in an unbracketed destination.
            let scanned = if self.notes.markdown {
                scan_markdown_inline_target(self.text, self.pos)
            } else {
                scan_inline_target(self.text, self.pos)
            };
            if let Some((target, next)) = scanned {
                return Explicit::Target(target, next);
            }
        }
        // Explicit reference: labels match raw source text; `spaced_reference_links` lets
        // whitespace separate the two brackets (not the `(...)` target handled above).
        let mut label_start = self.pos;
        if self.ext.contains(Extension::SpacedReferenceLinks) {
            while matches!(char_at(self.text, label_start), Some(' ' | '\t' | '\n')) {
                label_start += 1;
            }
        }
        if let Some((label, next)) = scan_following_label(self.text, label_start) {
            // With no definitions in scope an explicit reference can never resolve.
            if self.refs.is_empty() {
                return Explicit::Failed;
            }
            let key = if label.is_empty() {
                // A collapsed reference keys on the bracket's own span; past the limit, no label.
                let raw = self.raw_label(opener_index);
                if raw.len() > MAX_LABEL_BYTES {
                    return Explicit::Failed;
                }
                normalize_label(raw)
            } else {
                normalize_label(&label)
            };
            return match self.refs.get(&key).map(def_target) {
                Some(target) => Explicit::Target(target, next),
                None => Explicit::Failed,
            };
        }
        Explicit::None
    }

    /// The raw source between a bracket opener and the closing `]` just consumed.
    fn raw_label(&self, opener_index: usize) -> &'a str {
        let start = match self.nodes.get(opener_index) {
            Some(Node::Delimiter(d)) => d.text_start,
            _ => return "",
        };
        // The closing `]` is ASCII, so its byte offset is `self.pos - 1`.
        self.text
            .get(start..self.pos.saturating_sub(1))
            .unwrap_or_default()
    }

    /// If the bracket opener encloses a defined footnote reference (`[^label]`), emit the note and
    /// return `true`. The opener's raw label must begin with `^` and name a known footnote; the
    /// brackets and their content are then replaced wholesale, and an image opener's `!` survives as
    /// literal text. Inside a footnote definition's own body a reference collapses to an empty string
    /// rather than nesting a note. Returns `false` (leaving the brackets for other resolution) when
    /// the label has no `^` prefix, holds a bracket, or matches no definition.
    fn try_footnote(&mut self, opener_index: usize, is_image: bool) -> bool {
        if self.notes.defined.is_empty() {
            return false;
        }
        let raw = self.raw_label(opener_index);
        let Some(label) = raw.strip_prefix('^') else {
            return false;
        };
        if label.is_empty() || label.contains('[') || label.contains(']') {
            return false;
        }
        let key = normalize_label(label);
        if !self.notes.defined.contains(&key) {
            return false;
        }
        self.nodes.truncate(opener_index);
        self.bracket_stack.retain(|&ni| ni < opener_index);
        if is_image {
            self.push_text('!');
        }
        let note = if self.notes.in_definition {
            Inline::Str(carta_ast::Text::default())
        } else {
            Inline::Note(self.notes.by_id.get(&key).cloned().unwrap_or_default())
        };
        self.nodes.push(Node::Inline(note));
        true
    }

    /// Resolve an inline note `^[...]` at the cursor (which sits on the `^`, with `[` following).
    /// The bracket content runs up to its balanced closing `]`, is parsed as inline markdown, and
    /// becomes a single-paragraph `Note`. Returns `false` without advancing when the bracket has no
    /// balanced closer, leaving the `^` for literal/superscript handling.
    pub(super) fn try_inline_note(&mut self) -> bool {
        // The `[` sits at self.pos + 1 (`^` is ASCII); walk forward tracking bracket depth.
        let mut depth = 0usize;
        let mut index = self.pos + 1;
        let mut end = None;
        while let Some(ch) = char_at(self.text, index) {
            match ch {
                '\\' => index += 1 + char_at(self.text, index + 1).map_or(0, char::len_utf8),
                '[' => {
                    depth += 1;
                    index += 1;
                }
                ']' => {
                    depth -= 1;
                    index += 1;
                    if depth == 0 {
                        end = Some(index);
                        break;
                    }
                }
                _ => index += ch.len_utf8(),
            }
        }
        let Some(end) = end else {
            return false;
        };
        // Content lies between `[` (self.pos + 1) and the closing `]` (ASCII, at end - 1).
        let inner = self
            .text
            .get(self.pos + 2..end.saturating_sub(1))
            .map(str::to_owned)
            .unwrap_or_default();
        let inlines = parse_inlines(&inner, self.refs, self.notes, self.ext);
        self.pos = end;
        self.nodes
            .push(Node::Inline(Inline::Note(vec![para(inlines)])));
        true
    }

    fn build_link(&mut self, opener_index: usize, is_image: bool, mut target: Target, attr: Attr) {
        // Markdown dialect percent-encodes the destination; strict dialects keep it verbatim.
        if self.notes.markdown {
            target.url = escape_uri(&target.url).into();
        }
        let inner: Vec<Node> = self.nodes.split_off(opener_index + 1);
        self.nodes.pop();
        // Stack entries pointing into the split-off range now belong to the inner node list.
        self.bracket_stack.retain(|&ni| ni < opener_index);
        let content = resolve_inline_nodes(inner, self.ext, self.notes.markdown);
        let inline = if is_image {
            Inline::Image(Box::new(attr), content, Box::new(target))
        } else {
            Inline::Link(Box::new(attr), content, Box::new(target))
        };
        self.nodes.push(Node::Inline(inline));
    }
}
