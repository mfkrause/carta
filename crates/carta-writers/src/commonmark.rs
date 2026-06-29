//! `CommonMark` writer: renders the document model to `CommonMark` text.
//!
//! Inline markup is kept (emphasis, strong, links, inline code, …) and block structure is conveyed
//! with `CommonMark` constructs (ATX headers, fenced or indented code, blockquotes, lists). Inline
//! content is wrapped at a fill column of 72. Constructs `CommonMark` has no native syntax for fall
//! back to inline HTML: underline, strikeout, super/subscript, small caps, and any link, image,
//! span, or div that carries attributes. Output carries no trailing newline; the caller appends one.
//! This format has no public specification.

use carta_ast::{
    Attr, Block, Document, Format, Inline, ListAttributes, ListNumberDelim, ListNumberStyle,
    MathType, Target, Text,
};
use carta_core::{Result, WrapMode, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, NotesHost, Piece, append_notes, escape_attr, fill, fill_hang, fill_offset,
    indent_block, is_known_scheme, is_loose, is_percent_escaped_uri, item_separator,
    normalize_image_attr, offset_as_i32, ordered_marker, quote_marks, render_html_attr,
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

    // `CommonMark` has no syntax for a link's identifier, so a contents entry carrying one would
    // degrade to raw HTML; entries link without a back-reference anchor instead.
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
}

