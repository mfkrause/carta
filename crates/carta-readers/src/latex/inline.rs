//! Inline LaTeX parsing: paragraphs, control sequences, quotes, and math.

use std::collections::BTreeMap;
use std::rc::Rc;

use carta_ast::{
    Attr, Block, Citation, CitationMode, Format, Inline, MathType, QuoteType, Target, to_plain_text,
};
use carta_core::Extension;

use crate::heading_ids::IdRegistry;

use super::support::{
    Accent, Switch, emit, emit_all, extract_spaces, flush_buf, font_span_class, group_span,
    inline_wrapper, math_env, push_whitespace, reference_link, section_intrinsic, span_class,
    switch_kind, symbol_text, trim_inlines, unescape_url, word_accent,
};
use super::tables::image_attributes;
use super::{Frame, InlineStop, Parser, Stop};

impl Parser {
    // --- Paragraph & inline ----------------------------------------------------------------------

    pub(super) fn parse_paragraph(&mut self) -> Vec<Inline> {
        let inlines = self.parse_inlines(InlineStop::Paragraph);
        trim_inlines(inlines)
    }

    pub(super) fn parse_inlines(&mut self, stop: InlineStop) -> Vec<Inline> {
        let mut out = Vec::new();
        let mut buf = String::new();
        loop {
            let Some(c) = self.cur() else {
                break;
            };
            match stop {
                InlineStop::Group if c == '}' => break,
                InlineStop::Bracket if c == ']' => break,
                InlineStop::QuoteSingle if c == '\'' && self.quote_closes(1) => break,
                InlineStop::QuoteDouble
                    if c == '\'' && self.at(1) == Some('\'') && self.quote_closes(2) =>
                {
                    break;
                }
                _ => {}
            }
            match c {
                ' ' | '\t' | '\n' | '\r' | '%' => {
                    let had_blank = self.consume_inline_ws();
                    if had_blank && matches!(stop, InlineStop::Paragraph) {
                        break;
                    }
                    flush_buf(&mut buf, &mut out);
                    if !self.eof() {
                        let ws = if had_blank || self.last_ws_had_newline {
                            Inline::SoftBreak
                        } else {
                            Inline::Space
                        };
                        push_whitespace(&mut out, ws);
                    }
                }
                '\\' => {
                    if matches!(stop, InlineStop::Paragraph) && self.inline_break_ahead() {
                        break;
                    }
                    if let Some(env) = self.peek_env_after("\\begin")
                        && math_env(&env)
                    {
                        let math = self.read_math_environment(&env);
                        emit(&mut out, &mut buf, math);
                        continue;
                    }
                    if self.try_expand_macro() {
                        continue;
                    }
                    // A font-switch command applies to the remainder of the enclosing group.
                    if let Some(word) = self.peek_control_word()
                        && let Some(switch) = switch_kind(&word)
                    {
                        self.apply_switch(switch, stop, &mut out, &mut buf);
                        break;
                    }
                    // a command flushes the buffer only when it emits an inline; accents and
                    // symbols append so they join the surrounding word
                    self.exec_control(&mut out, &mut buf);
                }
                '{' => {
                    self.bump();
                    let inner = self.parse_inlines(InlineStop::Group);
                    if self.cur() == Some('}') {
                        self.bump();
                    }
                    // an empty group keeps the word intact; a non-empty one becomes a grouping span
                    if let Some(span) = group_span(inner) {
                        emit(&mut out, &mut buf, span);
                    }
                }
                '}' => {
                    // A stray close brace outside a group is treated as a literal.
                    buf.push('}');
                    self.bump();
                }
                '$' => {
                    flush_buf(&mut buf, &mut out);
                    let math = self.read_dollar_math();
                    out.push(math);
                }
                '~' => {
                    buf.push('\u{a0}');
                    self.bump();
                }
                '-' => {
                    self.read_dashes(&mut buf);
                }
                '`' if self.smart => {
                    flush_buf(&mut buf, &mut out);
                    self.read_open_quote(&mut out);
                }
                '\'' => {
                    self.read_apostrophe(&mut buf);
                }
                _ => {
                    buf.push(c);
                    self.bump();
                }
            }
        }
        flush_buf(&mut buf, &mut out);
        out
    }

    /// Whether `\`-introduced content at the cursor starts a new block, ending a paragraph.
    pub(super) fn inline_break_ahead(&self) -> bool {
        if let Some(env) = self.peek_env_after("\\begin") {
            return !math_env(&env);
        }
        if self.looking_at("\\end") && self.peek_env_after("\\end").is_some() {
            return true;
        }
        if let Some(word) = self.peek_control_word() {
            if section_intrinsic(&word).is_some() {
                return true;
            }
            if self.in_float && word == "caption" {
                return true;
            }
            return matches!(word.as_str(), "item" | "par");
        }
        false
    }

