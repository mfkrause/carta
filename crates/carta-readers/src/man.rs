//! Reader for the `man` macro package (the `groff`/`troff` manual-page language).
//!
//! A manual page is a sequence of control lines (a request or macro, introduced by `.` or `'` in
//! the first column) and text lines. Text lines are *filled*: consecutive lines collapse into one
//! paragraph, their words separated by single spaces. Macros structure the page — section headings
//! (`.SH`/`.SS`), paragraph breaks (`.PP`), tagged and indented lists (`.TP`/`.IP`), relative insets
//! (`.RS`/`.RE`), verbatim regions (`.nf`/`.EX`), and hyperlinks (`.UR`/`.MT`). Inline font macros
//! (`.B`, `.I`, `.BR`, …) and the `\f` escape switch between roman, bold, and italic; the `\(xx`,
//! `\[…]`, and `\*x` escapes produce special characters and predefined strings.
//!
//! The title macro `.TH` populates document metadata (`title`, `section`, `date`, `footer`,
//! `header`); everything else becomes the block sequence.

use std::collections::BTreeMap;

use carta_ast::{
    Attr, Block, Document, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MetaValue,
    Target, to_plain_text,
};
use carta_core::{Extensions, Reader, ReaderOptions, Result};

use crate::heading_ids::{IdRegistry, IdScheme};
use crate::inline_text::trim_inline_ends;

/// Parses a manual page written in the `man` macro language into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct ManReader;

impl Reader for ManReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let lines: Vec<&str> = input
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .collect();
        let mut parser = Parser::new(lines, options.extensions);
        let blocks = parser.parse_blocks(Ctx::TOP);
        Ok(Document {
            meta: parser.meta,
            blocks,
            ..Document::default()
        })
    }
}

/// The active typeface for a run of text. `\f(BI` and the `.BI`/`.IB` macros render bold-italic as
/// emphasis wrapping strong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Font {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Font {
    /// Wraps already-built inline content in the markup for this font; roman content is unwrapped.
    fn wrap(self, inlines: Vec<Inline>) -> Vec<Inline> {
        if inlines.is_empty() {
            return Vec::new();
        }
        match self {
            Font::Regular => inlines,
            Font::Bold => vec![Inline::Strong(inlines)],
            Font::Italic => vec![Inline::Emph(inlines)],
            Font::BoldItalic => vec![Inline::Emph(vec![Inline::Strong(inlines)])],
        }
    }
}

/// What may end a block sequence early. Section headings always rise to the top level; a new
/// paragraph or list-item macro closes an open list-item body so the list can continue or finish.
#[derive(Debug, Clone, Copy)]
struct Ctx {
    /// Inside a `.RS` inset, so a closing `.RE` returns control to the inset's opener.
    in_inset: bool,
    /// Inside a list item's body, so a sibling item or a paragraph break ends the body.
    in_item: bool,
}

impl Ctx {
    const TOP: Ctx = Ctx {
        in_inset: false,
        in_item: false,
    };
    const INSET: Ctx = Ctx {
        in_inset: true,
        in_item: false,
    };
    const ITEM: Ctx = Ctx {
        in_inset: false,
        in_item: true,
    };
}

/// Hands out heading identifiers in reading order, disambiguating repeats the way the active
/// auto-identifier extension prescribes.
struct HeadingIds {
    scheme: Option<IdScheme>,
    registry: IdRegistry,
}

impl HeadingIds {
    fn new(extensions: Extensions) -> Self {
        Self {
            scheme: IdScheme::select(extensions, false),
            registry: IdRegistry::default(),
        }
    }

    fn assign(&mut self, inlines: &[Inline]) -> String {
        match self.scheme {
            None => String::new(),
            Some(scheme) => self.registry.assign(scheme, &to_plain_text(inlines)),
        }
    }
}

struct Parser<'a> {
    lines: Vec<&'a str>,
    pos: usize,
    meta: BTreeMap<String, MetaValue>,
    headings: HeadingIds,
}

impl<'a> Parser<'a> {
    fn new(lines: Vec<&'a str>, extensions: Extensions) -> Self {
        Self {
            lines,
            pos: 0,
            meta: BTreeMap::new(),
            headings: HeadingIds::new(extensions),
        }
    }

