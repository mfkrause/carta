use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use carta_ast::{Attr, Block, Citation, CitationMode, Inline, Target};

use super::super::identifiers::HeaderNumbering;
use super::super::resolve::{
    HeaderParseCache, RefContext, gather_headers, heading_content_is_context_independent,
    resolve_block,
};
use super::super::{ExampleMap, IrBlock, LinkDef, RefMap};
use super::parse_inlines;
use carta_core::{Extension, Extensions};

static NO_DEFINED: BTreeSet<String> = BTreeSet::new();
static NO_BY_ID: BTreeMap<String, Vec<Block>> = BTreeMap::new();
static NO_EXAMPLES: ExampleMap = BTreeMap::new();

/// An empty reference context, for tests that exercise inline syntax without footnotes or example
/// references. Each call leaks a fresh citation count so a test starts numbering from zero.
fn no_notes() -> RefContext<'static> {
    RefContext {
        defined: &NO_DEFINED,
        by_id: &NO_BY_ID,
        in_definition: false,
        markdown: false,
        examples: &NO_EXAMPLES,
        cite_count: Box::leak(Box::new(Cell::new(0))),
    }
}

fn no_ext() -> Extensions {
    Extensions::empty()
}

fn exts(list: &[Extension]) -> Extensions {
    Extensions::from_list(list)
}

fn empty_refs() -> RefMap {
    BTreeMap::new()
}

fn ref_map(entries: &[(&str, &str)]) -> RefMap {
    let mut m = BTreeMap::new();
    for (k, v) in entries {
        m.insert(
            k.to_string(),
            LinkDef {
                url: v.to_string(),
                title: String::new(),
            },
        );
    }
    m
}

fn p(text: &str) -> Vec<Inline> {
    parse_inlines(text, &empty_refs(), no_notes(), no_ext())
}

fn pe(text: &str, ext: Extensions) -> Vec<Inline> {
    parse_inlines(text, &empty_refs(), no_notes(), ext)
}

/// A reference context in the markdown dialect, where escaped spaces bind as non-breaking
/// spaces, code spans trim their content, and superscripts and subscripts reject inner
/// whitespace.
fn md_notes() -> RefContext<'static> {
    RefContext {
        markdown: true,
        ..no_notes()
    }
}

fn pm(text: &str, ext: Extensions) -> Vec<Inline> {
    parse_inlines(text, &empty_refs(), md_notes(), ext)
}

fn str(s: &str) -> Inline {
    Inline::Str(s.to_owned().into())
}

fn link(content: Vec<Inline>, url: &str) -> Inline {
    Inline::Link(
        Box::default(),
        content,
        Box::new(Target {
            url: url.to_owned().into(),
            title: carta_ast::Text::default(),
        }),
    )
}

fn image(alt: Vec<Inline>, url: &str) -> Inline {
    Inline::Image(
        Box::default(),
        alt,
        Box::new(Target {
            url: url.to_owned().into(),
            title: carta_ast::Text::default(),
        }),
    )
}

// --- Emphasis and strong ---

#[test]
fn nested_emphasis_and_strong() {
    // *a **b** c* → Emph([a, Strong([b]), c])
    assert_eq!(
        p("*a **b** c*"),
        vec![Inline::Emph(vec![
            str("a"),
            Inline::Space,
            Inline::Strong(vec![str("b")]),
            Inline::Space,
            str("c"),
        ])]
    );
}

#[test]
fn mixed_asterisk_and_underscore() {
    // *a _b_ c* → Emph([a, Emph([b]), c])
    assert_eq!(
        p("*a _b_ c*"),
        vec![Inline::Emph(vec![
            str("a"),
            Inline::Space,
            Inline::Emph(vec![str("b")]),
            Inline::Space,
            str("c"),
        ])]
    );
}

#[test]
fn triple_asterisk_produces_emph_of_strong() {
    // ***a*** → Emph([Strong([a])])
    assert_eq!(
        p("***a***"),
        vec![Inline::Emph(vec![Inline::Strong(vec![str("a")])])]
    );
}

#[test]
fn rule_of_3_prevents_outer_strong() {
    // **a*b** — the `*` closer + `**` opener sum is 3 which would violate rule-of-3 when one
    // side can both open and close, so the `*b` ends up literal inside Strong.
    assert_eq!(p("**a*b**"), vec![Inline::Strong(vec![str("a*b")])]);
}

#[test]
fn rule_of_3_prevents_inner_strong() {
    // *a**b* — **b closes with * giving sum=3 but both must be mult-of-3 which they aren't,
    // so the **b is left literal.
    assert_eq!(p("*a**b*"), vec![Inline::Emph(vec![str("a**b")])]);
}

#[test]
fn unmatched_openers_become_literal() {
    assert_eq!(p("*a"), vec![str("*a")]);
    assert_eq!(p("a*"), vec![str("a*")]);
    // **a* — the single * can close an emphasis inside the **, leaving ** - 1 = * literal
    assert_eq!(p("**a*"), vec![str("*"), Inline::Emph(vec![str("a")])]);
}

#[test]
fn underscore_intraword_stays_literal() {
    // `_` between word chars cannot open or close (spec §6.3 rules).
    assert_eq!(p("a_b_c"), vec![str("a_b_c")]);
    assert_eq!(p("_a_b"), vec![str("_a_b")]);
}

#[test]
fn emphasis_flanks_across_multi_byte_neighbors() {
    // `*` pairs intraword, so multi-byte word characters around the delimiters behave like
    // ASCII ones.
    assert_eq!(
        p("α*β*γ"),
        vec![str("α"), Inline::Emph(vec![str("β")]), str("γ")]
    );
}

#[test]
fn emphasis_with_multi_byte_content_at_input_edges() {
    // The opener sits at the very start of the input and the closer at its very end, so both
    // boundary lookups run against the buffer's edges.
    assert_eq!(p("*β*"), vec![Inline::Emph(vec![str("β")])]);
}

#[test]
fn emphasis_between_emoji_neighbors() {
    // An emoji is a symbol (punctuation for flanking purposes), not a word character, so the
    // run still opens and closes around it.
    assert_eq!(
        p("😀*a*😀"),
        vec![str("😀"), Inline::Emph(vec![str("a")]), str("😀")]
    );
}