    /// Whether a smart-quote delimiter at `offset` from the cursor closes an open quote: it does
    /// when the character after it is not alphanumeric.
    pub(super) fn quote_closes(&self, offset: usize) -> bool {
        match self.at(offset) {
            Some(c) => !c.is_alphanumeric(),
            None => true,
        }
    }

    /// Consumes a run of whitespace and comments. Returns whether it spanned a blank line. Records
    /// whether the run contained a newline in `last_ws_had_newline`.
    pub(super) fn consume_inline_ws(&mut self) -> bool {
        let mut newlines = 0u32;
        loop {
            match self.cur() {
                Some('\n') => {
                    newlines += 1;
                    self.bump();
                }
                Some(' ' | '\t' | '\r') => {
                    self.bump();
                }
                Some('%') => self.skip_comment(),
                _ => break,
            }
        }
        self.last_ws_had_newline = newlines > 0;
        newlines >= 2
    }

    pub(super) fn read_dashes(&mut self, buf: &mut String) {
        let mut count = 0;
        while self.cur() == Some('-') {
            count += 1;
            self.bump();
        }
        while count >= 3 {
            buf.push('\u{2014}');
            count -= 3;
        }
        if count == 2 {
            buf.push('\u{2013}');
        } else if count == 1 {
            buf.push('-');
        }
    }

    pub(super) fn read_apostrophe(&mut self, buf: &mut String) {
        if self.cur() == Some('\'') && self.at(1) == Some('\'') {
            buf.push('\u{201d}');
            self.bump();
            self.bump();
        } else {
            buf.push('\u{2019}');
            self.bump();
        }
    }

    /// Opens a smart quote at a `` ` ``, reading its content up to the matching close.
    pub(super) fn read_open_quote(&mut self, out: &mut Vec<Inline>) {
        if self.at(1) == Some('`') {
            self.bump();
            self.bump();
            let inner = self.parse_inlines(InlineStop::QuoteDouble);
            if self.cur() == Some('\'') && self.at(1) == Some('\'') {
                self.bump();
                self.bump();
                out.push(Inline::Quoted(QuoteType::DoubleQuote, inner));
            } else {
                out.push(Inline::Str("\u{201c}".into()));
                out.extend(inner);
            }
        } else {
            self.bump();
            let inner = self.parse_inlines(InlineStop::QuoteSingle);
            if self.cur() == Some('\'') {
                self.bump();
                out.push(Inline::Quoted(QuoteType::SingleQuote, inner));
            } else {
                out.push(Inline::Str("\u{2018}".into()));
                out.extend(inner);
            }
        }
    }

    // --- Inline commands -------------------------------------------------------------------------

    /// Dispatches a control sequence in inline context, appending inlines to `out` or text to `buf`.
    pub(super) fn exec_control(&mut self, out: &mut Vec<Inline>, buf: &mut String) {
        // A control symbol: a backslash followed by a single non-letter.
        if self.at(1).is_some_and(|c| !c.is_ascii_alphabetic()) {
            self.exec_control_symbol(out, buf);
            return;
        }
        let name = self.consume_control_word();
        self.exec_named(&name, out, buf);
    }

    pub(super) fn exec_control_symbol(&mut self, out: &mut Vec<Inline>, buf: &mut String) {
        self.bump(); // backslash
        let Some(c) = self.bump() else {
            return;
        };
        match c {
            '\\' => {
                // hard line break: `*` and `[dimen]` discarded, surrounding spacing absorbed
                if self.cur() == Some('*') {
                    self.bump();
                }
                let _ = self.read_optional_raw();
                flush_buf(buf, out);
                while matches!(out.last(), Some(Inline::Space | Inline::SoftBreak)) {
                    out.pop();
                }
                out.push(Inline::LineBreak);
            }
            '[' => {
                let text = self.read_math_body("\\]");
                emit(out, buf, Inline::Math(MathType::DisplayMath, text.into()));
            }
            '(' => {
                let text = self.read_math_body("\\)");
                emit(out, buf, Inline::Math(MathType::InlineMath, text.into()));
            }
            // An explicit inter-word space is a non-breaking space.
            ' ' | '\n' | '\t' => buf.push('\u{a0}'),
            // A thin space.
            ',' => buf.push('\u{2006}'),
            '&' | '%' | '#' | '$' | '_' | '{' | '}' => buf.push(c),
            '~' => self.read_accent_symbol(Accent::Tilde, buf),
            '^' => self.read_accent_symbol(Accent::Circumflex, buf),
            '\'' => self.read_accent_symbol(Accent::Acute, buf),
            '`' => self.read_accent_symbol(Accent::Grave, buf),
            '"' => self.read_accent_symbol(Accent::Diaeresis, buf),
            '=' => self.read_accent_symbol(Accent::Macron, buf),
            '.' => self.read_accent_symbol(Accent::DotAbove, buf),
            // Discretionary/zero-width spacing and escaped delimiters that carry no text.
            '-' | '/' | ';' | ':' | '!' | '@' | ')' | ']' => {}
            other => buf.push(other),
        }
    }

