//! reStructuredText reader.
//!
//! Parsing runs in two structural passes. The first pass scans the whole input for the explicit
//! markup that defines document-global references — hyperlink targets, substitution definitions,
//! footnotes, and citations — since a reference may resolve against a definition that appears later.
//! The second pass walks the line structure block by block, building the document tree and resolving
//! each reference against the collected definitions. Inline markup is parsed from the raw text of
//! each leaf during the second pass.

use std::collections::BTreeMap;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Format, Inline,
    ListAttributes, ListNumberDelim, ListNumberStyle, MathType, Row, Table, TableBody, TableFoot,
    TableHead, Target,
};
use carta_core::{Extension, Extensions, Reader, ReaderOptions, Result};

use crate::inline_text::trim_inline_ends;

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
            auto_footnote: 0,
            symbol_footnote: 0,
            anonymous: 0,
        };
        let mut blocks = parser.blocks(&lines);
        if let Some(div) = parser.citation_block() {
            blocks.push(div);
        }
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

/// Normalize line endings, expand tabs to spaces on an eight-column grid, and split into lines with
/// trailing whitespace removed.
fn preprocess(input: &str) -> Vec<String> {
    input
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|line| expand_tabs(line).trim_end().to_string())
        .collect()
}

fn expand_tabs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let next = (col / TAB_STOP + 1) * TAB_STOP;
            while col < next {
                out.push(' ');
                col += 1;
            }
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
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

// --- adornments and markers --------------------------------------------------------------------

const ADORNMENT_CHARS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

/// The single repeated punctuation character of a section adornment or transition line, or `None`
/// when the line is not a run of one such character.
fn adornment_char(line: &str) -> Option<char> {
    let trimmed = line.trim();
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    if !ADORNMENT_CHARS.contains(first) {
        return None;
    }
    if chars.all(|c| c == first) {
        Some(first)
    } else {
        None
    }
}

const BULLETS: &str = "*+-\u{2022}\u{2023}\u{2043}";

/// For a bullet list item, the column at which its content begins.
fn bullet_content_col(line: &str) -> Option<usize> {
    let mut chars = line.chars();
    let marker = chars.next()?;
    if !BULLETS.contains(marker) {
        return None;
    }
    match chars.next() {
        None => Some(1),
        Some(' ') => Some(2 + chars.take_while(|c| *c == ' ').count()),
        Some(_) => None,
    }
}

fn roman_value(text: &str) -> Option<i32> {
    let mut total = 0;
    let mut prev = 0;
    for ch in text.chars().rev() {
        let value = match ch.to_ascii_lowercase() {
            'i' => 1,
            'v' => 5,
            'x' => 10,
            'l' => 50,
            'c' => 100,
            'd' => 500,
            'm' => 1000,
            _ => return None,
        };
        if value < prev {
            total -= value;
        } else {
            total += value;
            prev = value;
        }
    }
    if total > 0 { Some(total) } else { None }
}

/// The parsed leading marker of an enumerated list item: its start value, numeral style, delimiter,
/// and the column at which its content begins.
fn enumerator(line: &str) -> Option<(i32, ListNumberStyle, ListNumberDelim, usize)> {
    let bytes: Vec<char> = line.chars().collect();
    let (two_parens, numeral_start) = match bytes.first() {
        Some('(') => (true, 1),
        _ => (false, 0),
    };
    let mut end = numeral_start;
    while let Some(ch) = bytes.get(end) {
        if ch.is_ascii_alphanumeric() || *ch == '#' {
            end += 1;
        } else {
            break;
        }
    }
    let numeral: String = bytes.get(numeral_start..end)?.iter().collect();
    if numeral.is_empty() {
        return None;
    }
    let (style, start) = classify_numeral(&numeral)?;
    let delim = if two_parens {
        if bytes.get(end) != Some(&')') {
            return None;
        }
        end += 1;
        ListNumberDelim::TwoParens
    } else {
        match bytes.get(end) {
            Some('.') => {
                end += 1;
                ListNumberDelim::Period
            }
            Some(')') => {
                end += 1;
                ListNumberDelim::OneParen
            }
            _ => return None,
        }
    };
    // An auto-numbered (`#`) enumerator carries no concrete style or delimiter.
    let delim = if numeral == "#" {
        ListNumberDelim::DefaultDelim
    } else {
        delim
    };
    // An enumerator must be followed by whitespace; a marker that ends the line is ordinary text.
    match bytes.get(end) {
        Some(' ') => {
            let spaces = bytes
                .get(end + 1..)?
                .iter()
                .take_while(|c| **c == ' ')
                .count();
            Some((start, style, delim, end + 1 + spaces))
        }
        _ => None,
    }
}

fn classify_numeral(numeral: &str) -> Option<(ListNumberStyle, i32)> {
    if numeral == "#" {
        return Some((ListNumberStyle::DefaultStyle, 1));
    }
    if numeral.chars().all(|c| c.is_ascii_digit()) {
        return numeral
            .parse::<i32>()
            .ok()
            .map(|n| (ListNumberStyle::Decimal, n));
    }
    if let Some(value) = roman_value(numeral) {
        let style = if numeral.chars().all(|c| c.is_ascii_uppercase()) {
            ListNumberStyle::UpperRoman
        } else {
            ListNumberStyle::LowerRoman
        };
        return Some((style, value));
    }
    let mut chars = numeral.chars();
    let single = chars.next()?;
    if chars.next().is_none() && single.is_ascii_alphabetic() {
        let ordinal = i32::from((single.to_ascii_lowercase() as u8) - b'a' + 1);
        let style = if single.is_ascii_uppercase() {
            ListNumberStyle::UpperAlpha
        } else {
            ListNumberStyle::LowerAlpha
        };
        return Some((style, ordinal));
    }
    None
}

/// A field marker `:name: value`: the field name and the column at which the value begins.
fn field_marker(line: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    if chars.first() != Some(&':') {
        return None;
    }
    let mut idx = 1;
    while let Some(ch) = chars.get(idx) {
        if *ch == ':' && (chars.get(idx + 1).is_none() || chars.get(idx + 1) == Some(&' ')) {
            let name: String = chars.get(1..idx)?.iter().collect();
            if name.is_empty() {
                return None;
            }
            let value_col = if chars.get(idx + 1).is_some() {
                idx + 2
            } else {
                idx + 1
            };
            return Some((name, value_col));
        }
        if *ch == ':' && idx == 1 {
            return None;
        }
        idx += 1;
    }
    None
}

// --- explicit markup blocks --------------------------------------------------------------------

/// The extent of an explicit-markup block (a `..` or `__` construct, a directive, or a comment):
/// the index one past its last content line. The block runs over its first line plus all following
/// blank or further-indented lines, up to but not including the next line indented no more than the
/// marker.
fn explicit_extent(lines: &[String], start: usize, marker_indent: usize) -> usize {
    let mut last_content = start;
    let mut i = start + 1;
    while let Some(line) = lines.get(i) {
        if is_blank(line) {
            i += 1;
        } else if indent_of(line) > marker_indent {
            last_content = i;
            i += 1;
        } else {
            break;
        }
    }
    last_content + 1
}

/// The body region of an explicit-markup block: the first line's text after `prefix_len` columns,
/// followed by the continuation lines dedented by their shared minimum indentation. A leading empty
/// first-line remainder is dropped.
fn explicit_body(lines: &[String], start: usize, end: usize, prefix_len: usize) -> Vec<String> {
    let mut body = Vec::new();
    let first = line_at(lines, start);
    let remainder: String = first.chars().skip(prefix_len).collect();
    if !remainder.trim().is_empty() {
        body.push(remainder.trim_start().to_string());
    }
    let continuation: Vec<&String> = (start + 1..end).filter_map(|i| lines.get(i)).collect();
    let min_indent = continuation
        .iter()
        .filter(|l| !is_blank(l))
        .map(|l| indent_of(l))
        .min()
        .unwrap_or(0);
    for line in continuation {
        if is_blank(line) {
            body.push(String::new());
        } else {
            body.push(dedent(line, min_indent));
        }
    }
    while body.last().is_some_and(std::string::String::is_empty) {
        body.pop();
    }
    body
}

/// A classified explicit-markup construct, by the first non-`..` token on its line.
enum Explicit {
    Target,
    AnonymousTarget,
    Footnote(String),
    Citation(String),
    Substitution,
    Directive(String),
    Comment,
}

fn classify_explicit(line: &str) -> Option<Explicit> {
    let trimmed = line.trim_start();
    if trimmed == "__" || trimmed.starts_with("__ ") {
        return Some(Explicit::AnonymousTarget);
    }
    if trimmed != ".." && !trimmed.starts_with(".. ") {
        return None;
    }
    let rest = trimmed.strip_prefix("..").unwrap_or("").trim_start();
    if rest.is_empty() {
        return Some(Explicit::Comment);
    }
    if rest.starts_with("__") {
        return Some(Explicit::AnonymousTarget);
    }
    if rest.starts_with('_') {
        return Some(Explicit::Target);
    }
    if let Some(after) = rest.strip_prefix('[') {
        if let Some(close) = after.find(']') {
            let label = after.get(..close).unwrap_or("");
            if !label.is_empty() {
                return Some(if is_citation_label(label) {
                    Explicit::Citation(label.to_string())
                } else {
                    Explicit::Footnote(label.to_string())
                });
            }
        }
        return Some(Explicit::Comment);
    }
    if rest.starts_with('|') {
        return Some(Explicit::Substitution);
    }
    if let Some(name) = directive_name(rest) {
        return Some(Explicit::Directive(name));
    }
    Some(Explicit::Comment)
}

