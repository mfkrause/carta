//! Read `AsciiDoc` markup into the document model.

use std::collections::BTreeMap;

use carta_ast::{Document, Inline};
use carta_core::{Reader, ReaderOptions, Result};

mod attributes;
mod block;
mod document;
mod inline;
mod lists;
mod resolve;

const ATTRIBUTE_REFERENCE: &str = "asciidoc-attribute";
const ICON_NAME: &str = "asciidoc-icon";
const AUTO_HEADING: &str = "asciidoc-heading";
const SECTION: &str = "asciidoc-section";
const REFERENCE_LABEL: &str = "asciidoc-reference";
const IDENTIFIER_SOURCE: &str = "asciidoc-identifier-source";
const MONOSPACE_ATTRIBUTE: &str = "asciidoc-monospace-attribute";
const MONOSPACE_REFERENCE: &str = "asciidoc-monospace-reference";
const MONOSPACE_SCOPE: &str = "asciidoc-monospace-scope";
const TERM_STEM_ERROR: &str = "asciidoc-term-stem-error";
mod table;
#[cfg(test)]
mod tests;

use attributes::Attributes;

/// Parses `AsciiDoc` documents using Pandoc 3.10 syntax and conversion rules.
#[derive(Debug, Default, Clone, Copy)]
pub struct AsciidocReader;

impl Reader for AsciidocReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        if options.source_dirs.len() > 1 {
            let mut document = Document::default();
            let mut header_meta = BTreeMap::new();
            let mut footnotes = BTreeMap::new();
            let starts: Vec<_> = options.source_dirs.iter().collect();
            for (index, (offset, directory)) in starts.iter().enumerate() {
                let offset = **offset;
                let end = starts
                    .get(index + 1)
                    .map_or(input.len(), |(offset, _)| **offset);
                let mut source_options = options.clone();
                source_options.source_dirs.clear();
                source_options.source_dir = Some((*directory).clone());
                source_options.source_paths.clear();
                if let Some(path) = options.source_paths.get(&offset) {
                    source_options.source_paths.insert(0, path.clone());
                }
                let mut source = read_source(
                    input.get(offset..end).ok_or_else(|| {
                        carta_core::Error::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "invalid source offset",
                        ))
                    })?,
                    &source_options,
                    std::mem::take(&mut footnotes),
                )?;
                footnotes = source.footnotes;
                for (name, value) in std::mem::take(&mut source.attributes) {
                    if matches!(name.as_str(), "sectids" | "stem") || name.starts_with('!') {
                        continue;
                    }
                    document.meta.entry(name.into()).or_insert_with(|| {
                        if value.is_empty() {
                            carta_ast::MetaValue::MetaBool(true)
                        } else {
                            carta_ast::MetaValue::MetaString(value.into())
                        }
                    });
                }
                let takes_revision = !header_meta.contains_key("version")
                    && source.header_meta.contains_key("version");
                for (name, mut value) in source.header_meta {
                    if matches!(name.as_str(), "version" | "date" | "remark") {
                        if takes_revision {
                            header_meta.insert(name, value);
                        }
                        continue;
                    }
                    if name == "author"
                        && let (
                            Some(carta_ast::MetaValue::MetaList(existing)),
                            carta_ast::MetaValue::MetaList(authors),
                        ) = (header_meta.get_mut(name.as_str()), &mut value)
                    {
                        existing.append(authors);
                        continue;
                    }
                    header_meta.entry(name).or_insert(value);
                }
                document.blocks.append(&mut source.document.blocks);
            }
            for (name, value) in header_meta {
                document.meta.insert(name.into(), value);
            }
            resolve::icons_from_metadata(&mut document);
            attributes::restore_attribute_names(&mut document);
            return Ok(document);
        }
        let mut source = read_source(input, options, BTreeMap::new())?;
        resolve::icons(&mut source.document, &source.attributes);
        attributes::restore_attribute_names(&mut source.document);
        Ok(source.document)
    }
}

