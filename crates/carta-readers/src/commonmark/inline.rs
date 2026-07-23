//! Inline phase: parse the raw text of leaf blocks into inline nodes.
//!
//! Implements the spec's inline algorithm — a left-to-right scan that resolves code spans,
//! autolinks, raw HTML, entities and escapes immediately, records `*`/`_`/`[`/`![` runs on a
//! delimiter stack, resolves links/images at each `]`, and finally collapses emphasis. The raw
//! char-slice scanners it drives (autolinks, HTML tags, entities, link targets) live in `scan`.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{Attr, Citation, CitationMode, Inline, MathType, Target, Text};
use carta_core::{Extension, Extensions};

use super::attr;
use super::emphasis::{delimiter_literal, process_emphasis, resolve_mark, run_flanking};
use super::postprocess::is_format_name_char;
use super::resolve::RefContext;
use super::scan::{
    char_at, escape_uri, is_ascii_punctuation, normalize_label, scan_autolink, scan_entity,
    scan_following_label, scan_html_tag, scan_inline_target, unescape_string,
};
use super::{ExampleMap, LinkDef, RefMap, para};
use crate::emoji;
use crate::inline_scan::is_unicode_whitespace;
use crate::smart_fold::{fold_dash_run_thirds, fold_ellipsis_run};

/// A node in the in-progress inline list. Delimiter runs stay as nodes until emphasis resolution.
#[derive(Debug, Clone)]
pub(super) enum Node {
    Text(String),
    Inline(Inline),
    SoftBreak,
    LineBreak,
    Delimiter(Delimiter),
    /// A slot vacated during emphasis resolution: its former content was folded into a wrapping
    /// inline. Emitted by [`process_emphasis`] and dropped by [`collapse`].
    Empty,
}

// The flags are independent properties of a delimiter run, not a state enum.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub(super) struct Delimiter {
    pub(super) ch: u8,
    pub(super) count: usize,
    pub(super) can_open: bool,
    pub(super) can_close: bool,
    /// Whether this is an image opener (`![`).
    pub(super) image: bool,
    /// Source index just past a bracket opener, where its raw label text begins. Unused otherwise.
    pub(super) text_start: usize,
    /// Whether this bracket opener is still eligible to form a link or image. Non-bracket
    /// delimiters leave this `false` (the field is unused for them).
    ///
    /// A `[` opener is deactivated when a link is successfully built whose text span contains
    /// it — a link may not contain another link. On `]`, an inactive opener is popped and
    /// literalized without attempting any link-target parse (spec §6.3, rule 6).
    pub(super) active: bool,
    /// The citation count at the moment this bracket opened. If the bracket later resolves to a
    /// single citation, any bare citations counted while scanning its interior are discarded along
    /// with their nodes, so the count rewinds to this value first. Unused for non-bracket
    /// delimiters.
    pub(super) cite_count_at_open: i32,
}

/// Outcome of resolving an explicit link target after a closing `]`.
enum Explicit {
    /// An inline or reference target resolved to this destination, ending at the given position.
    Target(Target, usize),
    /// An explicit reference was present but its label is undefined: not a link.
    Failed,
    /// No explicit target syntax follows; a span or shortcut reference may still apply.
    None,
}

// `notes` (the footnote context) and `nodes` (the in-progress inline list) are distinct concepts
// that unavoidably read alike.
/// Run the gated highlight-mark pass and emphasis resolution over a node list, then collapse it into
/// inlines — the shared finishing sequence for a parsed inline run, a span body, and a link label.
fn resolve_inline_nodes(mut nodes: Vec<Node>, ext: Extensions, markdown: bool) -> Vec<Inline> {
    if ext.contains(Extension::Mark) {
        resolve_mark(&mut nodes, ext, markdown);
    }
    process_emphasis(&mut nodes, 0, ext, markdown);
    collapse(nodes)
}

/// The character ending just before byte offset `at`, or `None` at the start of `text`.
pub(super) fn char_before(text: &str, at: usize) -> Option<char> {
    text.get(..at).and_then(|head| head.chars().next_back())
}

#[allow(clippy::similar_names)]
pub(super) fn parse_inlines(
    text: &str,
    refs: &RefMap,
    notes: RefContext,
    ext: Extensions,
) -> Vec<Inline> {
    let mut parser = InlineParser {
        text,
        pos: 0,
        nodes: Vec::new(),
        refs,
        notes,
        ext,
        bracket_stack: Vec::new(),
        interesting: interesting_chars(ext),
        backtick_runs: None,
        raw_tex_budget: text.len().saturating_mul(8).saturating_add(64),
        last_brace: std::cell::OnceCell::new(),
        last_bracket: std::cell::OnceCell::new(),
        env_last_close: BTreeMap::new(),
    };
    parser.run();
    let mut inlines = resolve_inline_nodes(parser.nodes, ext, notes.markdown);
    if ext.contains(Extension::Autolink) {
        super::autolink::autolink_inlines(&mut inlines, notes.markdown);
    }
    if ext.contains(Extension::NativeSpans) {
        inlines = pair_native_spans(inlines);
    }
    inlines
}

/// Parse standalone text — a document metadata value — into inlines with no reference context, so
/// footnote and example references in the text resolve to nothing. `markdown` selects the markdown
/// dialect's inline rules, matching the dialect the document body is parsed under.
pub(crate) fn parse_meta_inlines(text: &str, ext: Extensions, markdown: bool) -> Vec<Inline> {
    let defined = BTreeSet::new();
    let by_id = BTreeMap::new();
    let examples = ExampleMap::new();
    let refs = RefMap::new();
    let cite_count = Cell::new(0);
    let notes = RefContext {
        defined: &defined,
        by_id: &by_id,
        in_definition: false,
        markdown,
        examples: &examples,
        cite_count: &cite_count,
    };
    parse_inlines(text, &refs, notes, ext)
}

/// A link label may hold at most 999 characters between its brackets (`CommonMark`, "Links"). A
/// UTF-8 character is at most four bytes, so a span longer than this in bytes exceeds that
/// character bound. Reference lookups treat such a span as no label at all — skipping extraction,
/// normalization, and the map lookup — regardless of what definitions exist, which also keeps a
/// close's label work bounded on adversarially nested brackets.
const MAX_LABEL_BYTES: usize = 999 * 4;

