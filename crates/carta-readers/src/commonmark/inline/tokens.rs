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

enum TexArgument {
    Braced,
    Verbatim,
    Text,
    Optional,
    ControlSequence,
}

fn raw_tex_arguments(name: &str) -> Option<&'static [TexArgument]> {
    use TexArgument::{Braced, ControlSequence, Optional, Text, Verbatim};
    match name {
        "emph" | "textbf" | "textit" | "textsl" | "textsc" | "texttt" | "textrm" | "textsf"
        | "textup" | "textmd" | "textnormal" | "underline" | "uline" | "sout" | "st"
        | "textsuperscript" | "textsubscript" | "enquote" => Some(&[Text]),
        "href" | "hyperlink" | "foreignlanguage" | "foreignquote" => Some(&[Verbatim, Text]),
        "hypertarget" => Some(&[Verbatim, Braced]),
        "texorpdfstring" => Some(&[Text, Text]),
        "textcolor" | "colorbox" => Some(&[Optional, Verbatim, Text]),
        "url" | "nolinkurl" => Some(&[Verbatim]),
        "cite" | "citep" | "citet" | "citeauthor" | "citeyear" | "citeyearpar" | "autocite"
        | "parencite" | "textcite" | "footnote" | "footnotetext" | "thanks" => {
            Some(&[Optional, Optional, Braced])
        }
        "includegraphics" => Some(&[Optional, Optional, Verbatim]),
        "footnotemark" => Some(&[Optional, Optional]),
        "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" => {
            Some(&[ControlSequence, Optional, Optional, Braced])
        }
        "newenvironment" | "renewenvironment" => {
            Some(&[Braced, Optional, Optional, Braced, Braced])
        }
        "TeX" | "LaTeX" | "ldots" | "dots" | "newline" | "textbackslash" | "textasciitilde"
        | "textasciicircum" | "textless" | "textgreater" | "textregistered" | "textcopyright"
        | "copyright" | "pounds" | "euro" | "ae" | "AE" | "oe" | "OE" | "ss" | "AA" | "aa"
        | "O" | "o" | "L" | "l" | "S" | "P" => Some(&[]),
        _ => None,
    }
}

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
            note_num: i64::from(note_num),
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
        // An in-span delimiter takes precedence over the short script form.
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
        // TeX outranks escapes; try longer openers first.
        if self.try_backslash_math() || self.try_raw_tex() {
            return;
        }
        self.pos += 1;
        // Broad escaping accepts all ASCII punctuation; Markdown otherwise uses the classic set.
        let broad = !self.notes.markdown || self.ext.contains(Extension::AllSymbolsEscapable);
        match self.peek() {
            // CommonMark always hard-breaks; Markdown needs `escaped_line_breaks`.
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

    /// Capture a raw TeX command, retaining adjacent groups for unknown names.
    fn try_raw_tex(&mut self) -> bool {
        if !self.ext.contains(Extension::RawTex) {
            return false;
        }
        let Some(end) = self.scan_raw_tex_command(self.pos) else {
            return false;
        };
        let Some(source) = self.text.get(self.pos..end) else {
            return false;
        };
        self.nodes.push(Node::Inline(Inline::RawInline(
            carta_ast::Format("tex".into()),
            source.into(),
        )));
        self.pos = end;
        true
    }

    fn scan_raw_tex_command(&self, start: usize) -> Option<usize> {
        carta_core::with_stack(|| self.scan_raw_tex_command_nested(start))
    }

    fn scan_raw_tex_command_nested(&self, start: usize) -> Option<usize> {
        if char_at(self.text, start) != Some('\\') {
            return None;
        }
        let mut position = start + 1;
        if !char_at(self.text, position).is_some_and(|ch| ch.is_ascii_alphabetic()) {
            return None;
        }
        let mut letters_only = true;
        while let Some(character) = char_at(self.text, position) {
            if !character.is_ascii_alphanumeric() {
                break;
            }
            letters_only &= character.is_ascii_alphabetic();
            position += 1;
        }
        let name = self.text.get(start + 1..position)?;
        if name == "begin" {
            if char_at(self.text, position) != Some('{') {
                return None;
            }
            return self.scan_environment_close(self.scan_balanced_group(position)?);
        }
        if name == "end" {
            return None;
        }
        if char_at(self.text, position) == Some('*') {
            position += 1;
        }
        let name_end = position;
        if let Some(arguments) = raw_tex_arguments(name) {
            for argument in arguments {
                let argument_start = match argument {
                    TexArgument::Verbatim => self.tex_horizontal_start(position),
                    _ => self.tex_argument_start(position),
                };
                match argument {
                    TexArgument::Optional => {
                        if char_at(self.text, argument_start) == Some('[') {
                            position = self.scan_balanced_group(argument_start)?;
                        }
                    }
                    TexArgument::Braced | TexArgument::Verbatim => {
                        if char_at(self.text, argument_start) != Some('{') {
                            return None;
                        }
                        position = self.scan_balanced_group(argument_start)?;
                    }
                    TexArgument::Text => position = self.scan_tex_text_argument(argument_start)?,
                    TexArgument::ControlSequence => {
                        position = self.scan_tex_control_argument(argument_start)?;
                    }
                }
            }
        } else {
            let mut argument_start = if letters_only {
                self.tex_horizontal_start(position)
            } else {
                position
            };
            while char_at(self.text, argument_start) == Some('[') {
                let Some(end) = self.scan_balanced_group(argument_start) else {
                    break;
                };
                position = end;
                argument_start = self.tex_horizontal_start(end);
            }
            while char_at(self.text, argument_start) == Some('{') {
                position = self.scan_balanced_group(argument_start)?;
                argument_start = position;
            }
        }
        if position == name_end && letters_only {
            position = self.tex_horizontal_start(position);
        }
        Some(position)
    }

    fn tex_horizontal_start(&self, mut position: usize) -> usize {
        while matches!(char_at(self.text, position), Some(' ' | '\t')) {
            position += 1;
        }
        position
    }

    fn tex_argument_start(&self, mut position: usize) -> usize {
        while matches!(char_at(self.text, position), Some(' ' | '\t' | '\n' | '\r')) {
            position += 1;
        }
        position
    }

    fn scan_tex_text_argument(&self, start: usize) -> Option<usize> {
        let character = char_at(self.text, start)?;
        match character {
            '{' => self.scan_balanced_group(start),
            '}' => None,
            '\\' if char_at(self.text, start + 1).is_some_and(|ch| ch.is_ascii_alphabetic()) => {
                self.scan_raw_tex_command(start)
            }
            '\\' => self.scan_tex_control_sequence(start),
            _ => Some(start + character.len_utf8()),
        }
    }

    fn scan_tex_control_argument(&self, start: usize) -> Option<usize> {
        if char_at(self.text, start) == Some('{') {
            let end = self.scan_balanced_group(start)?;
            let control = self.tex_argument_start(start + 1);
            let after = self.scan_tex_control_sequence(control)?;
            (self.tex_argument_start(after) == end - 1).then_some(end)
        } else {
            self.scan_tex_control_sequence(start)
        }
    }

    fn scan_tex_control_sequence(&self, start: usize) -> Option<usize> {
        if char_at(self.text, start) != Some('\\') {
            return None;
        }
        let character = char_at(self.text, start + 1)?;
        let mut end = start + 1 + character.len_utf8();
        if character.is_ascii_alphabetic() {
            while char_at(self.text, end).is_some_and(|ch| ch.is_ascii_alphabetic()) {
                end += 1;
            }
        }
        Some(end)
    }

    /// Find the closing marker for the environment whose opening group ends at `from`.
    fn scan_environment_close(&self, from: usize) -> Option<usize> {
        self.raw_tex_environments
            .get_or_init(|| {
                let groups = self.raw_tex_group_ends();
                let mut openings: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
                let mut ends = BTreeMap::new();
                for (position, _) in self.text.match_indices('\\') {
                    let tail = self.text.get(position..).unwrap_or_default();
                    let (opening, group) = if tail.starts_with("\\begin{") {
                        (true, position + 6)
                    } else if tail.starts_with("\\end{") {
                        (false, position + 4)
                    } else {
                        continue;
                    };
                    let Some(&end) = groups.get(&group) else {
                        continue;
                    };
                    let Some(name) = self.text.get(group + 1..end - 1) else {
                        continue;
                    };
                    if opening {
                        openings.entry(name).or_default().push(end);
                    } else if let Some(open) = openings.get_mut(name).and_then(Vec::pop) {
                        ends.insert(open, end);
                    }
                }
                ends
            })
            .get(&from)
            .copied()
    }

    fn raw_tex_group_ends(&self) -> &BTreeMap<usize, usize> {
        self.raw_tex_groups.get_or_init(|| {
            let mut braces = Vec::new();
            let mut brackets = Vec::new();
            let mut ends = BTreeMap::new();
            for (position, byte) in self.text.bytes().enumerate() {
                let stack = match byte {
                    b'{' | b'}' => &mut braces,
                    b'[' | b']' => &mut brackets,
                    _ => continue,
                };
                if matches!(byte, b'{' | b'[') {
                    stack.push(position);
                } else if let Some(open) = stack.pop() {
                    ends.insert(open, position + 1);
                }
            }
            ends
        })
    }

    /// Find the end of a brace or bracket group at `start`.
    fn scan_balanced_group(&self, start: usize) -> Option<usize> {
        self.raw_tex_group_ends().get(&start).copied()
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
            // Classify against the raw destination before percent-encoding it.
            let inline = if self.notes.markdown {
                escape_link_destination(classify_angle_autolink(inline))
            } else {
                inline
            };
            self.nodes.push(Node::Inline(inline));
            return;
        }
        let rest = self.text.get(self.pos..).unwrap_or("");
        let declaration =
            self.notes.markdown && rest.starts_with("<!") && !rest.starts_with("<!--");
        let ending = if rest.starts_with("<!--")
            && !rest.starts_with("<!-->")
            && !rest.starts_with("<!--->")
        {
            Some((0, 4))
        } else if rest.starts_with("<?") {
            Some((1, 2))
        } else if rest.starts_with("<![CDATA[") {
            Some((2, 9))
        } else if rest.starts_with("<!") {
            Some((3, 2))
        } else {
            None
        };
        let closes = ending.is_none_or(|(index, prefix)| {
            self.html_closing
                .get_or_init(|| {
                    [
                        self.text.rfind("-->"),
                        self.text.rfind("?>"),
                        self.text.rfind("]]>"),
                        self.text.rfind('>'),
                    ]
                })
                .get(index)
                .copied()
                .flatten()
                .is_some_and(|last| last >= self.pos + prefix)
        });
        if !declaration
            && closes
            && let Some((html, next)) = scan_html_tag(self.text, self.pos)
        {
            self.pos = next;
            // With `raw_html` off, Markdown keeps tags literal; CommonMark emits raw HTML.
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
        let script = self.notes.markdown
            && (ch == b'^'
                || (ch == b'~'
                    && self.ext.contains(Extension::Subscript)
                    && !self.ext.contains(Extension::Strikeout)));
        while self.peek() == Some(ch as char) {
            self.pos += 1;
            if script {
                break;
            }
        }
        let count = self.pos - start;
        let before = char_before(self.text, start);
        let after = self.peek();
        // With `intraword_underscores` off, Markdown pairs `_` like `*` inside words.
        let relax_underscore =
            self.notes.markdown && !self.ext.contains(Extension::IntrawordUnderscores);
        let (can_open, can_close) = run_flanking(ch, before, after, relax_underscore);
        if script
            && matches!(self.nodes.last(), Some(Node::Delimiter(delimiter)) if delimiter.ch == ch)
        {
            // Leave a slot for an empty script between adjacent delimiters.
            self.nodes.push(Node::Empty);
        }
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
    /// into the fewest dashes that reproduce its length: groups of three become em dashes (U+2014)
    /// and groups of two become en dashes (U+2013), preferring em dashes for any odd remainder.
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

    /// Fold ellipses and bind recognized Markdown abbreviations to the next word.
    pub(super) fn smart_period(&mut self) {
        let mut len = 0;
        while self.peek() == Some('.') {
            self.pos += 1;
            len += 1;
        }
        let out = fold_ellipsis_run(len);
        self.push_str(&out);
        if len != 1 || !self.notes.markdown || !matches!(self.peek(), Some(' ' | '\t')) {
            return;
        }
        let Some(Node::Text(text)) = self.nodes.last() else {
            return;
        };
        // Bound lookback so repeated periods cannot rescan the whole text run.
        let tail = text
            .get(text.floor_char_boundary(text.len().saturating_sub(8))..)
            .unwrap_or("");
        let word = tail
            .rsplit(|ch: char| !ch.is_alphanumeric() && ch != '.')
            .next()
            .unwrap_or("");
        if ![
            "p.", "pp.", "ch.", "chap.", "sec.", "fig.", "vol.", "n.", "nn.", "pt.", "ed.", "eds.",
            "cf.", "vs.", "al.", "e.g.", "i.e.", "viz.", "Mr.", "Mrs.", "Ms.", "Dr.", "Prof.",
            "Rev.", "St.", "Jr.", "Sr.", "Jan.", "Feb.", "Mar.", "Apr.", "Aug.", "Sept.", "Sep.",
            "Oct.", "Nov.", "Dec.", "Ph.D.", "Inc.", "Ltd.", "No.",
        ]
        .contains(&word)
        {
            return;
        }
        let start = self.pos;
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.pos += 1;
        }
        if matches!(self.peek(), Some('\n' | '\r')) {
            self.pos = start;
        } else if self.pos > start {
            self.push_text('\u{a0}');
        }
    }
}
