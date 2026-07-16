//! Facade integration tests: the high-level `convert_text` and `convert` entry points and format-name resolution. These
//! run fully offline — outputs are the writers' own deterministic text. The error-classification cases
//! are feature-gated so they assert the right branch under both `--all-features` and a minimal
//! `--no-default-features --features read-commonmark,write-html` build.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use carta::{
    Error, Output, ReaderOptions, WriterOptions, any_reader_for, any_writer_for, convert,
    convert_text, reader_for, writer_for,
};

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn convert_on_text_target_matches_convert_text() {
    let bytes = convert(
        "commonmark",
        "html",
        b"# Hi\n",
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap();
    let text = convert_text(
        "commonmark",
        "html",
        "# Hi\n",
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap();
    assert_eq!(bytes, Output::Text(text));
}

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn convert_rejects_invalid_utf8_for_a_text_reader() {
    let error = convert(
        "commonmark",
        "html",
        &[0xff, 0xfe],
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::InvalidUtf8(_)), "{error:?}");
}

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn commonmark_to_html() {
    let output = convert_text(
        "commonmark",
        "html",
        "# Hi\n",
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )
    .unwrap();
    // `convert_text` returns no trailing newline.
    assert_eq!(output, "<h1>Hi</h1>");
}

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn number_sections_without_standalone_numbers_the_body_in_place() {
    let mut writer_options = WriterOptions::default();
    writer_options.number_sections = true;
    let output = convert_text(
        "commonmark",
        "html",
        "# First\n\n## Nested\n",
        &ReaderOptions::default(),
        &writer_options,
    )
    .unwrap();

    assert!(
        output.contains(r#"<span class="header-section-number">1</span>"#),
        "top heading not numbered: {output}"
    );
    assert!(
        output.contains(r#"<span class="header-section-number">1.1</span>"#),
        "nested heading not numbered: {output}"
    );
}

#[cfg(all(
    feature = "read-commonmark",
    feature = "write-html",
    feature = "standalone"
))]
#[test]
fn number_sections_with_standalone_toc_numbers_body_and_toc_exactly_once() {
    let mut writer_options = WriterOptions::default();
    writer_options.number_sections = true;
    writer_options.standalone = true;
    writer_options.toc = true;
    let output = convert_text(
        "commonmark",
        "html",
        "# First\n\n## Nested\n",
        &ReaderOptions::default(),
        &writer_options,
    )
    .unwrap();

    assert!(
        output.contains(r#"<span class="toc-section-number">1</span>"#)
            && output.contains(r#"<span class="toc-section-number">1.1</span>"#),
        "contents entries not numbered: {output}"
    );
    // The body headings carry a section number; the contents entries carry their own. Neither the
    // body nor the contents should show a doubled number, so each span appears exactly as many times
    // as there are headings.
    assert_eq!(
        output.matches(r#"class="header-section-number""#).count(),
        2,
        "body section numbers doubled or leaked into the contents: {output}"
    );
    assert_eq!(
        output.matches(r#"class="toc-section-number""#).count(),
        2,
        "contents section numbers doubled: {output}"
    );
}

#[cfg(all(feature = "read-json", feature = "write-json"))]
#[test]
fn json_round_trips() {
    let input = r#"{"pandoc-api-version":[1,23,1,2],"meta":{},"blocks":[{"t":"Para","c":[{"t":"Str","c":"hi"}]}]}"#;
    let output = convert_text(
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
    let error = reader_for("notaformat").err().expect("expected an error");
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
    // Resolution is kind-agnostic: a supported format resolves whether it is text- or byte-shaped
    // (`writer_for` deliberately rejects byte-shaped formats, so it cannot stand in here).
    for name in carta::supported_input_formats() {
        assert!(
            any_reader_for(name).is_ok(),
            "input format {name} does not resolve"
        );
    }
    for name in carta::supported_output_formats() {
        assert!(
            any_writer_for(name).is_ok(),
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
        assert!(
            any_reader_for(name).is_ok(),
            "input name {name} does not resolve"
        );
    }

    let outputs = carta::output_format_names();
    let mut sorted = outputs.clone();
    sorted.sort_unstable();
    assert_eq!(outputs, sorted, "output names are not sorted");
    for name in &outputs {
        assert!(
            any_writer_for(name).is_ok(),
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
