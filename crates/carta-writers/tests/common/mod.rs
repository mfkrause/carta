//! Shared helpers for the writer highlight tests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    dead_code,
    unreachable_pub
)]

use carta_highlight::Highlighter;

/// A highlighter with the Python grammar loaded from the runtime pack, since the highlight
/// fixtures colorize Python and the default embedded set omits it.
pub fn highlighter_with_python() -> Highlighter {
    let mut highlighter = Highlighter::new();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../carta-highlight/data/syntax-copyleft/python.xml");
    let xml = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    highlighter
        .registry_mut()
        .add_definition_with_stem(&xml, "python")
        .expect("parse python grammar");
    highlighter
}
