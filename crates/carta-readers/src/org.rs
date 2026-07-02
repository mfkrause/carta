//! Org reader: parses Org markup into the document model.
//!
//! Parsing is two-phase. A line-oriented block pass consumes the input into [`Block`]s, dispatching
//! on each line's opening: headlines (`* `), greater blocks (`#+begin_…`/`#+end_…`), keyword lines
//! (`#+key: value`), tables (`|`), lists, drawers, fixed-width (`: `) and comment (`# `) lines, and
//! everything else as a paragraph. A second, per-fragment pass then scans each paragraph, headline,
//! cell, and item into [`Inline`]s: emphasis, verbatim, sub/superscripts, links, footnotes, math,
//! entities, and citations.
//!
//! Footnote definitions are gathered up front and their references resolved inline, so a `[fn:label]`
//! reference expands to a [`Inline::Note`] carrying the definition's blocks.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::mem;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Format, Inline,
    ListAttributes, ListNumberDelim, ListNumberStyle, MathType, MetaValue, QuoteType, Row, Table,
    TableBody, TableFoot, TableHead, Text, slug, slug_gfm,
};
use carta_core::{Extension, Extensions, Reader, ReaderOptions, Result};

use crate::heading_ids::{IdRegistry, IdScheme, fold_to_ascii};

/// Parses Org markup into the document model.
///
/// The default extension set enables auto identifiers, citations, task-list checkboxes, and the
/// typographic replacements of `special_strings`; `smart` adds curly quotes, `fancy_lists` numbered
/// list markers, and `gfm_auto_identifiers`/`ascii_identifiers` alternate identifier shapes.
#[derive(Debug, Default, Clone, Copy)]
pub struct OrgReader;

impl Reader for OrgReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let ext = options.extensions;
        let normalized = normalize(input);
        let lines: Vec<&str> = normalized.split('\n').collect();

        let (body_lines, defs) = collect_footnotes(&lines);

        // Footnote bodies are parsed first so a reference in the body can carry the definition's
        // blocks. Nested footnote references inside a definition resolve against an empty table.
        let empty_notes: BTreeMap<String, Vec<Block>> = BTreeMap::new();
        let mut notes: BTreeMap<String, Vec<Block>> = BTreeMap::new();
        for (label, text) in &defs {
            let def_lines: Vec<&str> = text.split('\n').collect();
            let mut throwaway_ids = new_id_registry();
            let mut throwaway_meta = BTreeMap::new();
            let blocks = parse_blocks(
                &def_lines,
                ext,
                &empty_notes,
                &mut throwaway_ids,
                &mut throwaway_meta,
            );
            notes.insert(label.clone(), blocks);
        }

        let mut ids = new_id_registry();
        let mut meta: BTreeMap<Text, MetaValue> = BTreeMap::new();
        let blocks = parse_blocks(&body_lines, ext, &notes, &mut ids, &mut meta);

        Ok(Document {
            meta,
            blocks,
            ..Document::default()
        })
    }
}

/// Normalizes line endings to `\n` so the line-oriented pass sees a single terminator. Input without
/// a carriage return is already normalized and is borrowed unchanged.
fn normalize(input: &str) -> Cow<'_, str> {
    if input.contains('\r') {
        Cow::Owned(input.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(input)
    }
}

// -- Footnote gathering ------------------------------------------------------------------------

/// Splits block-level footnote definitions (`[fn:label] …`) out of the line stream, returning the
/// remaining body lines and the ordered `(label, joined-body)` definitions. A definition's body
/// continues across single blank lines, so it can hold several blocks; it ends at the next footnote
/// definition, a headline, two consecutive blank lines, or the end of input.
fn collect_footnotes<'a>(lines: &[&'a str]) -> (Vec<&'a str>, Vec<(String, String)>) {
    let mut body = Vec::new();
    let mut defs = Vec::new();
    let mut i = 0;
    while let Some(line) = lines.get(i) {
        if let Some((label, first)) = footnote_definition(line) {
            let mut collected = vec![first];
            i += 1;
            while let Some(next) = lines.get(i) {
                if footnote_definition(next).is_some() || headline_level(next).is_some() {
                    break;
                }
                if next.trim().is_empty()
                    && lines
                        .get(i + 1)
                        .is_none_or(|following| following.trim().is_empty())
                {
                    break;
                }
                collected.push((*next).to_owned());
                i += 1;
            }
            defs.push((label, collected.join("\n")));
        } else {
            body.push(*line);
            i += 1;
        }
    }
    (body, defs)
}

/// Recognizes a block-level footnote definition `[fn:label] rest`, returning the label and the text
/// after the closing bracket.
fn footnote_definition(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("[fn:")?;
    let close = rest.find(']')?;
    let label = &rest[..close];
    if label.is_empty() || !label.chars().all(is_footnote_label_char) {
        return None;
    }
    let after = rest.get(close + 1..).unwrap_or("");
    Some((label.to_owned(), after.trim_start().to_owned()))
}

fn is_footnote_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

// -- Identifier derivation ---------------------------------------------------------------------

/// A fresh heading-identifier registry with `section` reserved from the start, so the first heading
/// that reduces to it is already `section-1`.
fn new_id_registry() -> IdRegistry {
    let mut ids = IdRegistry::default();
    ids.reserve_native("section");
    ids
}

/// Derives an identifier for `text` under the active extensions, or an empty string when no
/// auto-identifier extension is on. The slug shape follows the extension, but headings always
/// disambiguate natively: an empty slug becomes `section` and repeats increment until unused.
fn assign_id(ids: &mut IdRegistry, text: &str, ext: Extensions) -> String {
    let Some(scheme) = IdScheme::select(ext, true) else {
        return String::new();
    };
    let folded;
    let source = if ext.contains(Extension::AsciiIdentifiers) {
        folded = fold_to_ascii(text);
        folded.as_str()
    } else {
        text
    };
    let base = match scheme {
        IdScheme::Plain => slug(source),
        IdScheme::Gfm => slug_gfm(source),
    };
    ids.assign_native(base)
}

// -- Block parsing -----------------------------------------------------------------------------

/// Affiliated keywords (`#+caption:`, `#+name:`) that attach to the block that follows them.
#[derive(Default)]
struct Affiliated {
    caption: Option<Vec<Inline>>,
    name: Option<String>,
}

impl Affiliated {
    fn is_empty(&self) -> bool {
        self.caption.is_none() && self.name.is_none()
    }
}

#[allow(clippy::too_many_lines)]
fn parse_blocks(
    lines: &[&str],
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
    meta: &mut BTreeMap<Text, MetaValue>,
) -> Vec<Block> {
    let mut out = Vec::new();
    let mut pending = Affiliated::default();
    let mut i = 0;
    while let Some(&line) = lines.get(i) {
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        // Headline.
        if let Some(level) = headline_level(line) {
            i += 1;
            let mut id_override = None;
            if let Some((custom_id, skip)) = read_property_drawer(lines, i) {
                id_override = custom_id;
                i += skip;
            }
            out.push(build_headline(line, level, id_override, ext, notes, ids));
            pending = Affiliated::default();
            continue;
        }
        // Greater block: #+begin_… / #+end_….
        if let Some(name) = greater_block_open(line) {
            let (block, consumed) = parse_greater_block(lines, i, &name, ext, notes, ids, meta);
            i += consumed;
            if let Some(block) = block {
                out.push(apply_affiliated(block, &mut pending));
            }
            continue;
        }
        // Keyword line: #+key: value.
        if let Some((key, value)) = keyword_line(line) {
            handle_keyword(&key, &value, line, ext, notes, meta, &mut pending, &mut out);
            i += 1;
            continue;
        }
        // Comment line.
        if line.trim_start() == "#" || line.trim_start().starts_with("# ") {
            i += 1;
            continue;
        }
        // Horizontal rule.
        if is_horizontal_rule(line) {
            out.push(Block::HorizontalRule);
            i += 1;
            pending = Affiliated::default();
            continue;
        }
        // Fixed-width (colon) block.
        if is_fixed_width(line) {
            let (text, consumed) = collect_fixed_width(lines, i);
            out.push(Block::CodeBlock(Box::default(), text.into()));
            i += consumed;
            pending = Affiliated::default();
            continue;
        }
        // Drawer.
        if let Some(name) = drawer_open(line) {
            let (inner, consumed) = collect_drawer(lines, i);
            i += consumed;
            // A metadata drawer holds bookkeeping, not document content, and is elided; every other
            // named drawer becomes a div wrapping its parsed contents.
            if name.eq_ignore_ascii_case("PROPERTIES") || name.eq_ignore_ascii_case("LOGBOOK") {
                pending = Affiliated::default();
                continue;
            }
            let body = parse_blocks(&inner, ext, notes, ids, meta);
            let attr = Attr {
                classes: vec![name.into(), "drawer".to_owned().into()],
                ..Attr::default()
            };
            out.push(Block::Div(Box::new(attr), body));
            pending = Affiliated::default();
            continue;
        }
        // Table.
        if is_table_line(line) {
            let (rows, consumed) = collect_table(lines, i);
            let table = build_table(&rows, ext, notes, &mut pending);
            out.push(table);
            i += consumed;
            continue;
        }
        // List.
        if list_marker(line).is_some() {
            let (block, consumed) = parse_list(lines, i, ext, notes, ids, meta);
            i += consumed;
            if let Some(block) = block {
                out.push(block);
            }
            pending = Affiliated::default();
            continue;
        }
        // Paragraph: gather until a structural line or blank. The dispatch above already proved this
        // first line is neither blank nor a block opener, so continuation begins at the next line.
        let start = i;
        i += 1;
        while let Some(&l) = lines.get(i) {
            if l.trim().is_empty() || opens_block(l) {
                break;
            }
            i += 1;
        }
        let text = lines
            .get(start..i)
            .unwrap_or(&[])
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join("\n");
        let para = Block::Para(parse_inlines(&text, ext, notes));
        out.push(apply_affiliated(para, &mut pending));
    }
    out
}

