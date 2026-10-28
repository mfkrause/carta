use super::{MAX_REPEATED_SPACES, OdtReader, parse_length};
use carta_ast::{Attr, Block, Inline};
use carta_core::container::zip::ZipArchive;
use carta_core::{BytesReader, ReaderOptions};

/// Wraps body markup in a minimal `content.xml` document.
fn content(body: &str) -> String {
    format!(
        "<office:document-content>\
             <office:body><office:text>{body}</office:text></office:body>\
             </office:document-content>"
    )
}

/// Packages named parts into an ODT (ZIP) archive.
fn package(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = ZipArchive::new();
    for (name, data) in parts {
        archive.deflate(name, data).expect("store part");
    }
    archive.finish().expect("finish archive")
}

fn read(input: &[u8]) -> carta_core::Result<carta_ast::Document> {
    OdtReader.read(input, &ReaderOptions::default())
}

#[test]
fn a_well_formed_package_reads_its_body() {
    let odt = package(&[("content.xml", content("<text:p>Hi</text:p>").as_bytes())]);
    let document = read(&odt).expect("read odt");
    assert_eq!(
        document.blocks,
        vec![Block::Para(vec![Inline::Str("Hi".into())])]
    );
}

#[test]
fn table_caption_style_keeps_its_class() {
    let body = content(
        "<text:p text:style-name=\"Table\">Caption</text:p>\
         <text:p text:style-name=\"Alias\">Alias</text:p>\
         <text:p text:style-name=\"_54_able\">Encoded</text:p>",
    );
    let styles = "<office:document-styles><office:styles>\
        <style:style style:name=\"Table\" style:family=\"paragraph\"/>\
        <style:style style:name=\"Alias\" style:family=\"paragraph\" style:parent-style-name=\"Table\"/>\
        <style:style style:name=\"_54_able\" style:family=\"paragraph\"/>\
        </office:styles></office:document-styles>";
    let odt = package(&[
        ("content.xml", body.as_bytes()),
        ("styles.xml", styles.as_bytes()),
    ]);
    let document = read(&odt).expect("read caption");
    let caption = Block::Para(vec![Inline::Str("Caption".into())]);
    assert_eq!(
        document.blocks,
        vec![
            Block::Div(
                Box::new(Attr {
                    classes: vec!["caption".into()],
                    ..Attr::default()
                }),
                vec![caption.clone()]
            ),
            Block::Para(vec![Inline::Str("Alias".into())]),
            Block::Para(vec![Inline::Str("Encoded".into())]),
        ]
    );
    let caption_body = content("<text:p text:style-name=\"Table\">Caption</text:p>");
    let unstyled = package(&[("content.xml", caption_body.as_bytes())]);
    assert_eq!(
        read(&unstyled).expect("read unstyled caption").blocks,
        vec![caption]
    );
    let preformatted_styles = styles.replace(
        "<style:style style:name=\"Table\" style:family=\"paragraph\"/>",
        "<style:style style:name=\"Table\" style:family=\"paragraph\" style:parent-style-name=\"Preformatted_20_Text\"/>\
         <style:style style:name=\"Preformatted_20_Text\" style:family=\"paragraph\"/>",
    );
    let preformatted = package(&[
        ("content.xml", caption_body.as_bytes()),
        ("styles.xml", preformatted_styles.as_bytes()),
    ]);
    assert_eq!(
        read(&preformatted).expect("read code caption").blocks,
        vec![Block::CodeBlock(Box::default(), "Caption".into())]
    );
}

#[test]
fn figure_caption_labels_do_not_change_image_alt_text() {
    for (original, expected) in [
        ("Figure 1: Caption", "Caption"),
        ("Illustration 2: Caption", "Caption"),
        ("Label :: Caption", "Caption"),
        ("Figure 1.2: Caption", "Figure 1.2: Caption"),
        ("Figure ١: Caption", "Figure ١: Caption"),
    ] {
        let body = content(&format!(
            "<text:p><draw:frame><draw:text-box><text:p>\
             <draw:frame><draw:image xlink:href=\"image.png\"/></draw:frame>{original}\
             </text:p></draw:text-box></draw:frame></text:p>"
        ));
        let odt = package(&[("content.xml", body.as_bytes())]);
        let document = read(&odt).expect("read figure");
        let Some(Block::Figure(_, caption, blocks)) = document.blocks.first() else {
            panic!("expected a figure");
        };
        assert_eq!(
            carta_ast::to_plain_text(carta_ast::single_block_inlines(&caption.long)),
            expected
        );
        let Some(Block::Plain(inlines)) = blocks.first() else {
            panic!("expected the figure body");
        };
        let Some(Inline::Image(_, alt, _)) = inlines.first() else {
            panic!("expected an image");
        };
        assert_eq!(carta_ast::to_plain_text(alt), original);
    }
}

#[test]
fn a_missing_content_part_is_an_error() {
    let odt = package(&[("styles.xml", b"<office:document-styles/>")]);
    assert!(read(&odt).is_err());
}

#[test]
fn an_unparsable_content_part_is_an_error() {
    let odt = package(&[("content.xml", b"%%% not markup %%%")]);
    assert!(read(&odt).is_err());
}

#[test]
fn a_pathological_space_repeat_is_clamped_not_crashed() {
    // `usize::MAX` spaces would exhaust memory; the count is bounded instead.
    let body = "<text:p>A<text:s text:c=\"18446744073709551615\"/>B</text:p>";
    let odt = package(&[("content.xml", content(body).as_bytes())]);
    let document = read(&odt).expect("read odt");
    let Some(Block::Para(inlines)) = document.blocks.first() else {
        panic!("expected a paragraph");
    };
    let spaces = inlines
        .iter()
        .filter(|inline| matches!(inline, Inline::Space))
        .count();
    assert_eq!(spaces, MAX_REPEATED_SPACES);
}

#[test]
fn parse_length_resolves_absolute_units_only() {
    assert_eq!(parse_length("0.5in"), Some(0.5));
    assert_eq!(parse_length("2.54cm"), Some(1.0));
    assert_eq!(parse_length("72pt"), Some(1.0));
    assert_eq!(parse_length("50%"), None);
    assert_eq!(parse_length("5"), None);
    assert_eq!(parse_length("10zz"), None);
}
