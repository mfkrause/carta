use super::{MarkdownConfig, State};
use carta_core::{Extension, Extensions, WrapMode};

fn state(extensions: Extensions, cmark: bool) -> State {
    State::new(MarkdownConfig { extensions, cmark }, 72, WrapMode::Auto)
}

#[test]
fn citation_marker_sees_word_start_after_a_verbatim_run() {
    let state = state(Extensions::from_list(&[Extension::Citations]), false);
    assert_eq!(state.escape_str("see @cite"), "see \\@cite");
    assert_eq!(state.escape_str("user@host"), "user@host");
}

#[test]
fn underscore_escaping_depends_on_neighbors_across_verbatim_runs() {
    let intraword = state(Extensions::empty(), true);
    assert_eq!(intraword.escape_str("snake_case"), "snake_case");
    assert_eq!(intraword.escape_str("a _b"), "a \\_b");
    let plain = state(Extensions::empty(), false);
    assert_eq!(plain.escape_str("snake_case"), "snake\\_case");
}

#[test]
fn backslash_run_parity_resets_across_verbatim_runs() {
    let state = state(Extensions::empty(), true);
    assert_eq!(state.escape_str("a\\b"), "a\\b");
    // A trailing lone backslash pads to an even pair; an already-even run stays as-is.
    assert_eq!(state.escape_str("\\x\\"), "\\x\\\\");
    assert_eq!(state.escape_str("x\\\\"), "x\\\\");
}