/// A footnote label is a number, `#`, `#name`, or `*`; any other bracket label is a citation.
fn is_citation_label(label: &str) -> bool {
    !(label.chars().all(|c| c.is_ascii_digit())
        || label == "*"
        || label == "#"
        || label.starts_with('#'))
}

/// The lowercased name of a directive (`name::`), or `None` when the text is not a directive.
fn directive_name(rest: &str) -> Option<String> {
    let end = rest.find("::")?;
    let name = rest.get(..end)?;
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.' | ':'))
    {
        return None;
    }
    Some(name.to_lowercase())
}

// --- definitions (pass one) --------------------------------------------------------------------

#[derive(Default)]
struct Definitions {
    /// Normalized hyperlink-target name to destination URL.
    targets: BTreeMap<String, String>,
    /// Anonymous-target destinations, in document order.
    anonymous: Vec<String>,
    /// Normalized substitution name to its definition.
    substitutions: BTreeMap<String, Substitution>,
    /// Labeled footnote bodies, keyed by the label as written (`1`, `#name`).
    footnotes: BTreeMap<String, Vec<String>>,
    /// Auto-numbered (`#`) footnote bodies, in document order.
    auto_footnotes: Vec<Vec<String>>,
    /// Symbol (`*`) footnote bodies, in document order.
    symbol_footnotes: Vec<Vec<String>>,
    /// Citations: original label and body, in document order.
    citations: Vec<(String, Vec<String>)>,
}

enum Substitution {
    Replace(String),
    Image(String, Attr, Vec<Inline>),
}

fn collect_definitions(lines: &[String]) -> Definitions {
    let mut defs = Definitions::default();
    let mut i = 0;
    while i < lines.len() {
        let line = line_at(lines, i);
        if is_blank(line) {
            i += 1;
            continue;
        }
        let indent = indent_of(line);
        let trimmed = line.trim_start();
        if let Some(kind) = classify_explicit(trimmed) {
            let end = explicit_extent(lines, i, indent);
            record_definition(&mut defs, lines, i, end, indent, kind);
            i = end;
        } else {
            i += 1;
        }
    }
    defs
}

fn record_definition(
    defs: &mut Definitions,
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
    kind: Explicit,
) {
    let first = line_at(lines, start).trim_start();
    match kind {
        Explicit::Target => {
            if let Some((name, url)) = parse_target(first, lines, start, end, indent) {
                defs.targets.insert(normalize_name(&name), url);
            }
        }
        Explicit::AnonymousTarget => {
            let url = parse_anonymous(first, lines, start, end, indent);
            defs.anonymous.push(url);
        }
        Explicit::Footnote(label) => {
            let body = footnote_body(lines, start, end, indent);
            if label == "#" {
                defs.auto_footnotes.push(body);
            } else if label == "*" {
                defs.symbol_footnotes.push(body);
            } else {
                defs.footnotes.insert(label, body);
            }
        }
        Explicit::Citation(label) => {
            let body = footnote_body(lines, start, end, indent);
            defs.citations.push((label, body));
        }
        Explicit::Substitution => {
            if let Some((name, subst)) = parse_substitution(first, lines, start, end, indent) {
                defs.substitutions.insert(normalize_name(&name), subst);
            }
        }
        Explicit::Directive(_) | Explicit::Comment => {}
    }
}

/// Parse a hyperlink target `_name: url` (the URL may continue across lines, joined without spaces).
fn parse_target(
    first: &str,
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
) -> Option<(String, String)> {
    let rest = first.strip_prefix("..").unwrap_or(first).trim_start();
    let rest = rest.strip_prefix('_')?;
    let (name, after) = split_target_name(rest)?;
    let mut url = after.trim().to_string();
    for i in start + 1..end {
        let line = line_at(lines, i);
        if !is_blank(line) && indent_of(line) > indent {
            url.push_str(line.trim());
        }
    }
    Some((name, url))
}

/// Split a target's name from its destination at the terminating colon, honoring a backtick-quoted
/// phrase name.
fn split_target_name(rest: &str) -> Option<(String, String)> {
    if let Some(after) = rest.strip_prefix('`') {
        let close = after.find('`')?;
        let name = &after[..close];
        let tail = after.get(close + 1..)?.trim_start();
        let tail = tail.strip_prefix(':')?;
        return Some((name.to_string(), tail.to_string()));
    }
    let colon = rest.find(": ").or_else(|| {
        if rest.ends_with(':') {
            Some(rest.len() - 1)
        } else {
            None
        }
    })?;
    let name = rest.get(..colon)?.replace("\\:", ":");
    let after = rest.get(colon + 1..).unwrap_or("");
    Some((name, after.to_string()))
}

fn parse_anonymous(
    first: &str,
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
) -> String {
    let rest = first.strip_prefix("..").map_or(first, str::trim_start);
    let rest = rest.trim_start_matches('_');
    let rest = rest.trim_start_matches(':');
    let mut url = rest.trim().to_string();
    for i in start + 1..end {
        let line = line_at(lines, i);
        if !is_blank(line) && indent_of(line) > indent {
            url.push_str(line.trim());
        }
    }
    url
}

/// The body region of a footnote or citation: the text after the `.. [label]` marker, plus the
/// dedented continuation, which the second pass parses as block content.
fn footnote_body(lines: &[String], start: usize, end: usize, indent: usize) -> Vec<String> {
    let first = line_at(lines, start);
    let trimmed = first.trim_start();
    let prefix_len = indent + trimmed.find(']').map_or_else(|| trimmed.len(), |p| p + 1);
    explicit_body(lines, start, end, prefix_len)
}

fn parse_substitution(
    first: &str,
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
) -> Option<(String, Substitution)> {
    let trimmed = first.strip_prefix("..").unwrap_or(first).trim_start();
    let rest = trimmed.strip_prefix('|')?;
    let close = rest.find('|')?;
    let name = rest.get(..close)?.to_string();
    let after = rest.get(close + 1..)?.trim_start();
    let coloncolon = after.find("::")?;
    let directive = after.get(..coloncolon)?.trim().to_lowercase();
    let arg_remainder = after.get(coloncolon + 2..).unwrap_or("").trim_start();
    let prefix_len = indent + (first.chars().count() - arg_remainder.chars().count());
    let body = explicit_body(lines, start, end, prefix_len);
    let (argument, options, _content) = split_directive(&body);
    match directive.as_str() {
        "replace" => Some((name, Substitution::Replace(argument))),
        "image" => {
            let (attr, mut alt, url) = image_parts(&argument, &options);
            // A substitution image with no explicit alt text falls back to the substitution name.
            if alt.is_empty() {
                push_text(&mut alt, &name);
            }
            Some((name, Substitution::Image(url, attr, alt)))
        }
        "unicode" => Some((name, Substitution::Replace(unicode_chars(&argument)))),
        _ => Some((name, Substitution::Replace(String::new()))),
    }
}

/// Decode the code points of a `unicode::` substitution argument into their characters.
fn unicode_chars(argument: &str) -> String {
    let mut out = String::new();
    for token in argument.split_whitespace() {
        let hex = token
            .strip_prefix("0x")
            .or_else(|| token.strip_prefix("0X"))
            .or_else(|| token.strip_prefix("U+"))
            .or_else(|| token.strip_prefix("u+"))
            .or_else(|| token.strip_prefix('x'));
        if let Some(code) = hex.and_then(|h| u32::from_str_radix(h, 16).ok()) {
            if let Some(ch) = char::from_u32(code) {
                out.push(ch);
            }
        } else if let Ok(code) = token.parse::<u32>() {
            if let Some(ch) = char::from_u32(code) {
                out.push(ch);
            }
        } else {
            out.push_str(token);
        }
    }
    out
}

// --- block parsing (pass two) ------------------------------------------------------------------

struct Parser<'a> {
    defs: &'a Definitions,
    ext: Extensions,
    heading_styles: Vec<(char, bool)>,
    auto_footnote: usize,
    symbol_footnote: usize,
    anonymous: usize,
}

