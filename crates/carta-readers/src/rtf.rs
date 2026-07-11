//! Reader for the Rich Text Format (RTF), the word-processor interchange language.
//!
//! An RTF document is a tree of brace-delimited groups holding three things: control words
//! (`\word`, optionally with a numeric argument), control symbols (`\` before a single non-letter),
//! and literal text. A group scopes formatting: entering one saves the character state, leaving one
//! restores it. Reading proceeds in one pass over a token stream ([`tokenize`]), with a stack of
//! group states carrying the active character formatting and a stack of block-building contexts (a
//! fresh one opens for each footnote).
//!
//! Character control words toggle formatting (`\b`, `\i`, `\ul`, `\strike`, `\super`, `\sub`,
//! `\scaps`, `\caps`); text between them is wrapped in the corresponding inline nodes, nested in a
//! fixed order and coalesced so a run that stays bold across an italic span keeps one enclosing
//! bold. Paragraph breaks (`\par`) close paragraphs; `\outlinelevelN` turns one into a heading;
//! `\line` is a hard break. Encoded characters arrive as `\'xx` (a byte in the ANSI code page) or
//! `\uN` (a Unicode scalar with a following fallback the reader skips). Structural groups are
//! recognized by their leading destination word: `\info` fills document metadata, `\pict` decodes an
//! embedded image into the media bag, `\field` unpacks a hyperlink, `\footnote` becomes a note, and
//! `\*\bkmkstart`/`\*\bkmkend` bracket a bookmark span. Font, color, and style tables — and any
//! group flagged ignorable with `\*` — are skipped. A run of `\trowd`/`\cell`/`\row` rows assembles
//! a table.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::mem::take;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MetaValue, Row, Table, TableBody, Target, Text,
};
use carta_core::media::content_addressed_name;
use carta_core::{BytesReader, MediaBag, ReaderOptions, Result};

use crate::inline_text::words_to_inlines;
use crate::numeric::general_decimal;

/// Parses a Rich Text Format document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct RtfReader;

impl BytesReader for RtfReader {
    fn read(&self, input: &[u8], options: &ReaderOptions) -> Result<Document> {
        Ok(self.read_media(input, options)?.0)
    }

    fn read_media(&self, input: &[u8], _options: &ReaderOptions) -> Result<(Document, MediaBag)> {
        let text = decode_input(input);
        let tokens = tokenize(&text);
        let mut parser = Parser::new(tokens);
        parser.run();
        let (meta, blocks, media) = parser.finish();
        Ok((
            Document {
                meta,
                blocks,
                ..Document::default()
            },
            media,
        ))
    }
}

/// Decodes the input bytes to text. The stream is UTF-8 when it parses as such; otherwise each byte
/// is taken as its own code point (an 8-bit Latin-1 reading), the layer the wire form falls back to
/// so a document carrying raw high bytes still reads rather than being rejected. A `\'xx` escape is
/// unaffected either way, since it is spelled in ASCII and resolved through the code page later.
fn decode_input(input: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(input) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => Cow::Owned(input.iter().map(|&byte| byte as char).collect()),
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// One lexical unit of an RTF stream.
#[derive(Debug, Clone)]
enum Token {
    /// `{` — opens a group.
    GroupStart,
    /// `}` — closes a group.
    GroupEnd,
    /// `\word` with an optional trailing numeric argument.
    Control(String, Option<i32>),
    /// `\` before a single non-letter character (e.g. `\~`, `\-`, `\\`).
    Symbol(char),
    /// `\'xx` — a raw byte in the document's code page.
    Hex(u8),
    /// The raw bytes introduced by a `\binN` control word: exactly `N` bytes of embedded binary,
    /// carried opaque so their values never re-enter lexing as braces, backslashes, or text.
    Binary(Vec<u8>),
    /// A literal text character.
    Char(char),
    /// A literal space; a run of them collapses when emitted.
    Space,
}

/// Splits an RTF source string into its token stream. Carriage returns, line feeds, and literal
/// tabs are structural whitespace in the wire form and carry no content, so they are dropped.
fn tokenize(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '{' => {
                tokens.push(Token::GroupStart);
                i += 1;
            }
            '}' => {
                tokens.push(Token::GroupEnd);
                i += 1;
            }
            '\\' => {
                i = lex_backslash(&chars, i, &mut tokens);
            }
            '\r' | '\n' | '\t' => i += 1,
            ' ' => {
                tokens.push(Token::Space);
                i += 1;
            }
            _ => {
                tokens.push(Token::Char(c));
                i += 1;
            }
        }
    }
    tokens
}

/// Whether `c` is the delimiter that ends a control word and is consumed with it. A space always
/// qualifies; any other single printable punctuation mark does too, except the three characters that
/// open their own token (`{` and `}` begin and end groups, `\` starts the next control sequence) and
/// letters or digits, which would extend the word or its numeric parameter instead.
fn is_control_delimiter(c: char) -> bool {
    c == ' '
        || (c.is_ascii_graphic() && !c.is_ascii_alphanumeric() && !matches!(c, '{' | '}' | '\\'))
}

/// Lexes one backslash-introduced token starting at `start` (the backslash). Returns the index just
/// past what was consumed.
fn lex_backslash(chars: &[char], start: usize, tokens: &mut Vec<Token>) -> usize {
    let mut i = start + 1;
    match chars.get(i) {
        None => i,
        Some(&n) if n.is_ascii_alphabetic() => {
            let word_start = i;
            while matches!(chars.get(i), Some(c) if c.is_ascii_alphabetic()) {
                i += 1;
            }
            let word: String = chars.get(word_start..i).unwrap_or(&[]).iter().collect();
            let negative = matches!(chars.get(i), Some('-'));
            let digits_start = if negative { i + 1 } else { i };
            let mut j = digits_start;
            while matches!(chars.get(j), Some(c) if c.is_ascii_digit()) {
                j += 1;
            }
            let param = if j > digits_start {
                let digits: String = chars.get(digits_start..j).unwrap_or(&[]).iter().collect();
                i = j;
                digits.parse::<i64>().ok().map(|value| {
                    let signed = if negative { -value } else { value };
                    let clamped = signed.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
                    i32::try_from(clamped).unwrap_or(0)
                })
            } else {
                None
            };
            // A control word ends at a delimiter, which is absorbed with the word when it is a
            // space or a lone punctuation mark that does not itself begin another token.
            if matches!(chars.get(i), Some(&c) if is_control_delimiter(c)) {
                i += 1;
            }
            // `\binN` is followed by exactly N bytes of embedded binary. They are captured here, at
            // the lexer, so their values never reach the group/text grammar: a `{`, `}`, or `\`
            // among them is data, not structure, and a raw-byte picture cannot desync brace nesting.
            if word == "bin"
                && let Some(count) = param.and_then(|value| usize::try_from(value).ok())
                && count > 0
            {
                let end = i.saturating_add(count).min(chars.len());
                let bytes = chars
                    .get(i..end)
                    .unwrap_or(&[])
                    .iter()
                    .map(|&c| u8::try_from(u32::from(c) & 0xFF).unwrap_or(0))
                    .collect();
                tokens.push(Token::Binary(bytes));
                return end;
            }
            tokens.push(Token::Control(word, param));
            i
        }
        Some(&'\'') => {
            i += 1;
            let hi = chars.get(i).and_then(|c| c.to_digit(16));
            let lo = chars.get(i + 1).and_then(|c| c.to_digit(16));
            match (hi, lo) {
                (Some(hi), Some(lo)) => {
                    tokens.push(Token::Hex(u8::try_from((hi << 4) | lo).unwrap_or(0)));
                    i + 2
                }
                _ => i,
            }
        }
        Some(&symbol) => {
            tokens.push(Token::Symbol(symbol));
            i + 1
        }
    }
}

// ---------------------------------------------------------------------------
// Character state
// ---------------------------------------------------------------------------

/// The active character formatting. Copied on group entry and restored on exit. Compared for
/// equality to merge adjacent text sharing the same formatting into one run. Each field is an
/// independent on/off attribute the format toggles separately, so a flat set of flags models it
/// directly.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct CharProps {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    superscript: bool,
    subscript: bool,
    smallcaps: bool,
    allcaps: bool,
    hidden: bool,
    /// The active font belongs to the monospace (fixed-pitch) family, so its text is code: a run
    /// spanning a whole paragraph becomes a code block, a shorter run inline code.
    mono: bool,
}

impl CharProps {
    /// Whether two runs share the same inline wrappers, so a text run stays unbroken across them.
    /// `allcaps` is folded into each character as it is pushed and `hidden` content is dropped before
    /// it becomes a run, so neither contributes a wrapper and neither alone splits a run.
    fn same_run(self, other: Self) -> bool {
        Self {
            allcaps: false,
            hidden: false,
            ..self
        } == Self {
            allcaps: false,
            hidden: false,
            ..other
        }
    }

    /// Whether this state yields the same inline wrapper path as `other` (see [`wrappers`]), comparing
    /// only the wrapper-bearing attributes so no path is built.
    fn same_wrappers(self, other: Self) -> bool {
        self.bold == other.bold
            && self.italic == other.italic
            && self.strike == other.strike
            && self.subscript == other.subscript
            && self.superscript == other.superscript
            && self.smallcaps == other.smallcaps
            && self.underline == other.underline
    }

    /// Whether this state carries any inline wrapper at all (see [`wrappers`]).
    fn has_wrapper(self) -> bool {
        self.bold
            || self.italic
            || self.strike
            || self.subscript
            || self.superscript
            || self.smallcaps
            || self.underline
    }
}

/// The character formatting a paragraph style contributes. Each field is set only for an attribute
/// the style declares, so applying the style overrides exactly those attributes and leaves the rest
/// inherited. `font` records the style's selected font number, resolved to monospace membership when
/// the style is applied so it tracks the font table regardless of table order.
#[derive(Debug, Clone, Copy, Default)]
struct StyleFormat {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    superscript: Option<bool>,
    subscript: Option<bool>,
    smallcaps: Option<bool>,
    allcaps: Option<bool>,
    hidden: Option<bool>,
    font: Option<i32>,
}

impl StyleFormat {
    /// Folds one control word from a style definition into the accumulating format.
    fn apply_control(&mut self, word: &str, param: Option<i32>) {
        let on = param != Some(0);
        match word {
            "b" => self.bold = Some(on),
            "i" => self.italic = Some(on),
            "ul" => self.underline = Some(on),
            "ulnone" => self.underline = Some(false),
            "uld" | "uldb" | "ulw" | "uldash" | "uldashd" | "uldashdd" | "ulhwave" | "ulth"
            | "ulthd" | "ulwave" => self.underline = Some(true),
            "strike" | "striked" => self.strike = Some(on),
            "super" | "superscript" => self.superscript = Some(on),
            "sub" | "subscript" => self.subscript = Some(on),
            "nosupersub" => {
                self.superscript = Some(false);
                self.subscript = Some(false);
            }
            "scaps" => self.smallcaps = Some(on),
            "caps" => self.allcaps = Some(on),
            "v" => self.hidden = Some(on),
            "plain" => {
                let font = self.font;
                *self = Self {
                    bold: Some(false),
                    italic: Some(false),
                    underline: Some(false),
                    strike: Some(false),
                    superscript: Some(false),
                    subscript: Some(false),
                    smallcaps: Some(false),
                    allcaps: Some(false),
                    hidden: Some(false),
                    font,
                };
            }
            "f" => self.font = param,
            _ => {}
        }
    }
}

/// A group's saved state: the character formatting plus the Unicode fallback skip count (`\ucN`).
#[derive(Debug, Clone, Copy)]
struct GroupState {
    props: CharProps,
    uc: i32,
}

impl Default for GroupState {
    fn default() -> Self {
        Self {
            props: CharProps::default(),
            uc: 1,
        }
    }
}

/// One enclosing inline wrapper. The declaration order is the nesting order applied to a run:
/// earlier variants wrap later ones, regardless of the order the source enabled them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wrapper {
    Strong,
    Emph,
    Strikeout,
    Subscript,
    Superscript,
    SmallCaps,
    Underline,
}

