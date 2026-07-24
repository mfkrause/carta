//! man writer: renders the document model to roff source for the `man` macro package.
//!
//! Block structure is carried by roff requests (`.SH`/`.SS` headings, `.PP` paragraphs, `.IP`
//! indented items, `.RS`/`.RE` relative-indent groups, `.EX`/`.EE` example blocks, and `.TS`/`.TE`
//! tables). Inline emphasis is conveyed through a font stack rendered as `\f[..]` selectors. Footnotes
//! are pulled out of the flow, numbered, and emitted as a trailing `NOTES` section. Literal text is
//! escaped so roff control and special characters render as themselves, and paragraph text is wrapped
//! to a fill column on its visible width. Output carries no trailing newline; the caller appends one.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Block, Caption, ColWidth, Document, Format, Inline, ListAttributes, MathType,
    QuoteType, Row, Table, Target, to_plain_text,
};
use carta_core::{Result, WrapMode, Writer, WriterOptions};

use crate::common::{
    FILL_COLUMN, GridSlot, RowSpanGrid, clean_prefix_len, display_width, is_known_scheme,
    label_matches_url, ordered_marker,
};

/// Renders a document to roff man source (no trailing newline).
#[derive(Debug, Default, Clone, Copy)]
pub struct ManWriter;

impl Writer for ManWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        let mut state = State {
            width: options.columns.unwrap_or(FILL_COLUMN),
            wrap: options.wrap,
            ..State::default()
        };
        let body = state.blocks(&document.blocks);
        let mut out = body;
        if !state.notes.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(".SH NOTES");
            for (index, note) in state.notes.iter().enumerate() {
                let _ = write!(out, "\n.SS [{}]", index + 1);
                if !note.is_empty() {
                    out.push('\n');
                    out.push_str(note);
                }
            }
        }
        Ok(out.trim_end_matches('\n').to_owned())
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.man"))
    }

    fn flatten_block_metadata(&self) -> bool {
        true
    }
}

/// Writer state threaded through the render: the accumulated footnote bodies, in reference order,
/// the fill column that bounds wrapped prose, and the paragraph layout mode that governs whether
/// filled text wraps to that column.
#[derive(Debug)]
struct State {
    notes: Vec<String>,
    width: usize,
    wrap: WrapMode,
}

impl Default for State {
    fn default() -> Self {
        Self {
            notes: Vec::new(),
            width: FILL_COLUMN,
            wrap: WrapMode::default(),
        }
    }
}

/// The active font attributes, rendered to a `\f[..]` selector. Each emphasis inline pushes its
/// attribute; the selector is computed from whichever are currently active.
#[derive(Debug, Clone, Copy, Default)]
struct Font {
    bold: bool,
    italic: bool,
    mono: bool,
}

impl Font {
    /// The roff font selector for this combination: a leading `C` for monospace, then `B`/`I` for the
    /// active weight and slant, falling back to `R` when neither applies.
    fn code(self) -> String {
        let mut weight = String::new();
        if self.bold {
            weight.push('B');
        }
        if self.italic {
            weight.push('I');
        }
        if self.mono {
            let tail = if weight.is_empty() { "R" } else { &weight };
            format!("C{tail}")
        } else if weight.is_empty() {
            "R".to_owned()
        } else {
            weight
        }
    }
}

/// One unit of laid-out inline content. Text fragments carry their rendered (escaped) form alongside
/// the visible column width used for wrapping, so multi-byte escapes and font selectors do not distort
/// the fill. A `Control` fragment is a run of whole request lines (a link or a forced break) that
/// interrupts the filled flow. A `Display` fragment is one display equation's rendered content, which
/// is set off in its own relative-indent group on its own line.
#[derive(Debug, Clone)]
enum Fragment {
    Text { rendered: String, width: usize },
    Space,
    Control(String),
    Display(String),
}

impl State {
    /// Render a block sequence at the top level or inside an indent group. A paragraph is preceded by
    /// `.PP` unless it immediately follows a heading; every other block carries its own leading
    /// request, so blocks are newline-joined.
    fn blocks(&mut self, items: &[Block]) -> String {
        let mut out = String::new();
        let mut previous_is_header = false;
        for item in items {
            let piece = self.block(item, previous_is_header);
            if piece.is_empty() {
                previous_is_header = matches!(item, Block::Header(..));
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&piece);
            previous_is_header = matches!(item, Block::Header(..));
        }
        out
    }

