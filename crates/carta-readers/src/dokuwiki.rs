//! Reader for the `DokuWiki` markup language.
//!
//! The grammar is line-oriented at the block level and recursive-descent at the inline level. A
//! block is recognised by its first line: a heading (`=` runs), a code or raw passthrough region
//! (`<code>`, `<file>`, `<HTML>`, `<PHP>`), a table (rows opening with `|` or `^`), a list (`*` for
//! bullets, `-` for ordered, indented at least two columns), an indented code block, a thematic
//! break, a blockquote (`>` runs), or, failing all of those, a paragraph. Inline content is scanned
//! left to right with a small pending-text buffer: emphasis (`//`), strong (`**`), underline
//! (`__`), monospace (`''`), the `<sub>`/`<sup>`/`<del>` spans, links (`[[…]]`), media (`{{…}}`),
//! footnotes (`((…))`), bare URLs, and angle-bracket email addresses each form their own node.
//!
//! When the `Smart` extension is enabled, straight quotes fold into curly [`Inline::Quoted`] runs,
//! `--`/`---` fold into en/em dashes, and `...` folds into an ellipsis.

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, ColSpec, ColWidth, Document, Format, Inline,
    ListAttributes, ListNumberDelim, ListNumberStyle, MathType, QuoteType, Row, Table, TableBody,
    TableFoot, TableHead, Target, to_plain_text,
};
use carta_core::{Extension, Reader, ReaderOptions, Result};
use unicode_normalization::UnicodeNormalization;

use crate::heading_ids::{IdRegistry, IdScheme};
use crate::inline_text::trim_inline_ends;

/// The inline-syntax toggles that the scanner threads through every level of parsing.
#[derive(Debug, Clone, Copy)]
struct Ctx {
    /// Straight quotes, dashes, and ellipses fold into their typographic forms.
    smart: bool,
    /// `$…$` and `$$…$$` spans are read as inline and display math.
    math: bool,
}

/// Parses `DokuWiki` markup into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct DokuwikiReader;

impl Reader for DokuwikiReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let ctx = Ctx {
            smart: options.extensions.contains(Extension::Smart),
            math: options.extensions.contains(Extension::TexMathDollars),
        };
        let text = normalize_newlines(input);
        let lines: Vec<&str> = text.split('\n').collect();
        let mut index = 0;
        let mut blocks = parse_blocks(&lines, &mut index, ctx, 0);
        if options.extensions.contains(Extension::EastAsianLineBreaks) {
            strip_wide_line_breaks(&mut blocks);
        }
        // Identifiers are derived only when `auto_identifiers` is on; the gfm variant and the
        // ASCII fold only select the algorithm, they do not enable derivation on their own.
        if options.extensions.contains(Extension::AutoIdentifiers)
            && let Some(scheme) = IdScheme::select(options.extensions)
        {
            let ascii = options.extensions.contains(Extension::AsciiIdentifiers);
            let mut registry = IdRegistry::default();
            assign_heading_ids(&mut blocks, scheme, ascii, &mut registry);
        }
        Ok(Document {
            blocks,
            ..Default::default()
        })
    }
}

/// The deepest level of inline or block nesting that recursive parsing will follow. Beyond it,
/// would-be delimiters are taken literally, bounding stack use on adversarial input.
const MAX_DEPTH: usize = 32;

/// Replace Windows and classic-Mac line endings with `\n` so the line-oriented scanner sees one
/// newline convention.
fn normalize_newlines(input: &str) -> String {
    input.replace("\r\n", "\n").replace('\r', "\n")
}

/// Whether `chars` from index `i` begins with the characters of `needle`.
fn matches_at(chars: &[char], i: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, ch)| chars.get(i + k) == Some(&ch))
}

/// Count of leading space characters on a line.
fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|&c| c == ' ').count()
}

/// The width of one tab stop, in columns. A tab advances to the next multiple of this width.
const TAB_STOP: usize = 4;

