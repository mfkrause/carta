//! Inline rendering and running-text escaping for the markdown engine.

use carta_ast::{Attr, Citation, CitationMode, Format, Inline, MathType, Target, Text};
use carta_core::Extension;

use crate::common::{
    NotesHost, Piece, clean_prefix_len, escape_html_attr, quote_marks, render_html_attr,
    render_html_fragment_attr,
};
use crate::markdown_common::{
    attr_is_empty, autolink, begins_character_reference, begins_named_entity, code_span,
    destination, is_autolink_class, is_html_format, is_word_boundary, longest_backtick_run,
    push_html,
};

use super::{State, attr_body, attr_braces};

impl State {
    pub(super) fn inlines_oneline(&mut self, inlines: &[Inline]) -> String {
        let pieces = self.pieces(inlines);
        let mut out = String::new();
        for piece in &pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Space | Piece::Soft | Piece::Hard => out.push(' '),
            }
        }
        out
    }

    pub(super) fn pieces(&mut self, inlines: &[Inline]) -> Vec<Piece> {
        let mut out = Vec::new();
        self.extend_pieces(inlines, &mut out);
        out
    }

    pub(super) fn extend_pieces(&mut self, inlines: &[Inline], out: &mut Vec<Piece>) {
        for (position, inline) in inlines.iter().enumerate() {
            if let Inline::Str(text) = inline
                && let Some(prefix) = text.strip_suffix('!')
                && matches!(inlines.get(position + 1), Some(Inline::Link(..)))
            {
                out.push(Piece::text(format!("{}\\!", self.escape_str(prefix))));
                continue;
            }
            self.inline(inline, out);
        }
    }

    fn inline(&mut self, inline: &Inline, out: &mut Vec<Piece>) {
        match inline {
            Inline::Str(text) => out.push(Piece::text(self.escape_str(text))),
            Inline::Emph(inlines) => self.wrap_markup("*", inlines, out),
            Inline::Strong(inlines) => self.wrap_markup("**", inlines, out),
            Inline::Strikeout(inlines) => {
                if self.config.has(Extension::Strikeout) {
                    self.wrap_markup("~~", inlines, out);
                } else {
                    self.wrap_tag("s", inlines, out);
                }
            }
            Inline::Underline(inlines) => {
                if self.config.span_syntax() {
                    self.wrap_span(&underline_attr(), inlines, out);
                } else {
                    self.wrap_tag("u", inlines, out);
                }
            }
            Inline::Superscript(inlines) => {
                if self.config.has(Extension::Superscript) {
                    self.wrap_markup("^", inlines, out);
                } else {
                    self.wrap_tag("sup", inlines, out);
                }
            }
            Inline::Subscript(inlines) => {
                if self.config.has(Extension::Subscript) {
                    self.wrap_markup("~", inlines, out);
                } else {
                    self.wrap_tag("sub", inlines, out);
                }
            }
            Inline::SmallCaps(inlines) => {
                if self.config.span_syntax() {
                    self.wrap_span(&smallcaps_attr(), inlines, out);
                } else {
                    out.push(Piece::text("<span class=\"smallcaps\">"));
                    self.extend_pieces(inlines, out);
                    out.push(Piece::text("</span>"));
                }
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = if self.config.has(Extension::Smart) {
                    ascii_quote_marks(kind)
                } else {
                    quote_marks(kind)
                };
                out.push(Piece::text(open.to_string()));
                self.extend_pieces(inlines, out);
                out.push(Piece::text(close.to_string()));
            }
            Inline::Cite(citations, inlines) => self.cite(citations, inlines, out),
            Inline::Code(_, text) => out.push(Piece::text(code_span(text))),
            Inline::Space => out.push(Piece::Space),
            Inline::SoftBreak => out.push(Piece::Soft),
            Inline::LineBreak => {
                out.push(Piece::text(self.config.hard_break()));
                out.push(Piece::Hard);
            }
            Inline::Math(kind, text) => self.math(kind, text, out),
            Inline::RawInline(format, text) => self.raw_inline(format, text, out),
            Inline::Link(attr, inlines, target) => self.link(attr, inlines, target, out),
            Inline::Image(attr, inlines, target) => self.image(attr, inlines, target, out),
            Inline::Span(attr, inlines) => {
                if attr_is_empty(attr) {
                    self.extend_pieces(inlines, out);
                } else if self.config.span_syntax() {
                    self.wrap_span(attr, inlines, out);
                } else {
                    out.push(Piece::text(format!("<span{}>", render_html_attr(attr))));
                    self.extend_pieces(inlines, out);
                    out.push(Piece::text("</span>"));
                }
            }
            Inline::Note(blocks) => {
                let marker = self.record_note(blocks);
                out.push(Piece::text(marker));
            }
        }
    }

    /// Render a math node. The GitHub math surface writes an inline `` $`…`$ `` span and a fenced
    /// ```` ```math ```` display block; the dollar surface writes `$…$`/`$$…$$`; the single- and
    /// double-backslash surfaces write `\(…\)`/`\[…\]` and `\\(…\\)`/`\\[…\\]`. With no math syntax
    /// at all the expression linearizes to inline markup, and a display expression then occupies its
    /// own source line, set off from the surrounding text by line breaks.
    fn math(&mut self, kind: &MathType, text: &str, out: &mut Vec<Piece>) {
        if self.config.has(Extension::TexMathGfm) {
            let rendered = match kind {
                MathType::InlineMath => format!("$`{text}`$"),
                MathType::DisplayMath => format!("``` math\n{text}\n```"),
            };
            out.push(Piece::text(rendered));
            return;
        }
        if self.config.has(Extension::TexMathDollars) {
            let rendered = match kind {
                MathType::InlineMath => format!("${text}$"),
                MathType::DisplayMath => format!("$${text}$$"),
            };
            out.push(Piece::text(rendered));
            return;
        }
        if self.config.has(Extension::TexMathSingleBackslash) {
            let rendered = match kind {
                MathType::InlineMath => format!("\\({text}\\)"),
                MathType::DisplayMath => format!("\\[{text}\\]"),
            };
            out.push(Piece::text(rendered));
            return;
        }
        if self.config.has(Extension::TexMathDoubleBackslash) {
            let rendered = match kind {
                MathType::InlineMath => format!("\\\\({text}\\\\)"),
                MathType::DisplayMath => format!("\\\\[{text}\\\\]"),
            };
            out.push(Piece::text(rendered));
            return;
        }
        if matches!(kind, MathType::DisplayMath) {
            let mut inner = Vec::new();
            self.math_fallback(kind, text, &mut inner);
            if !inner.is_empty() {
                out.push(Piece::Hard);
                out.append(&mut inner);
                out.push(Piece::Hard);
            }
            return;
        }
        self.math_fallback(kind, text, out);
    }

    /// Render a math node when the dialect has no math syntax: the expression linearized to inline
    /// markup when it converts, nothing when it is empty, otherwise the verbatim source wrapped in
    /// the kind's `$`/`$$` delimiters and routed through the running-text path so its literal text is
    /// escaped. Inline source has its edge whitespace trimmed before wrapping; display source is
    /// wrapped as written.
    fn math_fallback(&mut self, kind: &MathType, tex: &str, out: &mut Vec<Piece>) {
        match crate::math::to_inlines(tex) {
            Some(inlines) => {
                for converted in &inlines {
                    self.inline(converted, out);
                }
            }
            None if tex.trim().is_empty() => {}
            None => {
                let (delim, body) = match kind {
                    MathType::DisplayMath => ("$$", tex),
                    MathType::InlineMath => ("$", tex.trim()),
                };
                let fallback = Inline::Str(format!("{delim}{body}{delim}").into());
                self.inline(&fallback, out);
            }
        }
    }

    fn raw_inline(&mut self, format: &Format, text: &str, out: &mut Vec<Piece>) {
        if !self.config.has(Extension::RawAttribute) {
            if is_html_format(format) {
                out.push(Piece::text(text.to_owned()));
            }
            return;
        }
        let fence = "`".repeat((longest_backtick_run(text) + 1).max(1));
        out.push(Piece::text(format!(
            "{fence}{text}{fence}{{={}}}",
            format.0
        )));
    }

    /// Render a citation. With the citation extension this reconstructs citation syntax; without it
    /// there is no such syntax, so the citation's display inlines render instead.
    fn cite(&mut self, citations: &[Citation], inlines: &[Inline], out: &mut Vec<Piece>) {
        if !self.config.has(Extension::Citations) {
            self.extend_pieces(inlines, out);
            return;
        }
        let text = self.render_citations(citations);
        out.push(Piece::text(text));
    }

    fn render_citations(&mut self, citations: &[Citation]) -> String {
        if let [single] = citations
            && single.mode == CitationMode::AuthorInText
        {
            let prefix = self.affix(&single.prefix);
            let suffix = self.affix(&single.suffix);
            let mut out = String::new();
            if !prefix.is_empty() {
                out.push_str(&prefix);
                out.push(' ');
            }
            out.push('@');
            out.push_str(&single.id);
            if !suffix.is_empty() {
                out.push(' ');
                out.push_str(&suffix);
            }
            return out;
        }
        let parts: Vec<String> = citations
            .iter()
            .map(|citation| self.citation_in_brackets(citation))
            .collect();
        format!("[{}]", parts.join("; "))
    }

    fn citation_in_brackets(&mut self, citation: &Citation) -> String {
        let prefix = self.affix(&citation.prefix);
        let suffix = self.affix(&citation.suffix);
        let mut out = String::new();
        if !prefix.is_empty() {
            out.push_str(&prefix);
            out.push(' ');
        }
        if citation.mode == CitationMode::SuppressAuthor {
            out.push('-');
        }
        out.push('@');
        out.push_str(&citation.id);
        if !suffix.is_empty() {
            out.push(' ');
            out.push_str(&suffix);
        }
        out
    }

    fn affix(&mut self, inlines: &[Inline]) -> String {
        self.inlines_oneline(inlines)
    }

    fn wrap_markup(&mut self, marker: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
        out.push(Piece::text(marker.to_owned()));
        self.extend_pieces(inlines, out);
        out.push(Piece::text(marker.to_owned()));
    }

    fn wrap_tag(&mut self, tag: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
        out.push(Piece::text(format!("<{tag}>")));
        self.extend_pieces(inlines, out);
        out.push(Piece::text(format!("</{tag}>")));
    }

    fn wrap_span(&mut self, attr: &Attr, inlines: &[Inline], out: &mut Vec<Piece>) {
        out.push(Piece::text("["));
        self.extend_pieces(inlines, out);
        out.push(Piece::text(format!("]{{{}}}", attr_body(attr))));
    }

    fn link(&mut self, attr: &Attr, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
        if self.in_anchor {
            push_html(
                out,
                &format!("<span{}>", render_html_fragment_attr(attr)),
                true,
            );
            self.extend_pieces(inlines, out);
            out.push(Piece::text("</span>"));
            return;
        }
        if (attr_is_empty(attr) || is_autolink_class(attr))
            && let Some(autolink) = autolink(inlines, target)
        {
            out.push(Piece::text(autolink));
            return;
        }
        if !self.config.has(Extension::LinkAttributes) && !attr_is_empty(attr) {
            push_html(
                out,
                &format!(
                    "<a href=\"{}\"{}{}>",
                    escape_html_attr(&target.url),
                    render_html_fragment_attr(attr),
                    title_attr(&target.title)
                ),
                true,
            );
            self.in_anchor = true;
            self.extend_pieces(inlines, out);
            self.in_anchor = false;
            out.push(Piece::text("</a>"));
            return;
        }
        out.push(Piece::text("["));
        self.extend_pieces(inlines, out);
        let attr_suffix = if attr_is_empty(attr) {
            String::new()
        } else {
            attr_braces(attr)
        };
        out.push(Piece::text(format!(
            "]({}){attr_suffix}",
            destination(target)
        )));
    }

    /// Whether an image carrying `attr` must fall back to an HTML `<img>`: it has attributes the
    /// dialect cannot express as a native `{…}` suffix because it lacks `link_attributes`.
    pub(super) fn image_renders_as_html(&self, attr: &Attr) -> bool {
        !self.config.has(Extension::LinkAttributes) && (has_dimension(attr) || !attr_is_empty(attr))
    }

    pub(super) fn image(
        &mut self,
        attr: &Attr,
        inlines: &[Inline],
        target: &Target,
        out: &mut Vec<Piece>,
    ) {
        if self.image_renders_as_html(attr) {
            out.push(Piece::text(image_html(attr, inlines, target)));
            return;
        }
        out.push(Piece::text("!["));
        self.extend_pieces(inlines, out);
        let attr_suffix = if attr_is_empty(attr) {
            String::new()
        } else {
            attr_braces(attr)
        };
        out.push(Piece::text(format!(
            "]({}){attr_suffix}",
            destination(target)
        )));
    }

    /// Escape the markdown-significant characters of running text. Inline-markup openers (`` ` ``,
    /// `*`, `[`, `]`, `<`, `>`), the math delimiter `$`, and entity-introducing `&` are always
    /// escaped; `|` only when pipe tables make it a cell separator; `~` and `^` only when subscript
    /// and superscript have native syntax; and a word-initial `@` only when citations do. A `#` run
    /// that would open a heading is escaped at the start of a line. An `_` is escaped at a word
    /// boundary, and everywhere in a `markdown` dialect without `intraword_underscores` (the
    /// `CommonMark` family never treats an intra-word `_` as emphasis, so it is left literal there).
    /// A backslash is escaped per the raw-TeX extension. Smart-punctuation glyphs are rewritten to
    /// ASCII when the `smart` extension is active.
    pub(super) fn escape_str(&self, text: &str) -> String {
        let downgraded;
        let text = if self.config.has(Extension::Smart) {
            downgraded = downgrade_smart(text);
            downgraded.as_str()
        } else {
            text
        };
        let is_trigger = |byte: u8| {
            matches!(
                byte,
                b'#' | b'!'
                    | b'`'
                    | b'*'
                    | b'['
                    | b']'
                    | b'<'
                    | b'>'
                    | b'|'
                    | b'$'
                    | b'~'
                    | b'^'
                    | b'@'
                    | b'&'
                    | b'_'
                    | b'\\'
            )
        };
        let mut out = String::with_capacity(text.len());
        let mut prev: Option<char> = None;
        let mut backslash_run = 0usize;
        let mut offset = 0usize;
        loop {
            let remaining = text.get(offset..).unwrap_or_default();
            let clean = clean_prefix_len(remaining, is_trigger);
            if clean > 0 {
                let head = text.get(offset..offset + clean).unwrap_or_default();
                out.push_str(head);
                prev = head.chars().next_back();
                backslash_run = 0;
                offset += clean;
                continue;
            }
            let Some(ch) = remaining.chars().next() else {
                break;
            };
            let next = remaining
                .get(ch.len_utf8()..)
                .and_then(|s| s.chars().next());
            let at_start = offset == 0;
            let word_start = at_start || prev.is_some_and(char::is_whitespace);
            let tail = || text.get(offset..).unwrap_or_default();
            backslash_run = if ch == '\\' { backslash_run + 1 } else { 0 };
            match ch {
                '#' if word_start && starts_heading(tail()) => out.push_str("\\#"),
                '!' if next == Some('[') => out.push_str("\\!"),
                '`' | '*' | '[' | ']' | '<' | '>' => {
                    out.push('\\');
                    out.push(ch);
                }
                '|' if self.config.has(Extension::PipeTables) => {
                    out.push('\\');
                    out.push(ch);
                }
                '$' if self.config.has(Extension::TexMathDollars) => {
                    out.push('\\');
                    out.push(ch);
                }
                '~' if self.config.has(Extension::Subscript) => {
                    out.push('\\');
                    out.push(ch);
                }
                '~' if self.config.has(Extension::Strikeout) && next == Some('~') => {
                    out.push('\\');
                    out.push(ch);
                }
                '^' if self.config.has(Extension::Superscript) => {
                    out.push('\\');
                    out.push(ch);
                }
                '@' if self.config.has(Extension::Citations) && word_start => out.push_str("\\@"),
                '&' if begins_character_reference(tail()) => out.push_str("\\&"),
                '&' if begins_named_entity(tail()) => out.push_str("\\&"),
                '_' if is_word_boundary(prev, next)
                    || !(self.config.cmark || self.config.has(Extension::IntrawordUnderscores)) =>
                {
                    out.push_str("\\_");
                }
                '\\' => self.escape_backslash(next, backslash_run, &mut out),
                other => out.push(other),
            }
            prev = Some(ch);
            offset += ch.len_utf8();
        }
        out
    }

    /// Escape a backslash. When raw TeX passes through verbatim every backslash is doubled so it is
    /// not mistaken for an escape. Otherwise a backslash is emitted verbatim except where a run of
    /// them ends the text with an odd length: the final one is then doubled so the run pads to an
    /// even number of backslashes and its last character is part of an escaped pair rather than a
    /// stray escape. `run_len` is the length of the backslash run ending at this character.
    fn escape_backslash(&self, next: Option<char>, run_len: usize, out: &mut String) {
        if self.config.has(Extension::RawTex) {
            out.push_str("\\\\");
            return;
        }
        out.push('\\');
        if next.is_none() && run_len % 2 == 1 {
            out.push('\\');
        }
    }
}

