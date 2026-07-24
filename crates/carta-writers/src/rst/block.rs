//! Block rendering for the reStructuredText writer: paragraphs, headings, lists, quotes, figures, and code blocks.

use carta_ast::{
    Attr, Block, Caption, Format, Inline, ListAttributes, ListNumberDelim, ListNumberStyle,
    MathType, Target, Text, slug, to_plain_text,
};

use crate::common::{
    Dimension, attribute_value, display_width, format_length_dimension, format_percent_dimension,
    indent_block, offset_as_i32, ordered_marker, pad_marker, parse_dimension,
};

use super::State;
use super::inline::flatten;

impl State {
    /// Render a paragraph. A paragraph holding a forced line break becomes a line block; one holding
    /// display math is split around each formula into separate paragraphs and `.. math::` directives;
    /// otherwise its inlines are filled to `width`. With `hang`, a space that opens the paragraph is
    /// kept (see [`Self::lay`]).
    pub(super) fn para(&mut self, inlines: &[Inline], width: usize, hang: bool) -> String {
        if inlines
            .iter()
            .any(|inline| matches!(inline, Inline::LineBreak))
        {
            let lines = split_at(inlines, |inline| matches!(inline, Inline::LineBreak));
            let rendered: Vec<String> = lines
                .iter()
                .map(|line| self.render_line(line, width))
                .collect();
            return rendered.join("\n");
        }
        if contains_display_math(inlines) {
            let flattened = unwrap_transparent(inlines);
            return self.para_with_math(&flattened, width);
        }
        self.lay(inlines, width, hang)
    }