/// Expand every tab in `line` to spaces, advancing to the next tab stop. Each non-tab character
/// counts as one column.
fn expand_tabs(line: &str) -> String {
    let mut out = String::new();
    let mut col = 0;
    for c in line.chars() {
        if c == '\t' {
            let next = (col / TAB_STOP + 1) * TAB_STOP;
            for _ in col..next {
                out.push(' ');
            }
            col = next;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

/// The column at which a line's first non-whitespace character sits, counting a tab as the width to
/// the next tab stop.
fn leading_columns(line: &str) -> usize {
    let mut col = 0;
    for c in line.chars() {
        match c {
            '\t' => col = (col / TAB_STOP + 1) * TAB_STOP,
            ' ' => col += 1,
            _ => break,
        }
    }
    col
}

// ===================================================================================================
// Block level
// ===================================================================================================

/// Parse a run of lines into blocks, advancing `index` past the consumed lines.
fn parse_blocks(lines: &[&str], index: &mut usize, ctx: Ctx, depth: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    while *index < lines.len() {
        let line = lines.get(*index).copied().unwrap_or("");
        if line.trim().is_empty() {
            *index += 1;
            continue;
        }
        if let Some((level, title, trailing)) = header_split(line) {
            blocks.push(Block::Header(
                level,
                Attr::default(),
                inline_content(&title, ctx),
            ));
            *index += 1;
            // Content after the closing run is re-parsed as a fresh block of its own.
            if !trailing.trim().is_empty() && depth < MAX_DEPTH {
                let tail = [trailing.as_str()];
                let mut tail_index = 0;
                blocks.append(&mut parse_blocks(&tail, &mut tail_index, ctx, depth + 1));
            }
            continue;
        }
        if let Some(block) = parse_code_or_raw(lines, index) {
            blocks.push(block);
            continue;
        }
        if is_table_line(line) {
            blocks.push(parse_table(lines, index, ctx));
            continue;
        }
        if list_marker(line).is_some() {
            blocks.push(parse_list(lines, index, ctx, depth));
            continue;
        }
        if is_indented_code(line) {
            blocks.push(parse_indented_code(lines, index));
            continue;
        }
        if is_thematic_break(line) {
            blocks.push(Block::HorizontalRule);
            *index += 1;
            continue;
        }
        if quote_depth(line).is_some() {
            blocks.push(parse_quote(lines, index, ctx, depth));
            continue;
        }
        blocks.push(parse_paragraph(lines, index, ctx));
    }
    blocks
}

/// Whether the line, as the next line of an open paragraph, would instead begin a new block and so
/// interrupt the paragraph.
fn interrupts_paragraph(line: &str) -> bool {
    line.trim().is_empty()
        || header_split(line).is_some()
        || is_block_tag(line)
        || is_table_line(line)
        || list_marker(line).is_some()
        || is_indented_code(line)
        || is_thematic_break(line)
        || quote_depth(line).is_some()
}

/// Gather consecutive non-interrupting lines into one paragraph.
fn parse_paragraph(lines: &[&str], index: &mut usize, ctx: Ctx) -> Block {
    let mut buffer = String::new();
    let mut first = true;
    while *index < lines.len() {
        let line = lines.get(*index).copied().unwrap_or("");
        if !first && interrupts_paragraph(line) {
            break;
        }
        if !first {
            buffer.push('\n');
        }
        buffer.push_str(line);
        first = false;
        *index += 1;
    }
    Block::Para(inline_content(buffer.trim(), ctx))
}

/// A heading line split into its level, title text, and any trailing content after the closing run.
/// A heading opens with two to six `=` and carries no leading whitespace; it closes at the first run
/// of at least two `=` that follows the opening run, and the level is six minus one for each opening
/// `=` beyond the first. Whatever follows the closing run is returned verbatim as trailing content.
/// `None` when the line does not open or never closes a heading.
fn header_split(line: &str) -> Option<(i32, String, String)> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let open = chars.iter().take_while(|&&c| c == '=').count();
    if !(2..=6).contains(&open) {
        return None;
    }
    let mut at = open;
    while at < chars.len() {
        if chars.get(at) == Some(&'=') {
            let run = run_length(&chars, at, '=');
            if run >= 2 {
                let title: String = chars.get(open..at).unwrap_or(&[]).iter().collect();
                let trailing: String = chars.get(at + run..).unwrap_or(&[]).iter().collect();
                let level = i32::try_from(7 - open).unwrap_or(1);
                return Some((level, title.trim().to_string(), trailing));
            }
            at += run;
        } else {
            at += 1;
        }
    }
    None
}

/// Whether the line, at column zero, opens a code, file, or raw passthrough region.
fn is_block_tag(line: &str) -> bool {
    starts_named_tag(line, "code")
        || starts_named_tag(line, "file")
        || line.starts_with("<HTML>")
        || line.starts_with("<PHP>")
}

/// Whether `line` opens with `<name` followed by either `>` or whitespace (an attribute list).
fn starts_named_tag(line: &str, name: &str) -> bool {
    let Some(rest) = line.strip_prefix('<').and_then(|l| l.strip_prefix(name)) else {
        return false;
    };
    matches!(rest.chars().next(), Some('>')) || rest.starts_with(|c: char| c.is_whitespace())
}

/// Whether the line opens a table row.
fn is_table_line(line: &str) -> bool {
    line.starts_with('|') || line.starts_with('^')
}

/// Whether the line is an indented code line: indented at least two columns and carrying content.
fn is_indented_code(line: &str) -> bool {
    leading_columns(line) >= 2 && !line.trim().is_empty()
}

/// Whether the line is a thematic break: four or more `-` and nothing else. Any other character,
/// including a trailing space, disqualifies it.
fn is_thematic_break(line: &str) -> bool {
    line.len() >= 4 && line.chars().all(|c| c == '-')
}

/// The list marker on a line: its indentation and whether it is ordered (`-`) rather than a bullet
/// (`*`). A marker needs at least two leading spaces and a space after the marker character.
fn list_marker(line: &str) -> Option<(usize, bool)> {
    let indent = leading_spaces(line);
    if indent < 2 {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let marker = chars.get(indent)?;
    let ordered = match marker {
        '*' => false,
        '-' => true,
        _ => return None,
    };
    if chars.get(indent + 1) == Some(&' ') {
        Some((indent, ordered))
    } else {
        None
    }
}

/// The blockquote nesting depth of a line (its run of leading `>`), or `None` when the line is not a
/// quote — a `>` run with no content after it is treated as ordinary text.
fn quote_depth(line: &str) -> Option<usize> {
    if !line.starts_with('>') {
        return None;
    }
    let depth = line.chars().take_while(|&c| c == '>').count();
    let rest = line.get(depth..).unwrap_or("");
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    if rest.is_empty() { None } else { Some(depth) }
}

/// The kind of region a block-level passthrough tag opens.
enum RawKind {
    /// `<code …>` or `<file …>`: a code block whose first attribute word, when not `-`, is a class.
    Code,
    /// `<HTML>`: an HTML raw block.
    Html,
    /// `<PHP>`: a PHP snippet, wrapped as an HTML raw block.
    Php,
}

/// Parse a code, file, or raw passthrough region beginning at the current line. The opening tag may
/// carry content on its own line after `>`, and the region runs to its closing tag, possibly several
/// lines below.
fn parse_code_or_raw(lines: &[&str], index: &mut usize) -> Option<Block> {
    let line = lines.get(*index).copied().unwrap_or("");
    let (kind, close) = if starts_named_tag(line, "code") || starts_named_tag(line, "file") {
        (RawKind::Code, "</code>")
    } else if line.starts_with("<HTML>") {
        (RawKind::Html, "</HTML>")
    } else if line.starts_with("<PHP>") {
        (RawKind::Php, "</PHP>")
    } else {
        return None;
    };
    let close = if matches!(kind, RawKind::Code) && starts_named_tag(line, "file") {
        "</file>"
    } else {
        close
    };

    let joined: String = lines.get(*index..).unwrap_or(&[]).join("\n");
    let chars: Vec<char> = joined.chars().collect();
    let open_end = chars.iter().position(|&c| c == '>')?;
    let attr_text: String = chars.get(1..open_end).unwrap_or(&[]).iter().collect();

    let content_start = open_end + 1;
    let (content_chars, after) = match find_subsequence(&chars, content_start, close) {
        Some(at) => (
            chars.get(content_start..at).unwrap_or(&[]),
            at + close.chars().count(),
        ),
        None => (chars.get(content_start..).unwrap_or(&[]), chars.len()),
    };
    let mut content: String = content_chars.iter().collect();
    if let Some(stripped) = content.strip_prefix('\n') {
        content = stripped.to_string();
    }

    let consumed = chars
        .get(..after)
        .unwrap_or(&[])
        .iter()
        .filter(|&&c| c == '\n')
        .count();
    *index += consumed + 1;

    Some(match kind {
        RawKind::Code => {
            let class = code_language(&attr_text);
            let attr = Attr {
                classes: class.into_iter().collect(),
                ..Default::default()
            };
            Block::CodeBlock(attr, content)
        }
        RawKind::Html => Block::RawBlock(Format("html".to_string()), content),
        RawKind::Php => Block::RawBlock(Format("html".to_string()), format!("<?php {content} ?>")),
    })
}

/// The language class of a code or file region: its first attribute word, unless that word is `-`
/// (an explicit "no language") or absent.
fn code_language(attr_text: &str) -> Option<String> {
    let mut words = attr_text.split_whitespace();
    let first = words.next();
    // Skip the tag name itself, which the attribute slice may still carry for `file`/`code`.
    match first {
        Some("code" | "file") => {}
        Some(word) if word != "-" => return Some(word.to_string()),
        _ => return None,
    }
    match words.next() {
        Some(word) if word != "-" => Some(word.to_string()),
        _ => None,
    }
}

/// The index just past the first occurrence of `needle` in `chars` at or after `from`.
fn find_subsequence(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let len = needle.chars().count();
    (from..=chars.len().saturating_sub(len)).find(|&i| matches_at(chars, i, needle))
}

/// Parse a run of indented code lines. Tabs are expanded to spaces, then the common two-column indent
/// is stripped from each line.
fn parse_indented_code(lines: &[&str], index: &mut usize) -> Block {
    let mut out = String::new();
    while *index < lines.len() {
        let line = lines.get(*index).copied().unwrap_or("");
        if !is_indented_code(line) {
            break;
        }
        let expanded = expand_tabs(line);
        let body = expanded.get(2..).unwrap_or("");
        out.push_str(body);
        out.push('\n');
        *index += 1;
    }
    Block::CodeBlock(Attr::default(), out)
}

/// Parse a thematically grouped run of list lines into one bullet or ordered list.
fn parse_list(lines: &[&str], index: &mut usize, ctx: Ctx, depth: usize) -> Block {
    let mut items = Vec::new();
    while *index < lines.len() {
        let line = lines.get(*index).copied().unwrap_or("");
        let Some((indent, ordered)) = list_marker(line) else {
            break;
        };
        let text: String = line.chars().skip(indent + 2).collect();
        items.push((indent, ordered, text));
        *index += 1;
    }
    let mut pos = 0;
    let list = build_list(&items, &mut pos, ctx, depth);
    // A marker-type switch or a dedent below the opening indent ends this list; rewind so the
    // remaining marker lines are parsed as a sibling list on the next pass.
    *index -= items.len() - pos;
    list
}

/// Build one list (and its nested sublists) from the collected items, advancing `pos`. A deeper
/// indent opens a child list; the same indent with the other marker ends this list.
fn build_list(items: &[(usize, bool, String)], pos: &mut usize, ctx: Ctx, depth: usize) -> Block {
    let (base_indent, ordered) = items
        .get(*pos)
        .map_or((0, false), |(indent, ordered, _)| (*indent, *ordered));
    let mut entries: Vec<Vec<Block>> = Vec::new();
    while let Some((indent, item_ordered, text)) = items.get(*pos) {
        if *indent < base_indent {
            break;
        }
        if *indent == base_indent {
            if *item_ordered != ordered {
                break;
            }
            let mut blocks = vec![Block::Plain(inline_content(text, ctx))];
            *pos += 1;
            if depth < MAX_DEPTH && items.get(*pos).is_some_and(|(i, _, _)| *i > base_indent) {
                blocks.push(build_list(items, pos, ctx, depth + 1));
            }
            entries.push(blocks);
        } else if depth < MAX_DEPTH {
            let child = build_list(items, pos, ctx, depth + 1);
            match entries.last_mut() {
                Some(last) => last.push(child),
                None => entries.push(vec![child]),
            }
        } else {
            *pos += 1;
        }
    }
    if ordered {
        Block::OrderedList(
            ListAttributes {
                start: 1,
                style: ListNumberStyle::DefaultStyle,
                delim: ListNumberDelim::DefaultDelim,
            },
            entries,
        )
    } else {
        Block::BulletList(entries)
    }
}

/// Parse a run of blockquote lines, nesting by `>` depth.
fn parse_quote(lines: &[&str], index: &mut usize, ctx: Ctx, depth: usize) -> Block {
    let mut items = Vec::new();
    while *index < lines.len() {
        let line = lines.get(*index).copied().unwrap_or("");
        let Some(level) = quote_depth(line) else {
            break;
        };
        let rest = line.get(level..).unwrap_or("");
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        items.push((level, rest.to_string()));
        *index += 1;
    }
    let mut pos = 0;
    Block::BlockQuote(build_quote(&items, &mut pos, 1, ctx, depth))
}

/// Build the blocks of a blockquote at nesting `level`, recursing into deeper runs.
fn build_quote(
    items: &[(usize, String)],
    pos: &mut usize,
    level: usize,
    ctx: Ctx,
    depth: usize,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    while let Some((line_level, _)) = items.get(*pos) {
        if *line_level < level {
            break;
        }
        if *line_level == level {
            let mut inlines = Vec::new();
            while let Some((line_level, text)) = items.get(*pos) {
                if *line_level != level {
                    break;
                }
                if !inlines.is_empty() {
                    inlines.push(Inline::LineBreak);
                }
                inlines.extend(inline_content(text, ctx));
                *pos += 1;
            }
            blocks.push(Block::Plain(inlines));
        } else if depth < MAX_DEPTH {
            blocks.push(Block::BlockQuote(build_quote(
                items,
                pos,
                level + 1,
                ctx,
                depth + 1,
            )));
        } else {
            *pos += 1;
        }
    }
    blocks
}

// ===================================================================================================
// Heading identifiers
// ===================================================================================================

/// Assign a derived identifier to every heading in document order, descending through block
/// containers. The slug is formed from the heading's plain text — folded to ASCII first when `ascii`
/// is set — and made unique within the document by the registry.
fn assign_heading_ids(
    blocks: &mut [Block],
    scheme: IdScheme,
    ascii: bool,
    registry: &mut IdRegistry,
) {
    for block in blocks {
        match block {
            Block::Header(_, attr, inlines) => {
                let text = to_plain_text(inlines);
                let text = if ascii { asciify(&text) } else { text };
                attr.id = registry.assign(scheme, &text);
            }
            Block::BlockQuote(children)
            | Block::Div(_, children)
            | Block::Figure(_, _, children) => {
                assign_heading_ids(children, scheme, ascii, registry);
            }
            Block::BulletList(items) | Block::OrderedList(_, items) => {
                for item in items {
                    assign_heading_ids(item, scheme, ascii, registry);
                }
            }
            _ => {}
        }
    }
}

/// Fold text to ASCII by canonical decomposition, dropping every character that is not ASCII so a
/// letter carrying a diacritic keeps its base letter.
fn asciify(text: &str) -> String {
    text.nfd().filter(char::is_ascii).collect()
}

// ===================================================================================================
// East Asian line breaks
// ===================================================================================================

/// Drop soft line breaks that fall between two wide East Asian characters, where the break carries no
/// visual width. The surrounding text runs are left separate rather than merged.
fn strip_wide_line_breaks(blocks: &mut [Block]) {
    for block in blocks {
        match block {
            Block::Para(inlines) | Block::Plain(inlines) | Block::Header(_, _, inlines) => {
                strip_wide_in_inlines(inlines);
            }
            Block::BlockQuote(children)
            | Block::Div(_, children)
            | Block::Figure(_, _, children) => {
                strip_wide_line_breaks(children);
            }
            Block::BulletList(items) | Block::OrderedList(_, items) => {
                for item in items {
                    strip_wide_line_breaks(item);
                }
            }
            _ => {}
        }
    }
}

/// Drop width-free soft breaks within one inline sequence, recursing into nested inline containers.
fn strip_wide_in_inlines(inlines: &mut Vec<Inline>) {
    for inline in inlines.iter_mut() {
        match inline {
            Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Cite(_, children)
            | Inline::Link(_, children, _)
            | Inline::Image(_, children, _)
            | Inline::Span(_, children) => strip_wide_in_inlines(children),
            Inline::Note(blocks) => strip_wide_line_breaks(blocks),
            _ => {}
        }
    }
    let mut i = 0;
    while i < inlines.len() {
        if matches!(inlines.get(i), Some(Inline::SoftBreak)) {
            let prev_wide = i
                .checked_sub(1)
                .and_then(|p| inlines.get(p))
                .and_then(last_char)
                .is_some_and(is_east_asian_wide);
            let next_wide = inlines
                .get(i + 1)
                .and_then(first_char)
                .is_some_and(is_east_asian_wide);
            if prev_wide && next_wide {
                inlines.remove(i);
                continue;
            }
        }
        i += 1;
    }
}

/// The last character of an inline's textual content, descending into nested containers.
fn last_char(inline: &Inline) -> Option<char> {
    match inline {
        Inline::Str(s) | Inline::Code(_, s) | Inline::Math(_, s) | Inline::RawInline(_, s) => {
            s.chars().last()
        }
        Inline::Emph(children)
        | Inline::Underline(children)
        | Inline::Strong(children)
        | Inline::Strikeout(children)
        | Inline::Superscript(children)
        | Inline::Subscript(children)
        | Inline::SmallCaps(children)
        | Inline::Quoted(_, children)
        | Inline::Cite(_, children)
        | Inline::Link(_, children, _)
        | Inline::Image(_, children, _)
        | Inline::Span(_, children) => children.iter().rev().find_map(last_char),
        _ => None,
    }
}

/// The first character of an inline's textual content, descending into nested containers.
fn first_char(inline: &Inline) -> Option<char> {
    match inline {
        Inline::Str(s) | Inline::Code(_, s) | Inline::Math(_, s) | Inline::RawInline(_, s) => {
            s.chars().next()
        }
        Inline::Emph(children)
        | Inline::Underline(children)
        | Inline::Strong(children)
        | Inline::Strikeout(children)
        | Inline::Superscript(children)
        | Inline::Subscript(children)
        | Inline::SmallCaps(children)
        | Inline::Quoted(_, children)
        | Inline::Cite(_, children)
        | Inline::Link(_, children, _)
        | Inline::Image(_, children, _)
        | Inline::Span(_, children) => children.iter().find_map(first_char),
        _ => None,
    }
}

/// Whether a character occupies a wide cell in East Asian text (Unicode East Asian Width Wide or
/// Fullwidth). Halfwidth and Ambiguous-width characters are excluded.
fn is_east_asian_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F
        | 0x2E80..=0x2EFF
        | 0x2F00..=0x2FDF
        | 0x2FF0..=0x2FFF
        | 0x3000..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B16F
        | 0x1F200..=0x1F2FF
        | 0x20000..=0x2FFFD
        | 0x30000..=0x3FFFD)
}

// ===================================================================================================
// Inline level
// ===================================================================================================

/// Parse a block's inline content: scan it, then drop leading and trailing whitespace.
fn inline_content(text: &str, ctx: Ctx) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    let (mut inlines, _) = scan(&chars, &mut pos, None, ctx, 0);
    trim_inline_ends(&mut inlines);
    inlines
}

