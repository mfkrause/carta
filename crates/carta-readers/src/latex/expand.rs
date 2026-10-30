//! Accent, argument, and macro-expansion machinery for the LaTeX parser.

use std::rc::Rc;

use carta_ast::{Block, Format, Inline, MetaValue};
use carta_core::Extension;

use super::support::{
    Accent, apply_accent, command_arg_count, resolve_accent_base, split_on_command,
    substitute_macro, symbol_text,
};
use super::tables::parse_cell_inlines;
use super::{Expansion, Frame, InlineStop, Macro, MacroDefinitions, Parser};

enum Scope {
    Group,
    Environment(String),
    Explicit,
}

impl Parser {
    // Accents & arguments

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
        self.read_braced_raw(false)
    }

    pub(super) fn read_macro_group(&mut self) -> Option<String> {
        self.skip_macro_space();
        self.read_braced_raw(true)
    }

    fn read_braced_raw(&mut self, comments: bool) -> Option<String> {
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
                '%' if comments && self.tex_comments => self.copy_comment(&mut s),
                _ => {
                    s.push(c);
                    self.bump();
                }
            }
        }
        Some(s)
    }

    pub(super) fn read_optional_raw(&mut self) -> Option<String> {
        self.read_optional(false)
    }

    fn read_optional(&mut self, comments: bool) -> Option<String> {
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
                '\\' => {
                    s.push(c);
                    self.bump();
                    if let Some(character) = self.bump() {
                        s.push(character);
                    }
                }
                '%' if comments && self.tex_comments => self.copy_comment(&mut s),
                _ => {
                    s.push(c);
                    self.bump();
                }
            }
        }
        Some(s)
    }

    fn copy_comment(&mut self, output: &mut String) {
        while let Some(character) = self.bump() {
            output.push(character);
            if character == '\n' {
                break;
            }
        }
    }

    fn skip_macro_space(&mut self) {
        loop {
            match self.cur() {
                Some(' ' | '\t' | '\n' | '\r') => {
                    self.bump();
                }
                Some('%') if self.tex_comments => self.skip_comment(),
                _ => break,
            }
        }
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

    // Macros

    /// Parses a macro definition. With macro expansion enabled the definition is recorded for later
    /// expansion and contributes no block; with it disabled the definition is left in the output as a
    /// raw LaTeX block, preserving its source verbatim.
    pub(super) fn parse_macro_definition(&mut self, name: &str) -> Vec<Block> {
        // With macros off, no expansion frame can invalidate source offsets.
        let start = self.frames.last().map_or(0, |frame| frame.pos);
        self.consume_control_word();
        if self.cur() == Some('*') {
            self.bump();
        }
        if name == "def" {
            self.parse_def();
        } else if name == "let" {
            self.parse_let();
        } else if let Some(macro_name) = self.take_defined_name() {
            self.skip_macro_space();
            let args = self
                .read_optional(true)
                .and_then(|source| {
                    source
                        .lines()
                        .map(|line| line.split('%').next().unwrap_or_default())
                        .collect::<String>()
                        .trim()
                        .parse::<usize>()
                        .ok()
                })
                .filter(|&count| count <= 9)
                .unwrap_or(0);
            self.skip_macro_space();
            let optional_default = self.read_optional(true);
            let body = self.read_macro_group().unwrap_or_default();
            if name != "providecommand" || !self.macros.contains_key(&macro_name) {
                self.store_macro(
                    macro_name,
                    Rc::new(Macro::Defined {
                        prefix: Vec::new(),
                        parameters: vec![Vec::new(); args],
                        optional_default,
                        body,
                    }),
                );
            }
        }
        if self.ext.contains(Extension::LatexMacros) {
            return Vec::new();
        }
        let raw: String = self
            .frames
            .last()
            .and_then(|frame| frame.source.chars.get(start..frame.pos))
            .unwrap_or_default()
            .iter()
            .collect();
        vec![Block::RawBlock(Format("latex".into()), raw.into())]
    }

    /// Reads a `\newcommand`-style target name, whether written `{\name}` or bare `\name`.
    pub(super) fn take_defined_name(&mut self) -> Option<String> {
        self.skip_macro_space();
        if self.cur() == Some('{') {
            let raw = self.read_braced_raw(true)?;
            let mut target = self.child(&raw, false);
            target.skip_macro_space();
            let name = target.read_control_sequence()?;
            target.skip_macro_space();
            target.cur().is_none().then_some(name)
        } else if self.cur() == Some('\\') {
            self.read_control_sequence()
        } else {
            None
        }
    }

    fn read_control_sequence(&mut self) -> Option<String> {
        if self.cur() != Some('\\') {
            return None;
        }
        if self.peek_control_word().is_some() {
            Some(self.consume_control_word())
        } else {
            self.bump();
            self.bump().map(|c| c.to_string())
        }
    }

    pub(super) fn parse_let(&mut self) {
        self.skip_macro_space();
        let Some(name) = self.read_control_sequence() else {
            return;
        };
        self.skip_macro_space();
        if self.cur() == Some('=') {
            self.bump();
            self.skip_macro_space();
        }
        let binding = if self.cur() == Some('\\') {
            let Some(target) = self.read_control_sequence() else {
                return;
            };
            self.macros
                .get(&target)
                .cloned()
                .unwrap_or_else(|| Rc::new(Macro::Token(format!("\\{target}"))))
        } else if let Some(character) = self.bump() {
            Rc::new(Macro::Token(character.to_string()))
        } else {
            return;
        };
        self.store_macro(name, binding);
    }

    pub(super) fn parse_def(&mut self) {
        let Some(macro_name) = self.take_defined_name() else {
            return;
        };
        self.skip_macro_space();
        let mut prefix = Vec::new();
        let mut parameters = Vec::<Vec<String>>::new();
        while let Some(c) = self.cur() {
            if c == '{' {
                break;
            }
            if c == '#' {
                self.bump();
                if self.cur() == Some('{') {
                    parameters
                        .last_mut()
                        .unwrap_or(&mut prefix)
                        .push("{".to_owned());
                    break;
                }
                if self
                    .bump()
                    .and_then(|c| c.to_digit(10))
                    .filter(|&number| (1..=9).contains(&number))
                    .and_then(|number| usize::try_from(number).ok())
                    != Some(parameters.len() + 1)
                {
                    return;
                }
                parameters.push(Vec::new());
            } else {
                let token = if c == '\\' {
                    self.read_control_sequence().map(|name| format!("\\{name}"))
                } else if c == '%' {
                    self.skip_comment();
                    self.bump();
                    continue;
                } else if c.is_whitespace() {
                    while self.cur().is_some_and(char::is_whitespace) {
                        self.bump();
                    }
                    Some(" ".to_owned())
                } else {
                    self.bump().map(|c| c.to_string())
                };
                if let Some(token) = token {
                    parameters.last_mut().unwrap_or(&mut prefix).push(token);
                }
            }
        }
        let body = self.read_macro_group().unwrap_or_default();
        self.store_macro(
            macro_name,
            Rc::new(Macro::Defined {
                prefix,
                parameters,
                optional_default: None,
                body,
            }),
        );
    }

    fn store_macro(&mut self, name: String, definition: Rc<Macro>) {
        if Rc::strong_count(&self.macros) > 1 {
            let Some(remaining) = self.expansions.calls.get().checked_sub(self.macros.len()) else {
                self.expansions.calls.set(0);
                return;
            };
            self.expansions.calls.set(remaining);
        }
        Rc::make_mut(&mut self.macros).insert(name, definition);
    }

    /// Pushes a macro or expansion primitive's replacement before the remaining input.
    pub(super) fn try_expand_macro(&mut self) -> bool {
        self.expand_macro(true)
    }

    fn expand_macro(&mut self, track_call: bool) -> bool {
        if !self.ext.contains(Extension::LatexMacros) || self.expansions.calls.get() == 0 {
            return false;
        }
        self.drop_exhausted_frames();
        if self.is_expand_after() {
            return self.expand_after();
        }
        if self.frames.last().is_some_and(|frame| frame.fixed_token) {
            return false;
        }
        let parent = self.frames.last().and_then(|frame| frame.expansion.clone());
        let name = self.peek_control_word().or_else(|| {
            (self.cur() == Some('\\'))
                .then(|| self.at(1).map(|c| c.to_string()))
                .flatten()
        });
        let Some(name) = name else {
            return false;
        };
        let macros = Rc::clone(&self.macros);
        let Some(mac) = macros.get(&name) else {
            return false;
        };
        let preserve = matches!(mac.as_ref(), Macro::Defined { prefix, parameters, .. }
            if !prefix.is_empty() || !parameters.is_empty());
        if preserve {
            self.recording = Some(String::new());
        }
        self.read_control_sequence();
        let mut args = Vec::new();
        if let Macro::Defined {
            prefix,
            parameters,
            optional_default,
            ..
        } = mac.as_ref()
        {
            if !self.eat_macro_delimiter(prefix) {
                self.restore_macro_call(parent);
                return false;
            }
            if let Some(default) = optional_default {
                let first = if self.cur() == Some('[') {
                    self.read_optional(true).unwrap_or_default()
                } else {
                    default.clone()
                };
                args.push(first);
            }
            for delimiter in parameters
                .iter()
                .skip(usize::from(optional_default.is_some()))
            {
                if let Some(argument) = self.read_delimited_arg(delimiter) {
                    args.push(argument);
                } else {
                    self.restore_macro_call(parent);
                    return false;
                }
            }
        }
        self.recording = None;
        if self.expansions.calls.get() == 0 {
            return true;
        }
        let call = (name, Rc::clone(mac), args);
        if self.expansions.active.borrow().contains(&call) {
            return true;
        }
        let expanded = match mac.as_ref() {
            Macro::Defined { body, .. } => substitute_macro(body, &call.2),
            Macro::Token(token) => token.clone(),
        };
        let Some(left) = self.expansions.characters.get().checked_sub(expanded.len()) else {
            self.expansions.calls.set(0);
            return true;
        };
        self.expansions.characters.set(left);
        self.expansions
            .calls
            .set(self.expansions.calls.get().saturating_sub(1));
        if !expanded.is_empty() {
            let expansion = if track_call {
                self.expansions.active.borrow_mut().insert(call.clone());
                Some(Rc::new(Expansion {
                    call,
                    state: Rc::clone(&self.expansions),
                    parent,
                }))
            } else {
                parent
            };
            let mut frame = Frame::new(&expanded, expansion);
            frame.fixed_token = matches!(mac.as_ref(), Macro::Token(_));
            self.frames.push(frame);
        }
        true
    }

    fn restore_macro_call(&mut self, parent: Option<Rc<Expansion>>) {
        let source = self.recording.take().unwrap_or_default();
        self.frames.push(Frame::new(&source, parent));
    }

    fn is_expand_after(&self) -> bool {
        let Some(name) = self.peek_control_word() else {
            return false;
        };
        if self
            .frames
            .iter()
            .rev()
            .find(|frame| frame.pos < frame.end)
            .is_some_and(|frame| frame.fixed_token)
        {
            return name == "expandafter";
        }
        match self.macros.get(&name).map(Rc::as_ref) {
            Some(Macro::Token(token)) => token == "\\expandafter",
            Some(Macro::Defined { .. }) => false,
            None => name == "expandafter",
        }
    }

    fn expand_after(&mut self) -> bool {
        let parent = self.frames.last().and_then(|frame| frame.expansion.clone());
        let mut delayed = Vec::new();
        while self.expansions.calls.get() > 0 && self.is_expand_after() {
            self.consume_control_word();
            self.expansions
                .calls
                .set(self.expansions.calls.get().saturating_sub(1));
            self.drop_exhausted_frames();
            let fixed_token = self.frames.last().is_some_and(|frame| frame.fixed_token);
            let token = if self.cur() == Some('\\') {
                self.read_control_sequence().map(|name| format!("\\{name}"))
            } else {
                self.bump().map(|c| c.to_string())
            };
            let Some(token) = token else {
                break;
            };
            delayed.push((token, fixed_token));
        }
        // The delayed token starts a separate expansion branch.
        self.expand_macro(false);
        for (token, fixed_token) in delayed.into_iter().rev() {
            let mut frame = Frame::new(&token, parent.clone());
            frame.fixed_token = fixed_token;
            self.frames.push(frame);
        }
        true
    }

    fn eat_macro_delimiter(&mut self, delimiter: &[String]) -> bool {
        // Bound repeated prefix matches with the shared expansion allowance.
        let mut remaining = self.expansions.calls.get();
        let mut length = 0;
        let matches = delimiter.iter().all(|token| {
            if token == " " {
                if !self.at(length).is_some_and(char::is_whitespace) {
                    return false;
                }
            } else {
                for character in token.chars() {
                    if remaining == 0 {
                        return false;
                    }
                    remaining -= 1;
                    if self.at(length) != Some(character) {
                        return false;
                    }
                    length += 1;
                }
            }
            let control_word = token.strip_prefix('\\').is_some_and(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic())
            });
            if control_word && self.at(length).is_some_and(|c| c.is_ascii_alphabetic()) {
                return false;
            }
            if control_word || token == " " {
                while self.at(length).is_some_and(char::is_whitespace) {
                    if remaining == 0 {
                        return false;
                    }
                    remaining -= 1;
                    length += 1;
                }
            }
            true
        });
        self.expansions.calls.set(remaining);
        if !matches {
            return false;
        }
        let retained_brace = usize::from(delimiter.last().is_some_and(|token| token == "{"));
        self.advance_chars(length.saturating_sub(retained_brace));
        true
    }

    fn read_delimited_arg(&mut self, delimiter: &[String]) -> Option<String> {
        if delimiter.is_empty() {
            return self.read_macro_arg();
        }
        let mut argument = String::new();
        while let Some(character) = self.cur() {
            if self.eat_macro_delimiter(delimiter) {
                return Some(argument);
            }
            if self.expansions.calls.get() == 0 {
                return None;
            }
            match character {
                '{' => {
                    let group = self.read_braced_raw(true)?;
                    if argument.is_empty() && self.eat_macro_delimiter(delimiter) {
                        return Some(group);
                    }
                    argument.push('{');
                    argument.push_str(&group);
                    argument.push('}');
                }
                '\\' => {
                    let name = self.read_control_sequence()?;
                    argument.push('\\');
                    argument.push_str(&name);
                    if name.chars().all(|c| c.is_ascii_alphabetic())
                        && self.cur().is_some_and(|c| c.is_ascii_alphabetic())
                    {
                        argument.push(' ');
                    }
                }
                '%' if self.tex_comments => self.copy_comment(&mut argument),
                _ => {
                    argument.push(character);
                    self.bump();
                }
            }
        }
        None
    }

    pub(super) fn expand_math(&self, source: &str) -> String {
        if !self.ext.contains(Extension::LatexMacros)
            || (self.macros.is_empty() && !source.contains("\\expandafter"))
        {
            return source.to_owned();
        }
        let mut parser = self.child(source, false);
        parser.expand_source(false)
    }

    fn expand_link_argument(&mut self) -> String {
        self.recording = Some(String::new());
        self.consume_control_word();
        let argument = self.read_group_raw();
        let source = self.recording.take().unwrap_or_default();
        let Some(argument) = argument else {
            return source;
        };
        let Some(open) = source.find('{') else {
            return source;
        };
        if !source.ends_with('}') {
            return source;
        }
        let mut parser = self.child(&argument, false);
        parser.tex_comments = false;
        let expanded = parser.expand_source(false);
        let prefix = source.get(..=open).unwrap_or_default();
        format!("{prefix}{expanded}}}")
    }

    fn expand_scope(&mut self, name: &str, scopes: &mut Vec<(Scope, MacroDefinitions)>) -> String {
        self.recording = Some(String::new());
        self.consume_control_word();
        if matches!(name, "begin" | "end") {
            if let Some(environment) = self.read_group_raw() {
                if name == "begin" {
                    if matches!(
                        environment.as_str(),
                        "verbatim" | "verbatim*" | "Verbatim" | "lstlisting" | "minted"
                    ) || (environment == "code" && self.ext.contains(Extension::LiterateHaskell))
                    {
                        self.read_verbatim_body(&environment);
                    } else {
                        scopes.push((Scope::Environment(environment), Rc::clone(&self.macros)));
                    }
                } else if scopes.last().is_some_and(
                    |(scope, _)| matches!(scope, Scope::Environment(open) if open == &environment),
                ) && let Some((_, macros)) = scopes.pop()
                {
                    self.macros = macros;
                }
            }
        } else if name == "begingroup" {
            scopes.push((Scope::Explicit, Rc::clone(&self.macros)));
        } else if scopes
            .last()
            .is_some_and(|(scope, _)| matches!(scope, Scope::Explicit))
            && let Some((_, macros)) = scopes.pop()
        {
            self.macros = macros;
        }
        self.recording.take().unwrap_or_default()
    }

    fn expand_source(&mut self, definitions: bool) -> String {
        let mut output = String::new();
        let mut control_word = false;
        let mut scopes = Vec::new();
        while let Some(character) = self.cur() {
            if character == '\\' {
                if self.try_expand_macro() {
                    continue;
                }
                if let Some(name) = self.peek_control_word() {
                    if definitions
                        && matches!(
                            name.as_str(),
                            "newcommand"
                                | "renewcommand"
                                | "providecommand"
                                | "DeclareRobustCommand"
                                | "def"
                                | "let"
                        )
                    {
                        self.recording = Some(String::new());
                        self.parse_macro_definition(&name);
                        output.push_str(&self.recording.take().unwrap_or_default());
                        control_word = false;
                        continue;
                    }
                    if definitions
                        && matches!(name.as_str(), "begin" | "end" | "begingroup" | "endgroup")
                    {
                        let source = self.expand_scope(&name, &mut scopes);
                        control_word = source
                            .chars()
                            .next_back()
                            .is_some_and(|character| character.is_ascii_alphabetic());
                        output.push_str(&source);
                        continue;
                    }
                    if definitions && matches!(name.as_str(), "url" | "href") {
                        output.push_str(&self.expand_link_argument());
                        control_word = false;
                        continue;
                    }
                    if definitions && matches!(name.as_str(), "nolinkurl" | "path" | "verb") {
                        self.recording = Some(String::new());
                        self.consume_control_word();
                        if self.cur() == Some('*') {
                            self.bump();
                        }
                        if name == "verb" {
                            if let Some(delimiter) = self.bump() {
                                while let Some(character) = self.bump() {
                                    if character == delimiter {
                                        break;
                                    }
                                }
                            }
                        } else {
                            self.read_group_raw();
                        }
                        output.push_str(&self.recording.take().unwrap_or_default());
                        control_word = false;
                        continue;
                    }
                    self.advance_chars(name.len() + 1);
                    output.push('\\');
                    output.push_str(&name);
                    control_word = true;
                    continue;
                }
                output.push('\\');
                self.bump();
                if let Some(symbol) = self.bump() {
                    output.push(symbol);
                }
            } else if character == '%' && self.tex_comments {
                self.copy_comment(&mut output);
            } else {
                if definitions {
                    if character == '{' {
                        scopes.push((Scope::Group, Rc::clone(&self.macros)));
                    } else if character == '}'
                        && scopes
                            .last()
                            .is_some_and(|(scope, _)| matches!(scope, Scope::Group))
                        && let Some((_, macros)) = scopes.pop()
                    {
                        self.macros = macros;
                    }
                }
                if control_word && character.is_ascii_alphabetic() {
                    output.push(' ');
                }
                output.push(character);
                self.bump();
            }
            control_word = false;
        }
        self.drop_exhausted_frames();
        output
    }

    /// Reads a single macro argument: a braced group, or the next single token.
    pub(super) fn read_macro_arg(&mut self) -> Option<String> {
        self.skip_macro_space();
        if self.cur() == Some('{') {
            self.read_braced_raw(true)
        } else if self.cur() == Some('\\') {
            self.read_control_sequence().map(|name| format!("\\{name}"))
        } else {
            self.bump().map(|c| c.to_string())
        }
    }
}

