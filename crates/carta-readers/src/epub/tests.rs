use super::EpubReader;
use carta_ast::{Block, Inline, MetaValue};
use carta_core::container::zip::ZipArchive;
use carta_core::{BytesReader, Extension, Extensions, MediaBag, ReaderOptions};

const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

fn options() -> ReaderOptions {
    let mut options = ReaderOptions::default();
    options.extensions = Extensions::from_list(&[
        Extension::NativeDivs,
        Extension::NativeSpans,
        Extension::RawHtml,
    ]);
    options
}

fn build(opf: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = ZipArchive::new();
    archive
        .store("mimetype", b"application/epub+zip")
        .expect("store mimetype");
    archive
        .deflate("META-INF/container.xml", CONTAINER.as_bytes())
        .expect("store container");
    archive
        .deflate("OEBPS/content.opf", opf.as_bytes())
        .expect("store opf");
    for (name, data) in files {
        archive.deflate(name, data).expect("store file");
    }
    archive.finish().expect("finish archive")
}

fn read(opf: &str, files: &[(&str, &[u8])]) -> (carta_ast::Document, MediaBag) {
    EpubReader
        .read_media(&build(opf, files), &options())
        .expect("read epub")
}

fn opf_with(metadata: &str, manifest: &str, spine: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">{metadata}</metadata>
  <manifest>{manifest}</manifest>
  <spine>{spine}</spine>
</package>"#
    )
}

#[test]
fn spine_content_is_concatenated_with_anchors() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>
               <item id="b" href="b.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/><itemref idref="b"/>"#,
    );
    let a = b"<html><body><h1>First</h1></body></html>";
    let b = b"<html><body><p>Second</p></body></html>";
    let (document, _) = read(&opf, &[("OEBPS/a.xhtml", a), ("OEBPS/b.xhtml", b)]);

    let anchors: Vec<&str> = document
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Para(inlines) => match inlines.as_slice() {
                [Inline::Span(attr, _)] => Some(attr.id.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(anchors, ["a.xhtml", "b.xhtml"]);
    assert!(
        document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Header(1, _, _)))
    );
}

#[test]
fn non_linear_items_are_skipped() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>
               <item id="b" href="b.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a" linear="no"/><itemref idref="b"/>"#,
    );
    let doc = b"<html><body><p>x</p></body></html>";
    let (document, _) = read(&opf, &[("OEBPS/a.xhtml", doc), ("OEBPS/b.xhtml", doc)]);
    let anchors: Vec<&str> = document
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Para(inlines) => match inlines.as_slice() {
                [Inline::Span(attr, _)] => Some(attr.id.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(anchors, ["b.xhtml"]);
}

#[test]
fn creators_become_reversed_author_list() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier>
               <dc:title>Only Title</dc:title>
               <dc:creator>First</dc:creator>
               <dc:creator>Second</dc:creator>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/>"#,
    );
    let (document, _) = read(&opf, &[("OEBPS/a.xhtml", b"<html><body/></html>")]);
    let authors = document.meta.get("author").expect("author metadata");
    let names: Vec<String> = match authors {
        MetaValue::MetaList(items) => items
            .iter()
            .map(|item| match item {
                MetaValue::MetaInlines(inlines) => match inlines.as_slice() {
                    [Inline::Str(name)] => name.to_string(),
                    _ => String::new(),
                },
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    assert_eq!(names, ["Second", "First"]);
    assert!(matches!(
        document.meta.get("title"),
        Some(MetaValue::MetaInlines(_))
    ));
}

#[test]
fn title_page_is_dropped_but_anchor_remains() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/>"#,
    );
    let a = br#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
            <section epub:type="titlepage"><h1>The Title</h1></section></body></html>"#;
    let (document, _) = read(&opf, &[("OEBPS/a.xhtml", a)]);
    assert_eq!(document.blocks.len(), 1);
    assert!(matches!(document.blocks.first(), Some(Block::Para(_))));
}

