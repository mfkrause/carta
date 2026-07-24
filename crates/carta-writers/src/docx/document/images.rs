//! Image resolution and the drawing element for an embedded picture.

use super::{Ctx, ImageMedia};
use crate::image_size::{image_dimensions, image_dpi};
use carta_ast::{Attr, Inline, Target};
use carta_core::container::xml::Element;
use carta_core::media::extension_for_mime;

/// Resolves an image reference to a sized drawing, recording the picture in the media set. The bytes
/// come from the media set when the reference names an entry there, or from the reference itself when
/// it is a `data:` URI carrying an embedded payload; any other reference whose bytes are not on hand
/// returns `None`.
pub(super) fn image_drawing_for(
    attr: &Attr,
    target: &Target,
    alt: &[Inline],
    ctx: &mut Ctx,
) -> Option<Element> {
    let (bytes, mime) = match ctx.media.get(target.url.as_str()) {
        Some(item) => {
            let mime = item
                .mime
                .clone()
                .unwrap_or_else(|| mime_from_url(target.url.as_str()));
            (item.bytes.clone(), mime)
        }
        None => carta_core::media::decode_data_uri(target.url.as_str())?,
    };
    let (cx, cy) = image_extent(&bytes, &attr.attributes);

    let rel_id = ctx.next_id;
    ctx.next_id = ctx.next_id.saturating_add(3);
    let extension = extension_for_mime(&mime);
    let file_name = format!("rId{rel_id}.{extension}");
    let alt_text = carta_ast::to_plain_text(alt);
    let drawing = image_drawing(
        rel_id,
        cx,
        cy,
        target.url.as_str(),
        &alt_text,
        target.title.as_str(),
    );
    ctx.images.push(ImageMedia {
        rel_id,
        file_name,
        mime,
        bytes,
    });
    Some(drawing)
}

/// The drawn size of an image in English metric units. A requested width and height, given in pixels
/// or an absolute unit, map through a 96-dpi baseline; when only one is given the other follows from
/// the natural aspect ratio. With neither given, the natural pixel size maps through the image's own
/// resolution, so a picture that records a higher dpi draws correspondingly smaller.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // Image dimensions stay well inside range.
fn image_extent(bytes: &[u8], attributes: &[(carta_ast::Text, carta_ast::Text)]) -> (i64, i64) {
    const EMU_PER_PX: f64 = 9525.0;
    const EMU_PER_INCH: i64 = 914_400;
    // Page text width: fractional widths are measured against it and capped by it.
    const DEFAULT_WIDTH: f64 = 5_334_000.0;
    let (natural_px_w, natural_px_h) = image_dimensions(bytes);
    let natural_w = f64::from(natural_px_w);
    let natural_h = f64::from(natural_px_h);
    // Percent width scales the text width, capped at natural width; height follows the aspect ratio.
    if let Some(fraction) = dimension_fraction(attributes, "width") {
        let (dpi_x, _) = image_dpi(bytes);
        let intrinsic_w = i64::from(natural_px_w) * EMU_PER_INCH / i64::from(dpi_x.max(1));
        let scaled = (fraction * DEFAULT_WIDTH).round() as i64;
        let cx = if natural_px_w > 0 {
            intrinsic_w.min(scaled)
        } else {
            scaled
        };
        let cy = if natural_w > 0.0 {
            (cx as f64 * natural_h / natural_w).round() as i64
        } else {
            scaled
        };
        return (cx, cy);
    }
    let requested_w = dimension_px(attributes, "width");
    let requested_h = dimension_px(attributes, "height");
    let px_to_emu = |value: f64| (value * EMU_PER_PX).round() as i64;
    match (requested_w, requested_h) {
        (Some(w), Some(h)) => (px_to_emu(w), px_to_emu(h)),
        (Some(w), None) if natural_w > 0.0 => (px_to_emu(w), px_to_emu(w * natural_h / natural_w)),
        (Some(w), None) => (px_to_emu(w), px_to_emu(natural_h)),
        (None, Some(h)) if natural_h > 0.0 => (px_to_emu(h * natural_w / natural_h), px_to_emu(h)),
        (None, Some(h)) => (px_to_emu(natural_w), px_to_emu(h)),
        (None, None) => {
            let (dpi_x, dpi_y) = image_dpi(bytes);
            let dpi_x = i64::from(dpi_x.max(1));
            let dpi_y = i64::from(dpi_y.max(1));
            (
                i64::from(natural_px_w) * EMU_PER_INCH / dpi_x,
                i64::from(natural_px_h) * EMU_PER_INCH / dpi_y,
            )
        }
    }
}