    fn peek(&self) -> Option<&'a str> {
        self.lines.get(self.pos).copied()
    }

    fn advance(&mut self) {
        self.pos += 1;
    }

    /// Consumes and returns the next line, if any.
    fn take_line(&mut self) -> Option<&'a str> {
        let line = self.lines.get(self.pos).copied();
        if line.is_some() {
            self.pos += 1;
        }
        line
    }

    /// Parses a sequence of blocks until the context's terminator (or end of input). A terminator
    /// line is left unconsumed for the caller, except a `.RE` that closes the inset it belongs to.
    // The macro dispatch lists names separately for clarity even where their handling coincides.
    #[allow(clippy::too_many_lines, clippy::match_same_arms)]
    fn parse_blocks(&mut self, ctx: Ctx) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut fill = Vec::new();
        while let Some(line) = self.peek() {
            if line.trim().is_empty() {
                flush_para(&mut fill, &mut blocks);
                self.advance();
                continue;
            }
            let Some((name, rest)) = control_parts(line) else {
                self.advance();
                append_text(&mut fill, tokenize(line, Font::Regular));
                continue;
            };
            if is_comment(line) {
                self.advance();
                continue;
            }
            match name {
                "SH" | "SS" => {
                    if ctx.in_inset || ctx.in_item {
                        flush_para(&mut fill, &mut blocks);
                        return blocks;
                    }
                    flush_para(&mut fill, &mut blocks);
                    self.advance();
                    let level = if name == "SH" { 1 } else { 2 };
                    let inlines = self.heading_inlines(rest);
                    let id = self.headings.assign(&inlines);
                    blocks.push(Block::Header(
                        level,
                        Attr {
                            id,
                            ..Attr::default()
                        },
                        inlines,
                    ));
                }
                "PP" | "LP" | "P" | "HP" => {
                    flush_para(&mut fill, &mut blocks);
                    if ctx.in_item {
                        return blocks;
                    }
                    self.advance();
                }
                "TP" | "IP" => {
                    flush_para(&mut fill, &mut blocks);
                    if ctx.in_item {
                        return blocks;
                    }
                    let list = self.parse_list();
                    blocks.extend(list);
                }
                "TQ" => {
                    flush_para(&mut fill, &mut blocks);
                    if ctx.in_item {
                        return blocks;
                    }
                    self.advance();
                }
                "RS" => {
                    flush_para(&mut fill, &mut blocks);
                    self.advance();
                    let inner = self.parse_blocks(Ctx::INSET);
                    if ctx.in_item {
                        blocks.extend(inner);
                    } else {
                        blocks.push(Block::BlockQuote(inner));
                    }
                }
                "RE" => {
                    flush_para(&mut fill, &mut blocks);
                    self.advance();
                    if ctx.in_inset {
                        return blocks;
                    }
                }
                "nf" | "EX" => {
                    flush_para(&mut fill, &mut blocks);
                    self.advance();
                    blocks.push(self.parse_verbatim());
                }
                "fi" | "EE" | "UE" | "ME" => {
                    flush_para(&mut fill, &mut blocks);
                    self.advance();
                }
                "TS" => {
                    flush_para(&mut fill, &mut blocks);
                    self.advance();
                    blocks.push(self.parse_tbl());
                }
                "br" => {
                    self.advance();
                    fill.push(Inline::LineBreak);
                }
                "sp" => {
                    flush_para(&mut fill, &mut blocks);
                    self.advance();
                }
                "TH" => {
                    self.advance();
                    self.parse_title(rest);
                }
                "B" | "I" => {
                    self.advance();
                    let font = single_font(name);
                    let text = if rest.is_empty() {
                        self.take_line().unwrap_or("").to_owned()
                    } else {
                        split_args(rest).join(" ")
                    };
                    append_text(&mut fill, font_macro(font, &text));
                }
                "BR" | "RB" | "BI" | "IB" | "RI" | "IR" => {
                    self.advance();
                    let rest = if rest.is_empty() {
                        self.take_line().unwrap_or("").to_owned()
                    } else {
                        rest.to_owned()
                    };
                    append_text(&mut fill, alternating(&rest, fonts_for(name)));
                }
                "UR" | "MT" => {
                    self.advance();
                    let url = split_args(rest).into_iter().next().unwrap_or_default();
                    let url = if name == "MT" {
                        format!("mailto:{url}")
                    } else {
                        url
                    };
                    self.parse_link(url, &mut fill);
                }
                _ => {
                    flush_para(&mut fill, &mut blocks);
                    self.advance();
                }
            }
        }
        flush_para(&mut fill, &mut blocks);
        blocks
    }

    /// Heading inline content: the macro's arguments joined by spaces, or — when the macro carries
    /// none — the following input line.
    fn heading_inlines(&mut self, rest: &str) -> Vec<Inline> {
        if rest.is_empty() {
            let next = self.take_line().unwrap_or("").to_owned();
            tokenize(&next, Font::Regular)
        } else {
            tokenize(&split_args(rest).join(" "), Font::Regular)
        }
    }

    /// Reads `.TH` arguments into metadata: identifier, section, date, footer, header.
    fn parse_title(&mut self, rest: &str) {
        let keys = ["title", "section", "date", "footer", "header"];
        for (key, arg) in keys.iter().zip(split_args(rest)) {
            if arg.is_empty() {
                continue;
            }
            let inlines = tokenize(&arg, Font::Regular);
            self.meta
                .insert((*key).to_owned(), MetaValue::MetaInlines(inlines));
        }
    }

    /// Collects a verbatim region (`.nf`/`.EX`) as a code block. Lines keep their literal spacing;
    /// escapes and font macros are reduced to plain text. The region ends at `.fi`/`.EE`, or at a
    /// section heading or end of input (both left unconsumed).
    fn parse_verbatim(&mut self) -> Block {
        let mut text_lines: Vec<String> = Vec::new();
        while let Some(line) = self.peek() {
            if let Some((name, rest)) = control_parts(line) {
                if is_comment(line) {
                    self.advance();
                    continue;
                }
                match name {
                    "fi" | "EE" => {
                        self.advance();
                        break;
                    }
                    "SH" | "SS" => break,
                    "B" | "I" | "BR" | "RB" | "BI" | "IB" | "RI" | "IR" => {
                        self.advance();
                        text_lines.push(flatten(&split_args(rest).join(" ")));
                    }
                    _ => self.advance(),
                }
            } else {
                self.advance();
                text_lines.push(flatten(line));
            }
        }
        Block::CodeBlock(Attr::default(), text_lines.join("\n"))
    }

    /// Collects a tbl table region (`.TS`/`.TE`) as a code block. The table preprocessor's layout
    /// directives are not interpreted; the region's literal lines (options, format, and cell rows)
    /// are kept verbatim, with font macros and escapes reduced to plain text. The region ends at
    /// `.TE`, or at a section heading or end of input (both left unconsumed).
    fn parse_tbl(&mut self) -> Block {
        let mut text_lines: Vec<String> = Vec::new();
        while let Some(line) = self.peek() {
            if let Some((name, _)) = control_parts(line) {
                if is_comment(line) {
                    self.advance();
                    continue;
                }
                match name {
                    "TE" => {
                        self.advance();
                        break;
                    }
                    "SH" | "SS" => break,
                    _ => self.advance(),
                }
            } else {
                self.advance();
                text_lines.push(flatten(line));
            }
        }
        Block::CodeBlock(Attr::default(), text_lines.join("\n"))
    }

    /// Parses a run of consecutive `.TP`/`.IP` items into list blocks. Items of the same kind merge
    /// into one list; an unmarked `.IP` becomes a standalone inset.
    fn parse_list(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        let mut pending: Option<Pending> = None;
        while let Some(line) = self.peek() {
            let Some((name, rest)) = control_parts(line) else {
                break;
            };
            if is_comment(line) {
                self.advance();
                continue;
            }
            match name {
                "TP" => {
                    self.advance();
                    let term = self.read_term();
                    let body = self.parse_blocks(Ctx::ITEM);
                    push_definition(&mut pending, &mut out, term, body);
                }
                "IP" => {
                    self.advance();
                    let args = split_args(rest);
                    let mark = args.first().map_or("", String::as_str);
                    match classify_mark(mark) {
                        Mark::None => {
                            flush_pending(&mut pending, &mut out);
                            let body = self.parse_blocks(Ctx::ITEM);
                            out.push(Block::BlockQuote(body));
                        }
                        Mark::Bullet => {
                            let body = self.parse_blocks(Ctx::ITEM);
                            push_bullet(&mut pending, &mut out, body);
                        }
                        Mark::Ordered(attrs) => {
                            let body = self.parse_blocks(Ctx::ITEM);
                            push_ordered(&mut pending, &mut out, attrs, body);
                        }
                        Mark::Text => {
                            let term = tokenize(mark, Font::Regular);
                            let body = self.parse_blocks(Ctx::ITEM);
                            push_definition(&mut pending, &mut out, term, body);
                        }
                    }
                }
                _ => break,
            }
        }
        flush_pending(&mut pending, &mut out);
        out
    }

    /// The term of a `.TP` item: the next line, which is either a font macro or plain text.
    fn read_term(&mut self) -> Vec<Inline> {
        let Some(line) = self.take_line() else {
            return Vec::new();
        };
        if let Some((name, rest)) = control_parts(line) {
            if is_comment(line) {
                return self.read_term();
            }
            match name {
                "B" | "I" => {
                    let font = single_font(name);
                    return font_macro(font, &split_args(rest).join(" "));
                }
                "BR" | "RB" | "BI" | "IB" | "RI" | "IR" => {
                    return alternating(rest, fonts_for(name));
                }
                _ => return tokenize(rest, Font::Regular),
            }
        }
        tokenize(line, Font::Regular)
    }

    /// Collects a hyperlink's label between `.UR`/`.MT` and its `.UE`/`.ME` terminator, appending
    /// the resulting link to the open paragraph. Text after the terminator attaches without a space.
    fn parse_link(&mut self, url: String, fill: &mut Vec<Inline>) {
        let mut label = Vec::new();
        let mut trailing = String::new();
        while let Some(line) = self.peek() {
            if let Some((name, rest)) = control_parts(line) {
                if is_comment(line) {
                    self.advance();
                    continue;
                }
                match name {
                    "UE" | "ME" => {
                        self.advance();
                        trailing = split_args(rest).join(" ");
                        break;
                    }
                    "br" => {
                        self.advance();
                        label.push(Inline::LineBreak);
                    }
                    "B" | "I" => {
                        self.advance();
                        let font = single_font(name);
                        append_text(&mut label, font_macro(font, &split_args(rest).join(" ")));
                    }
                    "BR" | "RB" | "BI" | "IB" | "RI" | "IR" => {
                        self.advance();
                        append_text(&mut label, alternating(rest, fonts_for(name)));
                    }
                    _ => break,
                }
            } else {
                self.advance();
                append_text(&mut label, tokenize(line, Font::Regular));
            }
        }
        append_text(
            fill,
            vec![Inline::Link(
                Attr::default(),
                label,
                Target {
                    url,
                    title: String::new(),
                },
            )],
        );
        if !trailing.is_empty() {
            fill.extend(tokenize(&trailing, Font::Regular));
        }
    }
}