    fn block(&mut self, value: &Block, suppress_para_marker: bool) -> String {
        match value {
            Block::Plain(items) => self.fill_inlines(items),
            Block::Para(items) => {
                let body = self.fill_inlines(items);
                if suppress_para_marker {
                    body
                } else if body.is_empty() {
                    ".PP".to_owned()
                } else {
                    format!(".PP\n{body}")
                }
            }
            Block::Header(level, _, items) => self.header(*level, items),
            Block::CodeBlock(_, text) => code_block(text),
            Block::RawBlock(format, text) => raw_passthrough(format, text),
            Block::BlockQuote(items) => self.block_quote(items),
            Block::BulletList(items) => self.bullet_list(items),
            Block::OrderedList(attrs, items) => self.ordered_list(attrs, items),
            Block::DefinitionList(items) => self.definition_list(items),
            Block::HorizontalRule => ".PP\n   *   *   *   *   *".to_owned(),
            Block::LineBlock(lines) => self.line_block(lines),
            Block::Table(table) => self.table(table),
            Block::Figure(_, caption, items) => self.figure(caption, items),
            Block::Div(_, items) => self.blocks(items),
        }
    }

    fn header(&mut self, level: i32, items: &[Inline]) -> String {
        let text = self.inline_run(items);
        if level <= 1 {
            format!(".SH {text}")
        } else {
            format!(".SS {text}")
        }
    }

