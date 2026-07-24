//! Final tree construction: turning the parsed node tree into the block IR.

use super::{
    Attr, ExampleMap, Extension, FootnoteDefs, Format, IrBlock, IrDefItem, Kind, ListAttributes,
    ListInfo, ListNumberDelim, ListNumberStyle, Node, Parser, RefMap, alert_marker_type,
    attach_table_captions, demote_loose_paragraphs, fence_attr, grid, line_block_lines,
    next_example_number, raw_block_format, scan, split_table_lines, strip_one_trailing_newline,
    strip_trailing_blank_lines, table, texttable, tighten_last_block,
};

impl Parser {
    pub(super) fn finish(mut self) -> (Vec<IrBlock>, RefMap, FootnoteDefs, ExampleMap) {
        for index in 0..self.nodes.len() {
            if matches!(self.kind(index), Some(Kind::Paragraph)) {
                let column_zero = self.nodes.get(index).is_some_and(|node| node.indent == 0);
                let consumed = self.extract_leading_definitions(index, column_zero);
                if consumed > 0
                    && let Some(node) = self.nodes.get_mut(index)
                {
                    node.text.drain(..consumed);
                }
            }
        }
        let mut footnotes = self.collect_footnotes();
        for blocks in footnotes.values_mut() {
            attach_table_captions(blocks, self.extensions);
        }
        let examples = self.number_examples();
        let mut blocks = self.build_children(0);
        attach_table_captions(&mut blocks, self.extensions);
        (blocks, self.refs, footnotes, examples)
    }

    /// Number every example-list item in one document-wide sequence and record the label→number map.
    /// Each example list's `start` becomes its first item's number; the map drives `@label`
    /// references during the inline phase.
    pub(super) fn number_examples(&mut self) -> ExampleMap {
        let mut counter = 0;
        let mut map = ExampleMap::new();
        self.number_examples_in(0, &mut counter, &mut map);
        map
    }

    /// Walk `index` and its descendants in document order, assigning example numbers. Items are
    /// visited before the content nested beneath them, so a nested example list continues the same
    /// sequence in reading order.
    pub(super) fn number_examples_in(
        &mut self,
        index: usize,
        counter: &mut i32,
        map: &mut ExampleMap,
    ) {
        let Some(node) = self.nodes.get(index) else {
            return;
        };
        let is_example =
            matches!(&node.kind, Kind::List(info) if info.style == ListNumberStyle::Example);
        let children = node.children.clone();
        if !is_example {
            for child in children {
                self.number_examples_in(child, counter, map);
            }
            return;
        }
        let mut start = None;
        for item in children {
            let Some(item_node) = self.nodes.get(item) else {
                continue;
            };
            let Kind::Item(info) = &item_node.kind else {
                continue;
            };
            let label = info.example_label.clone();
            let item_children = item_node.children.clone();
            start.get_or_insert(next_example_number(label, counter, map));
            for child in item_children {
                self.number_examples_in(child, counter, map);
            }
        }
        if let Some(start) = start
            && let Some(node) = self.nodes.get_mut(index)
            && let Kind::List(info) = &mut node.kind
        {
            info.start = start;
        }
    }

    /// Gather every footnote definition's block content, keyed by its normalized label. Definitions
    /// are visited in document order (node-creation order); when a label repeats, the first wins.
    pub(super) fn collect_footnotes(&self) -> FootnoteDefs {
        let mut footnotes = FootnoteDefs::new();
        for index in 0..self.nodes.len() {
            if let Some(Kind::FootnoteDef(key)) = self.kind(index)
                && !footnotes.contains_key(key)
            {
                footnotes.insert(key.clone(), self.build_children(index));
            }
        }
        footnotes
    }

