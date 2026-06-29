//! reStructuredText reader.
//!
//! Parsing runs in two structural passes. The first pass scans the whole input for the explicit
//! markup that defines document-global references — hyperlink targets, substitution definitions,
//! footnotes, and citations — since a reference may resolve against a definition that appears later.
//! The second pass walks the line structure block by block, building the document tree and resolving
//! each reference against the collected definitions. Inline markup is parsed from the raw text of
//! each leaf during the second pass.

use std::collections::{BTreeMap, VecDeque};

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Format, Inline,
    ListAttributes, ListNumberDelim, ListNumberStyle, MathType, QuoteType, Row, Table, TableBody,
    TableFoot, TableHead, Target,
};
use carta_core::{Extension, Extensions, Reader, ReaderOptions, Result};

use crate::heading_ids::{IdRegistry, IdScheme};
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
            ids: IdRegistry::default(),
            auto_footnote: 0,
            symbol_footnote: 0,
            anonymous: 0,
            custom_roles: BTreeMap::new(),
            default_role: DEFAULT_ROLE.to_string(),
            include_depth: 0,
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
    // A lone letter is ambiguous between alphabetic and Roman numbering; it defaults to alphabetic
    // unless it is `i`/`I`, the only single letter taken as Roman. A multi-letter token that is a
    // valid Roman numeral (`iv`, `xii`) is Roman.
    let mut chars = numeral.chars();
    let single = chars.next()?;
    if chars.next().is_none() && single.is_ascii_alphabetic() && !matches!(single, 'i' | 'I') {
        let ordinal = i32::from((single.to_ascii_lowercase() as u8) - b'a' + 1);
        let style = if single.is_ascii_uppercase() {
            ListNumberStyle::UpperAlpha
        } else {
            ListNumberStyle::LowerAlpha
        };
        return Some((style, ordinal));
    }
    if let Some(value) = roman_value(numeral) {
        let style = if numeral.chars().all(|c| c.is_ascii_uppercase()) {
            ListNumberStyle::UpperRoman
        } else {
            ListNumberStyle::LowerRoman
        };
        return Some((style, value));
    }
    None
}

/// Whether `ch` is one of the letters that form a Roman numeral.
fn is_roman_letter(ch: char) -> bool {
    matches!(
        ch.to_ascii_lowercase(),
        'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm'
    )
}

/// The leading enumerator numeral of `line` — the token before its delimiter — when `line` opens
/// with an enumerator. Used to reinterpret an ambiguous single-letter enumerator in the context of
/// an already-established list style.
fn enum_numeral(line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let start = usize::from(chars.first() == Some(&'('));
    let mut end = start;
    while chars
        .get(end)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '#')
    {
        end += 1;
    }
    let numeral: String = chars.get(start..end)?.iter().collect();
    if numeral.is_empty() {
        None
    } else {
        Some(numeral)
    }
}

/// Whether a single-letter `numeral` continues a list whose established `style` it does not match on
/// its own: any letter (of the style's case) continues an alphabetic list; only a Roman-numeral
/// letter continues a Roman list.
fn letter_continues(numeral: &str, style: ListNumberStyle) -> bool {
    let mut chars = numeral.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return false;
    };
    if !ch.is_ascii_alphabetic() {
        return false;
    }
    let upper = ch.is_ascii_uppercase();
    match style {
        ListNumberStyle::UpperAlpha => upper,
        ListNumberStyle::LowerAlpha => !upper,
        ListNumberStyle::UpperRoman => upper && is_roman_letter(ch),
        ListNumberStyle::LowerRoman => !upper && is_roman_letter(ch),
        _ => false,
    }
}

/// Whether the enumerator opening `line` can belong to a list whose first item established `style`
/// and `delim`. An auto-numbered (`#`) item joins any list and vice versa; otherwise the delimiter
/// must match and the style must match directly or by an ambiguous single letter adopting it.
fn enum_compatible(line: &str, style: ListNumberStyle, delim: ListNumberDelim) -> bool {
    let Some((_, s, d, _)) = enumerator(line) else {
        return false;
    };
    let item_auto = s == ListNumberStyle::DefaultStyle && d == ListNumberDelim::DefaultDelim;
    let list_auto =
        style == ListNumberStyle::DefaultStyle && delim == ListNumberDelim::DefaultDelim;
    let style_ok = style == s || enum_numeral(line).is_some_and(|n| letter_continues(&n, style));
    item_auto || list_auto || (style_ok && delim == d)
}

/// Whether the enumerated-list item whose first line is `lines[idx]` (content column `col`) is a
/// well-formed item rather than the opening of an ordinary wrapped paragraph. The line after the
/// item's first line must be blank, indented into the item, or itself a matching sibling enumerator;
/// an under-indented line of ordinary text means the construct is a paragraph, not a list.
fn item_well_formed(
    lines: &[String],
    idx: usize,
    col: usize,
    style: ListNumberStyle,
    delim: ListNumberDelim,
) -> bool {
    let next = line_at(lines, idx + 1);
    if is_blank(next) || indent_of(next) >= col {
        return true;
    }
    enum_compatible(next, style, delim)
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

/// An option-list marker: an option group (a comma-separated run of `-a`, `-fARG`, `-f ARG`,
/// `--word`, `--word=ARG`, or `/S` options) that fully fills the line up to the first run of two
/// or more spaces (or the end of line). Returns the option-group text and the column at which the
/// description body begins. The group must consume its entire candidate span — a trailing token
/// after a single-space gap (e.g. `-f FILE extra`) is ordinary prose, not an option list.
fn option_marker(line: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let gap = chars.windows(2).position(|pair| pair == [' ', ' ']);
    let candidate_end = gap.unwrap_or(chars.len());
    let candidate: String = chars.get(..candidate_end)?.iter().collect();
    let candidate = candidate.trim_end();
    if !valid_option_group(candidate) {
        return None;
    }
    let value_col = match gap {
        Some(g) => {
            let mut v = g;
            while chars.get(v) == Some(&' ') {
                v += 1;
            }
            v
        }
        None => candidate.chars().count(),
    };
    Some((candidate.to_string(), value_col))
}

/// Whether `text` is a complete, comma-separated group of option specifiers with nothing left over.
fn valid_option_group(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return false;
    }
    let mut i = 0;
    loop {
        let Some(next) = parse_one_option(&chars, i) else {
            return false;
        };
        i = next;
        if i == chars.len() {
            return true;
        }
        // Options are joined by a comma and a single space.
        if chars.get(i) == Some(&',') && chars.get(i + 1) == Some(&' ') {
            i += 2;
        } else {
            return false;
        }
    }
}

/// Parse a single option specifier starting at `i`, returning the index just past it (and any
/// argument). Recognizes long options (`--word`, `--word=ARG`, `--word ARG`), short options
/// (`-a`, `-aARG`, `-a ARG`), and DOS-style options (`/S`, `/S ARG`). Returns `None` if no valid
/// specifier begins at `i`.
fn parse_one_option(chars: &[char], i: usize) -> Option<usize> {
    match chars.get(i) {
        Some('-') if chars.get(i + 1) == Some(&'-') => {
            // Long option: a name of letters, digits, and hyphens.
            let mut j = i + 2;
            let name_start = j;
            while chars
                .get(j)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '-')
            {
                j += 1;
            }
            if j == name_start {
                return None;
            }
            parse_optional_arg(chars, j)
        }
        Some('-') => {
            // Short option: a hyphen and exactly one alphanumeric character.
            let ch = chars.get(i + 1)?;
            if !ch.is_ascii_alphanumeric() {
                return None;
            }
            parse_optional_arg(chars, i + 2)
        }
        Some('/') => {
            // DOS/VMS-style option: a slash and exactly one alphanumeric character.
            let ch = chars.get(i + 1)?;
            if !ch.is_ascii_alphanumeric() {
                return None;
            }
            parse_optional_arg(chars, i + 2)
        }
        _ => None,
    }
}

/// Parse an optional argument that follows an option specifier at `i`: an `=`-delimited argument,
/// a single-space-delimited argument, or an argument attached directly to a short option. Returns
/// the index just past the option (and argument, if present).
fn parse_optional_arg(chars: &[char], i: usize) -> Option<usize> {
    let delim = chars.get(i);
    if delim == Some(&'=') || delim == Some(&' ') {
        let arg_start = i + 1;
        let mut j = arg_start;
        while chars
            .get(j)
            .is_some_and(|c| !c.is_whitespace() && *c != ',')
        {
            j += 1;
        }
        if j == arg_start {
            return None;
        }
        return Some(j);
    }
    // An argument attached directly to a short option (e.g. `-fARG`).
    let mut j = i;
    while chars
        .get(j)
        .is_some_and(|c| !c.is_whitespace() && *c != ',')
    {
        j += 1;
    }
    Some(j)
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

#[derive(Clone)]
enum Substitution {
    Replace(String),
    Image(String, Attr, Vec<Inline>),
}

/// A custom interpreted-text role declared by a `role` directive: an optional base role whose
/// formatting it inherits, plus the classes it adds.
#[derive(Clone, Default)]
struct RoleDef {
    base: Option<String>,
    classes: Vec<String>,
}

/// Read and parse an included file, returning its blocks for splicing into the document. Returns
/// `None` when the file cannot be read.
fn included_blocks(path: &str, ext: Extensions, depth: usize) -> Option<Vec<Block>> {
    let content = std::fs::read_to_string(path).ok()?;
    let lines = preprocess(&content);
    let defs = collect_definitions(&lines);
    let mut parser = Parser {
        defs: &defs,
        ext,
        heading_styles: Vec::new(),
        ids: IdRegistry::default(),
        auto_footnote: 0,
        symbol_footnote: 0,
        anonymous: 0,
        custom_roles: BTreeMap::new(),
        default_role: DEFAULT_ROLE.to_string(),
        include_depth: depth,
    };
    let mut blocks = parser.blocks(&lines);
    if let Some(div) = parser.citation_block() {
        blocks.push(div);
    }
    Some(blocks)
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
                // A target with no destination is internal: a reference to it points at the
                // identifier the target will carry onto its block.
                let url = if url.trim().is_empty() {
                    format!("#{}", name.trim())
                } else {
                    url
                };
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
            let (mut attr, mut alt, url) = image_parts(&argument, &options);
            attr.classes = image_classes(&options);
            // A substitution image with no explicit alt text falls back to the substitution name.
            if alt.is_empty() {
                push_text(&mut alt, &name);
            }
            Some((name, Substitution::Image(url, attr, alt)))
        }
        "unicode" => Some((name, Substitution::Replace(unicode_chars(&argument)))),
        "date" => Some((name, Substitution::Replace(format_date(argument.trim())))),
        _ => Some((name, Substitution::Replace(String::new()))),
    }
}

