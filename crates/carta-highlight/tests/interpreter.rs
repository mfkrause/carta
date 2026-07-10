//! Behavioral coverage of the tokenizer against small, purpose-built syntax definitions.
//!
//! Each test registers a minimal definition that exercises one rule type or one piece of the context
//! machinery, then asserts the classified spans a line tokenizes to. Building definitions by hand —
//! rather than leaning on the bundled catalog — keeps every rule's behavior pinned independently of
//! any one language's quirks, and reaches the matchers the bundled grammars happen not to use.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use carta_highlight::{Highlighter, TokenKind};

/// The item-data block shared by every definition below: one entry per attribute name the tests
/// reference, each bound to a default style so the resulting token kind is predictable.
const ITEM_DATAS: &str = r#"
  <itemDatas>
    <itemData name="nrm" defStyleNum="dsNormal"/>
    <itemData name="kw"  defStyleNum="dsKeyword"/>
    <itemData name="dv"  defStyleNum="dsDecVal"/>
    <itemData name="bn"  defStyleNum="dsBaseN"/>
    <itemData name="fl"  defStyleNum="dsFloat"/>
    <itemData name="ch"  defStyleNum="dsChar"/>
    <itemData name="sc"  defStyleNum="dsSpecialChar"/>
    <itemData name="st"  defStyleNum="dsString"/>
    <itemData name="op"  defStyleNum="dsOperator"/>
    <itemData name="co"  defStyleNum="dsComment"/>
    <itemData name="fu"  defStyleNum="dsFunction"/>
    <itemData name="va"  defStyleNum="dsVariable"/>
  </itemDatas>"#;

/// Assemble a definition from its optional `<general>` block, keyword lists, and contexts.
fn definition(name: &str, general: &str, lists: &str, contexts: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<language name="{name}" section="Test" extensions="*.{name}">
  {general}
  <highlighting>
    {lists}
    <contexts>
      {contexts}
    </contexts>
    {ITEM_DATAS}
  </highlighting>
</language>"#
    )
}

/// A highlighter carrying every supplied definition, registered in order.
fn with(defs: &[&str]) -> Highlighter {
    let mut hl = Highlighter::new();
    for def in defs {
        hl.registry_mut()
            .add_definition(def)
            .expect("valid definition");
    }
    hl
}

/// Tokenize `code` and return `(kind, text)` pairs per line.
fn lines(hl: &Highlighter, lang: &str, code: &str) -> Vec<Vec<(TokenKind, String)>> {
    hl.highlight(lang, code)
        .expect("known language")
        .into_iter()
        .map(|line| line.into_iter().map(|t| (t.kind, t.text)).collect())
        .collect()
}

/// Tokenize a single-line `code` and return its `(kind, text)` pairs.
fn line0(hl: &Highlighter, lang: &str, code: &str) -> Vec<(TokenKind, String)> {
    lines(hl, lang, code).into_iter().next().unwrap_or_default()
}

/// Shorthand to build one expected `(kind, text)` pair.
fn tok(kind: TokenKind, text: &str) -> (TokenKind, String) {
    (kind, text.to_string())
}

// --- number and C-literal matchers ------------------------------------------

#[test]
fn number_matchers_classify_each_base() {
    let def = definition(
        "num",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <HlCHex attribute="bn" context="#stay"/>
             <HlCOct attribute="bn" context="#stay"/>
             <Float attribute="fl" context="#stay"/>
             <Int attribute="dv" context="#stay"/>
             <DetectSpaces context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "num", "0x1F 0755 3.5 42"),
        vec![
            tok(TokenKind::BaseN, "0x1F"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::BaseN, "0755"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Float, "3.5"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::DecVal, "42"),
        ]
    );
}