    /// Pull leading definitions off `text` and return what remains. Link reference definitions are
    /// always eligible; an abbreviation definition (`abbreviations`) requires its host paragraph to
    /// begin flush at the container's left edge, so `column_zero` gates it.
    pub(super) fn extract_refs(&mut self, text: &str, column_zero: bool) -> String {
        let abbreviations = column_zero && self.extensions.contains(Extension::Abbreviations);
        let mut remaining = text;
        loop {
            if let Some((label, def, rest)) =
                scan::parse_link_reference_definition(remaining, self.greedy_paragraphs)
            {
                self.refs.entry(label).or_insert(def);
                remaining = rest;
                continue;
            }
            if abbreviations && let Some(rest) = scan::parse_abbreviation_definition(remaining) {
                remaining = rest;
                continue;
            }
            break;
        }
        remaining.to_owned()
    }

    /// Register the definitions that lead a paragraph's text and return the byte length they occupy,
    /// so the caller can strip them from the node's buffer in place. Behaves as [`Self::extract_refs`]
    /// but reports the consumed prefix length rather than copying the remainder.
    pub(super) fn extract_leading_definitions(&mut self, index: usize, column_zero: bool) -> usize {
        let abbreviations = column_zero && self.extensions.contains(Extension::Abbreviations);
        let greedy = self.greedy_paragraphs;
        let Some(node) = self.nodes.get(index) else {
            return 0;
        };
        let text = node.text.as_str();
        let mut remaining = text;
        loop {
            if let Some((label, def, rest)) =
                scan::parse_link_reference_definition(remaining, greedy)
            {
                self.refs.entry(label).or_insert(def);
                remaining = rest;
                continue;
            }
            if abbreviations && let Some(rest) = scan::parse_abbreviation_definition(remaining) {
                remaining = rest;
                continue;
            }
            break;
        }
        text.len() - remaining.len()
    }

    pub(super) fn build_children(&self, index: usize) -> Vec<IrBlock> {
        let Some(node) = self.nodes.get(index) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &child in &node.children {
            // A raw HTML element flattens to open tag, content, and close tag.
            if let Some(Kind::HtmlElement(info)) = self.kind(child)
                && !info.as_div
            {
                out.push(IrBlock::RawHtml(info.raw_open.clone()));
                let mut content = self.build_children(child);
                if info.tighten_last {
                    tighten_last_block(&mut content);
                }
                out.append(&mut content);
                // An element left open at end of input has no close tag; emit one only when present.
                if !info.raw_close.is_empty() {
                    out.push(IrBlock::RawHtml(info.raw_close.clone()));
                }
                continue;
            }
            if let Some(block) = self.build_block(child) {
                out.push(block);
            }
        }
        out
    }