/// Whether a line begins a block that interrupts an open paragraph.
fn opens_block(line: &str) -> bool {
    headline_level(line).is_some()
        || greater_block_open(line).is_some()
        || keyword_line(line).is_some()
        || line.trim_start() == "#"
        || line.trim_start().starts_with("# ")
        || is_horizontal_rule(line)
        || is_fixed_width(line)
        || drawer_open(line).is_some()
        || is_table_line(line)
        || list_marker(line).is_some()
}

/// Attaches a pending caption/name to a freshly built block: a caption turns a lone-image paragraph
/// into a figure, and a name supplies its identifier.
fn apply_affiliated(block: Block, pending: &mut Affiliated) -> Block {
    if pending.is_empty() {
        return block;
    }
    let Affiliated { caption, name } = mem::take(pending);
    match block {
        Block::Para(inlines) if is_lone_image(&inlines) => {
            let attr = Attr {
                id: name.unwrap_or_default().into(),
                ..Attr::default()
            };
            let long = caption.map(|c| vec![Block::Plain(c)]).unwrap_or_default();
            Block::Figure(
                Box::new(attr),
                Box::new(Caption { short: None, long }),
                vec![Block::Plain(inlines)],
            )
        }
        Block::CodeBlock(mut attr, text) => {
            if let Some(name) = name {
                attr.id = name.into();
            }
            Block::CodeBlock(attr, text)
        }
        other => other,
    }
}

fn is_lone_image(inlines: &[Inline]) -> bool {
    matches!(inlines, [Inline::Image(..)])
}

/// The headline level (count of leading `*`) when a line is a headline, i.e. one or more `*` at
/// column zero followed by a space.
fn headline_level(line: &str) -> Option<usize> {
    let stars = line.len() - line.trim_start_matches('*').len();
    if stars == 0 {
        return None;
    }
    match line.as_bytes().get(stars) {
        Some(b' ') => Some(stars),
        _ => None,
    }
}

/// Builds a `Header`, splitting off a leading todo keyword and trailing tags and deriving an
/// identifier from the remaining title text (or the property drawer's custom id).
fn build_headline(
    line: &str,
    level: usize,
    id_override: Option<String>,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
) -> Block {
    let rest = line.get(level..).unwrap_or("").trim();

    let (todo, rest) = split_todo_keyword(rest);
    let (title_text, tags) = split_tags(rest);

    let title_inlines = parse_inlines(title_text, ext, notes);

    let id = if let Some(custom) = id_override {
        ids.reserve_native(&custom);
        custom
    } else {
        assign_id(ids, &carta_ast::to_plain_text(&title_inlines), ext)
    };

    let mut inlines = Vec::new();
    if let Some(keyword) = todo {
        inlines.push(todo_span(keyword));
        inlines.push(Inline::Space);
    }
    inlines.extend(title_inlines);
    if !tags.is_empty() {
        inlines.push(Inline::Space);
        for (n, tag) in tags.iter().enumerate() {
            if n > 0 {
                inlines.push(Inline::Str("\u{a0}".to_owned().into()));
            }
            inlines.push(tag_span(tag));
        }
    }

    let attr = Attr {
        id: id.into(),
        ..Attr::default()
    };
    let level = i32::try_from(level).unwrap_or(6).clamp(1, 6);
    Block::Header(level, Box::new(attr), inlines)
}

fn todo_span(keyword: &str) -> Inline {
    let state = if keyword == "DONE" { "done" } else { "todo" };
    let attr = Attr {
        classes: vec![state.to_owned().into(), keyword.to_owned().into()],
        ..Attr::default()
    };
    Inline::Span(Box::new(attr), vec![Inline::Str(keyword.to_owned().into())])
}

fn tag_span(tag: &str) -> Inline {
    let attr = Attr {
        classes: vec!["tag".to_owned().into()],
        attributes: vec![("tag-name".to_owned().into(), tag.to_owned().into())],
        ..Attr::default()
    };
    Inline::Span(
        Box::new(attr),
        vec![Inline::SmallCaps(vec![Inline::Str(tag.to_owned().into())])],
    )
}

/// Splits a leading `TODO`/`DONE` keyword (which must be followed by a space or end the text) from
/// the headline body.
fn split_todo_keyword(rest: &str) -> (Option<&str>, &str) {
    for keyword in ["TODO", "DONE"] {
        if let Some(after) = rest.strip_prefix(keyword)
            && (after.is_empty() || after.starts_with(' '))
        {
            return (Some(keyword), after.trim_start());
        }
    }
    (None, rest)
}

/// Splits trailing `:tag:tag:` tags from a headline, returning the title text and the tag names.
fn split_tags(rest: &str) -> (&str, Vec<String>) {
    let trimmed = rest.trim_end();
    if !trimmed.ends_with(':') {
        return (rest, Vec::new());
    }
    let Some(space) = trimmed.rfind(char::is_whitespace) else {
        return (rest, Vec::new());
    };
    let candidate = trimmed.get(space + 1..).unwrap_or("");
    if candidate.len() < 2 || !candidate.starts_with(':') || !candidate.ends_with(':') {
        return (rest, Vec::new());
    }
    let inner = &candidate[1..candidate.len() - 1];
    if inner.is_empty()
        || !inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '#' | '%' | ':'))
    {
        return (rest, Vec::new());
    }
    let tags: Vec<String> = inner
        .split(':')
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    if tags.is_empty() {
        return (rest, Vec::new());
    }
    (trimmed.get(..space).unwrap_or("").trim_end(), tags)
}

/// Reads a `:PROPERTIES:`…`:END:` drawer immediately following a headline, returning the custom
/// identifier (if any) and the number of lines consumed. Returns `None` when no drawer follows.
fn read_property_drawer(lines: &[&str], start: usize) -> Option<(Option<String>, usize)> {
    let first = lines.get(start)?;
    if !first.trim().eq_ignore_ascii_case(":PROPERTIES:") {
        return None;
    }
    let mut custom = None;
    let mut i = start + 1;
    while let Some(line) = lines.get(i) {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(":END:") {
            return Some((custom, i + 1 - start));
        }
        if let Some(rest) = trimmed.strip_prefix(':')
            && let Some((key, value)) = rest.split_once(':')
            && key.eq_ignore_ascii_case("CUSTOM_ID")
        {
            custom = Some(value.trim().to_owned());
        }
        i += 1;
    }
    // Unterminated drawer: leave the lines to the block parser.
    None
}

// -- Greater blocks ----------------------------------------------------------------------------

/// The block name of a `#+begin_<name>` line, as written (case preserved). Callers compare it
/// case-insensitively.
fn greater_block_open(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = strip_prefix_ci(trimmed, "#+begin_")?;
    let name: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect::<String>();
    if name.is_empty() { None } else { Some(name) }
}

#[allow(clippy::too_many_arguments)]
fn parse_greater_block(
    lines: &[&str],
    start: usize,
    name: &str,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
    meta: &mut BTreeMap<Text, MetaValue>,
) -> (Option<Block>, usize) {
    // `name` is the block name parsed from this same open line, so the header arguments are whatever
    // follows it on that line.
    let open_line = lines.get(start).copied().unwrap_or("");
    let header_args = strip_prefix_ci(open_line.trim_start(), "#+begin_")
        .unwrap_or("")
        .get(name.len()..)
        .unwrap_or("")
        .trim();

    let lower = name.to_ascii_lowercase();
    let end_marker = format!("#+end_{lower}");
    let mut depth = 1usize;
    let mut content: Vec<&str> = Vec::new();
    let mut i = start + 1;
    while let Some(&line) = lines.get(i) {
        let t = line.trim_start();
        if let Some(open) = greater_block_open(line)
            && open.eq_ignore_ascii_case(name)
        {
            depth += 1;
        }
        if t.eq_ignore_ascii_case(&end_marker) {
            depth -= 1;
            if depth == 0 {
                i += 1;
                break;
            }
        }
        content.push(line);
        i += 1;
    }
    let consumed = i - start;

    let block = match lower.as_str() {
        "src" => {
            let lang = header_args
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_owned();
            let attr = Attr {
                classes: if lang.is_empty() {
                    vec![]
                } else {
                    vec![lang.into()]
                },
                ..Attr::default()
            };
            Some(Block::CodeBlock(
                Box::new(attr),
                dedent_verbatim(&content).into(),
            ))
        }
        "example" => Some(Block::CodeBlock(
            Box::default(),
            dedent_verbatim(&content).into(),
        )),
        "export" => {
            let fmt = header_args
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_owned();
            Some(Block::RawBlock(
                Format(fmt.into()),
                verbatim(&content).into(),
            ))
        }
        "quote" => Some(Block::BlockQuote(parse_blocks(
            &content, ext, notes, ids, meta,
        ))),
        "verse" => Some(Block::LineBlock(
            content
                .iter()
                .map(|l| parse_inlines(l.trim(), ext, notes))
                .collect(),
        )),
        "comment" => None,
        _ => {
            let attr = Attr {
                classes: vec![name.to_owned().into()],
                ..Attr::default()
            };
            Some(Block::Div(
                Box::new(attr),
                parse_blocks(&content, ext, notes, ids, meta),
            ))
        }
    };
    (block, consumed)
}

