//! Inline lowering of single symbols, scripts, styles, and accents.

use super::super::{to_inlines, to_typst};
use super::{str_inline, var};
use carta_ast::Inline;

#[test]
fn single_variable_is_italic() {
    assert_eq!(to_inlines("x"), Some(vec![var("x")]));
}

#[test]
fn digits_stay_upright() {
    assert_eq!(to_inlines("2"), Some(vec![str_inline("2")]));
}

#[test]
fn multi_digit_number_is_one_string() {
    assert_eq!(to_inlines("123"), Some(vec![str_inline("123")]));
}

#[test]
fn superscript_wraps_the_exponent() {
    assert_eq!(
        to_inlines("x^2"),
        Some(vec![var("x"), Inline::Superscript(vec![str_inline("2")])])
    );
}

#[test]
fn subscript_wraps_the_index() {
    assert_eq!(
        to_inlines("a_i"),
        Some(vec![var("a"), Inline::Subscript(vec![var("i")])])
    );
}

#[test]
fn both_scripts_emit_sub_then_sup() {
    assert_eq!(
        to_inlines("x_i^2"),
        Some(vec![
            var("x"),
            Inline::Subscript(vec![var("i")]),
            Inline::Superscript(vec![str_inline("2")]),
        ])
    );
}

#[test]
fn lowercase_greek_is_an_italic_codepoint() {
    assert_eq!(to_inlines("\\alpha"), Some(vec![var("\u{3b1}")]));
}

#[test]
fn uppercase_greek_is_italicised() {
    assert_eq!(to_inlines("\\Gamma"), Some(vec![var("\u{393}")]));
}

#[test]
fn large_operator_is_a_bare_symbol() {
    assert_eq!(to_inlines("\\sum"), Some(vec![str_inline("\u{2211}")]));
}

#[test]
fn named_function_gets_a_trailing_thin_space() {
    // upright name plus trailing thin space (U+2006); adjacent strings collapse into one
    assert_eq!(to_inlines("\\sin"), Some(vec![str_inline("sin\u{2006}")]));
}

#[test]
fn binary_operator_is_surrounded_by_four_per_em_spaces() {
    assert_eq!(
        to_inlines("a+b"),
        Some(vec![var("a"), str_inline("\u{2005}+\u{2005}"), var("b"),])
    );
}

#[test]
fn relation_is_surrounded_by_three_per_em_spaces() {
    assert_eq!(
        to_inlines("a=b"),
        Some(vec![var("a"), str_inline("\u{2004}=\u{2004}"), var("b"),])
    );
}

#[test]
fn blackboard_bold_maps_to_letterlike_codepoint() {
    // R double-struck is the named codepoint U+211D, not the algorithmic slot.
    assert_eq!(
        to_inlines("\\mathbb{R}"),
        Some(vec![str_inline("\u{211d}")])
    );
}

#[test]
fn math_italic_letter_is_a_single_emph() {
    assert_eq!(to_inlines("\\mathit{x}"), Some(vec![var("x")]));
}

#[test]
fn text_command_is_upright_literal() {
    assert_eq!(to_inlines("\\text{hello}"), Some(vec![str_inline("hello")]));
}

#[test]
fn text_command_keeps_interior_spaces() {
    assert_eq!(to_inlines("\\text{a b}"), Some(vec![str_inline("a b")]));
}

#[test]
fn monospace_text_becomes_per_character_code() {
    assert_eq!(
        to_inlines("\\mathtt{ab}"),
        Some(vec![
            Inline::Code(Box::default(), "a".to_string().into()),
            Inline::Code(Box::default(), "b".to_string().into()),
        ])
    );
}

#[test]
fn hat_appends_a_combining_mark() {
    // \hat{x} is x followed by the combining circumflex (U+0302), inside the italic.
    assert_eq!(to_inlines("\\hat{x}"), Some(vec![var("x\u{302}")]));
}

#[test]
fn wide_hat_matches_plain_hat_for_one_letter() {
    assert_eq!(to_inlines("\\widehat{x}"), to_inlines("\\hat{x}"));
}

#[test]
fn prime_becomes_a_superscript_prime_glyph() {
    assert_eq!(
        to_inlines("f'"),
        Some(vec![
            var("f"),
            Inline::Superscript(vec![str_inline("\u{2032}")])
        ])
    );
}

