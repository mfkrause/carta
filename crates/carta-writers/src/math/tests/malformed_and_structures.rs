//! Malformed input handling, number lexing, and structural constructs.

use super::super::{to_inlines, to_typst};
use super::{emph, str_inline, var};
use carta_ast::Inline;

#[test]
fn malformed_input_returns_without_panic() {
    for bad in [
        "{",
        "}",
        "^",
        "_",
        "a_",
        "a^",
        "\\",
        "\\\\",
        "$",
        "\\frac",
        "\\sqrt",
        "\\mathbb",
        "\\begin{pmatrix}",
        "\\hat",
        "{a",
        "a}",
        "_{",
        "^{",
    ] {
        // The only contract for malformed input is: it terminates and yields an Option.
        let _ = to_inlines(bad);
        let _ = to_typst(bad);
    }
}

#[test]
fn deep_nesting_is_bounded_not_overflowing() {
    // Pathologically deep grouping must be rejected (or accepted) without a stack overflow.
    let deep_groups = format!("{}a{}", "{".repeat(5000), "}".repeat(5000));
    let _ = to_inlines(&deep_groups);
    let _ = to_typst(&deep_groups);

    let deep_scripts = format!("a{}", "^a".repeat(5000));
    let _ = to_inlines(&deep_scripts);
    let _ = to_typst(&deep_scripts);

    let deep_fracs = format!("{}{{1}}{{2}}", "\\frac".repeat(2000));
    let _ = to_inlines(&deep_fracs);
    let _ = to_typst(&deep_fracs);
}

#[test]
fn group_flattens_and_script_binds_last_atom() {
    // {a+b}^2 == a + b^2: the brace group does not become an atom of its own.
    assert_eq!(to_inlines("{a+b}^2"), to_inlines("a+b^2"));
    assert_eq!(to_typst("{a+b}^2"), to_typst("a+b^2"));
}

#[test]
fn adjacent_digits_form_one_number() {
    // A run of digits is a single atom, so a style applies to it as a unit rather than per digit.
    assert_eq!(to_inlines("12"), Some(vec![str_inline("12")]));
}

#[test]
fn decimal_point_joins_a_number_when_flanked_by_digits() {
    assert_eq!(to_inlines("3.14"), Some(vec![str_inline("3.14")]));
}

#[test]
fn leading_decimal_point_starts_a_number() {
    // `.5` is one numeric atom; the leading point is absorbed because a digit follows it.
    assert_eq!(to_inlines(".5"), Some(vec![str_inline(".5")]));
}

#[test]
fn trailing_decimal_point_is_not_part_of_the_number() {
    // `1.` lexes as the number `1` then a bare `.`; in Typst they are separate atoms and so spaced.
    assert_eq!(to_typst("1.").as_deref(), Some("1 ."));
}

#[test]
fn second_decimal_point_starts_a_fresh_number() {
    // one interior point per number, then `.5` starts a separate number atom
    assert_eq!(
        to_typst("\\mathbf{1.5.5}").as_deref(),
        Some("upright(bold(1.5 .5))")
    );
}

#[test]
fn a_space_breaks_a_digit_run_into_separate_numbers() {
    // The source space between digits keeps them as distinct atoms, so Typst sets them apart.
    assert_eq!(to_typst("1 2").as_deref(), Some("1 2"));
}

#[test]
fn bold_keeps_a_digit_run_as_one_styled_atom() {
    // \mathbf{12a} bolds the whole number `12` as one atom, then the letter `a`.
    assert_eq!(
        to_inlines("\\mathbf{12a}"),
        Some(vec![
            Inline::Strong(vec![str_inline("12")]),
            Inline::Strong(vec![str_inline("a")]),
        ])
    );
}

#[test]
fn monospace_keeps_a_digit_run_as_one_code_span() {
    assert_eq!(
        to_inlines("\\mathtt{1a23}"),
        Some(vec![
            Inline::Code(Box::default(), "1".to_string().into()),
            Inline::Code(Box::default(), "a".to_string().into()),
            Inline::Code(Box::default(), "23".to_string().into()),
        ])
    );
}

