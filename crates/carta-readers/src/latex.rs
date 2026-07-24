//! LaTeX reader: parses a LaTeX source document into the document model.
//!
//! LaTeX has no line-oriented block grammar the way lightweight markup does; a source file is a
//! stream of ordinary text, control sequences (`\name` / `\<symbol>`), grouping braces, math shifts,
//! and environments (`\begin{env}` … `\end{env}`). Parsing is a single character-level recursive
//! descent over the source: `Parser::parse_blocks` recognises block-level constructs (sectioning
//! commands, environments, paragraphs) and `Parser::parse_inlines` the inline run inside each,
//! accumulating a text buffer that flushes to [`Inline::Str`](carta_ast::Inline::Str) at every
//! space, break, or markup
//! boundary.
//!
//! Only the body between `\begin{document}` and `\end{document}` is rendered when a document
//! environment is present; the preamble is scanned for metadata (`\title`, `\author`, `\date`) and
//! macro definitions but otherwise dropped. A construct that has no faithful model representation
//! degrades: an unknown command is dropped (or passed through verbatim under `raw_tex`), and an
//! unknown environment becomes a classed [`Block::Div`] (or a raw block under `raw_tex`).

use std::collections::BTreeMap;
use std::rc::Rc;

use carta_ast::{Block, Document, MetaValue};
use carta_core::{Extension, Extensions, Reader, ReaderOptions, Result};

use crate::heading_ids::IdRegistry;

use self::support::{math_env, section_intrinsic};

mod blocks;
mod expand;
mod inline;
mod support;
mod tables;

#[cfg(test)]
mod tests;

/// Parses LaTeX source into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct LatexReader;

impl Reader for LatexReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let ext = options.extensions;
        let (preamble, body) = split_document(input);

        // section levels depend on the highest sectioning command used, so scan the whole body first
        let base_level = base_section_level(body);

        let mut parser = Parser {
            frames: Vec::new(),
            ext,
            smart: ext.contains(Extension::Smart),
            meta: BTreeMap::new(),
            macros: Rc::new(BTreeMap::new()),
            ids: IdRegistry::default(),
            base_level,
            in_figure: false,
            in_float: false,
            expand_depth: 0,
            total_expansions: 0,
            last_ws_had_newline: false,
        };

        // The preamble contributes metadata and macro definitions but no blocks.
        if let Some(preamble) = preamble {
            parser.set_source(preamble);
            let _ = parser.parse_blocks(&Stop::Eof);
        }

        parser.set_source(body);
        let blocks = parser.parse_blocks(&Stop::Env("document"));

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

/// Splits the source into an optional preamble and the body to render. With a `\begin{document}`,
/// the preamble is everything before it and the body is the text up to a matching `\end{document}`;
/// without one, the whole source is the body and there is no preamble.
fn split_document(input: &str) -> (Option<&str>, &str) {
    // the needles end in `}`, so substring search cannot latch onto a longer control word
    if let Some(begin) = input.find("\\begin{document}") {
        let after = &input[begin + "\\begin{document}".len()..];
        let body = match after.find("\\end{document}") {
            Some(end) => &after[..end],
            None => after,
        };
        (Some(&input[..begin]), body)
    } else {
        (None, input)
    }
}

/// The header level the top-most sectioning command in `body` maps to. `\part` shifts every section
/// down by two levels and `\chapter` by one, so that whichever appears highest becomes level one.
fn base_section_level(body: &str) -> i32 {
    if has_command(body, "part") {
        -1
    } else {
        i32::from(!has_command(body, "chapter"))
    }
}

/// Whether the control word `\name` (a backslash, the exact letters, then a non-letter) occurs in
/// `text`. A trailing `*` counts, so the starred form is found too.
fn has_command(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    let pat = format!("\\{name}");
    let mut from = 0;
    while let Some(rel) = text[from..].find(&pat) {
        let start = from + rel;
        let after = start + pat.len();
        let next = bytes.get(after).copied();
        match next {
            Some(b) if b.is_ascii_alphabetic() => {}
            _ => return true,
        }
        from = after;
    }
    false
}

