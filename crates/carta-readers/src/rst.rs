//! reStructuredText reader.
//!
//! Parsing runs in two structural passes. The first pass scans the whole input for the explicit
//! markup that defines document-global references (hyperlink targets, substitution definitions,
//! footnotes, and citations), since a reference may resolve against a definition that appears later.
//! The second pass walks the line structure block by block, building the document tree and resolving
//! each reference against the collected definitions. Inline markup is parsed from the raw text of
//! each leaf during the second pass.

use crate::heading_ids::IdRegistry;
use crate::tabs::expand_tabs;
use carta_ast::Document;
use carta_core::{Extensions, Reader, ReaderOptions, Result};
use std::collections::BTreeMap;

mod block;
mod definitions;
mod directives;
mod explicit;
mod inline;
mod inline_helpers;
mod markers;
mod tables;

#[cfg(test)]
mod tests;

use self::definitions::{Definitions, RoleDef, collect_definitions};

/// Parses reStructuredText into the document model.
///
/// `auto_identifiers` (on by default) derives a slug identifier for each section header; with it
/// off, headers carry no identifier.
#[derive(Debug, Default, Clone, Copy)]
pub struct RstReader;

impl Reader for RstReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let lines = preprocess(input);
        let defs = collect_definitions(&lines);
        let mut parser = Parser {
            defs: &defs,
            ext: options.extensions,
            heading_styles: Vec::new(),
            ids: IdRegistry::default(),
            auto_footnote: 0,
            symbol_footnote: 0,
            anonymous: 0,
            custom_roles: BTreeMap::new(),
            default_role: DEFAULT_ROLE.to_string(),
            include_depth: 0,
            active_substitutions: Vec::new(),
            deferred: BTreeMap::new(),
        };
        let mut blocks = parser.blocks(&lines);
        if let Some(div) = parser.citation_block() {
            blocks.push(div);
        }
        parser.resolve_deferred(&mut blocks);
        Ok(Document {
            blocks,
            ..Document::default()
        })
    }
}

// --- preprocessing -----------------------------------------------------------------------------

const TAB_STOP: usize = 8;

/// A reserved first-class marker on a `Div` left by an empty `class` directive, signaling that the
/// directive's classes apply to the next sibling block. Carries a NUL so it cannot collide with a
/// class name drawn from the input.
const PENDING_CLASS: &str = "\u{0}pending-class";

/// Prefix marking a link destination that names an unresolved reference rather than a concrete URL.
/// A reference may point at a target or section that appears later in the document, so the link is
/// emitted carrying this marker plus the normalized name and resolved in a final pass once every
/// definition is known. The leading NUL keeps it from colliding with any real destination.
const REF_SENTINEL: &str = "\u{0}ref\u{0}";

/// Mark a normalized reference name as an unresolved link destination, to be filled in once every
/// definition in the document has been seen. A name's destination cannot be known at the reference
/// site because it may be defined later (a forward reference) or redefined (the last definition
/// wins); the marker carries the name through tree construction so a final pass can resolve it.
fn defer_reference(name: &str) -> String {
    format!("{REF_SENTINEL}{}", normalize_name(name))
}

/// The target name an indirect destination points at, if any. An indirect target's destination is
/// the name of another target written with a trailing underscore (`other_` or `` `other name`_ ``);
/// the underscore, surrounding whitespace, and backtick quoting are stripped to recover the name. A
/// doubled trailing underscore is an anonymous reference, not an indirect name, and yields `None`.
fn indirect_referent(url: &str) -> Option<String> {
    let referent = url.strip_suffix('_')?;
    if referent.ends_with('_') {
        return None;
    }
    Some(referent.trim().trim_matches('`').trim().to_string())
}

/// Percent-encode the characters a URL may not carry literally: whitespace and the delimiter set
/// `<>|"{}[]^` plus the backtick. Each such character's UTF-8 bytes become `%XX` with uppercase
/// hexadecimal digits; every other character passes through unchanged.
fn escape_uri(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        if ch.is_whitespace()
            || matches!(
                ch,
                '<' | '>' | '|' | '"' | '{' | '}' | '[' | ']' | '^' | '`'
            )
        {
            let mut buf = [0u8; 4];
            for &byte in ch.encode_utf8(&mut buf).as_bytes() {
                out.push('%');
                out.push(hex_digit(byte >> 4));
                out.push(hex_digit(byte & 0x0f));
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// The uppercase hexadecimal digit for a nibble (`0..=15`); values above `15` are not produced by
/// the callers.
fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'A' + (nibble - 10)) as char,
    }
}

/// Normalize line endings, expand tabs to spaces on an eight-column grid, and split into lines with
/// trailing whitespace removed.
fn preprocess(input: &str) -> Vec<String> {
    input
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|line| expand_tabs(line, TAB_STOP).trim_end().to_string())
        .collect()
}

fn is_blank(line: &str) -> bool {
    line.chars().all(char::is_whitespace)
}

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

fn line_at(lines: &[String], i: usize) -> &str {
    lines.get(i).map_or("", String::as_str)
}

/// A reference name normalized for case-insensitive, whitespace-insensitive lookup.
fn normalize_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Drop `count` leading columns of spaces from a line, keeping any content that begins before the
/// cut intact.
fn dedent(line: &str, count: usize) -> String {
    let mut skipped = 0;
    for (idx, ch) in line.char_indices() {
        if ch == ' ' && skipped < count {
            skipped += 1;
        } else {
            return line.get(idx..).unwrap_or("").to_string();
        }
    }
    String::new()
}

// --- block parsing (pass two) ------------------------------------------------------------------

/// The role applied to interpreted text written without an explicit role, until a `default-role`
/// directive selects another.
const DEFAULT_ROLE: &str = "title-reference";

struct Parser<'a> {
    defs: &'a Definitions,
    ext: Extensions,
    heading_styles: Vec<(char, bool)>,
    ids: IdRegistry,
    auto_footnote: usize,
    symbol_footnote: usize,
    anonymous: usize,
    /// Roles declared by `role` directives, keyed by role name.
    custom_roles: BTreeMap<String, RoleDef>,
    /// The role applied to interpreted text with no explicit role.
    default_role: String,
    /// How many nested `include` directives deep this parser is, bounding include recursion.
    include_depth: usize,
    /// The chain of substitution names currently being expanded, by normalized name. A
    /// substitution replacement is itself parsed as inline markup, so a definition that refers
    /// to itself, directly or through a cycle of other definitions, would recurse without
    /// bound and overflow the stack. RST forbids circular substitution references; a name already
    /// on this stack is left unexpanded instead of re-entered.
    active_substitutions: Vec<String>,
    /// Every hyperlink-target name discovered while building the tree (explicit targets, internal
    /// targets, section titles, and the labels of phrase references with an embedded destination),
    /// mapped to its destination. Filled in document order so a later definition supersedes an
    /// earlier one, and consulted by the final pass that resolves the references left deferred
    /// during tree construction.
    deferred: BTreeMap<String, String>,
}

/// The deepest chain of nested `include` directives that is followed before further includes are
/// ignored, guarding against a cycle of files including one another.
const MAX_INCLUDE_DEPTH: usize = 64;