/// Decode the tokens of a `unicode::` substitution argument. A token written as a hexadecimal code
/// point (`0x`, `x`, `u`, `\x`, `\u`, `U+`, or an `&#x…;` character reference) becomes its
/// character; any other token, including a bare decimal number, stays as written. Tokens are joined
/// with a single space, and a standalone `..` ends the text.
fn unicode_chars(argument: &str) -> String {
    let mut tokens = Vec::new();
    for token in argument.split_whitespace() {
        if token == ".." {
            break;
        }
        tokens.push(decode_unicode_token(token));
    }
    tokens.join(" ")
}

fn decode_unicode_token(token: &str) -> String {
    if let Some(rest) = token.strip_prefix("&#x")
        && let Some(hex) = rest.strip_suffix(';')
        && let Some(ch) = code_point(hex)
    {
        return ch.to_string();
    }
    let hex = token
        .strip_prefix("U+")
        .or_else(|| token.strip_prefix("0x"))
        .or_else(|| token.strip_prefix("\\u"))
        .or_else(|| token.strip_prefix("\\x"))
        .or_else(|| token.strip_prefix('x'))
        .or_else(|| token.strip_prefix('u'));
    if let Some(hex) = hex
        && let Some(ch) = code_point(hex)
    {
        return ch.to_string();
    }
    token.to_string()
}

/// Parse a non-empty run of hexadecimal digits into its character, or `None` for empty or
/// out-of-range input.
fn code_point(hex: &str) -> Option<char> {
    if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

/// Render the current date with a strftime-style format string, defaulting to `%Y-%m-%d`. The date
/// is taken in UTC.
fn format_date(format: &str) -> String {
    let format = if format.is_empty() {
        "%Y-%m-%d"
    } else {
        format
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    render_date(secs, format)
}

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Expand a strftime-style format against the civil date and time of day at `secs` seconds past the
/// epoch (UTC). Unrecognized `%`-codes are emitted verbatim; `%%` yields a single percent.
fn render_date(secs: i64, format: &str) -> String {
    let parts = DateParts::from_secs(secs);
    let mut out = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(spec) => {
                if let Some(value) = parts.field(spec) {
                    out.push_str(&value);
                } else {
                    out.push('%');
                    if spec != '%' {
                        out.push(spec);
                    }
                }
            }
            None => out.push('%'),
        }
    }
    out
}

fn pad2(n: i64) -> String {
    format!("{n:02}")
}

fn pad3(n: i64) -> String {
    format!("{n:03}")
}

fn space2(n: i64) -> String {
    format!("{n:2}")
}

/// `53` for ISO long years (those whose 1 January is a Thursday, or whose previous year's 1 January
/// is a Wednesday), `52` otherwise.
fn iso_weeks_in_year(year: i64) -> i64 {
    let dominical =
        |y: i64| (y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400)).rem_euclid(7);
    if dominical(year) == 4 || dominical(year - 1) == 3 {
        53
    } else {
        52
    }
}

/// The decomposed civil date and time of day for a moment, in UTC.
struct DateParts {
    year: i64,
    /// 1-12.
    month: i64,
    /// 1-31.
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    /// 0 = Sunday … 6 = Saturday.
    weekday: i64,
    /// Day of the year, 1-366.
    yday: i64,
}

impl DateParts {
    fn from_secs(secs: i64) -> Self {
        let days = secs.div_euclid(86_400);
        let day_secs = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            hour: day_secs / 3600,
            minute: day_secs / 60 % 60,
            second: day_secs % 60,
            // 1970-01-01 was a Thursday (index 4).
            weekday: (days.rem_euclid(7) + 4).rem_euclid(7),
            yday: days - days_from_civil(year, 1, 1) + 1,
        }
    }

    /// ISO 8601 weekday: 1 = Monday … 7 = Sunday.
    fn iso_weekday(&self) -> i64 {
        if self.weekday == 0 { 7 } else { self.weekday }
    }

    /// Hour on a 12-hour clock, 1-12.
    fn hour12(&self) -> i64 {
        let h = self.hour % 12;
        if h == 0 { 12 } else { h }
    }

    fn meridiem(&self, upper: bool) -> &'static str {
        match (self.hour < 12, upper) {
            (true, true) => "AM",
            (true, false) => "am",
            (false, true) => "PM",
            (false, false) => "pm",
        }
    }

    /// Week of the year counting from the first Sunday (`%U`), 00-53.
    fn week_from_sunday(&self) -> i64 {
        (self.yday - 1 + 7 - self.weekday) / 7
    }

    /// Week of the year counting from the first Monday (`%W`), 00-53.
    fn week_from_monday(&self) -> i64 {
        (self.yday - 1 + 7 - (self.weekday + 6) % 7) / 7
    }

    /// ISO 8601 (week-numbering-year, week-of-year), the latter 01-53.
    fn iso_week(&self) -> (i64, i64) {
        let week = (self.yday + 10 - self.iso_weekday()) / 7;
        if week < 1 {
            (self.year - 1, iso_weeks_in_year(self.year - 1))
        } else if week > iso_weeks_in_year(self.year) {
            (self.year + 1, 1)
        } else {
            (self.year, week)
        }
    }

    /// The rendering of one strftime field, or `None` for an unrecognized code.
    fn field(&self, spec: char) -> Option<String> {
        let month_name = MONTH_NAMES
            .get(usize::try_from(self.month - 1).unwrap_or(0))
            .copied()
            .unwrap_or("");
        let weekday_name = WEEKDAY_NAMES
            .get(usize::try_from(self.weekday).unwrap_or(0))
            .copied()
            .unwrap_or("");
        Some(match spec {
            'Y' => self.year.to_string(),
            'y' => pad2(self.year.rem_euclid(100)),
            'C' => pad2(self.year.div_euclid(100)),
            'm' => pad2(self.month),
            'd' => pad2(self.day),
            'e' => space2(self.day),
            'H' => pad2(self.hour),
            'k' => space2(self.hour),
            'I' => pad2(self.hour12()),
            'l' => space2(self.hour12()),
            'M' => pad2(self.minute),
            'S' => pad2(self.second),
            'j' => pad3(self.yday),
            'p' => self.meridiem(true).to_string(),
            'P' => self.meridiem(false).to_string(),
            'u' => self.iso_weekday().to_string(),
            'w' => self.weekday.to_string(),
            'U' => pad2(self.week_from_sunday()),
            'W' => pad2(self.week_from_monday()),
            'V' => pad2(self.iso_week().1),
            'G' => self.iso_week().0.to_string(),
            'g' => pad2(self.iso_week().0.rem_euclid(100)),
            'B' => month_name.to_string(),
            'b' | 'h' => month_name.get(..3).unwrap_or(month_name).to_string(),
            'A' => weekday_name.to_string(),
            'a' => weekday_name.get(..3).unwrap_or(weekday_name).to_string(),
            'D' => format!(
                "{:02}/{:02}/{:02}",
                self.month,
                self.day,
                self.year.rem_euclid(100)
            ),
            'F' => format!("{}-{:02}-{:02}", self.year, self.month, self.day),
            'R' => format!("{:02}:{:02}", self.hour, self.minute),
            'T' => format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second),
            'r' => format!(
                "{:02}:{:02}:{:02} {}",
                self.hour12(),
                self.minute,
                self.second,
                self.meridiem(true)
            ),
            'n' => "\n".to_string(),
            't' => "\t".to_string(),
            _ => return None,
        })
    }
}

/// The civil (year, month, day) of a day count measured from the epoch, by the standard
/// days-to-civil conversion. `month` is 1-12 and `day` is 1-31.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The day count from the epoch of a civil date, the inverse of `civil_from_days`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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
}

/// The deepest chain of nested `include` directives that is followed before further includes are
/// ignored, guarding against a cycle of files including one another.
const MAX_INCLUDE_DEPTH: usize = 64;

