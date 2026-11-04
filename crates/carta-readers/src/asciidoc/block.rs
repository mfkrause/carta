use super::attributes::{
    add_block_attr, block_anchor, block_attributes, block_attributes_at, protect_attribute_names,
};
use super::lists::{definition, definition_item, marker, marker_after_space, marker_in_list};
use super::{AUTO_HEADING, line_comment};
use super::{Attributes, Parser, ParserError, class_attr, text_inlines};
use carta_ast::{Attr, Block, Caption, Inline, MathType};
use carta_core::{Extension, Reader, ReaderOptions};
use std::collections::BTreeSet;
use std::fmt::Write as _;

pub(super) fn physical_blank_line(line: &str) -> bool {
    line.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
}

impl Parser {
    pub(super) fn blocks(&mut self, lines: &[&str]) -> Vec<Block> {
        let section = (self.depth == 0).then(|| {
            if self.attributes.get("doctype").is_some_and(|s| s == "book") {
                -1
            } else {
                0
            }
        });
        self.block_sequence(lines, section, false, '\0')
    }

    pub(super) fn block_sequence(
        &mut self,
        lines: &[&str],
        section: Option<i64>,
        partial: bool,
        paragraph_style: char,
    ) -> Vec<Block> {
        if self.depth >= 128 {
            return vec![];
        }
        self.depth += 1;
        let output = self.scan_blocks(lines, &mut 0, false, section, partial, paragraph_style);
        self.depth -= 1;
        output
    }

    fn container_blocks(&mut self, lines: &[&str]) -> Option<Vec<Block>> {
        if self.error.is_some() {
            return None;
        }
        if self.load_includes && lines.iter().any(|line| line.contains("include::")) {
            let snapshot = self.snapshot();
            self.load_includes = false;
            let _ = self.blocks(lines);
            let error = self.error.take();
            self.restore(snapshot);
            match error {
                Some(ParserError::Syntax(_)) => return None,
                Some(error @ ParserError::Fatal(_)) => {
                    self.error = Some(error);
                    return None;
                }
                None => {}
            }
        }

        let snapshot = self.snapshot();
        let blocks = self.blocks(lines);
        if self.rollback_failed_parse(snapshot) {
            None
        } else {
            Some(blocks)
        }
    }

