//! Inline scanning: emphasis, verbatim, sub/superscripts, links, footnotes, math, and quotes.

use std::collections::BTreeMap;
use std::mem;

use carta_ast::{Attr, Block, Format, Inline, MathType, QuoteType};
use carta_core::{Extension, Extensions, with_stack};

use super::citations::parse_citation_items;
use super::entities::entity;
use super::inline_helpers::{
    image, is_image_target, is_internal_target, is_uri, is_url_boundary, link, plain_words,
    post_ok, pre_ok, process_target, verbatim_code, wrap_markup,
};

pub(super) fn parse_inlines(
    text: &str,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut closes: BTreeMap<(char, Option<char>), Vec<usize>> = BTreeMap::new();
    let mut groups = BTreeMap::new();
    let mut brackets = Vec::new();
    let mut braces = Vec::new();
    let mut parentheses = Vec::new();
    let mut closing_brackets = Vec::new();
    let mut backslashes = Vec::new();
    let mut newlines = Vec::new();
    for (index, &character) in chars.iter().enumerate() {
        let previous = index
            .checked_sub(1)
            .and_then(|index| chars.get(index))
            .copied();
        let next = chars.get(index + 1).copied();
        if closes_alone(character, previous, next) {
            closes.entry((character, None)).or_default().push(index);
        }
        if matches!(
            (character, next),
            ('$', Some('$')) | ('@', Some('@')) | ('\\', Some(')' | ']'))
        ) || is_verbatim_marker(character)
            && next.is_some_and(is_emphasis_marker)
            && previous.is_some_and(|character| !character.is_whitespace())
        {
            closes.entry((character, next)).or_default().push(index);
        }
        match character {
            '[' => brackets.push(index),
            ']' => {
                closing_brackets.push(index);
                if let Some(open) = brackets.pop() {
                    groups.insert(open, index);
                }
            }
            '\\' => backslashes.push(index),
            '{' => braces.push(index),
            '}' => {
                if let Some(open) = braces.pop() {
                    groups.insert(open, index);
                }
            }
            '(' => parentheses.push(index),
            ')' => {
                if let Some(open) = parentheses.pop() {
                    groups.insert(open, index);
                }
            }
            '\n' => {
                braces.clear();
                parentheses.clear();
                newlines.push(index);
            }
            _ => {}
        }
    }
    let text = Text {
        chars: &chars,
        ext,
        notes,
        closes: &closes,
        groups: &groups,
        closing_brackets: &closing_brackets,
        backslashes: &backslashes,
        newlines: &newlines,
    };
    Inlines::new(text, 0, chars.len(), None).parse()
}

/// Whether `character` closes a span by itself between these neighbors.
fn closes_alone(character: char, previous: Option<char>, next: Option<char>) -> bool {
    matches!(character, '<' | '>')
        || previous.is_some_and(|character| !character.is_whitespace())
            && (matches!(character, '=' | '~' | '\'' | '"') && post_ok(next)
                || character == '$' && !next.is_some_and(char::is_alphanumeric))
}

fn is_emphasis_marker(character: char) -> bool {
    matches!(character, '*' | '/' | '+' | '_')
}

fn is_verbatim_marker(character: char) -> bool {
    matches!(character, '=' | '~')
}

/// Whether `character` is plain text wherever it stands: not a space and not a character that
/// markup is written with.
fn is_word_character(character: char) -> bool {
    !matches!(
        character,
        ' ' | '\t'
            | '\n'
            | '"'
            | '$'
            | '\''
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | '-'
            | '.'
            | '/'
            | ':'
            | ';'
            | '<'
            | '='
            | '>'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '_'
            | '{'
            | '|'
            | '}'
            | '~'
    )
}

/// One inline run: its characters, the positions its scanners look up, and the reader settings.
#[derive(Clone, Copy)]
struct Text<'a> {
    chars: &'a [char],
    ext: Extensions,
    notes: &'a BTreeMap<String, Vec<Block>>,
    closes: &'a BTreeMap<(char, Option<char>), Vec<usize>>,
    groups: &'a BTreeMap<usize, usize>,
    closing_brackets: &'a [usize],
    backslashes: &'a [usize],
    newlines: &'a [usize],
}

/// The first of the ascending `positions` at or after `from`.
fn first_from(positions: &[usize], from: usize) -> Option<usize> {
    positions
        .get(positions.partition_point(|&position| position < from))
        .copied()
}

