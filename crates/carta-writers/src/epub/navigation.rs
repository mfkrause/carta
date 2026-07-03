//! Building the two tables of contents an EPUB carries: the XHTML navigation document (`nav.xhtml`,
//! primary in EPUB 3) and the NCX (`toc.ncx`, primary in EPUB 2). Both are derived from the same
//! tree of section entries collected from the document's section hierarchy.

use super::Version;
use super::metadata::{BookMeta, UNTITLED};
use super::pages::{BodyKind, inline_plain, xhtml_page};
use crate::html::render_epub_inlines;
use carta_ast::{Attr, Block, Inline, Text};
use carta_core::container::xml::{Element, escape_attribute, escape_text};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The path to the generated title page, relative to the container root.
const TITLE_PAGE_HREF: &str = "text/title_page.xhtml";
/// The path to the generated cover page, relative to the container root.
const COVER_HREF: &str = "text/cover.xhtml";

/// One entry in the table of contents: a section's identifier and heading, the file it lives in, and
/// any subsections nested beneath it.
pub(crate) struct TocEntry {
    id: String,
    title: Vec<Inline>,
    file: String,
    children: Vec<TocEntry>,
}

/// Collect the table-of-contents tree from the full section hierarchy, including every section down
/// to `toc_depth` heading levels. The tree follows the sections' nesting, independent of how they
/// were split across files: `file_of` maps each section's identifier to the file it landed in, so a
/// subsection promoted to its own chapter still nests under its parent while linking to its own file.
pub(crate) fn collect_toc(
    sections: &[Block],
    file_of: &BTreeMap<String, String>,
    toc_depth: usize,
) -> Vec<TocEntry> {
    collect(sections, file_of, toc_depth)
}

/// The section entries directly within `blocks`, each carrying its own nested subsections.
fn collect(
    blocks: &[Block],
    file_of: &BTreeMap<String, String>,
    toc_depth: usize,
) -> Vec<TocEntry> {
    let mut out = Vec::new();
    for block in blocks {
        let Block::Div(attr, children) = block else {
            continue;
        };
        if !attr.classes.iter().any(|class| class == "section") {
            continue;
        }
        let level = attr
            .classes
            .iter()
            .find_map(|class| {
                class
                    .strip_prefix("level")
                    .and_then(|n| n.parse::<usize>().ok())
            })
            .unwrap_or(1);
        if level > toc_depth {
            continue;
        }
        let id = attr.id.to_string();
        let file = file_of.get(&id).cloned().unwrap_or_default();
        out.push(TocEntry {
            title: header_inlines(children),
            file,
            id,
            children: collect(children, file_of, toc_depth),
        });
    }
    out
}