    #[allow(
        clippy::indexing_slicing,
        reason = "The scanner bounds the opening and paragraph end."
    )]
    fn container_fallback(
        &mut self,
        lines: &[&str],
        opening: usize,
        list_item: bool,
        hard_breaks: bool,
    ) -> (Vec<Inline>, usize) {
        let end = paragraph_end(lines, opening, list_item);
        let source = lines[opening..end].join("\n");
        (self.paragraph_inlines(&source, hard_breaks), end)
    }

    #[allow(
        clippy::too_many_lines,
        clippy::indexing_slicing,
        reason = "Line bounds are checked before block dispatch."
    )]
    pub(super) fn scan_blocks(
        &mut self,
        lines: &[&str],
        cursor: &mut usize,
        list_item: bool,
        section: Option<i64>,
        partial: bool,
        paragraph_style: char,
    ) -> Vec<Block> {
        let source_lines = lines;
        let mut output = Vec::new();
        let mut position = *cursor;
        let mut pending_start = position;
        let mut attributes = Attributes::default();
        let mut title = None;
        // Partial parsing must not advance counters in unused titles.
        let mut pending_titles = Vec::new();
        let mut pending_line = None;
        let mut body_attribute_group = false;
        let mut body_attribute_gap = false;
        let mut pending_block_metadata = false;
        let mut pending_metadata_start = None;
        let mut section_levels = vec![section.unwrap_or(0)];
        let inherited_hard_breaks = self.hard_breaks;
        while position < source_lines.len() {
            self.hard_breaks = inherited_hard_breaks;
            if let Some(line) = self.pending_line.take() {
                pending_line = Some((position, line));
            }
            let effective_override = pending_line
                .as_ref()
                .filter(|(line_position, _)| *line_position == position)
                .map(|(_, line)| line.clone());
            let raw_line = effective_override
                .as_deref()
                .unwrap_or(source_lines[position]);
            let mut effective_lines = effective_override.as_deref().map(|line| {
                let mut effective = source_lines.to_vec();
                effective[position] = line;
                effective
            });
            let raw_lines = effective_lines.as_deref().unwrap_or(source_lines);
            let line = if self.list_context.is_empty() {
                raw_line
            } else {
                raw_line.trim_start_matches(' ')
            };
            let pending_tail = pending_line
                .as_ref()
                .is_some_and(|(line_position, _)| *line_position == position);
            let macro_block = block_macro_line(source_lines, position, line);
            let definition_start = definition_item(line, self.list_context.last().copied());
            let list_item_boundary = list_item
                && macro_block.is_none()
                && ((!definition_start
                    && marker(line).is_some_and(|marker| {
                        self.list_context
                            .iter()
                            .any(|(kind, depth)| marker.kind == *kind && marker.depth <= *depth)
                    }))
                    || marker_after_space(line).is_some()
                    || (marker(line).is_none() && definition(line).is_some() && !definition_start)
                    || raw_line.starts_with(' ')
                        && block_attributes(line).is_some()
                        && block_attributes(raw_line).is_none()
                    || pending_tail && line == "+");
            if list_item && physical_blank_line(line) {
                if pending_block_metadata {
                    body_attribute_gap |= body_attribute_group;
                    position += 1;
                    pending_start = position;
                    continue;
                }
                break;
            }
            if list_item && line == "+" && !pending_tail {
                position += 1;
                pending_start = position;
                continue;
            }
            if list_item_boundary {
                if pending_line
                    .as_ref()
                    .is_some_and(|(line_position, _)| *line_position == position)
                    && let Some((_, line)) = pending_line.take()
                {
                    self.pending_line = Some(line);
                }
                if pending_block_metadata {
                    let metadata_start = pending_metadata_start.unwrap_or(position);
                    if metadata_start == 0
                        && let Some(line) = raw_lines.get(metadata_start)
                    {
                        self.pending_line = Some((*line).to_owned());
                    }
                    position = metadata_start;
                    pending_block_metadata = false;
                } else {
                    position = pending_start;
                }
                break;
            }
            if physical_blank_line(line) {
                body_attribute_gap |= body_attribute_group;
                position += 1;
                pending_start = position;
                continue;
            }
            if line.starts_with("////") && line.chars().all(|character| character == '/') {
                position += 1;
                while position < raw_lines.len() && raw_lines[position] != raw_line {
                    position += 1;
                }
                position += usize::from(position < raw_lines.len());
                // A discarded comment consumes the metadata attached to it.
                attributes = Attributes::default();
                title = None;
                pending_titles.clear();
                pending_block_metadata = false;
                pending_metadata_start = None;
                body_attribute_group = false;
                body_attribute_gap = false;
                pending_start = position;
                continue;
            }
            if line_comment(line) || !list_item && super::blank_line(line) {
                position += 1;
                pending_start = position;
                continue;
            }
            if !(body_attribute_group && body_attribute_gap)
                && self.attribute_line(raw_lines, &mut position)
            {
                body_attribute_group = true;
                pending_start = position;
                continue;
            }

            if let Some(anchor) = block_anchor(raw_line) {
                attributes.named.insert("id".to_owned(), anchor.to_owned());
                pending_metadata_start.get_or_insert(position);
                pending_block_metadata = true;
                position += 1;
                continue;
            }
            if let Some((content, count)) = block_attributes_at(raw_lines, position) {
                attributes.extend(&content);
                pending_metadata_start.get_or_insert(position);
                pending_block_metadata = true;
                position += count;
                continue;
            }
            if raw_line.starts_with('.')
                && !raw_line.starts_with("..")
                && !raw_line.starts_with(". ")
            {
                if partial {
                    pending_titles.push(raw_line[1..].to_owned());
                } else {
                    title = Some(self.inlines(&raw_line[1..]));
                }
                pending_metadata_start.get_or_insert(position);
                pending_block_metadata = true;
                position += 1;
                continue;
            }
            let has_block_attributes = block_attributes(raw_line).is_some();
            let dispatch_line = if !self.list_context.is_empty()
                && raw_line.starts_with(' ')
                && block_attributes(line).is_some()
                && !has_block_attributes
            {
                raw_line
            } else {
                line
            };
            if let Some(effective) = &mut effective_lines {
                effective[position] = dispatch_line;
            } else if dispatch_line != raw_line {
                let mut effective = source_lines.to_vec();
                effective[position] = dispatch_line;
                effective_lines = Some(effective);
            }
            let lines = effective_lines.as_deref().unwrap_or(source_lines);
            let line = lines[position];
            let style = attributes.style().to_owned();
            if partial
                && (style != "discrete"
                    && heading(line)
                        .is_some_and(|(level, _)| section.is_some_and(|minimum| level <= minimum))
                    || has_block_attributes
                        && !matches!(style.as_str(), "literal" | "listing" | "source"))
            {
                position = pending_start;
                break;
            }
            for pending in std::mem::take(&mut pending_titles) {
                title = Some(self.inlines(&pending));
            }
            if style == "comment"
                && !(section.is_some()
                    && (heading(line).is_some()
                        || line.starts_with("= ")
                        || line.starts_with("# ")))
            {
                if let Some(delimiter) = comment_literal_delimiter(line) {
                    position += 1;
                    while position < lines.len()
                        && lines[position].trim_end_matches([' ', '\t']) != delimiter
                    {
                        position += 1;
                    }
                    position += usize::from(position < lines.len());
                } else {
                    while position < lines.len() && !physical_blank_line(lines[position]) {
                        position += 1;
                    }
                }
                attributes = Attributes::default();
                title = None;
                pending_block_metadata = false;
                pending_metadata_start = None;
                body_attribute_group = false;
                body_attribute_gap = false;
                pending_start = position;
                continue;
            }
            let mut is_example = block_fence(lines, position, &self.list_context, &attributes)
                .is_some_and(|(_, kind)| kind == "example");
            self.hard_breaks |= attributes
                .get("options")
                .is_some_and(|options| options.contains("hardbreaks"));
            let mut block;
            if (line.starts_with("= ") || line.starts_with("# "))
                && section.is_some()
                && self.attributes.get("doctype").is_none_or(|s| s != "book")
            {
                self.syntax_error("unexpected document title");
                break;
            }
            let fence_kind =
                block_fence(lines, position, &self.list_context, &attributes).map(|(_, kind)| kind);
            let raw_style = macro_block.is_none()
                && (style == "literal"
                    && !matches!(fence_kind, Some("literal" | "example" | "sidebar"))
                    || matches!(style.as_str(), "source" | "listing")
                        && !matches!(
                            fence_kind,
                            Some("literal" | "example" | "sidebar" | "listing")
                        ));
            let quoted_style = macro_block.is_none()
                && matches!(style.as_str(), "quote" | "verse")
                && !matches!(
                    fence_kind,
                    Some(
                        "example"
                            | "sidebar"
                            | "literal"
                            | "listing"
                            | "markdown"
                            | "open"
                            | "quote"
                    )
                )
                && !(section.is_some() && heading(line).is_some())
                && !(list_item && (marker(line).is_some() || definition(line).is_some()));
            if raw_style {
                let start = position;
                while position < lines.len()
                    && !physical_blank_line(lines[position])
                    && !line_comment(lines[position])
                {
                    position += 1;
                }
                let mut attr = Attr::default();
                if style == "source"
                    && let Some(language) = attributes.positional.get(1)
                {
                    attr.classes.push(language.as_str().into());
                }
                let content = lines[start..position].join("\n");
                block = Block::CodeBlock(
                    Box::new(attr),
                    if style == "literal" {
                        format!("{content}\n")
                    } else {
                        callouts(&content)
                    }
                    .into(),
                );
                position += usize::from(position < lines.len());
            } else if quoted_style {
                let start = position;
                position = paragraph_end(lines, position, list_item);
                let content = lines[start..position].join("\n");
                let quoted = if let Some((kind, text)) = admonition(&content) {
                    self.admonition(kind, text, style == "verse")
                } else {
                    Block::Para(if style == "verse" {
                        self.paragraph_inlines(&content, true)
                    } else {
                        self.inlines(&content)
                    })
                };
                let mut blocks = vec![quoted];
                attribution(&mut blocks, &attributes);
                block = Block::BlockQuote(blocks);
            } else if let Some((original_level, heading)) =
                heading(line).filter(|_| section.is_some() || style == "discrete")
            {
                while section_levels.len() > 1
                    && section_levels
                        .last()
                        .is_some_and(|level| *level >= original_level)
                {
                    section_levels.pop();
                }
                let level = if style == "discrete" {
                    original_level
                } else {
                    section_levels.last().copied().unwrap_or(0) + 1
                };
                if style != "discrete" {
                    section_levels.push(original_level);
                }
                let inlines = self.inlines(heading);
                let mut attr = Attr::default();
                if style != "discrete" {
                    attr.attributes
                        .push((super::SECTION.into(), level.to_string().into()));
                }
                if attributes.get("id").is_none() && style != "discrete" {
                    attr.attributes.push((
                        AUTO_HEADING.into(),
                        super::document::identifier_text(&inlines).into(),
                    ));
                }
                block = Block::Header(level, Box::new(attr), inlines);
                position += 1;
            } else if let Some((delimiter, kind)) =
                block_fence(lines, position, &self.list_context, &attributes)
            {
                let opening = position;
                let table_separator = table_cell_separator(&attributes, delimiter);
                let table_inline_close = table_allows_inline_close(&attributes, delimiter);
                position += 1;
                let start = position;
                let container_end = matches!(kind, "open" | "example" | "sidebar" | "quote")
                    .then(|| container_close(lines, opening, delimiter, kind, 0));
                while position < lines.len()
                    && match container_end {
                        Some(Some(end)) => position < end,
                        _ => !closing_fence_with_table_settings(
                            lines[position],
                            delimiter,
                            kind,
                            table_separator,
                            table_inline_close,
                        ),
                    }
                {
                    position += 1;
                }
                let contents = &lines[start..position];
                let closed = position < lines.len();
                let closing_position = position;
                let markdown_tail = (kind == "markdown" && closed)
                    .then(|| markdown_tail(lines[position], delimiter))
                    .flatten();
                if let Some(tail) = markdown_tail {
                    pending_line = Some((position, tail.to_owned()));
                } else {
                    position += usize::from(closed);
                }
                let content = contents.join("\n");

                let kind = match kind {
                    "pass" if matches!(style.as_str(), "stem" | "asciimath" | "latexmath") => {
                        "math"
                    }
                    "open" | "quote" if style == "verse" => "verse",
                    "open" | "quote" if style == "quote" => "quote",
                    _ => kind,
                };
                block = match kind {
                    "listing" | "markdown" => {
                        let language = if kind == "markdown" {
                            super::trim_space(line.trim_start_matches('`'))
                        } else {
                            attributes.positional.get(1).map_or("", String::as_str)
                        };
                        let mut attr = Attr::default();
                        if !language.is_empty() {
                            attr.classes.push(language.into());
                        }
                        let content = if kind == "listing" {
                            self.include_listing(&content)
                                .unwrap_or_else(|| callouts(&content))
                        } else {
                            callouts(&content)
                        };
                        Block::CodeBlock(
                            Box::new(attr),
                            if closed {
                                content
                            } else {
                                format!("{content}\n")
                            }
                            .into(),
                        )
                    }
                    "literal" => Block::CodeBlock(
                        Box::default(),
                        if closed && contents.is_empty() {
                            String::new().into()
                        } else {
                            format!("{content}\n{}", if closed { "" } else { "\n" }).into()
                        },
                    ),
                    "math" => Block::Para(vec![Inline::Math(
                        MathType::DisplayMath,
                        format!("{content}\n").into(),
                    )]),
                    "pass" => {
                        let mut blocks = Self::html_blocks(&format!("{content}\n"));
                        if let Some(title) = title.take() {
                            let block = if blocks.len() == 1 {
                                blocks.remove(0)
                            } else {
                                Block::Div(Box::default(), blocks)
                            };
                            blocks = vec![add_title(block, title)];
                        }
                        let mut blocks = blocks.into_iter();
                        if let Some(first) = blocks.next() {
                            output.push(add_block_attr(
                                first,
                                attributes.attr(),
                                attributes.has_empty_id(),
                            ));
                            output.extend(blocks);
                        }
                        attributes = Attributes::default();
                        title = None;
                        pending_block_metadata = false;
                        pending_metadata_start = None;
                        pending_start = position;
                        continue;
                    }
                    "table" => {
                        let closing_prefix = if closed {
                            table_closing_prefix(
                                lines[closing_position],
                                table_separator,
                                table_inline_close,
                            )
                        } else {
                            None
                        };
                        if let Some(table) = self.table(
                            contents,
                            &attributes,
                            line,
                            closing_prefix,
                            section.map(|_| section_levels.last().copied().unwrap_or(0)),
                        ) {
                            table
                        } else {
                            position = start;
                            while position < lines.len()
                                && !physical_blank_line(lines[position])
                                && block_attributes_at(lines, position).is_none()
                                && block_attributes(lines[position]).is_none()
                                && !lines[position].starts_with("////")
                            {
                                position += 1;
                            }
                            let source = lines[start - 1..position].join("\n");
                            Block::Para(self.paragraph_inlines(
                                &source,
                                self.hard_breaks || attributes.has("hardbreaks"),
                            ))
                        }
                    }
                    "quote" | "verse" => {
                        if kind == "verse" {
                            let mut blocks: Vec<_> = content
                                .split("\n\n")
                                .filter(|part| !super::trim_space(part).is_empty())
                                .map(|part| Block::Para(self.paragraph_inlines(part, true)))
                                .collect();
                            attribution(&mut blocks, &attributes);
                            Block::BlockQuote(blocks)
                        } else if let Some(mut blocks) = self.container_blocks(contents) {
                            attribution(&mut blocks, &attributes);
                            Block::BlockQuote(blocks)
                        } else if self.error.is_some() {
                            break;
                        } else {
                            let (nodes, end) = self.container_fallback(
                                lines,
                                opening,
                                list_item,
                                self.hard_breaks || attributes.has("hardbreaks"),
                            );
                            position = end;
                            Block::Para(nodes)
                        }
                    }
                    _ => {
                        let class = match kind {
                            "example" => "example",
                            "sidebar" => "sidebar",
                            _ => "",
                        };
                        if let Some(blocks) = self.container_blocks(contents) {
                            Block::Div(
                                Box::new(if class.is_empty() {
                                    Attr::default()
                                } else {
                                    class_attr(class)
                                }),
                                blocks,
                            )
                        } else if self.error.is_some() {
                            break;
                        } else {
                            let (nodes, end) = self.container_fallback(
                                lines,
                                opening,
                                list_item,
                                self.hard_breaks || attributes.has("hardbreaks"),
                            );
                            position = end;
                            is_example = false;
                            Block::Para(nodes)
                        }
                    }
                };
            } else if line == "<<<" {
                block = Block::Div(
                    Box::new(Attr {
                        classes: vec!["page-break".into()],
                        attributes: vec![("wrapper".into(), "1".into())],
                        ..Default::default()
                    }),
                    vec![Block::HorizontalRule],
                );
                position += 1;
            } else if thematic(line) {
                block = Block::HorizontalRule;
                position += 1;
            } else if let Some(macro_block) =
                macro_block.as_ref().filter(|block| block.name == "image")
            {
                let image_attributes = Attributes::parse(&macro_block.attributes);
                attributes.named.extend(image_attributes.named.clone());
                let positional = image_attributes.positional.join(",");
                block = Block::Figure(
                    Box::default(),
                    Box::default(),
                    vec![Block::Plain(vec![Self::image(
                        &macro_block.target,
                        &positional,
                    )])],
                );
                advance_macro(&mut position, macro_block, &mut pending_line);
            } else if let Some(macro_block) =
                macro_block.as_ref().filter(|block| block.name == "toc")
            {
                attributes.extend(&macro_block.attributes);
                block = Block::Div(
                    Box::new(Attr {
                        id: "toc".into(),
                        ..Attr::default()
                    }),
                    vec![],
                );
                advance_macro(&mut position, macro_block, &mut pending_line);
            } else if let Some(macro_block) = macro_block
                .as_ref()
                .filter(|block| matches!(block.name, "audio" | "video"))
            {
                attributes.extend(&macro_block.attributes);
                block = Block::Plain(vec![Inline::Image(
                    Box::default(),
                    vec![Inline::Str(macro_block.target.as_str().into())],
                    Box::new(carta_ast::Target {
                        url: macro_block.target.as_str().into(),
                        title: "".into(),
                    }),
                )]);
                advance_macro(&mut position, macro_block, &mut pending_line);
            } else if let Some(macro_block) =
                macro_block.as_ref().filter(|block| block.name == "include")
            {
                attributes.extend(&macro_block.attributes);
                if self.load_includes {
                    let path = super::normalize_path(&self.directory.as_ref().map_or_else(
                        || std::path::PathBuf::from(&macro_block.target),
                        |directory| directory.join(&macro_block.target),
                    ));
                    if self.includes.contains(&path) {
                        attributes = Attributes::default();
                        title = None;
                        pending_block_metadata = false;
                        pending_metadata_start = None;
                        advance_macro(&mut position, macro_block, &mut pending_line);
                        pending_start = position;
                        continue;
                    }
                    match std::fs::read_to_string(&path) {
                        Ok(content) => {
                            block = Block::Div(
                                Box::new(Attr {
                                    classes: vec!["included".into()],
                                    attributes: vec![(
                                        "path".into(),
                                        path.to_string_lossy().as_ref().into(),
                                    )],
                                    ..Attr::default()
                                }),
                                self.included_document(&content, &path),
                            );
                        }
                        Err(error) => {
                            self.fatal_error(
                                std::io::Error::new(
                                    error.kind(),
                                    format!("{}: {error}", path.display()),
                                )
                                .into(),
                            );
                            break;
                        }
                    }
                } else {
                    block = Block::Para(vec![]);
                }
                advance_macro(&mut position, macro_block, &mut pending_line);
            } else if marker(line).is_some_and(|marker| {
                !self
                    .list_context
                    .iter()
                    .any(|(kind, depth)| marker.kind == *kind && marker.depth <= *depth)
            }) {
                block = self.list(lines, &mut position, &attributes);
            } else if definition_start {
                block = self.definition_list(lines, &mut position, attributes.style() == "ordered");
            } else if !list_item && pending_tail && line == "+" {
                block = Block::CodeBlock(Box::default(), "+\n".into());
                position += 1;
            } else if line.starts_with(' ') {
                let mut content = Vec::new();
                let mut indent = line.len() - line.trim_start_matches(' ').len();
                while position < lines.len()
                    && !physical_blank_line(lines[position])
                    && lines[position].starts_with(' ')
                {
                    content.push(lines[position]);
                    position += 1;
                }
                if let Some(minimum) = content
                    .iter()
                    .map(|s| s.len() - s.trim_start_matches(' ').len())
                    .min()
                {
                    indent = minimum;
                }
                let content = content
                    .iter()
                    .map(|s| s.get(indent..).unwrap_or(s))
                    .collect::<Vec<_>>()
                    .join("\n");
                block = Block::CodeBlock(Box::default(), format!("{content}\n").into());
            } else {
                if block_attributes(line).is_some()
                    && block_attributes_at(lines, position).is_none()
                {
                    self.syntax_error("malformed block attributes");
                    break;
                }
                if list_item && (marker(line).is_some() || definition(line).is_some()) {
                    position = pending_start;
                    break;
                }
                let start = position;
                position += 1;
                while position < lines.len()
                    && !physical_blank_line(lines[position])
                    && block_attributes_at(lines, position).is_none()
                    && block_attributes(lines[position]).is_none()
                    && !lines[position].starts_with("////")
                    && (!list_item
                        || (marker_in_list(lines[position]).is_none()
                            && marker_after_space(lines[position]).is_none()
                            && definition(lines[position]).is_none()
                            && lines[position] != "+"))
                {
                    position += 1;
                }
                let source = lines[start..position].join("\n");
                if matches!(style.as_str(), "literal" | "listing" | "source") {
                    let mut attr = Attr::default();
                    if let Some(language) =
                        attributes.positional.get(1).filter(|_| style == "source")
                    {
                        attr.classes.push(language.as_str().into());
                    }
                    block = Block::CodeBlock(
                        Box::new(attr),
                        if style == "literal" {
                            format!("{source}\n").into()
                        } else {
                            callouts(&source).into()
                        },
                    );
                } else if let Some((admonition, content)) = admonition(&source) {
                    block = self.admonition(
                        admonition,
                        content,
                        attributes.has("hardbreaks") || self.hard_breaks || style == "verse",
                    );
                } else {
                    let nodes = self.paragraph_inlines(
                        &source,
                        attributes.has("hardbreaks") || self.hard_breaks || style == "verse",
                    );
                    block = Block::Para(nodes);
                }
            }
            if matches!(
                style.as_str(),
                "NOTE" | "TIP" | "IMPORTANT" | "CAUTION" | "WARNING"
            ) && is_example
            {
                let blocks = if let Block::Div(_, blocks) = &mut block {
                    std::mem::take(blocks)
                } else {
                    vec![block]
                };
                block = admonition_blocks(&style, blocks);
            }
            if section_levels.len() == 1
                && let Block::Para(nodes) = &mut block
            {
                match paragraph_style {
                    's' => *nodes = vec![Inline::Strong(std::mem::take(nodes))],
                    'e' => *nodes = vec![Inline::Emph(std::mem::take(nodes))],
                    'm' => super::inline::monospace_run(nodes),
                    _ => {}
                }
            }
            if let Some(title) = title.take() {
                block = add_title(block, title);
            }
            let mut attr = attributes.attr();
            if matches!(block, Block::Table(..)) {
                attr.attributes.retain(|(key, _)| {
                    !matches!(key.as_str(), "options" | "cols" | "format" | "separator")
                });
            }
            if matches!(block, Block::OrderedList(..)) {
                attr.attributes.retain(|(key, _)| key != "start");
            }
            block = add_block_attr(block, attr, attributes.has_empty_id());
            output.push(block);
            if list_item
                && macro_block
                    .as_ref()
                    .is_some_and(|block| super::trim_space(&block.tail).is_empty())
            {
                self.list_boundary = true;
            }
            attributes = Attributes::default();
            pending_block_metadata = false;
            pending_metadata_start = None;
            body_attribute_group = false;
            body_attribute_gap = false;
            pending_start = position;
            if self.list_boundary {
                if list_item {
                    break;
                }
                self.list_boundary = false;
            }
        }
        if !partial
            && self.error.is_none()
            && (pending_block_metadata || position == lines.len() && pending_start < position)
        {
            self.syntax_error("attributes without a block");
        }
        group_titled_sections(&mut output);
        self.hard_breaks = inherited_hard_breaks;
        *cursor = position;
        output
    }

    pub(super) fn admonition(&mut self, kind: &str, content: &str, hard_breaks: bool) -> Block {
        admonition_blocks(
            kind,
            vec![Block::Para(self.paragraph_inlines(content, hard_breaks))],
        )
    }

    pub(super) fn html_blocks(content: &str) -> Vec<Block> {
        let mut options = ReaderOptions::default();
        options.extensions = carta_core::Extensions::from_list(&[Extension::RawHtml]);
        crate::HtmlReader.read(content, &options).map_or_else(
            |_| {
                vec![Block::RawBlock(
                    carta_ast::Format("html".into()),
                    content.into(),
                )]
            },
            |mut document| {
                carta_core::walk::for_each_document_attribute(&mut document, &mut |attr| {
                    let mut seen = BTreeSet::new();
                    attr.attributes.reverse();
                    attr.attributes
                        .retain(|(name, _)| seen.insert(name.to_string()));
                    attr.attributes.reverse();
                    protect_attribute_names(attr);
                });
                document.blocks
            },
        )
    }

    pub(super) fn syntax_error(&mut self, message: &str) {
        if self.error.is_none() {
            self.error = Some(ParserError::Syntax(message.to_owned()));
        }
    }

    pub(super) fn fatal_error(&mut self, error: carta_core::Error) {
        self.error = Some(ParserError::Fatal(error));
    }

    fn included_document(&mut self, source: &str, path: &std::path::Path) -> Vec<Block> {
        if self.include_depth >= 128 {
            self.fatal_error(carta_core::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "include nesting limit exceeded",
            )));
            return vec![];
        }
        self.include_depth += 1;
        self.includes.push(path.to_path_buf());
        let directory = std::mem::replace(
            &mut self.directory,
            path.parent().map(std::path::Path::to_path_buf),
        );
        let attributes = std::mem::take(&mut self.attributes);
        let counters = std::mem::take(&mut self.counters);
        let hard_breaks = self.hard_breaks;
        self.hard_breaks = false;
        let lists = std::mem::take(&mut self.list_context);
        let depth = self.depth;
        // Included documents have their own section context.
        self.depth = 0;
        let source = source.replace('\r', "");
        let document = self.document(&source);
        if let Some(ParserError::Syntax(message)) = self.error.as_ref() {
            let message = message.clone();
            self.fatal_error(carta_core::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                message,
            )));
        }
        self.depth = depth;
        self.list_context = lists;
        self.attributes = attributes;
        self.counters = counters;
        self.hard_breaks = hard_breaks;
        self.directory = directory;
        self.include_depth -= 1;
        self.includes.pop();
        let mut document = document;
        std::mem::take(&mut document.blocks)
    }

    fn include_listing(&mut self, content: &str) -> Option<String> {
        if !self.load_includes {
            return None;
        }
        if let Some((target, _)) = content
            .strip_prefix("include::")
            .filter(|_| !content.contains('\n') && callouts(content) == content)
            .and_then(super::inline::macro_target)
        {
            let path = self.directory.as_ref().map_or_else(
                || std::path::PathBuf::from(&target),
                |directory| directory.join(&target),
            );
            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    let contents = contents.replace('\r', "");
                    return Some(contents.lines().collect::<Vec<_>>().join("\n"));
                }
                Err(error) => {
                    self.fatal_error(
                        std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
                            .into(),
                    );
                }
            }
        }
        None
    }
}