impl Parser<'_> {
    fn blocks(&mut self, lines: &[String]) -> Vec<Block> {
        let mut out = Vec::new();
        let mut pending_classes: Option<Vec<String>> = None;
        let mut i = 0;
        while i < lines.len() {
            let line = line_at(lines, i);
            if is_blank(line) {
                i += 1;
                continue;
            }
            let before = out.len();
            i = self.block_at(lines, i, &mut out);
            // A preceding empty `class` directive wraps the block just produced.
            if let Some(classes) = pending_classes.take()
                && out.len() > before
            {
                let wrapped = out.split_off(before);
                out.push(class_div(classes, wrapped));
            }
            // An empty `class` directive leaves a marker whose classes wrap the next block.
            if let Some(Block::Div(attr, content)) = out.last()
                && content.is_empty()
                && attr.classes.first().map(String::as_str) == Some(PENDING_CLASS)
            {
                pending_classes = Some(attr.classes.get(1..).unwrap_or(&[]).to_vec());
                out.pop();
            }
        }
        out
    }

    /// Parse the block beginning at line `i`, appending it to `out`, and return the next line index.
    fn block_at(&mut self, lines: &[String], i: usize, out: &mut Vec<Block>) -> usize {
        let line = line_at(lines, i);
        let indent = indent_of(line);

        if indent > 0 {
            return self.block_quote(lines, i, out);
        }

        if let Some(c) = adornment_char(line) {
            // Overline section header.
            let title = line_at(lines, i + 1);
            if !is_blank(title)
                && adornment_char(title).is_none()
                && adornment_char(line_at(lines, i + 2)) == Some(c)
            {
                out.push(self.header(title.trim(), c, true));
                return i + 3;
            }
            if line.trim().chars().count() >= 4
                && (i + 1 >= lines.len() || is_blank(line_at(lines, i + 1)))
            {
                out.push(Block::HorizontalRule);
                return i + 1;
            }
        }

        // Underline section header.
        let next = line_at(lines, i + 1);
        if let Some(c) = adornment_char(next)
            && next.trim().chars().count() >= line.trim().chars().count()
        {
            out.push(self.header(line.trim(), c, false));
            return i + 2;
        }

        if line.starts_with('+')
            && let Some(next_i) = self.grid_table(lines, i, out)
        {
            return next_i;
        }

        if is_simple_table_ruler(line)
            && let Some(next_i) = self.simple_table(lines, i, out)
        {
            return next_i;
        }

        if bullet_content_col(line).is_some() {
            return self.bullet_list(lines, i, out);
        }

        if enumerator(line).is_some() {
            return self.ordered_list(lines, i, out);
        }

        if field_marker(line).is_some() {
            return self.field_list(lines, i, out);
        }

        if classify_explicit(line).is_some() {
            return self.explicit(lines, i, out);
        }

        if line.trim_start().starts_with('|') && matches!(line.chars().nth(1), Some(' ') | None) {
            return self.line_block(lines, i, out);
        }

        // Definition list: a single-line term immediately followed by a more-indented definition.
        if !is_blank(next) && indent_of(next) > 0 {
            return self.definition_list(lines, i, out);
        }

        self.paragraph(lines, i, out)
    }

    fn header(&mut self, title: &str, adornment: char, overline: bool) -> Block {
        let level = self.heading_level(adornment, overline);
        let inlines = self.inlines(title);
        let id = if self.ext.contains(Extension::AutoIdentifiers) {
            carta_ast::slug(&carta_ast::to_plain_text(&inlines))
        } else {
            String::new()
        };
        Block::Header(
            level,
            Attr {
                id,
                classes: Vec::new(),
                attributes: Vec::new(),
            },
            inlines,
        )
    }

    fn heading_level(&mut self, adornment: char, overline: bool) -> i32 {
        let key = (adornment, overline);
        let level = if let Some(pos) = self.heading_styles.iter().position(|s| *s == key) {
            pos + 1
        } else {
            self.heading_styles.push(key);
            self.heading_styles.len()
        };
        i32::try_from(level).unwrap_or(i32::MAX)
    }

    fn block_quote(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
        let base = indent_of(line_at(lines, start));
        let mut end = start;
        let mut i = start;
        while let Some(line) = lines.get(i) {
            if is_blank(line) {
                i += 1;
            } else if indent_of(line) >= base {
                end = i;
                i += 1;
            } else {
                break;
            }
        }
        let region: Vec<String> = (start..=end)
            .filter_map(|j| lines.get(j))
            .map(|l| {
                if is_blank(l) {
                    String::new()
                } else {
                    dedent(l, base)
                }
            })
            .collect();
        let inner = self.blocks(&region);
        out.push(Block::BlockQuote(inner));
        end + 1
    }

    fn paragraph(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
        let mut collected: Vec<&str> = Vec::new();
        let mut i = start;
        while let Some(line) = lines.get(i) {
            if is_blank(line) {
                break;
            }
            // A title underline below an earlier line ends the paragraph at that line.
            if i > start && adornment_char(line).is_some() {
                let prev = line_at(lines, i - 1).trim();
                if line.trim().chars().count() >= prev.chars().count() {
                    break;
                }
            }
            collected.push(line.trim());
            i += 1;
        }
        let text = collected.join("\n");
        let literal = text.trim_end().ends_with("::");
        if literal && let Some((code, next)) = Self::literal_block(lines, i) {
            let trimmed = minimize_colons(&text);
            if !trimmed.is_empty() {
                out.push(Block::Para(self.inlines(&trimmed)));
            }
            out.push(code);
            return next;
        }
        out.push(Block::Para(self.inlines(&text)));
        i
    }

    /// The literal (code) block following a `::` paragraph, when an indented block follows.
    fn literal_block(lines: &[String], from: usize) -> Option<(Block, usize)> {
        let mut i = from;
        while lines.get(i).is_some_and(|l| is_blank(l)) {
            i += 1;
        }
        let line = lines.get(i)?;
        let base = indent_of(line);
        if base == 0 {
            return None;
        }
        let start = i;
        let mut end = i;
        while let Some(l) = lines.get(i) {
            if is_blank(l) {
                i += 1;
            } else if indent_of(l) >= base {
                end = i;
                i += 1;
            } else {
                break;
            }
        }
        let mut text_lines: Vec<String> = (start..=end)
            .filter_map(|j| lines.get(j))
            .map(|l| {
                if is_blank(l) {
                    String::new()
                } else {
                    dedent(l, base)
                }
            })
            .collect();
        while text_lines.last().is_some_and(std::string::String::is_empty) {
            text_lines.pop();
        }
        Some((
            Block::CodeBlock(Attr::default(), text_lines.join("\n")),
            end + 1,
        ))
    }

    fn line_block(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
        let mut entries: Vec<Vec<Inline>> = Vec::new();
        let mut i = start;
        while let Some(line) = lines.get(i) {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix('|') {
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                // Indentation beyond the single separating space is preserved as non-breaking
                // spaces so it survives into the rendered line.
                let leading = rest.chars().take_while(|c| *c == ' ').count();
                let content = format!(
                    "{}{}",
                    "\u{a0}".repeat(leading),
                    rest.trim_start_matches(' ')
                );
                entries.push(self.inlines(&content));
                i += 1;
            } else {
                break;
            }
        }
        out.push(Block::LineBlock(entries));
        i
    }

    fn bullet_list(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
        let marker = line_at(lines, start).chars().next();
        let mut items: Vec<Vec<Block>> = Vec::new();
        let mut i = start;
        while let Some(line) = lines.get(i) {
            if is_blank(line) {
                i += 1;
                continue;
            }
            if indent_of(line) != 0 {
                break;
            }
            if line.chars().next() != marker {
                break;
            }
            let Some(col) = bullet_content_col(line) else {
                break;
            };
            let (region, next) = Self::item_region(lines, i, col);
            items.push(self.blocks(&region));
            i = next;
        }
        compactify(&mut items);
        out.push(Block::BulletList(items));
        i
    }

    fn ordered_list(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
        let Some((start_num, style, delim, _)) = enumerator(line_at(lines, start)) else {
            return self.paragraph(lines, start, out);
        };
        let mut items: Vec<Vec<Block>> = Vec::new();
        let mut i = start;
        while let Some(line) = lines.get(i) {
            if is_blank(line) {
                i += 1;
                continue;
            }
            if indent_of(line) != 0 {
                break;
            }
            let Some((_, s, d, col)) = enumerator(line) else {
                break;
            };
            // An auto-numbered (`#`) item carries no concrete style, so it joins whatever list is
            // open; likewise an auto-numbered list absorbs a later concrete item. Otherwise the
            // style and delimiter must match for the item to belong to the same list.
            let item_auto =
                s == ListNumberStyle::DefaultStyle && d == ListNumberDelim::DefaultDelim;
            let list_auto =
                style == ListNumberStyle::DefaultStyle && delim == ListNumberDelim::DefaultDelim;
            if !(item_auto || list_auto || (style == s && delim == d)) {
                break;
            }
            let (region, next) = Self::item_region(lines, i, col);
            items.push(self.blocks(&region));
            i = next;
        }
        compactify(&mut items);
        out.push(Block::OrderedList(
            ListAttributes {
                start: start_num,
                style,
                delim,
            },
            items,
        ));
        i
    }

    /// The dedented body region of a list item beginning at line `start`, whose content starts at
    /// column `col`.
    fn item_region(lines: &[String], start: usize, col: usize) -> (Vec<String>, usize) {
        let first: String = line_at(lines, start).chars().skip(col).collect();
        let mut region = vec![first];
        let mut end = start;
        let mut i = start + 1;
        while let Some(line) = lines.get(i) {
            if is_blank(line) {
                i += 1;
            } else if indent_of(line) >= col {
                end = i;
                i += 1;
            } else {
                break;
            }
        }
        for j in start + 1..=end {
            let line = line_at(lines, j);
            region.push(if is_blank(line) {
                String::new()
            } else {
                dedent(line, col)
            });
        }
        while region.last().is_some_and(std::string::String::is_empty) {
            region.pop();
        }
        (region, end + 1)
    }

    fn field_list(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
        let mut entries: Vec<(Vec<Inline>, Vec<Block>)> = Vec::new();
        let mut i = start;
        while let Some(line) = lines.get(i) {
            if is_blank(line) {
                i += 1;
                continue;
            }
            if indent_of(line) != 0 {
                break;
            }
            let Some((name, value_col)) = field_marker(line) else {
                break;
            };
            let end = explicit_extent(lines, i, indent_of(line));
            let body = explicit_body(lines, i, end, value_col);
            let term = self.inlines(&name);
            entries.push((term, self.blocks(&body)));
            i = end;
        }
        let mut defs: Vec<Vec<Block>> = entries.iter().map(|(_, blocks)| blocks.clone()).collect();
        compactify(&mut defs);
        let items = entries
            .into_iter()
            .zip(defs)
            .map(|((term, _), blocks)| (term, vec![blocks]))
            .collect();
        out.push(Block::DefinitionList(items));
        i
    }

    fn definition_list(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
        let mut items: Vec<(Vec<Inline>, Vec<Vec<Block>>)> = Vec::new();
        let mut i = start;
        while let Some(line) = lines.get(i) {
            if is_blank(line) {
                i += 1;
                continue;
            }
            if indent_of(line) != 0 {
                break;
            }
            let def = line_at(lines, i + 1);
            if is_blank(def) || indent_of(def) == 0 {
                break;
            }
            let term = self.inlines(line.trim());
            let col = indent_of(def);
            let (region, next) = Self::item_region(lines, i + 1, col);
            items.push((term, vec![self.blocks(&region)]));
            i = next;
        }
        out.push(Block::DefinitionList(items));
        i
    }

    // --- explicit markup ---

    fn explicit(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
        let line = line_at(lines, start);
        let indent = indent_of(line);
        let end = explicit_extent(lines, start, indent);
        if let Some(Explicit::Directive(name)) = classify_explicit(line) {
            self.directive(&name, lines, start, end, out);
        }
        end
    }

    #[allow(clippy::too_many_lines)]
    fn directive(
        &mut self,
        name: &str,
        lines: &[String],
        start: usize,
        end: usize,
        out: &mut Vec<Block>,
    ) {
        let first = line_at(lines, start).trim_start();
        let after = first
            .strip_prefix("..")
            .unwrap_or(first)
            .trim_start()
            .strip_prefix(name)
            .and_then(|r| r.strip_prefix("::"))
            .unwrap_or("");
        let prefix_len = line_at(lines, start).len() - after.len();
        let body = explicit_body(lines, start, end, prefix_len);
        let (argument, options, content) = split_directive(&body);

        match name {
            "raw" => {
                out.push(Block::RawBlock(
                    Format(argument.trim().to_string()),
                    content.join("\n"),
                ));
            }
            "code" | "code-block" | "sourcecode" => {
                let attr = code_attr(&argument, &options);
                let mut text = content.join("\n");
                while text.ends_with('\n') {
                    text.pop();
                }
                out.push(Block::CodeBlock(attr, text));
            }
            "math" => {
                let math = content.join("\n");
                out.push(Block::Para(vec![Inline::Math(
                    MathType::DisplayMath,
                    math.trim().to_string(),
                )]));
            }
            "image" => {
                let (attr, mut alt, url) = image_parts(&argument, &options);
                if alt.is_empty() {
                    alt = vec![Inline::Str("image".to_string())];
                }
                let image = Inline::Image(
                    attr,
                    alt,
                    Target {
                        url,
                        title: String::new(),
                    },
                );
                out.push(Block::Para(vec![Self::wrap_target(image, &options)]));
            }
            "figure" => out.push(self.figure(&argument, &options, &content)),
            "note" | "warning" | "attention" | "caution" | "danger" | "error" | "hint"
            | "important" | "tip" => {
                let title = capitalize(name);
                let mut blocks = vec![Block::Div(
                    Attr {
                        id: String::new(),
                        classes: vec!["title".to_string()],
                        attributes: Vec::new(),
                    },
                    vec![Block::Para(vec![Inline::Str(title)])],
                )];
                blocks.extend(self.blocks(&directive_content(&body)));
                out.push(class_div(vec![name.to_string()], blocks));
            }
            "admonition" => {
                let mut blocks = Vec::new();
                if !argument.trim().is_empty() {
                    blocks.push(Block::Para(self.inlines(argument.trim())));
                }
                blocks.extend(self.blocks(&content));
                out.push(class_div(vec!["admonition".to_string()], blocks));
            }
            "topic" | "sidebar" => {
                let mut blocks = Vec::new();
                if !argument.trim().is_empty() {
                    blocks.push(Block::Para(vec![Inline::Strong(
                        self.inlines(argument.trim()),
                    )]));
                }
                blocks.extend(self.blocks(&content));
                out.push(class_div(vec![name.to_string()], blocks));
            }
            "rubric" => {
                out.push(Block::Para(vec![Inline::Strong(
                    self.inlines(argument.trim()),
                )]));
            }
            "container" => {
                let mut classes = vec!["container".to_string()];
                classes.extend(argument.split_whitespace().map(str::to_string));
                out.push(class_div(classes, self.blocks(&content)));
            }
            "epigraph" | "highlights" | "pull-quote" => {
                out.push(Block::BlockQuote(self.blocks(&content)));
            }
            "compound" => out.extend(self.blocks(&content)),
            "class" => {
                let classes: Vec<String> =
                    argument.split_whitespace().map(str::to_string).collect();
                if content.is_empty() {
                    // Apply the classes to the next sibling block via a marker the loop unwraps.
                    let mut marker = vec![PENDING_CLASS.to_string()];
                    marker.extend(classes);
                    out.push(class_div(marker, Vec::new()));
                } else {
                    out.push(class_div(classes, self.blocks(&content)));
                }
            }
            "role"
            | "default-role"
            | "sectnum"
            | "section-numbering"
            | "meta"
            | "title"
            | "header"
            | "footer"
            | "target-notes"
            | "restructuredtext-test-directive" => {}
            _ => {
                let mut blocks = Vec::new();
                if !argument.trim().is_empty() {
                    blocks.push(Block::Para(self.inlines(argument.trim())));
                }
                blocks.extend(self.blocks(&content));
                out.push(class_div(vec![name.to_string()], blocks));
            }
        }
    }

    fn wrap_target(image: Inline, options: &[(String, String)]) -> Inline {
        if let Some((_, url)) = options.iter().find(|(k, _)| k == "target") {
            Inline::Link(
                Attr::default(),
                vec![image],
                Target {
                    url: url.clone(),
                    title: String::new(),
                },
            )
        } else {
            image
        }
    }

    fn figure(
        &mut self,
        argument: &str,
        options: &[(String, String)],
        content: &[String],
    ) -> Block {
        let (img_attr, alt, url) = image_parts(argument, options);
        let inner = self.blocks(content);
        let mut caption = Caption::default();
        let mut caption_inlines = Vec::new();
        let mut iter = inner.into_iter();
        if let Some(first) = iter.next() {
            let plain = to_plain(first);
            if let Block::Plain(inlines) = &plain {
                caption_inlines.clone_from(inlines);
            }
            caption.long = vec![plain];
        }
        // The image description defaults to the figure's caption when no explicit alt is given.
        let description = if alt.is_empty() { caption_inlines } else { alt };
        let image = Inline::Image(
            img_attr,
            description,
            Target {
                url,
                title: String::new(),
            },
        );
        let mut body = vec![Block::Plain(vec![image])];
        body.extend(iter);
        Block::Figure(figure_attr(options), caption, body)
    }

    /// The trailing `citations` division gathering every citation definition, or `None` when the
    /// document defines no citations.
    fn citation_block(&mut self) -> Option<Block> {
        if self.defs.citations.is_empty() {
            return None;
        }
        let items = self
            .defs
            .citations
            .iter()
            .map(|(label, body)| {
                let term = vec![Inline::Span(
                    Attr {
                        id: label.clone(),
                        classes: vec!["citation-label".to_string()],
                        attributes: Vec::new(),
                    },
                    vec![Inline::Str(label.clone())],
                )];
                (term, vec![self.blocks(body)])
            })
            .collect();
        Some(Block::Div(
            Attr {
                id: "citations".to_string(),
                classes: Vec::new(),
                attributes: Vec::new(),
            },
            vec![Block::DefinitionList(items)],
        ))
    }

    // --- grid tables ---

    // Column widths are small character spans, far inside f64's exact-integer range.
    #[allow(clippy::cast_precision_loss)]
    fn grid_table(
        &mut self,
        lines: &[String],
        start: usize,
        out: &mut Vec<Block>,
    ) -> Option<usize> {
        let columns = grid_columns(line_at(lines, start))?;
        let mut rows_text: Vec<Vec<String>> = Vec::new();
        let mut header_rows = 0usize;
        let mut current: Vec<Vec<String>> = vec![Vec::new(); columns.len()];
        let mut i = start + 1;
        while let Some(line) = lines.get(i) {
            if is_row_separator(line) {
                let is_header = line.contains('=');
                let cells: Vec<String> = current.iter().map(|cell| cell.join("\n")).collect();
                rows_text.push(cells);
                if is_header {
                    header_rows = rows_text.len();
                }
                current = vec![Vec::new(); columns.len()];
                i += 1;
                if !lines.get(i).is_some_and(|l| is_grid_line(l)) {
                    break;
                }
            } else if is_grid_content(line, &columns) {
                for (col, (lo, hi)) in columns.iter().enumerate() {
                    let segment: String = line.chars().take(*hi).skip(*lo).collect();
                    if let Some(slot) = current.get_mut(col) {
                        slot.push(segment.trim_end().to_string());
                    }
                }
                i += 1;
            } else {
                break;
            }
        }
        let total: usize = columns.iter().map(|(lo, hi)| hi.saturating_sub(*lo)).sum();
        let divisor = total.max(72) as f64;
        let col_specs: Vec<ColSpec> = columns
            .iter()
            .map(|(lo, hi)| ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidth((hi.saturating_sub(*lo) + 1) as f64 / divisor),
            })
            .collect();
        let header = if header_rows > 0 {
            rows_text.get(..header_rows).unwrap_or(&[]).to_vec()
        } else {
            Vec::new()
        };
        let body = rows_text.get(header_rows..).unwrap_or(&[]).to_vec();
        let table = Table {
            attr: Attr::default(),
            caption: Caption::default(),
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: header.iter().map(|r| self.grid_row(r)).collect(),
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: body.iter().map(|r| self.grid_row(r)).collect(),
            }],
            foot: TableFoot::default(),
        };
        out.push(Block::Table(Box::new(table)));
        Some(i)
    }

    fn grid_row(&mut self, cells: &[String]) -> Row {
        Row {
            attr: Attr::default(),
            cells: cells.iter().map(|text| self.text_cell(text, 1)).collect(),
        }
    }

    /// Build a cell from its newline-joined text. The shared blank-edges/min-indent normalization is
    /// applied, the text is parsed as block content, and a lone paragraph is demoted to a plain block.
    fn text_cell(&mut self, text: &str, col_span: i32) -> Cell {
        let raw: Vec<String> = text.split('\n').map(str::to_string).collect();
        let trimmed = trim_blank_edges(raw);
        let min_indent = trimmed
            .iter()
            .filter(|l| !is_blank(l))
            .map(|l| indent_of(l))
            .min()
            .unwrap_or(0);
        let region: Vec<String> = trimmed
            .iter()
            .map(|l| {
                if is_blank(l) {
                    String::new()
                } else {
                    dedent(l, min_indent)
                }
            })
            .collect();
        let mut content = self.blocks(&region);
        if let [Block::Para(_)] = content.as_slice()
            && let Some(Block::Para(inlines)) = content.pop()
        {
            content.push(Block::Plain(inlines));
        }
        Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span,
            content,
        }
    }

    // --- simple tables ---

    /// Parse a simple table beginning at its top border. Columns come from the `=` runs of the top
    /// border; the bottom border is the first `=` border followed by a blank line or the end of
    /// input, and any earlier interior `=` border separates the header rows from the body. Returns
    /// `None` (so the caller falls back to paragraph parsing) when no bottom border is found.
    fn simple_table(
        &mut self,
        lines: &[String],
        start: usize,
        out: &mut Vec<Block>,
    ) -> Option<usize> {
        let columns = simple_columns(line_at(lines, start))?;
        let mut header_end: Option<usize> = None;
        let mut bottom: Option<usize> = None;
        let mut i = start + 1;
        while let Some(line) = lines.get(i) {
            if is_equals_border(line) {
                let next_blank = lines.get(i + 1).is_none_or(|l| is_blank(l));
                if next_blank {
                    bottom = Some(i);
                    break;
                }
                if header_end.is_none() {
                    header_end = Some(i);
                }
            }
            i += 1;
        }
        let bottom = bottom?;
        let header_lines: Vec<String> = match header_end {
            Some(end) => (start + 1..end)
                .filter_map(|j| lines.get(j).cloned())
                .collect(),
            None => Vec::new(),
        };
        let body_start = header_end.map_or(start + 1, |end| end + 1);
        let body_lines: Vec<String> = (body_start..bottom)
            .filter_map(|j| lines.get(j).cloned())
            .collect();

        let head_rows = self.simple_rows(&header_lines, &columns);
        let body_rows = self.simple_rows(&body_lines, &columns);

        let col_specs: Vec<ColSpec> = columns
            .iter()
            .map(|_| ColSpec {
                align: Alignment::AlignDefault,
                width: ColWidth::ColWidthDefault,
            })
            .collect();
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
        out.push(Block::Table(Box::new(table)));
        Some(bottom + 1)
    }

    /// Group a region's lines into table rows. A line whose first column carries text starts a new
    /// row; a text line with a blank first column continues the current one. A `-` underline ends the
    /// row above it, joining the columns its filled margins span.
    fn simple_rows(&mut self, lines: &[String], columns: &[(usize, usize)]) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut current: Vec<String> = Vec::new();
        for line in lines {
            if let Some(groups) = span_underline_groups(line, columns) {
                if !current.is_empty() {
                    rows.push(self.simple_row(&current, columns, &groups));
                    current.clear();
                }
                continue;
            }
            if is_blank(line) {
                if !current.is_empty() {
                    current.push(String::new());
                }
                continue;
            }
            if !current.is_empty() && first_column_blank(line, columns) {
                current.push(line.clone());
            } else {
                if !current.is_empty() {
                    let groups = default_groups(columns.len());
                    rows.push(self.simple_row(&current, columns, &groups));
                    current.clear();
                }
                current.push(line.clone());
            }
        }
        if !current.is_empty() {
            let groups = default_groups(columns.len());
            rows.push(self.simple_row(&current, columns, &groups));
        }
        rows
    }

    fn simple_row(
        &mut self,
        row_lines: &[String],
        columns: &[(usize, usize)],
        groups: &[(usize, usize)],
    ) -> Row {
        let last_col = columns.len().saturating_sub(1);
        let cells = groups
            .iter()
            .map(|(a, b)| {
                let lo = columns.get(*a).map_or(0, |c| c.0);
                let hi = if *b >= last_col {
                    usize::MAX
                } else {
                    columns.get(b + 1).map_or(usize::MAX, |c| c.0)
                };
                let text = row_lines
                    .iter()
                    .map(|line| {
                        let cs: Vec<char> = line.chars().collect();
                        let end = hi.min(cs.len());
                        let seg: String = cs
                            .get(lo..end)
                            .map(|s| s.iter().collect())
                            .unwrap_or_default();
                        seg.trim_end().to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.text_cell(&text, i32::try_from(b - a + 1).unwrap_or(1))
            })
            .collect();
        Row {
            attr: Attr::default(),
            cells,
        }
    }

    // --- inline parsing ---

    fn inlines(&mut self, text: &str) -> Vec<Inline> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut pending = String::new();
        let mut pos = 0;
        while pos < chars.len() {
            let ch = chars.get(pos).copied().unwrap_or(' ');
            if ch == '\\' {
                match chars.get(pos + 1) {
                    Some(next) if next.is_whitespace() => pos += 2,
                    Some(next) => {
                        pending.push(*next);
                        pos += 2;
                    }
                    None => {
                        pending.push('\\');
                        pos += 1;
                    }
                }
                continue;
            }
            // A trailing underscore closes a simple hyperlink reference whose name is the run of
            // name characters that has just accumulated.
            if ch == '_'
                && let Some((link, next)) = self.simple_reference(&chars, pos, &mut pending)
            {
                push_text(&mut out, &pending);
                pending.clear();
                out.push(link);
                pos = next;
                continue;
            }
            if let Some((inline, drop_space, next)) = self.try_markup(&chars, pos) {
                push_text(&mut out, &pending);
                pending.clear();
                if drop_space && matches!(out.last(), Some(Inline::Space)) {
                    out.pop();
                }
                out.extend(inline);
                pos = next;
                continue;
            }
            pending.push(ch);
            pos += 1;
        }
        push_text(&mut out, &pending);
        trim_inline_ends(&mut out);
        out
    }

    /// Attempt to parse inline markup at `pos`. On success returns the produced inlines, whether a
    /// directly preceding space should be dropped (footnotes), and the index past the construct.
    fn try_markup(&mut self, chars: &[char], pos: usize) -> Option<(Vec<Inline>, bool, usize)> {
        let ch = chars.get(pos).copied()?;
        let prev = pos.checked_sub(1).and_then(|p| chars.get(p)).copied();
        match ch {
            '`' => self.backtick(chars, pos, prev),
            '*' => Self::emphasis(chars, pos, prev),
            '|' => self.substitution(chars, pos, prev),
            '[' => self.note_reference(chars, pos, prev),
            ':' => self.role_prefix(chars, pos, prev),
            _ => None,
        }
    }

    fn emphasis(
        chars: &[char],
        pos: usize,
        prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        if !inline_start_ok(prev) {
            return None;
        }
        let strong = chars.get(pos + 1) == Some(&'*');
        let delim = if strong { 2 } else { 1 };
        if chars.get(pos + delim).is_some_and(|c| c.is_whitespace()) {
            return None;
        }
        let (content, end) = find_close(chars, pos + delim, if strong { "**" } else { "*" })?;
        let inner = literal_text(&content);
        let node = if strong {
            Inline::Strong(inner)
        } else {
            Inline::Emph(inner)
        };
        Some((vec![node], false, end))
    }

    fn backtick(
        &mut self,
        chars: &[char],
        pos: usize,
        prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        // An inline literal is recognized wherever its delimiters appear, even mid-word; the other
        // backtick constructs require a boundary before their opening delimiter.
        if chars.get(pos + 1) == Some(&'`') {
            let (content, end) = find_close_literal(chars, pos + 2, "``")?;
            return Some((vec![Inline::Code(Attr::default(), content)], false, end));
        }
        if !inline_start_ok(prev) {
            return None;
        }
        let (content, mut end) = find_close_literal(chars, pos + 1, "`")?;
        // A trailing underscore turns interpreted text into a hyperlink reference.
        if chars.get(end) == Some(&'_') {
            let anonymous = chars.get(end + 1) == Some(&'_');
            end += if anonymous { 2 } else { 1 };
            return Some((vec![self.phrase_reference(&content, anonymous)], false, end));
        }
        // A trailing role applies to the interpreted text.
        if chars.get(end) == Some(&':')
            && let Some((role, role_end)) = parse_role(chars, end)
        {
            let inline = self.apply_role(&role, &content);
            return Some((vec![inline], false, role_end));
        }
        Some((
            vec![self.apply_role("title-reference", &content)],
            false,
            end,
        ))
    }

    fn role_prefix(
        &mut self,
        chars: &[char],
        pos: usize,
        prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        if !inline_start_ok(prev) {
            return None;
        }
        let (role, after) = parse_role(chars, pos)?;
        if chars.get(after) != Some(&'`') {
            return None;
        }
        let (content, end) = find_close_literal(chars, after + 1, "`")?;
        Some((vec![self.apply_role(&role, &content)], false, end))
    }

    fn apply_role(&mut self, role: &str, content: &str) -> Inline {
        match role {
            "emphasis" => Inline::Emph(self.inlines(content)),
            "strong" => Inline::Strong(self.inlines(content)),
            "subscript" | "sub" => Inline::Subscript(self.inlines(content)),
            "superscript" | "sup" => Inline::Superscript(self.inlines(content)),
            "literal" | "code" => Inline::Code(Attr::default(), content.to_string()),
            "math" => Inline::Math(MathType::InlineMath, content.to_string()),
            "title-reference" | "title" | "t" => Inline::Span(
                Attr {
                    id: String::new(),
                    classes: vec!["title-ref".to_string()],
                    attributes: Vec::new(),
                },
                self.inlines(content),
            ),
            // An unrecognized role keeps its content verbatim, tagged with the role name so the
            // information survives a round-trip.
            other => Inline::Code(
                Attr {
                    id: String::new(),
                    classes: vec!["interpreted-text".to_string()],
                    attributes: vec![("role".to_string(), other.to_string())],
                },
                content.to_string(),
            ),
        }
    }

    fn substitution(
        &mut self,
        chars: &[char],
        pos: usize,
        prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        if !inline_start_ok(prev) {
            return None;
        }
        if chars.get(pos + 1).is_some_and(|c| c.is_whitespace()) {
            return None;
        }
        let (name, mut end) = find_close_literal(chars, pos + 1, "|")?;
        if chars.get(end) == Some(&'_') {
            end += 1;
        }
        match self.defs.substitutions.get(&normalize_name(&name)) {
            Some(Substitution::Replace(text)) => {
                let inlines = self.inlines(text);
                // A replacement that expands to several inlines is kept together as one unit.
                let replacement = match inlines.len() {
                    1 => inlines,
                    _ => vec![Inline::Span(Attr::default(), inlines)],
                };
                Some((replacement, false, end))
            }
            Some(Substitution::Image(url, attr, alt)) => Some((
                vec![Inline::Image(
                    attr.clone(),
                    alt.clone(),
                    Target {
                        url: url.clone(),
                        title: String::new(),
                    },
                )],
                false,
                end,
            )),
            None => {
                let literal = format!("|{name}|");
                Some((vec![Inline::Str(literal)], false, end))
            }
        }
    }

    fn note_reference(
        &mut self,
        chars: &[char],
        pos: usize,
        prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        if !inline_start_ok(prev) {
            return None;
        }
        let (label, after) = find_close_literal(chars, pos + 1, "]")?;
        if chars.get(after) != Some(&'_') {
            return None;
        }
        let end = after + 1;
        if !inline_end_ok(chars.get(end).copied()) {
            return None;
        }
        if is_citation_label(&label) {
            let url = format!("#{label}");
            let link = Inline::Link(
                Attr {
                    id: String::new(),
                    classes: vec!["citation".to_string()],
                    attributes: Vec::new(),
                },
                vec![Inline::Str(format!("[{label}]"))],
                Target {
                    url,
                    title: String::new(),
                },
            );
            return Some((vec![link], false, end));
        }
        let body = self.footnote_body_for(&label)?;
        let blocks = self.blocks(&body);
        Some((vec![Inline::Note(blocks)], true, end))
    }

    fn footnote_body_for(&mut self, label: &str) -> Option<Vec<String>> {
        if label == "#" {
            let body = self.defs.auto_footnotes.get(self.auto_footnote)?.clone();
            self.auto_footnote += 1;
            Some(body)
        } else if label == "*" {
            let body = self
                .defs
                .symbol_footnotes
                .get(self.symbol_footnote)?
                .clone();
            self.symbol_footnote += 1;
            Some(body)
        } else {
            self.defs.footnotes.get(label).cloned()
        }
    }

    fn phrase_reference(&mut self, text: &str, anonymous: bool) -> Inline {
        let (label, url) = split_embedded_uri(text);
        let display = if label.trim().is_empty() {
            url.clone().unwrap_or_default()
        } else {
            label.clone()
        };
        let target = match url {
            Some(url) => url,
            None if anonymous => self.next_anonymous(),
            None => self.resolve_target(&label),
        };
        Inline::Link(
            Attr::default(),
            self.inlines(&display),
            Target {
                url: target,
                title: String::new(),
            },
        )
    }

    /// Close a simple reference `name_` (or anonymous `name__`) whose name is the trailing run of
    /// name characters already accumulated in `pending`. The name is removed from `pending` and the
    /// link returned, with the index past the closing underscore(s).
    fn simple_reference(
        &mut self,
        chars: &[char],
        pos: usize,
        pending: &mut String,
    ) -> Option<(Inline, usize)> {
        let anonymous = chars.get(pos + 1) == Some(&'_');
        let after = pos + if anonymous { 2 } else { 1 };
        if !inline_end_ok(chars.get(after).copied()) {
            return None;
        }
        let trailing: String = pending
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '+'))
            .collect();
        if trailing.is_empty() {
            return None;
        }
        let name: String = trailing.chars().rev().collect();
        let keep = pending.len().saturating_sub(name.len());
        pending.truncate(keep);
        let url = if anonymous {
            self.next_anonymous()
        } else {
            self.resolve_target(&name)
        };
        let link = Inline::Link(
            Attr::default(),
            vec![Inline::Str(name)],
            Target {
                url,
                title: String::new(),
            },
        );
        Some((link, after))
    }

    fn resolve_target(&self, name: &str) -> String {
        self.defs
            .targets
            .get(&normalize_name(name))
            .cloned()
            .unwrap_or_default()
    }

    fn next_anonymous(&mut self) -> String {
        let idx = self.anonymous;
        self.anonymous += 1;
        self.defs.anonymous.get(idx).cloned().unwrap_or_default()
    }
}

