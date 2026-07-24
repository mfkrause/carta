//! The group-walking parser: character-state stack, control-word dispatch, and group descent.

use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{Block, MetaValue, Text};
use carta_core::MediaBag;

use super::chars::{code_page_char, combine_surrogate, special_char, symbol_char, unicode_code};
use super::emitter::{Emitter, LevelDef};
use super::inlines::{CharProps, GroupState, StyleFormat};
use super::lexer::Token;

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

pub(super) struct Parser {
    pub(super) tokens: Vec<Token>,
    pub(super) pos: usize,
    pub(super) states: Vec<GroupState>,
    pub(super) emitters: Vec<Emitter>,
    pub(super) media: MediaBag,
    pub(super) meta: BTreeMap<Text, MetaValue>,
    pub(super) skip: usize,
    pub(super) pending_high_surrogate: Option<u32>,
    pub(super) depth: usize,
    pub(super) list_defs: BTreeMap<i32, Vec<LevelDef>>,
    pub(super) list_overrides: BTreeMap<i32, i32>,
    pub(super) style_outlines: BTreeMap<i32, i32>,
    pub(super) style_formats: BTreeMap<i32, StyleFormat>,
    pub(super) mono_fonts: BTreeSet<i32>,
    /// Fallback state returned by [`Parser::state_mut`] only if the state stack is ever empty, which
    /// its guard prevents; keeps the accessor total without a panic or a leak.
    pub(super) scratch_state: GroupState,
}

/// Ceiling on nested group depth. Beyond it a group's content is discarded rather than descended
/// into, so adversarially deep nesting cannot exhaust the call stack.
const MAX_GROUP_DEPTH: usize = 512;

impl Parser {
    pub(super) fn new(tokens: Vec<Token>) -> Self {
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

    pub(super) fn run(&mut self) {
        self.process();
    }

    pub(super) fn finish(mut self) -> (BTreeMap<Text, MetaValue>, Vec<Block>, MediaBag) {
        while self.emitters.len() > 1 {
            self.emitters.pop();
        }
        let blocks = match self.emitters.pop() {
            Some(emitter) => emitter.finish_blocks(),
            None => Vec::new(),
        };
        (self.meta, blocks, self.media)
    }

    pub(super) fn props(&self) -> CharProps {
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

    pub(super) fn emitter(&mut self) -> Option<&mut Emitter> {
        self.emitters.last_mut()
    }

    /// Processes tokens at the current group level, returning once the matching `}` is consumed or
    /// input is exhausted.
    pub(super) fn process(&mut self) {
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
                // Binary data outside a picture destination carries no text; consumed and dropped.
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
                // Drawing wrapper around `\pict`: process transparently, no character-state save.
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

    pub(super) fn skip_optional_marker(&mut self) {
        if matches!(self.tokens.get(self.pos), Some(Token::Symbol('*'))) {
            self.pos += 1;
        }
    }

    /// Consumes the current group in full, discarding its content.
    pub(super) fn skip_group(&mut self) {
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
                // `\pard` restores the default style `\s0`, whose formatting and outline level
                // apply to every paragraph that selects no other style.
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
                // Overlay the style's formatting and heading level on the current state; later
                // explicit control words in the same paragraph still win.
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
                // A monospace-family font marks the run as code.
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
}
