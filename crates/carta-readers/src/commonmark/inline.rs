//! Inline phase: parse the raw text of leaf blocks into inline nodes.
//!
//! Implements the spec's inline algorithm: a left-to-right scan that resolves code spans,
//! autolinks, raw HTML, entities and escapes immediately, records `*`/`_`/`[`/`![` runs on a
//! delimiter stack, resolves links/images at each `]`, and finally collapses emphasis. The raw
//! char-slice scanners it drives (autolinks, HTML tags, entities, link targets) live in `scan`.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{Inline, Text};
use carta_core::{Extension, Extensions};

use super::emphasis::{delimiter_literal, process_emphasis, resolve_mark};
use super::resolve::RefContext;
use super::scan::{char_at, is_ascii_punctuation};
use super::{ExampleMap, RefMap};
use crate::emoji;
use crate::inline_scan::is_unicode_whitespace;
use crate::smart_fold::{fold_dash_run_thirds, fold_ellipsis_run};

mod helpers;
mod links;
mod native_span;
mod tokens;

use native_span::pair_native_spans;

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
    /// it: a link may not contain another link. On `]`, an inactive opener is popped and
    /// literalized without attempting any link-target parse (spec §6.3, rule 6).
    pub(super) active: bool,
    /// The citation count at the moment this bracket opened. If the bracket later resolves to a
    /// single citation, any bare citations counted while scanning its interior are discarded along
    /// with their nodes, so the count rewinds to this value first. Unused for non-bracket
    /// delimiters.
    pub(super) cite_count_at_open: i32,
}

// `notes` (footnote context) and `nodes` (inline list) unavoidably read alike.
/// Run the gated highlight-mark pass and emphasis resolution over a node list, then collapse it into
/// inlines, the shared finishing sequence for a parsed inline run, a span body, and a link label.
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

/// Parse standalone text (a document metadata value) into inlines with no reference context, so
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
/// character bound. Reference lookups treat such a span as no label at all (skipping extraction,
/// normalization, and the map lookup) regardless of what definitions exist, which also keeps a
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
    /// attempt reverts (the backslash stays literal, the same outcome an unmatched group already
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
/// ordinary text. Must stay in lockstep with the dispatch arms in [`InlineParser::run`]: any new
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

impl InlineParser<'_> {
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
    /// this scan stops on: the run scan can advance one byte at a time and still break only on a
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
                // The `!` lives in the `image` flag, not a node; restore it here.
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
    // Boundaries are single ASCII spaces, so byte scanning is exact and each word copies in one
    // step (one memcpy per word).
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
        // `~`/`^` anchor only on whitespace; the rule-of-three guard still applies on top.
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

#[cfg(test)]
#[path = "inline_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "inline_parse_tests.rs"]
mod inline_parse_tests;
