//! `CommonMark` writer: renders the document model to `CommonMark` text.
//!
//! Inline markup is kept (emphasis, strong, links, inline code, …) and block structure is conveyed
//! with `CommonMark` constructs (ATX headers, fenced or indented code, blockquotes, lists). Inline
//! content is wrapped at a fill column of 72. Constructs `CommonMark` has no native syntax for fall
//! back to inline HTML: underline, strikeout, super/subscript, small caps, and any link, image,
//! span, or div that carries attributes. Output carries no trailing newline; the caller appends one.
//! This format has no public specification.

use oxidoc_ast::{Attr, Block, Document, Format, Inline, ListAttributes, Target, Text};
use oxidoc_core::{Result, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, Piece, escape_xml, fill, fill_offset, indent_block, is_known_attribute,
    list_is_tight, offset_as_i32, ordered_marker, quote_marks,
};

/// Renders a document to `CommonMark` text.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommonmarkWriter;

impl Writer for CommonmarkWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let mut state = State::default();
        let body = state.blocks_to_string(&document.blocks, FILL_COLUMN);
        let mut out = body;
        if !state.footnotes.is_empty() {
            let notes = state.footnotes.join("\n\n");
            out = if out.is_empty() {
                notes
            } else {
                format!("{out}\n\n{notes}")
            };
        }
        Ok(out.trim_end_matches('\n').to_owned())
    }
}

/// Carries the footnote bodies accumulated while rendering, so notes can be collected inline and
/// emitted as a section at the end of the document.
#[derive(Debug, Default)]
struct State {
    footnotes: Vec<String>,
}

impl State {
    /// Render a block sequence, dropping blocks that produce no output. Blocks are separated by a
    /// blank line, except that a [`Block::Plain`] is followed by a single newline and certain
    /// neighbors require an HTML-comment separator (see [`needs_separator`]). This is the layout used
    /// for the document body, blockquotes, divs, list items, and definitions.
    fn blocks_to_string(&mut self, blocks: &[Block], width: usize) -> String {
        let mut out = String::new();
        let mut previous: Option<&Block> = None;
        for block in blocks {
            let text = self.block(block, width);
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
        }
        out
    }