#[test]
fn role_attribute_becomes_a_class() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/>"#,
    );
    let a = br#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
            <div epub:type="cover"><p>c</p></div></body></html>"#;
    let (document, _) = read(&opf, &[("OEBPS/a.xhtml", a)]);
    let has_cover = document.blocks.iter().any(|block| match block {
        Block::Div(attr, _) => attr.classes.iter().any(|class| class == "cover"),
        _ => false,
    });
    assert!(has_cover);
}

#[test]
fn identifiers_are_namespaced_and_fragment_links_rewritten() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>
               <item id="b" href="b.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/><itemref idref="b"/>"#,
    );
    let a = br#"<html><body><section id="intro"><h1>Intro</h1></section></body></html>"#;
    let b = br##"<html><body><p><a href="a.xhtml#intro">x</a> <a href="#local">y</a>
            <a href="http://e.com/p#f">z</a></p></body></html>"##;
    let (document, _) = read(&opf, &[("OEBPS/a.xhtml", a), ("OEBPS/b.xhtml", b)]);

    let namespaced = document.blocks.iter().any(|block| match block {
        Block::Div(attr, _) => attr.id == "a.xhtml_intro",
        _ => false,
    });
    assert!(namespaced);

    let mut urls = Vec::new();
    carta_core::walk::for_each_link_target(&mut document.blocks.clone(), &mut |target| {
        urls.push(target.url.to_string());
    });
    assert!(urls.contains(&"#a.xhtml_intro".to_string()));
    assert!(urls.contains(&"#b.xhtml_local".to_string()));
    assert!(urls.contains(&"http://e.com/p#f".to_string()));
}

#[test]
fn images_are_resolved_into_the_media_bag() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="text/a.xhtml" media-type="application/xhtml+xml"/>
               <item id="img" href="media/p.png" media-type="image/png"/>"#,
        r#"<itemref idref="a"/>"#,
    );
    let a = br#"<html><body><p><img src="../media/p.png" alt="x"/></p></body></html>"#;
    let png = b"\x89PNG\r\n\x1a\nDATA";
    let (document, media) = read(
        &opf,
        &[("OEBPS/text/a.xhtml", a), ("OEBPS/media/p.png", png)],
    );

    assert!(media.contains("media/p.png"));
    assert_eq!(
        media
            .get("media/p.png")
            .and_then(|item| item.mime.as_deref()),
        Some("image/png")
    );
    let mut urls = Vec::new();
    carta_core::walk::for_each_image_target(&mut document.blocks.clone(), &mut |target| {
        urls.push(target.url.to_string());
    });
    assert_eq!(urls, ["media/p.png"]);
}

#[test]
fn chapter_and_subchapter_sections_are_flattened() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/>"#,
    );
    let a = br#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
            <section epub:type="chapter" id="ch1"><h1>Chapter</h1><p>body</p>
            <section epub:type="subchapter" id="sub"><h2>Sub</h2><p>more</p></section>
            </section></body></html>"#;
    let (document, _) = read(&opf, &[("OEBPS/a.xhtml", a)]);
    assert!(
        !document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Div(..)))
    );
    assert!(
        document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Header(1, _, _)))
    );
    assert!(
        document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Header(2, _, _)))
    );
}

#[test]
fn halftitlepage_and_toc_sections_are_dropped() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/>"#,
    );
    let a = br#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
            <section epub:type="halftitlepage"><h1>Half</h1></section>
            <section epub:type="toc"><p>contents</p></section>
            <p>kept</p></body></html>"#;
    let (document, _) = read(&opf, &[("OEBPS/a.xhtml", a)]);
    assert!(
        !document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Header(..) | Block::Div(..)))
    );
    let has_kept = document.blocks.iter().any(|block| match block {
        Block::Para(inlines) => inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Str(text) if text == "kept")),
        _ => false,
    });
    assert!(has_kept);
}