/// The font a single-font macro selects: `.B` is bold, every other (`.I`) is italic.
fn single_font(name: &str) -> Font {
    if name == "B" {
        Font::Bold
    } else {
        Font::Italic
    }
}

/// The two alternating fonts of an alternating font macro, applied to arguments in turn.
fn fonts_for(name: &str) -> [Font; 2] {
    match name {
        "BR" => [Font::Bold, Font::Regular],
        "RB" => [Font::Regular, Font::Bold],
        "BI" => [Font::Bold, Font::Italic],
        "IB" => [Font::Italic, Font::Bold],
        "RI" => [Font::Regular, Font::Italic],
        _ => [Font::Italic, Font::Regular],
    }
}

/// Renders a single-font macro (`.B`/`.I`): the whole argument is read as roman text and then
/// wrapped once in the macro's font, so an inner `\f` font change nests inside that font rather than
/// replacing it.
fn font_macro(font: Font, text: &str) -> Vec<Inline> {
    font.wrap(tokenize(text, Font::Regular))
}

/// Renders an alternating font macro: each argument takes the next font in the cycle, is read as
/// roman text, and is wrapped in that font; the rendered arguments abut with no separating space.
fn alternating(rest: &str, fonts: [Font; 2]) -> Vec<Inline> {
    let mut out = Vec::new();
    for (index, arg) in split_args(rest).into_iter().enumerate() {
        let font = fonts.get(index % 2).copied().unwrap_or(Font::Regular);
        out.extend(font.wrap(tokenize(&arg, Font::Regular)));
    }
    out
}

/// What kind of list a `.IP` marker introduces.
enum Mark {
    None,
    Bullet,
    Ordered(ListAttributes),
    Text,
}

/// Classifies a `.IP` marker: a bullet glyph, an enumerator (decimal, alphabetic, or roman), or
/// arbitrary text that becomes a definition term.
fn classify_mark(mark: &str) -> Mark {
    if mark.is_empty() {
        return Mark::None;
    }
    if mark == "*" || mark == "\u{2022}" || mark == "\u{00b7}" || mark == "\\(bu" {
        return Mark::Bullet;
    }
    if let Some(attrs) = parse_enumerator(mark) {
        return Mark::Ordered(attrs);
    }
    Mark::Text
}

/// Parses an ordered-list enumerator (`1.`, `a)`, `iv.`, a bare letter, …) into its list
/// attributes, or returns `None` when the marker is not an enumerator.
fn parse_enumerator(mark: &str) -> Option<ListAttributes> {
    let (body, delim) = match mark.strip_suffix('.') {
        Some(body) => (body, ListNumberDelim::Period),
        None => match mark.strip_suffix(')') {
            Some(body) => (body, ListNumberDelim::OneParen),
            None => (mark, ListNumberDelim::DefaultDelim),
        },
    };
    if body.is_empty() {
        return None;
    }
    if body.chars().all(|c| c.is_ascii_digit()) {
        let start = body.parse().ok()?;
        return Some(ListAttributes {
            start,
            style: ListNumberStyle::Decimal,
            delim,
        });
    }
    if let Some(start) = roman_value(body) {
        let style = if body.chars().next().is_some_and(char::is_uppercase) {
            ListNumberStyle::UpperRoman
        } else {
            ListNumberStyle::LowerRoman
        };
        return Some(ListAttributes {
            start,
            style,
            delim,
        });
    }
    let mut chars = body.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii_alphabetic()
    {
        let start = i32::from((c.to_ascii_lowercase() as u8) - b'a') + 1;
        let style = if c.is_ascii_uppercase() {
            ListNumberStyle::UpperAlpha
        } else {
            ListNumberStyle::LowerAlpha
        };
        return Some(ListAttributes {
            start,
            style,
            delim,
        });
    }
    None
}