#[test]
fn float_shapes_and_int_signs() {
    let def = definition(
        "inum",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <Float attribute="fl" context="#stay"/>
             <Int attribute="dv" context="#stay"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    // The exponent-only and leading-dot float shapes, plus a signed integer whose sign is reached
    // only when a word boundary precedes it (so an identifier sits in front).
    assert_eq!(
        line0(&hl, "inum", "a-0x10"),
        vec![
            tok(TokenKind::Variable, "a"),
            tok(TokenKind::DecVal, "-0x10")
        ]
    );
    assert_eq!(
        line0(&hl, "inum", "a-010"),
        vec![
            tok(TokenKind::Variable, "a"),
            tok(TokenKind::DecVal, "-010")
        ]
    );
    assert_eq!(
        line0(&hl, "inum", "a-42"),
        vec![tok(TokenKind::Variable, "a"), tok(TokenKind::DecVal, "-42")]
    );
    assert_eq!(
        line0(&hl, "inum", "2e3"),
        vec![tok(TokenKind::Float, "2e3")]
    );
    // The leading-dot shape needs a word boundary in front, so an identifier precedes the dot.
    assert_eq!(
        line0(&hl, "inum", "a.5"),
        vec![tok(TokenKind::Variable, "a"), tok(TokenKind::Float, ".5")]
    );
}

#[test]
fn c_string_and_char_escapes() {
    let def = definition(
        "cesc",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <HlCStringChar attribute="sc" context="#stay"/>
             <HlCChar attribute="ch" context="#stay"/>
             <DetectSpaces context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "cesc", r"\t \x41 'a'"),
        vec![
            tok(TokenKind::SpecialChar, r"\t"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::SpecialChar, r"\x41"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Char, "'a'"),
        ]
    );
    assert_eq!(
        line0(&hl, "cesc", r"\012 '\n'"),
        vec![
            tok(TokenKind::SpecialChar, r"\012"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Char, r"'\n'"),
        ]
    );
}

// --- character and literal matchers -----------------------------------------

#[test]
fn range_any_and_char_detectors() {
    let def = definition(
        "range",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <Detect2Chars attribute="co" char="/" char1="/" context="Comment"/>
             <RangeDetect attribute="st" char="&lt;" char1="&gt;" context="#stay"/>
             <AnyChar attribute="op" String="+-*" context="#stay"/>
             <DetectChar attribute="op" char="=" context="#stay"/>
           </context>
           <context name="Comment" attribute="co" lineEndContext="#pop"/>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "range", "//x"),
        vec![tok(TokenKind::Comment, "//x")]
    );
    assert_eq!(
        line0(&hl, "range", "<abc> +="),
        vec![
            tok(TokenKind::String, "<abc>"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Operator, "+="),
        ]
    );
}

#[test]
fn word_and_string_detect_with_boundaries() {
    let def = definition(
        "words",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <WordDetect attribute="kw" String="if" insensitive="true" context="#stay"/>
             <StringDetect attribute="op" String="=&gt;" context="#stay"/>
             <DetectIdentifier attribute="va" context="#stay"/>
             <DetectSpaces context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "words", "IF ifx => a"),
        vec![
            tok(TokenKind::Keyword, "IF"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Variable, "ifx"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Operator, "=>"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Variable, "a"),
        ]
    );
}

// --- keyword lists ----------------------------------------------------------

#[test]
fn keywords_are_case_insensitive_and_pull_in_included_lists() {
    let def = definition(
        "kws",
        r#"<general><keywords casesensitive="false" additionalDeliminator="$"/></general>"#,
        r#"<list name="base"><item>int</item></list>
           <list name="all"><item>float</item><include>base</include></list>"#,
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <keyword attribute="kw" String="all" context="#stay"/>
             <DetectSpaces context="#stay"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "kws", "INT float x$y"),
        vec![
            tok(TokenKind::Keyword, "INT"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Keyword, "float"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Variable, "x"),
            tok(TokenKind::Normal, "$"),
            tok(TokenKind::Variable, "y"),
        ]
    );
}

#[test]
fn keyword_list_pulls_words_from_another_definition() {
    let source = definition(
        "kwsrc",
        "",
        r#"<list name="shared"><item>shared_kw</item></list>"#,
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay"/>"##,
    );
    let dest = definition(
        "kwdst",
        "",
        r#"<list name="mine"><include>shared##kwsrc</include></list>"#,
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <keyword attribute="kw" String="mine" context="#stay"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&source, &dest]);
    assert_eq!(
        line0(&hl, "kwdst", "shared_kw"),
        vec![tok(TokenKind::Keyword, "shared_kw")]
    );
}

// --- regular expressions ----------------------------------------------------

#[test]
fn regex_word_boundary_and_case_folding() {
    let def = definition(
        "rxa",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <RegExpr attribute="kw" String="\band\b" insensitive="true" context="#stay"/>
             <DetectSpaces context="#stay"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "rxa", "AND andx"),
        vec![
            tok(TokenKind::Keyword, "AND"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Variable, "andx"),
        ]
    );
}

