//! LaTeX reader: parses a LaTeX source document into the document model.
//!
//! LaTeX has no line-oriented block grammar the way lightweight markup does; a source file is a
//! stream of ordinary text, control sequences (`\name` / `\<symbol>`), grouping braces, math shifts,
//! and environments (`\begin{env}` … `\end{env}`). Parsing is a single character-level recursive
//! descent over the source: `Parser::parse_blocks` recognises block-level constructs (sectioning
//! commands, environments, paragraphs) and `Parser::parse_inlines` the inline run inside each,
//! accumulating a text buffer that flushes to [`Inline::Str`] at every space, break, or markup
//! boundary.
//!
//! Only the body between `\begin{document}` and `\end{document}` is rendered when a document
//! environment is present; the preamble is scanned for metadata (`\title`, `\author`, `\date`) and
//! macro definitions but otherwise dropped. A construct that has no faithful model representation
//! degrades: an unknown command is dropped (or passed through verbatim under `raw_tex`), and an
//! unknown environment becomes a classed [`Block::Div`] (or a raw block under `raw_tex`).

use std::collections::BTreeMap;
use std::rc::Rc;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, Citation, CitationMode, ColSpec, ColWidth, Document,
    Format, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType, MetaValue,
    QuoteType, Row, Table, TableBody, TableFoot, TableHead, Target, slug, slug_gfm, to_plain_text,
};
use carta_core::{Extension, Extensions, Reader, ReaderOptions, Result};