#[test]
fn double_prime_uses_the_double_prime_glyph() {
    assert_eq!(
        to_inlines("f''"),
        Some(vec![
            var("f"),
            Inline::Superscript(vec![str_inline("\u{2033}")])
        ])
    );
}

#[test]
fn prime_then_explicit_superscript_keeps_both() {
    // x'^2 places the prime mark before the explicit exponent within one superscript.
    assert_eq!(
        to_inlines("x'^2"),
        Some(vec![
            var("x"),
            Inline::Superscript(vec![str_inline("\u{2032}")]),
            Inline::Superscript(vec![str_inline("2")]),
        ])
    );
}

#[test]
fn prime_after_a_subscript_fills_the_free_superscript_slot() {
    // the base's superscript slot is free, so the prime nests there beside the subscript
    assert_eq!(to_typst("a_b'"), Some("a'_b".to_string()));
}

#[test]
fn prime_after_a_filled_subscript_and_superscript_detaches() {
    // both slots filled: the prime detaches as a bare glyph after the script
    assert_eq!(to_typst("a_b^c'"), Some("a_b^c'".to_string()));
}

#[test]
fn prime_detaches_after_a_nested_prime_superscript() {
    // the outer prime reaches a parent whose superscript is already filled, so it detaches
    assert_eq!(to_typst("a^c'_d'"), Some("a^(c'_d)'".to_string()));
}

#[test]
fn prime_detaches_symmetrically_for_the_subscript_first_shape() {
    // mirror shape: the closing prime detaches onto its own empty base
    assert_eq!(to_typst("a_c'^d'"), Some("a'_c \"\"^(d')".to_string()));
}

#[test]
fn consecutive_primes_after_a_subscript_merge_into_one_double_prime() {
    // the first prime stays the active atom; the second merges into one double prime
    assert_eq!(to_typst("a_b''"), Some("a''_b".to_string()));
    assert_eq!(
        to_inlines("a_b''"),
        Some(vec![
            var("a"),
            Inline::Subscript(vec![var("b")]),
            Inline::Superscript(vec![str_inline("\u{2033}")]),
        ])
    );
}

#[test]
fn function_alone_in_a_script_drops_its_trailing_thin_space() {
    // sole script content has no following operand, so the trailing thin space (U+2006) drops
    assert_eq!(
        to_inlines("x^{\\sin}"),
        Some(vec![var("x"), Inline::Superscript(vec![str_inline("sin")])])
    );
    assert_eq!(
        to_inlines("x_{\\log}"),
        Some(vec![var("x"), Inline::Subscript(vec![str_inline("log")])])
    );
}

#[test]
fn function_among_other_atoms_in_a_script_keeps_its_thin_space() {
    // more than one atom in the script keeps the function's thin space
    assert_eq!(
        to_inlines("x^{\\sin\\cos}"),
        Some(vec![
            var("x"),
            Inline::Superscript(vec![str_inline("sin\u{2006}cos\u{2006}")]),
        ])
    );
    assert_eq!(
        to_inlines("x^{1+\\sin}"),
        Some(vec![
            var("x"),
            Inline::Superscript(vec![str_inline("1\u{2005}+\u{2005}sin\u{2006}")]),
        ])
    );
}

#[test]
fn leading_subscript_attaches_to_an_empty_nucleus() {
    // `_x` has no base: it lowers to just the subscript, with no preceding glyph.
    assert_eq!(
        to_inlines("_x"),
        Some(vec![Inline::Subscript(vec![var("x")])])
    );
}

#[test]
fn leading_superscript_attaches_to_an_empty_nucleus() {
    assert_eq!(
        to_inlines("^2"),
        Some(vec![Inline::Superscript(vec![str_inline("2")])])
    );
}

#[test]
fn negative_exponent_keeps_the_minus_sign() {
    // The minus in an exponent is the math minus (U+2212), merged with the following digit.
    assert_eq!(
        to_inlines("x^{-1}"),
        Some(vec![
            var("x"),
            Inline::Superscript(vec![str_inline("\u{2212}1")])
        ])
    );
}

#[test]
fn bold_styles_each_character_separately() {
    assert_eq!(
        to_inlines("\\mathbf{ab}"),
        Some(vec![
            Inline::Strong(vec![str_inline("a")]),
            Inline::Strong(vec![str_inline("b")]),
        ])
    );
}

