//! Figure, caption, numbering and footnote-entry rendering.

use super::images::image_drawing_for;
use super::runs::{RunProps, render_paragraph_runs, render_runs, run_with_props, text_run};
use super::{
    Ctx, Inherited, TABLE_TEXT_WIDTH, close_bookmark, flow_style, open_bookmark, paragraph_props,
    render_block, render_blocks, render_paragraph,
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
    inherited: Inherited,
) {
    let mark = open_bookmark(id, out, ctx);
    ctx.prev_paragraph = false;
    let image = figure_single_image(body)
        .and_then(|(attr, alt, target)| Some((attr, image_drawing_for(attr, target, alt, ctx)?)));
    if let Some((attr, drawing)) = image {
        let numbering = ctx.take_number();
        let style = if caption.long.is_empty() {
            "Figure"
        } else {
            "CaptionedFigure"
        };
        let mut paragraph = Element::new("w:p").child(paragraph_props(
            Some(style),
            numbering,
            inherited.jc,
            ctx.run_properties.right_to_left,
        ));
        let image_mark = open_bookmark(attr.id.as_str(), &mut paragraph, ctx);
        paragraph.push(Element::new("w:r").child(drawing));
        close_bookmark(image_mark, &mut paragraph, ctx);
        out.push(paragraph);
    } else {
        render_figure_frame(body, out, ctx, inherited);
    }
    render_caption(
        &caption.long,
        out,
        ctx,
        inherited.styled("ImageCaption"),
        Numbered::Figure,
    );
    close_bookmark(mark, out, ctx);
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

/// Renders a figure's content as a centered, full-width frame: one row with a cell per block, and
/// no row at all for a figure without content. A frame inside a list item uses up the item's marker.
#[allow(clippy::cast_precision_loss)] // Block counts are tiny, far inside f64's exact range.
fn render_figure_frame(body: &[Block], out: &mut Element, ctx: &mut Ctx, inherited: Inherited) {
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
    let width = (TABLE_TEXT_WIDTH / body.len() as f64).round().to_string();
    let mut grid = Element::new("w:tblGrid");
    for _ in body {
        grid.push(Element::new("w:gridCol").attr("w:w", &width));
    }
    tbl.push(grid);

    if let Some(item) = ctx.item.as_mut() {
        item.lead_used = true;
    }
    ctx.in_table = true;
    let in_cell = Inherited {
        jc: Some("center"),
        ..inherited.nested()
    };
    let mut row = Element::new("w:tr");
    for block in body {
        let mut tc = Element::new("w:tc").child(Element::new("w:tcPr"));
        render_block(block, &mut tc, ctx, in_cell);
        // Cells require a paragraph and must not end on a table.
        if tc.child_count() == 1 {
            tc.push(Element::new("w:p").child(Element::new("w:pPr")));
        } else if tc.last_child_element_name() == Some("w:tbl") {
            tc.push(Element::new("w:p"));
        }
        row.push(tc);
    }
    ctx.in_table = false;
    if !body.is_empty() {
        tbl.push(row);
    }
    out.push(tbl);
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

/// Renders a caption's blocks under the caption style `inherited` carries. With native numbering on, the
/// first paragraph gains a "Figure N: " (or "Table N: ") label drawn from the running count for its
/// kind; otherwise the blocks render plainly. A figure caption's tight paragraphs keep the caption
/// style wherever the figure sits.
pub(super) fn render_caption(
    blocks: &[Block],
    out: &mut Element,
    ctx: &mut Ctx,
    inherited: Inherited,
    kind: Numbered,
) {
    let mut rest = blocks;
    if ctx.features.native_numbering
        && let Some((first @ (Block::Para(inlines) | Block::Plain(inlines)), tail)) =
            blocks.split_first()
    {
        let tight = matches!((kind, first), (Numbered::Table, Block::Plain(_)));
        out.push(numbered_caption(inherited, tight, kind, inlines, ctx));
        ctx.prev_paragraph = true;
        rest = tail;
    }
    match kind {
        Numbered::Table => render_blocks(rest, out, ctx, inherited),
        Numbered::Figure => {
            for block in rest {
                match block {
                    Block::Plain(inlines) => render_paragraph(inlines, false, out, ctx, inherited),
                    other => render_block(other, out, ctx, inherited),
                }
            }
        }
    }
}

/// A caption paragraph led by an auto-number: the label word, the sequence field, an anchoring
/// bookmark, then the caption text after a `": "` separator.
fn numbered_caption(
    inherited: Inherited,
    tight: bool,
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
    let name = format!("{}{number}", kind.bookmark_prefix());
    let properties = ctx.run_properties.clone();

    let mut para = Element::new("w:p");
    para.push(paragraph_props(
        flow_style(inherited, tight, false, ctx),
        ctx.take_number(),
        inherited.jc,
        ctx.run_properties.right_to_left,
    ));
    let mark = open_bookmark(&name, &mut para, ctx);
    // The label word and its number are joined by a non-breaking space so they never wrap apart.
    para.push(text_run(&properties, &format!("{}\u{a0}", kind.label())));
    para.push(sequence_field(kind.field_instruction(), number));
    close_bookmark(mark, &mut para, ctx);

    let mut prefixed = Vec::with_capacity(inlines.len() + 1);
    prefixed.push(Inline::Str(": ".into()));
    prefixed.extend(inlines.iter().cloned());
    render_paragraph_runs(&prefixed, &properties, ctx, &mut para);
    para
}

/// A simple field drawing one running sequence number; its number run carries no space handling so
/// it stays a bare digit.
fn sequence_field(instruction: &str, number: u32) -> Element {
    Element::new("w:fldSimple")
        .attr("w:instr", instruction)
        .child(Element::new("w:r").child(Element::new("w:t").text(&number.to_string())))
}

/// Renders one footnote entry: the marker paragraph joined to the note's first paragraph, then the
/// note's remaining blocks.
pub(super) fn render_footnote_entry(id: u32, blocks: &[Block], ctx: &mut Ctx) -> Element {
    let mut footnote = Element::new("w:footnote").attr("w:id", &id.to_string());
    let properties = ctx.run_properties.clone();
    let rest: &[Block] = if let Some(Block::Para(inlines) | Block::Plain(inlines)) = blocks.first()
    {
        let mut para = Element::new("w:p");
        para.push(paragraph_props(
            Some("FootnoteText"),
            None,
            None,
            ctx.run_properties.right_to_left,
        ));
        para.push(footnote_marker_run(&properties));
        para.push(text_run(&properties, " "));
        render_runs(inlines, &properties, ctx, &mut para);
        footnote.push(para);
        blocks.get(1..).unwrap_or(&[])
    } else {
        // A note whose first block is not a paragraph gets a standalone marker paragraph.
        footnote.push(
            Element::new("w:p")
                .child(paragraph_props(
                    Some("FootnoteText"),
                    None,
                    None,
                    ctx.run_properties.right_to_left,
                ))
                .child(footnote_marker_run(&properties)),
        );
        blocks
    };
    let inherited = Inherited {
        style: Some("FootnoteText"),
        jc: None,
        sections: false,
    };
    render_blocks(rest, &mut footnote, ctx, inherited);
    footnote
}

/// The run that draws a footnote's own back-reference mark inside its entry.
fn footnote_marker_run(props: &RunProps) -> Element {
    run_with_props(&props.with_style("FootnoteReference")).child(Element::new("w:footnoteRef"))
}

/// The run that references a footnote from the body. The mark takes the footnote-reference
/// character style alone, free of the formatting of the text around it.
pub(super) fn footnote_reference_run(id: u32, props: &RunProps) -> Element {
    let mut run = run_with_props(&props.scope().with_style("FootnoteReference"));
    run.push(Element::new("w:footnoteReference").attr("w:id", &id.to_string()));
    run
}
