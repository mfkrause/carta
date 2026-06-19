//! Facade integration tests: the high-level `convert` entry point and format-name resolution. These
//! run fully offline — outputs are the writers' own deterministic text. The error-classification cases
//! are feature-gated so they assert the right branch under both `--all-features` and a minimal
//! `--no-default-features --features read-commonmark,write-html` build.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use carta::{Error, ReaderOptions, WriterOptions, convert, reader_for, writer_for};

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn commonmark_to_html() {
    let output = convert(
        "commonmark",
        "html",
        "# Hi\n",
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap();
    // `convert` returns no trailing newline.
    assert_eq!(output, "<h1>Hi</h1>");
}

#[cfg(all(feature = "read-json", feature = "write-json"))]
#[test]
fn json_round_trips() {
    let input = r#"{"pandoc-api-version":[1,23,1,2],"meta":{},"blocks":[{"t":"Para","c":[{"t":"Str","c":"hi"}]}]}"#;
    let output = convert(
        "json",
        "json",
        input,
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap();
    assert_eq!(output, input);
}

#[test]
fn unknown_input_format_is_unsupported() {
    let error = reader_for("docx").err().expect("expected an error");
    assert!(matches!(error, Error::UnsupportedFormat(_)), "{error:?}");
}

#[test]
fn unknown_output_format_is_unsupported() {
    let error = writer_for("pdf").err().expect("expected an error");
    assert!(matches!(error, Error::UnsupportedFormat(_)), "{error:?}");
}

#[cfg(not(feature = "read-json"))]
#[test]
fn recognized_but_disabled_input_is_not_enabled() {
    let error = reader_for("json").err().expect("expected an error");
    assert!(matches!(error, Error::FormatNotEnabled(_)), "{error:?}");
}

#[cfg(feature = "read-commonmark")]
#[test]
fn supported_input_formats_reflect_build() {
    assert!(carta::supported_input_formats().contains(&"commonmark"));
}

#[test]
fn every_supported_format_resolves() {
    for name in carta::supported_input_formats() {
        assert!(
            reader_for(name).is_ok(),
            "input format {name} does not resolve"
        );
    }
    for name in carta::supported_output_formats() {
        assert!(
            writer_for(name).is_ok(),
            "output format {name} does not resolve"
        );
    }
}

#[test]
fn format_names_are_sorted_and_every_name_resolves() {
    let inputs = carta::input_format_names();
    let mut sorted = inputs.clone();
    sorted.sort_unstable();
    assert_eq!(inputs, sorted, "input names are not sorted");
    for name in &inputs {
        assert!(reader_for(name).is_ok(), "input name {name} does not resolve");
    }

    let outputs = carta::output_format_names();
    let mut sorted = outputs.clone();
    sorted.sort_unstable();
    assert_eq!(outputs, sorted, "output names are not sorted");
    for name in &outputs {
        assert!(
            writer_for(name).is_ok(),
            "output name {name} does not resolve"
        );
    }
}

#[cfg(feature = "read-commonmark")]
#[test]
fn format_names_include_aliases() {
    let inputs = carta::input_format_names();
    for alias in ["commonmark", "commonmark_x", "gfm", "markdown"] {
        assert!(inputs.contains(&alias), "missing input alias {alias}");
    }
}

#[test]
fn format_extensions_default_is_the_markdown_dialect() {
    let entries = carta::format_extensions(None).unwrap();
    let enabled = |name: &str| {
        entries
            .iter()
            .find(|(ext, _)| ext.name() == name)
            .map(|(_, on)| *on)
    };
    assert_eq!(enabled("smart"), Some(true));
    assert_eq!(enabled("gfm_auto_identifiers"), Some(false));

    // Sorted by extension name.
    let names: Vec<&str> = entries.iter().map(|(ext, _)| ext.name()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
}

#[test]
fn format_extensions_honor_a_format_spec() {
    let entries = carta::format_extensions(Some("commonmark+strikeout")).unwrap();
    let enabled = |name: &str| entries.iter().any(|(ext, on)| ext.name() == name && *on);
    assert!(enabled("raw_html"));
    assert!(enabled("strikeout"));
    assert!(!entries.iter().any(|(ext, on)| ext.name() == "smart" && *on));
}

#[test]
fn format_extensions_reject_an_unknown_format() {
    let error = carta::format_extensions(Some("bogus")).unwrap_err();
    assert!(matches!(error, Error::UnsupportedFormat(name) if name == "bogus"));
}
