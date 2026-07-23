//! Building the fixed XHTML and XML parts of an EPUB: the page wrapper each chapter's body is
//! placed in, the generated title and cover pages, and the container's own bookkeeping files.

use super::Version;
use super::metadata::BookMeta;
use crate::html::render_epub_inlines;
use carta_ast::Inline;
use carta_core::container::xml::{DECLARATION, Element, escape_attribute, escape_text};
use std::fmt::Write as _;

/// The value recorded in each page's `generator` meta.
const GENERATOR: &str = "carta";

/// How a page's `<body>` is tagged. In the EPUB 3 dialect the front and body matter carry an
/// `epub:type`; EPUB 2 pages carry none. The cover page is an exception in both dialects: its body
/// is identified by `id="cover"` so a reading system can style it.
#[derive(Clone, Copy)]
pub(crate) enum BodyKind {
    Frontmatter,
    Bodymatter,
    Cover,
}

impl BodyKind {
    fn epub_type(self) -> &'static str {
        match self {
            BodyKind::Frontmatter => "frontmatter",
            BodyKind::Bodymatter => "bodymatter",
            BodyKind::Cover => "cover",
        }
    }

    /// The `<body …>` open tag for this page in the given dialect.
    fn open_tag(self, epub3: bool) -> String {
        match self {
            BodyKind::Cover => String::from("<body id=\"cover\">"),
            other if epub3 => format!("<body epub:type=\"{}\">", other.epub_type()),
            _ => String::from("<body>"),
        }
    }
}

/// Wrap a rendered XHTML `body` in the full page document for `version`: the XML declaration,
/// doctype, `<html>` root, and a `<head>` linking every stylesheet. `css_prefix` is the relative
/// path from the page to the container root (`""` at the root, `"../"` for a page under `text/`).
/// `style` is an inline stylesheet placed after the linked ones, empty when the page needs none.
#[allow(clippy::too_many_arguments)]
pub(crate) fn xhtml_page(
    version: Version,
    lang: &str,
    title: &str,
    css_prefix: &str,
    kind: BodyKind,
    stylesheets: &[String],
    style: &str,
    body: &str,
) -> String {
    let mut out = String::from(DECLARATION);
    let mut lang_attr = String::new();
    escape_attribute(lang, &mut lang_attr);
    let mut title_text = String::new();
    escape_text(title, &mut title_text);

    let mut links = String::new();
    for name in stylesheets {
        let _ = writeln!(
            links,
            "  <link rel=\"stylesheet\" type=\"text/css\" href=\"{css_prefix}styles/{name}\" />"
        );
    }

    if version.is_epub3() {
        out.push_str("<!DOCTYPE html>\n");
        let _ = writeln!(
            out,
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{lang_attr}\" xml:lang=\"{lang_attr}\">"
        );
        out.push_str("<head>\n");
        out.push_str("  <meta charset=\"utf-8\" />\n");
        let _ = writeln!(out, "  <meta name=\"generator\" content=\"{GENERATOR}\" />");
        let _ = writeln!(out, "  <title>{title_text}</title>");
        out.push_str("  <style>\n");
        out.push_str(style);
        out.push_str("  </style>\n");
        out.push_str(&links);
        out.push_str("</head>\n");
    } else {
        out.push_str(
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">\n",
        );
        let _ = writeln!(
            out,
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" lang=\"{lang_attr}\" xml:lang=\"{lang_attr}\">"
        );
        out.push_str("<head>\n");
        out.push_str(
            "  <meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\" />\n",
        );
        out.push_str("  <meta http-equiv=\"Content-Style-Type\" content=\"text/css\" />\n");
        let _ = writeln!(out, "  <meta name=\"generator\" content=\"{GENERATOR}\" />");
        let _ = writeln!(out, "  <title>{title_text}</title>");
        out.push_str("  <style type=\"text/css\">\n");
        out.push_str(style);
        out.push_str("  </style>\n");
        out.push_str(&links);
        out.push_str("</head>\n");
    }
    out.push_str(&kind.open_tag(version.is_epub3()));
    out.push('\n');
    // An empty body sits on a single line (`<body>\n</body>`); content keeps its own trailing break.
    if !body.is_empty() {
        out.push_str(body);
        out.push('\n');
    }
    out.push_str("</body>\n</html>\n");
    out
}