#[test]
fn blackboard_digits_use_double_struck_glyphs() {
    // \mathbb{12} maps each digit to its double-struck codepoint (U+1D7D9, U+1D7DA).
    assert_eq!(
        to_inlines("\\mathbb{12}"),
        Some(vec![str_inline("\u{1D7D9}\u{1D7DA}")])
    );
}

#[test]
fn script_alphabet_passes_digits_through_unchanged() {
    // script/fraktur have no styled digits: digits stay plain ASCII beside the styled letter
    assert_eq!(
        to_inlines("\\mathscr{F12}"),
        Some(vec![str_inline("\u{2131}12")])
    );
}

#[test]
fn math_italic_styles_each_atom_including_digits() {
    assert_eq!(
        to_inlines("\\mathit{a12}"),
        Some(vec![
            emph(vec![str_inline("a")]),
            emph(vec![str_inline("12")]),
        ])
    );
}

#[test]
fn not_equals_uses_the_precomposed_glyph() {
    // \not= is the single not-equal codepoint (U+2260), a relation.
    assert_eq!(to_inlines("\\not="), Some(vec![str_inline("\u{2260}")]));
}

#[test]
fn not_with_a_relation_command_uses_its_precomposed_glyph() {
    // \not\in is the not-an-element glyph (U+2209).
    assert_eq!(to_inlines("\\not\\in"), Some(vec![str_inline("\u{2209}")]));
}

#[test]
fn not_without_a_precomposed_glyph_overlays_a_solidus() {
    // no precomposed form: base glyph carries a combining long solidus (U+0338)
    assert_eq!(
        to_inlines("\\not\\vdash"),
        Some(vec![str_inline("\u{22A2}\u{0338}")])
    );
}

#[test]
fn not_with_a_precomposed_typst_token() {
    assert_eq!(to_typst("\\not=").as_deref(), Some("eq.not"));
}

#[test]
fn not_typst_overlay_uses_the_literal_base_glyph() {
    // no dedicated negated token: strike the literal base glyph, not its token name
    assert_eq!(
        to_typst("\\not\\vdash").as_deref(),
        Some("\u{22A2}\u{0338}")
    );
}

#[test]
fn not_over_a_relation_command_without_a_precomposed_glyph_overlays_a_solidus() {
    // no precomposed negated codepoint: base glyph carries a combining long solidus (U+0338)
    assert_eq!(
        to_inlines("\\not\\sim"),
        Some(vec![str_inline("\u{223C}\u{0338}")])
    );
    assert_eq!(
        to_inlines("\\not\\vDash"),
        Some(vec![str_inline("\u{22A8}\u{0338}")])
    );
    assert_eq!(
        to_inlines("\\not\\models"),
        Some(vec![str_inline("\u{22A8}\u{0338}")])
    );
}

#[test]
fn not_over_an_ordinary_or_binary_bar_command_falls_back_to_verbatim() {
    // no struck form for the bar commands: verbatim; the literal pipe \not| below does strike
    assert_eq!(to_inlines("\\not\\|"), None);
    assert_eq!(to_inlines("\\not\\Vert"), None);
    assert_eq!(to_inlines("\\not\\mid"), None);
    assert_eq!(to_typst("\\not\\|"), None);
    assert_eq!(to_typst("\\not\\Vert"), None);
}

#[test]
fn not_over_the_literal_pipe_character_strikes_an_italic_bar() {
    // a literal pipe (not the \| command) strikes as an italicised bar with a combining solidus
    assert_eq!(to_inlines("\\not|"), Some(vec![var("|\u{0338}")]));
}