    /// Render a paragraph that carries display math, splitting it into `.. math::` directives around
    /// the surrounding inline runs. A run adjacent to a directive keeps an escaped-space marker on the
    /// touching side so the inline flow survives the block break; two abutting directives are parted by
    /// a standalone escaped space.
    fn para_with_math(&mut self, inlines: &[Inline], width: usize) -> String {
        enum Piece {
            Math(String),
            Text(String),
        }
        const MARKER: &str = "\\ ";
        let mut pieces: Vec<Piece> = Vec::new();
        let mut start = 0;
        for (index, inline) in inlines.iter().enumerate() {
            if let Inline::Math(MathType::DisplayMath, tex) = inline {
                if let Some(segment) = inlines.get(start..index) {
                    let text = self.lay(segment, width, false);
                    if !text.is_empty() {
                        pieces.push(Piece::Text(text));
                    }
                }
                pieces.push(Piece::Math(tex.to_string()));
                start = index + 1;
            }
        }
        if let Some(segment) = inlines.get(start..) {
            let text = self.lay(segment, width, false);
            if !text.is_empty() {
                pieces.push(Piece::Text(text));
            }
        }

        let mut parts: Vec<String> = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            let prev_math = index
                .checked_sub(1)
                .is_some_and(|prev| matches!(pieces.get(prev), Some(Piece::Math(_))));
            let next_math = matches!(pieces.get(index + 1), Some(Piece::Math(_)));
            match piece {
                Piece::Math(tex) => {
                    if prev_math {
                        parts.push(MARKER.to_owned());
                    }
                    parts.push(math_directive(tex));
                }
                Piece::Text(text) => {
                    let mut line = String::new();
                    if prev_math {
                        line.push_str(MARKER);
                    }
                    line.push_str(text);
                    if next_math {
                        line.push_str(MARKER);
                    }
                    parts.push(line);
                }
            }
        }
        parts.join("\n\n")
    }

    /// Render one line-block line: its inlines filled to the body width, then prefixed with `| ` and
    /// continuation lines indented to match.
    pub(super) fn render_line(&mut self, line: &[Inline], width: usize) -> String {
        let body = self.lay(line, width.saturating_sub(2), false);
        indent_block(&body, "| ", "  ")
    }

    pub(super) fn header(
        &mut self,
        level: i32,
        attr: &Attr,
        inlines: &[Inline],
        top: bool,
    ) -> String {
        let line = flatten(self.tokens(inlines)).trim().to_owned();
        if !top {
            let mut rubric = format!(".. rubric:: {line}");
            if !attr.id.is_empty() {
                rubric.push_str("\n   :name: ");
                rubric.push_str(&attr.id);
            }
            return rubric;
        }
        if line.is_empty() {
            return String::new();
        }
        let underline = heading_char(level).to_string().repeat(display_width(&line));
        let header = format!("{line}\n{underline}");
        if attr.id.is_empty() || attr.id == auto_identifier(inlines) {
            header
        } else {
            format!(".. _{}:\n\n{header}", attr.id)
        }
    }

    fn item_body(&mut self, item: &[Block], width: usize) -> String {
        let body = self.blocks_laid(item, width, false, true);
        if !body.is_empty() && item.first().is_some_and(marker_stands_alone) {
            format!("\n\n{body}")
        } else {
            lead_quote_fence(item, body)
        }
    }

    pub(super) fn bullet_list(&mut self, items: &[Vec<Block>], width: usize) -> String {
        let mut units = Vec::new();
        for item in items {
            let simple = is_simple_item(item);
            let body = self.item_body(item, width.saturating_sub(2));
            units.push((simple, indent_block(&body, "- ", "  ")));
        }
        join_loose_items(units)
    }

    pub(super) fn ordered_list(
        &mut self,
        attrs: &ListAttributes,
        items: &[Vec<Block>],
        width: usize,
    ) -> String {
        // Unstyled lists starting at 1 use the `#` auto-enumerator; an explicit style or later start needs literal numbers.
        let auto_enumerated = attrs.start == 1
            && matches!(attrs.style, ListNumberStyle::DefaultStyle)
            && matches!(attrs.delim, ListNumberDelim::DefaultDelim);
        let markers: Vec<String> = (0..items.len())
            .map(|offset| {
                if auto_enumerated {
                    "#.".to_string()
                } else {
                    let number = attrs.start.saturating_add(offset_as_i32(offset));
                    ordered_marker(number, attrs.style, attrs.delim)
                }
            })
            .collect();
        let field = markers.iter().map(|m| m.chars().count()).max().unwrap_or(0) + 1;
        let rest = " ".repeat(field);
        let mut units = Vec::new();
        for (offset, item) in items.iter().enumerate() {
            let marker = markers.get(offset).cloned().unwrap_or_default();
            let first = pad_marker(&marker, field);
            let simple = is_simple_item(item);
            let body = self.item_body(item, width.saturating_sub(field));
            units.push((simple, indent_block(&body, &first, &rest)));
        }
        join_loose_items(units)
    }

    pub(super) fn definition_list(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        let mut groups = Vec::new();
        for (term, definitions) in items {
            let term_line = self.term_line(term);
            let mut def_units = Vec::new();
            for definition in definitions {
                let simple = matches!(definition.as_slice(), [Block::Plain(_)]);
                let body = self.blocks_to_string(definition, width.saturating_sub(3), false);
                let body = lead_quote_fence(definition, body);
                let indented = if body.is_empty() {
                    String::new()
                } else {
                    indent_block(&body, "   ", "   ")
                };
                def_units.push((simple, indented));
            }
            let group_simple = definitions
                .iter()
                .all(|definition| matches!(definition.as_slice(), [Block::Plain(_)]));
            let bodies = join_loose_items(def_units);
            let group = if term_line.is_empty() {
                bodies
            } else if bodies.is_empty() {
                term_line
            } else {
                format!("{term_line}\n{bodies}")
            };
            groups.push((group_simple, group));
        }
        join_loose_items(groups)
    }

    pub(super) fn figure(
        &mut self,
        attr: &Attr,
        caption: &Caption,
        blocks: &[Block],
        width: usize,
    ) -> String {
        let image = find_image(blocks);
        let url = image
            .map(|(_, _, target)| target.url.clone())
            .unwrap_or_default();
        let mut directive = format!(".. figure:: {url}");
        if !attr.id.is_empty() {
            directive.push_str("\n   name: ");
            directive.push_str(&attr.id);
        }
        if let Some((image_attr, alt, target)) = image {
            let alt_text = to_plain_text(alt);
            let alt_text = if alt_text.is_empty() {
                target.title.to_string()
            } else {
                alt_text
            };
            if !alt_text.is_empty() {
                directive.push_str("\n   :alt: ");
                directive.push_str(&alt_text);
            }
            for option in dimension_options(image_attr) {
                directive.push_str("\n   ");
                directive.push_str(&option);
            }
        }
        let caption_text = self.blocks_to_string(&caption.long, width.saturating_sub(3), false);
        if caption_text.is_empty() {
            directive
        } else {
            format!(
                "{directive}\n\n{}",
                indent_block(&caption_text, "   ", "   ")
            )
        }
    }

    pub(super) fn div(&mut self, attr: &Attr, blocks: &[Block], width: usize) -> String {
        if is_bare_title(attr) {
            return String::new();
        }
        let body = self.blocks_to_string(blocks, width.saturating_sub(3), false);
        let body = lead_quote_fence(blocks, body);
        let mut directive = match attr.classes.first() {
            Some(class) if is_admonition(class) => format!(".. {class}::"),
            _ if attr.classes.is_empty() => ".. container::".to_owned(),
            _ => format!(".. container:: {}", attr.classes.join(" ")),
        };
        if !attr.id.is_empty() {
            directive.push_str("\n   :name: ");
            directive.push_str(&attr.id);
        }
        if body.is_empty() {
            return directive;
        }
        let indented = indent_block(&body, "   ", "   ");
        format!("{directive}\n\n{indented}")
    }
}