/// The value of a roman numeral, or `None` if the string is not a well-formed roman numeral.
fn roman_value(text: &str) -> Option<i32> {
    fn digit(c: char) -> Option<i32> {
        match c.to_ascii_lowercase() {
            'i' => Some(1),
            'v' => Some(5),
            'x' => Some(10),
            'l' => Some(50),
            'c' => Some(100),
            'd' => Some(500),
            'm' => Some(1000),
            _ => None,
        }
    }
    let values: Vec<i32> = text.chars().map(digit).collect::<Option<Vec<_>>>()?;
    let mut total = 0;
    for (index, &value) in values.iter().enumerate() {
        match values.get(index + 1) {
            Some(&next) if value < next => total -= value,
            _ => total += value,
        }
    }
    (total > 0).then_some(total)
}

/// The accumulating list of the current kind. Consecutive same-kind items append to it; a
/// different kind flushes it first.
enum Pending {
    Definition(Vec<(Vec<Inline>, Vec<Vec<Block>>)>),
    Bullet(Vec<Vec<Block>>),
    Ordered(ListAttributes, Vec<Vec<Block>>),
}

fn flush_pending(pending: &mut Option<Pending>, out: &mut Vec<Block>) {
    match pending.take() {
        Some(Pending::Definition(items)) => out.push(Block::DefinitionList(items)),
        Some(Pending::Bullet(items)) => out.push(Block::BulletList(items)),
        Some(Pending::Ordered(attrs, items)) => out.push(Block::OrderedList(attrs, items)),
        None => {}
    }
}

fn push_definition(
    pending: &mut Option<Pending>,
    out: &mut Vec<Block>,
    term: Vec<Inline>,
    body: Vec<Block>,
) {
    if let Some(Pending::Definition(items)) = pending {
        items.push((term, vec![body]));
        return;
    }
    flush_pending(pending, out);
    *pending = Some(Pending::Definition(vec![(term, vec![body])]));
}

fn push_bullet(pending: &mut Option<Pending>, out: &mut Vec<Block>, body: Vec<Block>) {
    if let Some(Pending::Bullet(items)) = pending {
        items.push(body);
        return;
    }
    flush_pending(pending, out);
    *pending = Some(Pending::Bullet(vec![body]));
}

fn push_ordered(
    pending: &mut Option<Pending>,
    out: &mut Vec<Block>,
    attrs: ListAttributes,
    body: Vec<Block>,
) {
    if let Some(Pending::Ordered(_, items)) = pending {
        items.push(body);
        return;
    }
    flush_pending(pending, out);
    *pending = Some(Pending::Ordered(attrs, vec![body]));
}

/// Moves an open paragraph's inlines into the block list, dropping a paragraph that holds nothing
/// but spacing.
fn flush_para(fill: &mut Vec<Inline>, blocks: &mut Vec<Block>) {
    let mut trimmed = std::mem::take(fill);
    trim_inline_ends(&mut trimmed);
    if !trimmed.is_empty() {
        blocks.push(Block::Para(trimmed));
    }
}

/// Appends fillable inline content to the open paragraph, inserting a single separating space
/// unless the paragraph is empty or already ends at a line break.
fn append_text(fill: &mut Vec<Inline>, inlines: Vec<Inline>) {
    if inlines.is_empty() {
        return;
    }
    if !fill.is_empty() && !matches!(fill.last(), Some(Inline::LineBreak)) {
        fill.push(Inline::Space);
    }
    fill.extend(inlines);
}

/// Whether a line is a control line — one introduced by the `.` or `'` control character.
fn is_control(line: &str) -> bool {
    line.starts_with('.') || line.starts_with('\'')
}

/// Whether a control line is a comment (`.\"` or `.\#`).
fn is_comment(line: &str) -> bool {
    if !is_control(line) {
        return false;
    }
    let body = line.get(1..).unwrap_or("");
    body.starts_with("\\\"") || body.starts_with("\\#")
}

/// Splits a control line into its request name and the remaining argument text, or returns `None`
/// for a text line.
fn control_parts(line: &str) -> Option<(&str, &str)> {
    if !is_control(line) {
        return None;
    }
    let body = line.get(1..).unwrap_or("");
    match body.split_once([' ', '\t']) {
        Some((name, rest)) => Some((name, rest.trim_start_matches([' ', '\t']))),
        None => Some((body, "")),
    }
}

/// Splits a macro argument string the way `groff` does: on spaces and tabs, with double quotes
/// grouping an argument that may contain spaces and `""` denoting a literal quote. A backslash keeps
/// the following character (so an escaped space does not split).
fn split_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while matches!(chars.peek(), Some(' ' | '\t')) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut arg = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        arg.push('"');
                    } else {
                        break;
                    }
                } else {
                    arg.push(c);
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' || c == '\t' {
                    break;
                }
                chars.next();
                arg.push(c);
                if c == '\\'
                    && let Some(next) = chars.next()
                {
                    arg.push(next);
                }
            }
        }
        args.push(arg);
    }
    args
}

/// A scanned character together with the font in effect, or an inter-word separator.
enum Atom {
    Char(Font, char),
    Space,
}

/// Tokenizes a line of `man` text into inlines: words become [`Inline::Str`], runs of whitespace a
/// single [`Inline::Space`], and font runs wrap in the appropriate markup. Leading and trailing
/// spaces are dropped.
fn tokenize(text: &str, start_font: Font) -> Vec<Inline> {
    let atoms = scan(text, start_font);
    let mut result: Vec<Inline> = Vec::new();
    let mut run: Vec<Inline> = Vec::new();
    let mut run_font = Font::Regular;
    let mut word = String::new();
    let mut word_font = Font::Regular;
    let mut pending_space = false;

    let commit_word = |word: &mut String,
                       word_font: Font,
                       run: &mut Vec<Inline>,
                       run_font: &mut Font,
                       result: &mut Vec<Inline>,
                       pending_space: &mut bool| {
        if word.is_empty() {
            return;
        }
        let text = std::mem::take(word);
        if !run.is_empty() && word_font == *run_font {
            if *pending_space {
                run.push(Inline::Space);
            }
            run.push(Inline::Str(text));
        } else {
            flush_run(run, *run_font, result);
            if *pending_space {
                push_space(result);
            }
            *run_font = word_font;
            run.push(Inline::Str(text));
        }
        *pending_space = false;
    };

    for atom in atoms {
        match atom {
            Atom::Char(font, c) => {
                if !word.is_empty() && font != word_font {
                    commit_word(
                        &mut word,
                        word_font,
                        &mut run,
                        &mut run_font,
                        &mut result,
                        &mut pending_space,
                    );
                }
                if word.is_empty() {
                    word_font = font;
                }
                word.push(c);
            }
            Atom::Space => {
                commit_word(
                    &mut word,
                    word_font,
                    &mut run,
                    &mut run_font,
                    &mut result,
                    &mut pending_space,
                );
                pending_space = true;
            }
        }
    }
    commit_word(
        &mut word,
        word_font,
        &mut run,
        &mut run_font,
        &mut result,
        &mut pending_space,
    );
    flush_run(&mut run, run_font, &mut result);
    trim_inline_ends(&mut result);
    result
}