#[test]
fn underscore_intraword_stays_literal_with_multi_byte_neighbors() {
    // The word-character test before and after a `_` run reads whole characters, not bytes.
    assert_eq!(p("α_β_γ"), vec![str("α_β_γ")]);
}

#[test]
fn empty_input_parses_to_nothing() {
    assert_eq!(p(""), Vec::new());
}

// --- Links and images ---

#[test]
fn inline_link_and_image() {
    assert_eq!(p("[a](u)"), vec![link(vec![str("a")], "u")]);
    assert_eq!(p("![i](u)"), vec![image(vec![str("i")], "u")]);
}

#[test]
fn unmatched_image_opener_keeps_its_bang() {
    // An image opener that never finds a closing `]` reverts to the literal `![`, not `[`.
    assert_eq!(p("![x"), vec![str("![x")]);
    assert_eq!(p("![[a]x"), vec![str("![[a]x")]);
}

#[test]
fn reference_link_with_and_without_ref() {
    // Without ref: stays literal.
    assert_eq!(p("[a][r]"), vec![str("[a][r]")]);
    // With ref defined: resolves.
    let refs = ref_map(&[("r", "http://r")]);
    let result = parse_inlines("[a][r]", &refs, no_notes(), no_ext());
    assert_eq!(result, vec![link(vec![str("a")], "http://r")]);
}

#[test]
fn shortcut_reference_resolves_only_with_a_matching_definition() {
    // No definitions in scope: the brackets stay literal.
    assert_eq!(p("[foo]"), vec![str("[foo]")]);
    // A matching definition resolves the shortcut, with case folding on the label.
    let refs = ref_map(&[("foo", "http://f")]);
    assert_eq!(
        parse_inlines("[Foo]", &refs, no_notes(), no_ext()),
        vec![link(vec![str("Foo")], "http://f")]
    );
}

#[test]
fn shortcut_label_near_the_length_bound_still_resolves() {
    // A 999-character label sits under the byte guard, so its lookup runs normally; the guard
    // only skips spans that are too long to be a label at all.
    let label = "a".repeat(999);
    let refs = ref_map(&[(label.as_str(), "http://f")]);
    let source = format!("[{label}]");
    assert_eq!(
        parse_inlines(&source, &refs, no_notes(), no_ext()),
        vec![link(vec![str(&label)], "http://f")]
    );
}

#[test]
fn span_past_the_label_bound_never_resolves_as_shortcut_or_collapsed() {
    // A span longer than MAX_LABEL_BYTES is no label, even when a definition with the same
    // oversized key exists: both the shortcut and the collapsed lookup leave it literal.
    let oversized = "a".repeat(super::MAX_LABEL_BYTES + 1);
    let refs = ref_map(&[(oversized.as_str(), "http://big")]);
    let shortcut = format!("[{oversized}]");
    assert_eq!(
        parse_inlines(&shortcut, &refs, no_notes(), no_ext()),
        vec![str(&shortcut)]
    );
    let collapsed = format!("[{oversized}][]");
    assert_eq!(
        parse_inlines(&collapsed, &refs, no_notes(), no_ext()),
        vec![str(&collapsed)]
    );
    // The same collapsed form under the bound resolves through the identical path.
    let refs = ref_map(&[("foo", "http://f")]);
    assert_eq!(
        parse_inlines("[Foo][]", &refs, no_notes(), no_ext()),
        vec![link(vec![str("Foo")], "http://f")]
    );
}

#[test]
fn footnote_reference_resolves_at_the_bracket_boundary() {
    let mut defined = BTreeSet::new();
    defined.insert("x".to_owned());
    let mut by_id: BTreeMap<String, Vec<Block>> = BTreeMap::new();
    by_id.insert("x".to_owned(), vec![Block::Para(vec![str("note")])]);
    let examples = ExampleMap::new();
    let cite = Cell::new(0);
    let notes = RefContext {
        defined: &defined,
        by_id: &by_id,
        in_definition: false,
        markdown: false,
        examples: &examples,
        cite_count: &cite,
    };
    let ext = exts(&[Extension::Footnotes]);
    assert_eq!(
        parse_inlines("[^x]", &empty_refs(), notes, ext),
        vec![Inline::Note(vec![Block::Para(vec![str("note")])])]
    );
    // With no footnotes defined, the same syntax stays literal.
    assert_eq!(pe("[^x]", ext), vec![str("[^x]")]);
}

#[test]
fn spaced_reference_link_allows_whitespace_before_the_label() {
    let refs = ref_map(&[("ref", "http://r"), ("text", "http://t")]);
    let ext = exts(&[Extension::SpacedReferenceLinks]);
    // A space or newline separates the text bracket from the reference label; the display comes
    // from the first bracket and the target from the second.
    assert_eq!(
        parse_inlines("[text] [ref]", &refs, no_notes(), ext),
        vec![link(vec![str("text")], "http://r")]
    );
    assert_eq!(
        parse_inlines("[text]\n[ref]", &refs, no_notes(), ext),
        vec![link(vec![str("text")], "http://r")]
    );
    // An empty second bracket is a collapsed reference keyed on the first bracket.
    assert_eq!(
        parse_inlines("[text] []", &refs, no_notes(), ext),
        vec![link(vec![str("text")], "http://t")]
    );
    // A defined text but an undefined second label leaves the whole run literal — the text is
    // not retried as a shortcut.
    let only_text = ref_map(&[("text", "http://t")]);
    assert_eq!(
        parse_inlines("[text] [ref]", &only_text, no_notes(), ext),
        vec![str("[text]"), Inline::Space, str("[ref]")]
    );
    // Without the extension the space breaks the pair into two shortcut references.
    assert_eq!(
        parse_inlines("[text] [ref]", &refs, no_notes(), no_ext()),
        vec![
            link(vec![str("text")], "http://t"),
            Inline::Space,
            link(vec![str("ref")], "http://r"),
        ]
    );
}

#[test]
fn nested_bracket_in_link_text() {
    // [[a]](u) — the inner [a] becomes a literal `[a]` in the link text because it has no
    // matching target of its own, and the outer pair provides the `(u)` target.
    assert_eq!(p("[[a]](u)"), vec![link(vec![str("[a]")], "u")]);
}

#[test]
fn unmatched_brackets_are_literal() {
    assert_eq!(p("]]]"), vec![str("]]]")]);
}

