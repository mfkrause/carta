//! Reader for the `man` macro package (the `groff`/`troff` manual-page language).
//!
//! A manual page is a sequence of control lines (a request or macro, introduced by `.` or `'` in
//! the first column) and text lines. Text lines are *filled*: consecutive lines collapse into one
//! paragraph, their words separated by single spaces. Macros structure the page: section headings
//! (`.SH`/`.SS`), paragraph breaks (`.PP`), tagged and indented lists (`.TP`/`.IP`), relative insets
//! (`.RS`/`.RE`), verbatim regions (`.nf`/`.EX`), and hyperlinks (`.UR`/`.MT`). Inline font macros
//! (`.B`, `.I`, `.BR`, …) and the `\f` escape switch between roman, bold, and italic; the `\(xx`,
//! `\[…]`, and `\*x` escapes produce special characters and predefined strings.
//!
//! The title macro `.TH` populates document metadata (`title`, `section`, `date`, `footer`,
//! `header`); everything else becomes the block sequence.

use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{Attr, Block, Document, Inline, MetaValue, slug, slug_gfm, to_plain_text};
use carta_core::{Extensions, Reader, ReaderOptions, Result};

use crate::heading_ids::{IdRegistry, IdScheme};
use crate::inline_text::trim_inline_ends;
use crate::transliterate::fold_to_ascii;

mod blocks;
mod inline;
mod lists;
mod requests;
mod tables;

use inline::{Font, alternating, font_macro, fonts_for, option_synopsis, single_font, tokenize};
use requests::{
    condition_true, control_parts, is_comment, is_noop_request, reduce_copy_mode, split_args,
    split_condition, substitute_macro_args,
};

/// A table of named strings: the predefined groff strings plus any defined with `.ds`, looked up by
/// the `\*` interpolation escape.
type Strings = BTreeMap<String, String>;

/// The deepest a `\*` interpolation may recurse, bounding self-referential string definitions.
const MAX_STRING_DEPTH: usize = 8;

/// The most lines all macro expansion may produce across a document. The budget is cumulative
/// rather than per-invocation because argument substitution can synthesize a fresh call line
/// (`\$` followed by a non-digit leaves the rest of the line intact, so `\$.X` expands to the call
/// `.X`), and each such invocation restarts with an empty recursion guard; only a shared budget
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
        // Slug shape follows the active extension; empty slugs become `section`, repeats increment.
        let base = match scheme {
            IdScheme::Plain => slug(&text),
            IdScheme::Gfm => slug_gfm(&text),
        };
        // Folding runs on the finished slug, keeping separators left by unfoldable words. The plain
        // shape then re-drops the leading run folding can expose; the gfm shape never strips it.
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
    /// this queue's front when non-empty, else `lines[pos]`: expanding a macro call pushes its
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
                    // Substituted arguments debit the budget even unemitted, bounding nested-call growth.
                    let substituted = substitute_macro_args(inner_rest, args);
                    budget.debit(substituted.len());
                    let inner_args = split_args(&substituted);
                    self.expand_macro_into(inner, &inner_args, active, out, budget);
                }
                // Request lines pass verbatim; argument references fall to escape processing (nothing).
                Some(_) => {
                    budget.debit(raw.len());
                    out.push(raw.clone());
                }
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
        // Whether a text line opened the paragraph: whitespace-only paragraphs still emit `Para []`.
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
                        // A request inside the label aborts the link; the label emits as its own blocks.
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
                // A user-macro call queues its expansion ahead of the current position, parsed in place.
                _ if self.macros.contains_key(name) => {
                    self.advance();
                    let args = split_args(rest);
                    let expansion = self.expand_macro_call(name, &args);
                    for line in expansion.into_iter().rev() {
                        self.pending.push_front(line);
                    }
                }
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

#[cfg(test)]
use carta_ast::{
    Alignment, ColSpec, ColWidth, ListAttributes, ListNumberDelim, ListNumberStyle, Target,
};
#[cfg(test)]
use lists::parse_enumerator;

#[cfg(test)]
mod tests;