impl Wrapper {
    fn wrap(self, children: Vec<Inline>) -> Inline {
        match self {
            Wrapper::Strong => Inline::Strong(children),
            Wrapper::Emph => Inline::Emph(children),
            Wrapper::Strikeout => Inline::Strikeout(children),
            Wrapper::Subscript => Inline::Subscript(children),
            Wrapper::Superscript => Inline::Superscript(children),
            Wrapper::SmallCaps => Inline::SmallCaps(children),
            Wrapper::Underline => Inline::Underline(children),
        }
    }
}

/// The wrapper path implied by a character state, outermost first.
fn wrappers(props: CharProps) -> Vec<Wrapper> {
    let mut path = Vec::new();
    if props.bold {
        path.push(Wrapper::Strong);
    }
    if props.italic {
        path.push(Wrapper::Emph);
    }
    if props.strike {
        path.push(Wrapper::Strikeout);
    }
    if props.subscript {
        path.push(Wrapper::Subscript);
    }
    if props.superscript {
        path.push(Wrapper::Superscript);
    }
    if props.smallcaps {
        path.push(Wrapper::SmallCaps);
    }
    if props.underline {
        path.push(Wrapper::Underline);
    }
    path
}

// ---------------------------------------------------------------------------
// Inline assembly
// ---------------------------------------------------------------------------

/// A leaf produced within a paragraph, tagged with the formatting active when it was emitted.
#[derive(Debug, Clone)]
struct Atom {
    props: CharProps,
    kind: AtomKind,
}

#[derive(Debug, Clone)]
enum AtomKind {
    Text(String),
    Space,
    LineBreak,
    /// An already-built inline (link, image, note, or bookmark span) inserted verbatim.
    Node(Inline),
}

/// One level of the bookmark nesting active while a paragraph is being built. The root frame has no
/// bookmark; each `\*\bkmkstart` pushes a named frame that `\*\bkmkend` folds into a span.
#[derive(Debug, Clone)]
struct Frame {
    bookmark: Option<Text>,
    atoms: Vec<Atom>,
}

/// Builds nested inlines from a flat, formatting-tagged atom sequence. Adjacent atoms sharing a
/// common wrapper prefix stay inside a single instance of that wrapper; a divergence closes the
/// wrappers past the shared prefix and opens the new ones.
/// Unwraps a bold or italic emphasis that spans an entire heading. A heading's level already conveys
/// prominence, so a single `Strong` or `Emph` enclosing all of its content is replaced by that
/// content, repeatedly while one remains (so nested bold-in-italic collapses fully). Emphasis over
/// only part of the heading, or any other kind of wrapper, is left untouched.
fn strip_heading_emphasis(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while inlines.len() == 1 {
        match inlines.first() {
            Some(Inline::Strong(_) | Inline::Emph(_)) => {}
            _ => break,
        }
        match inlines.pop() {
            Some(Inline::Strong(children) | Inline::Emph(children)) => inlines = children,
            _ => break,
        }
    }
    inlines
}

fn build_inlines(atoms: Vec<Atom>) -> Vec<Inline> {
    let atoms = collapse_mono(atoms);
    let mut root: Vec<Inline> = Vec::new();
    let mut open: Vec<(Wrapper, Vec<Inline>)> = Vec::new();

    let close_to =
        |open: &mut Vec<(Wrapper, Vec<Inline>)>, root: &mut Vec<Inline>, depth: usize| {
            while open.len() > depth {
                if let Some((wrapper, children)) = open.pop() {
                    let inline = wrapper.wrap(children);
                    match open.last_mut() {
                        Some((_, parent)) => parent.push(inline),
                        None => root.push(inline),
                    }
                }
            }
        };

    for atom in atoms {
        let path = wrappers(atom.props);
        let mut shared = 0;
        while shared < open.len()
            && open
                .get(shared)
                .zip(path.get(shared))
                .is_some_and(|((wrapper, _), next)| wrapper == next)
        {
            shared += 1;
        }
        close_to(&mut open, &mut root, shared);
        for &wrapper in path.get(shared..).unwrap_or(&[]) {
            open.push((wrapper, Vec::new()));
        }
        let base = match atom.kind {
            AtomKind::Text(text) => Inline::Str(text.into()),
            AtomKind::Space => Inline::Space,
            AtomKind::LineBreak => Inline::LineBreak,
            AtomKind::Node(node) => node,
        };
        match open.last_mut() {
            Some((_, children)) => children.push(base),
            None => root.push(base),
        }
    }
    close_to(&mut open, &mut root, 0);
    root
}