    fn block(&mut self, block: &Block, width: usize) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                let pieces = self.pieces(inlines, true);
                fill(&pieces, width)
            }
            Block::Header(level, _, inlines) => {
                let hashes = "#".repeat(usize::try_from((*level).max(1)).unwrap_or(1));
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
                    let collapsed = text.replace("\n\n", "\n&#10;");
                    collapsed
                        .strip_suffix('\n')
                        .unwrap_or(&collapsed)
                        .to_owned()
                } else {
                    String::new()
                }
            }
            Block::BlockQuote(blocks) => {
                let body = self.blocks_to_string(blocks, width.saturating_sub(2));
                quote_block(&body)
            }
            Block::BulletList(items) => self.bullet_list(items, width),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items, width),
            Block::DefinitionList(items) => self.definition_list(items, width),
            Block::HorizontalRule => "-".repeat(FILL_COLUMN),
            Block::Div(attr, blocks) => {
                let body = self.blocks_to_string(blocks, width);
                format!("<div{}>\n\n{body}\n\n</div>", render_attr(attr))
            }
            Block::LineBlock(lines) => self.line_block(lines),
            Block::Figure(_, _, _) => todo!("commonmark writer: render figures as HTML fallback"),
            Block::Table(_) => todo!("commonmark writer: render tables as HTML fallback"),
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
                let rendered = self.blocks_to_string(item, body_width);
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
        let rendered: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(offset, item)| {
                let number = attrs.start.saturating_add(offset_as_i32(offset));
                let marker = ordered_marker(number, &attrs.style, &attrs.delim);
                let field = (marker.chars().count() + 1).max(4);
                let rendered = self.blocks_to_string(item, width.saturating_sub(field));
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
                let term_line = self.inlines_oneline(term, true);
                let bodies: Vec<String> = definitions
                    .iter()
                    .map(|definition| self.blocks_to_string(definition, width))
                    .collect();
                let body = bodies.join("\n\n");
                format!("{term_line}  \n{body}")
            })
            .collect();
        groups.join("\n\n")
    }

    /// Render an inline sequence to a single line, collapsing breakable and forced breaks to spaces.
    /// Used where a construct cannot span lines (headers, line-block lines, definition terms).
    fn inlines_oneline(&mut self, inlines: &[Inline], line_start: bool) -> String {
        let pieces = self.pieces(inlines, line_start);
        let mut out = String::new();
        for piece in &pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Space | Piece::Hard => out.push(' '),
            }
        }
        out
    }

    fn pieces(&mut self, inlines: &[Inline], line_start: bool) -> Vec<Piece> {
        let mut out = Vec::new();
        self.extend_pieces(inlines, &mut out, line_start);
        out
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
                out.push(Piece::Text(" ".to_owned()));
                continue;
            }
            if let Inline::Str(text) = inline
                && let Some(prefix) = text.strip_suffix('!')
                && matches!(inlines.get(position + 1), Some(Inline::Link(..)))
            {
                out.push(Piece::Text(format!("{}\\!", escape_str(prefix, starts))));
                continue;
            }
            self.inline(inline, out, starts);
        }
    }

    fn inline(&mut self, inline: &Inline, out: &mut Vec<Piece>, line_start: bool) {
        match inline {
            Inline::Str(text) => out.push(Piece::Text(escape_str(text, line_start))),
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
                out.push(Piece::Text("<span class=\"smallcaps\">".to_owned()));
                self.extend_pieces(inlines, out, false);
                out.push(Piece::Text("</span>".to_owned()));
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_marks(kind);
                out.push(Piece::Text(open.to_string()));
                self.extend_pieces(inlines, out, false);
                out.push(Piece::Text(close.to_string()));
            }
            Inline::Cite(_, inlines) => self.extend_pieces(inlines, out, false),
            Inline::Code(_, text) => out.push(Piece::Text(code_span(text))),
            Inline::Space | Inline::SoftBreak => out.push(Piece::Space),
            Inline::LineBreak => {
                out.push(Piece::Text("\\".to_owned()));
                out.push(Piece::Hard);
            }
            Inline::Math(_, _) => todo!("commonmark writer: render math"),
            Inline::RawInline(format, text) => {
                if is_html_format(format) {
                    out.push(Piece::Text(text.clone()));
                }
            }
            Inline::Link(attr, inlines, target) => self.link(attr, inlines, target, out),
            Inline::Image(attr, inlines, target) => self.image(attr, inlines, target, out),
            Inline::Span(attr, inlines) => {
                if attr_is_empty(attr) {
                    self.extend_pieces(inlines, out, false);
                } else {
                    out.push(Piece::Text(format!("<span{}>", render_attr(attr))));
                    self.extend_pieces(inlines, out, false);
                    out.push(Piece::Text("</span>".to_owned()));
                }
            }
            Inline::Note(blocks) => self.note(blocks, out),
        }
    }

    fn wrap_markup(&mut self, marker: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
        out.push(Piece::Text(marker.to_owned()));
        self.extend_pieces(inlines, out, false);
        out.push(Piece::Text(marker.to_owned()));
    }

    fn wrap_tag(&mut self, tag: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
        out.push(Piece::Text(format!("<{tag}>")));
        self.extend_pieces(inlines, out, false);
        out.push(Piece::Text(format!("</{tag}>")));
    }

    fn link(&mut self, attr: &Attr, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
        if attr_is_empty(attr) {
            if let Some(autolink) = autolink(inlines, target) {
                out.push(Piece::Text(autolink));
                return;
            }
            out.push(Piece::Text("[".to_owned()));
            self.extend_pieces(inlines, out, false);
            out.push(Piece::Text(format!("]({})", destination(target))));
        } else {
            out.push(Piece::Text(format!(
                "<a href=\"{}\"{}{}>",
                escape_attr(&target.url),
                render_attr(attr),
                title_attr(&target.title)
            )));
            self.extend_pieces(inlines, out, false);
            out.push(Piece::Text("</a>".to_owned()));
        }
    }

    fn image(&mut self, attr: &Attr, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
        if attr_is_empty(attr) {
            out.push(Piece::Text("![".to_owned()));
            self.extend_pieces(inlines, out, false);
            out.push(Piece::Text(format!("]({})", destination(target))));
            return;
        }
        if has_dimension(attr) {
            todo!("commonmark writer: render image dimensions (width/height) as HTML fallback");
        }
        let alt = alt_text(inlines);
        let alt_attr = if alt.is_empty() {
            String::new()
        } else {
            format!(" alt=\"{}\"", escape_attr(&alt))
        };
        out.push(Piece::Text(format!(
            "<img src=\"{}\"{}{}{alt_attr} />",
            escape_attr(&target.url),
            title_attr(&target.title),
            render_attr(attr),
        )));
    }

    fn note(&mut self, blocks: &[Block], out: &mut Vec<Piece>) {
        let index = self.footnotes.len();
        self.footnotes.push(String::new());
        let marker = format!("[{}]", index + 1);
        let field = marker.chars().count() + 1;
        let body = self.note_body(blocks, field);
        let rendered = if body.is_empty() {
            marker.clone()
        } else {
            format!("{marker} {body}")
        };
        if let Some(slot) = self.footnotes.get_mut(index) {
            *slot = rendered;
        }
        out.push(Piece::Text(marker));
    }

    /// Render a footnote's body. The marker the caller prepends shifts only the first line's wrap
    /// point (modeled with `initial`); continuation lines and later blocks sit at the margin.
    fn note_body(&mut self, blocks: &[Block], initial: usize) -> String {
        let rendered = blocks
            .iter()
            .enumerate()
            .map(|(position, block)| {
                let is_plain = matches!(block, Block::Plain(_));
                let text = if position == 0 {
                    self.block_offset(block, FILL_COLUMN, initial)
                } else {
                    self.block(block, FILL_COLUMN)
                };
                (is_plain, text)
            })
            .collect();
        join_loose(rendered)
    }

    /// Render a block whose first line begins `initial` columns in. Only text blocks wrap, so the
    /// offset is meaningful for them alone; other block kinds render at the margin.
    fn block_offset(&mut self, block: &Block, width: usize, initial: usize) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                let pieces = self.pieces(inlines, true);
                fill_offset(&pieces, width, initial)
            }
            other => self.block(other, width),
        }
    }
}

