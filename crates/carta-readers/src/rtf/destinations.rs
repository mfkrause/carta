//! Destination-group parsers: metadata, tables, pictures, fields, footnotes, and bookmarks.

use carta_ast::{Inline, MetaValue, Target, Text};
use carta_core::media::content_addressed_name;

use crate::inline_text::words_to_inlines;

use super::chars::{
    code_page_char, combine_surrogate, decode_hex, parse_hyperlink, picture_attr, special_char,
    symbol_char, unicode_code,
};
use super::emitter::{Emitter, LevelDef, nfc_to_style};
use super::inlines::{CharProps, GroupState, StyleFormat, linkify};
use super::lexer::Token;
use super::parser::Parser;

/// Document-metadata destination words carried into the `meta` map as inline values.
const META_FIELDS: &[&str] = &[
    "title", "author", "keywords", "subject", "comment", "company", "doccomm", "operator",
    "category", "manager",
];

impl Parser {
    pub(super) fn parse_info(&mut self) {
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
    pub(super) fn parse_font_table(&mut self) {
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
    pub(super) fn parse_stylesheet(&mut self) {
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
    pub(super) fn parse_list_table(&mut self) {
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
    pub(super) fn parse_list_override_table(&mut self) {
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
    pub(super) fn resolve_list(&self, ls: Option<i32>) -> Vec<LevelDef> {
        let Some(ls) = ls else {
            return Vec::new();
        };
        let id = self.list_overrides.get(&ls).copied().unwrap_or(ls);
        self.list_defs.get(&id).cloned().unwrap_or_default()
    }

    pub(super) fn parse_picture(&mut self) {
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
    pub(super) fn parse_shape(&mut self) {
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

    pub(super) fn parse_field(&mut self) {
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

    pub(super) fn parse_footnote(&mut self) {
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

    pub(super) fn parse_bookmark(&mut self, start: bool) {
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
            // Consume the `uc` fallback items after `\uN` so the fallback `?` never leaks out.
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
