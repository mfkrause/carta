//! `CommonMark` writer: renders the document model to `CommonMark` text.
//!
//! Inline markup is kept (emphasis, strong, links, inline code, …) and block structure is conveyed
//! with `CommonMark` constructs (ATX headers, fenced or indented code, blockquotes, lists). Inline
//! content is wrapped at a fill column of 72. Constructs `CommonMark` has no native syntax for fall
//! back to inline HTML: underline, strikeout, super/subscript, small caps, and any link, image,
//! span, or div that carries attributes. Output carries no trailing newline; the caller appends one.
//! This format has no public specification.

use carta_ast::{
    Attr, Block, Document, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType,
    Target, Text,
};
use carta_core::{Result, WrapMode, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, NotesHost, Piece, append_notes, escape_html_attr, fill, fill_groups, indent_block,
    is_loose, item_separator, normalize_image_attr, offset_as_i32, ordered_marker, quote_marks,
    render_html_attr, render_html_fragment_attr,
};
use crate::markdown_common::{
    attr_is_empty, atx_heading_marker, autolink, begins_character_reference, begins_named_entity,
    code_span, destination, indent_code, is_autolink_class, is_html_format, is_word_boundary,
    longest_backtick_run, needs_separator, offset_horizontal_rule, push_html, quote_block,
};

/// Renders a document to `CommonMark` text.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommonmarkWriter;

impl Writer for CommonmarkWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let width = options.columns.unwrap_or(FILL_COLUMN);
        let mut state = State {
            wrap: options.wrap,
            width,
            ..State::default()
        };
        let body = state.blocks_to_string(&document.blocks, width, false);
        Ok(append_notes(body, &state.footnotes))
    }

    fn body_ends_with_newline(&self) -> bool {
        true
    }

    // No syntax for a link identifier; an entry carrying one would degrade to raw HTML.
    fn toc_link_anchors(&self) -> bool {
        false
    }
}

/// Carries the footnote bodies accumulated while rendering, so notes can be collected inline and
/// emitted as a section at the end of the document, along with the configured fill width.
#[derive(Debug)]
struct State {
    footnotes: Vec<String>,
    wrap: WrapMode,
    width: usize,
    /// Whether rendering is currently inside a raw-HTML anchor. An anchor cannot contain another
    /// anchor, so a link nested inside one degrades to a `<span>` carrying its attributes.
    in_anchor: bool,
    /// Half-open index ranges into the piece run being built that must wrap as a single atom: a
    /// raw-HTML link fallback, whose opening tag, body, and closing tag fold together rather than
    /// letting the surrounding text wrap between them.
    groups: Vec<(usize, usize)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            footnotes: Vec::new(),
            wrap: WrapMode::default(),
            width: FILL_COLUMN,
            in_anchor: false,
            groups: Vec::new(),
        }
    }
}

impl State {
    /// Render a block sequence, dropping blocks that produce no output. Blocks are separated by a
    /// blank line, except that a [`Block::Plain`] is followed by a single newline and certain
    /// neighbors require an HTML-comment separator (see [`needs_separator`]). This is the layout used
    /// for the document body, blockquotes, divs, list items, and definitions. When `hang` is set the
    /// first non-empty block keeps a space that opens it, so content laid out under a list marker or
    /// block-quote prefix keeps the gap the source put after that prefix.
    fn blocks_to_string(&mut self, blocks: &[Block], width: usize, hang: bool) -> String {
        let mut out = String::new();
        let mut previous: Option<&Block> = None;
        let mut first = true;
        for block in blocks {
            let text = self.block(block, width, hang && first);
            if text.is_empty() {
                continue;
            }
            if let Some(previous) = previous {
                if needs_separator(previous, block) {
                    out.push_str("\n\n<!-- -->\n\n");
                } else if matches!(previous, Block::Plain(_)) {
                    out.push('\n');
                } else {
                    out.push_str("\n\n");
                }
            }
            out.push_str(&text);
            previous = Some(block);
            first = false;
        }
        out
    }