/// Join already-rendered blocks with the document's default blank-line spacing, dropping blocks that
/// produced no output. A [`Block::Plain`] contributes only a single newline (not a blank line)
/// before the next visible block when an empty block falls between them.
fn join_loose(rendered: Vec<(bool, String)>) -> String {
    let mut out = String::new();
    let mut previous_was_plain: Option<bool> = None;
    let mut empty_since_previous = false;
    for (is_plain, text) in rendered {
        if text.is_empty() {
            if previous_was_plain.is_some() {
                empty_since_previous = true;
            }
            continue;
        }
        if let Some(was_plain) = previous_was_plain {
            if was_plain && empty_since_previous {
                out.push('\n');
            } else {
                out.push_str("\n\n");
            }
        }
        out.push_str(&text);
        previous_was_plain = Some(is_plain);
        empty_since_previous = false;
    }
    out
}

fn is_loose(items: &[Vec<Block>]) -> bool {
    !list_is_tight(items)
}

/// Whether an HTML comment must separate two consecutive blocks so the second is not absorbed into
/// the first: two lists of the same kind would merge into one, and an indented code block following
/// a list would read as a continuation of the final item.
fn needs_separator(previous: &Block, current: &Block) -> bool {
    match (previous, current) {
        (Block::BulletList(_), Block::BulletList(_))
        | (Block::OrderedList(..), Block::OrderedList(..)) => true,
        (Block::BulletList(_) | Block::OrderedList(..), Block::CodeBlock(attr, _)) => {
            attr_is_empty(attr)
        }
        _ => false,
    }
}