/// Joins verbatim content lines with a trailing newline on each.
fn verbatim(lines: &[&str]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Joins verbatim content, first stripping the common leading indentation shared by all non-blank
/// lines.
fn dedent_verbatim(lines: &[&str]) -> String {
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for line in lines {
        let trimmed = line.get(indent..).unwrap_or("");
        out.push_str(if line.trim().is_empty() {
            line
        } else {
            trimmed
        });
        out.push('\n');
    }
    out
}

// -- Keyword lines -----------------------------------------------------------------------------

/// Splits a `#+key: value` keyword line into `(key, value)`. Block delimiters (`#+begin_…`) are not
/// keyword lines.
fn keyword_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("#+")?;
    let colon = rest.find(':')?;
    let key = rest.get(..colon)?;
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return None;
    }
    if key.eq_ignore_ascii_case("begin_src")
        || starts_with_ci(key, "begin_")
        || starts_with_ci(key, "end_")
    {
        return None;
    }
    let value = rest.get(colon + 1..).unwrap_or("").trim_start().to_owned();
    Some((key.to_owned(), value))
}

#[allow(clippy::too_many_arguments)]
fn handle_keyword(
    key: &str,
    value: &str,
    line: &str,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    meta: &mut BTreeMap<Text, MetaValue>,
    pending: &mut Affiliated,
    out: &mut Vec<Block>,
) {
    let upper = key.to_ascii_uppercase();
    match upper.as_str() {
        "TITLE" | "SUBTITLE" | "AUTHOR" | "DATE" | "KEYWORDS" | "DESCRIPTION" => {
            meta.insert(
                upper.to_ascii_lowercase().into(),
                MetaValue::MetaInlines(parse_inlines(value, ext, notes)),
            );
        }
        "LANGUAGE" => {
            meta.insert(
                "lang".to_owned().into(),
                MetaValue::MetaString(value.to_owned().into()),
            );
        }
        "CAPTION" => pending.caption = Some(parse_inlines(value, ext, notes)),
        "NAME" | "LABEL" => pending.name = Some(value.to_owned()),
        "OPTIONS" | "TODO" | "SEQ_TODO" | "TYP_TODO" | "PRIORITIES" | "TAGS" | "COLUMNS"
        | "SETUPFILE" | "CONSTANTS" | "MACRO" | "DRAWERS" | "ARCHIVE" | "RESULTS" | "HEADER"
        | "PLOT" => {}
        other if other.starts_with("ATTR_") => {}
        other if other.starts_with("LATEX_HEADER") => {
            append_header_include(meta, "latex", value);
        }
        other if other.starts_with("HTML_HEAD") => {
            append_header_include(meta, "html", value);
        }
        _ => out.push(Block::RawBlock(
            Format("org".to_owned().into()),
            line.trim_end().to_owned().into(),
        )),
    }
}

fn append_header_include(meta: &mut BTreeMap<Text, MetaValue>, format: &str, value: &str) {
    let entry = MetaValue::MetaInlines(vec![Inline::RawInline(
        Format(format.to_owned().into()),
        value.to_owned().into(),
    )]);
    match meta
        .entry("header-includes".to_owned().into())
        .or_insert_with(|| MetaValue::MetaList(Vec::new()))
    {
        MetaValue::MetaList(list) => list.push(entry),
        slot => *slot = MetaValue::MetaList(vec![entry]),
    }
}

// -- Fixed-width, drawers, rules ---------------------------------------------------------------

fn is_horizontal_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 5 && t.chars().all(|c| c == '-')
}

fn is_fixed_width(line: &str) -> bool {
    let t = line.trim_start();
    t == ":" || t.starts_with(": ")
}

fn collect_fixed_width(lines: &[&str], start: usize) -> (String, usize) {
    let mut text = String::new();
    let mut i = start;
    while let Some(&line) = lines.get(i) {
        if !is_fixed_width(line) {
            break;
        }
        let t = line.trim_start();
        let content = t
            .strip_prefix(": ")
            .or_else(|| t.strip_prefix(':'))
            .unwrap_or("");
        text.push_str(content);
        text.push('\n');
        i += 1;
    }
    (text, i - start)
}

/// The drawer name of a `:NAME:` line (excluding `:END:`), or `None` when the line is not a drawer.
fn drawer_open(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix(':')?.strip_suffix(':')?;
    if inner.is_empty()
        || inner.contains(':')
        || inner.eq_ignore_ascii_case("END")
        || !inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '@' | '#' | '%'))
    {
        return None;
    }
    Some(inner.to_owned())
}

fn collect_drawer<'a>(lines: &[&'a str], start: usize) -> (Vec<&'a str>, usize) {
    let mut inner = Vec::new();
    let mut i = start + 1;
    while let Some(&line) = lines.get(i) {
        if line.trim().eq_ignore_ascii_case(":END:") {
            i += 1;
            break;
        }
        inner.push(line);
        i += 1;
    }
    (inner, i - start)
}

// -- Tables ------------------------------------------------------------------------------------

fn is_table_line(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// One parsed table row: either a separator (`|---+---|`) or content cells.
enum TableRow {
    Separator,
    Cells(Vec<String>),
}

fn collect_table(lines: &[&str], start: usize) -> (Vec<TableRow>, usize) {
    let mut rows = Vec::new();
    let mut i = start;
    while let Some(&line) = lines.get(i) {
        if !is_table_line(line) {
            break;
        }
        rows.push(parse_table_row(line));
        i += 1;
    }
    (rows, i - start)
}

fn parse_table_row(line: &str) -> TableRow {
    let t = line.trim();
    let inner = t.strip_prefix('|').unwrap_or(t);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    if !inner.is_empty()
        && inner
            .chars()
            .all(|c| matches!(c, '-' | '+' | '|' | ' ' | ':'))
    {
        return TableRow::Separator;
    }
    let cells = inner.split('|').map(|c| c.trim().to_owned()).collect();
    TableRow::Cells(cells)
}

fn build_table(
    rows: &[TableRow],
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    pending: &mut Affiliated,
) -> Block {
    let mut head_rows: Vec<Vec<String>> = Vec::new();
    let mut body_rows: Vec<Vec<String>> = Vec::new();
    let mut seen_separator = false;
    let mut header_done = false;
    for row in rows {
        match row {
            TableRow::Separator => {
                if !body_rows.is_empty() {
                    header_done = true;
                } else if !head_rows.is_empty() {
                    seen_separator = true;
                }
            }
            TableRow::Cells(cells) => {
                if seen_separator || header_done {
                    body_rows.push(cells.clone());
                } else {
                    head_rows.push(cells.clone());
                }
            }
        }
    }
    // With no separator, every row is a body row.
    if !seen_separator {
        body_rows.splice(0..0, head_rows.drain(..));
    }

    let columns = head_rows
        .iter()
        .chain(body_rows.iter())
        .map(Vec::len)
        .max()
        .unwrap_or(0);

    let col_specs = (0..columns)
        .map(|_| ColSpec {
            align: Alignment::AlignDefault,
            width: ColWidth::ColWidthDefault,
        })
        .collect();

    let to_rows = |cells: &[Vec<String>]| -> Vec<Row> {
        cells
            .iter()
            .map(|row| Row {
                attr: Attr::default(),
                cells: (0..columns)
                    .map(|c| build_cell(row.get(c).map_or("", String::as_str), ext, notes))
                    .collect(),
            })
            .collect()
    };

    let Affiliated { caption, name } = mem::take(pending);
    let caption = Caption {
        short: None,
        long: caption.map(|c| vec![Block::Plain(c)]).unwrap_or_default(),
    };

    let table = Table {
        attr: Attr {
            id: name.unwrap_or_default().into(),
            ..Attr::default()
        },
        caption,
        col_specs,
        head: TableHead {
            attr: Attr::default(),
            rows: to_rows(&head_rows),
        },
        bodies: vec![TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: Vec::new(),
            body: to_rows(&body_rows),
        }],
        foot: TableFoot::default(),
    };
    Block::Table(Box::new(table))
}

fn build_cell(text: &str, ext: Extensions, notes: &BTreeMap<String, Vec<Block>>) -> Cell {
    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![Block::Plain(parse_inlines(text, ext, notes))]
    };
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content,
    }
}

// -- Lists -------------------------------------------------------------------------------------