fn flush_run(run: &mut Vec<Inline>, run_font: Font, result: &mut Vec<Inline>) {
    if !run.is_empty() {
        result.extend(run_font.wrap(std::mem::take(run)));
    }
}

/// Appends a single top-level space, coalescing with any space already present.
fn push_space(result: &mut Vec<Inline>) {
    if !result.is_empty() && !matches!(result.last(), Some(Inline::Space)) {
        result.push(Inline::Space);
    }
}

/// Reduces a line to plain text for a verbatim region: escapes and special characters resolve, font
/// markup is discarded, and literal spacing is preserved.
fn flatten(text: &str) -> String {
    let mut out = String::new();
    for atom in scan(text, Font::Regular) {
        match atom {
            Atom::Char(_, c) => out.push(c),
            Atom::Space => out.push(' '),
        }
    }
    out
}

/// Scans a line into atoms, resolving escape sequences. Font escapes (`\f…`) update the running
/// font; an inline comment (`\"`/`\#`) ends the line.
#[allow(clippy::too_many_lines)]
fn scan(text: &str, start_font: Font) -> Vec<Atom> {
    let mut atoms = Vec::new();
    let mut chars = text.chars().peekable();
    let mut font = start_font;
    let mut previous = start_font;
    while let Some(c) = chars.next() {
        if c == ' ' || c == '\t' {
            atoms.push(Atom::Space);
            continue;
        }
        if c != '\\' {
            atoms.push(Atom::Char(font, c));
            continue;
        }
        let Some(&escape) = chars.peek() else {
            break;
        };
        match escape {
            'f' => {
                chars.next();
                let name = read_escape_name(&mut chars);
                apply_font(&name, &mut font, &mut previous);
            }
            '"' | '#' => break,
            '-' => {
                chars.next();
                atoms.push(Atom::Char(font, '-'));
            }
            'e' | '\\' => {
                chars.next();
                atoms.push(Atom::Char(font, '\\'));
            }
            '.' => {
                chars.next();
                atoms.push(Atom::Char(font, '.'));
            }
            ' ' => {
                chars.next();
                atoms.push(Atom::Space);
            }
            '~' => {
                chars.next();
                atoms.push(Atom::Char(font, '\u{00a0}'));
            }
            '0' => {
                chars.next();
                atoms.push(Atom::Char(font, '\u{2007}'));
            }
            '^' => {
                chars.next();
                atoms.push(Atom::Char(font, '\u{200a}'));
            }
            '|' => {
                chars.next();
                atoms.push(Atom::Char(font, '\u{2006}'));
            }
            '&' | ')' | ',' | '/' | ':' | '!' | '%' | '{' | '}' => {
                chars.next();
            }
            '(' => {
                chars.next();
                let name: String = (&mut chars).take(2).collect();
                push_chars(&mut atoms, font, special_char(&name));
            }
            '[' => {
                chars.next();
                let name = read_delimited(&mut chars, ']');
                push_chars(&mut atoms, font, bracket_char(&name));
            }
            '*' => {
                chars.next();
                let name = read_escape_name(&mut chars);
                for c in string_escape(&name).chars() {
                    atoms.push(Atom::Char(font, c));
                }
            }
            's' => {
                chars.next();
                skip_size(&mut chars);
            }
            'n' => {
                chars.next();
                let _ = read_escape_name(&mut chars);
            }
            'h' | 'v' | 'w' | 'o' | 'b' | 'l' | 'L' | 'D' | 'C' | 'N' | 'R' | 'A' | 'Z' | 'X'
            | 'M' | 'B' | 'd' | 'u' | 'z' | 'k' => {
                chars.next();
                skip_delimited_arg(&mut chars);
            }
            other => {
                chars.next();
                atoms.push(Atom::Char(font, other));
            }
        }
    }
    atoms
}

fn push_chars(atoms: &mut Vec<Atom>, font: Font, mapped: Option<char>) {
    atoms.push(Atom::Char(font, mapped.unwrap_or('\u{fffd}')));
}

/// Reads an escape name after `\f`, `\*` or `\n`: one character, a two-character `(xx` name, or a
/// `[name]` group.
fn read_escape_name(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    match chars.peek() {
        Some('(') => {
            chars.next();
            chars.take(2).collect()
        }
        Some('[') => {
            chars.next();
            read_delimited(chars, ']')
        }
        Some(_) => chars.next().map(String::from).unwrap_or_default(),
        None => String::new(),
    }
}

fn read_delimited(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, close: char) -> String {
    let mut name = String::new();
    for c in chars.by_ref() {
        if c == close {
            break;
        }
        name.push(c);
    }
    name
}

/// Skips an argument delimited by a repeated character, as in `\h'amount'`.
fn skip_delimited_arg(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    let Some(delim) = chars.next() else {
        return;
    };
    for c in chars.by_ref() {
        if c == delim {
            break;
        }
    }
}

/// Skips a `\s` size argument: an optional sign and one or two digits, or a delimited or grouped
/// form.
fn skip_size(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.peek() {
        Some('(') => {
            chars.next();
            chars.next();
            chars.next();
        }
        Some('[') => {
            chars.next();
            read_delimited(chars, ']');
        }
        Some('\'') => {
            chars.next();
            read_delimited(chars, '\'');
        }
        _ => {
            if matches!(chars.peek(), Some('+' | '-')) {
                chars.next();
            }
            for _ in 0..2 {
                if matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
                    chars.next();
                } else {
                    break;
                }
            }
        }
    }
}