/// When to stop a block-parsing loop.
enum Stop<'a> {
    /// End of input.
    Eof,
    /// A matching `\end{name}` (or end of input).
    Env(&'a str),
    /// A `\item`, a matching `\end{name}`, or end of input; used inside a list environment.
    Item(&'a str),
}

/// A user-defined macro: its argument count and replacement text, with `#1`…`#9` marking parameters.
/// When `optional_default` is set, the first parameter is optional and takes this value unless the
/// call supplies a `[…]` argument.
#[derive(Clone)]
struct Macro {
    args: usize,
    optional_default: Option<String>,
    body: String,
}

/// One level of input the cursor reads from: a character buffer and a position within it. The
/// bottom frame holds the source text; each macro expansion pushes a new frame that is read to
/// exhaustion, then popped, so the expansion's characters are consumed in place of the invocation.
struct Frame {
    chars: Vec<char>,
    pos: usize,
}

/// The character cursor and parse state.
#[allow(clippy::struct_excessive_bools)]
struct Parser {
    /// The input-frame stack: the bottom frame is the source text, and each frame above it is a
    /// pending macro expansion still being read. Never empty; the bottom frame is never popped.
    frames: Vec<Frame>,
    ext: Extensions,
    smart: bool,
    meta: BTreeMap<String, MetaValue>,
    /// User macro definitions, shared with sub-parsers by reference; a sub-parser that defines its own
    /// macro copies the table on write, so definitions never leak across scopes.
    macros: Rc<BTreeMap<String, Macro>>,
    ids: IdRegistry,
    /// The level offset for sectioning commands (see [`base_section_level`]).
    base_level: i32,
    /// Whether the current context is a figure body, where an image carries no alt text.
    in_figure: bool,
    /// Whether the current context is a float body, where `\caption` ends a paragraph so it can be
    /// hoisted out as the float's caption.
    in_float: bool,
    /// Current macro-expansion nesting depth (the number of live expansion frames), bounded to stop
    /// runaway recursive expansions.
    expand_depth: u32,
    /// Total macro expansions performed by this parser, bounded to stop a branching macro from doing
    /// exponential work while staying under the nesting cap.
    total_expansions: u32,
    /// Whether the most recently consumed inter-word whitespace contained a newline, so the gap
    /// renders as a soft break rather than a plain space.
    last_ws_had_newline: bool,
}

/// Where an inline run ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InlineStop {
    /// A closing `}`.
    Group,
    /// A closing `]`.
    Bracket,
    /// A block boundary: a blank line, a sectioning command, an environment, or `\par`.
    Paragraph,
    /// A closing single smart quote `'`.
    QuoteSingle,
    /// A closing double smart quote `''`.
    QuoteDouble,
}

/// Bounds macro-expansion nesting depth (how deeply expansions may recurse into one another).
const MAX_EXPAND_DEPTH: u32 = 200;

/// Bounds the total number of expansions one parser may perform (its overall work budget).
const MAX_TOTAL_EXPANSIONS: u32 = 100_000;

impl Parser {
    /// Replaces the frame stack with a single bottom frame over `source`, resetting the cursor and
    /// the expansion nesting depth. Macro definitions and the total-expansion budget carry over.
    fn set_source(&mut self, source: &str) {
        self.frames = vec![Frame {
            chars: source.chars().collect(),
            pos: 0,
        }];
        self.expand_depth = 0;
    }

    /// Pops expansion frames that have been read to exhaustion, decrementing the nesting depth for
    /// each. The bottom frame (the source) is never popped, so at least one frame always remains.
    fn drop_exhausted_frames(&mut self) {
        while self.frames.len() > 1 {
            match self.frames.last() {
                Some(frame) if frame.pos >= frame.chars.len() => {
                    self.frames.pop();
                    self.expand_depth = self.expand_depth.saturating_sub(1);
                }
                _ => break,
            }
        }
    }