#[test]
fn referenced_notes_are_inlined_and_orphans_dropped() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/>"#,
    );
    let a = br##"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
            <p>See<a epub:type="noteref" href="#fn1">1</a>.</p>
            <aside epub:type="footnote" id="fn1"><p>Note one.</p></aside>
            <aside epub:type="rearnote" id="fn2"><p>Orphan.</p></aside>
            </body></html>"##;
    let (document, _) = read(&opf, &[("OEBPS/a.xhtml", a)]);
    assert!(
        !document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Div(..)))
    );
    let note_count = document
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Para(inlines) => Some(inlines),
            _ => None,
        })
        .flat_map(|inlines| inlines.iter())
        .filter(|inline| matches!(inline, Inline::Note(_)))
        .count();
    assert_eq!(note_count, 1);
    let mut has_link = false;
    carta_core::walk::for_each_link_target(&mut document.blocks.clone(), &mut |_| {
        has_link = true;
    });
    assert!(!has_link);
}

/// Whether a block is a text block whose inlines include a lifted note.
fn block_holds_note(block: &Block) -> bool {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Note(_))),
        _ => false,
    }
}

#[test]
fn notes_defined_inside_a_cell_and_a_definition_are_lifted() {
    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/>"#,
    );
    let a = br##"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
            <table><tr><td>cell<a epub:type="noteref" href="#fn1">1</a><aside epub:type="footnote" id="fn1"><p>table note</p></aside></td></tr></table>
            <dl><dt>Term</dt><dd>def<a epub:type="noteref" href="#fn2">2</a><aside epub:type="footnote" id="fn2"><p>dd note</p></aside></dd></dl>
            </body></html>"##;
    let (document, _) = read(&opf, &[("OEBPS/a.xhtml", a)]);

    let cell_has_note = document.blocks.iter().any(|block| match block {
        Block::Table(table) => table
            .bodies
            .iter()
            .flat_map(|body| &body.body)
            .flat_map(|row| &row.cells)
            .flat_map(|cell| &cell.content)
            .any(block_holds_note),
        _ => false,
    });
    assert!(
        cell_has_note,
        "footnote defined in a cell should be lifted into it"
    );

    let definition_has_note = document.blocks.iter().any(|block| match block {
        Block::DefinitionList(items) => items
            .iter()
            .flat_map(|(_, defs)| defs)
            .flatten()
            .any(block_holds_note),
        _ => false,
    });
    assert!(
        definition_has_note,
        "footnote defined in a definition should be lifted into it"
    );

    // Both note containers are consumed, so no reference link survives.
    let mut has_link = false;
    carta_core::walk::for_each_link_target(&mut document.blocks.clone(), &mut |_| {
        has_link = true;
    });
    assert!(!has_link);
}

#[test]
fn malformed_archive_is_an_error() {
    assert!(EpubReader.read(b"not a zip", &options()).is_err());
}

/// Counts how deeply `Div` blocks nest, following one `Div` child per level. The walk is
/// iterative so it stays shallow even when the tree it inspects is thousands of levels deep.
fn div_nesting_depth(blocks: &[Block]) -> usize {
    let mut depth = 0;
    let mut level = blocks;
    while let Some(inner) = level.iter().find_map(|block| match block {
        Block::Div(_, inner) => Some(inner.as_slice()),
        _ => None,
    }) {
        depth += 1;
        level = inner;
    }
    depth
}

#[test]
fn deeply_nested_markup_reads_from_a_small_caller_stack() {
    const DEPTH: usize = 6000;
    let mut body = String::with_capacity(DEPTH * 12 + 64);
    body.push_str("<html><body>");
    for _ in 0..DEPTH {
        body.push_str("<div>");
    }
    body.push_str("leaf");
    for _ in 0..DEPTH {
        body.push_str("</div>");
    }
    body.push_str("</body></html>");

    let opf = opf_with(
        r#"<dc:identifier id="id">ID</dc:identifier><dc:title>T</dc:title>"#,
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/>"#,
    );
    let epub = build(&opf, &[("OEBPS/a.xhtml", body.as_bytes())]);

    // A shallow caller stack proves the read runs on its own deep worker stack. The tree is
    // too deep to drop recursively, so it is leaked after an iterative depth check.
    let depth = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            let (document, _media) = EpubReader
                .read_media(&epub, &options())
                .expect("read deeply nested epub");
            let depth = div_nesting_depth(&document.blocks);
            std::mem::forget(document);
            depth
        })
        .expect("spawn shallow caller")
        .join()
        .expect("shallow caller finished");

    assert_eq!(depth, DEPTH);
}
