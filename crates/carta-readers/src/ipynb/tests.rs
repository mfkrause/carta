use super::*;
use carta_core::MediaBag;

fn read(input: &str) -> Document {
    IpynbReader
        .read(input, &ReaderOptions::default())
        .expect("notebook input parses")
}

fn read_with(input: &str, extensions: carta_core::Extensions) -> Document {
    let mut options = ReaderOptions::default();
    options.extensions = extensions;
    IpynbReader.read(input, &options).expect("notebook parses")
}

fn read_media(input: &str) -> (Document, MediaBag) {
    IpynbReader
        .read_media(input, &ReaderOptions::default())
        .expect("notebook input parses")
}

fn jupyter(document: &Document) -> &BTreeMap<carta_ast::Text, MetaValue> {
    match document.meta.get("jupyter") {
        Some(MetaValue::MetaMap(map)) => map,
        _ => panic!("expected a jupyter metadata map"),
    }
}

#[test]
fn empty_notebook_exposes_only_version_metadata() {
    let document = read(r#"{"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#);
    assert!(document.blocks.is_empty());
    let map = jupyter(&document);
    assert_eq!(
        map.get("nbformat"),
        Some(&MetaValue::MetaString("4".to_owned().into()))
    );
    assert_eq!(
        map.get("nbformat_minor"),
        Some(&MetaValue::MetaString("5".to_owned().into()))
    );
}

#[test]
fn missing_minor_version_defaults_to_zero() {
    let document = read(r#"{"cells": [], "metadata": {}, "nbformat": 4}"#);
    assert_eq!(
        jupyter(&document).get("nbformat_minor"),
        Some(&MetaValue::MetaString("0".to_owned().into()))
    );
}

#[test]
fn metadata_scalars_normalize_and_recurse() {
    let document = read(
        r#"{"cells": [], "metadata": {"afloat": 3.0, "aint": 7, "abool": true,
               "anull": null, "alist": [1, "two", 3.0], "amap": {"z": 1, "a": 2.0}},
               "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let map = jupyter(&document);
    assert_eq!(
        map.get("afloat"),
        Some(&MetaValue::MetaString("3".to_owned().into()))
    );
    assert_eq!(
        map.get("aint"),
        Some(&MetaValue::MetaString("7".to_owned().into()))
    );
    assert_eq!(map.get("abool"), Some(&MetaValue::MetaBool(true)));
    assert_eq!(
        map.get("anull"),
        Some(&MetaValue::MetaString(carta_ast::Text::default()))
    );
    assert_eq!(
        map.get("alist"),
        Some(&MetaValue::MetaList(vec![
            MetaValue::MetaString("1".to_owned().into()),
            MetaValue::MetaString("two".to_owned().into()),
            MetaValue::MetaString("3".to_owned().into()),
        ]))
    );
    let Some(MetaValue::MetaMap(nested)) = map.get("amap") else {
        panic!("expected a nested map");
    };
    assert_eq!(
        nested.get("a"),
        Some(&MetaValue::MetaString("2".to_owned().into()))
    );
    assert_eq!(
        nested.get("z"),
        Some(&MetaValue::MetaString("1".to_owned().into()))
    );
}

#[test]
fn markdown_cell_becomes_a_div_with_parsed_blocks() {
    let document = read(
        r##"{"cells": [{"cell_type": "markdown", "id": "m1", "metadata": {},
               "source": ["# Title\n", "\n", "text"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"##,
    );
    let Some(Block::Div(attr, blocks)) = document.blocks.first() else {
        panic!("expected a cell div");
    };
    assert_eq!(attr.id, "m1");
    assert_eq!(attr.classes, vec!["cell".to_owned(), "markdown".to_owned()]);
    assert!(matches!(blocks.first(), Some(Block::Header(1, _, _))));
    assert!(matches!(blocks.get(1), Some(Block::Para(_))));
}

#[test]
fn markdown_cell_honors_forwarded_extensions() {
    // A pipe table parses only with the table extension on.
    let input = r#"{"cells": [{"cell_type": "markdown", "metadata": {},
            "source": ["| a | b |\n|---|---|\n| 1 | 2 |\n"]}],
            "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#;
    let with_tables = read_with(input, carta_core::presets::GFM);
    let Some(Block::Div(_, blocks)) = with_tables.blocks.first() else {
        panic!("expected a cell div");
    };
    assert!(matches!(blocks.first(), Some(Block::Table(_))));

    let strict = read_with(input, carta_core::Extensions::empty());
    let Some(Block::Div(_, blocks)) = strict.blocks.first() else {
        panic!("expected a cell div");
    };
    assert!(!matches!(blocks.first(), Some(Block::Table(_))));
}

#[test]
fn markdown_attachment_prefix_is_stripped_from_images() {
    let document = read(
        r#"{"cells": [{"cell_type": "markdown", "metadata": {},
               "attachments": {"a.png": {"image/png": "x"}},
               "source": ["![alt](attachment:a.png)"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(_, blocks)) = document.blocks.first() else {
        panic!("expected a cell div");
    };
    let Some(Block::Para(inlines)) = blocks.first() else {
        panic!("expected a paragraph");
    };
    let Some(Inline::Image(_, _, target)) = inlines.first() else {
        panic!("expected an image");
    };
    // A cell without an `id` leaves the bare reference in place.
    assert_eq!(target.url, "a.png");
}

#[test]
fn markdown_attachment_reference_is_scoped_to_the_cell_id() {
    let document = read(
        r#"{"cells": [{"cell_type": "markdown", "id": "cell9", "metadata": {},
               "attachments": {"a.png": {"image/png": "x"}},
               "source": ["![alt](attachment:a.png)"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(_, blocks)) = document.blocks.first() else {
        panic!("expected a cell div");
    };
    let Some(Block::Para(inlines)) = blocks.first() else {
        panic!("expected a paragraph");
    };
    let Some(Inline::Image(_, _, target)) = inlines.first() else {
        panic!("expected an image");
    };
    // A cell with an `id` scopes the reference to that cell.
    assert_eq!(target.url, "cell9-a.png");
}

#[test]
fn code_cell_emits_source_then_outputs() {
    let document = read(
        r#"{"cells": [{"cell_type": "code", "metadata": {"scrolled": true},
               "execution_count": 5, "source": ["import os\n", "print(1)"],
               "outputs": [
                 {"output_type": "stream", "name": "stdout", "text": ["hello\n"]},
                 {"output_type": "execute_result", "execution_count": 5,
                  "data": {"text/plain": ["42"]}, "metadata": {}},
                 {"output_type": "error", "ename": "E", "evalue": "v",
                  "traceback": ["line1", "line2"]}
               ]}],
               "metadata": {"kernelspec": {"language": "python"}},
               "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(attr, blocks)) = document.blocks.first() else {
        panic!("expected a cell div");
    };
    assert_eq!(
        attr.attributes,
        vec![
            ("execution_count".into(), "5".into()),
            ("scrolled".into(), "true".into()),
        ]
    );
    let Some(Block::CodeBlock(source_attr, source)) = blocks.first() else {
        panic!("expected a source code block");
    };
    assert_eq!(source_attr.classes, vec!["python".to_owned()]);
    assert_eq!(source, "import os\nprint(1)");

    let Some(Block::Div(stream_attr, stream_body)) = blocks.get(1) else {
        panic!("expected a stream div");
    };
    assert_eq!(
        stream_attr.classes,
        vec![
            "output".to_owned(),
            "stream".to_owned(),
            "stdout".to_owned()
        ]
    );
    assert!(matches!(
        stream_body.first(),
        Some(Block::CodeBlock(_, text)) if text == "hello\n"
    ));

    let Some(Block::Div(result_attr, result_body)) = blocks.get(2) else {
        panic!("expected a result div");
    };
    assert_eq!(
        result_attr.classes,
        vec!["output".to_owned(), "execute_result".to_owned()]
    );
    assert_eq!(
        result_attr.attributes,
        vec![("execution_count".into(), "5".into())]
    );
    assert!(matches!(
        result_body.first(),
        Some(Block::CodeBlock(_, text)) if text == "42"
    ));

    let Some(Block::Div(error_attr, error_body)) = blocks.get(3) else {
        panic!("expected an error div");
    };
    assert_eq!(
        error_attr.attributes,
        vec![("ename".into(), "E".into()), ("evalue".into(), "v".into()),]
    );
    assert!(matches!(
        error_body.first(),
        Some(Block::CodeBlock(_, text)) if text == "line1\nline2\n"
    ));
}

#[test]
fn null_execution_count_yields_no_attribute() {
    let document = read(
        r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": null,
               "source": [], "outputs": []}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(attr, _)) = document.blocks.first() else {
        panic!("expected a cell div");
    };
    assert!(attr.attributes.is_empty());
}

