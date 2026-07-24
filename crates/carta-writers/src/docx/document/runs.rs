//! Run properties and the lowering of inline sequences and code to runs.

use super::comments::{close_comment, open_comment, tracked_change};
use super::figures::footnote_reference_run;
use super::images::image_drawing_for;
use super::{
    Ctx, DocxHl, clean_bookmark_name, close_bookmark, custom_style, has_class, open_bookmark,
    paragraph_props, style_id,
};
use carta_ast::{Attr, Inline, MathType, QuoteType};
use carta_core::container::xml::Element;
use std::borrow::Cow;

/// A code block: one paragraph in the source-code style. When a highlighter classifies the block's
/// language, each token becomes a run carrying its token style; otherwise each line becomes a run in
/// the plain code character style. Lines are separated by breaks. `numbering` binds the paragraph to
/// a list number when the block sits inside a list item.
#[cfg_attr(not(feature = "highlight"), allow(unused_variables))]
pub(super) fn code_paragraph(
    attr: &Attr,
    code: &str,
    numbering: Option<(u32, u32)>,
    hl: &DocxHl,
) -> Element {
    let mut para = Element::new("w:p");
    para.push(paragraph_props(Some("SourceCode"), numbering, None));
    #[cfg(feature = "highlight")]
    if let Some(runs) = highlighted_code_runs(attr, code, hl) {
        for run in runs {
            para.push(run);
        }
        return para;
    }
    let props = RunProps {
        style: Some(Cow::Borrowed("VerbatimChar")),
        ..RunProps::default()
    };
    let mut first = true;
    for line in code.split('\n') {
        if !first {
            para.push(break_run(&RunProps::default()));
        }
        first = false;
        para.push(text_run(&props, line));
    }
    para
}

/// The token runs for a code block whose language a highlighter recognizes, or `None` when the block
/// carries no recognized language class and should fall back to the plain code style. Every token,
/// including plain and whitespace-only ones, is wrapped in its own styled run, and consecutive
/// source lines are joined by break runs.
#[cfg(feature = "highlight")]
fn highlighted_code_runs(attr: &Attr, code: &str, hl: &DocxHl) -> Option<Vec<Element>> {
    let highlighter = hl.as_ref()?;
    let language = attr
        .classes
        .iter()
        .find(|class| highlighter.registry().is_known(class.as_str()))?;
    let source = code.strip_suffix('\n').unwrap_or(code);
    let lines = highlighter
        .highlight(language.as_str(), source)
        .unwrap_or_default();
    let mut runs = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            runs.push(break_run(&RunProps::default()));
        }
        for token in line {
            let props = RunProps {
                style: Some(Cow::Owned(format!("{}Tok", token.kind.style_key()))),
                ..RunProps::default()
            };
            runs.push(text_run(&props, &token.text));
        }
    }
    Some(runs)
}

/// The token runs for inline code whose language a highlighter recognizes, or `None` when the span
/// carries no recognized language class and should fall back to the plain verbatim run. Each token
/// becomes its own styled run; a deletion context is carried onto every run.
#[cfg(feature = "highlight")]
fn highlighted_inline_runs(
    attr: &Attr,
    text: &str,
    hl: &DocxHl,
    deletion: bool,
) -> Option<Vec<Element>> {
    let highlighter = hl.as_ref()?;
    let language = attr
        .classes
        .iter()
        .find(|class| highlighter.registry().is_known(class.as_str()))?;
    let lines = highlighter
        .highlight(language.as_str(), text)
        .unwrap_or_default();
    let mut runs = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            runs.push(break_run(&RunProps {
                deletion,
                ..RunProps::default()
            }));
        }
        for token in line {
            let props = RunProps {
                style: Some(Cow::Owned(format!("{}Tok", token.kind.style_key()))),
                deletion,
                ..RunProps::default()
            };
            runs.push(text_run(&props, &token.text));
        }
    }
    Some(runs)
}

/// A line block: one paragraph whose lines are separated by breaks, each line's inlines lowered to
/// runs in the surrounding paragraph style.
pub(super) fn line_block_paragraph(style: &str, lines: &[Vec<Inline>], ctx: &mut Ctx) -> Element {
    let mut para = Element::new("w:p");
    para.push(paragraph_props(Some(style), None, None));
    let mut first = true;
    for line in lines {
        if !first {
            para.push(break_run(&RunProps::default()));
        }
        first = false;
        render_runs(line, &RunProps::default(), ctx, &mut para);
    }
    para
}