struct Source {
    document: Document,
    footnotes: BTreeMap<String, Vec<Inline>>,
    attributes: BTreeMap<String, String>,
    header_meta: BTreeMap<String, carta_ast::MetaValue>,
}

fn read_source(
    input: &str,
    options: &ReaderOptions,
    footnotes: BTreeMap<String, Vec<Inline>>,
) -> Result<Source> {
    let mut parser = Parser {
        directory: options
            .source_dirs
            .first_key_value()
            .map(|(_, directory)| directory.clone())
            .or_else(|| options.source_dir.clone()),
        attributes: BTreeMap::new(),
        header_meta: BTreeMap::new(),
        identifiers: BTreeMap::new(),
        footnotes,
        anchors: BTreeMap::new(),
        counters: BTreeMap::new(),
        depth: 0,
        hard_breaks: false,
        include_depth: 0,
        load_includes: true,
        includes: options
            .source_paths
            .get(&0)
            .map(|path| normalize_path(path))
            .into_iter()
            .collect(),
        definition_term_depth: 0,
        list_context: Vec::new(),
        list_boundary: false,
        pending_line: None,
        error: None,
    };
    let normalized = input.trim_start_matches('\u{feff}').replace('\r', "");
    let mut document = parser.document(&crate::tabs::expand_tabs(&normalized, options.tab_stop));
    if let Some(error) = parser.error.take() {
        return Err(match error {
            ParserError::Fatal(error) => error,
            ParserError::Syntax(message) => carta_core::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                message,
            )),
        });
    }
    if resolve::has_term_stem_error(&mut document) {
        return Err(carta_core::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Encountered math type Nothing",
        )));
    }
    resolve::footnotes(&mut parser.footnotes, &parser.attributes);
    let mut header_meta = parser.header_meta;
    for (name, value) in &mut header_meta {
        if let Some(resolved) = document.meta.get(name.as_str()) {
            value.clone_from(resolved);
        }
    }
    Ok(Source {
        document,
        footnotes: parser.footnotes,
        attributes: parser.attributes,
        header_meta,
    })
}

pub(super) enum ParserError {
    Syntax(String),
    Fatal(carta_core::Error),
}

struct Parser {
    directory: Option<std::path::PathBuf>,
    attributes: BTreeMap<String, String>,
    header_meta: BTreeMap<String, carta_ast::MetaValue>,
    identifiers: BTreeMap<String, usize>,
    footnotes: BTreeMap<String, Vec<Inline>>,
    anchors: BTreeMap<String, Vec<Inline>>,
    counters: BTreeMap<String, (i64, bool)>,
    depth: usize,
    hard_breaks: bool,
    include_depth: usize,
    load_includes: bool,
    includes: Vec<std::path::PathBuf>,
    definition_term_depth: usize,
    list_context: Vec<(char, usize)>,
    list_boundary: bool,
    pending_line: Option<String>,
    error: Option<ParserError>,
}

pub(super) struct ParserSnapshot {
    directory: Option<std::path::PathBuf>,
    attributes: BTreeMap<String, String>,
    header_meta: BTreeMap<String, carta_ast::MetaValue>,
    identifiers: BTreeMap<String, usize>,
    footnotes: BTreeMap<String, Vec<Inline>>,
    anchors: BTreeMap<String, Vec<Inline>>,
    counters: BTreeMap<String, (i64, bool)>,
    depth: usize,
    hard_breaks: bool,
    include_depth: usize,
    load_includes: bool,
    includes: Vec<std::path::PathBuf>,
    definition_term_depth: usize,
    list_context: Vec<(char, usize)>,
    list_boundary: bool,
    pending_line: Option<String>,
}

impl Parser {
    pub(super) fn definition_term(&mut self, source: &str) -> Vec<Inline> {
        self.definition_term_depth += 1;
        let term = self.inlines(source);
        self.definition_term_depth -= 1;
        term
    }

