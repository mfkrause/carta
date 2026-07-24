//! Block-level parsing: paragraphs, headings, code and raw regions, quotes, and heading identifiers.

use carta_ast::{Attr, Block, Format, Inline, to_plain_text};

use crate::heading_ids::{IdRegistry, IdScheme};
use crate::tabs::expand_tabs;
use crate::transliterate::dokuwiki_asciify;

use super::helpers::{TAB_STOP, find_subsequence, leading_columns, matches_at, run_length};
use super::inline::inline_content;
use super::lists::{opens_list, parse_list};
use super::tables::{is_table_line, parse_table};
use super::{Ctx, MAX_DEPTH};

/// Parse a run of lines into blocks, advancing `index` past the consumed lines.
pub(super) fn parse_blocks(
    lines: &[&str],
    index: &mut usize,
    ctx: Ctx,
    depth: usize,
) -> Vec<Block> {
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
                Box::default(),
                inline_content(&title, ctx, depth),
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
            blocks.push(parse_table(lines, index, ctx, depth));
            continue;
        }
        if opens_list(line) {
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
        blocks.append(&mut parse_paragraph(lines, index, ctx, depth));
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
        || opens_list(line)
        || is_indented_code(line)
        || is_thematic_break(line)
        || quote_depth(line).is_some()
}

/// Gather consecutive non-interrupting lines into a paragraph. An embedded `<code>` or `<file>`
/// region, even mid-line, breaks the run into the text before it, the region as its own block, and
/// the text after it.
fn parse_paragraph(lines: &[&str], index: &mut usize, ctx: Ctx, depth: usize) -> Vec<Block> {
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
    split_on_embedded_code(&buffer, ctx, depth)
}

/// Split a paragraph's text on the first embedded `<code>`/`<file>` region that has a closing tag:
/// the text before becomes a paragraph, the region its own code block, and the remainder is split
/// again. Text with no such region is a single paragraph (or nothing, when blank).
fn split_on_embedded_code(text: &str, ctx: Ctx, depth: usize) -> Vec<Block> {
    let chars: Vec<char> = text.chars().collect();
    if depth < MAX_DEPTH
        && let Some((start, block, end)) = find_embedded_code(&chars)
    {
        let mut out = Vec::new();
        let before: String = chars.get(..start).unwrap_or(&[]).iter().collect();
        if !before.trim().is_empty() {
            out.push(Block::Para(inline_content(before.trim(), ctx, depth)));
        }
        out.push(block);
        let after: String = chars.get(end..).unwrap_or(&[]).iter().collect();
        if !after.trim().is_empty() {
            out.append(&mut split_on_embedded_code(&after, ctx, depth + 1));
        }
        out
    } else if text.trim().is_empty() {
        Vec::new()
    } else {
        vec![Block::Para(inline_content(text.trim(), ctx, depth))]
    }
}

/// The first `<code>`/`<file>` region in `chars` that carries a closing tag, as its start index, the
/// parsed code block, and the index just past the closing tag.
fn find_embedded_code(chars: &[char]) -> Option<(usize, Block, usize)> {
    let mut i = 0;
    while i < chars.len() {
        if chars.get(i) == Some(&'<')
            && (named_tag_at(chars, i, "code") || named_tag_at(chars, i, "file"))
            && let Some((block, end)) = parse_raw_region(chars, i)
        {
            return Some((i, block, end));
        }
        i += 1;
    }
    None
}