/// Scan a slice of characters as inline content with no surrounding-quote context.
fn scan_slice(chars: &[char], ctx: Ctx, depth: usize) -> Vec<Inline> {
    let mut pos = 0;
    let (inlines, _) = scan(chars, &mut pos, None, ctx, depth);
    inlines
}

/// Push the buffered text as a `Str` and clear the buffer.
fn flush(pending: &mut String, out: &mut Vec<Inline>) {
    if !pending.is_empty() {
        out.push(Inline::Str(std::mem::take(pending)));
    }
}

/// Scan characters into inlines from `*pos`. When `end_quote` is set, the scan stops and reports
/// `true` on the matching closing quote; otherwise it runs to the end and reports `false`.
#[allow(clippy::too_many_lines)]
fn scan(
    chars: &[char],
    pos: &mut usize,
    end_quote: Option<char>,
    ctx: Ctx,
    depth: usize,
) -> (Vec<Inline>, bool) {
    let mut out: Vec<Inline> = Vec::new();
    let mut pending = String::new();
    while let Some(&c) = chars.get(*pos) {
        if let Some(quote) = end_quote
            && c == quote
            && can_close_quote(chars, *pos, quote)
        {
            flush(&mut pending, &mut out);
            *pos += 1;
            return (coalesce(out), true);
        }
        if c.is_ascii_alphabetic()
            && boundary_before(chars, *pos)
            && let Some((link, end)) = try_autolink(chars, *pos)
        {
            flush(&mut pending, &mut out);
            out.push(link);
            *pos = end;
            continue;
        }
        match c {
            ' ' | '\t' | '\n' => scan_whitespace_run(chars, pos, &mut pending, &mut out),
            '\\' if chars.get(*pos + 1) == Some(&'\\') => {
                scan_hard_break(chars, pos, &mut pending, &mut out);
            }
            '*' if chars.get(*pos + 1) == Some(&'*') && depth < MAX_DEPTH => {
                handle_delim(
                    chars,
                    pos,
                    '*',
                    ctx,
                    depth,
                    &mut pending,
                    &mut out,
                    Inline::Strong,
                );
            }
            '/' if chars.get(*pos + 1) == Some(&'/') && depth < MAX_DEPTH => {
                handle_delim(
                    chars,
                    pos,
                    '/',
                    ctx,
                    depth,
                    &mut pending,
                    &mut out,
                    Inline::Emph,
                );
            }
            '_' if chars.get(*pos + 1) == Some(&'_') && depth < MAX_DEPTH => {
                handle_delim(
                    chars,
                    pos,
                    '_',
                    ctx,
                    depth,
                    &mut pending,
                    &mut out,
                    Inline::Underline,
                );
            }
            '\'' if chars.get(*pos + 1) == Some(&'\'') => {
                handle_mono_or_quote(chars, pos, ctx, depth, &mut pending, &mut out);
            }
            '\'' | '"' if ctx.smart => {
                handle_quote(chars, pos, c, ctx, depth, &mut pending, &mut out);
            }
            '$' if ctx.math => {
                handle_math(chars, pos, &mut pending, &mut out);
            }
            '-' if ctx.smart => {
                let run = run_length(chars, *pos, '-');
                pending.push_str(&fold_dashes(run));
                *pos += run;
            }
            '.' if ctx.smart => {
                let run = run_length(chars, *pos, '.');
                pending.push_str(&fold_ellipsis(run));
                *pos += run;
            }
            '[' if chars.get(*pos + 1) == Some(&'[') && depth < MAX_DEPTH => {
                handle_construct(chars, pos, c, ctx, depth, &mut pending, &mut out);
            }
            '{' if chars.get(*pos + 1) == Some(&'{') && depth < MAX_DEPTH => {
                handle_construct(chars, pos, c, ctx, depth, &mut pending, &mut out);
            }
            '(' if chars.get(*pos + 1) == Some(&'(') && depth < MAX_DEPTH => {
                handle_construct(chars, pos, c, ctx, depth, &mut pending, &mut out);
            }
            '%' if chars.get(*pos + 1) == Some(&'%') => {
                handle_construct(chars, pos, c, ctx, depth, &mut pending, &mut out);
            }
            '<' if depth < MAX_DEPTH => {
                handle_construct(chars, pos, c, ctx, depth, &mut pending, &mut out);
            }
            '~' if chars.get(*pos + 1) == Some(&'~') => {
                handle_construct(chars, pos, c, ctx, depth, &mut pending, &mut out);
            }
            other => {
                pending.push(other);
                *pos += 1;
            }
        }
    }
    flush(&mut pending, &mut out);
    (coalesce(out), end_quote.is_none())
}