/// A scan position with what the scan remembers of the characters before it.
#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
struct Cursor {
    at: usize,
    /// The position directly follows the marker that opened the span being read.
    opened: bool,
    /// Markup may open after any character: nothing it may follow has been read since a link
    /// description or a braced script ended.
    loose: bool,
    /// The emphasis being read may still hold a line break.
    lines: bool,
    /// The position directly follows plain text, which a sub- or superscript attaches to.
    word: bool,
}

/// What one scan step found.
enum Piece {
    Space,
    SoftBreak,
    /// A character of the word being gathered.
    Char(char),
    /// The characters up to `end`, which join the word being gathered.
    Word {
        end: usize,
    },
    /// Text of the word being gathered.
    Str(&'static str),
    Leaf(Inline),
    /// Emphasis or verbatim text from the marker at the scan position to the one at `close`.
    Span {
        close: usize,
    },
    /// A sub- or superscript whose content is the window `start..stop`, in braces or with its
    /// parentheses.
    Script {
        sup: bool,
        start: usize,
        stop: usize,
        braced: bool,
    },
    /// A bracketed link that closes at `close`, with the start of its description when it has one.
    Link {
        description: Option<usize>,
        close: usize,
    },
    /// An inline footnote whose text is the window `start..stop`.
    Note {
        start: usize,
        stop: usize,
    },
    /// A reference to the footnote labeled by the characters `start..stop`.
    NoteReference {
        start: usize,
        stop: usize,
    },
    Citation {
        close: usize,
    },
    Quote {
        kind: QuoteType,
        close: usize,
    },
}

/// A scanner over the window `start..end` of an inline run. Nested content is a narrower window
/// over the same characters and position tables, read as if nothing lay outside it.
struct Inlines<'a> {
    text: Text<'a>,
    start: usize,
    end: usize,
    /// Markup may open after any character from the start of the window.
    loose: bool,
    /// A line break in the window reads as a space.
    joined: bool,
    /// The characters around the window that its border rules see. A window is otherwise read
    /// as if nothing lay outside it.
    borders: (Option<char>, Option<char>),
    /// The marker of the emphasis that encloses the window, which may directly follow a closing
    /// marker inside it.
    enclosing: Option<char>,
    out: Vec<Inline>,
    word: String,
    /// Where the span opened at a position closes, per enclosing marker.
    spans: BTreeMap<(usize, Option<char>), Option<SpanClose>>,
    /// Where the walk from a cursor first closes a span.
    closers: BTreeMap<Walk, Option<(usize, bool)>>,
}

/// Where a span closes, with whether the emphasis around it may still hold a line break when the
/// span settles that.
type SpanClose = (usize, Option<bool>);

/// A walk through the content of a span toward its close: the position, whether markup may open
/// after any character, whether a line break may still follow, whether plain text comes directly
/// before, the marker of the span, and the marker of the emphasis enclosing it.
type Walk = (usize, bool, bool, bool, char, Option<char>);

impl<'a> Inlines<'a> {
    fn new(text: Text<'a>, start: usize, end: usize, enclosing: Option<char>) -> Self {
        Self {
            text,
            start,
            end,
            loose: false,
            joined: false,
            borders: (None, None),
            enclosing,
            out: Vec::new(),
            word: String::new(),
            spans: BTreeMap::new(),
            closers: BTreeMap::new(),
        }
    }

    fn parse(mut self) -> Vec<Inline> {
        let cursor = Cursor {
            at: self.start,
            opened: false,
            loose: self.loose,
            lines: true,
            word: false,
        };
        self.content(cursor, self.end, self.enclosing)
    }

