//! HTML writer: renders the document model to an html5 fragment.
//!
//! Output is a fragment with no trailing newline; the caller appends one.

use std::fmt::Write as _;

use carta_ast::{Attr, Block, Document, Inline};
use carta_core::{MathMethod, MetaVarStyle, Result, WrapMode, Writer, WriterOptions};

use crate::common::{FILL_COLUMN, clean_prefix_len, is_wide};

mod helpers;
mod render;

use self::helpers::{
    header_tag, render_attr_into, render_class_into, render_id_into, render_keyvals_into,
};

/// Renders a document to an html5 fragment.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlWriter;

impl Writer for HtmlWriter {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_with_flavor(
            &document.blocks,
            Flavor::Html5,
            options.wrap,
            fill_width(options),
            math_output(options),
            highlighting(options),
        ))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.html"))
    }

    fn meta_var_style(&self) -> MetaVarStyle {
        MetaVarStyle::Web
    }

    fn numbers_sections_in_body(&self) -> bool {
        true
    }
}

/// Renders a document to an html4 fragment. The html4 dialect uses presentational attributes
/// (`align`, `width`) where html5 uses inline `style`, wraps figures in `<div class="float">`
/// rather than `<figure>`, groups footnotes in a `<div>` rather than a `<section>`, drops the
/// ARIA document roles, and emits non-standard attributes by their bare name rather than under a
/// `data-` prefix.
#[derive(Debug, Default, Clone, Copy)]
pub struct Html4Writer;

impl Writer for Html4Writer {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String> {
        Ok(render_with_flavor(
            &document.blocks,
            Flavor::Html4,
            options.wrap,
            fill_width(options),
            math_output(options),
            highlighting(options),
        ))
    }

    fn default_template(&self) -> Option<&'static str> {
        Some(include_str!("templates/default.html4"))
    }

    fn meta_var_style(&self) -> MetaVarStyle {
        MetaVarStyle::Web
    }

    fn numbers_sections_in_body(&self) -> bool {
        true
    }
}

/// The HTML dialect a render targets. They differ in a handful of element and attribute choices;
/// every divergence is keyed off this value.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Flavor {
    #[default]
    Html5,
    Html4,
    /// The dialect of an html5 slide deck: identical to [`Flavor::Html5`] except that footnote
    /// links carry the deck's in-page navigation prefix on their fragment targets.
    // Constructed only by the slide writer; absent when its feature is sliced out of the build.
    #[cfg_attr(not(feature = "revealjs"), allow(dead_code))]
    Slides,
    /// The XHTML of an EPUB 3 chapter. Follows [`Flavor::Html5`] but wraps each section in a
    /// `<section>` element (hoisting the heading's identifier onto it), and renders footnotes as
    /// `<aside epub:type="footnote">` collected in an `epub:type="footnotes"` section, with the
    /// reference links carrying `epub:type="noteref"`.
    // Constructed only by the EPUB writer; absent when its feature is sliced out of the build.
    #[cfg_attr(not(feature = "epub"), allow(dead_code))]
    Epub3,
    /// The XHTML 1.1 of an EPUB 2 chapter. Follows [`Flavor::Html4`] for its presentational
    /// element and attribute choices, but drops any attribute XHTML 1.1 does not admit, wraps each
    /// section in `<div class="section">`, and renders footnotes as `<div>` items carrying a
    /// leading back-reference link.
    // Constructed only by the EPUB writer; absent when its feature is sliced out of the build.
    #[cfg_attr(not(feature = "epub"), allow(dead_code))]
    Epub2,
}

impl Flavor {
    /// Whether the dialect follows html5's element and attribute choices (as opposed to the
    /// presentational html4 choices).
    fn is_html5_family(self) -> bool {
        matches!(self, Flavor::Html5 | Flavor::Slides | Flavor::Epub3)
    }
}

/// The fragment-target prefix on a footnote link. The slide dialect routes links through the deck's
/// in-page navigation, so its fragments are reached as `#/<id>` rather than `#<id>`.
fn fragment_prefix(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Slides => "#/",
        Flavor::Html5 | Flavor::Html4 | Flavor::Epub3 | Flavor::Epub2 => "#",
    }
}

