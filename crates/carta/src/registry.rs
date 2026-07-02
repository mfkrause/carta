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

use carta_core::{AnyReader, AnyWriter, Error, Reader, Result, Writer};

/// Expands one per-direction format table into its resolver and enumerators. Each entry reads
/// `<feature> => <canonical> [| <alias>]* => <kind> <constructor>;`, where `<kind>` is `text` or
/// `bytes`.
///
/// The `@wrap` internal rules box a constructor into the tagged dispatch enum by its kind token.
/// Folding them in (rather than a standalone helper macro) keeps this macro invoked in every feature
/// configuration, including the zero-format build where all resolver arms are `#[cfg]`-stripped.
macro_rules! format_dispatch {
    (@wrap $any:ident text $constructor:expr) => {
        $any::Text(Box::new($constructor))
    };
    (@wrap $any:ident bytes $constructor:expr) => {
        $any::Bytes(Box::new($constructor))
    };
    (
        trait: $trait:ident;
        any: $any_enum:ident;
        resolve: $resolve:ident;
        recognizes: $recognizes:ident;
        supported: $supported:ident;
        names: $names:ident;
        $( $feature:literal => $canonical:literal $(| $alias:literal)* => $kind:ident $constructor:expr ; )+
    ) => {
        #[doc = concat!("Resolves a format name to its [`", stringify!($any_enum), "`].")]
        #[doc = ""]
        #[doc = "[`Error::FormatNotEnabled`] if the format is recognized but its feature is off;"]
        #[doc = "[`Error::UnsupportedFormat`] if the name is unknown."]
        pub fn $resolve(name: &str) -> Result<$any_enum> {
            match name {
                $(
                    #[cfg(feature = $feature)]
                    $canonical $(| $alias)* => Ok(format_dispatch!(@wrap $any_enum $kind $constructor)),
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
    any: AnyReader;
    resolve: any_reader_for;
    recognizes: reader_recognizes;
    supported: supported_input_formats;
    names: input_format_names;
    "read-commonmark" => "commonmark" | "commonmark_x" | "markdown" | "gfm" | "markdown_strict" | "markdown_mmd" | "markdown_phpextra" | "markdown_github" => text carta_readers::CommonmarkReader;
    "read-json" => "json" => text carta_readers::JsonReader;
    "read-native" => "native" => text carta_readers::NativeReader;
    "read-html" => "html" => text carta_readers::HtmlReader;
    "read-csv" => "csv" => text carta_readers::CsvReader;
    "read-tsv" => "tsv" => text carta_readers::TsvReader;
    "read-opml" => "opml" => text carta_readers::OpmlReader;
    "read-rst" => "rst" => text carta_readers::RstReader;
    "read-ipynb" => "ipynb" => text carta_readers::IpynbReader;
    "read-mediawiki" => "mediawiki" => text carta_readers::MediawikiReader;
    "read-dokuwiki" => "dokuwiki" => text carta_readers::DokuwikiReader;
    "read-jira" => "jira" => text carta_readers::JiraReader;
    "read-man" => "man" => text carta_readers::ManReader;
    "read-latex" => "latex" => text carta_readers::LatexReader;
    "read-org" => "org" => text carta_readers::OrgReader;
}

format_dispatch! {
    trait: Writer;
    any: AnyWriter;
    resolve: any_writer_for;
    recognizes: writer_recognizes;
    supported: supported_output_formats;
    names: output_format_names;
    "write-html" => "html" | "html5" => text carta_writers::HtmlWriter;
    "write-html4" => "html4" => text carta_writers::Html4Writer;
    "write-json" => "json" => text carta_writers::JsonWriter;
    "write-plain" => "plain" => text carta_writers::PlainWriter;
    "write-native" => "native" => text carta_writers::NativeWriter;
    "write-latex" => "latex" => text carta_writers::LatexWriter;
    "write-commonmark" => "commonmark" => text carta_writers::CommonmarkWriter;
    "write-markdown" => "markdown" => text carta_writers::MarkdownWriter;
    "write-markdown" => "commonmark_x" => text carta_writers::CommonmarkXWriter;
    "write-markdown" => "markdown_github" => text carta_writers::MarkdownGithubWriter;
    "write-markdown" => "markdown_phpextra" => text carta_writers::MarkdownPhpextraWriter;
    "write-markdown" => "markdown_mmd" => text carta_writers::MarkdownMmdWriter;
    "write-markdown" => "markdown_strict" => text carta_writers::MarkdownStrictWriter;
    "write-gfm" => "gfm" => text carta_writers::GfmWriter;
    "write-rst" => "rst" => text carta_writers::RstWriter;
    "write-mediawiki" => "mediawiki" => text carta_writers::MediawikiWriter;
    "write-typst" => "typst" => text carta_writers::TypstWriter;
    "write-dokuwiki" => "dokuwiki" => text carta_writers::DokuwikiWriter;
    "write-jira" => "jira" => text carta_writers::JiraWriter;
    "write-asciidoc" => "asciidoc" => text carta_writers::AsciidocWriter;
    "write-man" => "man" => text carta_writers::ManWriter;
    "write-opml" => "opml" => text carta_writers::OpmlWriter;
    "write-org" => "org" => text carta_writers::OrgWriter;
    "write-beamer" => "beamer" => text carta_writers::BeamerWriter;
    "write-revealjs" => "revealjs" => text carta_writers::RevealjsWriter;
    "write-ipynb" => "ipynb" => text carta_writers::IpynbWriter;
}

/// Resolves a format name to its boxed [`Reader`], the text-only view of [`any_reader_for`].
///
/// [`Error::FormatNotEnabled`] if the format is recognized but its feature is off;
/// [`Error::UnsupportedFormat`] if the name is unknown; [`Error::BinaryFormat`] if the format is
/// byte-shaped, so it has no text reader.
pub fn reader_for(name: &str) -> Result<Box<dyn Reader>> {
    match any_reader_for(name)? {
        AnyReader::Text(reader) => Ok(reader),
        AnyReader::Bytes(_) => Err(Error::BinaryFormat(name.to_owned())),
    }
}

/// Resolves a format name to its boxed [`Writer`], the text-only view of [`any_writer_for`].
///
/// [`Error::FormatNotEnabled`] if the format is recognized but its feature is off;
/// [`Error::UnsupportedFormat`] if the name is unknown; [`Error::BinaryFormat`] if the format is
/// byte-shaped, so it has no text writer.
pub fn writer_for(name: &str) -> Result<Box<dyn Writer>> {
    match any_writer_for(name)? {
        AnyWriter::Text(writer) => Ok(writer),
        AnyWriter::Bytes(_) => Err(Error::BinaryFormat(name.to_owned())),
    }
}