#[test]
fn image_output_is_content_addressed() {
    // PNG bytes from base64 are hashed; SVG is hashed as its own text.
    let document = read(
        r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(_, body)) = first_output(&document) else {
        panic!("expected an output div");
    };
    let Some(Block::Para(inlines)) = body.first() else {
        panic!("expected a paragraph");
    };
    let Some(Inline::Image(_, _, target)) = inlines.first() else {
        panic!("expected an image");
    };
    assert_eq!(target.url, "22f545ac6b50163ce39bac49094c3f64e0858403.png");

    let svg = read(
        r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/svg+xml": ["<svg/>"]},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(_, body)) = first_output(&svg) else {
        panic!("expected an output div");
    };
    let Some(Block::Para(inlines)) = body.first() else {
        panic!("expected a paragraph");
    };
    let Some(Inline::Image(_, _, target)) = inlines.first() else {
        panic!("expected an image");
    };
    assert_eq!(target.url, "1c3ba3b813e1080e9721846f23a21c09e5c3fd27.svg");
}

#[test]
fn image_output_bytes_are_lifted_into_the_media_bag() {
    let (document, media) = read_media(
        r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(_, body)) = first_output(&document) else {
        panic!("expected an output div");
    };
    let Some(Block::Para(inlines)) = body.first() else {
        panic!("expected a paragraph");
    };
    let Some(Inline::Image(_, _, target)) = inlines.first() else {
        panic!("expected an image");
    };
    let name = "22f545ac6b50163ce39bac49094c3f64e0858403.png";
    assert_eq!(target.url, name);
    assert_eq!(media.len(), 1);
    let item = media.get(name).expect("image is in the bag");
    assert_eq!(item.mime.as_deref(), Some("image/png"));
    assert_eq!(
        item.bytes,
        carta_core::media::base64_decode("iVBORw0KGgoAAAANSUhEUg==").unwrap()
    );
}