/// A list item whose first block is a horizontal rule cannot place the rule on the marker line,
/// where it would read as part of the marker; the rule is pushed onto its own line below an empty
/// marker line by prefixing the rendered body with a blank line.
fn offset_horizontal_rule(item: &[Block], body: String) -> String {
    if matches!(item.first(), Some(Block::HorizontalRule)) {
        format!("\n\n{body}")
    } else {
        body
    }
}

/// Whether the next inline is a bare `1.` or `1)`: an ordered-list marker whose start number is one,
/// the only ordered marker that can interrupt a paragraph. If such a token began a wrapped
/// continuation line it would be re-read as a list, so the space before it is held non-breakable to
/// keep it on the line with the preceding word.
fn next_is_para_interrupting_marker(inline: Option<&Inline>) -> bool {
    matches!(inline, Some(Inline::Str(text)) if text == "1." || text == "1)")
}

fn item_separator(loose: bool) -> &'static str {
    if loose { "\n\n" } else { "\n" }
}

/// Prefix every line of a blockquote body with `> ` (a bare `>` on an otherwise empty line).
fn quote_block(body: &str) -> String {
    if body.is_empty() {
        return "> ".to_owned();
    }
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.is_empty() {
            out.push('>');
        } else {
            out.push_str("> ");
            out.push_str(line);
        }
    }
    out
}

/// Whether a raw node targets HTML and should pass its content through verbatim.
fn is_html_format(format: &Format) -> bool {
    matches!(format.0.as_str(), "html" | "html4" | "html5")
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

/// An indented code block: every non-blank line is prefixed with four spaces, blank lines stay
/// empty, and trailing blank lines are dropped. Empty content yields no output.
fn indent_code(text: &str) -> String {
    let body = text.trim_end_matches('\n');
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if !line.is_empty() {
            out.push_str("    ");
            out.push_str(line);
        }
    }
    out
}

/// The backtick run length for a fenced code block: longer than the longest backtick run in the
/// content, and at least three.
fn backtick_fence_len(text: &str) -> usize {
    (longest_backtick_run(text) + 1).max(3)
}

