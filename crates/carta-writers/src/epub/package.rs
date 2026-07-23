//! Building the package document (`content.opf`): the publication's metadata, the manifest of every
//! file it contains, the spine ordering the reading systems follow, and the guide's reading-order
//! landmarks. The EPUB 3 and EPUB 2 dialects share this structure but differ in the package version,
//! how a creator's role is recorded, and the accessibility and modification metadata EPUB 3 adds.

use super::Version;
use super::metadata::{BookMeta, Creator, Identifier};
use carta_core::container::xml::Element;

/// The identifier the package's unique identifier attribute points at.
const IDENTIFIER_ID: &str = "epub-id-1";

/// One entry in the manifest: a file in the container, its media type, and any EPUB properties it
/// carries (`nav`, `cover-image`, `svg`).
pub(crate) struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub properties: Option<String>,
}

/// One entry in the spine: the manifest item it references and, when set, whether it is part of the
/// linear reading order.
pub(crate) struct SpineItem {
    pub idref: String,
    pub linear: Option<&'static str>,
}

/// The dates a package records: the publication date (`dc:date`, always present) and, for EPUB 3,
/// the last-modified timestamp (`dcterms:modified`).
pub(crate) struct Dates {
    pub publication: String,
    pub modified: Option<String>,
}

/// Render the package document for `version`.
pub(crate) fn content_opf(
    version: Version,
    meta: &BookMeta,
    dates: &Dates,
    cover_id: Option<&str>,
    manifest: &[ManifestItem],
    spine: &[SpineItem],
) -> String {
    let epub3 = version.is_epub3();

    let mut package = Element::new("package")
        .attr("version", if epub3 { "3.0" } else { "2.0" })
        .attr("xmlns", "http://www.idpf.org/2007/opf");
    if epub3 {
        package = package.attr("xml:lang", &meta.language);
    }
    package = package.attr("unique-identifier", IDENTIFIER_ID);
    if epub3 {
        package = package.attr(
            "prefix",
            "ibooks: http://vocabulary.itunes.apple.com/rdf/ibooks/vocabulary-extensions-1.0/",
        );
    }

    package
        .child(build_metadata(version, meta, dates, cover_id))
        .child(build_manifest(manifest))
        .child(build_spine(spine))
        .child(build_guide(meta, cover_id.is_some()))
        .render_document_pretty()
}

/// The publication metadata block.
fn build_metadata(
    version: Version,
    meta: &BookMeta,
    dates: &Dates,
    cover_id: Option<&str>,
) -> Element {
    let epub3 = version.is_epub3();
    let mut block = Element::new("metadata")
        .attr("xmlns:dc", "http://purl.org/dc/elements/1.1/")
        .attr("xmlns:opf", "http://www.idpf.org/2007/opf");

    for (index, identifier) in meta.identifiers.iter().enumerate() {
        push_identifier(&mut block, epub3, index, identifier);
    }
    // `dc:title` is required, so an untitled document gets a placeholder rather than an invalid package.
    block.push(
        Element::new("dc:title")
            .attr("id", "epub-title-1")
            .text(meta.display_title()),
    );
    let date_id = if epub3 { "epub-date" } else { "epub-date-1" };
    block.push(
        Element::new("dc:date")
            .attr("id", date_id)
            .text(&dates.publication),
    );
    block.push(Element::new("dc:language").text(&meta.language));

    for (index, creator) in meta.creators.iter().enumerate() {
        push_agent(
            &mut block,
            epub3,
            "dc:creator",
            "epub-creator",
            index,
            creator,
        );
    }
    for (index, contributor) in meta.contributors.iter().enumerate() {
        push_agent(
            &mut block,
            epub3,
            "dc:contributor",
            "epub-contributor",
            index,
            contributor,
        );
    }

    for (index, subject) in meta.subjects.iter().enumerate() {
        block.push(
            Element::new("dc:subject")
                .attr("id", &format!("subject-{}", index + 1))
                .text(subject),
        );
    }
    if let Some(description) = &meta.description {
        block.push(Element::new("dc:description").text(description));
    }
    if let Some(publisher) = &meta.publisher {
        block.push(Element::new("dc:publisher").text(publisher));
    }
    for extra in &meta.extra {
        let mut element = Element::new(&extra.name);
        for (key, value) in &extra.attributes {
            element = element.attr(key, value);
        }
        block.push(element.text(&extra.text));
    }
    if let Some(rights) = &meta.rights_text {
        block.push(Element::new("dc:rights").text(rights));
    }
    // The cover pointer and `dcterms:modified` are singletons; skip the generated one when a
    // carried-through fragment already declares it.
    if let Some(cover) = cover_id
        && !extra_has_meta(meta, "name", "cover")
    {
        block.push(
            Element::new("meta")
                .attr("name", "cover")
                .attr("content", cover),
        );
    }

    if epub3 {
        if let Some(modified) = &dates.modified
            && !extra_has_meta(meta, "property", "dcterms:modified")
        {
            block.push(
                Element::new("meta")
                    .attr("property", "dcterms:modified")
                    .text(modified),
            );
        }
        for (property, value) in ACCESSIBILITY_META {
            block.push(Element::new("meta").attr("property", property).text(value));
        }
    }

    block
}

