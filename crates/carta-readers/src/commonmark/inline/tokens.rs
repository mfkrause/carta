//! Inline token scanners: escapes, math, raw TeX, code spans, autolinks, entities, and the
//! `@`/emoji/short-script/smart-punctuation constructs the main scan dispatches to.

use std::collections::BTreeMap;

use carta_ast::{Attr, Citation, CitationMode, Inline, MathType};
use carta_core::Extension;

use super::super::emphasis::run_flanking;
use super::super::scan::{
    char_at, is_ascii_punctuation, scan_autolink, scan_entity, scan_html_tag,
};
use super::helpers::{
    backtick_run_len, classify_angle_autolink, escape_link_destination, is_citation_word,
    is_classic_markdown_escapable, normalize_code, scan_citation_id,
};
use super::{
    Delimiter, InlineParser, Node, char_before, emoji, fold_dash_run_thirds, fold_ellipsis_run,
};

impl InlineParser<'_> {
    /// Resolve an `@` at the cursor. An example-list label assigned a number becomes that number; a
    /// well-formed citation key becomes a bare author-in-text `Cite`; anything else leaves the `@`
    /// as literal text, so the rest of the run reparses normally.
    pub(super) fn at_sign(&mut self) {
        if self.ext.contains(Extension::ExampleLists) && self.try_example_ref() {
            return;
        }
        if self.ext.contains(Extension::Citations) && self.try_bare_citation() {
            return;
        }
        self.pos += 1;
        self.push_text('@');
    }

    /// Try an example-list reference `@label` at the cursor. A label assigned a number by an example
    /// item is replaced with that number and the cursor advances past it, returning `true`. An
    /// undefined or empty label leaves the cursor in place and returns `false`.
    fn try_example_ref(&mut self) -> bool {
        // `@` and label chars are ASCII, so byte and char offsets coincide.
        let name_start = self.pos + 1;
        let mut end = name_start;
        while matches!(
            char_at(self.text, end),
            Some('0'..='9' | 'a'..='z' | 'A'..='Z' | '-' | '_')
        ) {
            end += 1;
        }
        if end == name_start {
            return false;
        }
        let Some(label) = self.text.get(name_start..end) else {
            return false;
        };
        if let Some(number) = self.notes.examples.get(label) {
            self.pos = end;
            self.push_str(&number.to_string());
            return true;
        }
        false
    }

    /// Try a bare author-in-text citation `@key` at the cursor (which sits on the `@`). It forms a
    /// citation only when the `@` is not glued to a preceding word character and a well-formed key
    /// follows. On success the cursor advances past the key, the running citation count rises, and a
    /// single-entry `Cite` is pushed whose fallback text is the literal `@key`. Returns `false`
    /// (without advancing) otherwise, leaving the `@` for literal handling.
    fn try_bare_citation(&mut self) -> bool {
        if matches!(char_before(self.text, self.pos), Some(c) if is_citation_word(c)) {
            return false;
        }
        let Some((id, next)) = scan_citation_id(self.text, self.pos + 1) else {
            return false;
        };
        let note_num = self.bump_cite_count();
        self.pos = next;
        let citation = Citation {
            id: id.clone().into(),
            prefix: Vec::new(),
            suffix: Vec::new(),
            mode: CitationMode::AuthorInText,
            note_num,
            hash: 0,
        };
        self.nodes.push(Node::Inline(Inline::Cite(
            vec![citation],
            vec![Inline::Str(format!("@{id}").into())],
        )));
        true
    }

    /// Advance the document-wide citation count and return the new value.
    pub(super) fn bump_cite_count(&self) -> i32 {
        let next = self.notes.cite_count.get().saturating_add(1);
        self.notes.cite_count.set(next);
        next
    }

    /// Resolve an emoji shortcode `:name:` at the cursor (which sits on the opening `:`). A name is
    /// one or more ASCII letters, digits, `_`, `+`, or `-`, terminated by a closing `:`. When the
    /// name is in the curated table, the whole `:name:` becomes a `Span` classed `emoji` carrying the
    /// name in a `data-emoji` attribute and the unicode character as its text; the cursor advances
    /// past the closing `:` and `true` is returned. An unrecognized name (or no closing `:`) leaves
    /// the leading `:` untouched and returns `false`, so the run reparses as literal text.
    pub(super) fn try_emoji(&mut self) -> bool {
        // The `:` delimiters and every name character (`[0-9A-Za-z_+-]`) are ASCII.
        let name_start = self.pos + 1;
        let mut index = name_start;
        while matches!(
            char_at(self.text, index),
            Some('0'..='9' | 'a'..='z' | 'A'..='Z' | '_' | '+' | '-')
        ) {
            index += 1;
        }
        if index == name_start || char_at(self.text, index) != Some(':') {
            return false;
        }
        let Some(name) = self.text.get(name_start..index) else {
            return false;
        };
        let Some(codepoints) = emoji::lookup(name) else {
            return false;
        };
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: vec!["emoji".into()],
            attributes: vec![("data-emoji".into(), name.into())],
        };
        self.pos = index + 1;
        self.nodes.push(Node::Inline(Inline::Span(
            Box::new(attr),
            vec![Inline::Str(codepoints.into())],
        )));
        true
    }

    /// Try a short sub/superscript at the cursor (`short_subsuperscripts`): a `~` or `^` directly
    /// followed by a run of alphanumerics, taken as the sub/superscript content without a closing
    /// delimiter. Within a caret's whitespace-bounded span the delimiters pair up left to right into
    /// the delimited `^x^`/`~x~` form, which the delimiter stack resolves; the short form applies
    /// only to an unpaired opener: an even number of matching delimiters precede it in the span and
    /// none follow. An empty alphanumeric run (a delimiter met by a non-alphanumeric or the line's
    /// end) is not a script.
    pub(super) fn try_short_script(&mut self, delimiter: char) -> bool {
        let mut preceding = 0usize;
        let mut behind = self.pos;
        while let Some(ch) = char_before(self.text, behind) {
            if ch.is_whitespace() {
                break;
            }
            if ch == delimiter {
                preceding += 1;
            }
            behind -= ch.len_utf8();
        }
        // An odd count leaves this delimiter closing a prior opener, never starting a short script.
        if preceding % 2 == 1 {
            return false;
        }
        // A matching delimiter ahead in-span pairs into the delimited form instead; `~`/`^` are
        // ASCII, so the span begins one byte past the cursor.
        let mut ahead = self.pos + 1;
        while let Some(ch) = char_at(self.text, ahead) {
            if ch.is_whitespace() {
                break;
            }
            if ch == delimiter {
                return false;
            }
            ahead += ch.len_utf8();
        }
        let start = self.pos + 1;
        let mut end = start;
        while let Some(ch) = char_at(self.text, end) {
            if ch.is_alphanumeric() {
                end += ch.len_utf8();
            } else {
                break;
            }
        }
        let content = match self.text.get(start..end) {
            Some(slice) if !slice.is_empty() => slice,
            _ => return false,
        };
        let inner = vec![Inline::Str(content.into())];
        let node = if delimiter == '^' {
            Inline::Superscript(inner)
        } else {
            Inline::Subscript(inner)
        };
        self.nodes.push(Node::Inline(node));
        self.pos = end;
        true
    }

    pub(super) fn backslash(&mut self) {
        // TeX math and raw TeX outrank a plain escape; `\\(`/`\\[` tried before `\(`/`\[` so the
        // longer opener wins.
        if self.try_backslash_math() || self.try_raw_tex() {
            return;
        }
        self.pos += 1;
        // Broad escaping (`all_symbols_escapable`, always on in bare CommonMark) covers all ASCII
        // punctuation; without it the markdown dialect escapes only the classic set.
        let broad = !self.notes.markdown || self.ext.contains(Extension::AllSymbolsEscapable);
        match self.peek() {
            // Without `escaped_line_breaks` the markdown dialect keeps the backslash literal and
            // soft-breaks; bare CommonMark always hard-breaks here.
            Some('\n')
                if self.notes.markdown && !self.ext.contains(Extension::EscapedLineBreaks) =>
            {
                self.push_text('\\');
            }
            Some('\n') => {
                self.pos += 1;
                while matches!(self.peek(), Some(' ' | '\t')) {
                    self.pos += 1;
                }
                self.nodes.push(Node::LineBreak);
            }
            // Backslash-space is a non-breaking space, binding into the surrounding text.
            Some(' ') if self.notes.markdown && broad => {
                self.pos += 1;
                self.push_text('\u{a0}');
            }
            Some(ch) if broad && is_ascii_punctuation(ch) => {
                self.pos += 1;
                self.push_text(ch);
            }
            Some(ch) if is_classic_markdown_escapable(ch) => {
                self.pos += 1;
                self.push_text(ch);
            }
            _ => self.push_text('\\'),
        }
    }

    /// Try the backslash math delimiters at the cursor (which sits on the leading `\`). `\(…\)` is
    /// inline math and `\[…\]` is display math; the double-backslash forms `\\(…\\)` and `\\[…\\]`
    /// use the same shapes with a doubled delimiter. Each form is gated behind its extension, and the
    /// double-backslash form is preferred so its longer opener is not stolen by the single form.
    /// Returns `true` (and advances past the closer) on a match, leaving a fallback escape otherwise.
    fn try_backslash_math(&mut self) -> bool {
        if self.ext.contains(Extension::TexMathDoubleBackslash)
            && char_at(self.text, self.pos) == Some('\\')
            && char_at(self.text, self.pos + 1) == Some('\\')
            && self.scan_backslash_math(2)
        {
            return true;
        }
        if self.ext.contains(Extension::TexMathSingleBackslash)
            && char_at(self.text, self.pos) == Some('\\')
            && self.scan_backslash_math(1)
        {
            return true;
        }
        false
    }

    /// Scan a backslash math span at the cursor (on the first backslash), pushing a `Math` node and
    /// advancing past the closer on a match. See [`crate::inline_scan::scan_backslash_math_bytes`].
    fn scan_backslash_math(&mut self, slashes: usize) -> bool {
        match crate::inline_scan::scan_backslash_math_bytes(self.text, self.pos, slashes) {
            Some((math_type, content, next)) => {
                self.pos = next;
                self.nodes
                    .push(Node::Inline(Inline::Math(math_type, content.into())));
                true
            }
            None => false,
        }
    }

    /// Try a raw inline TeX command at the cursor (on the leading `\`), gated behind `raw_tex`. A
    /// command is a backslash, an ASCII letter, and any following ASCII alphanumerics, optionally
    /// followed by balanced `{…}` and `[…]` argument groups. A `{`-group that opens but cannot be
    /// balance-closed reverts the whole command to literal text; an unclosable `[`-group ends
    /// the group run, leaving the command captured so far. A command with no argument groups and an
    /// all-letter name absorbs any run of trailing spaces and tabs (but not a newline). On a match the
    /// verbatim source becomes a `RawInline (Format "tex")` and the cursor advances past it.
    ///
    /// Known limitations:
    /// - Every group is consumed greedily, so a command takes all the `{…}`/`[…]` groups that
    ///   directly follow it. Some commands accept only a fixed number of arguments and leave the
    ///   rest as text; that per-command arity is not modeled here.
    /// - A paragraph that is wholly a `\begin{env}…\end{env}` environment is recognized in the block
    ///   phase; here every `\begin`/`\end` is treated as an ordinary inline command.
    fn try_raw_tex(&mut self) -> bool {
        if !self.ext.contains(Extension::RawTex) {
            return false;
        }
        if char_at(self.text, self.pos) != Some('\\') {
            return false;
        }
        // `\` and the name (ASCII) are single-byte, so byte and char offsets coincide up to `i`.
        let mut i = self.pos + 1;
        if !char_at(self.text, i).is_some_and(|c| c.is_ascii_alphabetic()) {
            return false;
        }
        i += 1;
        let mut name_all_letters = true;
        while let Some(ch) = char_at(self.text, i) {
            if ch.is_ascii_alphabetic() {
                i += 1;
            } else if ch.is_ascii_digit() {
                name_all_letters = false;
                i += 1;
            } else {
                break;
            }
        }
        // `\begin`/`\end` are raw TeX only as a complete matched environment, never bare.
        let name = self.text.get(self.pos + 1..i);
        if name == Some("begin") {
            return self.try_raw_tex_environment(i);
        }
        if name == Some("end") {
            return false;
        }
        // Consume argument groups. A `{`-group must balance or the entire command reverts to text.
        let mut had_group = false;
        loop {
            match char_at(self.text, i) {
                Some('{') => match self.scan_balanced_group(i, '{', '}') {
                    Some(end) => {
                        i = end;
                        had_group = true;
                    }
                    None => return false,
                },
                Some('[') => match self.scan_balanced_group(i, '[', ']') {
                    Some(end) => {
                        i = end;
                        had_group = true;
                    }
                    None => break,
                },
                _ => break,
            }
        }

        // A bare all-letter command absorbs trailing spaces and tabs.
        if !had_group && name_all_letters {
            while matches!(char_at(self.text, i), Some(' ' | '\t')) {
                i += 1;
            }
        }

        let source = match self.text.get(self.pos..i) {
            Some(slice) => slice.to_owned(),
            None => return false,
        };
        self.pos = i;
        self.nodes.push(Node::Inline(Inline::RawInline(
            carta_ast::Format("tex".into()),
            source.into(),
        )));
        true
    }

    /// Capture a complete `\begin{ENV}`…matching `\end{ENV}` as a single raw TeX inline. The
    /// opener's `{ENV}` group names the environment; nested `\begin{ENV}`/`\end{ENV}` of that same
    /// name deepen and lift the nesting, and the capture ends at the `\end{ENV}` that returns the
    /// depth to zero. Without a `{ENV}` group or a matching close the `\begin` is not raw TeX and
    /// the call reverts to literal text by returning `false`.
    fn try_raw_tex_environment(&mut self, name_end: usize) -> bool {
        if char_at(self.text, name_end) != Some('{') {
            return false;
        }
        let Some(group_end) = self.scan_balanced_group(name_end, '{', '}') else {
            return false;
        };
        // `group_end - 1` is the closing `}` (ASCII); `name_end + 1` is past the opening `{`.
        let Some(env) = self.text.get(name_end + 1..group_end - 1) else {
            return false;
        };
        let Some(end) = self.scan_environment_close(group_end, env) else {
            return false;
        };
        let source = match self.text.get(self.pos..end) {
            Some(slice) => slice.to_owned(),
            None => return false,
        };
        self.pos = end;
        self.nodes.push(Node::Inline(Inline::RawInline(
            carta_ast::Format("tex".into()),
            source.into(),
        )));
        true
    }

    /// From `from`, find the index just past the `\end{ENV}` that closes an open `\begin{ENV}`,
    /// tracking nested same-name environments by depth. `None` when no matching close is found.
    fn scan_environment_close(&mut self, from: usize, env: &str) -> Option<usize> {
        if self.raw_tex_budget == 0 {
            return None;
        }
        // A close cannot lie past the buffer's last `\end{ENV}` marker: the depth counter only
        // delays accepting a close, never conjures one, so the bound is exact and this is O(1).
        if self
            .last_environment_close(env)
            .is_none_or(|last| from > last)
        {
            return None;
        }
        let mut depth = 1usize;
        let mut i = from;
        while let Some(ch) = char_at(self.text, i) {
            if ch == '\\' {
                if let Some(after) = self.match_environment_marker(i, "begin", env) {
                    depth += 1;
                    i = after;
                    continue;
                }
                if let Some(after) = self.match_environment_marker(i, "end", env) {
                    depth -= 1;
                    if depth == 0 {
                        self.charge_raw_tex(after - from);
                        return Some(after);
                    }
                    i = after;
                    continue;
                }
            }
            i += ch.len_utf8();
        }
        self.charge_raw_tex(i - from);
        None
    }

    /// Byte offset where the last `\end{NAME}` marker begins, or `None` when the buffer holds none.
    /// Computed once per environment name (a literal substring search) and cached.
    fn last_environment_close(&mut self, env: &str) -> Option<usize> {
        if let Some(&cached) = self.env_last_close.get(env) {
            return cached;
        }
        let marker = format!("\\end{{{env}}}");
        let last = self.text.rfind(&marker);
        self.env_last_close.insert(env.to_owned(), last);
        last
    }

    /// If the characters at `at` spell `\KEYWORD{ENV}` (e.g. `\end{equation}`), return the index
    /// just past the closing brace; otherwise `None`.
    fn match_environment_marker(&self, at: usize, keyword: &str, env: &str) -> Option<usize> {
        let mut i = at;
        if char_at(self.text, i) != Some('\\') {
            return None;
        }
        i += 1;
        for kc in keyword.chars() {
            if char_at(self.text, i) != Some(kc) {
                return None;
            }
            i += kc.len_utf8();
        }
        if char_at(self.text, i) != Some('{') {
            return None;
        }
        i += 1;
        for ec in env.chars() {
            if char_at(self.text, i) != Some(ec) {
                return None;
            }
            i += ec.len_utf8();
        }
        if char_at(self.text, i) != Some('}') {
            return None;
        }
        Some(i + 1)
    }

    /// Scan a balanced group `open`…`close` starting at index `start` (which must hold `open`),
    /// returning the index just past the matching `close`, or `None` if it never closes. Nested
    /// same-kind delimiters are tracked by depth. `open` and `close` are ASCII delimiters.
    fn scan_balanced_group(&mut self, start: usize, open: char, close: char) -> Option<usize> {
        if self.raw_tex_budget == 0 {
            return None;
        }
        // A group opened past the buffer's last `close` can never balance: fail in O(1) without
        // charging the budget. Only `}` and `]` arise as raw-TeX group closers.
        let last_close = match close {
            '}' => *self.last_brace.get_or_init(|| self.text.rfind('}')),
            ']' => *self.last_bracket.get_or_init(|| self.text.rfind(']')),
            _ => return None,
        };
        if last_close.is_none_or(|last| start > last) {
            return None;
        }
        let mut depth = 0usize;
        let mut i = start;
        while let Some(ch) = char_at(self.text, i) {
            if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    self.charge_raw_tex(i + 1 - start);
                    return Some(i + 1);
                }
            }
            i += ch.len_utf8();
        }
        self.charge_raw_tex(i - start);
        None
    }

    /// Charge a raw-TeX look-ahead scan's traversal length against the shared per-buffer budget.
    fn charge_raw_tex(&mut self, steps: usize) {
        self.raw_tex_budget = self.raw_tex_budget.saturating_sub(steps);
    }

    pub(super) fn code_span(&mut self) {
        let start = self.pos;
        let open = backtick_run_len(self.text, self.pos);
        self.pos += open;
        if let Some(close) = self.next_backtick_run(open, self.pos) {
            let content = self
                .text
                .get(self.pos..close)
                .map(str::to_owned)
                .unwrap_or_default();
            self.pos = close + open;
            if let Some((format, next)) = self.scan_raw_format() {
                self.pos = next;
                self.nodes.push(Node::Inline(Inline::RawInline(
                    carta_ast::Format(format.into()),
                    normalize_code(&content, self.notes.markdown).into(),
                )));
                return;
            }
            let attr = self.take_code_attr();
            self.nodes.push(Node::Inline(Inline::Code(
                Box::new(attr),
                normalize_code(&content, self.notes.markdown).into(),
            )));
            return;
        }
        let literal = self
            .text
            .get(start..self.pos)
            .map(str::to_owned)
            .unwrap_or_default();
        self.push_str(&literal);
    }

    /// The start of the first maximal run of exactly `len` backticks at or after `from`, or `None`
    /// if the buffer holds no such run. Backed by a per-buffer index of every maximal run's start
    /// keyed by run length; the index is built once on first use and then binary-searched, so a
    /// close search costs O(log n) rather than a scan to end-of-buffer.
    fn next_backtick_run(&mut self, len: usize, from: usize) -> Option<usize> {
        let text = self.text;
        let runs = self.backtick_runs.get_or_insert_with(|| {
            let mut index: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            let bytes = text.as_bytes();
            let mut scan = 0;
            while scan < bytes.len() {
                // Backtick is ASCII: one-byte steps over non-backtick bytes never land mid-run.
                if bytes.get(scan) == Some(&b'`') {
                    let run = backtick_run_len(text, scan);
                    index.entry(run).or_default().push(scan);
                    scan += run;
                } else {
                    scan += 1;
                }
            }
            index
        });
        let positions = runs.get(&len)?;
        let at = positions.partition_point(|&p| p < from);
        positions.get(at).copied()
    }

    /// Parse `$…$` (inline) or `$$…$$` (display) TeX math at the cursor.
    ///
    /// A `$$` opener is display math, closed by the next `$$`; if no closing `$$` follows, the first
    /// `$` is literal and the second is reconsidered (it may open inline math). A single `$` opens
    /// inline math only when followed by a non-space character and closed by an unescaped `$` that is
    /// preceded by a non-space and not followed by a digit; inline content holds no unescaped `$`, so
    /// a failed first closer leaves the opener literal.
    pub(super) fn dollar_math(&mut self) {
        if self.at(1) == Some('$') {
            if let Some((content, next)) =
                crate::inline_scan::scan_display_math_bytes(self.text, self.pos)
            {
                self.pos = next;
                self.nodes.push(Node::Inline(Inline::Math(
                    MathType::DisplayMath,
                    content.into(),
                )));
                return;
            }
        } else if let Some((content, next)) =
            crate::inline_scan::scan_inline_math_bytes(self.text, self.pos)
        {
            self.pos = next;
            self.nodes.push(Node::Inline(Inline::Math(
                MathType::InlineMath,
                content.into(),
            )));
            return;
        }
        self.pos += 1;
        self.push_text('$');
    }

    pub(super) fn left_angle(&mut self) {
        if let Some((inline, next)) = scan_autolink(self.text, self.pos) {
            self.pos = next;
            // Markdown dialect classes (`uri`/`email`) and percent-encodes angle autolinks;
            // classify first, it compares shown text against the still-raw destination.
            let inline = if self.notes.markdown {
                escape_link_destination(classify_angle_autolink(inline))
            } else {
                inline
            };
            self.nodes.push(Node::Inline(inline));
            return;
        }
        if let Some((html, next)) = scan_html_tag(self.text, self.pos) {
            self.pos = next;
            // With `raw_html` off the markdown dialect keeps the recognized tag as literal text;
            // bare CommonMark always emits raw HTML.
            if self.notes.markdown && !self.ext.contains(Extension::RawHtml) {
                self.push_str(&html);
            } else {
                self.nodes.push(Node::Inline(Inline::RawInline(
                    carta_ast::Format("html".into()),
                    html.into(),
                )));
            }
            return;
        }
        self.pos += 1;
        self.push_text('<');
    }

    pub(super) fn entity(&mut self) {
        if let Some((decoded, next)) = scan_entity(self.text, self.pos) {
            self.pos = next;
            self.push_str(&decoded);
        } else {
            self.pos += 1;
            self.push_text('&');
        }
    }

    pub(super) fn line_ending(&mut self) {
        let hard = matches!(self.nodes.last(), Some(Node::Text(t)) if t.ends_with("  "));
        let backslash_hard = matches!(self.nodes.last(), Some(Node::LineBreak));
        if let Some(Node::Text(text)) = self.nodes.last_mut() {
            let keep = text.trim_end_matches(' ').len();
            text.truncate(keep);
            if text.is_empty() {
                self.nodes.pop();
            }
        }
        self.pos += 1;
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.pos += 1;
        }
        if hard || backslash_hard || self.ext.contains(Extension::HardLineBreaks) {
            self.nodes.push(Node::LineBreak);
        } else {
            self.nodes.push(Node::SoftBreak);
        }
    }

    pub(super) fn emphasis_run(&mut self, ch: u8) {
        let start = self.pos;
        while self.peek() == Some(ch as char) {
            self.pos += 1;
        }
        // The delimiter is ASCII, so the run's byte length equals its character count.
        let count = self.pos - start;
        let before = char_before(self.text, start);
        let after = self.peek();
        // With `intraword_underscores` off in the markdown dialect, `_` pairs like `*` even
        // between word characters.
        let relax_underscore =
            self.notes.markdown && !self.ext.contains(Extension::IntrawordUnderscores);
        let (can_open, can_close) = run_flanking(ch, before, after, relax_underscore);
        self.nodes.push(Node::Delimiter(Delimiter {
            ch,
            count,
            can_open,
            can_close,
            image: false,
            text_start: self.pos,
            active: false,
            cite_count_at_open: 0,
        }));
    }

    /// Replace a run of two or more `-` with em/en dashes; a lone `-` stays literal. A run folds
    /// into the fewest dashes that reproduce its length: groups of three become em dashes (`—`)
    /// and groups of two become en dashes (`–`), preferring em dashes for any odd remainder.
    pub(super) fn smart_dash(&mut self) {
        let mut len = 0;
        while self.peek() == Some('-') {
            self.pos += 1;
            len += 1;
        }
        if len == 1 {
            self.push_text('-');
            return;
        }
        let out = fold_dash_run_thirds(len);
        self.push_str(&out);
    }

    /// Replace each run of three dots with an ellipsis (`…`), leaving any remaining one or two dots
    /// literal. Dots separated by other characters are never joined.
    pub(super) fn smart_ellipsis(&mut self) {
        let mut len = 0;
        while self.peek() == Some('.') {
            self.pos += 1;
            len += 1;
        }
        let out = fold_ellipsis_run(len);
        self.push_str(&out);
    }
}