/// Drives html5 block rendering across a slide deck's frames, gathering every frame's footnotes into
/// one accumulator so they can be emitted as a single trailing section. Each method returns an
/// unreflowed fragment carrying the break sentinels; the caller assembles the slide structure around
/// the fragments and then calls [`fill_slides`] once over the whole document.
// Used by the slide writer; unreferenced when its feature is sliced out of the build.
#[cfg_attr(not(feature = "revealjs"), allow(dead_code))]
pub(crate) struct SlideRenderer {
    state: State,
}

#[cfg_attr(not(feature = "revealjs"), allow(dead_code))]
impl SlideRenderer {
    #[must_use]
    pub(crate) fn new(highlighting: Highlighting) -> Self {
        #[cfg_attr(not(feature = "highlight"), allow(unused_mut))]
        let mut state = State {
            flavor: Flavor::Slides,
            ..State::default()
        };
        #[cfg(feature = "highlight")]
        {
            state.highlighter = highlighting;
        }
        #[cfg(not(feature = "highlight"))]
        let () = highlighting;
        Self { state }
    }

    /// The open tag of a slide's `<section>`: the header's `id`, then a `class` whose value is the
    /// given class words followed by the header's own classes, then the header's key/value pairs. A
    /// titleless slide passes an empty `attr`, yielding the class words alone.
    #[must_use]
    pub(crate) fn section_open(attr: &Attr, class_words: &[&str]) -> String {
        let mut classes: Vec<carta_ast::Text> =
            class_words.iter().map(|word| (*word).into()).collect();
        classes.extend(attr.classes.iter().cloned());
        let mut tag = String::from("<section");
        render_id_into(&mut tag, &attr.id);
        render_class_into(&mut tag, &classes);
        render_keyvals_into(&mut tag, &attr.attributes, Flavor::Slides);
        tag.push('>');
        tag
    }

    /// A slide title rendered as its heading element with the header's classes and key/value pairs
    /// but without its `id` (the `id` belongs to the enclosing `<section>`).
    #[must_use]
    pub(crate) fn title(&mut self, level: i32, attr: &Attr, inlines: &[Inline]) -> String {
        let tag = header_tag(level);
        let titleless = Attr {
            id: carta_ast::Text::default(),
            classes: attr.classes.clone(),
            attributes: attr.attributes.clone(),
        };
        let mut out = format!("<{tag}");
        render_attr_into(&mut out, &titleless, AttrOrder::Header, Flavor::Slides);
        out.push('>');
        self.state.inlines(&mut out, inlines);
        let _ = write!(out, "</{tag}>");
        out
    }

    /// A frame body rendered as an html5 fragment; any footnotes it carries join the accumulator.
    #[must_use]
    pub(crate) fn body(&mut self, blocks: &[Block]) -> String {
        let mut out = String::new();
        self.state.blocks(&mut out, blocks);
        out
    }

    /// The accumulated footnotes as a trailing `<section>`, or `None` when no note was rendered.
    #[must_use]
    pub(crate) fn footnote_section(&self) -> Option<String> {
        let mut out = String::new();
        self.state.push_footnote_section(&mut out);
        // the deck supplies its own separator, so drop the section's leading newline
        let trimmed = out.trim_start_matches('\n');
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }
}

/// Resolve the break sentinels in an assembled slide document, filling inline runs to the fill
/// column, and trim the trailing newlines. Counterpart to the per-frame rendering on
/// [`SlideRenderer`].
// Used by the slide writer; unreferenced when its feature is sliced out of the build.
#[cfg_attr(not(feature = "revealjs"), allow(dead_code))]
#[must_use]
pub(crate) fn fill_slides(assembled: &str, wrap: WrapMode, width: usize) -> String {
    let mut filled = restore(reflow(assembled, wrap, width));
    filled.truncate(filled.trim_end_matches('\n').len());
    filled
}

/// Render a block sequence to an html5 fragment, including the footnote section for any notes the
/// blocks carry, laid out under `wrap` and filled to the default column. The fragment carries no
/// trailing newline.
#[cfg(any(feature = "commonmark", feature = "markdown", feature = "gfm"))]
pub(crate) fn render_fragment(blocks: &[Block], wrap: WrapMode) -> String {
    render_with_flavor(
        blocks,
        Flavor::Html5,
        wrap,
        FILL_COLUMN,
        MathOutput::Delimited,
        no_highlighting(),
    )
}