/// The kind of a list item marker.
#[derive(Clone, Copy, PartialEq)]
enum Marker {
    Bullet,
    Ordered(ListNumberStyle, ListNumberDelim),
}

/// A recognized list marker: its column, the width consumed by the marker plus following space, and
/// the marker kind.
struct MarkerInfo {
    indent: usize,
    content_col: usize,
    kind: Marker,
}

fn list_marker(line: &str) -> Option<MarkerInfo> {
    let indent = line.len() - line.trim_start().len();
    let rest = line.get(indent..)?;
    let bytes = rest.as_bytes();
    // Bullet: '-' or '+', or '*' only when indented.
    if let Some(&c) = bytes.first()
        && (matches!(c, b'-' | b'+') || (c == b'*' && indent > 0))
        && (bytes.get(1) == Some(&b' ') || bytes.len() == 1)
    {
        return Some(MarkerInfo {
            indent,
            content_col: indent + 2,
            kind: Marker::Bullet,
        });
    }
    // Ordered: digits or a single letter, then '.' or ')'.
    let mut j = 0;
    while bytes.get(j).is_some_and(u8::is_ascii_digit) {
        j += 1;
    }
    let style = if j > 0 {
        ListNumberStyle::Decimal
    } else if let Some(&letter) = bytes
        .first()
        .filter(|c| c.is_ascii_alphabetic())
        .filter(|_| bytes.get(1).is_some_and(|&c| c == b'.' || c == b')'))
    {
        j = 1;
        if letter.is_ascii_uppercase() {
            ListNumberStyle::UpperAlpha
        } else {
            ListNumberStyle::LowerAlpha
        }
    } else {
        return None;
    };
    let delim = match bytes.get(j) {
        Some(b'.') => ListNumberDelim::Period,
        Some(b')') => ListNumberDelim::OneParen,
        _ => return None,
    };
    if bytes.get(j + 1) == Some(&b' ') || bytes.len() == j + 1 {
        Some(MarkerInfo {
            indent,
            content_col: indent + j + 2,
            kind: Marker::Ordered(style, delim),
        })
    } else {
        None
    }
}

fn parse_list(
    lines: &[&str],
    start: usize,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
    meta: &mut BTreeMap<Text, MetaValue>,
) -> (Option<Block>, usize) {
    let Some(first) = list_marker(lines.get(start).copied().unwrap_or("")) else {
        return (None, 1);
    };
    let base_indent = first.indent;
    let first_kind = first.kind;

    let mut items: Vec<Vec<&str>> = Vec::new();
    let mut loose = false;
    let mut i = start;
    let mut pending_blank = false;

    while let Some(&line) = lines.get(i) {
        if line.trim().is_empty() {
            pending_blank = true;
            i += 1;
            continue;
        }
        if let Some(marker) = list_marker(line)
            && marker.indent == base_indent
            && same_series(first_kind, marker.kind)
        {
            if pending_blank && !items.is_empty() {
                loose = true;
            }
            pending_blank = false;
            let content_col = marker.content_col;
            let mut item_lines = vec![line.get(content_col..).unwrap_or("")];
            i += 1;
            // Gather continuation lines belonging to this item.
            while let Some(&next) = lines.get(i) {
                if next.trim().is_empty() {
                    pending_blank = true;
                    item_lines.push("");
                    i += 1;
                    continue;
                }
                let next_indent = next.len() - next.trim_start().len();
                let is_sibling = list_marker(next).is_some_and(|m| m.indent == base_indent);
                if next_indent > base_indent && !is_sibling {
                    if pending_blank {
                        loose = true;
                    }
                    pending_blank = false;
                    item_lines.push(dedent_line(next, content_col));
                    i += 1;
                } else {
                    break;
                }
            }
            // Trim a trailing blank kept inside the item.
            while item_lines.last() == Some(&"") {
                item_lines.pop();
            }
            items.push(item_lines);
            continue;
        }
        break;
    }

    if items.is_empty() {
        return (None, 1);
    }

    // Definition list when the first item carries a `::` separator.
    if let Some(defs) = try_definition_list(&items, ext, notes, ids, meta, loose) {
        return (Some(defs), i - start);
    }

    let item_blocks: Vec<Vec<Block>> = items
        .iter()
        .map(|item| {
            let blocks = parse_list_item(item, ext, notes, ids, meta);
            if loose { blocks } else { tighten(blocks) }
        })
        .collect();

    let block = match first_kind {
        Marker::Bullet => Block::BulletList(item_blocks),
        Marker::Ordered(style, delim) => {
            let (style, delim) = if ext.contains(Extension::FancyLists) {
                (style, delim)
            } else {
                (ListNumberStyle::DefaultStyle, ListNumberDelim::DefaultDelim)
            };
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style,
                    delim,
                },
                item_blocks,
            )
        }
    };
    (Some(block), i - start)
}

/// Whether two markers belong to the same list (both bullets, or both ordered).
fn same_series(a: Marker, b: Marker) -> bool {
    matches!(
        (a, b),
        (Marker::Bullet, Marker::Bullet) | (Marker::Ordered(..), Marker::Ordered(..))
    )
}

fn parse_list_item(
    item: &[&str],
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
    meta: &mut BTreeMap<Text, MetaValue>,
) -> Vec<Block> {
    let mut lines = item.to_vec();
    let mut checkbox = None;
    if ext.contains(Extension::TaskLists)
        && let Some(first) = lines.first_mut()
        && let Some((glyph, rest)) = strip_checkbox(first)
    {
        checkbox = Some(glyph);
        *first = rest;
    }
    let mut blocks = parse_blocks(&lines, ext, notes, ids, meta);
    if let Some(glyph) = checkbox {
        prepend_checkbox(&mut blocks, glyph);
    }
    blocks
}

/// Splits a leading `[ ]`/`[X]`/`[-]` checkbox off a list item's first line, returning its ballot
/// glyph and the remaining text. The checkbox must be followed by a space or end the line.
fn strip_checkbox(line: &str) -> Option<(&'static str, &str)> {
    for (token, glyph) in [
        ("[ ]", "\u{2610}"),
        ("[-]", "\u{2610}"),
        ("[X]", "\u{2612}"),
    ] {
        if let Some(rest) = line.strip_prefix(token) {
            if rest.is_empty() {
                return Some((glyph, rest));
            }
            if let Some(after) = rest.strip_prefix(' ') {
                return Some((glyph, after));
            }
        }
    }
    None
}

/// Prepends a checkbox glyph to a list item's first inline-bearing block, or introduces a plain block
/// when the item has no content.
fn prepend_checkbox(blocks: &mut Vec<Block>, glyph: &str) {
    match blocks.first_mut() {
        Some(Block::Plain(inlines) | Block::Para(inlines)) => {
            inlines.splice(0..0, [Inline::Str(glyph.to_owned().into()), Inline::Space]);
        }
        _ => blocks.insert(0, Block::Plain(vec![Inline::Str(glyph.to_owned().into())])),
    }
}

/// Converts leading paragraphs to plain blocks for a tight list.
fn tighten(blocks: Vec<Block>) -> Vec<Block> {
    blocks
        .into_iter()
        .map(|b| match b {
            Block::Para(inlines) => Block::Plain(inlines),
            other => other,
        })
        .collect()
}

fn try_definition_list(
    items: &[Vec<&str>],
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
    meta: &mut BTreeMap<Text, MetaValue>,
    loose: bool,
) -> Option<Block> {
    let first = items.first()?;
    split_definition(first.first().copied().unwrap_or(""))?;
    let mut entries = Vec::new();
    for item in items {
        let head = item.first().copied().unwrap_or("");
        let (term_text, def_first) = match split_definition(head) {
            Some(pair) => pair,
            None => (head, ""),
        };
        let term = parse_inlines(term_text.trim(), ext, notes);
        let mut def_lines = vec![def_first];
        def_lines.extend(item.get(1..).unwrap_or(&[]).iter().copied());
        let blocks = parse_blocks(&def_lines, ext, notes, ids, meta);
        let blocks = if loose { blocks } else { tighten(blocks) };
        entries.push((term, vec![blocks]));
    }
    Some(Block::DefinitionList(entries))
}

/// Splits a definition-list item head `term :: definition` into its term and the start of its
/// definition.
fn split_definition(line: &str) -> Option<(&str, &str)> {
    let idx = line.find(" :: ")?;
    Some((line.get(..idx)?, line.get(idx + 4..)?))
}

/// Removes up to `col` leading spaces from a continuation line, borrowing the remaining slice.
fn dedent_line(line: &str, col: usize) -> &str {
    let indent = line.len() - line.trim_start().len();
    let drop = indent.min(col);
    line.get(drop..).unwrap_or("")
}

// -- Inline parsing ----------------------------------------------------------------------------

fn parse_inlines(text: &str, ext: Extensions, notes: &BTreeMap<String, Vec<Block>>) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut scanner = Inlines {
        chars: &chars,
        ext,
        notes,
        out: Vec::new(),
        word: String::new(),
    };
    scanner.run();
    scanner.finish()
}

struct Inlines<'a> {
    chars: &'a [char],
    ext: Extensions,
    notes: &'a BTreeMap<String, Vec<Block>>,
    out: Vec<Inline>,
    word: String,
}

