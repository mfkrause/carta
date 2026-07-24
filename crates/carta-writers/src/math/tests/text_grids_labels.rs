//! Text ligatures, grids, matrices, and equation labels.

use super::super::{to_inlines, to_typst};
use super::{emph, str_inline, typst_labeled, var};
use carta_ast::Inline;

#[test]
fn bare_dollar_in_math_falls_back_to_verbatim() {
    // a bare $ is a mode delimiter, untranslatable; the escaped \$ is the dollar glyph
    assert_eq!(to_inlines("a$b"), None);
    assert_eq!(to_inlines("$"), None);
    assert_eq!(to_typst("a$b"), None);
    assert_eq!(to_typst("x+$"), None);
    assert_eq!(
        to_inlines("a\\$b"),
        Some(vec![var("a"), str_inline("$"), var("b")])
    );
    assert_eq!(to_typst("\\$").as_deref(), Some("\\$"));
}

#[test]
fn quote_or_backtick_inside_a_text_wrapper_stays_literal() {
    // inside a text wrapper a quote is literal text, escaped for the Typst string
    assert_eq!(to_inlines("\\text{a\"b}"), Some(vec![str_inline("a\"b")]));
    assert_eq!(
        to_typst("\\text{a\"b}").as_deref(),
        Some("upright(\"a\\\"b\")")
    );
    // the rejection is scoped to a bare math-mode quote only
    assert!(to_inlines("a\"b").is_none() && to_inlines("\\text{a\"b}").is_some());
}

#[test]
fn text_apostrophe_becomes_a_right_single_quote() {
    assert_eq!(
        to_inlines("\\text{don't}"),
        Some(vec![str_inline("don\u{2019}t")])
    );
}

#[test]
fn text_paired_quotes_become_curly_quotes() {
    // A doubled backtick opens, a doubled apostrophe closes.
    assert_eq!(
        to_inlines("\\text{``hello''}"),
        Some(vec![str_inline("\u{201C}hello\u{201D}")])
    );
}

#[test]
fn text_lone_quote_run_pairs_then_keeps_a_single() {
    // Three backticks are one open-double plus one open-single quote.
    assert_eq!(
        to_inlines("\\text{```x}"),
        Some(vec![str_inline("\u{201C}\u{2018}x")])
    );
}

#[test]
fn text_dash_runs_match_greedily() {
    // `--` is an en dash, `---` an em dash; longer runs take em dashes first, then en, then hyphen.
    assert_eq!(
        to_inlines("\\text{a--b}"),
        Some(vec![str_inline("a\u{2013}b")])
    );
    assert_eq!(
        to_inlines("\\text{a---b}"),
        Some(vec![str_inline("a\u{2014}b")])
    );
    assert_eq!(
        to_inlines("\\text{----}"),
        Some(vec![str_inline("\u{2014}-")])
    );
    assert_eq!(
        to_inlines("\\text{-----}"),
        Some(vec![str_inline("\u{2014}\u{2013}")])
    );
}

#[test]
fn text_ldots_becomes_an_ellipsis() {
    assert_eq!(
        to_inlines("\\text{\\ldots}"),
        Some(vec![str_inline("\u{2026}")])
    );
}

