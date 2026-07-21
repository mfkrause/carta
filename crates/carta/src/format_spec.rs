//! Parsing of format specifiers of the form `base[+ext][-ext]…`.
//!
//! A specifier names a base format and a sequence of extension toggles applied onto that format's
//! default extension set: `+name` enables an extension, `-name` disables it. The base name (the text
//! before the first `+`/`-`) is what [`reader_for`](crate::reader_for)/[`writer_for`](crate::writer_for)
//! resolve; the resulting [`Extensions`] is merged into the reader/writer options.

use carta_core::{Error, Extension, Extensions, Result, presets};

/// The extensions enabled by default for `base`, before any `+`/`-` toggles.
#[allow(clippy::match_same_arms)] // Formats sharing a default set stay separate arms for their own notes.
fn default_extensions(base: &str) -> Extensions {
    match base {
        "commonmark" => Extensions::from_list(&[Extension::RawHtml]),
        "commonmark_x" => presets::COMMONMARK_X,
        "markdown" => presets::MARKDOWN,
        "markdown_github" => presets::MARKDOWN_GITHUB,
        "markdown_phpextra" => presets::MARKDOWN_PHPEXTRA,
        "markdown_mmd" => presets::MARKDOWN_MMD,
        "markdown_strict" => presets::MARKDOWN_STRICT,
        "gfm" => presets::GFM,
        // Both default to `smart`: quotes, dashes, and ellipses are folded into their typographic
        // glyphs unless `-smart` asks for the literal Unicode punctuation instead. Typst additionally
        // recognizes `[@key]` citations by default.
        "typst" => Extensions::from_list(&[Extension::Smart, Extension::Citations]),
        "dokuwiki" => Extensions::from_list(&[Extension::Smart]),
        // The LaTeX family additionally assigns header identifiers from title text and expands
        // user-defined macros by default; both can be turned off with a `-` toggle.
        "latex" | "beamer" => Extensions::from_list(&[
            Extension::Smart,
            Extension::AutoIdentifiers,
            Extension::LatexMacros,
        ]),
        "html" | "html5" | "html4" => Extensions::from_list(&[
            Extension::AutoIdentifiers,
            Extension::LineBlocks,
            Extension::NativeDivs,
            Extension::NativeSpans,
        ]),
        // An EPUB's content documents are XHTML, so reading one preserves the same structural HTML
        // constructs: native divs and spans, and raw HTML passthrough.
        "epub" | "epub2" | "epub3" => Extensions::from_list(&[
            Extension::NativeDivs,
            Extension::NativeSpans,
            Extension::RawHtml,
        ]),
        "rst" | "mediawiki" | "man" => Extensions::from_list(&[Extension::AutoIdentifiers]),
        // Writing DOCX assigns header identifiers by default; the remaining extensions in its set —
        // custom styles, native figure/table numbering, and empty-paragraph preservation among them —
        // are opt-in `+` toggles.
        "docx" => Extensions::from_list(&[Extension::AutoIdentifiers]),
        // Writing ODT assigns header identifiers by default; empty-paragraph preservation and custom
        // styles are opt-in `+` toggles.
        "odt" => Extensions::from_list(&[Extension::AutoIdentifiers]),
        // Org assigns header identifiers, recognizes `[cite:@key]` citations, and reads task-list
        // checkboxes by default; `smart`, `fancy_lists`, and the alternate identifier shapes are
        // opt-in toggles.
        "org" => Extensions::from_list(&[
            Extension::AutoIdentifiers,
            Extension::Citations,
            Extension::TaskLists,
        ]),
        // A notebook's markdown cells are parsed and rendered in a GitHub-flavored dialect with dollar
        // math and auto identifiers on by default; a hash run needs a following space to open a
        // heading (so a bare `#.` is a list marker, not a heading).
        "ipynb" => Extensions::from_list(&[
            Extension::AllSymbolsEscapable,
            Extension::AutoIdentifiers,
            Extension::GfmAutoIdentifiers,
            Extension::Autolink,
            Extension::BacktickCodeBlocks,
            Extension::FencedCodeBlocks,
            Extension::IntrawordUnderscores,
            Extension::ListsWithoutPrecedingBlankline,
            Extension::PipeTables,
            Extension::RawHtml,
            Extension::ShortcutReferenceLinks,
            Extension::SpaceInAtxHeader,
            Extension::Strikeout,
            Extension::TaskLists,
            Extension::TexMathDollars,
        ]),
        // Writing AsciiDoc assigns header identifiers from title text by default.
        "asciidoc" => Extensions::from_list(&[Extension::AutoIdentifiers]),
        // Plain text is the Markdown writer stripped of inline and block markup, but it keeps the
        // structural extensions that shape how blocks are laid out: the list, table, definition, and
        // figure forms, blank-line spacing rules, intra-word underscores, and macro expansion.
        "plain" => Extensions::from_list(&[
            Extension::BlankBeforeBlockquote,
            Extension::BlankBeforeHeader,
            Extension::DefinitionLists,
            Extension::ExampleLists,
            Extension::FancyLists,
            Extension::GridTables,
            Extension::ImplicitFigures,
            Extension::IntrawordUnderscores,
            Extension::LatexMacros,
            Extension::MultilineTables,
            Extension::SimpleTables,
            Extension::Startnum,
            Extension::Strikeout,
            Extension::TableCaptions,
        ]),
        _ => Extensions::empty(),
    }
}