/// A pixel dimension read from an attribute, when it is given in pixels or an absolute unit. A
/// percentage or unknown unit yields `None`, leaving the natural size to stand.
fn dimension_px(attributes: &[(carta_ast::Text, carta_ast::Text)], key: &str) -> Option<f64> {
    let raw = attributes
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, value)| value.as_str())?;
    let trimmed = raw.trim();
    let split = trimmed
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let value: f64 = number.parse().ok()?;
    match unit.trim() {
        "" | "px" => Some(value),
        "in" => Some(value * 96.0),
        "cm" => Some(value * 96.0 / 2.54),
        "mm" => Some(value * 96.0 / 25.4),
        "pt" => Some(value * 96.0 / 72.0),
        "pc" => Some(value * 16.0),
        _ => None,
    }
}

/// A dimension given as a percentage, returned as a fraction (so `50%` yields `0.5`). Any value that
/// is not a percentage yields `None`.
fn dimension_fraction(attributes: &[(carta_ast::Text, carta_ast::Text)], key: &str) -> Option<f64> {
    let raw = attributes
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, value)| value.as_str())?;
    let number = raw.trim().strip_suffix('%')?;
    let value: f64 = number.trim().parse().ok()?;
    Some(value / 100.0)
}

/// A MIME type guessed from a URL's file extension, for a media entry that recorded none.
fn mime_from_url(url: &str) -> String {
    carta_core::media::image_mime_for_extension(url)
        .unwrap_or("application/octet-stream")
        .to_owned()
}

/// The inline drawing for an embedded picture: an anchored image sized by `cx`×`cy` EMU, referencing
/// the relationship the media entry was stored under. The picture's own description records the
/// `source` the reference was written as, while the drawing's description carries the alt text.
fn image_drawing(
    rel_id: u32,
    cx: i64,
    cy: i64,
    source: &str,
    description: &str,
    title: &str,
) -> Element {
    let doc_id = rel_id.saturating_add(1);
    let picture_id = rel_id.saturating_add(2);
    let cx = cx.to_string();
    let cy = cy.to_string();

    let blip_fill = Element::new("pic:blipFill")
        .child(Element::new("a:blip").attr("r:embed", &format!("rId{rel_id}")))
        .child(Element::new("a:stretch").child(Element::new("a:fillRect")));

    let shape_props = Element::new("pic:spPr")
        .attr("bwMode", "auto")
        .child(
            Element::new("a:xfrm")
                .child(Element::new("a:off").attr("x", "0").attr("y", "0"))
                .child(Element::new("a:ext").attr("cx", &cx).attr("cy", &cy)),
        )
        .child(Element::new("a:prstGeom").attr("prst", "rect").child(Element::new("a:avLst")))
        .child(Element::new("a:noFill"))
        // An explicit empty outline bounds the picture's shape so a viewer draws no stray border.
        .child(
            Element::new("a:ln")
                .attr("w", "9525")
                .child(Element::new("a:noFill"))
                .child(Element::new("a:headEnd"))
                .child(Element::new("a:tailEnd")),
        );

    let picture = Element::new("pic:pic")
        .child(
            Element::new("pic:nvPicPr")
                .child(
                    Element::new("pic:cNvPr")
                        .attr("id", &picture_id.to_string())
                        .attr("name", "Picture")
                        .attr("descr", source),
                )
                .child(
                    Element::new("pic:cNvPicPr").child(
                        Element::new("a:picLocks")
                            .attr("noChangeArrowheads", "1")
                            .attr("noChangeAspect", "1"),
                    ),
                ),
        )
        .child(blip_fill)
        .child(shape_props);

    let graphic = Element::new("a:graphic").child(
        Element::new("a:graphicData")
            .attr(
                "uri",
                "http://schemas.openxmlformats.org/drawingml/2006/picture",
            )
            .child(picture),
    );

    let inline = Element::new("wp:inline")
        .child(Element::new("wp:extent").attr("cx", &cx).attr("cy", &cy))
        // A zero effect extent declares the drawing claims no space beyond its own bounds.
        .child(
            Element::new("wp:effectExtent")
                .attr("l", "0")
                .attr("t", "0")
                .attr("r", "0")
                .attr("b", "0"),
        )
        .child(
            Element::new("wp:docPr")
                .attr("id", &doc_id.to_string())
                .attr("name", "Picture")
                .attr("descr", description)
                .attr("title", title),
        )
        .child(graphic);

    Element::new("w:drawing").child(inline)
}
