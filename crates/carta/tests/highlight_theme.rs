//! Snapshot built-in theme CSS and LaTeX macros; review with `cargo insta review`.

#![cfg(all(feature = "highlight", feature = "write-html"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;

use carta::{builtin_style, styles};
use carta_writers::{theme_css, theme_latex_macros};

#[test]
fn builtin_theme_css_and_macros() {
    let mut css = String::new();
    let mut macros = String::new();
    for name in styles() {
        let theme = builtin_style(&name)
            .unwrap_or_else(|| panic!("built-in style {name} is registered"))
            .unwrap_or_else(|error| panic!("built-in style {name} parses: {error}"));
        let _ = write!(css, "===== {name} =====\n{}\n", theme_css(&theme));
        let _ = write!(
            macros,
            "===== {name} =====\n{}\n",
            theme_latex_macros(&theme)
        );
    }
    insta::assert_snapshot!("theme_css", css);
    insta::assert_snapshot!("theme_latex_macros", macros);
}