// --- directive helpers -------------------------------------------------------------------------

/// Split a directive body into its argument (first line), its options (the immediately following
/// `:key: value` lines), and its content (everything after the blank separator).
fn split_directive(body: &[String]) -> (String, Vec<(String, String)>, Vec<String>) {
    let mut idx = 0;
    let mut argument = String::new();
    if let Some(first) = body.first()
        && !first.is_empty()
        && option_line(first).is_none()
    {
        argument.clone_from(first);
        idx = 1;
    }
    let mut options = Vec::new();
    while let Some(line) = body.get(idx) {
        match option_line(line) {
            Some(option) => {
                options.push(option);
                idx += 1;
            }
            None => break,
        }
    }
    while body.get(idx).is_some_and(std::string::String::is_empty) {
        idx += 1;
    }
    let content = body.get(idx..).unwrap_or(&[]).to_vec();
    (argument, options, content)
}

/// The block content of a directive whose first-line text is body content rather than an argument:
/// the body with any leading option lines (and the blank line that follows them) removed.
fn directive_content(body: &[String]) -> Vec<String> {
    let mut idx = 0;
    while body.get(idx).is_some_and(|l| option_line(l).is_some()) {
        idx += 1;
    }
    if idx > 0 {
        while body.get(idx).is_some_and(std::string::String::is_empty) {
            idx += 1;
        }
    }
    body.get(idx..).unwrap_or(&[]).to_vec()
}