#[test]
fn minimal_regex_stops_at_the_first_close() {
    let def = definition(
        "rxm",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <RegExpr attribute="st" String="&quot;.*&quot;" minimal="true" context="#stay"/>
             <DetectIdentifier attribute="va" context="#stay"/>
             <DetectSpaces context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    // Greedy `.*` would swallow the whole line; the minimal flag stops at the first closing quote.
    assert_eq!(
        line0(&hl, "rxm", r#""a" x "b""#),
        vec![
            tok(TokenKind::String, r#""a""#),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::Variable, "x"),
            tok(TokenKind::Normal, " "),
            tok(TokenKind::String, r#""b""#),
        ]
    );
}

#[test]
fn dynamic_string_detect_matches_a_captured_delimiter() {
    let def = definition(
        "dynstr",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <RegExpr attribute="op" String="&lt;&lt;(\w+)" context="Heredoc"/>
             <DetectIdentifier attribute="va" context="#stay"/>
             <DetectSpaces context="#stay"/>
           </context>
           <context name="Heredoc" attribute="st" dynamic="true" lineEndContext="#stay">
             <StringDetect attribute="op" dynamic="true" String="%1" context="#pop"/>
             <DetectIdentifier attribute="st" context="#stay"/>
             <DetectSpaces context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "dynstr", "<<END x END"),
        vec![
            tok(TokenKind::Operator, "<<END"),
            tok(TokenKind::String, " x "),
            tok(TokenKind::Operator, "END"),
        ]
    );
}

#[test]
fn dynamic_char_matches_a_captured_character() {
    let def = definition(
        "dynchar",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <RegExpr attribute="op" String="x(.)" context="Q"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>
           <context name="Q" attribute="st" dynamic="true" lineEndContext="#stay">
             <DetectChar attribute="op" dynamic="true" char="1" context="#pop"/>
             <DetectIdentifier attribute="st" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "dynchar", "x.a.z"),
        vec![
            tok(TokenKind::Operator, "x."),
            tok(TokenKind::String, "a"),
            tok(TokenKind::Operator, "."),
            tok(TokenKind::Variable, "z"),
        ]
    );
}

#[test]
fn dynamic_regex_escapes_the_captured_text() {
    let def = definition(
        "dynrx",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <RegExpr attribute="op" String="x(.)" context="Q"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>
           <context name="Q" attribute="st" dynamic="true" lineEndContext="#stay">
             <RegExpr attribute="op" dynamic="true" String="%1+" context="#pop"/>
             <DetectIdentifier attribute="st" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    // The captured `.` is a regex metacharacter; it is escaped before substitution, so `%1+` matches
    // a run of literal dots, not any run of characters.
    assert_eq!(
        line0(&hl, "dynrx", "x.a..."),
        vec![
            tok(TokenKind::Operator, "x."),
            tok(TokenKind::String, "a"),
            tok(TokenKind::Operator, "..."),
        ]
    );
}

// --- line continuation ------------------------------------------------------

#[test]
fn line_continuation_carries_state_across_the_break() {
    let def = definition(
        "cont",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <LineContinue attribute="op" char="\" context="#stay"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        lines(&hl, "cont", "a\\\nb"),
        vec![
            vec![
                tok(TokenKind::Variable, "a"),
                tok(TokenKind::Operator, "\\")
            ],
            vec![tok(TokenKind::Variable, "b")],
        ]
    );
}

// --- rule inclusion ---------------------------------------------------------

#[test]
fn include_rules_splices_a_local_context() {
    let def = definition(
        "inc",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <IncludeRules context="Shared"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>
           <context name="Shared" attribute="co" lineEndContext="#stay">
             <DetectChar attribute="kw" char="#" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "inc", "#a"),
        vec![tok(TokenKind::Keyword, "#"), tok(TokenKind::Variable, "a")]
    );
}

#[test]
fn include_rules_repaints_with_include_attrib() {
    let def = definition(
        "incattr",
        "",
        "",
        r##"<context name="Main" attribute="kw" lineEndContext="#stay">
             <IncludeRules context="Plain" includeAttrib="true"/>
           </context>
           <context name="Plain" attribute="nrm" lineEndContext="#stay">
             <DetectChar char="z" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    // The included rule produces a Normal span; `includeAttrib` repaints it with the including
    // context's own attribute, here Keyword.
    assert_eq!(
        line0(&hl, "incattr", "z"),
        vec![tok(TokenKind::Keyword, "z")]
    );
}

#[test]
fn include_rules_reaches_into_another_definition() {
    let guest = definition(
        "guest",
        "",
        "",
        r##"<context name="Body" attribute="kw" lineEndContext="#stay">
             <DetectChar attribute="kw" char="g" context="#stay"/>
           </context>"##,
    );
    let host = definition(
        "host",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <IncludeRules context="Body##guest"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&guest, &host]);
    assert_eq!(
        line0(&hl, "host", "ga"),
        vec![tok(TokenKind::Keyword, "g"), tok(TokenKind::Variable, "a")]
    );
}

#[test]
fn mutually_including_contexts_terminate() {
    let def = definition(
        "cyc",
        "",
        "",
        r##"<context name="A" attribute="nrm" lineEndContext="#stay">
             <IncludeRules context="B"/>
           </context>
           <context name="B" attribute="nrm" lineEndContext="#stay">
             <IncludeRules context="A"/>
             <DetectChar attribute="kw" char="c" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    // The include-depth guard breaks the A↔B cycle; tokenization still reaches the literal rule.
    assert_eq!(line0(&hl, "cyc", "c"), vec![tok(TokenKind::Keyword, "c")]);
}

// --- context transitions ----------------------------------------------------

#[test]
fn multi_pop_unwinds_several_contexts_at_once() {
    let def = definition(
        "popn",
        "",
        "",
        r##"<context name="Base" attribute="nrm" lineEndContext="#stay">
             <DetectChar attribute="op" char="a" context="One"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>
           <context name="One" attribute="nrm" lineEndContext="#stay">
             <DetectChar attribute="op" char="b" context="Two"/>
           </context>
           <context name="Two" attribute="st" lineEndContext="#stay">
             <DetectChar attribute="op" char="c" context="#pop#pop"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "popn", "abcX"),
        vec![
            tok(TokenKind::Operator, "abc"),
            tok(TokenKind::Variable, "X")
        ]
    );
}

#[test]
fn line_end_context_pops_at_the_break() {
    let def = definition(
        "le",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <DetectChar attribute="op" char="#" context="Cmt"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>
           <context name="Cmt" attribute="co" lineEndContext="#pop">
             <DetectIdentifier attribute="co" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        lines(&hl, "le", "#ab\nx"),
        vec![
            vec![tok(TokenKind::Operator, "#"), tok(TokenKind::Comment, "ab")],
            vec![tok(TokenKind::Variable, "x")],
        ]
    );
}

#[test]
fn line_empty_context_switches_on_a_blank_line() {
    let def = definition(
        "empty",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <DetectChar attribute="op" char="#" context="Cmt"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>
           <context name="Cmt" attribute="co" lineEndContext="#stay" lineEmptyContext="#pop">
             <DetectIdentifier attribute="co" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        lines(&hl, "empty", "#ab\n\nx"),
        vec![
            vec![tok(TokenKind::Operator, "#"), tok(TokenKind::Comment, "ab")],
            vec![],
            vec![tok(TokenKind::Variable, "x")],
        ]
    );
}

#[test]
fn fallthrough_to_a_target_leaves_the_context_without_consuming() {
    let def = definition(
        "ft",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <DetectChar attribute="st" char="&quot;" context="Str"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>
           <context name="Str" attribute="st" fallthrough="true" fallthroughContext="#pop" lineEndContext="#stay">
             <DetectChar attribute="st" char="&quot;" context="#pop"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "ft", "\"a"),
        vec![tok(TokenKind::String, "\""), tok(TokenKind::Variable, "a")]
    );
}

#[test]
fn boolean_fallthrough_pops_a_single_context() {
    let def = definition(
        "ftb",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <DetectChar attribute="op" char="(" context="P"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>
           <context name="P" attribute="st" fallthrough="true" lineEndContext="#stay">
             <DetectChar attribute="op" char=")" context="#pop"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "ftb", "(a"),
        vec![tok(TokenKind::Operator, "("), tok(TokenKind::Variable, "a")]
    );
}