/// Stateful expansion of TeX fragments embedded in another text format.
#[cfg(feature = "commonmark")]
pub(crate) struct MacroExpander(Parser);

#[cfg(feature = "commonmark")]
impl MacroExpander {
    pub(crate) fn new(source_len: usize, mut extensions: carta_core::Extensions) -> Self {
        extensions.insert(Extension::LatexMacros);
        Self(Parser::new(source_len, extensions, 1))
    }

    pub(crate) fn has_definitions(&self) -> bool {
        !self.0.macros.is_empty()
    }

    pub(crate) fn probe(&self) -> Self {
        let mut parser = Parser::new(0, self.0.ext, self.0.base_level);
        parser.macros = Rc::clone(&self.0.macros);
        parser.expansions.calls.set(self.0.expansions.calls.get());
        parser
            .expansions
            .characters
            .set(self.0.expansions.characters.get());
        Self(parser)
    }

    pub(crate) fn fork(&self) -> Self {
        Self(self.0.child("", false))
    }

    pub(crate) fn raw(&mut self, source: &str) -> String {
        let content = source.trim_end_matches([' ', '\t']);
        self.0.set_source(content);
        let mut output = self.0.expand_source(true);
        output.push_str(source.get(content.len()..).unwrap_or_default());
        output
    }

    pub(crate) fn math(&self, source: &str) -> String {
        self.0.expand_math(source)
    }
}

#[cfg(all(test, feature = "commonmark"))]
mod tests {
    use super::MacroExpander;

    #[test]
    fn scope_copies_stop_when_the_work_allowance_is_spent() {
        let mut engine = MacroExpander::new(100, carta_core::Extensions::empty());
        engine.raw(r"\newcommand{\word}{first}\newcommand{\other}{second}");
        engine.0.expansions.calls.set(1);
        let mut scope = engine.fork();
        let source = r"\renewcommand{\word}{inside}\word";
        assert_eq!(scope.raw(source), source);
        engine.0.expansions.calls.set(10);
        assert_eq!(engine.math(r"\word"), "first");
    }
}