/// An inline code span, delimited by a backtick run one longer than the longest run it contains
/// (at least one). A space pads each side when the content holds a backtick or is space-flanked,
/// so the delimiters and content stay distinct.
fn code_span(text: &str) -> String {
    let max_run = longest_backtick_run(text);
    let fence = "`".repeat((max_run + 1).max(1));
    let needs_padding = max_run > 0
        || (text.starts_with(' ') && text.ends_with(' ') && text.chars().any(|ch| ch != ' '));
    if needs_padding {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

/// The `(url "title")` destination tail of a link or image, with the title omitted when empty.
fn destination(target: &Target) -> String {
    if target.title.is_empty() {
        target.url.clone()
    } else {
        format!("{} \"{}\"", target.url, target.title)
    }
}

/// The angle-bracket autolink form when a link's text is exactly its URL (a URI) or the address of a
/// `mailto:` URL, else `None`.
fn autolink(inlines: &[Inline], target: &Target) -> Option<String> {
    let [Inline::Str(text)] = inlines else {
        return None;
    };
    if &target.url == text && is_uri(text) {
        return Some(format!("<{text}>"));
    }
    if target.url == format!("mailto:{text}") {
        return Some(format!("<{text}>"));
    }
    None
}

/// Whether a string is a bare URI eligible to stand as an angle-bracket autolink: it opens with a
/// recognized scheme and every character is valid in a URI.
fn is_uri(text: &str) -> bool {
    let Some(colon) = text.find(':') else {
        return false;
    };
    text.get(..colon).is_some_and(is_known_scheme) && has_valid_uri_chars(text)
}

/// Whether `scheme` is one of the recognized URI schemes (compared case-insensitively).
fn is_known_scheme(scheme: &str) -> bool {
    let lowered = scheme.to_ascii_lowercase();
    URI_SCHEMES.binary_search(&lowered.as_str()).is_ok()
}

/// Whether every character of `text` is permitted in a URI: any non-ASCII character, the unreserved
/// and reserved ASCII punctuation, an ASCII alphanumeric, or a percent escape (`%` and two hex
/// digits).
fn has_valid_uri_chars(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while let Some(&ch) = chars.get(index) {
        if ch == '%' {
            let valid = chars.get(index + 1).is_some_and(char::is_ascii_hexdigit)
                && chars.get(index + 2).is_some_and(char::is_ascii_hexdigit);
            if !valid {
                return false;
            }
            index += 3;
            continue;
        }
        if !is_uri_char(ch) {
            return false;
        }
        index += 1;
    }
    true
}

/// Whether a single character may appear literally in a URI (percent escapes aside).
fn is_uri_char(ch: char) -> bool {
    if !ch.is_ascii() {
        return true;
    }
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '-' | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '@'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
        )
}

fn attr_is_empty(attr: &Attr) -> bool {
    attr.id.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty()
}

fn has_dimension(attr: &Attr) -> bool {
    attr.attributes
        .iter()
        .any(|(key, _)| matches!(key.as_str(), "width" | "height"))
}

/// The plain-text projection of an inline sequence, used for an image's `alt` attribute.
fn alt_text(inlines: &[Inline]) -> String {
    oxidoc_ast::to_plain_text(inlines)
}

fn title_attr(title: &Text) -> String {
    if title.is_empty() {
        String::new()
    } else {
        format!(" title=\"{}\"", escape_attr(title))
    }
}

/// Render an [`Attr`] to its HTML attribute string (leading space per attribute, empty when blank):
/// `id`, then `class`, then key/value pairs, with unrecognized keys `data-` prefixed.
fn render_attr(attr: &Attr) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if !attr.id.is_empty() {
        let _ = write!(out, " id=\"{}\"", escape_attr(&attr.id));
    }
    if !attr.classes.is_empty() {
        let _ = write!(out, " class=\"{}\"", escape_attr(&attr.classes.join(" ")));
    }
    for (key, value) in &attr.attributes {
        let name = if is_known_attribute(key) {
            key.clone()
        } else {
            format!("data-{key}")
        };
        let _ = write!(out, " {name}=\"{}\"", escape_attr(value));
    }
    out
}

/// Escape an HTML attribute value: `&`, `<`, `>`, and `"` to their entities.
fn escape_attr(text: &str) -> String {
    escape_xml(text, true)
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
    let chars: Vec<char> = text.chars().collect();
    let leading = if line_start {
        leading_escape(&chars)
    } else {
        None
    };
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while let Some(&ch) = chars.get(index) {
        if Some(index) == leading {
            out.push('\\');
            out.push(ch);
            index += 1;
            continue;
        }
        match ch {
            '#' if index == 0 => out.push_str("\\#"),
            '!' if matches!(chars.get(index + 1), Some('[')) => out.push_str("\\!"),
            '[' if index > 0 && chars.get(index - 1) == Some(&'!') => out.push('['),
            '`' | '*' | '[' | ']' | '<' | '>' => {
                out.push('\\');
                out.push(ch);
            }
            '&' if begins_character_reference(&chars, index) => out.push_str("\\&"),
            '&' if begins_named_entity(&chars, index) => out.push_str("\\&"),
            '_' if is_word_boundary(&chars, index) => out.push_str("\\_"),
            '\\' => match chars.get(index + 1) {
                Some(next) if next.is_alphanumeric() => out.push('\\'),
                Some(_) => {
                    out.push_str("\\\\");
                    index += 1;
                }
                None => out.push_str("\\\\"),
            },
            other => out.push(other),
        }
        index += 1;
    }
    out
}