/// The generated title page: the work's title, subtitle, authors, publisher, date and rights, each
/// rendered as its own labelled element. In the EPUB 3 dialect these sit inside a `titlepage`
/// section; in EPUB 2 they stand directly in the body.
pub(crate) fn title_page(
    version: Version,
    meta: &BookMeta,
    display_title: &str,
    stylesheets: &[String],
) -> String {
    let epub3 = version.is_epub3();
    let mut fields: Vec<String> = Vec::new();
    if !meta.title_inlines.is_empty() {
        fields.push(field(
            "h1",
            "title",
            &render_epub_inlines(&meta.title_inlines, epub3),
        ));
    }
    if let Some(subtitle) = &meta.subtitle_inlines {
        fields.push(field(
            "p",
            "subtitle",
            &render_epub_inlines(subtitle, epub3),
        ));
    }
    for creator in &meta.creators {
        fields.push(field(
            "p",
            "author",
            &render_epub_inlines(&creator.inlines, epub3),
        ));
    }
    if let Some(publisher) = &meta.publisher {
        fields.push(field("p", "publisher", &escaped(publisher)));
    }
    if let Some(date) = &meta.date {
        fields.push(field("p", "date", &escaped(date)));
    }
    if let Some(rights) = &meta.rights_inlines {
        fields.push(field("div", "rights", &render_epub_inlines(rights, epub3)));
    }

    let body = if epub3 {
        let mut inner = String::new();
        for line in &fields {
            let _ = writeln!(inner, "{line}");
        }
        format!("<section epub:type=\"titlepage\" class=\"titlepage\">\n{inner}</section>")
    } else if fields.is_empty() {
        // XHTML 1.1 requires at least one block element in the body; fall back to an empty container.
        String::from("<div class=\"titlepage\"></div>")
    } else {
        fields.join("\n")
    };

    xhtml_page(
        version,
        &meta.language,
        display_title,
        "../",
        BodyKind::Frontmatter,
        stylesheets,
        "",
        &body,
    )
}

/// One `  <tag class="class">value</tag>` title-page field, indented two spaces.
fn field(tag: &str, class: &str, value: &str) -> String {
    format!("  <{tag} class=\"{class}\">{value}</{tag}>")
}

/// Render an inline sequence to escaped plain text (no markup), for a field whose value is a bare
/// string.
fn escaped(text: &str) -> String {
    let mut out = String::new();
    escape_text(text, &mut out);
    out
}

/// The generated cover page: an SVG that scales the cover image to the viewport, so a reader
/// displays the whole cover regardless of its pixel size. `title` labels the page and `lang` sets
/// its language, both inherited from the publication.
pub(crate) fn cover_page(
    version: Version,
    lang: &str,
    title: &str,
    image_href: &str,
    width: u32,
    height: u32,
    stylesheets: &[String],
) -> String {
    // A known pixel size pins the SVG viewport so the image scales exactly; an unknown size fills
    // the viewport and lets the reading system scale, avoiding a zero-sized box.
    let body = if width == 0 || height == 0 {
        format!(
            "<div id=\"cover-image\">\n\
             <svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" version=\"1.1\" width=\"100%\" height=\"100%\" preserveAspectRatio=\"xMidYMid meet\">\n\
             <image width=\"100%\" height=\"100%\" preserveAspectRatio=\"xMidYMid meet\" xlink:href=\"{image_href}\" />\n\
             </svg>\n\
             </div>"
        )
    } else {
        format!(
            "<div id=\"cover-image\">\n\
             <svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" version=\"1.1\" width=\"100%\" height=\"100%\" viewBox=\"0 0 {width} {height}\" preserveAspectRatio=\"xMidYMid\">\n\
             <image width=\"{width}\" height=\"{height}\" xlink:href=\"{image_href}\" />\n\
             </svg>\n\
             </div>"
        )
    };
    xhtml_page(
        version,
        lang,
        title,
        "../",
        BodyKind::Cover,
        stylesheets,
        "",
        &body,
    )
}

/// The `META-INF/container.xml` that points a reading system at the package document. `container_dir`
/// is the directory holding the content, or empty when it sits at the archive root.
pub(crate) fn container_xml(container_dir: &str) -> String {
    let full_path = if container_dir.is_empty() {
        String::from("content.opf")
    } else {
        format!("{container_dir}/content.opf")
    };
    Element::new("container")
        .attr("version", "1.0")
        .attr("xmlns", "urn:oasis:names:tc:opendocument:xmlns:container")
        .child(
            Element::new("rootfiles").child(
                Element::new("rootfile")
                    .attr("full-path", &full_path)
                    .attr("media-type", "application/oebps-package+xml"),
            ),
        )
        .render_document_pretty()
}

/// The Apple `com.apple.ibooks.display-options.xml` that opts embedded fonts into use.
pub(crate) fn ibooks_display_options() -> String {
    Element::new("display_options")
        .child(
            Element::new("platform").attr("name", "*").child(
                Element::new("option")
                    .attr("name", "specified-fonts")
                    .text("true"),
            ),
        )
        .render_document_pretty()
}

/// Render an inline sequence to a single line of plain text with no markup, collapsing spacing.
/// Used where a value must appear as an XML attribute or bare text.
pub(crate) fn inline_plain(inlines: &[Inline]) -> String {
    carta_ast::to_plain_text(inlines)
}

#[cfg(test)]
mod tests {
    use super::{Version, cover_page};

    #[test]
    fn cover_page_pins_the_viewport_to_a_known_size() {
        let page = cover_page(
            Version::Epub3,
            "en",
            "Title",
            "../media/file0.png",
            2,
            3,
            &[],
        );
        assert!(page.contains("viewBox=\"0 0 2 3\""));
        assert!(page.contains("<image width=\"2\" height=\"3\""));
    }

    #[test]
    fn cover_page_falls_back_to_full_bleed_without_a_known_size() {
        let page = cover_page(
            Version::Epub3,
            "en",
            "Title",
            "../media/file0.svg",
            0,
            0,
            &[],
        );
        // With no measurable size the fixed viewBox is dropped and the image fills the viewport.
        assert!(!page.contains("viewBox"));
        assert!(page.contains("<image width=\"100%\" height=\"100%\""));
    }
}