    fn block(&mut self, block: &Block, width: usize, hang: bool) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                let (pieces, groups) = self.pieces(inlines, true);
                fill_groups(&pieces, &groups, width, 0, hang, self.wrap)
            }
            Block::Header(level, _, inlines) => {
                let hashes = atx_heading_marker(*level);
                let text = self.inlines_oneline(inlines, false);
                if text.is_empty() {
                    format!("{hashes} ")
                } else {
                    format!("{hashes} {text}")
                }
            }
            Block::CodeBlock(attr, text) => code_block(attr, text),
            Block::RawBlock(format, text) => {
                if is_html_format(format) {
                    collapse_html_block(text)
                } else {
                    String::new()
                }
            }
            Block::BlockQuote(blocks) => {
                let body = self.blocks_to_string(blocks, width.saturating_sub(2), true);
                quote_block(&body)
            }
            Block::BulletList(items) => self.bullet_list(items, width),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::DefinitionList(items) => self.definition_list(items, width),
            Block::HorizontalRule => "-".repeat(width),
            Block::Div(attr, blocks) => {
                let open = fill(
                    &html_tag_pieces(&format!("<div{}>", render_html_attr(attr))),
                    width,
                    self.wrap,
                );
                let body = self.blocks_to_string(blocks, width, false);
                if body.is_empty() {
                    format!("{open}\n\n</div>")
                } else {
                    format!("{open}\n\n{body}\n\n</div>")
                }
            }
            Block::LineBlock(lines) => self.line_block(lines),
            Block::Figure(..) | Block::Table(_) => collapse_html_block(
                &crate::html::render_fragment(std::slice::from_ref(block), self.wrap),
            ),
        }
    }

    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| self.inlines_oneline(line, true))
            .collect();
        rendered.join("\\\n")
    }

    fn bullet_list(&mut self, items: &[Vec<Block>], width: usize) -> String {
        let loose = is_loose(items);
        let body_width = width.saturating_sub(2);
        let rendered: Vec<String> = items
            .iter()
            .map(|item| {
                let rendered = self.blocks_to_string(item, body_width, true);
                let body = offset_horizontal_rule(item, rendered);
                indent_block(&body, "- ", "  ")
            })
            .collect();
        rendered.join(item_separator(loose))
    }

    fn ordered_list(
        &mut self,
        attrs: &ListAttributes,
        items: &[Vec<Block>],
        width: usize,
    ) -> String {
        let loose = is_loose(items);
        // Only decimal numbering with `.` or `)` exists; other styles and delimiters collapse.
        let delim = match attrs.delim {
            ListNumberDelim::OneParen | ListNumberDelim::TwoParens => ListNumberDelim::OneParen,
            ListNumberDelim::Period | ListNumberDelim::DefaultDelim => ListNumberDelim::Period,
        };
        let rendered: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(offset, item)| {
                let number = attrs.start.saturating_add(offset_as_i32(offset));
                let marker = ordered_marker(number, ListNumberStyle::Decimal, delim);
                let field = (marker.chars().count() + 1).max(4);
                let rendered = self.blocks_to_string(item, width.saturating_sub(field), true);
                let body = offset_horizontal_rule(item, rendered);
                let first = format!("{marker:<field$}");
                let rest = " ".repeat(field);
                indent_block(&body, &first, &rest)
            })
            .collect();
        rendered.join(item_separator(loose))
    }

    fn definition_list(
        &mut self,
        items: &[(Vec<Inline>, Vec<Vec<Block>>)],
        width: usize,
    ) -> String {
        let groups: Vec<String> = items
            .iter()
            .map(|(term, definitions)| {
                let term_line = self.term_line(term);
                let bodies: Vec<String> = definitions
                    .iter()
                    .map(|definition| self.blocks_to_string(definition, width, false))
                    .collect();
                let body = bodies.join("\n\n");
                if body.is_empty() {
                    format!("{term_line}  ")
                } else {
                    format!("{term_line}  \n{body}")
                }
            })
            .collect();
        groups.join("\n\n")
    }

    /// Render a definition-list term, collapsing breakable breaks to spaces but keeping a forced
    /// line break as an actual newline, so a term that spans lines stays split across them.
    fn term_line(&mut self, inlines: &[Inline]) -> String {
        let (pieces, _groups) = self.pieces(inlines, true);
        let mut out = String::new();
        for piece in &pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Space | Piece::Soft => out.push(' '),
                Piece::Hard => out.push('\n'),
            }
        }
        out
    }

    /// Render an inline sequence to a single line, collapsing breakable and forced breaks to spaces.
    /// Used where a construct cannot span lines (headers, line-block lines).
    fn inlines_oneline(&mut self, inlines: &[Inline], line_start: bool) -> String {
        let (pieces, _groups) = self.pieces(inlines, line_start);
        let mut out = String::new();
        for piece in &pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Space | Piece::Soft | Piece::Hard => out.push(' '),
            }
        }
        out
    }

    /// Build the inline pieces and the atomic-group ranges recorded while building them. A footnote
    /// renders its own body mid-build (calling back into here), so the in-progress ranges are set
    /// aside and restored around the nested build rather than cleared.
    fn pieces(
        &mut self,
        inlines: &[Inline],
        line_start: bool,
    ) -> (Vec<Piece>, Vec<(usize, usize)>) {
        let saved = std::mem::take(&mut self.groups);
        let mut out = Vec::new();
        self.extend_pieces(inlines, &mut out, line_start);
        let groups = std::mem::replace(&mut self.groups, saved);
        (out, groups)
    }

    /// Append the inline sequence's pieces to `out`. `line_start` enables block-start escaping for
    /// the first inline (when it is a [`Inline::Str`]). A `Str` ending in `!` immediately before a
    /// link is escaped so the pair is not re-read as an image marker.
    fn extend_pieces(&mut self, inlines: &[Inline], out: &mut Vec<Piece>, line_start: bool) {
        for (position, inline) in inlines.iter().enumerate() {
            let starts = line_start && position == 0;
            if matches!(inline, Inline::Space | Inline::SoftBreak)
                && next_is_para_interrupting_marker(inlines.get(position + 1))
            {
                out.push(Piece::text(" "));
                continue;
            }
            if let Inline::Str(text) = inline
                && let Some(prefix) = text.strip_suffix('!')
                && matches!(inlines.get(position + 1), Some(Inline::Link(..)))
            {
                out.push(Piece::text(format!("{}\\!", escape_str(prefix, starts))));
                continue;
            }
            self.inline(inline, out, starts);
        }
    }

    fn inline(&mut self, inline: &Inline, out: &mut Vec<Piece>, line_start: bool) {
        match inline {
            Inline::Str(text) => out.push(Piece::text(escape_str(text, line_start))),
            Inline::Emph(inlines) => match inlines.as_slice() {
                [Inline::Emph(inner)] => self.extend_pieces(inner, out, line_start),
                _ => self.wrap_markup("*", inlines, out),
            },
            Inline::Strong(inlines) => self.wrap_markup("**", inlines, out),
            Inline::Underline(inlines) => self.wrap_tag("u", inlines, out),
            Inline::Strikeout(inlines) => self.wrap_tag("s", inlines, out),
            Inline::Superscript(inlines) => self.wrap_tag("sup", inlines, out),
            Inline::Subscript(inlines) => self.wrap_tag("sub", inlines, out),
            Inline::SmallCaps(inlines) => {
                push_html(out, "<span class=\"smallcaps\">", self.in_anchor);
                self.extend_pieces(inlines, out, false);
                out.push(Piece::text("</span>"));
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_marks(kind);
                out.push(Piece::text(open.to_string()));
                self.extend_pieces(inlines, out, false);
                out.push(Piece::text(close.to_string()));
            }
            Inline::Cite(_, inlines) => self.extend_pieces(inlines, out, false),
            Inline::Code(_, text) => out.push(Piece::text(code_span(text))),
            Inline::Space => out.push(Piece::Space),
            Inline::SoftBreak => out.push(Piece::Soft),
            Inline::LineBreak => {
                out.push(Piece::text("\\"));
                out.push(Piece::Hard);
            }
            Inline::Math(MathType::DisplayMath, tex) => {
                // Breaks are emitted unconditionally; paragraph-edge breaks are dropped or
                // trimmed later, so the boundary cases self-correct.
                out.push(Piece::Hard);
                self.math_content(&MathType::DisplayMath, tex, out);
                out.push(Piece::Hard);
            }
            Inline::Math(kind @ MathType::InlineMath, tex) => {
                self.math_content(kind, tex, out);
            }
            Inline::RawInline(format, text) => {
                if is_html_format(format) {
                    push_html(out, text, self.in_anchor);
                }
            }
            Inline::Link(attr, inlines, target) => self.link(attr, inlines, target, out),
            Inline::Image(attr, inlines, target) => self.image(attr, inlines, target, out),
            Inline::Span(attr, inlines) => {
                if attr_is_empty(attr) {
                    self.extend_pieces(inlines, out, false);
                } else {
                    push_html(
                        out,
                        &format!("<span{}>", render_html_attr(attr)),
                        self.in_anchor,
                    );
                    self.extend_pieces(inlines, out, false);
                    out.push(Piece::text("</span>"));
                }
            }
            Inline::Note(blocks) => {
                let marker = self.record_note(blocks);
                out.push(Piece::text(marker));
            }
        }
    }

    /// Push the rendered pieces of a math node's content: the converted inline tree when the
    /// expression linearizes, nothing when it is empty, otherwise the verbatim source wrapped in the
    /// kind's `$`/`$$` delimiters and routed through the running-text path so its literal text is
    /// escaped. Inline source has its edge whitespace trimmed before wrapping (interior whitespace
    /// is kept); display source is wrapped as written.
    fn math_content(&mut self, kind: &MathType, tex: &str, out: &mut Vec<Piece>) {
        match crate::math::to_inlines(tex) {
            Some(inlines) => {
                for converted in &inlines {
                    self.inline(converted, out, false);
                }
            }
            None if tex.trim().is_empty() => {}
            None => {
                let (delim, body) = match kind {
                    MathType::DisplayMath => ("$$", tex),
                    MathType::InlineMath => ("$", tex.trim()),
                };
                let fallback = Inline::Str(format!("{delim}{body}{delim}").into());
                self.inline(&fallback, out, false);
            }
        }
    }

    fn wrap_markup(&mut self, marker: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
        if inlines.is_empty() {
            return;
        }
        out.push(Piece::text(marker.to_owned()));
        self.extend_pieces(inlines, out, false);
        out.push(Piece::text(marker.to_owned()));
    }

    fn wrap_tag(&mut self, tag: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
        if inlines.is_empty() {
            return;
        }
        out.push(Piece::text(format!("<{tag}>")));
        self.extend_pieces(inlines, out, false);
        out.push(Piece::text(format!("</{tag}>")));
    }

    fn link(&mut self, attr: &Attr, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
        if self.in_anchor {
            push_html(
                out,
                &format!("<span{}>", render_html_fragment_attr(attr)),
                true,
            );
            self.extend_pieces(inlines, out, false);
            out.push(Piece::text("</span>"));
            return;
        }
        if (attr_is_empty(attr) || is_autolink_class(attr))
            && let Some(autolink) = autolink(inlines, target)
        {
            out.push(Piece::text(autolink));
            return;
        }
        if attr_is_empty(attr) {
            out.push(Piece::text("["));
            self.extend_pieces(inlines, out, false);
            out.push(Piece::text(format!("]({})", destination(target))));
        } else {
            let start = out.len();
            push_html(
                out,
                &format!(
                    "<a href=\"{}\"{}{}>",
                    escape_html_attr(&target.url),
                    render_html_fragment_attr(attr),
                    title_attr(&target.title)
                ),
                true,
            );
            self.in_anchor = true;
            self.extend_pieces(inlines, out, false);
            self.in_anchor = false;
            out.push(Piece::text("</a>"));
            self.groups.push((start, out.len()));
        }
    }

    fn image(&mut self, attr: &Attr, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
        if attr_is_empty(attr) {
            out.push(Piece::text("!["));
            self.extend_pieces(inlines, out, false);
            out.push(Piece::text(format!("]({})", destination(target))));
            return;
        }
        let alt_attr = if inlines.is_empty() {
            String::new()
        } else {
            format!(" alt=\"{}\"", escape_html_attr(&alt_text(inlines)))
        };
        let start = out.len();
        push_html(
            out,
            &format!(
                "<img src=\"{}\"{}{}{alt_attr} />",
                escape_html_attr(&target.url),
                title_attr(&target.title),
                render_html_fragment_attr(&normalize_image_attr(attr)),
            ),
            true,
        );
        // A raw `<img>` tag is one atom so breaks fall between tags; a linked image already
        // belongs to the link's atom.
        if !self.in_anchor {
            self.groups.push((start, out.len()));
        }
    }
}