#[test]
fn link_suppresses_earlier_bracket_openers() {
    // [a [b](u) c](v) — the inner [b](u) is a valid link; its `[` opener then causes
    // the outer `[a ` opener to be deactivated (it cannot form a link containing a link),
    // so the outer `[` and `](v)` stay literal.
    assert_eq!(
        p("[a [b](u) c](v)"),
        vec![
            str("[a"),
            Inline::Space,
            link(vec![str("b")], "u"),
            Inline::Space,
            str("c](v)"),
        ]
    );
}

#[test]
fn emphasis_inside_link_text() {
    assert_eq!(
        p("[*a*](u)"),
        vec![link(vec![Inline::Emph(vec![str("a")])], "u")]
    );
}

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
    // With the broad escape set a markdown-dialect `\ ` is a non-breaking space bound into the
    // surrounding word; without it (as in the strict dialect) and in the bare CommonMark engine a
    // backslash before a space is a literal backslash and the space splits the run.
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
    // No inner whitespace: still a superscript.
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
    // A closing delimiter in the span forms the delimited pair instead; a leftover unpaired
    // caret then still opens a short script.
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
    // The markdown dialect trims a code span's content; the strict dialect strips at most a
    // single leading and trailing space (and only when the content is not all spaces).
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
    // `~a~~b~~` with strikeout on but subscript off: length-1 run has no strikeout mapping
    // (`match_use_count` returns None), so it stays literal; `~~b~~` matches as strikeout.
    assert_eq!(
        pe("~a~~b~~", exts(&[Extension::Strikeout])),
        vec![str("~a"), Inline::Strikeout(vec![str("b")])]
    );
}