/// Parse a directive option line `:key: value`, returning the key and its trimmed value.
fn option_line(line: &str) -> Option<(String, String)> {
    let (name, col) = field_marker(line)?;
    let value: String = line.chars().skip(col).collect();
    Some((name, value.trim().to_string()))
}

/// Build the attributes of a code block from its language argument and options.
fn code_attr(argument: &str, options: &[(String, String)]) -> Attr {
    let mut classes = Vec::new();
    let lang = argument.trim();
    if !lang.is_empty() {
        classes.push(lang.to_string());
    }
    let mut id = String::new();
    let mut attributes = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "name" => id.clone_from(value),
            "class" => classes.extend(value.split_whitespace().map(str::to_string)),
            other => attributes.push((other.to_string(), value.clone())),
        }
    }
    Attr {
        id,
        classes,
        attributes,
    }
}

/// Build the attributes, description, and destination of an image from its URI argument and options.
fn image_parts(argument: &str, options: &[(String, String)]) -> (Attr, Vec<Inline>, String) {
    let url = argument.split_whitespace().collect::<Vec<_>>().join("");
    let mut id = String::new();
    let mut classes = Vec::new();
    let mut attributes = Vec::new();
    let mut description = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "alt" => description = vec![Inline::Str(value.clone())],
            "name" => id.clone_from(value),
            "class" => classes.extend(value.split_whitespace().map(str::to_string)),
            "width" => attributes.push(("width".to_string(), value.clone())),
            "height" => attributes.push(("height".to_string(), value.clone())),
            _ => {}
        }
    }
    (
        Attr {
            id,
            classes,
            attributes,
        },
        description,
        url,
    )
}