#[test]
fn not_over_a_delimiter_or_operator_command_falls_back_to_verbatim() {
    // delimiters, set/space operators, and upright letterlikes carry no struck form
    assert_eq!(to_inlines("\\not\\lvert"), None);
    assert_eq!(to_inlines("\\not\\langle"), None);
    assert_eq!(to_inlines("\\not\\setminus"), None);
    assert_eq!(to_inlines("\\not\\hbar"), None);
    assert_eq!(to_inlines("\\not\\nabla"), None);
}

#[test]
fn not_over_an_italic_letterlike_command_strikes_the_glyph() {
    // A Greek letter or slanted letterlike command strikes as an italicised ordinary atom.
    assert_eq!(
        to_inlines("\\not\\alpha"),
        Some(vec![var("\u{03B1}\u{0338}")])
    );
    assert_eq!(
        to_inlines("\\not\\ell"),
        Some(vec![var("\u{2113}\u{0338}")])
    );
}

#[test]
fn nolimits_keeps_a_single_script_beside_a_limit_operator() {
    // one script already lays out inline, so \nolimits is a no-op here
    assert!(to_inlines("\\sum\\nolimits_{i}").is_some());
}

#[test]
fn limits_stacks_a_single_script_and_blocks_inline_lowering() {
    // \limits forces stacking whenever any script is present, which cannot be laid out on one line.
    assert_eq!(to_inlines("\\sum\\limits_i"), None);
}

#[test]
fn nolimits_forces_a_doubly_scripted_operator_to_lay_out_inline() {
    // Both scripts would normally stack on a sum; \nolimits keeps them beside it so it lowers.
    assert!(to_inlines("\\sum\\nolimits_i^n").is_some());
}

#[test]
fn delimited_group_spaces_an_escaped_comma() {
    // inside \left(..\right) the escaped comma is spaced; at top level it binds tightly
    assert_eq!(to_typst("x, y").as_deref(), Some("x\\,y"));
    assert_eq!(
        to_typst("\\left( x, y \\right)").as_deref(),
        Some("(x \\, y)")
    );
}

#[test]
fn brace_does_not_linearise_to_inlines() {
    assert_eq!(to_inlines("\\overbrace{a+b}"), None);
    assert_eq!(to_inlines("\\underbrace{x+y}"), None);
}

#[test]
fn overbrace_lowers_to_a_typst_function() {
    assert_eq!(
        to_typst("\\overbrace{a+b}").as_deref(),
        Some("overbrace(a + b)")
    );
}

#[test]
fn overbrace_takes_its_superscript_as_a_label() {
    assert_eq!(
        to_typst("\\overbrace{a+b}^{n}").as_deref(),
        Some("overbrace(a + b, n)")
    );
}

#[test]
fn underbrace_takes_its_subscript_as_a_label() {
    assert_eq!(
        to_typst("\\underbrace{x+y}_{k}").as_deref(),
        Some("underbrace(x + y, k)")
    );
}

#[test]
fn brace_with_both_scripts_takes_neither_as_a_label() {
    // with both scripts present neither annotates the brace; both render as ordinary scripts
    assert_eq!(
        to_typst("\\overbrace{x}^a_b").as_deref(),
        Some("overbrace(x)_b^a")
    );
}

#[test]
fn undelimited_matrix_is_a_bare_alignment() {
    // undelimited grid: cells joined by ` & `, rows by a trailing backslash and line break
    assert_eq!(
        to_typst("\\begin{matrix} a & b \\\\ c & d \\end{matrix}").as_deref(),
        Some("a & b\\\nc & d")
    );
}

#[test]
fn undelimited_matrix_does_not_linearise_to_inlines() {
    assert_eq!(
        to_inlines("\\begin{matrix} a & b \\\\ c & d \\end{matrix}"),
        None
    );
}

#[test]
fn lone_backslash_has_no_conversion() {
    assert_eq!(to_inlines("\\"), None);
    assert_eq!(to_typst("\\"), None);
}

#[test]
fn trailing_backslash_has_no_conversion() {
    assert_eq!(to_inlines("a\\"), None);
    assert_eq!(to_typst("a\\"), None);
}