/// Handle a `''` opener: a monospace run when both delimiters flank non-whitespace content,
/// otherwise — under smart typography — the two quotes fold individually, and otherwise the opener
/// stays literal.
fn handle_mono_or_quote(
    chars: &[char],
    pos: &mut usize,
    ctx: Ctx,
    depth: usize,
    pending: &mut String,
    out: &mut Vec<Inline>,
) {
    if depth < MAX_DEPTH
        && let Some((node, end)) = parse_mono(chars, *pos, ctx, depth)
    {
        flush(pending, out);
        out.push(node);
        *pos = end;
    } else if ctx.smart {
        handle_quote(chars, pos, '\'', ctx, depth, pending, out);
    } else {
        pending.push('\'');
        *pos += 1;
    }
}

/// Consume a run of spaces, tabs, and newlines at `*pos`, emitting a single break: a soft break
/// when the run contains a newline, an ordinary space otherwise.
fn scan_whitespace_run(
    chars: &[char],
    pos: &mut usize,
    pending: &mut String,
    out: &mut Vec<Inline>,
) {
    flush(pending, out);
    let mut has_newline = false;
    while let Some(&w) = chars.get(*pos) {
        match w {
            '\n' => {
                has_newline = true;
                *pos += 1;
            }
            ' ' | '\t' => *pos += 1,
            _ => break,
        }
    }
    out.push(if has_newline {
        Inline::SoftBreak
    } else {
        Inline::Space
    });
}