struct InlineParser<'a> {
    text: &'a str,
    /// Cursor as a byte offset into `text`; every mutation lands on a UTF-8 character boundary.
    pos: usize,
    nodes: Vec<Node>,
    refs: &'a RefMap,
    notes: RefContext<'a>,
    ext: Extensions,
    /// Indices into `nodes` for each open `[` or `![` delimiter, in parse order. O(1) lookup of
    /// the most recent bracket opener instead of a backward scan through all nodes.
    bracket_stack: Vec<usize>,
    /// For each ASCII code, whether a character can start a syntactic construct under the active
    /// extensions. Everything else is ordinary text a run scan can skip over in one step.
    interesting: [bool; 128],
    /// Byte offsets of every maximal backtick run in `text`, grouped by run length and ascending
    /// within each group. Built lazily on the first code-span attempt (`None` until then, so
    /// backtick-free text pays nothing) and immutable afterwards: it stays valid because `text`
    /// never changes during inline parsing. A feature that mutated the buffer mid-parse would have
    /// to rebuild it.
    backtick_runs: Option<BTreeMap<usize, Vec<usize>>>,
    /// Remaining work budget shared by all raw-TeX look-ahead scans in this buffer. Seeded from the
    /// buffer length and charged the traversal length of each scan; when it reaches zero a raw-TeX
    /// attempt reverts (the backslash stays literal — the same outcome an unmatched group already
    /// produces). It caps the total look-ahead at O(n) so a run of never-closing openers cannot cost
    /// O(n²); a genuine document, where each group is scanned once, never approaches the ceiling.
    raw_tex_budget: usize,
    /// Byte offset of the last `}` / last `]` in `text` (inner `None` when the delimiter is absent),
    /// computed on the first raw-TeX group scan so buffers that never reach one skip the search. A
    /// group scan starting past its closing delimiter's final occurrence cannot balance, so it fails
    /// in O(1) without touching the budget.
    last_brace: std::cell::OnceCell<Option<usize>>,
    last_bracket: std::cell::OnceCell<Option<usize>>,
    /// Per environment name, the start offset of the last `\end{NAME}` marker in `text` (inner `None`
    /// when there is none). Scanning for an environment close past this offset cannot succeed.
    env_last_close: BTreeMap<String, Option<usize>>,
}

/// The ASCII characters that can begin an inline construct under `ext`; every other character is
/// ordinary text. Must stay in lockstep with the dispatch arms in [`InlineParser::run`] — any new
/// arm's trigger character has to be marked here, or the construct becomes unreachable mid-text.
fn interesting_chars(ext: Extensions) -> [bool; 128] {
    let smart = ext.contains(Extension::Smart);
    core::array::from_fn(|code| {
        let Ok(byte) = u8::try_from(code) else {
            return false;
        };
        match char::from(byte) {
            '\\' | '`' | '<' | '&' | '\n' | '*' | '_' | '[' | ']' | '!' => true,
            '$' => ext.contains(Extension::TexMathDollars),
            '~' => ext.contains(Extension::Subscript) || ext.contains(Extension::Strikeout),
            '^' => ext.contains(Extension::InlineNotes) || ext.contains(Extension::Superscript),
            '=' => ext.contains(Extension::Mark),
            '@' => ext.contains(Extension::ExampleLists) || ext.contains(Extension::Citations),
            ':' => ext.contains(Extension::Emoji),
            '\'' | '"' | '-' | '.' => smart,
            _ => false,
        }
    })
}

impl<'a> InlineParser<'a> {
    fn peek(&self) -> Option<char> {
        char_at(self.text, self.pos)
    }

    fn at(&self, offset: usize) -> Option<char> {
        self.text
            .get(self.pos..)
            .and_then(|rest| rest.chars().nth(offset))
    }

    fn is_interesting(&self, ch: char) -> bool {
        usize::try_from(u32::from(ch))
            .ok()
            .and_then(|code| self.interesting.get(code))
            .copied()
            .unwrap_or(false)
    }

    /// Whether the byte at `pos` begins an interesting construct. Every interesting trigger is an
    /// ASCII character, so a byte `>= 128` (any part of a multi-byte character) is never a boundary
    /// this scan stops on — the run scan can advance one byte at a time and still break only on a
    /// character boundary.
    fn interesting_byte(&self, byte: u8) -> bool {
        byte < 128
            && self
                .interesting
                .get(usize::from(byte))
                .copied()
                .unwrap_or(false)
    }

    fn run(&mut self) {
        while let Some(ch) = self.peek() {
            if !self.is_interesting(ch) {
                let start = self.pos;
                let bytes = self.text.as_bytes();
                while let Some(&next) = bytes.get(self.pos) {
                    if self.interesting_byte(next) {
                        break;
                    }
                    self.pos += 1;
                }
                if let Some(run) = self.text.get(start..self.pos) {
                    self.push_str(run);
                }
                continue;
            }
            match ch {
                '\\' => self.backslash(),
                '`' => self.code_span(),
                '$' if self.ext.contains(Extension::TexMathDollars) => self.dollar_math(),
                '<' => self.left_angle(),
                '&' => self.entity(),
                '\n' => self.line_ending(),
                '*' | '_' => self.emphasis_run(ch as u8),
                '~' if self.ext.contains(Extension::Subscript)
                    && self.ext.contains(Extension::ShortSubsuperscripts)
                    && self.try_short_script('~') => {}
                '~' if self.ext.contains(Extension::Subscript)
                    || self.ext.contains(Extension::Strikeout) =>
                {
                    self.emphasis_run(b'~');
                }
                '^' if self.ext.contains(Extension::InlineNotes)
                    && self.at(1) == Some('[')
                    && self.try_inline_note() => {}
                '^' if self.ext.contains(Extension::Superscript)
                    && self.ext.contains(Extension::ShortSubsuperscripts)
                    && self.try_short_script('^') => {}
                '^' if self.ext.contains(Extension::Superscript) => self.emphasis_run(b'^'),
                '=' if self.ext.contains(Extension::Mark) => self.emphasis_run(b'='),
                '@' if self.ext.contains(Extension::ExampleLists)
                    || self.ext.contains(Extension::Citations) =>
                {
                    self.at_sign();
                }
                ':' if self.ext.contains(Extension::Emoji) && self.try_emoji() => {}
                '\'' | '"' if self.ext.contains(Extension::Smart) => self.emphasis_run(ch as u8),
                '-' if self.ext.contains(Extension::Smart) => self.smart_dash(),
                '.' if self.ext.contains(Extension::Smart) => self.smart_ellipsis(),
                '[' => {
                    self.pos += 1;
                    self.push_open_bracket(false);
                }
                '!' if self.at(1) == Some('[') => {
                    self.pos += 2;
                    self.push_open_bracket(true);
                }
                ']' => self.close_bracket(),
                _ => {
                    self.pos += ch.len_utf8();
                    self.push_text(ch);
                }
            }
        }
    }

    fn push_text(&mut self, ch: char) {
        if let Some(Node::Text(text)) = self.nodes.last_mut() {
            text.push(ch);
        } else {
            self.nodes.push(Node::Text(ch.to_string()));
        }
    }

    fn push_str(&mut self, value: &str) {
        if let Some(Node::Text(text)) = self.nodes.last_mut() {
            text.push_str(value);
        } else {
            self.nodes.push(Node::Text(value.to_owned()));
        }
    }