/// Render a chapter's blocks to the XHTML body fragment of an EPUB page. `epub3` selects the EPUB 3
/// dialect (sectioning `<section>` elements, `<aside>` footnotes); otherwise the EPUB 2 dialect is
/// used. Lines are not wrapped, matching a container whose pages are read by software, and the math
/// presentation follows the chosen renderer. The fragment carries no trailing newline.
// Used by the EPUB writer; unreferenced when its feature is sliced out of the build.
#[cfg(feature = "epub")]
pub(crate) fn render_epub_chapter(
    blocks: &[Block],
    epub3: bool,
    options: &WriterOptions,
) -> String {
    let flavor = if epub3 { Flavor::Epub3 } else { Flavor::Epub2 };
    strip_xml_invalid(render_with_flavor(
        blocks,
        flavor,
        WrapMode::None,
        fill_width(options),
        math_output(options),
        highlighting(options),
    ))
}

/// Render an inline sequence to a single line of EPUB XHTML, for a table-of-contents entry or a
/// title-page field. `epub3` selects the EPUB 3 dialect; breakable spaces collapse to ordinary
/// spaces.
// Used by the EPUB writer; unreferenced when its feature is sliced out of the build.
#[cfg(feature = "epub")]
pub(crate) fn render_epub_inlines(inlines: &[Inline], epub3: bool) -> String {
    let flavor = if epub3 { Flavor::Epub3 } else { Flavor::Epub2 };
    let mut state = State {
        flavor,
        ..State::default()
    };
    let mut out = String::new();
    state.inlines(&mut out, inlines);
    // reflow(None) collapses breakable-space runs to one space; restore first, so a protected control
    // character becomes literal and is dropped as XML-invalid instead of leaking its escape tag
    strip_xml_invalid(restore(reflow(&out, WrapMode::None, FILL_COLUMN)))
}

/// The shared predicate for characters XML 1.0 permits; an EPUB page is XML, so the same rule that
/// keeps the emitter well-formed governs what may survive in a chapter's rendered text.
#[cfg(feature = "epub")]
use carta_core::container::xml::is_xml_char;

/// Drop characters XML forbids from an EPUB page's text. An EPUB chapter is XML, so a stray control
/// character in the source (which no escaping can represent) is removed rather than emitted into a
/// document no reading system can parse. Most text is already clean, so the input is returned intact
/// unless it actually carries a forbidden character.
#[cfg(feature = "epub")]
fn strip_xml_invalid(text: String) -> String {
    if text.chars().all(is_xml_char) {
        text
    } else {
        text.chars().filter(|&ch| is_xml_char(ch)).collect()
    }
}

/// The syntax-highlighting catalog carried into a render: the shared tokenizer, or `None` to leave
/// code blocks plain. When the feature is compiled out this collapses to the unit type, so every
/// render entry point keeps one signature.
#[cfg(feature = "highlight")]
pub(crate) type Highlighting = Option<std::sync::Arc<carta_highlight::Highlighter>>;
#[cfg(not(feature = "highlight"))]
pub(crate) type Highlighting = ();

/// The highlighter a render draws from the writer options.
#[cfg(feature = "highlight")]
pub(crate) fn highlighting(options: &WriterOptions) -> Highlighting {
    options.highlight.highlighter.clone()
}
#[cfg(not(feature = "highlight"))]
pub(crate) fn highlighting(_options: &WriterOptions) -> Highlighting {}

/// A render that colorizes nothing, for a fragment embedded in another format's output.
// Called only by the fragment entry point, which a feature slice can compile out.
#[cfg(feature = "highlight")]
#[cfg_attr(
    not(any(feature = "commonmark", feature = "gfm", feature = "markdown")),
    allow(dead_code)
)]
fn no_highlighting() -> Highlighting {
    None
}
#[cfg(not(feature = "highlight"))]
#[cfg_attr(
    not(any(feature = "commonmark", feature = "gfm", feature = "markdown")),
    allow(dead_code)
)]
fn no_highlighting() -> Highlighting {}

fn render_with_flavor(
    blocks: &[Block],
    flavor: Flavor,
    wrap: WrapMode,
    width: usize,
    math: MathOutput,
    highlighting: Highlighting,
) -> String {
    let mut state = State {
        flavor,
        math,
        ..State::default()
    };
    #[cfg(feature = "highlight")]
    {
        state.highlighter = highlighting;
    }
    #[cfg(not(feature = "highlight"))]
    let () = highlighting;
    let mut out = String::new();
    state.blocks(&mut out, blocks);
    state.push_footnote_section(&mut out);
    let mut filled = restore(reflow(&out, wrap, width));
    filled.truncate(filled.trim_end_matches('\n').len());
    filled
}