/// Handle a `\\` sequence at `*pos`: a hard line break when followed by whitespace or the line end,
/// and two literal backslashes otherwise.
fn scan_hard_break(chars: &[char], pos: &mut usize, pending: &mut String, out: &mut Vec<Inline>) {
    let after = chars.get(*pos + 2);
    if after.is_none_or(|c| c.is_whitespace()) {
        flush(pending, out);
        out.push(Inline::LineBreak);
        *pos += 2;
        if after.is_some() {
            *pos += 1;
        }
    } else {
        pending.push('\\');
        pending.push('\\');
        *pos += 2;
    }
}

/// Try to parse the inline construct introduced by `c` at `pos`: a link (`[[`), media (`{{`), a
/// footnote (`((`), a verbatim span (`%%`), an angle-bracket construct (`<`), or a dropped macro
/// (`~~`). Returns the produced nodes and the index past the construct.
fn scan_construct(
    chars: &[char],
    pos: usize,
    c: char,
    ctx: Ctx,
    depth: usize,
) -> Option<(Vec<Inline>, usize)> {
    match c {
        '[' => parse_link(chars, pos).map(|(node, end)| (vec![node], end)),
        '{' => parse_media(chars, pos).map(|(node, end)| (vec![node], end)),
        '(' => parse_footnote(chars, pos, ctx, depth).map(|(node, end)| (vec![node], end)),
        '%' => parse_nowiki_pct(chars, pos),
        '<' => parse_angle(chars, pos, ctx, depth),
        '~' => parse_macro(chars, pos).map(|end| (Vec::new(), end)),
        _ => None,
    }
}

/// Dispatch an inline construct opener at `*pos`: on a successful parse the produced nodes are
/// appended and `*pos` advances past the construct; otherwise the opener is buffered literally and
/// `*pos` advances one character.
#[allow(clippy::too_many_arguments)]
fn handle_construct(
    chars: &[char],
    pos: &mut usize,
    c: char,
    ctx: Ctx,
    depth: usize,
    pending: &mut String,
    out: &mut Vec<Inline>,
) {
    if let Some((mut nodes, end)) = scan_construct(chars, *pos, c, ctx, depth) {
        flush(pending, out);
        out.append(&mut nodes);
        *pos = end;
    } else {
        pending.push(c);
        *pos += 1;
    }
}

/// Wrap a generic two-character emphasis run, or, when no valid closer exists, emit the opener
/// literally.
#[allow(clippy::too_many_arguments)]
fn handle_delim(
    chars: &[char],
    pos: &mut usize,
    delim: char,
    ctx: Ctx,
    depth: usize,
    pending: &mut String,
    out: &mut Vec<Inline>,
    wrap: fn(Vec<Inline>) -> Inline,
) {
    if let Some((inner, end)) = delim_span(chars, *pos, delim, ctx, depth) {
        flush(pending, out);
        out.push(wrap(inner));
        *pos = end;
    } else {
        pending.push(delim);
        pending.push(delim);
        *pos += 2;
    }
}

/// The contents and end index of a `delim`-delimited emphasis run starting at `start`, or `None`
/// when the run does not open (whitespace right after the marker) or has no valid closer (a marker
/// with non-whitespace before it).
fn delim_span(
    chars: &[char],
    begin: usize,
    delim: char,
    ctx: Ctx,
    depth: usize,
) -> Option<(Vec<Inline>, usize)> {
    if chars.get(begin + 2).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    let mut j = begin + 2;
    while j < chars.len() {
        if chars.get(j) == Some(&delim)
            && chars.get(j + 1) == Some(&delim)
            && j > begin + 2
            && chars.get(j - 1).is_some_and(|c| !c.is_whitespace())
        {
            let content = chars.get(begin + 2..j).unwrap_or(&[]);
            return Some((scan_slice(content, ctx, depth + 1), j + 2));
        }
        j += 1;
    }
    None
}

/// Try to open a curly-quote run at `*pos`; on a missing closer, leave the opener as the apt quote
/// glyph and let the scan reprocess what follows. An empty run is kept for double quotes but folds to
/// apostrophes for single quotes.
fn handle_quote(
    chars: &[char],
    pos: &mut usize,
    quote: char,
    ctx: Ctx,
    depth: usize,
    pending: &mut String,
    out: &mut Vec<Inline>,
) {
    let begin = *pos;
    if can_open_quote(chars, begin) && depth < MAX_DEPTH {
        *pos = begin + 1;
        let (inner, closed) = scan(chars, pos, Some(quote), ctx, depth + 1);
        if closed && (quote == '"' || !inner.is_empty()) {
            flush(pending, out);
            out.push(Inline::Quoted(quote_type(quote), inner));
            return;
        }
        *pos = begin + 1;
    } else {
        *pos = begin + 1;
    }
    pending.push(quote_glyph(chars, begin, quote));
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

/// Monospace run `''…''`: its interior is parsed and then flattened to plain text. The run forms only
/// when the opener is followed by a non-space, the closer preceded by a non-space, and the interior
/// is non-empty; otherwise the opener is not a monospace marker.
fn parse_mono(chars: &[char], begin: usize, ctx: Ctx, depth: usize) -> Option<(Inline, usize)> {
    if is_ws_opt(chars.get(begin + 2).copied()) {
        return None;
    }
    let close = find_subsequence(chars, begin + 2, "''")?;
    if close <= begin + 2 || is_ws_opt(chars.get(close - 1).copied()) {
        return None;
    }
    let content = chars.get(begin + 2..close).unwrap_or(&[]);
    let inner = scan_slice(content, ctx, depth + 1);
    Some((
        Inline::Code(Attr::default(), to_plain_text(&inner)),
        close + 2,
    ))
}

/// Handle a `$` opener under dollar-math: a `$$…$$` display span when the next character is also `$`,
/// otherwise a `$…$` inline span. A failed attempt emits a single literal `$` and resumes scanning at
/// the following character, so an unmatched dollar is taken as text.
fn handle_math(chars: &[char], pos: &mut usize, pending: &mut String, out: &mut Vec<Inline>) {
    let begin = *pos;
    let parsed = if chars.get(begin + 1) == Some(&'$') {
        parse_display_math(chars, begin)
    } else {
        parse_inline_math(chars, begin)
    };
    if let Some((node, end)) = parsed {
        flush(pending, out);
        out.push(node);
        *pos = end;
    } else {
        pending.push('$');
        *pos = begin + 1;
    }
}

/// A `$$…$$` display-math span: its interior is taken verbatim. `None` when the span has no closer or
/// encloses nothing.
fn parse_display_math(chars: &[char], begin: usize) -> Option<(Inline, usize)> {
    let close = find_subsequence(chars, begin + 2, "$$")?;
    if close <= begin + 2 {
        return None;
    }
    let content: String = chars.get(begin + 2..close).unwrap_or(&[]).iter().collect();
    Some((Inline::Math(MathType::DisplayMath, content), close + 2))
}

/// A `$…$` inline-math span: the opener must be followed by a non-space, the closer preceded by a
/// non-space and not followed by a digit. Its interior is taken verbatim.
fn parse_inline_math(chars: &[char], begin: usize) -> Option<(Inline, usize)> {
    if is_ws_opt(chars.get(begin + 1).copied()) {
        return None;
    }
    let mut j = begin + 1;
    while j < chars.len() {
        if chars.get(j) == Some(&'$')
            && j > begin + 1
            && chars.get(j - 1).is_some_and(|c| !c.is_whitespace())
            && !chars.get(j + 1).is_some_and(char::is_ascii_digit)
        {
            let content: String = chars.get(begin + 1..j).unwrap_or(&[]).iter().collect();
            return Some((Inline::Math(MathType::InlineMath, content), j + 1));
        }
        j += 1;
    }
    None
}

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

// --- flanking ---

/// The character before `pos`, if any.
fn before_char(chars: &[char], pos: usize) -> Option<char> {
    pos.checked_sub(1).and_then(|p| chars.get(p)).copied()
}

/// Whether an optional character is whitespace, treating a missing character (a boundary) as
/// whitespace.
fn is_ws_opt(opt: Option<char>) -> bool {
    opt.is_none_or(char::is_whitespace)
}

/// Whether an optional character is punctuation, treating a missing character as not punctuation.
fn is_punct_opt(opt: Option<char>) -> bool {
    opt.is_some_and(is_punct)
}

/// Whether a character counts as punctuation for flanking: ASCII punctuation, or any other
/// non-alphanumeric, non-whitespace character.
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace())
}