    fn block_quote(&mut self, items: &[Block]) -> String {
        format!(".RS\n{}\n.RE", self.blocks(items))
    }

    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        let mut out = String::from(".PP\n");
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                out.push_str("\n.PD 0\n.P\n.PD\n");
            }
            out.push_str(&self.inline_run(line));
        }
        out
    }

    fn bullet_list(&mut self, items: &[Vec<Block>]) -> String {
        let mut out = String::new();
        for item in items {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&self.list_item("\\(bu", false, 2, item));
        }
        out
    }

    fn ordered_list(&mut self, attrs: &ListAttributes, items: &[Vec<Block>]) -> String {
        let markers = ordered_markers(attrs, items.len());
        let mut out = String::new();
        for (marker, item) in markers.iter().zip(items) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&self.list_item(marker, true, 4, item));
        }
        out
    }

    /// Render one list item: the first block follows the `.IP` marker line directly, every later
    /// block sits inside an `.RS`/`.RE` group indented by `relative` columns. A numbered marker is
    /// quoted (its delimiter could otherwise be read as a request argument); the bullet glyph is not.
    fn list_item(&mut self, marker: &str, quote: bool, relative: usize, item: &[Block]) -> String {
        let field = visible_width(marker) + 1;
        let head = if quote {
            format!(".IP \"{marker}\" {field}")
        } else {
            format!(".IP {marker} {field}")
        };
        match item.split_first() {
            None => head,
            Some((first, rest)) => {
                let mut out = head;
                let first_text = self.block(first, true);
                if !first_text.is_empty() {
                    out.push('\n');
                    out.push_str(&first_text);
                }
                if !rest.is_empty() {
                    let _ = write!(out, "\n.RS {relative}\n{}\n.RE", self.blocks(rest));
                }
                out
            }
        }
    }

    /// Render a definition list: each term sits on a `.TP` line, and its definitions are flattened
    /// into one block run whose first block follows the term directly and whose later blocks continue
    /// under an `.RS`/`.RE` group.
    fn definition_list(&mut self, items: &[(Vec<Inline>, Vec<Vec<Block>>)]) -> String {
        let mut out = String::new();
        for (term, definitions) in items {
            if !out.is_empty() {
                out.push('\n');
            }
            let _ = write!(out, ".TP\n{}", self.inline_run(term));
            let blocks: Vec<Block> = definitions.iter().flatten().cloned().collect();
            if let Some((first, rest)) = blocks.split_first() {
                let first_text = self.block(first, true);
                if !first_text.is_empty() {
                    out.push('\n');
                    out.push_str(&first_text);
                }
                if !rest.is_empty() {
                    let _ = write!(out, "\n.RS\n{}\n.RE", self.blocks(rest));
                }
            }
        }
        out
    }

    fn figure(&mut self, caption: &Caption, items: &[Block]) -> String {
        let mut out = self.blocks(items);
        let caption_text = self.fill_inlines(&caption_inlines(&caption.long));
        if !caption_text.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&caption_text);
        }
        out
    }

    fn table(&mut self, table: &Table) -> String {
        let mut out = String::from(".PP\n");
        let caption = self.caption(&table.caption.long);
        if !caption.is_empty() {
            out.push_str(&caption);
            out.push('\n');
        }
        out.push_str(".TS\ntab(@);\n");
        out.push_str(&column_spec(table));
        out.push_str(".\n");

        let mut grid = RowSpanGrid::new(table.col_specs.len());

        for (index, row) in table.head.rows.iter().enumerate() {
            out.push_str(&self.table_row(row, &mut grid));
            if index == 0 {
                out.push_str("_\n");
            }
        }
        for body in &table.bodies {
            for row in body.head.iter().chain(body.body.iter()) {
                out.push_str(&self.table_row(row, &mut grid));
            }
        }
        for row in &table.foot.rows {
            out.push_str(&self.table_row(row, &mut grid));
        }
        out.push_str(".TE");
        out
    }

    /// Render one table row across the column grid. Each cell's content wraps in a `T{`/`T}` block;
    /// a column covered by a column span or by a row span opened above contributes an empty block.
    fn table_row(&mut self, row: &Row, grid: &mut RowSpanGrid) -> String {
        let mut blocks: Vec<String> = Vec::new();
        for slot in grid.place_slots(&row.cells) {
            match slot {
                GridSlot::Cell(_, cell) => {
                    let body = self.blocks(&cell.content);
                    blocks.push(if body.is_empty() {
                        "T{\nT}".to_owned()
                    } else if cell_opens_with_space(&cell.content) {
                        // roff eats plain leading whitespace; escape it so it survives
                        format!("T{{\n\\ {body}\nT}}")
                    } else {
                        format!("T{{\n{body}\nT}}")
                    });
                }
                GridSlot::Covered => blocks.push("T{\nT}".to_owned()),
            }
        }
        format!("{}\n", blocks.join("@"))
    }

    /// Render a table caption: each paragraph fills at the column, and consecutive paragraphs are
    /// separated by the same zero-distance break used for a forced line break.
    fn caption(&mut self, blocks: &[Block]) -> String {
        let mut parts = Vec::new();
        for block in blocks {
            if let Block::Plain(items) | Block::Para(items) = block {
                parts.push(self.fill_flowed(items));
            }
        }
        parts.join("\n.PD 0\n.P\n.PD\n")
    }

    /// Render inline content as paragraph text, breaking the line after a sentence-ending word. Under
    /// `WrapMode::Auto` the text is filled to the fill column; otherwise the paragraph stays one line.
    fn fill_inlines(&mut self, items: &[Inline]) -> String {
        let fragments = self.fragments(items, Font::default());
        fill(&fragments, self.width, true, self.wrap)
    }

    /// Render inline content as paragraph text without breaking after sentence ends; used where the
    /// surrounding macro reflows the text itself, as in a caption. Under `WrapMode::Auto` the text is
    /// filled to the fill column; otherwise the paragraph stays one line.
    fn fill_flowed(&mut self, items: &[Inline]) -> String {
        let fragments = self.fragments(items, Font::default());
        fill(&fragments, self.width, false, self.wrap)
    }

    /// Render inline content as an unwrapped run (heading, list term, caption fragment). Text and
    /// spaces stay on one line; a control run (a forced break) takes its own lines.
    fn inline_run(&mut self, items: &[Inline]) -> String {
        let mut out = String::new();
        for fragment in self.fragments(items, Font::default()) {
            match fragment {
                Fragment::Text { rendered, .. } => out.push_str(&rendered),
                Fragment::Space => out.push(' '),
                Fragment::Control(text) => {
                    if !out.is_empty() && !out.ends_with('\n') && !text.starts_with("\\c") {
                        out.push('\n');
                    }
                    out.push_str(&text);
                    if !text.ends_with("\\c") {
                        out.push('\n');
                    }
                }
                Fragment::Display(content) => {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&display_group(&content));
                }
            }
        }
        escape_line_start(out.trim_end_matches('\n'))
    }

    fn fragments(&mut self, items: &[Inline], font: Font) -> Vec<Fragment> {
        let mut out = Vec::new();
        for item in items {
            self.fragment(item, font, &mut out);
        }
        out
    }

    fn fragment(&mut self, item: &Inline, font: Font, out: &mut Vec<Fragment>) {
        match item {
            Inline::Str(text) => push_text(out, &escape_text(text)),
            Inline::Space | Inline::SoftBreak => out.push(Fragment::Space),
            Inline::LineBreak => out.push(Fragment::Control(".PD 0\n.P\n.PD".to_owned())),
            // roff man has no underline font, so underlined text falls back to italic.
            Inline::Emph(items) | Inline::Underline(items) => {
                self.styled(out, font, font_with(font, |f| f.italic = !f.italic), items);
            }
            Inline::Strong(items) => {
                self.styled(out, font, font_with(font, |f| f.bold = !f.bold), items);
            }
            Inline::Strikeout(items) => {
                push_text(out, "[STRIKEOUT:");
                out.extend(self.fragments(items, font));
                push_text(out, "]");
            }
            Inline::Superscript(items) => {
                push_text(out, "^");
                out.extend(self.fragments(items, font));
                push_text(out, "^");
            }
            Inline::Subscript(items) => {
                push_text(out, "~");
                out.extend(self.fragments(items, font));
                push_text(out, "~");
            }
            Inline::SmallCaps(items) | Inline::Span(_, items) => {
                out.extend(self.fragments(items, font));
            }
            Inline::Quoted(kind, items) => self.quoted(out, font, kind, items),
            Inline::Cite(_, items) => out.extend(self.fragments(items, font)),
            Inline::Code(_, text) => Self::code(out, font, text),
            Inline::Math(kind, text) => self.math(out, font, kind, text),
            Inline::RawInline(format, text) => {
                if format.0 == "man" {
                    push_text(out, text);
                }
            }
            Inline::Link(_, items, target) => self.link(out, font, items, target),
            Inline::Image(_, alt, target) => self.image(out, font, alt, target),
            Inline::Note(blocks) => {
                let marker = self.record_note(blocks);
                push_text(out, &marker);
            }
        }
    }

    /// Render a font-changing inline: emit a selector for the new font, the children, then a selector
    /// restoring the outer font.
    fn styled(&mut self, out: &mut Vec<Fragment>, outer: Font, inner: Font, items: &[Inline]) {
        push_text(out, &format!("\\f[{}]", inner.code()));
        out.extend(self.fragments(items, inner));
        push_text(out, &format!("\\f[{}]", outer.code()));
    }

    fn quoted(&mut self, out: &mut Vec<Fragment>, font: Font, kind: &QuoteType, items: &[Inline]) {
        let (open, close) = match kind {
            QuoteType::SingleQuote => ("`", "'"),
            QuoteType::DoubleQuote => ("\\(lq", "\\(rq"),
        };
        push_text(out, open);
        out.extend(self.fragments(items, font));
        push_text(out, close);
    }

    fn code(out: &mut Vec<Fragment>, font: Font, text: &str) {
        let mono = font_with(font, |f| f.mono = true);
        push_text(out, &format!("\\f[{}]", mono.code()));
        push_text(out, &escape_text(text));
        push_text(out, &format!("\\f[{}]", font.code()));
    }

    /// A hyperlink renders as a `.UR`/`.UE` (or `.MT`/`.ME` for `mailto:`) pair only when its target
    /// carries a registered URI scheme; otherwise roff cannot address it, so the label text is filled
    /// inline. An autolink (label equal to the bare target) and an empty label both drop the label line.
    fn link(&mut self, out: &mut Vec<Fragment>, font: Font, items: &[Inline], target: &Target) {
        let Some((request, end, address)) = link_request(&target.url) else {
            out.extend(self.fragments(items, font));
            return;
        };
        let label = to_plain_text(items);
        let mut control = format!("\\c\n.{request} {}\n", escape_url(&address));
        if !label.is_empty() && !label_matches_url(&label, &target.url) && label != address {
            control.push_str(&self.fill_inlines(items));
            control.push('\n');
        }
        let _ = write!(control, ".{end} \\c");
        out.push(Fragment::Control(control));
    }

    fn image(&mut self, out: &mut Vec<Fragment>, font: Font, alt: &[Inline], target: &Target) {
        push_text(out, "[IMAGE: ");
        let caption: Vec<Inline> = if alt.is_empty() {
            vec![Inline::Str("image".into())]
        } else {
            alt.to_vec()
        };
        if link_request(&target.url).is_some() {
            self.link(out, font, &caption, target);
        } else {
            out.extend(self.fragments(&caption, font));
        }
        push_text(out, "]");
    }

    /// Render a math expression. A convertible expression lowers to the writer-agnostic inline tree
    /// (italic variables, sub/superscripts via `~`/`^`, unicode symbols and Greek letters), which the
    /// inline machinery above renders with no math-specific code. An expression with no single-line
    /// form is emitted verbatim, wrapped in the delimiters of its kind (`$…$` inline, `$$…$$` display)
    /// and roff-escaped like ordinary text. A display equation is set off on its own indented line.
    fn math(&mut self, out: &mut Vec<Fragment>, font: Font, kind: &MathType, text: &str) {
        let content = match crate::math::to_inlines(text) {
            Some(inlines) => self.fragments(&inlines, font),
            None if text.trim().is_empty() => Vec::new(),
            None => {
                let delimiter = match kind {
                    MathType::InlineMath => "$",
                    MathType::DisplayMath => "$$",
                };
                vec![Fragment::Text {
                    rendered: escape_text(&format!("{delimiter}{text}{delimiter}")),
                    width: 0,
                }]
            }
        };
        match kind {
            MathType::InlineMath => out.extend(content),
            MathType::DisplayMath => out.push(Fragment::Display(flatten_fragments(&content))),
        }
    }

    /// Record a footnote: reserve its slot before rendering so nested notes number after it, then fill
    /// the slot with the rendered body and return the inline `[n]` reference marker.
    fn record_note(&mut self, blocks: &[Block]) -> String {
        let index = self.notes.len();
        self.notes.push(String::new());
        let body = self.blocks(blocks);
        if let Some(slot) = self.notes.get_mut(index) {
            *slot = body;
        }
        format!("[{}]", index + 1)
    }
}