    pub(super) fn in_definition_term(&self) -> bool {
        self.definition_term_depth > 0
    }

    pub(super) fn snapshot(&self) -> ParserSnapshot {
        ParserSnapshot {
            directory: self.directory.clone(),
            attributes: self.attributes.clone(),
            header_meta: self.header_meta.clone(),
            identifiers: self.identifiers.clone(),
            footnotes: self.footnotes.clone(),
            anchors: self.anchors.clone(),
            counters: self.counters.clone(),
            depth: self.depth,
            hard_breaks: self.hard_breaks,
            include_depth: self.include_depth,
            load_includes: self.load_includes,
            includes: self.includes.clone(),
            definition_term_depth: self.definition_term_depth,
            list_context: self.list_context.clone(),
            list_boundary: self.list_boundary,
            pending_line: self.pending_line.clone(),
        }
    }

    pub(super) fn restore(&mut self, snapshot: ParserSnapshot) {
        self.directory = snapshot.directory;
        self.attributes = snapshot.attributes;
        self.header_meta = snapshot.header_meta;
        self.identifiers = snapshot.identifiers;
        self.footnotes = snapshot.footnotes;
        self.anchors = snapshot.anchors;
        self.counters = snapshot.counters;
        self.depth = snapshot.depth;
        self.hard_breaks = snapshot.hard_breaks;
        self.include_depth = snapshot.include_depth;
        self.load_includes = snapshot.load_includes;
        self.includes = snapshot.includes;
        self.definition_term_depth = snapshot.definition_term_depth;
        self.list_context = snapshot.list_context;
        self.list_boundary = snapshot.list_boundary;
        self.pending_line = snapshot.pending_line;
    }
}

fn class_attr(class: &str) -> carta_ast::Attr {
    carta_ast::Attr {
        classes: vec![class.into()],
        ..Default::default()
    }
}

pub(super) fn is_space(character: char) -> bool {
    // Haskell excludes NEL and Unicode line and paragraph separators from this set.
    matches!(
        character,
        '\t'..='\r'
            | ' '
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

pub(super) fn trim_space(source: &str) -> &str {
    source.trim_matches(is_space)
}

pub(super) fn trim_space_start(source: &str) -> &str {
    source.trim_start_matches(is_space)
}

pub(super) fn trim_space_end(source: &str) -> &str {
    source.trim_end_matches(is_space)
}

pub(super) fn space_words(source: &str) -> Vec<&str> {
    source
        .split(is_space)
        .filter(|word| !word.is_empty())
        .collect()
}

fn text_inlines(text: &str) -> Vec<Inline> {
    let mut output = Vec::new();
    for character in text.chars() {
        if matches!(character, ' ' | '\t' | '\n' | '\r') {
            if matches!(character, '\n' | '\r') {
                output.push(Inline::SoftBreak);
            } else if !matches!(output.last(), Some(Inline::Space | Inline::SoftBreak)) {
                output.push(Inline::Space);
            }
        } else {
            push_text(&mut output, &character.to_string());
        }
    }
    output
}

fn push_text(output: &mut Vec<Inline>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Str(previous)) = output.last_mut() {
        previous.push_str(text);
    } else {
        output.push(Inline::Str(text.into()));
    }
}

fn line_comment(line: &str) -> bool {
    line.strip_prefix("//")
        .is_some_and(|rest| rest.starts_with([' ', '\t']))
}

fn blank_line(line: &str) -> bool {
    let line = line.trim_start_matches([' ', '\t']);
    line.is_empty() || line_comment(line)
}

fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    path.components()
        .filter(|part| !matches!(part, std::path::Component::CurDir))
        .collect()
}

fn unsigned(source: &str, radix: u32) -> Option<u64> {
    if source.is_empty() {
        return None;
    }
    source.chars().try_fold(0u64, |value, character| {
        character.to_digit(radix).map(|digit| {
            value
                .wrapping_mul(u64::from(radix))
                .wrapping_add(u64::from(digit))
        })
    })
}
