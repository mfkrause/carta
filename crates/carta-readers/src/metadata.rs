//! Standalone metadata files: a YAML or JSON document parsed into a metadata map on its own, apart
//! from any document's input format. Scalar values are read as inline Markdown — a `title: *Hi*`
//! yields emphasized inlines — so this layer fixes the dialect those scalars are parsed in and
//! delegates the per-scalar conversion to the Markdown reader.

use std::collections::BTreeMap;

use carta_ast::MetaValue;
use carta_core::{ReaderOptions, Result, presets};

/// The options governing metadata-scalar parsing: the broad Markdown dialect with greedy paragraphs,
/// so a scalar's inline markup is recognized the way a leading metadata block's is.
fn scalar_options() -> ReaderOptions {
    let mut options = ReaderOptions::default();
    options.extensions = presets::MARKDOWN;
    options.greedy_paragraphs = true;
    options
}

/// Parse a JSON metadata file into a metadata map. String and number scalars are read as inline
/// Markdown, booleans become [`MetaValue::MetaBool`], and arrays and objects recurse.
///
/// # Errors
/// [`carta_core::Error::InvalidMetadata`] if the content is not a valid JSON object.
pub fn parse_json(content: &str) -> Result<BTreeMap<String, MetaValue>> {
    crate::commonmark::parse_metadata_json(content, &scalar_options())
}

/// Parse a YAML metadata file (which also accepts single-line JSON) into a metadata map. A file that
/// is not a mapping contributes no metadata.
///
/// # Errors
/// [`carta_core::Error::InvalidMetadata`] if the content is not valid YAML.
pub fn parse_yaml(content: &str) -> Result<BTreeMap<String, MetaValue>> {
    crate::commonmark::parse_metadata_yaml(content, &scalar_options())
}

#[cfg(test)]
mod tests {
    use super::{parse_json, parse_yaml};
    use carta_ast::{Inline, MetaValue};

    fn emph(text: &str) -> MetaValue {
        MetaValue::MetaInlines(vec![Inline::Emph(vec![Inline::Str(
            text.to_owned().into(),
        )])])
    }

    #[test]
    fn yaml_scalar_is_parsed_as_inline_markdown() {
        let meta = parse_yaml("title: \"*Hi*\"\n").expect("valid yaml");
        assert_eq!(meta.get("title"), Some(&emph("Hi")));
    }

    #[test]
    fn yaml_that_is_not_a_mapping_yields_no_metadata() {
        assert!(
            parse_yaml("- just\n- a list\n")
                .expect("valid yaml")
                .is_empty()
        );
    }

    #[test]
    fn json_object_keeps_typed_scalars_and_parses_strings_as_markdown() {
        let meta = parse_json(r#"{"draft": true, "title": "*Hi*"}"#).expect("valid json");
        assert_eq!(meta.get("draft"), Some(&MetaValue::MetaBool(true)));
        assert_eq!(meta.get("title"), Some(&emph("Hi")));
    }

    #[test]
    fn a_json_metadata_file_must_be_an_object() {
        assert!(parse_json("[1, 2, 3]").is_err());
    }
}