/// The column an html writer fills to: the requested width, or the default when none is set.
pub(crate) fn fill_width(options: &WriterOptions) -> usize {
    options.columns.unwrap_or(FILL_COLUMN)
}

/// Which math markup an html writer emits for the chosen renderer. KaTeX reads bare TeX from the
/// span; every other method keeps the delimiters.
fn math_output(options: &WriterOptions) -> MathOutput {
    match options.math_method {
        MathMethod::Katex(_) => MathOutput::Raw,
        MathMethod::Plain | MathMethod::MathJax(_) => MathOutput::Delimited,
    }
}

/// Render an inline sequence to a single line of html, with every breakable space emitted as one
/// ordinary space (no reflow). Exposed for writers that embed inline html in an attribute value.
// Used by the outline writer; unreferenced when its feature is sliced out of the build.
#[cfg_attr(not(feature = "opml"), allow(dead_code))]
pub(crate) fn render_inline_line(inlines: &[Inline]) -> String {
    let mut state = State::default();
    let mut out = String::new();
    state.inlines(&mut out, inlines);
    out.replace([BREAK, SOFT], " ")
}

/// Sentinel marking a breakable inline space while the document is assembled as a flat string.
/// [`reflow`] later turns each into either a single space or a line break to fill to
/// [`FILL_COLUMN`]. A literal `U+0000` from document content is preserved
/// verbatim, so content can legitimately contain this scalar; [`protect_char`] encodes any such
/// occurrence before reflow and [`restore`] decodes it afterwards, keeping the channel unambiguous.
const BREAK: char = '\u{0}';

/// Escape introducer that protects a literal [`BREAK`] (or a literal introducer) appearing in
/// document content from being mistaken for a writer-inserted break during [`reflow`]. `U+0001` is
/// a control scalar the writer never emits structurally; [`protect_char`] encodes and [`restore`]
/// reverses it.
const ESCAPE: char = '\u{1}';

/// Tag following an [`ESCAPE`] introducer that stands for one content `U+0000`. The pair is removed
/// again by [`restore`]; any printable char distinct from [`ESCAPE`] would serve.
const BREAK_TAG: char = '0';

/// Sentinel marking a soft line break from the source, distinct from the breakable space [`BREAK`].
/// [`reflow`] keeps it as a line break when the document preserves its own breaks and otherwise
/// treats it exactly like [`BREAK`]. As with [`BREAK`], a literal `U+0002` from document content is
/// protected by [`protect_char`] and decoded by [`restore`] so the channel stays unambiguous.
const SOFT: char = '\u{2}';

/// Tag following an [`ESCAPE`] introducer that stands for one content `U+0002`, the counterpart of
/// [`BREAK_TAG`] for the [`SOFT`] sentinel.
const SOFT_TAG: char = '2';

/// Zero-width sentinel ending the breakable chunk that a preceding break point measures. It is never
/// rendered and never becomes a space or newline: [`reflow`] drops it. It guards a preformatted
/// region (a `<pre><code>` body) so the verbatim text after it cannot lengthen the chunk weighed
/// when deciding whether the enclosing start tag wraps. A start tag therefore wraps on its own width,
/// independent of however long the preformatted body that follows runs. As with the other sentinels,
/// a literal `U+0003` from document content is protected by [`protect_char`] and decoded by
/// [`restore`].
const FLUSH: char = '\u{3}';

/// Tag following an [`ESCAPE`] introducer that stands for one content `U+0003`, the counterpart of
/// [`BREAK_TAG`] for the [`FLUSH`] sentinel.
const FLUSH_TAG: char = '3';

/// Where an attribute set is being rendered, which selects the field order. Most elements emit
/// `id`, then `class`, then key/value pairs; headers emit `class`, then key/value pairs, then `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttrOrder {
    Standard,
    Header,
}

/// How a `span.math` carries its TeX. A typesetting loader that scans the page for delimited math
/// (MathJax) needs the `\(…\)` / `\[…\]` wrappers; KaTeX reads the bare TeX from the span and so
/// takes [`MathOutput::Raw`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum MathOutput {
    /// The TeX is wrapped in `\(…\)` (inline) or `\[…\]` (display). The default.
    #[default]
    Delimited,
    /// The span carries the bare TeX with no delimiters.
    Raw,
}