/// Whether the single character at `pos` is left-flanking (it leans against following content).
fn left_flanking(chars: &[char], pos: usize) -> bool {
    let before = before_char(chars, pos);
    let after = chars.get(pos + 1).copied();
    !is_ws_opt(after) && (!is_punct_opt(after) || is_ws_opt(before) || is_punct_opt(before))
}

/// Whether the single character at `pos` is right-flanking (it leans against preceding content).
fn right_flanking(chars: &[char], pos: usize) -> bool {
    let before = before_char(chars, pos);
    let after = chars.get(pos + 1).copied();
    !is_ws_opt(before) && (!is_punct_opt(before) || is_ws_opt(after) || is_punct_opt(after))
}

/// Whether a straight quote at `pos` may open a quoted run.
fn can_open_quote(chars: &[char], pos: usize) -> bool {
    left_flanking(chars, pos)
}

/// Whether a straight quote at `pos` may close a quoted run. A single quote may not close against a
/// following alphanumeric, so a word-internal apostrophe never ends a quotation.
fn can_close_quote(chars: &[char], pos: usize, quote: char) -> bool {
    if !right_flanking(chars, pos) {
        return false;
    }
    if quote == '\'' {
        !chars.get(pos + 1).is_some_and(|c| c.is_alphanumeric())
    } else {
        true
    }
}

/// Whether `pos` sits at a non-alphanumeric boundary (the start of a word for autolink purposes).
fn boundary_before(chars: &[char], pos: usize) -> bool {
    before_char(chars, pos).is_none_or(|c| !c.is_alphanumeric())
}

// --- bare URL autolinking ---

/// Match a bare URL beginning at `pos` (`scheme://…`), returning the link and the end index.
fn try_autolink(chars: &[char], pos: usize) -> Option<(Inline, usize)> {
    let mut k = pos;
    while chars
        .get(k)
        .is_some_and(|&c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        k += 1;
    }
    if !matches_at(chars, k, "://") {
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
    Some((
        Inline::Link(
            Attr::default(),
            vec![Inline::Str(url.clone())],
            Target {
                url,
                title: String::new(),
            },
        ),
        end,
    ))
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

// --- post-processing ---

/// Merge adjacent text runs and collapse adjacent whitespace into a single token (preferring a hard
/// space), so dropped macros and split apostrophes leave no doubled spacing or fragmented words.
fn coalesce(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Str(s) => {
                if let Some(Inline::Str(prev)) = out.last_mut() {
                    prev.push_str(&s);
                } else if !s.is_empty() {
                    out.push(Inline::Str(s));
                }
            }
            Inline::Space | Inline::SoftBreak => match out.last() {
                Some(Inline::Space) => {}
                Some(Inline::SoftBreak) => {
                    if matches!(inline, Inline::Space)
                        && let Some(slot) = out.last_mut()
                    {
                        *slot = Inline::Space;
                    }
                }
                _ => out.push(inline),
            },
            other => out.push(other),
        }
    }
    out
}

/// Split text into `Str` words separated by single whitespace tokens, with no markup interpretation.
fn tokenize_text(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_whitespace() {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word)));
            }
            let token = if c == '\n' {
                Inline::SoftBreak
            } else {
                Inline::Space
            };
            if !matches!(out.last(), Some(Inline::Space | Inline::SoftBreak)) {
                out.push(token);
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word));
    }
    out
}

// --- links and media ---

/// Parse a `[[target|label]]` link, returning the link node and its end index. A bracket pair whose
/// target side (the text before the first `|`) is entirely empty is not a link; the opener stays
/// literal.
fn parse_link(chars: &[char], start: usize) -> Option<(Inline, usize)> {
    let close = find_subsequence(chars, start + 2, "]]")?;
    let inner: String = chars.get(start + 2..close).unwrap_or(&[]).iter().collect();
    let (raw_target, label) = match inner.split_once('|') {
        Some((t, l)) => (t, Some(l.to_string())),
        None => (inner.as_str(), None),
    };
    if raw_target.is_empty() {
        return None;
    }
    let target = raw_target.trim().to_string();
    let (url, display) = classify_link_target(&target);
    let label_inlines = match label {
        Some(text) => tokenize_text(text.trim()),
        None => vec![Inline::Str(display)],
    };
    Some((
        Inline::Link(
            Attr::default(),
            label_inlines,
            Target {
                url,
                title: String::new(),
            },
        ),
        close + 2,
    ))
}

/// Resolve a link target to its destination URL and auto-display text.
fn classify_link_target(target: &str) -> (String, String) {
    if target.starts_with("\\\\") || is_external(target) {
        (target.to_string(), target.to_string())
    } else if let Some((prefix, rest)) = target.split_once('>') {
        (interwiki_url(prefix, rest), rest.to_string())
    } else {
        (resolve_id(target), display_id(target))
    }
}