/// Whether a table cell's sole paragraph opens with a space inline, which must render as an escaped
/// space to stay visible.
fn cell_opens_with_space(content: &[Block]) -> bool {
    matches!(
        content,
        [Block::Plain(inlines) | Block::Para(inlines)]
            if matches!(inlines.first(), Some(Inline::Space))
    )
}

/// Protect a link target for a roff macro argument: only the escape character itself must be guarded,
/// since URI punctuation carries no roff meaning in this position.
fn escape_url(url: &str) -> String {
    url.replace('\\', "\\(rs")
}

/// Split a link target into the roff macro pair and the address it carries, or `None` when the target
/// has no registered URI scheme. A `mailto:` target maps to the mail macros with the scheme stripped.
fn link_request(url: &str) -> Option<(&'static str, &'static str, String)> {
    let scheme = url.split_once(':').map(|(scheme, _)| scheme)?;
    if !is_known_scheme(scheme) {
        return None;
    }
    if scheme.eq_ignore_ascii_case("mailto") {
        let address = url.split_once(':').map_or("", |(_, rest)| rest).to_owned();
        return Some(("MT", "ME", address));
    }
    Some(("UR", "UE", url.to_owned()))
}

fn font_with(base: Font, apply: impl Fn(&mut Font)) -> Font {
    let mut font = base;
    apply(&mut font);
    font
}