/// The extensions enabled by default for `base` when it is read, before any `+`/`-` toggles.
///
/// A reader enables every construct its dialect can parse, which for the Markdown variants is a
/// broader set than the writer-shaping subset in [`default_extensions`]. Every other format reads and
/// writes with the same defaults.
fn reader_default_extensions(base: &str) -> Extensions {
    match base {
        // An OPML outline embeds Markdown in its node text, so reading one parses that text with the
        // extended Markdown dialect's defaults.
        "markdown" | "opml" => presets::MARKDOWN_READ,
        "gfm" => presets::GFM_READ,
        "commonmark_x" => presets::COMMONMARK_X_READ,
        "markdown_strict" => presets::MARKDOWN_STRICT_READ,
        "markdown_github" => presets::MARKDOWN_GITHUB_READ,
        "markdown_phpextra" => presets::MARKDOWN_PHPEXTRA_READ,
        "markdown_mmd" => presets::MARKDOWN_MMD_READ,
        _ => default_extensions(base),
    }
}

/// The fixed set of extensions a base format accepts, when it declares one.
///
/// `Some(set)` means the format admits exactly these extensions: a `+`/`-` toggle naming anything
/// outside the set is rejected, and only members appear in `--list-extensions`. `None` means the
/// format declares no fixed set, so any modeled extension may be toggled.
#[allow(clippy::too_many_lines)] // A per-format lookup table: one arm per accepted set.
#[allow(clippy::match_same_arms)] // Formats sharing an accepted set stay separate arms for their own notes.
pub(crate) fn supported_extensions(base: &str) -> Option<Extensions> {
    let set = match base {
        "commonmark" | "commonmark_x" | "gfm" => Extensions::from_list(&[
            // not modeled: sourcepos
            Extension::Alerts,
            Extension::AsciiIdentifiers,
            Extension::Attributes,
            Extension::Autolink,
            Extension::BracketedSpans,
            Extension::DefinitionLists,
            Extension::EastAsianLineBreaks,
            Extension::Emoji,
            Extension::FancyLists,
            Extension::FencedDivs,
            Extension::Footnotes,
            Extension::GfmAutoIdentifiers,
            Extension::HardLineBreaks,
            Extension::ImplicitFigures,
            Extension::ImplicitHeaderReferences,
            Extension::PipeTables,
            Extension::RawAttribute,
            Extension::RawHtml,
            Extension::RebaseRelativePaths,
            Extension::Smart,
            Extension::Strikeout,
            Extension::Subscript,
            Extension::Superscript,
            Extension::TaskLists,
            Extension::TexMathDollars,
            Extension::TexMathGfm,
            Extension::WikilinksTitleAfterPipe,
            Extension::WikilinksTitleBeforePipe,
            Extension::YamlMetadataBlock,
        ]),
        "html" | "html4" | "html5" | "epub" | "epub2" | "epub3" => Extensions::from_list(&[
            // not modeled: epub_html_exts
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::EastAsianLineBreaks,
            Extension::EmptyParagraphs,
            Extension::GfmAutoIdentifiers,
            Extension::LineBlocks,
            Extension::LiterateHaskell,
            Extension::NativeDivs,
            Extension::NativeSpans,
            Extension::RawHtml,
            Extension::RawTex,
            Extension::Smart,
            Extension::TaskLists,
            Extension::TexMathDollars,
            Extension::TexMathDoubleBackslash,
            Extension::TexMathSingleBackslash,
        ]),
        "latex" | "beamer" => Extensions::from_list(&[
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::EastAsianLineBreaks,
            Extension::EmptyParagraphs,
            Extension::GfmAutoIdentifiers,
            Extension::LatexMacros,
            Extension::LiterateHaskell,
            Extension::RawTex,
            Extension::Smart,
            Extension::TaskLists,
        ]),
        "rst" => Extensions::from_list(&[
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::EastAsianLineBreaks,
            Extension::GfmAutoIdentifiers,
            Extension::LiterateHaskell,
            Extension::Smart,
        ]),
        "docx" => Extensions::from_list(&[
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::Citations,
            Extension::EastAsianLineBreaks,
            Extension::EmptyParagraphs,
            Extension::GfmAutoIdentifiers,
            Extension::NativeNumbering,
            Extension::Styles,
        ]),
        "odt" => Extensions::from_list(&[
            // not modeled: xrefs_name, xrefs_number
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::EastAsianLineBreaks,
            Extension::EmptyParagraphs,
            Extension::GfmAutoIdentifiers,
            Extension::NativeNumbering,
        ]),
        "dokuwiki" => Extensions::from_list(&[
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::EastAsianLineBreaks,
            Extension::GfmAutoIdentifiers,
            Extension::RawHtml,
            Extension::Smart,
            Extension::TexMathDollars,
        ]),
        "org" => Extensions::from_list(&[
            // not modeled: smart_quotes, special_strings
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::Citations,
            Extension::EastAsianLineBreaks,
            Extension::FancyLists,
            Extension::GfmAutoIdentifiers,
            Extension::Smart,
            Extension::TaskLists,
        ]),
        "man" => Extensions::from_list(&[
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::EastAsianLineBreaks,
            Extension::GfmAutoIdentifiers,
        ]),
        "mediawiki" => Extensions::from_list(&[
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::EastAsianLineBreaks,
            Extension::GfmAutoIdentifiers,
            Extension::Smart,
        ]),
        "typst" => Extensions::from_list(&[
            Extension::Citations,
            Extension::EastAsianLineBreaks,
            Extension::Smart,
        ]),
        "asciidoc" => Extensions::from_list(&[
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::EastAsianLineBreaks,
            Extension::GfmAutoIdentifiers,
        ]),
        "markdown" | "markdown_github" | "markdown_mmd" | "markdown_phpextra"
        | "markdown_strict" | "opml" | "plain" => Extensions::from_list(&[
            Extension::Abbreviations,
            Extension::Alerts,
            Extension::AllSymbolsEscapable,
            Extension::AngleBracketsEscapable,
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::Autolink,
            Extension::BacktickCodeBlocks,
            Extension::BlankBeforeBlockquote,
            Extension::BlankBeforeHeader,
            Extension::BracketedSpans,
            Extension::Citations,
            Extension::DefinitionLists,
            Extension::EastAsianLineBreaks,
            Extension::Emoji,
            Extension::EscapedLineBreaks,
            Extension::ExampleLists,
            Extension::FancyLists,
            Extension::FencedCodeAttributes,
            Extension::FencedCodeBlocks,
            Extension::FencedDivs,
            Extension::Footnotes,
            Extension::FourSpaceRule,
            Extension::GfmAutoIdentifiers,
            Extension::GridTables,
            Extension::Gutenberg,
            Extension::HardLineBreaks,
            Extension::HeaderAttributes,
            Extension::IgnoreLineBreaks,
            Extension::ImplicitFigures,
            Extension::ImplicitHeaderReferences,
            Extension::InlineCodeAttributes,
            Extension::InlineNotes,
            Extension::IntrawordUnderscores,
            Extension::LatexMacros,
            Extension::LineBlocks,
            Extension::LinkAttributes,
            Extension::ListsWithoutPrecedingBlankline,
            Extension::LiterateHaskell,
            Extension::Mark,
            Extension::MarkdownAttribute,
            Extension::MarkdownInHtmlBlocks,
            Extension::MmdHeaderIdentifiers,
            Extension::MmdLinkAttributes,
            Extension::MmdTitleBlock,
            Extension::MultilineTables,
            Extension::NativeDivs,
            Extension::NativeSpans,
            Extension::OldDashes,
            Extension::PandocTitleBlock,
            Extension::PipeTables,
            Extension::RawAttribute,
            Extension::RawHtml,
            Extension::RawTex,
            Extension::RebaseRelativePaths,
            Extension::ShortSubsuperscripts,
            Extension::ShortcutReferenceLinks,
            Extension::SimpleTables,
            Extension::Smart,
            Extension::SpaceInAtxHeader,
            Extension::SpacedReferenceLinks,
            Extension::Startnum,
            Extension::Strikeout,
            Extension::Subscript,
            Extension::Superscript,
            Extension::TableAttributes,
            Extension::TableCaptions,
            Extension::TaskLists,
            Extension::TexMathDollars,
            Extension::TexMathDoubleBackslash,
            Extension::TexMathSingleBackslash,
            Extension::WikilinksTitleAfterPipe,
            Extension::WikilinksTitleBeforePipe,
            Extension::YamlMetadataBlock,
        ]),
        "ipynb" => Extensions::from_list(&[
            Extension::Abbreviations,
            Extension::Alerts,
            Extension::AllSymbolsEscapable,
            Extension::AngleBracketsEscapable,
            Extension::AsciiIdentifiers,
            Extension::AutoIdentifiers,
            Extension::Autolink,
            Extension::BacktickCodeBlocks,
            Extension::BlankBeforeBlockquote,
            Extension::BlankBeforeHeader,
            Extension::BracketedSpans,
            Extension::Citations,
            Extension::DefinitionLists,
            Extension::EastAsianLineBreaks,
            Extension::Emoji,
            Extension::EscapedLineBreaks,
            Extension::ExampleLists,
            Extension::FancyLists,
            Extension::FencedCodeAttributes,
            Extension::FencedCodeBlocks,
            Extension::FencedDivs,
            Extension::Footnotes,
            Extension::FourSpaceRule,
            Extension::GfmAutoIdentifiers,
            Extension::GridTables,
            Extension::Gutenberg,
            Extension::HardLineBreaks,
            Extension::HeaderAttributes,
            Extension::IgnoreLineBreaks,
            Extension::ImplicitFigures,
            Extension::ImplicitHeaderReferences,
            Extension::InlineCodeAttributes,
            Extension::InlineNotes,
            Extension::IntrawordUnderscores,
            Extension::LatexMacros,
            Extension::LineBlocks,
            Extension::LinkAttributes,
            Extension::ListsWithoutPrecedingBlankline,
            Extension::LiterateHaskell,
            Extension::Mark,
            Extension::MarkdownAttribute,
            Extension::MarkdownInHtmlBlocks,
            Extension::MmdHeaderIdentifiers,
            Extension::MmdLinkAttributes,
            Extension::MmdTitleBlock,
            Extension::MultilineTables,
            Extension::NativeDivs,
            Extension::NativeSpans,
            Extension::OldDashes,
            Extension::PandocTitleBlock,
            Extension::PipeTables,
            Extension::RawAttribute,
            Extension::RawHtml,
            Extension::RawMarkdown,
            Extension::RawTex,
            Extension::RebaseRelativePaths,
            Extension::ShortSubsuperscripts,
            Extension::ShortcutReferenceLinks,
            Extension::SimpleTables,
            Extension::Smart,
            Extension::SpaceInAtxHeader,
            Extension::SpacedReferenceLinks,
            Extension::Startnum,
            Extension::Strikeout,
            Extension::Subscript,
            Extension::Superscript,
            Extension::TableAttributes,
            Extension::TableCaptions,
            Extension::TaskLists,
            Extension::TexMathDollars,
            Extension::TexMathDoubleBackslash,
            Extension::TexMathSingleBackslash,
            Extension::WikilinksTitleAfterPipe,
            Extension::WikilinksTitleBeforePipe,
            Extension::YamlMetadataBlock,
        ]),
        "jira" | "native" | "json" | "csv" | "tsv" | "rtf" | "revealjs" => {
            Extensions::from_list(&[Extension::EastAsianLineBreaks])
        }
        _ => return None,
    };
    Some(set)
}

