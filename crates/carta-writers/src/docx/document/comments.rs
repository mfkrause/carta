//! Comment range markers and the comment entries the comments part carries.

use super::runs::{RunProps, render_runs, run_with_props};
use super::{Comment, Ctx, attr_value, paragraph_props};
use carta_ast::{Attr, Inline};
use carta_core::container::xml::Element;

/// The identifier that ties a comment's range markers to its entry, drawn from the marker's `id`.
fn comment_id(attr: &Attr) -> String {
    attr_value(attr, "id").unwrap_or("0").to_owned()
}

/// Opens a comment range: drops the range-start boundary into the flow and queues the comment's text
/// and author metadata for the comments part. The opening marker's inline content is the comment's
/// own text, so it is held back from the body rather than rendered where the range begins.
pub(super) fn open_comment(attr: &Attr, body: &[Inline], out: &mut Element, ctx: &mut Ctx) {
    let id = comment_id(attr);
    out.push(Element::new("w:commentRangeStart").attr("w:id", &id));
    ctx.comments.push(Comment {
        id,
        author: attr_value(attr, "author").map(str::to_owned),
        date: attr_value(attr, "date").map(str::to_owned),
        initials: attr_value(attr, "initials").map(str::to_owned),
        body: body.to_vec(),
    });
}

/// Closes a comment range: drops the range-end boundary and the reference mark that ties the range
/// back to its entry in the comments part.
pub(super) fn close_comment(attr: &Attr, props: &RunProps, out: &mut Element) {
    let id = comment_id(attr);
    out.push(Element::new("w:commentRangeEnd").attr("w:id", &id));
    let mut run = run_with_props(&props.with_style("CommentReference"));
    run.push(Element::new("w:commentReference").attr("w:id", &id));
    out.push(run);
}

/// Renders one comment's entry for the comments part: its identifier and author metadata, then a
/// single paragraph in the comment style that opens with the annotation mark and carries the text.
pub(super) fn render_comment_entry(comment: &Comment, ctx: &mut Ctx) -> Element {
    let mut element = Element::new("w:comment").attr("w:id", &comment.id);
    if let Some(author) = &comment.author {
        element = element.attr("w:author", author);
    }
    if let Some(date) = &comment.date {
        element = element.attr("w:date", date);
    }
    if let Some(initials) = &comment.initials {
        element = element.attr("w:initials", initials);
    }
    let mut para = Element::new("w:p");
    para.push(paragraph_props(Some("CommentText"), None, None));
    para.push(annotation_reference_run());
    render_runs(&comment.body, &RunProps::default(), ctx, &mut para);
    element.child(para)
}

/// A tracked-change wrapper (`w:ins` or `w:del`) carrying its change id and the author and date the
/// marker records. An unattributed change is credited to an unknown author; a change with no date
/// records none.
pub(super) fn tracked_change(tag: &str, id: u32, attr: &Attr) -> Element {
    let mut element = Element::new(tag)
        .attr("w:id", &id.to_string())
        .attr("w:author", attr_value(attr, "author").unwrap_or("unknown"));
    if let Some(date) = attr_value(attr, "date") {
        element = element.attr("w:date", date);
    }
    element
}

/// The run that opens a comment entry with its annotation reference mark.
fn annotation_reference_run() -> Element {
    Element::new("w:r")
        .child(
            Element::new("w:rPr").child(Element::new("w:rStyle").attr("w:val", "CommentReference")),
        )
        .child(Element::new("w:annotationRef"))
}