use crate::heading_ids::{IdRegistry, IdScheme};

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

    // --- Sectioning ------------------------------------------------------------------------------

    fn parse_section(&mut self, intrinsic: i32) -> Block {
        self.consume_control_word();
        let starred = self.cur() == Some('*');
        if starred {
            self.bump();
            while matches!(self.cur(), Some(' ' | '\t')) {
                self.bump();
            }
        }
        // An optional `[short title]` is ignored.
        let _ = self.read_optional_raw();
        let mut label = None;
        let title = self.parse_group_inlines_capturing_label(&mut label);
        // A `\label` immediately following the heading names it too.
        self.skip_block_ws();
        if let Some(id) = self.peek_env_arg_after_label() {
            label = Some(id);
        }

        let level = (intrinsic - self.base_level + 1).max(1);
        let id = match label {
            Some(id) => {
                self.ids.reserve_native(&id);
                id
            }
            None => self.assign_id(&to_plain_text(&title)),
        };
        let mut classes = Vec::new();
        if starred {
            classes.push("unnumbered".into());
        }
        Block::Header(
            level,
            Box::new(Attr {
                id: id.into(),
                classes,
                attributes: Vec::new(),
            }),
            title,
        )
    }

    /// If the cursor is at `\label{id}`, consumes it and returns the identifier.
    fn peek_env_arg_after_label(&mut self) -> Option<String> {
        if self.at_control_word("label") {
            self.consume_control_word();
            return self.read_group_raw();
        }
        None
    }

    /// Parses a braced inline group, capturing the identifier of any `\label` inside it into `label`.
    fn parse_group_inlines_capturing_label(&mut self, label: &mut Option<String>) -> Vec<Inline> {
        if self.cur() != Some('{') {
            return Vec::new();
        }
        self.bump();
        let inlines = self.parse_inlines(InlineStop::Group);
        if self.cur() == Some('}') {
            self.bump();
        }
        // a `\label` renders as an empty span with a `label` attribute; pull it out as the header id
        let mut kept = Vec::new();
        for inline in inlines {
            if let Inline::Span(attr, content) = &inline
                && content.is_empty()
                && attr.attributes.iter().any(|(k, _)| k == "label")
            {
                *label = Some(attr.id.to_string());
                continue;
            }
            kept.push(inline);
        }
        kept
    }

    /// Derives a heading identifier from its title text. The slug shape follows the active extension,
    /// but a section always disambiguates natively: an empty slug becomes `section` and a repeat
    /// increments a numeric suffix until unused (also avoiding any reserved `\label`).
    fn assign_id(&mut self, text: &str) -> String {
        let Some(scheme) = IdScheme::select(self.ext, false) else {
            return String::new();
        };
        let base = match scheme {
            IdScheme::Plain => slug(text),
            IdScheme::Gfm => slug_gfm(text),
        };
        self.ids.assign_native(base)
    }

    // --- Environments ----------------------------------------------------------------------------

    fn parse_environment(&mut self, env: &str) -> Vec<Block> {
        self.consume_env_marker("\\begin");
        match env {
            "itemize" | "enumerate" => {
                let _ = self.read_optional_raw();
                vec![self.parse_list(env)]
            }
            "description" => {
                let _ = self.read_optional_raw();
                vec![self.parse_description()]
            }
            "quote" | "quotation" | "verse" => {
                let inner = self.parse_blocks(&Stop::Env(env));
                self.consume_env_marker("\\end");
                vec![Block::BlockQuote(inner)]
            }
            "center" | "flushleft" | "flushright" => {
                let inner = self.parse_blocks(&Stop::Env(env));
                self.consume_env_marker("\\end");
                vec![Block::Div(
                    Box::new(Attr {
                        id: carta_ast::Text::default(),
                        classes: vec![env.into()],
                        attributes: Vec::new(),
                    }),
                    inner,
                )]
            }
            "minipage" => {
                // Positional options precede the mandatory width; none affect the content.
                while self.read_optional_raw().is_some() {}
                let _ = self.read_group_raw();
                let inner = self.parse_blocks(&Stop::Env(env));
                self.consume_env_marker("\\end");
                vec![Block::Div(
                    Box::new(Attr {
                        id: carta_ast::Text::default(),
                        classes: vec!["minipage".into()],
                        attributes: Vec::new(),
                    }),
                    inner,
                )]
            }
            "verbatim" | "verbatim*" | "Verbatim" | "lstlisting" | "minted" | "alltt"
            | "lstinputlisting" => self.parse_verbatim_env(env),
            "comment" => {
                self.skip_to_end_env(env);
                Vec::new()
            }
            "figure" | "figure*" | "wrapfigure" | "SCfigure" | "marginfigure" => {
                vec![self.parse_figure(env)]
            }
            "table" | "table*" => self.parse_table_float(env),
            "tabular" | "tabular*" | "tabularx" | "array" | "longtable" | "supertabular"
            | "tabulary" => {
                vec![self.parse_tabular(env)]
            }
            "abstract" => {
                let inner = self.parse_blocks(&Stop::Env(env));
                self.consume_env_marker("\\end");
                self.meta
                    .insert("abstract".to_owned(), MetaValue::MetaBlocks(inner));
                Vec::new()
            }
            "document" => {
                let inner = self.parse_blocks(&Stop::Env(env));
                self.consume_env_marker("\\end");
                inner
            }
            _ => {
                if self.ext.contains(Extension::RawTex) {
                    self.parse_raw_env(env)
                } else {
                    let inner = self.parse_blocks(&Stop::Env(env));
                    self.consume_env_marker("\\end");
                    vec![Block::Div(
                        Box::new(Attr {
                            id: carta_ast::Text::default(),
                            classes: vec![env.into()],
                            attributes: Vec::new(),
                        }),
                        inner,
                    )]
                }
            }
        }
    }

    /// Captures an unknown environment verbatim as a raw LaTeX block (under `raw_tex`).
    fn parse_raw_env(&mut self, env: &str) -> Vec<Block> {
        let mut raw = format!("\\begin{{{env}}}");
        while !self.eof() {
            if self.at_end_env(env) {
                break;
            }
            if let Some(c) = self.bump() {
                raw.push(c);
            }
        }
        raw.push_str("\\end{");
        raw.push_str(env);
        raw.push('}');
        self.consume_env_marker("\\end");
        vec![Block::RawBlock(Format("latex".into()), raw.into())]
    }

    /// Skips to and consumes the matching `\end{env}`.
    fn skip_to_end_env(&mut self, env: &str) {
        while !self.eof() {
            if self.at_end_env(env) {
                break;
            }
            self.bump();
        }
        self.consume_env_marker("\\end");
    }

    // --- Lists -----------------------------------------------------------------------------------

    fn parse_list(&mut self, env: &str) -> Block {
        let items = self.parse_items(env);
        if env == "enumerate" {
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::DefaultDelim,
                },
                items,
            )
        } else {
            Block::BulletList(items)
        }
    }

    /// Reads the `\item` entries of an itemize/enumerate environment.
    fn parse_items(&mut self, env: &str) -> Vec<Vec<Block>> {
        let mut items: Vec<Vec<Block>> = Vec::new();
        loop {
            self.skip_block_ws();
            if self.eof() || self.at_end_env(env) {
                break;
            }
            if self.at_control_word("item") {
                self.consume_control_word();
                let _ = self.read_optional_raw(); // custom marker, dropped
                let blocks = self.parse_blocks(&Stop::Item(env));
                items.push(blocks);
            } else if !self.advance_over_stray() {
                break;
            }
        }
        self.consume_env_marker("\\end");
        items
    }

    fn parse_description(&mut self) -> Block {
        let mut entries: Vec<(Vec<Inline>, Vec<Vec<Block>>)> = Vec::new();
        loop {
            self.skip_block_ws();
            if self.eof() || self.at_end_env("description") {
                break;
            }
            if self.at_control_word("item") {
                self.consume_control_word();
                let term = self.read_optional_inlines().unwrap_or_default();
                let blocks = self.parse_blocks(&Stop::Item("description"));
                entries.push((term, vec![blocks]));
            } else if !self.advance_over_stray() {
                break;
            }
        }
        self.consume_env_marker("\\end");
        Block::DefinitionList(entries)
    }

    // --- Verbatim --------------------------------------------------------------------------------

    fn parse_verbatim_env(&mut self, env: &str) -> Vec<Block> {
        let mut classes = Vec::new();
        let mut attributes = Vec::new();
        // `lstlisting`/`Verbatim` take `[key=value,…]` options; `minted` takes `{language}`.
        if matches!(env, "lstlisting" | "Verbatim" | "lstinputlisting") {
            if let Some(opts) = self.read_optional_raw() {
                for (k, v) in parse_key_values(&opts) {
                    if k == "language" && !v.is_empty() {
                        classes.push(v.to_lowercase().into());
                    }
                    attributes.push((k, v));
                }
            }
        } else if env == "minted" {
            let _ = self.read_optional_raw();
            if let Some(lang) = self.read_group_raw()
                && !lang.is_empty()
            {
                classes.push(lang.to_lowercase().into());
            }
        }
        let content = self.read_verbatim_body(env);
        vec![Block::CodeBlock(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes,
                attributes: attributes
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
            }),
            content.into(),
        )]
    }

    /// Reads a verbatim environment body verbatim, stopping before `\end{env}`.
    fn read_verbatim_body(&mut self, env: &str) -> String {
        let closing = format!("\\end{{{env}}}");
        let mut body = String::new();
        while !self.eof() {
            if self.looking_at(&closing) {
                break;
            }
            if let Some(c) = self.bump() {
                body.push(c);
            }
        }
        self.consume_env_marker("\\end");
        body.trim_matches('\n').to_owned()
    }

    // --- Figures & tables ------------------------------------------------------------------------

    fn parse_figure(&mut self, env: &str) -> Block {
        let _ = self.read_optional_raw(); // float placement
        if env == "wrapfigure" {
            let _ = self.read_group_raw(); // placement
            let _ = self.read_group_raw(); // width
        }
        let was_in_figure = self.in_figure;
        self.in_figure = true;
        let (blocks, caption, id) = self.collect_float(env);
        self.in_figure = was_in_figure;
        Block::Figure(
            Box::new(Attr {
                id: id.unwrap_or_default().into(),
                classes: Vec::new(),
                attributes: Vec::new(),
            }),
            Box::new(Caption {
                short: None,
                long: caption,
            }),
            blocks.into_iter().map(demote_image_para).collect(),
        )
    }

    fn parse_table_float(&mut self, env: &str) -> Vec<Block> {
        let _ = self.read_optional_raw();
        let (mut blocks, caption, id) = self.collect_float(env);
        if !caption.is_empty()
            && let Some(Block::Table(table)) =
                blocks.iter_mut().find(|b| matches!(b, Block::Table(_)))
        {
            table.caption = Caption {
                short: None,
                long: caption,
            };
            if let Some(id) = id {
                table.attr.id = id.into();
            }
        }
        blocks
    }

    /// Parses a float body, pulling out a `\caption` (as caption blocks) and a `\label` (as an id).
    fn collect_float(&mut self, env: &str) -> (Vec<Block>, Vec<Block>, Option<String>) {
        let mut blocks = Vec::new();
        let mut caption = Vec::new();
        let mut id = None;
        let was_in_float = self.in_float;
        self.in_float = true;
        loop {
            self.skip_block_ws();
            if self.eof() || self.at_end_env(env) {
                break;
            }
            if self.at_control_word("caption") {
                self.consume_control_word();
                let _ = self.read_optional_raw();
                let inlines = self.parse_group_inlines_capturing_label(&mut id);
                caption = vec![Block::Plain(inlines)];
                continue;
            }
            if self.at_control_word("centering")
                || self.at_control_word("small")
                || self.at_control_word("footnotesize")
            {
                self.consume_control_word();
                continue;
            }
            if self.at_control_word("label") {
                self.consume_control_word();
                id = self.read_group_raw();
                continue;
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
        self.consume_env_marker("\\end");
        self.in_float = was_in_float;
        (blocks, caption, id)
    }

    fn parse_tabular(&mut self, env: &str) -> Block {
        if env == "tabular*" || env == "tabularx" || env == "tabulary" {
            let _ = self.read_group_raw(); // width
        }
        let _ = self.read_optional_raw(); // vertical position
        let spec = self.read_group_raw().unwrap_or_default();
        let aligns = parse_column_spec(&spec);
        let body = self.read_environment_source(env);
        self.consume_env_marker("\\end");
        build_table(self, &aligns, &body)
    }

    /// Reads a math environment as a single math inline. `math`/`displaymath` carry the body alone;
    /// the aligned and numbered environments carry their `\begin`/`\end` wrapper inside the formula.
    fn read_math_environment(&mut self, env: &str) -> Inline {
        self.consume_env_marker("\\begin");
        let body = self.read_environment_source(env);
        self.consume_env_marker("\\end");
        match env {
            "math" => Inline::Math(MathType::InlineMath, body.trim().into()),
            "displaymath" => Inline::Math(MathType::DisplayMath, body.trim().into()),
            _ => {
                // markers re-emitted on their own lines; trailing whitespace stripped, first-line indent kept
                let content = body.trim_end().trim_start_matches(['\n', '\r']);
                Inline::Math(
                    MathType::DisplayMath,
                    format!("\\begin{{{env}}}\n{content}\n\\end{{{env}}}").into(),
                )
            }
        }
    }

    /// Reads the raw source of an environment body up to (but not consuming) its `\end{env}`.
    fn read_environment_source(&mut self, env: &str) -> String {
        let closing = format!("\\end{{{env}}}");
        let mut out = String::new();
        while !self.eof() {
            if self.looking_at(&closing) {
                break;
            }
            if let Some(c) = self.bump() {
                out.push(c);
            }
        }
        out
    }

    // --- Paragraph & inline ----------------------------------------------------------------------

    fn parse_paragraph(&mut self) -> Vec<Inline> {
        let inlines = self.parse_inlines(InlineStop::Paragraph);
        trim_inlines(inlines)
    }

    fn parse_inlines(&mut self, stop: InlineStop) -> Vec<Inline> {
        let mut out = Vec::new();
        let mut buf = String::new();
        loop {
            let Some(c) = self.cur() else {
                break;
            };
            match stop {
                InlineStop::Group if c == '}' => break,
                InlineStop::Bracket if c == ']' => break,
                InlineStop::QuoteSingle if c == '\'' && self.quote_closes(1) => break,
                InlineStop::QuoteDouble
                    if c == '\'' && self.at(1) == Some('\'') && self.quote_closes(2) =>
                {
                    break;
                }
                _ => {}
            }
            match c {
                ' ' | '\t' | '\n' | '\r' | '%' => {
                    let had_blank = self.consume_inline_ws();
                    if had_blank && matches!(stop, InlineStop::Paragraph) {
                        break;
                    }
                    flush_buf(&mut buf, &mut out);
                    if !self.eof() {
                        let ws = if had_blank || self.last_ws_had_newline {
                            Inline::SoftBreak
                        } else {
                            Inline::Space
                        };
                        push_whitespace(&mut out, ws);
                    }
                }
                '\\' => {
                    if matches!(stop, InlineStop::Paragraph) && self.inline_break_ahead() {
                        break;
                    }
                    if let Some(env) = self.peek_env_after("\\begin")
                        && math_env(&env)
                    {
                        let math = self.read_math_environment(&env);
                        emit(&mut out, &mut buf, math);
                        continue;
                    }
                    if self.try_expand_macro() {
                        continue;
                    }
                    // A font-switch command applies to the remainder of the enclosing group.
                    if let Some(word) = self.peek_control_word()
                        && let Some(switch) = switch_kind(&word)
                    {
                        self.apply_switch(switch, stop, &mut out, &mut buf);
                        break;
                    }
                    // a command flushes the buffer only when it emits an inline; accents and
                    // symbols append so they join the surrounding word
                    self.exec_control(&mut out, &mut buf);
                }
                '{' => {
                    self.bump();
                    let inner = self.parse_inlines(InlineStop::Group);
                    if self.cur() == Some('}') {
                        self.bump();
                    }
                    // an empty group keeps the word intact; a non-empty one becomes a grouping span
                    if let Some(span) = group_span(inner) {
                        emit(&mut out, &mut buf, span);
                    }
                }
                '}' => {
                    // A stray close brace outside a group is treated as a literal.
                    buf.push('}');
                    self.bump();
                }
                '$' => {
                    flush_buf(&mut buf, &mut out);
                    let math = self.read_dollar_math();
                    out.push(math);
                }
                '~' => {
                    buf.push('\u{a0}');
                    self.bump();
                }
                '-' => {
                    self.read_dashes(&mut buf);
                }
                '`' if self.smart => {
                    flush_buf(&mut buf, &mut out);
                    self.read_open_quote(&mut out);
                }
                '\'' => {
                    self.read_apostrophe(&mut buf);
                }
                _ => {
                    buf.push(c);
                    self.bump();
                }
            }
        }
        flush_buf(&mut buf, &mut out);
        out
    }

    /// Whether `\`-introduced content at the cursor starts a new block, ending a paragraph.
    fn inline_break_ahead(&self) -> bool {
        if let Some(env) = self.peek_env_after("\\begin") {
            return !math_env(&env);
        }
        if self.looking_at("\\end") && self.peek_env_after("\\end").is_some() {
            return true;
        }
        if let Some(word) = self.peek_control_word() {
            if section_intrinsic(&word).is_some() {
                return true;
            }
            if self.in_float && word == "caption" {
                return true;
            }
            return matches!(word.as_str(), "item" | "par");
        }
        false
    }

    /// Whether a smart-quote delimiter at `offset` from the cursor closes an open quote: it does
    /// when the character after it is not alphanumeric.
    fn quote_closes(&self, offset: usize) -> bool {
        match self.at(offset) {
            Some(c) => !c.is_alphanumeric(),
            None => true,
        }
    }

    /// Consumes a run of whitespace and comments. Returns whether it spanned a blank line. Records
    /// whether the run contained a newline in `last_ws_had_newline`.
    fn consume_inline_ws(&mut self) -> bool {
        let mut newlines = 0u32;
        loop {
            match self.cur() {
                Some('\n') => {
                    newlines += 1;
                    self.bump();
                }
                Some(' ' | '\t' | '\r') => {
                    self.bump();
                }
                Some('%') => self.skip_comment(),
                _ => break,
            }
        }
        self.last_ws_had_newline = newlines > 0;
        newlines >= 2
    }

    fn read_dashes(&mut self, buf: &mut String) {
        let mut count = 0;
        while self.cur() == Some('-') {
            count += 1;
            self.bump();
        }
        while count >= 3 {
            buf.push('\u{2014}');
            count -= 3;
        }
        if count == 2 {
            buf.push('\u{2013}');
        } else if count == 1 {
            buf.push('-');
        }
    }

    fn read_apostrophe(&mut self, buf: &mut String) {
        if self.cur() == Some('\'') && self.at(1) == Some('\'') {
            buf.push('\u{201d}');
            self.bump();
            self.bump();
        } else {
            buf.push('\u{2019}');
            self.bump();
        }
    }

    /// Opens a smart quote at a `` ` ``, reading its content up to the matching close.
    fn read_open_quote(&mut self, out: &mut Vec<Inline>) {
        if self.at(1) == Some('`') {
            self.bump();
            self.bump();
            let inner = self.parse_inlines(InlineStop::QuoteDouble);
            if self.cur() == Some('\'') && self.at(1) == Some('\'') {
                self.bump();
                self.bump();
                out.push(Inline::Quoted(QuoteType::DoubleQuote, inner));
            } else {
                out.push(Inline::Str("\u{201c}".into()));
                out.extend(inner);
            }
        } else {
            self.bump();
            let inner = self.parse_inlines(InlineStop::QuoteSingle);
            if self.cur() == Some('\'') {
                self.bump();
                out.push(Inline::Quoted(QuoteType::SingleQuote, inner));
            } else {
                out.push(Inline::Str("\u{2018}".into()));
                out.extend(inner);
            }
        }
    }

    // --- Inline commands -------------------------------------------------------------------------

    /// Dispatches a control sequence in inline context, appending inlines to `out` or text to `buf`.
    fn exec_control(&mut self, out: &mut Vec<Inline>, buf: &mut String) {
        // A control symbol: a backslash followed by a single non-letter.
        if self.at(1).is_some_and(|c| !c.is_ascii_alphabetic()) {
            self.exec_control_symbol(out, buf);
            return;
        }
        let name = self.consume_control_word();
        self.exec_named(&name, out, buf);
    }

    fn exec_control_symbol(&mut self, out: &mut Vec<Inline>, buf: &mut String) {
        self.bump(); // backslash
        let Some(c) = self.bump() else {
            return;
        };
        match c {
            '\\' => {
                // hard line break: `*` and `[dimen]` discarded, surrounding spacing absorbed
                if self.cur() == Some('*') {
                    self.bump();
                }
                let _ = self.read_optional_raw();
                flush_buf(buf, out);
                while matches!(out.last(), Some(Inline::Space | Inline::SoftBreak)) {
                    out.pop();
                }
                out.push(Inline::LineBreak);
            }
            '[' => {
                let text = self.read_math_body("\\]");
                emit(out, buf, Inline::Math(MathType::DisplayMath, text.into()));
            }
            '(' => {
                let text = self.read_math_body("\\)");
                emit(out, buf, Inline::Math(MathType::InlineMath, text.into()));
            }
            // An explicit inter-word space is a non-breaking space.
            ' ' | '\n' | '\t' => buf.push('\u{a0}'),
            // A thin space.
            ',' => buf.push('\u{2006}'),
            '&' | '%' | '#' | '$' | '_' | '{' | '}' => buf.push(c),
            '~' => self.read_accent_symbol(Accent::Tilde, buf),
            '^' => self.read_accent_symbol(Accent::Circumflex, buf),
            '\'' => self.read_accent_symbol(Accent::Acute, buf),
            '`' => self.read_accent_symbol(Accent::Grave, buf),
            '"' => self.read_accent_symbol(Accent::Diaeresis, buf),
            '=' => self.read_accent_symbol(Accent::Macron, buf),
            '.' => self.read_accent_symbol(Accent::DotAbove, buf),
            // Discretionary/zero-width spacing and escaped delimiters that carry no text.
            '-' | '/' | ';' | ':' | '!' | '@' | ')' | ']' => {}
            other => buf.push(other),
        }
    }

    /// Applies a font-switch command (`\bf`, `\em`, …) to the remainder of the enclosing group.
    fn apply_switch(
        &mut self,
        switch: Switch,
        stop: InlineStop,
        out: &mut Vec<Inline>,
        buf: &mut String,
    ) {
        self.consume_control_word();
        flush_buf(buf, out);
        let rest = self.parse_inlines(stop);
        if matches!(switch, Switch::Code) {
            out.push(switch.wrap(rest));
        } else {
            out.extend(extract_spaces(rest, |i| switch.wrap(i)));
        }
    }

    #[allow(clippy::too_many_lines)]
    fn exec_named(&mut self, name: &str, out: &mut Vec<Inline>, buf: &mut String) {
        // Wrapping formatters. Most pull surrounding spacing out of the wrapper; underline keeps it.
        if let Some(wrap) = inline_wrapper(name) {
            let inner = self.parse_group_inlines();
            if matches!(name, "underline" | "uline") {
                emit(out, buf, wrap(inner));
            } else {
                emit_all(out, buf, extract_spaces(inner, wrap));
            }
            return;
        }
        // Accent commands spelled as control words apply to their argument's base character.
        if let Some(accent) = word_accent(name) {
            self.read_accent_symbol(accent, buf);
            return;
        }
        // Font family/shape/series switches wrap their argument in a single-class span.
        if let Some(class) = font_span_class(name) {
            let inner = self.parse_group_inlines();
            emit_all(out, buf, extract_spaces(inner, |i| span_class(i, class)));
            return;
        }
        match name {
            "textcolor" | "colorbox" => {
                let color = self.read_group_raw().unwrap_or_default();
                let inner = self.parse_group_inlines();
                let property = if name == "colorbox" {
                    "background-color"
                } else {
                    "color"
                };
                let attr = Attr {
                    id: carta_ast::Text::default(),
                    classes: Vec::new(),
                    attributes: vec![(
                        "style".into(),
                        format!("{property}: {}", color.trim()).into(),
                    )],
                };
                emit(out, buf, Inline::Span(Box::new(attr), inner));
            }
            "texttt" | "lstinline" => {
                if name == "lstinline" {
                    let _ = self.read_optional_raw();
                }
                let inner = self.parse_group_inlines();
                emit(
                    out,
                    buf,
                    Inline::Code(Box::default(), to_plain_text(&inner).into()),
                );
            }
            "verb" => {
                if let Some(code) = self.read_verb() {
                    emit(out, buf, Inline::Code(Box::default(), code.into()));
                }
            }
            "footnote" | "footnotetext" | "thanks" => {
                let _ = self.read_optional_raw();
                let blocks = self.parse_group_blocks();
                emit(out, buf, Inline::Note(blocks));
            }
            "url" | "nolinkurl" => {
                if let Some(url) = self.read_group_raw() {
                    let url = unescape_url(&url);
                    emit(
                        out,
                        buf,
                        Inline::Link(
                            Box::new(Attr {
                                id: carta_ast::Text::default(),
                                classes: vec!["uri".into()],
                                attributes: Vec::new(),
                            }),
                            vec![Inline::Str(url.clone().into())],
                            Box::new(Target {
                                url: url.into(),
                                title: carta_ast::Text::default(),
                            }),
                        ),
                    );
                }
            }
            "href" => {
                let url = self
                    .read_group_raw()
                    .map(|u| unescape_url(&u))
                    .unwrap_or_default();
                let text = self.parse_group_inlines();
                emit(
                    out,
                    buf,
                    Inline::Link(
                        Box::default(),
                        text,
                        Box::new(Target {
                            url: url.into(),
                            title: carta_ast::Text::default(),
                        }),
                    ),
                );
            }
            "includegraphics" => {
                let opts = self.read_optional_raw().unwrap_or_default();
                let path = self.read_group_raw().unwrap_or_default();
                let attributes = image_attributes(&opts);
                let alt = if self.in_figure {
                    Vec::new()
                } else {
                    vec![Inline::Str("image".into())]
                };
                emit(
                    out,
                    buf,
                    Inline::Image(
                        Box::new(Attr {
                            id: carta_ast::Text::default(),
                            classes: Vec::new(),
                            attributes: attributes
                                .into_iter()
                                .map(|(k, v)| (k.into(), v.into()))
                                .collect(),
                        }),
                        alt,
                        Box::new(Target {
                            url: path.into(),
                            title: carta_ast::Text::default(),
                        }),
                    ),
                );
            }
            "label" => {
                if let Some(id) = self.read_group_raw() {
                    emit(
                        out,
                        buf,
                        Inline::Span(
                            Box::new(Attr {
                                id: id.clone().into(),
                                classes: Vec::new(),
                                attributes: vec![("label".into(), id.into())],
                            }),
                            Vec::new(),
                        ),
                    );
                }
            }
            "ref" | "eqref" | "autoref" | "cref" | "Cref" => {
                if let Some(target) = self.read_group_raw() {
                    emit(out, buf, reference_link(name, &target));
                }
            }
            "cite" | "citep" | "citet" | "citealp" | "citealt" | "citeauthor" | "citeyear"
            | "parencite" | "textcite" | "footcite" | "autocite" => {
                flush_buf(buf, out);
                self.read_citation(name, out);
            }
            "textsuperscript" | "textsubscript" => {
                let inner = self.parse_group_inlines();
                let wrap: fn(Vec<Inline>) -> Inline = if name == "textsubscript" {
                    Inline::Subscript
                } else {
                    Inline::Superscript
                };
                emit_all(out, buf, extract_spaces(inner, wrap));
            }
            "mbox" | "hbox" => {
                let inner = self.parse_group_inlines();
                emit_all(out, buf, inner);
            }
            "ensuremath" => {
                let body = self.read_group_raw().unwrap_or_default();
                emit(
                    out,
                    buf,
                    Inline::Math(MathType::InlineMath, body.trim().into()),
                );
            }
            "footnotemark" | "protect" | "noindent" | "indent" | "bigskip" | "medskip"
            | "smallskip" | "centering" | "hfill" | "hrulefill" | "dotfill" | "par"
            | "displaystyle" | "scriptsize" | "small" | "footnotesize" | "large" | "Large"
            | "LARGE" | "huge" | "Huge" | "normalsize" | "rmfamily" | "sffamily" | "ttfamily"
            | "mdseries" | "upshape" | "normalfont" | "sc" | "rm" | "sf" | "boldmath"
            | "unboldmath" | "clearpage" | "newpage" | "nolinebreak" | "sloppy" | "raggedright"
            | "item" => {
                // no-argument font-switch/spacing commands contribute nothing; a stray `\item` is dropped
            }
            "linebreak" => {
                let _ = self.read_optional_raw();
                emit(out, buf, Inline::LineBreak);
            }
            "newline" => emit(out, buf, Inline::LineBreak),
            "hspace" | "vspace" | "hskip" | "vskip" | "setlength" | "vphantom" | "hphantom"
            | "phantom" | "rule" | "settowidth" => {
                self.skip_command_args(name);
            }
            _ => {
                if let Some(text) = symbol_text(name) {
                    buf.push_str(text);
                } else if self.ext.contains(Extension::RawTex) {
                    let raw = self.reconstruct_command(name);
                    emit(
                        out,
                        buf,
                        Inline::RawInline(Format("latex".into()), raw.into()),
                    );
                } else {
                    // Unknown command: drop it along with any adjacent bracket/brace arguments.
                    self.skip_adjacent_arguments();
                }
            }
        }
    }

    /// Rebuilds an unknown command's source, including any immediately following optional and braced
    /// arguments, for verbatim passthrough.
    fn reconstruct_command(&mut self, name: &str) -> String {
        let mut raw = format!("\\{name}");
        loop {
            match self.cur() {
                Some('[') => {
                    if let Some(opt) = self.read_optional_raw() {
                        raw.push('[');
                        raw.push_str(&opt);
                        raw.push(']');
                    } else {
                        break;
                    }
                }
                Some('{') => {
                    if let Some(arg) = self.read_group_raw() {
                        raw.push('{');
                        raw.push_str(&arg);
                        raw.push('}');
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        raw
    }

    fn read_citation(&mut self, name: &str, out: &mut Vec<Inline>) {
        // one bracketed arg is the trailing note; with two, the first precedes the key
        let opt1 = self.read_optional_raw();
        let opt2 = self.read_optional_raw();
        let keys_raw = self.read_group_raw().unwrap_or_default();
        let (prefix_raw, suffix_raw) = match (&opt1, &opt2) {
            (Some(pre), Some(post)) => (Some(pre.as_str()), Some(post.as_str())),
            (Some(post), None) => (None, Some(post.as_str())),
            _ => (None, None),
        };
        let prefix = prefix_raw
            .map(|s| self.parse_fragment(s))
            .unwrap_or_default();
        let suffix = suffix_raw
            .map(|s| self.parse_fragment(s))
            .unwrap_or_default();
        let mode = if matches!(name, "citet" | "textcite" | "citeauthor") {
            CitationMode::AuthorInText
        } else {
            CitationMode::NormalCitation
        };
        let mut citations = Vec::new();
        for key in keys_raw.split(',') {
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            citations.push(Citation {
                id: key.into(),
                prefix: Vec::new(),
                suffix: Vec::new(),
                mode: mode.clone(),
                note_num: 0,
                hash: 0,
            });
        }
        if citations.is_empty() {
            return;
        }
        if let Some(first) = citations.first_mut() {
            first.prefix = prefix;
        }
        if let Some(last) = citations.last_mut() {
            last.suffix = suffix;
        }
        let mut raw = format!("\\{name}");
        for opt in [&opt1, &opt2].into_iter().flatten() {
            raw.push('[');
            raw.push_str(opt);
            raw.push(']');
        }
        raw.push('{');
        raw.push_str(&keys_raw);
        raw.push('}');
        out.push(Inline::Cite(
            citations,
            vec![Inline::RawInline(Format("latex".into()), raw.into())],
        ));
    }

    /// Reads `\verb<delim>…<delim>` (or `\verb*…`) verbatim.
    fn read_verb(&mut self) -> Option<String> {
        if self.cur() == Some('*') {
            self.bump();
        }
        let delim = self.bump()?;
        let mut code = String::new();
        while let Some(c) = self.cur() {
            self.bump();
            if c == delim {
                break;
            }
            code.push(c);
        }
        Some(code)
    }

    /// A sub-parser over `source` that inherits the shared context (extensions, smart mode, macro
    /// table, section base level, expansion depth) but starts with fresh cursor and output state
    /// (metadata and heading ids). It never inherits float context.
    fn child(&self, source: &str, in_figure: bool) -> Parser {
        Parser {
            frames: vec![Frame {
                chars: source.chars().collect(),
                pos: 0,
            }],
            ext: self.ext,
            smart: self.smart,
            meta: BTreeMap::new(),
            macros: Rc::clone(&self.macros),
            ids: IdRegistry::default(),
            base_level: self.base_level,
            in_figure,
            in_float: false,
            expand_depth: self.expand_depth,
            total_expansions: 0,
            last_ws_had_newline: false,
        }
    }

    fn parse_group_blocks(&mut self) -> Vec<Block> {
        if self.cur() != Some('{') {
            return Vec::new();
        }
        // Slice out the balanced group and parse it as a sub-document so paragraph breaks work.
        let source = self.read_group_raw().unwrap_or_default();
        let mut sub = self.child(&source, self.in_figure);
        sub.parse_blocks(&Stop::Eof)
    }

    fn parse_group_inlines(&mut self) -> Vec<Inline> {
        if self.cur() != Some('{') {
            return Vec::new();
        }
        self.bump();
        let inner = self.parse_inlines(InlineStop::Group);
        if self.cur() == Some('}') {
            self.bump();
        }
        inner
    }

    // --- Math ------------------------------------------------------------------------------------

    fn read_dollar_math(&mut self) -> Inline {
        if self.cur() == Some('$') && self.at(1) == Some('$') {
            self.bump();
            self.bump();
            Inline::Math(MathType::DisplayMath, self.read_math_body("$$").into())
        } else {
            self.bump();
            Inline::Math(MathType::InlineMath, self.read_math_body("$").into())
        }
    }

    /// Reads math source up to and consuming `close`, then trims surrounding whitespace.
    fn read_math_body(&mut self, close: &str) -> String {
        let mut text = String::new();
        while !self.eof() {
            if self.looking_at(close) {
                self.advance_chars(close.chars().count());
                break;
            }
            // A backslash escape keeps its following character, so `\$` does not end `$` math.
            if self.cur() == Some('\\') {
                if let Some(c) = self.bump() {
                    text.push(c);
                }
                if let Some(c) = self.bump() {
                    text.push(c);
                }
                continue;
            }
            if let Some(c) = self.bump() {
                text.push(c);
            }
        }
        text.trim().to_owned()
    }

    // --- Accents & arguments ---------------------------------------------------------------------

    fn read_accent_symbol(&mut self, accent: Accent, buf: &mut String) {
        let base = self.read_accent_argument();
        buf.push_str(&apply_accent(accent, base.as_deref()));
    }

    /// Reads an accent's argument: a braced group, a control sequence (e.g. `\i` for a dotless i), or
    /// the next single character. The result is the accent's base text.
    fn read_accent_argument(&mut self) -> Option<String> {
        while matches!(self.cur(), Some(' ' | '\t')) {
            self.bump();
        }
        match self.cur() {
            Some('{') => {
                let raw = self.read_group_raw().unwrap_or_default();
                Some(resolve_accent_base(raw.trim()))
            }
            Some('\\') if self.at(1).is_some_and(|c| c.is_ascii_alphabetic()) => {
                let word = self.consume_control_word();
                Some(symbol_text(&word).map_or(word.clone(), str::to_owned))
            }
            Some('\\') => {
                self.bump();
                self.bump().map(|c| c.to_string())
            }
            _ => self.bump().map(|c| c.to_string()),
        }
    }

    fn read_group_raw(&mut self) -> Option<String> {
        while matches!(self.cur(), Some(' ' | '\t')) {
            self.bump();
        }
        if self.cur() != Some('{') {
            return None;
        }
        self.bump();
        let mut depth = 1;
        let mut s = String::new();
        while let Some(c) = self.cur() {
            match c {
                '{' => {
                    depth += 1;
                    s.push(c);
                    self.bump();
                }
                '}' => {
                    depth -= 1;
                    self.bump();
                    if depth == 0 {
                        break;
                    }
                    s.push('}');
                }
                '\\' => {
                    s.push(c);
                    self.bump();
                    if let Some(n) = self.cur() {
                        s.push(n);
                        self.bump();
                    }
                }
                _ => {
                    s.push(c);
                    self.bump();
                }
            }
        }
        Some(s)
    }

    fn read_optional_raw(&mut self) -> Option<String> {
        if self.cur() != Some('[') {
            return None;
        }
        self.bump();
        let mut depth = 0;
        let mut s = String::new();
        while let Some(c) = self.cur() {
            match c {
                '{' => {
                    depth += 1;
                    s.push(c);
                    self.bump();
                }
                '}' => {
                    if depth > 0 {
                        depth -= 1;
                    }
                    s.push(c);
                    self.bump();
                }
                ']' if depth == 0 => {
                    self.bump();
                    break;
                }
                _ => {
                    s.push(c);
                    self.bump();
                }
            }
        }
        Some(s)
    }

    fn read_optional_inlines(&mut self) -> Option<Vec<Inline>> {
        if self.cur() != Some('[') {
            return None;
        }
        self.bump();
        let inner = self.parse_inlines(InlineStop::Bracket);
        if self.cur() == Some(']') {
            self.bump();
        }
        Some(inner)
    }

    /// Consumes the arguments of a command whose output is dropped: any optional `[…]` groups
    /// followed by the number of braced groups the command name is known to take.
    fn skip_command_args(&mut self, name: &str) {
        while self.cur() == Some('[') {
            let _ = self.read_optional_raw();
        }
        for _ in 0..command_arg_count(name) {
            while self.cur() == Some('[') {
                let _ = self.read_optional_raw();
            }
            if self.read_group_raw().is_none() {
                break;
            }
        }
    }

    /// Consumes the optional and braced argument groups directly following a command, stopping at the
    /// first space or other token. Used to swallow an unknown command's arguments.
    fn skip_adjacent_arguments(&mut self) {
        loop {
            match self.cur() {
                Some('[') => {
                    if self.read_optional_raw().is_none() {
                        break;
                    }
                }
                Some('{') => {
                    if self.read_group_raw().is_none() {
                        break;
                    }
                }
                _ => break,
            }
        }
    }

    /// Captures a `\title`/`\author`/`\date`-family command's argument as document metadata.
    fn capture_meta(&mut self, name: &str) {
        let _ = self.read_optional_raw();
        if name == "author" {
            // Authors are `\and`-separated and stored as a list of inline sequences, one per author.
            let raw = self.read_group_raw().unwrap_or_default();
            let authors: Vec<MetaValue> = split_on_command(&raw, "and")
                .into_iter()
                .filter(|part| !part.trim().is_empty())
                .map(|part| MetaValue::MetaInlines(self.parse_fragment(part.trim())))
                .collect();
            self.meta
                .insert("author".to_owned(), MetaValue::MetaList(authors));
            return;
        }
        let inlines = self.parse_group_inlines();
        self.meta
            .insert(name.to_owned(), MetaValue::MetaInlines(inlines));
    }

    /// Parses a self-contained fragment of LaTeX source into inlines with a fresh sub-parser.
    fn parse_fragment(&self, source: &str) -> Vec<Inline> {
        parse_cell_inlines(self, source)
    }

    // --- Macros ----------------------------------------------------------------------------------

    /// Parses a macro definition. With macro expansion enabled the definition is recorded for later
    /// expansion and contributes no block; with it disabled the definition is left in the output as a
    /// raw LaTeX block, preserving its source verbatim.
    fn parse_macro_definition(&mut self, name: &str) -> Vec<Block> {
        // verbatim capture runs only with `LatexMacros` off: no expansion frame is ever pushed, so
        // `start` indexes the same (sole) buffer as the final position
        let start = self.frames.last().map_or(0, |frame| frame.pos);
        self.consume_control_word();
        if self.cur() == Some('*') {
            self.bump();
        }
        if name == "def" {
            self.parse_def();
        } else if name == "let" {
            // `\let\a\b` / `\let\a=\b`: operands consumed, binding not modelled
            let _ = self.take_defined_name();
            if self.cur() == Some('=') {
                self.bump();
            }
            if self.peek_control_word().is_some() {
                self.consume_control_word();
            }
        } else if let Some(macro_name) = self.take_defined_name() {
            let args = self
                .read_optional_raw()
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let optional_default = self.read_optional_raw();
            let body = self.read_group_raw().unwrap_or_default();
            Rc::make_mut(&mut self.macros).insert(
                macro_name,
                Macro {
                    args,
                    optional_default,
                    body,
                },
            );
        }
        if self.ext.contains(Extension::LatexMacros) {
            return Vec::new();
        }
        let raw: String = self
            .frames
            .last()
            .and_then(|frame| frame.chars.get(start..frame.pos))
            .unwrap_or_default()
            .iter()
            .collect();
        vec![Block::RawBlock(Format("latex".into()), raw.into())]
    }

    /// Reads a `\newcommand`-style target name, whether written `{\name}` or bare `\name`.
    fn take_defined_name(&mut self) -> Option<String> {
        while matches!(self.cur(), Some(' ' | '\t')) {
            self.bump();
        }
        if self.cur() == Some('{') {
            let raw = self.read_group_raw()?;
            Some(raw.trim().trim_start_matches('\\').to_owned())
        } else if self.cur() == Some('\\') {
            Some(self.consume_control_word())
        } else {
            None
        }
    }

    fn parse_def(&mut self) {
        let Some(macro_name) = self.take_defined_name() else {
            return;
        };
        // Skip a simple parameter text (e.g. `#1#2`) up to the opening brace.
        let mut args = 0usize;
        while let Some(c) = self.cur() {
            if c == '{' {
                break;
            }
            if c == '#' {
                args += 1;
                self.bump();
                self.bump();
            } else {
                self.bump();
            }
        }
        let body = self.read_group_raw().unwrap_or_default();
        Rc::make_mut(&mut self.macros).insert(
            macro_name,
            Macro {
                args,
                optional_default: None,
                body,
            },
        );
    }

    /// If the cursor is at a user macro invocation, pushes its expansion as a new input frame for the
    /// cursor to read next. Returns whether an expansion occurred.
    fn try_expand_macro(&mut self) -> bool {
        if !self.ext.contains(Extension::LatexMacros)
            || self.expand_depth >= MAX_EXPAND_DEPTH
            || self.total_expansions >= MAX_TOTAL_EXPANSIONS
        {
            return false;
        }
        let Some(name) = self.peek_control_word() else {
            return false;
        };
        let macros = Rc::clone(&self.macros);
        let Some(mac) = macros.get(&name) else {
            return false;
        };
        self.consume_control_word();
        let mut args = Vec::new();
        let mut mandatory = mac.args;
        if let Some(default) = &mac.optional_default {
            let first = if self.cur() == Some('[') {
                self.read_optional_raw().unwrap_or_default()
            } else {
                default.clone()
            };
            args.push(first);
            mandatory = mandatory.saturating_sub(1);
        }
        for _ in 0..mandatory {
            match self.read_macro_arg() {
                Some(a) => args.push(a),
                None => args.push(String::new()),
            }
        }
        // arguments are consumed before the frame is pushed: `#n` sees the invocation's own
        // arguments and the cursor resumes past them once the frame is exhausted
        let expanded = substitute_macro(&mac.body, &args);
        if !expanded.is_empty() {
            self.frames.push(Frame {
                chars: expanded.chars().collect(),
                pos: 0,
            });
            self.expand_depth += 1;
        }
        self.total_expansions += 1;
        true
    }

    /// Reads a single macro argument: a braced group, or the next single token.
    fn read_macro_arg(&mut self) -> Option<String> {
        while matches!(self.cur(), Some(' ' | '\t' | '\n')) {
            self.bump();
        }
        if self.cur() == Some('{') {
            self.read_group_raw()
        } else if self.cur() == Some('\\') {
            Some(format!("\\{}", self.consume_control_word()))
        } else {
            self.bump().map(|c| c.to_string())
        }
    }
}

// --- Free helpers --------------------------------------------------------------------------------

fn flush_buf(buf: &mut String, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        out.push(Inline::Str(std::mem::take(buf).into()));
    }
}

/// Flushes the pending text buffer, then appends an inline, so buffered text stays ahead of it.
fn emit(out: &mut Vec<Inline>, buf: &mut String, inline: Inline) {
    flush_buf(buf, out);
    out.push(inline);
}

fn emit_all(out: &mut Vec<Inline>, buf: &mut String, inlines: Vec<Inline>) {
    flush_buf(buf, out);
    out.extend(inlines);
}

/// Appends a whitespace break, coalescing runs of spacing (which arise around dropped commands)
/// into a single break, with a soft line break taking precedence over a plain space.
fn push_whitespace(out: &mut Vec<Inline>, ws: Inline) {
    match out.last() {
        // A trailing plain space is promoted when the new break is soft.
        Some(Inline::Space) if matches!(ws, Inline::SoftBreak) => {
            if let Some(last) = out.last_mut() {
                *last = Inline::SoftBreak;
            }
        }
        // Any existing break or space already separates; further spacing is swallowed.
        Some(Inline::LineBreak | Inline::SoftBreak | Inline::Space) => {}
        _ => out.push(ws),
    }
}

fn trim_inlines(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.remove(0);
    }
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
    inlines
}

/// The intrinsic nesting level of a sectioning command, before the document-wide level offset:
/// `\part` is -1, `\chapter` 0, `\section` 1, and so on down to `\subparagraph` at 5.
fn section_intrinsic(name: &str) -> Option<i32> {
    match name {
        "part" => Some(-1),
        "chapter" => Some(0),
        "section" => Some(1),
        "subsection" => Some(2),
        "subsubsection" => Some(3),
        "paragraph" => Some(4),
        "subparagraph" => Some(5),
        _ => None,
    }
}

/// Whether `env` typesets mathematics and so is inline content rather than a block environment.
fn math_env(env: &str) -> bool {
    matches!(
        env,
        "math"
            | "displaymath"
            | "equation"
            | "equation*"
            | "align"
            | "align*"
            | "alignat"
            | "alignat*"
            | "gather"
            | "gather*"
            | "multline"
            | "multline*"
            | "flalign"
            | "flalign*"
            | "eqnarray"
            | "eqnarray*"
            | "split"
            | "cases"
    )
}

/// The formatter that wraps a braced group's inlines, for the simple font/emphasis commands.
fn inline_wrapper(name: &str) -> Option<fn(Vec<Inline>) -> Inline> {
    match name {
        "emph" | "textit" | "textsl" | "italic" | "emphasize" => Some(Inline::Emph),
        "textbf" | "strong" => Some(Inline::Strong),
        "underline" | "uline" => Some(Inline::Underline),
        "textsc" => Some(Inline::SmallCaps),
        "sout" | "st" => Some(Inline::Strikeout),
        _ => None,
    }
}

/// The CSS class a font-family/shape/series command wraps its argument in, or `None` for a command
/// that is not one of these span-producing font switches.
fn font_span_class(name: &str) -> Option<&'static str> {
    match name {
        "textrm" | "textnormal" => Some("roman"),
        "textsf" => Some("sans-serif"),
        "textup" => Some("upright"),
        "textmd" => Some("medium"),
        _ => None,
    }
}

/// Wraps inlines in a formatter, but lifts leading and trailing spacing out of the wrapper so it
/// stays between words rather than inside the formatted run.
fn extract_spaces<F: FnOnce(Vec<Inline>) -> Inline>(
    mut inner: Vec<Inline>,
    wrap: F,
) -> Vec<Inline> {
    let leading =
        matches!(inner.first(), Some(Inline::Space | Inline::SoftBreak)).then(|| inner.remove(0));
    let trailing = if matches!(inner.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inner.pop()
    } else {
        None
    };
    let mut result = Vec::new();
    result.extend(leading);
    result.push(wrap(inner));
    result.extend(trailing);
    result
}

fn span_class(inlines: Vec<Inline>, class: &str) -> Inline {
    Inline::Span(
        Box::new(Attr {
            id: carta_ast::Text::default(),
            classes: vec![class.into()],
            attributes: Vec::new(),
        }),
        inlines,
    )
}

/// A cross-reference command becomes a link to the anchor, showing the raw target text.
fn reference_link(name: &str, target: &str) -> Inline {
    let kind = match name {
        "autoref" | "cref" => "ref+label",
        "Cref" => "ref+Label",
        other => other,
    };
    Inline::Link(
        Box::new(Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: vec![
                ("reference-type".into(), kind.into()),
                ("reference".into(), target.into()),
            ],
        }),
        vec![Inline::Str(format!("[{target}]").into())],
        Box::new(Target {
            url: format!("#{target}").into(),
            title: carta_ast::Text::default(),
        }),
    )
}

/// A font-switch command that formats the remainder of its enclosing group.
#[derive(Clone, Copy)]
enum Switch {
    Strong,
    Emph,
    SmallCaps,
    Code,
}

impl Switch {
    fn wrap(self, inner: Vec<Inline>) -> Inline {
        match self {
            Switch::Strong => Inline::Strong(inner),
            Switch::Emph => Inline::Emph(inner),
            Switch::SmallCaps => Inline::SmallCaps(inner),
            Switch::Code => Inline::Code(Box::default(), to_plain_text(&inner).into()),
        }
    }
}

fn switch_kind(name: &str) -> Option<Switch> {
    Some(match name {
        "bf" | "bfseries" => Switch::Strong,
        "it" | "itshape" | "em" | "sl" | "slshape" => Switch::Emph,
        "scshape" => Switch::SmallCaps,
        "tt" => Switch::Code,
        _ => return None,
    })
}

/// Wraps a bare brace group's content in a null-attribute span to preserve its grouping. An empty
/// group vanishes; a group that already is a single null-attribute span is not wrapped again.
fn group_span(inner: Vec<Inline>) -> Option<Inline> {
    match inner.first() {
        None => None,
        Some(Inline::Span(attr, _))
            if inner.len() == 1
                && attr.id.is_empty()
                && attr.classes.is_empty()
                && attr.attributes.is_empty() =>
        {
            inner.into_iter().next()
        }
        _ => Some(Inline::Span(Box::default(), inner)),
    }
}

/// A combining diacritic requested by an accent command.
#[derive(Clone, Copy)]
enum Accent {
    Acute,
    Grave,
    Circumflex,
    Tilde,
    Diaeresis,
    Macron,
    DotAbove,
    Cedilla,
    Caron,
    Breve,
    DoubleAcute,
    Ring,
    Ogonek,
}

/// The accent for a control-word accent command (`\c`, `\v`, `\u`, `\H`, `\r`, `\k`). The
/// control-symbol accents (`\'`, `` \` ``, `\^`, …) are dispatched separately.
fn word_accent(name: &str) -> Option<Accent> {
    match name {
        "c" => Some(Accent::Cedilla),
        "v" => Some(Accent::Caron),
        "u" => Some(Accent::Breve),
        "H" => Some(Accent::DoubleAcute),
        "r" => Some(Accent::Ring),
        "k" => Some(Accent::Ogonek),
        _ => None,
    }
}

/// The standalone combining mark for an accent, used when no precomposed character exists.
fn combining_mark(accent: Accent) -> char {
    match accent {
        Accent::Acute => '\u{301}',
        Accent::Grave => '\u{300}',
        Accent::Circumflex => '\u{302}',
        Accent::Tilde => '\u{303}',
        Accent::Diaeresis => '\u{308}',
        Accent::Macron => '\u{304}',
        Accent::DotAbove => '\u{307}',
        Accent::Cedilla => '\u{327}',
        Accent::Caron => '\u{30c}',
        Accent::Breve => '\u{306}',
        Accent::DoubleAcute => '\u{30b}',
        Accent::Ring => '\u{30a}',
        Accent::Ogonek => '\u{328}',
    }
}

/// Resolves an accent's braced argument to its base text: a control sequence such as `\i` becomes its
/// glyph, and plain text is returned unchanged.
fn resolve_accent_base(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix('\\') {
        let word: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
        if !word.is_empty() {
            return symbol_text(&word).map_or(word, str::to_owned);
        }
    }
    raw.to_owned()
}

/// Applies an accent to its base text, producing the precomposed character when one exists and
/// otherwise the base followed by the standalone combining mark. An empty argument yields nothing.
fn apply_accent(accent: Accent, base: Option<&str>) -> String {
    let base = base.unwrap_or("");
    let mut chars = base.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let rest: String = chars.collect();
    match combine_accent(accent, first) {
        Some(composed) => format!("{composed}{rest}"),
        None => format!("{first}{}{rest}", combining_mark(accent)),
    }
}

/// The precomposed character for a base letter and accent, for the common Latin-1/Latin-A cases.
#[allow(clippy::too_many_lines)]
fn combine_accent(accent: Accent, base: char) -> Option<char> {
    let table: &[(char, char)] = match accent {
        Accent::Acute => &[
            ('a', 'á'),
            ('e', 'é'),
            ('i', 'í'),
            ('o', 'ó'),
            ('u', 'ú'),
            ('y', 'ý'),
            ('c', 'ć'),
            ('n', 'ń'),
            ('s', 'ś'),
            ('z', 'ź'),
            ('r', 'ŕ'),
            ('l', 'ĺ'),
            ('A', 'Á'),
            ('E', 'É'),
            ('I', 'Í'),
            ('O', 'Ó'),
            ('U', 'Ú'),
            ('Y', 'Ý'),
            ('C', 'Ć'),
            ('N', 'Ń'),
            ('S', 'Ś'),
            ('Z', 'Ź'),
        ],
        Accent::Grave => &[
            ('a', 'à'),
            ('e', 'è'),
            ('i', 'ì'),
            ('o', 'ò'),
            ('u', 'ù'),
            ('A', 'À'),
            ('E', 'È'),
            ('I', 'Ì'),
            ('O', 'Ò'),
            ('U', 'Ù'),
        ],
        Accent::Circumflex => &[
            ('a', 'â'),
            ('e', 'ê'),
            ('i', 'î'),
            ('o', 'ô'),
            ('u', 'û'),
            ('A', 'Â'),
            ('E', 'Ê'),
            ('I', 'Î'),
            ('O', 'Ô'),
            ('U', 'Û'),
        ],
        Accent::Tilde => &[
            ('a', 'ã'),
            ('o', 'õ'),
            ('n', 'ñ'),
            ('A', 'Ã'),
            ('O', 'Õ'),
            ('N', 'Ñ'),
        ],
        Accent::Diaeresis => &[
            ('a', 'ä'),
            ('e', 'ë'),
            ('i', 'ï'),
            ('o', 'ö'),
            ('u', 'ü'),
            ('y', 'ÿ'),
            ('A', 'Ä'),
            ('E', 'Ë'),
            ('I', 'Ï'),
            ('O', 'Ö'),
            ('U', 'Ü'),
        ],
        Accent::Macron => &[
            ('a', 'ā'),
            ('e', 'ē'),
            ('i', 'ī'),
            ('o', 'ō'),
            ('u', 'ū'),
            ('A', 'Ā'),
            ('E', 'Ē'),
            ('I', 'Ī'),
            ('O', 'Ō'),
            ('U', 'Ū'),
        ],
        Accent::DotAbove => &[('e', 'ė'), ('z', 'ż'), ('E', 'Ė'), ('Z', 'Ż')],
        Accent::Cedilla => &[
            ('c', 'ç'),
            ('s', 'ş'),
            ('t', 'ţ'),
            ('g', 'ģ'),
            ('C', 'Ç'),
            ('S', 'Ş'),
            ('T', 'Ţ'),
        ],
        Accent::Caron => &[
            ('c', 'č'),
            ('s', 'š'),
            ('z', 'ž'),
            ('r', 'ř'),
            ('e', 'ě'),
            ('n', 'ň'),
            ('d', 'ď'),
            ('t', 'ť'),
            ('l', 'ľ'),
            ('C', 'Č'),
            ('S', 'Š'),
            ('Z', 'Ž'),
            ('R', 'Ř'),
            ('E', 'Ě'),
            ('N', 'Ň'),
        ],
        Accent::Breve => &[
            ('a', 'ă'),
            ('e', 'ĕ'),
            ('g', 'ğ'),
            ('i', 'ĭ'),
            ('o', 'ŏ'),
            ('u', 'ŭ'),
            ('A', 'Ă'),
            ('G', 'Ğ'),
        ],
        Accent::DoubleAcute => &[('o', 'ő'), ('u', 'ű'), ('O', 'Ő'), ('U', 'Ű')],
        Accent::Ring => &[('a', 'å'), ('u', 'ů'), ('A', 'Å'), ('U', 'Ů')],
        Accent::Ogonek => &[
            ('a', 'ą'),
            ('e', 'ę'),
            ('i', 'į'),
            ('u', 'ų'),
            ('A', 'Ą'),
            ('E', 'Ę'),
        ],
    };
    table.iter().find(|(b, _)| *b == base).map(|(_, c)| *c)
}

/// The literal text a symbol or named-glyph command produces.
fn symbol_text(name: &str) -> Option<&'static str> {
    let text = match name {
        "LaTeX" => "LaTeX",
        "TeX" => "TeX",
        "ldots" | "dots" => "\u{2026}",
        "textbackslash" => "\\",
        "textasciitilde" => "~",
        "textasciicircum" => "^",
        "textless" => "<",
        "textgreater" => ">",
        "textbullet" => "\u{2022}",
        "textquoteright" => "\u{2019}",
        "textquoteleft" => "\u{2018}",
        "textquotedblright" => "\u{201d}",
        "textquotedblleft" => "\u{201c}",
        "textregistered" => "\u{ae}",
        "textcopyright" | "copyright" => "\u{a9}",
        "textdegree" => "\u{b0}",
        "textdagger" => "\u{2020}",
        "S" | "textsection" => "\u{a7}",
        "P" | "textparagraph" => "\u{b6}",
        "pounds" | "textsterling" => "\u{a3}",
        "euro" => "\u{20ac}",
        "textyen" => "\u{a5}",
        "guillemotleft" => "\u{ab}",
        "guillemotright" => "\u{bb}",
        "aa" => "\u{e5}",
        "AA" => "\u{c5}",
        "ae" => "\u{e6}",
        "AE" => "\u{c6}",
        "oe" => "\u{153}",
        "OE" => "\u{152}",
        "o" => "\u{f8}",
        "O" => "\u{d8}",
        "ss" => "\u{df}",
        "l" => "\u{142}",
        "L" => "\u{141}",
        "i" => "\u{131}",
        "j" => "\u{237}",
        "textquotesingle" => "'",
        "textquotedbl" => "\"",
        "slash" => "/",
        "&" => "&",
        _ => return None,
    };
    Some(text)
}