/// Carries the footnote bodies accumulated while rendering, so notes can be collected inline and
/// emitted as a section at the end of the document.
#[derive(Debug, Default)]
struct State {
    footnotes: Vec<String>,
    in_anchor: bool,
    flavor: Flavor,
    math: MathOutput,
    /// Sequence number of the code block being rendered, incremented for every code block so a
    /// colorized block that names no identifier can synthesize a stable `cbN` one.
    code_block_id: usize,
    /// The tokenizer for colorizing code blocks, or `None` to leave them plain.
    #[cfg(feature = "highlight")]
    highlighter: Highlighting,
}

/// Class names that select a dedicated HTML element for a [`Inline::Span`] instead of a generic
/// `<span>`. Listed in the precedence used when several apply: the first such class found becomes the
/// outermost element, and any further ones nest inside it.
const SEMANTIC_SPAN_TAGS: [&str; 3] = ["mark", "kbd", "dfn"];

/// Serializing a block or inline tree recurses once per nesting level, so a pathologically deep
/// document could exhaust the thread's stack. Before descending into a nested sequence, the writer
/// checks that at least this much stack headroom remains and grows a fresh segment when it does not,
/// bounding depth by available memory rather than by a single stack segment's size.
const STACK_RED_ZONE: usize = 128 * 1024;
/// Size of each stack segment grown on demand once [`STACK_RED_ZONE`] headroom is unavailable.
const STACK_SEGMENT: usize = 32 * 1024 * 1024;

/// Resolve the break sentinels in an assembled fragment under the document's wrap mode.
///
/// Under [`WrapMode::Auto`] inline content fills to `width` columns with a greedy fill: a break point
/// ([`BREAK`] or [`SOFT`]) becomes a newline when keeping the following chunk on the current line
/// would exceed the fill column, where the chunk is the run of literal text up to the next break
/// point or hard newline. Under [`WrapMode::None`] no break point ever becomes a newline: every one
/// is a space. Under [`WrapMode::Preserve`] a [`SOFT`] (a soft break from the source) becomes a
/// newline while a [`BREAK`] (a breakable space) stays a space, and lines are not reflowed. Hard
/// newlines (block structure) always reset the column; consecutive break points collapse to one.
fn reflow(input: &str, wrap: WrapMode, width: usize) -> String {
    let mut out = String::with_capacity(input.len());
    let mut column = 0usize;
    let mut chars = input.chars();
    while let Some(current) = chars.next() {
        match current {
            '\n' => {
                out.push('\n');
                column = 0;
            }
            FLUSH => {}
            BREAK | SOFT => match wrap {
                // a run of break points is one decision: break only when the next chunk would overflow
                WrapMode::Auto => {
                    while let Some(BREAK | SOFT) = chars.clone().next() {
                        chars.next();
                    }
                    let mut chunk = 0usize;
                    for following in chars.clone() {
                        if following == BREAK
                            || following == SOFT
                            || following == '\n'
                            || following == FLUSH
                        {
                            break;
                        }
                        chunk += char_width(following);
                    }
                    if column + 1 + chunk > width {
                        out.push('\n');
                        column = 0;
                    } else {
                        out.push(' ');
                        column += 1;
                    }
                }
                // a run still collapses to one space: spaces around a vanished inline read as one
                WrapMode::None => {
                    while let Some(BREAK | SOFT) = chars.clone().next() {
                        chars.next();
                    }
                    out.push(' ');
                    column += 1;
                }
                // Preserve keeps each break point: SOFT starts a line, others are literal spaces, none merged
                WrapMode::Preserve if current == SOFT => {
                    out.push('\n');
                    column = 0;
                }
                WrapMode::Preserve => {
                    out.push(' ');
                    column += 1;
                }
            },
            other => {
                out.push(other);
                column += char_width(other);
            }
        }
    }
    out
}

/// Display width of a character in columns: zero for combining marks and control characters, two
/// for wide and fullwidth East Asian characters, one otherwise.
///
/// This uses a Unicode-category zero-width test, distinct from the range-table measure in
/// [`crate::common`] that the plain and LaTeX writers share.
fn char_width(ch: char) -> usize {
    let code = ch as u32;
    // below U+0300 only C0/C1 controls and the soft hyphen are zero-width, so range tests suffice
    if code < 0x0300 {
        let zero_width = code < 0x20 || (0x7F..=0x9F).contains(&code) || code == 0x00AD;
        return usize::from(!zero_width);
    }
    if is_zero_width(ch) {
        return 0;
    }
    if is_wide(code) { 2 } else { 1 }
}