/// Whether a list item renders to a single line: either it has no content, or its one block is a
/// [`Block::Plain`]. Any other shape spans multiple lines and makes its list loose.
fn is_simple_item(item: &[Block]) -> bool {
    item.is_empty() || matches!(item, [Block::Plain(_)])
}

/// Join already-rendered list items or definition groups. The gap before each unit depends on the
/// one above it: a single-line unit is followed on the next line, a multi-line unit is followed
/// across a blank line. Empty units are dropped and do not influence the gap around them.
pub(super) fn join_loose_items(units: Vec<(bool, String)>) -> String {
    let mut out = String::new();
    let mut previous_simple: Option<bool> = None;
    for (simple, text) in units {
        if text.is_empty() {
            continue;
        }
        if let Some(above_is_simple) = previous_simple {
            out.push_str(if above_is_simple { "\n" } else { "\n\n" });
        }
        out.push_str(&text);
        previous_simple = Some(simple);
    }
    out
}

/// The text placed between two consecutive rendered blocks. A block quote that follows a block whose
/// body is indented or directive-introduced is fenced off with an empty `..` comment, so the quote's
/// indentation is not read as a continuation of the block above. A [`Block::Plain`] sits one newline
/// above a block that can follow it tightly; everything else is separated by a blank line.
pub(super) fn block_separator(previous: &Block, current: &Block) -> &'static str {
    if matches!(current, Block::BlockQuote(_)) && needs_quote_fence(previous) {
        return "\n\n..\n\n";
    }
    if matches!(previous, Block::Plain(_)) && tight_after_plain(current) {
        return "\n";
    }
    "\n\n"
}

/// Prefix an empty `..` comment when an indented container's content opens with a block quote, whose
/// own indentation would otherwise merge into the container's body. Returns `body` unchanged when it
/// is empty or does not begin with a quote.
fn lead_quote_fence(blocks: &[Block], body: String) -> String {
    if !body.is_empty() && matches!(blocks.first(), Some(Block::BlockQuote(_))) {
        format!("..\n\n{body}")
    } else {
        body
    }
}

/// Whether a block quote placed directly below this block would be misread as a continuation of it,
/// requiring an empty `..` comment between them. True for blocks whose rendering ends in indented or
/// directive-introduced content: quotes, literal and directive blocks, lists, and figures.
fn needs_quote_fence(previous: &Block) -> bool {
    match previous {
        Block::BlockQuote(_)
        | Block::CodeBlock(..)
        | Block::BulletList(_)
        | Block::OrderedList(..)
        | Block::DefinitionList(_)
        | Block::Div(..)
        | Block::Table(_)
        | Block::Figure(..) => true,
        Block::RawBlock(format, _) => !format.0.eq_ignore_ascii_case("rst"),
        _ => false,
    }
}