/// The number of trailing braced arguments a dropped command consumes.
fn command_arg_count(name: &str) -> usize {
    match name {
        "setlength" | "addtolength" | "setcounter" | "addtocounter" | "settowidth"
        | "definecolor" | "rule" | "newtheorem" => 2,
        "hspace" | "vspace" | "hskip" | "vskip" | "vphantom" | "hphantom" | "phantom"
        | "raisebox" | "pagestyle" | "thispagestyle" | "pagenumbering" | "documentclass"
        | "usepackage" | "RequirePackage" | "geometry" | "hypersetup" | "bibliographystyle"
        | "include" | "input" | "graphicspath" | "theoremstyle" | "captionsetup"
        | "bibliography" => 1,
        _ => 0,
    }
}

/// Splits raw source on a `\name` control word at brace depth zero, returning the between-parts.
fn split_on_command(raw: &str, name: &str) -> Vec<String> {
    let marker: Vec<char> = format!("\\{name}").chars().collect();
    let chars: Vec<char> = raw.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            '\\' if depth == 0 => {
                let matches_marker = marker
                    .iter()
                    .enumerate()
                    .all(|(k, m)| chars.get(i + k) == Some(m));
                let after = chars.get(i + marker.len());
                let is_word_boundary = after.is_none_or(|c| !c.is_ascii_alphabetic());
                if matches_marker && is_word_boundary {
                    parts.push(std::mem::take(&mut current));
                    i += marker.len();
                    continue;
                }
            }
            _ => {}
        }
        current.push(c);
        i += 1;
    }
    parts.push(current);
    parts
}