impl Default for State {
    fn default() -> Self {
        Self {
            footnotes: Vec::new(),
            wrap: WrapMode::default(),
            width: FILL_COLUMN,
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
                let pieces = self.pieces(inlines, true);
                if hang {
                    fill_hang(&pieces, width, self.wrap)
                } else {
                    fill(&pieces, width, self.wrap)
                }
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
                let body = self.blocks_to_string(blocks, width, false);
                format!("<div{}>\n\n{body}\n\n</div>", render_html_attr(attr))
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
        // CommonMark numbers ordered lists only in decimal, delimited by a period or a single
        // closing parenthesis; every other numeral style collapses to decimal and a two-parenthesis
        // delimiter collapses to the single closing form.
        let delim = match attrs.delim {
            ListNumberDelim::OneParen | ListNumberDelim::TwoParens => ListNumberDelim::OneParen,
            ListNumberDelim::Period | ListNumberDelim::DefaultDelim => ListNumberDelim::Period,
        };
        let rendered: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(offset, item)| {
                let number = attrs.start.saturating_add(offset_as_i32(offset));
                let marker = ordered_marker(number, &ListNumberStyle::Decimal, &delim);
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
                let term_line = self.inlines_oneline(term, true);
                let bodies: Vec<String> = definitions
                    .iter()
                    .map(|definition| self.blocks_to_string(definition, width, false))
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
                Piece::Space | Piece::Soft | Piece::Hard => out.push(' '),
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
            Inline::Space => out.push(Piece::Space),
            Inline::SoftBreak => out.push(Piece::Soft),
            Inline::LineBreak => {
                out.push(Piece::Text("\\".to_owned()));
                out.push(Piece::Hard);
            }
            Inline::Math(MathType::DisplayMath, tex) => {
                // Display math is set off on its own line: a line break before and after sets it
                // apart from the surrounding text. A break at the paragraph's start is dropped (the
                // line is already fresh) and one at its end is trimmed, so the breaks are emitted
                // unconditionally and the boundary cases self-correct. An adjacent space is absorbed
                // by the break.
                out.push(Piece::Hard);
                self.math_content(&MathType::DisplayMath, tex, out);
                out.push(Piece::Hard);
            }
            Inline::Math(kind @ MathType::InlineMath, tex) => {
                self.math_content(kind, tex, out);
            }
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
                    out.push(Piece::Text(format!("<span{}>", render_html_attr(attr))));
                    self.extend_pieces(inlines, out, false);
                    out.push(Piece::Text("</span>".to_owned()));
                }
            }
            Inline::Note(blocks) => {
                let marker = self.record_note(blocks);
                out.push(Piece::Text(marker));
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
                let fallback = Inline::Str(format!("{delim}{body}{delim}"));
                self.inline(&fallback, out, false);
            }
        }
    }

    fn wrap_markup(&mut self, marker: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
        out.push(Piece::Text(marker.to_owned()));
        self.extend_pieces(inlines, out, false);
        out.push(Piece::Text(marker.to_owned()));
    }

    fn wrap_tag(&mut self, tag: &str, inlines: &[Inline], out: &mut Vec<Piece>) {
        if inlines.is_empty() {
            return;
        }
        out.push(Piece::Text(format!("<{tag}>")));
        self.extend_pieces(inlines, out, false);
        out.push(Piece::Text(format!("</{tag}>")));
    }

    fn link(&mut self, attr: &Attr, inlines: &[Inline], target: &Target, out: &mut Vec<Piece>) {
        if (attr_is_empty(attr) || is_autolink_class(attr))
            && let Some(autolink) = autolink(inlines, target)
        {
            out.push(Piece::Text(autolink));
            return;
        }
        if attr_is_empty(attr) {
            out.push(Piece::Text("[".to_owned()));
            self.extend_pieces(inlines, out, false);
            out.push(Piece::Text(format!("]({})", destination(target))));
        } else {
            out.push(Piece::Text(format!(
                "<a href=\"{}\"{}{}>",
                escape_attr(&target.url),
                render_html_attr(attr),
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
            render_html_attr(&normalize_image_attr(attr)),
        )));
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
        let pieces = self.pieces(inlines, true);
        fill_offset(&pieces, width, initial, self.wrap)
    }

    fn base_width(&self) -> usize {
        self.width
    }
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
/// (at least one). A single space pads each side exactly when the content holds a backtick, so the
/// delimiters and the embedded backtick stay distinct; content that merely has leading or trailing
/// spaces (or is entirely spaces) is wrapped without extra padding.
fn code_span(text: &str) -> String {
    let max_run = longest_backtick_run(text);
    let fence = "`".repeat((max_run + 1).max(1));
    if max_run > 0 {
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
    text.get(..colon).is_some_and(is_known_scheme) && is_percent_escaped_uri(text, true)
}

fn attr_is_empty(attr: &Attr) -> bool {
    attr.id.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty()
}

/// Whether a link's attributes consist solely of the `uri` or `email` class that marks it as an
/// autolink: with no id and no further attributes, such a link is written in angle-bracket form.
fn is_autolink_class(attr: &Attr) -> bool {
    attr.id.is_empty()
        && attr.attributes.is_empty()
        && matches!(attr.classes.as_slice(), [class] if class == "uri" || class == "email")
}

/// The plain-text projection of an inline sequence, used for an image's `alt` attribute.
fn alt_text(inlines: &[Inline]) -> String {
    carta_ast::to_plain_text(inlines)
}

fn title_attr(title: &Text) -> String {
    if title.is_empty() {
        String::new()
    } else {
        format!(" title=\"{}\"", escape_attr(title))
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

/// Whether `text` opens with a syntactically valid numeric character reference: `&#` followed by at
/// least one decimal digit, or `&#x`/`&#X` followed by at least one hex digit, terminated by `;`. The
/// reference syntax is wholly ASCII, so this scans bytes.
fn begins_character_reference(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'&') || bytes.get(1) != Some(&b'#') {
        return false;
    }
    let hex = matches!(bytes.get(2), Some(b'x' | b'X'));
    let start = if hex { 3 } else { 2 };
    let mut pos = start;
    while bytes.get(pos).is_some_and(|byte| {
        if hex {
            byte.is_ascii_hexdigit()
        } else {
            byte.is_ascii_digit()
        }
    }) {
        pos += 1;
    }
    pos > start && bytes.get(pos) == Some(&b';')
}

/// Whether `text` opens with a valid named character reference: an ASCII letter followed by further
/// ASCII alphanumerics and a `;`, whose name is one the format recognizes. The reference syntax is
/// wholly ASCII, so this scans bytes.
fn begins_named_entity(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'&') {
        return false;
    }
    if !bytes.get(1).is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let mut pos = 2;
    while bytes.get(pos).is_some_and(u8::is_ascii_alphanumeric) {
        pos += 1;
    }
    if bytes.get(pos) != Some(&b';') {
        return false;
    }
    let name = text.get(1..pos).unwrap_or_default();
    entity_names::ENTITY_NAMES.binary_search(&name).is_ok()
}

mod entity_names {
    include!(concat!(env!("OUT_DIR"), "/entity_names.rs"));
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

/// Whether an `_` with the given neighbors sits at a word boundary: at least one neighbor (treating
/// the ends of the run as boundaries) is not alphanumeric, so the `_` could flank emphasis.
fn is_word_boundary(before: Option<char>, after: Option<char>) -> bool {
    let alnum = |ch: Option<char>| ch.is_some_and(char::is_alphanumeric);
    !(alnum(before) && alnum(after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::QuoteType;

    fn render(blocks: Vec<Block>) -> String {
        CommonmarkWriter
            .write(
                &Document {
                    blocks,
                    ..Document::default()
                },
                &WriterOptions::default(),
            )
            .unwrap()
    }

    fn para(inlines: Vec<Inline>) -> Block {
        Block::Para(inlines)
    }

    fn str_inlines(text: &str) -> Vec<Inline> {
        vec![Inline::Str(text.to_owned())]
    }

    fn plain_item(text: &str) -> Vec<Block> {
        vec![Block::Plain(str_inlines(text))]
    }

    #[test]
    fn ordered_list_collapses_to_decimal_and_one_paren() {
        let attrs = ListAttributes {
            start: 5,
            style: ListNumberStyle::UpperRoman,
            delim: ListNumberDelim::TwoParens,
        };
        let out = render(vec![Block::OrderedList(
            attrs,
            vec![plain_item("a"), plain_item("b")],
        )]);
        assert!(out.starts_with("5)  a"));
        assert!(out.contains("6)  b"));
    }

    #[test]
    fn ordered_list_period_delimiter_preserved() {
        let attrs = ListAttributes {
            start: 1,
            style: ListNumberStyle::LowerAlpha,
            delim: ListNumberDelim::Period,
        };
        let out = render(vec![Block::OrderedList(attrs, vec![plain_item("x")])]);
        assert!(out.starts_with("1.  x"));
    }

    #[test]
    fn autolink_for_uri_and_mailto() {
        let uri = Target {
            url: "http://example.com".into(),
            title: String::new(),
        };
        assert_eq!(
            autolink(&str_inlines("http://example.com"), &uri),
            Some("<http://example.com>".to_owned())
        );
        let mail = Target {
            url: "mailto:a@b.com".into(),
            title: String::new(),
        };
        assert_eq!(
            autolink(&str_inlines("a@b.com"), &mail),
            Some("<a@b.com>".to_owned())
        );
        let plain = Target {
            url: "http://other".into(),
            title: String::new(),
        };
        assert_eq!(autolink(&str_inlines("text"), &plain), None);
    }

    #[test]
    fn uri_and_scheme_recognition() {
        assert!(is_uri("http://example.com"));
        assert!(!is_uri("noscheme"));
        assert!(!is_uri("bogusscheme:rest"));
        assert!(is_known_scheme("HTTP"));
        assert!(is_known_scheme("mailto"));
        assert!(!is_known_scheme("nope"));
    }

    #[test]
    fn autolink_class_detection() {
        let uri_class = Attr {
            classes: vec!["uri".into()],
            ..Attr::default()
        };
        let email_class = Attr {
            classes: vec!["email".into()],
            ..Attr::default()
        };
        let other = Attr {
            classes: vec!["other".into()],
            ..Attr::default()
        };
        let with_id = Attr {
            id: "x".into(),
            classes: vec!["uri".into()],
            ..Attr::default()
        };
        assert!(is_autolink_class(&uri_class));
        assert!(is_autolink_class(&email_class));
        assert!(!is_autolink_class(&other));
        assert!(!is_autolink_class(&with_id));
    }

    #[test]
    fn link_with_autolink_class_renders_angle_form() {
        let link = Inline::Link(
            Attr {
                classes: vec!["uri".into()],
                ..Attr::default()
            },
            str_inlines("http://example.com"),
            Target {
                url: "http://example.com".into(),
                title: String::new(),
            },
        );
        assert_eq!(render(vec![para(vec![link])]), "<http://example.com>");
    }

    #[test]
    fn attributed_link_falls_back_to_html() {
        let link = Inline::Link(
            Attr {
                id: "l".into(),
                ..Attr::default()
            },
            str_inlines("text"),
            Target {
                url: "/p".into(),
                title: "T".into(),
            },
        );
        let out = render(vec![para(vec![link])]);
        assert!(out.contains("<a href=\"/p\" id=\"l\" title=\"T\">text</a>"));
    }

    #[test]
    fn plain_link_uses_inline_destination() {
        let link = Inline::Link(
            Attr::default(),
            str_inlines("text"),
            Target {
                url: "/p".into(),
                title: "T".into(),
            },
        );
        assert_eq!(render(vec![para(vec![link])]), "[text](/p \"T\")");
    }

    #[test]
    fn consecutive_lists_get_comment_separator() {
        let out = render(vec![
            Block::BulletList(vec![plain_item("a")]),
            Block::BulletList(vec![plain_item("b")]),
        ]);
        assert!(out.contains("<!-- -->"));
    }

    #[test]
    fn plain_followed_by_block_uses_single_newline() {
        let out = render(vec![
            Block::Plain(str_inlines("a")),
            Block::Plain(str_inlines("b")),
        ]);
        assert_eq!(out, "a\nb");
    }

    #[test]
    fn empty_header_keeps_marker() {
        assert_eq!(
            render(vec![Block::Header(2, Attr::default(), vec![])]),
            "## "
        );
    }

    #[test]
    fn raw_html_block_collapses_blank_lines() {
        let out = render(vec![Block::RawBlock(
            Format("html".into()),
            "<p>\n\nx\n".into(),
        )]);
        assert_eq!(out, "<p>\n&#10;x");
    }

    #[test]
    fn empty_blockquote_renders_bare_marker() {
        assert_eq!(quote_block(""), "> ");
        let out = render(vec![Block::BlockQuote(vec![])]);
        assert_eq!(out, "> ");
    }

    #[test]
    fn smallcaps_and_double_emph() {
        assert_eq!(
            render(vec![para(vec![Inline::SmallCaps(str_inlines("x"))])]),
            "<span class=\"smallcaps\">x</span>"
        );
        let double = Inline::Emph(vec![Inline::Emph(str_inlines("x"))]);
        assert_eq!(render(vec![para(vec![double])]), "x");
    }

    #[test]
    fn quoted_inline_uses_glyphs() {
        let quoted = Inline::Quoted(QuoteType::DoubleQuote, str_inlines("x"));
        assert_eq!(render(vec![para(vec![quoted])]), "\u{201c}x\u{201d}");
    }

    #[test]
    fn span_with_attrs_wraps_in_tag() {
        let span = Inline::Span(
            Attr {
                id: "s".into(),
                ..Attr::default()
            },
            str_inlines("x"),
        );
        assert_eq!(render(vec![para(vec![span])]), "<span id=\"s\">x</span>");
    }

    #[test]
    fn image_with_attrs_falls_back_to_html() {
        let image = Inline::Image(
            Attr {
                classes: vec!["c".into()],
                ..Attr::default()
            },
            str_inlines("alt"),
            Target {
                url: "i.png".into(),
                title: "T".into(),
            },
        );
        let out = render(vec![para(vec![image])]);
        assert!(out.contains("<img src=\"i.png\" title=\"T\" class=\"c\" alt=\"alt\" />"));
    }

    #[test]
    fn code_span_pads_only_when_backtick_bearing() {
        // Backtick-free content is wrapped with no padding, whatever its spacing.
        assert_eq!(code_span(""), "``");
        assert_eq!(code_span("plain"), "`plain`");
        assert_eq!(code_span("   "), "`   `");
        assert_eq!(code_span(" x "), "` x `");
        assert_eq!(code_span(" and "), "` and `");
        assert_eq!(code_span(" x"), "` x`");
        assert_eq!(code_span("x "), "`x `");
        // A backtick anywhere forces a single space of padding and a longer fence.
        assert_eq!(code_span("`x"), "`` `x ``");
        assert_eq!(code_span("x`"), "`` x` ``");
        assert_eq!(code_span("`x`"), "`` `x` ``");
        assert_eq!(code_span("a`b"), "`` a`b ``");
        assert_eq!(code_span("a``b"), "``` a``b ```");
        assert_eq!(code_span("`"), "`` ` ``");
        assert_eq!(longest_backtick_run("a``b`c"), 2);
    }

    #[test]
    fn fenced_code_block_with_class() {
        let attr = Attr {
            classes: vec!["rust".into()],
            ..Attr::default()
        };
        assert_eq!(
            render(vec![Block::CodeBlock(attr.clone(), "fn x(){}\n".into())]),
            "``` rust\nfn x(){}\n```"
        );
        assert_eq!(
            render(vec![Block::CodeBlock(attr, String::new())]),
            "``` rust\n```"
        );
    }

    #[test]
    fn indented_code_block_without_attrs() {
        assert_eq!(
            render(vec![Block::CodeBlock(Attr::default(), "a\n\nb\n".into())]),
            "    a\n\n    b"
        );
    }

    #[test]
    fn character_and_named_reference_detection() {
        assert!(begins_character_reference("&#65;"));
        assert!(begins_character_reference("&#x41;"));
        assert!(!begins_character_reference("&#;"));
        assert!(!begins_character_reference("&65;"));
        assert!(begins_named_entity("&amp;"));
        assert!(!begins_named_entity("&notareal;"));
        assert!(!begins_named_entity("&amp"));
    }

    #[test]
    fn escape_str_escapes_markup_and_references() {
        assert_eq!(
            escape_str("a*b`c[d]e<f>", false),
            "a\\*b\\`c\\[d\\]e\\<f\\>"
        );
        assert_eq!(escape_str("&amp;", false), "\\&amp;");
        assert_eq!(escape_str("&#65;", false), "\\&#65;");
        assert_eq!(escape_str("a_b", false), "a_b");
        assert_eq!(escape_str("a _ b", false), "a \\_ b");
        assert_eq!(escape_str("#lead", true), "\\#lead");
    }

    #[test]
    fn leading_escape_finds_block_starters() {
        assert_eq!(leading_escape("#x"), Some(0));
        assert_eq!(leading_escape("- x"), Some(0));
        assert_eq!(leading_escape("+ x"), Some(0));
        assert_eq!(leading_escape("-x"), None);
        assert_eq!(leading_escape("12. x"), Some(2));
        assert_eq!(leading_escape("12) x"), Some(2));
        assert_eq!(leading_escape("12.x"), None);
        assert_eq!(leading_escape("1234567890. x"), None);
        assert_eq!(leading_escape("abc"), None);
    }

    #[test]
    fn word_boundary_for_underscore() {
        assert!(!is_word_boundary(Some('a'), Some('b')));
        assert!(is_word_boundary(Some('a'), Some(' ')));
        assert!(is_word_boundary(None, Some('a')));
    }

    #[test]
    fn code_block_indented_then_list_separates() {
        assert!(needs_separator(
            &Block::BulletList(vec![plain_item("a")]),
            &Block::CodeBlock(Attr::default(), "x".into())
        ));
        assert!(!needs_separator(&Block::Para(vec![]), &Block::Para(vec![])));
    }

    #[test]
    fn destination_and_title_helpers() {
        assert_eq!(
            destination(&Target {
                url: "/p".into(),
                title: String::new()
            }),
            "/p"
        );
        assert_eq!(
            destination(&Target {
                url: "/p".into(),
                title: "T".into()
            }),
            "/p \"T\""
        );
        assert_eq!(title_attr(&String::new()), "");
        assert_eq!(title_attr(&"T".to_owned()), " title=\"T\"");
    }

    fn inline_math(tex: &str) -> Inline {
        Inline::Math(MathType::InlineMath, tex.to_owned())
    }

    fn display_math(tex: &str) -> Inline {
        Inline::Math(MathType::DisplayMath, tex.to_owned())
    }

    #[test]
    fn convertible_math_uses_inline_markup() {
        // Binary operators and relations carry their math spacing (`U+2005` around `+`,
        // `U+2004` around `=`).
        assert_eq!(
            render(vec![para(vec![inline_math("a^2 + b^2 = c^2")])]),
            "*a*<sup>2</sup>\u{2005}+\u{2005}*b*<sup>2</sup>\u{2004}=\u{2004}*c*<sup>2</sup>"
        );
    }

    #[test]
    fn display_math_shares_inline_conversion() {
        // `\,` is a thin space (`U+2006`) in the converted tree.
        assert_eq!(
            render(vec![para(vec![display_math("\\int_0^1 x \\, dx")])]),
            "\u{222b}<sub>0</sub><sup>1</sup>*x*\u{2006}*d**x*"
        );
    }

    #[test]
    fn unconvertible_inline_math_falls_back_to_single_dollars() {
        // The fallback routes the literal through the running-text path, so a word-boundary `_`
        // is escaped while the `$` delimiters stay literal.
        assert_eq!(
            render(vec![para(vec![inline_math("\\sum_{i=1}^n a_i")])]),
            "$\\sum\\_{i=1}^n a_i$"
        );
    }

    #[test]
    fn unconvertible_display_math_falls_back_to_double_dollars() {
        assert_eq!(
            render(vec![para(vec![display_math("\\sqrt{x}")])]),
            "$$\\sqrt{x}$$"
        );
    }

    #[test]
    fn inline_math_fallback_trims_edge_whitespace() {
        // The verbatim inline fallback strips leading and trailing whitespace before wrapping in
        // `$…$`; interior whitespace is preserved.
        assert_eq!(
            render(vec![para(vec![inline_math("\\sqrt{x} ")])]),
            "$\\sqrt{x}$"
        );
        assert_eq!(
            render(vec![para(vec![inline_math(" \\sqrt{x}")])]),
            "$\\sqrt{x}$"
        );
        assert_eq!(
            render(vec![para(vec![inline_math("  \\sqrt{x}  ")])]),
            "$\\sqrt{x}$"
        );
        assert_eq!(
            render(vec![para(vec![inline_math("\\sqrt{x}   y")])]),
            "$\\sqrt{x}   y$"
        );
    }

    #[test]
    fn display_math_fallback_keeps_edge_whitespace() {
        // Display fallback wraps the source as written; only inline math trims its edges.
        assert_eq!(
            render(vec![para(vec![display_math("\\sqrt{x} ")])]),
            "$$\\sqrt{x} $$"
        );
        assert_eq!(
            render(vec![para(vec![display_math(" \\sqrt{x}")])]),
            "$$ \\sqrt{x}$$"
        );
    }

    #[test]
    fn inline_math_fallback_of_lone_backslash_escapes() {
        // A fallback body of a lone backslash wraps to `$\$`, whose backslash the running-text path
        // escapes to `$\\`. A `\ ` whose conversion bails to verbatim trims to this same lone-
        // backslash body, so the trim composes with that bail to reach this end state.
        assert_eq!(render(vec![para(vec![inline_math("\\")])]), "$\\\\");
    }

    #[test]
    fn empty_math_emits_nothing() {
        // Empty or whitespace-only math contributes no output; the flanking spaces collapse.
        let out = render(vec![para(vec![
            Inline::Str("a".into()),
            Inline::Space,
            inline_math("  "),
            Inline::Space,
            Inline::Str("b".into()),
        ])]);
        assert_eq!(out, "a b");
        assert_eq!(render(vec![para(vec![display_math("")])]), "");
    }

    #[test]
    fn figure_renders_as_html_fallback() {
        let caption = carta_ast::Caption {
            short: None,
            long: vec![Block::Plain(str_inlines("a caption"))],
        };
        let image = Inline::Image(
            Attr::default(),
            str_inlines("a caption"),
            Target {
                url: "pic.png".into(),
                title: "fig title".into(),
            },
        );
        let figure = Block::Figure(Attr::default(), caption, vec![Block::Plain(vec![image])]);
        assert_eq!(
            render(vec![figure]),
            "<figure>\n<img src=\"pic.png\" title=\"fig title\" alt=\"a caption\" />\n\
             <figcaption aria-hidden=\"true\">a caption</figcaption>\n</figure>"
        );
    }

    #[test]
    fn dimensioned_image_falls_back_to_html_img() {
        let image = Inline::Image(
            Attr {
                attributes: vec![("width".into(), "200".into())],
                ..Attr::default()
            },
            str_inlines("alt"),
            Target {
                url: "pic.png".into(),
                title: String::new(),
            },
        );
        assert_eq!(
            render(vec![para(vec![image])]),
            "<img src=\"pic.png\" width=\"200\" alt=\"alt\" />"
        );
    }

    #[test]
    fn attrless_image_stays_markdown() {
        let image = Inline::Image(
            Attr::default(),
            str_inlines("alt"),
            Target {
                url: "pic.png".into(),
                title: String::new(),
            },
        );
        assert_eq!(render(vec![para(vec![image])]), "![alt](pic.png)");
    }

    fn dimensioned_image(attributes: Vec<(String, String)>) -> Inline {
        Inline::Image(
            Attr {
                attributes,
                ..Attr::default()
            },
            str_inlines("alt"),
            Target {
                url: "pic.png".into(),
                title: String::new(),
            },
        )
    }

    #[test]
    fn pixel_dimensions_strip_px_and_render_as_attributes() {
        let image = dimensioned_image(vec![("width".into(), "200px".into())]);
        assert_eq!(
            render(vec![para(vec![image])]),
            "<img src=\"pic.png\" width=\"200\" alt=\"alt\" />"
        );
        let both = dimensioned_image(vec![
            ("width".into(), "200".into()),
            ("height".into(), "100".into()),
        ]);
        assert_eq!(
            render(vec![para(vec![both])]),
            "<img src=\"pic.png\" width=\"200\" height=\"100\" alt=\"alt\" />"
        );
    }

    #[test]
    fn percent_and_length_dimensions_become_style() {
        let percent = dimensioned_image(vec![("width".into(), "50%".into())]);
        assert_eq!(
            render(vec![para(vec![percent])]),
            "<img src=\"pic.png\" style=\"width:50.0%\" alt=\"alt\" />"
        );
        let length = dimensioned_image(vec![("width".into(), "5cm".into())]);
        assert_eq!(
            render(vec![para(vec![length])]),
            "<img src=\"pic.png\" style=\"width:5cm\" alt=\"alt\" />"
        );
    }

    #[test]
    fn mixed_pixel_and_style_dimensions_separate_correctly() {
        let image = dimensioned_image(vec![
            ("width".into(), "200".into()),
            ("height".into(), "50%".into()),
        ]);
        assert_eq!(
            render(vec![para(vec![image])]),
            "<img src=\"pic.png\" style=\"height:50.0%\" width=\"200\" alt=\"alt\" />"
        );
    }

    #[test]
    fn unrecognized_dimension_is_dropped() {
        let image = dimensioned_image(vec![("width".into(), "4ex".into())]);
        assert_eq!(
            render(vec![para(vec![image])]),
            "<img src=\"pic.png\" alt=\"alt\" />",
            "the unparsable dimension is dropped but the attributed image keeps its HTML form"
        );
    }

    // The relation `=` carries math spacing (`U+2004`) on each side in the converted inline tree.
    const X_EQ_Y: &str = "*x*\u{2004}=\u{2004}*y*";

    #[test]
    fn display_math_is_set_off_on_its_own_line() {
        let out = render(vec![para(vec![
            Inline::Str("before".into()),
            Inline::Space,
            display_math("x=y"),
            Inline::Space,
            Inline::Str("after".into()),
        ])]);
        assert_eq!(out, format!("before\n{X_EQ_Y}\nafter"));
    }

    #[test]
    fn display_math_breaks_at_paragraph_edges_collapse() {
        // At the paragraph's start the leading break is dropped; at its end the trailing break is
        // trimmed.
        let at_start = render(vec![para(vec![
            display_math("x=y"),
            Inline::Space,
            Inline::Str("after".into()),
        ])]);
        assert_eq!(at_start, format!("{X_EQ_Y}\nafter"));
        let at_end = render(vec![para(vec![
            Inline::Str("before".into()),
            Inline::Space,
            display_math("x=y"),
        ])]);
        assert_eq!(at_end, format!("before\n{X_EQ_Y}"));
        let alone = render(vec![para(vec![display_math("x=y")])]);
        assert_eq!(alone, X_EQ_Y);
    }

    #[test]
    fn inline_math_stays_on_the_line() {
        let out = render(vec![para(vec![
            Inline::Str("before".into()),
            Inline::Space,
            inline_math("x=y"),
            Inline::Space,
            Inline::Str("after".into()),
        ])]);
        assert_eq!(out, format!("before {X_EQ_Y} after"));
    }

    #[test]
    fn unconvertible_display_math_still_breaks_and_falls_back() {
        let out = render(vec![para(vec![
            Inline::Str("before".into()),
            Inline::Space,
            display_math("\\sqrt{x}"),
            Inline::Space,
            Inline::Str("after".into()),
        ])]);
        assert_eq!(out, "before\n$$\\sqrt{x}$$\nafter");
    }

    #[test]
    fn empty_display_math_still_sets_off_a_break() {
        let out = render(vec![para(vec![
            Inline::Str("before".into()),
            Inline::Space,
            display_math("   "),
            Inline::Space,
            Inline::Str("after".into()),
        ])]);
        assert_eq!(out, "before\nafter");
    }

    #[test]
    fn consecutive_display_math_each_take_a_line() {
        let out = render(vec![para(vec![
            Inline::Str("a".into()),
            Inline::Space,
            display_math("x=y"),
            Inline::Space,
            display_math("p=q"),
            Inline::Space,
            Inline::Str("b".into()),
        ])]);
        assert_eq!(out, format!("a\n{X_EQ_Y}\n*p*\u{2004}=\u{2004}*q*\nb"));
    }
}