/// Whether a block may sit one newline below a preceding [`Block::Plain`] rather than across a blank
/// line. Lists, rules, and directive-introduced blocks require the blank line; flowing text, literal
/// blocks, quotes, headers, line blocks, and figures do not.
fn tight_after_plain(block: &Block) -> bool {
    match block {
        Block::Para(inlines) => !inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Math(MathType::DisplayMath, _))),
        Block::Plain(_)
        | Block::BlockQuote(_)
        | Block::Header(..)
        | Block::LineBlock(_)
        | Block::Figure(..) => true,
        Block::CodeBlock(attr, _) => code_language(attr).is_none(),
        _ => false,
    }
}

/// Whether a list item whose first block is this one must place its marker on a line of its own,
/// pushing the content down past a blank line. True for blocks that introduce their content through a
/// directive or nested marker (rules, raw passthrough, nested lists, containers, and highlighted
/// literal blocks), none of which can share the marker's line.
fn marker_stands_alone(block: &Block) -> bool {
    match block {
        Block::HorizontalRule
        | Block::BulletList(_)
        | Block::OrderedList(..)
        | Block::DefinitionList(_)
        | Block::Table(_)
        | Block::Div(..) => true,
        Block::CodeBlock(attr, _) => code_language(attr).is_some(),
        Block::RawBlock(format, _) => !format.0.eq_ignore_ascii_case("rst"),
        _ => false,
    }
}

/// Whether a class name maps to a standard RST admonition directive.
fn is_admonition(name: &str) -> bool {
    matches!(
        name,
        "attention"
            | "caution"
            | "danger"
            | "error"
            | "hint"
            | "important"
            | "note"
            | "tip"
            | "warning"
            | "admonition"
    )
}

/// Render a display-math formula as a `.. math::` directive. A single-line formula sits on the
/// directive line; a formula spanning several lines moves to an indented body, each line indented by
/// three spaces with blank lines left empty and trailing blanks dropped.
fn math_directive(tex: &str) -> String {
    if !tex.contains('\n') {
        return format!(".. math:: {}", tex.trim());
    }
    let mut lines: Vec<String> = tex
        .split('\n')
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("   {line}")
            }
        })
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    format!(".. math::\n\n{}", lines.join("\n"))
}

/// Whether any inline is display math, looking through the transparent wrappers (spans and citations)
/// that the writer renders without their own markup.
fn contains_display_math(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::Math(MathType::DisplayMath, _) => true,
        Inline::Span(_, inner) | Inline::Cite(_, inner) => contains_display_math(inner),
        _ => false,
    })
}

/// Lift the children of transparent wrappers (spans and citations) to the top level so any display
/// math they enclose drives the paragraph's `.. math::` directives directly.
fn unwrap_transparent(inlines: &[Inline]) -> Vec<Inline> {
    let mut out = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Span(_, inner) | Inline::Cite(_, inner) => {
                out.extend(unwrap_transparent(inner));
            }
            other => out.push(other.clone()),
        }
    }
    out
}

/// Whether a `Div` is a bare title wrapper: its only attribute is the single class `title`. Such a
/// wrapper carries a title implied by its surrounding directive, so reStructuredText leaves it
/// unwritten.
fn is_bare_title(attr: &Attr) -> bool {
    attr.id.is_empty()
        && attr.attributes.is_empty()
        && attr.classes.len() == 1
        && attr.classes.first().map(Text::as_str) == Some("title")
}

/// The language argument of a code block: its first class that is not the line-numbering flag.
fn code_language(attr: &Attr) -> Option<&str> {
    attr.classes
        .iter()
        .map(Text::as_str)
        .find(|class| *class != "numberLines")
}

pub(super) fn code_block(attr: &Attr, text: &str) -> String {
    let head = match code_language(attr) {
        Some(language) => {
            let mut head = format!(".. code:: {language}");
            if attr.classes.iter().any(|class| class == "numberLines") {
                head.push_str("\n   :number-lines:");
                if let Some(start) = start_from(attr) {
                    head.push(' ');
                    head.push_str(start);
                }
            }
            head
        }
        None => "::".to_owned(),
    };
    literal_directive(&head, text)
}

/// The line-numbering start value carried by a code block's `startFrom` attribute, if any.
fn start_from(attr: &Attr) -> Option<&str> {
    attr.attributes
        .iter()
        .find(|(key, _)| key == "startFrom")
        .map(|(_, value)| value.as_str())
}

