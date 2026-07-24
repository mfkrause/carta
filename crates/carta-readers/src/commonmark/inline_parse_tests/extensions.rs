//! Extension-delimiter and markdown-dialect inline-parse tests.

use super::*;

// --- Extension delimiters ---

#[test]
fn strikeout_double_tilde() {
    assert_eq!(
        pe("~~a~~", exts(&[Extension::Strikeout])),
        vec![Inline::Strikeout(vec![str("a")])]
    );
}

#[test]
fn subscript_single_tilde() {
    assert_eq!(
        pe("~a~", exts(&[Extension::Subscript])),
        vec![Inline::Subscript(vec![str("a")])]
    );
}

#[test]
fn superscript_caret() {
    assert_eq!(
        pe("^a^", exts(&[Extension::Superscript])),
        vec![Inline::Superscript(vec![str("a")])]
    );
}

// --- Markdown-dialect inline rules ---

#[test]
fn markdown_escaped_space_becomes_non_breaking() {
    // broad escape set: `\ ` is a non-breaking space bound into the word; strict dialect and bare
    // engine: literal backslash, and the space splits the run
    assert_eq!(
        pm("a\\ b", exts(&[Extension::AllSymbolsEscapable])),
        vec![str("a\u{a0}b")]
    );
    assert_eq!(
        pm("a\\ b", no_ext()),
        vec![str("a\\"), Inline::Space, str("b")]
    );
    assert_eq!(p("a\\ b"), vec![str("a\\"), Inline::Space, str("b")]);
}

#[test]
fn broad_escape_set_is_gated_on_all_symbols_escapable() {
    // With the broad escape set a backslash drops before any ASCII punctuation.
    let broad = exts(&[Extension::AllSymbolsEscapable]);
    assert_eq!(pm("x\\|y", broad), vec![str("x|y")]);
    assert_eq!(pm("x\\~y", broad), vec![str("x~y")]);
    assert_eq!(pm("x\\<y", broad), vec![str("x<y")]);
    // Without it only the classic Markdown set is escapable; every other backslash stays literal.
    assert_eq!(pm("x\\|y", no_ext()), vec![str("x\\|y")]);
    assert_eq!(pm("x\\~y", no_ext()), vec![str("x\\~y")]);
    assert_eq!(pm("x\\<y", no_ext()), vec![str("x\\<y")]);
    // The classic set is escapable regardless of the extension.
    assert_eq!(pm("x\\!y", no_ext()), vec![str("x!y")]);
    assert_eq!(pm("x\\*y", no_ext()), vec![str("x*y")]);
    // The bare CommonMark engine escapes all ASCII punctuation with no extension needed.
    assert_eq!(p("x\\|y"), vec![str("x|y")]);
}

#[test]
fn markdown_superscript_rejects_inner_space() {
    // A raw space anywhere inside a superscript voids it; the delimiters stay literal.
    let ext = exts(&[Extension::Superscript, Extension::AllSymbolsEscapable]);
    assert_eq!(pm("^a b^", ext), vec![str("^a"), Inline::Space, str("b^")]);
    // An escaped (non-breaking) space keeps the superscript intact.
    assert_eq!(
        pm("^a\\ b^", ext),
        vec![Inline::Superscript(vec![str("a\u{a0}b")])]
    );
    assert_eq!(pm("^ab^", ext), vec![Inline::Superscript(vec![str("ab")])]);
}

#[test]
fn short_subsuperscripts_consume_an_alphanumeric_run() {
    let ext = exts(&[
        Extension::Superscript,
        Extension::Subscript,
        Extension::ShortSubsuperscripts,
    ]);
    // A caret or tilde with an alphanumeric run and no closing delimiter is a short script.
    assert_eq!(
        pm("x^2y", ext),
        vec![str("x"), Inline::Superscript(vec![str("2y")])]
    );
    assert_eq!(
        pm("H~2O", ext),
        vec![str("H"), Inline::Subscript(vec![str("2O")])]
    );
    // The run stops at the first non-alphanumeric character.
    assert_eq!(
        pm("x^2.5", ext),
        vec![str("x"), Inline::Superscript(vec![str("2")]), str(".5")]
    );
    // a closing delimiter forms the delimited pair; a leftover unpaired caret still opens a short script
    assert_eq!(
        pm("a^b^c", ext),
        vec![str("a"), Inline::Superscript(vec![str("b")]), str("c")]
    );
    assert_eq!(
        pm("a^b^c^d", ext),
        vec![
            str("a"),
            Inline::Superscript(vec![str("b")]),
            str("c"),
            Inline::Superscript(vec![str("d")]),
        ]
    );
    // A delimiter with no alphanumeric run is literal.
    assert_eq!(pm("x^(2)", ext), vec![str("x^(2)")]);
    assert_eq!(pm("foo^", ext), vec![str("foo^")]);
    // Without the extension the short form does not fire.
    let off = exts(&[Extension::Superscript, Extension::Subscript]);
    assert_eq!(pm("x^2y", off), vec![str("x^2y")]);
}