/// A thematic break, rendered as a paragraph holding a full-width horizontal-rule drawing.
pub(super) fn horizontal_rule() -> Element {
    let rect = Element::new("v:rect")
        .attr("style", "width:0;height:1.5pt")
        .attr("o:hralign", "center")
        .attr("o:hrstd", "t")
        .attr("o:hr", "t");
    let run = Element::new("w:r").child(Element::new("w:pict").child(rect));
    Element::new("w:p").child(run)
}

/// The run properties accumulated down a chain of nested inline formatting. Rendered in the fixed
/// schema order so output stays stable regardless of nesting order.
// Independent on/off toggles; a flat set of bools is the natural shape.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Default)]
pub(super) struct RunProps {
    style: Option<Cow<'static, str>>,
    bold: bool,
    italic: bool,
    smallcaps: bool,
    strike: bool,
    underline: bool,
    east_asian: bool,
    vert_align: Option<&'static str>,
    /// Whether these runs sit inside a deletion, so their text is emitted as deleted text rather
    /// than visible text. This is a run-context flag, not a `w:rPr` toggle, so it stays out of the
    /// property element.
    deletion: bool,
}

impl RunProps {
    fn is_empty(&self) -> bool {
        self.style.is_none()
            && !self.bold
            && !self.italic
            && !self.smallcaps
            && !self.strike
            && !self.underline
            && !self.east_asian
            && self.vert_align.is_none()
    }

    fn with_bold(&self) -> Self {
        Self {
            bold: true,
            ..self.clone()
        }
    }

    fn with_italic(&self) -> Self {
        Self {
            italic: true,
            ..self.clone()
        }
    }

    fn with_smallcaps(&self) -> Self {
        Self {
            smallcaps: true,
            ..self.clone()
        }
    }

    fn with_strike(&self) -> Self {
        Self {
            strike: true,
            ..self.clone()
        }
    }

    fn with_underline(&self) -> Self {
        Self {
            underline: true,
            ..self.clone()
        }
    }

    fn with_vert_align(&self, value: &'static str) -> Self {
        Self {
            vert_align: Some(value),
            ..self.clone()
        }
    }

    pub(super) fn with_style(&self, value: impl Into<Cow<'static, str>>) -> Self {
        Self {
            style: Some(value.into()),
            ..self.clone()
        }
    }

    fn with_east_asian(&self, value: bool) -> Self {
        Self {
            east_asian: value,
            ..self.clone()
        }
    }

    fn with_deletion(&self) -> Self {
        Self {
            deletion: true,
            ..self.clone()
        }
    }

    /// The `w:rPr` element for these properties, or `None` when no property is set.
    fn element(&self) -> Option<Element> {
        if self.is_empty() {
            return None;
        }
        let mut rpr = Element::new("w:rPr");
        if let Some(style) = &self.style {
            rpr.push(Element::new("w:rStyle").attr("w:val", style_id(style.as_ref()).as_ref()));
        }
        // The East Asian hint follows the character style but precedes the toggles (schema order).
        if self.east_asian {
            rpr.push(Element::new("w:rFonts").attr("w:hint", "eastAsia"));
        }
        if self.bold {
            rpr.push(Element::new("w:b"));
            rpr.push(Element::new("w:bCs"));
        }
        if self.italic {
            rpr.push(Element::new("w:i"));
            rpr.push(Element::new("w:iCs"));
        }
        if self.smallcaps {
            rpr.push(Element::new("w:smallCaps"));
        }
        if self.strike {
            rpr.push(Element::new("w:strike"));
        }
        if self.underline {
            rpr.push(Element::new("w:u").attr("w:val", "single"));
        }
        if let Some(value) = self.vert_align {
            rpr.push(Element::new("w:vertAlign").attr("w:val", value));
        }
        Some(rpr)
    }
}

/// A `w:r` run carrying `props`' `w:rPr`, if any, ready for its content to be pushed.
pub(super) fn run_with_props(props: &RunProps) -> Element {
    let mut run = Element::new("w:r");
    if let Some(rpr) = props.element() {
        run.push(rpr);
    }
    run
}

/// A text run carrying the given properties, its whitespace preserved.
pub(super) fn text_run(props: &RunProps, text: &str) -> Element {
    let mut run = run_with_props(props);
    let tag = if props.deletion { "w:delText" } else { "w:t" };
    run.push(Element::new(tag).attr("xml:space", "preserve").text(text));
    run
}

fn break_run(props: &RunProps) -> Element {
    let mut run = run_with_props(props);
    run.push(Element::new("w:br"));
    run
}

