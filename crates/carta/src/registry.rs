//! Format-name dispatch. Every format is declared once inside a [`format_dispatch!`] block; the
//! macro expands that single declaration into all three views that must agree — the resolver
//! (name/alias → boxed trait object), the `supported_*` enumerator, and the recognized-name set
//! that separates "disabled" from "unknown". Because the views share one source, they cannot drift
//! as formats are added.
//!
//! Each constructor is `#[cfg]`-gated on its per-direction feature, so only formats compiled into
//! the build resolve. A recognized name whose feature is off yields [`Error::FormatNotEnabled`]; an
//! unrecognized name yields [`Error::UnsupportedFormat`].

use carta_core::{Error, Reader, Result, Writer};

fn resolution_error(name: &str, known: &[&str]) -> Error {
    if known.contains(&name) {
        Error::FormatNotEnabled(name.to_owned())
    } else {
        Error::UnsupportedFormat(name.to_owned())
    }
}

/// Expands one per-direction format table into its resolver and enumerator. Each entry reads
/// `<feature> => <canonical> [| <alias>]* => <constructor>;`.
macro_rules! format_dispatch {
    (
        trait: $trait:ident;
        resolve: $resolve:ident;
        supported: $supported:ident;
        $( $feature:literal => $canonical:literal $(| $alias:literal)* => $constructor:expr ; )+
    ) => {
        #[doc = concat!("Resolves a format name to its boxed [`", stringify!($trait), "`].")]
        #[doc = ""]
        #[doc = "[`Error::FormatNotEnabled`] if the format is recognized but its feature is off;"]
        #[doc = "[`Error::UnsupportedFormat`] if the name is unknown."]
        pub fn $resolve(name: &str) -> Result<Box<dyn $trait>> {
            const KNOWN: &[&str] = &[ $( $canonical $(, $alias)* ),+ ];
            match name {
                $(
                    #[cfg(feature = $feature)]
                    $canonical $(| $alias)* => Ok(Box::new($constructor)),
                )+
                other => Err(resolution_error(other, KNOWN)),
            }
        }

        #[doc = concat!("The canonical names of every compiled-in ", stringify!($trait), " format, in declaration order.")]
        #[must_use]
        pub fn $supported() -> Vec<&'static str> {
            [ $( cfg!(feature = $feature).then_some($canonical) ),+ ]
                .into_iter()
                .flatten()
                .collect()
        }
    };
}

format_dispatch! {
    trait: Reader;
    resolve: reader_for;
    supported: supported_input_formats;
    "read-commonmark" => "commonmark" | "commonmark_x" | "markdown" | "gfm" => carta_readers::CommonmarkReader;
    "read-json" => "json" => carta_readers::JsonReader;
    "read-native" => "native" => carta_readers::NativeReader;
    "read-html" => "html" => carta_readers::HtmlReader;
    "read-csv" => "csv" => carta_readers::CsvReader;
    "read-tsv" => "tsv" => carta_readers::TsvReader;
    "read-opml" => "opml" => carta_readers::OpmlReader;
}

format_dispatch! {
    trait: Writer;
    resolve: writer_for;
    supported: supported_output_formats;
    "write-html" => "html" | "html5" => carta_writers::HtmlWriter;
    "write-html4" => "html4" => carta_writers::Html4Writer;
    "write-json" => "json" => carta_writers::JsonWriter;
    "write-plain" => "plain" => carta_writers::PlainWriter;
    "write-native" => "native" => carta_writers::NativeWriter;
    "write-latex" => "latex" => carta_writers::LatexWriter;
    "write-commonmark" => "commonmark" => carta_writers::CommonmarkWriter;
    "write-markdown" => "markdown" => carta_writers::MarkdownWriter;
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
}
