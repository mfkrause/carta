//! Tests for the JSON interchange codec: the round-trip entry points and the hand-written
//! `Document` deserializer's error handling (the array/object-shaped records that serde derives
//! cannot express).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use carta_ast::{Document, from_json, to_json, to_json_writer};

const SAMPLE: &str = r#"{"pandoc-api-version":[1,23,1,2],"meta":{"k":{"t":"MetaBool","c":true}},"blocks":[{"t":"Para","c":[{"t":"Str","c":"hi"}]}]}"#;

#[test]
fn round_trips_through_string_and_writer() {
    let document = from_json(SAMPLE.as_bytes()).expect("parse sample");

    let string = to_json(&document).expect("to_json");
    assert_eq!(string, SAMPLE);

    let mut buffer = Vec::new();
    to_json_writer(&document, &mut buffer).expect("to_json_writer");
    assert_eq!(String::from_utf8(buffer).unwrap(), SAMPLE);

    let reparsed = from_json(string.as_bytes()).expect("reparse");
    assert_eq!(document, reparsed);
}

#[test]
fn meta_defaults_to_empty_when_absent() {
    let document =
        from_json(r#"{"pandoc-api-version":[1,23,1,2],"blocks":[]}"#.as_bytes()).expect("parse");
    assert!(document.meta.is_empty());
    assert!(document.blocks.is_empty());
}

#[test]
fn rejects_non_object_document() {
    assert!(from_json(b"[]").is_err());
}

#[test]
fn rejects_unknown_field() {
    let json = r#"{"pandoc-api-version":[1,23,1,2],"meta":{},"blocks":[],"extra":1}"#;
    assert!(from_json(json.as_bytes()).is_err());
}

#[test]
fn rejects_duplicate_fields() {
    let api = r#"{"pandoc-api-version":[1],"pandoc-api-version":[1],"meta":{},"blocks":[]}"#;
    let meta = r#"{"pandoc-api-version":[1,23,1,2],"meta":{},"meta":{},"blocks":[]}"#;
    let blocks = r#"{"pandoc-api-version":[1,23,1,2],"meta":{},"blocks":[],"blocks":[]}"#;
    for json in [api, meta, blocks] {
        assert!(from_json(json.as_bytes()).is_err(), "should reject: {json}");
    }
}

#[test]
fn rejects_missing_required_fields() {
    let no_version = r#"{"meta":{},"blocks":[]}"#;
    let no_blocks = r#"{"pandoc-api-version":[1,23,1,2],"meta":{}}"#;
    assert!(from_json(no_version.as_bytes()).is_err());
    assert!(from_json(no_blocks.as_bytes()).is_err());
}

#[test]
fn round_trips_a_default_document() {
    let document = Document::default();
    let json = to_json(&document).expect("to_json");
    assert_eq!(document, from_json(json.as_bytes()).expect("reparse"));
}