impl Inlines<'_> {
    fn finish(mut self) -> Vec<Inline> {
        self.flush();
        self.out
    }

    fn flush(&mut self) {
        if !self.word.is_empty() {
            self.out.push(Inline::Str(mem::take(&mut self.word).into()));
        }
    }

    fn push_inline(&mut self, inline: Inline) {
        self.flush();
        self.out.push(inline);
    }

    fn at(&self, i: usize) -> Option<char> {
        self.chars.get(i).copied()
    }

    #[allow(clippy::too_many_lines)]
    fn run(&mut self) {
        let mut i = 0;
        while let Some(c) = self.at(i) {
            let prev = if i == 0 { None } else { self.at(i - 1) };

            // Bare autolink at a word boundary.
            if is_url_boundary(prev)
                && let Some((url, end)) = self.scan_bare_url(i)
            {
                self.push_inline(link(&url, vec![Inline::Str(url.clone().into())]));
                i = end;
                continue;
            }

            match c {
                ' ' | '\t' => {
                    self.flush();
                    while matches!(self.at(i), Some(' ' | '\t')) {
                        i += 1;
                    }
                    self.out.push(Inline::Space);
                }
                '\n' => {
                    self.flush();
                    self.out.push(Inline::SoftBreak);
                    i += 1;
                }
                '\\' => i = self.scan_backslash(i),
                '*' | '/' | '+' => {
                    if let Some(end) = self.scan_emphasis(i, c, prev) {
                        let inner = self.chars.get(i + 1..end).unwrap_or(&[]);
                        let content = parse_inlines(&collect_str(inner), self.ext, self.notes);
                        self.push_inline(wrap_markup(c, content));
                        i = end + 1;
                    } else {
                        self.word.push(c);
                        i += 1;
                    }
                }
                '_' => {
                    if let Some(end) = self.scan_emphasis(i, '_', prev) {
                        let inner = self.chars.get(i + 1..end).unwrap_or(&[]);
                        let content = parse_inlines(&collect_str(inner), self.ext, self.notes);
                        self.push_inline(Inline::Underline(content));
                        i = end + 1;
                    } else if let Some((inline, end)) = self.scan_subsup(i, prev, false) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('_');
                        i += 1;
                    }
                }
                '^' => {
                    if let Some((inline, end)) = self.scan_subsup(i, prev, true) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('^');
                        i += 1;
                    }
                }
                '=' | '~' => {
                    // Verbatim uses the same border rules as markup emphasis but takes its body
                    // literally.
                    if let Some(end) = self.scan_emphasis(i, c, prev) {
                        let inner = self.chars.get(i + 1..end).unwrap_or(&[]);
                        self.push_inline(verbatim_code(c, inner));
                        i = end + 1;
                    } else {
                        self.word.push(c);
                        i += 1;
                    }
                }
                '[' => {
                    if let Some((inline, end)) = self.scan_bracket(i) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('[');
                        i += 1;
                    }
                }
                '<' => {
                    if let Some((inline, end)) = self.scan_angle(i) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('<');
                        i += 1;
                    }
                }
                '$' => {
                    if let Some((inline, end)) = self.scan_math_dollar(i, prev) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('$');
                        i += 1;
                    }
                }
                '@' => {
                    if let Some((inline, end)) = self.scan_export(i) {
                        self.push_inline(inline);
                        i = end;
                    } else {
                        self.word.push('@');
                        i += 1;
                    }
                }
                '-' | '.' => {
                    // The typographic dash and ellipsis replacements are always active.
                    if let Some((text, end)) = self.scan_special_string(i) {
                        self.word.push_str(text);
                        i = end;
                    } else {
                        self.word.push(c);
                        i += 1;
                    }
                }
                '\'' if self.ext.contains(Extension::Smart)
                    && prev.is_some_and(char::is_alphanumeric) =>
                {
                    // A word-internal or trailing apostrophe becomes a right single quotation mark.
                    self.word.push('\u{2019}');
                    i += 1;
                }
                '"' | '\'' if self.ext.contains(Extension::Smart) => {
                    let (inline, end) = self.scan_quote(i, c);
                    if let Some(q) = inline {
                        self.push_inline(q);
                        i = end;
                    } else {
                        self.word.push(c);
                        i += 1;
                    }
                }
                _ => {
                    self.word.push(c);
                    i += 1;
                }
            }
        }
    }

    // -- Emphasis ------------------------------------------------------------------------------

    /// Finds the closing marker for markup emphasis opened at `i`, honoring the pre/post border
    /// rules and the single-newline body limit.
    fn scan_emphasis(&self, i: usize, marker: char, prev: Option<char>) -> Option<usize> {
        if !pre_ok(prev) {
            return None;
        }
        let first = self.at(i + 1)?;
        if first.is_whitespace() {
            return None;
        }
        let mut newlines = 0;
        let mut j = i + 1;
        while let Some(c) = self.at(j) {
            if c == '\n' {
                newlines += 1;
                if newlines > 1 {
                    return None;
                }
            }
            if c == marker
                && j > i + 1
                && !self.at(j - 1).is_some_and(char::is_whitespace)
                && post_ok(self.at(j + 1))
            {
                return Some(j);
            }
            j += 1;
        }
        None
    }

    // -- Sub/superscript -----------------------------------------------------------------------

    /// Parses a subscript (`_`) or superscript (`^`) at `i`. Requires a preceding non-space base and
    /// accepts either a `{…}` group or a bare token ending in an alphanumeric.
    fn scan_subsup(&self, i: usize, prev: Option<char>, sup: bool) -> Option<(Inline, usize)> {
        // The base must be a non-space character, and never an underscore: a run like `a__b` is a
        // literal double underscore, not a subscript.
        if prev.is_none_or(|c| c.is_whitespace() || c == '_') {
            return None;
        }
        let content;
        let end;
        if self.at(i + 1) == Some('{') {
            let close = self.match_brace(i + 1)?;
            let inner = self.chars.get(i + 2..close).unwrap_or(&[]);
            content = parse_inlines(&collect_str(inner), self.ext, self.notes);
            end = close + 1;
        } else {
            let (text, stop) = self.scan_bare_script(i + 1)?;
            content = vec![Inline::Str(text.into())];
            end = stop;
        }
        let inline = if sup {
            Inline::Superscript(content)
        } else {
            Inline::Subscript(content)
        };
        Some((inline, end))
    }

    /// Scans a bare sub/superscript token: an optional sign then alphanumerics, dots, and commas,
    /// which must end in an alphanumeric.
    fn scan_bare_script(&self, start: usize) -> Option<(String, usize)> {
        let mut j = start;
        if matches!(self.at(j), Some('-' | '+')) {
            j += 1;
        }
        let body_start = j;
        while matches!(self.at(j), Some(c) if c.is_alphanumeric() || matches!(c, '.' | ',' | '\\'))
        {
            j += 1;
        }
        // Trim trailing non-alphanumerics; require at least one alphanumeric in the body.
        let mut last = j;
        while last > body_start && !self.at(last - 1).is_some_and(char::is_alphanumeric) {
            last -= 1;
        }
        if last <= body_start {
            return None;
        }
        let text: String = self.chars.get(start..last).unwrap_or(&[]).iter().collect();
        Some((text, last))
    }

    fn match_brace(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut j = open;
        while let Some(c) = self.at(j) {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                '\n' => return None,
                _ => {}
            }
            j += 1;
        }
        None
    }

    // -- Backslash: line break, math, entities -------------------------------------------------

    fn scan_backslash(&mut self, i: usize) -> usize {
        match self.at(i + 1) {
            Some('\\') => {
                // Line break: consume both backslashes, trailing spaces, and one newline.
                self.push_inline(Inline::LineBreak);
                let mut j = i + 2;
                while matches!(self.at(j), Some(' ' | '\t')) {
                    j += 1;
                }
                if self.at(j) == Some('\n') {
                    j += 1;
                }
                j
            }
            Some('(') => self.scan_tex_math(i + 2, "\\)", MathType::InlineMath, i),
            Some('[') => self.scan_tex_math(i + 2, "\\]", MathType::DisplayMath, i),
            Some(c) if c.is_ascii_alphabetic() => self.scan_entity(i),
            _ => {
                self.word.push('\\');
                i + 1
            }
        }
    }

    fn scan_tex_math(
        &mut self,
        start: usize,
        close: &str,
        kind: MathType,
        fallback: usize,
    ) -> usize {
        let closing: Vec<char> = close.chars().collect();
        let mut j = start;
        while j < self.chars.len() {
            if self.matches_at(j, &closing) {
                let inner: String = self.chars.get(start..j).unwrap_or(&[]).iter().collect();
                self.push_inline(Inline::Math(kind, inner.into()));
                return j + closing.len();
            }
            j += 1;
        }
        // Unterminated: emit the opening delimiter literally.
        self.word.push('\\');
        fallback + 1
    }

    fn scan_entity(&mut self, i: usize) -> usize {
        let mut j = i + 1;
        while matches!(self.at(j), Some(c) if c.is_ascii_alphabetic()) {
            j += 1;
        }
        let name: String = self.chars.get(i + 1..j).unwrap_or(&[]).iter().collect();
        // An optional `{}` terminates the entity and is consumed.
        let mut end = j;
        if self.at(j) == Some('{') && self.at(j + 1) == Some('}') {
            end = j + 2;
        }
        if let Some(replacement) = entity(&name) {
            self.word.push_str(replacement);
        } else {
            self.push_inline(Inline::RawInline(
                Format("latex".to_owned().into()),
                format!("\\{name}").into(),
            ));
        }
        end
    }

    // -- Brackets: links, footnotes, citations -------------------------------------------------

    fn scan_bracket(&self, i: usize) -> Option<(Inline, usize)> {
        if self.at(i + 1) == Some('[') {
            return self.scan_link(i);
        }
        if self.matches_at(i + 1, &['f', 'n', ':']) {
            return self.scan_footnote(i);
        }
        if self.ext.contains(Extension::Citations)
            && (self.matches_at(i + 1, &['c', 'i', 't', 'e', ':'])
                || self.matches_at(i + 1, &['c', 'i', 't', 'e', '/']))
        {
            return self.scan_citation(i);
        }
        None
    }

    fn scan_link(&self, i: usize) -> Option<(Inline, usize)> {
        // `[[` … `]]`, with an optional `][description]`.
        let inner_start = i + 2;
        let close = self.find_double_close(inner_start)?;
        let inner: String = self
            .chars
            .get(inner_start..close)
            .unwrap_or(&[])
            .iter()
            .collect();
        let (target_raw, desc_raw) = match inner.find("][") {
            Some(idx) => (
                inner.get(..idx).unwrap_or(""),
                Some(inner.get(idx + 2..).unwrap_or("")),
            ),
            None => (inner.as_str(), None),
        };
        let target = process_target(target_raw);
        let end = close + 2;
        match desc_raw {
            Some(desc) => Some((
                link(&target, parse_inlines(desc, self.ext, self.notes)),
                end,
            )),
            None => {
                if is_image_target(&target) {
                    Some((image(&target, Vec::new()), end))
                } else {
                    Some((
                        link(&target, vec![Inline::Str(target_raw.to_owned().into())]),
                        end,
                    ))
                }
            }
        }
    }

    /// Finds a `]]` starting at or after `from`.
    fn find_double_close(&self, from: usize) -> Option<usize> {
        let mut j = from;
        while j + 1 < self.chars.len() {
            if self.at(j) == Some(']') && self.at(j + 1) == Some(']') {
                return Some(j);
            }
            j += 1;
        }
        None
    }

    fn scan_footnote(&self, i: usize) -> Option<(Inline, usize)> {
        // `[fn:label]`, `[fn:label:text]`, or `[fn::text]`.
        let close = self.match_bracket(i)?;
        let inner: String = self.chars.get(i + 1..close).unwrap_or(&[]).iter().collect();
        let body = inner.strip_prefix("fn:")?;
        let end = close + 1;
        if let Some((label, text)) = body.split_once(':') {
            // Inline definition (named or anonymous).
            let note = vec![Block::Para(parse_inlines(
                text.trim(),
                self.ext,
                self.notes,
            ))];
            let _ = label;
            return Some((Inline::Note(note), end));
        }
        // Bare reference: resolve against the gathered definitions.
        let blocks = self.notes.get(body).cloned().unwrap_or_default();
        Some((Inline::Note(blocks), end))
    }

    fn scan_citation(&self, i: usize) -> Option<(Inline, usize)> {
        let close = self.match_bracket(i)?;
        let inner: String = self.chars.get(i + 1..close).unwrap_or(&[]).iter().collect();
        let raw: String = self.chars.get(i..close + 1).unwrap_or(&[]).iter().collect();
        let rest = inner.strip_prefix("cite")?;
        let (style, payload) = match rest.strip_prefix('/') {
            Some(after) => {
                let (sty, pay) = after.split_once(':')?;
                (Some(sty), pay)
            }
            None => (None, rest.strip_prefix(':')?),
        };
        let citations = parse_citation_items(payload, style, self.ext, self.notes)?;
        Some((Inline::Cite(citations, plain_words(&raw)), close + 1))
    }

    fn match_bracket(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut j = open;
        while let Some(c) = self.at(j) {
            match c {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
            j += 1;
        }
        None
    }

    // -- Angle brackets: targets and autolinks -------------------------------------------------

    fn scan_angle(&self, i: usize) -> Option<(Inline, usize)> {
        if self.at(i + 1) == Some('<') {
            // Target `<<name>>`.
            let name_start = i + 2;
            let mut j = name_start;
            while matches!(self.at(j), Some(c) if c != '<' && c != '>' && c != '\n') {
                j += 1;
            }
            if j > name_start && self.at(j) == Some('>') && self.at(j + 1) == Some('>') {
                let name: String = self
                    .chars
                    .get(name_start..j)
                    .unwrap_or(&[])
                    .iter()
                    .collect();
                let attr = Attr {
                    id: name.into(),
                    ..Attr::default()
                };
                // A target absorbs the whitespace that follows it.
                let mut end = j + 2;
                while matches!(self.at(end), Some(' ' | '\t')) {
                    end += 1;
                }
                return Some((Inline::Span(Box::new(attr), Vec::new()), end));
            }
            return None;
        }
        // Autolink `<uri>`.
        let mut j = i + 1;
        while matches!(self.at(j), Some(c) if c != '>' && c != '\n') {
            j += 1;
        }
        if self.at(j) != Some('>') {
            return None;
        }
        let content: String = self.chars.get(i + 1..j).unwrap_or(&[]).iter().collect();
        if is_uri(&content) {
            return Some((
                link(&content, vec![Inline::Str(content.clone().into())]),
                j + 1,
            ));
        }
        None
    }

    // -- Dollar math ---------------------------------------------------------------------------

    fn scan_math_dollar(&self, i: usize, prev: Option<char>) -> Option<(Inline, usize)> {
        if self.at(i + 1) == Some('$') {
            // Display `$$…$$`.
            let start = i + 2;
            let mut j = start;
            while j + 1 < self.chars.len() {
                if self.at(j) == Some('$') && self.at(j + 1) == Some('$') {
                    let inner: String = self.chars.get(start..j).unwrap_or(&[]).iter().collect();
                    return Some((Inline::Math(MathType::DisplayMath, inner.into()), j + 2));
                }
                j += 1;
            }
            return None;
        }
        // Inline `$…$` with word-boundary and border constraints.
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '$') {
            return None;
        }
        let first = self.at(i + 1)?;
        if first.is_whitespace() || first == '$' {
            return None;
        }
        let mut j = i + 1;
        while let Some(c) = self.at(j) {
            if c == '\n' {
                return None;
            }
            if c == '$'
                && !self.at(j - 1).is_some_and(char::is_whitespace)
                && !self.at(j + 1).is_some_and(char::is_alphanumeric)
            {
                let inner: String = self.chars.get(i + 1..j).unwrap_or(&[]).iter().collect();
                return Some((Inline::Math(MathType::InlineMath, inner.into()), j + 1));
            }
            j += 1;
        }
        None
    }

    // -- Inline export ---------------------------------------------------------------------------

    fn scan_export(&self, i: usize) -> Option<(Inline, usize)> {
        // `@@format:content@@`.
        if self.at(i + 1) != Some('@') {
            return None;
        }
        let fmt_start = i + 2;
        let mut j = fmt_start;
        while matches!(self.at(j), Some(c) if c.is_ascii_alphanumeric() || c == '-') {
            j += 1;
        }
        if self.at(j) != Some(':') || j == fmt_start {
            return None;
        }
        let fmt: String = self.chars.get(fmt_start..j).unwrap_or(&[]).iter().collect();
        let content_start = j + 1;
        let mut k = content_start;
        while k + 1 < self.chars.len() {
            if self.at(k) == Some('@') && self.at(k + 1) == Some('@') {
                let content: String = self
                    .chars
                    .get(content_start..k)
                    .unwrap_or(&[])
                    .iter()
                    .collect();
                return Some((Inline::RawInline(Format(fmt.into()), content.into()), k + 2));
            }
            k += 1;
        }
        None
    }

    // -- Smart quotes --------------------------------------------------------------------------

    fn scan_quote(&self, i: usize, quote: char) -> (Option<Inline>, usize) {
        let (kind, close) = if quote == '"' {
            (QuoteType::DoubleQuote, '"')
        } else {
            (QuoteType::SingleQuote, '\'')
        };
        // The opening quote must be followed immediately by a non-space character.
        if !matches!(self.at(i + 1), Some(c) if !c.is_whitespace()) {
            return (None, i + 1);
        }
        let mut j = i + 1;
        while let Some(c) = self.at(j) {
            if c == close
                && !self.at(j - 1).is_some_and(char::is_whitespace)
                && post_ok(self.at(j + 1))
            {
                let inner = self.chars.get(i + 1..j).unwrap_or(&[]);
                let content = parse_inlines(&collect_str(inner), self.ext, self.notes);
                return (Some(Inline::Quoted(kind, content)), j + 1);
            }
            if c == '\n' {
                break;
            }
            j += 1;
        }
        (None, i + 1)
    }

    // -- Special strings -----------------------------------------------------------------------

    /// Replaces `---`/`--` with em/en dashes and `...` with an ellipsis.
    fn scan_special_string(&self, i: usize) -> Option<(&'static str, usize)> {
        if self.at(i) == Some('.') {
            if self.at(i + 1) == Some('.') && self.at(i + 2) == Some('.') {
                return Some(("\u{2026}", i + 3));
            }
            return None;
        }
        // Dash sequences.
        if self.at(i + 1) == Some('-') {
            if self.at(i + 2) == Some('-') {
                return Some(("\u{2014}", i + 3));
            }
            return Some(("\u{2013}", i + 2));
        }
        None
    }

    // -- Bare autolinks ------------------------------------------------------------------------

    fn scan_bare_url(&self, i: usize) -> Option<(String, usize)> {
        const SCHEMES: [&str; 3] = ["https://", "http://", "ftp://"];
        let scheme = SCHEMES.iter().find(|s| self.matches_str(i, s)).copied()?;
        let mut j = i + scheme.chars().count();
        while matches!(self.at(j), Some(c) if !c.is_whitespace() && !matches!(c, '<' | '>' | '(' | ')' | '[' | ']'))
        {
            j += 1;
        }
        // Trim trailing sentence punctuation.
        while j > i + scheme.chars().count()
            && self
                .at(j - 1)
                .is_some_and(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"'))
        {
            j -= 1;
        }
        let url: String = self.chars.get(i..j).unwrap_or(&[]).iter().collect();
        Some((url, j))
    }

    // -- Low-level matching --------------------------------------------------------------------

    fn matches_at(&self, i: usize, pat: &[char]) -> bool {
        pat.iter()
            .enumerate()
            .all(|(k, &c)| self.at(i + k) == Some(c))
    }

    fn matches_str(&self, i: usize, pat: &str) -> bool {
        pat.chars()
            .enumerate()
            .all(|(k, c)| self.at(i + k) == Some(c))
    }
}