#[test]
fn control_space_mid_expression_converts() {
    // A control space between operands lays out as a thin space, not a fallback to verbatim.
    assert!(to_typst("a \\ b").is_some());
    assert!(to_inlines("a \\ b").is_some());
}

#[test]
fn literal_slash_is_escaped_in_typst() {
    assert_eq!(to_typst("a/b").as_deref(), Some("a\\/b"));
}

#[test]
fn fraction_divider_stays_bare_in_typst() {
    assert_eq!(to_typst("\\frac{a}{b}").as_deref(), Some("a / b"));
}

#[test]
fn capital_greek_lookalike_spells_its_name_in_typst() {
    assert_eq!(to_typst("\\Alpha").as_deref(), Some("Alpha"));
    assert_eq!(to_typst("\\Omicron").as_deref(), Some("Omicron"));
    assert_eq!(to_typst("\\Chi").as_deref(), Some("Chi"));
}

#[test]
fn capital_greek_lookalike_is_a_codepoint_inline() {
    // The capital alpha lookalike is the Greek capital letter, italicised like other capitals.
    assert_eq!(to_inlines("\\Alpha"), Some(vec![var("\u{0391}")]));
}

#[test]
fn prime_superscript_collapses_to_apostrophe() {
    assert_eq!(to_typst("f^{\\prime}").as_deref(), Some("f'"));
}

#[test]
fn medspace_and_enspace_render_fixed_inline_spaces() {
    assert_eq!(to_inlines("\\medspace"), Some(vec![str_inline("\u{205F}")]));
    assert_eq!(to_inlines("\\enspace"), Some(vec![str_inline("\u{2000}")]));
}

#[test]
fn medspace_and_enspace_in_typst() {
    assert_eq!(to_typst("\\medspace").as_deref(), Some("space.med"));
    assert_eq!(to_typst("\\enspace").as_deref(), Some("#h(0em)"));
}

#[test]
fn mod_leads_its_operand() {
    assert_eq!(
        to_inlines("a \\mod b"),
        Some(vec![
            var("a"),
            str_inline("\u{2000}mod\u{2006}\u{00A0}"),
            var("b"),
        ])
    );
}

#[test]
fn pod_is_a_parenthesised_trailer() {
    assert_eq!(
        to_inlines("\\pod{x}"),
        Some(vec![str_inline("\u{00A0}("), var("x"), str_inline(")")])
    );
}

#[test]
fn pmod_and_pod_in_typst() {
    assert_eq!(to_typst("\\pmod{n}").as_deref(), Some("med\\(mod med n\\)"));
    assert_eq!(to_typst("\\pod{x}").as_deref(), Some("med\\(x\\)"));
}

#[test]
fn single_atom_class_wrapper_is_upright() {
    // A one-atom \mathord argument is set upright with no surrounding glyph.
    assert_eq!(to_inlines("\\mathord{x}"), Some(vec![str_inline("x")]));
}

#[test]
fn class_wrappers_are_transparent_in_typst() {
    assert_eq!(to_typst("\\mathord{x}").as_deref(), Some("x"));
    assert_eq!(to_typst("\\mathbin{x}").as_deref(), Some("x"));
}

#[test]
fn mathop_single_letter_is_bare_in_typst() {
    assert_eq!(to_typst("\\mathop{x}").as_deref(), Some("x"));
}

#[test]
fn mathop_unknown_run_is_quoted_in_typst() {
    assert_eq!(to_typst("\\mathop{xy}").as_deref(), Some("\"xy\""));
}

#[test]
fn mathop_known_operator_is_bare_in_typst() {
    assert_eq!(to_typst("\\mathop{lim}").as_deref(), Some("lim"));
}

#[test]
fn color_is_stripped() {
    assert_eq!(to_typst("\\color{red}{x}").as_deref(), Some("x"));
    assert_eq!(to_inlines("\\color{red}{x}"), Some(vec![var("x")]));
}

