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

use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Inline, ListAttributes,
    ListNumberDelim, ListNumberStyle, MetaValue, Row, Table, TableBody, TableFoot, TableHead,
    Target, slug, slug_gfm, to_plain_text,
};
use carta_core::{Extensions, Reader, ReaderOptions, Result};

use crate::heading_ids::{IdRegistry, IdScheme};
use crate::inline_text::{trim_inline_ends, words_to_inlines};
use crate::roman::roman_value_loose_forward;
use crate::transliterate::fold_to_ascii;

/// A table of named strings: the predefined groff strings plus any defined with `.ds`, looked up by
/// the `\*` interpolation escape.
type Strings = BTreeMap<String, String>;

/// The deepest a `\*` interpolation may recurse, bounding self-referential string definitions.
const MAX_STRING_DEPTH: usize = 8;

/// The most lines all macro expansion may produce across a document. The budget is cumulative
/// rather than per-invocation because argument substitution can synthesize a fresh call line
/// (`\$` followed by a non-digit leaves the rest of the line intact, so `\$.X` expands to the call
/// `.X`), and each such invocation restarts with an empty recursion guard — only a shared budget
/// makes that cycle terminate.
const MAX_MACRO_EXPANSION_LINES: usize = 100_000;

/// The most bytes all macro expansion may produce across a document, counting nested-call argument
/// substitution as well as emitted lines. Bounds calls that double an argument's length on each
/// synthesized re-invocation, which the line budget alone would let grow geometrically.
const MAX_MACRO_EXPANSION_BYTES: usize = 1 << 22;

/// The unspent portion of the document-wide macro-expansion budget.
#[derive(Clone, Copy)]
struct ExpansionBudget {
    lines: usize,
    bytes: usize,
}

impl ExpansionBudget {
    fn exhausted(&self) -> bool {
        self.lines == 0 || self.bytes == 0
    }

    /// Debits one produced line (or nested-call substitution) of `bytes` length; the caller checks
    /// `exhausted` before producing.
    fn debit(&mut self, bytes: usize) {
        self.lines = self.lines.saturating_sub(1);
        self.bytes = self.bytes.saturating_sub(bytes.max(1));
    }
}

/// The named strings groff defines before any input is read, keyed as the `\*` escape spells them:
/// `\*R`, `\*(Tm`, `\*(lq`, `\*(rq`.
fn predefined_strings() -> Strings {
    [
        ("R", "\u{00ae}"),
        ("Tm", "\u{2122}"),
        ("lq", "\u{201c}"),
        ("rq", "\u{201d}"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect()
}

/// Parses a manual page written in the `man` macro language into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct ManReader;

impl Reader for ManReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let lines = logical_lines(input);
        let mut parser = Parser::new(lines, options.extensions);
        let blocks = parser.parse_blocks(Ctx::TOP);
        Ok(Document {
            meta: parser
                .meta
                .into_iter()
                .map(|(k, v)| (k.into(), v))
                .collect(),
            blocks,
            ..Document::default()
        })
    }
}

/// Splits the input into logical lines, joining input-continuation lines. A line ending in an odd
/// number of backslashes continues onto the next: the trailing backslash is removed and the following
/// line is appended directly, with no separating space. An even count leaves the line intact.
fn logical_lines(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut acc = String::new();
    let mut continuing = false;
    for raw in input.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if !continuing {
            acc.clear();
        }
        acc.push_str(raw);
        let trailing = acc.chars().rev().take_while(|&c| c == '\\').count();
        if trailing % 2 == 1 {
            acc.pop();
            continuing = true;
        } else {
            out.push(std::mem::take(&mut acc));
            continuing = false;
        }
    }
    if continuing {
        out.push(acc);
    }
    out
}

/// The active typeface for a run of text. `\f(BI` and the `.BI`/`.IB` macros render bold-italic as
/// emphasis wrapping strong. The constant-width faces (`\f(CW`, `\fC`, `.CW`) render as inline code,
/// with a bold or italic constant-width face wrapping that code in the corresponding markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Font {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono,
    MonoBold,
    MonoItalic,
}

impl Font {
    /// Wraps already-built inline content in the markup for this font; roman content is unwrapped.
    /// A constant-width face collapses its content to a single inline-code span.
    fn wrap(self, inlines: Vec<Inline>) -> Vec<Inline> {
        if inlines.is_empty() {
            return Vec::new();
        }
        self.wrap_forced(inlines)
    }

    /// Wraps the inlines in this font's markup unconditionally — even when they are empty. A
    /// single-font macro called with an explicit argument keeps its styled wrapper around empty
    /// content, whereas a font run that produces nothing collapses (see [`wrap`]).
    fn wrap_forced(self, inlines: Vec<Inline>) -> Vec<Inline> {
        match self {
            Font::Regular => inlines,
            Font::Bold => vec![Inline::Strong(inlines)],
            Font::Italic => vec![Inline::Emph(inlines)],
            Font::BoldItalic => vec![Inline::Emph(vec![Inline::Strong(inlines)])],
            Font::Mono => vec![code_inline(&inlines)],
            Font::MonoBold => vec![Inline::Strong(vec![code_inline(&inlines)])],
            Font::MonoItalic => vec![Inline::Emph(vec![code_inline(&inlines)])],
        }
    }
}

/// Collapses a run of inline content into a single inline-code span, recovering its literal text.
fn code_inline(inlines: &[Inline]) -> Inline {
    let mut text = String::new();
    collect_code_text(inlines, &mut text);
    Inline::Code(Box::default(), text.into())
}

fn collect_code_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Str(s) => out.push_str(s),
            Inline::Space => out.push(' '),
            Inline::Strong(xs) | Inline::Emph(xs) => collect_code_text(xs, out),
            _ => {}
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
    ascii: bool,
    registry: IdRegistry,
}

impl HeadingIds {
    fn new(extensions: Extensions) -> Self {
        Self {
            scheme: IdScheme::select(extensions, false),
            ascii: extensions.contains(carta_core::Extension::AsciiIdentifiers),
            registry: IdRegistry::default(),
        }
    }

    fn assign(&mut self, inlines: &[Inline]) -> String {
        let Some(scheme) = self.scheme else {
            return String::new();
        };
        let text = to_plain_text(inlines);
        // The slug shape follows the active extension, but a manual page always disambiguates
        // natively: an empty slug becomes `section` and repeats increment until unused.
        let base = match scheme {
            IdScheme::Plain => slug(&text),
            IdScheme::Gfm => slug_gfm(&text),
        };
        // ASCII folding transliterates the finished slug, so a separator left by a word whose
        // letters all lack an ASCII base is preserved. The plain shape then re-drops its leading
        // run up to the first letter, which folding away a leading word can expose; the gfm shape
        // never strips a leading run.
        let base = if self.ascii {
            let folded = fold_to_ascii(&base);
            match scheme {
                IdScheme::Plain => folded
                    .chars()
                    .skip_while(|c| !c.is_ascii_alphabetic())
                    .collect(),
                IdScheme::Gfm => folded,
            }
        } else {
            base
        };
        self.registry.assign_native(base)
    }
}

struct Parser {
    lines: Vec<String>,
    pos: usize,
    /// Lines from an in-progress macro expansion, not yet consumed. The logical current line is
    /// this queue's front when non-empty, else `lines[pos]` — expanding a macro call pushes its
    /// body here instead of splicing it into `lines`, so expansion cost is independent of how much
    /// of the document remains unparsed.
    pending: std::collections::VecDeque<String>,
    meta: BTreeMap<String, MetaValue>,
    headings: HeadingIds,
    /// Named strings interpolated by `\*`: the predefined groff set, extended by `.ds`.
    strings: Strings,
    /// User-defined macros (`.de`/`.de1`), keyed by name; the value is the macro body's lines.
    macros: BTreeMap<String, Vec<String>>,
    /// Set when the most recent `.ie` condition was false, so the following `.el` takes its branch.
    else_branch: bool,
    /// What remains of the document-wide macro-expansion budget; once spent, macro calls expand to
    /// nothing.
    expansion_budget: ExpansionBudget,
}

impl Parser {
    fn new(lines: Vec<String>, extensions: Extensions) -> Self {
        Self {
            lines,
            pos: 0,
            pending: std::collections::VecDeque::new(),
            meta: BTreeMap::new(),
            headings: HeadingIds::new(extensions),
            strings: predefined_strings(),
            macros: BTreeMap::new(),
            else_branch: false,
            expansion_budget: ExpansionBudget {
                lines: MAX_MACRO_EXPANSION_LINES,
                bytes: MAX_MACRO_EXPANSION_BYTES,
            },
        }
    }

    fn peek(&self) -> Option<&str> {
        self.pending
            .front()
            .map(String::as_str)
            .or_else(|| self.lines.get(self.pos).map(String::as_str))
    }