// -- Inline helpers ----------------------------------------------------------------------------

fn collect_str(chars: &[char]) -> String {
    chars.iter().collect()
}

/// Tokenizes text into `Str` words separated by `Space`, used for the literal fallback rendering of a
/// citation.
fn plain_words(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(Inline::Space);
        }
        out.push(Inline::Str(word.to_owned().into()));
    }
    out
}

fn wrap_markup(marker: char, content: Vec<Inline>) -> Inline {
    match marker {
        '*' => Inline::Strong(content),
        '+' => Inline::Strikeout(content),
        // The only other marker routed here is `/`.
        _ => Inline::Emph(content),
    }
}

fn verbatim_code(marker: char, inner: &[char]) -> Inline {
    // A newline inside verbatim collapses to a space.
    let text: String = inner
        .iter()
        .map(|&c| if c == '\n' { ' ' } else { c })
        .collect();
    let attr = if marker == '=' {
        Attr {
            classes: vec!["verbatim".to_owned().into()],
            ..Attr::default()
        }
    } else {
        Attr::default()
    };
    Inline::Code(Box::new(attr), text.into())
}

fn link(target: &str, desc: Vec<Inline>) -> Inline {
    Inline::Link(
        Box::default(),
        desc,
        Box::new(carta_ast::Target {
            url: target.to_owned().into(),
            title: carta_ast::Text::default(),
        }),
    )
}