    /// A scanner for the window `start..end` as inline content of its own.
    fn nested(&self, start: usize, end: usize, enclosing: Option<char>) -> Inlines<'a> {
        Inlines::new(self.text, start, end, enclosing)
    }

    /// Read the inlines from `cursor` up to `stop`, apart from the output gathered so far.
    fn content(&mut self, cursor: Cursor, stop: usize, enclosing: Option<char>) -> Vec<Inline> {
        self.flush();
        let outer = mem::take(&mut self.out);
        with_stack(|| self.run(cursor, stop, enclosing));
        self.flush();
        mem::replace(&mut self.out, outer)
    }

    fn flush(&mut self) {
        if !self.word.is_empty() {
            self.out.push(Inline::Str(mem::take(&mut self.word).into()));
        }
    }

    fn push_inline(&mut self, inline: Inline) {
        self.flush();
        self.out.push(inline);
    }

    fn at(&self, i: usize) -> Option<char> {
        if i < self.end {
            self.text.chars.get(i).copied()
        } else {
            None
        }
    }

    fn before(&self, i: usize) -> Option<char> {
        if i > self.start {
            self.at(i - 1)
        } else {
            self.borders.0
        }
    }

    fn after(&self, i: usize) -> Option<char> {
        if i + 1 < self.end {
            self.at(i + 1)
        } else {
            self.borders.1
        }
    }

    /// The first closing delimiter at or after `from`, which lies past the window's first
    /// character. The tables record each character between its neighbors in the whole run, so the
    /// window's last character, which has no successor here, is judged apart.
    fn next_close(&self, from: usize, first: char, second: Option<char>) -> Option<usize> {
        let last = self.end.checked_sub(1)?;
        let indexed = self
            .text
            .closes
            .get(&(first, second))
            .and_then(|positions| {
                positions
                    .get(positions.partition_point(|&position| position < from))
                    .copied()
            });
        if let Some(position) = indexed.filter(|&position| position < last) {
            return Some(position);
        }
        (second.is_none()
            && from <= last
            && self.at(last) == Some(first)
            && closes_alone(first, self.before(last), None))
        .then_some(last)
    }

    /// The closing bracket or brace paired with the one at `open`, when the window holds it.
    fn group_end(&self, open: usize) -> Option<usize> {
        self.text
            .groups
            .get(&open)
            .copied()
            .filter(|&close| close < self.end)
    }

    fn within_lines(&self, from: usize, to: usize, count: usize) -> bool {
        let start = self
            .text
            .newlines
            .partition_point(|&position| position < from);
        self.text
            .newlines
            .get(start + count)
            .is_none_or(|&position| position >= to)
    }

    fn run(&mut self, mut cursor: Cursor, stop: usize, enclosing: Option<char>) {
        while cursor.at < stop {
            let Some(character) = self.at(cursor.at) else {
                break;
            };
            let (piece, next) = self.scan(cursor, character, enclosing);
            self.emit(piece, cursor.at, enclosing);
            cursor = next;
        }
    }

    /// Recognize the inline at `cursor`, whose character is `c`, without reading nested content.
    #[allow(clippy::too_many_lines)]
    fn scan(&mut self, cursor: Cursor, c: char, enclosing: Option<char>) -> (Piece, Cursor) {
        let i = cursor.at;
        let prev = self.before(i);
        let to = |at: usize, loose: bool| Cursor {
            at,
            opened: false,
            loose,
            lines: cursor.lines,
            word: false,
        };

        if is_url_boundary(prev)
            && let Some((url, end)) = self.scan_bare_url(i)
        {
            let label = vec![Inline::Str(url.clone().into())];
            return (Piece::Leaf(link(&url, label)), to(end, cursor.loose));
        }

        let may_open = || cursor.opened || cursor.loose || pre_ok(prev);
        let found = match c {
            c if c.is_alphanumeric() => {
                let mut end = i + 1;
                while self.at(end).is_some_and(char::is_alphanumeric) {
                    end += 1;
                }
                let after = Cursor {
                    word: true,
                    ..to(end, cursor.loose)
                };
                Some((Piece::Word { end }, after))
            }
            ' ' | '\t' => Some((Piece::Space, to(self.space_end(i), false))),
            '\n' if self.joined => Some((Piece::Space, to(self.space_end(i), false))),
            '\n' => {
                let after = Cursor {
                    lines: false,
                    ..to(i + 1, false)
                };
                Some((Piece::SoftBreak, after))
            }
            '\\' => {
                let (piece, end) = self.scan_backslash(i);
                let loose = cursor.loose && !matches!(piece, Piece::Leaf(Inline::LineBreak));
                Some((piece, to(end, loose)))
            }
            // A subscript binds to the text before it ahead of an underline.
            '_' if cursor.word => self
                .scan_subsup(cursor, false)
                .or_else(|| self.scan_span(cursor, may_open(), enclosing)),
            '*' | '/' | '+' | '_' | '=' | '~' => self.scan_span(cursor, may_open(), enclosing),
            '^' if cursor.word => self.scan_subsup(cursor, true),
            '[' => self.scan_bracket(i).map(|(piece, end)| {
                let described = matches!(
                    piece,
                    Piece::Link {
                        description: Some(_),
                        ..
                    }
                );
                (piece, to(end, cursor.loose || described))
            }),
            '<' => self
                .scan_angle(i)
                .map(|(inline, end)| (Piece::Leaf(inline), to(end, cursor.loose))),
            '$' => self
                .scan_math_dollar(i, prev)
                .map(|(inline, end)| (Piece::Leaf(inline), to(end, cursor.loose))),
            '@' => self
                .scan_export(i)
                .map(|(inline, end)| (Piece::Leaf(inline), to(end, cursor.loose))),
            // The typographic dash and ellipsis replacements are always active.
            '-' | '.' => self
                .scan_special_string(i)
                .map(|(text, end)| (Piece::Str(text), to(end, cursor.loose && c != '-'))),
            // A word-internal or trailing apostrophe becomes a right single quotation mark.
            '\'' if self.text.ext.contains(Extension::Smart)
                && prev.is_some_and(char::is_alphanumeric) =>
            {
                Some((Piece::Char('\u{2019}'), to(i + 1, false)))
            }
            '"' | '\'' if self.text.ext.contains(Extension::Smart) => self
                .scan_quote(i, c)
                .map(|(kind, close)| (Piece::Quote { kind, close }, to(close + 1, cursor.loose))),
            _ => None,
        };
        found.unwrap_or_else(|| {
            let after = Cursor {
                word: is_word_character(c),
                ..to(i + 1, cursor.loose && !pre_ok(Some(c)))
            };
            (Piece::Char(c), after)
        })
    }

    /// The end of the run of spaces at `i`, which takes in line breaks where they read as spaces.
    fn space_end(&self, i: usize) -> usize {
        let mut end = i;
        while self
            .at(end)
            .is_some_and(|c| matches!(c, ' ' | '\t') || c == '\n' && self.joined)
        {
            end += 1;
        }
        end
    }

    /// Add the piece scanned at `i` to the output, reading the content nested in it.
    fn emit(&mut self, piece: Piece, i: usize, enclosing: Option<char>) {
        match piece {
            Piece::Space => {
                self.flush();
                self.out.push(Inline::Space);
            }
            Piece::SoftBreak => {
                self.flush();
                self.out.push(Inline::SoftBreak);
            }
            Piece::Char(character) => self.word.push(character),
            Piece::Word { end } => self.word.extend(self.text.chars.get(i..end).unwrap_or(&[])),
            Piece::Str(text) => self.word.push_str(text),
            Piece::Leaf(inline) => self.push_inline(inline),
            Piece::Span { close } => {
                let Some(marker) = self.at(i) else {
                    return;
                };
                let inline = if is_verbatim_marker(marker) {
                    verbatim_code(marker, self.text.chars.get(i + 1..close).unwrap_or(&[]))
                } else {
                    let first = Cursor {
                        at: i + 1,
                        opened: true,
                        loose: false,
                        lines: true,
                        word: false,
                    };
                    wrap_markup(marker, self.content(first, close, Some(marker)))
                };
                self.push_inline(inline);
            }
            Piece::Script {
                sup,
                start,
                stop,
                braced,
            } => {
                let mut group = self.nested(start, stop, enclosing);
                group.loose = braced;
                let content = group.parse();
                self.push_inline(if sup {
                    Inline::Superscript(content)
                } else {
                    Inline::Subscript(content)
                });
            }
            Piece::Link { description, close } => {
                let inline = self.build_link(i + 2, description, close, enclosing);
                self.push_inline(inline);
            }
            Piece::Note { start, stop } => {
                let mut note = self.nested(start, stop, enclosing);
                note.borders = (self.before(start), self.at(stop));
                self.push_inline(Inline::Note(vec![Block::Para(note.parse())]));
            }
            Piece::NoteReference { start, stop } => {
                let label: String = self
                    .text
                    .chars
                    .get(start..stop)
                    .unwrap_or(&[])
                    .iter()
                    .collect();
                let blocks = self.text.notes.get(&label).cloned().unwrap_or_default();
                self.push_inline(Inline::Note(blocks));
            }
            Piece::Citation { close } => {
                if let Some(inline) = self.build_citation(i, close) {
                    self.push_inline(inline);
                }
            }
            Piece::Quote { kind, close } => {
                let content = self.nested(i + 1, close, enclosing).parse();
                self.push_inline(Inline::Quoted(kind, content));
            }
        }
    }

    // Emphasis and verbatim

    /// Recognizes the emphasis or verbatim span opened by the marker at `cursor`. A marker opens
    /// a span where markup may open and a non-space character follows it.
    fn scan_span(
        &mut self,
        cursor: Cursor,
        may_open: bool,
        enclosing: Option<char>,
    ) -> Option<(Piece, Cursor)> {
        let i = cursor.at;
        if !may_open || self.at(i + 1).is_none_or(char::is_whitespace) {
            return None;
        }
        let (close, lines) = if let Some(&known) = self.spans.get(&(i, enclosing)) {
            known
        } else {
            let found = with_stack(|| self.span_close(i, enclosing));
            self.spans.insert((i, enclosing), found);
            found
        }?;
        let after = Cursor {
            at: close + 1,
            opened: false,
            loose: false,
            // Verbatim text counts its own line break.
            lines: lines.unwrap_or(cursor.lines),
            word: false,
        };
        Some((Piece::Span { close }, after))
    }

    /// The close of the span opened at `open`, with whether the emphasis around it may still hold
    /// a line break when the span settles that. The content of a span is not empty. Verbatim text
    /// holds at most one line break. Emphasis holds at most one too, counted afresh from each
    /// emphasis that opens inside it.
    fn span_close(&mut self, open: usize, enclosing: Option<char>) -> Option<SpanClose> {
        let marker = self.at(open)?;
        let first = open + 1;
        if self.closes_span(first, marker, enclosing) {
            return None;
        }
        if is_verbatim_marker(marker) {
            let plain = self.next_close(first + 1, marker, None);
            let enclosed =
                enclosing.and_then(|outer| self.next_close(first + 1, marker, Some(outer)));
            let close = match (plain, enclosed) {
                (Some(plain), Some(enclosed)) => plain.min(enclosed),
                (plain, enclosed) => plain.or(enclosed)?,
            };
            return self.within_lines(first, close, 1).then_some((close, None));
        }
        let cursor = Cursor {
            at: first,
            opened: true,
            loose: false,
            lines: true,
            word: false,
        };
        let character = self.at(first)?;
        let (_, next) = self.scan(cursor, character, Some(marker));
        let (close, lines) = self.first_close(next, marker, enclosing)?;
        Some((close, Some(lines)))
    }

    /// The first position at or after `cursor`, stepping inline by inline, where `marker` closes,
    /// with whether a line break may still follow it.
    fn first_close(
        &mut self,
        mut cursor: Cursor,
        marker: char,
        enclosing: Option<char>,
    ) -> Option<(usize, bool)> {
        // A walk that joins this one follows it step by step, so it meets a mark left every few
        // steps. Short walks leave none.
        const STRIDE: usize = 16;
        let mut marks = Vec::new();
        let mut steps = 0usize;
        let found = loop {
            let key = (
                cursor.at,
                cursor.loose,
                cursor.lines,
                cursor.word,
                marker,
                enclosing,
            );
            if let Some(&known) = self.closers.get(&key) {
                break known;
            }
            if self.closes_span(cursor.at, marker, enclosing) {
                break Some((cursor.at, cursor.lines));
            }
            let Some(character) = self.at(cursor.at) else {
                break None;
            };
            if character == '\n' && !cursor.lines {
                break None;
            }
            steps += 1;
            if steps.is_multiple_of(STRIDE) {
                marks.push(key);
            }
            cursor = self.scan(cursor, character, Some(marker)).1;
        };
        for key in marks {
            self.closers.insert(key, found);
        }
        found
    }

    /// Whether the character at `i` is `marker` closing its span: it follows a non-space character
    /// and precedes the end of the window, a character markup may precede, or the marker of the
    /// enclosing emphasis.
    fn closes_span(&self, i: usize, marker: char, enclosing: Option<char>) -> bool {
        self.at(i) == Some(marker)
            && self
                .before(i)
                .is_some_and(|character| !character.is_whitespace())
            && {
                let next = self.after(i);
                post_ok(next) || next == enclosing
            }
    }

    // Sub/superscript

    /// Recognizes a subscript (`_`) or superscript (`^`) at `cursor`, which directly follows plain
    /// text. The script is a braced group, a parenthesized group that keeps its parentheses, a
    /// lone `*`, or alphanumerics after an optional sign.
    fn scan_subsup(&self, cursor: Cursor, sup: bool) -> Option<(Piece, Cursor)> {
        let first = cursor.at + 1;
        let to = |at: usize, loose: bool| Cursor {
            at,
            opened: false,
            loose,
            lines: cursor.lines,
            word: false,
        };
        let group = |start: usize, stop: usize, braced: bool| Piece::Script {
            sup,
            start,
            stop,
            braced,
        };
        match self.at(first)? {
            '{' => {
                let close = self.group_end(first)?;
                Some((group(first + 1, close, true), to(close + 1, true)))
            }
            '(' => {
                let close = self.group_end(first)?;
                Some((group(first, close + 1, false), to(close + 1, true)))
            }
            character => {
                let mut stop = first + usize::from(matches!(character, '+' | '-'));
                let digits = stop;
                while self.at(stop).is_some_and(char::is_alphanumeric) {
                    stop += 1;
                }
                if stop == digits {
                    if character != '*' {
                        return None;
                    }
                    stop = first + 1;
                }
                let text: String = self
                    .text
                    .chars
                    .get(first..stop)
                    .unwrap_or(&[])
                    .iter()
                    .collect();
                let content = vec![Inline::Str(text.into())];
                let inline = if sup {
                    Inline::Superscript(content)
                } else {
                    Inline::Subscript(content)
                };
                Some((Piece::Leaf(inline), to(stop, cursor.loose)))
            }
        }
    }

    // Backslash: line break, math, entities

    fn scan_backslash(&self, i: usize) -> (Piece, usize) {
        match self.at(i + 1) {
            Some('\\') => {
                let mut j = i + 2;
                while matches!(self.at(j), Some(' ' | '\t')) {
                    j += 1;
                }
                if self.at(j) == Some('\n') {
                    j += 1;
                }
                (Piece::Leaf(Inline::LineBreak), j)
            }
            Some('(') => self.scan_tex_math(i, ')', MathType::InlineMath),
            Some('[') => self.scan_tex_math(i, ']', MathType::DisplayMath),
            Some(c) if c.is_ascii_alphabetic() => self.scan_entity(i),
            _ => (Piece::Char('\\'), i + 1),
        }
    }

    fn scan_tex_math(&self, i: usize, close: char, kind: MathType) -> (Piece, usize) {
        let start = i + 2;
        let Some(end) = self.next_close(start, '\\', Some(close)) else {
            return (Piece::Char('\\'), i + 1);
        };
        let inner: String = self
            .text
            .chars
            .get(start..end)
            .unwrap_or(&[])
            .iter()
            .collect();
        (Piece::Leaf(Inline::Math(kind, inner.into())), end + 2)
    }

    fn scan_entity(&self, i: usize) -> (Piece, usize) {
        let mut j = i + 1;
        while matches!(self.at(j), Some(c) if c.is_ascii_alphabetic()) {
            j += 1;
        }
        let name: String = self
            .text
            .chars
            .get(i + 1..j)
            .unwrap_or(&[])
            .iter()
            .collect();
        let mut end = j;
        if self.at(j) == Some('{') && self.at(j + 1) == Some('}') {
            end = j + 2;
        }
        let piece = match entity(&name) {
            Some(replacement) => Piece::Str(replacement),
            None => Piece::Leaf(Inline::RawInline(
                Format("latex".into()),
                format!("\\{name}").into(),
            )),
        };
        (piece, end)
    }

    // Brackets: links, footnotes, citations

    fn scan_bracket(&self, i: usize) -> Option<(Piece, usize)> {
        if self.at(i + 1) == Some('[') {
            return self.scan_link(i);
        }
        if self.matches_at(i + 1, &['f', 'n', ':']) {
            return self.scan_footnote(i);
        }
        if self.text.ext.contains(Extension::Citations)
            && (self.matches_at(i + 1, &['c', 'i', 't', 'e', ':'])
                || self.matches_at(i + 1, &['c', 'i', 't', 'e', '/']))
        {
            return self.scan_citation(i);
        }
        None
    }

    /// Recognizes a bracketed link: a target without brackets or a line break, then possibly a
    /// description. The description runs to the first closing bracket and holds no backslash. It
    /// holds at most one line break and does not end with it. Only a link with a description may
    /// have an empty target.
    fn scan_link(&self, i: usize) -> Option<(Piece, usize)> {
        let start = i + 2;
        let mut target_end = start;
        while self
            .at(target_end)
            .is_some_and(|c| !matches!(c, '[' | ']' | '\n'))
        {
            target_end += 1;
        }
        if self.at(target_end) != Some(']') {
            return None;
        }
        if self.at(target_end + 1) == Some(']') {
            let piece = Piece::Link {
                description: None,
                close: target_end,
            };
            return (target_end > start).then_some((piece, target_end + 2));
        }
        if self.at(target_end + 1) != Some('[') {
            return None;
        }
        let description = target_end + 2;
        let close = first_from(self.text.closing_brackets, description)?;
        let valid = close > description
            && self.at(close + 1) == Some(']')
            && self.at(close - 1) != Some('\n')
            && first_from(self.text.backslashes, description).is_none_or(|at| at > close)
            && self.within_lines(description, close, 1);
        let piece = Piece::Link {
            description: Some(description),
            close,
        };
        valid.then_some((piece, close + 2))
    }

    /// Build the link whose target starts at `start` and whose brackets close at `close`.
    fn build_link(
        &self,
        start: usize,
        description: Option<usize>,
        close: usize,
        enclosing: Option<char>,
    ) -> Inline {
        let target_end = description.map_or(close, |description| description - 2);
        let target_raw: String = self
            .text
            .chars
            .get(start..target_end)
            .unwrap_or(&[])
            .iter()
            .collect();
        let target = process_target(&target_raw);
        let internal = !target_raw.is_empty() && is_internal_target(&target_raw);
        if !internal && description.is_none() && is_image_target(&target) {
            return image(&target, Vec::new());
        }
        let target = if internal {
            format!("{}{target_raw}", super::links::INTERNAL_TARGET)
        } else {
            target
        };
        let label = match description {
            Some(description) => {
                let mut label = self.nested(description, close, enclosing);
                label.loose = true;
                label.joined = true;
                label.parse()
            }
            None => vec![Inline::Str(target_raw.into())],
        };
        link(&target, label)
    }

    fn scan_footnote(&self, i: usize) -> Option<(Piece, usize)> {
        let close = self.group_end(i)?;
        let body_start = i + "[fn:".len();
        let body = self.text.chars.get(body_start..close)?;
        let piece = match body.iter().position(|&character| character == ':') {
            Some(colon) => {
                let mut start = body_start + colon + 1;
                let mut stop = close;
                while start < stop && self.at(start).is_some_and(char::is_whitespace) {
                    start += 1;
                }
                while stop > start && self.at(stop - 1).is_some_and(char::is_whitespace) {
                    stop -= 1;
                }
                Piece::Note { start, stop }
            }
            None => Piece::NoteReference {
                start: body_start,
                stop: close,
            },
        };
        Some((piece, close + 1))
    }

    /// A citation names at least one key.
    fn scan_citation(&self, i: usize) -> Option<(Piece, usize)> {
        let close = self.group_end(i)?;
        let inner: String = self
            .text
            .chars
            .get(i + 1..close)
            .unwrap_or(&[])
            .iter()
            .collect();
        let (_, payload) = citation_parts(&inner)?;
        payload
            .contains('@')
            .then_some((Piece::Citation { close }, close + 1))
    }

    fn build_citation(&self, i: usize, close: usize) -> Option<Inline> {
        let raw: String = self
            .text
            .chars
            .get(i..=close)
            .unwrap_or(&[])
            .iter()
            .collect();
        let inner = raw.get(1..raw.len() - 1)?;
        let (style, payload) = citation_parts(inner)?;
        let citations = parse_citation_items(payload, style, self.text.ext, self.text.notes)?;
        Some(Inline::Cite(citations, plain_words(&raw)))
    }

    // Angle brackets: targets and autolinks

    fn scan_angle(&self, i: usize) -> Option<(Inline, usize)> {
        if self.at(i + 1) == Some('<') {
            let name_start = i + 2;
            let j = self.next_close(name_start, '>', None)?;
            if !self.within_lines(name_start, j, 0)
                || self
                    .next_close(name_start, '<', None)
                    .is_some_and(|open| open < j)
            {
                return None;
            }
            if j > name_start && self.at(j) == Some('>') && self.at(j + 1) == Some('>') {
                let name: String = self
                    .text
                    .chars
                    .get(name_start..j)
                    .unwrap_or(&[])
                    .iter()
                    .collect();
                if name
                    .chars()
                    .any(|character| matches!(character, ' ' | '\t' | '\n' | '\'' | '"'))
                {
                    return None;
                }
                let attr = Attr {
                    id: name.into(),
                    ..Attr::default()
                };
                // A target absorbs the whitespace that follows it.
                let mut end = j + 2;
                while matches!(self.at(end), Some(' ' | '\t')) {
                    end += 1;
                }
                return Some((Inline::Span(Box::new(attr), Vec::new()), end));
            }
            return None;
        }
        let j = self.next_close(i + 1, '>', None)?;
        if !self.within_lines(i + 1, j, 0) {
            return None;
        }
        let content: String = self
            .text
            .chars
            .get(i + 1..j)
            .unwrap_or(&[])
            .iter()
            .collect();
        if is_uri(&content) {
            return Some((
                link(&content, vec![Inline::Str(content.clone().into())]),
                j + 1,
            ));
        }
        None
    }

    // Dollar math

    fn scan_math_dollar(&self, i: usize, prev: Option<char>) -> Option<(Inline, usize)> {
        if self.at(i + 1) == Some('$') {
            let start = i + 2;
            let end = self.next_close(start, '$', Some('$'))?;
            let inner: String = self
                .text
                .chars
                .get(start..end)
                .unwrap_or(&[])
                .iter()
                .collect();
            return Some((Inline::Math(MathType::DisplayMath, inner.into()), end + 2));
        }
        // Inline `$…$` with word-boundary and border constraints.
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '$') {
            return None;
        }
        let first = self.at(i + 1)?;
        if first.is_whitespace() || first == '$' {
            return None;
        }
        let end = self.next_close(i + 1, '$', None)?;
        if !self.within_lines(i + 1, end, 0) {
            return None;
        }
        let inner: String = self
            .text
            .chars
            .get(i + 1..end)
            .unwrap_or(&[])
            .iter()
            .collect();
        Some((Inline::Math(MathType::InlineMath, inner.into()), end + 1))
    }

    // Inline export

    fn scan_export(&self, i: usize) -> Option<(Inline, usize)> {
        if self.at(i + 1) != Some('@') {
            return None;
        }
        let fmt_start = i + 2;
        let mut j = fmt_start;
        while matches!(self.at(j), Some(c) if c.is_ascii_alphanumeric() || c == '-') {
            j += 1;
        }
        if self.at(j) != Some(':') || j == fmt_start {
            return None;
        }
        let fmt: String = self
            .text
            .chars
            .get(fmt_start..j)
            .unwrap_or(&[])
            .iter()
            .collect();
        let content_start = j + 1;
        let end = self.next_close(content_start, '@', Some('@'))?;
        let content: String = self
            .text
            .chars
            .get(content_start..end)
            .unwrap_or(&[])
            .iter()
            .collect();
        Some((
            Inline::RawInline(Format(fmt.into()), content.into()),
            end + 2,
        ))
    }

    // Smart quotes

    fn scan_quote(&self, i: usize, quote: char) -> Option<(QuoteType, usize)> {
        let (kind, close) = if quote == '"' {
            (QuoteType::DoubleQuote, '"')
        } else {
            (QuoteType::SingleQuote, '\'')
        };
        // The opening quote must be followed immediately by a non-space character.
        if !matches!(self.at(i + 1), Some(c) if !c.is_whitespace()) {
            return None;
        }
        let end = self.next_close(i + 1, close, None)?;
        self.within_lines(i + 1, end, 0).then_some((kind, end))
    }

    // Special strings

    /// Replaces `---`/`--` with em/en dashes and `...` with an ellipsis.
    fn scan_special_string(&self, i: usize) -> Option<(&'static str, usize)> {
        if self.at(i) == Some('.') {
            if self.at(i + 1) == Some('.') && self.at(i + 2) == Some('.') {
                return Some(("\u{2026}", i + 3));
            }
            return None;
        }
        if self.at(i + 1) == Some('-') {
            if self.at(i + 2) == Some('-') {
                return Some(("\u{2014}", i + 3));
            }
            return Some(("\u{2013}", i + 2));
        }
        None
    }

    // Bare autolinks

    fn scan_bare_url(&self, i: usize) -> Option<(String, usize)> {
        const SCHEMES: [&str; 3] = ["https://", "http://", "ftp://"];
        let scheme = SCHEMES.iter().find(|s| self.matches_str(i, s)).copied()?;
        let mut j = i + scheme.chars().count();
        while matches!(self.at(j), Some(c) if !c.is_whitespace() && !matches!(c, '<' | '>' | '(' | ')' | '[' | ']'))
        {
            j += 1;
        }
        while j > i + scheme.chars().count()
            && self
                .at(j - 1)
                .is_some_and(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"'))
        {
            j -= 1;
        }
        let url: String = self.text.chars.get(i..j).unwrap_or(&[]).iter().collect();
        Some((url, j))
    }

    // Low-level matching

    fn matches_at(&self, i: usize, pat: &[char]) -> bool {
        pat.iter()
            .enumerate()
            .all(|(k, &c)| self.at(i + k) == Some(c))
    }

    fn matches_str(&self, i: usize, pat: &str) -> bool {
        pat.chars()
            .enumerate()
            .all(|(k, c)| self.at(i + k) == Some(c))
    }
}

/// The style and the payload of the citation text between its brackets.
fn citation_parts(inner: &str) -> Option<(Option<&str>, &str)> {
    let rest = inner.strip_prefix("cite")?;
    match rest.strip_prefix('/') {
        Some(after) => {
            let (style, payload) = after.split_once(':')?;
            Some((Some(style), payload))
        }
        None => Some((None, rest.strip_prefix(':')?)),
    }
}