    fn advance(&mut self) {
        if self.pending.pop_front().is_none() {
            self.pos += 1;
        }
    }

    /// The control-line request name of the line at `pos`, if it is a non-comment control line.
    fn peek_request(&self) -> Option<&str> {
        let line = self.peek()?;
        if is_comment(line) {
            return None;
        }
        control_parts(line).map(|(name, _)| name)
    }

    /// Consumes and returns the next line, if any.
    fn take_line(&mut self) -> Option<String> {
        if let Some(line) = self.pending.pop_front() {
            return Some(line);
        }
        let line = self.lines.get(self.pos).cloned();
        if line.is_some() {
            self.pos += 1;
        }
        line
    }

    /// Replaces the current line with the taken branch of a conditional so the main loop reprocesses
    /// it as a fresh logical line (text or control line). An empty branch is skipped outright.
    fn reprocess_as(&mut self, content: &str) {
        let content = content.trim_start_matches([' ', '\t']);
        if content.is_empty() {
            self.advance();
        } else if let Some(slot) = self.pending.front_mut() {
            content.clone_into(slot);
        } else if let Some(slot) = self.lines.get_mut(self.pos) {
            content.clone_into(slot);
        } else {
            self.advance();
        }
    }

    /// Consumes the body of a `.de`/`.de1` macro definition up to (but not including) the line whose
    /// request name is `end` (the default end is `..`, whose request name is a single `.`), or to end
    /// of input, and returns the collected body lines. The terminator line is consumed.
    fn collect_macro_definition(&mut self, end: &str) -> Vec<String> {
        let mut body = Vec::new();
        while let Some(line) = self.peek().map(str::to_owned) {
            self.advance();
            let is_end =
                !is_comment(&line) && control_parts(&line).is_some_and(|(name, _)| name == end);
            if is_end {
                break;
            }
            body.push(reduce_copy_mode(&line));
        }
        body
    }

    /// Expands a macro invocation into a flat list of lines, substituting the call's arguments for
    /// `\$N` references and inlining any nested macro calls. Re-entrant calls and the document-wide
    /// expansion budget bound the expansion so a self- or mutually-referential macro cannot loop
    /// forever.
    fn expand_macro_call(&mut self, name: &str, args: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        let mut active = BTreeSet::new();
        let mut budget = self.expansion_budget;
        self.expand_macro_into(name, args, &mut active, &mut out, &mut budget);
        self.expansion_budget = budget;
        out
    }

    fn expand_macro_into(
        &self,
        name: &str,
        args: &[String],
        active: &mut BTreeSet<String>,
        out: &mut Vec<String>,
        budget: &mut ExpansionBudget,
    ) {
        if budget.exhausted() || active.contains(name) {
            return;
        }
        let Some(body) = self.macros.get(name) else {
            return;
        };
        active.insert(name.to_owned());
        for raw in body {
            if budget.exhausted() {
                break;
            }
            match control_parts(raw) {
                Some((inner, inner_rest))
                    if !is_comment(raw) && self.macros.contains_key(inner) =>
                {
                    // A nested call to a user macro receives the substituted arguments. The
                    // substituted argument text debits the budget even though it is not emitted,
                    // so argument growth across nested calls stays bounded.
                    let substituted = substitute_macro_args(inner_rest, args);
                    budget.debit(substituted.len());
                    let inner_args = split_args(&substituted);
                    self.expand_macro_into(inner, &inner_args, active, out, budget);
                }
                // A request line is emitted verbatim; argument references in a request's own
                // arguments are left for ordinary escape processing, which yields nothing.
                Some(_) => {
                    budget.debit(raw.len());
                    out.push(raw.clone());
                }
                // A text line has its argument references substituted.
                None => {
                    let substituted = substitute_macro_args(raw, args);
                    budget.debit(substituted.len());
                    out.push(substituted);
                }
            }
        }
        active.remove(name);
    }

