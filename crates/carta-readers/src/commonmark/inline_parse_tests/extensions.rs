//! Extension-delimiter and markdown-dialect inline-parse tests.

use super::*;

// Extension delimiters

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

#[test]
fn markdown_keeps_empty_scripts_and_pairs_adjacent_delimiters() {
    let extensions = exts(&[Extension::Superscript, Extension::Subscript]);
    for (source, expected) in [
        (
            "a^^b",
            vec![str("a"), Inline::Superscript(vec![]), str("b")],
        ),
        ("a~~b", vec![str("a"), Inline::Subscript(vec![]), str("b")]),
        (
            "^^a^^",
            vec![
                Inline::Superscript(vec![]),
                str("a"),
                Inline::Superscript(vec![]),
            ],
        ),
        ("^^^a^^^", vec![Inline::Superscript(vec![str("a")])]),
        (
            "^a^^b^",
            vec![Inline::Superscript(vec![str("a"), str("b")])],
        ),
        (
            "a^b^^",
            vec![str("a"), Inline::Superscript(vec![str("b")]), str("^")],
        ),
    ] {
        assert_eq!(pm(source, extensions), expected, "{source}");
    }
    assert_eq!(pe("a^^b", extensions), vec![str("a^^b")]);
}

// Markdown-dialect inline rules

#[test]
fn undefined_brackets_consume_one_link_attribute_block() {
    let extensions = exts(&[Extension::LinkAttributes]);
    assert_eq!(pm("[text]{}", extensions), vec![str("[text]")]);
    assert_eq!(
        pm("[*text*]{.first}{.second}", extensions),
        vec![str("["), Inline::Emph(vec![str("text")]), str("]{.second}")]
    );
    assert_eq!(pm("[text]{.class}", no_ext()), vec![str("[text]{.class}")]);
}

#[test]
fn markdown_escaped_space_becomes_non_breaking() {
    // Broad escapes treat `\ ` as a non-breaking space; the strict engine keeps the backslash.
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
    let broad = exts(&[Extension::AllSymbolsEscapable]);
    assert_eq!(pm("x\\|y", broad), vec![str("x|y")]);
    assert_eq!(pm("x\\~y", broad), vec![str("x~y")]);
    assert_eq!(pm("x\\<y", broad), vec![str("x<y")]);
    assert_eq!(pm("x\\|y", no_ext()), vec![str("x\\|y")]);
    assert_eq!(pm("x\\~y", no_ext()), vec![str("x\\~y")]);
    assert_eq!(pm("x\\<y", no_ext()), vec![str("x\\<y")]);
    assert_eq!(pm("x\\!y", no_ext()), vec![str("x!y")]);
    assert_eq!(pm("x\\*y", no_ext()), vec![str("x*y")]);
    assert_eq!(p("x\\|y"), vec![str("x|y")]);
}

#[test]
fn markdown_superscript_rejects_inner_space() {
    let ext = exts(&[Extension::Superscript, Extension::AllSymbolsEscapable]);
    assert_eq!(pm("^a b^", ext), vec![str("^a"), Inline::Space, str("b^")]);
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
    assert_eq!(
        pm("x^2y", ext),
        vec![str("x"), Inline::Superscript(vec![str("2y")])]
    );
    assert_eq!(
        pm("H~2O", ext),
        vec![str("H"), Inline::Subscript(vec![str("2O")])]
    );
    assert_eq!(
        pm("x^2.5", ext),
        vec![str("x"), Inline::Superscript(vec![str("2")]), str(".5")]
    );
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
    assert_eq!(pm("x^(2)", ext), vec![str("x^(2)")]);
    assert_eq!(pm("foo^", ext), vec![str("foo^")]);
    let off = exts(&[Extension::Superscript, Extension::Subscript]);
    assert_eq!(pm("x^2y", off), vec![str("x^2y")]);
}

#[test]
fn markdown_subscript_rejects_inner_space_but_strikeout_allows_it() {
    assert_eq!(
        pm("~a b~", exts(&[Extension::Subscript])),
        vec![str("~a"), Inline::Space, str("b~")]
    );
    assert_eq!(
        pm("~~a b~~", exts(&[Extension::Strikeout])),
        vec![Inline::Strikeout(vec![str("a"), Inline::Space, str("b")])]
    );
}

#[test]
fn markdown_superscript_rejects_space_in_nested_span() {
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
    // Markdown trims code edges; strict mode strips one space except from all-space spans.
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
    assert_eq!(
        pe("~~a~~", exts(&[Extension::Subscript])),
        vec![Inline::Subscript(vec![Inline::Subscript(vec![str("a")])])]
    );
}

#[test]
fn single_tilde_skipped_when_strikeout_only() {
    assert_eq!(
        pe("~a~~b~~", exts(&[Extension::Strikeout])),
        vec![str("~a"), Inline::Strikeout(vec![str("b")])]
    );
}

#[test]
fn unmatched_tilde_run_stays_literal_when_strikeout_only() {
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
