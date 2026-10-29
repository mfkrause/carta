//! Block-level parsing methods for the `man` reader's parser.

use carta_ast::{Block, Inline, MetaValue, Target};

use crate::inline_text::{trim_inline_ends, words_to_inlines};

use super::inline::{
    Font, alternating, flatten, font_macro, fonts_for, manual_reference, single_font, tokenize,
};
use super::lists::{
    Mark, Pending, classify_mark, flush_pending, push_bullet, push_definition, push_ordered,
};
use super::requests::{control_parts, is_comment, split_args};
use super::tables::build_tbl;
use super::{Ctx, Parser, append_text};

impl Parser {
    /// Heading inline content: the macro's arguments joined by spaces, or, when the macro carries
    /// none, the following input line.
    pub(super) fn heading_inlines(&mut self, rest: &str) -> Vec<Inline> {
        if rest.is_empty() {
            let next = self.take_line().unwrap_or_default();
            tokenize(&next, Font::Regular, &self.strings)
        } else {
            tokenize(&split_args(rest).join(" "), Font::Regular, &self.strings)
        }
    }

    /// Reads `.TH` arguments into metadata: identifier, section, date, footer, header.
    pub(super) fn parse_title(&mut self, rest: &str) {
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
    pub(super) fn define_string(&mut self, rest: &str) {
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
    pub(super) fn parse_verbatim(&mut self) -> Block {
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
                    "B" | "I" => {
                        self.advance();
                        text_lines.push(flatten(&split_args(rest).join(" "), &self.strings));
                    }
                    "BR" | "RB" | "BI" | "IB" | "RI" | "IR" => {
                        self.advance();
                        text_lines.push(
                            split_args(rest)
                                .iter()
                                .map(|argument| flatten(argument, &self.strings))
                                .collect(),
                        );
                    }
                    "SM" | "SB" | "MR" => {
                        self.advance();
                        let text = if name == "MR" {
                            carta_ast::to_plain_text(&manual_reference(rest, &self.strings))
                        } else {
                            flatten(&split_args(rest).join(" "), &self.strings)
                        };
                        if !text.is_empty() {
                            text_lines.push(text);
                        }
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
    pub(super) fn parse_tbl(&mut self) -> Vec<Block> {
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
    pub(super) fn parse_list(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        let mut pending: Option<Pending> = None;
        let mut parents = Vec::new();
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
                        parents.push((out, pending, term));
                        out = Vec::new();
                        pending = None;
                    } else {
                        push_definition(&mut pending, &mut out, term, body);
                    }
                }
                "IP" => {
                    self.advance();
                    let args = split_args(rest);
                    match args.first() {
                        None => {
                            flush_pending(&mut pending, &mut out);
                            let body = self.parse_blocks(Ctx::ITEM);
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
                                // Neither bullet nor enumerator (even reducing to nothing): a term.
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
        while let Some((mut outer, mut pending, term)) = parents.pop() {
            if out.is_empty() {
                flush_pending(&mut pending, &mut outer);
                outer.push(Block::Para(term));
            } else {
                push_definition(&mut pending, &mut outer, term, out);
            }
            flush_pending(&mut pending, &mut outer);
            out = outer;
        }
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
    pub(super) fn read_term(&mut self) -> Vec<Inline> {
        let mut fonts = Vec::new();
        while let Some(line) = self.take_line() {
            if is_comment(&line) {
                continue;
            }
            let mut inlines = if let Some((name, rest)) = control_parts(&line) {
                match name {
                    "B" | "I" | "SM" | "SB" => {
                        let font = single_font(name);
                        if rest.is_empty() {
                            fonts.push(font);
                            continue;
                        }
                        font_macro(font, &split_args(rest).join(" "), &self.strings)
                    }
                    "BR" | "RB" | "BI" | "IB" | "RI" | "IR" => {
                        alternating(rest, fonts_for(name), &self.strings)
                    }
                    "MR" => manual_reference(rest, &self.strings),
                    _ => tokenize(rest, Font::Regular, &self.strings),
                }
            } else {
                tokenize(&line, Font::Regular, &self.strings)
            };
            for font in fonts.into_iter().rev() {
                inlines = font.wrap(inlines);
            }
            return inlines;
        }
        Vec::new()
    }

    /// Whether the label that opens at the current position is plain: only text lines (and comments)
    /// up to a `.UE`/`.ME` terminator. A request inside the label, or end of input before the
    /// terminator, makes the label non-plain, so the link is abandoned.
    pub(super) fn link_label_is_plain(&self) -> bool {
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
    pub(super) fn parse_link(&mut self, url: String, fill: &mut Vec<Inline>) {
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
    pub(super) fn parse_aborted_link(&mut self) -> Vec<Block> {
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
                "B" | "I" | "SM" | "SB" => {
                    self.advance();
                    let font = single_font(name);
                    let inlines = if rest.is_empty() {
                        font.wrap(self.read_term())
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
                "MR" => {
                    self.advance();
                    append_text(&mut fill, manual_reference(rest, &self.strings));
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