/// Whether `chars` at `start` opens with `<name` followed by `>` or whitespace.
fn named_tag_at(chars: &[char], start: usize, name: &str) -> bool {
    if chars.get(start) != Some(&'<') {
        return false;
    }
    let after = start + 1 + name.chars().count();
    if !matches_at(chars, start + 1, name) {
        return false;
    }
    matches!(chars.get(after), Some('>')) || chars.get(after).is_some_and(|c| c.is_whitespace())
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

/// Whether the line is an indented code line: indented at least two columns and carrying content.
fn is_indented_code(line: &str) -> bool {
    leading_columns(line) >= 2 && !line.trim().is_empty()
}

/// Whether the line is a thematic break: four or more `-` and nothing else. Any other character,
/// including a trailing space, disqualifies it.
fn is_thematic_break(line: &str) -> bool {
    line.len() >= 4 && line.chars().all(|c| c == '-')
}

/// The blockquote nesting depth of a line (its run of leading `>`), or `None` when the line is not a
/// quote; a `>` run with no content after it is treated as ordinary text.
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
/// lines below. A region that is never closed is not a block here; it stays as ordinary text.
fn parse_code_or_raw(lines: &[&str], index: &mut usize) -> Option<Block> {
    let line = lines.get(*index).copied().unwrap_or("");
    if !is_block_tag(line) {
        return None;
    }
    let joined: String = lines.get(*index..).unwrap_or(&[]).join("\n");
    let chars: Vec<char> = joined.chars().collect();
    let (block, end) = parse_raw_region(&chars, 0)?;
    let consumed = chars
        .get(..end)
        .unwrap_or(&[])
        .iter()
        .filter(|&&c| c == '\n')
        .count();
    *index += consumed + 1;
    Some(block)
}

/// Parse a `<code>`/`<file>`/`<HTML>`/`<PHP>` passthrough region beginning at `start`, returning the
/// block and the index just past its closing tag. `None` when no such opener sits at `start` or the
/// region has no closing tag.
fn parse_raw_region(chars: &[char], start: usize) -> Option<(Block, usize)> {
    let (kind, close) = if named_tag_at(chars, start, "code") {
        (RawKind::Code, "</code>")
    } else if named_tag_at(chars, start, "file") {
        (RawKind::Code, "</file>")
    } else if matches_at(chars, start, "<HTML>") {
        (RawKind::Html, "</HTML>")
    } else if matches_at(chars, start, "<PHP>") {
        (RawKind::Php, "</PHP>")
    } else {
        return None;
    };
    let open_end = (start..chars.len()).find(|&i| chars.get(i) == Some(&'>'))?;
    let attr_text: String = chars
        .get(start + 1..open_end)
        .unwrap_or(&[])
        .iter()
        .collect();
    let content_start = open_end + 1;
    let close_at = find_subsequence(chars, content_start, close)?;
    let mut content: String = chars
        .get(content_start..close_at)
        .unwrap_or(&[])
        .iter()
        .collect();
    if let Some(stripped) = content.strip_prefix('\n') {
        content = stripped.to_string();
    }
    let end = close_at + close.chars().count();
    let block = match kind {
        RawKind::Code => {
            let class = code_language(&attr_text);
            let attr = Attr {
                classes: class.into_iter().map(Into::into).collect(),
                ..Default::default()
            };
            Block::CodeBlock(Box::new(attr), content.into())
        }
        RawKind::Html => Block::RawBlock(Format("html".into()), content.into()),
        RawKind::Php => {
            Block::RawBlock(Format("html".into()), format!("<?php {content} ?>").into())
        }
    };
    Some((block, end))
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

/// Parse a run of indented code lines. Tabs are expanded to spaces, then the common two-column indent
/// is stripped from each line.
fn parse_indented_code(lines: &[&str], index: &mut usize) -> Block {
    let mut out = String::new();
    while *index < lines.len() {
        let line = lines.get(*index).copied().unwrap_or("");
        if !is_indented_code(line) {
            break;
        }
        let expanded = expand_tabs(line, TAB_STOP);
        let body = expanded.get(2..).unwrap_or("");
        out.push_str(body);
        out.push('\n');
        *index += 1;
    }
    Block::CodeBlock(Box::default(), out.into())
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
                inlines.extend(inline_content(text, ctx, depth));
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

/// Assign a derived identifier to every heading in document order, descending through block
/// containers. The slug is formed from the heading's plain text (folded to ASCII first when `ascii`
/// is set) and made unique within the document by the registry.
pub(super) fn assign_heading_ids(
    blocks: &mut [Block],
    scheme: IdScheme,
    ascii: bool,
    registry: &mut IdRegistry,
) {
    for block in blocks {
        match block {
            Block::Header(_, attr, inlines) => {
                let text = to_plain_text(inlines);
                let text = if ascii { dokuwiki_asciify(&text) } else { text };
                attr.id = registry.assign(scheme, &text).into();
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

/// Drop soft line breaks that fall between two wide East Asian characters, where the break carries no
/// visual width. The surrounding text runs are left separate rather than merged.
pub(super) fn strip_wide_line_breaks(blocks: &mut [Block]) {
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

/// Split text into lines and parse them as blocks.
pub(super) fn parse_blocks_str(text: &str, ctx: Ctx, depth: usize) -> Vec<Block> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut index = 0;
    parse_blocks(&lines, &mut index, ctx, depth)
}