impl Parser<'_> {
    fn blocks(&mut self, lines: &[String]) -> Vec<Block> {
        let mut out = Vec::new();
        let mut pending_classes: Option<Vec<String>> = None;
        let mut pending_targets: Vec<String> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let line = line_at(lines, i);
            if is_blank(line) {
                i += 1;
                continue;
            }
            // An internal hyperlink target (a `.. _name:` with no destination) carries its
            // identifier onto the block that follows it.
            if matches!(classify_explicit(line), Some(Explicit::Target)) {
                let indent = indent_of(line);
                let end = explicit_extent(lines, i, indent);
                if let Some((name, url)) = parse_target(line.trim_start(), lines, i, end, indent)
                    && url.trim().is_empty()
                {
                    pending_targets.push(name.trim().to_string());
                    i = end;
                    continue;
                }
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
            // Internal targets seen since the last block attach their identifiers to it.
            if !pending_targets.is_empty() && out.len() > before {
                let produced = out.split_off(before);
                out.extend(attach_targets(
                    produced,
                    std::mem::take(&mut pending_targets),
                ));
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

        if let Some((_, style, delim, col)) = enumerator(line)
            && item_well_formed(lines, i, col, style, delim)
        {
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

        if option_marker(line).is_some() {
            return self.option_list(lines, i, out);
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
        let id = match IdScheme::select(self.ext) {
            Some(scheme) => {
                let plain = carta_ast::to_plain_text(&inlines);
                let text = if self.ext.contains(Extension::AsciiIdentifiers) {
                    asciify(&plain)
                } else {
                    plain
                };
                self.ids.assign(scheme, &text)
            }
            None => String::new(),
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
                out.push(Block::Para(splice_lone_span(self.inlines(&trimmed))));
            }
            out.push(code);
            return next;
        }
        out.push(Block::Para(splice_lone_span(self.inlines(&text))));
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
            // An unindented block whose every line opens with the same quoting character is a
            // quoted literal block; the quoting characters are kept verbatim.
            return Self::quoted_literal_block(lines, i);
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

    /// A quoted literal block: an unindented run of lines that each begin with the same quoting
    /// character (one of the adornment characters). The lines, quoting characters included, are the
    /// code block's verbatim text.
    fn quoted_literal_block(lines: &[String], start: usize) -> Option<(Block, usize)> {
        let quote = line_at(lines, start).chars().next()?;
        if !ADORNMENT_CHARS.contains(quote) {
            return None;
        }
        let mut i = start;
        let mut text_lines: Vec<String> = Vec::new();
        while let Some(line) = lines.get(i) {
            if is_blank(line) || line.chars().next() != Some(quote) {
                break;
            }
            text_lines.push(line.clone());
            i += 1;
        }
        if text_lines.is_empty() {
            return None;
        }
        Some((Block::CodeBlock(Attr::default(), text_lines.join("\n")), i))
    }

    fn line_block(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
        let base = indent_of(line_at(lines, start));
        let mut entries: Vec<String> = Vec::new();
        let mut i = start;
        while let Some(line) = lines.get(i) {
            if is_blank(line) {
                break;
            }
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix('|') {
                if !matches!(rest.chars().next(), Some(' ') | None) {
                    break;
                }
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                // Indentation beyond the single separating space is preserved as non-breaking
                // spaces so it survives into the rendered line.
                let leading = rest.chars().take_while(|c| *c == ' ').count();
                let content = format!(
                    "{}{}",
                    "\u{a0}".repeat(leading),
                    rest.trim_start_matches(' ')
                );
                entries.push(content);
                i += 1;
            } else if !entries.is_empty() && indent_of(line) > base {
                // A further-indented line without its own `|` continues the preceding line,
                // joined to it by a single space.
                if let Some(last) = entries.last_mut() {
                    last.push(' ');
                    last.push_str(trimmed);
                }
                i += 1;
            } else {
                break;
            }
        }
        let parsed = entries.iter().map(|entry| self.inlines(entry)).collect();
        out.push(Block::LineBlock(parsed));
        i
    }

    fn bullet_list(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
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
            let Some((_, _, _, col)) = enumerator(line) else {
                break;
            };
            // An auto-numbered (`#`) item joins whatever list is open and vice versa; otherwise the
            // delimiter must match and the style must match directly or by an ambiguous single
            // letter adopting the list's established style. A later item that is itself a run-on
            // paragraph (its continuation under-indented) ends the list before it.
            if !enum_compatible(line, style, delim)
                || !item_well_formed(lines, i, col, style, delim)
            {
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

    /// An option list: each item pairs an option group (`-a`, `--all=ARG`, `/S`, comma-joined
    /// variants) rendered as inline code with a description body. The body begins after the
    /// two-or-more-space gap that follows the option group, or on the following indented lines.
    fn option_list(&mut self, lines: &[String], start: usize, out: &mut Vec<Block>) -> usize {
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
            let Some((term, value_col)) = option_marker(line) else {
                break;
            };
            let end = explicit_extent(lines, i, 0);
            let body = explicit_body(lines, i, end, value_col);
            let term_inline = vec![Inline::Code(Attr::default(), term)];
            items.push((term_inline, vec![self.blocks(&body)]));
            i = end;
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
                let mut equations = Vec::new();
                if !argument.trim().is_empty() {
                    equations.push(argument.trim().to_string());
                }
                equations.extend(blank_separated(&content));
                let math: Vec<Inline> = equations
                    .into_iter()
                    .map(|eq| Inline::Math(MathType::DisplayMath, eq))
                    .collect();
                let (id, classes, attributes) = common_options(&options);
                // Options (a `:label:`, `:nowrap:`, …) attach to the whole equation group through a
                // wrapping span; without them the equations stand on their own.
                let inlines = if id.is_empty() && classes.is_empty() && attributes.is_empty() {
                    math
                } else {
                    vec![Inline::Span(
                        Attr {
                            id,
                            classes,
                            attributes,
                        },
                        math,
                    )]
                };
                out.push(Block::Para(inlines));
            }
            "image" => {
                let (mut attr, mut alt, url) = image_parts(&argument, &options);
                attr.classes = image_classes(&options);
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
                out.push(options_div(name, &options, blocks));
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
            "csv-table" => self.csv_table(&argument, &options, &content, out),
            "list-table" => self.list_table(&argument, &options, &content, out),
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
            "line-block" => out.push(self.line_block_directive(&content)),
            "table" => self.table_directive(&argument, &options, &content, out),
            // A role definition configures inline interpretation; it produces no block of its own.
            "role" => self.register_role(&argument, &options),
            "default-role" => {
                let selected = argument.trim();
                self.default_role = if selected.is_empty() {
                    DEFAULT_ROLE.to_string()
                } else {
                    selected.to_string()
                };
            }
            // An include directive splices the parsed content of an external file in place. A file
            // that cannot be read contributes nothing.
            "include" => {
                if self.include_depth < MAX_INCLUDE_DEPTH
                    && let Some(blocks) =
                        included_blocks(argument.trim(), self.ext, self.include_depth + 1)
                {
                    out.extend(blocks);
                }
            }
            _ => {
                let mut blocks = Vec::new();
                if !argument.trim().is_empty() {
                    blocks.push(Block::Para(self.inlines(argument.trim())));
                }
                blocks.extend(self.blocks(&content));
                out.push(options_div(name, &options, blocks));
            }
        }
    }

    /// Record a `role` directive: an `name(base)` argument names the role and the base role it
    /// inherits, while a `:class:` option supplies the classes a no-base role applies.
    fn register_role(&mut self, argument: &str, options: &[(String, String)]) {
        let argument = argument.trim();
        let (name, base) = match argument.split_once('(') {
            Some((name, rest)) => (
                name.trim(),
                Some(rest.trim_end_matches(')').trim().to_string()),
            ),
            None => (argument, None),
        };
        if name.is_empty() {
            return;
        }
        let base = base.filter(|b| !b.is_empty());
        let classes = class_list(options, "class");
        self.custom_roles
            .insert(name.to_string(), RoleDef { base, classes });
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
            // The first body block is the caption proper; any further blocks are the legend, which
            // joins the caption rather than the figure body.
            caption.long = vec![plain];
            caption.long.extend(iter);
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
        let body = vec![Block::Plain(vec![image])];
        Block::Figure(figure_attr(options), caption, body)
    }

    /// A `line-block` directive: each body line becomes one line of the block, with a blank body line
    /// rendering as an empty line.
    fn line_block_directive(&mut self, content: &[String]) -> Block {
        let mut end = content.len();
        while end > 0 && content.get(end - 1).is_some_and(|l| l.trim().is_empty()) {
            end -= 1;
        }
        let lines = content
            .get(..end)
            .unwrap_or(&[])
            .iter()
            .map(|line| self.inlines(line.trim()))
            .collect();
        Block::LineBlock(lines)
    }

    /// A `table` directive: its body is an ordinary table whose caption is taken from the directive's
    /// argument.
    fn table_directive(
        &mut self,
        argument: &str,
        _options: &[(String, String)],
        content: &[String],
        out: &mut Vec<Block>,
    ) {
        let mut blocks = self.blocks(content);
        let argument = argument.trim();
        if !argument.is_empty() {
            let caption = self.inlines(argument);
            if let Some(Block::Table(table)) =
                blocks.iter_mut().find(|b| matches!(b, Block::Table(_)))
            {
                table.caption = Caption {
                    short: None,
                    long: vec![Block::Plain(caption)],
                };
            }
        }
        out.extend(blocks);
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

    // --- table directives ---

    /// A `csv-table` directive: its rows are comma-separated values, with an optional explicit
    /// `:header:` row and/or a count of leading `:header-rows:` taken from the data.
    fn csv_table(
        &mut self,
        argument: &str,
        options: &[(String, String)],
        content: &[String],
        out: &mut Vec<Block>,
    ) {
        let widths = directive_widths(options);
        let mut records = parse_csv(&content.join("\n"));
        let mut header_records: Vec<Vec<String>> = Vec::new();
        if let Some((_, header)) = options.iter().find(|(k, _)| k == "header") {
            header_records.extend(parse_csv(header));
        }
        let take = directive_count(options, "header-rows").min(records.len());
        header_records.extend(records.drain(..take));
        let num_cols = header_records
            .iter()
            .chain(records.iter())
            .map(Vec::len)
            .max()
            .unwrap_or(0);
        if num_cols == 0 {
            return;
        }
        let head_rows = header_records
            .iter()
            .map(|r| self.csv_row(r, num_cols))
            .collect();
        let body_rows = records.iter().map(|r| self.csv_row(r, num_cols)).collect();
        out.push(self.make_table(argument, widths, head_rows, body_rows, num_cols));
    }

    fn csv_row(&mut self, fields: &[String], num_cols: usize) -> Vec<Cell> {
        (0..num_cols)
            .map(|i| {
                let content = match fields.get(i) {
                    Some(f) if !f.is_empty() => vec![Block::Plain(self.inlines(f))],
                    _ => Vec::new(),
                };
                Cell {
                    attr: Attr::default(),
                    align: Alignment::AlignDefault,
                    row_span: 1,
                    col_span: 1,
                    content,
                }
            })
            .collect()
    }

    /// A `list-table` directive: a two-level bullet list where each outer item is a row and its
    /// nested bullet list supplies the row's cells.
    fn list_table(
        &mut self,
        argument: &str,
        options: &[(String, String)],
        content: &[String],
        out: &mut Vec<Block>,
    ) {
        let widths = directive_widths(options);
        let mut rows: Vec<Vec<Vec<Block>>> = Vec::new();
        for block in self.blocks(content) {
            if let Block::BulletList(items) = block {
                for item in items {
                    let mut cells = Vec::new();
                    for inner in item {
                        if let Block::BulletList(cell_items) = inner {
                            cells.extend(cell_items);
                        }
                    }
                    rows.push(cells);
                }
            }
        }
        let num_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if num_cols == 0 {
            return;
        }
        let take = directive_count(options, "header-rows").min(rows.len());
        let head_src: Vec<Vec<Vec<Block>>> = rows.drain(..take).collect();
        let head_rows = head_src
            .into_iter()
            .map(|r| list_row(r, num_cols))
            .collect();
        let body_rows = rows.into_iter().map(|r| list_row(r, num_cols)).collect();
        out.push(self.make_table(argument, widths, head_rows, body_rows, num_cols));
    }

    /// Assemble a table from already-built header and body cell rows, a caption drawn from the
    /// directive argument, and either explicit column widths or the default.
    fn make_table(
        &mut self,
        caption: &str,
        widths: Option<Vec<f64>>,
        head_rows: Vec<Vec<Cell>>,
        body_rows: Vec<Vec<Cell>>,
        num_cols: usize,
    ) -> Block {
        let caption = if caption.trim().is_empty() {
            Caption::default()
        } else {
            Caption {
                short: None,
                long: vec![Block::Plain(self.inlines(caption.trim()))],
            }
        };
        let col_specs = (0..num_cols)
            .map(|i| ColSpec {
                align: Alignment::AlignDefault,
                width: match &widths {
                    Some(w) if w.len() == num_cols => w
                        .get(i)
                        .copied()
                        .map_or(ColWidth::ColWidthDefault, ColWidth::ColWidth),
                    _ => ColWidth::ColWidthDefault,
                },
            })
            .collect();
        Block::Table(Box::new(Table {
            attr: Attr::default(),
            caption,
            col_specs,
            head: TableHead {
                attr: Attr::default(),
                rows: cells_to_rows(head_rows),
            },
            bodies: vec![TableBody {
                attr: Attr::default(),
                row_head_columns: 0,
                head: Vec::new(),
                body: cells_to_rows(body_rows),
            }],
            foot: TableFoot::default(),
        }))
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
        // The table runs over consecutive lines that belong to the grid (a border or a `|`-led row).
        let mut end = start;
        while lines.get(end).is_some_and(|l| is_grid_line(l)) {
            end += 1;
        }
        if end - start < 3 {
            return None;
        }
        // A padded character matrix so every position can be addressed by (row, column).
        let width = (start..end)
            .filter_map(|i| lines.get(i))
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        let block: Vec<Vec<char>> = (start..end)
            .filter_map(|i| lines.get(i))
            .map(|l| {
                let mut row: Vec<char> = l.chars().collect();
                row.resize(width, ' ');
                row
            })
            .collect();

        let cells = scan_grid_cells(&block)?;
        if cells.is_empty() {
            return None;
        }

        // The vertical and horizontal grid lines, as the distinct cell-edge positions.
        let mut col_edges: Vec<usize> = cells.iter().flat_map(|c| [c.left, c.right]).collect();
        col_edges.sort_unstable();
        col_edges.dedup();
        let mut row_edges: Vec<usize> = cells.iter().flat_map(|c| [c.top, c.bottom]).collect();
        row_edges.sort_unstable();
        row_edges.dedup();
        let col_index = |pos: usize| col_edges.iter().position(|e| *e == pos);
        let row_index = |pos: usize| row_edges.iter().position(|e| *e == pos);
        let num_cols = col_edges.len().checked_sub(1)?;
        let num_rows = row_edges.len().checked_sub(1)?;
        if num_cols == 0 || num_rows == 0 {
            return None;
        }

        // Place each cell into a row/column grid, validating that the cells tile it exactly.
        let mut grid: Vec<Vec<Option<GridCell>>> = vec![vec![None; num_cols]; num_rows];
        let mut covered = vec![vec![false; num_cols]; num_rows];
        for cell in &cells {
            let r0 = row_index(cell.top)?;
            let r1 = row_index(cell.bottom)?;
            let c0 = col_index(cell.left)?;
            let c1 = col_index(cell.right)?;
            let text: String = (cell.top + 1..cell.bottom)
                .filter_map(|r| block.get(r))
                .map(|row| {
                    let seg: String = row
                        .get(cell.left + 1..cell.right)
                        .map_or_else(String::new, |s| s.iter().collect());
                    seg.trim_end().to_string()
                })
                .collect::<Vec<_>>()
                .join("\n");
            for r in r0..r1 {
                for c in c0..c1 {
                    if covered.get(r).and_then(|row| row.get(c)).copied() != Some(false) {
                        return None;
                    }
                    if let Some(slot) = covered.get_mut(r).and_then(|row| row.get_mut(c)) {
                        *slot = true;
                    }
                }
            }
            if let Some(slot) = grid.get_mut(r0).and_then(|row| row.get_mut(c0)) {
                *slot = Some(GridCell {
                    text,
                    row_span: r1 - r0,
                    col_span: c1 - c0,
                });
            }
        }
        if covered.iter().any(|row| row.iter().any(|c| !c)) {
            return None;
        }

        // A `=` separator line marks the boundary between header rows and body rows.
        let header_rows = row_edges
            .iter()
            .position(|edge| block.get(*edge).is_some_and(|row| row.contains(&'=')))
            .unwrap_or(0);

        let last = *col_edges.last()?;
        let first = *col_edges.first()?;
        let total = last.saturating_sub(first).saturating_sub(num_cols);
        let divisor = total.max(72) as f64;
        let col_specs: Vec<ColSpec> = (0..num_cols)
            .map(|i| {
                let lo = col_edges.get(i).copied().unwrap_or(0);
                let hi = col_edges.get(i + 1).copied().unwrap_or(lo);
                ColSpec {
                    align: Alignment::AlignDefault,
                    width: ColWidth::ColWidth(hi.saturating_sub(lo) as f64 / divisor),
                }
            })
            .collect();

        let mut head_rows = Vec::new();
        let mut body_rows = Vec::new();
        for (r, row) in grid.iter().enumerate() {
            let built = self.grid_row(row);
            if r < header_rows {
                head_rows.push(built);
            } else {
                body_rows.push(built);
            }
        }

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
        Some(end)
    }

    /// Build one table row, emitting only the cells that originate in this row band; positions
    /// covered by a row- or column-spanning cell that began earlier carry no cell of their own.
    fn grid_row(&mut self, row: &[Option<GridCell>]) -> Row {
        let cells = row
            .iter()
            .filter_map(|slot| slot.as_ref())
            .map(|cell| {
                let row_span = i32::try_from(cell.row_span).unwrap_or(1);
                let col_span = i32::try_from(cell.col_span).unwrap_or(1);
                self.text_cell(&cell.text, row_span, col_span)
            })
            .collect();
        Row {
            attr: Attr::default(),
            cells,
        }
    }

    /// Build a cell from its newline-joined text. The shared blank-edges/min-indent normalization is
    /// applied, the text is parsed as block content, and a lone paragraph is demoted to a plain block.
    fn text_cell(&mut self, text: &str, row_span: i32, col_span: i32) -> Cell {
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
            row_span,
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
                self.text_cell(&text, 1, i32::try_from(b - a + 1).unwrap_or(1))
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
        let smart = self.ext.contains(Extension::Smart);
        let mut out = Vec::new();
        let mut pending = String::new();
        let mut pos = 0;
        while pos < chars.len() {
            let ch = chars.get(pos).copied().unwrap_or(' ');
            let prev = pos.checked_sub(1).and_then(|p| chars.get(p)).copied();
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
            // An inline internal hyperlink target `_`name`` becomes a span carrying a slug
            // identifier so the location can be linked to.
            if ch == '_'
                && chars.get(pos + 1) == Some(&'`')
                && inline_start_ok(prev)
                && let Some((span, next)) = self.inline_target(&chars, pos)
            {
                push_text(&mut out, &pending);
                pending.clear();
                out.push(span);
                pos = next;
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
            // A bare URI or email address that begins at a word boundary is auto-linked.
            if autolink_boundary(prev)
                && let Some((link, next)) = autolink(&chars, pos)
            {
                push_text(&mut out, &pending);
                pending.clear();
                out.push(link);
                pos = next;
                continue;
            }
            // Typographic punctuation under the `smart` extension: paired quotes become quotation
            // nodes, a lone quote its apt curly glyph, hyphen runs en/em dashes, dot runs ellipses.
            if smart {
                match ch {
                    '"' | '\'' => {
                        if let Some((quoted, next)) = self.smart_quote(&chars, pos, ch) {
                            push_text(&mut out, &pending);
                            pending.clear();
                            out.push(quoted);
                            pos = next;
                            continue;
                        }
                        pending.push(quote_glyph(&chars, pos, ch));
                        pos += 1;
                        continue;
                    }
                    '-' => {
                        let n = run_length(&chars, pos, '-');
                        pending.push_str(&fold_dashes(n));
                        pos += n;
                        continue;
                    }
                    '.' => {
                        let n = run_length(&chars, pos, '.');
                        pending.push_str(&fold_ellipsis(n));
                        pos += n;
                        continue;
                    }
                    _ => {}
                }
            }
            pending.push(ch);
            pos += 1;
        }
        push_text(&mut out, &pending);
        trim_inline_ends(&mut out);
        out
    }

    /// An inline internal hyperlink target `_`name``: a span whose identifier is the slug of its
    /// text, marking a location elsewhere markup can link to.
    fn inline_target(&mut self, chars: &[char], pos: usize) -> Option<(Inline, usize)> {
        let (name, end) = find_close_literal(chars, pos + 2, "`")?;
        if name.trim().is_empty() {
            return None;
        }
        let inner = self.inlines(&name);
        let id = carta_ast::slug(&carta_ast::to_plain_text(&inner));
        Some((
            Inline::Span(
                Attr {
                    id,
                    classes: Vec::new(),
                    attributes: Vec::new(),
                },
                inner,
            ),
            end,
        ))
    }

    /// A quoted run opened by a straight quote: scan for a matching closer and, on success, parse the
    /// interior recursively into a quotation node. Returns `None` when the quote cannot open a run or
    /// has no closer, leaving the caller to fold it into a lone glyph.
    fn smart_quote(&mut self, chars: &[char], pos: usize, quote: char) -> Option<(Inline, usize)> {
        if !can_open_quote(chars, pos) {
            return None;
        }
        // A single quote against a preceding letter or digit is a word-internal apostrophe, never the
        // opener of a quoted run.
        if quote == '\'' {
            let before = pos.checked_sub(1).and_then(|p| chars.get(p)).copied();
            if before.is_some_and(char::is_alphanumeric) {
                return None;
            }
        }
        let mut j = pos + 1;
        while j < chars.len() {
            match chars.get(j).copied() {
                Some('\\') => j += 2,
                Some(c) if c == quote && can_close_quote(chars, j, quote) => {
                    let content: String = chars.get(pos + 1..j)?.iter().collect();
                    let inner = self.inlines(&content);
                    return Some((Inline::Quoted(quote_type(quote), inner), j + 1));
                }
                Some(_) => j += 1,
                None => break,
            }
        }
        None
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
        if chars.get(pos + 1) == Some(&'*') {
            if chars.get(pos + 2).is_none_or(|c| c.is_whitespace()) {
                return None;
            }
            let (inner, end) = Self::scan_strong(chars, pos)?;
            if quote_suppresses(prev, chars.get(end).copied()) {
                return None;
            }
            return Some((vec![Inline::Strong(inner)], false, end));
        }
        if chars.get(pos + 1).is_none_or(|c| c.is_whitespace()) {
            return None;
        }
        let (inner, end) = Self::scan_emphasis(chars, pos)?;
        if quote_suppresses(prev, chars.get(end).copied()) {
            return None;
        }
        Some((vec![Inline::Emph(inner)], false, end))
    }

    /// Scan a strong span opened by `**` at `pos`. Its content is verbatim text in which a single `*`
    /// is an ordinary character; the first later run of two or more `*` closes the span. Returns the
    /// parsed content and the index past the closing delimiter, or `None` when no closer is found.
    fn scan_strong(chars: &[char], pos: usize) -> Option<(Vec<Inline>, usize)> {
        let mut pending = String::new();
        let mut i = pos + 2;
        while i < chars.len() {
            match chars.get(i).copied() {
                Some('\\') => {
                    pending.push('\\');
                    if let Some(&next) = chars.get(i + 1) {
                        pending.push(next);
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                Some('*') if run_length(chars, i, '*') >= 2 => {
                    return Some((literal_text(&pending), i + 2));
                }
                Some(c) => {
                    pending.push(c);
                    i += 1;
                }
                None => break,
            }
        }
        None
    }

    /// Scan an emphasis span opened by a single `*` at `pos`. A later single `*`, or a `**` run that
    /// is followed by whitespace, closes the span (consuming one `*`); a `**` run followed by content
    /// is an inner strong start-string that is stripped, flushing the text gathered so far as its own
    /// segment. Returns the content segments and the index past the closing `*`, or `None` with no
    /// closer.
    fn scan_emphasis(chars: &[char], pos: usize) -> Option<(Vec<Inline>, usize)> {
        let mut result = Vec::new();
        let mut pending = String::new();
        let mut i = pos + 1;
        while i < chars.len() {
            match chars.get(i).copied() {
                Some('\\') => {
                    pending.push('\\');
                    if let Some(&next) = chars.get(i + 1) {
                        pending.push(next);
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                Some('*') => {
                    let run = run_length(chars, i, '*');
                    let after = chars.get(i + run).copied();
                    if run >= 2 && after.is_some_and(|c| !c.is_whitespace()) {
                        result.extend(literal_text(&pending));
                        pending.clear();
                        i += run;
                    } else {
                        result.extend(literal_text(&pending));
                        return Some((result, i + 1));
                    }
                }
                Some(c) => {
                    pending.push(c);
                    i += 1;
                }
                None => break,
            }
        }
        None
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
            if quote_suppresses(prev, chars.get(end).copied()) {
                return None;
            }
            return Some((vec![self.phrase_reference(&content, anonymous)], false, end));
        }
        // A trailing role applies to the interpreted text.
        if chars.get(end) == Some(&':')
            && let Some((role, role_end)) = parse_role(chars, end)
        {
            if quote_suppresses(prev, chars.get(role_end).copied()) {
                return None;
            }
            let inline = self.apply_role(&role, &content);
            return Some((vec![inline], false, role_end));
        }
        if quote_suppresses(prev, chars.get(end).copied()) {
            return None;
        }
        let role = self.default_role.clone();
        Some((vec![self.apply_role(&role, &content)], false, end))
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
            // A role declared by a `role` directive: a base role supplies the formatting, otherwise
            // the content becomes a span carrying the role's classes (its own name when none are
            // given).
            other if self.custom_roles.contains_key(other) => {
                let def = self.custom_roles.get(other).cloned().unwrap_or_default();
                if let Some(base) = def.base {
                    self.apply_role(&base, content)
                } else {
                    let classes = if def.classes.is_empty() {
                        vec![other.to_string()]
                    } else {
                        def.classes
                    };
                    Inline::Span(
                        Attr {
                            id: String::new(),
                            classes,
                            attributes: Vec::new(),
                        },
                        self.inlines(content),
                    )
                }
            }
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
        _prev: Option<char>,
    ) -> Option<(Vec<Inline>, bool, usize)> {
        if chars.get(pos + 1).is_some_and(|c| c.is_whitespace()) {
            return None;
        }
        let (name, mut end) = find_close_literal(chars, pos + 1, "|")?;
        // A trailing underscore turns the substitution into a hyperlink reference: the expansion
        // becomes the link text and the like-named target supplies the destination.
        let referenced = chars.get(end) == Some(&'_');
        if referenced {
            end += 1;
        }
        let expansion = match self.defs.substitutions.get(&normalize_name(&name)).cloned() {
            Some(Substitution::Replace(text)) => {
                let inlines = self.inlines(&text);
                // A replacement that expands to several inlines is kept together as one unit.
                match inlines.len() {
                    1 => inlines,
                    _ => vec![Inline::Span(Attr::default(), inlines)],
                }
            }
            Some(Substitution::Image(url, attr, alt)) => vec![Inline::Image(
                attr,
                alt,
                Target {
                    url,
                    title: String::new(),
                },
            )],
            None => {
                // An undefined substitution is preserved as a placeholder link whose visible text is
                // the reference as written and whose destination flags it as unresolved.
                let mut display = Vec::new();
                push_text(&mut display, &format!("|{name}|"));
                return Some((
                    vec![Inline::Link(
                        Attr::default(),
                        display,
                        Target {
                            url: format!("##SUBST##|{name}|"),
                            title: String::new(),
                        },
                    )],
                    false,
                    end,
                ));
            }
        };
        let result = if referenced {
            vec![Inline::Link(
                Attr::default(),
                expansion,
                Target {
                    url: self.resolve_target(&name),
                    title: String::new(),
                },
            )]
        } else {
            expansion
        };
        Some((result, false, end))
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
        // A reference wrapped in matching quotes is suppressed: the quotes and underscore stay
        // literal text.
        let before_name = pending.chars().rev().nth(trailing.chars().count());
        if quote_suppresses(before_name, chars.get(after).copied()) {
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
        // An indirect target's destination is itself another target's name (`name_`); follow the
        // chain to its concrete destination, stopping on an unknown name or a reference cycle.
        let mut current = normalize_name(name);
        let mut seen = std::collections::BTreeSet::new();
        while seen.insert(current.clone()) {
            let Some(url) = self.defs.targets.get(&current) else {
                return String::new();
            };
            let referent = url
                .strip_suffix('_')
                .filter(|r| !r.ends_with('_'))
                .map(|r| normalize_name(r.trim().trim_matches('`')))
                .filter(|key| self.defs.targets.contains_key(key));
            match referent {
                Some(next) => current = next,
                None => return url.clone(),
            }
        }
        String::new()
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

/// The normalized column widths from a `:widths:` option, each as a fraction of their sum.
/// `None` when the option is absent, set to `auto`, or carries no positive numbers.
fn directive_widths(options: &[(String, String)]) -> Option<Vec<f64>> {
    let value = options.iter().find(|(k, _)| k == "widths")?.1.trim();
    if value.is_empty() || value == "auto" {
        return None;
    }
    let nums: Vec<f64> = value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f64>().ok())
        .collect();
    let sum: f64 = nums.iter().sum();
    if nums.is_empty() || sum <= 0.0 {
        return None;
    }
    Some(nums.iter().map(|n| n / sum).collect())
}

/// The non-negative integer value of a directive option, defaulting to zero when absent or unparsable.
fn directive_count(options: &[(String, String)], key: &str) -> usize {
    options
        .iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0)
}

/// Wrap each row's cells in a table [`Row`].
fn cells_to_rows(rows: Vec<Vec<Cell>>) -> Vec<Row> {
    rows.into_iter()
        .map(|cells| Row {
            attr: Attr::default(),
            cells,
        })
        .collect()
}

/// Build one `list-table` row, padding short rows with empty cells and demoting a lone paragraph
/// in a cell to a plain block.
fn list_row(cells: Vec<Vec<Block>>, num_cols: usize) -> Vec<Cell> {
    let mut row: Vec<Cell> = cells
        .into_iter()
        .map(|content| {
            let content = if let [Block::Para(_)] = content.as_slice() {
                content.into_iter().map(to_plain).collect()
            } else {
                content
            };
            Cell {
                attr: Attr::default(),
                align: Alignment::AlignDefault,
                row_span: 1,
                col_span: 1,
                content,
            }
        })
        .collect();
    while row.len() < num_cols {
        row.push(Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content: Vec::new(),
        });
    }
    row
}

/// Parse comma-separated values into records of trimmed fields. Fields may be double-quoted, with a
/// doubled quote denoting a literal quote; whitespace after a delimiter is ignored; and a quoted
/// field may span lines. Blank records are dropped.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = text.chars().collect();
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while matches!(chars.get(i), Some(' ' | '\t')) {
            i += 1;
        }
        let mut field = String::new();
        if chars.get(i) == Some(&'"') {
            i += 1;
            loop {
                match chars.get(i) {
                    Some('"') if chars.get(i + 1) == Some(&'"') => {
                        field.push('"');
                        i += 2;
                    }
                    Some('"') => {
                        i += 1;
                        break;
                    }
                    Some(c) => {
                        field.push(*c);
                        i += 1;
                    }
                    None => break,
                }
            }
            while !matches!(chars.get(i), Some(',' | '\n') | None) {
                i += 1;
            }
        } else {
            while !matches!(chars.get(i), Some(',' | '\n') | None) {
                if let Some(c) = chars.get(i) {
                    field.push(*c);
                }
                i += 1;
            }
        }
        record.push(field.trim().to_string());
        match chars.get(i) {
            Some(',') => i += 1,
            Some('\n') => {
                i += 1;
                records.push(std::mem::take(&mut record));
            }
            _ => i += 1,
        }
    }
    if !record.is_empty() {
        records.push(record);
    }
    records.retain(|r| !(r.len() == 1 && r.first().is_some_and(String::is_empty)));
    records
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
            // Line numbering is requested by a marker class; a non-empty value sets the first line.
            "number-lines" => {
                classes.push("numberLines".to_string());
                let start = value.trim();
                if !start.is_empty() {
                    attributes.push(("startFrom".to_string(), start.to_string()));
                }
            }
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
/// The returned classes are the plain `:class:` list; callers that render a standalone image fold
/// the alignment into them with [`image_classes`].
fn image_parts(argument: &str, options: &[(String, String)]) -> (Attr, Vec<Inline>, String) {
    let url = argument.split_whitespace().collect::<Vec<_>>().join("");
    let mut id = String::new();
    let mut description = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "alt" => description = vec![Inline::Str(value.clone())],
            "name" => id.clone_from(value),
            _ => {}
        }
    }
    (
        Attr {
            id,
            classes: class_list(options, "class"),
            attributes: image_dimensions(options),
        },
        description,
        url,
    )
}

/// The classes of a standalone image: the `:class:` list, repeated, with the alignment appended to
/// the last entry (or standing alone when there are no classes).
fn image_classes(options: &[(String, String)]) -> Vec<String> {
    let classes = class_list(options, "class");
    aligned_classes(classes.clone(), classes, &align_suffix(options))
}

/// Build the attributes of a figure from its options: its `:figclass:` and `:class:` lists with the
/// alignment folded in. The figure's `:name:` identifies its image, not the figure itself.
fn figure_attr(options: &[(String, String)]) -> Attr {
    Attr {
        id: String::new(),
        classes: aligned_classes(
            class_list(options, "figclass"),
            class_list(options, "class"),
            &align_suffix(options),
        ),
        attributes: Vec::new(),
    }
}

/// The values of every option named `key`, split on whitespace, in source order.
fn class_list(options: &[(String, String)], key: &str) -> Vec<String> {
    options
        .iter()
        .filter(|(k, _)| k == key)
        .flat_map(|(_, v)| v.split_whitespace().map(str::to_string))
        .collect()
}

/// The class an `:align:` option contributes (`align-<value>`), or empty when there is none.
fn align_suffix(options: &[(String, String)]) -> String {
    options
        .iter()
        .find(|(k, _)| k == "align")
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
        .map_or_else(String::new, |v| format!("align-{v}"))
}

/// Combine two class lists with an optional alignment class. With no alignment the lists are
/// concatenated; otherwise the alignment is appended to the last class of the second list, or stands
/// alone when that list is empty.
fn aligned_classes(first: Vec<String>, second: Vec<String>, align: &str) -> Vec<String> {
    let mut classes = first;
    if align.is_empty() {
        classes.extend(second);
    } else if second.is_empty() {
        classes.push(align.to_string());
    } else {
        let last = second.len() - 1;
        for (index, mut class) in second.into_iter().enumerate() {
            if index == last {
                class.push_str(align);
            }
            classes.push(class);
        }
    }
    classes
}

/// The `width`/`height` attributes of an image, each normalized and scaled by an `:scale:` option.
fn image_dimensions(options: &[(String, String)]) -> Vec<(String, String)> {
    let scale = options
        .iter()
        .find(|(k, _)| k == "scale")
        .and_then(|(_, v)| parse_scale(v));
    let mut attributes = Vec::new();
    for (key, value) in options {
        if key == "width" || key == "height" {
            attributes.push((key.clone(), normalize_dimension(value, scale)));
        }
    }
    attributes
}

/// A length with the unit categories the output distinguishes: integral pixels, a percentage, or a
/// value in some other unit.
enum Dimension {
    Pixel(f64),
    Percent(f64),
    Other(f64, String),
}

/// Parse a `:scale:` value into its factor and whether it was written as a percentage. A bare number
/// scales directly; a trailing `%` divides by a hundred.
fn parse_scale(value: &str) -> Option<(f64, bool)> {
    let value = value.trim();
    let percent = value.contains('%');
    let digits: String = value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.parse::<f64>().ok().map(|factor| (factor, percent))
}

/// Normalize a dimension and apply a scale factor: pixels round to the nearest integer (ties to
/// even), percentages always carry a fractional part, and other units keep their shortest form.
fn normalize_dimension(value: &str, scale: Option<(f64, bool)>) -> String {
    let Some(dimension) = parse_dimension(value) else {
        return value.to_string();
    };
    let dimension = scale_dimension(dimension, scale);
    match dimension {
        Dimension::Pixel(pixels) => format!("{}px", pixels.round_ties_even()),
        Dimension::Percent(percent) => {
            let text = format!("{percent}");
            if text.contains('.') {
                format!("{text}%")
            } else {
                format!("{text}.0%")
            }
        }
        Dimension::Other(magnitude, unit) => format!("{magnitude}{unit}"),
    }
}

fn parse_dimension(value: &str) -> Option<Dimension> {
    let value = value.trim();
    let split = value
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.'))
        .map_or(value.len(), |(index, _)| index);
    let magnitude: f64 = value.get(..split)?.parse().ok()?;
    let unit = value.get(split..).unwrap_or("").trim();
    Some(match unit {
        "" | "px" => Dimension::Pixel(magnitude.trunc()),
        "%" => Dimension::Percent(magnitude),
        other => Dimension::Other(magnitude, other.to_string()),
    })
}

fn scale_dimension(dimension: Dimension, scale: Option<(f64, bool)>) -> Dimension {
    let Some((factor, percent)) = scale else {
        return dimension;
    };
    let divisor = if percent { 100.0 } else { 1.0 };
    let apply = |value: f64| value * factor / divisor;
    match dimension {
        Dimension::Pixel(value) => Dimension::Pixel(apply(value)),
        Dimension::Percent(value) => Dimension::Percent(apply(value)),
        Dimension::Other(value, unit) => Dimension::Other(apply(value), unit),
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

/// When a paragraph's only content is an attribute-free span — the shape a multi-inline substitution
/// expands to — the span dissolves into the paragraph, which carries its inlines directly.
fn splice_lone_span(mut inlines: Vec<Inline>) -> Vec<Inline> {
    let lone_plain_span = matches!(
        inlines.as_slice(),
        [Inline::Span(attr, _)]
            if attr.id.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty()
    );
    if lone_plain_span && let Some(Inline::Span(_, inner)) = inlines.pop() {
        return inner;
    }
    inlines
}

/// Attach internal-target identifiers to the block they precede. A single target immediately before
/// a section heading supplies the heading's identifier; otherwise each target wraps the block in a
/// division carrying its identifier, the last target sitting innermost.
fn attach_targets(mut blocks: Vec<Block>, mut targets: Vec<String>) -> Vec<Block> {
    if targets.len() == 1
        && let [Block::Header(_, attr, _)] = blocks.as_mut_slice()
    {
        attr.id = targets.remove(0);
        return blocks;
    }
    for name in targets.into_iter().rev() {
        blocks = vec![Block::Div(
            Attr {
                id: name,
                classes: Vec::new(),
                attributes: Vec::new(),
            },
            blocks,
        )];
    }
    blocks
}

/// Split a directive's options into the identifier it sets (`:name:`), the extra classes it adds
/// (`:class:`), and the remaining options carried as attributes, each in source order.
fn common_options(options: &[(String, String)]) -> (String, Vec<String>, Vec<(String, String)>) {
    let mut id = String::new();
    let mut classes = Vec::new();
    let mut attributes = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "name" => id.clone_from(value),
            "class" => classes.extend(value.split_whitespace().map(str::to_string)),
            other => attributes.push((other.to_string(), value.clone())),
        }
    }
    (id, classes, attributes)
}

/// Wrap a directive's blocks in a division named for the directive, folding its common options into
/// the division's identifier, classes, and attributes. The directive name leads the class list.
fn options_div(name: &str, options: &[(String, String)], blocks: Vec<Block>) -> Block {
    let (id, extra, attributes) = common_options(options);
    let mut classes = vec![name.to_string()];
    classes.extend(extra);
    Block::Div(
        Attr {
            id,
            classes,
            attributes,
        },
        blocks,
    )
}

/// Group a directive body into the runs of consecutive non-blank lines, joined with newlines and
/// trimmed. A blank line separates one group from the next; empty groups are dropped.
fn blank_separated(lines: &[String]) -> Vec<String> {
    let mut groups = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            if !current.is_empty() {
                groups.push(current.join("\n").trim().to_string());
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        groups.push(current.join("\n").trim().to_string());
    }
    groups
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Reduce text to ASCII for identifier derivation: an accented Latin letter maps to its base letter,
/// any remaining non-ASCII character is dropped, and ASCII characters pass through unchanged. The
/// caller's slug step then keeps only the identifier-valid characters.
fn asciify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else if let Some(base) = ascii_base(ch) {
            out.push(base);
        }
    }
    out
}

/// The base ASCII letter an accented Latin letter reduces to, or `None` when the character has no
/// such base (ligatures, stroked letters, and non-Latin scripts are dropped).
fn ascii_base(ch: char) -> Option<char> {
    let base = match ch {
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' | 'Ą' => 'a',
        'Ç' | 'Ć' | 'Č' | 'Ĉ' | 'Ċ' => 'c',
        'Ď' | 'Ḋ' => 'd',
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => 'e',
        'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => 'g',
        'Ĥ' => 'h',
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ĩ' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => 'i',
        'Ĵ' => 'j',
        'Ķ' => 'k',
        'Ĺ' | 'Ļ' | 'Ľ' => 'l',
        'Ñ' | 'Ń' | 'Ņ' | 'Ň' => 'n',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ō' | 'Ŏ' | 'Ő' => 'o',
        'Ŕ' | 'Ŗ' | 'Ř' => 'r',
        'Ś' | 'Ŝ' | 'Ş' | 'Š' => 's',
        'Ţ' | 'Ť' => 't',
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ũ' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => 'u',
        'Ŵ' => 'w',
        'Ý' | 'Ŷ' | 'Ÿ' => 'y',
        'Ź' | 'Ż' | 'Ž' => 'z',
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => 'c',
        'ď' | 'ḋ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ŷ' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        _ => return None,
    };
    Some(base)
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
        } else if ch == ' ' || ch == '\t' {
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

/// Whether markup is suppressed because it is wrapped in a matching pair of quoting characters: a
/// run opened by `"`, `'`, or `<` and closed by its partner keeps its contents as literal text.
fn quote_suppresses(before: Option<char>, after: Option<char>) -> bool {
    matches!(
        (before, after),
        (Some('"'), Some('"')) | (Some('\''), Some('\'')) | (Some('<'), Some('>'))
    )
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

// --- bare URI and email autolinking ------------------------------------------------------------

/// Whether the character before a candidate autolink permits it to begin: a boundary, whitespace, or
/// an opening bracket. This keeps an address that is already part of larger markup (an angle-bracket
/// URI, a word fragment) from being linked twice.
fn autolink_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '(' | '[' | '{'),
    }
}

/// Attempt to auto-link a bare URI or email address beginning at `pos`.
fn autolink(chars: &[char], pos: usize) -> Option<(Inline, usize)> {
    try_uri_autolink(chars, pos).or_else(|| try_email_autolink(chars, pos))
}

/// Match a bare URI `scheme://…` whose scheme is registered, returning the link and the end index.
fn try_uri_autolink(chars: &[char], pos: usize) -> Option<(Inline, usize)> {
    if !chars.get(pos).is_some_and(char::is_ascii_alphabetic) {
        return None;
    }
    let mut k = pos;
    while chars
        .get(k)
        .is_some_and(|&c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        k += 1;
    }
    if !(chars.get(k) == Some(&':')
        && chars.get(k + 1) == Some(&'/')
        && chars.get(k + 2) == Some(&'/'))
    {
        return None;
    }
    let scheme: String = chars.get(pos..k)?.iter().collect::<String>().to_lowercase();
    if !SCHEMES.contains(&scheme.as_str()) {
        return None;
    }
    let content_start = k + 3;
    let scan_end = forward_scan(chars, pos);
    let end = trim_trailing(chars, content_start, scan_end);
    if end <= content_start {
        return None;
    }
    let url: String = chars.get(pos..end)?.iter().collect();
    Some((link_to(url), end))
}

/// Match a bare email address `local@domain`, returning a `mailto:` link and the end index.
fn try_email_autolink(chars: &[char], pos: usize) -> Option<(Inline, usize)> {
    let mut i = pos;
    while chars.get(i).is_some_and(|&c| is_email_local(c)) {
        i += 1;
    }
    if i == pos || chars.get(i) != Some(&'@') {
        return None;
    }
    i += 1;
    let domain_start = i;
    let mut dots = 0usize;
    let mut end = i;
    loop {
        let label_start = i;
        if !chars.get(i).is_some_and(char::is_ascii_alphanumeric) {
            break;
        }
        while chars
            .get(i)
            .is_some_and(|&c| c.is_ascii_alphanumeric() || c == '-')
        {
            i += 1;
        }
        let mut label_end = i;
        while label_end > label_start && chars.get(label_end - 1) == Some(&'-') {
            label_end -= 1;
        }
        end = label_end;
        i = label_end;
        if chars.get(i) == Some(&'.') {
            dots += 1;
            i += 1;
        } else {
            break;
        }
    }
    if dots == 0 || end <= domain_start {
        return None;
    }
    let address: String = chars.get(pos..end)?.iter().collect();
    Some((
        Inline::Link(
            Attr::default(),
            vec![Inline::Str(address.clone())],
            Target {
                url: format!("mailto:{address}"),
                title: String::new(),
            },
        ),
        end,
    ))
}

/// A link whose visible text and destination are the same URL.
fn link_to(url: String) -> Inline {
    Inline::Link(
        Attr::default(),
        vec![Inline::Str(url.clone())],
        Target {
            url,
            title: String::new(),
        },
    )
}

/// Whether a character may appear in an email address's local part.
fn is_email_local(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '.' | '!'
                | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '/'
                | '='
                | '?'
                | '^'
                | '_'
                | '`'
                | '{'
                | '|'
                | '}'
                | '~'
                | '-'
        )
}

/// Walk a URL run forward, stopping at whitespace or `<`, balancing parentheses, and ending at an
/// unbalanced `)` or a `]` outside any parenthesis.
fn forward_scan(chars: &[char], from: usize) -> usize {
    let mut depth: i32 = 0;
    let mut j = from;
    while let Some(&c) = chars.get(j) {
        if c.is_whitespace() || c == '<' {
            break;
        }
        match c {
            '(' => depth += 1,
            ')' | ']' if depth == 0 => break,
            ')' => depth -= 1,
            _ => {}
        }
        j += 1;
    }
    j
}

/// Drop trailing punctuation from a URL run, never below `min`. A trailing `;` takes a preceding
/// `&entity;` with it.
fn trim_trailing(chars: &[char], min: usize, mut end: usize) -> usize {
    while end > min {
        match chars.get(end - 1) {
            Some('!' | '"' | '\'' | '*' | ',' | '.' | ':' | '?' | '_' | '~') => end -= 1,
            Some(';') => {
                let mut j = end - 1;
                while j > min
                    && chars
                        .get(j - 1)
                        .is_some_and(|&c| c.is_ascii_alphanumeric() || c == '#')
                {
                    j -= 1;
                }
                end = if j > min && chars.get(j - 1) == Some(&'&') {
                    j - 1
                } else {
                    end - 1
                };
            }
            _ => break,
        }
    }
    end
}

// --- typographic punctuation (smart) -----------------------------------------------------------

/// The number of consecutive `ch` at `pos`.
fn run_length(chars: &[char], pos: usize, ch: char) -> usize {
    let mut n = 0;
    while chars.get(pos + n) == Some(&ch) {
        n += 1;
    }
    n
}

/// Fold a run of `n` hyphens into em and en dashes: every three become an em dash, a remaining two a
/// single en dash, a remaining one a hyphen.
fn fold_dashes(n: usize) -> String {
    let mut s = "\u{2014}".repeat(n / 3);
    match n % 3 {
        2 => s.push('\u{2013}'),
        1 => s.push('-'),
        _ => {}
    }
    s
}

/// Fold a run of `n` dots: every three become an ellipsis, with any remainder kept as dots.
fn fold_ellipsis(n: usize) -> String {
    let mut s = "\u{2026}".repeat(n / 3);
    s.push_str(&".".repeat(n % 3));
    s
}

/// The quote-node kind for a straight quote character.
fn quote_type(quote: char) -> QuoteType {
    if quote == '\'' {
        QuoteType::SingleQuote
    } else {
        QuoteType::DoubleQuote
    }
}

/// The curly glyph a non-paired straight quote folds into: an apostrophe for `'`, and an opening or
/// closing double quote depending on which side it leans.
fn quote_glyph(chars: &[char], pos: usize, quote: char) -> char {
    if quote == '\'' {
        '\u{2019}'
    } else if can_open_quote(chars, pos) {
        '\u{201c}'
    } else {
        '\u{201d}'
    }
}

/// Whether a character counts as punctuation for flanking: ASCII punctuation, or any other
/// non-alphanumeric, non-whitespace character.
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace())
}

fn is_ws_opt(opt: Option<char>) -> bool {
    opt.is_none_or(char::is_whitespace)
}

fn is_punct_opt(opt: Option<char>) -> bool {
    opt.is_some_and(is_punct)
}

/// Whether the quote at `pos` leans against following content (may open a quoted run).
fn can_open_quote(chars: &[char], pos: usize) -> bool {
    let before = pos.checked_sub(1).and_then(|p| chars.get(p)).copied();
    let after = chars.get(pos + 1).copied();
    !is_ws_opt(after) && (!is_punct_opt(after) || is_ws_opt(before) || is_punct_opt(before))
}

/// Whether the quote at `pos` leans against preceding content (may close a quoted run). A single
/// quote may not close against a following alphanumeric, so a word-internal apostrophe never ends a
/// quotation.
fn can_close_quote(chars: &[char], pos: usize, quote: char) -> bool {
    let before = pos.checked_sub(1).and_then(|p| chars.get(p)).copied();
    let after = chars.get(pos + 1).copied();
    let right_flanking =
        !is_ws_opt(before) && (!is_punct_opt(before) || is_ws_opt(after) || is_punct_opt(after));
    if !right_flanking {
        return false;
    }
    if quote == '\'' {
        !after.is_some_and(|c| c.is_alphanumeric())
    } else {
        true
    }
}

/// Registered URI schemes recognized when auto-linking a bare URI.
const SCHEMES: &[&str] = &[
    "aaa",
    "aaas",
    "about",
    "acap",
    "acct",
    "acr",
    "adiumxtra",
    "afp",
    "afs",
    "aim",
    "apt",
    "attachment",
    "aw",
    "barion",
    "beshare",
    "bitcoin",
    "blob",
    "bolo",
    "browserext",
    "callto",
    "cap",
    "chrome",
    "chrome-extension",
    "cid",
    "coap",
    "coaps",
    "com-eventbrite-attendee",
    "content",
    "crid",
    "cvs",
    "data",
    "dav",
    "dict",
    "dlna-playcontainer",
    "dlna-playsingle",
    "dns",
    "dntp",
    "dtn",
    "dvb",
    "ed2k",
    "example",
    "facetime",
    "fax",
    "feed",
    "feedready",
    "file",
    "filesystem",
    "finger",
    "fish",
    "ftp",
    "geo",
    "gg",
    "git",
    "gizmoproject",
    "go",
    "gopher",
    "gtalk",
    "h323",
    "ham",
    "hcp",
    "http",
    "https",
    "hxxp",
    "hxxps",
    "iax",
    "icap",
    "icon",
    "im",
    "imap",
    "info",
    "iotdisco",
    "ipn",
    "ipp",
    "ipps",
    "irc",
    "irc6",
    "ircs",
    "iris",
    "isostore",
    "itms",
    "jabber",
    "jar",
    "jms",
    "keyparc",
    "lastfm",
    "ldap",
    "ldaps",
    "lvlt",
    "magnet",
    "mailserver",
    "mailto",
    "maps",
    "market",
    "message",
    "mid",
    "mms",
    "modem",
    "mongodb",
    "moz",
    "ms-access",
    "ms-browser-extension",
    "ms-drive-to",
    "ms-enrollment",
    "ms-excel",
    "ms-gamebarservices",
    "ms-getoffice",
    "ms-help",
    "ms-infopath",
    "ms-media-stream-id",
    "ms-officeapp",
    "ms-project",
    "ms-powerpoint",
    "ms-publisher",
    "ms-search-repair",
    "ms-secondary-screen-controller",
    "ms-secondary-screen-setup",
    "ms-settings",
    "ms-settings-airplanemode",
    "ms-settings-bluetooth",
    "ms-settings-camera",
    "ms-settings-cellular",
    "ms-settings-cloudstorage",
    "ms-settings-connectabledevices",
    "ms-settings-displays-topology",
    "ms-settings-emailandaccounts",
    "ms-settings-language",
    "ms-settings-location",
    "ms-settings-lock",
    "ms-settings-nfctransactions",
    "ms-settings-notifications",
    "ms-settings-power",
    "ms-settings-privacy",
    "ms-settings-proximity",
    "ms-settings-screenrotation",
    "ms-settings-wifi",
    "ms-settings-workplace",
    "ms-spd",
    "ms-sttoverlay",
    "ms-transit-to",
    "ms-virtualtouchpad",
    "ms-visio",
    "ms-walk-to",
    "ms-whiteboard",
    "ms-whiteboard-cmd",
    "ms-word",
    "msnim",
    "msrp",
    "msrps",
    "mtqp",
    "mumble",
    "mupdate",
    "mvn",
    "news",
    "nfs",
    "ni",
    "nih",
    "nntp",
    "notes",
    "ocf",
    "oid",
    "onenote",
    "onenote-cmd",
    "opaquelocktoken",
    "pack",
    "palm",
    "paparazzi",
    "pkcs11",
    "platform",
    "pop",
    "pres",
    "prospero",
    "proxy",
    "pwid",
    "psyc",
    "qb",
    "query",
    "redis",
    "rediss",
    "reload",
    "res",
    "resource",
    "rmi",
    "rsync",
    "rtmfp",
    "rtmp",
    "rtsp",
    "rtsps",
    "rtspu",
    "secondlife",
    "service",
    "session",
    "sftp",
    "sgn",
    "shttp",
    "sieve",
    "sip",
    "sips",
    "skype",
    "smb",
    "sms",
    "smtp",
    "snews",
    "snmp",
    "soap.beep",
    "soap.beeps",
    "soldat",
    "spotify",
    "ssh",
    "steam",
    "stun",
    "stuns",
    "submit",
    "svn",
    "tag",
    "teamspeak",
    "tel",
    "teliaeid",
    "telnet",
    "tftp",
    "things",
    "thismessage",
    "tip",
    "tn3270",
    "tool",
    "turn",
    "turns",
    "tv",
    "udp",
    "unreal",
    "urn",
    "ut2004",
    "v-event",
    "vemmi",
    "vnc",
    "view-source",
    "wais",
    "webcal",
    "wpid",
    "ws",
    "wss",
    "wtai",
    "wyciwyg",
    "xcon",
    "xcon-userid",
    "xfire",
    "xmlrpc.beep",
    "xmlrpc.beeps",
    "xmpp",
    "xri",
    "ymsgr",
    "z39.50",
    "z39.50r",
    "z39.50s",
];

// --- grid table helpers ------------------------------------------------------------------------

/// Parse a grid table's top border into the inclusive-exclusive character ranges of its columns.
fn is_grid_line(line: &str) -> bool {
    line.starts_with('+') || line.starts_with('|')
}

/// A cell rectangle traced out of a grid table, in (line, column) matrix coordinates: its corners
/// are the `+` at the top-left and the `+` at the bottom-right.
struct ScanCell {
    top: usize,
    left: usize,
    bottom: usize,
    right: usize,
}

/// A placed grid-table cell: its raw interior text and its extent in row and column bands.
#[derive(Clone)]
struct GridCell {
    text: String,
    row_span: usize,
    col_span: usize,
}

fn grid_at(block: &[Vec<char>], row: usize, col: usize) -> Option<char> {
    block.get(row).and_then(|r| r.get(col)).copied()
}

/// Trace every cell of a grid table out of its character matrix. From the top-left corner, each
/// cell rectangle is found by following its top edge right to a `+`, its right edge down to a `+`,
/// its bottom edge left to the starting column, and its left edge back up to the top — each edge
/// made solely of its border character (`-` across, `|` down), with `+` permitted where another
/// grid line crosses. The corners opposite each cell seed the search for its right and lower
/// neighbours. Returns `None` for a matrix that does not open with a corner.
fn scan_grid_cells(block: &[Vec<char>]) -> Option<Vec<ScanCell>> {
    let height = block.len();
    let width = block.first().map_or(0, Vec::len);
    if height < 2 || width < 2 || grid_at(block, 0, 0) != Some('+') {
        return None;
    }
    let bottom = height - 1;
    let right = width - 1;
    let mut cells = Vec::new();
    let mut visited = vec![vec![false; width]; height];
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    queue.push_back((0, 0));
    while let Some((top, left)) = queue.pop_front() {
        if top >= bottom || left >= right {
            continue;
        }
        if visited.get(top).and_then(|r| r.get(left)).copied() == Some(true) {
            continue;
        }
        if let Some(slot) = visited.get_mut(top).and_then(|r| r.get_mut(left)) {
            *slot = true;
        }
        let Some(cell) = trace_cell(block, top, left, bottom, right) else {
            continue;
        };
        queue.push_back((cell.top, cell.right));
        queue.push_back((cell.bottom, cell.left));
        cells.push(cell);
    }
    Some(cells)
}

fn trace_cell(
    block: &[Vec<char>],
    top: usize,
    left: usize,
    bottom: usize,
    right: usize,
) -> Option<ScanCell> {
    for col in left + 1..=right {
        match grid_at(block, top, col) {
            Some('+') => {
                if let Some(b) = scan_cell_down(block, top, left, col, bottom) {
                    return Some(ScanCell {
                        top,
                        left,
                        bottom: b,
                        right: col,
                    });
                }
            }
            // A `-` extends a body border; `=` extends the header/body separator.
            Some('-' | '=') => {}
            _ => return None,
        }
    }
    None
}

fn scan_cell_down(
    block: &[Vec<char>],
    top: usize,
    left: usize,
    right: usize,
    bottom: usize,
) -> Option<usize> {
    for row in top + 1..=bottom {
        match grid_at(block, row, right) {
            Some('+') => {
                if scan_cell_close(block, top, left, right, row) {
                    return Some(row);
                }
            }
            Some('|') => {}
            _ => return None,
        }
    }
    None
}

/// Verify the bottom and left edges of a candidate cell: the bottom edge from `right` back to
/// `left` is `-` (or a `+` crossing) and reaches a `+` at the bottom-left corner, and the left edge
/// from `bottom` back to `top` is `|` (or a `+` crossing).
fn scan_cell_close(
    block: &[Vec<char>],
    top: usize,
    left: usize,
    right: usize,
    bottom: usize,
) -> bool {
    for col in left + 1..right {
        if !matches!(grid_at(block, bottom, col), Some('-' | '=' | '+')) {
            return false;
        }
    }
    if grid_at(block, bottom, left) != Some('+') {
        return false;
    }
    for row in top + 1..bottom {
        if !matches!(grid_at(block, row, left), Some('|' | '+')) {
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

    #[test]
    fn date_renders_strftime_fields_for_fixed_timestamps() {
        // Expected values follow the Gregorian calendar in UTC; each timestamp is seconds past the
        // epoch. The `date` directive's live form draws on the wall clock, so it is exercised here
        // against frozen moments to keep the assertions reproducible.
        let cases: &[(i64, &str, &str)] = &[
            // 2026-06-29 14:50:50 UTC, a Monday.
            (1_782_744_650, "%Y-%m-%d", "2026-06-29"),
            (1_782_744_650, "%j", "180"),
            (1_782_744_650, "%A %a", "Monday Mon"),
            (1_782_744_650, "%B %b %h", "June Jun Jun"),
            (1_782_744_650, "%u %w", "1 1"),
            (1_782_744_650, "%U %W", "26 26"),
            (1_782_744_650, "%V %G %g", "27 2026 26"),
            (1_782_744_650, "%I %l %p %P", "02  2 PM pm"),
            (1_782_744_650, "%C %y", "20 26"),
            (1_782_744_650, "%D", "06/29/26"),
            (1_782_744_650, "%F %T", "2026-06-29 14:50:50"),
            (1_782_744_650, "%R %k", "14:50 14"),
            (1_782_744_650, "%r", "02:50:50 PM"),
            (1_782_744_650, "%e", "29"),
            // 2024-02-29 00:00:00 UTC, a leap day on a Thursday.
            (1_709_164_800, "%Y-%m-%d", "2024-02-29"),
            (1_709_164_800, "%j", "060"),
            (1_709_164_800, "%A", "Thursday"),
            (1_709_164_800, "%U %W", "08 09"),
            (1_709_164_800, "%V %G %g", "09 2024 24"),
            (1_709_164_800, "%I %p", "12 AM"),
            (1_709_164_800, "%e", "29"),
            // 1970-01-01 00:00:00 UTC, the epoch, a Thursday.
            (0, "%Y-%m-%d", "1970-01-01"),
            (0, "%j", "001"),
            (0, "%A", "Thursday"),
            (0, "%U %W", "00 00"),
            (0, "%V %G %g", "01 1970 70"),
            (0, "%e", " 1"),
            // 2027-01-01 12:00:00 UTC: an ISO week that rolls back into the previous year.
            (1_798_804_800, "%V %G %g", "53 2026 26"),
            (1_798_804_800, "%A", "Friday"),
            (1_798_804_800, "%r", "12:00:00 PM"),
            // A literal percent, and an unrecognized code emitted verbatim.
            (0, "before %% after", "before % after"),
            (0, "%Q", "%Q"),
        ];
        for (secs, format, expected) in cases {
            assert_eq!(
                &render_date(*secs, format),
                expected,
                "render_date({secs}, {format:?})"
            );
        }
        // The empty format string falls back to an ISO date, whatever today happens to be.
        let today = format_date("");
        assert_eq!(today.len(), 10);
        assert_eq!(today.matches('-').count(), 2);
    }

    #[test]
    fn include_directive_splices_referenced_file() {
        let path =
            std::env::temp_dir().join(format!("carta_rst_include_{}.rst", std::process::id()));
        std::fs::write(&path, "Pulled in **bold** text.\n").expect("write temp include");
        let source = format!("Before.\n\n.. include:: {}\n\nAfter.\n", path.display());
        let blocks = parse(&source);
        std::fs::remove_file(&path).ok();

        let paragraphs: Vec<&Vec<Inline>> = blocks
            .iter()
            .filter_map(|block| match block {
                Block::Para(inlines) => Some(inlines),
                _ => None,
            })
            .collect();
        assert_eq!(paragraphs.len(), 3);
        let included = paragraphs.get(1).expect("the spliced include paragraph");
        assert!(
            included
                .iter()
                .any(|inline| matches!(inline, Inline::Strong(_)))
        );
    }

    /// The attributes of the first image found in a paragraph or plain block.
    fn first_image_attr(blocks: &[Block]) -> Option<Attr> {
        for block in blocks {
            let (Block::Para(inlines) | Block::Plain(inlines)) = block else {
                continue;
            };
            for inline in inlines {
                if let Inline::Image(attr, _, _) = inline {
                    return Some(attr.clone());
                }
            }
        }
        None
    }

    fn image_width(source: &str) -> Option<String> {
        first_image_attr(&parse(source))?
            .attributes
            .into_iter()
            .find(|(key, _)| key == "width")
            .map(|(_, value)| value)
    }

    #[test]
    fn image_directive_resolves_width_and_scale() {
        // A pixel width is truncated to an integer at parse time and rounds to even at the boundary
        // once a scale is applied.
        assert_eq!(
            image_width(".. image:: a.png\n   :width: 200px\n   :scale: 50%\n"),
            Some("100px".into())
        );
        assert_eq!(
            image_width(".. image:: a.png\n   :width: 201px\n   :scale: 50%\n"),
            Some("100px".into())
        );
        assert_eq!(
            image_width(".. image:: a.png\n   :width: 100.7px\n"),
            Some("100px".into())
        );
        // A percentage width keeps a single fractional digit.
        assert_eq!(
            image_width(".. image:: a.png\n   :width: 100%\n   :scale: 33\n"),
            Some("3300.0%".into())
        );
        // A physical unit scales and renders in the shortest form.
        assert_eq!(
            image_width(".. image:: a.png\n   :width: 2.5in\n   :scale: 50%\n"),
            Some("1.25in".into())
        );
        assert_eq!(
            image_width(".. image:: a.png\n   :width: 3cm\n"),
            Some("3cm".into())
        );
    }

    #[test]
    fn image_directive_doubles_classes_and_appends_alignment() {
        let classes = |source: &str| first_image_attr(&parse(source)).expect("an image").classes;
        // Alignment alone becomes an `align-<value>` class.
        assert_eq!(
            classes(".. image:: a.png\n   :align: center\n"),
            vec!["align-center".to_string()]
        );
        // An explicit class list is doubled, with the alignment fused onto the final entry.
        assert_eq!(
            classes(".. image:: a.png\n   :class: foo\n   :align: center\n"),
            vec!["foo".to_string(), "fooalign-center".to_string()]
        );
        assert_eq!(
            classes(".. image:: a.png\n   :class: foo bar\n"),
            vec![
                "foo".to_string(),
                "bar".to_string(),
                "foo".to_string(),
                "bar".to_string()
            ]
        );
    }

    #[test]
    fn substitution_image_carries_options() {
        let badge = parse("|i|\n\n.. |i| image:: a.png\n   :class: foo\n   :align: middle\n");
        assert_eq!(
            first_image_attr(&badge).expect("an image").classes,
            vec!["foo".to_string(), "fooalign-middle".to_string()]
        );
        assert_eq!(
            image_width("|i|\n\n.. |i| image:: a.png\n   :width: 200px\n   :scale: 50%\n"),
            Some("100px".into())
        );
    }

    #[test]
    fn figure_directive_separates_figure_and_image_attributes() {
        // `:name:` identifies the inner image, `:align:` classes the figure; the figure id is empty.
        let blocks = parse(".. figure:: a.png\n   :name: first\n   :align: center\n\n   Cap\n");
        let (outer, body) = match blocks.first() {
            Some(Block::Figure(attr, _, body)) => (attr.clone(), body.clone()),
            other => panic!("expected a figure, got {other:?}"),
        };
        assert!(outer.id.is_empty());
        assert_eq!(outer.classes, vec!["align-center".to_string()]);
        let inner = first_image_attr(&body).expect("an inner image");
        assert_eq!(inner.id.as_str(), "first");
        assert!(inner.classes.is_empty());

        // `:figclass:` and `:class:` both class the figure; only `:class:` reaches the inner image.
        let blocks = parse(".. figure:: a.png\n   :figclass: frame\n   :class: photo\n\n   Cap\n");
        let (outer, body) = match blocks.first() {
            Some(Block::Figure(attr, _, body)) => (attr.clone(), body.clone()),
            other => panic!("expected a figure, got {other:?}"),
        };
        assert_eq!(
            outer.classes,
            vec!["frame".to_string(), "photo".to_string()]
        );
        let inner = first_image_attr(&body).expect("an inner image");
        assert_eq!(inner.classes, vec!["photo".to_string()]);
    }
}