/// Collapses each maximal run of contiguous monospace leaf atoms that share the same inline
/// wrappers into one code node, joining their text with a space for each [`AtomKind::Space`] and a
/// newline for each [`AtomKind::LineBreak`]. Atoms outside the monospace family, and already-built
/// nodes, pass through untouched, so a code run ends at the first differing wrapper, non-code atom,
/// or embedded node.
fn collapse_mono(atoms: Vec<Atom>) -> Vec<Atom> {
    let mut out: Vec<Atom> = Vec::new();
    let mut run: Vec<Atom> = Vec::new();
    let flush = |run: &mut Vec<Atom>, out: &mut Vec<Atom>| {
        let Some(first) = run.first() else {
            return;
        };
        let mut props = first.props;
        props.mono = false;
        let mut code = String::new();
        for atom in run.iter() {
            match &atom.kind {
                AtomKind::Text(text) => code.push_str(text),
                AtomKind::Space => code.push(' '),
                AtomKind::LineBreak => code.push('\n'),
                AtomKind::Node(_) => {}
            }
        }
        out.push(Atom {
            props,
            kind: AtomKind::Node(Inline::Code(Box::default(), code.into())),
        });
        run.clear();
    };
    for atom in atoms {
        let mono_leaf = atom.props.mono && !matches!(atom.kind, AtomKind::Node(_));
        if mono_leaf {
            let split = run
                .first()
                .is_some_and(|first| !first.props.same_wrappers(atom.props));
            if split {
                flush(&mut run, &mut out);
            }
            run.push(atom);
        } else {
            flush(&mut run, &mut out);
            out.push(atom);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// When every atom of a paragraph is monospace text carrying no other inline formatting, returns the
/// paragraph body as code (a space for each [`AtomKind::Space`], a newline for each
/// [`AtomKind::LineBreak`]); otherwise returns `None`, so the paragraph is built as inline content.
fn mono_code_block(atoms: &[Atom]) -> Option<String> {
    let mut code = String::new();
    for atom in atoms {
        if !atom.props.mono || atom.props.has_wrapper() {
            return None;
        }
        match &atom.kind {
            AtomKind::Text(text) => code.push_str(text),
            AtomKind::Space => code.push(' '),
            AtomKind::LineBreak => code.push('\n'),
            AtomKind::Node(_) => return None,
        }
    }
    Some(code)
}

/// Distributes a hyperlink over already-built display inlines, hoisting character-formatting
/// wrappers outside the link. Adjacent inlines that carry no wrapper share a single link; each
/// formatting wrapper stays outside and has the link distributed into its children.
fn linkify(target: &Target, inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut run: Vec<Inline> = Vec::new();
    for inline in inlines {
        match into_wrapper(inline) {
            Ok((wrapper, children)) => {
                flush_link(target, &mut run, &mut out);
                out.push(wrapper.wrap(linkify(target, children)));
            }
            Err(leaf) => run.push(leaf),
        }
    }
    flush_link(target, &mut run, &mut out);
    out
}

/// Emits any accumulated non-wrapper inlines as a single link, resetting the run.
fn flush_link(target: &Target, run: &mut Vec<Inline>, out: &mut Vec<Inline>) {
    if !run.is_empty() {
        out.push(Inline::Link(
            Box::default(),
            take(run),
            Box::new(target.clone()),
        ));
    }
}

/// Decomposes an inline into its character-formatting wrapper and children, or returns it unchanged
/// when it is not one of the wrappers a run can carry.
fn into_wrapper(inline: Inline) -> std::result::Result<(Wrapper, Vec<Inline>), Inline> {
    match inline {
        Inline::Strong(children) => Ok((Wrapper::Strong, children)),
        Inline::Emph(children) => Ok((Wrapper::Emph, children)),
        Inline::Strikeout(children) => Ok((Wrapper::Strikeout, children)),
        Inline::Subscript(children) => Ok((Wrapper::Subscript, children)),
        Inline::Superscript(children) => Ok((Wrapper::Superscript, children)),
        Inline::SmallCaps(children) => Ok((Wrapper::SmallCaps, children)),
        Inline::Underline(children) => Ok((Wrapper::Underline, children)),
        other => Err(other),
    }
}

// ---------------------------------------------------------------------------
// Block context
// ---------------------------------------------------------------------------

/// One block-building context: the emitted blocks plus the paragraph and table under construction.
/// The document has one; every footnote opens another.
#[derive(Debug)]
struct Emitter {
    blocks: Vec<Block>,
    frames: Vec<Frame>,
    pending_text: String,
    pending_props: CharProps,
    pending_space: bool,
    space_props: CharProps,
    has_content: bool,
    /// Keeps a space at the leading or trailing edge of the content instead of trimming it. A
    /// paragraph trims its edge whitespace, but a hyperlink's display text is inline and its edge
    /// space is meaningful — it separates the link from the word beside it — so it is preserved.
    preserve_edge_space: bool,
    outline_level: Option<i32>,
    in_table_para: bool,
    rows: Vec<Row>,
    cells: Vec<Cell>,
    cell_blocks: Vec<Block>,
    columns: usize,
    row_cell_bounds: usize,
    list_active: bool,
    list_id: i32,
    list_level: i32,
    list_levels: Vec<LevelDef>,
    pending_list: Vec<ListParagraph>,
    /// Fallback buffer returned by [`Emitter::frame_atoms`] only if the frame stack is ever empty,
    /// which its guard prevents; keeps the accessor total without a panic or a leak.
    scratch_atoms: Vec<Atom>,
}

/// One paragraph belonging to a list, tagged with the list it selects (`\ls`), its nesting level
/// (`\ilvl`) and, when the level's definition numbers it, the start value and numeral style read from
/// the list table. Consecutive entries are reassembled into nested lists when the list run ends: a
/// numbered level becomes an [`Block::OrderedList`], an unnumbered one a [`Block::BulletList`]. Two
/// adjacent paragraphs that select different lists stay in separate sibling lists even at the same
/// level, so a numbered list directly followed by a bulleted one is not fused.
#[derive(Debug)]
struct ListParagraph {
    list_id: i32,
    level: i32,
    numbering: Option<(i32, ListNumberStyle)>,
    block: Block,
}

/// One list level's marker configuration, read from a `\listlevel` group: the numeral style (absent
/// when the level is a bullet or carries no number) and the start value (`\levelstartat`, default 1).
#[derive(Debug, Clone, Copy)]
struct LevelDef {
    style: Option<ListNumberStyle>,
    start: i32,
}

impl LevelDef {
    /// The start value and numeral style when this level is numbered; `None` when it is a bullet.
    fn numbering(self) -> Option<(i32, ListNumberStyle)> {
        self.style.map(|style| (self.start, style))
    }
}

/// Maps a `\levelnfc` number-format code to a numeral style. Bullet (`23`) and unnumbered (`255`)
/// levels, and a level that declares no format, carry no numeral style and render as bullets; every
/// other code is a numbered level, with the common decimal, roman, and alphabetic codes named and
/// the rest left to the target format's default numbering.
fn nfc_to_style(nfc: Option<i32>) -> Option<ListNumberStyle> {
    match nfc {
        None | Some(23 | 255) => None,
        Some(0) => Some(ListNumberStyle::Decimal),
        Some(1) => Some(ListNumberStyle::UpperRoman),
        Some(2) => Some(ListNumberStyle::LowerRoman),
        Some(3) => Some(ListNumberStyle::UpperAlpha),
        Some(4) => Some(ListNumberStyle::LowerAlpha),
        Some(_) => Some(ListNumberStyle::DefaultStyle),
    }
}

impl Emitter {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            frames: vec![Frame {
                bookmark: None,
                atoms: Vec::new(),
            }],
            pending_text: String::new(),
            pending_props: CharProps::default(),
            pending_space: false,
            space_props: CharProps::default(),
            has_content: false,
            preserve_edge_space: false,
            outline_level: None,
            in_table_para: false,
            rows: Vec::new(),
            cells: Vec::new(),
            cell_blocks: Vec::new(),
            columns: 0,
            row_cell_bounds: 0,
            list_active: false,
            list_id: 0,
            list_level: 0,
            list_levels: Vec::new(),
            pending_list: Vec::new(),
            scratch_atoms: Vec::new(),
        }
    }

    fn frame_atoms(&mut self) -> &mut Vec<Atom> {
        if self.frames.is_empty() {
            self.frames.push(Frame {
                bookmark: None,
                atoms: Vec::new(),
            });
        }
        // The guard above guarantees a last element; the scratch buffer is dead-code fallback.
        match self.frames.last_mut() {
            Some(frame) => &mut frame.atoms,
            None => &mut self.scratch_atoms,
        }
    }

    fn flush_text(&mut self) {
        if !self.pending_text.is_empty() {
            let props = self.pending_props;
            let text = take(&mut self.pending_text);
            self.frame_atoms().push(Atom {
                props,
                kind: AtomKind::Text(text),
            });
        }
    }

    fn resolve_space(&mut self) {
        if self.pending_space {
            self.pending_space = false;
            if self.has_content || self.preserve_edge_space {
                let props = self.space_props;
                self.frame_atoms().push(Atom {
                    props,
                    kind: AtomKind::Space,
                });
                self.has_content = true;
            }
        }
    }

    fn push_char(&mut self, c: char, props: CharProps) {
        if props.hidden {
            return;
        }
        self.resolve_space();
        if !self.pending_text.is_empty() && !self.pending_props.same_run(props) {
            self.flush_text();
        }
        if self.pending_text.is_empty() {
            self.pending_props = props;
        }
        if props.allcaps {
            for upper in c.to_uppercase() {
                self.pending_text.push(upper);
            }
        } else {
            self.pending_text.push(c);
        }
        self.has_content = true;
    }

    fn push_str(&mut self, s: &str, props: CharProps) {
        for c in s.chars() {
            self.push_char(c, props);
        }
    }

    fn push_space(&mut self, props: CharProps) {
        if props.hidden {
            return;
        }
        self.flush_text();
        if !self.pending_space {
            self.pending_space = true;
            self.space_props = props;
        }
    }

    fn push_break(&mut self, props: CharProps) {
        if props.hidden {
            return;
        }
        self.flush_text();
        self.resolve_space();
        self.frame_atoms().push(Atom {
            props,
            kind: AtomKind::LineBreak,
        });
        self.has_content = true;
    }

    fn push_node(&mut self, node: Inline, props: CharProps) {
        if props.hidden {
            return;
        }
        self.flush_text();
        self.resolve_space();
        self.frame_atoms().push(Atom {
            props,
            kind: AtomKind::Node(node),
        });
        self.has_content = true;
    }

    fn open_bookmark(&mut self, id: Text) {
        self.flush_text();
        self.resolve_space();
        self.frames.push(Frame {
            bookmark: Some(id),
            atoms: Vec::new(),
        });
    }

    fn close_bookmark(&mut self) {
        self.flush_text();
        self.resolve_space();
        if self.frames.len() <= 1 {
            return;
        }
        if let Some(frame) = self.frames.pop()
            && self.fold_bookmark(frame)
        {
            self.has_content = true;
        }
    }

    /// Wraps a popped bookmark frame into a span node in its parent frame, and reports whether it
    /// produced one. A bookmark with no content between its start and end — the point anchors a word
    /// processor scatters for every cross-reference and revision cursor — carries no span and is
    /// dropped, so an empty span never appears in the output.
    fn fold_bookmark(&mut self, frame: Frame) -> bool {
        let id = frame.bookmark.unwrap_or_default();
        let inlines = build_inlines(frame.atoms);
        if inlines.is_empty() {
            return false;
        }
        let span = Inline::Span(
            Box::new(Attr {
                id,
                classes: Vec::new(),
                attributes: Vec::new(),
            }),
            inlines,
        );
        self.frame_atoms().push(Atom {
            props: CharProps::default(),
            kind: AtomKind::Node(span),
        });
        true
    }

    /// Closes the current paragraph's atom sequence, folding any unclosed bookmarks into spans.
    fn take_atoms(&mut self) -> Vec<Atom> {
        self.flush_text();
        if self.preserve_edge_space && self.pending_space {
            let props = self.space_props;
            self.frame_atoms().push(Atom {
                props,
                kind: AtomKind::Space,
            });
        }
        self.pending_space = false;
        while self.frames.len() > 1 {
            if let Some(frame) = self.frames.pop() {
                self.fold_bookmark(frame);
            }
        }
        self.has_content = false;
        match self.frames.first_mut() {
            Some(frame) => take(&mut frame.atoms),
            None => Vec::new(),
        }
    }

    /// Closes the current paragraph's inline content, folding any unclosed bookmarks into spans.
    fn take_inlines(&mut self) -> Vec<Inline> {
        build_inlines(self.take_atoms())
    }

    /// Ends a paragraph: the accumulated content becomes a heading, a code block, or a paragraph,
    /// routed into the current table cell when the paragraph is marked in-table, otherwise into the
    /// block stream. A paragraph whose every run is monospace and otherwise unformatted is a code
    /// block; an outline level makes it a heading.
    fn end_paragraph(&mut self) {
        let mut atoms = self.take_atoms();
        // A hard break immediately before the paragraph boundary renders no line of its own; one such
        // trailing break is dropped so the paragraph does not end on a dangling break. A paragraph
        // left with no content after this — a line that held only the break — is emitted as nothing.
        if atoms
            .last()
            .is_some_and(|atom| matches!(atom.kind, AtomKind::LineBreak))
        {
            atoms.pop();
        }
        if atoms.is_empty() {
            return;
        }
        let block = if let Some(level) = self.outline_level {
            Block::Header(
                level.saturating_add(1).max(1),
                Box::default(),
                strip_heading_emphasis(build_inlines(atoms)),
            )
        } else if let Some(code) = mono_code_block(&atoms) {
            Block::CodeBlock(Box::default(), code.into())
        } else {
            Block::Para(build_inlines(atoms))
        };
        if self.in_table_para {
            self.cell_blocks.push(block);
        } else if self.list_active {
            // A list paragraph joins the pending run; any open table is closed ahead of it so the
            // list and table keep their source order.
            self.finish_table();
            let numbering = usize::try_from(self.list_level)
                .ok()
                .and_then(|index| self.list_levels.get(index))
                .and_then(|level| level.numbering());
            self.pending_list.push(ListParagraph {
                list_id: self.list_id,
                level: self.list_level,
                numbering,
                block,
            });
        } else {
            self.flush_list();
            self.finish_table();
            self.blocks.push(block);
        }
    }

    /// Emits the buffered list paragraphs as nested lists, in source order.
    fn flush_list(&mut self) {
        if self.pending_list.is_empty() {
            return;
        }
        let entries = take(&mut self.pending_list);
        self.blocks.extend(build_lists(&entries));
    }

    fn end_cell(&mut self) {
        self.end_paragraph();
        let content = take(&mut self.cell_blocks);
        self.cells.push(Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content,
        });
        self.columns = self.columns.max(self.cells.len());
    }

    fn end_row(&mut self) {
        self.end_paragraph();
        if !self.cell_blocks.is_empty() {
            let content = take(&mut self.cell_blocks);
            self.cells.push(Cell {
                attr: Attr::default(),
                align: Alignment::AlignDefault,
                row_span: 1,
                col_span: 1,
                content,
            });
        }
        if !self.cells.is_empty() {
            let cells = take(&mut self.cells);
            self.columns = self.columns.max(cells.len());
            self.rows.push(Row {
                attr: Attr::default(),
                cells,
            });
        }
        self.in_table_para = false;
    }

    fn begin_row_definition(&mut self) {
        self.row_cell_bounds = 0;
    }

    fn note_cell_boundary(&mut self) {
        self.row_cell_bounds += 1;
        self.columns = self.columns.max(self.row_cell_bounds);
    }

    /// Emits any pending table rows as a [`Block::Table`]. A row shorter than the widest is padded
    /// with empty cells so every row spans the full column count.
    fn finish_table(&mut self) {
        if !self.cells.is_empty() {
            let cells = take(&mut self.cells);
            self.columns = self.columns.max(cells.len());
            self.rows.push(Row {
                attr: Attr::default(),
                cells,
            });
        }
        if self.rows.is_empty() {
            self.columns = 0;
            return;
        }
        let columns = self.columns.max(1);
        let mut rows = take(&mut self.rows);
        for row in &mut rows {
            while row.cells.len() < columns {
                row.cells.push(Cell {
                    attr: Attr::default(),
                    align: Alignment::AlignDefault,
                    row_span: 1,
                    col_span: 1,
                    content: Vec::new(),
                });
            }
        }
        let col_specs = (0..columns)
            .map(|_| ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            })
            .collect();
        let table = Table {
            attr: Attr::default(),
            caption: Caption::default(),
            col_specs,
            head: carta_ast::TableHead::default(),
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: rows,
            }],
            foot: carta_ast::TableFoot::default(),
        };
        self.blocks.push(Block::Table(Box::new(table)));
        self.columns = 0;
    }

    fn finish_blocks(mut self) -> Vec<Block> {
        self.end_paragraph();
        self.flush_list();
        self.finish_table();
        self.blocks
    }

    /// Flattens the context to a single inline sequence, for content that is inline by construction
    /// (a hyperlink's display text). Any block breaks inside contribute their inlines in order.
    fn finish_inlines(mut self) -> Vec<Inline> {
        let mut out = Vec::new();
        let trailing = self.take_inlines();
        for block in self.blocks {
            if let Block::Para(inlines) | Block::Plain(inlines) = block {
                out.extend(inlines);
            }
        }
        out.extend(trailing);
        out
    }
}

/// Reassembles a run of list paragraphs into nested lists. A maximal span of entries at the
/// shallowest level forms one list; a return to that level after a deeper span starts a fresh
/// sibling list, so denesting is expressed as consecutive lists just as the format records it.
fn build_lists(entries: &[ListParagraph]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut rest = entries;
    while !rest.is_empty() {
        let (block, consumed) = build_one_list(rest, 0);
        out.push(block);
        let step = consumed.max(1).min(rest.len());
        rest = rest.get(step..).unwrap_or(&[]);
    }
    out
}