/// Whether an `&` at `index` opens a syntactically valid numeric character reference: `&#` followed by
/// at least one decimal digit, or `&#x`/`&#X` followed by at least one hex digit, terminated by `;`.
fn begins_character_reference(chars: &[char], index: usize) -> bool {
    if chars.get(index) != Some(&'&') || chars.get(index + 1) != Some(&'#') {
        return false;
    }
    let hex = matches!(chars.get(index + 2), Some('x' | 'X'));
    let start = if hex { index + 3 } else { index + 2 };
    let mut pos = start;
    while chars.get(pos).is_some_and(|ch| {
        if hex {
            ch.is_ascii_hexdigit()
        } else {
            ch.is_ascii_digit()
        }
    }) {
        pos += 1;
    }
    pos > start && chars.get(pos) == Some(&';')
}

/// Whether an `&` at `index` opens a valid named character reference: an ASCII letter followed by
/// further ASCII alphanumerics and a `;`, whose name is one the format recognizes.
fn begins_named_entity(chars: &[char], index: usize) -> bool {
    if chars.get(index) != Some(&'&') {
        return false;
    }
    let start = index + 1;
    if !chars.get(start).is_some_and(char::is_ascii_alphabetic) {
        return false;
    }
    let mut pos = start + 1;
    while chars.get(pos).is_some_and(char::is_ascii_alphanumeric) {
        pos += 1;
    }
    if chars.get(pos) != Some(&';') {
        return false;
    }
    let name: String = chars.get(start..pos).unwrap_or_default().iter().collect();
    entity_names::ENTITY_NAMES
        .binary_search(&name.as_str())
        .is_ok()
}

mod entity_names {
    include!(concat!(env!("OUT_DIR"), "/entity_names.rs"));
}

/// The index of a leading character that must be escaped because it would otherwise start a block
/// construct: a `#` header marker, a `-`/`+` bullet marker followed by a space, or the `.`/`)`
/// delimiter terminating a leading run of digits that forms an ordered-list marker.
fn leading_escape(chars: &[char]) -> Option<usize> {
    match chars.first()? {
        '#' => Some(0),
        '-' | '+' if chars.len() == 1 || chars.get(1).is_some_and(|ch| ch.is_whitespace()) => {
            Some(0)
        }
        first if first.is_ascii_digit() => {
            let delim = chars.iter().position(|ch| !ch.is_ascii_digit())?;
            if delim > 9 {
                return None;
            }
            if matches!(chars.get(delim), Some('.' | ')'))
                && chars.get(delim + 1).is_none_or(|ch| ch.is_whitespace())
            {
                Some(delim)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Whether an `_` at `index` sits at a word boundary: at least one neighbor (treating the ends of
/// the run as boundaries) is not alphanumeric, so the `_` could flank emphasis.
fn is_word_boundary(chars: &[char], index: usize) -> bool {
    let before = index
        .checked_sub(1)
        .and_then(|previous| chars.get(previous));
    let after = chars.get(index + 1);
    let alnum = |ch: Option<&char>| ch.is_some_and(|c| c.is_alphanumeric());
    !(alnum(before) && alnum(after))
}
/// Recognized URI schemes, sorted for binary search. A bare URI opens with one of these.
const URI_SCHEMES: &[&str] = &[
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
    "appdata",
    "apt",
    "attachment",
    "aw",
    "barion",
    "beshare",
    "bitcoin",
    "bitcoincash",
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
    "did",
    "dis",
    "dlna-playcontainer",
    "dlna-playsingle",
    "dns",
    "dntp",
    "doi",
    "dtn",
    "dvb",
    "ed2k",
    "ethereum",
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
    "graph",
    "gtalk",
    "h323",
    "ham",
    "hcp",
    "http",
    "https",
    "hxxp",
    "hxxps",
    "hydrazone",
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
    "iris.beep",
    "iris.lwz",
    "iris.xpc",
    "iris.xpcs",
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
    "matrix",
    "message",
    "mid",
    "mms",
    "modem",
    "monero",
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
    "ms-powerpoint",
    "ms-project",
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
    "mvrp",
    "news",
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
    "payto",
    "pkcs11",
    "platform",
    "pop",
    "pres",
    "prospero",
    "proxy",
    "psyc",
    "pwid",
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
    "view-source",
    "vnc",
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
