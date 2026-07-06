//! The package's structural parts: the content-type catalogue and the relationship files that tie
//! the parts together. Most of their layout is fixed; the content-type catalogue and the main
//! document's relationships also name each embedded image.

use super::document::ImageMedia;
use carta_core::container::xml::Element;
use std::collections::BTreeMap;

/// Content-type namespace for the `[Content_Types].xml` catalogue.
const CONTENT_TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
/// Relationship namespace shared by every `.rels` part.
const RELATIONSHIPS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
/// Prefix under which the relationship-type URIs below all live.
const REL_BASE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A default content type keyed by file extension.
fn default_type(extension: &str, content_type: &str) -> Element {
    Element::new("Default")
        .attr("Extension", extension)
        .attr("ContentType", content_type)
}

/// An explicit content type for a single part.
fn override_type(part: &str, content_type: &str) -> Element {
    Element::new("Override")
        .attr("PartName", part)
        .attr("ContentType", content_type)
}

/// The `[Content_Types].xml` catalogue naming every part's content type, including one entry per
/// embedded image.
pub(super) fn content_types(images: &[ImageMedia]) -> String {
    let word = "application/vnd.openxmlformats-officedocument.wordprocessingml";
    let mut types = Element::new("Types").attr("xmlns", CONTENT_TYPES_NS);
    types.push(default_type("xml", "application/xml"));
    types.push(default_type(
        "rels",
        "application/vnd.openxmlformats-package.relationships+xml",
    ));
    types.push(default_type(
        "odttf",
        "application/vnd.openxmlformats-officedocument.obfuscatedFont",
    ));
    types.push(override_type(
        "/word/webSettings.xml",
        &format!("{word}.webSettings+xml"),
    ));
    types.push(override_type(
        "/word/numbering.xml",
        &format!("{word}.numbering+xml"),
    ));
    types.push(override_type(
        "/word/settings.xml",
        &format!("{word}.settings+xml"),
    ));
    types.push(override_type(
        "/word/theme/theme1.xml",
        "application/vnd.openxmlformats-officedocument.theme+xml",
    ));
    types.push(override_type(
        "/word/fontTable.xml",
        &format!("{word}.fontTable+xml"),
    ));
    types.push(override_type(
        "/docProps/app.xml",
        "application/vnd.openxmlformats-officedocument.extended-properties+xml",
    ));
    types.push(override_type(
        "/docProps/core.xml",
        "application/vnd.openxmlformats-package.core-properties+xml",
    ));
    types.push(override_type(
        "/docProps/custom.xml",
        "application/vnd.openxmlformats-officedocument.custom-properties+xml",
    ));
    types.push(override_type(
        "/word/styles.xml",
        &format!("{word}.styles+xml"),
    ));
    types.push(override_type(
        "/word/document.xml",
        &format!("{word}.document.main+xml"),
    ));
    types.push(override_type(
        "/word/comments.xml",
        &format!("{word}.comments+xml"),
    ));
    types.push(override_type(
        "/word/footnotes.xml",
        &format!("{word}.footnotes+xml"),
    ));
    for image in images {
        types.push(override_type(
            &format!("/word/media/{}", image.file_name),
            &image.mime,
        ));
    }
    types.render_document()
}

/// A single relationship entry.
fn relationship(id: &str, rel_type: &str, target: &str) -> Element {
    Element::new("Relationship")
        .attr("Id", id)
        .attr("Type", rel_type)
        .attr("Target", target)
}

/// The package root relationships: the main document and the three metadata parts.
pub(super) fn root_rels() -> String {
    let mut rels = Element::new("Relationships").attr("xmlns", RELATIONSHIPS_NS);
    rels.push(relationship(
        "rId1",
        &format!("{REL_BASE}/officeDocument"),
        "word/document.xml",
    ));
    rels.push(relationship(
        "rId4",
        &format!("{REL_BASE}/extended-properties"),
        "docProps/app.xml",
    ));
    rels.push(relationship(
        "rId3",
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties",
        "docProps/core.xml",
    ));
    rels.push(relationship(
        "rId5",
        &format!("{REL_BASE}/custom-properties"),
        "docProps/custom.xml",
    ));
    rels.render_document()
}

/// The main document's relationships: the styling, settings and note parts it always references,
/// one image relationship per embedded picture, then one external relationship per distinct
/// hyperlink destination. The hyperlinks come from an ordered map, so they emit in a stable order
/// regardless of where each destination first appeared.
pub(super) fn document_rels(images: &[ImageMedia], hyperlinks: &BTreeMap<String, u32>) -> String {
    let mut rels = Element::new("Relationships").attr("xmlns", RELATIONSHIPS_NS);
    rels.push(relationship(
        "rId8",
        &format!("{REL_BASE}/numbering"),
        "numbering.xml",
    ));
    rels.push(relationship(
        "rId7",
        &format!("{REL_BASE}/styles"),
        "styles.xml",
    ));
    rels.push(relationship(
        "rId6",
        &format!("{REL_BASE}/settings"),
        "settings.xml",
    ));
    rels.push(relationship(
        "rId5",
        &format!("{REL_BASE}/webSettings"),
        "webSettings.xml",
    ));
    rels.push(relationship(
        "rId4",
        &format!("{REL_BASE}/fontTable"),
        "fontTable.xml",
    ));
    rels.push(relationship(
        "rId3",
        &format!("{REL_BASE}/theme"),
        "theme/theme1.xml",
    ));
    rels.push(relationship(
        "rId2",
        &format!("{REL_BASE}/footnotes"),
        "footnotes.xml",
    ));
    rels.push(relationship(
        "rId1",
        &format!("{REL_BASE}/comments"),
        "comments.xml",
    ));
    for image in images {
        rels.push(relationship(
            &format!("rId{}", image.rel_id),
            &format!("{REL_BASE}/image"),
            &format!("media/{}", image.file_name),
        ));
    }
    push_hyperlink_rels(&mut rels, hyperlinks);
    rels.render_document()
}

/// The footnotes part references the same external hyperlink destinations as the body, since a link
/// may sit in a footnote as readily as in the flow, so it carries the identical external
/// relationships under their shared ids.
pub(super) fn footnotes_rels(hyperlinks: &BTreeMap<String, u32>) -> String {
    let mut rels = Element::new("Relationships").attr("xmlns", RELATIONSHIPS_NS);
    push_hyperlink_rels(&mut rels, hyperlinks);
    rels.render_document()
}

/// Appends one external relationship per distinct hyperlink destination, in the map's key order so
/// the emission is stable regardless of where each destination first appeared.
fn push_hyperlink_rels(rels: &mut Element, hyperlinks: &BTreeMap<String, u32>) {
    for (url, id) in hyperlinks {
        rels.push(
            relationship(&format!("rId{id}"), &format!("{REL_BASE}/hyperlink"), url)
                .attr("TargetMode", "External"),
        );
    }
}