    pub(super) fn build_block(&self, index: usize) -> Option<IrBlock> {
        let node = self.nodes.get(index)?;
        let block = match &node.kind {
            // Footnote defs lift into the footnote map; definition-list levels carry no block.
            Kind::Document
            | Kind::Item(_)
            | Kind::FootnoteDef(_)
            | Kind::DefinitionItem { .. }
            | Kind::Definition { .. } => return None,
            Kind::Paragraph => {
                let trimmed = node.text.trim();
                if trimmed.is_empty() {
                    return None;
                }
                if self.extensions.contains(Extension::GridTables)
                    && let Some(table) = grid::parse(trimmed)
                {
                    IrBlock::GridTable(Box::new(table))
                } else if self.extensions.contains(Extension::PipeTables)
                    && let Some((alignments, header, rows)) =
                        table::try_parse(trimmed, self.greedy_paragraphs)
                {
                    IrBlock::Table {
                        alignments,
                        header,
                        rows,
                        caption: None,
                        attr: Attr::default(),
                    }
                } else if node.as_plain {
                    IrBlock::Plain(trimmed.to_owned())
                } else {
                    IrBlock::Para(trimmed.to_owned())
                }
            }
            Kind::LineBlock => IrBlock::LineBlock(line_block_lines(&node.text)),
            Kind::TextTable => {
                let lines = split_table_lines(&node.text);
                let (table, _) = texttable::parse(&lines, self.extensions)?;
                IrBlock::TextTable(Box::new(table))
            }
            Kind::DefinitionList => self.build_definition_list(index),
            Kind::Heading(level) => IrBlock::Heading(*level, node.text.trim().to_owned()),
            Kind::ThematicBreak => IrBlock::ThematicBreak,
            Kind::IndentedCode => {
                IrBlock::CodeBlock(Attr::default(), strip_trailing_blank_lines(&node.text))
            }
            Kind::FencedCode(fence) => {
                // A closing fence drops the final newline; end-of-input keeps it.
                let text = if node.open {
                    node.text.clone()
                } else {
                    strip_one_trailing_newline(&node.text)
                };
                // An info string of exactly `{=FORMAT}` marks the contents as raw output for FORMAT.
                if self.extensions.contains(Extension::RawAttribute)
                    && let Some(format) = raw_block_format(&fence.info)
                {
                    IrBlock::RawBlock(Format(format.into()), text)
                } else {
                    IrBlock::CodeBlock(fence_attr(&fence.info, self.extensions), text)
                }
            }
            Kind::HtmlBlock(kind) => IrBlock::RawHtml(self.finalize_html_block(node, *kind)),
            Kind::RawHtmlSpan { .. } => {
                IrBlock::RawHtml(node.text.trim_end_matches('\n').to_owned())
            }
            Kind::RawTex { .. } => {
                // Still open at end of input: no `\end` found, the lines fall back to a paragraph.
                if node.open {
                    let trimmed = node.text.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    IrBlock::Para(trimmed.to_owned())
                } else {
                    IrBlock::RawBlock(Format("tex".into()), node.text.clone())
                }
            }
            Kind::BlockQuote => {
                if self.extensions.contains(Extension::Alerts)
                    && let Some(alert) = self.build_alert(index)
                {
                    alert
                } else {
                    IrBlock::BlockQuote(self.build_children(index))
                }
            }
            Kind::FencedDiv(info) => IrBlock::Div(info.attr.clone(), self.build_children(index)),
            Kind::HtmlElement(info) => {
                // The raw form is spliced in by `build_children`; only the div form is a block here.
                if !info.as_div {
                    return None;
                }
                let mut children = self.build_children(index);
                if info.tighten_last {
                    tighten_last_block(&mut children);
                }
                IrBlock::Div(info.attr.clone(), children)
            }
            Kind::List(info) => self.build_list(index, info),
        };
        Some(block)
    }

    /// Finalize a raw HTML block's text. The markdown dialect drops a block's final newline; the
    /// strict dialect instead pads an unterminated kind 1–5 block, which closes only on an explicit
    /// end tag, so reaching end-of-input with it still open surfaces as a trailing blank line.
    pub(super) fn finalize_html_block(&self, node: &Node, kind: u8) -> String {
        let mut text = node.text.clone();
        if self.greedy_paragraphs {
            if text.ends_with('\n') {
                text.pop();
            }
        } else if node.open && matches!(kind, 1..=5) {
            text.push('\n');
        }
        text
    }

    /// A blockquote whose first content line is exactly an alert marker `[!TYPE]` (with `TYPE` one of
    /// the recognized kinds, and nothing but trailing whitespace after the `]`) becomes a `Div`
    /// classed by the lowercased type. The broad Markdown dialect requires the uppercase spelling;
    /// the `CommonMark` engine accepts any casing. Its first child is a titled `Div` holding the
    /// type's display name; the marker line is stripped from the quote's first paragraph, and the
    /// rest of the quote's content follows. Returns `None` when the first line is not a clean marker,
    /// leaving the blockquote as an ordinary `BlockQuote`.
    pub(super) fn build_alert(&self, index: usize) -> Option<IrBlock> {
        let node = self.nodes.get(index)?;
        let &first = node.children.first()?;
        let first_node = self.nodes.get(first)?;
        if !matches!(first_node.kind, Kind::Paragraph) {
            return None;
        }
        // Raw (untrimmed) text keeps a leading space visible, which disables the marker.
        let (marker_line, rest_of_para) = match first_node.text.split_once('\n') {
            Some((line, rest)) => (line, Some(rest)),
            None => (first_node.text.as_str(), None),
        };
        // Known limitation: a marker indented 2+ columns still matches; the indent was folded away.
        let alert_type = alert_marker_type(marker_line, self.greedy_paragraphs)?;

        let title = IrBlock::Div(
            Attr {
                id: carta_ast::Text::default(),
                classes: vec!["title".into()],
                attributes: Vec::new(),
            },
            vec![IrBlock::Para(alert_type.title.to_owned())],
        );

        let mut content = vec![title];
        // Anything left on the marker's own paragraph after dropping its first line stays a paragraph.
        if let Some(rest) = rest_of_para {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                content.push(IrBlock::Para(trimmed.to_owned()));
            }
        }
        for &child in node.children.iter().skip(1) {
            if let Some(block) = self.build_block(child) {
                content.push(block);
            }
        }