    fn cur(&self) -> Option<char> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.chars.get(frame.pos).copied())
    }

    fn at(&self, offset: usize) -> Option<char> {
        let mut remaining = offset;
        for frame in self.frames.iter().rev() {
            let available = frame.chars.len().saturating_sub(frame.pos);
            if remaining < available {
                return frame.chars.get(frame.pos + remaining).copied();
            }
            remaining -= available;
        }
        None
    }

    fn bump(&mut self) -> Option<char> {
        self.drop_exhausted_frames();
        let frame = self.frames.last_mut()?;
        let c = frame.chars.get(frame.pos).copied();
        if c.is_some() {
            frame.pos += 1;
        }
        c
    }

    /// Advances the cursor by `n` characters, crossing frame boundaries as needed.
    fn advance_chars(&mut self, n: usize) {
        for _ in 0..n {
            self.bump();
        }
    }

    fn eof(&self) -> bool {
        self.cur().is_none()
    }

    fn looking_at(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(i, c)| self.at(i) == Some(c))
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.looking_at(s) {
            self.advance_chars(s.chars().count());
            true
        } else {
            false
        }
    }

    // --- Block level -----------------------------------------------------------------------------

    fn parse_blocks(&mut self, stop: &Stop) -> Vec<Block> {
        let mut blocks = Vec::new();
        loop {
            self.skip_block_ws();
            if self.at_stop(stop) {
                break;
            }
            if let Some(mut produced) = self.parse_block_construct() {
                blocks.append(&mut produced);
            } else {
                let para = self.parse_paragraph();
                if !para.is_empty() {
                    blocks.push(Block::Para(para));
                } else if !self.advance_over_stray() {
                    break;
                }
            }
        }
        blocks
    }

    fn at_stop(&self, stop: &Stop) -> bool {
        match stop {
            Stop::Eof => self.eof(),
            Stop::Env(name) => self.eof() || self.at_end_env(name),
            Stop::Item(name) => self.eof() || self.at_end_env(name) || self.at_control_word("item"),
        }
    }

    /// Whether the cursor is at `\end{name}` (tolerating spaces inside the braces).
    fn at_end_env(&self, name: &str) -> bool {
        match self.peek_env_after("\\end") {
            Some(env) => env == name,
            None => false,
        }
    }

    /// Whether the cursor is at the control word `\name` followed by a non-letter.
    fn at_control_word(&self, name: &str) -> bool {
        if self.cur() != Some('\\') {
            return false;
        }
        for (i, c) in name.chars().enumerate() {
            if self.at(1 + i) != Some(c) {
                return false;
            }
        }
        match self.at(1 + name.chars().count()) {
            Some(c) => !c.is_ascii_alphabetic(),
            None => true,
        }
    }

    /// The environment name in `\begin{env}` / `\end{env}` when the cursor is at `prefix`
    /// (`"\\begin"` or `"\\end"`), skipping a `*` and spaces around the braces.
    fn peek_env_after(&self, prefix: &str) -> Option<String> {
        let mut i = 0;
        for c in prefix.chars() {
            if self.at(i) != Some(c) {
                return None;
            }
            i += 1;
        }
        while self.at(i) == Some(' ') {
            i += 1;
        }
        if self.at(i) != Some('{') {
            return None;
        }
        i += 1;
        let mut name = String::new();
        while let Some(c) = self.at(i) {
            if c == '}' {
                return Some(name);
            }
            name.push(c);
            i += 1;
        }
        None
    }

    /// Consumes a single stray character so a stalled loop makes progress. Returns `false` at EOF.
    fn advance_over_stray(&mut self) -> bool {
        // A dangling `\end{env}` for an environment we are not inside is skipped whole.
        if self.looking_at("\\end")
            && let Some(env) = self.peek_env_after("\\end")
        {
            self.consume_env_marker("\\end");
            let _ = env;
            return true;
        }
        // a stray non-block control word (`\item`, `\par`) is dropped whole with a following optional arg
        if self.cur() == Some('\\') && self.at(1).is_some_and(|c| c.is_ascii_alphabetic()) {
            self.bump();
            while self.cur().is_some_and(|c| c.is_ascii_alphabetic()) {
                self.bump();
            }
            let _ = self.read_optional_raw();
            return true;
        }
        self.bump().is_some()
    }

    /// Skips whitespace, blank lines, and comments between blocks.
    fn skip_block_ws(&mut self) {
        loop {
            match self.cur() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('%') => self.skip_comment(),
                _ => break,
            }
        }
    }

    /// Skips a comment: `%` to the end of the line, but not the newline itself.
    fn skip_comment(&mut self) {
        while let Some(c) = self.cur() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    /// Recognises a block-level construct at the cursor. Returns `None` when the cursor is at inline
    /// content that a paragraph should collect.
    fn parse_block_construct(&mut self) -> Option<Vec<Block>> {
        if self.cur() != Some('\\') {
            return None;
        }
        // `\begin{env}`: an environment, unless it is a math environment (which is inline content).
        if let Some(env) = self.peek_env_after("\\begin") {
            if math_env(&env) {
                return None;
            }
            return Some(self.parse_environment(&env));
        }
        // A macro that expands is read from its expansion frame, then re-examined.
        if self.try_expand_macro() {
            return self.parse_block_construct();
        }
        let name = self.peek_control_word()?;
        if let Some(level) = section_intrinsic(&name) {
            return Some(vec![self.parse_section(level)]);
        }
        match name.as_str() {
            "title" | "author" | "date" | "subtitle" => {
                self.consume_control_word();
                self.capture_meta(&name);
                Some(Vec::new())
            }
            "newcommand"
            | "renewcommand"
            | "providecommand"
            | "DeclareRobustCommand"
            | "def"
            | "let" => Some(self.parse_macro_definition(&name)),
            // commands that emit no block are consumed with their arguments and dropped
            "par" | "maketitle" | "tableofcontents" | "listoffigures" | "listoftables"
            | "frontmatter" | "mainmatter" | "backmatter" | "appendix" | "clearpage"
            | "cleardoublepage" | "newpage" | "pagebreak" | "noindent" | "bigskip" | "medskip"
            | "smallskip" | "centering" | "raggedright" | "raggedleft" | "printindex"
            | "printbibliography" | "documentclass" | "usepackage" | "RequirePackage"
            | "pagestyle" | "thispagestyle" | "pagenumbering" | "setlength" | "setcounter"
            | "addtocounter" | "geometry" | "hypersetup" | "bibliographystyle" | "include"
            | "input" | "graphicspath" | "definecolor" | "newtheorem" | "theoremstyle"
            | "captionsetup" | "bibliography" => {
                self.consume_control_word();
                self.skip_command_args(&name);
                Some(Vec::new())
            }
            _ => None,
        }
    }

    /// The control word at the cursor (letters after a `\`), without consuming it. `None` for a
    /// control symbol or a non-command position.
    fn peek_control_word(&self) -> Option<String> {
        if self.cur() != Some('\\') {
            return None;
        }
        let mut i = 1;
        let mut name = String::new();
        while let Some(c) = self.at(i) {
            if c.is_ascii_alphabetic() {
                name.push(c);
                i += 1;
            } else {
                break;
            }
        }
        if name.is_empty() { None } else { Some(name) }
    }

    /// Consumes a `\name` control word and the spaces that follow it (which LaTeX absorbs).
    fn consume_control_word(&mut self) -> String {
        self.bump(); // backslash
        let mut name = String::new();
        while let Some(c) = self.cur() {
            if c.is_ascii_alphabetic() {
                name.push(c);
                self.bump();
            } else {
                break;
            }
        }
        while matches!(self.cur(), Some(' ' | '\t')) {
            self.bump();
        }
        name
    }

    /// Consumes a `\begin`/`\end` marker together with its `{env}` (and any `*` / spaces).
    fn consume_env_marker(&mut self, prefix: &str) {
        self.eat(prefix);
        while self.cur() == Some(' ') {
            self.bump();
        }
        if self.cur() == Some('{') {
            self.bump();
            while let Some(c) = self.cur() {
                self.bump();
                if c == '}' {
                    break;
                }
            }
        }
    }
}