    /// Resolve an `@` at the cursor. An example-list label assigned a number becomes that number; a
    /// well-formed citation key becomes a bare author-in-text `Cite`; anything else leaves the `@`
    /// as literal text, so the rest of the run reparses normally.
    fn at_sign(&mut self) {
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
        // The `@` and every label character (`[0-9A-Za-z_-]`) are ASCII, so byte and character
        // offsets coincide across the label.
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
    fn bump_cite_count(&self) -> i32 {
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
    fn try_emoji(&mut self) -> bool {
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
    /// only to an unpaired opener — an even number of matching delimiters precede it in the span and
    /// none follow. An empty alphanumeric run (a delimiter met by a non-alphanumeric or the line's
    /// end) is not a script.
    fn try_short_script(&mut self, delimiter: char) -> bool {
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
        // An opener pairs into the delimited form when another matching delimiter follows in-span.
        // The delimiter (`~`/`^`) is ASCII, so the span begins one byte past the cursor.
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

    fn backslash(&mut self) {
        // Backslash-delimited TeX math and raw TeX commands take precedence over a plain escape,
        // each gated behind its own extension. `\\(`/`\\[` (double backslash) is tried before
        // `\(`/`\[` (single backslash) so the longer opener wins.
        if self.try_backslash_math() || self.try_raw_tex() {
            return;
        }
        self.pos += 1;
        // With `all_symbols_escapable` — and always in the bare CommonMark engine — a backslash
        // escapes any ASCII punctuation character (and, in the markdown dialect, turns a following
        // space into a non-breaking space). Without it a markdown dialect escapes only the classic
        // set and leaves every other backslash literal.
        let broad = !self.notes.markdown || self.ext.contains(Extension::AllSymbolsEscapable);
        match self.peek() {
            // In the broad Markdown dialect a backslash before a line ending is a hard break only
            // when `escaped_line_breaks` is on; with it off the backslash is literal and the line
            // ending is an ordinary soft break. The bare CommonMark engine always hard-breaks here.
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
            // In the markdown dialect a backslash before a space is a non-breaking space, which binds
            // into the surrounding text rather than splitting it on whitespace.
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
    /// balance-closed reverts the whole command to literal text; an unclosable `[`-group simply ends
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
        // The leading `\` and the command name (ASCII letters and digits) are single-byte, so
        // byte and character offsets coincide up to `i`.
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
        // `\begin`/`\end` are raw TeX only as a complete, matched environment, never as bare
        // commands: capture the whole `\begin{ENV}`…`\end{ENV}`, or leave the text literal.
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

        // A bare command whose name is all letters (no argument groups, no digits) absorbs a
        // trailing run of spaces and tabs.
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
        // `group_end` sits just past the closing `}` (ASCII), so `group_end - 1` is its byte offset
        // and `name_end + 1` the byte past the opening `{`.
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
        // A close cannot lie past the last `\end{ENV}` marker in the buffer; a scan starting beyond it
        // fails without walking. Exact — the depth counter only delays accepting an existing close, it
        // never conjures one, so the last marker's position bounds every possible close.
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
        // A group opened past the last close delimiter in the buffer can never balance; fail in O(1)
        // without charging the budget. Exact: closing the group requires a `close` at some later
        // offset, and none exists beyond this one. Only `}` and `]` arise as raw-TeX group closers.
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

    fn code_span(&mut self) {
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
        // No closing run: emit the opening backticks literally.
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
                // The backtick is ASCII, so advancing one byte over any non-backtick byte (including
                // every byte of a multi-byte character) never lands mid-run or records a false start.
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
    fn dollar_math(&mut self) {
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

    fn left_angle(&mut self) {
        if let Some((inline, next)) = scan_autolink(self.text, self.pos) {
            self.pos = next;
            // The markdown dialect tags an explicit angle autolink with a `uri` or `email` class
            // and percent-encodes its destination; the strict dialect leaves it unclassed and
            // verbatim. The destination is encoded after classification, which compares the shown
            // text against the still-raw destination to tell a `uri` from an `email`.
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
            // In the Markdown dialect with `raw_html` off the tag is still recognized as a unit but
            // kept as literal text rather than a passthrough span. The bare CommonMark engine always
            // emits raw HTML, since HTML is part of its core grammar.
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

    fn entity(&mut self) {
        if let Some((decoded, next)) = scan_entity(self.text, self.pos) {
            self.pos = next;
            self.push_str(&decoded);
        } else {
            self.pos += 1;
            self.push_text('&');
        }
    }

    fn line_ending(&mut self) {
        // Trailing spaces before the newline determine hard vs soft break.
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
        // Skip leading spaces/tabs of the next line.
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.pos += 1;
        }
        if hard || backslash_hard || self.ext.contains(Extension::HardLineBreaks) {
            self.nodes.push(Node::LineBreak);
        } else {
            self.nodes.push(Node::SoftBreak);
        }
    }

    fn emphasis_run(&mut self, ch: u8) {
        let start = self.pos;
        while self.peek() == Some(ch as char) {
            self.pos += 1;
        }
        // The delimiter is ASCII, so the run's byte length equals its character count.
        let count = self.pos - start;
        let before = char_before(self.text, start);
        let after = self.peek();
        // With `intraword_underscores` off in the Markdown dialect, a `_` run pairs like `*`,
        // emphasizing even between word characters; otherwise the CommonMark `_` rule keeps an
        // intraword run inert.
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
    fn smart_dash(&mut self) {
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
    fn smart_ellipsis(&mut self) {
        let mut len = 0;
        while self.peek() == Some('.') {
            self.pos += 1;
            len += 1;
        }
        let out = fold_ellipsis_run(len);
        self.push_str(&out);
    }

    fn push_open_bracket(&mut self, image: bool) {
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

    fn close_bracket(&mut self) {
        self.pos += 1;
        let Some(&opener_index) = self.bracket_stack.last() else {
            self.push_text(']');
            return;
        };
        let (is_image, is_active) = match self.nodes.get(opener_index) {
            Some(Node::Delimiter(d)) => (d.image, d.active),
            _ => (false, false),
        };

        // A defined footnote reference `[^label]` wins over every other use of the brackets: it
        // consumes nothing past the `]` and ignores any following inline target or reference.
        if is_active
            && self.ext.contains(Extension::Footnotes)
            && self.try_footnote(opener_index, is_image)
        {
            return;
        }

        // An active opener may form a link or image from an explicit inline `(...)` target or an
        // explicit `[label]`/`[]` reference. (An inactive `[` cannot — a link may not contain
        // another link, spec §6.3 rule 6 — but it may still open a bracketed span.)
        if is_active {
            match self.resolve_explicit(opener_index) {
                Explicit::Target(target, next) => {
                    self.finish_link(opener_index, is_image, target, next);
                    return;
                }
                // An explicit reference whose label is undefined is not a link; the brackets stay
                // literal and no span or shortcut fallback is tried.
                Explicit::Failed => {
                    self.bracket_stack.pop();
                    self.literalize_bracket(opener_index);
                    self.push_text(']');
                    return;
                }
                Explicit::None => {}
            }
        }

        // With no explicit target, a non-image bracket directly followed by a non-empty attribute
        // block is a span — this wins over a shortcut reference of the same label.
        if !is_image
            && self.ext.contains(Extension::BracketedSpans)
            && let Some((attr, next)) = self.scan_attr_block()
        {
            self.bracket_stack.pop();
            self.pos = next;
            self.build_span(opener_index, attr);
            return;
        }

        // A shortcut reference: the bracket's own text names the definition. Skip the whole lookup
        // when no definitions exist or the span is too long to be a label.
        if is_active && !self.refs.is_empty() {
            let raw = self.raw_label(opener_index);
            if raw.len() <= MAX_LABEL_BYTES
                && let Some(target) = self.refs.get(&normalize_label(raw)).map(def_target)
            {
                self.finish_link(opener_index, is_image, target, self.pos);
                return;
            }
        }

        // A bracket whose content is a well-formed citation list becomes a `Cite`. An image's `!`
        // survives as literal text before it.
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
        // A citation list must carry at least one `@key`; without an `@` the span cannot be one, so
        // skip the segment scan entirely.
        if !raw.as_bytes().contains(&b'@') {
            return false;
        }
        let Some(segments) = split_citation_segments(raw) else {
            return false;
        };
        // Scanning the interior may have counted bare citations that are about to be discarded with
        // their nodes; rewind to the count this bracket opened with before numbering the group. (For
        // the rare `![@key]`, the discarded interior count is not added back, so such a group's
        // number is one lower than a longer document with the same citation order would otherwise
        // give it.)
        if let Some(Node::Delimiter(d)) = self.nodes.get(opener_index) {
            self.notes.cite_count.set(d.cite_count_at_open);
        }
        // Reserve this group's number before parsing affixes, so nested citations are counted after
        // it and the group ends up stamped with the highest number it contains.
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
        // The prefix is trimmed of surrounding whitespace; the suffix keeps any leading space (so
        // `@a x` separates the key from `x`) but drops trailing space.
        //
        // A suffix opening with a locator label such as `p.` or `vol.` carries a non-breaking space
        // before its number (`p.\u{a0}5`). That join, gated on a fixed set of abbreviations, is not
        // applied here: the suffix is tokenized as ordinary inlines, so `p. 5` stays three tokens.
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
    fn scan_raw_format(&self) -> Option<(String, usize)> {
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
    fn take_code_attr(&mut self) -> Attr {
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
        self.nodes.pop(); // remove the opener delimiter
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
            // The markdown dialect lets an unbracketed destination hold spaces and balanced
            // parentheses; the strict dialect ends a destination at the first space.
            let scanned = if self.notes.markdown {
                scan_markdown_inline_target(self.text, self.pos)
            } else {
                scan_inline_target(self.text, self.pos)
            };
            if let Some((target, next)) = scanned {
                return Explicit::Target(target, next);
            }
        }
        // Explicit reference. Labels match on their raw source text (the closing `]` sits at `pos - 1`).
        // With `spaced_reference_links`, whitespace may separate the text bracket from the reference
        // label bracket — `[text] [ref]` and `[text]\n[ref]` — though not from the inline `(...)`
        // target handled above.
        let mut label_start = self.pos;
        if self.ext.contains(Extension::SpacedReferenceLinks) {
            while matches!(char_at(self.text, label_start), Some(' ' | '\t' | '\n')) {
                label_start += 1;
            }
        }
        if let Some((label, next)) = scan_following_label(self.text, label_start) {
            // An explicit reference with no definitions in scope can never resolve, so the brackets
            // stay literal without extracting or normalizing the label.
            if self.refs.is_empty() {
                return Explicit::Failed;
            }
            let key = if label.is_empty() {
                // A collapsed reference is keyed on the bracket's own span, which is unbounded
                // source text; past the label limit it is no label at all.
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
    fn try_inline_note(&mut self) -> bool {
        // self.pos is the caret; the `[` sits at self.pos + 1 (the `^` is ASCII). Walk forward
        // tracking bracket depth.
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
        // The bracket content lies between `[` (at self.pos + 1) and the closing `]` (ASCII, at
        // end - 1).
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
        // The markdown dialect percent-encodes a destination's unsafe characters; the strict
        // CommonMark and GitHub dialects keep it verbatim.
        if self.notes.markdown {
            target.url = escape_uri(&target.url).into();
        }
        let inner: Vec<Node> = self.nodes.split_off(opener_index + 1);
        self.nodes.pop(); // remove the opener delimiter
        // Any bracket stack entries that pointed into the split-off range are now part of the
        // inner node list passed to emphasis resolution; they no longer belong to the outer parse.
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

/// A character that may appear directly before `@` and block a bare citation: an alphanumeric
/// glues the `@` to a preceding word (`foo@bar`, an email-like run), so no citation forms there.
fn is_citation_word(ch: char) -> bool {
    ch.is_alphanumeric()
}

/// Tokenize raw citation source into the literal inlines that stand in for the citation: whitespace
/// runs become `SoftBreak` (when they hold a newline) or `Space`, and every other run becomes a
/// `Str`. No inline markup is interpreted, so the source reads back verbatim word by word.
fn citation_fallback_inlines(raw: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut had_space = false;
    let mut had_newline = false;
    let flush_word = |out: &mut Vec<Inline>, word: &mut String| {
        if !word.is_empty() {
            out.push(Inline::Str(std::mem::take(word).into()));
        }
    };
    for ch in raw.chars() {
        if ch.is_whitespace() {
            flush_word(&mut out, &mut word);
            had_space = true;
            had_newline |= ch == '\n';
        } else {
            if had_space {
                out.push(if had_newline {
                    Inline::SoftBreak
                } else {
                    Inline::Space
                });
                had_space = false;
                had_newline = false;
            }
            word.push(ch);
        }
    }
    flush_word(&mut out, &mut word);
    if had_space {
        out.push(if had_newline {
            Inline::SoftBreak
        } else {
            Inline::Space
        });
    }
    out
}

/// A character that always belongs to a citation key: an alphanumeric or `_`. A key begins with one
/// of these.
fn is_citation_key_start(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Scan a citation key beginning at `start` (the index just past `@`). A key opens with an
/// alphanumeric or `_` and runs over further such characters; the internal punctuation `-`, `.`,
/// `:`, and `/` extend it only when another key character follows, so a trailing `-key.` keeps the
/// `key` but drops the `.`. Returns the key text and the index just past it, or `None` when no key
/// begins at `start`.
fn scan_citation_id(text: &str, start: usize) -> Option<(String, usize)> {
    let first = char_at(text, start)?;
    if !is_citation_key_start(first) {
        return None;
    }
    let mut end = start + first.len_utf8();
    while let Some(ch) = char_at(text, end) {
        if is_citation_key_start(ch) {
            end += ch.len_utf8();
        } else if matches!(ch, '-' | '.' | ':' | '/')
            // The internal punctuation is ASCII, so the following key character begins one byte on.
            && matches!(char_at(text, end + 1), Some(next) if is_citation_key_start(next))
        {
            end += 1 + char_at(text, end + 1).map_or(0, char::len_utf8);
        } else {
            break;
        }
    }
    let id = text.get(start..end)?.to_owned();
    Some((id, end))
}

/// Advance past one escape, backtick code span, or bracket at `index`, updating bracket `depth` and
/// returning the next index. Returns `None` when the character is none of those — the caller then
/// inspects it for a top-level delimiter (`;` or `@`) and advances itself.
fn step_citation_scan(text: &str, index: usize, depth: &mut usize) -> Option<usize> {
    match char_at(text, index) {
        Some('\\') => Some(index + 1 + char_at(text, index + 1).map_or(0, char::len_utf8)),
        Some('`') => {
            let run = backtick_run_len(text, index);
            Some(skip_code_span(text, index, run))
        }
        Some('[') => {
            *depth += 1;
            Some(index + 1)
        }
        Some(']') => {
            *depth = depth.saturating_sub(1);
            Some(index + 1)
        }
        _ => None,
    }
}

/// Split a bracket's raw content into citation segments on top-level semicolons. A semicolon inside
/// a nested `[...]` or a backtick code span does not split. Returns `None` when the content is not a
/// citation list: it has no `@` at all, or any segment is empty (including a leading or trailing
/// empty segment from a stray semicolon).
fn split_citation_segments(text: &str) -> Option<Vec<std::ops::Range<usize>>> {
    if !text.contains('@') {
        return None;
    }
    let mut segments = Vec::new();
    let mut start = 0;
    let mut index = 0;
    let mut depth = 0usize;
    while index < text.len() {
        if let Some(next) = step_citation_scan(text, index, &mut depth) {
            index = next;
        } else if char_at(text, index) == Some(';') && depth == 0 {
            segments.push(start..index);
            start = index + 1;
            index += 1;
        } else {
            index += char_at(text, index).map_or(1, char::len_utf8);
        }
    }
    segments.push(start..text.len());
    // An empty segment (a stray `;`, or whitespace-only between separators) is not a citation list.
    for segment in &segments {
        if text
            .get(segment.clone())
            .is_none_or(|s| s.chars().all(char::is_whitespace))
        {
            return None;
        }
    }
    Some(segments)
}

/// The length in bytes of the backtick run starting at byte offset `index`.
fn backtick_run_len(text: &str, index: usize) -> usize {
    let bytes = text.as_bytes();
    let mut len = 0;
    while bytes.get(index + len) == Some(&b'`') {
        len += 1;
    }
    len
}

/// Skip past a code span opened by `run` backticks at `index`, returning the index just past its
/// closing run. With no matching closer the backticks are not a code span and only the opening run
/// is skipped.
fn skip_code_span(text: &str, index: usize, run: usize) -> usize {
    let bytes = text.as_bytes();
    let mut scan = index + run;
    while scan < bytes.len() {
        if bytes.get(scan) == Some(&b'`') {
            let closer = backtick_run_len(text, scan);
            if closer == run {
                return scan + closer;
            }
            scan += closer;
        } else {
            scan += 1;
        }
    }
    index + run
}

/// The located citation key of one segment: the index of `@`, the key text and the index past it,
/// whether a `-` author-suppression marker precedes the `@`, and that marker's index.
struct CitationKey {
    at: usize,
    dash: usize,
    id: String,
    id_end: usize,
    suppress: bool,
}

/// Find the first top-level `@key` within `chars[range]`: an `@` not inside a nested `[...]` or a
/// backtick code span, immediately followed by a key. A `-` directly before the `@`, itself at the
/// segment start or preceded by whitespace, marks author suppression. Returns `None` when no such
/// key is present.
fn find_citation_key(text: &str, range: std::ops::Range<usize>) -> Option<CitationKey> {
    let mut index = range.start;
    let mut depth = 0usize;
    while index < range.end {
        if let Some(next) = step_citation_scan(text, index, &mut depth) {
            index = next;
            continue;
        }
        if depth == 0
            && char_at(text, index) == Some('@')
            && let Some((id, id_end)) = scan_citation_id(text, index + 1)
        {
            // A `-` is ASCII, so when it precedes the `@` it sits at byte `index - 1`, and the
            // character before it ends at `index - 1`.
            let dash_before = index > range.start && char_before(text, index) == Some('-');
            let dash_anchored = dash_before
                && (index - 1 == range.start
                    || char_before(text, index - 1).is_some_and(char::is_whitespace));
            let suppress = dash_anchored;
            return Some(CitationKey {
                at: index,
                dash: if suppress { index - 1 } else { index },
                id,
                id_end,
                suppress,
            });
        }
        index += char_at(text, index).map_or(1, char::len_utf8);
    }
    None
}

fn def_target(def: &LinkDef) -> Target {
    Target {
        url: def.url.clone().into(),
        title: def.title.clone().into(),
    }
}

/// Scan an inline link tail `(destination "title")` in the markdown dialect, where the unbracketed
/// destination may hold spaces (percent-encoded to `%20`) and balanced parentheses. The destination
/// runs until the parenthesis that balances the link's opener, save for a trailing quoted title
/// separated by whitespace. Returns `None` when the parentheses are unbalanced or a quoted title is
/// not immediately followed by the closing parenthesis. `pos` points at the opening `(`.
fn scan_markdown_inline_target(text: &str, pos: usize) -> Option<(Target, usize)> {
    let mut index = pos + 1;
    skip_target_whitespace(text, &mut index);
    if char_at(text, index) == Some('<') {
        // The angle-bracketed form has no special space handling; defer to the shared scanner,
        // which already reads `<...>` destinations and an optional title.
        return scan_inline_target(text, pos);
    }
    let mut url = String::new();
    let mut title = String::new();
    let mut depth: usize = 0;
    loop {
        match char_at(text, index) {
            None => return None,
            Some(')') if depth == 0 => {
                index += 1;
                break;
            }
            Some(')') => {
                depth -= 1;
                url.push(')');
                index += 1;
            }
            Some('(') => {
                depth += 1;
                url.push('(');
                index += 1;
            }
            // An escaped space is always part of the destination — never a title separator — and
            // encodes as `%20` like any other destination space.
            Some('\\') if matches!(char_at(text, index + 1), Some(' ' | '\t')) => {
                url.push_str("%20");
                index += 2;
            }
            Some('\\') if char_at(text, index + 1).is_some_and(is_ascii_punctuation) => {
                if let Some(next) = char_at(text, index + 1) {
                    url.push('\\');
                    url.push(next);
                    index += 1 + next.len_utf8();
                }
            }
            Some(ch) if ch == ' ' || ch == '\t' => {
                let mut after = index;
                skip_target_whitespace(text, &mut after);
                match char_at(text, after) {
                    // Trailing whitespace before the closing parenthesis ends the destination.
                    Some(')') if depth == 0 => {
                        index = after;
                    }
                    // A quoted title separated by whitespace ends the destination. It must be the
                    // last element before the closing parenthesis, else the whole tail fails.
                    Some('"' | '\'') if depth == 0 => {
                        let (parsed, mut close) = scan_target_title(text, after)?;
                        title = parsed;
                        skip_target_whitespace(text, &mut close);
                        if char_at(text, close) != Some(')') {
                            return None;
                        }
                        index = close + 1;
                        break;
                    }
                    // More destination follows: the whitespace run joins it as a single `%20`.
                    Some(_) => {
                        url.push_str("%20");
                        index = after;
                    }
                    None => return None,
                }
            }
            Some(ch) => {
                url.push(ch);
                index += ch.len_utf8();
            }
        }
    }
    Some((
        Target {
            url: unescape_string(&url).into(),
            title: unescape_string(&title).into(),
        },
        index,
    ))
}

/// Advance `index` past a run of spaces and tabs.
fn skip_target_whitespace(text: &str, index: &mut usize) {
    while matches!(char_at(text, *index), Some(' ' | '\t')) {
        *index += 1;
    }
}

/// Scan a quoted link title starting at `start` (a `"` or `'`), returning its raw content and the
/// index just past the closing quote. A backslash escapes the following punctuation character.
fn scan_target_title(text: &str, start: usize) -> Option<(String, usize)> {
    let close = char_at(text, start)?;
    if close != '"' && close != '\'' {
        return None;
    }
    let mut index = start + 1;
    let mut out = String::new();
    while let Some(ch) = char_at(text, index) {
        if ch == close {
            return Some((out, index + 1));
        }
        if ch == '\\'
            && let Some(next) = char_at(text, index + 1)
            && is_ascii_punctuation(next)
        {
            out.push('\\');
            out.push(next);
            index += 1 + next.len_utf8();
            continue;
        }
        out.push(ch);
        index += ch.len_utf8();
    }
    None
}

/// Tag an explicit angle-bracket autolink with its kind. A link whose visible text differs from its
/// destination is an email autolink (the destination gained a `mailto:` scheme), so it carries the
/// `email` class; every other angle autolink is a URI autolink and carries `uri`. Bare URLs linked
/// by the autolink post-pass keep an empty class list and never reach here.
fn classify_angle_autolink(inline: Inline) -> Inline {
    let Inline::Link(mut attr, text, target) = inline else {
        return inline;
    };
    let is_email = matches!(text.first(), Some(Inline::Str(shown)) if *shown != target.url);
    attr.classes
        .push(if is_email { "email" } else { "uri" }.into());
    Inline::Link(attr, text, target)
}

/// Percent-encode the destination of a link, leaving its shown text untouched.
fn escape_link_destination(inline: Inline) -> Inline {
    let Inline::Link(attr, text, mut target) = inline else {
        return inline;
    };
    target.url = escape_uri(&target.url).into();
    Inline::Link(attr, text, target)
}

/// Pair literal `<span …>` / `</span>` raw-inline tags into [`Inline::Span`] nodes.
///
/// The inline phase first leaves both tags as raw inline HTML, so emphasis and links resolve around
/// them exactly as they would around any other tag. This pass then walks the resolved tree and,
/// at each nesting level independently, matches an opening tag with the nearest later closing tag,
/// wrapping the inlines between them in a span whose attributes come from the opening tag. Matching
/// stays within one level: a `<span>` that emphasis pulled inside an `Emph` only pairs with a
/// `</span>` that landed inside that same `Emph`. The content between a matched pair is itself
/// re-paired, so nested spans nest. Unmatched tags keep their raw-inline form.
///
/// Known limitation: when an emphasis run straddles exactly one of the two tags — the run opens
/// before a `<span>` and its closing marker sits just before the matching `</span>` — the two tags
/// can land at different levels and stay raw even though a span could have formed.
fn pair_native_spans(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut input = inlines.into_iter().peekable();
    pair_spans_level(&mut input, false)
}

/// Pair spans within one nesting level. Pulls from `input` until it is drained, or — when
/// `stop_at_close` is set — until an unmatched closing `</span>` tag is reached, which is left
/// unconsumed for the caller to handle. Container inlines have their own children re-paired.
fn pair_spans_level(
    input: &mut std::iter::Peekable<std::vec::IntoIter<Inline>>,
    stop_at_close: bool,
) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    while let Some(item) = input.peek() {
        if let Inline::RawInline(format, text) = item
            && format.0 == "html"
        {
            match classify_span_tag(text) {
                SpanTag::Open(attr) => {
                    let _ = input.next();
                    let inner = pair_spans_level(input, true);
                    if matches!(input.peek(), Some(Inline::RawInline(f, t))
                        if f.0 == "html" && matches!(classify_span_tag(t), SpanTag::Close))
                    {
                        let _ = input.next();
                        out.push(Inline::Span(Box::new(attr), inner));
                    } else {
                        // No matching close at this level: the opener reverts to raw, and its
                        // gathered inner content rejoins the stream.
                        out.push(Inline::RawInline(
                            carta_ast::Format("html".into()),
                            open_tag_raw(&attr).into(),
                        ));
                        out.extend(inner);
                    }
                    continue;
                }
                SpanTag::Close if stop_at_close => break,
                _ => {}
            }
        }
        if let Some(next) = input.next() {
            out.push(recurse_span_children(next));
        }
    }
    out
}

/// Re-pair spans inside the child lists of a container inline.
fn recurse_span_children(inline: Inline) -> Inline {
    match inline {
        Inline::Emph(c) => Inline::Emph(pair_native_spans(c)),
        Inline::Underline(c) => Inline::Underline(pair_native_spans(c)),
        Inline::Strong(c) => Inline::Strong(pair_native_spans(c)),
        Inline::Strikeout(c) => Inline::Strikeout(pair_native_spans(c)),
        Inline::Superscript(c) => Inline::Superscript(pair_native_spans(c)),
        Inline::Subscript(c) => Inline::Subscript(pair_native_spans(c)),
        Inline::SmallCaps(c) => Inline::SmallCaps(pair_native_spans(c)),
        Inline::Quoted(q, c) => Inline::Quoted(q, pair_native_spans(c)),
        Inline::Cite(cites, c) => Inline::Cite(cites, pair_native_spans(c)),
        Inline::Link(a, c, t) => Inline::Link(a, pair_native_spans(c), t),
        Inline::Image(a, c, t) => Inline::Image(a, pair_native_spans(c), t),
        Inline::Span(a, c) => Inline::Span(a, pair_native_spans(c)),
        other => other,
    }
}

/// The role of an HTML tag with respect to span pairing.
enum SpanTag {
    /// A literal `<span …>` opener, with its attributes parsed.
    Open(Attr),
    /// A literal `</span>` closer.
    Close,
    /// Any other tag, which plays no part in span pairing.
    Other,
}

/// Cheap pre-check before the char-by-char classification: does `raw` open with `<span` or `</span`
/// (case-insensitive)? Lets the common non-span inline tag bail out without allocating.
fn opens_span_tag(raw: &str) -> bool {
    let Some(after_lt) = raw.strip_prefix('<') else {
        return false;
    };
    let candidate = after_lt.strip_prefix('/').unwrap_or(after_lt);
    candidate
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("span"))
}

/// Classify a raw HTML tag string, parsing attributes for an opening `<span …>`. A self-closing
/// `<span/>` is `Other`: it has no content to wrap and stays raw.
fn classify_span_tag(raw: &str) -> SpanTag {
    if !opens_span_tag(raw) {
        return SpanTag::Other;
    }
    if char_at(raw, 1) == Some('/') {
        // `</span>` with optional trailing whitespace before `>`.
        let mut i = 2;
        if !matches_name(raw, &mut i, "span") {
            return SpanTag::Other;
        }
        while matches!(char_at(raw, i), Some(' ' | '\t' | '\n')) {
            i += 1;
        }
        if char_at(raw, i) == Some('>') && i + 1 == raw.len() {
            return SpanTag::Close;
        }
        return SpanTag::Other;
    }
    let mut i = 1;
    if !matches_name(raw, &mut i, "span") {
        return SpanTag::Other;
    }
    // A name character right after `span` means a different tag (`<spanner>`).
    if matches!(char_at(raw, i), Some(c) if c.is_ascii_alphanumeric() || c == '-') {
        return SpanTag::Other;
    }
    match parse_span_attributes(raw, i) {
        Some(attr) => SpanTag::Open(attr),
        None => SpanTag::Other,
    }
}

/// Match the literal `name` case-insensitively at `*i`, advancing `*i` past it on success. `name`
/// is ASCII, so its character offsets are byte offsets.
fn matches_name(text: &str, i: &mut usize, name: &str) -> bool {
    for (offset, expected) in name.chars().enumerate() {
        match char_at(text, *i + offset) {
            Some(c) if c.eq_ignore_ascii_case(&expected) => {}
            _ => return false,
        }
    }
    *i += name.len();
    true
}

/// Parse the attributes of an opening `<span …>` tag whose name ends at `start`, expecting the tag
/// to end with `>` (a trailing `/` makes it self-closing, which is rejected here). An `id` attribute
/// becomes the identifier and a `class` attribute splits into classes; only the first of each is
/// kept. Every other attribute becomes a key/value pair in source order; a valueless attribute
/// carries an empty value. Entity and numeric character references in values are decoded.
fn parse_span_attributes(text: &str, start: usize) -> Option<Attr> {
    let mut attr = Attr::default();
    let mut seen_class = false;
    let mut i = start;
    loop {
        let ws_start = i;
        while matches!(char_at(text, i), Some(' ' | '\t' | '\n')) {
            i += 1;
        }
        match char_at(text, i) {
            Some('>') if i + 1 == text.len() => return Some(attr),
            // A self-closing tag has no content to wrap.
            Some('/') => return None,
            _ => {}
        }
        // An attribute must be preceded by whitespace.
        if i == ws_start {
            return None;
        }
        let name_start = i;
        while matches!(
            char_at(text, i),
            Some(c) if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')
        ) {
            i += 1;
        }
        if i == name_start {
            return None;
        }
        let name = text.get(name_start..i)?.to_owned();
        let mut value = String::new();
        // Optional `= value` with whitespace allowed around `=`.
        let mut after = i;
        while matches!(char_at(text, after), Some(' ' | '\t' | '\n')) {
            after += 1;
        }
        if char_at(text, after) == Some('=') {
            after += 1;
            while matches!(char_at(text, after), Some(' ' | '\t' | '\n')) {
                after += 1;
            }
            let (parsed, next) = read_attr_value(text, after)?;
            value = parsed;
            i = next;
        } else {
            i = after;
        }
        match name.as_str() {
            "id" => {
                if attr.id.is_empty() {
                    attr.id = value.into();
                }
            }
            "class" => {
                if !seen_class {
                    seen_class = true;
                    attr.classes = value.split_whitespace().map(Into::into).collect();
                }
            }
            _ => attr.attributes.push((name.into(), value.into())),
        }
    }
}

/// Read an HTML attribute value at `start`: a double- or single-quoted string, or an unquoted run.
/// Returns the decoded value and the index just past it. Character references inside the value are
/// decoded.
fn read_attr_value(text: &str, start: usize) -> Option<(String, usize)> {
    let quote = char_at(text, start);
    if matches!(quote, Some('"' | '\'')) {
        let quote = quote?;
        let mut i = start + 1;
        let mut out = String::new();
        loop {
            match char_at(text, i) {
                Some(c) if c == quote => return Some((out, i + 1)),
                Some('&') => {
                    if let Some((decoded, next)) = scan_entity(text, i) {
                        out.push_str(&decoded);
                        i = next;
                    } else {
                        out.push('&');
                        i += 1;
                    }
                }
                Some(c) => {
                    out.push(c);
                    i += c.len_utf8();
                }
                None => return None,
            }
        }
    }
    // Unquoted value: a run with no whitespace, quotes, `=`, `<`, `>`, or backtick.
    let mut i = start;
    let mut out = String::new();
    while let Some(c) = char_at(text, i) {
        if matches!(c, ' ' | '\t' | '\n' | '"' | '\'' | '=' | '<' | '>' | '`') {
            break;
        }
        if c == '&'
            && let Some((decoded, next)) = scan_entity(text, i)
        {
            out.push_str(&decoded);
            i = next;
            continue;
        }
        out.push(c);
        i += c.len_utf8();
    }
    if out.is_empty() {
        return None;
    }
    Some((out, i))
}

/// Reconstruct the raw `<span …>` opener for an opener that found no matching close, so it falls
/// back to literal raw inline HTML. The exact original spelling is not recovered; a normalized form
/// carrying the same attributes is emitted.
fn open_tag_raw(attr: &Attr) -> String {
    let mut s = String::from("<span");
    if !attr.id.is_empty() {
        s.push_str(" id=\"");
        s.push_str(&attr.id);
        s.push('"');
    }
    if !attr.classes.is_empty() {
        s.push_str(" class=\"");
        s.push_str(&attr.classes.join(" "));
        s.push('"');
    }
    for (k, v) in &attr.attributes {
        s.push(' ');
        s.push_str(k);
        s.push_str("=\"");
        s.push_str(v);
        s.push('"');
    }
    s.push('>');
    s
}

/// Collapse the node list into final inlines: leftover delimiters become text, adjacent text is
/// merged, and text is split into `Str`/`Space` runs.
pub(super) fn collapse(nodes: Vec<Node>) -> Vec<Inline> {
    let mut text = String::new();
    let mut out: Vec<Inline> = Vec::new();
    let flush = |text: &mut String, out: &mut Vec<Inline>| {
        if !text.is_empty() {
            push_text_inlines(out, text);
            text.clear();
        }
    };
    for node in nodes {
        match node {
            Node::Text(t) => text.push_str(&t),
            Node::Delimiter(d) => {
                // An unmatched image opener carries its `!` in the `image` flag rather than a
                // separate node, so restore it when the bracket reverts to literal text.
                if d.image {
                    text.push('!');
                }
                text.push_str(&delimiter_literal(d.ch, d.count));
            }
            Node::Inline(inline) => {
                flush(&mut text, &mut out);
                out.push(inline);
            }
            Node::SoftBreak => {
                flush(&mut text, &mut out);
                out.push(Inline::SoftBreak);
            }
            Node::LineBreak => {
                flush(&mut text, &mut out);
                out.push(Inline::LineBreak);
            }
            Node::Empty => {}
        }
    }
    flush(&mut text, &mut out);
    out
}

/// Split a text run into `Str` tokens separated by `Space` inlines, collapsing each run of
/// spaces to a single `Space`.
fn push_text_inlines(out: &mut Vec<Inline>, text: &str) {
    // Word boundaries are single spaces, so scanning bytes is exact: multi-byte UTF-8 units are
    // all >= 0x80 and every slice below starts and ends at a character boundary. Each word is
    // copied out in one step, which keeps short words on the stack and long ones to one memcpy.
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte == b' ' {
            while bytes.get(i) == Some(&b' ') {
                i += 1;
            }
            out.push(Inline::Space);
        } else {
            let start = i;
            while bytes.get(i).is_some_and(|&b| b != b' ') {
                i += 1;
            }
            if let Some(word) = text.get(start..i) {
                out.push(Inline::Str(Text::from(word)));
            }
        }
    }
}

pub(super) fn flanking(ch: u8, before: Option<char>, after: Option<char>) -> (bool, bool) {
    let before_ws = before.is_none_or(is_unicode_whitespace);
    let after_ws = after.is_none_or(is_unicode_whitespace);
    let before_punct = before.is_some_and(is_punctuation);
    let after_punct = after.is_some_and(is_punctuation);

    let left_flanking = !after_ws && (!after_punct || before_ws || before_punct);
    let right_flanking = !before_ws && (!before_punct || after_ws || after_punct);

    match ch {
        b'_' => {
            let can_open = left_flanking && (!right_flanking || before_punct);
            let can_close = right_flanking && (!left_flanking || after_punct);
            (can_open, can_close)
        }
        // Subscript/superscript/strikeout delimiters anchor only on whitespace: a run opens unless
        // a space follows it and closes unless a space precedes it. The rule-of-three guard
        // (`emphasis_match`) still applies on top of this.
        b'~' | b'^' => (!after_ws, !before_ws),
        _ => (left_flanking, right_flanking),
    }
}

/// Open/close eligibility for a smart-quote run at a boundary. A run opens only when it is
/// left-flanking and not glued to a preceding letter or digit, and closes only when it is
/// right-flanking and not glued to a following letter or digit. The leftover-curly fallback then
/// turns an unmatched single quote into an apostrophe and an unmatched double quote into an opener.
pub(super) fn quote_flanking(_ch: u8, before: Option<char>, after: Option<char>) -> (bool, bool) {
    let before_ws = before.is_none_or(is_unicode_whitespace);
    let after_ws = after.is_none_or(is_unicode_whitespace);
    let before_punct = before.is_some_and(is_punctuation);
    let after_punct = after.is_some_and(is_punctuation);
    let before_alnum = before.is_some_and(char::is_alphanumeric);
    let after_alnum = after.is_some_and(char::is_alphanumeric);

    let left_flanking = !after_ws && (!after_punct || before_ws || before_punct);
    let right_flanking = !before_ws && (!before_punct || after_ws || after_punct);

    let can_open = left_flanking && !before_alnum;
    let can_close = right_flanking && !after_alnum;
    (can_open, can_close)
}

/// The characters an original-Markdown backslash escape recognizes regardless of
/// `all_symbols_escapable`: the inline delimiters and block markers that carry syntactic weight. A
/// dialect without the broad escape set drops the backslash before only these, leaving every other
/// backslash literal.
fn is_classic_markdown_escapable(ch: char) -> bool {
    matches!(
        ch,
        '\\' | '`'
            | '*'
            | '_'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | '>'
            | '#'
            | '+'
            | '-'
            | '.'
            | '!'
    )
}

/// A Unicode punctuation character per the spec: an ASCII punctuation character or anything in the
/// Unicode `P` (punctuation) or `S` (symbol) general categories.
fn is_punctuation(ch: char) -> bool {
    use unicode_general_category::GeneralCategory::{
        ClosePunctuation, ConnectorPunctuation, CurrencySymbol, DashPunctuation, FinalPunctuation,
        InitialPunctuation, MathSymbol, ModifierSymbol, OpenPunctuation, OtherPunctuation,
        OtherSymbol,
    };
    if ch.is_ascii() {
        return is_ascii_punctuation(ch);
    }
    matches!(
        unicode_general_category::get_general_category(ch),
        ConnectorPunctuation
            | DashPunctuation
            | OpenPunctuation
            | ClosePunctuation
            | InitialPunctuation
            | FinalPunctuation
            | OtherPunctuation
            | MathSymbol
            | CurrencySymbol
            | ModifierSymbol
            | OtherSymbol
    )
}

/// Normalize the interior of a code span: line endings to spaces, and if it both begins and ends
/// with a space (and is not all spaces), strip one space from each end.
fn normalize_code(content: &str, markdown: bool) -> String {
    let collapsed: String = content
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    // The markdown dialect strips all surrounding whitespace; the strict dialect removes only a
    // single leading and trailing space, and only when the content is not all spaces.
    if markdown {
        return collapsed.trim().to_owned();
    }
    let bytes = collapsed.as_bytes();
    if collapsed.len() >= 2
        && bytes.first() == Some(&b' ')
        && bytes.last() == Some(&b' ')
        && !collapsed.chars().all(|c| c == ' ')
    {
        collapsed
            .get(1..collapsed.len() - 1)
            .unwrap_or("")
            .to_owned()
    } else {
        collapsed
    }
}

#[cfg(test)]
#[path = "inline_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "inline_parse_tests.rs"]
mod inline_tests;