#[test]
fn ligatures_apply_in_every_text_wrapper() {
    for wrapper in [
        "text", "textrm", "textbf", "textit", "texttt", "textsf", "mbox",
    ] {
        let src = format!("\\{wrapper}{{don't}}");
        let inlines = to_inlines(&src).unwrap_or_default();
        let rendered: String = inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Str(s) | Inline::Code(_, s) => Some(s.clone()),
                Inline::Strong(inner) | Inline::Emph(inner) => inner.iter().find_map(|i| match i {
                    Inline::Str(s) => Some(s.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();
        assert!(
            rendered.contains('\u{2019}'),
            "{wrapper} should curl the apostrophe"
        );
    }
}

#[test]
fn ligatures_do_not_corrupt_a_run_with_no_trigger() {
    assert_eq!(to_inlines("\\text{a_b}"), Some(vec![str_inline("a_b")]));
    assert_eq!(to_inlines("\\text{ab cd}"), Some(vec![str_inline("ab cd")]));
}

#[test]
fn straight_double_quote_is_not_a_ligature_trigger_in_text() {
    assert_eq!(to_inlines("\\text{a\"b}"), Some(vec![str_inline("a\"b")]));
}

#[test]
fn operatorname_argument_is_not_dash_ligatured() {
    // the argument is upright math, not text: a double dash stays two hyphens
    assert_eq!(
        to_typst("\\operatorname{a--b}").as_deref(),
        Some("\"a--b\"")
    );
}

#[test]
fn not_over_a_letter_strikes_the_italic_glyph() {
    assert_eq!(to_inlines("\\not a"), Some(vec![var("a\u{0338}")]));
    assert_eq!(to_inlines("\\not P"), Some(vec![var("P\u{0338}")]));
}

#[test]
fn not_over_greek_strikes_the_italic_glyph() {
    assert_eq!(
        to_inlines("\\not\\alpha"),
        Some(vec![var("\u{3B1}\u{0338}")])
    );
    assert_eq!(
        to_inlines("\\not\\Gamma"),
        Some(vec![var("\u{393}\u{0338}")])
    );
}

#[test]
fn not_over_a_digit_strikes_the_upright_glyph() {
    // A digit keeps its upright form, so the struck glyph is a bare string, not an italic.
    assert_eq!(to_inlines("\\not 1"), Some(vec![str_inline("1\u{0338}")]));
}

#[test]
fn not_over_an_open_delimiter_strikes_the_italic_glyph() {
    assert_eq!(to_inlines("\\not("), Some(vec![var("(\u{0338}")]));
}

#[test]
fn not_over_an_operator_falls_back() {
    // A binary or large operator base has no struck form and falls back to verbatim.
    assert_eq!(to_inlines("\\not+"), None);
    assert_eq!(to_inlines("\\not\\sum"), None);
    assert_eq!(to_inlines("\\not -"), None);
}

#[test]
fn not_over_a_non_relation_strikes_the_glyph_in_typst() {
    // A letter, Greek, or delimiter base strikes its quoted glyph; a digit strikes a bare glyph.
    assert_eq!(to_typst("\\not a").as_deref(), Some("\"a\u{0338}\""));
    assert_eq!(
        to_typst("\\not\\alpha").as_deref(),
        Some("\"\u{3B1}\u{0338}\"")
    );
    assert_eq!(to_typst("\\not(").as_deref(), Some("\"(\u{0338}\""));
    assert_eq!(to_typst("\\not 1").as_deref(), Some("1\u{0338}"));
}

#[test]
fn not_over_a_braced_group_overlays_an_accent_in_typst() {
    // a braced base carries the combining solidus through Typst's generic accent form
    assert_eq!(to_typst("\\not{a}").as_deref(), Some("accent(a, \u{0338})"));
    assert_eq!(
        to_typst("\\not{ab}").as_deref(),
        Some("accent(a b, \u{0338})")
    );
    assert_eq!(to_typst("\\not{=}").as_deref(), Some("accent(=, \u{0338})"));
    assert_eq!(
        to_typst("\\not{\\alpha}").as_deref(),
        Some("accent(alpha, \u{0338})")
    );
    // An empty braced group keeps the accent with empty content.
    assert_eq!(to_typst("\\not{}").as_deref(), Some("accent(, \u{0338})"));
}

#[test]
fn not_over_a_braced_group_falls_back_in_the_linear_backend() {
    // the linear inline backend has no overlay for a struck group
    assert_eq!(to_inlines("\\not{a}"), None);
    assert_eq!(to_inlines("\\not{\\alpha}"), None);
    assert_eq!(to_inlines("\\not{}"), None);
}

#[test]
fn alignat_lowers_to_an_alignment_grid() {
    // The mandatory column-count argument is consumed; the cells become a `&`-separated grid.
    assert_eq!(
        to_typst("\\begin{alignat}{2}a&=b&c&=d\\end{alignat}").as_deref(),
        Some("a & = b & c & = d")
    );
    // The inline backend has no grid form, so the same environment falls back to verbatim there.
    assert_eq!(to_inlines("\\begin{alignat}{2}a&=b\\end{alignat}"), None);
}

#[test]
fn overset_with_a_dotted_mark_needs_no_parentheses() {
    // A mark lowering to a single dotted Typst token attaches bare.
    assert_eq!(
        to_typst("\\overset{\\sim}{n}").as_deref(),
        Some("n^tilde.op")
    );
    assert_eq!(to_typst("\\overset{\\to}{n}").as_deref(), Some("n^arrow.r"));
    // A genuinely compound mark stays parenthesised.
    assert_eq!(to_typst("\\overset{ab}{n}").as_deref(), Some("n^(a b)"));
}

#[test]
fn nested_grid_inside_a_transparent_wrapper_flattens() {
    assert_eq!(
        to_typst("\\begin{equation}\\begin{aligned}a&=b\\\\c&=d\\end{aligned}\\end{equation}")
            .as_deref(),
        Some("a & = b\\\nc & = d")
    );
    assert_eq!(
        to_typst("\\begin{gather}\\begin{aligned}a&=b\\end{aligned}\\end{gather}").as_deref(),
        Some("a & = b")
    );
}

#[test]
fn single_column_cases_lowers_to_a_bare_brace() {
    assert_eq!(
        to_typst("\\begin{cases}a\\\\b\\end{cases}").as_deref(),
        Some("{a\\\nb")
    );
    // A multi-cell row keeps the `cases(..)` function form.
    assert_eq!(
        to_typst("\\begin{cases}a & x\\\\b & y\\end{cases}").as_deref(),
        Some("cases(delim: \"{\", a & x, b & y)")
    );
}

#[test]
fn break_dimension_argument_is_dropped() {
    assert_eq!(
        to_typst("\\begin{gather}a \\\\[5pt] b\\end{gather}").as_deref(),
        Some("a\\\nb")
    );
    assert_eq!(
        to_typst("\\begin{aligned}a&=b\\\\[1em]c&=d\\end{aligned}").as_deref(),
        Some("a & = b\\\nc & = d")
    );
}

#[test]
fn a_space_before_a_bracket_keeps_it_as_content() {
    // The optional break dimension binds only with no space; a `[` after a space is ordinary content.
    assert_eq!(
        to_typst("\\begin{aligned}a&=b\\\\ [x]\\end{aligned}").as_deref(),
        Some("a & = b\\\n\\[x\\]")
    );
}

#[test]
fn bare_bar_makes_a_following_sign_unary() {
    // a bare | opens a group, so the following - is unary: no space, coalesced
    assert_eq!(
        to_inlines("|-x|"),
        Some(vec![str_inline("|\u{2212}"), var("x"), str_inline("|")])
    );
}

#[test]
fn bare_bar_does_not_regress_a_plain_absolute_value() {
    assert_eq!(
        to_inlines("|x|"),
        Some(vec![str_inline("|"), var("x"), str_inline("|")])
    );
}

#[test]
fn trailing_binary_mod_falls_back() {
    // `\bmod` is infix and needs a following operand; with none it is invalid.
    assert_eq!(to_inlines("a \\bmod"), None);
    assert_eq!(to_typst("a \\bmod").as_deref(), None);
    assert_eq!(to_inlines("a^2 \\bmod"), None);
    // A `\bmod` at the close of its group likewise has no operand.
    assert_eq!(to_typst("{a \\bmod}").as_deref(), None);
}

#[test]
fn binary_mod_with_an_operand_still_renders() {
    assert!(to_inlines("a \\bmod b").is_some());
}

#[test]
fn dotminus_and_eqsim_render() {
    assert_eq!(to_inlines("\\dotminus"), Some(vec![str_inline("\u{2238}")]));
    assert_eq!(to_inlines("\\eqsim"), Some(vec![str_inline("\u{2242}")]));
    assert_eq!(to_typst("\\dotminus").as_deref(), Some("minus.dot"));
    assert_eq!(to_typst("\\eqsim").as_deref(), Some("minus.tilde"));
}

#[test]
fn swept_symbols_render() {
    assert_eq!(to_inlines("\\nsimeq"), Some(vec![str_inline("\u{2244}")]));
    assert_eq!(to_inlines("\\simneqq"), Some(vec![str_inline("\u{2246}")]));
    assert_eq!(to_inlines("\\nlessgtr"), Some(vec![str_inline("\u{2278}")]));
    assert_eq!(
        to_inlines("\\dashcolon"),
        Some(vec![str_inline("\u{2239}")])
    );
    assert_eq!(to_inlines("\\obar"), Some(vec![str_inline("\u{233D}")]));
    assert_eq!(
        to_inlines("\\ogreaterthan"),
        Some(vec![str_inline("\u{29C1}")])
    );
    assert_eq!(
        to_inlines("\\olessthan"),
        Some(vec![str_inline("\u{29C0}")])
    );
    assert_eq!(to_typst("\\nsimeq").as_deref(), Some("tilde.eq.not"));
    assert_eq!(to_typst("\\ogreaterthan").as_deref(), Some("gt.o"));
}

#[test]
fn n_prefixed_curly_precedes_succeeds_render() {
    // shares the negated-precedes/succeeds glyphs (U+22E0/U+22E1) with the non-curly siblings
    assert_eq!(
        to_inlines("\\npreccurlyeq"),
        Some(vec![str_inline("\u{22E0}")])
    );
    assert_eq!(
        to_inlines("\\nsucccurlyeq"),
        Some(vec![str_inline("\u{22E1}")])
    );
    assert_eq!(
        to_typst("\\npreccurlyeq").as_deref(),
        Some("prec.curly.eq.not")
    );
    assert_eq!(
        to_typst("\\nsucccurlyeq").as_deref(),
        Some("succ.curly.eq.not")
    );
}

#[test]
fn not_over_slanted_and_curly_relations_negate() {
    // slanted forms reuse the plain negated glyphs; curly forms the negated precede/succeed
    assert_eq!(
        to_inlines("\\not\\leqslant"),
        Some(vec![str_inline("\u{2270}")])
    );
    assert_eq!(
        to_inlines("\\not\\geqslant"),
        Some(vec![str_inline("\u{2271}")])
    );
    assert_eq!(
        to_inlines("\\not\\preccurlyeq"),
        Some(vec![str_inline("\u{22E0}")])
    );
    assert_eq!(
        to_inlines("\\not\\succcurlyeq"),
        Some(vec![str_inline("\u{22E1}")])
    );
}

#[test]
fn not_over_similarity_relations_strikes_with_a_solidus() {
    // no precomposed form: base glyph carries the combining long solidus (U+0338)
    assert_eq!(
        to_inlines("\\not\\lesssim"),
        Some(vec![str_inline("\u{2274}")])
    );
    assert_eq!(
        to_inlines("\\not\\gtrsim"),
        Some(vec![str_inline("\u{2275}")])
    );
    assert_eq!(to_typst("\\not\\lesssim").as_deref(), Some("lt.tilde.not"));
    assert_eq!(to_typst("\\not\\gtrsim").as_deref(), Some("gt.tilde.not"));
}

#[test]
fn text_accent_composes_a_precomposed_letter() {
    // A text-mode accent over a base composes to the precomposed Latin letter where one exists.
    assert_eq!(
        to_inlines("\\text{\\'a}"),
        Some(vec![str_inline("\u{00E1}")])
    );
    assert_eq!(
        to_inlines("\\text{\\\"o}"),
        Some(vec![str_inline("\u{00F6}")])
    );
    assert_eq!(
        to_inlines("\\text{\\~n}"),
        Some(vec![str_inline("\u{00F1}")])
    );
}

#[test]
fn text_accent_without_a_precomposed_letter_keeps_the_bare_base() {
    // An accent over a base with no precomposed form drops to the bare base letter.
    assert_eq!(to_inlines("\\text{\\'x}"), Some(vec![str_inline("x")]));
}

#[test]
fn text_accent_over_a_dotless_letter_keeps_the_control_word_source() {
    // \i and \j have no composed form: accent drops, control word kept as literal source
    assert_eq!(to_inlines("\\text{\\\"\\i}"), Some(vec![str_inline("\\i")]));
    assert_eq!(to_inlines("\\text{\\^\\j}"), Some(vec![str_inline("\\j")]));
    assert_eq!(
        to_inlines("\\text{na\\\"\\i ve}"),
        Some(vec![str_inline("na\\i ve")])
    );
    assert_eq!(
        to_typst("\\text{\\\"\\i}").as_deref(),
        Some("upright(\"\\\\i\")")
    );
}

#[test]
fn text_cedilla_keeps_a_combining_mark_over_o() {
    // The cedilla over `o` has no precomposed letter, so the base carries the combining cedilla.
    assert_eq!(
        to_inlines("\\text{\\c o}"),
        Some(vec![str_inline("o\u{0327}")])
    );
}

#[test]
fn text_foreign_letter_resolves() {
    assert_eq!(
        to_inlines("\\text{\\o}"),
        Some(vec![str_inline("\u{00F8}")])
    );
    assert_eq!(
        to_inlines("\\text{\\ae}"),
        Some(vec![str_inline("\u{00E6}")])
    );
    assert_eq!(
        to_inlines("\\text{\\ss}"),
        Some(vec![str_inline("\u{00DF}")])
    );
}

#[test]
fn text_symbol_resolves() {
    assert_eq!(
        to_inlines("\\text{\\textbackslash}"),
        Some(vec![str_inline("\\")])
    );
}

#[test]
fn text_quad_is_an_en_quad() {
    // A text-mode `\quad` folds into the run as a single en quad (U+2000).
    assert_eq!(
        to_inlines("\\text{a\\quad b}"),
        Some(vec![str_inline("a\u{2000}b")])
    );
}

#[test]
fn text_accent_resolves_under_other_text_wrappers() {
    // The text-command resolver runs under every text wrapper, not just `\text`.
    assert_eq!(
        to_inlines("\\textbf{\\'a}"),
        Some(vec![Inline::Strong(vec![str_inline("\u{00E1}")])])
    );
    assert_eq!(
        to_inlines("\\textit{\\o}"),
        Some(vec![emph(vec![str_inline("\u{00F8}")])])
    );
}

#[test]
fn text_nested_group_is_transparent() {
    // a nested group joins the run inline; Typst keeps per-group upright segments
    assert_eq!(to_inlines("\\text{a{b}c}"), Some(vec![str_inline("abc")]));
    assert_eq!(
        to_typst("\\text{a{b}c}").as_deref(),
        Some("upright(\"a\") upright(\"b\") upright(\"c\")")
    );
}

#[test]
fn operatorname_does_not_resolve_text_commands() {
    // math mode: a text accent is not resolved, verbatim fallback
    assert_eq!(to_inlines("\\operatorname{\\'a}"), None);
}

#[test]
fn trailing_control_space_falls_back_for_inlines() {
    // no following operand: inline falls back; Typst still resolves a medium space
    assert_eq!(to_inlines("a \\ "), None);
    assert!(to_typst("a \\ ").is_some());
}

#[test]
fn control_space_before_an_operand_is_kept() {
    // A control space that precedes further content keeps its space and lowers normally.
    assert!(to_inlines("a\\ b").is_some());
}

#[test]
fn doteq_and_dingbats_render() {
    assert_eq!(to_inlines("\\Doteq"), Some(vec![str_inline("\u{2251}")]));
    assert_eq!(to_inlines("\\smiley"), Some(vec![str_inline("\u{263A}")]));
    assert_eq!(to_inlines("\\female"), Some(vec![str_inline("\u{2640}")]));
    assert_eq!(to_inlines("\\male"), Some(vec![str_inline("\u{2642}")]));
    assert_eq!(
        to_inlines("\\eighthnote"),
        Some(vec![str_inline("\u{266A}")])
    );
    assert_eq!(to_typst("\\Doteq").as_deref(), Some("eq.dots"));
    assert_eq!(to_typst("\\female").as_deref(), Some("venus"));
    assert_eq!(to_typst("\\eighthnote").as_deref(), Some("note.eighth.alt"));
}

#[test]
fn starred_matrix_renders_as_its_unstarred_form() {
    assert_eq!(
        to_typst("\\begin{pmatrix*} a \\\\ b \\end{pmatrix*}").as_deref(),
        Some("vec(a, b)")
    );
    assert_eq!(
        to_typst("\\begin{matrix*} a \\\\ b \\end{matrix*}").as_deref(),
        Some("a\\\nb")
    );
    assert_eq!(
        to_typst("\\begin{Bmatrix*} a \\\\ b \\end{Bmatrix*}").as_deref(),
        Some("mat(delim: \"{\", a; b)")
    );
}

#[test]
fn starred_matrix_keeps_the_align_argument_as_literal_leading_content() {
    // the presentational [align] argument becomes literal leading content of the first cell
    assert_eq!(
        to_typst("\\begin{pmatrix*}[r] a \\\\ b \\end{pmatrix*}").as_deref(),
        Some("vec(\\[r\\]a, b)")
    );
    assert_eq!(
        to_typst("\\begin{bmatrix*}[c] a & b \\\\ c & d \\end{bmatrix*}").as_deref(),
        Some("mat(delim: \"[\", \\[c\\]a, b; c, d)")
    );
}

#[test]
fn starred_cases_renders_as_cases() {
    assert_eq!(
        to_typst("\\begin{cases*} a & x \\\\ b & y \\end{cases*}").as_deref(),
        Some("cases(delim: \"{\", a & x, b & y)")
    );
}

#[test]
fn left_right_around_a_bare_matrix_fuses_into_mat() {
    assert_eq!(
        to_typst("\\left( \\begin{matrix} a & b \\end{matrix} \\right)").as_deref(),
        Some("mat(delim: \"(\", a, b)")
    );
    assert_eq!(
        to_typst("\\left[ \\begin{matrix} a \\\\ b \\end{matrix} \\right]").as_deref(),
        Some("mat(delim: \"[\", a; b)")
    );
}

#[test]
fn left_right_single_column_paren_fuses_into_vec() {
    assert_eq!(
        to_typst("\\left( \\begin{matrix} a \\\\ b \\end{matrix} \\right)").as_deref(),
        Some("vec(a, b)")
    );
}

#[test]
fn left_right_around_an_aligned_grid_fuses() {
    assert_eq!(
        to_typst("\\left( \\begin{aligned} a &= b \\\\ c &= d \\end{aligned} \\right)").as_deref(),
        Some("mat(delim: \"(\", a, = b; c, = d)")
    );
}

#[test]
fn left_bar_around_a_matrix_fuses_with_a_single_bar() {
    // A single-bar pair fuses as `|`, distinct from the bar matrix environments which render `||`.
    assert_eq!(
        to_typst("\\left| \\begin{matrix} a & b \\\\ c & d \\end{matrix} \\right|").as_deref(),
        Some("mat(delim: \"|\", a, b; c, d)")
    );
}

#[test]
fn left_right_around_a_delimited_matrix_does_not_fuse() {
    // A matrix with its own brackets keeps them, so the surrounding pair does not fuse.
    assert_eq!(
        to_typst("\\left( \\begin{pmatrix} a \\\\ b \\end{pmatrix} \\right)").as_deref(),
        Some("(vec(a, b))")
    );
}

#[test]
fn left_right_around_cases_does_not_fuse() {
    // A `cases` block keeps its own braces, so the surrounding pair does not fuse.
    assert_eq!(
        to_typst("\\left( \\begin{cases} a \\\\ b \\end{cases} \\right)").as_deref(),
        Some("({a\\\nb)")
    );
}

#[test]
fn left_right_with_extra_content_does_not_fuse() {
    // A grid that is not the sole content of the pair does not fuse.
    assert_eq!(
        to_typst("\\left( x \\begin{matrix} a & b \\end{matrix} \\right)").as_deref(),
        Some("(x a & b)")
    );
}

#[test]
fn angle_delimited_grid_does_not_fuse() {
    // Angle brackets are outside the fusing set, so the grid renders bare between the angle glyphs.
    assert_eq!(
        to_typst("\\left\\langle \\begin{matrix} a \\\\ b \\end{matrix} \\right\\rangle")
            .as_deref(),
        Some("\u{27E8}a\\\nb\u{27E9}")
    );
}

#[test]
fn mismatched_paired_delimiters_wrap_in_lr() {
    assert_eq!(to_typst("\\left( a \\right]").as_deref(), Some("lr((a])"));
    assert_eq!(to_typst("\\left[ a \\right)").as_deref(), Some("lr([a))"));
    assert_eq!(to_typst("\\left\\{ a \\right)").as_deref(), Some("lr({a))"));
    assert_eq!(to_typst("\\left| a \\right)").as_deref(), Some("lr(|a))"));
}

#[test]
fn mismatch_with_an_angle_side_escapes_the_paren_directly() {
    // a paren opposite an angle cannot auto-pair: escaped paren, angle glyph, no lr(..)
    assert_eq!(
        to_typst("\\left( a \\right>").as_deref(),
        Some("\\(a\u{27E9}")
    );
    assert_eq!(
        to_typst("\\left< a \\right)").as_deref(),
        Some("\u{27E8}a\\)")
    );
}

#[test]
fn matched_paren_pair_keeps_bare_auto_pairing_glyphs() {
    // A matching same-kind pair stays bare so Typst stretches and matches it.
    assert_eq!(to_typst("\\left( a \\right)").as_deref(), Some("(a)"));
    assert_eq!(to_typst("\\left[ a \\right]").as_deref(), Some("[a]"));
}

#[test]
fn in_grid_nonumber_is_stripped() {
    assert_eq!(
        to_typst("\\begin{aligned} a &= b \\nonumber \\\\ c &= d \\end{aligned}").as_deref(),
        Some("a & = b\\\nc & = d")
    );
    assert_eq!(
        to_typst("\\begin{matrix} a \\nonumber \\\\ b \\end{matrix}").as_deref(),
        Some("a\\\nb")
    );
}

#[test]
fn in_grid_tag_and_label_consume_their_argument() {
    assert_eq!(
        to_typst("\\begin{aligned} a &= b \\tag{1} \\\\ c &= d \\end{aligned}").as_deref(),
        Some("a & = b\\\nc & = d")
    );
    assert_eq!(
        to_typst("\\begin{aligned} a &= b \\tag*{x} \\\\ c &= d \\end{aligned}").as_deref(),
        Some("a & = b\\\nc & = d")
    );
    assert_eq!(
        to_typst("\\begin{aligned} a &= b \\label{eq:x} \\\\ c &= d \\end{aligned}").as_deref(),
        Some("a & = b\\\nc & = d")
    );
}

#[test]
fn lone_label_lifts_to_a_trailing_reference() {
    assert_eq!(
        typst_labeled("\\label{foo}"),
        Some((String::new(), Some("<foo>".into())))
    );
    assert_eq!(
        typst_labeled("\\label{eq:1}"),
        Some((String::new(), Some("<eq:1>".into())))
    );
}

#[test]
fn leading_label_lifts_and_body_keeps_the_content() {
    assert_eq!(
        typst_labeled("\\label{foo} x"),
        Some(("x".into(), Some("<foo>".into())))
    );
}

#[test]
fn leading_nonumber_and_tag_are_dropped_without_a_label() {
    assert_eq!(typst_labeled("\\nonumber x"), Some(("x".into(), None)));
    assert_eq!(typst_labeled("\\tag{1} x"), Some(("x".into(), None)));
    assert_eq!(typst_labeled("\\nonumber"), Some((String::new(), None)));
}

#[test]
fn the_first_label_wins() {
    assert_eq!(
        typst_labeled("\\label{a} \\label{b} x"),
        Some(("x".into(), Some("<a>".into())))
    );
    assert_eq!(
        typst_labeled("\\nonumber \\label{a} x"),
        Some(("x".into(), Some("<a>".into())))
    );
}

#[test]
fn an_empty_label_carries_no_reference() {
    assert_eq!(typst_labeled("\\label{}"), Some((String::new(), None)));
}

#[test]
fn a_trailing_annotation_is_stripped_like_a_leading_one() {
    // annotations label the whole expression and are consumed wherever they sit; a trailing
    // \label still lifts to a reference, \nonumber and \tag drop without a trace
    assert_eq!(
        typst_labeled("x \\label{foo}"),
        Some(("x".into(), Some("<foo>".into())))
    );
    assert_eq!(typst_labeled("x \\nonumber"), Some(("x".into(), None)));
    assert_eq!(typst_labeled("x \\tag{1}"), Some(("x".into(), None)));
}

#[test]
fn nonumber_does_not_swallow_a_following_group() {
    // `\nonumber` takes no argument: a following `{x}` is ordinary content.
    assert_eq!(typst_labeled("\\nonumber{x}"), Some(("x".into(), None)));
}

#[test]
fn a_non_identifier_label_name_is_quoted() {
    assert_eq!(
        typst_labeled("\\label{a b}"),
        Some((String::new(), Some("#label(\"a b\")".into())))
    );
    assert_eq!(
        typst_labeled("\\label{@x}"),
        Some((String::new(), Some("#label(\"@x\")".into())))
    );
    // An escaped character keeps its backslash and forces the quoted form.
    assert_eq!(
        typst_labeled("\\label{a\\_b}"),
        Some((String::new(), Some("#label(\"a\\\\_b\")".into())))
    );
}

#[test]
fn a_label_inside_a_grid_cell_is_found() {
    assert_eq!(
        typst_labeled("\\begin{align} a &= b \\label{eq:1} \\end{align}"),
        Some(("a & = b".into(), Some("<eq:1>".into())))
    );
}

#[test]
fn an_annotation_argument_tolerates_an_inner_dollar() {
    // The flat argument round-trips an inner `$`; the body and label stay well-formed.
    assert_eq!(
        typst_labeled("\\begin{align} a &= b \\tag{$x$} \\end{align}"),
        Some(("a & = b".into(), None))
    );
    assert_eq!(
        typst_labeled("\\begin{align} a &= b \\label{$x$} \\end{align}"),
        Some(("a & = b".into(), Some("#label(\"$x$\")".into())))
    );
}

#[test]
fn a_nested_brace_in_an_annotation_argument_falls_back_to_verbatim() {
    assert_eq!(
        to_typst("\\begin{align} a &= b \\tag{$\\sqrt{x}$} \\end{align}"),
        None
    );
}

#[test]
fn a_label_renders_as_nothing_in_inline_output() {
    // Labels surface only in Typst output; the inline tree drops them.
    assert_eq!(to_inlines("\\label{foo} x"), Some(vec![var("x")]));
}

#[test]
fn mathds_is_the_double_struck_alphabet() {
    assert_eq!(to_typst("\\mathds{R}").as_deref(), Some("bb(R)"));
    assert_eq!(
        to_inlines("\\mathds{R}"),
        Some(vec![str_inline("\u{211D}")])
    );
}

#[test]
fn symbf_and_upright_bold_are_bold() {
    assert_eq!(to_typst("\\symbf{R}").as_deref(), Some("bold(R)"));
    assert_eq!(to_typst("\\mathbfup{R}").as_deref(), Some("bold(R)"));
    assert_eq!(
        to_typst("\\mathbfsfup{R}").as_deref(),
        Some("bold(sans(R))")
    );
    assert_eq!(
        to_inlines("\\symbf{R}"),
        Some(vec![Inline::Strong(vec![str_inline("R")])])
    );
    assert_eq!(
        to_inlines("\\mathbfup{R}"),
        Some(vec![Inline::Strong(vec![str_inline("R")])])
    );
    assert_eq!(
        to_inlines("\\mathbfsfup{R}"),
        Some(vec![Inline::Strong(vec![str_inline("R")])])
    );
}

#[test]
fn explicit_upright_spellings_set_letters_upright() {
    assert_eq!(to_typst("\\mathup{R}").as_deref(), Some("upright(R)"));
    assert_eq!(to_typst("\\mathsfup{R}").as_deref(), Some("sans(R)"));
    assert_eq!(to_inlines("\\mathup{R}"), Some(vec![str_inline("R")]));
    assert_eq!(to_inlines("\\mathsfup{R}"), Some(vec![str_inline("R")]));
}

#[test]
fn the_other_sym_alphabets_have_no_glyph_change() {
    // Of the `\sym…` family only `\symbf` is resolved; the rest fall back to verbatim.
    for cmd in ["symup", "symbb", "symsf", "symit", "symcal"] {
        assert_eq!(to_typst(&format!("\\{cmd}{{R}}")), None, "{cmd}");
        assert_eq!(to_inlines(&format!("\\{cmd}{{R}}")), None, "{cmd}");
    }
}

#[test]
fn a_function_as_a_sole_script_atom_has_no_trailing_thin_space() {
    assert_eq!(
        to_inlines("x^{\\sin}"),
        Some(vec![var("x"), Inline::Superscript(vec![str_inline("sin")])])
    );
}

#[test]
fn a_function_with_following_script_content_keeps_its_inner_space() {
    // The thin space between the function and a following operand stays; only a dangling one is cut.
    assert_eq!(
        to_inlines("x^{\\sin y}"),
        Some(vec![
            var("x"),
            Inline::Superscript(vec![str_inline("sin\u{2006}"), var("y")]),
        ])
    );
}

#[test]
fn a_bare_function_keeps_its_thin_space_outside_a_script() {
    // At the top level the trailing thin space sets the function off from what follows.
    assert_eq!(to_inlines("\\sin"), Some(vec![str_inline("sin\u{2006}")]));
}

#[test]
fn a_binary_operator_before_a_relation_loses_its_spacing() {
    // + has no right operand (a relation follows), so it binds tight and merges with the =
    assert_eq!(
        to_inlines("a+=b"),
        Some(vec![var("a"), str_inline("+\u{2004}=\u{2004}"), var("b"),])
    );
}

#[test]
fn a_binary_operator_before_a_closing_delimiter_or_punctuation_loses_its_spacing() {
    assert_eq!(
        to_inlines("a+)b"),
        Some(vec![var("a"), str_inline("+)"), var("b")])
    );
    assert_eq!(
        to_inlines("a+,b"),
        Some(vec![var("a"), str_inline("+,\u{2006}"), var("b")])
    );
}

#[test]
fn a_binary_operator_before_an_opening_delimiter_keeps_its_spacing() {
    // An opening delimiter is a valid right operand, so the operator stays binary and spaced.
    assert_eq!(
        to_inlines("a+(b)"),
        Some(vec![
            var("a"),
            str_inline("\u{2005}+\u{2005}("),
            var("b"),
            str_inline(")"),
        ])
    );
}