/// Converts a paragraph made up solely of images (and spacing) into a plain block, matching how a
/// bare image line reads inside a figure.
fn demote_image_para(block: Block) -> Block {
    if let Block::Para(inlines) = &block {
        let has_image = inlines.iter().any(|i| matches!(i, Inline::Image(..)));
        let only_images = inlines.iter().all(|i| {
            matches!(
                i,
                Inline::Image(..) | Inline::Space | Inline::SoftBreak | Inline::LineBreak
            )
        });
        if has_image
            && only_images
            && let Block::Para(inlines) = block
        {
            return Block::Plain(inlines);
        }
    }
    block
}

/// Substitutes `#1`…`#9` in a macro body with the given argument strings.
fn substitute_macro(body: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '#' {
            match chars.peek() {
                Some(d) if matches!(d, '1'..='9') => {
                    let idx = (*d as usize) - ('1' as usize);
                    chars.next();
                    if let Some(arg) = args.get(idx) {
                        out.push_str(arg);
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

/// Removes a backslash before a character that LaTeX escapes in URLs.
fn unescape_url(url: &str) -> String {
    let mut out = String::new();
    let mut chars = url.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.clone().next() {
                Some(n) if matches!(n, '%' | '#' | '_' | '&' | '{' | '}' | '$' | '~') => {
                    out.push(n);
                    chars.next();
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses a `key=value, key, …` option list into ordered attribute pairs. A bare key gets an empty
/// value.
fn parse_key_values(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for part in split_top_level(text, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('=') {
            Some((k, v)) => pairs.push((k.trim().to_owned(), v.trim().to_owned())),
            None => pairs.push((part.to_owned(), String::new())),
        }
    }
    pairs
}

/// Builds an image's attribute list from its bracketed options, keeping only the sizing keys and
/// expressing a fraction of the text block as a percentage.
fn image_attributes(opts: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    for (key, value) in parse_key_values(opts) {
        if key != "width" && key != "height" {
            continue;
        }
        let value = latex_length_to_percent(&value).unwrap_or(value);
        attrs.push((key, value));
    }
    attrs
}

/// Converts a length given as a fraction of the text block (`0.5\textwidth`) into a percentage
/// (`50%`). Returns `None` for absolute lengths or a value that lacks a leading digit.
fn latex_length_to_percent(value: &str) -> Option<String> {
    let value = value.trim();
    let number = ["\\textwidth", "\\linewidth", "\\textheight"]
        .into_iter()
        .find_map(|unit| value.strip_suffix(unit))?
        .trim();
    if !number.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let (int_part, frac_part) = number.split_once('.').unwrap_or((number, ""));
    if int_part.is_empty()
        || !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    // Multiply by 100 by shifting the decimal point two places to the right.
    let mut digits = format!("{int_part}{frac_part}");
    let point = int_part.len() + 2;
    while digits.len() < point {
        digits.push('0');
    }
    let (whole, frac) = digits.split_at_checked(point)?;
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let frac = frac.trim_end_matches('0');
    Some(if frac.is_empty() {
        format!("{whole}%")
    } else {
        format!("{whole}.{frac}%")
    })
}

/// Splits `text` on `sep`, ignoring separators nested inside `{…}`.
fn split_top_level(text: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '{' => {
                depth += 1;
                current.push(c);
            }
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
                current.push(c);
            }
            _ if c == sep && depth == 0 => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    parts.push(current);
    parts
}

/// Parses a tabular column specification into per-column alignments, skipping rules (`|`), inter-
/// column material (`@{…}`, `!{…}`, `>{…}`, `<{…}`), and paragraph-column widths.
fn parse_column_spec(spec: &str) -> Vec<Alignment> {
    let mut aligns = Vec::new();
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            'l' => aligns.push(Alignment::AlignLeft),
            'r' => aligns.push(Alignment::AlignRight),
            'c' => aligns.push(Alignment::AlignCenter),
            'p' | 'm' | 'b' | 'X' => {
                aligns.push(Alignment::AlignLeft);
                i = skip_brace_group(&chars, i + 1);
                continue;
            }
            '@' | '!' | '>' | '<' => {
                i = skip_brace_group(&chars, i + 1);
                continue;
            }
            '*' => {
                // `*{n}{cols}` repetition is not expanded; its groups are skipped.
                i = skip_brace_group(&chars, i + 1);
                i = skip_brace_group(&chars, i);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    aligns
}

/// Returns the index just past a `{…}` group starting at or after `start` (skipping leading spaces).
fn skip_brace_group(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while matches!(chars.get(i), Some(' ')) {
        i += 1;
    }
    if chars.get(i) != Some(&'{') {
        return i;
    }
    let mut depth = 0i32;
    while let Some(&c) = chars.get(i) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

/// Builds a table from a column spec and the raw tabular body: rows are split on `\\`, cells on `&`.
/// The rows before the first interior horizontal rule become the header; the rest form one body.
fn build_table(parser: &Parser, aligns: &[Alignment], body: &str) -> Block {
    let col_specs: Vec<ColSpec> = aligns
        .iter()
        .map(|a| ColSpec {
            align: a.clone(),
            width: ColWidth::ColWidthDefault,
        })
        .collect();
    let ncols = col_specs.len();

    // The first row becomes a header exactly when a horizontal rule immediately follows it.
    let mut rows: Vec<Row> = Vec::new();
    let mut rule_pending = false;
    let mut first_row_ruled = false;
    for chunk in split_top_level(body, '\n').join(" ").split("\\\\") {
        let (leading_rule, content) = strip_leading_rules(chunk);
        if content.trim().is_empty() {
            rule_pending |= leading_rule;
            continue;
        }
        rule_pending |= leading_rule;
        if rows.len() == 1 && rule_pending {
            first_row_ruled = true;
        }
        rule_pending = false;
        rows.push(build_row(parser, &strip_rules(&content), ncols));
    }
    // A rule trailing the sole row also makes that row a header.
    if rows.len() == 1 && rule_pending {
        first_row_ruled = true;
    }

    let (head_rows, body_rows) = if first_row_ruled && !rows.is_empty() {
        let body = rows.split_off(1);
        (rows, body)
    } else {
        (Vec::new(), rows)
    };

    let table = Table {
        attr: Attr::default(),
        caption: Caption::default(),
        col_specs,
        head: TableHead {
            attr: Attr::default(),
            rows: head_rows,
        },
        bodies: vec![TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: Vec::new(),
            body: body_rows,
        }],
        foot: TableFoot::default(),
    };
    Block::Table(Box::new(table))
}

/// Builds a table row from raw cell source, splitting on `&` and padding to `ncols` columns.
/// A `\multicolumn{n}{align}{content}` field yields a single cell spanning `n` columns.
fn build_row(parser: &Parser, source: &str, ncols: usize) -> Row {
    let mut cells = Vec::new();
    let mut span_total: i32 = 0;
    for cell_src in split_top_level(source, '&') {
        let trimmed = cell_src.trim();
        let (align, col_span, content_src) = match parse_multicolumn(trimmed) {
            Some((n, align, content)) => (align, n, content),
            None => (Alignment::AlignDefault, 1, trimmed.to_owned()),
        };
        let inlines = parse_cell_inlines(parser, content_src.trim());
        let content = if inlines.is_empty() {
            Vec::new()
        } else {
            vec![Block::Plain(inlines)]
        };
        cells.push(Cell {
            attr: Attr::default(),
            align,
            row_span: 1,
            col_span,
            content,
        });
        span_total += col_span.max(1);
    }
    while span_total < i32::try_from(ncols).unwrap_or(i32::MAX) {
        cells.push(Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content: Vec::new(),
        });
        span_total += 1;
    }
    Row {
        attr: Attr::default(),
        cells,
    }
}

/// Parses a `\multicolumn{n}{align}{content}` cell, returning the span, cell alignment, and the
/// raw content. Returns `None` when the field is not a multicolumn.
fn parse_multicolumn(src: &str) -> Option<(i32, Alignment, String)> {
    let rest = src.strip_prefix("\\multicolumn")?;
    let chars: Vec<char> = rest.chars().collect();
    let (span, next) = read_brace_group(&chars, 0)?;
    let (align_spec, next) = read_brace_group(&chars, next)?;
    let (content, _) = read_brace_group(&chars, next)?;
    let span: i32 = span.trim().parse().ok()?;
    if span < 1 {
        return None;
    }
    let align = parse_column_spec(&align_spec)
        .into_iter()
        .next()
        .unwrap_or(Alignment::AlignDefault);
    Some((span, align, content))
}

/// Reads a balanced `{…}` group at or after `start` (skipping leading spaces), returning its inner
/// content and the index just past the closing brace.
fn read_brace_group(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    while matches!(chars.get(i), Some(' ')) {
        i += 1;
    }
    if chars.get(i) != Some(&'{') {
        return None;
    }
    let mut depth = 0i32;
    let mut content = String::new();
    while let Some(&c) = chars.get(i) {
        match c {
            '{' => {
                depth += 1;
                if depth > 1 {
                    content.push(c);
                }
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((content, i + 1));
                }
                content.push(c);
            }
            _ => content.push(c),
        }
        i += 1;
    }
    None
}

/// Strips leading whitespace and horizontal-rule commands from a row chunk, returning whether a
/// header-separating rule was present and the remaining content.
fn strip_leading_rules(chunk: &str) -> (bool, String) {
    let chars: Vec<char> = chunk.chars().collect();
    let mut i = 0;
    let mut header_boundary = false;
    loop {
        while matches!(chars.get(i), Some(c) if c.is_whitespace()) {
            i += 1;
        }
        match rule_command_at(&chars, i) {
            Some((end, header)) => {
                header_boundary |= header;
                i = end;
            }
            None => break,
        }
    }
    (
        header_boundary,
        chars.get(i..).unwrap_or(&[]).iter().collect(),
    )
}

/// If a horizontal-rule command (`\hline`, `\toprule`, …) begins at `chars[start]`, returns the index
/// just past the command name and all its bracketed arguments, together with whether the rule marks a
/// header boundary. A dashed or custom rule (`\hdashline`, `\specialrule`) is removed from the source
/// but does not separate the header row from the body.
fn rule_command_at(chars: &[char], start: usize) -> Option<(usize, bool)> {
    if chars.get(start) != Some(&'\\') {
        return None;
    }
    let mut j = start + 1;
    let mut name = String::new();
    while let Some(&d) = chars.get(j) {
        if d.is_ascii_alphabetic() {
            name.push(d);
            j += 1;
        } else {
            break;
        }
    }
    if !is_rule_command(&name) {
        return None;
    }
    let header_boundary = !matches!(name.as_str(), "hdashline" | "specialrule");
    while matches!(chars.get(j), Some('{' | '[' | '(')) {
        j = skip_rule_argument(chars, j);
    }
    Some((j, header_boundary))
}

fn is_rule_command(name: &str) -> bool {
    matches!(
        name,
        "hline"
            | "toprule"
            | "midrule"
            | "bottomrule"
            | "cmidrule"
            | "cline"
            | "hdashline"
            | "specialrule"
    )
}

/// Returns the index just past a bracketed argument (`{…}`, `[…]`, or `(…)`) starting at `start`.
fn skip_rule_argument(chars: &[char], start: usize) -> usize {
    let close = match chars.get(start) {
        Some('{') => '}',
        Some('[') => ']',
        Some('(') => ')',
        _ => return start,
    };
    let mut i = start + 1;
    while let Some(&c) = chars.get(i) {
        i += 1;
        if c == close {
            break;
        }
    }
    i
}

/// Parses a single table cell's source into inlines using a fresh sub-parser.
fn parse_cell_inlines(parser: &Parser, source: &str) -> Vec<Inline> {
    let mut sub = parser.child(source, false);
    trim_inlines(sub.parse_inlines(InlineStop::Paragraph))
}

/// Removes horizontal-rule commands (`\hline`, `\toprule`, …, `\cline{…}`) from a table row source.
fn strip_rules(row: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = row.chars().collect();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if let Some((end, _)) = rule_command_at(&chars, i) {
            i = end;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reader defaults for LaTeX: smart punctuation, header identifiers from title text, and macro
    /// expansion. Mirrors the format's default extension set so unit tests observe the same behavior
    /// as an ordinary conversion.
    fn latex_defaults() -> Extensions {
        Extensions::from_list(&[
            Extension::Smart,
            Extension::AutoIdentifiers,
            Extension::LatexMacros,
        ])
    }

    fn parse(input: &str) -> Vec<Block> {
        parse_ext(input, latex_defaults())
    }

    fn parse_ext(input: &str, extensions: Extensions) -> Vec<Block> {
        let mut options = ReaderOptions::default();
        options.extensions = extensions;
        LatexReader
            .read(input, &options)
            .expect("latex reader does not fail")
            .blocks
    }

    fn attr(attributes: Vec<(String, String)>) -> Attr {
        Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    // `#0` is not a parameter reference (only `#1`…`#9` are) and is emitted verbatim
    #[test]
    fn substitute_macro_preserves_non_parameter_hash_zero() {
        let expanded = substitute_macro("a#0b", &["Y".to_owned()]);
        assert_eq!(expanded, "a#0b");
    }

    // a numbered display environment keeps its full `\begin`…`\end` source so numbering markup survives
    #[test]
    fn equation_environment_is_display_math_with_verbatim_body() {
        let blocks = parse("\\begin{equation}\n  f(x) = x + 1\n\\end{equation}\n");
        assert_eq!(
            blocks,
            vec![Block::Para(vec![Inline::Math(
                MathType::DisplayMath,
                "\\begin{equation}\n  f(x) = x + 1\n\\end{equation}"
                    .to_owned()
                    .into(),
            )])],
        );
    }

    // a cross-reference becomes a link tagged with the reference kind; a preceding `~` tie is a
    // non-breaking space
    #[test]
    fn ref_becomes_tagged_link_with_bracketed_label() {
        let blocks = parse("See Section~\\ref{sec:intro}.\n");
        assert_eq!(
            blocks,
            vec![Block::Para(vec![
                Inline::Str("See".to_owned().into()),
                Inline::Space,
                Inline::Str("Section\u{a0}".to_owned().into()),
                Inline::Link(
                    Box::new(attr(vec![
                        ("reference-type".to_owned(), "ref".to_owned()),
                        ("reference".to_owned(), "sec:intro".to_owned()),
                    ])),
                    vec![Inline::Str("[sec:intro]".to_owned().into())],
                    Box::new(Target {
                        url: "#sec:intro".to_owned().into(),
                        title: carta_ast::Text::default(),
                    }),
                ),
                Inline::Str(".".to_owned().into()),
            ])],
        );
    }

    // `\cref` and `\autoref` request a lowercase label, `\Cref` an uppercase one
    #[test]
    fn cref_variants_carry_their_reference_kind() {
        let kinds = |input: &str| match parse(input).into_iter().next() {
            Some(Block::Para(inlines)) => inlines
                .into_iter()
                .filter_map(|inline| match inline {
                    Inline::Link(attr, _, _) => attr
                        .attributes
                        .into_iter()
                        .find(|(k, _)| k == "reference-type")
                        .map(|(_, v)| v),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        assert_eq!(kinds("\\cref{a}"), vec!["ref+label".to_owned()]);
        assert_eq!(kinds("\\autoref{a}"), vec!["ref+label".to_owned()]);
        assert_eq!(kinds("\\Cref{a}"), vec!["ref+Label".to_owned()]);
    }

    // a `\textwidth`-fraction width becomes a percentage attribute; default alt text is `image`
    #[test]
    fn includegraphics_textwidth_fraction_becomes_percent_width() {
        let blocks = parse("x \\includegraphics[width=0.5\\textwidth]{diagram.png} y\n");
        assert_eq!(
            blocks,
            vec![Block::Para(vec![
                Inline::Str("x".to_owned().into()),
                Inline::Space,
                Inline::Image(
                    Box::new(attr(vec![("width".to_owned(), "50%".to_owned())])),
                    vec![Inline::Str("image".to_owned().into())],
                    Box::new(Target {
                        url: "diagram.png".to_owned().into(),
                        title: carta_ast::Text::default(),
                    }),
                ),
                Inline::Space,
                Inline::Str("y".to_owned().into()),
            ])],
        );
    }

    // with expansion off, a definition passes through verbatim as a raw block
    #[test]
    fn macro_definition_preserved_verbatim_when_expansion_disabled() {
        let ext = Extensions::from_list(&[Extension::Smart, Extension::AutoIdentifiers]);
        let blocks = parse_ext("\\newcommand{\\foo}{bar}\n", ext);
        assert_eq!(
            blocks,
            vec![Block::RawBlock(
                Format("latex".to_owned().into()),
                "\\newcommand{\\foo}{bar}".to_owned().into(),
            )],
        );
    }

    /// The concatenated plain text of every paragraph a source parses to.
    fn plain_text(input: &str) -> String {
        parse(input)
            .iter()
            .filter_map(|block| match block {
                Block::Para(inlines) => Some(to_plain_text(inlines)),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn simple_macro_expands_to_its_body() {
        assert_eq!(plain_text("\\newcommand{\\x}{Y}\n\n\\x\n"), "Y");
    }

    #[test]
    fn macro_optional_argument_defaults_when_absent() {
        assert_eq!(
            plain_text("\\newcommand{\\x}[2][d]{#1#2}\n\n\\x{B}\n"),
            "dB"
        );
        assert_eq!(
            plain_text("\\newcommand{\\x}[2][d]{#1#2}\n\n\\x[A]{B}\n"),
            "AB",
        );
    }

    #[test]
    fn macro_body_invoking_another_macro_expands_fully() {
        assert_eq!(
            plain_text("\\newcommand{\\a}{\\b}\n\\newcommand{\\b}{Z}\n\n\\a\n"),
            "Z",
        );
    }

    // nesting depth is released after each invocation, so a long sequence does not hit the cap
    #[test]
    fn nested_invocations_do_not_accumulate_depth() {
        let mut source = String::from("\\newcommand{\\a}{\\b}\n\\newcommand{\\b}{Z}\n\n");
        for _ in 0..300 {
            source.push_str("\\a ");
        }
        assert_eq!(plain_text(&source).matches('Z').count(), 300);
    }

    // More than 200 sequential invocations all expand: expansion is not capped by a total count.
    #[test]
    fn many_sequential_invocations_all_expand() {
        let mut source = String::from("\\newcommand{\\hi}{Hello}\n\n");
        for _ in 0..300 {
            source.push_str("\\hi ");
        }
        assert_eq!(plain_text(&source).matches("Hello").count(), 300);
    }

    // A self-recursive macro is stopped by the nesting-depth guard and returns without panicking.
    #[test]
    fn self_recursive_macro_terminates() {
        let _ = parse("\\newcommand{\\x}{\\x}\n\n\\x\n");
    }

    // an expansion ending mid-construct reads its argument across the frame boundary, matching
    // the flattened source
    #[test]
    fn expansion_completed_by_following_source_matches_flattened() {
        assert_eq!(
            parse("\\newcommand{\\bo}{\\textbf}\n\n\\bo{word}\n"),
            parse("\\textbf{word}\n"),
        );
    }

    // a frame emptying right before `\end{...}` pops cleanly at the environment boundary
    #[test]
    fn expansion_ending_at_environment_boundary_matches_flattened() {
        assert_eq!(
            parse("\\newcommand{\\c}{content}\n\n\\begin{quote}\\c\\end{quote}\n"),
            parse("\\begin{quote}content\\end{quote}\n"),
        );
    }
}