impl NotesHost for State {
    fn notes(&mut self) -> &mut Vec<String> {
        &mut self.footnotes
    }

    fn render_block(&mut self, block: &Block, width: usize) -> String {
        self.block(block, width, false)
    }

    fn render_offset_paragraph(
        &mut self,
        inlines: &[Inline],
        width: usize,
        initial: usize,
    ) -> String {
        let (pieces, groups) = self.pieces(inlines, true);
        fill_groups(&pieces, &groups, width, initial, false, self.wrap)
    }

    fn base_width(&self) -> usize {
        self.width
    }
}

/// Split a synthesized HTML tag into fill pieces breakable only at the spaces that separate its
/// attributes, never inside a quoted value. Used in place of a single text run so the line filler can
/// wrap a long opening tag at the fill column the way running text wraps.
fn html_tag_pieces(tag: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut word = String::new();
    let mut in_value = false;
    for ch in tag.chars() {
        match ch {
            '"' => {
                in_value = !in_value;
                word.push(ch);
            }
            ' ' if !in_value => {
                if !word.is_empty() {
                    pieces.push(Piece::text(std::mem::take(&mut word)));
                }
                pieces.push(Piece::Space);
            }
            _ => word.push(ch),
        }
    }
    if !word.is_empty() {
        pieces.push(Piece::text(word));
    }
    pieces
}