/// Parse a `{{image?query|caption}}` media reference into an image, or, when the query opts out of
/// embedding, a link.
fn parse_media(chars: &[char], start: usize) -> Option<(Inline, usize)> {
    let close = find_subsequence(chars, start + 2, "}}")?;
    let inner: String = chars.get(start + 2..close).unwrap_or(&[]).iter().collect();
    let end = close + 2;

    let leading_space = inner.starts_with(char::is_whitespace);
    let (spec, caption) = match inner.split_once('|') {
        Some((s, c)) => (s, Some(c)),
        None => (inner.as_str(), None),
    };
    // A brace pair whose source side (before the first `|`) is empty is not a media reference.
    if spec.is_empty() {
        return None;
    }
    let trailing_space = spec.ends_with(char::is_whitespace);
    let mut classes = Vec::new();
    if let Some(class) = media_align(leading_space, trailing_space) {
        classes.push(class.to_string());
    }

    let spec = spec.trim();
    let (id, query) = match spec.split_once('?') {
        Some((i, q)) => (i, Some(q)),
        None => (spec, None),
    };
    let url = if is_external(id) {
        id.to_string()
    } else {
        resolve_id(id)
    };
    let alt = match caption {
        Some(text) => tokenize_text(text.trim()),
        None if is_external(id) => vec![Inline::Str(id.to_string())],
        None => vec![Inline::Str(display_id(id))],
    };
    let target = Target {
        url,
        title: String::new(),
    };

    let node = match query {
        Some(q) if q.contains("linkonly") => Inline::Link(
            Attr {
                classes,
                ..Default::default()
            },
            alt,
            target,
        ),
        Some(q) => {
            let (width, height) = parse_size(q);
            let mut attributes = Vec::new();
            if let Some(w) = width {
                attributes.push(("width".to_string(), w));
            }
            if let Some(h) = height {
                attributes.push(("height".to_string(), h));
            }
            attributes.push(("query".to_string(), format!("?{q}")));
            Inline::Image(
                Attr {
                    classes,
                    attributes,
                    ..Default::default()
                },
                alt,
                target,
            )
        }
        None => Inline::Image(
            Attr {
                classes,
                ..Default::default()
            },
            alt,
            target,
        ),
    };
    Some((node, end))
}

/// The alignment class for a media reference, from whether its braces carry interior padding.
fn media_align(leading: bool, trailing: bool) -> Option<&'static str> {
    match (leading, trailing) {
        (true, true) => Some("align-center"),
        (false, true) => Some("align-left"),
        (true, false) => Some("align-right"),
        (false, false) => None,
    }
}

/// Parse the leading `width` and optional `xheight` of a media query into pixel strings.
fn parse_size(query: &str) -> (Option<String>, Option<String>) {
    let chars: Vec<char> = query.chars().collect();
    let mut i = 0;
    let mut width = String::new();
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_digit() {
            width.push(c);
            i += 1;
        } else {
            break;
        }
    }
    if width.is_empty() {
        return (None, None);
    }
    let mut height = String::new();
    if matches!(chars.get(i), Some('x' | 'X')) {
        i += 1;
        while let Some(&c) = chars.get(i) {
            if c.is_ascii_digit() {
                height.push(c);
                i += 1;
            } else {
                break;
            }
        }
    }
    let height = if height.is_empty() {
        None
    } else {
        Some(height)
    };
    (Some(width), height)
}

/// Whether a target names an external destination: a known scheme followed by `://`.
fn is_external(s: &str) -> bool {
    match s.find("://") {
        Some(idx) => {
            let scheme = s.get(..idx).unwrap_or("");
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
                && SCHEMES.contains(&scheme.to_lowercase().as_str())
        }
        None => false,
    }
}

/// Resolve a page identifier to a site-relative URL. A namespaced id becomes a slash path, rooted
/// unless it is relative (a leading `.`); an id with no namespace is left untouched.
fn resolve_id(id: &str) -> String {
    if !id.contains(':') {
        return id.to_string();
    }
    if let Some(rest) = id.strip_prefix('.') {
        return rest.trim_start_matches('.').replace(':', "/");
    }
    let replaced = id.replace(':', "/");
    if replaced.starts_with('/') {
        replaced
    } else {
        format!("/{replaced}")
    }
}

/// The display text for a bare page identifier: the segment after the last namespace separator.
fn display_id(id: &str) -> String {
    match id.rsplit_once(':') {
        Some((_, last)) => last.to_string(),
        None => id.to_string(),
    }
}

/// Map an interwiki shortcut and its tail to a destination URL.
fn interwiki_url(prefix: &str, rest: &str) -> String {
    match prefix {
        "wp" => format!("https://en.wikipedia.org/wiki/{rest}"),
        "wpfr" => format!("https://fr.wikipedia.org/wiki/{rest}"),
        "wpde" => format!("https://de.wikipedia.org/wiki/{rest}"),
        "wpes" => format!("https://es.wikipedia.org/wiki/{rest}"),
        "wpjp" => format!("https://jp.wikipedia.org/wiki/{rest}"),
        "wppl" => format!("https://pl.wikipedia.org/wiki/{rest}"),
        "doku" => format!("https://www.dokuwiki.org/{rest}"),
        "phpfn" => format!("https://secure.php.net/{rest}"),
        "callto" => format!("callto://{rest}"),
        other => format!("{other}>{rest}"),
    }
}

// --- footnotes, nowiki, angle tags, macros ---

/// Parse a `((…))` footnote into a note holding the block content of its body. A body that is empty
/// or only whitespace is not a footnote, so the opener stays literal.
fn parse_footnote(chars: &[char], begin: usize, ctx: Ctx, depth: usize) -> Option<(Inline, usize)> {
    let close = find_subsequence(chars, begin + 2, "))")?;
    let inner: String = chars.get(begin + 2..close).unwrap_or(&[]).iter().collect();
    if inner.trim().is_empty() {
        return None;
    }
    Some((
        Inline::Note(parse_blocks_str(&inner, ctx, depth + 1)),
        close + 2,
    ))
}

/// Parse a `%%…%%` no-formatting span: its content is taken verbatim as text. Like the emphasis
/// markers, the opener needs a non-whitespace character after it and the closer one before it, so a
/// `%%` adjacent to a space stays literal.
fn parse_nowiki_pct(chars: &[char], begin: usize) -> Option<(Vec<Inline>, usize)> {
    if chars.get(begin + 2).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    let mut j = begin + 2;
    while j < chars.len() {
        if chars.get(j) == Some(&'%')
            && chars.get(j + 1) == Some(&'%')
            && j > begin + 2
            && chars.get(j - 1).is_some_and(|c| !c.is_whitespace())
        {
            let inner: String = chars.get(begin + 2..j).unwrap_or(&[]).iter().collect();
            return Some((tokenize_text(&inner), j + 2));
        }
        j += 1;
    }
    None
}

