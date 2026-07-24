//! Figure, caption, numbering and footnote-entry rendering.

use super::images::image_drawing_for;
use super::runs::{RunProps, render_runs, run_with_props, text_run};
use super::{
    Ctx, FlowStyle, TABLE_TEXT_WIDTH, close_bookmark, is_display_equation, open_bookmark,
    paragraph, paragraph_props, render_flow, styled_flow_with_display, styled_paragraph,
};
use carta_ast::{Attr, Block, Caption, Inline, Target};
use carta_core::container::xml::Element;

/// Renders a figure: a single embedded image as a captioned drawing when its bytes resolve,
/// otherwise the figure's content boxed in a centered frame, with the caption following in either
/// case.
pub(super) fn render_figure(
    id: &str,
    caption: &Caption,
    body: &[Block],
    out: &mut Element,
    ctx: &mut Ctx,
) {
    let mark = open_bookmark(id, out, ctx);
    if let Some((attr, alt, target)) = figure_single_image(body)
        && let Some(drawing) = image_drawing_for(attr, target, alt, ctx)
    {
        out.push(
            Element::new("w:p")
                .child(paragraph_props(Some("CaptionedFigure"), None, None))
                .child(Element::new("w:r").child(drawing)),
        );
        render_figure_caption(caption, out, ctx);
        close_bookmark(mark, out);
        return;
    }
    render_figure_frame(body, out, ctx);
    render_figure_caption(caption, out, ctx);
    close_bookmark(mark, out);
}

/// The lone image a figure wraps, when its body is exactly one paragraph holding exactly one image.
fn figure_single_image(body: &[Block]) -> Option<(&Attr, &[Inline], &Target)> {
    let [only] = body else {
        return None;
    };
    let inlines = match only {
        Block::Plain(inlines) | Block::Para(inlines) => inlines.as_slice(),
        _ => return None,
    };
    let [Inline::Image(attr, alt, target)] = inlines else {
        return None;
    };
    Some((&**attr, alt.as_slice(), &**target))
}

/// Renders a figure's content as a single centered, full-width frame.
fn render_figure_frame(body: &[Block], out: &mut Element, ctx: &mut Ctx) {
    let mut tbl = Element::new("w:tbl");
    tbl.push(
        Element::new("w:tblPr")
            .child(Element::new("w:tblStyle").attr("w:val", "FigureTable"))
            .child(
                Element::new("w:tblW")
                    .attr("w:type", "auto")
                    .attr("w:w", "0"),
            )
            .child(Element::new("w:jc").attr("w:val", "center"))
            .child(
                Element::new("w:tblLook")
                    .attr("w:firstRow", "0")
                    .attr("w:lastRow", "0")
                    .attr("w:firstColumn", "0")
                    .attr("w:lastColumn", "0"),
            ),
    );
    tbl.push(
        Element::new("w:tblGrid")
            .child(Element::new("w:gridCol").attr("w:w", &TABLE_TEXT_WIDTH.to_string())),
    );

    let mut tc = Element::new("w:tc");
    tc.push(Element::new("w:tcPr"));
    let mut wrote = false;
    for block in body {
        match block {
            Block::Para(inlines) | Block::Plain(inlines) => {
                if !inlines.is_empty() || ctx.features.keep_empty_paragraphs {
                    tc.push(styled_paragraph(
                        Some("Compact"),
                        None,
                        Some("center"),
                        inlines,
                        ctx,
                    ));
                    wrote = true;
                }
            }
            other => {
                render_flow(
                    other,
                    &mut tc,
                    ctx,
                    FlowStyle {
                        para: "Compact",
                        plain: "Compact",
                        list_ambient: None,
                    },
                );
                wrote = true;
            }
        }
    }
    if !wrote {
        tc.push(Element::new("w:p").child(Element::new("w:pPr")));
    }
    tbl.push(Element::new("w:tr").child(tc));
    out.push(tbl);
}

/// Renders a figure's caption as `ImageCaption` paragraphs.
fn render_figure_caption(caption: &Caption, out: &mut Element, ctx: &mut Ctx) {
    render_caption(&caption.long, out, ctx, "ImageCaption", Numbered::Figure);
}

/// A figure or table whose caption can carry an auto-incrementing number.
#[derive(Clone, Copy)]
pub(super) enum Numbered {
    Figure,
    Table,
}

impl Numbered {
    /// The word that opens the caption label.
    fn label(self) -> &'static str {
        match self {
            Numbered::Figure => "Figure",
            Numbered::Table => "Table",
        }
    }

    /// The field instruction that draws and advances the running count for this kind.
    fn field_instruction(self) -> &'static str {
        match self {
            Numbered::Figure => "SEQ Figure \\* ARABIC ",
            Numbered::Table => "SEQ Table \\* ARABIC ",
        }
    }

    /// The prefix of the bookmark name a caption anchors for cross-references.
    fn bookmark_prefix(self) -> &'static str {
        match self {
            Numbered::Figure => "ref_fig",
            Numbered::Table => "table",
        }
    }
}