        Some(IrBlock::Div(
            Attr {
                id: carta_ast::Text::default(),
                classes: vec![alert_type.class.into()],
                attributes: Vec::new(),
            },
            content,
        ))
    }

    /// Build a definition list from its item and definition containers. Each item contributes its
    /// term text and the block content of each `:`/`~` definition; a tight item demotes its
    /// definitions' top-level paragraphs to `Plain`.
    pub(super) fn build_definition_list(&self, index: usize) -> IrBlock {
        let mut items = Vec::new();
        if let Some(list) = self.nodes.get(index) {
            for &item_index in &list.children {
                let Some(Kind::DefinitionItem { term, tight }) = self.kind(item_index) else {
                    continue;
                };
                let mut definitions = Vec::new();
                if let Some(item) = self.nodes.get(item_index) {
                    for &def_index in &item.children {
                        let mut blocks = self.build_children(def_index);
                        if *tight {
                            demote_loose_paragraphs(&mut blocks);
                        }
                        definitions.push(blocks);
                    }
                }
                items.push(IrDefItem {
                    term: term.clone(),
                    definitions,
                });
            }
        }
        IrBlock::DefinitionList(items)
    }

    pub(super) fn build_list(&self, index: usize, info: &ListInfo) -> IrBlock {
        let tight = self.list_is_tight(index);
        let mut items = Vec::new();
        if let Some(node) = self.nodes.get(index) {
            for &item in &node.children {
                let mut blocks = self.build_children(item);
                if tight {
                    demote_loose_paragraphs(&mut blocks);
                }
                items.push(blocks);
            }
        }
        if info.bullet {
            IrBlock::BulletList(items)
        } else {
            // Start numbers are honored only under `startnum` in the greedy dialect.
            let start = if self.greedy_paragraphs && !self.extensions.contains(Extension::Startnum)
            {
                1
            } else {
                info.start
            };
            // Without fancy lists the greedy dialect collapses style and delimiter to the defaults.
            let (style, delim) =
                if self.greedy_paragraphs && !self.extensions.contains(Extension::FancyLists) {
                    (ListNumberStyle::DefaultStyle, ListNumberDelim::DefaultDelim)
                } else {
                    (info.style, info.delim)
                };
            let attrs = ListAttributes {
                start,
                style,
                delim,
            };
            IrBlock::OrderedList(attrs, items)
        }
    }

    /// A list is tight unless a blank line separates two of its items, or separates two blocks
    /// within an item (`CommonMark` §5.3).
    pub(super) fn list_is_tight(&self, index: usize) -> bool {
        let Some(list) = self.nodes.get(index) else {
            return true;
        };
        for (position, &item) in list.children.iter().enumerate() {
            let has_next_item = position + 1 < list.children.len();
            if has_next_item && self.ends_with_blank_line(item) {
                return false;
            }
            if let Some(item_node) = self.nodes.get(item) {
                for (child_position, &child) in item_node.children.iter().enumerate() {
                    let has_next = has_next_item || child_position + 1 < item_node.children.len();
                    if has_next && self.ends_with_blank_line(child) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Whether `index` (or the tail of its last-child chain through lists and items) was followed
    /// by a blank line.
    pub(super) fn ends_with_blank_line(&self, mut index: usize) -> bool {
        loop {
            let Some(node) = self.nodes.get(index) else {
                return false;
            };
            if node.last_line_blank {
                return true;
            }
            if matches!(node.kind, Kind::List(_) | Kind::Item(_)) {
                match node.children.last() {
                    Some(&last) => index = last,
                    None => return false,
                }
            } else {
                return false;
            }
        }
    }
}
