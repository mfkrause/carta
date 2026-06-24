//! Parsing of format specifiers of the form `base[+ext][-ext]…`.
//!
//! A specifier names a base format and a sequence of extension toggles applied onto that format's
//! default extension set: `+name` enables an extension, `-name` disables it. The base name (the text
//! before the first `+`/`-`) is what [`reader_for`](crate::reader_for)/[`writer_for`](crate::writer_for)
//! resolve; the resulting [`Extensions`] is merged into the reader/writer options.

use carta_core::{Error, Extension, Extensions, Result, presets};

/// The extensions enabled by default for `base`, before any `+`/`-` toggles.
fn default_extensions(base: &str) -> Extensions {
    match base {
        "commonmark" => Extensions::from_list(&[Extension::RawHtml]),
        "commonmark_x" => presets::COMMONMARK_X,
        "markdown" => presets::MARKDOWN,
        "markdown_github" => presets::MARKDOWN_GITHUB,
        "markdown_phpextra" => presets::MARKDOWN_PHPEXTRA,
        "markdown_mmd" => presets::MARKDOWN_MMD,
        "gfm" => presets::GFM,
        _ => Extensions::empty(),
    }
}

/// Splits a format specifier into its base name and the [`Extensions`] it selects.
///
/// The base is the text up to the first `+` or `-`; the remainder is a run of `+name`/`-name`
/// toggles applied onto [`default_extensions`].
///
/// # Errors
/// [`Error::UnknownExtension`] if a toggle names an extension this build does not recognize.
pub fn parse_format_spec(spec: &str) -> Result<(String, Extensions)> {
    let base_end = spec.find(['+', '-']).unwrap_or(spec.len());
    let (base, mut rest) = spec.split_at(base_end);
    let mut extensions = default_extensions(base);

    while !rest.is_empty() {
        let (enable, after_sign) = match rest.strip_prefix('+') {
            Some(tail) => (true, tail),
            None => match rest.strip_prefix('-') {
                Some(tail) => (false, tail),
                // `find(['+', '-'])` bounds the base, so a non-empty `rest` always starts with one.
                None => break,
            },
        };
        let token_end = after_sign.find(['+', '-']).unwrap_or(after_sign.len());
        let (name, remainder) = after_sign.split_at(token_end);
        rest = remainder;

        let extension =
            Extension::from_name(name).ok_or_else(|| Error::UnknownExtension(name.to_owned()))?;
        if enable {
            extensions.insert(extension);
        } else {
            extensions.remove(extension);
        }
    }

    Ok((base.to_owned(), extensions))
}

#[cfg(test)]
mod tests {
    use super::parse_format_spec;
    use carta_core::{Error, Extension};

    #[test]
    fn bare_commonmark_enables_raw_html_by_default() {
        let (base, ext) = parse_format_spec("commonmark").unwrap();
        assert_eq!(base, "commonmark");
        assert!(ext.contains(Extension::RawHtml));
        assert!(!ext.contains(Extension::Strikeout));
    }

    #[test]
    fn bare_non_markdown_format_has_no_extensions() {
        let (base, ext) = parse_format_spec("json").unwrap();
        assert_eq!(base, "json");
        assert!(ext.is_empty());
    }

    #[test]
    fn plus_enables_minus_disables() {
        let (base, ext) = parse_format_spec("commonmark+strikeout+subscript").unwrap();
        assert_eq!(base, "commonmark");
        assert!(ext.contains(Extension::Strikeout));
        assert!(ext.contains(Extension::Subscript));
        assert!(ext.contains(Extension::RawHtml));

        let (_, ext) = parse_format_spec("commonmark-raw_html").unwrap();
        assert!(!ext.contains(Extension::RawHtml));
        assert!(ext.is_empty());
    }

    #[test]
    fn unknown_extension_is_an_error() {
        let err = parse_format_spec("commonmark+bogus").unwrap_err();
        assert!(matches!(err, Error::UnknownExtension(name) if name == "bogus"));
    }

    #[test]
    fn markdown_default_is_the_broad_dialect() {
        let (base, ext) = parse_format_spec("markdown").unwrap();
        assert_eq!(base, "markdown");
        assert!(ext.contains(Extension::Smart));
        assert!(ext.contains(Extension::DefinitionLists));
        assert!(ext.contains(Extension::PipeTables));
    }

    #[test]
    fn gfm_and_commonmark_x_presets_resolve() {
        let (base, ext) = parse_format_spec("gfm").unwrap();
        assert_eq!(base, "gfm");
        assert!(ext.contains(Extension::Strikeout));
        assert!(ext.contains(Extension::PipeTables));
        assert!(!ext.contains(Extension::DefinitionLists));

        let (base, ext) = parse_format_spec("commonmark_x").unwrap();
        assert_eq!(base, "commonmark_x");
        assert!(ext.contains(Extension::FencedDivs));
    }

    #[test]
    fn toggles_apply_over_a_preset() {
        let (_, ext) = parse_format_spec("markdown-smart-pipe_tables").unwrap();
        assert!(!ext.contains(Extension::Smart));
        assert!(!ext.contains(Extension::PipeTables));
        assert!(ext.contains(Extension::DefinitionLists));
    }
}