fn heading(line: &str) -> Option<(i64, &str)> {
    let first = line.chars().next()?;
    if first != '=' && first != '#' {
        return None;
    }
    let count = line.chars().take_while(|c| *c == first).count();
    if count > 6 {
        return None;
    }
    let text = line.get(count..)?.strip_prefix(' ')?;
    Some((i64::try_from(count).unwrap_or(1) - 1, text))
}
fn fence(line: &str) -> Option<(&str, &'static str)> {
    let line = line.trim_end_matches([' ', '\t']);
    if line.starts_with("```") {
        let count = line
            .chars()
            .take_while(|character| *character == '`')
            .count();
        return Some((&line[..count], "markdown"));
    }
    if line == "--" {
        return Some((line, "open"));
    }

    let first = line.chars().next()?;
    if line.len() >= 4 && line.chars().all(|c| c == first) {
        let kind = match first {
            '-' => "listing",
            '.' => "literal",
            '=' => "example",
            '*' => "sidebar",
            '_' => "quote",
            '+' => "pass",
            _ => return None,
        };
        return Some((line, kind));
    }
    if let Some(end) = table_border_end(line, 0) {
        return Some((&line[..end], "table"));
    }
    None
}

fn comment_literal_delimiter(line: &str) -> Option<&str> {
    let delimiter = line.trim_end_matches([' ', '\t']);
    (delimiter.len() >= 2 && delimiter.bytes().all(|byte| byte == b'-')).then_some(delimiter)
}