/// Push a fragment of already-rendered source as a fillable run. The fill column is measured against
/// the rendered (escaped) form, so an escape or font selector counts its full source length toward the
/// wrap, joining to its neighbours as one unbreakable word.
fn push_text(out: &mut Vec<Fragment>, rendered: &str) {
    if rendered.is_empty() {
        return;
    }
    out.push(Fragment::Text {
        rendered: rendered.to_owned(),
        width: display_width(rendered),
    });
}

fn display_group(content: &str) -> String {
    if content.is_empty() {
        ".RS\n.RE".to_owned()
    } else {
        format!(".RS\n{content}\n.RE")
    }
}

/// Flatten a fragment run onto a single line: text renders as itself, a space as one blank, and a
/// control run inlines its request lines. Display equation content is laid out on one line without
/// fill, so this collapses the laid-out fragments back to source order.
fn flatten_fragments(fragments: &[Fragment]) -> String {
    let mut out = String::new();
    for fragment in fragments {
        match fragment {
            Fragment::Text { rendered, .. } => out.push_str(rendered),
            Fragment::Space => out.push(' '),
            Fragment::Control(text) | Fragment::Display(text) => out.push_str(text),
        }
    }
    out
}

/// The visible column width of an escaped roff string, counting each `\(xx` special and the soft
/// `\&`/`\ ` escapes as the single glyph (or none) they render. Used to size a list marker's indent
/// field, which is laid out in display columns rather than source characters.
fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            width += display_width(&ch.to_string());
            continue;
        }
        match chars.next() {
            Some('(') => {
                chars.next();
                chars.next();
                width += 1;
            }
            Some('&') => {}
            _ => width += 1,
        }
    }
    width
}