/// The inline content of the first heading among a section's children.
fn header_inlines(blocks: &[Block]) -> Vec<Inline> {
    blocks
        .iter()
        .find_map(|block| match block {
            Block::Header(_, _, inlines) => Some(inlines.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Which optional entries the navigation landmarks list carries alongside the title page: the cover
/// page (when present) and the table of contents (when one was requested).
pub(crate) struct Landmarks {
    pub cover: bool,
    pub toc: bool,
}

/// Render the XHTML navigation document. In EPUB 3 it is the primary table of contents (a `<nav>`
/// with `epub:type` and a landmarks list); in EPUB 2 it degrades to a plain `<div>` with no
/// landmarks, since the NCX is authoritative there.
pub(crate) fn nav_xhtml(
    version: Version,
    meta: &BookMeta,
    doc_meta: &BookMeta,
    entries: &[TocEntry],
    landmarks: &Landmarks,
    stylesheets: &[String],
    source_name: Option<&str>,
) -> String {
    let epub3 = version.is_epub3();
    let mut counter = 0usize;
    let list = render_list(entries, epub3, &mut counter);
    // A table-of-contents `<nav>` must hold exactly one non-empty `<ol>`. When the document has no
    // sections to list, the list still points at the title page, so the navigation stays valid and
    // the book keeps one reachable entry.
    let list = if list.is_empty() {
        let mut label = String::new();
        escape_text(meta.display_title(), &mut label);
        format!(
            "<ol class=\"toc\"><li id=\"toc-li-1\"><a href=\"{TITLE_PAGE_HREF}\">{label}</a></li></ol>"
        )
    } else {
        list
    };

    let mut title = String::new();
    escape_text(meta.display_title(), &mut title);
    let heading = format!("<h1 id=\"toc-title\">{title}</h1>");

    let body = if epub3 {
        let landmarks_markup = render_landmarks(landmarks);
        format!(
            "<nav epub:type=\"toc\" role=\"doc-toc\" id=\"toc\">{heading}{list}</nav>\n{landmarks_markup}"
        )
    } else {
        format!("<div id=\"toc\">{heading}{list}</div>")
    };

    // The navigation document's own `<title>` reflects the document's title, falling back to the
    // source name when the document is untitled — unlike the visible heading above, which shows the
    // publication title (which a metadata fragment may override). Standard input (`-`) has no
    // meaningful name, so it keeps the placeholder.
    let head_title = match source_name {
        Some(name) if doc_meta.title_text.is_empty() && name != "-" => name,
        _ => doc_meta.display_title(),
    };
    xhtml_page(
        version,
        &meta.language,
        head_title,
        "",
        BodyKind::Frontmatter,
        stylesheets,
        &body,
    )
}

/// Render a nested `<ol class="toc">` list for `entries`, assigning each item a document-wide index.
fn render_list(entries: &[TocEntry], epub3: bool, counter: &mut usize) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::from("<ol class=\"toc\">");
    for entry in entries {
        *counter += 1;
        let index = *counter;
        let mut href = String::new();
        escape_attribute(&format!("{}#{}", entry.file, entry.id), &mut href);
        let text = render_epub_inlines(&nav_label_inlines(&entry.title), epub3);
        // A navigation anchor must carry text; an untitled section falls back to a placeholder rather
        // than an empty link the reading system cannot label.
        let label = if text.is_empty() { UNTITLED } else { &text };
        let anchor = format!("<a href=\"text/{href}\">{label}</a>");
        let nested = render_list(&entry.children, epub3, counter);
        let _ = write!(out, "<li id=\"toc-li-{index}\">{anchor}{nested}</li>");
    }
    out.push_str("</ol>");
    out
}

/// Prepare a heading's inlines for a navigation label: drop footnotes and replace each link with its
/// own content, since a nav anchor cannot nest another; relabel the body's section-number span with
/// the navigation's own class. Everything else — emphasis, code, images (already pointing at their
/// stored path) and raw inline markup — is carried through, recursing into styled spans.
fn nav_label_inlines(inlines: &[Inline]) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut after_number = false;
    for inline in inlines {
        // The separator a numbered heading places after its section number becomes a non-breaking
        // space in the navigation, keeping the number joined to its title across a line wrap.
        if after_number && matches!(inline, Inline::Space) {
            out.push(Inline::Str(Text::from("\u{a0}")));
            after_number = false;
            continue;
        }
        after_number = false;
        match inline {
            Inline::Note(_) => {}
            Inline::Link(_, inner, _) => out.extend(nav_label_inlines(inner)),
            Inline::Span(attr, inner) => {
                after_number = attr
                    .classes
                    .iter()
                    .any(|class| class == "header-section-number");
                let classes = attr
                    .classes
                    .iter()
                    .map(|class| {
                        if class == "header-section-number" {
                            Text::from("section-header-number")
                        } else {
                            class.clone()
                        }
                    })
                    .collect();
                let attr = Attr {
                    id: attr.id.clone(),
                    classes,
                    attributes: attr.attributes.clone(),
                };
                out.push(Inline::Span(Box::new(attr), nav_label_inlines(inner)));
            }
            Inline::Emph(inner) => out.push(Inline::Emph(nav_label_inlines(inner))),
            Inline::Underline(inner) => out.push(Inline::Underline(nav_label_inlines(inner))),
            Inline::Strong(inner) => out.push(Inline::Strong(nav_label_inlines(inner))),
            Inline::Strikeout(inner) => out.push(Inline::Strikeout(nav_label_inlines(inner))),
            Inline::Superscript(inner) => out.push(Inline::Superscript(nav_label_inlines(inner))),
            Inline::Subscript(inner) => out.push(Inline::Subscript(nav_label_inlines(inner))),
            Inline::SmallCaps(inner) => out.push(Inline::SmallCaps(nav_label_inlines(inner))),
            Inline::Quoted(quote, inner) => {
                out.push(Inline::Quoted(quote.clone(), nav_label_inlines(inner)));
            }
            other => out.push(other.clone()),
        }
    }
    out
}

/// The landmarks navigation listing the cover (when present), the title page, and the table of
/// contents (when one was requested).
fn render_landmarks(landmarks: &Landmarks) -> String {
    let mut items = String::new();
    let _ = writeln!(
        items,
        "    <li>\n      <a href=\"{TITLE_PAGE_HREF}\" epub:type=\"titlepage\">Title Page</a>\n    </li>"
    );
    if landmarks.cover {
        let _ = writeln!(
            items,
            "    <li>\n      <a href=\"{COVER_HREF}\" epub:type=\"cover\">Cover</a>\n    </li>"
        );
    }
    if landmarks.toc {
        let _ = writeln!(
            items,
            "    <li>\n      <a href=\"#toc\" epub:type=\"toc\">Table of Contents</a>\n    </li>"
        );
    }
    format!(
        "<nav epub:type=\"landmarks\" id=\"landmarks\" hidden=\"hidden\">\n  <ol>\n{items}  </ol>\n</nav>"
    )
}

/// Render the NCX navigation control file: the document identifier and title, then a navigation map
/// whose first point is the title page, followed by every section entry in reading order.
pub(crate) fn toc_ncx(
    meta: &BookMeta,
    doc_meta: &BookMeta,
    entries: &[TocEntry],
    cover_id: Option<&str>,
) -> String {
    let mut head = Element::new("head")
        .child(ncx_meta("dtb:uid", meta.primary_identifier()))
        // This reading system reports a single navigation level.
        .child(ncx_meta("dtb:depth", "1"))
        .child(ncx_meta("dtb:totalPageCount", "0"))
        .child(ncx_meta("dtb:maxPageNumber", "0"));
    if let Some(cover) = cover_id {
        head.push(ncx_meta("cover", cover));
    }

    let doc_title = Element::new("docTitle").child(Element::new("text").text(meta.display_title()));

    let mut counter = 0usize;
    let mut nav_map = Element::new("navMap");
    // The title page's navigation label reflects the document's own title, blank when it has none —
    // even where the document title (`docTitle`) shows a publication title supplied by a fragment.
    nav_map.push(nav_point(
        &mut counter,
        TITLE_PAGE_HREF,
        &doc_meta.title_text,
        &[],
    ));
    for entry in entries {
        nav_map.push(entry_nav_point(entry, &mut counter));
    }

    Element::new("ncx")
        .attr("version", "2005-1")
        .attr("xmlns", "http://www.daisy.org/z3986/2005/ncx/")
        .child(head)
        .child(doc_title)
        .child(nav_map)
        .render_document_pretty()
}

/// One `<meta name= content= />` entry in the NCX head.
fn ncx_meta(name: &str, content: &str) -> Element {
    Element::new("meta")
        .attr("name", name)
        .attr("content", content)
}

/// A navigation point for one table-of-contents entry, recursing into its subsections.
fn entry_nav_point(entry: &TocEntry, counter: &mut usize) -> Element {
    let src = format!("text/{}#{}", entry.file, entry.id);
    nav_point(counter, &src, &inline_plain(&entry.title), &entry.children)
}

/// Build a navigation point pointing at `src` (relative to the container root), labelled `label`,
/// with `children` nested beneath it. Points are numbered document-wide via `counter`.
fn nav_point(counter: &mut usize, src: &str, label: &str, children: &[TocEntry]) -> Element {
    let id = format!("navPoint-{counter}");
    *counter += 1;
    let mut point = Element::new("navPoint")
        .attr("id", &id)
        .child(Element::new("navLabel").child(Element::new("text").text(label)))
        .child(Element::new("content").attr("src", src));
    for child in children {
        point.push(entry_nav_point(child, counter));
    }
    point
}