/// Renders a caption's blocks under `style`. With native numbering on, the first paragraph gains a
/// "Figure N: " (or "Table N: ") label drawn from the running count for its kind; otherwise the
/// blocks render plainly.
pub(super) fn render_caption(
    blocks: &[Block],
    out: &mut Element,
    ctx: &mut Ctx,
    style: &'static str,
    kind: Numbered,
) {
    let mut rest = blocks.iter();
    if let Some(first) = rest.clone().next() {
        let leading = match first {
            Block::Para(inlines) | Block::Plain(inlines)
                if ctx.features.native_numbering && !inlines.is_empty() =>
            {
                Some(inlines)
            }
            _ => None,
        };
        if let Some(inlines) = leading {
            out.push(numbered_caption(style, kind, inlines, ctx));
            rest.next();
        }
    }
    for block in rest {
        render_styled_block(block, out, ctx, style);
    }
}

/// A caption paragraph led by an auto-number: the label word, the sequence field, an anchoring
/// bookmark, then the caption text after a `": "` separator.
fn numbered_caption(
    style: &'static str,
    kind: Numbered,
    inlines: &[Inline],
    ctx: &mut Ctx,
) -> Element {
    let number = match kind {
        Numbered::Figure => {
            ctx.figure_number = ctx.figure_number.saturating_add(1);
            ctx.figure_number
        }
        Numbered::Table => {
            ctx.table_number = ctx.table_number.saturating_add(1);
            ctx.table_number
        }
    };
    let mark = ctx.next_bookmark_id;
    ctx.next_bookmark_id = ctx.next_bookmark_id.wrapping_add(1);
    let name = format!("{}{number}", kind.bookmark_prefix());

    let mut para = Element::new("w:p");
    para.push(paragraph_props(Some(style), None, None));
    para.push(
        Element::new("w:bookmarkStart")
            .attr("w:id", &mark.to_string())
            .attr("w:name", &name),
    );
    // The label word and its number are joined by a non-breaking space so they never wrap apart.
    para.push(text_run(
        &RunProps::default(),
        &format!("{}\u{a0}", kind.label()),
    ));
    para.push(sequence_field(kind.field_instruction(), number));
    para.push(Element::new("w:bookmarkEnd").attr("w:id", &mark.to_string()));

    let mut prefixed = Vec::with_capacity(inlines.len() + 1);
    prefixed.push(Inline::Str(": ".into()));
    prefixed.extend(inlines.iter().cloned());
    render_runs(&prefixed, &RunProps::default(), ctx, &mut para);
    para
}

/// A simple field drawing one running sequence number; its number run carries no space handling so
/// it stays a bare digit.
fn sequence_field(instruction: &str, number: u32) -> Element {
    Element::new("w:fldSimple")
        .attr("w:instr", instruction)
        .child(Element::new("w:r").child(Element::new("w:t").text(&number.to_string())))
}

/// Renders a block whose `Para`/`Plain` shape takes one shared paragraph style.
fn render_styled_block(block: &Block, out: &mut Element, ctx: &mut Ctx, style: &'static str) {
    match block {
        Block::Para(inlines) if inlines.iter().any(is_display_equation) => {
            styled_flow_with_display(style, None, inlines, ctx, out);
        }
        Block::Para(inlines) | Block::Plain(inlines) => {
            if !inlines.is_empty() || ctx.features.keep_empty_paragraphs {
                out.push(paragraph(style, inlines, ctx));
            }
        }
        other => render_flow(
            other,
            out,
            ctx,
            FlowStyle {
                para: style,
                plain: style,
                list_ambient: None,
            },
        ),
    }
}

/// Renders one footnote entry: the marker paragraph joined to the note's first paragraph, then the
/// note's remaining blocks.
pub(super) fn render_footnote_entry(id: u32, blocks: &[Block], ctx: &mut Ctx) -> Element {
    let mut footnote = Element::new("w:footnote").attr("w:id", &id.to_string());
    let rest: &[Block] = if let Some(Block::Para(inlines) | Block::Plain(inlines)) = blocks.first()
    {
        let mut para = Element::new("w:p");
        para.push(paragraph_props(Some("FootnoteText"), None, None));
        para.push(footnote_marker_run());
        para.push(text_run(&RunProps::default(), " "));
        render_runs(inlines, &RunProps::default(), ctx, &mut para);
        footnote.push(para);
        blocks.get(1..).unwrap_or(&[])
    } else {
        // A note whose first block is not a paragraph gets a standalone marker paragraph.
        footnote.push(
            Element::new("w:p")
                .child(paragraph_props(Some("FootnoteText"), None, None))
                .child(footnote_marker_run()),
        );
        blocks
    };
    for block in rest {
        render_styled_block(block, &mut footnote, ctx, "FootnoteText");
    }
    footnote
}

/// The run that draws a footnote's own back-reference mark inside its entry.
fn footnote_marker_run() -> Element {
    Element::new("w:r")
        .child(
            Element::new("w:rPr")
                .child(Element::new("w:rStyle").attr("w:val", "FootnoteReference")),
        )
        .child(Element::new("w:footnoteRef"))
}

/// The run that references a footnote from the body, carrying the surrounding formatting plus the
/// footnote-reference character style.
pub(super) fn footnote_reference_run(id: u32, props: &RunProps) -> Element {
    let mut run = run_with_props(&props.with_style("FootnoteReference"));
    run.push(Element::new("w:footnoteReference").attr("w:id", &id.to_string()));
    run
}