/// Whether the next inline is a bare `1.` or `1)`: an ordered-list marker whose start number is one,
/// the only ordered marker that can interrupt a paragraph. If such a token began a wrapped
/// continuation line it would be re-read as a list, so the space before it is held non-breakable to
/// keep it on the line with the preceding word.
fn next_is_para_interrupting_marker(inline: Option<&Inline>) -> bool {
    matches!(inline, Some(Inline::Str(text)) if text == "1." || text == "1)")
}

/// Keep an embedded HTML block a single `CommonMark` block: each blank line inside it becomes a
/// line holding a `&#10;` character reference, and a trailing newline is dropped.
fn collapse_html_block(text: &str) -> String {
    let collapsed = text.replace("\n\n", "\n&#10;");
    collapsed
        .strip_suffix('\n')
        .unwrap_or(&collapsed)
        .to_owned()
}

/// A code block: indented four spaces when it carries no attributes, otherwise a backtick-fenced
/// block whose info string is the first class (`CommonMark` cannot express an id or further classes).
fn code_block(attr: &Attr, text: &str) -> String {
    if attr_is_empty(attr) {
        return indent_code(text);
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    let fence = "`".repeat(backtick_fence_len(body));
    let info = attr.classes.first();
    let open = match info {
        Some(class) if !class.is_empty() => format!("{fence} {class}"),
        _ => fence.clone(),
    };
    if body.is_empty() {
        format!("{open}\n{fence}")
    } else {
        format!("{open}\n{body}\n{fence}")
    }
}

/// The backtick run length for a fenced code block: longer than the longest backtick run in the
/// content, and at least three.
fn backtick_fence_len(text: &str) -> usize {
    (longest_backtick_run(text) + 1).max(3)
}

/// The plain-text projection of an inline sequence, used for an image's `alt` attribute.
fn alt_text(inlines: &[Inline]) -> String {
    carta_ast::to_plain_text(inlines)
}

fn title_attr(title: &Text) -> String {
    if title.is_empty() {
        String::new()
    } else {
        format!(" title=\"{}\"", escape_html_attr(title))
    }
}

/// Escape the `CommonMark`-significant characters of running text. The characters that can open inline
/// markup (`` ` ``, `*`, `[`, `]`, `<`, `>`) are escaped everywhere; a leading `#` is escaped so it
/// cannot open an ATX heading; an `&` that opens a valid numeric character reference is escaped so it
/// is not re-read as one; `_` is escaped only at a word boundary, where it could flank emphasis. A `!`
/// directly before a `[` is escaped (leaving the `[` bare) so the pair is not read as an image marker.
/// A backslash that precedes a non-alphanumeric character forms an escape sequence: it is written as
/// `\\` and the following character is consumed. When `line_start` is set, the opening character is
/// additionally escaped if it would begin a block construct (a list item or the delimiter of an
/// ordered-list marker).
fn escape_str(text: &str, line_start: bool) -> String {
    let leading = if line_start {
        leading_escape(text)
    } else {
        None
    };
    let paren_close = if line_start {
        paren_marker_close(text)
    } else {
        None
    };
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<char> = None;
    let mut iter = text.char_indices().peekable();
    while let Some((offset, ch)) = iter.next() {
        if Some(offset) == leading {
            out.push('\\');
            out.push(ch);
            prev = Some(ch);
            continue;
        }
        let next = iter.peek().map(|&(_, following)| following);
        let tail = || text.get(offset..).unwrap_or_default();
        match ch {
            '(' if offset == 0 && paren_close.is_some() => out.push_str("\\("),
            ')' if Some(offset) == paren_close => out.push_str("\\)"),
            '#' if offset == 0 => out.push_str("\\#"),
            '!' if next == Some('[') => out.push_str("\\!"),
            '[' if prev == Some('!') => out.push('['),
            '`' | '*' | '[' | ']' | '<' | '>' => {
                out.push('\\');
                out.push(ch);
            }
            '&' if begins_character_reference(tail()) => out.push_str("\\&"),
            '&' if begins_named_entity(tail()) => out.push_str("\\&"),
            '_' if is_word_boundary(prev, next) => out.push_str("\\_"),
            '\\' => match next {
                Some(following) if following.is_alphanumeric() => out.push('\\'),
                Some(following) => {
                    out.push_str("\\\\");
                    iter.next();
                    prev = Some(following);
                    continue;
                }
                None => out.push_str("\\\\"),
            },
            other => out.push(other),
        }
        prev = Some(ch);
    }
    out
}

/// The byte offset of a leading character that must be escaped because it would otherwise start a
/// block construct: a `#` header marker, a `-`/`+` bullet marker followed by a space, or the `.`/`)`
/// delimiter terminating a leading run of digits that forms an ordered-list marker. The offset always
/// falls inside an ASCII prefix, so it doubles as a character index for comparison.
fn leading_escape(text: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    match first {
        '#' => Some(0),
        '-' | '+' => {
            let followed_by_space = chars.next().is_none_or(|(_, ch)| ch.is_whitespace());
            followed_by_space.then_some(0)
        }
        first if first.is_ascii_digit() => {
            let (delim_offset, delim) = chars.by_ref().find(|(_, ch)| !ch.is_ascii_digit())?;
            if delim_offset > 9 {
                return None;
            }
            if matches!(delim, '.' | ')') && chars.next().is_none_or(|(_, ch)| ch.is_whitespace()) {
                Some(delim_offset)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The byte offset of the `)` that closes a leading ordered-list marker in parenthesized form (`(`
/// then a run of decimal digits then `)`) when that `)` falls within the first ten columns and is
/// followed by whitespace or the end of the text. Both parentheses are escaped so the run is not
/// re-read as a list item. `None` when the text does not open with such a marker. The offset always
/// falls inside an ASCII prefix, so it doubles as a character index.
fn paren_marker_close(text: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    if chars.next()?.1 != '(' {
        return None;
    }
    if !chars.next()?.1.is_ascii_digit() {
        return None;
    }
    let (delim_offset, delim) = chars.by_ref().find(|(_, ch)| !ch.is_ascii_digit())?;
    if delim_offset > 9 || delim != ')' {
        return None;
    }
    chars
        .next()
        .is_none_or(|(_, ch)| ch.is_whitespace())
        .then_some(delim_offset)
}

#[cfg(test)]
mod tests;