fn image(target: &str, alt: Vec<Inline>) -> Inline {
    Inline::Image(
        Box::default(),
        alt,
        Box::new(carta_ast::Target {
            url: target.to_owned().into(),
            title: carta_ast::Text::default(),
        }),
    )
}

/// Processes a link target: strips a `file:` prefix and leaves other targets untouched.
fn process_target(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("file:") {
        return rest.to_owned();
    }
    raw.to_owned()
}

fn is_image_target(target: &str) -> bool {
    const EXTS: [&str; 8] = [
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".bmp", ".tiff",
    ];
    let lower = target.to_ascii_lowercase();
    EXTS.iter().any(|e| lower.ends_with(e))
}

/// Whether an angle-bracketed string is a URI: it carries a scheme and no whitespace.
fn is_uri(s: &str) -> bool {
    if s.chars().any(char::is_whitespace) {
        return false;
    }
    if s.contains("://") {
        return true;
    }
    match s.split_once(':') {
        Some((scheme, rest)) => {
            !scheme.is_empty()
                && !rest.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        }
        None => false,
    }
}

fn is_url_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => !c.is_alphanumeric(),
    }
}

fn pre_ok(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '-' | '(' | '{' | '\'' | '"'),
    }
}

fn post_ok(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || matches!(
                    c,
                    '-' | '.' | ',' | ':' | '!' | '?' | ';' | '"' | '\'' | ')' | '}' | '['
                )
        }
    }
}

// -- Citations ---------------------------------------------------------------------------------

fn parse_citation_items(
    payload: &str,
    style: Option<&str>,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
) -> Option<Vec<carta_ast::Citation>> {
    let mode = citation_mode(style);
    let chunks: Vec<&str> = payload.split(';').collect();

    let mut prefix_carry: Option<&str> = None;
    let mut items: Vec<(String, Vec<Inline>, Vec<Inline>)> = Vec::new();
    let mut trailing_suffix: Option<&str> = None;

    for chunk in chunks {
        match chunk.find('@') {
            Some(at) => {
                let prefix = chunk.get(..at).unwrap_or("");
                let after = chunk.get(at + 1..).unwrap_or("");
                let key_end = after
                    .find(|c: char| !is_citation_key_char(c))
                    .unwrap_or(after.len());
                let key = after.get(..key_end).unwrap_or("").to_owned();
                let suffix = after.get(key_end..).unwrap_or("");
                let mut prefix_text = prefix.to_owned();
                if let Some(carry) = prefix_carry.take() {
                    prefix_text = format!("{carry};{prefix}");
                }
                items.push((
                    key,
                    parse_inlines(prefix_text.trim(), ext, notes),
                    parse_inlines(suffix.trim_end(), ext, notes),
                ));
            }
            None => {
                if items.is_empty() {
                    prefix_carry = Some(chunk);
                } else {
                    trailing_suffix = Some(chunk);
                }
            }
        }
    }

    if items.is_empty() {
        return None;
    }

    if let (Some(suffix), Some(last)) = (trailing_suffix, items.last_mut()) {
        let mut combined = last.2.clone();
        if !combined.is_empty() {
            combined.push(Inline::Str(";".to_owned().into()));
        }
        combined.extend(parse_inlines(suffix.trim(), ext, notes));
        last.2 = combined;
    }

    let citations = items
        .into_iter()
        .enumerate()
        .map(|(idx, (id, prefix, suffix))| carta_ast::Citation {
            id: id.into(),
            prefix,
            suffix,
            mode: if idx == 0 {
                mode.clone()
            } else {
                carta_ast::CitationMode::NormalCitation
            },
            note_num: 0,
            hash: 0,
        })
        .collect();
    Some(citations)
}

fn citation_mode(style: Option<&str>) -> carta_ast::CitationMode {
    match style {
        Some("t" | "text" | "author") => carta_ast::CitationMode::AuthorInText,
        Some(s)
            if s.starts_with("na") || s.starts_with("noauthor") || s.starts_with("suppress") =>
        {
            carta_ast::CitationMode::SuppressAuthor
        }
        _ => carta_ast::CitationMode::NormalCitation,
    }
}

fn is_citation_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.' | '/')
}

// -- Prefix helpers ----------------------------------------------------------------------------

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
        s.get(prefix.len()..)
    } else {
        None
    }
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    strip_prefix_ci(s, prefix).is_some()
}

// -- Entity table ------------------------------------------------------------------------------

