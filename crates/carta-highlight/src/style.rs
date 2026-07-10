//! The color model: a named theme mapping token kinds to colors and font weights, plus the code
//! block's overall foreground/background and line-number colors.
//!
//! A theme is stored as JSON (the `.theme` wire format) so the CLI can round-trip a theme through
//! `print_json` byte-for-byte, and users can supply their own theme file. Each renderer projects the
//! same model onto its target: CSS rules for HTML, color and macro definitions for LaTeX, run
//! properties for DOCX.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::token::TokenKind;

/// A complete color theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Theme {
    /// Default foreground color, or `None` to inherit.
    pub text_color: Option<String>,
    /// Default background color, or `None` to inherit.
    pub background_color: Option<String>,
    /// Color of line numbers, or `None` to inherit.
    pub line_number_color: Option<String>,
    /// Background color behind line numbers, or `None` to inherit.
    pub line_number_background_color: Option<String>,
    /// Per-token-kind styling, keyed by canonical style name (`Keyword`, …). Only kinds the theme
    /// customizes are present; absent kinds render with the defaults.
    pub text_styles: BTreeMap<String, TokenStyle>,
}

/// Styling for one token kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TokenStyle {
    /// Foreground color, or `None` to inherit.
    pub text_color: Option<String>,
    /// Background color, or `None` to inherit.
    pub background_color: Option<String>,
    /// Whether the text is bold.
    pub bold: bool,
    /// Whether the text is italic.
    pub italic: bool,
    /// Whether the text is underlined.
    pub underline: bool,
}

impl Theme {
    /// Parse a theme from its JSON representation.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        serde_json::from_slice(bytes).map_err(|e| Error::Parse(e.to_string()))
    }

    /// Render the theme back to its JSON representation, using the same layout the theme files use:
    /// four-space indentation and no trailing newline.
    pub fn to_json(&self) -> Result<String, Error> {
        let mut buf = Vec::new();
        let indent = b"    ";
        let formatter = serde_json::ser::PrettyFormatter::with_indent(indent);
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
        self.serialize(&mut ser)
            .map_err(|e| Error::Parse(e.to_string()))?;
        String::from_utf8(buf).map_err(|e| Error::Parse(e.to_string()))
    }

    /// The styling for a token kind, if the theme customizes it.
    #[must_use]
    pub fn style_for(&self, kind: TokenKind) -> Option<&TokenStyle> {
        self.text_styles.get(kind.style_key())
    }
}

/// A failure loading or serializing a theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The JSON was malformed or did not match the theme schema.
    Parse(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Parse(msg) => write!(f, "invalid theme: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_a_minimal_theme() {
        let json = r##"{
    "text-color": null,
    "background-color": null,
    "line-number-color": "#aaaaaa",
    "line-number-background-color": null,
    "text-styles": {
        "Keyword": {
            "text-color": "#007020",
            "background-color": null,
            "bold": true,
            "italic": false,
            "underline": false
        }
    }
}"##;
        let theme = Theme::from_json(json.as_bytes()).expect("parse");
        assert_eq!(theme.to_json().expect("serialize"), json);
    }

    #[test]
    fn resolves_style_by_kind() {
        let json = r##"{
    "text-color": null,
    "background-color": null,
    "line-number-color": null,
    "line-number-background-color": null,
    "text-styles": {
        "Keyword": {
            "text-color": "#007020",
            "background-color": null,
            "bold": true,
            "italic": false,
            "underline": false
        }
    }
}"##;
        let theme = Theme::from_json(json.as_bytes()).expect("parse");
        assert_eq!(
            theme
                .style_for(TokenKind::Keyword)
                .expect("keyword")
                .text_color,
            Some("#007020".to_string())
        );
        assert!(theme.style_for(TokenKind::Comment).is_none());
    }
}