#[test]
fn look_ahead_switches_context_without_consuming() {
    let def = definition(
        "la",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <StringDetect String="foo" lookAhead="true" context="Foo"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>
           <context name="Foo" attribute="kw" lineEndContext="#stay">
             <StringDetect attribute="kw" String="foo" context="#pop"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "la", "foo"),
        vec![tok(TokenKind::Keyword, "foo")]
    );
}

#[test]
fn column_constrains_a_rule_to_a_fixed_offset() {
    let def = definition(
        "col",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <DetectChar attribute="kw" char="x" column="0" context="#stay"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "col", "xxy"),
        vec![tok(TokenKind::Keyword, "x"), tok(TokenKind::Variable, "xy")]
    );
}

#[test]
fn first_non_space_constrains_a_rule_to_the_line_start() {
    let def = definition(
        "fns",
        "",
        "",
        r##"<context name="Normal" attribute="nrm" lineEndContext="#stay">
             <DetectChar attribute="kw" char="#" firstNonSpace="true" context="#stay"/>
             <DetectSpaces context="#stay"/>
             <DetectIdentifier attribute="va" context="#stay"/>
           </context>"##,
    );
    let hl = with(&[&def]);
    assert_eq!(
        line0(&hl, "fns", "  #a"),
        vec![
            tok(TokenKind::Normal, "  "),
            tok(TokenKind::Keyword, "#"),
            tok(TokenKind::Variable, "a"),
        ]
    );
}