#[test]
fn svg_output_is_stored_as_its_source_bytes() {
    let (_, media) = read_media(
        r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/svg+xml": ["<svg/>"]},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let name = "1c3ba3b813e1080e9721846f23a21c09e5c3fd27.svg";
    let item = media.get(name).expect("svg is in the bag");
    assert_eq!(item.mime.as_deref(), Some("image/svg+xml"));
    assert_eq!(item.bytes, b"<svg/>");
}

#[test]
fn identical_image_outputs_share_one_bag_entry() {
    let (_, media) = read_media(
        r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="},
                  "metadata": {}},
                 {"output_type": "display_data", "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    assert_eq!(media.len(), 1);
}

#[test]
fn markdown_attachment_bytes_are_lifted_into_the_media_bag() {
    let (_, media) = read_media(
        r#"{"cells": [{"cell_type": "markdown", "id": "cell9", "metadata": {},
               "attachments": {"a.png": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="}},
               "source": ["![alt](attachment:a.png)"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    // The attachment is keyed by the same cell-scoped name the image reference resolves to.
    let item = media.get("cell9-a.png").expect("attachment is in the bag");
    assert_eq!(item.mime.as_deref(), Some("image/png"));
    assert_eq!(
        item.bytes,
        carta_core::media::base64_decode("iVBORw0KGgoAAAANSUhEUg==").unwrap()
    );
}

#[test]
fn attachment_without_a_cell_id_uses_the_bare_reference() {
    let (_, media) = read_media(
        r#"{"cells": [{"cell_type": "markdown", "metadata": {},
               "attachments": {"a.png": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="}},
               "source": ["![alt](attachment:a.png)"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    assert!(media.contains("a.png"));
}

#[test]
fn image_wins_over_text_and_smaller_mime_wins_among_images() {
    let document = read(
        r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data",
                  "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg==", "image/jpeg": "iVBORw0KGgoAAAANSUhEUg==",
                           "text/plain": ["p"]},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(_, body)) = first_output(&document) else {
        panic!("expected an output div");
    };
    let Some(Block::Para(inlines)) = body.first() else {
        panic!("expected a paragraph");
    };
    let Some(Inline::Image(_, _, target)) = inlines.first() else {
        panic!("expected an image");
    };
    // image/jpeg sorts before image/png and both before text/plain.
    assert_eq!(target.url, "22f545ac6b50163ce39bac49094c3f64e0858403.jpg");
}

#[test]
fn image_output_metadata_becomes_sorted_attributes() {
    let document = read(
        r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"image/png": "iVBORw0KGgoAAAANSUhEUg=="},
                  "metadata": {"image/png": {"width": 100, "height": 50, "needs_background": "light"}}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(_, body)) = first_output(&document) else {
        panic!("expected an output div");
    };
    let Some(Block::Para(inlines)) = body.first() else {
        panic!("expected a paragraph");
    };
    let Some(Inline::Image(attr, _, _)) = inlines.first() else {
        panic!("expected an image");
    };
    assert_eq!(
        attr.attributes,
        vec![
            ("height".into(), "50".into()),
            ("needs_background".into(), "light".into()),
            ("width".into(), "100".into()),
        ]
    );
}

#[test]
fn structured_json_output_is_compact_and_sorted() {
    let document = read(
        r#"{"cells": [{"cell_type": "code", "metadata": {}, "execution_count": 1,
               "source": [], "outputs": [
                 {"output_type": "display_data", "data": {"application/json": {"z": 1, "a": 2.0}},
                  "metadata": {}}]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(_, body)) = first_output(&document) else {
        panic!("expected an output div");
    };
    let Some(Block::CodeBlock(attr, text)) = body.first() else {
        panic!("expected a code block");
    };
    assert_eq!(attr.classes, vec!["json".to_owned()]);
    assert_eq!(text, r#"{"a":2.0,"z":1}"#);
}

#[test]
fn raw_cell_maps_format_to_writer_name() {
    let document = read(
        r#"{"cells": [
                 {"cell_type": "raw", "metadata": {"format": "text/html"}, "source": ["<b>x</b>"]},
                 {"cell_type": "raw", "metadata": {}, "source": ["plain"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    let Some(Block::Div(attr, body)) = document.blocks.first() else {
        panic!("expected a raw cell div");
    };
    assert_eq!(attr.attributes, vec![("format".into(), "text/html".into())]);
    assert!(matches!(
        body.first(),
        Some(Block::RawBlock(Format(name), text)) if name == "html" && text == "<b>x</b>"
    ));
    // No declared format falls back to the notebook's own format name.
    let Some(Block::Div(_, body)) = document.blocks.get(1) else {
        panic!("expected a raw cell div");
    };
    assert!(matches!(
        body.first(),
        Some(Block::RawBlock(Format(name), _)) if name == "ipynb"
    ));
}

#[test]
fn unknown_cell_kinds_are_dropped() {
    let document = read(
        r#"{"cells": [{"cell_type": "heading", "level": 2, "metadata": {}, "source": ["H"]}],
               "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#,
    );
    assert!(document.blocks.is_empty());
}

#[test]
fn terminal_control_sequences_are_removed_from_text_outputs() {
    // The escape is assembled so this source file holds no literal control byte.
    let esc = format!("{}u001b", '\\');
    let input = format!(
        r#"{{"cells": [{{"cell_type": "code", "metadata": {{}}, "execution_count": 1,
               "source": [], "outputs": [
                 {{"output_type": "stream", "name": "stdout",
                  "text": ["{esc}[31mred{esc}[0m"]}}]}}],
               "metadata": {{}}, "nbformat": 4, "nbformat_minor": 5}}"#
    );
    let document = read(&input);
    let Some(Block::Div(_, body)) = first_output(&document) else {
        panic!("expected an output div");
    };
    assert!(matches!(
        body.first(),
        Some(Block::CodeBlock(_, text)) if text == "red"
    ));
}

#[test]
fn malformed_input_is_an_error_not_a_panic() {
    assert!(
        IpynbReader
            .read("not json", &ReaderOptions::default())
            .is_err()
    );
    assert!(IpynbReader.read("", &ReaderOptions::default()).is_err());
}

#[test]
fn pre_v4_notebook_is_an_error_not_a_panic() {
    let result = IpynbReader.read(
        r#"{"nbformat": 3, "nbformat_minor": 0, "worksheets": []}"#,
        &ReaderOptions::default(),
    );
    assert!(matches!(result, Err(Error::UnsupportedFormat(_))));
}

/// The body of the first output div of the first code cell.
fn first_output(document: &Document) -> Option<&Block> {
    let Some(Block::Div(_, blocks)) = document.blocks.first() else {
        return None;
    };
    blocks.get(1)
}