/// Splits a format specifier into its base name and the [`Extensions`] it selects.
///
/// The base is the text up to the first `+` or `-`; the remainder is a run of `+name`/`-name`
/// toggles applied onto `default_extensions`.
///
/// # Errors
/// [`Error::UnknownExtension`] if a toggle names an extension this build does not recognize.
pub fn parse_format_spec(spec: &str) -> Result<(String, Extensions)> {
    parse_format_spec_with(spec, default_extensions)
}

/// Splits a reading format specifier into its base name and the [`Extensions`] it selects, seeding
/// the toggles from the reader defaults (which for the Markdown variants are broader than the writer
/// defaults; see [`reader_default_extensions`]).
///
/// # Errors
/// [`Error::UnknownExtension`] if a toggle names an extension this build does not recognize.
pub(crate) fn parse_reader_format_spec(spec: &str) -> Result<(String, Extensions)> {
    parse_format_spec_with(spec, reader_default_extensions)
}

fn parse_format_spec_with(
    spec: &str,
    defaults: impl Fn(&str) -> Extensions,
) -> Result<(String, Extensions)> {
    let base_end = spec.find(['+', '-']).unwrap_or(spec.len());
    let (base, mut rest) = spec.split_at(base_end);
    let mut extensions = defaults(base);
    let supported = supported_extensions(base);

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

        // A format that declares a fixed extension set admits only its members; anything else —
        // including a name no extension answers to — is unsupported for that format. A format
        // without a declared set accepts any modeled extension and rejects only unknown names.
        let extension = match &supported {
            Some(set) => Extension::from_name(name)
                .filter(|ext| set.contains(*ext))
                .ok_or_else(|| Error::UnsupportedExtension {
                    extension: name.to_owned(),
                    format: base.to_owned(),
                })?,
            None => Extension::from_name(name)
                .ok_or_else(|| Error::UnknownExtension(name.to_owned()))?,
        };
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
        // A base with no declared accepted set rejects an unrecognized name as unknown.
        let err = parse_format_spec("unregistered_format+bogus").unwrap_err();
        assert!(matches!(err, Error::UnknownExtension(name) if name == "bogus"));

        // A base that declares an accepted set reports the same name as unsupported for it.
        let err = parse_format_spec("commonmark+bogus").unwrap_err();
        assert!(matches!(
            err,
            Error::UnsupportedExtension { extension, format }
                if extension == "bogus" && format == "commonmark"
        ));
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

    #[test]
    fn html_enables_its_structural_defaults() {
        for spec in ["html", "html5", "html4"] {
            let (base, ext) = parse_format_spec(spec).unwrap();
            assert_eq!(base, spec);
            assert!(ext.contains(Extension::AutoIdentifiers));
            assert!(ext.contains(Extension::LineBlocks));
            assert!(ext.contains(Extension::NativeDivs));
            assert!(ext.contains(Extension::NativeSpans));
            // The text extensions are opt-in, not part of the default set.
            assert!(!ext.contains(Extension::Smart));
            assert!(!ext.contains(Extension::TexMathDollars));
        }
    }

    #[test]
    fn html_toggles_add_text_and_remove_structural_extensions() {
        let (_, ext) = parse_format_spec("html+smart+tex_math_dollars").unwrap();
        assert!(ext.contains(Extension::Smart));
        assert!(ext.contains(Extension::TexMathDollars));
        assert!(ext.contains(Extension::NativeDivs));

        let (_, ext) = parse_format_spec("html-native_divs-auto_identifiers").unwrap();
        assert!(!ext.contains(Extension::NativeDivs));
        assert!(!ext.contains(Extension::AutoIdentifiers));
        assert!(ext.contains(Extension::NativeSpans));
        assert!(ext.contains(Extension::LineBlocks));
    }

    #[test]
    fn html_unknown_extension_is_an_error() {
        // HTML declares an accepted set, so an unrecognized name is unsupported for the format.
        let err = parse_format_spec("html+bogus").unwrap_err();
        assert!(matches!(
            err,
            Error::UnsupportedExtension { extension, format }
                if extension == "bogus" && format == "html"
        ));
    }

    #[test]
    fn recognized_dialect_toggle_names_parse_without_error() {
        // These extension names are part of the markdown-family vocabulary a format spec may toggle.
        // Each must be recognized so that toggling it on a base format succeeds rather than aborting,
        // and each toggle must apply in both directions: `+name` enables it and a trailing `-name`
        // disables it, regardless of whether the base enables it by default.
        let names = [
            "abbreviations",
            "all_symbols_escapable",
            "angle_brackets_escapable",
            "ascii_identifiers",
            "east_asian_line_breaks",
            "four_space_rule",
            "gutenberg",
            "ignore_line_breaks",
            "latex_macros",
            "literate_haskell",
            "mmd_link_attributes",
            "mmd_title_block",
            "old_dashes",
            "raw_markdown",
            "rebase_relative_paths",
            "short_subsuperscripts",
            "shortcut_reference_links",
            "space_in_atx_header",
            "spaced_reference_links",
            "wikilinks_title_after_pipe",
            "wikilinks_title_before_pipe",
        ];
        for name in names {
            let (_, enabled) = parse_format_spec(&format!("ipynb+{name}"))
                .unwrap_or_else(|err| panic!("ipynb+{name} should parse: {err:?}"));
            let extension =
                Extension::from_name(name).unwrap_or_else(|| panic!("{name} should be a variant"));
            assert!(enabled.contains(extension), "+{name} should enable it");

            let (_, disabled) = parse_format_spec(&format!("ipynb+{name}-{name}"))
                .unwrap_or_else(|err| panic!("ipynb+{name}-{name} should parse: {err:?}"));
            assert!(
                !disabled.contains(extension),
                "+{name}-{name} should disable it"
            );
        }
    }

    #[test]
    fn latex_defaults_include_identifiers_and_macros() {
        let (base, ext) = parse_format_spec("latex").unwrap();
        assert_eq!(base, "latex");
        assert!(ext.contains(Extension::Smart));
        assert!(ext.contains(Extension::AutoIdentifiers));
        assert!(ext.contains(Extension::LatexMacros));
        assert!(!ext.contains(Extension::RawTex));

        let (_, ext) = parse_format_spec("latex-smart-latex_macros+raw_tex").unwrap();
        assert!(!ext.contains(Extension::Smart));
        assert!(!ext.contains(Extension::LatexMacros));
        assert!(ext.contains(Extension::RawTex));
        assert!(ext.contains(Extension::AutoIdentifiers));
    }

    #[test]
    fn dokuwiki_defaults_to_smart_only() {
        let (base, ext) = parse_format_spec("dokuwiki").unwrap();
        assert_eq!(base, "dokuwiki");
        assert!(ext.contains(Extension::Smart));
        assert!(!ext.contains(Extension::TexMathDollars));
    }

    #[test]
    fn dokuwiki_admits_its_declared_extensions() {
        let (_, ext) = parse_format_spec("dokuwiki+tex_math_dollars-smart").unwrap();
        assert!(ext.contains(Extension::TexMathDollars));
        assert!(!ext.contains(Extension::Smart));
    }

    #[test]
    fn dokuwiki_rejects_an_extension_outside_its_set() {
        // `pipe_tables` is a modeled extension, but not one dokuwiki accepts.
        let err = parse_format_spec("dokuwiki+pipe_tables").unwrap_err();
        assert!(matches!(
            err,
            Error::UnsupportedExtension { extension, format }
                if extension == "pipe_tables" && format == "dokuwiki"
        ));
        // An unknown name is reported the same way: unsupported for this format.
        let err = parse_format_spec("dokuwiki+bogus").unwrap_err();
        assert!(matches!(
            err,
            Error::UnsupportedExtension { extension, .. } if extension == "bogus"
        ));
    }

    #[test]
    fn supported_set_is_declared_for_recognized_formats_only() {
        use super::supported_extensions;
        for base in [
            "dokuwiki",
            "docx",
            "odt",
            "commonmark",
            "html",
            "markdown",
            "rst",
        ] {
            assert!(
                supported_extensions(base).is_some(),
                "{base} should declare an accepted set"
            );
        }
        // An unrecognized base declares no set, so any modeled toggle is admitted for it.
        assert!(supported_extensions("unregistered_format").is_none());
    }

    #[test]
    fn docx_defaults_to_auto_identifiers_only() {
        let (base, ext) = parse_format_spec("docx").unwrap();
        assert_eq!(base, "docx");
        assert!(ext.contains(Extension::AutoIdentifiers));
        // The three writer-shaping toggles are opt-in, off by default.
        assert!(!ext.contains(Extension::Styles));
        assert!(!ext.contains(Extension::NativeNumbering));
        assert!(!ext.contains(Extension::EmptyParagraphs));
    }

    #[test]
    fn docx_admits_its_declared_extensions_and_rejects_others() {
        let (_, ext) = parse_format_spec("docx+styles+native_numbering+empty_paragraphs").unwrap();
        assert!(ext.contains(Extension::Styles));
        assert!(ext.contains(Extension::NativeNumbering));
        assert!(ext.contains(Extension::EmptyParagraphs));
        assert!(ext.contains(Extension::AutoIdentifiers));

        // `pipe_tables` is a modeled extension, but not one docx accepts.
        let err = parse_format_spec("docx+pipe_tables").unwrap_err();
        assert!(matches!(
            err,
            Error::UnsupportedExtension { extension, format }
                if extension == "pipe_tables" && format == "docx"
        ));
    }

    #[test]
    fn odt_defaults_to_auto_identifiers_only() {
        let (base, ext) = parse_format_spec("odt").unwrap();
        assert_eq!(base, "odt");
        assert!(ext.contains(Extension::AutoIdentifiers));
        // Empty-paragraph preservation is an opt-in toggle, off by default.
        assert!(!ext.contains(Extension::EmptyParagraphs));
    }

    #[test]
    fn odt_admits_its_declared_extensions_and_rejects_others() {
        let (_, ext) = parse_format_spec("odt+empty_paragraphs+native_numbering").unwrap();
        assert!(ext.contains(Extension::EmptyParagraphs));
        assert!(ext.contains(Extension::NativeNumbering));
        assert!(ext.contains(Extension::AutoIdentifiers));

        // `styles` and `raw_html` are modeled extensions, but not ones odt accepts.
        for spec in ["odt+styles", "odt+raw_html"] {
            let err = parse_format_spec(spec).unwrap_err();
            assert!(matches!(
                err,
                Error::UnsupportedExtension { format, .. } if format == "odt"
            ));
        }
    }

    #[test]
    fn rst_admits_its_declared_extensions_and_rejects_others() {
        let (_, ext) = parse_format_spec("rst+smart-auto_identifiers").unwrap();
        assert!(ext.contains(Extension::Smart));
        assert!(!ext.contains(Extension::AutoIdentifiers));

        // `pipe_tables` is a modeled extension, but not one rst accepts.
        let err = parse_format_spec("rst+pipe_tables").unwrap_err();
        assert!(matches!(
            err,
            Error::UnsupportedExtension { extension, format }
                if extension == "pipe_tables" && format == "rst"
        ));
    }

    #[test]
    fn commonmark_admits_its_declared_extensions_and_rejects_others() {
        let (_, ext) = parse_format_spec("commonmark+strikeout+task_lists").unwrap();
        assert!(ext.contains(Extension::Strikeout));
        assert!(ext.contains(Extension::TaskLists));

        // `latex_macros` is a modeled extension, but not one commonmark accepts.
        let err = parse_format_spec("commonmark+latex_macros").unwrap_err();
        assert!(matches!(
            err,
            Error::UnsupportedExtension { extension, format }
                if extension == "latex_macros" && format == "commonmark"
        ));
    }
}