/// Flushes an accumulated text buffer as one run, if it holds anything, carrying the East Asian hint
/// its content called for. The hint is reset ready for the next buffer.
fn flush_text(buffer: &mut String, hint: &mut bool, props: &RunProps, out: &mut Element) {
    if !buffer.is_empty() {
        out.push(text_run(&props.with_east_asian(*hint), buffer));
        buffer.clear();
    }
    *hint = false;
}

/// Whether a character calls for the East Asian font hint on its run. The recognized ranges are the
/// Han ideographs and the Yi, compatibility, half- and full-width, and supplementary ideographic
/// blocks; the kana, Hangul and bopomofo scripts are left to the default font.
fn is_east_asian(c: char) -> bool {
    matches!(
        u32::from(c),
        0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFFEE
            | 0x2_0000..=0x2_A6DF
            | 0x2_A700..=0x2_EBEF
            | 0x2_F800..=0x2_FA1F
            | 0x3_0000..=0x3_134A
    )
}

/// Lowers an inline sequence to runs. Consecutive text pieces gather into one run, split where the
/// text crosses into or out of an East Asian script so that only the East Asian stretch carries the
/// font hint; a single space between two text pieces joins the non-East-Asian side, while a space at
/// a formatting boundary, one between two East Asian stretches, and every soft break become their own
/// run; a formatted span recurses with the corresponding property set; a footnote drops a numbered
/// mark and queues its body; an image resolves to a drawing or degrades to its text; and constructs
/// without a run form degrade to the text they carry.
#[allow(clippy::too_many_lines)]
pub(super) fn render_runs(inlines: &[Inline], props: &RunProps, ctx: &mut Ctx, out: &mut Element) {
    let mut buffer = String::new();
    // East Asian kind of the buffered text; a kind change flushes first.
    let mut buffer_hint = false;
    let mut index = 0;
    while let Some(inline) = inlines.get(index) {
        match inline {
            // Consecutive text shares a run; a change of East Asian kind flushes the buffer.
            Inline::Str(_) => {
                let mut text = String::new();
                while let Some(Inline::Str(piece)) = inlines.get(index) {
                    text.push_str(piece);
                    index += 1;
                }
                let hint = text.chars().any(is_east_asian);
                if !buffer.is_empty() && buffer_hint != hint {
                    flush_text(&mut buffer, &mut buffer_hint, props, out);
                }
                if buffer.is_empty() {
                    buffer_hint = hint;
                }
                buffer.push_str(&text);
                continue;
            }
            // A space joins the non-East-Asian side of its neighbours; between two East Asian runs
            // or at a formatting boundary it stands alone.
            Inline::Space => {
                let next_hint = match inlines.get(index + 1) {
                    Some(Inline::Str(piece)) => Some(piece.chars().any(is_east_asian)),
                    _ => None,
                };
                if !buffer.is_empty() && next_hint.is_some() {
                    if !buffer_hint {
                        buffer.push(' ');
                    } else if next_hint == Some(false) {
                        flush_text(&mut buffer, &mut buffer_hint, props, out);
                        buffer.push(' ');
                    } else {
                        flush_text(&mut buffer, &mut buffer_hint, props, out);
                        out.push(text_run(props, " "));
                    }
                } else {
                    flush_text(&mut buffer, &mut buffer_hint, props, out);
                    out.push(text_run(props, " "));
                }
            }
            Inline::SoftBreak => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                out.push(text_run(props, " "));
            }
            Inline::LineBreak => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                out.push(break_run(props));
            }
            Inline::Emph(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_italic(), ctx, out);
            }
            Inline::Strong(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_bold(), ctx, out);
            }
            Inline::Underline(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_underline(), ctx, out);
            }
            Inline::Strikeout(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_strike(), ctx, out);
            }
            Inline::SmallCaps(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_smallcaps(), ctx, out);
            }
            Inline::Superscript(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_vert_align("superscript"), ctx, out);
            }
            Inline::Subscript(children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(children, &props.with_vert_align("subscript"), ctx, out);
            }
            Inline::Code(attr, text) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                #[cfg(feature = "highlight")]
                let highlighted =
                    highlighted_inline_runs(attr, text, &ctx.highlighter, props.deletion);
                #[cfg(not(feature = "highlight"))]
                let highlighted: Option<Vec<Element>> = {
                    let _ = attr;
                    None
                };
                if let Some(runs) = highlighted {
                    for run in runs {
                        out.push(run);
                    }
                } else {
                    let hint = text.chars().any(is_east_asian);
                    out.push(text_run(
                        &props.with_style("VerbatimChar").with_east_asian(hint),
                        text,
                    ));
                }
            }
            // The quotation glyphs join their inner text so a quoted word renders as one run.
            Inline::Quoted(kind, children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                let (open, close) = quotation_marks(kind);
                let mut quoted = Vec::with_capacity(children.len() + 2);
                quoted.push(Inline::Str(open.into()));
                quoted.extend(children.iter().cloned());
                quoted.push(Inline::Str(close.into()));
                render_runs(&quoted, props, ctx, out);
            }
            // Without citation processing a citation renders as the source text it was written as.
            Inline::Cite(_, source) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                render_runs(source, props, ctx, out);
            }
            // A span contributes its content, with `custom-style` as the character style. A comment
            // range is a span pair: the opening marker's text and author move to the comments part.
            Inline::Span(attr, children) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                if has_class(attr, "comment-start") {
                    open_comment(attr, children, out, ctx);
                } else if has_class(attr, "comment-end") {
                    close_comment(attr, props, out);
                } else if has_class(attr, "insertion") {
                    let id = ctx.next_insertion;
                    ctx.next_insertion = ctx.next_insertion.saturating_add(1);
                    let mut insertion = tracked_change("w:ins", id, attr);
                    render_runs(children, props, ctx, &mut insertion);
                    out.push(insertion);
                } else if has_class(attr, "deletion") {
                    let id = ctx.next_deletion;
                    ctx.next_deletion = ctx.next_deletion.saturating_add(1);
                    let mut deletion = tracked_change("w:del", id, attr);
                    render_runs(children, &props.with_deletion(), ctx, &mut deletion);
                    out.push(deletion);
                } else {
                    let mark = open_bookmark(attr.id.as_str(), out, ctx);
                    match custom_style(attr) {
                        Some(name) => {
                            render_runs(children, &props.with_style(name.to_owned()), ctx, out);
                        }
                        None => render_runs(children, props, ctx, out),
                    }
                    close_bookmark(mark, out);
                }
            }
            // A '#' destination targets the in-document bookmark; any other goes through an external
            // relationship. Content renders first, so an inner link is numbered ahead of its outer.
            Inline::Link(_, children, target) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                let mut hyperlink = Element::new("w:hyperlink");
                render_runs(
                    children,
                    &props.with_style("Hyperlink"),
                    ctx,
                    &mut hyperlink,
                );
                hyperlink = if let Some(anchor) = target.url.strip_prefix('#') {
                    hyperlink.attr("w:anchor", clean_bookmark_name(anchor).as_ref())
                } else {
                    let rel_id = ctx.hyperlink_rel(target.url.as_str());
                    hyperlink.attr("r:id", &format!("rId{rel_id}"))
                };
                out.push(hyperlink);
            }
            Inline::Image(attr, alt, target) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                match image_drawing_for(attr, target, alt, ctx) {
                    Some(drawing) => {
                        let mut run = run_with_props(props);
                        run.push(drawing);
                        out.push(run);
                    }
                    None => render_runs(alt, props, ctx, out),
                }
            }
            // Unrenderable math degrades to the delimited literal source.
            Inline::Math(kind, source) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                let display = matches!(kind, MathType::DisplayMath);
                if let Some(fragment) = crate::math::to_omml(source, display) {
                    out.push_raw(&fragment);
                } else {
                    let delimiter = if display { "$$" } else { "$" };
                    out.push(text_run(props, &format!("{delimiter}{source}{delimiter}")));
                }
            }
            // Only openxml passes through; any format still bounds the surrounding runs.
            Inline::RawInline(format, payload) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                if format.0.as_str() == "openxml" {
                    out.push_raw(payload);
                }
            }
            // A footnote drops a numbered mark here and queues its block content for the notes part.
            Inline::Note(blocks) => {
                flush_text(&mut buffer, &mut buffer_hint, props, out);
                let id = ctx.next_id;
                ctx.next_id = ctx.next_id.saturating_add(1);
                ctx.notes.push((id, blocks.clone()));
                out.push(footnote_reference_run(id, props));
            }
        }
        index += 1;
    }
    flush_text(&mut buffer, &mut buffer_hint, props, out);
}

/// The opening and closing glyphs for a quotation kind.
fn quotation_marks(kind: &QuoteType) -> (&'static str, &'static str) {
    match kind {
        QuoteType::SingleQuote => ("\u{2018}", "\u{2019}"),
        QuoteType::DoubleQuote => ("\u{201c}", "\u{201d}"),
    }
}