/// Build the attributes of a figure from its options.
fn figure_attr(options: &[(String, String)]) -> Attr {
    let mut id = String::new();
    let mut classes = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "name" => id.clone_from(value),
            "figclass" | "class" => classes.extend(value.split_whitespace().map(str::to_string)),
            _ => {}
        }
    }
    Attr {
        id,
        classes,
        attributes: Vec::new(),
    }
}

fn class_div(classes: Vec<String>, blocks: Vec<Block>) -> Block {
    Block::Div(
        Attr {
            id: String::new(),
            classes,
            attributes: Vec::new(),
        },
        blocks,
    )
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Demote a leading paragraph to a plain block, leaving any other block unchanged.
fn to_plain(block: Block) -> Block {
    match block {
        Block::Para(inlines) => Block::Plain(inlines),
        other => other,
    }
}

// --- list looseness ----------------------------------------------------------------------------

/// Tighten a list: when no item holds two or more paragraphs, each item's paragraphs become plain
/// blocks so the list renders compactly.
fn compactify(items: &mut [Vec<Block>]) {
    let loose = items
        .iter()
        .any(|item| item.iter().filter(|b| matches!(b, Block::Para(_))).count() >= 2);
    if loose {
        return;
    }
    for item in items.iter_mut() {
        for block in item.iter_mut() {
            if let Block::Para(inlines) = block {
                *block = Block::Plain(std::mem::take(inlines));
            }
        }
    }
}

/// Trim the literal-block marker from a paragraph's text: a trailing `::` is removed entirely when
/// preceded by whitespace (or when it is all the paragraph holds), and replaced by a single colon
/// otherwise.
fn minimize_colons(text: &str) -> String {
    let trimmed = text.trim_end();
    let body = trimmed.strip_suffix("::").unwrap_or(trimmed);
    if body.trim().is_empty() {
        return String::new();
    }
    if body.ends_with(char::is_whitespace) {
        body.trim_end().to_string()
    } else {
        format!("{body}:")
    }
}

// --- inline text helpers -----------------------------------------------------------------------

/// Append raw text to an inline sequence, splitting on the regular space into words and single
/// spaces, with embedded newlines becoming soft breaks and space runs collapsing. Other whitespace
/// (such as a non-breaking space) stays part of its surrounding word.
fn push_text(out: &mut Vec<Inline>, text: &str) {
    let mut word = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word)));
            }
            out.push(Inline::SoftBreak);
        } else if ch == ' ' {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word)));
            }
            if !matches!(out.last(), None | Some(Inline::Space | Inline::SoftBreak)) {
                out.push(Inline::Space);
            }
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word));
    }
}