/// The Unicode replacement for an Org entity name, or `None` when the name is unknown (the caller
/// then passes it through as raw LaTeX).
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn entity(name: &str) -> Option<&'static str> {
    let value = match name {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" => "ϵ",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "omicron" => "ο",
        "pi" => "π",
        "rho" => "ρ",
        "sigma" => "σ",
        "sigmaf" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "varphi" => "ϕ",
        "vartheta" => "ϑ",
        "varpi" => "ϖ",
        "Alpha" => "Α",
        "Beta" => "Β",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Epsilon" => "Ε",
        "Zeta" => "Ζ",
        "Eta" => "Η",
        "Theta" => "Θ",
        "Iota" => "Ι",
        "Kappa" => "Κ",
        "Lambda" => "Λ",
        "Mu" => "Μ",
        "Nu" => "Ν",
        "Xi" => "Ξ",
        "Omicron" => "Ο",
        "Pi" => "Π",
        "Rho" => "Ρ",
        "Sigma" => "Σ",
        "Tau" => "Τ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Chi" => "Χ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "pm" => "±",
        "mp" => "∓",
        "times" => "×",
        "div" => "÷",
        "cdot" => "ċ",
        "deg" => "°",
        "prime" => "′",
        "Prime" => "″",
        "infin" => "∞",
        "nabla" => "∇",
        "partial" => "∂",
        "forall" => "∀",
        "exist" => "∃",
        "empty" => "∅",
        "isin" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "sum" => "∑",
        "prod" => "∏",
        "minus" => "−",
        "lowast" => "∗",
        "radic" => "√",
        "prop" => "∝",
        "ang" => "∠",
        "or" => "∨",
        "cap" => "∩",
        "cup" => "∪",
        "int" => "∫",
        "there4" => "∴",
        "sim" => "∼",
        "cong" => "≅",
        "asymp" => "≈",
        "ne" => "≠",
        "equiv" => "≡",
        "le" => "≤",
        "ge" => "≥",
        "sub" => "⊂",
        "sup" => "⊃",
        "sube" => "⊆",
        "supe" => "⊇",
        "oplus" => "⊕",
        "otimes" => "⊗",
        "perp" => "⊥",
        "sdot" => "⋅",
        "larr" => "←",
        "rarr" => "→",
        "uarr" => "↑",
        "darr" => "↓",
        "harr" => "↔",
        "lArr" => "⇐",
        "rArr" => "⇒",
        "uArr" => "⇑",
        "dArr" => "⇓",
        "hArr" => "⇔",
        "Leftarrow" => "⇐",
        "Rightarrow" => "⇒",
        "Leftrightarrow" => "⇔",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "euro" => "€",
        "cent" => "¢",
        "pound" => "£",
        "yen" => "¥",
        "sect" => "§",
        "para" => "¶",
        "middot" => "·",
        "hellip" => "…",
        "dots" => "…",
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "ndash" => "–",
        "mdash" => "—",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "laquo" => "«",
        "raquo" => "»",
        "nbsp" => "\u{a0}",
        "shy" => "\u{ad}",
        "aacute" => "á",
        "eacute" => "é",
        "iacute" => "í",
        "oacute" => "ó",
        "uacute" => "ú",
        "auml" => "ä",
        "euml" => "ë",
        "iuml" => "ï",
        "ouml" => "ö",
        "uuml" => "ü",
        "ntilde" => "ñ",
        "ccedil" => "ç",
        "szlig" => "ß",
        "dagger" => "†",
        "Dagger" => "‡",
        "bull" => "•",
        "permil" => "‰",
        "frac12" => "½",
        "frac14" => "¼",
        "frac34" => "¾",
        "sup2" => "²",
        "sup3" => "³",
        "plusmn" => "±",
        _ => return None,
    };
    Some(value)
}

#[cfg(test)]
mod tests {
    // Test code: indexing into a block/inline vector produced from a known fixture is the idiomatic
    // assertion, and a wrong index panics the test rather than corrupting shipped output.
    #![allow(clippy::indexing_slicing)]
    use super::*;

    fn doc(input: &str) -> Document {
        let mut options = ReaderOptions::default();
        options.extensions = Extensions::from_list(&[
            Extension::AutoIdentifiers,
            Extension::Citations,
            Extension::TaskLists,
        ]);
        OrgReader.read(input, &options).unwrap()
    }

    fn blocks(input: &str) -> Vec<Block> {
        doc(input).blocks
    }

    #[test]
    fn paragraph_with_emphasis() {
        let b = blocks("Hello *world* /italic/ =verb= ~code~ +strike+.");
        assert_eq!(b.len(), 1);
        match &b[0] {
            Block::Para(inlines) => {
                assert!(inlines.contains(&Inline::Strong(vec![Inline::Str("world".into())])));
                assert!(inlines.contains(&Inline::Emph(vec![Inline::Str("italic".into())])));
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn headline_levels_and_ids() {
        let b = blocks("* First\n** Second");
        match &b[0] {
            Block::Header(1, attr, _) => assert_eq!(attr.id, "first"),
            other => panic!("expected header, got {other:?}"),
        }
        match &b[1] {
            Block::Header(2, attr, _) => assert_eq!(attr.id, "second"),
            other => panic!("expected header, got {other:?}"),
        }
    }

    #[test]
    fn todo_keyword_and_tags() {
        let b = blocks("* TODO Task :work:");
        match &b[0] {
            Block::Header(1, attr, inlines) => {
                assert_eq!(attr.id, "task");
                assert!(
                    matches!(inlines.first(), Some(Inline::Span(a, _)) if a.classes == ["todo", "TODO"])
                );
            }
            other => panic!("expected header, got {other:?}"),
        }
    }

    #[test]
    fn src_block_becomes_code_block() {
        let b = blocks("#+BEGIN_SRC python\nprint(1)\n#+END_SRC");
        match &b[0] {
            Block::CodeBlock(attr, text) => {
                assert_eq!(attr.classes, ["python"]);
                assert_eq!(text, "print(1)\n");
            }
            other => panic!("expected code block, got {other:?}"),
        }
    }

    #[test]
    fn bullet_and_ordered_lists() {
        assert!(
            matches!(blocks("- a\n- b").first(), Some(Block::BulletList(items)) if items.len() == 2)
        );
        assert!(matches!(
            blocks("1. a\n2. b").first(),
            Some(Block::OrderedList(..))
        ));
    }

    #[test]
    fn definition_list() {
        match blocks("- term :: definition").first() {
            Some(Block::DefinitionList(entries)) => assert_eq!(entries.len(), 1),
            other => panic!("expected definition list, got {other:?}"),
        }
    }

    #[test]
    fn link_and_image() {
        let b = blocks("[[https://example.com][label]] [[./x.png]]");
        match &b[0] {
            Block::Para(inlines) => {
                assert!(inlines.iter().any(|i| matches!(i, Inline::Link(..))));
                assert!(inlines.iter().any(|i| matches!(i, Inline::Image(..))));
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn footnote_reference_resolves() {
        let b = blocks("Text[fn:1] more.\n\n[fn:1] The note.");
        match &b[0] {
            Block::Para(inlines) => {
                assert!(inlines.iter().any(|i| matches!(i, Inline::Note(_))));
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn table_with_header() {
        match blocks("| a | b |\n|---+---|\n| 1 | 2 |").first() {
            Some(Block::Table(table)) => {
                assert_eq!(table.head.rows.len(), 1);
                assert_eq!(table.bodies.len(), 1);
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn metadata_title() {
        let d = doc("#+TITLE: My Doc\n\nbody");
        assert!(d.meta.contains_key("title"));
    }

    #[test]
    fn subscript_and_superscript() {
        let b = blocks("H_2O and x^2");
        match &b[0] {
            Block::Para(inlines) => {
                assert!(inlines.iter().any(|i| matches!(i, Inline::Subscript(_))));
                assert!(inlines.iter().any(|i| matches!(i, Inline::Superscript(_))));
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn special_strings_dashes() {
        let b = blocks("em --- en -- dots ...");
        match &b[0] {
            Block::Para(inlines) => {
                let text = carta_ast::to_plain_text(inlines);
                assert!(text.contains('\u{2014}'));
                assert!(text.contains('\u{2013}'));
                assert!(text.contains('\u{2026}'));
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    fn doc_with(input: &str, exts: &[Extension]) -> Document {
        let mut options = ReaderOptions::default();
        options.extensions = Extensions::from_list(exts);
        OrgReader.read(input, &options).unwrap()
    }

    #[test]
    fn smart_quotes_and_apostrophe() {
        let d = doc_with("He said \"hi\" and it's 'fine'.", &[Extension::Smart]);
        let Block::Para(inlines) = &d.blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(inlines.contains(&Inline::Quoted(
            QuoteType::DoubleQuote,
            vec![Inline::Str("hi".into())]
        )));
        assert!(inlines.contains(&Inline::Quoted(
            QuoteType::SingleQuote,
            vec![Inline::Str("fine".into())]
        )));
        assert!(inlines.contains(&Inline::Str("it\u{2019}s".into())));
    }

    #[test]
    fn quotes_literal_without_smart() {
        let d = doc_with("say \"hi\".", &[]);
        let Block::Para(inlines) = &d.blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(inlines.iter().all(|i| !matches!(i, Inline::Quoted(..))));
    }

    #[test]
    fn gfm_and_ascii_identifiers() {
        let gfm = doc_with(
            "* Foo Bar 1.2",
            &[Extension::AutoIdentifiers, Extension::GfmAutoIdentifiers],
        );
        assert!(matches!(&gfm.blocks[0], Block::Header(_, a, _) if a.id == "foo-bar-12"));

        let ascii = doc_with(
            "* Café Résumé",
            &[Extension::AutoIdentifiers, Extension::AsciiIdentifiers],
        );
        assert!(matches!(&ascii.blocks[0], Block::Header(_, a, _) if a.id == "cafe-resume"));
    }

    #[test]
    fn checkbox_literal_without_task_lists() {
        let d = doc_with("- [X] item", &[]);
        let Block::BulletList(items) = &d.blocks[0] else {
            panic!("expected bullet list");
        };
        let Block::Plain(inlines) = &items[0][0] else {
            panic!("expected plain");
        };
        assert!(inlines.contains(&Inline::Str("[X]".into())));
    }

    #[test]
    fn entity_replacement() {
        let b = blocks("\\alpha and \\unknownentity");
        match &b[0] {
            Block::Para(inlines) => {
                assert!(carta_ast::to_plain_text(inlines).contains('α'));
                assert!(inlines.iter().any(|i| matches!(i, Inline::RawInline(..))));
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }
}
