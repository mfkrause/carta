//! Format-name dispatch. Every format is declared once inside a [`format_dispatch!`] block; the
//! macro expands that single declaration into the views that must agree — the resolver (name/alias →
//! boxed trait object), the `supported_*` enumerator (canonical names), the `*_format_names`
//! enumerator (canonical names plus aliases, for introspection), and a `*_recognizes` predicate over
//! the full name set that separates "disabled" from "unknown". Because the views share one source,
//! they cannot drift as formats are added.
//!
//! Each constructor is `#[cfg]`-gated on its per-direction feature, so only formats compiled into
//! the build resolve. A recognized name whose feature is off yields [`Error::FormatNotEnabled`]; an
//! unrecognized name yields [`Error::UnsupportedFormat`].

use carta_core::{Error, Reader, Result, Writer};

/// Expands one per-direction format table into its resolver and enumerators. Each entry reads
/// `<feature> => <canonical> [| <alias>]* => <constructor>;`.
macro_rules! format_dispatch {
    (
        trait: $trait:ident;
        resolve: $resolve:ident;
        recognizes: $recognizes:ident;
        supported: $supported:ident;
        names: $names:ident;
        $( $feature:literal => $canonical:literal $(| $alias:literal)* => $constructor:expr ; )+
    ) => {
        #[doc = concat!("Resolves a format name to its boxed [`", stringify!($trait), "`].")]
        #[doc = ""]
        #[doc = "[`Error::FormatNotEnabled`] if the format is recognized but its feature is off;"]
        #[doc = "[`Error::UnsupportedFormat`] if the name is unknown."]
        pub fn $resolve(name: &str) -> Result<Box<dyn $trait>> {
            match name {
                $(
                    #[cfg(feature = $feature)]
                    $canonical $(| $alias)* => Ok(Box::new($constructor)),
                )+
                other if $recognizes(other) => Err(Error::FormatNotEnabled(other.to_owned())),
                other => Err(Error::UnsupportedFormat(other.to_owned())),
            }
        }

        #[doc = concat!("Whether `name` is a recognized ", stringify!($trait), " format — true regardless of whether its feature is compiled in.")]
        #[must_use]
        pub(crate) fn $recognizes(name: &str) -> bool {
            const KNOWN: &[&str] = &[ $( $canonical $(, $alias)* ),+ ];
            KNOWN.contains(&name)
        }

        #[doc = concat!("The canonical names of every compiled-in ", stringify!($trait), " format, in declaration order.")]
        #[must_use]
        pub fn $supported() -> Vec<&'static str> {
            [ $( cfg!(feature = $feature).then_some($canonical) ),+ ]
                .into_iter()
                .flatten()
                .collect()
        }

        #[doc = concat!("Every accepted ", stringify!($trait), " format name in this build — canonical names and their aliases — sorted.")]
        #[must_use]
        pub fn $names() -> Vec<&'static str> {
            let mut names: Vec<&'static str> = Vec::new();
            $(
                if cfg!(feature = $feature) {
                    names.extend_from_slice(&[ $canonical $(, $alias)* ]);
                }
            )+
            names.sort_unstable();
            names
        }
    };
}

format_dispatch! {
    trait: Reader;
    resolve: reader_for;
    recognizes: reader_recognizes;
    supported: supported_input_formats;
    names: input_format_names;
    "read-commonmark" => "commonmark" | "commonmark_x" | "markdown" | "gfm" | "markdown_strict" | "markdown_mmd" | "markdown_phpextra" | "markdown_github" => carta_readers::CommonmarkReader;
    "read-json" => "json" => carta_readers::JsonReader;
    "read-native" => "native" => carta_readers::NativeReader;
    "read-html" => "html" => carta_readers::HtmlReader;
    "read-csv" => "csv" => carta_readers::CsvReader;
    "read-tsv" => "tsv" => carta_readers::TsvReader;
    "read-opml" => "opml" => carta_readers::OpmlReader;
    "read-rst" => "rst" => carta_readers::RstReader;
    "read-ipynb" => "ipynb" => carta_readers::IpynbReader;
    "read-mediawiki" => "mediawiki" => carta_readers::MediawikiReader;
    "read-dokuwiki" => "dokuwiki" => carta_readers::DokuwikiReader;
    "read-jira" => "jira" => carta_readers::JiraReader;
    "read-man" => "man" => carta_readers::ManReader;
}

format_dispatch! {
    trait: Writer;
    resolve: writer_for;
    recognizes: writer_recognizes;
    supported: supported_output_formats;
    names: output_format_names;
    "write-html" => "html" | "html5" => carta_writers::HtmlWriter;
    "write-html4" => "html4" => carta_writers::Html4Writer;
    "write-json" => "json" => carta_writers::JsonWriter;
    "write-plain" => "plain" => carta_writers::PlainWriter;
    "write-native" => "native" => carta_writers::NativeWriter;
    "write-latex" => "latex" => carta_writers::LatexWriter;
    "write-commonmark" => "commonmark" => carta_writers::CommonmarkWriter;
    "write-markdown" => "markdown" => carta_writers::MarkdownWriter;
    "write-markdown" => "commonmark_x" => carta_writers::CommonmarkXWriter;
    "write-markdown" => "markdown_github" => carta_writers::MarkdownGithubWriter;
    "write-markdown" => "markdown_phpextra" => carta_writers::MarkdownPhpextraWriter;
    "write-markdown" => "markdown_mmd" => carta_writers::MarkdownMmdWriter;
    "write-markdown" => "markdown_strict" => carta_writers::MarkdownStrictWriter;
    "write-gfm" => "gfm" => carta_writers::GfmWriter;
    "write-rst" => "rst" => carta_writers::RstWriter;
    "write-mediawiki" => "mediawiki" => carta_writers::MediawikiWriter;
    "write-typst" => "typst" => carta_writers::TypstWriter;
    "write-dokuwiki" => "dokuwiki" => carta_writers::DokuwikiWriter;
    "write-jira" => "jira" => carta_writers::JiraWriter;
    "write-asciidoc" => "asciidoc" => carta_writers::AsciidocWriter;
    "write-man" => "man" => carta_writers::ManWriter;
    "write-opml" => "opml" => carta_writers::OpmlWriter;
    "write-beamer" => "beamer" => carta_writers::BeamerWriter;
    "write-revealjs" => "revealjs" => carta_writers::RevealjsWriter;
    "write-ipynb" => "ipynb" => carta_writers::IpynbWriter;
}