    /// Applies a font-switch command (`\bf`, `\em`, …) to the remainder of the enclosing group.
    pub(super) fn apply_switch(
        &mut self,
        switch: Switch,
        stop: InlineStop,
        out: &mut Vec<Inline>,
        buf: &mut String,
    ) {
        self.consume_control_word();
        flush_buf(buf, out);
        let rest = self.parse_inlines(stop);
        if matches!(switch, Switch::Code) {
            out.push(switch.wrap(rest));
        } else {
            out.extend(extract_spaces(rest, |i| switch.wrap(i)));
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn exec_named(&mut self, name: &str, out: &mut Vec<Inline>, buf: &mut String) {
        // Wrapping formatters. Most pull surrounding spacing out of the wrapper; underline keeps it.
        if let Some(wrap) = inline_wrapper(name) {
            let inner = self.parse_group_inlines();
            if matches!(name, "underline" | "uline") {
                emit(out, buf, wrap(inner));
            } else {
                emit_all(out, buf, extract_spaces(inner, wrap));
            }
            return;
        }
        // Accent commands spelled as control words apply to their argument's base character.
        if let Some(accent) = word_accent(name) {
            self.read_accent_symbol(accent, buf);
            return;
        }
        // Font family/shape/series switches wrap their argument in a single-class span.
        if let Some(class) = font_span_class(name) {
            let inner = self.parse_group_inlines();
            emit_all(out, buf, extract_spaces(inner, |i| span_class(i, class)));
            return;
        }
        match name {
            "textcolor" | "colorbox" => {
                let color = self.read_group_raw().unwrap_or_default();
                let inner = self.parse_group_inlines();
                let property = if name == "colorbox" {
                    "background-color"
                } else {
                    "color"
                };
                let attr = Attr {
                    id: carta_ast::Text::default(),
                    classes: Vec::new(),
                    attributes: vec![(
                        "style".into(),
                        format!("{property}: {}", color.trim()).into(),
                    )],
                };
                emit(out, buf, Inline::Span(Box::new(attr), inner));
            }
            "texttt" | "lstinline" => {
                if name == "lstinline" {
                    let _ = self.read_optional_raw();
                }
                let inner = self.parse_group_inlines();
                emit(
                    out,
                    buf,
                    Inline::Code(Box::default(), to_plain_text(&inner).into()),
                );
            }
            "verb" => {
                if let Some(code) = self.read_verb() {
                    emit(out, buf, Inline::Code(Box::default(), code.into()));
                }
            }
            "footnote" | "footnotetext" | "thanks" => {
                let _ = self.read_optional_raw();
                let blocks = self.parse_group_blocks();
                emit(out, buf, Inline::Note(blocks));
            }
            "url" | "nolinkurl" => {
                if let Some(url) = self.read_group_raw() {
                    let url = unescape_url(&url);
                    emit(
                        out,
                        buf,
                        Inline::Link(
                            Box::new(Attr {
                                id: carta_ast::Text::default(),
                                classes: vec!["uri".into()],
                                attributes: Vec::new(),
                            }),
                            vec![Inline::Str(url.clone().into())],
                            Box::new(Target {
                                url: url.into(),
                                title: carta_ast::Text::default(),
                            }),
                        ),
                    );
                }
            }
            "href" => {
                let url = self
                    .read_group_raw()
                    .map(|u| unescape_url(&u))
                    .unwrap_or_default();
                let text = self.parse_group_inlines();
                emit(
                    out,
                    buf,
                    Inline::Link(
                        Box::default(),
                        text,
                        Box::new(Target {
                            url: url.into(),
                            title: carta_ast::Text::default(),
                        }),
                    ),
                );
            }
            "includegraphics" => {
                let opts = self.read_optional_raw().unwrap_or_default();
                let path = self.read_group_raw().unwrap_or_default();
                let attributes = image_attributes(&opts);
                let alt = if self.in_figure {
                    Vec::new()
                } else {
                    vec![Inline::Str("image".into())]
                };
                emit(
                    out,
                    buf,
                    Inline::Image(
                        Box::new(Attr {
                            id: carta_ast::Text::default(),
                            classes: Vec::new(),
                            attributes: attributes
                                .into_iter()
                                .map(|(k, v)| (k.into(), v.into()))
                                .collect(),
                        }),
                        alt,
                        Box::new(Target {
                            url: path.into(),
                            title: carta_ast::Text::default(),
                        }),
                    ),
                );
            }
            "label" => {
                if let Some(id) = self.read_group_raw() {
                    emit(
                        out,
                        buf,
                        Inline::Span(
                            Box::new(Attr {
                                id: id.clone().into(),
                                classes: Vec::new(),
                                attributes: vec![("label".into(), id.into())],
                            }),
                            Vec::new(),
                        ),
                    );
                }
            }
            "ref" | "eqref" | "autoref" | "cref" | "Cref" => {
                if let Some(target) = self.read_group_raw() {
                    emit(out, buf, reference_link(name, &target));
                }
            }
            "cite" | "citep" | "citet" | "citealp" | "citealt" | "citeauthor" | "citeyear"
            | "parencite" | "textcite" | "footcite" | "autocite" => {
                flush_buf(buf, out);
                self.read_citation(name, out);
            }
            "textsuperscript" | "textsubscript" => {
                let inner = self.parse_group_inlines();
                let wrap: fn(Vec<Inline>) -> Inline = if name == "textsubscript" {
                    Inline::Subscript
                } else {
                    Inline::Superscript
                };
                emit_all(out, buf, extract_spaces(inner, wrap));
            }
            "mbox" | "hbox" => {
                let inner = self.parse_group_inlines();
                emit_all(out, buf, inner);
            }
            "ensuremath" => {
                let body = self.read_group_raw().unwrap_or_default();
                emit(
                    out,
                    buf,
                    Inline::Math(MathType::InlineMath, body.trim().into()),
                );
            }
            "footnotemark" | "protect" | "noindent" | "indent" | "bigskip" | "medskip"
            | "smallskip" | "centering" | "hfill" | "hrulefill" | "dotfill" | "par"
            | "displaystyle" | "scriptsize" | "small" | "footnotesize" | "large" | "Large"
            | "LARGE" | "huge" | "Huge" | "normalsize" | "rmfamily" | "sffamily" | "ttfamily"
            | "mdseries" | "upshape" | "normalfont" | "sc" | "rm" | "sf" | "boldmath"
            | "unboldmath" | "clearpage" | "newpage" | "nolinebreak" | "sloppy" | "raggedright"
            | "item" => {
                // no-argument font-switch/spacing commands contribute nothing; a stray `\item` is dropped
            }
            "linebreak" => {
                let _ = self.read_optional_raw();
                emit(out, buf, Inline::LineBreak);
            }
            "newline" => emit(out, buf, Inline::LineBreak),
            "hspace" | "vspace" | "hskip" | "vskip" | "setlength" | "vphantom" | "hphantom"
            | "phantom" | "rule" | "settowidth" => {
                self.skip_command_args(name);
            }
            _ => {
                if let Some(text) = symbol_text(name) {
                    buf.push_str(text);
                } else if self.ext.contains(Extension::RawTex) {
                    let raw = self.reconstruct_command(name);
                    emit(
                        out,
                        buf,
                        Inline::RawInline(Format("latex".into()), raw.into()),
                    );
                } else {
                    // Unknown command: drop it along with any adjacent bracket/brace arguments.
                    self.skip_adjacent_arguments();
                }
            }
        }
    }

    /// Rebuilds an unknown command's source, including any immediately following optional and braced
    /// arguments, for verbatim passthrough.
    pub(super) fn reconstruct_command(&mut self, name: &str) -> String {
        let mut raw = format!("\\{name}");
        loop {
            match self.cur() {
                Some('[') => {
                    if let Some(opt) = self.read_optional_raw() {
                        raw.push('[');
                        raw.push_str(&opt);
                        raw.push(']');
                    } else {
                        break;
                    }
                }
                Some('{') => {
                    if let Some(arg) = self.read_group_raw() {
                        raw.push('{');
                        raw.push_str(&arg);
                        raw.push('}');
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        raw
    }

    pub(super) fn read_citation(&mut self, name: &str, out: &mut Vec<Inline>) {
        // one bracketed arg is the trailing note; with two, the first precedes the key
        let opt1 = self.read_optional_raw();
        let opt2 = self.read_optional_raw();
        let keys_raw = self.read_group_raw().unwrap_or_default();
        let (prefix_raw, suffix_raw) = match (&opt1, &opt2) {
            (Some(pre), Some(post)) => (Some(pre.as_str()), Some(post.as_str())),
            (Some(post), None) => (None, Some(post.as_str())),
            _ => (None, None),
        };
        let prefix = prefix_raw
            .map(|s| self.parse_fragment(s))
            .unwrap_or_default();
        let suffix = suffix_raw
            .map(|s| self.parse_fragment(s))
            .unwrap_or_default();
        let mode = if matches!(name, "citet" | "textcite" | "citeauthor") {
            CitationMode::AuthorInText
        } else {
            CitationMode::NormalCitation
        };
        let mut citations = Vec::new();
        for key in keys_raw.split(',') {
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            citations.push(Citation {
                id: key.into(),
                prefix: Vec::new(),
                suffix: Vec::new(),
                mode: mode.clone(),
                note_num: 0,
                hash: 0,
            });
        }
        if citations.is_empty() {
            return;
        }
        if let Some(first) = citations.first_mut() {
            first.prefix = prefix;
        }
        if let Some(last) = citations.last_mut() {
            last.suffix = suffix;
        }
        let mut raw = format!("\\{name}");
        for opt in [&opt1, &opt2].into_iter().flatten() {
            raw.push('[');
            raw.push_str(opt);
            raw.push(']');
        }
        raw.push('{');
        raw.push_str(&keys_raw);
        raw.push('}');
        out.push(Inline::Cite(
            citations,
            vec![Inline::RawInline(Format("latex".into()), raw.into())],
        ));
    }

    /// Reads `\verb<delim>…<delim>` (or `\verb*…`) verbatim.
    pub(super) fn read_verb(&mut self) -> Option<String> {
        if self.cur() == Some('*') {
            self.bump();
        }
        let delim = self.bump()?;
        let mut code = String::new();
        while let Some(c) = self.cur() {
            self.bump();
            if c == delim {
                break;
            }
            code.push(c);
        }
        Some(code)
    }

    /// A sub-parser over `source` that inherits the shared context (extensions, smart mode, macro
    /// table, section base level, expansion depth) but starts with fresh cursor and output state
    /// (metadata and heading ids). It never inherits float context.
    pub(super) fn child(&self, source: &str, in_figure: bool) -> Parser {
        Parser {
            frames: vec![Frame {
                chars: source.chars().collect(),
                pos: 0,
            }],
            ext: self.ext,
            smart: self.smart,
            meta: BTreeMap::new(),
            macros: Rc::clone(&self.macros),
            ids: IdRegistry::default(),
            base_level: self.base_level,
            in_figure,
            in_float: false,
            expand_depth: self.expand_depth,
            total_expansions: 0,
            last_ws_had_newline: false,
        }
    }

    pub(super) fn parse_group_blocks(&mut self) -> Vec<Block> {
        if self.cur() != Some('{') {
            return Vec::new();
        }
        // Slice out the balanced group and parse it as a sub-document so paragraph breaks work.
        let source = self.read_group_raw().unwrap_or_default();
        let mut sub = self.child(&source, self.in_figure);
        sub.parse_blocks(&Stop::Eof)
    }

    pub(super) fn parse_group_inlines(&mut self) -> Vec<Inline> {
        if self.cur() != Some('{') {
            return Vec::new();
        }
        self.bump();
        let inner = self.parse_inlines(InlineStop::Group);
        if self.cur() == Some('}') {
            self.bump();
        }
        inner
    }

    // --- Math ------------------------------------------------------------------------------------

    pub(super) fn read_dollar_math(&mut self) -> Inline {
        if self.cur() == Some('$') && self.at(1) == Some('$') {
            self.bump();
            self.bump();
            Inline::Math(MathType::DisplayMath, self.read_math_body("$$").into())
        } else {
            self.bump();
            Inline::Math(MathType::InlineMath, self.read_math_body("$").into())
        }
    }

    /// Reads math source up to and consuming `close`, then trims surrounding whitespace.
    pub(super) fn read_math_body(&mut self, close: &str) -> String {
        let mut text = String::new();
        while !self.eof() {
            if self.looking_at(close) {
                self.advance_chars(close.chars().count());
                break;
            }
            // A backslash escape keeps its following character, so `\$` does not end `$` math.
            if self.cur() == Some('\\') {
                if let Some(c) = self.bump() {
                    text.push(c);
                }
                if let Some(c) = self.bump() {
                    text.push(c);
                }
                continue;
            }
            if let Some(c) = self.bump() {
                text.push(c);
            }
        }
        text.trim().to_owned()
    }
}