/// Render a raw block: content whose format is `rst` passes through verbatim; any other format is
/// wrapped in a `.. raw::` directive naming that format.
pub(super) fn raw_block(format: &Format, text: &str) -> String {
    if format.0.eq_ignore_ascii_case("rst") {
        text.strip_suffix('\n').unwrap_or(text).to_owned()
    } else {
        literal_directive(&format!(".. raw:: {}", format.0), text)
    }
}

/// Render a directive whose body is verbatim text indented three columns under a blank line. Each
/// line of content is indented, blank lines stay empty, and a final line break in the source is
/// dropped; the body keeps any blank lines that remain beyond the two a trailing run would otherwise
/// supply. Content that is empty or only blank lines collapses onto the directive head.
fn literal_directive(head: &str, text: &str) -> String {
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("   {line}")
            }
        })
        .collect();
    let trailing = lines
        .iter()
        .rev()
        .take_while(|line| line.is_empty())
        .count();
    let kept = lines.len().saturating_sub(trailing);
    let Some(body) = lines.get(..kept) else {
        return head.to_owned();
    };
    if body.is_empty() {
        return format!("{head}{}", "\n".repeat(trailing.saturating_sub(1)));
    }
    format!(
        "{head}\n\n{}{}",
        body.join("\n"),
        "\n".repeat(trailing.saturating_sub(2))
    )
}

/// Find the first image among a figure's blocks, returning its attributes, alt inlines, and target.
fn find_image(blocks: &[Block]) -> Option<(&Attr, &Vec<Inline>, &Target)> {
    for block in blocks {
        let (Block::Plain(inlines) | Block::Para(inlines)) = block else {
            continue;
        };
        for inline in inlines {
            if let Inline::Image(attr, alt, target) = inline {
                return Some((attr, alt, target));
            }
        }
    }
    None
}

/// The `:width:`/`:height:` directive options built from an attribute's `width`/`height` entries.
pub(super) fn dimension_options(attr: &Attr) -> Vec<String> {
    let mut options = Vec::new();
    if let Some(value) = attribute_value(attr, "width")
        && let Some(shown) = show_dimension(value)
    {
        options.push(format!(":width: {shown}"));
    }
    if let Some(value) = attribute_value(attr, "height")
        && let Some(shown) = show_dimension(value)
    {
        options.push(format!(":height: {shown}"));
    }
    options
}

/// Normalize a dimension value: a unitless or pixel value is floored to whole pixels; a percentage
/// keeps its number with at least one decimal; a recognized absolute unit renders with trailing
/// zeros trimmed; anything else is dropped.
pub(super) fn show_dimension(value: &str) -> Option<String> {
    match parse_dimension(value)? {
        Dimension::Pixels(count) => Some(format!("{count}px")),
        Dimension::Percent(magnitude) => Some(format_percent_dimension(magnitude)),
        Dimension::Length(magnitude, unit) => Some(format_length_dimension(magnitude, unit)),
    }
}

/// Derive a header's implicit identifier from its inline text: keep alphanumerics, spaces, and
/// `_`/`-`/`.`; lowercase; collapse whitespace runs into single hyphens; drop everything up to the
/// first letter; fall back to `section` when nothing remains. An explicit identifier equal to this is
/// redundant and need not be emitted as a reference target.
fn auto_identifier(inlines: &[Inline]) -> String {
    let identifier = slug(&to_plain_text(inlines));
    if identifier.is_empty() {
        "section".to_owned()
    } else {
        identifier
    }
}

/// The underline glyph for a heading level.
fn heading_char(level: i32) -> char {
    match level {
        1 => '=',
        2 => '-',
        3 => '~',
        4 => '^',
        5 => '\'',
        _ => ' ',
    }
}

/// Split an inline sequence at each element matching `is_separator`, dropping the separators.
fn split_at(inlines: &[Inline], is_separator: impl Fn(&Inline) -> bool) -> Vec<&[Inline]> {
    let mut segments = Vec::new();
    let mut start = 0;
    for (index, inline) in inlines.iter().enumerate() {
        if is_separator(inline) {
            segments.push(inlines.get(start..index).unwrap_or(&[]));
            start = index + 1;
        }
    }
    segments.push(inlines.get(start..).unwrap_or(&[]));
    segments
}