/// The visible glyph string of an escaped roff run: font selectors are dropped, each `\(xx` special
/// and the soft escapes collapse to a single placeholder glyph, and literal characters pass through.
fn visible_text(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('f') => {
                if chars.clone().next() == Some('[') {
                    for next in chars.by_ref() {
                        if next == ']' {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            Some('(') => {
                chars.next();
                chars.next();
                out.push('\u{1}');
            }
            Some('&') | None => {}
            Some(' ') => out.push(' '),
            Some(other) => out.push(other),
        }
    }
    out
}

/// A filled line is broken after a word ending a sentence so roff applies inter-sentence spacing. A
/// word ends a sentence when its visible text closes with `.`, `!`, or `?`, except a lone capital
/// letter followed by that mark (a name initial), which stays joined to what follows.
fn ends_sentence(visible: &str) -> bool {
    match visible.chars().next_back() {
        Some('.' | '!' | '?') => {}
        _ => return false,
    }
    let mut chars = visible.chars();
    let first = chars.next();
    let is_lone_initial =
        visible.chars().count() == 2 && matches!(first, Some(ch) if ch.is_ascii_uppercase());
    !is_lone_initial
}

/// Greedily fill fragments to the fill column, measured on each word's rendered length. A `Control`
/// fragment forces the flow onto its own lines; text resuming after it begins a fresh filled line.
/// Under `WrapMode::Auto` the effective fill column is the fill column; otherwise it is unbounded, so
/// no width-based break is ever taken and each paragraph stays a single physical line (only an
/// explicit hard break, carried as a `Control` fragment, still starts a new line).
fn fill(fragments: &[Fragment], width: usize, sentence_breaks: bool, wrap: WrapMode) -> String {
    let fill_column = if wrap == WrapMode::Auto {
        width
    } else {
        usize::MAX
    };
    let mut filler = Filler {
        at_line_start: true,
        fill_column,
        ..Filler::default()
    };
    for fragment in fragments {
        match fragment {
            Fragment::Text { rendered, width } => filler.push_word(rendered, *width),
            Fragment::Space => filler.space(sentence_breaks),
            Fragment::Control(text) => filler.control(text),
            Fragment::Display(content) => filler.display(content),
        }
    }
    filler.finish()
}

/// Word-wrap state machine. Words accumulate into `word`; `space`/`control` decide where line breaks
/// fall before the pending word is flushed to `out`. `fill_column` is the width a filled line may
/// reach before a space breaks it; setting it to `usize::MAX` disables width-based breaking entirely.
#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct Filler<'a> {
    out: String,
    column: usize,
    fill_column: usize,
    at_line_start: bool,
    pending_space: bool,
    pending_break: bool,
    after_continuation: bool,
    word: Vec<&'a str>,
    word_width: usize,
}

impl<'a> Filler<'a> {
    fn push_word(&mut self, rendered: &'a str, width: usize) {
        self.word.push(rendered);
        self.word_width += width;
    }

    /// A space separator. Consecutive spaces with no word between them (an empty inline collapsed to
    /// nothing) each materialize, so the pending one is emitted literally before the new one is held.
    fn space(&mut self, sentence_breaks: bool) {
        let closes_sentence = sentence_breaks
            && !self.word.is_empty()
            && ends_sentence(&visible_text(&self.word.concat()));
        if self.word.is_empty() && self.pending_space && !self.at_line_start {
            self.out.push(' ');
            self.column += 1;
        }
        self.flush_word();
        self.pending_space = true;
        self.pending_break = closes_sentence;
    }

    fn control(&mut self, text: &str) {
        let space_before = self.pending_space && self.word.is_empty();
        self.flush_word();
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            if !text.starts_with("\\c") {
                self.out.push('\n');
            } else if space_before && !self.out.ends_with(' ') {
                self.out.push(' ');
            }
        }
        self.out.push_str(text);
        self.out.push('\n');
        self.column = 0;
        self.at_line_start = true;
        self.pending_space = false;
        self.pending_break = false;
        self.after_continuation = text.ends_with("\\c");
    }

    /// Set off a display equation on its own line, indented one relative-indent level. The flow breaks
    /// to a fresh line before the group and resumes on the closing `.RE` line, so any following text
    /// continues after it on the same line.
    fn display(&mut self, content: &str) {
        self.flush_word();
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out.push_str(&display_group(content));
        self.column = visible_width(".RE");
        self.at_line_start = false;
        self.pending_space = false;
        self.pending_break = false;
        self.after_continuation = false;
    }

    fn flush_word(&mut self) {
        if self.word.is_empty() {
            return;
        }
        let starts_line = self.open_word_line();
        let opens_request = matches!(
            self.word.first().and_then(|part| part.chars().next()),
            Some('.' | '\'')
        );
        if starts_line && opens_request {
            self.out.push_str("\\&");
        }
        for part in &self.word {
            self.out.push_str(part);
        }
        self.column += self.word_width;
        self.word.clear();
        self.word_width = 0;
        self.pending_space = false;
        self.pending_break = false;
        self.after_continuation = false;
    }

    /// Emit the separator that precedes the pending word and report whether the word opens a line.
    fn open_word_line(&mut self) -> bool {
        if self.after_continuation && self.pending_space {
            self.out.push_str("\\ ");
            self.column = 2;
            self.at_line_start = false;
            true
        } else if self.at_line_start {
            self.at_line_start = false;
            true
        } else if self.pending_break
            || (self.pending_space
                && self
                    .column
                    .saturating_add(1)
                    .saturating_add(self.word_width)
                    > self.fill_column)
        {
            self.out.push('\n');
            self.column = 0;
            true
        } else {
            if self.pending_space {
                self.out.push(' ');
                self.column += 1;
            }
            false
        }
    }

    fn finish(mut self) -> String {
        self.flush_word();
        self.out.trim_end_matches('\n').to_owned()
    }
}