/// Whether a `#` run at the current position would open an ATX heading: one to six `#` followed by a
/// space or the end of the run.
fn starts_heading(text: &str) -> bool {
    let hashes = text.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return false;
    }
    matches!(text.chars().nth(hashes), None | Some(' '))
}

/// Replace smart-punctuation glyphs with their ASCII equivalents for a dialect that does not write
/// them: ellipsis, en/em dashes, and curly quotes.
fn downgrade_smart(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '…' => out.push_str("..."),
            '—' => out.push_str("---"),
            '–' => out.push_str("--"),
            '“' | '”' => out.push('"'),
            '‘' | '’' => out.push('\''),
            other => out.push(other),
        }
    }
    out
}

fn has_dimension(attr: &Attr) -> bool {
    attr.attributes
        .iter()
        .any(|(key, _)| matches!(key.as_str(), "width" | "height"))
}

fn title_attr(title: &Text) -> String {
    if title.is_empty() {
        String::new()
    } else {
        format!(" title=\"{}\"", escape_html_attr(title))
    }
}

/// An image rendered as an HTML `<img>` element (the fallback for an image carrying attributes when
/// link attributes have no native syntax).
fn image_html(attr: &Attr, inlines: &[Inline], target: &Target) -> String {
    let alt = carta_ast::to_plain_text(inlines);
    let alt_attr = if alt.is_empty() {
        String::new()
    } else {
        format!(" alt=\"{}\"", escape_html_attr(&alt))
    };
    format!(
        "<img src=\"{}\"{}{}{alt_attr} />",
        escape_html_attr(&target.url),
        title_attr(&target.title),
        render_html_fragment_attr(attr),
    )
}

fn underline_attr() -> Attr {
    Attr {
        classes: vec!["underline".into()],
        ..Attr::default()
    }
}

fn smallcaps_attr() -> Attr {
    Attr {
        classes: vec!["smallcaps".into()],
        ..Attr::default()
    }
}

/// The straight ASCII quote glyphs for a quote kind, used when the dialect downgrades smart
/// punctuation.
fn ascii_quote_marks(kind: &carta_ast::QuoteType) -> (char, char) {
    match kind {
        carta_ast::QuoteType::SingleQuote => ('\'', '\''),
        carta_ast::QuoteType::DoubleQuote => ('"', '"'),
    }
}