fn is_zero_width(ch: char) -> bool {
    use unicode_general_category::{GeneralCategory, get_general_category};
    matches!(
        get_general_category(ch),
        GeneralCategory::NonspacingMark
            | GeneralCategory::EnclosingMark
            | GeneralCategory::Format
            | GeneralCategory::Control
    )
}

/// Escape `&`, `<`, and `>` to their HTML entities, and additionally `"` when `double_quote` and `'`
/// when `single_quote` is set.
fn escape(text: &str, double_quote: bool, single_quote: bool) -> String {
    let mut out = String::with_capacity(text.len());
    escape_into(&mut out, text, double_quote, single_quote);
    out
}

/// Escape `text` directly into `out`, avoiding the throwaway allocation of [`escape`] on the hot
/// text-run path.
fn escape_into(out: &mut String, text: &str, double_quote: bool, single_quote: bool) {
    let is_trigger = |byte: u8| {
        matches!(byte, b'&' | b'<' | b'>' | 0..=3)
            || (double_quote && byte == b'"')
            || (single_quote && byte == b'\'')
    };
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
        escape_char(ch, double_quote, single_quote, out);
        rest = chars.as_str();
    }
}

/// Escape a single character under the [`escape_into`] policy, appending to `out`.
fn escape_char(ch: char, double_quote: bool, single_quote: bool, out: &mut String) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' if double_quote => out.push_str("&quot;"),
        '\'' if single_quote => out.push_str("&#39;"),
        _ => protect_char(ch, out),
    }
}

/// Encode the assembly sentinels so a literal occurrence in document content survives [`reflow`]
/// unchanged instead of being read as a writer-inserted break; [`restore`] reverses this after
/// reflow runs. Any other character is copied verbatim.
fn protect_char(ch: char, out: &mut String) {
    match ch {
        ESCAPE => {
            out.push(ESCAPE);
            out.push(ESCAPE);
        }
        BREAK => {
            out.push(ESCAPE);
            out.push(BREAK_TAG);
        }
        SOFT => {
            out.push(ESCAPE);
            out.push(SOFT_TAG);
        }
        FLUSH => {
            out.push(ESCAPE);
            out.push(FLUSH_TAG);
        }
        other => out.push(other),
    }
}

/// Protect already-escaped or raw content (raw HTML passthrough) that bypasses [`escape`].
fn protect(text: &str) -> String {
    if !text.contains([ESCAPE, BREAK, SOFT, FLUSH]) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        protect_char(ch, &mut out);
    }
    out
}

/// Reverse [`protect_char`]: collapse each escape sequence left in the reflowed output back to the
/// literal sentinel it stood for. Writer-inserted breaks are already gone (consumed by [`reflow`]),
/// so every remaining introducer marks protected content.
fn restore(text: String) -> String {
    if !text.contains(ESCAPE) {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != ESCAPE {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some(ESCAPE) | None => out.push(ESCAPE),
            Some(BREAK_TAG) => out.push(BREAK),
            Some(SOFT_TAG) => out.push(SOFT),
            Some(FLUSH_TAG) => out.push(FLUSH),
            Some(other) => {
                out.push(ESCAPE);
                out.push(other);
            }
        }
    }
    out
}

/// Escape running text and inline code directly into `out`, leaving both quote characters literal.
fn escape_text_into(out: &mut String, text: &str) {
    escape_into(out, text, false, false);
}

/// Escape an attribute value, where both quote characters must be entity-encoded. The same policy
/// applies to a `<pre><code>` block's body.
fn escape_attr(text: &str) -> String {
    escape(text, true, true)
}

/// Escape an attribute value directly into `out`, the [`escape_attr`] policy without the intermediate
/// allocation.
fn escape_attr_into(out: &mut String, text: &str) {
    escape_into(out, text, true, true);
}

/// Escape a math span's body and turn its spaces into break points, so a long formula wraps at the
/// fill column the way running text does rather than overflowing the line. Both quote characters are
/// entity-encoded so the verbatim formula survives intact for the math renderer.
fn fill_math(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == ' ' {
            out.push(BREAK);
        } else {
            escape_char(ch, true, true, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests;