/// Builds a single list from the front of `entries`, consuming every entry at the list's own level
/// that also selects the same list, and folding any deeper entry that follows an item into a nested
/// list inside it. The list is an [`Block::OrderedList`] when its first entry's level is numbered,
/// otherwise a [`Block::BulletList`]. An entry at the same level that selects a different list ends
/// this one, so adjacent lists of unlike kind (a numbered list then a bulleted one) stay separate.
/// Returns the list and how many entries it consumed. `depth` caps recursion so pathologically deep
/// nesting degrades into sibling lists instead of overflowing the stack.
fn build_one_list(entries: &[ListParagraph], depth: usize) -> (Block, usize) {
    const MAX_LIST_DEPTH: usize = 256;
    let base = entries.first().map_or(0, |entry| entry.level);
    let list_id = entries.first().map_or(0, |entry| entry.list_id);
    let numbering = entries.first().and_then(|entry| entry.numbering);
    let mut items: Vec<Vec<Block>> = Vec::new();
    let mut i = 0;
    while let Some(entry) = entries.get(i) {
        if entry.level != base || entry.list_id != list_id {
            break;
        }
        let mut item = vec![entry.block.clone()];
        i += 1;
        if matches!(entries.get(i), Some(next) if next.level > base) && depth < MAX_LIST_DEPTH {
            let (sub, consumed) = build_one_list(entries.get(i..).unwrap_or(&[]), depth + 1);
            item.push(sub);
            i += consumed;
            items.push(item);
            break;
        }
        items.push(item);
    }
    let block = match numbering {
        Some((start, style)) => Block::OrderedList(
            ListAttributes {
                start,
                style,
                delim: ListNumberDelim::Period,
            },
            items,
        ),
        None => Block::BulletList(items),
    };
    (block, i)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Document-metadata destination words carried into the `meta` map as inline values.
const META_FIELDS: &[&str] = &[
    "title", "author", "keywords", "subject", "comment", "company", "doccomm", "operator",
    "category", "manager",
];

/// Destination words whose entire group is discarded (tables, styling, and layout apparatus that
/// carries no body content).
const SKIP_DESTINATIONS: &[&str] = &[
    "colortbl",
    "listtext",
    "revtbl",
    "rsidtbl",
    "generator",
    "filetbl",
    "pgdsctbl",
    "header",
    "headerl",
    "headerr",
    "headerf",
    "footer",
    "footerl",
    "footerr",
    "footerf",
    "pnseclvl",
    "pn",
    "pntext",
    "pntxta",
    "pntxtb",
    "themedata",
    "colorschememapping",
    "latentstyles",
    "datastore",
    "nonshppict",
    "xmlnstbl",
    "wgrffmtfilter",
    "template",
    "fchars",
    "lchars",
    "atnid",
    "atnauthor",
    "annotation",
];

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    states: Vec<GroupState>,
    emitters: Vec<Emitter>,
    media: MediaBag,
    meta: BTreeMap<Text, MetaValue>,
    skip: usize,
    pending_high_surrogate: Option<u32>,
    depth: usize,
    list_defs: BTreeMap<i32, Vec<LevelDef>>,
    list_overrides: BTreeMap<i32, i32>,
    style_outlines: BTreeMap<i32, i32>,
    style_formats: BTreeMap<i32, StyleFormat>,
    mono_fonts: BTreeSet<i32>,
    /// Fallback state returned by [`Parser::state_mut`] only if the state stack is ever empty, which
    /// its guard prevents; keeps the accessor total without a panic or a leak.
    scratch_state: GroupState,
}

