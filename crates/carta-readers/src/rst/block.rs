//! Second-pass block-structure parsing.

use super::definitions::parse_target;
use super::directives::{attach_targets, class_div, splice_lone_span};
use super::markers::{
    ADORNMENT_CHARS, Explicit, adornment_char, bullet_content_col, classify_explicit,
    enum_compatible, enumerator, explicit_body, explicit_extent, field_marker, item_well_formed,
    option_marker,
};
use super::tables::is_simple_table_ruler;
use super::{
    PENDING_CLASS, Parser, dedent, escape_uri, indent_of, indirect_referent, is_blank, line_at,
    normalize_name,
};
use crate::heading_ids::IdScheme;
use crate::transliterate::rst_asciify;
use carta_ast::{Attr, Block, Inline, ListAttributes, Text};
use carta_core::Extension;

impl Parser<'_> {
    pub(super) fn blocks(&mut self, lines: &[String]) -> Vec<Block> {
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
            // No destination: internal target pointing at the next block's identifier. A trailing-underscore
            // destination is indirect, kept verbatim so the chain can be followed; others are percent-encoded URLs.
            if matches!(classify_explicit(line), Some(Explicit::Target)) {
                let indent = indent_of(line);
                let end = explicit_extent(lines, i, indent);
                if let Some((name, url)) = parse_target(line.trim_start(), lines, i, end, indent) {
                    if url.trim().is_empty() {
                        self.deferred
                            .insert(normalize_name(&name), format!("#{}", name.trim()));
                        pending_targets.push(name.trim().to_string());
                    } else {
                        let destination = if indirect_referent(&url).is_some() {
                            url
                        } else {
                            escape_uri(&url)
                        };
                        self.deferred.insert(normalize_name(&name), destination);
                    }
                    i = end;
                    continue;
                }
            }
            let before = out.len();
            let scanned_from = i;
            i = self.block_at(lines, i, &mut out);
            // Force progress so a construct that yields nothing cannot stall the scan.
            i = i.max(scanned_from + 1);
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
                && attr.classes.first().map(Text::as_str) == Some(PENDING_CLASS)
            {
                pending_classes = Some(
                    attr.classes
                        .get(1..)
                        .unwrap_or(&[])
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                );
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
            // Overline and underline must match and be at least title length; a mismatch falls
            // through (a single-column simple table opens the same way).
            let title = line_at(lines, i + 1);
            let under = line_at(lines, i + 2);
            let overline_len = line.trim().chars().count();
            if !is_blank(title)
                && adornment_char(title).is_none()
                && adornment_char(under) == Some(c)
                && overline_len == under.trim().chars().count()
                && overline_len >= title.trim().chars().count()
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

        // Line block: `|` then space or EOL, checked after dropping indentation so the character
        // after the pipe decides, not the line's second column.
        if let Some(after_pipe) = line.trim_start().strip_prefix('|')
            && matches!(after_pipe.chars().next(), Some(' ') | None)
        {
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
        let plain = carta_ast::to_plain_text(&inlines);
        let id = match IdScheme::select(self.ext, false) {
            Some(scheme) => {
                let text = if self.ext.contains(Extension::AsciiIdentifiers) {
                    rst_asciify(&plain)
                } else {
                    plain.clone()
                };
                // A title that slugs to nothing takes fallback id `section`, disambiguated like any repeat.
                if matches!(scheme, IdScheme::Gfm) && carta_ast::slug_gfm(&text).is_empty() {
                    self.ids.assign(scheme, "section")
                } else {
                    self.ids.assign(scheme, &text)
                }
            }
            None => String::new(),
        };
        // Section titles are implicit targets resolving to the section id; a later duplicate supersedes.
        if !plain.trim().is_empty() {
            self.deferred
                .insert(normalize_name(&plain), format!("#{id}"));
        }
        Block::Header(
            level,
            Box::new(Attr {
                id: id.into(),
                classes: Vec::new(),
                attributes: Vec::new(),
            }),
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
            // Quoted literal block: every line opens with the same quoting character, kept verbatim.
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
            Block::CodeBlock(Box::default(), text_lines.join("\n").into()),
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
            if is_blank(line) || !line.starts_with(quote) {
                break;
            }
            text_lines.push(line.clone());
            i += 1;
        }
        if text_lines.is_empty() {
            return None;
        }
        Some((
            Block::CodeBlock(Box::default(), text_lines.join("\n").into()),
            i,
        ))
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
                // Extra indentation becomes non-breaking spaces so it survives into the rendered line.
                let leading = rest.chars().take_while(|c| *c == ' ').count();
                let content = format!(
                    "{}{}",
                    "\u{a0}".repeat(leading),
                    rest.trim_start_matches(' ')
                );
                entries.push(content);
                i += 1;
            } else if !entries.is_empty() && indent_of(line) > base {
                // A further-indented line without its own `|` continues the previous line, joined by a space.
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
            // A later run-on item (continuation under-indented) ends the list before it.
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
            let term_inline = vec![Inline::Code(Box::default(), term.into())];
            items.push((term_inline, vec![self.blocks(&body)]));
            i = end;
        }
        out.push(Block::DefinitionList(items));
        i
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
