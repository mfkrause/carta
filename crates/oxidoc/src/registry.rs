//! Format-name dispatch. A static, `#[cfg]`-gated table maps a format name to its [`Reader`] or
//! [`Writer`]; only the formats whose features are enabled are compiled in. Names that are
//! recognized but not compiled in resolve to [`Error::FormatNotEnabled`]; unknown names to
//! [`Error::UnsupportedFormat`].

use oxidoc_core::{Error, Reader, Result, Writer};

/// Every input-format name oxidoc recognizes, whether or not it is compiled into this build.
const KNOWN_INPUT_FORMATS: &[&str] = &["commonmark", "markdown", "json"];
/// Every output-format name oxidoc recognizes, whether or not it is compiled into this build.
const KNOWN_OUTPUT_FORMATS: &[&str] = &["html", "html5", "json"];

/// Resolves an input-format name to its reader.
///
/// # Errors
/// [`Error::FormatNotEnabled`] if the format is recognized but its feature is off;
/// [`Error::UnsupportedFormat`] if the name is unknown.
pub fn reader_for(name: &str) -> Result<Box<dyn Reader>> {
    match name {
        #[cfg(feature = "read-json")]
        "json" => Ok(Box::new(oxidoc_readers::JsonReader)),
        #[cfg(feature = "read-commonmark")]
        "commonmark" | "markdown" => Ok(Box::new(oxidoc_readers::CommonmarkReader)),
        other => Err(resolution_error(other, KNOWN_INPUT_FORMATS)),
    }
}

/// Resolves an output-format name to its writer.
///
/// # Errors
/// [`Error::FormatNotEnabled`] if the format is recognized but its feature is off;
/// [`Error::UnsupportedFormat`] if the name is unknown.
pub fn writer_for(name: &str) -> Result<Box<dyn Writer>> {
    match name {
        #[cfg(feature = "write-json")]
        "json" => Ok(Box::new(oxidoc_writers::JsonWriter)),
        #[cfg(feature = "write-html")]
        "html" | "html5" => Ok(Box::new(oxidoc_writers::HtmlWriter)),
        other => Err(resolution_error(other, KNOWN_OUTPUT_FORMATS)),
    }
}

fn resolution_error(name: &str, known: &[&str]) -> Error {
    if known.contains(&name) {
        Error::FormatNotEnabled(name.to_owned())
    } else {
        Error::UnsupportedFormat(name.to_owned())
    }
}

/// The canonical input-format names compiled into this build, in a deterministic order.
#[must_use]
pub fn supported_input_formats() -> Vec<&'static str> {
    [
        cfg!(feature = "read-commonmark").then_some("commonmark"),
        cfg!(feature = "read-json").then_some("json"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The canonical output-format names compiled into this build, in a deterministic order.
#[must_use]
pub fn supported_output_formats() -> Vec<&'static str> {
    [
        cfg!(feature = "write-html").then_some("html"),
        cfg!(feature = "write-json").then_some("json"),
    ]
    .into_iter()
    .flatten()
    .collect()
}