    /// Parses a sequence of blocks until the context's terminator (or end of input). A terminator
    /// line is left unconsumed for the caller, except a `.RE` that closes the inset it belongs to.
    // The macro dispatch lists names separately for clarity even where their handling coincides.
    #[allow(clippy::too_many_lines, clippy::match_same_arms)]
    fn parse_blocks(&mut self, ctx: Ctx) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut fill = Vec::new();
        // Whether a text line has opened the current paragraph: a paragraph made only of
        // whitespace-filled lines is still emitted (as `Para []`), unlike a macro-driven flush.
        let mut started = false;
        while let Some(line) = self.peek().map(str::to_owned) {
            if line.is_empty() {
                flush_para(&mut fill, &mut blocks, &mut started);
                self.advance();
                continue;
            }
            let Some((name, rest)) = control_parts(&line) else {
                self.advance();
                append_text(&mut fill, tokenize(&line, Font::Regular, &self.strings));
                started = true;
                continue;
            };
            if is_comment(&line) {
                self.advance();
                continue;
            }
            match name {
                "SH" | "SS" => {
                    if ctx.in_inset || ctx.in_item {
                        flush_para(&mut fill, &mut blocks, &mut started);
                        return blocks;
                    }
                    flush_para(&mut fill, &mut blocks, &mut started);
                    self.advance();
                    let level = if name == "SH" { 1 } else { 2 };
                    let inlines = self.heading_inlines(rest);
                    let id = self.headings.assign(&inlines);
                    blocks.push(Block::Header(
                        level,
                        Box::new(Attr {
                            id: id.into(),
                            ..Attr::default()
                        }),
                        inlines,
                    ));
                }
                "PP" | "LP" | "P" | "HP" => {
                    flush_para(&mut fill, &mut blocks, &mut started);
                    if ctx.in_item {
                        return blocks;
                    }
                    self.advance();
                }
                "TP" | "IP" => {
                    flush_para(&mut fill, &mut blocks, &mut started);
                    if ctx.in_item {
                        return blocks;
                    }
                    let list = self.parse_list();
                    blocks.extend(list);
                }
                "TQ" => {
                    flush_para(&mut fill, &mut blocks, &mut started);
                    if ctx.in_item {
                        return blocks;
                    }
                    self.advance();
                }
                "RS" => {
                    flush_para(&mut fill, &mut blocks, &mut started);
                    self.advance();
                    let inner = self.parse_blocks(Ctx::INSET);
                    if ctx.in_item {
                        blocks.extend(inner);
                    } else {
                        blocks.push(Block::BlockQuote(inner));
                    }
                }
                "RE" => {
                    flush_para(&mut fill, &mut blocks, &mut started);
                    self.advance();
                    if ctx.in_inset {
                        return blocks;
                    }
                }
                "nf" | "EX" => {
                    flush_para(&mut fill, &mut blocks, &mut started);
                    self.advance();
                    blocks.push(self.parse_verbatim());
                }
                "fi" | "EE" | "UE" | "ME" => {
                    flush_para(&mut fill, &mut blocks, &mut started);
                    self.advance();
                }
                "TS" => {
                    flush_para(&mut fill, &mut blocks, &mut started);
                    self.advance();
                    blocks.extend(self.parse_tbl());
                }
                "ds" => {
                    self.advance();
                    self.define_string(rest);
                }
                "br" => {
                    self.advance();
                    fill.push(Inline::LineBreak);
                }
                "sp" => {
                    flush_para(&mut fill, &mut blocks, &mut started);
                    self.advance();
                }
                "TH" => {
                    self.advance();
                    self.parse_title(rest);
                }
                "B" | "I" => {
                    self.advance();
                    let font = single_font(name);
                    let inlines = if rest.is_empty() {
                        let text = self.take_line().unwrap_or_default();
                        font.wrap(tokenize(&text, Font::Regular, &self.strings))
                    } else {
                        let text = split_args(rest).join(" ");
                        font.wrap_forced(tokenize(&text, Font::Regular, &self.strings))
                    };
                    append_text(&mut fill, inlines);
                    started = true;
                }
                "BR" | "RB" | "BI" | "IB" | "RI" | "IR" => {
                    self.advance();
                    let rest = if rest.is_empty() {
                        self.take_line().unwrap_or_default()
                    } else {
                        rest.to_owned()
                    };
                    append_text(
                        &mut fill,
                        alternating(&rest, fonts_for(name), &self.strings),
                    );
                    started = true;
                }
                "SY" => {
                    self.advance();
                    let text = if rest.is_empty() {
                        self.take_line().unwrap_or_default()
                    } else {
                        split_args(rest).join(" ")
                    };
                    append_text(&mut fill, font_macro(Font::Bold, &text, &self.strings));
                    started = true;
                }
                "OP" => {
                    self.advance();
                    append_text(&mut fill, option_synopsis(rest, &self.strings));
                    started = true;
                }
                "YS" => {
                    self.advance();
                }
                "UR" | "MT" => {
                    self.advance();
                    let url = split_args(rest).into_iter().next().unwrap_or_default();
                    let url = if name == "MT" {
                        format!("mailto:{url}")
                    } else {
                        url
                    };
                    if self.link_label_is_plain() {
                        self.parse_link(url, &mut fill);
                        started = true;
                    } else {
                        // A font macro (or any request) inside the label aborts the link: the open
                        // paragraph is flushed and the label content is emitted as its own blocks.
                        flush_para(&mut fill, &mut blocks, &mut started);
                        blocks.extend(self.parse_aborted_link());
                    }
                }
                "de" | "de1" => {
                    self.advance();
                    let args = split_args(rest);
                    let end = args.get(1).map_or(".", String::as_str).to_owned();
                    let body = self.collect_macro_definition(&end);
                    if let Some(name) = args.into_iter().next() {
                        self.macros.insert(name, body);
                    }
                }
                "if" => {
                    let (cond, branch) = split_condition(rest);
                    if condition_true(cond) {
                        self.reprocess_as(branch);
                    } else {
                        self.advance();
                    }
                }
                "ie" => {
                    let (cond, branch) = split_condition(rest);
                    let taken = condition_true(cond);
                    self.else_branch = !taken;
                    if taken {
                        self.reprocess_as(branch);
                    } else {
                        self.advance();
                    }
                }
                "el" => {
                    if self.else_branch {
                        self.else_branch = false;
                        self.reprocess_as(rest);
                    } else {
                        self.advance();
                    }
                }
                // A call to a user-defined macro queues its expanded body ahead of the current
                // position so the queued lines are parsed in place, before the base document
                // resumes.
                _ if self.macros.contains_key(name) => {
                    self.advance();
                    let args = split_args(rest);
                    let expansion = self.expand_macro_call(name, &args);
                    for line in expansion.into_iter().rev() {
                        self.pending.push_front(line);
                    }
                }
                // An empty request (a bare control character) or one named only with control
                // characters (`.`, `..`, `'`) is a no-op that leaves the open paragraph filling.
                _ if is_noop_request(name) => {
                    self.advance();
                }
                _ => {
                    flush_para(&mut fill, &mut blocks, &mut started);
                    self.advance();
                }
            }
        }
        flush_para(&mut fill, &mut blocks, &mut started);
        blocks
    }

    /// Heading inline content: the macro's arguments joined by spaces, or — when the macro carries
    /// none — the following input line.
    fn heading_inlines(&mut self, rest: &str) -> Vec<Inline> {
        if rest.is_empty() {
            let next = self.take_line().unwrap_or_default();
            tokenize(&next, Font::Regular, &self.strings)
        } else {
            tokenize(&split_args(rest).join(" "), Font::Regular, &self.strings)
        }
    }

    /// Reads `.TH` arguments into metadata: identifier, section, date, footer, header.
    fn parse_title(&mut self, rest: &str) {
        let keys = ["title", "section", "date", "footer", "header"];
        for (key, arg) in keys.iter().zip(split_args(rest)) {
            if arg.is_empty() {
                continue;
            }
            let inlines = tokenize(&arg, Font::Regular, &self.strings);
            self.meta
                .insert((*key).to_owned(), MetaValue::MetaInlines(inlines));
        }
    }

    /// Records a `.ds` string definition. The name is the first argument; the value is the remainder
    /// of the line after the single separating space, truncated at an inline comment (`\"`) and with
    /// trailing whitespace removed. The value keeps its own escapes, expanded when it is interpolated.
    fn define_string(&mut self, rest: &str) {
        let (name, value) = match rest.split_once([' ', '\t']) {
            Some((name, value)) => (name, value),
            None => (rest, ""),
        };
        if name.is_empty() {
            return;
        }
        let value = match value.find("\\\"") {
            Some(index) => value.get(..index).unwrap_or(value),
            None => value,
        };
        let value = value.trim_end_matches([' ', '\t']);
        self.strings.insert(name.to_owned(), value.to_owned());
    }

    /// Collects a verbatim region (`.nf`/`.EX`) as a code block. Lines keep their literal spacing;
    /// escapes and font macros are reduced to plain text. The region ends at `.fi`/`.EE`, or at a
    /// section heading or end of input (both left unconsumed).
    fn parse_verbatim(&mut self) -> Block {
        let mut text_lines: Vec<String> = Vec::new();
        while let Some(line) = self.peek().map(str::to_owned) {
            if let Some((name, rest)) = control_parts(&line) {
                if is_comment(&line) {
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
                        text_lines.push(flatten(&split_args(rest).join(" "), &self.strings));
                    }
                    _ => self.advance(),
                }
            } else {
                self.advance();
                text_lines.push(flatten(&line, &self.strings));
            }
        }
        Block::CodeBlock(Box::default(), text_lines.join("\n").into())
    }

    /// Parses a tbl table region (`.TS`/`.TE`) into a [`Block::Table`]. The region's structure is the
    /// preprocessor's: an optional options line ending in `;` (from which the cell separator is read),
    /// one or more format lines ending in `.` (the first fixes the column count and alignments), then
    /// the data rows. A malformed region (no format line) yields no block. The region ends at `.TE`,
    /// or at a section heading or end of input (both left unconsumed).
    fn parse_tbl(&mut self) -> Vec<Block> {
        let mut region: Vec<String> = Vec::new();
        while let Some(line) = self.peek().map(str::to_owned) {
            if let Some((name, _)) = control_parts(&line) {
                if is_comment(&line) {
                    self.advance();
                    continue;
                }
                match name {
                    "TE" => {
                        self.advance();
                        break;
                    }
                    "SH" | "SS" => break,
                    _ => {
                        self.advance();
                        region.push(line);
                    }
                }
            } else {
                self.advance();
                region.push(line);
            }
        }
        build_tbl(&region).into_iter().collect()
    }

    /// Parses a run of consecutive `.TP`/`.IP` items into list blocks. Items of the same kind merge
    /// into one list; an unmarked `.IP` becomes a standalone inset.
    fn parse_list(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        let mut pending: Option<Pending> = None;
        while let Some(line) = self.peek().map(str::to_owned) {
            let Some((name, rest)) = control_parts(&line) else {
                break;
            };
            if is_comment(&line) {
                self.advance();
                continue;
            }
            match name {
                "TP" => {
                    self.advance();
                    let mut term = self.read_term();
                    // A `.TQ` adds a further tagged term to the same item, on its own line.
                    while self.peek_request() == Some("TQ") {
                        self.advance();
                        term.push(Inline::LineBreak);
                        term.extend(self.read_term());
                    }
                    let body = self.parse_blocks(Ctx::ITEM);
                    if body.is_empty() {
                        // A tag with no body of its own takes the rest of the list as its body,
                        // nesting it; with nothing left to take, the tag stands as a paragraph.
                        let rest = self.parse_list();
                        if rest.is_empty() {
                            flush_pending(&mut pending, &mut out);
                            out.push(Block::Para(term));
                        } else {
                            push_definition(&mut pending, &mut out, term, rest);
                        }
                    } else {
                        push_definition(&mut pending, &mut out, term, body);
                    }
                }
                "IP" => {
                    self.advance();
                    let args = split_args(rest);
                    match args.first() {
                        // No designator at all: an unmarked inset.
                        None => {
                            flush_pending(&mut pending, &mut out);
                            let body = self.parse_blocks(Ctx::ITEM);
                            // An unmarked inset with no body contributes nothing.
                            if !body.is_empty() {
                                out.push(Block::BlockQuote(body));
                            }
                        }
                        Some(mark_raw) => {
                            let mark = flatten(mark_raw, &self.strings);
                            match classify_mark(&mark) {
                                Mark::Bullet => {
                                    let body = self.item_body();
                                    push_bullet(&mut pending, &mut out, body);
                                }
                                Mark::Ordered(attrs) => {
                                    let body = self.item_body();
                                    push_ordered(&mut pending, &mut out, attrs, body);
                                }
                                // A present designator that is neither a bullet nor an enumerator —
                                // including one that reduces to nothing — is a definition term.
                                Mark::None | Mark::Text => {
                                    let term = words_to_inlines(&mark);
                                    let body = self.item_body();
                                    push_definition(&mut pending, &mut out, term, body);
                                }
                            }
                        }
                    }
                }
                _ => break,
            }
        }
        flush_pending(&mut pending, &mut out);
        out
    }

    /// A marked list item's body, where an empty body is represented as a single empty paragraph so
    /// the item is still rendered.
    fn item_body(&mut self) -> Vec<Block> {
        let body = self.parse_blocks(Ctx::ITEM);
        if body.is_empty() {
            vec![Block::Para(Vec::new())]
        } else {
            body
        }
    }

    /// The term of a `.TP` item: the next line, which is either a font macro or plain text.
    fn read_term(&mut self) -> Vec<Inline> {
        let Some(line) = self.take_line() else {
            return Vec::new();
        };
        if let Some((name, rest)) = control_parts(&line) {
            if is_comment(&line) {
                return self.read_term();
            }
            match name {
                "B" | "I" => {
                    let font = single_font(name);
                    return font_macro(font, &split_args(rest).join(" "), &self.strings);
                }
                "BR" | "RB" | "BI" | "IB" | "RI" | "IR" => {
                    return alternating(rest, fonts_for(name), &self.strings);
                }
                _ => return tokenize(rest, Font::Regular, &self.strings),
            }
        }
        tokenize(&line, Font::Regular, &self.strings)
    }

    /// Whether the label that opens at the current position is plain — only text lines (and comments)
    /// up to a `.UE`/`.ME` terminator. A request inside the label, or end of input before the
    /// terminator, makes the label non-plain, so the link is abandoned.
    fn link_label_is_plain(&self) -> bool {
        let lookahead = self
            .pending
            .iter()
            .chain(self.lines.get(self.pos..).into_iter().flatten());
        for line in lookahead {
            if is_comment(line) {
                continue;
            }
            if let Some((name, _)) = control_parts(line) {
                return matches!(name, "UE" | "ME");
            }
        }
        false
    }

    /// Collects a plain hyperlink's label between `.UR`/`.MT` and its `.UE`/`.ME` terminator,
    /// appending the resulting link to the open paragraph. The label's text lines are concatenated
    /// without separators; text after the terminator attaches to the link without a space.
    fn parse_link(&mut self, url: String, fill: &mut Vec<Inline>) {
        let mut label_text = String::new();
        let mut trailing = String::new();
        while let Some(line) = self.peek().map(str::to_owned) {
            if is_comment(&line) {
                self.advance();
                continue;
            }
            self.advance();
            if let Some((name, rest)) = control_parts(&line) {
                if matches!(name, "UE" | "ME") {
                    trailing = split_args(rest).join(" ");
                }
                break;
            }
            label_text.push_str(&line);
        }
        let label = tokenize(&label_text, Font::Regular, &self.strings);
        append_text(
            fill,
            vec![Inline::Link(
                Box::default(),
                label,
                Box::new(Target {
                    url: url.into(),
                    title: carta_ast::Text::default(),
                }),
            )],
        );
        if !trailing.is_empty() {
            fill.extend(tokenize(&trailing, Font::Regular, &self.strings));
        }
    }

    /// Parses the body of an abandoned link as a single paragraph: text lines fill normally, font
    /// macros and `.br` apply, and the `.UE`/`.ME` terminator is consumed (its trailing text dropped).
    /// Any other request ends the body, left unconsumed.
    fn parse_aborted_link(&mut self) -> Vec<Block> {
        let mut fill = Vec::new();
        while let Some(line) = self.peek().map(str::to_owned) {
            let Some((name, rest)) = control_parts(&line) else {
                self.advance();
                append_text(&mut fill, tokenize(&line, Font::Regular, &self.strings));
                continue;
            };
            if is_comment(&line) {
                self.advance();
                continue;
            }
            match name {
                "UE" | "ME" => {
                    self.advance();
                    break;
                }
                "br" => {
                    self.advance();
                    fill.push(Inline::LineBreak);
                }
                "B" | "I" => {
                    self.advance();
                    let font = single_font(name);
                    let inlines = if rest.is_empty() {
                        let text = self.take_line().unwrap_or_default();
                        font.wrap(tokenize(&text, Font::Regular, &self.strings))
                    } else {
                        let text = split_args(rest).join(" ");
                        font.wrap_forced(tokenize(&text, Font::Regular, &self.strings))
                    };
                    append_text(&mut fill, inlines);
                }
                "BR" | "RB" | "BI" | "IB" | "RI" | "IR" => {
                    self.advance();
                    let rest = if rest.is_empty() {
                        self.take_line().unwrap_or_default()
                    } else {
                        rest.to_owned()
                    };
                    append_text(
                        &mut fill,
                        alternating(&rest, fonts_for(name), &self.strings),
                    );
                }
                _ => break,
            }
        }
        trim_inline_ends(&mut fill);
        if fill.is_empty() {
            Vec::new()
        } else {
            vec![Block::Para(fill)]
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
fn font_macro(font: Font, text: &str, strings: &Strings) -> Vec<Inline> {
    font.wrap(tokenize(text, Font::Regular, strings))
}

/// Renders an alternating font macro: each argument takes the next font in the cycle, is read as
/// roman text, and is wrapped in that font; the rendered arguments abut with no separating space.
fn alternating(rest: &str, fonts: [Font; 2], strings: &Strings) -> Vec<Inline> {
    let mut out = Vec::new();
    for (index, arg) in split_args(rest).into_iter().enumerate() {
        let font = fonts.get(index % 2).copied().unwrap_or(Font::Regular);
        out.extend(font.wrap(tokenize(&arg, Font::Regular, strings)));
    }
    out
}

/// Renders a `.OP` command-option synopsis: the option name (the first argument) is set bold and an
/// optional argument (the rest) roman, the whole bracketed as optional — `[ -name argument ]`.
fn option_synopsis(rest: &str, strings: &Strings) -> Vec<Inline> {
    let args = split_args(rest);
    let mut out = vec![Inline::Str("[".into())];
    if let Some(name) = args.first() {
        out.push(Inline::Space);
        out.extend(font_macro(Font::Bold, name, strings));
    }
    let argument = args.get(1..).unwrap_or(&[]).join(" ");
    if !argument.is_empty() {
        out.push(Inline::Space);
        out.extend(tokenize(&argument, Font::Regular, strings));
    }
    out.push(Inline::Space);
    out.push(Inline::Str("]".into()));
    out
}

/// What kind of list a `.IP` marker introduces.
enum Mark {
    None,
    Bullet,
    Ordered(ListAttributes),
    Text,
}

/// Classifies a `.IP` marker, already reduced to plain text: a bullet glyph, an enumerator (decimal,
/// alphabetic, or roman), or arbitrary text that becomes a definition term.
fn classify_mark(mark: &str) -> Mark {
    if mark.is_empty() {
        return Mark::None;
    }
    if matches!(mark, "*" | "\u{2022}" | "\u{00b7}" | "-" | "+") {
        return Mark::Bullet;
    }
    if let Some(attrs) = parse_enumerator(mark) {
        return Mark::Ordered(attrs);
    }
    Mark::Text
}

/// Parses an ordered-list enumerator (`1.`, `a)`, `(iv)`, a bare letter, …) into its list
/// attributes, or returns `None` when the marker is not an enumerator.
fn parse_enumerator(mark: &str) -> Option<ListAttributes> {
    if let Some(inner) = mark.strip_prefix('(').and_then(|m| m.strip_suffix(')')) {
        return enumerator_body(inner, ListNumberDelim::TwoParens);
    }
    let (body, delim) = match mark.strip_suffix('.') {
        Some(body) => (body, ListNumberDelim::Period),
        None => match mark.strip_suffix(')') {
            Some(body) => (body, ListNumberDelim::OneParen),
            None => (mark, ListNumberDelim::DefaultDelim),
        },
    };
    enumerator_body(body, delim)
}

/// Parses the numeric/alphabetic/roman body of an enumerator, with its delimiter already determined,
/// into list attributes, or returns `None` when the body is not an enumerator.
fn enumerator_body(body: &str, delim: ListNumberDelim) -> Option<ListAttributes> {
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
    if let Some(start) = roman_value_loose_forward(body) {
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

/// Moves an open paragraph's inlines into the block list. A paragraph with visible content is
/// emitted; one that a text line opened but that filled to nothing (only whitespace) is still
/// emitted as an empty paragraph; a run that no text line opened is dropped.
fn flush_para(fill: &mut Vec<Inline>, blocks: &mut Vec<Block>, started: &mut bool) {
    let mut trimmed = std::mem::take(fill);
    trim_inline_ends(&mut trimmed);
    if !trimmed.is_empty() {
        blocks.push(Block::Para(trimmed));
    } else if *started {
        blocks.push(Block::Para(Vec::new()));
    }
    *started = false;
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
/// for a text line. Whitespace between the control character and the request name is allowed and
/// skipped, so `.  SH` names the `SH` request.
fn control_parts(line: &str) -> Option<(&str, &str)> {
    if !is_control(line) {
        return None;
    }
    let body = line.get(1..).unwrap_or("").trim_start_matches([' ', '\t']);
    match body.split_once([' ', '\t']) {
        Some((name, rest)) => Some((name, rest.trim_start_matches([' ', '\t']))),
        None => Some((body, "")),
    }
}

/// Whether a request name marks a no-op control line: an empty request (a bare control character) or
/// one named only with control characters (`.`, `..`, `...`, `'`). Such a line is transparent and
/// does not interrupt fill.
fn is_noop_request(name: &str) -> bool {
    name.chars().all(|c| matches!(c, '.' | '\''))
}

/// Splits a conditional request's argument into its one-token condition and the branch text that
/// follows it.
fn split_condition(rest: &str) -> (&str, &str) {
    match rest.split_once([' ', '\t']) {
        Some((cond, branch)) => (cond, branch),
        None => (rest, ""),
    }
}

/// Evaluates a conditional request's condition. The nroff target (`n`) and the constant `1` are
/// true; every other condition — the troff target `t`, `0`, other numbers, register and string
/// tests — is treated as false.
fn condition_true(cond: &str) -> bool {
    cond == "n" || cond == "1"
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

/// Substitutes a macro call's arguments for `\$N` references in one body line. `\$1`..`\$9` expand to
/// the corresponding argument (an absent one to nothing) and `\$0` to nothing; a doubled backslash
/// before the reference (`\\$N`, how a reference is written so it survives definition-time copying) is
/// treated the same. Every other backslash sequence is left untouched.
/// Applies copy-mode reduction to a line as it is stored in a macro body: an escaped backslash
/// `\\` collapses to a single `\`. This defers the remaining escapes — argument references `\$N`
/// among them — to the moment the macro is invoked, so a body written with `\\$1` and one written
/// with `\$1` resolve identically when the macro runs.
fn reduce_copy_mode(line: &str) -> String {
    if !line.contains('\\') {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'\\') {
            chars.next();
        }
        out.push(c);
    }
    out
}

fn substitute_macro_args(line: &str, args: &[String]) -> String {
    if !line.contains("\\$") {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                push_macro_arg(&mut chars, args, &mut out);
            }
            // Preserve an escaped backslash intact; consuming one here would let a following
            // `$` be misread as an argument reference.
            Some('\\') => {
                chars.next();
                out.push('\\');
                out.push('\\');
            }
            _ => out.push('\\'),
        }
    }
    out
}

/// After a `\$` reference, reads the one-digit argument index and appends the corresponding call
/// argument (nothing for `\$0` or an out-of-range index).
fn push_macro_arg(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    args: &[String],
    out: &mut String,
) {
    if let Some(&digit) = chars.peek()
        && let Some(index) = digit.to_digit(10)
    {
        chars.next();
        if index >= 1
            && let Some(arg) = args.get((index - 1) as usize)
        {
            out.push_str(arg);
        }
    }
}

/// A scanned character together with the font in effect, or an inter-word separator carrying the
/// literal whitespace character it stands for (so a verbatim region can preserve a tab).
enum Atom {
    Char(Font, char),
    Space(char),
}

/// Tokenizes a line of `man` text into inlines: words become [`Inline::Str`], runs of whitespace a
/// single [`Inline::Space`], and font runs wrap in the appropriate markup. Leading and trailing
/// spaces are dropped.
fn tokenize(text: &str, start_font: Font, strings: &Strings) -> Vec<Inline> {
    let atoms = scan(text, start_font, strings);
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
            run.push(Inline::Str(text.into()));
        } else {
            flush_run(run, *run_font, result);
            if *pending_space {
                push_space(result);
            }
            *run_font = word_font;
            run.push(Inline::Str(text.into()));
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
            Atom::Space(_) => {
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
fn flatten(text: &str, strings: &Strings) -> String {
    let mut out = String::new();
    for atom in scan(text, Font::Regular, strings) {
        match atom {
            Atom::Char(_, c) | Atom::Space(c) => out.push(c),
        }
    }
    out
}

/// Scans a line into atoms, resolving escape sequences and interpolating named strings.
fn scan(text: &str, start_font: Font, strings: &Strings) -> Vec<Atom> {
    let mut atoms = Vec::new();
    let mut font = start_font;
    let mut previous = start_font;
    scan_into(text, &mut font, &mut previous, &mut atoms, strings, 0);
    atoms
}

/// Scans `text` into `atoms`, carrying the running font across the call so an interpolated `\*`
/// string can change the font for the remainder of the line. Font escapes (`\f…`) update the font;
/// an inline comment (`\"`/`\#`) ends the line; a `\*` string is expanded by re-scanning its value,
/// bounded by [`MAX_STRING_DEPTH`] so a self-referential definition cannot loop forever.
// Escape arms are listed separately by groff semantics even where two reduce to the same body.
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn scan_into(
    text: &str,
    font: &mut Font,
    previous: &mut Font,
    atoms: &mut Vec<Atom>,
    strings: &Strings,
    depth: usize,
) {
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ' ' || c == '\t' {
            atoms.push(Atom::Space(c));
            continue;
        }
        if c != '\\' {
            atoms.push(Atom::Char(*font, c));
            continue;
        }
        let Some(&escape) = chars.peek() else {
            break;
        };
        match escape {
            'f' => {
                chars.next();
                let name = read_escape_name(&mut chars);
                apply_font(&name, font, previous);
            }
            '"' | '#' => break,
            '-' => {
                chars.next();
                atoms.push(Atom::Char(*font, '-'));
            }
            'e' | '\\' => {
                chars.next();
                atoms.push(Atom::Char(*font, '\\'));
            }
            '.' => {
                chars.next();
                atoms.push(Atom::Char(*font, '.'));
            }
            // An unpaddable space and a tab are inter-word separators; the tab keeps its own
            // character so a verbatim region preserves it.
            ' ' => {
                chars.next();
                atoms.push(Atom::Space(' '));
            }
            't' => {
                chars.next();
                atoms.push(Atom::Space('\t'));
            }
            '~' => {
                chars.next();
                atoms.push(Atom::Char(*font, '\u{00a0}'));
            }
            '0' => {
                chars.next();
                atoms.push(Atom::Char(*font, '\u{2007}'));
            }
            '^' => {
                chars.next();
                atoms.push(Atom::Char(*font, '\u{200a}'));
            }
            '|' => {
                chars.next();
                atoms.push(Atom::Char(*font, '\u{2006}'));
            }
            // Escapes that emit nothing: `\c` (continuation), the zero-width `\&` and friends, and
            // the half-line vertical motions `\u`/`\d`, which take no argument.
            '&' | ')' | ',' | '/' | ':' | '!' | '%' | '{' | '}' | 'c' | 'u' | 'd' => {
                chars.next();
            }
            '(' => {
                chars.next();
                let name: String = (&mut chars).take(2).collect();
                push_chars(atoms, *font, special_char(&name));
            }
            '[' => {
                chars.next();
                let name = read_delimited(&mut chars, ']');
                push_chars(atoms, *font, bracket_char(&name));
            }
            '*' => {
                chars.next();
                let name = read_escape_name(&mut chars);
                if depth < MAX_STRING_DEPTH
                    && let Some(value) = strings.get(&name)
                {
                    scan_into(value, font, previous, atoms, strings, depth + 1);
                }
            }
            's' => {
                chars.next();
                skip_size(&mut chars);
            }
            // `\n` reads a number-register name and `\k` a position-register name; both are discarded.
            'n' | 'k' => {
                chars.next();
                let _ = read_escape_name(&mut chars);
            }
            // `\z` outputs the next glyph with no width; the glyph is dropped here.
            'z' => {
                chars.next();
                chars.next();
            }
            // Color and named-argument escapes whose name (one char, `(xx`, or `[name]`) carries no
            // text: fill/stroke color (`\m`/`\M`), font family (`\F`), register format (`\g`),
            // environment value (`\V`), macro-as-string (`\Y`), and macro argument (`\$N`).
            'm' | 'M' | 'F' | 'g' | 'V' | 'Y' | '$' => {
                chars.next();
                let _ = read_escape_name(&mut chars);
            }
            // `\p` (break the output line) and `\a` (leader) both produce no text.
            'p' | 'a' => {
                chars.next();
            }
            // `\C'name'` names a glyph with an explicit delimiter, like `\[name]`.
            'C' => {
                chars.next();
                let name = match chars.next() {
                    Some(delim) => read_delimited(&mut chars, delim),
                    None => String::new(),
                };
                push_chars(atoms, *font, bracket_char(&name));
            }
            'h' | 'v' | 'w' | 'o' | 'b' | 'l' | 'L' | 'D' | 'N' | 'R' | 'A' | 'Z' | 'X' | 'B' => {
                chars.next();
                skip_delimited_arg(&mut chars);
            }
            other => {
                chars.next();
                atoms.push(Atom::Char(*font, other));
            }
        }
    }
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
        "C" | "CW" | "CR" => Font::Mono,
        "CB" => Font::MonoBold,
        "CI" => Font::MonoItalic,
        "R" => Font::Regular,
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

/// Builds a [`Block::Table`] from the lines of a tbl region (those between `.TS` and `.TE`, both
/// excluded). The region is the preprocessor's: an optional options line ending in `;` (carrying the
/// cell separator in its `tab(X)` option), one or more format lines the last of which ends in `.`
/// (the first fixes the column count and alignments), then the data rows. A rule line (`_`/`=`) just
/// below the first data row promotes that row to the table head. A `T{`…`T}` text block spanning
/// several input lines collapses into one filled cell. A format declaring a horizontal span, which
/// the table model cannot express, renders as a placeholder paragraph. Returns `None` for a region
/// with no format line, where there is no table to build.
fn build_tbl(region: &[String]) -> Option<Block> {
    let mut index = 0;
    let mut separator = "\t".to_owned();
    if let Some(first) = region.first()
        && first.trim_end().ends_with(';')
    {
        if let Some(sep) = tab_option(first) {
            separator = sep;
        }
        index = 1;
    }

    let aligns = parse_col_aligns(region.get(index)?);
    if aligns.is_empty() {
        return None;
    }
    let columns = aligns.len();
    let mut data_start = None;
    for (offset, line) in region.iter().enumerate().skip(index) {
        if line.trim_end().ends_with('.') {
            data_start = Some(offset + 1);
            break;
        }
    }
    let data_start = data_start?;

    // A column that horizontally spans its neighbor has no representation in the table model, so a
    // region whose format declares one is rendered as a placeholder paragraph instead.
    if region
        .get(index..data_start)
        .unwrap_or(&[])
        .iter()
        .any(|line| format_has_span(line))
    {
        return Some(Block::Para(vec![Inline::Str("TABLE".into())]));
    }

    let data = collapse_text_blocks(region.get(data_start..).unwrap_or(&[]), &separator);

    let (head_lines, body_lines): (&[String], &[String]) =
        if data.get(1).is_some_and(|line| is_rule(line)) {
            (data.get(..1).unwrap_or(&[]), data.get(2..).unwrap_or(&[]))
        } else {
            (&[], &data)
        };

    let col_specs = aligns
        .into_iter()
        .map(|align| ColSpec {
            align,
            width: ColWidth::ColWidthDefault,
        })
        .collect();
    let head = TableHead {
        attr: Attr::default(),
        rows: head_lines
            .iter()
            .map(|line| tbl_row(line, &separator, columns))
            .collect(),
    };
    let body = TableBody {
        attr: Attr::default(),
        row_head_columns: 0,
        head: Vec::new(),
        body: body_lines
            .iter()
            .filter(|line| !is_rule(line))
            .map(|line| tbl_row(line, &separator, columns))
            .collect(),
    };

    Some(Block::Table(Box::new(Table {
        attr: Attr::default(),
        caption: Caption::default(),
        col_specs,
        head,
        bodies: vec![body],
        foot: TableFoot::default(),
    })))
}

/// Reads the cell separator from a tbl options line's `tab(X)` option, if it carries one.
fn tab_option(options: &str) -> Option<String> {
    let inside = options.split_once("tab(")?.1.split_once(')')?.0;
    (!inside.is_empty()).then(|| inside.to_owned())
}

/// Parses the alignment of each column from a tbl format line. Each key letter (`l`/`a` left, `r`/`n`
/// right, `c` center) opens a column; `s` continues a horizontal span; a font modifier (`f` and its
/// name) and a width modifier (`w`/`p`/`v`/`m` and its parenthesized or numeric argument) are skipped.
fn parse_col_aligns(spec: &str) -> Vec<Alignment> {
    let mut aligns = Vec::new();
    let mut chars = spec.chars().peekable();
    while let Some(c) = chars.next() {
        match c.to_ascii_lowercase() {
            'l' | 'a' => aligns.push(Alignment::AlignLeft),
            'r' | 'n' => aligns.push(Alignment::AlignRight),
            'c' => aligns.push(Alignment::AlignCenter),
            'f' => match chars.peek() {
                Some('(') => {
                    chars.next();
                    chars.next();
                    chars.next();
                }
                Some('[') => {
                    chars.next();
                    read_delimited(&mut chars, ']');
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            'w' | 'p' | 'v' | 'm' => {
                if chars.peek() == Some(&'(') {
                    chars.next();
                    for d in chars.by_ref() {
                        if d == ')' {
                            break;
                        }
                    }
                } else {
                    while matches!(chars.peek(), Some(d) if d.is_ascii_digit()) {
                        chars.next();
                    }
                }
            }
            _ => {}
        }
    }
    aligns
}

/// Whether a tbl format line declares a horizontal span (an `s`/`S` key), skipping the font and width
/// modifiers whose own arguments could otherwise contain that letter.
fn format_has_span(spec: &str) -> bool {
    let mut chars = spec.chars().peekable();
    while let Some(c) = chars.next() {
        match c.to_ascii_lowercase() {
            's' => return true,
            'f' => match chars.peek() {
                Some('(') => {
                    chars.next();
                    chars.next();
                    chars.next();
                }
                Some('[') => {
                    chars.next();
                    read_delimited(&mut chars, ']');
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            'w' | 'p' | 'v' | 'm' => {
                if chars.peek() == Some(&'(') {
                    chars.next();
                    for d in chars.by_ref() {
                        if d == ')' {
                            break;
                        }
                    }
                } else {
                    while matches!(chars.peek(), Some(d) if d.is_ascii_digit()) {
                        chars.next();
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// Collapses tbl text blocks into single data lines. A field of `T{` begins a block whose content is
/// the following lines up to a line starting with `T}`; those lines join with single spaces into the
/// field, and any fields after `T}` on its line continue the row.
fn collapse_text_blocks(data: &[String], separator: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    while let Some(line) = data.get(index) {
        index += 1;
        if !line.split(separator).any(|field| field.trim() == "T{") {
            out.push(line.clone());
            continue;
        }
        let mut fields: Vec<String> = Vec::new();
        for field in line.split(separator) {
            if field.trim() != "T{" {
                fields.push(field.to_owned());
                continue;
            }
            let mut block: Vec<String> = Vec::new();
            let mut terminated = false;
            while let Some(block_line) = data.get(index) {
                index += 1;
                if block_line.trim_start().starts_with("T}") {
                    let mut tail = block_line.split(separator);
                    tail.next();
                    fields.push(block.join(" "));
                    fields.extend(tail.map(str::to_owned));
                    terminated = true;
                    break;
                }
                block.push(block_line.clone());
            }
            if !terminated {
                fields.push(block.join(" "));
            }
        }
        out.push(fields.join(separator));
    }
    out
}

/// Whether a tbl line is a horizontal rule: a non-empty line of only `_` or `=` characters.
fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '_' || c == '=')
}

/// Builds one table row of exactly `columns` cells from a tbl data line: fields past the column count
/// are dropped and missing fields are filled with empty cells.
fn tbl_row(line: &str, separator: &str, columns: usize) -> Row {
    let mut cells: Vec<Cell> = line.split(separator).take(columns).map(tbl_cell).collect();
    while cells.len() < columns {
        cells.push(tbl_cell(""));
    }
    Row {
        attr: Attr::default(),
        cells,
    }
}

/// Builds a table cell from raw field text: surviving backslash escapes are stripped and the
/// remainder is split on whitespace into words. An empty field yields a cell with no content.
fn tbl_cell(field: &str) -> Cell {
    let cleaned: String = field.chars().filter(|&c| c != '\\').collect();
    let mut inlines = Vec::new();
    for word in cleaned.split_whitespace() {
        if !inlines.is_empty() {
            inlines.push(Inline::Space);
        }
        inlines.push(Inline::Str(word.into()));
    }
    let content = if inlines.is_empty() {
        Vec::new()
    } else {
        vec![Block::Plain(inlines)]
    };
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content,
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
        "/O" => '\u{00d8}',
        "/o" => '\u{00f8}',
        "a-" => '\u{00af}',
        "a." => '\u{02d9}',
        "ad" => '\u{00a8}',
        "ah" => '\u{02c7}',
        "a^" => '^',
        // Diaeresis.
        ":a" => '\u{00e4}',
        ":e" => '\u{00eb}',
        ":i" => '\u{00ef}',
        ":o" => '\u{00f6}',
        ":u" => '\u{00fc}',
        ":y" => '\u{00ff}',
        ":A" => '\u{00c4}',
        ":E" => '\u{00cb}',
        ":I" => '\u{00cf}',
        ":O" => '\u{00d6}',
        ":U" => '\u{00dc}',
        ":Y" => '\u{0178}',
        // Acute accent.
        "'a" => '\u{00e1}',
        "'c" => '\u{0107}',
        "'e" => '\u{00e9}',
        "'i" => '\u{00ed}',
        "'o" => '\u{00f3}',
        "'u" => '\u{00fa}',
        "'y" => '\u{00fd}',
        "'A" => '\u{00c1}',
        "'C" => '\u{0106}',
        "'E" => '\u{00c9}',
        "'I" => '\u{00cd}',
        "'O" => '\u{00d3}',
        "'U" => '\u{00da}',
        "'Y" => '\u{00dd}',
        // Grave accent.
        "`a" => '\u{00e0}',
        "`e" => '\u{00e8}',
        "`i" => '\u{00ec}',
        "`o" => '\u{00f2}',
        "`u" => '\u{00f9}',
        "`A" => '\u{00c0}',
        "`E" => '\u{00c8}',
        "`I" => '\u{00cc}',
        "`O" => '\u{00d2}',
        "`U" => '\u{00d9}',
        // Circumflex.
        "^a" => '\u{00e2}',
        "^e" => '\u{00ea}',
        "^i" => '\u{00ee}',
        "^o" => '\u{00f4}',
        "^u" => '\u{00fb}',
        "^A" => '\u{00c2}',
        "^E" => '\u{00ca}',
        "^I" => '\u{00ce}',
        "^O" => '\u{00d4}',
        "^U" => '\u{00db}',
        // Tilde.
        "~a" => '\u{00e3}',
        "~n" => '\u{00f1}',
        "~o" => '\u{00f5}',
        "~A" => '\u{00c3}',
        "~N" => '\u{00d1}',
        "~O" => '\u{00d5}',
        // Cedilla.
        ",c" => '\u{00e7}',
        ",C" => '\u{00c7}',
        // Other Latin letters and ligatures.
        "ss" => '\u{00df}',
        "ae" => '\u{00e6}',
        "AE" => '\u{00c6}',
        "oe" => '\u{0153}',
        "OE" => '\u{0152}',
        "-D" => '\u{00d0}',
        "Sd" => '\u{00f0}',
        "TP" => '\u{00de}',
        "Tp" => '\u{00fe}',
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
        // Angle brackets and extensible bars.
        "la" => '\u{27e8}',
        "ra" => '\u{27e9}',
        "va" => '\u{2195}',
        "an" => '\u{23af}',
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
                Box::new(Attr {
                    id: "name".into(),
                    ..Attr::default()
                }),
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
    fn tbl_region_becomes_a_table() {
        let doc = read(".TH T 1\n.TS\nl r.\nName\tAge\n_\nAda\t36\n.TE\nafter\n");
        let Some(Block::Table(table)) = doc.blocks.first() else {
            panic!("expected a table");
        };
        // Alignments come from the format line; widths stay default.
        assert_eq!(
            table.col_specs,
            vec![
                ColSpec {
                    align: Alignment::AlignLeft,
                    width: ColWidth::ColWidthDefault,
                },
                ColSpec {
                    align: Alignment::AlignRight,
                    width: ColWidth::ColWidthDefault,
                },
            ]
        );
        // The rule line under the first data row promotes it to the head.
        assert_eq!(table.head.rows.len(), 1);
        assert_eq!(table.head.rows.first().map(|row| row.cells.len()), Some(2));
        assert_eq!(table.bodies.first().map(|body| body.body.len()), Some(1));
        assert_eq!(
            doc.blocks.get(1),
            Some(&Block::Para(vec![Inline::Str("after".into())]))
        );
    }

    #[test]
    fn tbl_without_header_rule_puts_every_row_in_the_body() {
        let doc = read(".TH T 1\n.TS\nc c.\nName\tAge\nAda\t36\n.TE\n");
        let Some(Block::Table(table)) = doc.blocks.first() else {
            panic!("expected a table");
        };
        assert!(table.head.rows.is_empty());
        assert_eq!(table.bodies.first().map(|body| body.body.len()), Some(2));
    }

    #[test]
    fn malformed_tbl_region_yields_no_block() {
        let doc = read(".TS");
        assert!(doc.blocks.is_empty());
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
                Box::default(),
                "line one\n  indented".into()
            ))
        );
    }

    #[test]
    fn example_region_becomes_a_code_block() {
        let doc = read(".TH T 1\n.EX\n\\fBcode\\fR \\- here\n.EE\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::CodeBlock(Box::default(), "code - here".into()))
        );
    }

    #[test]
    fn uri_macro_becomes_a_link() {
        let doc = read(".TH T 1\n.UR https://example.com\nthe text\n.UE\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Link(
                Box::default(),
                vec![
                    Inline::Str("the".into()),
                    Inline::Space,
                    Inline::Str("text".into()),
                ],
                Box::new(Target {
                    url: "https://example.com".into(),
                    title: carta_ast::Text::default(),
                }),
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

    #[test]
    fn defined_string_interpolates_and_rescans_its_escapes() {
        let doc = read(".TH T 1\n.ds B \\fBbold\\fP\nx \\*B y\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("x".into()),
                Inline::Space,
                Inline::Strong(vec![Inline::Str("bold".into())]),
                Inline::Space,
                Inline::Str("y".into()),
            ]))
        );
    }

    #[test]
    fn predefined_strings_resolve() {
        let doc = read(".TH T 1\n\\*(lq x \\*(rq \\*(Tm \\*R\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("\u{201c}".into()),
                Inline::Space,
                Inline::Str("x".into()),
                Inline::Space,
                Inline::Str("\u{201d}".into()),
                Inline::Space,
                Inline::Str("\u{2122}".into()),
                Inline::Space,
                Inline::Str("\u{00ae}".into()),
            ]))
        );
    }

    #[test]
    fn accented_special_characters_resolve() {
        let doc = read(".TH T 1\n\\(:a\\(ss\\('e\\(la\\(,c\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Str(
                "\u{e4}\u{df}\u{e9}\u{27e8}\u{e7}".into()
            )]))
        );
    }

    #[test]
    fn tab_escape_becomes_a_space() {
        let doc = read(".TH T 1\na\\tb\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("a".into()),
                Inline::Space,
                Inline::Str("b".into()),
            ]))
        );
    }

    #[test]
    fn continuation_escape_is_dropped() {
        // `\c` vanishes; the two text lines still fill with a separating space.
        let doc = read(".TH T 1\nabc\\c\ndef\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("abc".into()),
                Inline::Space,
                Inline::Str("def".into()),
            ]))
        );
    }

    #[test]
    fn zero_width_and_motion_escapes_drop_their_glyphs() {
        // `\z` drops the following glyph, `\u`/`\d` take no argument, `\k` reads a register name.
        let doc = read(".TH T 1\na\\zbc up\\udown\\d mark\\kx end\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("ac".into()),
                Inline::Space,
                Inline::Str("updown".into()),
                Inline::Space,
                Inline::Str("mark".into()),
                Inline::Space,
                Inline::Str("end".into()),
            ]))
        );
    }

    #[test]
    fn trailing_backslash_joins_the_next_line_without_a_space() {
        let doc = read(".TH T 1\nfoo\\\nbar\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Str("foobar".into())]))
        );
    }

    #[test]
    fn supplementary_tag_joins_terms_with_a_line_break() {
        let doc = read(".TH T 1\n.TP\n.B \\-a\n.TQ\n.B \\-b\nbody.\n");
        let Some(Block::DefinitionList(items)) = doc.blocks.first() else {
            panic!("expected a definition list");
        };
        assert_eq!(
            items.first().map(|(term, _)| term.clone()),
            Some(vec![
                Inline::Strong(vec![Inline::Str("-a".into())]),
                Inline::LineBreak,
                Inline::Strong(vec![Inline::Str("-b".into())]),
            ])
        );
    }

    #[test]
    fn request_in_link_label_aborts_the_link() {
        // The label's request makes a link impossible; the label is emitted as its own block and the
        // text trailing the terminator is dropped.
        let doc = read(".TH T 1\nbefore\n.UR u\n.B bold\n.UE after\nnext\n");
        assert_eq!(
            doc.blocks,
            vec![
                Block::Para(vec![Inline::Str("before".into())]),
                Block::Para(vec![Inline::Strong(vec![Inline::Str("bold".into())])]),
                Block::Para(vec![Inline::Str("next".into())]),
            ]
        );
    }

    #[test]
    fn link_without_a_terminator_emits_its_label() {
        let doc = read(".TH T 1\n.UR u\nlabel\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Str("label".into())]))
        );
    }

    #[test]
    fn whitespace_only_line_does_not_break_the_paragraph() {
        let doc = read(".TH T 1\none\n \ntwo\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("one".into()),
                Inline::Space,
                Inline::Str("two".into()),
            ]))
        );
        assert_eq!(doc.blocks.len(), 1);
    }

    #[test]
    fn lone_whitespace_line_is_an_empty_paragraph() {
        let doc = read(".TH T 1\n \n");
        assert_eq!(doc.blocks.first(), Some(&Block::Para(Vec::new())));
    }

    #[test]
    fn tagged_paragraph_with_no_body_becomes_a_paragraph() {
        let doc = read(".TH T 1\n.TP\n.B \\-x\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Strong(vec![Inline::Str(
                "-x".into()
            )])]))
        );
    }

    #[test]
    fn empty_tagged_paragraph_nests_the_following_items() {
        let doc = read(".TH T 1\n.TP\n.B \\-a\n.TP\n.B \\-b\nbody.\n");
        let Some(Block::DefinitionList(items)) = doc.blocks.first() else {
            panic!("expected a definition list");
        };
        assert_eq!(items.len(), 1);
        let nested = items
            .first()
            .and_then(|(_, bodies)| bodies.first())
            .and_then(|blocks| blocks.first());
        assert!(matches!(nested, Some(Block::DefinitionList(_))));
    }

    #[test]
    fn marked_item_with_no_body_keeps_an_empty_paragraph() {
        let doc = read(".TH T 1\n.IP \\(bu\n.IP \\(bu\nsecond.\n");
        let Some(Block::BulletList(items)) = doc.blocks.first() else {
            panic!("expected a bullet list");
        };
        assert_eq!(items.first(), Some(&vec![Block::Para(Vec::new())]));
    }

    #[test]
    fn unmarked_item_with_no_body_contributes_nothing() {
        let doc = read(".TH T 1\n.IP\n");
        assert!(doc.blocks.is_empty());
    }

    #[test]
    fn ascii_identifiers_fold_an_accented_heading() {
        let doc = read_with(
            ".TH T 1\n.SH Café\nx\n",
            Extensions::from_list(&[Extension::AutoIdentifiers, Extension::AsciiIdentifiers]),
        );
        assert!(matches!(
            doc.blocks.first(),
            Some(Block::Header(1, attr, _)) if attr.id == "cafe"
        ));
    }

    #[test]
    fn constant_width_font_escape_becomes_code() {
        let doc = read(".TH T 1\nplain \\f(CWmono\\fP back\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("plain".into()),
                Inline::Space,
                Inline::Code(Box::default(), "mono".into()),
                Inline::Space,
                Inline::Str("back".into()),
            ]))
        );
    }

    #[test]
    fn constant_width_bold_font_wraps_code_in_strong() {
        let doc = read(".TH T 1\n\\f(CBmono\\fP\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Strong(vec![Inline::Code(
                Box::default(),
                "mono".into()
            )])]))
        );
    }

    #[test]
    fn user_macro_substitutes_call_arguments() {
        let doc = read(".TH T 1\n.de GREET\nHello \\$1 and \\$2.\n..\n.GREET Alice Bob\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("Hello".into()),
                Inline::Space,
                Inline::Str("Alice".into()),
                Inline::Space,
                Inline::Str("and".into()),
                Inline::Space,
                Inline::Str("Bob.".into()),
            ]))
        );
    }

    #[test]
    fn multi_line_macro_expansion_fills_like_inline_text() {
        let inline = read(".TH T 1\nfirst line\nsecond line\n");
        let via_macro = read(".TH T 1\n.de M\nfirst line\nsecond line\n..\n.M\n");
        assert_eq!(inline.blocks, via_macro.blocks);
    }

    #[test]
    fn nested_macro_call_expands_in_place_preserving_order() {
        let doc =
            read(".TH T 1\n.de INNER\nmiddle\n..\n.de OUTER\nbefore\n.INNER\nafter\n..\n.OUTER\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("before".into()),
                Inline::Space,
                Inline::Str("middle".into()),
                Inline::Space,
                Inline::Str("after".into()),
            ]))
        );
    }

    #[test]
    fn macro_whose_expansion_synthesizes_its_own_call_terminates() {
        // `\$` followed by a non-digit is dropped by argument substitution, so the text line
        // `\$.M` expands to the call line `.M` — a self-call the recursion guard cannot see
        // because it starts a fresh invocation. Only the document-wide budget stops it.
        let _ = read(".TH T 1\n.de M\ntext\n\\$.M\n..\n.M\n");
    }

    #[test]
    fn macro_argument_doubling_across_synthesized_calls_terminates() {
        // Each synthesized re-invocation passes its argument twice, doubling its length; the
        // byte budget must cut the growth off.
        let _ = read(".TH T 1\n.de M\n\\$.M \"\\$1\\$1\"\n..\n.M xxxxxxxx\n");
    }

    #[test]
    fn macro_expansion_seam_keeps_base_lines_in_order() {
        let doc = read(".TH T 1\n.de M\nexpanded\n..\n.M\nbase line\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("expanded".into()),
                Inline::Space,
                Inline::Str("base".into()),
                Inline::Space,
                Inline::Str("line".into()),
            ]))
        );
    }

    #[test]
    fn conditional_inside_macro_expansion_reprocesses_the_queued_line() {
        // `.ie`/`.el` reprocess the *queued* expansion line in place; the base document's
        // following line must survive untouched.
        let doc = read(".TH T 1\n.de M\n.ie n kept\n.el dropped\n..\n.M\nbase line\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("kept".into()),
                Inline::Space,
                Inline::Str("base".into()),
                Inline::Space,
                Inline::Str("line".into()),
            ]))
        );
    }

    #[test]
    fn link_label_spanning_macro_expansion_and_base_document_is_recognized() {
        // The label opens inside a macro expansion (queued) and its terminator sits in the base
        // document (unqueued); the lookahead must chain across that seam to find it.
        let doc =
            read(".TH T 1\n.de LABEL\n.UR https://example.com\nfirst\n..\n.LABEL\nsecond\n.UE\n");
        let Some(Block::Para(inlines)) = doc.blocks.first() else {
            panic!("expected a paragraph");
        };
        assert!(matches!(
            inlines.first(),
            Some(Inline::Link(_, _, target)) if target.url == "https://example.com"
        ));
    }

    #[test]
    fn doubled_backslash_argument_reference_reduces_like_a_single_one() {
        let single = read(".TH T 1\n.de M\nvalue \\$1\n..\n.M x\n");
        let doubled = read(".TH T 1\n.de M\nvalue \\\\$1\n..\n.M x\n");
        assert_eq!(single.blocks, doubled.blocks);
        assert_eq!(
            single.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("value".into()),
                Inline::Space,
                Inline::Str("x".into()),
            ]))
        );
    }

    #[test]
    fn copy_mode_reduces_an_escaped_backslash_before_an_escape() {
        assert_eq!(reduce_copy_mode("x\\\\(buy"), "x\\(buy");
        assert_eq!(reduce_copy_mode("plain text"), "plain text");
    }

    #[test]
    fn font_macro_with_an_explicit_empty_argument_keeps_its_wrapper() {
        let doc = read(".TH T 1\nbefore\n.B \"\"\nafter\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("before".into()),
                Inline::Space,
                Inline::Strong(Vec::new()),
                Inline::Space,
                Inline::Str("after".into()),
            ]))
        );
    }

    #[test]
    fn font_macro_with_no_argument_takes_the_next_line() {
        let doc = read(".TH T 1\nbefore\n.I\nafter\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("before".into()),
                Inline::Space,
                Inline::Emph(vec![Inline::Str("after".into())]),
            ]))
        );
    }

    #[test]
    fn option_synopsis_brackets_a_bold_option_name() {
        let doc = read(".TH T 1\n.OP \\-o file\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![
                Inline::Str("[".into()),
                Inline::Space,
                Inline::Strong(vec![Inline::Str("-o".into())]),
                Inline::Space,
                Inline::Str("file".into()),
                Inline::Space,
                Inline::Str("]".into()),
            ]))
        );
    }

    #[test]
    fn table_with_a_horizontal_span_degrades_to_a_placeholder() {
        let doc = read(".TH T 1\n.TS\nl s l.\nWide\t\tEnd\none\ttwo\tthree\n.TE\n");
        assert_eq!(
            doc.blocks.first(),
            Some(&Block::Para(vec![Inline::Str("TABLE".into())]))
        );
    }

    #[test]
    fn table_text_block_joins_its_lines() {
        let doc = read(".TH T 1\n.TS\nl l.\nName\tT{\nA long\ndescription\nT}\nLeft\tRight\n.TE\n");
        let Some(Block::Table(table)) = doc.blocks.first() else {
            panic!("expected a table");
        };
        // The two source lines of the `T{ … T}` block join into a single cell.
        let cell_text = format!("{table:?}");
        assert!(cell_text.contains("long"));
        assert!(cell_text.contains("description"));
    }

    #[test]
    fn east_asian_line_breaks_is_accepted_and_inert() {
        let input = ".TH T 1\n.SH H\nplain filled text\n";
        let base = read(input);
        let with = read_with(
            input,
            Extensions::from_list(&[Extension::AutoIdentifiers, Extension::EastAsianLineBreaks]),
        );
        assert_eq!(base.blocks, with.blocks);
    }
}