/// Applies a `\f` font name to the running font, remembering the previous font so `P` (or an empty
/// name) can return to it.
// The named roman fonts are spelled out; any unrecognized name also falls back to roman.
#[allow(clippy::match_same_arms)]
fn apply_font(name: &str, font: &mut Font, previous: &mut Font) {
    let next = match name {
        "B" => Font::Bold,
        "I" => Font::Italic,
        "BI" | "IB" => Font::BoldItalic,
        "R" | "CW" => Font::Regular,
        "P" | "" => {
            std::mem::swap(font, previous);
            return;
        }
        _ => Font::Regular,
    };
    *previous = *font;
    *font = next;
}

/// Resolves a `\[name]` escape: a `uXXXX` Unicode escape or a special-character name.
fn bracket_char(name: &str) -> Option<char> {
    if let Some(hex) = name.strip_prefix('u') {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
    }
    special_char(name)
}

/// Maps a predefined string name (`\*x`) to its expansion; unknown names expand to nothing.
fn string_escape(name: &str) -> &'static str {
    match name {
        "R" => "\u{00ae}",
        "(Tm" => "\u{2122}",
        "(lq" => "\u{201c}",
        "(rq" => "\u{201d}",
        _ => "",
    }
}

/// Maps a special-character name (`\(xx`, `\[name]`) to its character; unknown names yield `None`,
/// which the caller renders as the replacement character.
// One name per arm keeps the glyph table legible even where distinct names share a character.
#[allow(clippy::match_same_arms, clippy::too_many_lines)]
fn special_char(name: &str) -> Option<char> {
    let c = match name {
        // Dashes, hyphens, and quotation.
        "hy" => '\u{2010}',
        "en" => '\u{2013}',
        "em" => '\u{2014}',
        "lq" => '\u{201c}',
        "rq" => '\u{201d}',
        "oq" => '\u{2018}',
        "cq" => '\u{2019}',
        "aq" => '\'',
        "dq" => '"',
        "Bq" => '\u{201e}',
        "bq" => '\u{201a}',
        "Fo" => '\u{00ab}',
        "Fc" => '\u{00bb}',
        "fo" => '\u{2039}',
        "fc" => '\u{203a}',
        "ga" => '`',
        "aa" => '\u{00b4}',
        "ha" => '^',
        "ti" => '~',
        "ul" => '_',
        "ru" => '_',
        "rs" => '\\',
        "sl" => '/',
        // Bullets, marks, and shapes.
        "bu" => '\u{00b7}',
        "ci" => '\u{25cb}',
        "sq" => '\u{25a1}',
        "lz" => '\u{25ca}',
        "dg" => '\u{2020}',
        "dd" => '\u{2021}',
        "ps" => '\u{00b6}',
        "sc" => '\u{00a7}',
        "lh" => '\u{261c}',
        "rh" => '\u{261e}',
        "co" => '\u{00a9}',
        "rg" => '\u{00ae}',
        "tm" => '\u{2122}',
        "fm" => '\u{2032}',
        "sd" => '\u{2033}',
        "de" => '\u{00b0}',
        "mc" => '\u{00b5}',
        "%0" => '\u{2030}',
        // Punctuation and bars.
        "at" => '@',
        "sh" => '#',
        "or" => '|',
        "ba" => '|',
        "br" => '\u{2502}',
        "bb" => '\u{00a6}',
        "rn" => '\u{203e}',
        "ct" => '\u{00a2}',
        // Currency.
        "Do" => '$',
        "Eu" | "eu" => '\u{20ac}',
        "Po" => '\u{00a3}',
        "Ye" => '\u{00a5}',
        "Cs" => '\u{00a4}',
        // Fractions and ligatures.
        "12" => '\u{00bd}',
        "14" => '\u{00bc}',
        "34" => '\u{00be}',
        "ff" => '\u{fb00}',
        "fi" => '\u{fb01}',
        "fl" => '\u{fb02}',
        "Fi" => '\u{fb03}',
        "Fl" => '\u{fb04}',
        // Accented letters and accents.
        "oA" => '\u{00c5}',
        "oa" => '\u{00e5}',
        "/L" => '\u{0141}',
        "/l" => '\u{0142}',
        "a-" => '\u{00af}',
        "a." => '\u{02d9}',
        "ad" => '\u{00a8}',
        "ah" => '\u{02c7}',
        "a^" => '^',
        // Mathematical operators and relations.
        "pl" => '+',
        "mi" => '\u{2212}',
        "mu" => '\u{00d7}',
        "di" => '\u{00f7}',
        "+-" => '\u{00b1}',
        "**" => '\u{2217}',
        "c*" => '\u{2297}',
        "c+" => '\u{2295}',
        "<=" => '\u{2264}',
        ">=" => '\u{2265}',
        "!=" => '\u{2260}',
        "==" => '\u{2261}',
        "->" => '\u{2192}',
        "<-" => '\u{2190}',
        "eq" => '=',
        "no" => '\u{00ac}',
        "sr" => '\u{221a}',
        "is" => '\u{222b}',
        "pd" => '\u{2202}',
        "gr" => '\u{2207}',
        "fa" => '\u{2200}',
        "te" => '\u{2203}',
        "if" => '\u{221e}',
        "pt" => '\u{221d}',
        "es" => '\u{2205}',
        "ca" => '\u{2229}',
        "cu" => '\u{222a}',
        "sb" => '\u{2282}',
        "sp" => '\u{2283}',
        "ib" => '\u{2286}',
        "ip" => '\u{2287}',
        "mo" => '\u{2208}',
        "nm" => '\u{2209}',
        "pp" => '\u{22a5}',
        "3d" => '\u{2234}',
        "Ah" => '\u{2135}',
        "Im" => '\u{2111}',
        "Re" => '\u{211c}',
        "wp" => '\u{2118}',
        // Greek lowercase.
        "*a" => '\u{03b1}',
        "*b" => '\u{03b2}',
        "*g" => '\u{03b3}',
        "*d" => '\u{03b4}',
        "*e" => '\u{03b5}',
        "*z" => '\u{03b6}',
        "*y" => '\u{03b7}',
        "*h" => '\u{03b8}',
        "*i" => '\u{03b9}',
        "*k" => '\u{03ba}',
        "*l" => '\u{03bb}',
        "*m" => '\u{03bc}',
        "*n" => '\u{03bd}',
        "*c" => '\u{03be}',
        "*o" => '\u{03bf}',
        "*p" => '\u{03c0}',
        "*r" => '\u{03c1}',
        "ts" => '\u{03c2}',
        "*s" => '\u{03c3}',
        "*t" => '\u{03c4}',
        "*u" => '\u{03c5}',
        "*f" => '\u{03c6}',
        "*x" => '\u{03c7}',
        "*q" => '\u{03c8}',
        "*w" => '\u{03c9}',
        // Greek uppercase.
        "*A" => '\u{0391}',
        "*B" => '\u{0392}',
        "*G" => '\u{0393}',
        "*D" => '\u{0394}',
        "*E" => '\u{0395}',
        "*Z" => '\u{0396}',
        "*Y" => '\u{0397}',
        "*H" => '\u{0398}',
        "*I" => '\u{0399}',
        "*K" => '\u{039a}',
        "*L" => '\u{039b}',
        "*M" => '\u{039c}',
        "*N" => '\u{039d}',
        "*C" => '\u{039e}',
        "*O" => '\u{039f}',
        "*P" => '\u{03a0}',
        "*R" => '\u{03a1}',
        "*S" => '\u{03a3}',
        "*T" => '\u{03a4}',
        "*U" => '\u{03a5}',
        "*F" => '\u{03a6}',
        "*X" => '\u{03a7}',
        "*Q" => '\u{03a8}',
        "*W" => '\u{03a9}',
        _ => return None,
    };
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_core::Extension;

    fn read(input: &str) -> Document {
        read_with(input, Extensions::from_list(&[Extension::AutoIdentifiers]))
    }

    fn read_with(input: &str, extensions: Extensions) -> Document {
        let mut options = ReaderOptions::default();
        options.extensions = extensions;
        ManReader.read(input, &options).expect("read")
    }

    #[test]
    fn title_populates_metadata() {
        let doc = read(".TH FOO 1 \"2024-01-01\" \"version 1.0\" \"Foo Manual\"\n");
        assert_eq!(
            doc.meta.get("title"),
            Some(&MetaValue::MetaInlines(vec![Inline::Str("FOO".into())]))
        );
        assert_eq!(
            doc.meta.get("section"),
            Some(&MetaValue::MetaInlines(vec![Inline::Str("1".into())]))
        );
        assert_eq!(
            doc.meta.get("header"),
            Some(&MetaValue::MetaInlines(vec![
                Inline::Str("Foo".into()),
                Inline::Space,
                Inline::Str("Manual".into()),
            ]))
        );
    }

    #[test]
    fn section_headings_get_identifiers() {
        let doc = read(".TH T 1\n.SH NAME\nfoo\n.SS Sub Title\nbar\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Header(
                1,
                Attr {
                    id: "name".into(),
                    ..Attr::default()
                },
                vec![Inline::Str("NAME".into())]
            ))
        );
        assert!(matches!(
            doc.blocks.get(2),
            Some(Block::Header(2, attr, _)) if attr.id == "sub-title"
        ));
    }

    #[test]
    fn duplicate_headings_disambiguate() {
        let doc = read(".TH T 1\n.SH Foo\nx\n.SH Foo\ny\n");
        let ids: Vec<&str> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Header(_, attr, _) => Some(attr.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["foo", "foo-1"]);
    }

    #[test]
    fn auto_identifiers_off_leaves_empty_id() {
        let doc = read_with(".TH T 1\n.SH Foo Bar\nx\n", Extensions::empty());
        assert!(matches!(
            doc.blocks.first(),
            Some(Block::Header(1, attr, _)) if attr.id.is_empty()
        ));
    }

    #[test]
    fn lines_fill_into_one_paragraph() {
        let doc = read(".TH T 1\nfirst line\nsecond line\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("first".into()),
                Inline::Space,
                Inline::Str("line".into()),
                Inline::Space,
                Inline::Str("second".into()),
                Inline::Space,
                Inline::Str("line".into()),
            ]))
        );
    }

    #[test]
    fn blank_line_separates_paragraphs() {
        let doc = read(".TH T 1\none\n\ntwo\n");
        assert_eq!(doc.blocks.len(), 2);
    }

    #[test]
    fn bold_macro_joins_arguments() {
        let doc = read(".TH T 1\n.B \"two words\" tail\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Strong(vec![
                Inline::Str("two".into()),
                Inline::Space,
                Inline::Str("words".into()),
                Inline::Space,
                Inline::Str("tail".into()),
            ])]))
        );
    }

    #[test]
    fn font_macro_nests_an_inner_font_escape() {
        let doc = read(".TH T 1\n.B \\-f \\fIfile\\fR tail\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Strong(vec![
                Inline::Str("-f".into()),
                Inline::Space,
                Inline::Emph(vec![Inline::Str("file".into())]),
                Inline::Space,
                Inline::Str("tail".into()),
            ])]))
        );
    }

    #[test]
    fn alternating_font_arg_wraps_an_inner_escape() {
        let doc = read(".TH T 1\n.BR a\\fIx\\fR b\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Strong(vec![
                    Inline::Str("a".into()),
                    Inline::Emph(vec![Inline::Str("x".into())]),
                ]),
                Inline::Str("b".into()),
            ]))
        );
    }

    #[test]
    fn alternating_fonts_abut_without_space() {
        let doc = read(".TH T 1\n.BR bold roman\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Strong(vec![Inline::Str("bold".into())]),
                Inline::Str("roman".into()),
            ]))
        );
    }

    #[test]
    fn inline_font_escape_groups_run() {
        let doc = read(".TH T 1\n\\fBtwo words\\fR plain\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Strong(vec![
                    Inline::Str("two".into()),
                    Inline::Space,
                    Inline::Str("words".into()),
                ]),
                Inline::Space,
                Inline::Str("plain".into()),
            ]))
        );
    }

    #[test]
    fn boundary_space_leaves_the_font_run() {
        let doc = read(".TH T 1\n\\fBbold \\fRroman\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Strong(vec![Inline::Str("bold".into())]),
                Inline::Space,
                Inline::Str("roman".into()),
            ]))
        );
    }

    #[test]
    fn break_macro_is_a_line_break() {
        let doc = read(".TH T 1\nbefore\n.br\nafter\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("before".into()),
                Inline::LineBreak,
                Inline::Str("after".into()),
            ]))
        );
    }

    #[test]
    fn comment_is_transparent() {
        let doc = read(".TH T 1\nvisible\n.\\\" a comment\nstill\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("visible".into()),
                Inline::Space,
                Inline::Str("still".into()),
            ]))
        );
    }

    #[test]
    fn special_characters_resolve() {
        let doc = read(".TH T 1\ndash \\- bullet \\(bu em \\(em\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("dash".into()),
                Inline::Space,
                Inline::Str("-".into()),
                Inline::Space,
                Inline::Str("bullet".into()),
                Inline::Space,
                Inline::Str("\u{00b7}".into()),
                Inline::Space,
                Inline::Str("em".into()),
                Inline::Space,
                Inline::Str("\u{2014}".into()),
            ]))
        );
    }

    #[test]
    fn unknown_special_character_is_replacement() {
        let doc = read(".TH T 1\nx \\(ZZ y\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("x".into()),
                Inline::Space,
                Inline::Str("\u{fffd}".into()),
                Inline::Space,
                Inline::Str("y".into()),
            ]))
        );
    }

    #[test]
    fn unicode_escape_resolves() {
        let doc = read(".TH T 1\n\\[u00C9]\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Str("\u{00c9}".into())]))
        );
    }

    #[test]
    fn tbl_region_is_kept_as_a_verbatim_code_block() {
        let doc = read(".TH T 1\n.TS\nl l.\nName\tAge\nAda\t36\n.TE\nafter\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::CodeBlock(
                Attr::default(),
                "l l.\nName Age\nAda 36".into()
            ))
        );
        assert_eq!(
            doc.blocks.get(1),
            Some(&Block::Para(vec![Inline::Str("after".into())]))
        );
    }

    #[test]
    fn unterminated_tbl_region_does_not_panic() {
        let doc = read(".TS");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::CodeBlock(Attr::default(), String::new()))
        );
    }

    #[test]
    fn tagged_paragraphs_become_a_definition_list() {
        let doc = read(".TH T 1\n.TP\n.B \\-v\nVerbose mode.\n.TP\n.B \\-f\nUse a file.\n");
        let Some(Block::DefinitionList(items)) = doc.blocks.first() else {
            panic!("expected a definition list");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(
            items.first().map(|(term, _)| term.clone()),
            Some(vec![Inline::Strong(vec![Inline::Str("-v".into())])])
        );
    }

    #[test]
    fn bullet_indented_paragraphs_become_a_bullet_list() {
        let doc = read(".TH T 1\n.IP \\(bu 2\none\n.IP \\(bu 2\ntwo\n");
        let Some(Block::BulletList(items)) = doc.blocks.first() else {
            panic!("expected a bullet list");
        };
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn numbered_indented_paragraphs_become_an_ordered_list() {
        let doc = read(".TH T 1\n.IP 3. 4\nthree\n.IP 4. 4\nfour\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::OrderedList(
                ListAttributes {
                    start: 3,
                    style: ListNumberStyle::Decimal,
                    delim: ListNumberDelim::Period,
                },
                vec![
                    vec![Block::Para(vec![Inline::Str("three".into())])],
                    vec![Block::Para(vec![Inline::Str("four".into())])],
                ]
            ))
        );
    }

    #[test]
    fn roman_marker_is_lower_roman() {
        assert!(matches!(
            parse_enumerator("iv."),
            Some(ListAttributes {
                start: 4,
                style: ListNumberStyle::LowerRoman,
                delim: ListNumberDelim::Period,
            })
        ));
    }

    #[test]
    fn bare_letter_marker_uses_its_position() {
        assert!(matches!(
            parse_enumerator("o"),
            Some(ListAttributes {
                start: 15,
                style: ListNumberStyle::LowerAlpha,
                delim: ListNumberDelim::DefaultDelim,
            })
        ));
    }

    #[test]
    fn unmarked_indented_paragraph_is_an_inset() {
        let doc = read(".TH T 1\n.IP\nplain indented\n");
        assert!(matches!(doc.blocks.first(), Some(Block::BlockQuote(_))));
    }

    #[test]
    fn relative_inset_becomes_a_block_quote() {
        let doc = read(".TH T 1\n.RS\ninside\n.RE\nafter\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::BlockQuote(vec![Block::Para(vec![Inline::Str(
                "inside".into()
            )])]))
        );
        assert_eq!(
            doc.blocks.get(1),
            Some(&Block::Para(vec![Inline::Str("after".into())]))
        );
    }

    #[test]
    fn nested_insets_nest_block_quotes() {
        let doc = read(".TH T 1\n.RS\nouter\n.RS\ninner\n.RE\n.RE\n");
        assert!(matches!(
            doc.blocks.first(),
            Some(Block::BlockQuote(inner)) if inner.iter().any(|b| matches!(b, Block::BlockQuote(_)))
        ));
    }

    #[test]
    fn no_fill_region_becomes_a_code_block() {
        let doc = read(".TH T 1\n.nf\nline one\n  indented\n.fi\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::CodeBlock(
                Attr::default(),
                "line one\n  indented".into()
            ))
        );
    }

    #[test]
    fn example_region_becomes_a_code_block() {
        let doc = read(".TH T 1\n.EX\n\\fBcode\\fR \\- here\n.EE\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::CodeBlock(Attr::default(), "code - here".into()))
        );
    }

    #[test]
    fn uri_macro_becomes_a_link() {
        let doc = read(".TH T 1\n.UR https://example.com\nthe text\n.UE\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Link(
                Attr::default(),
                vec![
                    Inline::Str("the".into()),
                    Inline::Space,
                    Inline::Str("text".into()),
                ],
                Target {
                    url: "https://example.com".into(),
                    title: String::new(),
                },
            )]))
        );
    }

    #[test]
    fn mail_macro_uses_mailto() {
        let doc = read(".TH T 1\n.MT user@example.com\nwrite me\n.ME\n");
        let Some(Block::Para(inlines)) = doc.blocks.first() else {
            panic!("expected a paragraph");
        };
        assert!(matches!(
            inlines.first(),
            Some(Inline::Link(_, _, target)) if target.url == "mailto:user@example.com"
        ));
    }

    #[test]
    fn link_trailing_text_attaches_without_space() {
        let doc = read(".TH T 1\nsee\n.UR https://x.org\nhere\n.UE .\nnext\n");
        let Some(Block::Para(inlines)) = doc.blocks.first() else {
            panic!("expected a paragraph");
        };
        // … the link, then the trailing "." with no separating space.
        let link_index = inlines
            .iter()
            .position(|i| matches!(i, Inline::Link(..)))
            .expect("link present");
        assert_eq!(inlines.get(link_index + 1), Some(&Inline::Str(".".into())));
    }

    #[test]
    fn unknown_macro_breaks_the_paragraph() {
        let doc = read(".TH T 1\nbefore\n.XYZ args\nafter\n");
        assert_eq!(doc.blocks.len(), 2);
    }
}
