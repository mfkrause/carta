//! Facade integration tests for embedded-media handling: reading a notebook lifts its image bytes
//! into the media bag under a content-addressed name, rendering back re-embeds them from the bag,
//! and the extract transform rewrites references to on-disk paths. These run fully offline; the
//! bytes and names are the code's own deterministic output.

#![cfg(all(feature = "read-ipynb", feature = "write-ipynb"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use carta::media::{base64_decode, content_addressed_name, rewrite_extracted_references};
use carta::{Output, ReaderOptions, WriterOptions, read_document, render_document};

// A one-cell notebook whose code cell emits a single PNG display output.
const NOTEBOOK_WITH_IMAGE: &str = r#"{"cells":[{"cell_type":"code","execution_count":1,"metadata":{},"outputs":[{"output_type":"display_data","data":{"image/png":"iVBORw0KGgoAAAANSUhEUg=="},"metadata":{}}],"source":["draw()"]}],"metadata":{"kernelspec":{"display_name":"Python 3","language":"python","name":"python3"}},"nbformat":4,"nbformat_minor":5}"#;

const IMAGE_BASE64: &str = "iVBORw0KGgoAAAANSUhEUg==";

#[test]
fn reading_a_notebook_lifts_image_bytes_into_the_bag() {
    let (_document, media) = read_document(
        "ipynb",
        NOTEBOOK_WITH_IMAGE.as_bytes(),
        &ReaderOptions::default(),
    )
    .unwrap();

    let bytes = base64_decode(IMAGE_BASE64).unwrap();
    let name = content_addressed_name("image/png", &bytes);
    let item = media.get(&name).expect("image is in the bag");
    assert_eq!(item.mime.as_deref(), Some("image/png"));
    assert_eq!(item.bytes, bytes);
}

#[test]
fn rendering_back_to_a_notebook_re_embeds_from_the_bag() {
    let (document, media) = read_document(
        "ipynb",
        NOTEBOOK_WITH_IMAGE.as_bytes(),
        &ReaderOptions::default(),
    )
    .unwrap();

    let Output::Text(rendered) =
        render_document("ipynb", document, media, &WriterOptions::default()).unwrap()
    else {
        panic!("ipynb output is text");
    };
    assert!(
        rendered.contains(IMAGE_BASE64),
        "re-embedded payload missing from output:\n{rendered}"
    );
}

// A notebook re-embeds, so extraction targets a text format here.
#[cfg(feature = "write-markdown")]
#[test]
fn extraction_rewrites_references_to_the_target_directory() {
    let (mut document, media) = read_document(
        "ipynb",
        NOTEBOOK_WITH_IMAGE.as_bytes(),
        &ReaderOptions::default(),
    )
    .unwrap();

    let bytes = base64_decode(IMAGE_BASE64).unwrap();
    let name = content_addressed_name("image/png", &bytes);

    rewrite_extracted_references(&mut document.blocks, &media, "assets/img");

    // Empty bag: the writer emits the rewritten external reference, not re-embedded bytes.
    let Output::Text(rendered) = render_document(
        "markdown",
        document,
        carta::MediaBag::new(),
        &WriterOptions::default(),
    )
    .unwrap() else {
        panic!("markdown output is text");
    };
    assert!(
        rendered.contains(&format!("assets/img/{name}")),
        "rewritten reference missing from output:\n{rendered}"
    );
    assert!(
        !rendered.contains(IMAGE_BASE64),
        "bytes should not be re-embedded after extraction:\n{rendered}"
    );
}
