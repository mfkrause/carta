//! Golden snapshots of the stylesheet and macro preambles the writers embed for each built-in color
//! theme: the HTML/EPUB `<style>` body (`theme_css`) and the LaTeX per-token macros
//! (`theme_latex_macros`). These freeze the exact bytes offline, independent of any command line, so
//! the layout rules, background and line-number colors, and per-kind token rules stay stable.
//! Reviewed with `cargo insta review`; never hand-edit the `.snap`s.

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