#[test]
fn script_capital_maps_to_its_named_letterlike_codepoint() {
    // \mathscr{F} is the named script F (U+2131), not the algorithmic slot.
    assert_eq!(
        to_inlines("\\mathscr{F}"),
        Some(vec![str_inline("\u{2131}")])
    );
}

#[test]
fn set_minus_is_a_spaced_backslash() {
    // trailing space stays its own string so a writer escaping the backslash does not consume it
    assert_eq!(
        to_inlines("a \\setminus b"),
        Some(vec![
            var("a"),
            str_inline("\u{2005}\\"),
            str_inline("\u{2005}"),
            var("b"),
        ])
    );
}

#[test]
fn bare_double_bar_pair_renders_as_the_double_line_glyph() {
    // \lVert x \rVert as a matched pair is the stretchy double vertical line (U+2016) on both sides.
    assert_eq!(
        to_inlines("\\lVert x \\rVert"),
        Some(vec![
            str_inline("\u{2016}"),
            var("x"),
            str_inline("\u{2016}"),
        ])
    );
}

#[test]
fn lone_double_bar_command_is_the_parallel_sign() {
    // Written alone, \lVert is the loose parallel sign (U+2225), not a paired delimiter.
    assert_eq!(to_inlines("\\lVert"), Some(vec![str_inline("\u{2225}")]));
}

#[test]
fn angle_brackets_pair_as_their_glyphs() {
    assert_eq!(
        to_inlines("\\langle x \\rangle"),
        Some(vec![
            str_inline("\u{27E8}"),
            var("x"),
            str_inline("\u{27E9}"),
        ])
    );
}

#[test]
fn binary_mod_renders_the_operator_word_with_spacing() {
    assert_eq!(
        to_inlines("a \\bmod b"),
        Some(vec![
            var("a"),
            str_inline("\u{00A0}mod\u{2006}\u{00A0}"),
            var("b"),
        ])
    );
}

#[test]
fn lone_binary_mod_has_no_inline_form() {
    // A modulo operator with nothing to bind to falls back to verbatim source.
    assert_eq!(to_inlines("\\bmod"), None);
    assert_eq!(to_typst("\\bmod"), None);
}

#[test]
fn overleftarrow_has_no_inline_form_but_translates() {
    assert_eq!(to_inlines("\\overleftarrow{AB}"), None);
    assert_eq!(
        to_typst("\\overleftarrow{AB}").as_deref(),
        Some("accent(A B, \u{20D6})")
    );
}

#[test]
fn surd_prefix_radical_has_no_inline_form_but_translates() {
    // \surd takes the next group as a radicand: a radical, which cannot sit inline.
    assert_eq!(to_inlines("\\surd x"), None);
    assert_eq!(to_typst("\\surd x").as_deref(), Some("sqrt(x)"));
}

#[test]
fn fraction_has_no_inline_form() {
    assert_eq!(to_inlines("\\frac{1}{2}"), None);
}

#[test]
fn radical_has_no_inline_form() {
    assert_eq!(to_inlines("\\sqrt{x}"), None);
}

#[test]
fn over_primitive_has_no_inline_form() {
    assert_eq!(to_inlines("a \\over b"), None);
}

#[test]
fn stacked_operator_limits_have_no_inline_form() {
    // A limit operator carrying both a lower and upper limit cannot sit on one line.
    assert_eq!(to_inlines("\\sum_{i=1}^{n}"), None);
}

#[test]
fn second_order_accent_has_no_inline_form() {
    // The double-dot accent has no single combining-mark rendering here.
    assert_eq!(to_inlines("\\ddot{x}"), None);
}

#[test]
fn matrix_has_no_inline_form() {
    assert_eq!(to_inlines("\\begin{pmatrix}a&b\\\\c&d\\end{pmatrix}"), None);
}

#[test]
fn unknown_command_has_no_inline_form() {
    assert_eq!(to_inlines("\\nosuchcommand"), None);
}

#[test]
fn empty_input_lowers_to_an_empty_result() {
    // empty and whitespace-only math converts successfully to nothing, not a verbatim fallback
    assert_eq!(to_inlines(""), Some(Vec::new()));
    assert_eq!(to_inlines("   "), Some(Vec::new()));
    assert_eq!(to_inlines("{}"), Some(Vec::new()));
    assert_eq!(to_typst("").as_deref(), Some(""));
    assert_eq!(to_typst("   ").as_deref(), Some(""));
    assert_eq!(to_typst("{}").as_deref(), Some(""));
}