/// Convert verbatim text into inlines without interpreting further markup: emphasis and strong
/// spans do not nest, so their content is plain text with only backslash escapes resolved.
fn literal_text(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut resolved = String::new();
    let mut pos = 0;
    while let Some(&ch) = chars.get(pos) {
        if ch == '\\' {
            match chars.get(pos + 1) {
                Some(next) if next.is_whitespace() => pos += 2,
                Some(next) => {
                    resolved.push(*next);
                    pos += 2;
                }
                None => {
                    resolved.push('\\');
                    pos += 1;
                }
            }
            continue;
        }
        resolved.push(ch);
        pos += 1;
    }
    let mut out = Vec::new();
    push_text(&mut out, &resolved);
    trim_inline_ends(&mut out);
    out
}

/// Whether the character before a markup start string allows it to begin markup: a boundary, a
/// whitespace, or one of the opening punctuation characters.
fn inline_start_ok(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => {
            c.is_whitespace() || matches!(c, '-' | ':' | '/' | '\'' | '"' | '<' | '(' | '[' | '{')
        }
    }
}

/// Whether the character after a markup end string allows it to end markup: a boundary, a
/// whitespace, or one of the closing punctuation characters.
fn inline_end_ok(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || matches!(
                    c,
                    '-' | '.'
                        | ','
                        | ':'
                        | ';'
                        | '!'
                        | '?'
                        | '\\'
                        | '/'
                        | '\''
                        | '"'
                        | ')'
                        | ']'
                        | '}'
                        | '>'
                )
        }
    }
}

fn matches_at(chars: &[char], at: usize, delim: &[char]) -> bool {
    delim
        .iter()
        .enumerate()
        .all(|(k, d)| chars.get(at + k) == Some(d))
}

/// Find the closing delimiter of a parsed-content span (emphasis, strong): the next occurrence
/// preceded by a non-whitespace character and followed by an end-context character. Returns the raw
/// inner text and the index past the closing delimiter.
fn find_close(chars: &[char], start: usize, delim: &str) -> Option<(String, usize)> {
    let dchars: Vec<char> = delim.chars().collect();
    let mut i = start;
    while i < chars.len() {
        if chars.get(i) == Some(&'\\') {
            i += 2;
            continue;
        }
        if matches_at(chars, i, &dchars) {
            let before = i.checked_sub(1).and_then(|p| chars.get(p)).copied();
            let after = chars.get(i + dchars.len()).copied();
            if before.is_some_and(|c| !c.is_whitespace()) && inline_end_ok(after) {
                let content: String = chars.get(start..i)?.iter().collect();
                return Some((content, i + dchars.len()));
            }
        }
        i += 1;
    }
    None
}

/// Find the closing delimiter of a verbatim span (inline literal, interpreted text, substitution,
/// bracketed label): the next occurrence of the delimiter. Returns the verbatim inner text and the
/// index past the closing delimiter.
fn find_close_literal(chars: &[char], start: usize, delim: &str) -> Option<(String, usize)> {
    let dchars: Vec<char> = delim.chars().collect();
    let mut i = start;
    while i < chars.len() {
        if matches_at(chars, i, &dchars) {
            let content: String = chars.get(start..i)?.iter().collect();
            return Some((content, i + dchars.len()));
        }
        i += 1;
    }
    None
}

/// Parse a role token `:name:` beginning at `pos` (which must be the opening colon), returning the
/// role name and the index past the closing colon.
fn parse_role(chars: &[char], pos: usize) -> Option<(String, usize)> {
    if chars.get(pos) != Some(&':') {
        return None;
    }
    let mut name = String::new();
    let mut i = pos + 1;
    while let Some(&c) = chars.get(i) {
        if c == ':' {
            if name.is_empty() {
                return None;
            }
            return Some((name, i + 1));
        }
        if c.is_alphanumeric() || matches!(c, '-' | '_' | '+' | '.') {
            name.push(c);
            i += 1;
        } else {
            return None;
        }
    }
    None
}

/// Split interpreted-text content into its display label and an optional embedded destination
/// `<uri>`.
fn split_embedded_uri(text: &str) -> (String, Option<String>) {
    let trimmed = text.trim_end();
    if trimmed.ends_with('>')
        && let Some(open) = text.rfind('<')
        && let Some(close) = text.rfind('>')
        && open < close
    {
        let url = text.get(open + 1..close).unwrap_or("").trim().to_string();
        let label = text.get(..open).unwrap_or("").trim().to_string();
        return (label, Some(url));
    }
    (text.to_string(), None)
}

// --- grid table helpers ------------------------------------------------------------------------

/// Parse a grid table's top border into the inclusive-exclusive character ranges of its columns.
fn grid_columns(border: &str) -> Option<Vec<(usize, usize)>> {
    let chars: Vec<char> = border.chars().collect();
    if chars.first() != Some(&'+') {
        return None;
    }
    let pluses: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '+')
        .map(|(i, _)| i)
        .collect();
    if pluses.len() < 2 {
        return None;
    }
    let mut columns = Vec::new();
    for window in pluses.windows(2) {
        let left = window.first().copied()?;
        let right = window.get(1).copied()?;
        let lo = left + 1;
        if right <= lo {
            return None;
        }
        for k in lo..right {
            if chars.get(k) != Some(&'-') {
                return None;
            }
        }
        columns.push((lo, right));
    }
    Some(columns)
}

fn is_grid_line(line: &str) -> bool {
    line.starts_with('+') || line.starts_with('|')
}