#[test]
fn unmatched_tilde_run_stays_literal_when_strikeout_only() {
    // `~~a~` — the single `~` is a closer that can't find an opener (the `~~` needs length-2
    // pair and subscript is off), so the whole thing stays literal.
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

// --- TeX math ---

fn math_inline(content: &str) -> Inline {
    Inline::Math(carta_ast::MathType::InlineMath, content.to_owned().into())
}

fn math_display(content: &str) -> Inline {
    Inline::Math(carta_ast::MathType::DisplayMath, content.to_owned().into())
}

fn math() -> Extensions {
    exts(&[Extension::TexMathDollars])
}

#[test]
fn inline_and_display_math() {
    assert_eq!(pe("$a+b$", math()), vec![math_inline("a+b")]);
    assert_eq!(pe("$$x=y$$", math()), vec![math_display("x=y")]);
    // Display math keeps interior spaces verbatim.
    assert_eq!(pe("$$ x $$", math()), vec![math_display(" x ")]);
}

#[test]
fn dollar_amounts_are_not_math() {
    // An opener must be followed by a non-space; a closer may not follow a digit or trail a space.
    assert_eq!(
        pe("$5 and $10", math()),
        vec![
            str("$5"),
            Inline::Space,
            str("and"),
            Inline::Space,
            str("$10")
        ]
    );
    assert_eq!(pe("$a$5", math()), vec![str("$a$5")]);
    assert_eq!(pe("$ a$", math()), vec![str("$"), Inline::Space, str("a$")]);
}

#[test]
fn math_content_is_verbatim_but_honors_backslash_escape() {
    // `_`/`*` inside math do not start emphasis.
    assert_eq!(pe("$x_1*y*$", math()), vec![math_inline("x_1*y*")]);
    // An escaped dollar inside content does not close the span.
    assert_eq!(pe(r"$a\$b$", math()), vec![math_inline(r"a\$b")]);
}

#[test]
fn failed_display_falls_back_to_inline() {
    // `$$x$` has no closing `$$`; the first `$` is literal and `$x$` parses as inline math.
    assert_eq!(pe("$$x$", math()), vec![str("$"), math_inline("x")]);
}

#[test]
fn dollar_is_literal_without_the_extension() {
    assert_eq!(p("$a+b$"), vec![str("$a+b$")]);
}

// --- Attributes: spans, inline code, links ---

fn span(attr: Attr, content: Vec<Inline>) -> Inline {
    Inline::Span(Box::new(attr), content)
}

fn attr(id: &str, classes: &[&str], kv: &[(&str, &str)]) -> Attr {
    Attr {
        id: id.to_owned().into(),
        classes: classes.iter().map(|c| (*c).into()).collect(),
        attributes: kv.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
    }
}

fn attrs() -> Extensions {
    exts(&[Extension::Attributes])
}

#[test]
fn bracketed_span_carries_attributes() {
    assert_eq!(
        pe("[text]{.cls #id}", exts(&[Extension::BracketedSpans])),
        vec![span(attr("id", &["cls"], &[]), vec![str("text")])]
    );
}

#[test]
fn empty_attribute_block_is_not_a_span() {
    assert_eq!(
        pe("[text]{}", exts(&[Extension::BracketedSpans])),
        vec![str("[text]{}")]
    );
}

#[test]
fn consecutive_attribute_blocks_merge_first_id_wins() {
    // Adjacent blocks accumulate classes and key/value pairs; the first identifier is kept.
    assert_eq!(
        pe(
            "[x]{#one .a}{#two .b k=v}",
            exts(&[Extension::BracketedSpans])
        ),
        vec![span(
            attr("one", &["a", "b"], &[("k", "v")]),
            vec![str("x")]
        )]
    );
}

#[test]
fn span_wins_over_shortcut_reference() {
    let refs = ref_map(&[("text", "http://r")]);
    let ext = exts(&[Extension::BracketedSpans]);
    assert_eq!(
        parse_inlines("[text]{.c}", &refs, no_notes(), ext),
        vec![span(attr("", &["c"], &[]), vec![str("text")])]
    );
}

#[test]
fn inline_code_takes_attributes() {
    assert_eq!(
        pe("`code`{.rust #x}", attrs()),
        vec![Inline::Code(
            Box::new(attr("x", &["rust"], &[])),
            "code".to_owned().into()
        )]
    );
    // A space before the block leaves it unattached (no wrapper artifact is produced).
    assert_eq!(
        pe("`code` x", attrs()),
        vec![
            Inline::Code(Box::default(), "code".to_owned().into()),
            Inline::Space,
            str("x")
        ]
    );
}

#[test]
fn link_and_image_take_attributes() {
    let link_with_attr = Inline::Link(
        Box::new(attr("home", &["external"], &[])),
        vec![str("t")],
        Box::new(Target {
            url: "u".to_owned().into(),
            title: carta_ast::Text::default(),
        }),
    );
    assert_eq!(pe("[t](u){.external #home}", attrs()), vec![link_with_attr]);
    let image_with_attr = Inline::Image(
        Box::new(attr("", &[], &[("width", "200")])),
        vec![str("a")],
        Box::new(Target {
            url: "i".to_owned().into(),
            title: carta_ast::Text::default(),
        }),
    );
    assert_eq!(pe("![a](i){width=200}", attrs()), vec![image_with_attr]);
}

#[test]
fn attributes_require_the_extension() {
    // Without any attribute extension the block stays literal text.
    assert_eq!(p("[text]{.cls}"), vec![str("[text]{.cls}")]);
}

#[test]
fn nested_image_with_inner_link_and_deactivated_bracket() {
    // ![[[foo](uri1)](uri2)](uri3)
    //
    // The outermost `![` is an image opener. The first `[` inside is a plain bracket opener.
    // `[foo](uri1)` matches as a link; that success deactivates the `[` opener between the
    // image `![` and `[foo]`. The next `]` encounters that deactivated opener: it must pop
    // it, literalize it, and emit `]` as text — not look further to the image opener below.
    // Only the final `](uri3)` closes the image.
    //
    // Expected: Image(uri3, alt=[Str("["), Link([Str("foo")], uri1), Str("](uri2)")])
    assert_eq!(
        p("![[[foo](uri1)](uri2)](uri3)"),
        vec![image(
            vec![str("["), link(vec![str("foo")], "uri1"), str("](uri2)"),],
            "uri3",
        )]
    );
}

// --- Inline raw attribute (`{=FORMAT}` on a code span) ---

fn raw(format: &str, text: &str) -> Inline {
    Inline::RawInline(
        carta_ast::Format(format.to_owned().into()),
        text.to_owned().into(),
    )
}

fn code(text: &str) -> Inline {
    Inline::Code(Box::default(), text.to_owned().into())
}

#[test]
fn raw_attribute_turns_code_span_into_raw_inline() {
    let ext = exts(&[Extension::RawAttribute]);
    assert_eq!(pe("`<b>`{=html}", ext), vec![raw("html", "<b>")]);
    assert_eq!(pe("`\\x`{=latex}", ext), vec![raw("latex", "\\x")]);
}

#[test]
fn raw_attribute_format_token_allows_word_chars_dash_underscore() {
    let ext = exts(&[Extension::RawAttribute]);
    assert_eq!(pe("`x`{=my-format}", ext), vec![raw("my-format", "x")]);
    assert_eq!(pe("`x`{=my_fmt}", ext), vec![raw("my_fmt", "x")]);
    assert_eq!(pe("`x`{=3d}", ext), vec![raw("3d", "x")]);
}

#[test]
fn raw_attribute_tolerates_whitespace_around_marker() {
    let ext = exts(&[Extension::RawAttribute]);
    assert_eq!(pe("`x`{ =html }", ext), vec![raw("html", "x")]);
    assert_eq!(pe("`x`{=html }", ext), vec![raw("html", "x")]);
    assert_eq!(pe("`x`{ =html}", ext), vec![raw("html", "x")]);
}

#[test]
fn raw_attribute_normalizes_code_content() {
    let ext = exts(&[Extension::RawAttribute]);
    // A single space padding each side is stripped, exactly as for a code span.
    assert_eq!(pe("` x `{=html}", ext), vec![raw("html", "x")]);
}

#[test]
fn raw_attribute_requires_a_pure_format_marker() {
    let ext = exts(&[Extension::RawAttribute]);
    // A space between `=` and the format is not a marker.
    assert_eq!(
        pe("`x`{= html}", ext),
        vec![code("x"), str("{="), Inline::Space, str("html}"),]
    );
    // An empty format is not a marker.
    assert_eq!(pe("`x`{=}", ext), vec![code("x"), str("{=}")]);
    // Anything beyond the format (a class, a dot) defeats the marker.
    assert_eq!(pe("`x`{=a.b}", ext), vec![code("x"), str("{=a.b}")]);
}

#[test]
fn plain_attribute_block_on_code_span_is_not_raw() {
    // `{.class}` keeps the code span and applies the attribute (inline code attributes on).
    let ext = exts(&[Extension::RawAttribute, Extension::InlineCodeAttributes]);
    assert_eq!(
        pe("`x`{.c}", ext),
        vec![Inline::Code(
            Box::new(Attr {
                classes: vec!["c".to_owned().into()],
                ..Attr::default()
            }),
            "x".to_owned().into()
        )]
    );
}

#[test]
fn raw_attribute_off_leaves_marker_literal() {
    assert_eq!(p("`<b>`{=html}"), vec![code("<b>"), str("{=html}")]);
}

#[test]
fn code_span_matches_equal_length_closer() {
    assert_eq!(p("`a`"), vec![code("a")]);
}

#[test]
fn code_span_with_no_closer_stays_literal() {
    assert_eq!(p("`a"), vec![str("`a")]);
}

#[test]
fn code_span_failed_search_does_not_mask_a_different_length_match() {
    // The length-1 opener finds no lone-backtick closer and stays literal; the length-2 span
    // that comes right after must still match, since the close index is keyed by run length
    // and one length's absence does not suppress another's.
    assert_eq!(p("`a ``b``"), vec![str("`a"), Inline::Space, code("b")]);
}

#[test]
fn code_span_opener_is_a_run_suffix_stays_literal() {
    // An escape consumes the backslash plus the first backtick of a run, so the opener that
    // follows is the run's suffix — its length (2 here) need not equal any run's full length.
    // No length-2 run closes it, so both openers emit their backticks literally.
    assert_eq!(
        p("\\``` x \\``` x"),
        vec![
            str("```"),
            Inline::Space,
            str("x"),
            Inline::Space,
            str("```"),
            Inline::Space,
            str("x"),
        ]
    );
}

#[test]
fn code_span_distinct_run_lengths_all_resolve() {
    // Strictly increasing, distinct run lengths with no closers — the correctness face of the
    // adversarial quadratic input: every opener stays literal and the text between is intact.
    assert_eq!(
        p("`a ``b ```c"),
        vec![
            str("`a"),
            Inline::Space,
            str("``b"),
            Inline::Space,
            str("```c"),
        ]
    );
}

#[test]
fn code_span_close_before_cursor_is_not_reused() {
    // The second span's close search must start at its own opener, never returning the first
    // span's already-consumed closer that lies before the cursor.
    assert_eq!(p("`a` `b`"), vec![code("a"), Inline::Space, code("b")]);
}

#[test]
fn code_span_runs_at_buffer_ends_match() {
    // Opener at position 0 and closer as the final characters of the buffer.
    assert_eq!(p("``a``"), vec![code("a")]);
}

#[test]
fn code_span_index_matches_scan_on_tricky_buffers() {
    // Nested lengths, adjacent runs of different lengths, and a triple-adjacency opener where a
    // shorter inner run precedes the matching closer. Expected values encoded literally.
    assert_eq!(p("``x`y``"), vec![code("x`y")]);
    assert_eq!(p("`a ``b`` c`"), vec![code("a ``b`` c")]);
    assert_eq!(p("``a` b``"), vec![code("a` b")]);
}

// --- Inline raw TeX and backslash math ---

fn tex(source: &str) -> Inline {
    Inline::RawInline(
        carta_ast::Format("tex".to_owned().into()),
        source.to_owned().into(),
    )
}

fn raw_tex() -> Extensions {
    exts(&[Extension::RawTex])
}

fn single_math() -> Extensions {
    exts(&[Extension::TexMathSingleBackslash])
}

fn double_math() -> Extensions {
    exts(&[Extension::TexMathDoubleBackslash])
}

#[test]
fn raw_tex_commands_with_argument_groups() {
    // Consecutive `{…}` groups are all captured.
    assert_eq!(
        pe(r"\textbf{b}\emph{c}", raw_tex()),
        vec![tex(r"\textbf{b}"), tex(r"\emph{c}")]
    );
    // A leading optional `[…]` group precedes a `{…}` argument.
    assert_eq!(pe(r"\sqrt[3]{8}", raw_tex()), vec![tex(r"\sqrt[3]{8}")]);
    // Nested braces inside a group are balanced.
    assert_eq!(pe(r"\foo{a{b}c}", raw_tex()), vec![tex(r"\foo{a{b}c}")]);
}

#[test]
fn raw_tex_bare_command_absorbs_trailing_blanks() {
    // A command with no argument group swallows following spaces.
    assert_eq!(pe(r"\alpha y", raw_tex()), vec![tex(r"\alpha "), str("y")]);
    // A command followed by an argument group does not absorb the trailing space.
    assert_eq!(
        pe(r"\foo{a} y", raw_tex()),
        vec![tex(r"\foo{a}"), Inline::Space, str("y")]
    );
    // A command name carrying a digit does not absorb the trailing space.
    assert_eq!(
        pe(r"\foo1 y", raw_tex()),
        vec![tex(r"\foo1"), Inline::Space, str("y")]
    );
    // The first character must be a letter, so a digit after the backslash is not a command.
    assert_eq!(pe(r"\1foo", raw_tex()), vec![str(r"\1foo")]);
}

#[test]
fn raw_tex_unbalanced_brace_reverts_whole_command() {
    // An unclosed `{`-group reverts the entire command to literal text.
    assert_eq!(
        pe(r"\foo{a y", raw_tex()),
        vec![str(r"\foo{a"), Inline::Space, str("y")]
    );
    // An unclosed `[`-group merely stops the group run; the command stands.
    assert_eq!(
        pe(r"\foo[a y", raw_tex()),
        vec![tex(r"\foo"), str("[a"), Inline::Space, str("y")]
    );
}

#[test]
fn raw_tex_unclosed_openers_stay_literal_at_scale() {
    // A long run of never-closing openers must revert every one to literal text. The close-
    // delimiter fast-fail and the shared scan budget bound the look-ahead cost without changing
    // the parse, so the output is the same all-literal shape at any size.
    let cases = [
        r"\a{".repeat(4096), // no `}` anywhere: close-delimiter fast-fail
        format!("{}}}", r"\a{".repeat(4096)), // a single far `}` that never balances: budget backstop
        r"\begin{x}".repeat(4096),            // no `\end{x}`: environment-close fast-fail
    ];
    for input in cases {
        let parsed = pe(&input, raw_tex());
        assert!(
            parsed
                .iter()
                .all(|inline| matches!(inline, Inline::Str(_) | Inline::Space)),
            "expected only literal text",
        );
        let rendered: String = parsed
            .iter()
            .map(|inline| match inline {
                Inline::Str(text) => text.as_str().to_owned(),
                Inline::Space => " ".to_owned(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(rendered, input);
    }
}

#[test]
fn raw_tex_off_leaves_escape_behavior() {
    // Without the extension a command name is not raw TeX; `\t` is not punctuation so the
    // backslash stays literal.
    assert_eq!(p(r"\textbf{b}"), vec![str(r"\textbf{b}")]);
    // A backslash escape of punctuation still works regardless of the extension.
    assert_eq!(pe(r"\*", raw_tex()), vec![str("*")]);
}

#[test]
fn raw_tex_environment_captured_as_one_inline() {
    // A complete `\begin{ENV}`…`\end{ENV}` is one raw inline spanning the whole environment,
    // body and interior newlines included.
    assert_eq!(
        pe("\\begin{equation}\nx\n\\end{equation}", raw_tex()),
        vec![tex("\\begin{equation}\nx\n\\end{equation}")]
    );
    // The environment may sit on a single line amid surrounding text.
    assert_eq!(
        pe(r"a \begin{eq} z \end{eq} b", raw_tex()),
        vec![
            str("a"),
            Inline::Space,
            tex(r"\begin{eq} z \end{eq}"),
            Inline::Space,
            str("b"),
        ]
    );
    // A trailing `*` is part of the environment name, so the close must carry it too.
    assert_eq!(
        pe(r"\begin{equation*} x \end{equation*}", raw_tex()),
        vec![tex(r"\begin{equation*} x \end{equation*}")]
    );
}

#[test]
fn raw_tex_environment_balances_nested_begins() {
    // A nested environment of the same name deepens the nesting; the capture ends at the
    // matching outer close, not the first inner one.
    assert_eq!(
        pe(r"\begin{eq}\begin{eq}a\end{eq}\end{eq}", raw_tex()),
        vec![tex(r"\begin{eq}\begin{eq}a\end{eq}\end{eq}")]
    );
    // A nested environment of a different name is just part of the outer body.
    assert_eq!(
        pe(
            r"\begin{align}\begin{matrix}a\end{matrix}\end{align}",
            raw_tex()
        ),
        vec![tex(r"\begin{align}\begin{matrix}a\end{matrix}\end{align}")]
    );
}

#[test]
fn raw_tex_unmatched_environment_reverts_to_text() {
    // Without a matching close, `\begin{ENV}` is literal text, not a raw command.
    assert_eq!(
        pe("\\begin{equation}\nx", raw_tex()),
        vec![str(r"\begin{equation}"), Inline::SoftBreak, str("x")]
    );
    // A bare `\begin` with no `{ENV}` group is not raw TeX: the backslash precedes a letter,
    // so it stays literal and the word is plain text.
    assert_eq!(
        pe(r"\begin x", raw_tex()),
        vec![str(r"\begin"), Inline::Space, str("x")]
    );
    // A standalone `\end{ENV}` is literal text.
    assert_eq!(
        pe(r"\end{equation}", raw_tex()),
        vec![str(r"\end{equation}")]
    );
    // A mismatched close does not satisfy the opener; the whole span reverts to text.
    assert_eq!(
        pe(r"\begin{equation} x \end{align}", raw_tex()),
        vec![
            str(r"\begin{equation}"),
            Inline::Space,
            str("x"),
            Inline::Space,
            str(r"\end{align}"),
        ]
    );
}

#[test]
fn single_backslash_math() {
    assert_eq!(
        pe(r"\(x\) \[y\]", single_math()),
        vec![math_inline("x"), Inline::Space, math_display("y")]
    );
    // Inline content is trimmed; display content is verbatim.
    assert_eq!(pe(r"\( x \)", single_math()), vec![math_inline("x")]);
    assert_eq!(
        pe(r"\[ x = y \]", single_math()),
        vec![math_display(" x = y ")]
    );
}

#[test]
fn single_backslash_math_empty_and_unclosed_fall_back() {
    // Empty content is not a math span: `\(` and `\)` revert to escaped parentheses.
    assert_eq!(pe(r"\(\)", single_math()), vec![str("()")]);
    // No closer: the opener's backslash escapes the `(`.
    assert_eq!(pe(r"\(x", single_math()), vec![str("(x")]);
    // A span of only spaces is still a (trimmed-empty) span.
    assert_eq!(pe(r"\( \)", single_math()), vec![math_inline("")]);
}

#[test]
fn single_backslash_math_escapes_inside_content() {
    // An escaped delimiter inside the content does not close the span.
    assert_eq!(pe(r"\(a\\)b\)", single_math()), vec![math_inline(r"a\\)b")]);
}

#[test]
fn double_backslash_math() {
    assert_eq!(
        pe(r"\\(x\\) \\[y\\]", double_math()),
        vec![math_inline("x"), Inline::Space, math_display("y")]
    );
}

#[test]
fn backslash_math_off_leaves_escape_behavior() {
    // Without the extension `\(` is a plain escaped parenthesis.
    assert_eq!(p(r"\(x\)"), vec![str("(x)")]);
}

// --- Native spans (`<span …>` … `</span>`) ---

fn native() -> Extensions {
    exts(&[Extension::NativeSpans])
}

#[test]
fn native_span_carries_id_class_and_pairs() {
    assert_eq!(
        pe(
            r#"<span id="i" class="a b" data-x="y">hi *there*</span>"#,
            native()
        ),
        vec![span(
            attr("i", &["a", "b"], &[("data-x", "y")]),
            vec![str("hi"), Inline::Space, Inline::Emph(vec![str("there")])]
        )]
    );
}

#[test]
fn native_span_without_attributes() {
    assert_eq!(
        pe("a <span>x</span> b", native()),
        vec![
            str("a"),
            Inline::Space,
            span(attr("", &[], &[]), vec![str("x")]),
            Inline::Space,
            str("b"),
        ]
    );
}

#[test]
fn native_span_empty_content() {
    assert_eq!(
        pe("<span></span>", native()),
        vec![span(attr("", &[], &[]), vec![])]
    );
}

#[test]
fn native_span_nests_innermost_first() {
    assert_eq!(
        pe(
            r#"<span class="o"><span class="i">x</span></span>"#,
            native()
        ),
        vec![span(
            attr("", &["o"], &[]),
            vec![span(attr("", &["i"], &[]), vec![str("x")])]
        )]
    );
}

#[test]
fn native_span_tag_name_is_case_insensitive() {
    assert_eq!(
        pe(r#"<SPAN class="a">x</SPAN>"#, native()),
        vec![span(attr("", &["a"], &[]), vec![str("x")])]
    );
}

#[test]
fn native_span_keeps_non_span_tags_raw() {
    // An unrelated tag inside a span stays raw inline HTML.
    assert_eq!(
        pe(r#"<span class="a">x <b>y</b></span>"#, native()),
        vec![span(
            attr("", &["a"], &[]),
            vec![
                str("x"),
                Inline::Space,
                raw("html", "<b>"),
                str("y"),
                raw("html", "</b>"),
            ]
        )]
    );
}

#[test]
fn native_span_attribute_values_and_booleans() {
    // Single-quoted, unquoted, and valueless attributes; a duplicate id/class keeps the first.
    assert_eq!(
        pe("<span data-x='y z'>q</span>", native()),
        vec![span(attr("", &[], &[("data-x", "y z")]), vec![str("q")])]
    );
    assert_eq!(
        pe("<span flag>q</span>", native()),
        vec![span(attr("", &[], &[("flag", "")]), vec![str("q")])]
    );
    assert_eq!(
        pe(
            r#"<span id="a" id="b" class="c" class="d">q</span>"#,
            native()
        ),
        vec![span(attr("a", &["c"], &[]), vec![str("q")])]
    );
}

#[test]
fn native_span_decodes_entities_in_attribute_values() {
    assert_eq!(
        pe(r#"<span title="a &amp; b">q</span>"#, native()),
        vec![span(attr("", &[], &[("title", "a & b")]), vec![str("q")])]
    );
}

#[test]
fn native_span_self_closing_stays_raw() {
    // `<span/>` has no content to wrap.
    assert_eq!(
        pe("a <span/> b", native()),
        vec![
            str("a"),
            Inline::Space,
            raw("html", "<span/>"),
            Inline::Space,
            str("b"),
        ]
    );
}

#[test]
fn native_span_unclosed_opener_reverts_to_raw() {
    assert_eq!(
        pe(r#"<span class="a">no close"#, native()),
        vec![
            raw("html", "<span class=\"a\">"),
            str("no"),
            Inline::Space,
            str("close"),
        ]
    );
}

#[test]
fn native_span_pairs_inside_emphasis() {
    assert_eq!(
        pe("*x <span>y</span> z*", native()),
        vec![Inline::Emph(vec![
            str("x"),
            Inline::Space,
            span(attr("", &[], &[]), vec![str("y")]),
            Inline::Space,
            str("z"),
        ])]
    );
}

#[test]
fn native_span_off_leaves_tags_raw() {
    assert_eq!(
        p(r#"<span class="a">x</span>"#),
        vec![
            raw("html", "<span class=\"a\">"),
            str("x"),
            raw("html", "</span>"),
        ]
    );
}

// --- Mark (highlight) ---

fn mark(content: Vec<Inline>) -> Inline {
    span(attr("", &["mark"], &[]), content)
}

#[test]
fn mark_wraps_a_double_equals_run() {
    let on = exts(&[Extension::Mark]);
    assert_eq!(
        pe("a ==x== b", on),
        vec![
            str("a"),
            Inline::Space,
            mark(vec![str("x")]),
            Inline::Space,
            str("b"),
        ]
    );
}

#[test]
fn mark_resolves_inner_emphasis() {
    let on = exts(&[Extension::Mark]);
    assert_eq!(
        pe("==x *y*==", on),
        vec![mark(vec![
            str("x"),
            Inline::Space,
            Inline::Emph(vec![str("y")]),
        ])]
    );
}

#[test]
fn mark_off_leaves_double_equals_literal() {
    // Without the extension the run is plain text.
    assert_eq!(
        pe("a ==x== b", no_ext()),
        vec![
            str("a"),
            Inline::Space,
            str("==x=="),
            Inline::Space,
            str("b"),
        ]
    );
}

#[test]
fn mark_opener_needs_no_following_space() {
    let on = exts(&[Extension::Mark]);
    // A space just inside either delimiter blocks the run; both sides stay literal.
    assert_eq!(pe("== x==", on), vec![str("=="), Inline::Space, str("x==")]);
    assert_eq!(pe("==x ==", on), vec![str("==x"), Inline::Space, str("==")]);
}

#[test]
fn mark_lone_equals_stays_literal() {
    let on = exts(&[Extension::Mark]);
    assert_eq!(
        pe("a = b", on),
        vec![str("a"), Inline::Space, str("="), Inline::Space, str("b")]
    );
}

#[test]
fn mark_run_pairs_once_and_leaves_excess_literal() {
    let on = exts(&[Extension::Mark]);
    // Four-on-four pairs only the innermost two from each side; the outer `==` stay literal and
    // do not re-pair into a nested mark.
    assert_eq!(
        pe("====x====", on),
        vec![str("=="), mark(vec![str("x")]), str("==")]
    );
    // Two-on-four consumes two from each, leaving the surplus `==` literal.
    assert_eq!(pe("==x====", on), vec![mark(vec![str("x")]), str("==")]);
}

// --- Citations ---

fn cites() -> Extensions {
    exts(&[Extension::Citations])
}

fn cite(citations: Vec<Citation>, fallback: Vec<Inline>) -> Inline {
    Inline::Cite(citations, fallback)
}

fn citation(
    id: &str,
    prefix: Vec<Inline>,
    suffix: Vec<Inline>,
    mode: CitationMode,
    note_num: i32,
) -> Citation {
    Citation {
        id: id.to_owned().into(),
        prefix,
        suffix,
        mode,
        note_num,
        hash: 0,
    }
}

#[test]
fn bare_citation_is_author_in_text() {
    assert_eq!(
        pe("@doe2020", cites()),
        vec![cite(
            vec![citation(
                "doe2020",
                vec![],
                vec![],
                CitationMode::AuthorInText,
                1
            )],
            vec![str("@doe2020")],
        )]
    );
}

#[test]
fn bare_citation_needs_a_non_word_before_the_at() {
    // Glued to a preceding word, the `@` is literal — no citation, no email autolink here.
    assert_eq!(pe("foo@bar", cites()), vec![str("foo@bar")]);
    // A space before the `@` lets it open a citation.
    assert_eq!(
        pe("a @b", cites()),
        vec![
            str("a"),
            Inline::Space,
            cite(
                vec![citation("b", vec![], vec![], CitationMode::AuthorInText, 1)],
                vec![str("@b")],
            ),
        ]
    );
}

#[test]
fn bracket_citation_carries_prefix_and_suffix() {
    assert_eq!(
        pe("[see @doe2020 and more]", cites()),
        vec![cite(
            vec![citation(
                "doe2020",
                vec![str("see")],
                vec![Inline::Space, str("and"), Inline::Space, str("more")],
                CitationMode::NormalCitation,
                1,
            )],
            vec![
                str("[see"),
                Inline::Space,
                str("@doe2020"),
                Inline::Space,
                str("and"),
                Inline::Space,
                str("more]"),
            ],
        )]
    );
}

#[test]
fn dash_before_at_suppresses_author() {
    assert_eq!(
        pe("[-@k]", cites()),
        vec![cite(
            vec![citation(
                "k",
                vec![],
                vec![],
                CitationMode::SuppressAuthor,
                1
            )],
            vec![str("[-@k]")],
        )]
    );
    // A `-` glued to a preceding word is part of the prefix, not a suppression marker.
    assert_eq!(
        pe("[a-@b]", cites()),
        vec![cite(
            vec![citation(
                "b",
                vec![str("a-")],
                vec![],
                CitationMode::NormalCitation,
                1
            )],
            vec![str("[a-@b]")],
        )]
    );
}

#[test]
fn semicolon_separates_entries_sharing_one_number() {
    assert_eq!(
        pe("[@a; @b]", cites()),
        vec![cite(
            vec![
                citation("a", vec![], vec![], CitationMode::NormalCitation, 1),
                citation("b", vec![], vec![], CitationMode::NormalCitation, 1),
            ],
            vec![str("[@a;"), Inline::Space, str("@b]")],
        )]
    );
}

#[test]
fn comma_nests_a_bare_citation_in_the_suffix() {
    // `@b` after a comma is not a new entry; it becomes a bare citation inside `a`'s suffix, and
    // the enclosing group takes the higher number.
    assert_eq!(
        pe("[@a, @b]", cites()),
        vec![cite(
            vec![citation(
                "a",
                vec![],
                vec![
                    str(","),
                    Inline::Space,
                    cite(
                        vec![citation("b", vec![], vec![], CitationMode::AuthorInText, 2)],
                        vec![str("@b")],
                    ),
                ],
                CitationMode::NormalCitation,
                2,
            )],
            vec![str("[@a,"), Inline::Space, str("@b]")],
        )]
    );
}

#[test]
fn document_order_numbers_each_group() {
    // Two separate groups in one block take consecutive numbers.
    let out = pe("@a and [@b]", cites());
    let nums: Vec<i32> = out
        .iter()
        .filter_map(|inline| match inline {
            Inline::Cite(citations, _) => citations.first().map(|c| c.note_num),
            _ => None,
        })
        .collect();
    assert_eq!(nums, vec![1, 2]);
}

#[test]
fn malformed_bracket_falls_back_to_inline_citations() {
    // A trailing empty segment is not a citation list; the brackets stay literal and the bare
    // `@a` inside becomes an author-in-text citation.
    assert_eq!(
        pe("[@a;]", cites()),
        vec![
            str("["),
            cite(
                vec![citation("a", vec![], vec![], CitationMode::AuthorInText, 1)],
                vec![str("@a")],
            ),
            str(";]"),
        ]
    );
}

#[test]
fn segment_without_a_key_is_not_a_citation_list() {
    // The first segment holds no `@`, so the whole bracket is not a citation; only the bare `@b`
    // citation survives.
    assert_eq!(
        pe("[no key; @b]", cites()),
        vec![
            str("[no"),
            Inline::Space,
            str("key;"),
            Inline::Space,
            cite(
                vec![citation("b", vec![], vec![], CitationMode::AuthorInText, 1)],
                vec![str("@b")],
            ),
            str("]"),
        ]
    );
}

#[test]
fn key_charset_keeps_internal_punctuation() {
    // Internal `_ : - . /` belong to a key only when more key characters follow.
    assert_eq!(
        pe("[@foo_bar:baz-qux.v/1]", cites()),
        vec![cite(
            vec![citation(
                "foo_bar:baz-qux.v/1",
                vec![],
                vec![],
                CitationMode::NormalCitation,
                1,
            )],
            vec![str("[@foo_bar:baz-qux.v/1]")],
        )]
    );
    // A trailing `-` is not part of the key; it falls to the suffix.
    assert_eq!(
        pe("[@a-]", cites()),
        vec![cite(
            vec![citation(
                "a",
                vec![],
                vec![str("-")],
                CitationMode::NormalCitation,
                1
            )],
            vec![str("[@a-]")],
        )]
    );
}

#[test]
fn citations_off_leaves_the_syntax_literal() {
    assert_eq!(
        pe("See [@a] and @b.", no_ext()),
        vec![
            str("See"),
            Inline::Space,
            str("[@a]"),
            Inline::Space,
            str("and"),
            Inline::Space,
            str("@b."),
        ]
    );
}

#[test]
fn escaped_at_is_not_a_citation() {
    assert_eq!(pe(r"[\@a]", cites()), vec![str("[@a]")]);
}

#[test]
fn citation_does_not_steal_a_link() {
    // An explicit link target wins; the key inside becomes a bare citation in the link text.
    assert_eq!(
        pe("[@a](http://x.com)", cites()),
        vec![link(
            vec![cite(
                vec![citation("a", vec![], vec![], CitationMode::AuthorInText, 1)],
                vec![str("@a")],
            )],
            "http://x.com",
        )]
    );
}

#[test]
fn heading_content_is_context_independent_gates_on_ref_trigger_chars() {
    assert!(heading_content_is_context_independent("Installation"));
    assert!(heading_content_is_context_independent("API reference"));
    assert!(!heading_content_is_context_independent("About @doe99"));
    assert!(!heading_content_is_context_independent("Title[^1]"));
    assert!(!heading_content_is_context_independent("See [spec]"));
}

#[test]
fn a_context_independent_heading_is_parsed_once_and_reused_by_the_body_pass() {
    let ir = vec![IrBlock::Heading(1, "Installation".to_owned())];
    let mut refs = empty_refs();
    let mut cache: HeaderParseCache = BTreeMap::new();
    let mut numbering = HeaderNumbering::new(no_ext(), false);
    gather_headers(
        &ir,
        &mut refs,
        no_notes(),
        no_ext(),
        &mut numbering,
        &mut cache,
    );

    let queued = cache
        .get("Installation")
        .expect("the pre-pass should have cached the heading's parse");
    assert_eq!(queued.len(), 1);

    let heading = ir.first().expect("one heading in the IR");
    let mut out = Vec::new();
    resolve_block(heading, &refs, no_notes(), no_ext(), &mut cache, &mut out);

    // The body pass popped the pre-pass's parse instead of running the inline scan again.
    assert!(cache.get("Installation").is_none_or(VecDeque::is_empty));
    assert_eq!(
        out,
        vec![Block::Header(1, Box::default(), p("Installation"))]
    );
}

#[test]
fn a_second_identical_heading_pops_its_own_queued_parse() {
    let ir = vec![
        IrBlock::Heading(1, "Dup".to_owned()),
        IrBlock::Heading(1, "Dup".to_owned()),
    ];
    let mut refs = empty_refs();
    let mut cache: HeaderParseCache = BTreeMap::new();
    let mut numbering = HeaderNumbering::new(no_ext(), false);
    gather_headers(
        &ir,
        &mut refs,
        no_notes(),
        no_ext(),
        &mut numbering,
        &mut cache,
    );
    assert_eq!(cache.get("Dup").map(VecDeque::len), Some(2));

    let mut out = Vec::new();
    for block in &ir {
        resolve_block(block, &refs, no_notes(), no_ext(), &mut cache, &mut out);
    }

    assert!(cache.get("Dup").is_none_or(VecDeque::is_empty));
    assert_eq!(
        out,
        vec![
            Block::Header(1, Box::default(), p("Dup")),
            Block::Header(1, Box::default(), p("Dup")),
        ]
    );
}