/// Parse an angle-bracket inline construct: the markup spans, a verbatim span, raw HTML/PHP, or an
/// email address.
fn parse_angle(
    chars: &[char],
    begin: usize,
    ctx: Ctx,
    depth: usize,
) -> Option<(Vec<Inline>, usize)> {
    if let Some((inner, end)) = tag_region(chars, begin, "<sub>", "</sub>") {
        return Some((
            vec![Inline::Subscript(scan_slice(&inner, ctx, depth + 1))],
            end,
        ));
    }
    if let Some((inner, end)) = tag_region(chars, begin, "<sup>", "</sup>") {
        return Some((
            vec![Inline::Superscript(scan_slice(&inner, ctx, depth + 1))],
            end,
        ));
    }
    if let Some((inner, end)) = tag_region(chars, begin, "<del>", "</del>") {
        return Some((
            vec![Inline::Strikeout(scan_slice(&inner, ctx, depth + 1))],
            end,
        ));
    }
    if let Some((inner, end)) = tag_region(chars, begin, "<nowiki>", "</nowiki>") {
        let text: String = inner.iter().collect();
        return Some((tokenize_text(&text), end));
    }
    if let Some((inner, end)) = tag_region(chars, begin, "<html>", "</html>") {
        let text: String = inner.iter().collect();
        return Some((
            vec![Inline::RawInline(Format("html".to_string()), text)],
            end,
        ));
    }
    if let Some((inner, end)) = tag_region(chars, begin, "<php>", "</php>") {
        let text: String = inner.iter().collect();
        return Some((
            vec![Inline::RawInline(
                Format("html".to_string()),
                format!("<?php {text} ?>"),
            )],
            end,
        ));
    }
    angle_email(chars, begin).map(|(node, end)| (vec![node], end))
}

/// The interior characters and end index of an `open…close` tag region starting at `start`.
fn tag_region(chars: &[char], start: usize, open: &str, close: &str) -> Option<(Vec<char>, usize)> {
    if !matches_at(chars, start, open) {
        return None;
    }
    let content_start = start + open.chars().count();
    let close_at = find_subsequence(chars, content_start, close)?;
    let inner = chars.get(content_start..close_at).unwrap_or(&[]).to_vec();
    Some((inner, close_at + close.chars().count()))
}

/// Parse `<local@domain>` into a `mailto:` link.
fn angle_email(chars: &[char], start: usize) -> Option<(Inline, usize)> {
    if chars.get(start) != Some(&'<') {
        return None;
    }
    let mut j = start + 1;
    while let Some(&c) = chars.get(j) {
        if c == '>' {
            break;
        }
        if c.is_whitespace() || c == '<' {
            return None;
        }
        j += 1;
    }
    if chars.get(j) != Some(&'>') {
        return None;
    }
    let inner: String = chars.get(start + 1..j).unwrap_or(&[]).iter().collect();
    let (local, domain) = inner.split_once('@')?;
    if local.is_empty() || !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.')
    {
        return None;
    }
    let url = format!("mailto:{inner}");
    Some((
        Inline::Link(
            Attr::default(),
            vec![Inline::Str(inner)],
            Target {
                url,
                title: String::new(),
            },
        ),
        j + 1,
    ))
}

/// Recognise a dropped page macro (`~~NOTOC~~`, `~~NOCACHE~~`), returning its end index.
fn parse_macro(chars: &[char], start: usize) -> Option<usize> {
    for token in ["~~NOTOC~~", "~~NOCACHE~~"] {
        if matches_at(chars, start, token) {
            return Some(start + token.chars().count());
        }
    }
    None
}

/// Split text into lines and parse them as blocks.
fn parse_blocks_str(text: &str, ctx: Ctx, depth: usize) -> Vec<Block> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut index = 0;
    parse_blocks(&lines, &mut index, ctx, depth)
}

/// URL schemes recognised for bare-URL autolinking and external-link detection.
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

// ===================================================================================================
// Tables
// ===================================================================================================

/// Parse a run of table rows. The first row sets the column count and per-column alignment, and is
/// the header row when it opens with `^`; all remaining rows form the single body.
fn parse_table(lines: &[&str], index: &mut usize, ctx: Ctx) -> Block {
    let mut rows: Vec<(bool, Vec<String>)> = Vec::new();
    while *index < lines.len() {
        let line = lines.get(*index).copied().unwrap_or("");
        if !is_table_line(line) {
            break;
        }
        rows.push((line.starts_with('^'), split_row(line)));
        *index += 1;
    }

    let first = rows.first();
    let col_count = first.map_or(0, |(_, cells)| cells.len());
    let col_specs: Vec<ColSpec> = first
        .map(|(_, cells)| {
            cells
                .iter()
                .map(|cell| ColSpec {
                    align: cell_align(cell),
                    width: ColWidth::ColWidthDefault,
                })
                .collect()
        })
        .unwrap_or_default();

    let mut head_rows = Vec::new();
    let mut body_rows = Vec::new();
    for (i, (header, cells)) in rows.iter().enumerate() {
        let row = build_row(cells, col_count, ctx);
        if i == 0 && *header {
            head_rows.push(row);
        } else {
            body_rows.push(row);
        }
    }

    Block::Table(Box::new(Table {
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
    }))
}

/// Build a table row, fitting it to `col_count` by truncating extra cells and padding short rows.
fn build_row(cells: &[String], col_count: usize, ctx: Ctx) -> Row {
    let mut out = Vec::with_capacity(col_count);
    for i in 0..col_count {
        let trimmed = cells.get(i).map_or("", |c| c.trim());
        let content = if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![Block::Plain(inline_content(trimmed, ctx))]
        };
        out.push(Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content,
        });
    }
    Row {
        attr: Attr::default(),
        cells: out,
    }
}

/// The column alignment implied by a raw cell's padding: at least two spaces on a side anchors that
/// side, both anchors centre, neither leaves the default.
fn cell_align(raw: &str) -> Alignment {
    let leading = raw.chars().take_while(|&c| c == ' ').count();
    let trailing = raw.chars().rev().take_while(|&c| c == ' ').count();
    match (leading >= 2, trailing >= 2) {
        (true, true) => Alignment::AlignCenter,
        (_, true) => Alignment::AlignLeft,
        (true, _) => Alignment::AlignRight,
        _ => Alignment::AlignDefault,
    }
}

/// Split a table row into its raw cell texts, treating `|` and `^` as delimiters but ignoring those
/// inside links, media, monospace, no-format spans, and verbatim regions.
fn split_row(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut segments: Vec<String> = Vec::new();
    let mut seg = String::new();
    let mut i = 0;
    while i < chars.len() {
        if let Some(skip) = protected_end(&chars, i) {
            seg.extend(chars.get(i..skip).unwrap_or(&[]));
            i = skip;
            continue;
        }
        match chars.get(i) {
            Some('|' | '^') => {
                segments.push(std::mem::take(&mut seg));
                i += 1;
            }
            Some(&c) => {
                seg.push(c);
                i += 1;
            }
            None => break,
        }
    }
    segments.push(seg);
    if !segments.is_empty() {
        segments.remove(0);
    }
    if segments.last().is_some_and(String::is_empty) {
        segments.pop();
    }
    segments
}

/// If a protected span opens at `i`, the index just past its closing delimiter (or the end of the
/// line when it is unterminated).
fn protected_end(chars: &[char], i: usize) -> Option<usize> {
    for (open, close) in [("[[", "]]"), ("{{", "}}"), ("''", "''"), ("%%", "%%")] {
        if matches_at(chars, i, open) {
            let from = i + open.chars().count();
            let end = find_subsequence(chars, from, close)
                .map_or(chars.len(), |p| p + close.chars().count());
            return Some(end);
        }
    }
    if matches_at(chars, i, "<nowiki>") {
        let from = i + "<nowiki>".chars().count();
        let end = find_subsequence(chars, from, "</nowiki>")
            .map_or(chars.len(), |p| p + "</nowiki>".chars().count());
        return Some(end);
    }
    None
}
