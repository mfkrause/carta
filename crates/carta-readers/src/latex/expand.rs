//! Accent, argument, and macro-expansion machinery for the LaTeX parser.

use std::rc::Rc;

use carta_ast::{Block, Format, Inline, MetaValue};
use carta_core::Extension;

use super::support::{
    Accent, apply_accent, command_arg_count, resolve_accent_base, split_on_command,
    substitute_macro, symbol_text,
};
use super::tables::parse_cell_inlines;
use super::{Frame, InlineStop, MAX_EXPAND_DEPTH, MAX_TOTAL_EXPANSIONS, Macro, Parser};

impl Parser {
    // --- Accents & arguments ---------------------------------------------------------------------

    pub(super) fn read_accent_symbol(&mut self, accent: Accent, buf: &mut String) {
        let base = self.read_accent_argument();
        buf.push_str(&apply_accent(accent, base.as_deref()));
    }

    /// Reads an accent's argument: a braced group, a control sequence (e.g. `\i` for a dotless i), or
    /// the next single character. The result is the accent's base text.
    pub(super) fn read_accent_argument(&mut self) -> Option<String> {
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

    pub(super) fn read_group_raw(&mut self) -> Option<String> {
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

    pub(super) fn read_optional_raw(&mut self) -> Option<String> {
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

    pub(super) fn read_optional_inlines(&mut self) -> Option<Vec<Inline>> {
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
    pub(super) fn skip_command_args(&mut self, name: &str) {
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
    pub(super) fn skip_adjacent_arguments(&mut self) {
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
    pub(super) fn capture_meta(&mut self, name: &str) {
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
    pub(super) fn parse_fragment(&self, source: &str) -> Vec<Inline> {
        parse_cell_inlines(self, source)
    }

    // --- Macros ----------------------------------------------------------------------------------

    /// Parses a macro definition. With macro expansion enabled the definition is recorded for later
    /// expansion and contributes no block; with it disabled the definition is left in the output as a
    /// raw LaTeX block, preserving its source verbatim.
    pub(super) fn parse_macro_definition(&mut self, name: &str) -> Vec<Block> {
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
    pub(super) fn take_defined_name(&mut self) -> Option<String> {
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

    pub(super) fn parse_def(&mut self) {
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
    pub(super) fn try_expand_macro(&mut self) -> bool {
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
    pub(super) fn read_macro_arg(&mut self) -> Option<String> {
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