#[test]
fn markdown_subscript_rejects_inner_space_but_strikeout_allows_it() {
    // A single tilde is a subscript and rejects inner whitespace.
    assert_eq!(
        pm("~a b~", exts(&[Extension::Subscript])),
        vec![str("~a"), Inline::Space, str("b~")]
    );
    // A double tilde is a strikeout, which may hold whitespace.
    assert_eq!(
        pm("~~a b~~", exts(&[Extension::Strikeout])),
        vec![Inline::Strikeout(vec![str("a"), Inline::Space, str("b")])]
    );
}

#[test]
fn markdown_superscript_rejects_space_in_nested_span() {
    // Whitespace inside an already-built nested inline voids the superscript too.
    let ext = exts(&[Extension::Superscript]);
    assert_eq!(
        pm("^*a b*^", ext),
        vec![
            str("^"),
            Inline::Emph(vec![str("a"), Inline::Space, str("b")]),
            str("^"),
        ]
    );
}

#[test]
fn markdown_code_span_trims_surrounding_space() {
    // markdown dialect trims code span content; strict strips at most one space per side, never from all-space content
    assert_eq!(pm("`  a  `", no_ext()), vec![code("a")]);
    assert_eq!(p("` a `"), vec![code("a")]);
    assert_eq!(p("`  a  `"), vec![code(" a ")]);
}

#[test]
fn inline_note_parses_bracket_content_as_paragraph() {
    assert_eq!(
        pe("x^[a *b*] y", exts(&[Extension::InlineNotes])),
        vec![
            str("x"),
            Inline::Note(vec![Block::Para(vec![
                str("a"),
                Inline::Space,
                Inline::Emph(vec![str("b")]),
            ])]),
            Inline::Space,
            str("y"),
        ]
    );
}

#[test]
fn inline_note_allows_nested_brackets() {
    assert_eq!(
        pe("^[outer [inner] end]", exts(&[Extension::InlineNotes])),
        vec![Inline::Note(vec![Block::Para(vec![
            str("outer"),
            Inline::Space,
            str("[inner]"),
            Inline::Space,
            str("end"),
        ])])]
    );
}

#[test]
fn empty_inline_note_is_an_empty_paragraph() {
    assert_eq!(
        pe("^[]", exts(&[Extension::InlineNotes])),
        vec![Inline::Note(vec![Block::Para(vec![])])]
    );
}

#[test]
fn unclosed_inline_note_stays_literal() {
    assert_eq!(
        pe("^[unclosed", exts(&[Extension::InlineNotes])),
        vec![str("^[unclosed")]
    );
}

#[test]
fn inline_note_syntax_is_literal_when_extension_off() {
    assert_eq!(
        pe("x^[a] y", Extensions::empty()),
        vec![str("x^[a]"), Inline::Space, str("y")]
    );
}

#[test]
fn inline_note_wins_over_superscript_for_bracket() {
    // With both on, `^[` opens a note; a bare `^2^` would still be a superscript elsewhere.
    assert_eq!(
        pe(
            "y^[n]",
            exts(&[Extension::InlineNotes, Extension::Superscript])
        ),
        vec![str("y"), Inline::Note(vec![Block::Para(vec![str("n")])])]
    );
}

#[test]
fn double_tilde_with_subscript_only_becomes_nested_subscript() {
    // Strikeout off, subscript on: ~~a~~ is two nested subscripts (each `~` consumed one).
    assert_eq!(
        pe("~~a~~", exts(&[Extension::Subscript])),
        vec![Inline::Subscript(vec![Inline::Subscript(vec![str("a")])])]
    );
}

#[test]
fn single_tilde_skipped_when_strikeout_only() {
    // strikeout on, subscript off: the length-1 run has no mapping and stays literal; `~~b~~` matches as strikeout
    assert_eq!(
        pe("~a~~b~~", exts(&[Extension::Strikeout])),
        vec![str("~a"), Inline::Strikeout(vec![str("b")])]
    );
}

#[test]
fn unmatched_tilde_run_stays_literal_when_strikeout_only() {
    // `~~a~`: the single `~` closer finds no opener (`~~` needs a length-2 pair, subscript off), so all stays literal
    assert_eq!(pe("~~a~", exts(&[Extension::Strikeout])), vec![str("~~a~")]);
}

#[test]
fn mixed_asterisk_and_strikeout() {
    assert_eq!(
        pe("*a ~~b~~ c*", exts(&[Extension::Strikeout])),
        vec![Inline::Emph(vec![
            str("a"),
            Inline::Space,
            Inline::Strikeout(vec![str("b")]),
            Inline::Space,
            str("c"),
        ])]
    );
}