/// Whether a line is a grid table row separator: a run of `+`, `-`, and `=` beginning with `+`.
fn is_row_separator(line: &str) -> bool {
    line.starts_with('+') && line.chars().all(|c| matches!(c, '+' | '-' | '='))
}

/// Whether a line carries grid table cell content: a `|`-led line whose column boundaries align with
/// the border.
fn is_grid_content(line: &str, columns: &[(usize, usize)]) -> bool {
    let chars: Vec<char> = line.chars().collect();
    if chars.first() != Some(&'|') {
        return false;
    }
    for (lo, hi) in columns {
        let left = lo.checked_sub(1).and_then(|p| chars.get(p)).copied();
        if !matches!(left, Some('|' | '+')) {
            return false;
        }
        if !matches!(chars.get(*hi).copied(), Some('|' | '+')) {
            return false;
        }
    }
    true
}

/// Whether a line is a simple-table ruler: two or more space-separated runs of `=`.
fn is_simple_table_ruler(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.starts_with('=')
        && trimmed.chars().all(|c| c == '=' || c == ' ')
        && trimmed.contains(' ')
}

/// The inclusive-exclusive character ranges of a simple table's columns, from the `=` runs of its
/// top border. `None` unless the border is made solely of `=` runs and spaces and has at least two
/// columns (the minimum that distinguishes a table from a section adornment).
fn simple_columns(border: &str) -> Option<Vec<(usize, usize)>> {
    let chars: Vec<char> = border.chars().collect();
    let mut columns = Vec::new();
    let mut i = 0;
    while let Some(c) = chars.get(i) {
        match c {
            '=' => {
                let start = i;
                while chars.get(i) == Some(&'=') {
                    i += 1;
                }
                columns.push((start, i));
            }
            ' ' => i += 1,
            _ => return None,
        }
    }
    (columns.len() >= 2).then_some(columns)
}

/// Whether a line is a `=` border: a non-empty run of `=` and spaces with no other content.
fn is_equals_border(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '=' || c == ' ')
}

/// Whether a line's first column holds no text, marking it a continuation of the row above.
fn first_column_blank(line: &str, columns: &[(usize, usize)]) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let lo = columns.first().map_or(0, |c| c.0);
    let hi = columns.get(1).map_or(chars.len(), |c| c.0);
    (lo..hi).all(|p| chars.get(p).is_none_or(|c| c.is_whitespace()))
}

/// Each column standing alone, the column grouping a row carries when no span underline joins any.
fn default_groups(count: usize) -> Vec<(usize, usize)> {
    (0..count).map(|i| (i, i)).collect()
}

/// The column groups a `-` underline imposes on the row above it: a margin filled with `-` joins the
/// columns on either side into one span. `None` unless the line is solely `-` and spaces with at
/// least one `-`, which is what distinguishes an underline from cell text.
fn span_underline_groups(line: &str, columns: &[(usize, usize)]) -> Option<Vec<(usize, usize)>> {
    let chars: Vec<char> = line.chars().collect();
    let has_dash = chars.contains(&'-');
    if !has_dash || !chars.iter().all(|c| matches!(c, '-' | ' ')) {
        return None;
    }
    let mut groups = Vec::new();
    let mut group_start = 0;
    let n = columns.len();
    for i in 0..n.saturating_sub(1) {
        let left_end = columns.get(i).map_or(0, |c| c.1);
        let right_start = columns.get(i + 1).map_or(left_end, |c| c.0);
        let filled = (left_end..right_start).any(|p| chars.get(p) == Some(&'-'));
        if !filled {
            groups.push((group_start, i));
            group_start = i + 1;
        }
    }
    groups.push((group_start, n.saturating_sub(1)));
    Some(groups)
}

fn trim_blank_edges(mut lines: Vec<String>) -> Vec<String> {
    while lines.first().is_some_and(|l| is_blank(l)) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| is_blank(l)) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Vec<Block> {
        parse_ext(input, Extensions::default())
    }

    fn parse_ext(input: &str, extensions: Extensions) -> Vec<Block> {
        let reader = RstReader;
        let mut options = ReaderOptions::default();
        options.extensions = extensions;
        reader
            .read(input, &options)
            .expect("reader does not fail")
            .blocks
    }

    fn with_auto_ids() -> Extensions {
        let mut extensions = Extensions::default();
        extensions.insert(Extension::AutoIdentifiers);
        extensions
    }

    #[test]
    fn paragraph_with_inline_markup() {
        let blocks = parse("A *word* and **two** and ``lit``.\n");
        assert_eq!(
            blocks,
            vec![Block::Para(vec![
                Inline::Str("A".into()),
                Inline::Space,
                Inline::Emph(vec![Inline::Str("word".into())]),
                Inline::Space,
                Inline::Str("and".into()),
                Inline::Space,
                Inline::Strong(vec![Inline::Str("two".into())]),
                Inline::Space,
                Inline::Str("and".into()),
                Inline::Space,
                Inline::Code(Attr::default(), "lit".into()),
                Inline::Str(".".into()),
            ])]
        );
    }

    #[test]
    fn underline_section_header_gets_slug_id() {
        let blocks = parse_ext("Title\n=====\n", with_auto_ids());
        assert_eq!(
            blocks,
            vec![Block::Header(
                1,
                Attr {
                    id: "title".into(),
                    classes: Vec::new(),
                    attributes: Vec::new(),
                },
                vec![Inline::Str("Title".into())],
            )]
        );
    }

    #[test]
    fn header_levels_follow_first_seen_adornment_order() {
        let blocks = parse("A\n=\n\nB\n-\n\nC\n=\n");
        let levels: Vec<i32> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Header(level, _, _) => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![1, 2, 1]);
    }

    #[test]
    fn transition_is_a_horizontal_rule() {
        let blocks = parse("Above\n\n----\n\nBelow\n");
        assert_eq!(blocks.get(1), Some(&Block::HorizontalRule));
    }

    #[test]
    fn bullet_list_is_tight() {
        let blocks = parse("- one\n- two\n");
        assert_eq!(
            blocks,
            vec![Block::BulletList(vec![
                vec![Block::Plain(vec![Inline::Str("one".into())])],
                vec![Block::Plain(vec![Inline::Str("two".into())])],
            ])]
        );
    }

    #[test]
    fn enumerated_list_carries_style_and_start() {
        let blocks = parse("3. third\n4. fourth\n");
        match blocks.first() {
            Some(Block::OrderedList(attrs, items)) => {
                assert_eq!(attrs.start, 3);
                assert_eq!(attrs.style, ListNumberStyle::Decimal);
                assert_eq!(attrs.delim, ListNumberDelim::Period);
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected ordered list, got {other:?}"),
        }
    }

    #[test]
    fn literal_block_drops_marker_paragraph() {
        let blocks = parse("::\n\n    code line\n");
        assert_eq!(
            blocks,
            vec![Block::CodeBlock(Attr::default(), "code line".into())]
        );
    }

    #[test]
    fn literal_block_keeps_single_colon() {
        let blocks = parse("Example::\n\n    code\n");
        assert_eq!(
            blocks.first(),
            Some(&Block::Para(vec![Inline::Str("Example:".into())]))
        );
    }

    #[test]
    fn field_list_becomes_definition_list() {
        let blocks = parse(":Author: Me\n");
        assert_eq!(
            blocks,
            vec![Block::DefinitionList(vec![(
                vec![Inline::Str("Author".into())],
                vec![vec![Block::Plain(vec![Inline::Str("Me".into())])]],
            )])]
        );
    }

    #[test]
    fn named_target_resolves_reference() {
        let blocks = parse("See website_.\n\n.. _website: https://example.org\n");
        match blocks.first() {
            Some(Block::Para(inlines)) => {
                let link = inlines.iter().find(|i| matches!(i, Inline::Link(..)));
                assert_eq!(
                    link,
                    Some(&Inline::Link(
                        Attr::default(),
                        vec![Inline::Str("website".into())],
                        Target {
                            url: "https://example.org".into(),
                            title: String::new(),
                        },
                    ))
                );
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn footnote_reference_inlines_the_note() {
        let blocks = parse("Ref [1]_\n\n.. [1] The note.\n");
        match blocks.first() {
            Some(Block::Para(inlines)) => {
                assert!(inlines.iter().any(|i| matches!(i, Inline::Note(_))));
                // The space before the note marker is dropped.
                assert_eq!(inlines.first(), Some(&Inline::Str("Ref".into())));
                assert!(matches!(inlines.get(1), Some(Inline::Note(_))));
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn comment_produces_no_output() {
        let blocks = parse(".. This is a comment.\n");
        assert!(blocks.is_empty());
    }

    #[test]
    fn interpreted_text_defaults_to_title_reference() {
        let blocks = parse("A `book title` here.\n");
        match blocks.first() {
            Some(Block::Para(inlines)) => {
                assert!(inlines.iter().any(|i| matches!(
                    i,
                    Inline::Span(attr, _) if attr.classes == vec!["title-ref".to_string()]
                )));
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn auto_identifiers_off_yields_no_id() {
        let blocks = parse_ext("Title\n=====\n", Extensions::empty());
        match blocks.first() {
            Some(Block::Header(_, attr, _)) => assert!(attr.id.is_empty()),
            other => panic!("expected header, got {other:?}"),
        }
    }
}