fn thematic(line: &str) -> bool {
    let compact = line.replace(' ', "");
    matches!(compact.as_str(), "'''" | "---" | "***")
}
pub(super) fn admonition(source: &str) -> Option<(&str, &str)> {
    let (kind, content) = source.split_once(": ")?;
    matches!(kind, "NOTE" | "TIP" | "IMPORTANT" | "CAUTION" | "WARNING").then_some((kind, content))
}
fn admonition_blocks(kind: &str, blocks: Vec<Block>) -> Block {
    let mut contents = vec![Block::Div(
        Box::new(class_attr("title")),
        vec![Block::Para(vec![Inline::Str(
            format!("{}{}", &kind[..1], kind[1..].to_lowercase()).into(),
        )])],
    )];
    contents.extend(blocks);
    Block::Div(Box::new(class_attr(&kind.to_lowercase())), contents)
}
fn attribution(blocks: &mut Vec<Block>, attributes: &Attributes) {
    if !matches!(attributes.style(), "quote" | "verse") {
        return;
    }
    let mut fields: Vec<_> = attributes
        .positional
        .iter()
        .skip(1)
        .map(|field| super::trim_space(field))
        .collect();
    let Some(last) = fields.iter().rposition(|field| !field.is_empty()) else {
        return;
    };
    fields.truncate(last + 1);
    let text = format!("— {}", fields.join(", "));
    blocks.push(Block::Para(text_inlines(&text)));
}
pub(super) fn add_title(block: Block, title: Vec<Inline>) -> Block {
    let mut block = block;
    let section_level = super::document::section_level(&block);
    match &mut block {
        Block::Table(table) => table.caption.long = vec![Block::Plain(title)],
        Block::Figure(_, caption, _) => {
            **caption = Caption {
                short: None,
                long: vec![Block::Plain(title)],
            }
        }
        Block::Div(_, contents) => {
            let title = Block::Div(Box::new(class_attr("title")), vec![Block::Para(title)]);
            if matches!(contents.first(), Some(Block::Div(attr, _)) if attr.classes == vec!["title"])
            {
                if let Some(first) = contents.first_mut() {
                    *first = title;
                }
            } else {
                contents.insert(0, title);
            }
        }
        _ => {
            return Block::Div(
                Box::new(Attr {
                    attributes: section_level
                        .map(|level| (super::SECTION.into(), level.to_string().into()))
                        .into_iter()
                        .collect(),
                    ..Default::default()
                }),
                vec![
                    Block::Div(Box::new(class_attr("title")), vec![Block::Para(title)]),
                    block,
                ],
            );
        }
    }
    block
}
fn callouts(source: &str) -> String {
    let mut next = 1;
    source
        .split('\n')
        .map(|line| {
            let mut line = line.to_owned();
            let mut marks = Vec::new();
            while super::trim_space_end(&line).ends_with('>') {
                let trimmed = super::trim_space_end(&line);
                let Some(start) = trimmed.rfind('<') else {
                    break;
                };
                let label = &trimmed[start + 1..trimmed.len() - 1];
                if label == "." {
                    marks.push(None);
                } else if let Ok(number) = label.parse::<u32>() {
                    marks.push(Some(number));
                } else {
                    break;
                }
                line.truncate(start);
                line.truncate(super::trim_space_end(&line).len());
            }
            for mark in marks.into_iter().rev() {
                let number = mark.unwrap_or(next);
                next = number.saturating_add(1);
                if number <= 20 {
                    line.push(' ');
                    line.push(char::from_u32(0x245f + number).unwrap_or_default());
                } else {
                    let _ = write!(line, "<{number}>");
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn block_fence<'a>(
    lines: &[&'a str],
    position: usize,
    list_context: &[(char, usize)],
    attributes: &Attributes,
) -> Option<(&'a str, &'static str)> {
    let line = *lines.get(position)?;
    let (delimiter, kind) = fence(line).or_else(|| (line == "--").then_some((line, "open")))?;
    let next = lines.get(position + 1..)?;
    let close = if matches!(kind, "open" | "example" | "sidebar" | "quote") {
        container_close(lines, position, delimiter, kind, 0).map(|end| end - position - 1)
    } else if kind == "table" {
        let separator = table_cell_separator(attributes, delimiter);
        let inline_close = table_allows_inline_close(attributes, delimiter);
        next.iter().position(|candidate| {
            closing_fence_with_table_settings(candidate, delimiter, kind, separator, inline_close)
        })
    } else {
        next.iter()
            .position(|candidate| closing_fence(candidate, delimiter, kind))
    };
    let Some(close) = close else {
        return matches!(kind, "listing" | "literal" | "pass" | "markdown")
            .then_some((delimiter, kind));
    };
    if matches!(kind, "open" | "example" | "sidebar" | "quote") {
        if close > 0 && block_macro(lines, position + close).is_some() {
            return None;
        }
        if container_has_crossing_child(lines, position, position + 1 + close, list_context, 0) {
            return None;
        }
    }
    Some((delimiter, kind))
}

#[allow(
    clippy::too_many_lines,
    reason = "Fence validation shares block and list boundary state."
)]
fn container_has_crossing_child(
    lines: &[&str],
    start: usize,
    end: usize,
    list_context: &[(char, usize)],
    depth: usize,
) -> bool {
    if depth >= 128 {
        return false;
    }
    let mut position = start + 1;
    let mut block_start = true;
    let mut in_list = !list_context.is_empty();
    while position < end {
        let Some(line) = lines.get(position) else {
            break;
        };
        if physical_blank_line(line) {
            position += 1;
            block_start = true;
            continue;
        }
        if in_list && super::trim_space(line) == "+" {
            position += 1;
            block_start = true;
            continue;
        }
        if !block_start {
            if block_attributes_at(lines, position).is_some()
                || line.starts_with("////") && line.chars().all(|character| character == '/')
            {
                block_start = true;
                continue;
            }
            if in_list && (marker_in_list(line).is_some() || definition(line).is_some()) {
                position += 1;
                in_list = true;
                continue;
            }
            position += 1;
            continue;
        }
        if line_comment(line) {
            position += 1;
            continue;
        }
        if let Some((_, count)) = block_attributes_at(lines, position) {
            if position.saturating_add(count) > end {
                return true;
            }
            position += count;
            continue;
        }
        if block_anchor_line(line)
            || line.starts_with('.') && !line.starts_with("..") && !line.starts_with(". ")
            || document_attribute(line)
        {
            position += 1;
            continue;
        }
        if line.starts_with("////") && line.chars().all(|character| character == '/') {
            let mut close = position + 1;
            while close < end && lines.get(close) != Some(line) {
                close += 1;
            }
            if close == end {
                return false;
            }
            position = close + 1;
            continue;
        }
        if let Some(marker) = if in_list {
            marker_in_list(line)
        } else {
            marker(line)
        } {
            if list_context
                .iter()
                .any(|(kind, depth)| marker.kind == *kind && marker.depth <= *depth)
            {
                block_start = false;
                position += 1;
                continue;
            }
            in_list = true;
            let body = marker.content.trim_start_matches(' ');
            if let Some((delimiter, kind)) = raw_fence(body) {
                let Some(close) = raw_fence_close(lines, position, delimiter, kind) else {
                    return true;
                };
                if close >= end {
                    return true;
                }
                position = close + 1;
                continue;
            }
            if body.is_empty() || block_metadata_line(body) {
                position += 1;
                continue;
            }
            block_start = false;
            position += 1;
            continue;
        }
        if let Some((_, body)) = definition(line) {
            in_list = true;
            if let Some((delimiter, kind)) = raw_fence(body) {
                let Some(close) = raw_fence_close(lines, position, delimiter, kind) else {
                    return true;
                };
                if close >= end {
                    return true;
                }
                position = close + 1;
                continue;
            }
            if body.is_empty() || block_metadata_line(body) {
                position += 1;
                continue;
            }
            block_start = false;
            position += 1;
            continue;
        }
        if let Some((delimiter, kind)) = fence(line) {
            if matches!(kind, "listing" | "literal" | "pass" | "markdown") {
                let Some(close) = raw_fence_close(lines, position, delimiter, kind) else {
                    return true;
                };
                if close >= end {
                    return true;
                }
                position = close + 1;
                continue;
            }
            if matches!(kind, "open" | "example" | "sidebar" | "quote")
                && let Some(close) = container_close(lines, position, delimiter, kind, 0)
                && close < end
                && !container_has_crossing_child(lines, position, close, list_context, depth + 1)
            {
                position = close + 1;
                continue;
            }
        }
        if (if in_list {
            marker_in_list(line).is_some()
        } else {
            marker(line).is_some()
        }) || definition(line).is_some()
        {
            in_list = true;
        }
        block_start = false;
        position += 1;
    }
    false
}

fn raw_fence(line: &str) -> Option<(&str, &'static str)> {
    fence(line).filter(|(_, kind)| matches!(*kind, "listing" | "literal" | "pass" | "markdown"))
}

fn raw_fence_close(lines: &[&str], start: usize, delimiter: &str, kind: &str) -> Option<usize> {
    lines
        .get(start + 1..)?
        .iter()
        .position(|line| closing_fence(line, delimiter, kind))
        .map(|close| start + close + 1)
}

fn block_metadata_line(line: &str) -> bool {
    block_attributes(line).is_some()
        || block_anchor_line(line)
        || line.starts_with('.') && !line.starts_with("..") && !line.starts_with(". ")
        || document_attribute(line)
}

fn block_anchor_line(line: &str) -> bool {
    let Some(source) = line.strip_prefix("[[") else {
        return false;
    };
    let Some(end) = source.find(|character: char| super::is_space(character) || character == ']')
    else {
        return false;
    };
    end > 0 && source.get(end..).is_some_and(|rest| rest == "]]")
}

fn document_attribute(line: &str) -> bool {
    let Some(source) = line.strip_prefix(':') else {
        return false;
    };
    let source = source.strip_prefix('!').unwrap_or(source);
    let Some((name, _)) = source.split_once(':') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn container_close(
    lines: &[&str],
    start: usize,
    delimiter: &str,
    kind: &str,
    depth: usize,
) -> Option<usize> {
    if depth >= 128 {
        return None;
    }
    let mut position = start + 1;
    while let Some(line) = lines.get(position) {
        if closing_fence(line, delimiter, kind) {
            let comment = line
                .strip_prefix(delimiter)
                .is_some_and(|rest| line_comment(super::trim_space_start(rest)));
            if !comment || previous_fence_ends_block(lines, position) {
                if position > start + 1
                    && lines
                        .get(position - 1)
                        .is_some_and(|previous| physical_blank_line(previous))
                {
                    position = container_close(lines, position, delimiter, kind, depth + 1)? + 1;
                    continue;
                }
                return Some(position);
            }
        }
        position += 1;
    }
    None
}

fn previous_fence_ends_block(lines: &[&str], position: usize) -> bool {
    let Some(previous_position) = position.checked_sub(1) else {
        return false;
    };
    let Some(previous) = lines.get(previous_position) else {
        return false;
    };
    let Some((delimiter, kind)) = fence(previous) else {
        return false;
    };
    if !matches!(kind, "listing" | "literal" | "pass" | "markdown") {
        return false;
    }
    lines.get(..previous_position).is_some_and(|before| {
        before
            .iter()
            .any(|line| fence(line) == Some((delimiter, kind)))
    })
}

fn closing_fence(line: &str, delimiter: &str, kind: &str) -> bool {
    closing_fence_with_table_settings(
        line,
        delimiter,
        kind,
        delimiter.chars().next().unwrap_or('|'),
        true,
    )
}

fn closing_fence_with_table_settings(
    line: &str,
    delimiter: &str,
    kind: &str,
    table_separator: char,
    table_inline_close: bool,
) -> bool {
    if kind == "table" {
        return table_closing_prefix(line, table_separator, table_inline_close).is_some();
    }
    let Some(rest) = line.strip_prefix(delimiter) else {
        return false;
    };
    kind == "markdown" || {
        let rest = rest.trim_start_matches([' ', '\t']);
        rest.is_empty() || line_comment(rest)
    }
}

fn table_cell_separator(attributes: &Attributes, delimiter: &str) -> char {
    attributes
        .get("separator")
        .and_then(|separator| separator.chars().next())
        .or_else(|| match attributes.get("format") {
            Some("psv") => Some('|'),
            Some("csv") => Some(','),
            Some("dsv") => Some(':'),
            Some("tsv") => Some('\t'),
            _ => delimiter
                .chars()
                .next()
                .filter(|character| matches!(character, '|' | ',' | ':')),
        })
        .unwrap_or('|')
}

fn table_allows_inline_close(attributes: &Attributes, delimiter: &str) -> bool {
    match attributes.get("format") {
        Some("psv") => true,
        Some("csv" | "dsv" | "tsv") => false,
        _ => delimiter.starts_with('|'),
    }
}

fn table_closing_prefix(line: &str, separator: char, inline: bool) -> Option<&str> {
    for (start, character) in line.char_indices() {
        if !inline && start != 0 {
            break;
        }
        if character == separator && line[..start].ends_with('\\') {
            continue;
        }
        if table_border_end(line, start).is_some() {
            return line.get(..start);
        }
    }
    None
}

fn table_border_end(line: &str, start: usize) -> Option<usize> {
    let marker = line.get(start..)?.chars().next()?;
    if !matches!(marker, '|' | ',' | ':') {
        return None;
    }
    let equals_start = start + marker.len_utf8();
    let equals = line.get(equals_start..)?;
    let count = equals.bytes().take_while(|byte| *byte == b'=').count();
    if count < 3 {
        return None;
    }
    let end = equals_start + count;
    let tail = line.get(end..)?.trim_start_matches([' ', '\t']);
    if !tail.is_empty() && !tail.starts_with("// ") && !tail.starts_with("//\t") {
        return None;
    }
    Some(end)
}

fn markdown_tail<'a>(line: &'a str, delimiter: &str) -> Option<&'a str> {
    let mut tail = line.strip_prefix(delimiter)?;
    if line_comment(super::trim_space_start(tail)) {
        return None;
    }
    let extra_ticks = tail.len() - tail.trim_start_matches('`').len();
    tail = tail.trim_start_matches('`');
    if extra_ticks > 0 {
        tail = super::trim_space_start(tail);
    }
    (!super::trim_space(tail).is_empty()).then_some(tail)
}

fn group_titled_sections(blocks: &mut Vec<Block>) {
    let mut position = 0;
    while position < blocks.len() {
        let section = blocks.get(position).and_then(|block| match block {
            Block::Div(..) => super::document::section_level(block),
            _ => None,
        });
        if let Some(level) = section {
            let mut end = position + 1;
            while end < blocks.len()
                && blocks
                    .get(end)
                    .and_then(super::document::section_level)
                    .is_none_or(|next| next > level)
            {
                end += 1;
            }
            let mut children = blocks.drain(position + 1..end).collect::<Vec<_>>();
            group_titled_sections(&mut children);
            if let Some(Block::Div(_, contents)) = blocks.get_mut(position) {
                contents.extend(children);
            }
        } else if let Some(Block::Div(_, contents) | Block::BlockQuote(contents)) =
            blocks.get_mut(position)
        {
            group_titled_sections(contents);
        }
        position += 1;
    }
}

struct BlockMacro {
    name: &'static str,
    target: String,
    attributes: String,
    tail: String,
    consumed_lines: usize,
}

fn block_macro(lines: &[&str], position: usize) -> Option<BlockMacro> {
    let line = *lines.get(position)?;
    block_macro_line(lines, position, line)
}

fn block_macro_line(lines: &[&str], position: usize, line: &str) -> Option<BlockMacro> {
    let (name, source) = line.split_once("::")?;
    let name = match name {
        "image" => "image",
        "video" => "video",
        "audio" => "audio",
        "toc" => "toc",
        "include" => "include",
        _ => return None,
    };
    let (target, start) = super::inline::macro_target(source)?;
    if !source[start..].starts_with('[') {
        return None;
    }
    let mut arguments = source[start..].to_owned();
    let mut end = super::inline::attribute_end(&arguments);
    if end.is_none() {
        for line in lines.get(position + 1..)? {
            arguments.push('\n');
            arguments.push_str(line);
        }
        end = super::inline::attribute_end(&arguments);
    }
    let end = end?;
    let consumed_lines = arguments[..end]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    let attributes = &arguments[1..end];
    if !Attributes::valid(attributes) {
        return None;
    }
    let tail = arguments[end + 1..].split('\n').next().unwrap_or_default();
    Some(BlockMacro {
        name,
        target,
        attributes: attributes.to_owned(),
        tail: tail.to_owned(),
        consumed_lines,
    })
}

fn advance_macro(
    position: &mut usize,
    block: &BlockMacro,
    pending_line: &mut Option<(usize, String)>,
) {
    let close = *position + block.consumed_lines;
    if super::trim_space(&block.tail).is_empty() {
        *position = close + 1;
        *pending_line = None;
    } else {
        *position = close;
        *pending_line = Some((close, block.tail.clone()));
    }
}

fn paragraph_end(lines: &[&str], position: usize, list_item: bool) -> usize {
    let mut end = position + 1;
    while let Some(line) = lines.get(end) {
        if physical_blank_line(line)
            || block_attributes_at(lines, end).is_some()
            || block_attributes(line).is_some()
            || line.starts_with("////")
            || list_item
                && (marker_in_list(line).is_some() || definition(line).is_some() || *line == "+")
        {
            break;
        }
        end += 1;
    }
    end
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::AsciidocReader;
    use carta_ast::{Block, Inline};
    use carta_core::{Reader, ReaderOptions};

    fn read(source: &str) -> Vec<Block> {
        AsciidocReader
            .read(source, &ReaderOptions::default())
            .unwrap()
            .blocks
    }

    #[test]
    fn passthrough_blocks_keep_their_final_newline() {
        assert_eq!(
            read("++++\n<!\n++++\n"),
            vec![Block::Plain(vec![Inline::RawInline(
                carta_ast::Format("html".into()),
                "<!--\n-->".into(),
            )])]
        );
    }

    #[test]
    fn indented_line_comments_preserve_headers_and_pending_attributes() {
        for indentation in [" ", "  ", "\t", " \t"] {
            for tab_stop in [0, 4] {
                let mut options = ReaderOptions::default();
                options.tab_stop = tab_stop;
                let source = format!(
                    "{indentation}// hidden\n= Title\n\n[.role]\n{indentation}// hidden\nText\n"
                );
                let document = AsciidocReader.read(&source, &options).unwrap();
                assert!(
                    matches!(document.blocks.as_slice(), [Block::Div(attr, _)]
                    if attr.classes == ["role"]),
                    "{source:?}"
                );
                assert!(document.meta.contains_key("title"), "{source:?}");
            }
        }
        assert_eq!(
            read(" code\n // retained\n"),
            vec![Block::CodeBlock(
                Box::default(),
                "code\n// retained\n".into()
            )]
        );
    }

    #[test]
    fn styled_comments_use_dash_literal_fences_and_paragraph_fallback() {
        for delimiter in ["--", "---", "----"] {
            let source =
                format!("[.role]\n[comment]\n{delimiter}\nhidden\n{delimiter}\nVisible.\n");
            assert_eq!(
                read(&source),
                vec![Block::Para(vec![Inline::Str("Visible.".into())])],
                "delimiter: {delimiter}"
            );
        }

        assert!(read("[.role]\n[comment]\n====\nText\n====\nText\n").is_empty());
        assert_eq!(
            read("[.role]\n[comment]\n====\nText\n====\n\nVisible.\n"),
            vec![Block::Para(vec![Inline::Str("Visible.".into())])]
        );
    }
}