/// Prefix `\&` when a line opens with a control character, unless it already opens a roff request.
fn escape_line_start(line: &str) -> String {
    match line.chars().next() {
        Some('.' | '\'') if !line.starts_with("\\&") => format!("\\&{line}"),
        _ => line.to_owned(),
    }
}

/// The ordered-list markers, each right-padded with leading spaces to the longest marker's width so
/// the delimiters align.
fn ordered_markers(attrs: &ListAttributes, count: usize) -> Vec<String> {
    let raw: Vec<String> = (0..count)
        .map(|offset| {
            let number = attrs
                .start
                .saturating_add(i32::try_from(offset).unwrap_or(i32::MAX));
            ordered_marker(number, attrs.style, attrs.delim)
        })
        .collect();
    let longest = raw
        .iter()
        .map(|marker| display_width(marker))
        .max()
        .unwrap_or(0);
    raw.into_iter()
        .map(|marker| {
            let pad = longest.saturating_sub(display_width(&marker));
            format!("{}{marker}", " ".repeat(pad))
        })
        .collect()
}

/// The column specification line for a table: per column an alignment letter, then any explicit width
/// in `n` units (every column carries a width once any column does).
fn column_spec(table: &Table) -> String {
    let any_width = table
        .col_specs
        .iter()
        .any(|spec| matches!(spec.width, ColWidth::ColWidth(_)));
    let mut parts = Vec::new();
    for spec in &table.col_specs {
        let letter = alignment_letter(&spec.align);
        match spec.width {
            ColWidth::ColWidth(fraction) if any_width => {
                parts.push(format!("{letter}w({}n)", width_units(fraction)));
            }
            _ if any_width => parts.push(format!("{letter}w(0.0n)")),
            _ => parts.push(letter.to_owned()),
        }
    }
    parts.join(" ")
}

