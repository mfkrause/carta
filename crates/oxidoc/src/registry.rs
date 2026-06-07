//! Format-name dispatch. Every format is declared once inside a [`format_dispatch!`] block; the
//! macro expands that single declaration into all three views that must agree — the resolver
//! (name/alias → boxed trait object), the `supported_*` enumerator, and the recognized-name set
//! that separates "disabled" from "unknown". Because the views share one source, they cannot drift
//! as formats are added.
//!
//! Each constructor is `#[cfg]`-gated on its per-direction feature, so only formats compiled into
//! the build resolve. A recognized name whose feature is off yields [`Error::FormatNotEnabled`]; an
//! unrecognized name yields [`Error::UnsupportedFormat`].

use oxidoc_core::{Error, Reader, Result, Writer};

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
    "read-commonmark" => "commonmark" | "markdown" => oxidoc_readers::CommonmarkReader;
    "read-json" => "json" => oxidoc_readers::JsonReader;
    "read-native" => "native" => oxidoc_readers::NativeReader;
    "read-html" => "html" => oxidoc_readers::HtmlReader;
}

format_dispatch! {
    trait: Writer;
    resolve: writer_for;
    supported: supported_output_formats;
    "write-html" => "html" | "html5" => oxidoc_writers::HtmlWriter;
    "write-json" => "json" => oxidoc_writers::JsonWriter;
    "write-plain" => "plain" => oxidoc_writers::PlainWriter;
    "write-native" => "native" => oxidoc_writers::NativeWriter;
    "write-latex" => "latex" => oxidoc_writers::LatexWriter;
}