/// Ceiling on nested group depth. Beyond it a group's content is discarded rather than descended
/// into, so adversarially deep nesting cannot exhaust the call stack.
const MAX_GROUP_DEPTH: usize = 512;

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            states: vec![GroupState::default()],
            emitters: vec![Emitter::new()],
            media: MediaBag::new(),
            meta: BTreeMap::new(),
            skip: 0,
            pending_high_surrogate: None,
            depth: 0,
            list_defs: BTreeMap::new(),
            list_overrides: BTreeMap::new(),
            style_outlines: BTreeMap::new(),
            style_formats: BTreeMap::new(),
            mono_fonts: BTreeSet::new(),
            scratch_state: GroupState::default(),
        }
    }

    fn run(&mut self) {
        self.process();
    }

    fn finish(mut self) -> (BTreeMap<Text, MetaValue>, Vec<Block>, MediaBag) {
        while self.emitters.len() > 1 {
            self.emitters.pop();
        }
        let blocks = match self.emitters.pop() {
            Some(emitter) => emitter.finish_blocks(),
            None => Vec::new(),
        };
        (self.meta, blocks, self.media)
    }

    fn props(&self) -> CharProps {
        self.states
            .last()
            .map(|state| state.props)
            .unwrap_or_default()
    }

    /// Overlays a paragraph style's character formatting onto the given properties, changing only the
    /// attributes the style declares. A font the style selects resolves to monospace membership now,
    /// once the font table is fully known.
    fn apply_style_format(&self, fmt: &StyleFormat, props: &mut CharProps) {
        if let Some(value) = fmt.bold {
            props.bold = value;
        }
        if let Some(value) = fmt.italic {
            props.italic = value;
        }
        if let Some(value) = fmt.underline {
            props.underline = value;
        }
        if let Some(value) = fmt.strike {
            props.strike = value;
        }
        if let Some(value) = fmt.superscript {
            props.superscript = value;
        }
        if let Some(value) = fmt.subscript {
            props.subscript = value;
        }
        if let Some(value) = fmt.smallcaps {
            props.smallcaps = value;
        }
        if let Some(value) = fmt.allcaps {
            props.allcaps = value;
        }
        if let Some(value) = fmt.hidden {
            props.hidden = value;
        }
        if let Some(font) = fmt.font {
            props.mono = self.mono_fonts.contains(&font);
        }
    }

    fn state_mut(&mut self) -> &mut GroupState {
        if self.states.is_empty() {
            self.states.push(GroupState::default());
        }
        // The guard above guarantees a last element; the scratch state is dead-code fallback.
        match self.states.last_mut() {
            Some(state) => state,
            None => &mut self.scratch_state,
        }
    }

    fn emitter(&mut self) -> Option<&mut Emitter> {
        self.emitters.last_mut()
    }

    /// Processes tokens at the current group level, returning once the matching `}` is consumed or
    /// input is exhausted.
    fn process(&mut self) {
        while self.pos < self.tokens.len() {
            if self.skip > 0 && self.consume_skipped() {
                continue;
            }
            let token = match self.tokens.get(self.pos) {
                Some(token) => token.clone(),
                None => break,
            };
            match token {
                Token::GroupEnd => {
                    self.pos += 1;
                    return;
                }
                Token::GroupStart => {
                    self.pos += 1;
                    self.enter_group();
                }
                Token::Control(word, param) => {
                    self.pos += 1;
                    self.handle_control(&word, param);
                }
                Token::Symbol(symbol) => {
                    self.pos += 1;
                    self.handle_symbol(symbol);
                }
                Token::Hex(byte) => {
                    self.pos += 1;
                    let props = self.props();
                    if let Some(emitter) = self.emitter() {
                        emitter.push_char(code_page_char(byte), props);
                    }
                }
                // Binary data outside a picture destination carries no text; it is consumed and
                // dropped (a `\binN` object body that no reader-handled destination collected).
                Token::Binary(_) => self.pos += 1,
                Token::Char(c) => {
                    self.pos += 1;
                    let props = self.props();
                    if let Some(emitter) = self.emitter() {
                        emitter.push_char(c, props);
                    }
                }
                Token::Space => {
                    self.pos += 1;
                    let props = self.props();
                    if let Some(emitter) = self.emitter() {
                        emitter.push_space(props);
                    }
                }
            }
        }
    }

    /// Skips one Unicode fallback item following a `\uN`. Returns whether an item was consumed;
    /// a group boundary ends the skip run so the boundary itself is handled normally.
    fn consume_skipped(&mut self) -> bool {
        match self.tokens.get(self.pos) {
            Some(Token::GroupEnd) | None => {
                self.skip = 0;
                false
            }
            Some(Token::GroupStart) => {
                self.pos += 1;
                self.skip_group();
                self.skip -= 1;
                true
            }
            Some(_) => {
                self.pos += 1;
                self.skip -= 1;
                true
            }
        }
    }

    /// Handles a group opened at the current position, dispatching on its destination word. Nesting
    /// past [`MAX_GROUP_DEPTH`] discards the group's content instead of descending into it.
    fn enter_group(&mut self) {
        self.depth += 1;
        if self.depth > MAX_GROUP_DEPTH {
            self.skip_group();
            self.depth -= 1;
            return;
        }
        let (ignorable, dest) = self.peek_destination();
        match dest.as_deref() {
            Some("info") => self.parse_info(),
            Some("fonttbl") => self.parse_font_table(),
            Some("stylesheet") => self.parse_stylesheet(),
            Some("listtable") => self.parse_list_table(),
            Some("listoverridetable") => self.parse_list_override_table(),
            Some("pict") => self.parse_picture(),
            Some("field") => self.parse_field(),
            Some("footnote") => self.parse_footnote(),
            Some("bkmkstart") => self.parse_bookmark(true),
            Some("bkmkend") => self.parse_bookmark(false),
            Some("shppict") => {
                // A drawing wrapper around a `\pict`; process transparently so the picture inside
                // is decoded, but do not save/restore character state for it.
                self.skip_optional_marker();
                self.pos += 1; // the `shppict` word
                self.process();
            }
            Some("shpinst") => self.parse_shape(),
            Some(word) if SKIP_DESTINATIONS.contains(&word) => self.skip_group(),
            _ if ignorable => self.skip_group(),
            _ => {
                self.states
                    .push(self.states.last().copied().unwrap_or_default());
                self.process();
                self.states.pop();
            }
        }
        self.depth -= 1;
    }

    /// Looks at the token after `{` to classify the group: whether it is flagged ignorable (`\*`)
    /// and its leading destination word, if any. Does not advance.
    fn peek_destination(&self) -> (bool, Option<String>) {
        let ignorable = matches!(self.tokens.get(self.pos), Some(Token::Symbol('*')));
        let word_pos = if ignorable { self.pos + 1 } else { self.pos };
        let word = match self.tokens.get(word_pos) {
            Some(Token::Control(word, _)) => Some(word.clone()),
            _ => None,
        };
        (ignorable, word)
    }

    fn skip_optional_marker(&mut self) {
        if matches!(self.tokens.get(self.pos), Some(Token::Symbol('*'))) {
            self.pos += 1;
        }
    }

    /// Consumes the current group in full, discarding its content.
    fn skip_group(&mut self) {
        let mut depth = 1;
        while let Some(token) = self.tokens.get(self.pos) {
            self.pos += 1;
            match token {
                Token::GroupStart => depth += 1,
                Token::GroupEnd => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_control(&mut self, word: &str, param: Option<i32>) {
        let on = param != Some(0);
        match word {
            "b" => self.state_mut().props.bold = on,
            "i" => self.state_mut().props.italic = on,
            "ul" => self.state_mut().props.underline = on,
            "ulnone" => self.state_mut().props.underline = false,
            "uld" | "uldb" | "ulw" | "uldash" | "uldashd" | "uldashdd" | "ulhwave" | "ulth"
            | "ulthd" | "ulwave" => self.state_mut().props.underline = true,
            "strike" | "striked" => self.state_mut().props.strike = on,
            "super" | "superscript" => self.state_mut().props.superscript = on,
            "sub" | "subscript" => self.state_mut().props.subscript = on,
            "nosupersub" => {
                let props = &mut self.state_mut().props;
                props.superscript = false;
                props.subscript = false;
            }
            "scaps" => self.state_mut().props.smallcaps = on,
            "caps" => self.state_mut().props.allcaps = on,
            "v" => self.state_mut().props.hidden = on,
            "plain" => self.state_mut().props = CharProps::default(),
            "pard" => {
                // A paragraph reset restores the default paragraph style (`\s0`): its character
                // formatting and outline level, if the stylesheet defines them, apply to every
                // paragraph that selects no other style.
                let mut props = CharProps::default();
                if let Some(fmt) = self.style_formats.get(&0).copied() {
                    self.apply_style_format(&fmt, &mut props);
                }
                self.state_mut().props = props;
                let outline = self.style_outlines.get(&0).copied();
                if let Some(emitter) = self.emitter() {
                    emitter.outline_level = outline;
                    emitter.in_table_para = false;
                    emitter.list_active = false;
                    emitter.list_id = 0;
                    emitter.list_level = 0;
                    emitter.list_levels = Vec::new();
                }
            }
            "ls" => {
                let levels = self.resolve_list(param);
                if let Some(emitter) = self.emitter() {
                    emitter.list_active = true;
                    emitter.list_id = param.unwrap_or(0);
                    emitter.list_levels = levels;
                }
            }
            "ilvl" => {
                if let Some(emitter) = self.emitter() {
                    emitter.list_level = param.unwrap_or(0);
                }
            }
            "uc" => self.state_mut().uc = param.unwrap_or(1).max(0),
            "u" => self.handle_unicode(param),
            "par" => {
                if let Some(emitter) = self.emitter() {
                    emitter.end_paragraph();
                }
            }
            "line" | "softline" => {
                let props = self.props();
                if let Some(emitter) = self.emitter() {
                    emitter.push_break(props);
                }
            }
            "tab" => {
                let props = self.props();
                if let Some(emitter) = self.emitter() {
                    emitter.push_space(props);
                }
            }
            "cell" | "nestcell" => {
                if let Some(emitter) = self.emitter() {
                    emitter.end_cell();
                }
            }
            "row" | "nestrow" => {
                if let Some(emitter) = self.emitter() {
                    emitter.end_row();
                }
            }
            "intbl" => {
                if let Some(emitter) = self.emitter() {
                    emitter.in_table_para = true;
                }
            }
            "trowd" => {
                if let Some(emitter) = self.emitter() {
                    emitter.begin_row_definition();
                }
            }
            "cellx" => {
                if let Some(emitter) = self.emitter() {
                    emitter.note_cell_boundary();
                }
            }
            "outlinelevel" => {
                if let Some(emitter) = self.emitter() {
                    emitter.outline_level = Some(param.unwrap_or(0));
                }
            }
            "s" => {
                // A paragraph style reference overlays the style's character formatting on the
                // current state and, when the stylesheet marks that style with an outline level,
                // makes the paragraph a heading of that level, just as an inline `\outlinelevel`
                // would. Later explicit control words in the same paragraph still win.
                let num = param.unwrap_or(0);
                if let Some(fmt) = self.style_formats.get(&num).copied() {
                    let mut props = self.props();
                    self.apply_style_format(&fmt, &mut props);
                    self.state_mut().props = props;
                }
                if let Some(level) = self.style_outlines.get(&num).copied()
                    && let Some(emitter) = self.emitter()
                {
                    emitter.outline_level = Some(level);
                }
            }
            "f" => {
                // Selecting a font from the font table: a font of the monospace family marks the run
                // as code, so it later lowers to inline code or a whole-paragraph code block.
                self.state_mut().props.mono = self.mono_fonts.contains(&param.unwrap_or(0));
            }
            _ => {
                if let Some(text) = special_char(word) {
                    let props = self.props();
                    if let Some(emitter) = self.emitter() {
                        emitter.push_str(text, props);
                    }
                }
            }
        }
    }

    fn handle_symbol(&mut self, symbol: char) {
        if let Some(text) = symbol_char(symbol) {
            let props = self.props();
            if let Some(emitter) = self.emitter() {
                emitter.push_str(text, props);
            }
        }
    }

    fn handle_unicode(&mut self, param: Option<i32>) {
        let code = unicode_code(param.unwrap_or(0));
        if let Some(scalar) = combine_surrogate(&mut self.pending_high_surrogate, code) {
            self.emit_scalar(scalar);
        }
        let uc = self.states.last().map_or(1, |state| state.uc);
        self.skip = usize::try_from(uc).unwrap_or(0);
    }

    fn emit_scalar(&mut self, code: u32) {
        if let Some(c) = char::from_u32(code) {
            let props = self.props();
            if let Some(emitter) = self.emitter() {
                emitter.push_char(c, props);
            }
        }
    }

    // --- Structural destinations ------------------------------------------

    fn parse_info(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `info` word
        loop {
            match self.tokens.get(self.pos) {
                None => break,
                Some(Token::GroupEnd) => {
                    self.pos += 1;
                    break;
                }
                Some(Token::GroupStart) => {
                    self.pos += 1;
                    self.parse_info_field();
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    fn parse_info_field(&mut self) {
        self.skip_optional_marker();
        let name = match self.tokens.get(self.pos) {
            Some(Token::Control(word, _)) => {
                let word = word.clone();
                self.pos += 1;
                Some(word)
            }
            _ => None,
        };
        let text = self.collect_text();
        let Some(name) = name.filter(|name| META_FIELDS.contains(&name.as_str())) else {
            return;
        };
        let inlines = words_to_inlines(&text);
        if !inlines.is_empty() {
            self.meta
                .insert(name.into(), MetaValue::MetaInlines(inlines));
        }
    }

    /// Reads the font table, noting which font numbers belong to the monospace (fixed-pitch) family.
    /// Each entry opens with `\fN` and declares a family (`\froman`, `\fswiss`, `\fmodern`, …); a
    /// `\fmodern` font is recorded so a run set in it renders as code. Entries may share one group,
    /// separated by `;`, or sit each in their own nested group, so both an entry terminator and a
    /// group boundary end the current font.
    fn parse_font_table(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `fonttbl` word
        let mut depth = 1;
        let mut current: Option<i32> = None;
        while let Some(token) = self.tokens.get(self.pos) {
            self.pos += 1;
            match token {
                Token::GroupStart => depth += 1,
                Token::GroupEnd => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    current = None;
                }
                Token::Control(word, param) => match word.as_str() {
                    "f" => current = *param,
                    "fmodern" => {
                        if let Some(num) = current {
                            self.mono_fonts.insert(num);
                        }
                    }
                    _ => {}
                },
                Token::Char(';') => current = None,
                _ => {}
            }
        }
    }

    /// Reads the stylesheet: each style definition that carries an `\outlinelevel` registers its
    /// paragraph style number (`\sN`) so a paragraph selecting that style becomes a heading.
    fn parse_stylesheet(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `stylesheet` word
        loop {
            match self.tokens.get(self.pos) {
                None => break,
                Some(Token::GroupEnd) => {
                    self.pos += 1;
                    break;
                }
                Some(Token::GroupStart) => {
                    self.pos += 1;
                    self.parse_style_def();
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    /// Reads one style definition (its `{` already consumed) through the matching `}`. A paragraph
    /// style is designated by a leading `\sN`; when the definition carries an `\outlinelevel`, the
    /// pair is recorded so paragraphs referencing style `N` render as headings, and any character
    /// formatting the definition sets is recorded so those paragraphs inherit it. Character and
    /// section styles carry no bare `\s` and are ignored. Nested groups are skipped.
    fn parse_style_def(&mut self) {
        self.skip_optional_marker();
        let mut style_num: Option<i32> = None;
        let mut outline: Option<i32> = None;
        let mut format = StyleFormat::default();
        loop {
            match self.tokens.get(self.pos) {
                None => break,
                Some(Token::GroupEnd) => {
                    self.pos += 1;
                    break;
                }
                Some(Token::GroupStart) => {
                    self.pos += 1;
                    self.skip_group();
                }
                Some(Token::Control(word, param)) => {
                    match word.as_str() {
                        "s" if style_num.is_none() => style_num = Some(param.unwrap_or(0)),
                        "outlinelevel" => outline = Some(param.unwrap_or(0)),
                        other => format.apply_control(other, *param),
                    }
                    self.pos += 1;
                }
                Some(_) => self.pos += 1,
            }
        }
        if let Some(num) = style_num {
            if let Some(level) = outline {
                self.style_outlines.insert(num, level);
            }
            self.style_formats.insert(num, format);
        }
    }

    /// Reads the list table: each `\list` group defines one abstract list, keyed by its `\listid`.
    fn parse_list_table(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `listtable` word
        loop {
            match self.tokens.get(self.pos) {
                None => break,
                Some(Token::GroupEnd) => {
                    self.pos += 1;
                    break;
                }
                Some(Token::GroupStart) => {
                    self.pos += 1;
                    self.skip_optional_marker();
                    match self.tokens.get(self.pos) {
                        Some(Token::Control(word, _)) if word == "list" => {
                            self.pos += 1;
                            self.parse_list_def();
                        }
                        _ => self.skip_group(),
                    }
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    /// Reads one `\list` group (its `{\list` already consumed) through the matching `}`, collecting
    /// its per-level marker definitions and registering them under the list's `\listid`.
    fn parse_list_def(&mut self) {
        let mut listid: Option<i32> = None;
        let mut levels: Vec<LevelDef> = Vec::new();
        loop {
            match self.tokens.get(self.pos) {
                None => break,
                Some(Token::GroupEnd) => {
                    self.pos += 1;
                    break;
                }
                Some(Token::GroupStart) => {
                    self.pos += 1;
                    self.skip_optional_marker();
                    match self.tokens.get(self.pos) {
                        Some(Token::Control(word, _)) if word == "listlevel" => {
                            self.pos += 1;
                            levels.push(self.parse_list_level());
                        }
                        _ => self.skip_group(),
                    }
                }
                Some(Token::Control(word, param)) => {
                    if word == "listid" {
                        listid = *param;
                    }
                    self.pos += 1;
                }
                Some(_) => self.pos += 1,
            }
        }
        if let Some(id) = listid {
            self.list_defs.insert(id, levels);
        }
    }

    /// Reads one `\listlevel` group (its `{\listlevel` already consumed) through the matching `}`,
    /// taking the numeral style from `\levelnfc` and the first item's number from `\levelstartat`.
    fn parse_list_level(&mut self) -> LevelDef {
        let mut nfc: Option<i32> = None;
        let mut start: i32 = 1;
        loop {
            match self.tokens.get(self.pos) {
                None => break,
                Some(Token::GroupEnd) => {
                    self.pos += 1;
                    break;
                }
                Some(Token::GroupStart) => {
                    self.pos += 1;
                    self.skip_group();
                }
                Some(Token::Control(word, param)) => {
                    match word.as_str() {
                        "levelnfc" => nfc = *param,
                        "levelnfcn" if nfc.is_none() => nfc = *param,
                        "levelstartat" => {
                            if let Some(value) = param {
                                start = *value;
                            }
                        }
                        _ => {}
                    }
                    self.pos += 1;
                }
                Some(_) => self.pos += 1,
            }
        }
        LevelDef {
            style: nfc_to_style(nfc),
            start,
        }
    }

    /// Reads the list-override table: each `\listoverride` maps the `\ls` number paragraphs reference
    /// to the `\listid` of an abstract list defined in the list table.
    fn parse_list_override_table(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `listoverridetable` word
        loop {
            match self.tokens.get(self.pos) {
                None => break,
                Some(Token::GroupEnd) => {
                    self.pos += 1;
                    break;
                }
                Some(Token::GroupStart) => {
                    self.pos += 1;
                    self.skip_optional_marker();
                    match self.tokens.get(self.pos) {
                        Some(Token::Control(word, _)) if word == "listoverride" => {
                            self.pos += 1;
                            self.parse_list_override();
                        }
                        _ => self.skip_group(),
                    }
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    /// Reads one `\listoverride` group (its `{\listoverride` already consumed) through the matching
    /// `}`, registering its `\ls`-to-`\listid` mapping.
    fn parse_list_override(&mut self) {
        let mut listid: Option<i32> = None;
        let mut ls: Option<i32> = None;
        loop {
            match self.tokens.get(self.pos) {
                None => break,
                Some(Token::GroupEnd) => {
                    self.pos += 1;
                    break;
                }
                Some(Token::GroupStart) => {
                    self.pos += 1;
                    self.skip_group();
                }
                Some(Token::Control(word, param)) => {
                    match word.as_str() {
                        "listid" => listid = *param,
                        "ls" => ls = *param,
                        _ => {}
                    }
                    self.pos += 1;
                }
                Some(_) => self.pos += 1,
            }
        }
        if let (Some(ls), Some(id)) = (ls, listid) {
            self.list_overrides.insert(ls, id);
        }
    }

    /// The level definitions the list-override number `\lsN` on a paragraph selects: resolved through
    /// the override table to a `\listid`, falling back to that number as a direct list id. An unknown
    /// number yields no levels, so its paragraphs render as a plain bullet list.
    fn resolve_list(&self, ls: Option<i32>) -> Vec<LevelDef> {
        let Some(ls) = ls else {
            return Vec::new();
        };
        let id = self.list_overrides.get(&ls).copied().unwrap_or(ls);
        self.list_defs.get(&id).cloned().unwrap_or_default()
    }

    fn parse_picture(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `pict` word
        let mut extension: Option<&'static str> = None;
        let mut hex = String::new();
        let mut binary: Vec<u8> = Vec::new();
        let mut depth = 1;
        let mut goal_width: Option<i32> = None;
        let mut goal_height: Option<i32> = None;
        while let Some(token) = self.tokens.get(self.pos) {
            self.pos += 1;
            match token {
                Token::GroupStart => depth += 1,
                Token::GroupEnd => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Token::Control(word, param) => match word.as_str() {
                    "pngblip" => extension = extension.or(Some("png")),
                    "jpegblip" => extension = extension.or(Some("jpg")),
                    "emfblip" => extension = extension.or(Some("emf")),
                    "picwgoal" => goal_width = *param,
                    "pichgoal" => goal_height = *param,
                    _ => {}
                },
                Token::Char(c) if c.is_ascii_hexdigit() => hex.push(*c),
                // Picture data can arrive raw via `\binN` instead of hex; take those bytes directly.
                Token::Binary(data) => binary.extend_from_slice(data),
                _ => {}
            }
        }
        if let Some(extension) = extension {
            let bytes = if binary.is_empty() {
                decode_hex(&hex)
            } else {
                binary
            };
            if !bytes.is_empty() {
                let mime = match extension {
                    "png" => "image/png",
                    "jpg" => "image/jpeg",
                    _ => "image/emf",
                };
                let name = content_addressed_name(mime, &bytes);
                self.media
                    .insert(name.clone(), Some(mime.to_string()), bytes);
                let props = self.props();
                let image = Inline::Image(
                    Box::new(picture_attr(goal_width, goal_height)),
                    vec![Inline::Str(Text::from("image"))],
                    Box::new(Target {
                        url: name.into(),
                        title: Text::default(),
                    }),
                );
                if let Some(emitter) = self.emitter() {
                    emitter.push_node(image, props);
                }
            }
        }
    }

    /// Reads a `\*\shpinst` shape-instructions group (its `{` already consumed): the body of a
    /// drawing object. Its embedded picture and text box carry document content that the surrounding
    /// positioning and property words do not, so those two are descended into and everything else is
    /// discarded. A `\sp` shape property named `pib` holds an inline picture; a `\shptxt` group holds
    /// block content emitted in source order.
    fn parse_shape(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `shpinst` word
        while let Some(token) = self.tokens.get(self.pos) {
            match token {
                Token::GroupEnd => {
                    self.pos += 1;
                    break;
                }
                Token::GroupStart => {
                    self.pos += 1;
                    self.skip_optional_marker();
                    match self.tokens.get(self.pos) {
                        Some(Token::Control(word, _)) if word == "sp" => {
                            self.parse_shape_property();
                        }
                        Some(Token::Control(word, _)) if word == "shptxt" => {
                            self.pos += 1; // the `shptxt` word
                            self.process();
                        }
                        _ => self.skip_group(),
                    }
                }
                _ => self.pos += 1,
            }
        }
    }

    /// Reads one `\sp` shape property (its `{` already consumed, position at the `\sp` word) through
    /// the matching `}`: an `\sn` name/`\sv` value pair. Only the `pib` property carries a picture, so
    /// its value's embedded `\pict` is decoded into an inline image; every other property is discarded.
    fn parse_shape_property(&mut self) {
        self.pos += 1; // the `sp` word
        let mut name: Option<String> = None;
        while let Some(token) = self.tokens.get(self.pos) {
            match token {
                Token::GroupEnd => {
                    self.pos += 1;
                    break;
                }
                Token::GroupStart => {
                    self.pos += 1;
                    self.skip_optional_marker();
                    match self.tokens.get(self.pos) {
                        Some(Token::Control(word, _)) if word == "sn" => {
                            self.pos += 1; // the `sn` word
                            name = Some(self.collect_text().trim().to_owned());
                        }
                        Some(Token::Control(word, _)) if word == "sv" => {
                            self.pos += 1; // the `sv` word
                            if name.as_deref() == Some("pib") {
                                self.process();
                            } else {
                                self.skip_group();
                            }
                        }
                        _ => self.skip_group(),
                    }
                }
                _ => self.pos += 1,
            }
        }
    }

    fn parse_field(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `field` word
        let mut url: Option<String> = None;
        let mut display: Vec<Inline> = Vec::new();
        let mut depth = 1;
        while let Some(token) = self.tokens.get(self.pos) {
            match token {
                Token::GroupEnd => {
                    self.pos += 1;
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Token::GroupStart => {
                    self.pos += 1;
                    self.skip_optional_marker();
                    let word = match self.tokens.get(self.pos) {
                        Some(Token::Control(word, _)) => {
                            let word = word.clone();
                            self.pos += 1;
                            Some(word)
                        }
                        _ => None,
                    };
                    match word.as_deref() {
                        Some("fldinst") => {
                            let instruction = self.collect_field_instruction();
                            if let Some(found) = parse_hyperlink(&instruction) {
                                url = Some(found);
                            }
                        }
                        Some("fldrslt") => display = self.collect_group_inlines(),
                        _ => self.skip_group(),
                    }
                }
                _ => self.pos += 1,
            }
        }
        let inlines = match url {
            Some(url) => {
                let target = Target {
                    url: url.into(),
                    title: Text::default(),
                };
                linkify(&target, display)
            }
            None => display,
        };
        for inline in inlines {
            if let Some(emitter) = self.emitter() {
                emitter.push_node(inline, CharProps::default());
            }
        }
    }

    fn parse_footnote(&mut self) {
        self.skip_optional_marker();
        self.pos += 1; // the `footnote` word
        self.emitters.push(Emitter::new());
        self.states.push(GroupState::default());
        self.process();
        self.states.pop();
        let blocks = match self.emitters.pop() {
            Some(emitter) => emitter.finish_blocks(),
            None => Vec::new(),
        };
        let props = self.props();
        if let Some(emitter) = self.emitter() {
            emitter.push_node(Inline::Note(blocks), props);
        }
    }

    fn parse_bookmark(&mut self, start: bool) {
        self.skip_optional_marker();
        self.pos += 1; // the `bkmkstart` / `bkmkend` word
        let name = self.collect_text();
        let name = name.trim();
        if let Some(emitter) = self.emitter() {
            if start {
                emitter.open_bookmark(Text::from(name));
            } else {
                emitter.close_bookmark();
            }
        }
    }

    /// Builds the inline content of the group opened at the current position, in a throwaway block
    /// context, and returns it flattened. Edge whitespace is kept: the content is inline display
    /// text (a hyperlink's `\fldrslt`), where a leading or trailing space separates it from the
    /// surrounding words.
    fn collect_group_inlines(&mut self) -> Vec<Inline> {
        let mut emitter = Emitter::new();
        emitter.preserve_edge_space = true;
        self.emitters.push(emitter);
        self.states
            .push(self.states.last().copied().unwrap_or_default());
        self.process();
        self.states.pop();
        match self.emitters.pop() {
            Some(emitter) => emitter.finish_inlines(),
            None => Vec::new(),
        }
    }

    /// Gathers the plain text of the group currently open (its `{` already consumed), through the
    /// matching `}`. Nested groups contribute their text too.
    fn collect_text(&mut self) -> String {
        let mut out = String::new();
        let mut depth: usize = 1;
        let mut uc: i32 = self.states.last().map_or(1, |state| state.uc);
        let mut skip: i32 = 0;
        let mut pending_high: Option<u32> = None;
        while let Some(token) = self.tokens.get(self.pos).cloned() {
            // A `\uN` is followed by `uc` fallback items for readers that cannot render the scalar;
            // they are consumed here so the fallback `?` never leaks into the collected text.
            if skip > 0 {
                match token {
                    Token::GroupEnd => skip = 0,
                    Token::GroupStart => {
                        self.pos += 1;
                        self.skip_group();
                        skip -= 1;
                        continue;
                    }
                    _ => {
                        self.pos += 1;
                        skip -= 1;
                        continue;
                    }
                }
            }
            self.pos += 1;
            match token {
                Token::GroupStart => depth += 1,
                Token::GroupEnd => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Token::Char(c) => out.push(c),
                Token::Space => out.push(' '),
                Token::Hex(byte) => out.push(code_page_char(byte)),
                Token::Binary(_) => {}
                Token::Control(word, param) => match word.as_str() {
                    "uc" => uc = param.unwrap_or(1).max(0),
                    "u" => {
                        let code = unicode_code(param.unwrap_or(0));
                        if let Some(scalar) = combine_surrogate(&mut pending_high, code)
                            && let Some(c) = char::from_u32(scalar)
                        {
                            out.push(c);
                        }
                        skip = uc;
                    }
                    other => {
                        if let Some(text) = special_char(other) {
                            out.push_str(text);
                        }
                    }
                },
                Token::Symbol(symbol) => {
                    if let Some(text) = symbol_char(symbol) {
                        out.push_str(text);
                    }
                }
            }
        }
        out
    }

    /// Gathers a field instruction (its `{` already consumed) through the matching `}`, preserving a
    /// backslash at every control word, control symbol, and escaped byte. Field switches and escapes
    /// therefore stay marked as backslashes so the destination of a `HYPERLINK` field can be split off
    /// at the first one; nested groups contribute their content too. A switch spelled with an escaped
    /// backslash (`\\l`) keeps its letters as ordinary text, so a switch such as `\l` is still
    /// recognizable by name.
    fn collect_field_instruction(&mut self) -> String {
        let mut out = String::new();
        let mut depth = 1;
        while let Some(token) = self.tokens.get(self.pos) {
            self.pos += 1;
            match token {
                Token::GroupStart => depth += 1,
                Token::GroupEnd => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Token::Char(c) => out.push(*c),
                Token::Space => out.push(' '),
                Token::Control(_, _) | Token::Symbol(_) | Token::Hex(_) => out.push('\\'),
                Token::Binary(_) => {}
            }
        }
        out
    }
}

/// Builds the [`Attr`] for an embedded picture from its goal dimensions. A `\picwgoal`/`\pichgoal`
/// value is a measurement in twips (1/1440 inch); each present dimension becomes a `width`/`height`
/// attribute expressed in inches.
fn picture_attr(goal_width: Option<i32>, goal_height: Option<i32>) -> Attr {
    let mut attributes: Vec<(Text, Text)> = Vec::new();
    if let Some(twips) = goal_width {
        attributes.push((Text::from("width"), Text::from(twips_to_inches(twips))));
    }
    if let Some(twips) = goal_height {
        attributes.push((Text::from("height"), Text::from(twips_to_inches(twips))));
    }
    Attr {
        id: Text::default(),
        classes: Vec::new(),
        attributes,
    }
}

/// A twip measurement (1/1440 inch) rendered as an inch dimension, e.g. `1440` -> `1.0in`.
fn twips_to_inches(twips: i32) -> String {
    format!("{}in", general_decimal(f64::from(twips) / 1440.0))
}

// ---------------------------------------------------------------------------
// Character mappings and helpers
// ---------------------------------------------------------------------------

/// The Unicode string a special-character control word stands for, or `None` if the word carries no
/// character.
fn special_char(word: &str) -> Option<&'static str> {
    Some(match word {
        "emdash" => "\u{2014}",
        "endash" => "\u{2013}",
        "bullet" => "\u{2022}",
        "lquote" => "\u{2018}",
        "rquote" => "\u{2019}",
        "ldblquote" => "\u{201C}",
        "rdblquote" => "\u{201D}",
        "emspace" => "\u{2003}",
        "enspace" => "\u{2002}",
        "qmspace" => "\u{2005}",
        "zwj" => "\u{200D}",
        "zwnj" => "\u{200C}",
        "ltrmark" => "\u{200E}",
        "rtlmark" => "\u{200F}",
        _ => return None,
    })
}

/// The character a control symbol (`\` before one non-letter) stands for, or `None` if it carries
/// no text.
fn symbol_char(symbol: char) -> Option<&'static str> {
    Some(match symbol {
        '\\' => "\\",
        '{' => "{",
        '}' => "}",
        '~' => "\u{00A0}",
        '-' => "\u{00AD}",
        '_' => "\u{2011}",
        _ => return None,
    })
}

/// Resolves a `\uN` parameter to a Unicode scalar value. The parameter is a signed 16-bit integer;
/// a negative value denotes a code point above 0x7FFF, recovered by adding 65536.
fn unicode_code(raw: i32) -> u32 {
    if raw < 0 {
        u32::try_from(i64::from(raw) + 65536).unwrap_or(0)
    } else {
        u32::try_from(raw).unwrap_or(0)
    }
}

/// Folds a `\u` scalar into the pending UTF-16 surrogate state, returning the code point to emit, if
/// any. A high surrogate is held back; a following low surrogate combines with it into a supplementary
/// scalar; any other value clears a stale pending high and is emitted unchanged.
fn combine_surrogate(pending_high: &mut Option<u32>, code: u32) -> Option<u32> {
    if (0xD800..=0xDBFF).contains(&code) {
        *pending_high = Some(code);
        None
    } else if (0xDC00..=0xDFFF).contains(&code) {
        pending_high
            .take()
            .map(|high| 0x1_0000 + ((high - 0xD800) << 10) + (code - 0xDC00))
    } else {
        *pending_high = None;
        Some(code)
    }
}

/// Maps a code-page byte to a character. Bytes outside `0x80..=0x9F` are Latin-1; that window uses
/// the Windows-1252 assignments, the code page an unqualified `\ansi` document carries.
fn code_page_char(byte: u8) -> char {
    let scalar: u32 = match byte {
        0x80 => 0x20AC,
        0x82 => 0x201A,
        0x83 => 0x0192,
        0x84 => 0x201E,
        0x85 => 0x2026,
        0x86 => 0x2020,
        0x87 => 0x2021,
        0x88 => 0x02C6,
        0x89 => 0x2030,
        0x8A => 0x0160,
        0x8B => 0x2039,
        0x8C => 0x0152,
        0x8E => 0x017D,
        0x91 => 0x2018,
        0x92 => 0x2019,
        0x93 => 0x201C,
        0x94 => 0x201D,
        0x95 => 0x2022,
        0x96 => 0x2013,
        0x97 => 0x2014,
        0x98 => 0x02DC,
        0x99 => 0x2122,
        0x9A => 0x0161,
        0x9B => 0x203A,
        0x9C => 0x0153,
        0x9E => 0x017E,
        0x9F => 0x0178,
        other => u32::from(other),
    };
    char::from_u32(scalar).unwrap_or('\u{FFFD}')
}

/// Decodes a hex-digit string into bytes, ignoring a trailing unpaired digit.
fn decode_hex(hex: &str) -> Vec<u8> {
    let digits: Vec<u32> = hex.chars().filter_map(|c| c.to_digit(16)).collect();
    digits
        .chunks_exact(2)
        .filter_map(|pair| match pair {
            [hi, lo] => u8::try_from((hi << 4) | lo).ok(),
            _ => None,
        })
        .collect()
}

/// Extracts a link target from a field instruction. A `HYPERLINK` instruction is followed by its
/// destination, which runs from the keyword to the first backslash — the marker every field switch
/// (`\l`, `\o`, …) carries — with any quotes removed and outer whitespace trimmed. When no such
/// destination is present, an `\l` switch names an in-document bookmark, and its argument becomes a
/// fragment target (`#name`). An instruction without the `HYPERLINK` keyword is not a link and yields
/// `None`.
fn parse_hyperlink(instruction: &str) -> Option<String> {
    const KEYWORD: &str = "HYPERLINK";
    let after = instruction.find(KEYWORD)? + KEYWORD.len();
    let tail = instruction.get(after..).unwrap_or_default();
    let destination = match tail.find('\\') {
        Some(cut) => tail.get(..cut).unwrap_or_default(),
        None => tail,
    };
    let target = strip_field_quotes(destination);
    if !target.is_empty() {
        return Some(target);
    }
    if let Some(anchor) = field_switch_argument(tail, "l")
        && !anchor.is_empty()
    {
        return Some(format!("#{anchor}"));
    }
    Some(target)
}

/// Strips the double quotes and outer whitespace that wrap a field argument.
fn strip_field_quotes(text: &str) -> String {
    text.chars()
        .filter(|&c| c != '"')
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Locates a field switch (`\<name>`) in an instruction and returns the argument that follows it —
/// the text up to the next switch — with quotes and outer whitespace removed. Returns `None` when no
/// switch of that name is present. Matching is by whole control-word name, so `\l` is not found
/// inside a longer word such as `\line`.
fn field_switch_argument(tail: &str, name: &str) -> Option<String> {
    let mut rest = tail;
    while let Some(backslash) = rest.find('\\') {
        let after = rest.get(backslash + 1..).unwrap_or_default();
        let word_len = after
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .map(char::len_utf8)
            .sum();
        let word = after.get(..word_len).unwrap_or_default();
        let argument = after.get(word_len..).unwrap_or_default();
        if word == name {
            let value = match argument.find('\\') {
                Some(cut) => argument.get(..cut).unwrap_or_default(),
                None => argument,
            };
            return Some(strip_field_quotes(value));
        }
        rest = argument;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(input: &str) -> Document {
        read_bytes(input.as_bytes())
    }

    fn read_bytes(input: &[u8]) -> Document {
        RtfReader
            .read(input, &ReaderOptions::default())
            .expect("read")
    }

    fn read_media(input: &str) -> (Document, MediaBag) {
        read_media_bytes(input.as_bytes())
    }

    fn read_media_bytes(input: &[u8]) -> (Document, MediaBag) {
        RtfReader
            .read_media(input, &ReaderOptions::default())
            .expect("read")
    }

    fn para(inlines: Vec<Inline>) -> Block {
        Block::Para(inlines)
    }

    fn s(text: &str) -> Inline {
        Inline::Str(text.into())
    }

    #[test]
    fn plain_paragraph_splits_words() {
        let doc = read(r"{\rtf1\ansi Hello world.\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("Hello"), Inline::Space, s("world.")])]
        );
    }

    #[test]
    fn collapses_runs_of_spaces() {
        let doc = read(r"{\rtf1\ansi a  b   c\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                s("a"),
                Inline::Space,
                s("b"),
                Inline::Space,
                s("c"),
            ])]
        );
    }

    #[test]
    fn bold_and_italic_map_to_strong_and_emph() {
        let doc = read(r"{\rtf1\ansi \b bold\b0  normal\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                Inline::Strong(vec![s("bold")]),
                Inline::Space,
                s("normal"),
            ])]
        );
        let doc = read(r"{\rtf1\ansi \i italic\i0\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Emph(vec![s("italic")])])]
        );
    }

    #[test]
    fn style_reference_inherits_character_formatting() {
        let doc = read(r"{\rtf1\ansi{\stylesheet{\s1\i Emphasis;}}\pard\s1 italic text\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Emph(vec![
                s("italic"),
                Inline::Space,
                s("text"),
            ])])]
        );
    }

    #[test]
    fn default_style_applies_to_unstyled_paragraphs() {
        let doc = read(r"{\rtf1\ansi{\stylesheet{\s0\b Normal;}}\pard first\par\pard second\par}");
        assert_eq!(
            doc.blocks,
            vec![
                para(vec![Inline::Strong(vec![s("first")])]),
                para(vec![Inline::Strong(vec![s("second")])]),
            ]
        );
    }

    #[test]
    fn style_formatting_overlays_default_style() {
        // `\s0` sets bold for every paragraph and `\s1` adds italic, so the run is both.
        let doc =
            read(r"{\rtf1\ansi{\stylesheet{\s0\b Normal;}{\s1\i Emphasis;}}\pard\s1 word\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Strong(vec![Inline::Emph(vec![s(
                "word"
            )])])])]
        );
    }

    #[test]
    fn whole_heading_emphasis_is_stripped() {
        // A styled heading whose entire content is bold drops the emphasis, since the heading level
        // already conveys prominence.
        let doc =
            read(r"{\rtf1\ansi{\stylesheet{\s1\outlinelevel0\b Heading;}}\pard\s1 the title\par}");
        assert_eq!(
            doc.blocks,
            vec![Block::Header(
                1,
                Box::default(),
                vec![s("the"), Inline::Space, s("title")],
            )]
        );
    }

    #[test]
    fn nesting_order_is_fixed() {
        let doc = read(r"{\rtf1\ansi \i\b x\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Strong(vec![Inline::Emph(vec![s("x")])])])]
        );
    }

    #[test]
    fn shared_formatting_coalesces_across_inner_group() {
        let doc = read(r"{\rtf1\ansi \b bold {\i both} stillbold\b0 normal\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                Inline::Strong(vec![
                    s("bold"),
                    Inline::Space,
                    Inline::Emph(vec![s("both")]),
                    Inline::Space,
                    s("stillbold"),
                ]),
                s("normal"),
            ])]
        );
    }

    #[test]
    fn formatting_persists_across_par_but_pard_resets() {
        let doc = read(r"{\rtf1\ansi \b bold\par next\par}");
        assert_eq!(
            doc.blocks,
            vec![
                para(vec![Inline::Strong(vec![s("bold")])]),
                para(vec![Inline::Strong(vec![s("next")])]),
            ]
        );
        let doc = read(r"{\rtf1\ansi \b bold\par\pard normal\par}");
        assert_eq!(
            doc.blocks,
            vec![
                para(vec![Inline::Strong(vec![s("bold")])]),
                para(vec![s("normal")]),
            ]
        );
    }

    #[test]
    fn line_break_and_tab() {
        let doc = read(r"{\rtf1\ansi one\line two\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("one"), Inline::LineBreak, s("two")])]
        );
        let doc = read(r"{\rtf1\ansi a\tab b\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("a"), Inline::Space, s("b")])]);
    }

    #[test]
    fn trailing_line_break_at_paragraph_boundary_is_dropped() {
        // A single break just before the paragraph mark carries no line and is removed.
        let doc = read(r"{\rtf1\ansi a\line\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("a")])]);
        // Only one trailing break is removed; the earlier break stays.
        let doc = read(r"{\rtf1\ansi a\line\line\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("a"), Inline::LineBreak])]);
        // A paragraph holding only a break becomes empty and is emitted as nothing.
        let doc = read(r"{\rtf1\ansi first\par\line\par second\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("first")]), para(vec![s("second")])]
        );
        // The same trimming applies where a paragraph is closed by a cell or footnote boundary.
        let doc = read(r"{\rtf1\ansi x{\footnote note\line}\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                s("x"),
                Inline::Note(vec![para(vec![s("note")])])
            ])]
        );
    }

    #[test]
    fn escapes_and_special_characters() {
        let doc = read(r"{\rtf1\ansi a\{b\}c\\d\par}");
        assert_eq!(doc.blocks, vec![para(vec![s(r"a{b}c\d")])]);
        let doc = read(r"{\rtf1\ansi em\emdash dash\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("em\u{2014}dash")])]);
        let doc = read(r"{\rtf1\ansi non\~breaking\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("non\u{00A0}breaking")])]);
    }

    #[test]
    fn hex_escape_uses_code_page() {
        let doc = read("{\\rtf1\\ansi caf\\'e9\\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("caf\u{00E9}")])]);
        let doc = read("{\\rtf1\\ansi \\'80\\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("\u{20AC}")])]);
    }

    #[test]
    fn unicode_with_fallback_skip() {
        let doc = read(r"{\rtf1\ansi \u233 e\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("\u{00E9}")])]);
        let doc = read(r"{\rtf1\ansi \uc2\u233 xx after\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("\u{00E9}"), Inline::Space, s("after")])]
        );
        let doc = read(r"{\rtf1\ansi \uc0\u233 x\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("\u{00E9}x")])]);
    }

    #[test]
    fn negative_unicode_wraps() {
        let doc = read(r"{\rtf1\ansi \u-3647 ?after\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("\u{F1C1}after")])]);
    }

    #[test]
    fn outline_level_becomes_header() {
        let doc = read(r"{\rtf1\ansi \outlinelevel0 Chapter\par}");
        assert_eq!(
            doc.blocks,
            vec![Block::Header(1, Box::default(), vec![s("Chapter")])]
        );
        let doc = read(r"{\rtf1\ansi \outlinelevel2 Sub\par}");
        assert_eq!(
            doc.blocks,
            vec![Block::Header(3, Box::default(), vec![s("Sub")])]
        );
    }

    #[test]
    fn extreme_outline_level_saturates_without_overflow() {
        let doc = read(r"{\rtf1\ansi \outlinelevel2147483647 Edge\par}");
        assert_eq!(
            doc.blocks,
            vec![Block::Header(i32::MAX, Box::default(), vec![s("Edge")])]
        );
    }

    #[test]
    fn info_group_populates_metadata() {
        let doc = read(r"{\rtf1{\info{\title My Title}{\author Jane Doe}}\ansi Body\par}");
        assert_eq!(
            doc.meta.get("title"),
            Some(&MetaValue::MetaInlines(vec![
                s("My"),
                Inline::Space,
                s("Title")
            ]))
        );
        assert_eq!(
            doc.meta.get("author"),
            Some(&MetaValue::MetaInlines(vec![
                s("Jane"),
                Inline::Space,
                s("Doe")
            ]))
        );
        assert_eq!(doc.blocks, vec![para(vec![s("Body")])]);
    }

    #[test]
    fn destinations_are_skipped() {
        let doc = read(r"{\rtf1{\fonttbl{\f0 Times;}}\ansi text {\*\generator X;}more\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("text"), Inline::Space, s("more")])]
        );
    }

    #[test]
    fn unknown_group_word_keeps_text() {
        let doc = read(r"{\rtf1\ansi text {\madeupword hidden} more\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                s("text"),
                Inline::Space,
                s("hidden"),
                Inline::Space,
                s("more"),
            ])]
        );
    }

    #[test]
    fn hyperlink_field_becomes_link() {
        let doc = read(
            r#"{\rtf1\ansi {\field{\*\fldinst HYPERLINK "http://x.com"}{\fldrslt click}}\par}"#,
        );
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Link(
                Box::default(),
                vec![s("click")],
                Box::new(Target {
                    url: "http://x.com".into(),
                    title: Text::default(),
                }),
            )])]
        );
    }

    #[test]
    fn hyperlink_field_without_quotes_becomes_link() {
        let doc = read(
            r#"{\rtf1\ansi {\field{\*\fldinst HYPERLINK http://x.com \o "tip"}{\fldrslt click}}\par}"#,
        );
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Link(
                Box::default(),
                vec![s("click")],
                Box::new(Target {
                    url: "http://x.com".into(),
                    title: Text::default(),
                }),
            )])]
        );
    }

    #[test]
    fn list_table_numbering_becomes_ordered() {
        let doc = read(
            r"{\rtf1\ansi{\listtable{\list{\listlevel\levelnfc4\levelstartat3{\leveltext\'02\'00.;}}\listid1}}{\listoverridetable{\listoverride\listid1\ls1}}\pard\ls1\ilvl0 First\par\pard\ls1\ilvl0 Second\par}",
        );
        assert_eq!(
            doc.blocks,
            vec![Block::OrderedList(
                ListAttributes {
                    start: 3,
                    style: ListNumberStyle::LowerAlpha,
                    delim: ListNumberDelim::Period,
                },
                vec![vec![para(vec![s("First")])], vec![para(vec![s("Second")])],],
            )]
        );
    }

    #[test]
    fn list_without_a_table_stays_a_bullet() {
        let doc = read(
            r"{\rtf1\ansi {\listtext\'B7}\ls1\ilvl0 First\par {\listtext\'B7}\ls1\ilvl0 Second\par}",
        );
        assert_eq!(
            doc.blocks,
            vec![Block::BulletList(vec![
                vec![para(vec![s("First")])],
                vec![para(vec![s("Second")])],
            ])]
        );
    }

    #[test]
    fn footnote_becomes_note() {
        let doc = read(r"{\rtf1\ansi text{\footnote note body}more\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                s("text"),
                Inline::Note(vec![para(vec![s("note"), Inline::Space, s("body")])]),
                s("more"),
            ])]
        );
    }

    #[test]
    fn bookmark_becomes_span() {
        let doc = read(r"{\rtf1\ansi {\*\bkmkstart mark}anchored{\*\bkmkend mark}\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Span(
                Box::new(Attr {
                    id: "mark".into(),
                    classes: Vec::new(),
                    attributes: Vec::new(),
                }),
                vec![s("anchored")],
            )])]
        );
    }

    #[test]
    fn table_is_reconstructed() {
        let doc = read(
            r"{\rtf1\ansi \trowd\cellx3000\cellx6000\pard\intbl A\cell\pard\intbl B\cell\row\pard after\par}",
        );
        let Some(Block::Table(table)) = doc.blocks.first() else {
            panic!("expected a leading table, got {:?}", doc.blocks);
        };
        assert_eq!(table.col_specs.len(), 2);
        assert_eq!(table.head.rows.len(), 0);
        let body = table.bodies.first().expect("body");
        assert_eq!(body.row_head_columns, 0);
        assert_eq!(body.body.len(), 1);
        let row = body.body.first().expect("row");
        assert_eq!(row.cells.len(), 2);
        assert_eq!(
            row.cells.first().expect("cell").content,
            vec![para(vec![s("A")])]
        );
        assert_eq!(
            row.cells.get(1).expect("cell").content,
            vec![para(vec![s("B")])]
        );
        assert_eq!(doc.blocks.get(1), Some(&para(vec![s("after")])));
    }

    #[test]
    fn picture_decodes_into_media() {
        let (doc, media) = read_media(r"{\rtf1\ansi {\pict\pngblip 89504e47}\par}");
        let name = content_addressed_name("image/png", &[0x89, 0x50, 0x4e, 0x47]);
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::Image(
                Box::default(),
                vec![s("image")],
                Box::new(Target {
                    url: name.clone().into(),
                    title: Text::default(),
                }),
            )])]
        );
        assert!(media.contains(&name));
    }

    #[test]
    fn shape_picture_becomes_inline_image() {
        let (doc, media) = read_media(
            r"{\rtf1\ansi A{\shp{\*\shpinst{\sp{\sn pib}{\sv {\pict\pngblip 89504e470d0a1a0a}}}}}B\par}",
        );
        let name = content_addressed_name(
            "image/png",
            &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
        );
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                s("A"),
                Inline::Image(
                    Box::default(),
                    vec![s("image")],
                    Box::new(Target {
                        url: name.clone().into(),
                        title: Text::default(),
                    }),
                ),
                s("B"),
            ])]
        );
        assert!(media.contains(&name));
    }

    #[test]
    fn shape_text_box_becomes_paragraph() {
        let doc = read(
            r"{\rtf1\ansi Para one.\par {\shp{\*\shpinst{\shptxt \pard Callout text here.\par}}}Para two.\par}",
        );
        assert_eq!(
            doc.blocks,
            vec![
                para(vec![s("Para"), Inline::Space, s("one.")]),
                para(vec![
                    s("Callout"),
                    Inline::Space,
                    s("text"),
                    Inline::Space,
                    s("here."),
                ]),
                para(vec![s("Para"), Inline::Space, s("two.")]),
            ]
        );
    }

    #[test]
    fn raw_high_bytes_fall_back_to_latin1() {
        // Byte 0xE9 sits in a stream that is not valid UTF-8, so the whole document is read as
        // Latin-1: 0xE9 -> U+00E9 (é). A `\'xx` escape keeps its code-page reading regardless.
        let doc = read_bytes(b"{\\rtf1\\ansi caf\xe9 here\\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("caf\u{00E9}"), Inline::Space, s("here")])]
        );
        let doc = read_bytes(b"{\\rtf1\\ansi A\x93B\xa0C\\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("A\u{0093}B\u{00A0}C")])]);
    }

    #[test]
    fn empty_paragraphs_are_dropped() {
        let doc = read(r"{\rtf1\ansi text\par\par\par more\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![s("text")]), para(vec![s("more")])]
        );
    }

    #[test]
    fn trailing_content_without_par_flushes() {
        let doc = read(r"{\rtf1\ansi Hello}");
        assert_eq!(doc.blocks, vec![para(vec![s("Hello")])]);
    }

    #[test]
    fn allcaps_uppercases_text() {
        let doc = read(r"{\rtf1\ansi \caps upper\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("UPPER")])]);
    }

    #[test]
    fn small_caps_wraps() {
        let doc = read(r"{\rtf1\ansi \scaps x\par}");
        assert_eq!(
            doc.blocks,
            vec![para(vec![Inline::SmallCaps(vec![s("x")])])]
        );
    }

    #[test]
    fn down_level_numbering_placeholder_is_skipped() {
        // `\pntext`/`\pntxtb`/`\pntxta` carry the down-level rendering of an auto-number or bullet;
        // the surrounding text is what belongs to the paragraph, so the placeholders drop out and
        // the words on either side join directly.
        let doc = read(r"{\rtf1\ansi before{\pntxtb X}{\pntxta Y}after\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("beforeafter")])]);

        let doc = read(r"{\rtf1\ansi {\pntext\pnlvlblt\pnf1{\pntxtb\'B7}}Item\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("Item")])]);
    }

    #[test]
    fn binary_data_is_consumed_by_byte_count() {
        // `\binN` introduces exactly N raw bytes; they are not text and must not desync the parse.
        // Here the three bytes `ABC` are swallowed, leaving the words around them adjacent.
        let doc = read(r"{\rtf1\ansi price\bin3ABCtag\par}");
        assert_eq!(doc.blocks, vec![para(vec![s("pricetag")])]);
    }

    #[test]
    fn binary_picture_decodes_into_media() {
        // A `\bin`-encoded picture payload decodes just like a hex one. The raw bytes here include
        // `0x7d` (`}`): captured as data at the lexer, it neither ends the picture group early nor
        // corrupts the rest of the document.
        let (doc, media) = read_media_bytes(
            b"{\\rtf1\\ansi before {\\pict\\pngblip\\bin4 \x89\x7d\x50\x47}after\\par}",
        );
        let name = content_addressed_name("image/png", &[0x89, 0x7d, 0x50, 0x47]);
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                s("before"),
                Inline::Space,
                Inline::Image(
                    Box::default(),
                    vec![s("image")],
                    Box::new(Target {
                        url: name.clone().into(),
                        title: Text::default(),
                    }),
                ),
                s("after"),
            ])]
        );
        assert!(media.contains(&name));
    }

    #[test]
    fn hyperlink_display_keeps_edge_space() {
        // A space at the start or end of a link's display text is part of the surrounding sentence
        // and is preserved, so the link does not fuse with the adjacent word.
        let doc = read(
            r#"{\rtf1\ansi {\field{\*\fldinst{HYPERLINK "http://x.com"}}{\fldrslt link }}after\par}"#,
        );
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                Inline::Link(
                    Box::default(),
                    vec![s("link"), Inline::Space],
                    Box::new(Target {
                        url: "http://x.com".into(),
                        title: Text::default(),
                    }),
                ),
                s("after"),
            ])]
        );

        let doc = read(
            r#"{\rtf1\ansi {\field{\*\fldinst{HYPERLINK "http://y.com"}}{\fldrslt  lead}}tail\par}"#,
        );
        assert_eq!(
            doc.blocks,
            vec![para(vec![
                Inline::Link(
                    Box::default(),
                    vec![Inline::Space, s("lead")],
                    Box::new(Target {
                        url: "http://y.com".into(),
                        title: Text::default(),
                    }),
                ),
                s("tail"),
            ])]
        );
    }

    #[test]
    fn empty_input_is_empty_document() {
        let doc = read("");
        assert!(doc.blocks.is_empty());
        assert!(doc.meta.is_empty());
    }
}