/// A column width fraction expressed in `n` units, to one decimal place. The reference width is the
/// fill column less the inter-column padding the table layout reserves.
fn width_units(fraction: f64) -> String {
    const TABLE_WIDTH: f64 = 70.0;
    format!("{:.1}", fraction * TABLE_WIDTH)
}

fn alignment_letter(align: &Alignment) -> &'static str {
    match align {
        Alignment::AlignRight => "r",
        Alignment::AlignCenter => "c",
        Alignment::AlignLeft | Alignment::AlignDefault => "l",
    }
}

/// A code block: each line escaped and control-character-protected, wrapped in an `.IP`/`.EX`/`.EE`
/// example group.
fn code_block(text: &str) -> String {
    let mut out = String::from(".IP\n.EX");
    if !text.is_empty() {
        for line in text.trim_end_matches('\n').split('\n') {
            out.push('\n');
            out.push_str(&escape_line_start(&escape_text(line)));
        }
    }
    out.push_str("\n.EE");
    out
}

fn raw_passthrough(format: &Format, text: &str) -> String {
    if format.0 == "man" {
        text.trim_end_matches('\n').to_owned()
    } else {
        String::new()
    }
}

/// The block-level caption flattened to inline content for a one-line rendering.
fn caption_inlines(blocks: &[Block]) -> Vec<Inline> {
    blocks
        .iter()
        .flat_map(|block| match block {
            Block::Plain(items) | Block::Para(items) => items.clone(),
            _ => Vec::new(),
        })
        .collect()
}

/// Escape literal text for roff: backslash and roff special characters become their `\(xx` forms or
/// escaped equivalents; typographic punctuation maps to its roff special; everything else passes
/// through as UTF-8.
fn escape_text(text: &str) -> String {
    let is_trigger = |byte: u8| {
        matches!(
            byte,
            b'\\' | b'-' | b'~' | b'^' | b'`' | b'\'' | b'"' | b'@'
        ) || byte >= 0x80
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let clean = clean_prefix_len(rest, is_trigger);
        let Some((head, tail)) = rest.split_at_checked(clean) else {
            out.push_str(rest);
            break;
        };
        out.push_str(head);
        let mut chars = tail.chars();
        let Some(ch) = chars.next() else { break };
        match ch {
            '\\' => out.push_str("\\(rs"),
            '-' => out.push_str("\\-"),
            '~' => out.push_str("\\(ti"),
            '^' => out.push_str("\\(ha"),
            '`' => out.push_str("\\(ga"),
            '\'' => out.push_str("\\(aq"),
            '"' => out.push_str("\\(dq"),
            '@' => out.push_str("\\(at"),
            '\u{2013}' => out.push_str("\\(en"),
            '\u{2014}' => out.push_str("\\(em"),
            '\u{2026}' => out.push_str("\\&..."),
            '\u{2018}' => out.push_str("\\(oq"),
            '\u{2019}' => out.push_str("\\(cq"),
            '\u{201C}' => out.push_str("\\(lq"),
            '\u{201D}' => out.push_str("\\(rq"),
            '\u{00A0}' => out.push_str("\\ "),
            other => out.push(other),
        }
        rest = chars.as_str();
    }
    out
}

#[cfg(test)]
mod tests;