#[test]
fn style_switch_is_dropped() {
    assert_eq!(to_typst("\\displaystyle x").as_deref(), Some("x"));
}

#[test]
fn phantom_and_cancel_and_boxed_in_typst() {
    assert_eq!(to_typst("\\phantom{x}").as_deref(), Some("#hide[x]"));
    assert_eq!(to_typst("\\cancel{x}").as_deref(), Some("cancel(x)"));
    assert_eq!(
        to_typst("\\boxed{x}").as_deref(),
        Some("#box(stroke: black, inset: 3pt, [$ x $])")
    );
}

#[test]
fn operatorname_star_renders_known_operator_bare() {
    assert_eq!(to_typst("\\operatorname*{lim}").as_deref(), Some("lim"));
}

#[test]
fn operatorname_unknown_is_quoted() {
    assert_eq!(to_typst("\\operatorname{foo}").as_deref(), Some("\"foo\""));
}

#[test]
fn overparen_sets_content_as_an_over_script() {
    assert_eq!(
        to_typst("\\overparen{abc}").as_deref(),
        Some("a b c^paren.t")
    );
}

#[test]
fn triple_dot_accent_in_typst() {
    assert_eq!(
        to_typst("\\dddot{x}").as_deref(),
        Some("accent(x, \u{20DB})")
    );
}

#[test]
fn overset_bare_letter_mark_stands_alone() {
    assert_eq!(to_typst("\\overset{a}{b}").as_deref(), Some("b^a"));
}

#[test]
fn overset_non_letter_mark_is_parenthesised() {
    assert_eq!(to_typst("\\overset{!}{=}").as_deref(), Some("=^(!)"));
}

#[test]
fn stackrel_is_the_over_form() {
    assert_eq!(
        to_typst("\\stackrel{\\text{def}}{=}").as_deref(),
        Some("=^(upright(\"def\"))")
    );
}

#[test]
fn underset_sets_a_subscript_mark() {
    assert_eq!(to_typst("\\underset{a}{b}").as_deref(), Some("b_a"));
}

#[test]
fn stack_has_no_inline_form() {
    assert_eq!(to_inlines("\\overset{a}{b}"), None);
}

#[test]
fn cases_joins_columns_then_rows() {
    assert_eq!(
        to_typst("\\begin{cases} a & x > 0 \\\\ b \\end{cases}").as_deref(),
        Some("cases(delim: \"{\", a & x > 0, b)")
    );
}

#[test]
fn aligned_joins_cells_and_rows() {
    assert_eq!(
        to_typst("\\begin{aligned} a &= b \\\\ c &= d \\end{aligned}").as_deref(),
        Some("a & = b\\\nc & = d")
    );
}

#[test]
fn substack_stacks_single_cells() {
    assert_eq!(to_typst("\\substack{a \\\\ b}").as_deref(), Some("a\\\nb"));
}

#[test]
fn infix_choose_is_a_binom() {
    assert_eq!(to_typst("{n \\choose k}").as_deref(), Some("binom(n, k)"));
}

#[test]
fn infix_brace_and_brack_select_their_brackets() {
    assert_eq!(to_typst("{a \\brace b}").as_deref(), Some("{a / b}"));
    assert_eq!(to_typst("{a \\brack b}").as_deref(), Some("[a / b]"));
}

#[test]
fn genfrac_paren_delimiter_is_a_binom() {
    assert_eq!(
        to_typst("\\genfrac{(}{)}{0pt}{}{a}{b}").as_deref(),
        Some("binom(a, b)")
    );
}

#[test]
fn genfrac_bracket_delimiter_uses_square_brackets() {
    assert_eq!(
        to_typst("\\genfrac{[}{]}{0pt}{}{a}{b}").as_deref(),
        Some("[a / b]")
    );
}

#[test]
fn genfrac_without_delimiter_has_no_typst_form() {
    assert_eq!(to_typst("\\genfrac{}{}{0pt}{}{a}{b}"), None);
}
