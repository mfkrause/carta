//! Facade integration tests: the high-level `convert` entry point and format-name resolution. No
//! oracle needed — outputs are the writers' own deterministic text. The error-classification cases
//! are feature-gated so they assert the right branch under both `--all-features` and a minimal
//! `--no-default-features --features read-commonmark,write-html` build.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oxidoc::{Error, ReaderOptions, WriterOptions, convert, reader_for, writer_for};

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
    assert!(oxidoc::supported_input_formats().contains(&"commonmark"));
}

#[test]
fn every_supported_format_resolves() {
    for name in oxidoc::supported_input_formats() {
        assert!(
            reader_for(name).is_ok(),
            "input format {name} does not resolve"
        );
    }
    for name in oxidoc::supported_output_formats() {
        assert!(
            writer_for(name).is_ok(),
            "output format {name} does not resolve"
        );
    }
}
