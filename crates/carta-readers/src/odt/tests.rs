use super::{MAX_REPEATED_SPACES, OdtReader, parse_length};
use carta_ast::{Block, Inline};
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
    // A percentage, a unitless number, and an unknown unit name no absolute length.
    assert_eq!(parse_length("50%"), None);
    assert_eq!(parse_length("5"), None);
    assert_eq!(parse_length("10zz"), None);
}