/// Whether the carried-through extra metadata already declares a `<meta>` element bearing the given
/// attribute, so a generated singleton with the same role is not emitted a second time.
fn extra_has_meta(meta: &BookMeta, attr_name: &str, attr_value: &str) -> bool {
    meta.extra.iter().any(|extra| {
        extra.name == "meta"
            && extra
                .attributes
                .iter()
                .any(|(key, value)| key == attr_name && value == attr_value)
    })
}

/// Emit one publication identifier. A named scheme is projected as the scheme name in EPUB 2 (an
/// `opf:scheme` attribute) and as its ONIX code in EPUB 3 (an `identifier-type` refinement pointing
/// back at the element). The first identifier is the package's unique identifier, [`IDENTIFIER_ID`].
fn push_identifier(block: &mut Element, epub3: bool, index: usize, identifier: &Identifier) {
    let id = format!("epub-id-{}", index + 1);
    if epub3 {
        block.push(
            Element::new("dc:identifier")
                .attr("id", &id)
                .text(&identifier.text),
        );
        if let Some(code) = identifier.onix_code() {
            block.push(
                Element::new("meta")
                    .attr("refines", &format!("#{id}"))
                    .attr("property", "identifier-type")
                    .attr("scheme", "onix:codelist5")
                    .text(&code),
            );
        }
    } else {
        let mut element = Element::new("dc:identifier").attr("id", &id);
        if let Some(scheme) = &identifier.scheme {
            element = element.attr("opf:scheme", scheme);
        }
        block.push(element.text(&identifier.text));
    }
}

/// Emit one creator or contributor. EPUB 3 records the sortable name and role as `refines` metadata
/// pointing back at the element; EPUB 2 carries them as `opf:` attributes on the element itself.
fn push_agent(
    block: &mut Element,
    epub3: bool,
    element_name: &str,
    id_prefix: &str,
    index: usize,
    agent: &Creator,
) {
    let id = format!("{id_prefix}-{}", index + 1);
    if epub3 {
        block.push(Element::new(element_name).attr("id", &id).text(&agent.text));
        if let Some(file_as) = &agent.file_as {
            block.push(
                Element::new("meta")
                    .attr("refines", &format!("#{id}"))
                    .attr("property", "file-as")
                    .text(file_as),
            );
        }
        if let Some(role) = &agent.role {
            block.push(
                Element::new("meta")
                    .attr("refines", &format!("#{id}"))
                    .attr("property", "role")
                    .attr("scheme", "marc:relators")
                    .text(role),
            );
        }
    } else {
        let mut element = Element::new(element_name).attr("id", &id);
        if let Some(file_as) = &agent.file_as {
            element = element.attr("opf:file-as", file_as);
        }
        if let Some(role) = &agent.role {
            element = element.attr("opf:role", role);
        }
        block.push(element.text(&agent.text));
    }
}

/// The accessibility metadata an EPUB 3 package advertises for a text-only publication with a
/// navigable structure.
const ACCESSIBILITY_META: &[(&str, &str)] = &[
    ("schema:accessMode", "textual"),
    ("schema:accessModeSufficient", "textual"),
    ("schema:accessibilityFeature", "alternativeText"),
    ("schema:accessibilityFeature", "readingOrder"),
    ("schema:accessibilityFeature", "structuralNavigation"),
    ("schema:accessibilityFeature", "tableOfContents"),
    ("schema:accessibilityHazard", "none"),
];

/// The manifest of every file in the container.
fn build_manifest(items: &[ManifestItem]) -> Element {
    let mut manifest = Element::new("manifest");
    for item in items {
        // The cover image leads with `properties`; every other item carries it last.
        let element = if item.properties.as_deref() == Some("cover-image") {
            Element::new("item")
                .attr("properties", "cover-image")
                .attr("id", &item.id)
                .attr("href", &item.href)
                .attr("media-type", &item.media_type)
        } else {
            let mut element = Element::new("item")
                .attr("id", &item.id)
                .attr("href", &item.href)
                .attr("media-type", &item.media_type);
            if let Some(properties) = &item.properties {
                element = element.attr("properties", properties);
            }
            element
        };
        manifest.push(element);
    }
    manifest
}

/// The spine, ordering the documents a reading system presents.
fn build_spine(items: &[SpineItem]) -> Element {
    let mut spine = Element::new("spine").attr("toc", "ncx");
    for item in items {
        let mut element = Element::new("itemref").attr("idref", &item.idref);
        if let Some(linear) = item.linear {
            element = element.attr("linear", linear);
        }
        spine.push(element);
    }
    spine
}

/// The guide, naming the reading-order landmarks: the table of contents and, when present, the
/// cover page.
fn build_guide(meta: &BookMeta, has_cover: bool) -> Element {
    let mut guide = Element::new("guide").child(
        Element::new("reference")
            .attr("type", "toc")
            .attr("title", meta.display_title())
            .attr("href", "nav.xhtml"),
    );
    if has_cover {
        guide.push(
            Element::new("reference")
                .attr("type", "cover")
                .attr("title", "Cover")
                .attr("href", "text/cover.xhtml"),
        );
    }
    guide
}
