use super::{to_inlines, to_typst, to_typst_display, to_typst_labeled};
use carta_ast::Inline;

/// The Typst body and formatted trailing label for an expression, for the equation-label tests.
fn typst_labeled(tex: &str) -> Option<(String, Option<String>)> {
    to_typst_labeled(tex, false).map(|m| (m.body, m.label))
}

fn str_inline(s: &str) -> Inline {
    Inline::Str(s.to_string().into())
}

fn emph(inner: Vec<Inline>) -> Inline {
    Inline::Emph(inner)
}

/// A single italic letter: the common shape for a math variable.
fn var(s: &str) -> Inline {
    emph(vec![str_inline(s)])
}

// ----------------------------------------------------------------------------
// Inline tree — atoms and variables
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Inline tree — symbols, Greek letters, functions
// ----------------------------------------------------------------------------

#[test]
fn lowercase_greek_is_an_italic_codepoint() {
    // A Greek small letter is a variable: the codepoint wrapped in italic.
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
    // A function name renders upright and is followed by a thin space (U+2006); the two
    // adjacent strings collapse into one.
    assert_eq!(to_inlines("\\sin"), Some(vec![str_inline("sin\u{2006}")]));
}

// ----------------------------------------------------------------------------
// Inline tree — atom-class spacing
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Inline tree — styled alphabets and text
// ----------------------------------------------------------------------------

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
    // \mathit{x} is just an italic x — no double wrapping.
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

// ----------------------------------------------------------------------------
// Inline tree — accents
// ----------------------------------------------------------------------------

#[test]
fn hat_appends_a_combining_mark() {
    // \hat{x} is x followed by the combining circumflex (U+0302), inside the italic.
    assert_eq!(to_inlines("\\hat{x}"), Some(vec![var("x\u{302}")]));
}

#[test]
fn wide_hat_matches_plain_hat_for_one_letter() {
    assert_eq!(to_inlines("\\widehat{x}"), to_inlines("\\hat{x}"));
}

// ----------------------------------------------------------------------------
// Inline tree — primes
// ----------------------------------------------------------------------------

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
    // `a_b'` has a free superscript slot on the base, so the prime nests there rather than
    // detaching: the prime sits on `a`, beside the subscript.
    assert_eq!(to_typst("a_b'"), Some("a'_b".to_string()));
}

#[test]
fn prime_after_a_filled_subscript_and_superscript_detaches() {
    // With both primary slots filled (`a_b^c`), a following prime has nowhere to nest, so it
    // detaches and surfaces as a bare glyph after the script.
    assert_eq!(to_typst("a_b^c'"), Some("a_b^c'".to_string()));
}

#[test]
fn prime_detaches_after_a_nested_prime_superscript() {
    // `a^c'_d'` carries a prime inside the superscript and a filled subscript on that nest; the
    // outer prime reaches a parent that already has a superscript, so it detaches.
    assert_eq!(to_typst("a^c'_d'"), Some("a^(c'_d)'".to_string()));
}

#[test]
fn prime_detaches_symmetrically_for_the_subscript_first_shape() {
    // The mirror `a_c'^d'`: a prime on the subscript nest, then a primed superscript. The closing
    // prime detaches onto its own empty base, separated from the superscript.
    assert_eq!(to_typst("a_c'^d'"), Some("a'_c \"\"^(d')".to_string()));
}

#[test]
fn consecutive_primes_after_a_subscript_merge_into_one_double_prime() {
    // The first prime fills the base superscript slot and stays the active atom; the second merges
    // into it, so `a_b''` is one double-prime on the base, not two detached primes.
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

// ----------------------------------------------------------------------------
// A named function as the sole content of a script drops its trailing thin space
// ----------------------------------------------------------------------------

#[test]
fn function_alone_in_a_script_drops_its_trailing_thin_space() {
    // A bare function name carries a trailing thin space (U+2006) that sets it off from a following
    // operand. Standing as the entire script content there is no operand to separate, so the space is
    // dropped: `x^{\sin}` superscripts a bare `sin` and `x_{\log}` subscripts a bare `log`.
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
    // With more than one atom in the script the function is not standing alone, so its thin space is
    // kept — whether two functions sit together (`x^{\sin\cos}`) or the function ends a longer run
    // (`x^{1+\sin}`).
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

// ----------------------------------------------------------------------------
// Inline tree — leading scripts on a synthesized empty nucleus
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Inline tree — per-character styled alphabets and script letters
// ----------------------------------------------------------------------------

#[test]
fn bold_styles_each_character_separately() {
    // \mathbf{ab} bolds a and b as two independent upright-bold atoms.
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

// ----------------------------------------------------------------------------
// Inline tree — set-minus keeps its space before a backslash glyph
// ----------------------------------------------------------------------------

#[test]
fn set_minus_is_a_spaced_backslash() {
    // \setminus is a binary operator rendering as a backslash with four-per-em spaces. The trailing
    // space is kept in its own string rather than merged onto the backslash, so a writer that
    // escapes the backslash does not also consume the following space.
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

// ----------------------------------------------------------------------------
// Inline tree — auto-paired and explicit delimiters
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Inline tree — modulo operators
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Inline tree — constructs that lower to no inline form but do translate to Typst
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Inline tree — fallback boundary (return None)
// ----------------------------------------------------------------------------

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
    // Empty and whitespace-only math is a successful conversion to nothing, not a fallback to
    // verbatim source: the inline form is the empty list and the Typst form is the empty string.
    assert_eq!(to_inlines(""), Some(Vec::new()));
    assert_eq!(to_inlines("   "), Some(Vec::new()));
    assert_eq!(to_inlines("{}"), Some(Vec::new()));
    assert_eq!(to_typst("").as_deref(), Some(""));
    assert_eq!(to_typst("   ").as_deref(), Some(""));
    assert_eq!(to_typst("{}").as_deref(), Some(""));
}

// ----------------------------------------------------------------------------
// Typst backend — atoms, scripts, spacing
// ----------------------------------------------------------------------------

#[test]
fn typst_variable_is_bare() {
    assert_eq!(to_typst("x").as_deref(), Some("x"));
}

#[test]
fn typst_superscript() {
    assert_eq!(to_typst("x^2").as_deref(), Some("x^2"));
}

#[test]
fn typst_subscript() {
    assert_eq!(to_typst("a_i").as_deref(), Some("a_i"));
}

#[test]
fn typst_both_scripts() {
    assert_eq!(to_typst("x_i^2").as_deref(), Some("x_i^2"));
}

#[test]
fn typst_binary_operator_is_spaced() {
    assert_eq!(to_typst("a+b").as_deref(), Some("a + b"));
}

#[test]
fn typst_relation_is_spaced() {
    assert_eq!(to_typst("a=b").as_deref(), Some("a = b"));
}

#[test]
fn typst_greek_is_a_named_symbol() {
    assert_eq!(to_typst("\\alpha").as_deref(), Some("alpha"));
    assert_eq!(to_typst("\\Gamma").as_deref(), Some("Gamma"));
}

#[test]
fn typst_large_operator_is_named() {
    assert_eq!(to_typst("\\sum").as_deref(), Some("sum"));
}

#[test]
fn typst_function_is_named() {
    assert_eq!(to_typst("\\sin").as_deref(), Some("sin"));
}

// ----------------------------------------------------------------------------
// Symbol coverage — horizontal-dots alias, named parentheses, double colon
// ----------------------------------------------------------------------------

#[test]
fn horizontal_dots_alias_is_the_ellipsis_glyph() {
    // `\hdots` is an alias for the horizontal ellipsis, an ordinary glyph.
    assert_eq!(to_inlines("\\hdots"), Some(vec![str_inline("\u{2026}")]));
    assert_eq!(to_typst("\\hdots").as_deref(), Some("dots.h"));
}

#[test]
fn named_parentheses_are_open_and_close_delimiters() {
    // `\lparen`/`\rparen` are the round parentheses, carrying open/close spacing: no space hugs the
    // operand between them.
    assert_eq!(to_inlines("\\lparen"), Some(vec![str_inline("(")]));
    assert_eq!(to_inlines("\\rparen"), Some(vec![str_inline(")")]));
    assert_eq!(
        to_inlines("\\lparen x\\rparen"),
        Some(vec![str_inline("("), var("x"), str_inline(")")])
    );
    assert_eq!(to_typst("\\lparen").as_deref(), Some("\\("));
    assert_eq!(to_typst("\\rparen").as_deref(), Some("\\)"));
}

#[test]
fn double_colon_is_a_relation_glyph() {
    // `\Colon` is the proportion sign, a relation: surrounded by relation spacing (U+2004).
    assert_eq!(to_inlines("\\Colon"), Some(vec![str_inline("\u{2237}")]));
    assert_eq!(
        to_inlines("a\\Colon b"),
        Some(vec![
            var("a"),
            str_inline("\u{2004}\u{2237}\u{2004}"),
            var("b")
        ])
    );
    assert_eq!(to_typst("\\Colon").as_deref(), Some("colon.double"));
}

#[test]
fn upright_differential_letters_are_ordinary_double_struck_glyphs() {
    // `\dd \ee \ii \jj \DD` are the double-struck differential and operator letters (U+2146–U+2149
    // and U+2145): ordinary class, so no spacing hugs them, and upright rather than italicised. Typst
    // has no named symbol for them, so it sets the raw glyph.
    assert_eq!(to_inlines("\\dd"), Some(vec![str_inline("\u{2146}")]));
    assert_eq!(to_inlines("\\ee"), Some(vec![str_inline("\u{2147}")]));
    assert_eq!(to_inlines("\\ii"), Some(vec![str_inline("\u{2148}")]));
    assert_eq!(to_inlines("\\jj"), Some(vec![str_inline("\u{2149}")]));
    assert_eq!(to_inlines("\\DD"), Some(vec![str_inline("\u{2145}")]));
    // Ordinary class: an upright glyph sits directly between two italic operands with no spacing.
    assert_eq!(
        to_inlines("a\\dd b"),
        Some(vec![var("a"), str_inline("\u{2146}"), var("b")])
    );
    assert_eq!(to_typst("\\dd").as_deref(), Some("\u{2146}"));
    assert_eq!(to_typst("\\DD").as_deref(), Some("\u{2145}"));
}

#[test]
fn extended_relations_carry_relation_spacing() {
    // The added comparison and ordering relations surround their operands with relation spacing
    // (U+2004).
    assert_eq!(to_inlines("\\nsime"), Some(vec![str_inline("\u{2244}")]));
    assert_eq!(to_inlines("\\eqgtr"), Some(vec![str_inline("\u{22DD}")]));
    assert_eq!(to_inlines("\\eqless"), Some(vec![str_inline("\u{22DC}")]));
    assert_eq!(to_inlines("\\gescc"), Some(vec![str_inline("\u{2AA9}")]));
    assert_eq!(to_inlines("\\lescc"), Some(vec![str_inline("\u{2AA8}")]));
    assert_eq!(to_inlines("\\strictif"), Some(vec![str_inline("\u{297D}")]));
    assert_eq!(to_inlines("\\strictfi"), Some(vec![str_inline("\u{297C}")]));
    assert_eq!(
        to_inlines("a\\nsime b"),
        Some(vec![
            var("a"),
            str_inline("\u{2004}\u{2244}\u{2004}"),
            var("b")
        ])
    );
    assert_eq!(to_typst("\\nsime").as_deref(), Some("tilde.eq.not"));
    assert_eq!(to_typst("\\eqgtr").as_deref(), Some("eq.gt"));
    assert_eq!(to_typst("\\eqless").as_deref(), Some("eq.lt"));
}

#[test]
fn extended_circled_binary_operators_carry_binary_spacing() {
    // The added circled binary operators surround their operands with binary spacing (U+2005).
    assert_eq!(
        to_inlines("\\circledequal"),
        Some(vec![str_inline("\u{229C}")])
    );
    assert_eq!(
        to_inlines("\\circledparallel"),
        Some(vec![str_inline("\u{29B7}")])
    );
    assert_eq!(
        to_inlines("a\\circledequal b"),
        Some(vec![
            var("a"),
            str_inline("\u{2005}\u{229C}\u{2005}"),
            var("b")
        ])
    );
    assert_eq!(to_typst("\\circledequal").as_deref(), Some("eq.o"));
    assert_eq!(to_typst("\\circledparallel").as_deref(), Some("parallel.o"));
}

// ----------------------------------------------------------------------------
// Typst backend — styled, text, accents
// ----------------------------------------------------------------------------

#[test]
fn typst_blackboard_bold() {
    assert_eq!(to_typst("\\mathbb{R}").as_deref(), Some("bb(R)"));
}

#[test]
fn typst_italic() {
    assert_eq!(to_typst("\\mathit{x}").as_deref(), Some("italic(x)"));
}

#[test]
fn typst_text_is_quoted_upright() {
    assert_eq!(
        to_typst("\\text{hello}").as_deref(),
        Some("upright(\"hello\")")
    );
}

#[test]
fn typst_single_token_accent_is_a_named_function() {
    assert_eq!(to_typst("\\hat{x}").as_deref(), Some("hat(x)"));
    assert_eq!(to_typst("\\widehat{x}").as_deref(), Some("hat(x)"));
}

#[test]
fn typst_double_dot_accent() {
    assert_eq!(to_typst("\\ddot{x}").as_deref(), Some("dot.double(x)"));
}

// ----------------------------------------------------------------------------
// Typst backend — the constructs that have no inline form but do translate
// ----------------------------------------------------------------------------

#[test]
fn typst_fraction_uses_slash_for_simple_operands() {
    assert_eq!(to_typst("\\frac{1}{2}").as_deref(), Some("1 / 2"));
}

#[test]
fn typst_radical() {
    assert_eq!(to_typst("\\sqrt{x}").as_deref(), Some("sqrt(x)"));
}

#[test]
fn typst_stacked_limits() {
    assert_eq!(
        to_typst("\\sum_{i=1}^{n}").as_deref(),
        Some("sum_(i = 1)^n")
    );
}

#[test]
fn typst_prime_on_a_limit_operator_stacks_in_display() {
    // In display context a prime over a limit operator sets as a stacked superscript above the
    // operator (`sum^(')`), where Typst's automatic limit placement raises it; a literal `'` would
    // instead set beside the operator. Repeated primes stack together.
    assert_eq!(to_typst_display("\\sum'").as_deref(), Some("sum^(')"));
    assert_eq!(to_typst_display("\\sum''").as_deref(), Some("sum^('')"));
    assert_eq!(to_typst_display("\\prod'").as_deref(), Some("product^(')"));
    assert_eq!(
        to_typst_display("\\bigcup'").as_deref(),
        Some("union.big^(')")
    );
}

#[test]
fn typst_prime_on_a_limit_function_stacks_in_display() {
    // The named limit functions (`\lim`, `\max`, …) stack their prime the same way the big
    // operators do.
    assert_eq!(to_typst_display("\\lim'").as_deref(), Some("lim^(')"));
    assert_eq!(to_typst_display("\\max'").as_deref(), Some("max^(')"));
    assert_eq!(to_typst_display("\\Pr'").as_deref(), Some("Pr^(')"));
}

#[test]
fn typst_prime_on_a_limit_operator_stays_literal_inline() {
    // Inline math sets the prime as a literal mark beside the operator, since Typst sets the
    // operator's scripts to the side inline regardless.
    assert_eq!(to_typst("\\sum'").as_deref(), Some("sum'"));
    assert_eq!(to_typst("\\lim'").as_deref(), Some("lim'"));
}

#[test]
fn typst_display_stacked_prime_follows_a_subscript() {
    // A subscript and a prime both present set the subscript first, then the prime as the stacked
    // superscript — `\sum_a'` and `\sum'_a` both give `sum_a^(')`.
    assert_eq!(to_typst_display("\\sum_a'").as_deref(), Some("sum_a^(')"));
    assert_eq!(to_typst_display("\\sum'_a").as_deref(), Some("sum_a^(')"));
}

#[test]
fn typst_display_prime_with_a_real_superscript_is_not_restacked() {
    // When the superscript slot already holds real content, the prime sets inside it the ordinary
    // way (`sum_a^b'`), and a prime followed by a separate superscript restarts the base after the
    // stacked prime (`sum^(') ""^b`).
    assert_eq!(to_typst_display("\\sum_a^b'").as_deref(), Some("sum_a^b'"));
    assert_eq!(
        to_typst_display("\\sum'^b").as_deref(),
        Some("sum^(') \"\"^b")
    );
}

#[test]
fn typst_display_prime_on_a_side_script_operator_stays_literal() {
    // The side-script large operators (`\int`, `\oint`, the circled/boxed big operators) set their
    // scripts beside themselves even in display, so their prime stays a literal mark.
    assert_eq!(to_typst_display("\\int'").as_deref(), Some("integral'"));
    assert_eq!(to_typst_display("\\bigoplus'").as_deref(), Some("xor.big'"));
    assert_eq!(to_typst_display("\\sin'").as_deref(), Some("sin'"));
}

#[test]
fn typst_display_stacked_prime_propagates_into_nested_content() {
    // The display context reaches a limit operator nested inside a fraction, radical, or
    // superscript, so its prime stacks there too.
    assert_eq!(
        to_typst_display("\\frac{\\sum'}{2}").as_deref(),
        Some("sum^(') / 2")
    );
    assert_eq!(
        to_typst_display("\\sqrt{\\sum'}").as_deref(),
        Some("sqrt(sum^('))")
    );
    assert_eq!(
        to_typst_display("x^{\\sum'}").as_deref(),
        Some("x^(sum^('))")
    );
}

#[test]
fn typst_binomial() {
    assert_eq!(to_typst("\\binom{n}{k}").as_deref(), Some("binom(n, k)"));
}

#[test]
fn typst_delimited_matrix() {
    assert_eq!(
        to_typst("\\begin{pmatrix}a&b\\\\c&d\\end{pmatrix}").as_deref(),
        Some("mat(delim: \"(\", a, b; c, d)")
    );
}

#[test]
fn typst_escapes_grouping_delimiters() {
    // Parentheses, commas and similar literals must be backslash-escaped in Typst math.
    assert_eq!(to_typst("f(x,y)").as_deref(), Some("f\\(x\\,y\\)"));
}

#[test]
fn typst_unknown_command_is_none() {
    assert_eq!(to_typst("\\nosuchcommand"), None);
}

// ----------------------------------------------------------------------------
// Typst backend — delimiters
// ----------------------------------------------------------------------------

#[test]
fn typst_paren_group_drops_the_left_right() {
    assert_eq!(to_typst("\\left( x \\right)").as_deref(), Some("(x)"));
}

#[test]
fn typst_bar_group_stretches_with_lr() {
    // A balanced single bar becomes a stretchy `lr(| .. |)` with bare bars inside.
    assert_eq!(to_typst("\\left| x \\right|").as_deref(), Some("lr(|x|)"));
}

#[test]
fn typst_half_open_bar_group_escapes_the_lone_bar() {
    // With one side absent (`.`) there is no `lr(..)`; the present bar is escaped.
    assert_eq!(to_typst("\\left. x \\right|").as_deref(), Some("x\\|"));
}

#[test]
fn typst_double_bar_pair_stretches_as_named_glyph() {
    // Both an auto-paired `\lVert .. \rVert` and an explicit `\left\Vert .. \right\Vert` are the
    // stretchy named double line.
    assert_eq!(
        to_typst("\\lVert x \\rVert").as_deref(),
        Some("lr(bar.v.double x bar.v.double)")
    );
    assert_eq!(
        to_typst("\\left\\Vert x \\right\\Vert").as_deref(),
        Some("lr(bar.v.double x bar.v.double)")
    );
}

#[test]
fn typst_left_parallel_is_the_literal_parallel_glyph() {
    // `\left\|` and `\left\lVert` are the parallel sign printed directly, not the stretchy double
    // line and not wrapped in `lr(..)`.
    assert_eq!(
        to_typst("\\left\\| x \\right\\|").as_deref(),
        Some("\u{2225}x\u{2225}")
    );
}

#[test]
fn typst_angle_and_floor_and_ceil_are_named() {
    assert_eq!(
        to_typst("\\langle x \\rangle").as_deref(),
        Some("chevron.l x chevron.r")
    );
    assert_eq!(
        to_typst("\\lfloor x \\rfloor").as_deref(),
        Some("floor.l x floor.r")
    );
    assert_eq!(
        to_typst("\\lceil x \\rceil").as_deref(),
        Some("ceil.l x ceil.r")
    );
}

#[test]
fn corner_delimiters_stretch_as_left_right_brackets() {
    // `\ulcorner`/`\urcorner` act as `\left`/`\right` quine-corner brackets, printing the raw corner
    // glyphs around their content.
    assert_eq!(
        to_typst("\\left\\ulcorner a \\right\\urcorner").as_deref(),
        Some("\u{231C}a\u{231D}")
    );
    // The corner glyph is fixed to its side regardless of which slot fills it.
    assert_eq!(
        to_typst("\\left\\urcorner a \\right\\ulcorner").as_deref(),
        Some("\u{231D}a\u{231C}")
    );
    // A one-sided pair drops the absent (`.`) side.
    assert_eq!(
        to_typst("\\left. a \\right\\urcorner").as_deref(),
        Some("a\u{231D}")
    );
    // Bare (no `\left`/`\right`), the corners are their named glyphs.
    assert_eq!(
        to_typst("\\ulcorner A \\urcorner").as_deref(),
        Some("corner.l.t A corner.r.t")
    );
}

#[test]
fn corner_delimiters_render_as_glyphs_in_inlines() {
    assert_eq!(
        to_inlines("\\left\\ulcorner a \\right\\urcorner"),
        Some(vec![
            str_inline("\u{231C}"),
            var("a"),
            str_inline("\u{231D}")
        ])
    );
}

// ----------------------------------------------------------------------------
// Typst backend — primes, scripts, modulo
// ----------------------------------------------------------------------------

#[test]
fn typst_prime_is_an_apostrophe() {
    assert_eq!(to_typst("f'").as_deref(), Some("f'"));
    assert_eq!(to_typst("f''").as_deref(), Some("f''"));
}

#[test]
fn typst_prime_with_explicit_superscript_separates_them() {
    // A prime followed by an explicit exponent inserts an empty base before the `^`.
    assert_eq!(to_typst("x'^2").as_deref(), Some("x' \"\"^2"));
}

#[test]
fn typst_leading_script_uses_an_empty_base() {
    assert_eq!(to_typst("_x").as_deref(), Some("\"\"_x"));
    assert_eq!(to_typst("^2").as_deref(), Some("\"\"^2"));
}

#[test]
fn typst_multi_digit_exponent_is_not_parenthesised() {
    // A run of digits is a single token; only a multi-atom script needs parentheses.
    assert_eq!(to_typst("x^{10}").as_deref(), Some("x^10"));
}

#[test]
fn typst_binary_mod_uses_medium_spaced_operator() {
    assert_eq!(to_typst("a \\bmod b").as_deref(), Some("a med mod med b"));
}

#[test]
fn typst_parenthesised_mod_wraps_the_argument() {
    assert_eq!(
        to_typst("a \\pmod{n}").as_deref(),
        Some("a med\\(mod med n\\)")
    );
}

// ----------------------------------------------------------------------------
// Typst backend — per-character styles and radical prefix
// ----------------------------------------------------------------------------

#[test]
fn typst_monospace_command_is_mono() {
    assert_eq!(to_typst("\\texttt{ab}").as_deref(), Some("mono(\"ab\")"));
}

#[test]
fn typst_set_minus_is_an_escaped_backslash() {
    assert_eq!(to_typst("a \\setminus b").as_deref(), Some("a\\\\b"));
}

#[test]
fn typst_surd_is_a_radical_over_the_next_group() {
    assert_eq!(to_typst("\\surd x").as_deref(), Some("sqrt(x)"));
}

// ----------------------------------------------------------------------------
// Robustness — malformed and adversarial input must never panic
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Both backends share one parse: a group flattens so a script binds the last atom
// ----------------------------------------------------------------------------

#[test]
fn group_flattens_and_script_binds_last_atom() {
    // {a+b}^2 == a + b^2: the brace group does not become an atom of its own.
    assert_eq!(to_inlines("{a+b}^2"), to_inlines("a+b^2"));
    assert_eq!(to_typst("{a+b}^2"), to_typst("a+b^2"));
}

// ----------------------------------------------------------------------------
// Numeric literals: adjacent digits group into one number atom
// ----------------------------------------------------------------------------

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
    // `1.5.5` greedily takes one interior point, then the next `.5` is a separate number atom; in a
    // styled group the two numbers stay apart (`1.5` then `.5`).
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
    // \mathtt{1a23} is one span over `1`, one over `a`, one over `23`.
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
    // Script and fraktur have no styled digits, so digits stay as plain ASCII beside the styled
    // letter (here the named script F, U+2131).
    assert_eq!(
        to_inlines("\\mathscr{F12}"),
        Some(vec![str_inline("\u{2131}12")])
    );
}

// ----------------------------------------------------------------------------
// \mathit slants every atom individually
// ----------------------------------------------------------------------------

#[test]
fn math_italic_styles_each_atom_including_digits() {
    // \mathit{a12} emphasises the letter and the number as separate italic runs.
    assert_eq!(
        to_inlines("\\mathit{a12}"),
        Some(vec![
            emph(vec![str_inline("a")]),
            emph(vec![str_inline("12")]),
        ])
    );
}

// ----------------------------------------------------------------------------
// \not-negated relations
// ----------------------------------------------------------------------------

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
    // \not\vdash has no precomposed form, so the base glyph (U+22A2) carries a combining long
    // solidus (U+0338).
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
    // With no dedicated negated token, Typst strikes the literal base glyph (U+22A2) rather than
    // its token name.
    assert_eq!(
        to_typst("\\not\\vdash").as_deref(),
        Some("\u{22A2}\u{0338}")
    );
}

#[test]
fn not_over_a_relation_command_without_a_precomposed_glyph_overlays_a_solidus() {
    // \not\sim, \not\vDash and \not\models are relation commands with no precomposed negated
    // codepoint, so the base glyph carries a combining long solidus (U+0338).
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
    // The bar commands \| and \Vert are ordinary-class double bars and \mid is a binary divides
    // bar; none has a struck form, so \not over them leaves the whole expression verbatim. This is
    // distinct from the literal pipe character \not| below, which does strike.
    assert_eq!(to_inlines("\\not\\|"), None);
    assert_eq!(to_inlines("\\not\\Vert"), None);
    assert_eq!(to_inlines("\\not\\mid"), None);
    assert_eq!(to_typst("\\not\\|"), None);
    assert_eq!(to_typst("\\not\\Vert"), None);
}

#[test]
fn not_over_the_literal_pipe_character_strikes_an_italic_bar() {
    // \not| (a literal pipe, not the \| command) strikes through as an ordinary italicised bar with
    // a combining long solidus.
    assert_eq!(to_inlines("\\not|"), Some(vec![var("|\u{0338}")]));
}

#[test]
fn not_over_a_delimiter_or_operator_command_falls_back_to_verbatim() {
    // Delimiter commands (\lvert, \langle), set/space operators (\setminus, \cup) and upright
    // letterlikes (\hbar, \nabla) carry no struck form, so \not over them is left verbatim.
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

// ----------------------------------------------------------------------------
// \limits / \nolimits override stacked-limit placement (inline backend only)
// ----------------------------------------------------------------------------

#[test]
fn nolimits_keeps_a_single_script_beside_a_limit_operator() {
    // A limit operator stacks only with both scripts; with one script it already lays out inline, so
    // \nolimits is a no-op here and the sum keeps its subscript beside it.
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

// ----------------------------------------------------------------------------
// Typst: a \left … \right group sets escaped punctuation off with spaces
// ----------------------------------------------------------------------------

#[test]
fn delimited_group_spaces_an_escaped_comma() {
    // Inside \left( … \right) the escaped comma is set off with spaces (`x \, y`); at the top level
    // it binds tightly (`x\,y`).
    assert_eq!(to_typst("x, y").as_deref(), Some("x\\,y"));
    assert_eq!(
        to_typst("\\left( x, y \\right)").as_deref(),
        Some("(x \\, y)")
    );
}

// ----------------------------------------------------------------------------
// \overbrace / \underbrace
// ----------------------------------------------------------------------------

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
    // When both scripts are present neither annotates the brace; both render as ordinary scripts.
    assert_eq!(
        to_typst("\\overbrace{x}^a_b").as_deref(),
        Some("overbrace(x)_b^a")
    );
}

// ----------------------------------------------------------------------------
// Typst: undelimited matrix renders as a bare alignment block
// ----------------------------------------------------------------------------

#[test]
fn undelimited_matrix_is_a_bare_alignment() {
    // \begin{matrix} … is an undelimited grid: cells joined by ` & `, rows by a trailing `\` and a
    // line break.
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

// ----------------------------------------------------------------------------
// A lone, trailing, or dangling backslash is not a complete expression
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Typst: the literal slash is escaped; a fraction keeps its bare divider
// ----------------------------------------------------------------------------

#[test]
fn literal_slash_is_escaped_in_typst() {
    assert_eq!(to_typst("a/b").as_deref(), Some("a\\/b"));
}

#[test]
fn fraction_divider_stays_bare_in_typst() {
    assert_eq!(to_typst("\\frac{a}{b}").as_deref(), Some("a / b"));
}

// ----------------------------------------------------------------------------
// Capital Greek lookalikes spell their name in Typst and map to the codepoint inline
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// A `\prime` superscript collapses to a literal apostrophe in Typst
// ----------------------------------------------------------------------------

#[test]
fn prime_superscript_collapses_to_apostrophe() {
    assert_eq!(to_typst("f^{\\prime}").as_deref(), Some("f'"));
}

// ----------------------------------------------------------------------------
// Named spacing macros
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// `\mod` and `\pod` modulo forms
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Math-class wrappers
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Colour and style switches are invisible
// ----------------------------------------------------------------------------

#[test]
fn color_is_stripped() {
    assert_eq!(to_typst("\\color{red}{x}").as_deref(), Some("x"));
    assert_eq!(to_inlines("\\color{red}{x}"), Some(vec![var("x")]));
}

#[test]
fn style_switch_is_dropped() {
    assert_eq!(to_typst("\\displaystyle x").as_deref(), Some("x"));
}

// ----------------------------------------------------------------------------
// Wrappers and operator-name handling in Typst
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Accents that fall back to the generic Typst accent
// ----------------------------------------------------------------------------

#[test]
fn triple_dot_accent_in_typst() {
    assert_eq!(
        to_typst("\\dddot{x}").as_deref(),
        Some("accent(x, \u{20DB})")
    );
}

// ----------------------------------------------------------------------------
// Two-dimensional stacking
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Grid environments
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Binomial-style stacks: infix operators and genfrac
// ----------------------------------------------------------------------------

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

#[test]
fn binom_has_no_inline_form() {
    assert_eq!(to_inlines("{n \\choose k}"), None);
}

// ----------------------------------------------------------------------------
// Extensible arrows
// ----------------------------------------------------------------------------

#[test]
fn extensible_arrows_carry_labels_as_scripts() {
    assert_eq!(to_typst("\\xrightarrow{f}").as_deref(), Some("arrow.r^f"));
    assert_eq!(to_typst("\\xleftarrow{g}").as_deref(), Some("arrow.l^g"));
}

#[test]
fn extensible_arrow_below_label_is_a_subscript() {
    assert_eq!(
        to_typst("\\xrightarrow[h]{f}").as_deref(),
        Some("arrow.r_h^f")
    );
}

#[test]
fn extensible_arrow_has_no_inline_form() {
    assert_eq!(to_inlines("\\xrightarrow{f}"), None);
}

// ----------------------------------------------------------------------------
// `smallsmile` / `smallfrown` relations
// ----------------------------------------------------------------------------

#[test]
fn small_smile_and_frown_are_relations() {
    assert_eq!(
        to_inlines("\\smallsmile"),
        Some(vec![str_inline("\u{2323}")])
    );
    assert_eq!(to_typst("\\smallsmile").as_deref(), Some("smile"));
    assert_eq!(to_typst("\\smallfrown").as_deref(), Some("frown"));
}

// ----------------------------------------------------------------------------
// Integrals, big operators, and miscellaneous glyphs
// ----------------------------------------------------------------------------

#[test]
fn volume_and_square_integrals_are_bare_glyphs() {
    assert_eq!(to_inlines("\\oiiint"), Some(vec![str_inline("\u{2230}")]));
    assert_eq!(to_inlines("\\sqint"), Some(vec![str_inline("\u{2A16}")]));
    assert_eq!(to_inlines("\\fint"), Some(vec![str_inline("\u{2A0F}")]));
    assert_eq!(to_inlines("\\awint"), Some(vec![str_inline("\u{2A11}")]));
}

#[test]
fn contour_integrals_have_directional_glyphs() {
    assert_eq!(
        to_inlines("\\varointclockwise"),
        Some(vec![str_inline("\u{2232}")])
    );
    assert_eq!(
        to_inlines("\\ointctrclockwise"),
        Some(vec![str_inline("\u{2233}")])
    );
}

#[test]
fn integral_glyphs_have_typst_names() {
    assert_eq!(to_typst("\\oiiint").as_deref(), Some("integral.vol"));
    assert_eq!(to_typst("\\sqint").as_deref(), Some("integral.square"));
    assert_eq!(to_typst("\\fint").as_deref(), Some("integral.slash"));
    assert_eq!(to_typst("\\awint").as_deref(), Some("integral.ccw"));
    assert_eq!(
        to_typst("\\varointclockwise").as_deref(),
        Some("integral.cont.cw")
    );
    assert_eq!(
        to_typst("\\ointctrclockwise").as_deref(),
        Some("integral.cont.ccw")
    );
}

#[test]
fn big_dotted_union_square_meet_and_product_are_operators() {
    assert_eq!(
        to_inlines("\\bigcupdot"),
        Some(vec![str_inline("\u{2A03}")])
    );
    assert_eq!(to_inlines("\\bigsqcap"), Some(vec![str_inline("\u{2A05}")]));
    assert_eq!(to_inlines("\\bigtimes"), Some(vec![str_inline("\u{2A09}")]));
    assert_eq!(to_typst("\\bigcupdot").as_deref(), Some("union.dot.big"));
    assert_eq!(to_typst("\\bigsqcap").as_deref(), Some("inter.sq.big"));
    assert_eq!(to_typst("\\bigtimes").as_deref(), Some("times.big"));
}

#[test]
fn restriction_harpoon_is_a_relation_glyph() {
    assert_eq!(
        to_inlines("\\restriction"),
        Some(vec![str_inline("\u{21BE}")])
    );
    assert_eq!(to_typst("\\restriction").as_deref(), Some("harpoon.tr"));
}

#[test]
fn maps_from_and_double_barred_arrows() {
    assert_eq!(to_inlines("\\mapsfrom"), Some(vec![str_inline("\u{21A4}")]));
    assert_eq!(to_inlines("\\Mapsto"), Some(vec![str_inline("\u{2907}")]));
    assert_eq!(to_inlines("\\Mapsfrom"), Some(vec![str_inline("\u{2906}")]));
    assert_eq!(
        to_inlines("\\longmapsfrom"),
        Some(vec![str_inline("\u{27FB}")])
    );
    assert_eq!(to_typst("\\mapsfrom").as_deref(), Some("arrow.l.bar"));
    assert_eq!(to_typst("\\Mapsto").as_deref(), Some("arrow.r.double.bar"));
    assert_eq!(
        to_typst("\\Mapsfrom").as_deref(),
        Some("arrow.l.double.bar")
    );
    assert_eq!(
        to_typst("\\longmapsfrom").as_deref(),
        Some("arrow.l.long.bar")
    );
}

#[test]
fn variant_suits_moons_and_sun_are_ordinary_glyphs() {
    assert_eq!(
        to_inlines("\\varclubsuit"),
        Some(vec![str_inline("\u{2667}")])
    );
    assert_eq!(
        to_inlines("\\varheartsuit"),
        Some(vec![str_inline("\u{2665}")])
    );
    assert_eq!(
        to_inlines("\\vardiamondsuit"),
        Some(vec![str_inline("\u{2666}")])
    );
    assert_eq!(
        to_inlines("\\varspadesuit"),
        Some(vec![str_inline("\u{2664}")])
    );
    assert_eq!(
        to_inlines("\\rightmoon"),
        Some(vec![str_inline("\u{263D}")])
    );
    assert_eq!(to_inlines("\\leftmoon"), Some(vec![str_inline("\u{263E}")]));
    assert_eq!(to_inlines("\\sun"), Some(vec![str_inline("\u{263C}")]));
}

// ----------------------------------------------------------------------------
// Colon-equality family and doubled relations
// ----------------------------------------------------------------------------

#[test]
fn colon_equality_relations_carry_their_named_typst_forms() {
    assert_eq!(to_inlines("\\coloneqq"), Some(vec![str_inline("\u{2254}")]));
    assert_eq!(to_inlines("\\eqqcolon"), Some(vec![str_inline("\u{2255}")]));
    assert_eq!(to_inlines("\\Coloneqq"), Some(vec![str_inline("\u{2A74}")]));
    assert_eq!(to_typst("\\coloneqq").as_deref(), Some("colon.eq"));
    assert_eq!(to_typst("\\eqqcolon").as_deref(), Some("eq.colon"));
    assert_eq!(to_typst("\\Coloneqq").as_deref(), Some("colon.double.eq"));
}

#[test]
fn not_an_element_reversed_is_a_named_relation() {
    assert_eq!(to_inlines("\\notni"), Some(vec![str_inline("\u{220C}")]));
    assert_eq!(to_typst("\\notni").as_deref(), Some("in.rev.not"));
}

#[test]
fn doubled_inequalities_use_equivalence_typst_names() {
    assert_eq!(to_inlines("\\leqq"), Some(vec![str_inline("\u{2266}")]));
    assert_eq!(to_inlines("\\geqq"), Some(vec![str_inline("\u{2267}")]));
    assert_eq!(to_inlines("\\lneqq"), Some(vec![str_inline("\u{2268}")]));
    assert_eq!(to_inlines("\\gneqq"), Some(vec![str_inline("\u{2269}")]));
    assert_eq!(to_typst("\\leqq").as_deref(), Some("lt.equiv"));
    assert_eq!(to_typst("\\geqq").as_deref(), Some("gt.equiv"));
    assert_eq!(to_typst("\\lneqq").as_deref(), Some("lt.nequiv"));
    assert_eq!(to_typst("\\gneqq").as_deref(), Some("gt.nequiv"));
}

#[test]
fn a_colon_equality_relation_takes_relation_spacing() {
    // U+2004 (three-per-em) flanks a relation between two operands.
    assert_eq!(
        to_inlines("a\\coloneqq b"),
        Some(vec![
            var("a"),
            str_inline("\u{2004}\u{2254}\u{2004}"),
            var("b"),
        ])
    );
}

// ----------------------------------------------------------------------------
// Large operators: scripts sit beside the symbol, not stacked
// ----------------------------------------------------------------------------

#[test]
fn big_circled_operators_place_scripts_to_the_side() {
    // The circled-sum operator does not stack its limits; the scripts follow it
    // as ordinary sub/superscripts.
    assert_eq!(
        to_inlines("\\bigoplus_{x}^{y}"),
        Some(vec![
            str_inline("\u{2A01}"),
            Inline::Subscript(vec![var("x")]),
            Inline::Superscript(vec![var("y")]),
        ])
    );
    assert_eq!(
        to_typst("\\bigoplus_{x}^{y}").as_deref(),
        Some("xor.big_x^y")
    );
}

#[test]
fn a_large_operator_does_not_poison_a_following_relation() {
    // After a side-script operator, a relation keeps its own spacing.
    assert_eq!(
        to_inlines("\\bigoplus a = b"),
        Some(vec![
            str_inline("\u{2A01}"),
            var("a"),
            str_inline("\u{2004}=\u{2004}"),
            var("b"),
        ])
    );
}

#[test]
fn summation_stacks_its_limits_in_typst() {
    // A genuine limit operator stacks: its script becomes a `limits` subscript.
    assert_eq!(to_typst("\\sum_{i}").as_deref(), Some("sum_i"));
}

// ----------------------------------------------------------------------------
// Empty-base consecutive scripts
// ----------------------------------------------------------------------------

#[test]
fn doubled_superscript_nests_through_an_empty_base() {
    // An implicit empty base renders as an empty string literal in the script.
    assert_eq!(to_typst("a^^b").as_deref(), Some("a^(\"\"^b)"));
    assert_eq!(to_typst("a__b").as_deref(), Some("a_(\"\"_b)"));
}

#[test]
fn tripled_superscript_nests_three_deep() {
    assert_eq!(to_typst("a^^^b").as_deref(), Some("a^(\"\"^(\"\"^b))"));
    assert_eq!(to_typst("a___b").as_deref(), Some("a_(\"\"_(\"\"_b))"));
}

#[test]
fn a_leading_doubled_script_has_an_empty_outer_base() {
    assert_eq!(to_typst("^^a").as_deref(), Some("\"\"^(\"\"^a)"));
    assert_eq!(to_typst("__b").as_deref(), Some("\"\"_(\"\"_b)"));
}

#[test]
fn an_explicit_empty_brace_base_renders_as_a_zero_width_space() {
    // An explicit `{}` is a real (empty) group, distinct from an implicit base.
    assert_eq!(to_typst("a^{}^c").as_deref(), Some("a^(zws^c)"));
}

#[test]
fn a_real_script_then_a_doubled_script_nests_inside() {
    assert_eq!(to_typst("a^2^^b").as_deref(), Some("a^(2^(\"\"^b))"));
}

#[test]
fn explicit_empty_braces_keep_their_zero_width_scripts() {
    assert_eq!(to_typst("a^{}^{}").as_deref(), Some("a^(zws^())"));
    assert_eq!(to_typst("x_{}_{}").as_deref(), Some("x_(zws_())"));
}

#[test]
fn a_pathologically_deep_script_chain_falls_back_to_verbatim() {
    // The depth guard keeps a runaway chain from recursing without bound; once it
    // trips, the whole expression has no structured form.
    let deep = "a".to_string() + &"^".repeat(70) + "b";
    assert_eq!(to_typst(&deep), None);
    assert_eq!(to_inlines(&deep), None);
}

// ----------------------------------------------------------------------------
// TeX-active characters: bare `#`, `&`, `%` are not convertible
// ----------------------------------------------------------------------------

#[test]
fn a_bare_active_character_has_no_structured_form() {
    assert_eq!(to_inlines("a#b"), None);
    assert_eq!(to_inlines("a&b"), None);
    assert_eq!(to_inlines("a%b"), None);
    assert_eq!(to_typst("a#b"), None);
    assert_eq!(to_typst("a&b"), None);
    assert_eq!(to_typst("a%b"), None);
}

#[test]
fn an_escaped_hash_is_an_ordinary_literal() {
    assert_eq!(
        to_inlines("a\\#b"),
        Some(vec![var("a"), str_inline("#"), var("b")])
    );
    assert_eq!(to_typst("a\\#b").as_deref(), Some("a\\#b"));
}

#[test]
fn an_alignment_tab_still_separates_cells() {
    // `&` is structural inside an alignment, where it keeps working.
    assert_eq!(
        to_typst("\\begin{cases} a & b \\\\ c & d \\end{cases}").as_deref(),
        Some("cases(delim: \"{\", a & b, c & d)")
    );
}

// ----------------------------------------------------------------------------
// Empty styled-alphabet and text wrappers emit nothing inline
// ----------------------------------------------------------------------------

#[test]
fn an_empty_blackboard_wrapper_contributes_no_inline() {
    assert_eq!(to_inlines("\\mathbb{}"), Some(Vec::new()));
    assert_eq!(to_inlines("a\\mathbb{}b"), Some(vec![var("a"), var("b")]));
}

#[test]
fn empty_text_wrappers_contribute_no_inline() {
    assert_eq!(to_inlines("\\text{}"), Some(Vec::new()));
    assert_eq!(to_inlines("\\textbf{}"), Some(Vec::new()));
    assert_eq!(to_inlines("\\mathbf{}"), Some(Vec::new()));
}

// ----------------------------------------------------------------------------
// Negated relations, the colon-equals aliases, the dots family, and the
// double-stroke angle/brace delimiters
// ----------------------------------------------------------------------------

/// A relation between two variables: the glyph framed by three-per-em spaces. Returns `Option` so it
/// compares directly against `to_inlines`.
#[allow(clippy::unnecessary_wraps)]
fn rel_between(glyph: &str) -> Option<Vec<Inline>> {
    Some(vec![
        var("a"),
        str_inline(&format!("\u{2004}{glyph}\u{2004}")),
        var("b"),
    ])
}

#[test]
fn negated_relations_use_their_precomposed_glyphs() {
    assert_eq!(to_inlines("a \\nin b"), rel_between("\u{2209}"));
    assert_eq!(to_inlines("a \\napprox b"), rel_between("\u{2249}"));
    assert_eq!(to_inlines("a \\nasymp b"), rel_between("\u{226D}"));
    assert_eq!(to_inlines("a \\nequiv b"), rel_between("\u{2262}"));
    assert_eq!(to_inlines("a \\nVdash b"), rel_between("\u{22AE}"));
    assert_eq!(to_inlines("a \\ngtrsim b"), rel_between("\u{2275}"));
    assert_eq!(to_inlines("a \\nlesssim b"), rel_between("\u{2274}"));
    assert_eq!(to_inlines("a \\ntrianglelefteq b"), rel_between("\u{22EC}"));
    assert_eq!(
        to_inlines("a \\ntrianglerighteq b"),
        rel_between("\u{22ED}")
    );
    assert_eq!(to_inlines("a \\lnapprox b"), rel_between("\u{2A89}"));
    assert_eq!(to_inlines("a \\gnapprox b"), rel_between("\u{2A8A}"));
}

#[test]
fn negated_relations_have_named_typst_tokens() {
    assert_eq!(to_typst("a \\nin b").as_deref(), Some("a in.not b"));
    assert_eq!(to_typst("a \\napprox b").as_deref(), Some("a approx.not b"));
    assert_eq!(to_typst("a \\nasymp b").as_deref(), Some("a asymp.not b"));
    assert_eq!(to_typst("a \\nequiv b").as_deref(), Some("a equiv.not b"));
    assert_eq!(to_typst("a \\nVdash b").as_deref(), Some("a forces.not b"));
    assert_eq!(
        to_typst("a \\ngtrsim b").as_deref(),
        Some("a gt.tilde.not b")
    );
    assert_eq!(
        to_typst("a \\nlesssim b").as_deref(),
        Some("a lt.tilde.not b")
    );
    assert_eq!(
        to_typst("a \\ntrianglelefteq b").as_deref(),
        Some("a lt.tri.eq.not b")
    );
    assert_eq!(
        to_typst("a \\ntrianglerighteq b").as_deref(),
        Some("a gt.tri.eq.not b")
    );
    assert_eq!(
        to_typst("a \\lnapprox b").as_deref(),
        Some("a lt.napprox b")
    );
    assert_eq!(
        to_typst("a \\gnapprox b").as_deref(),
        Some("a gt.napprox b")
    );
}

#[test]
fn single_letter_colon_equals_aliases_share_the_double_letter_glyphs() {
    assert_eq!(to_inlines("a \\coloneq b"), rel_between("\u{2254}"));
    assert_eq!(to_inlines("a \\Coloneq b"), rel_between("\u{2A74}"));
    assert_eq!(to_inlines("a \\eqcolon b"), rel_between("\u{2255}"));
    // The single- and double-letter spellings render identically.
    assert_eq!(to_inlines("a \\coloneq b"), to_inlines("a \\coloneqq b"));
    assert_eq!(to_inlines("a \\Coloneq b"), to_inlines("a \\Coloneqq b"));
}

#[test]
fn colon_equals_aliases_have_named_typst_tokens() {
    assert_eq!(to_typst("a \\coloneq b").as_deref(), Some("a colon.eq b"));
    assert_eq!(
        to_typst("a \\Coloneq b").as_deref(),
        Some("a colon.double.eq b")
    );
    assert_eq!(to_typst("a \\eqcolon b").as_deref(), Some("a eq.colon b"));
}

#[test]
fn the_dots_family_maps_to_its_horizontal_glyphs() {
    // \dotsm and \dotsi are the centered ellipsis; \dotso is the baseline ellipsis.
    assert_eq!(
        to_inlines("a \\dotsm b"),
        Some(vec![var("a"), str_inline("\u{22EF}"), var("b")])
    );
    assert_eq!(
        to_inlines("a \\dotsi b"),
        Some(vec![var("a"), str_inline("\u{22EF}"), var("b")])
    );
    assert_eq!(
        to_inlines("a \\dotso b"),
        Some(vec![var("a"), str_inline("\u{2026}"), var("b")])
    );
}

#[test]
fn the_dots_family_has_named_typst_tokens() {
    assert_eq!(to_typst("a \\dotsm b").as_deref(), Some("a dots.h.c b"));
    assert_eq!(to_typst("a \\dotsi b").as_deref(), Some("a dots.h.c b"));
    assert_eq!(to_typst("a \\dotso b").as_deref(), Some("a dots.h b"));
}

#[test]
fn double_stroke_angle_and_brace_delimiters_pair_as_glyphs() {
    assert_eq!(
        to_inlines("\\lAngle x \\rAngle"),
        Some(vec![
            str_inline("\u{27EA}"),
            var("x"),
            str_inline("\u{27EB}")
        ])
    );
    assert_eq!(
        to_inlines("\\lBrace x \\rBrace"),
        Some(vec![
            str_inline("\u{2983}"),
            var("x"),
            str_inline("\u{2984}")
        ])
    );
}

#[test]
fn double_stroke_delimiters_have_named_typst_tokens() {
    assert_eq!(
        to_typst("\\lAngle x \\rAngle").as_deref(),
        Some("chevron.l.double x chevron.r.double")
    );
    assert_eq!(
        to_typst("\\lBrace x \\rBrace").as_deref(),
        Some("brace.l.stroked x brace.r.stroked")
    );
}

// ----------------------------------------------------------------------------
// Accents convert only over a single letter-class base
// ----------------------------------------------------------------------------

#[test]
fn accent_over_a_letter_applies_a_combining_mark() {
    assert_eq!(to_inlines("\\hat{R}"), Some(vec![var("R\u{302}")]));
    // A Greek letter is a letter-class base.
    assert_eq!(
        to_inlines("\\hat{\\alpha}"),
        Some(vec![var("\u{3B1}\u{302}")])
    );
    // A named single-glyph italic letter is a letter-class base.
    assert_eq!(
        to_inlines("\\hat{\\ell}"),
        Some(vec![var("\u{2113}\u{302}")])
    );
    // An accent followed by a script still converts.
    assert_eq!(
        to_inlines("\\hat{x}_i"),
        Some(vec![var("x\u{302}"), Inline::Subscript(vec![var("i")])])
    );
}

#[test]
fn accent_over_a_non_letter_base_has_no_inline_form() {
    // A digit, operator, relation, delimiter, large operator, or symbol base keeps
    // the whole expression verbatim rather than stacking a combining mark on it.
    assert_eq!(to_inlines("\\hat{1}"), None);
    assert_eq!(to_inlines("\\hat{+}"), None);
    assert_eq!(to_inlines("\\hat{(}"), None);
    assert_eq!(to_inlines("\\hat{=}"), None);
    assert_eq!(to_inlines("\\hat{\\sum}"), None);
    assert_eq!(to_inlines("\\hat{\\nabla}"), None);
    assert_eq!(to_inlines("\\hat{\\bigoplus}"), None);
    assert_eq!(to_inlines("\\hat{\\dotplus}"), None);
    assert_eq!(to_inlines("\\hat{\\oiint}"), None);
    assert_eq!(to_inlines("\\hat{\\mathbb{R}}"), None);
}

// ----------------------------------------------------------------------------
// The relation-sizing m-suffix delimiter variants are kept verbatim
// ----------------------------------------------------------------------------

#[test]
fn relation_sizing_delimiter_variants_have_no_inline_form() {
    assert_eq!(to_inlines("a \\bigm| b"), None);
    assert_eq!(to_inlines("a \\Bigm\\| b"), None);
    assert_eq!(to_inlines("a \\biggm< b"), None);
    assert_eq!(to_inlines("a \\Biggm> b"), None);
}

#[test]
fn relation_sizing_delimiter_variants_are_not_translated_to_typst() {
    assert_eq!(to_typst("a \\bigm| b"), None);
    assert_eq!(to_typst("a \\Bigm\\| b"), None);
    assert_eq!(to_typst("a \\biggm< b"), None);
    assert_eq!(to_typst("a \\Biggm> b"), None);
}

#[test]
fn non_m_delimiter_sizes_still_translate() {
    // The plain big-delimiter forms are unaffected by excluding the m-suffix variants.
    assert!(to_typst("a \\big| b").is_some());
    assert!(to_typst("a \\bigl( b").is_some());
    assert!(to_typst("a \\bigr) b").is_some());
    assert!(to_typst("a \\Bigg| b").is_some());
}

// ----------------------------------------------------------------------------
// Restart-base multi-script runs render subscript-before-superscript
// ----------------------------------------------------------------------------

#[test]
fn a_restart_run_renders_sub_before_sup_in_typst() {
    // After a base already carries a sub and a sup, the next pair starts a fresh
    // empty base; that restart run is reordered sub-then-sup, like the first run.
    assert_eq!(to_typst("a^a_b^c_d").as_deref(), Some("a_b^a \"\"_d^c"));
    assert_eq!(to_typst("a^1_2^3_4^5").as_deref(), Some("a_2^1 \"\"_4^3^5"));
    assert_eq!(
        to_typst("\\sum^1_2^3_4").as_deref(),
        Some("sum_2^1 \"\"_4^3")
    );
}

#[test]
fn a_restart_run_renders_sub_before_sup_in_the_inline_tree() {
    // a^1_2^3_4: the first run is sub 2 then sup 1; the restart run is sub 4 then sup 3.
    assert_eq!(
        to_inlines("a^1_2^3_4"),
        Some(vec![
            var("a"),
            Inline::Subscript(vec![str_inline("2")]),
            Inline::Superscript(vec![str_inline("1")]),
            Inline::Subscript(vec![str_inline("4")]),
            Inline::Superscript(vec![str_inline("3")]),
        ])
    );
}

#[test]
fn single_run_and_sub_first_script_orders_do_not_regress() {
    assert_eq!(to_typst("a_1^2_3").as_deref(), Some("a_1^2 \"\"_3"));
    assert_eq!(to_typst("a^b_c").as_deref(), Some("a_c^b"));
}

// ----------------------------------------------------------------------------
// Fixed-size delimiters (`\big`, `\Big`, `\bigg`, `\Bigg`, with `l`/`r` variants):
// argument-form completeness
// ----------------------------------------------------------------------------

#[test]
fn fixed_size_wrapper_sizes_the_null_delimiter_as_a_literal_period() {
    // `\big.` is a sized null delimiter, which prints as a literal `.` bound tightly to
    // its neighbours (ordinary class), across every scale and the l/r variants.
    assert_eq!(to_inlines("\\big."), Some(vec![str_inline(".")]));
    assert_eq!(to_inlines("\\Bigl."), Some(vec![str_inline(".")]));
    assert_eq!(to_inlines("\\bigg."), Some(vec![str_inline(".")]));
    assert_eq!(
        to_inlines("\\big. x"),
        Some(vec![str_inline("."), var("x")])
    );
    assert_eq!(to_inlines("( \\big. )"), Some(vec![str_inline("(.)")]));
    assert_eq!(
        to_typst("\\big.").as_deref(),
        Some("#scale(x: 120%, y: 120%)[.]")
    );
}

#[test]
fn fixed_size_wrapper_treats_angle_brackets_as_tight_delimiters() {
    // `<` and `>` after a sizing wrapper are open/close delimiters, not relations, so
    // they take tight (ordinary) spacing rather than the spacing of a relation.
    assert_eq!(
        to_inlines("a \\big< b"),
        Some(vec![var("a"), str_inline("<"), var("b")])
    );
    assert_eq!(
        to_inlines("a \\big> b"),
        Some(vec![var("a"), str_inline(">"), var("b")])
    );
    assert_eq!(
        to_typst("a \\big< b").as_deref(),
        Some("a #scale(x: 120%, y: 120%)[<] b")
    );
    assert_eq!(
        to_typst("a \\big> b").as_deref(),
        Some("a #scale(x: 120%, y: 120%)[>] b")
    );
}

#[test]
fn fixed_size_wrapper_sizes_an_ordinary_or_relation_character_tightly() {
    // The wrapper accepts ordinary, relation, and punctuation marks too, sizing each as a
    // literal glyph with tight (ordinary) spacing — `\big +` is a tight `+`, not a binary
    // operator with its usual surrounding space.
    assert_eq!(to_inlines("\\big +"), Some(vec![str_inline("+")]));
    assert_eq!(to_inlines("\\big-"), Some(vec![str_inline("\u{2212}")]));
    assert_eq!(to_inlines("\\bigl +"), Some(vec![str_inline("+")]));
    assert_eq!(to_inlines("\\big ="), Some(vec![str_inline("=")]));
    assert_eq!(
        to_inlines("a \\big + b"),
        Some(vec![var("a"), str_inline("+"), var("b")])
    );
    assert_eq!(
        to_typst("\\big +").as_deref(),
        Some("#scale(x: 120%, y: 120%)[+]")
    );
}

#[test]
fn fixed_size_wrapper_sizes_a_prime_run_as_one_multi_prime_delimiter() {
    // A run of primes after the wrapper is one sized multi-prime glyph, not a prime
    // carrying a prime as a script.
    assert_eq!(to_inlines("\\big '"), Some(vec![str_inline("\u{2032}")]));
    assert_eq!(to_inlines("\\big ''"), Some(vec![str_inline("\u{2033}")]));
    assert_eq!(to_inlines("\\big '''"), Some(vec![str_inline("\u{2034}")]));
    assert_eq!(to_inlines("\\big ''''"), Some(vec![str_inline("\u{2057}")]));
    assert_eq!(
        to_typst("\\big ''").as_deref(),
        Some("#scale(x: 120%, y: 120%)['']")
    );
}

#[test]
fn fixed_size_wrapper_sizes_the_colon_equals_digraph_as_one_relation() {
    // `:=` after the wrapper is one sized relation digraph, not a sized `:` followed by a
    // loose `=`.
    assert_eq!(to_inlines("\\big :="), Some(vec![str_inline(":=")]));
    assert_eq!(
        to_inlines("a \\big := b"),
        Some(vec![var("a"), str_inline(":="), var("b")])
    );
    assert_eq!(
        to_typst("\\big :=").as_deref(),
        Some("#scale(x: 120%, y: 120%)[:=]")
    );
    assert_eq!(
        to_typst("a \\big := b").as_deref(),
        Some("a #scale(x: 120%, y: 120%)[:=] b")
    );
}

#[test]
fn fixed_size_wrapper_rejects_a_letter_digit_or_arrow_follower() {
    // A letter, a digit, and a stretchy arrow are not delimiters the wrapper sizes, so the
    // whole expression keeps its source form (no inline or typst rendering).
    assert_eq!(to_inlines("\\big a"), None);
    assert_eq!(to_inlines("\\big 1"), None);
    assert_eq!(to_inlines("\\big\\uparrow"), None);
    assert_eq!(to_typst("\\big a"), None);
    assert_eq!(to_typst("\\big 1"), None);
    assert_eq!(to_typst("\\big\\uparrow"), None);
}

#[test]
fn fixed_size_scale_matrix_covers_every_width_and_variant() {
    // Each width sizes to a fixed percentage; the l/r variants share their base width.
    assert_eq!(
        to_typst("\\big(").as_deref(),
        Some("#scale(x: 120%, y: 120%)[\\(]")
    );
    assert_eq!(
        to_typst("\\Big(").as_deref(),
        Some("#scale(x: 180%, y: 180%)[\\(]")
    );
    assert_eq!(
        to_typst("\\bigg(").as_deref(),
        Some("#scale(x: 240%, y: 240%)[\\(]")
    );
    assert_eq!(
        to_typst("\\Bigg(").as_deref(),
        Some("#scale(x: 300%, y: 300%)[\\(]")
    );
    assert_eq!(
        to_typst("\\bigr(").as_deref(),
        Some("#scale(x: 120%, y: 120%)[\\(]")
    );
    assert_eq!(
        to_typst("\\Bigl(").as_deref(),
        Some("#scale(x: 180%, y: 180%)[\\(]")
    );
}

// ----------------------------------------------------------------------------
// Typst spacing — the `:=` relation digraph and scripted escaped close-delimiters
// ----------------------------------------------------------------------------

#[test]
fn typst_colon_equals_digraph_is_tight() {
    // A colon immediately followed by an equals is the `:=` relation digraph and renders
    // with no space between the two characters; a colon with a space, or other colon
    // sequences, keep their loose spacing.
    assert_eq!(to_typst(":=").as_deref(), Some(":="));
    assert_eq!(to_typst(": =").as_deref(), Some(": ="));
    assert_eq!(to_typst("=:").as_deref(), Some("= :"));
    assert_eq!(to_typst("::=").as_deref(), Some(": :="));
    assert_eq!(to_typst("::").as_deref(), Some(": :"));
    assert_eq!(to_typst("a : b").as_deref(), Some("a : b"));
    assert_eq!(to_typst("a := b").as_deref(), Some("a := b"));
}

#[test]
fn typst_colon_equals_digraph_is_tight_in_the_inline_tree() {
    // The same digraph prints as the two literal characters as one relation-class unit.
    assert_eq!(to_inlines(":="), Some(vec![str_inline(":=")]));
    assert_eq!(
        to_inlines("a := b"),
        Some(vec![var("a"), str_inline("\u{2004}:=\u{2004}"), var("b")])
    );
}

#[test]
fn typst_scripted_escaped_close_delimiter_has_no_leading_space() {
    // A literal `)`/`]`/`}` rendered as an escaped typst delimiter is tight when it is the
    // base of a script, so no stray space appears before the script.
    assert_eq!(to_typst("(a)").as_deref(), Some("\\(a\\)"));
    assert_eq!(to_typst("[a]").as_deref(), Some("\\[a\\]"));
    assert_eq!(to_typst("(a)^2").as_deref(), Some("\\(a\\)^2"));
    assert_eq!(to_typst("(a)_n").as_deref(), Some("\\(a\\)_n"));
    assert_eq!(to_typst("((a))^2").as_deref(), Some("\\(\\(a\\)\\)^2"));
    assert_eq!(to_typst("\\left(a\\right)^2").as_deref(), Some("(a)^2"));
}

// ----------------------------------------------------------------------------
// Text-mode escapes inside a wrapper (A1)
// ----------------------------------------------------------------------------

#[test]
fn text_escape_unescapes_to_the_literal_character() {
    // Each backslash-escape inside a text wrapper becomes its literal character.
    assert_eq!(to_inlines("\\text{a\\%b}"), Some(vec![str_inline("a%b")]));
    assert_eq!(to_inlines("\\text{a\\&b}"), Some(vec![str_inline("a&b")]));
    assert_eq!(to_inlines("\\text{a\\_b}"), Some(vec![str_inline("a_b")]));
    assert_eq!(to_inlines("\\text{a\\$b}"), Some(vec![str_inline("a$b")]));
    assert_eq!(to_inlines("\\text{a\\#b}"), Some(vec![str_inline("a#b")]));
    assert_eq!(
        to_inlines("\\text{a\\{b\\}c}"),
        Some(vec![str_inline("a{b}c")])
    );
}

#[test]
fn text_escape_preserves_the_wrapper_formatting() {
    // The wrapper's own emphasis still applies around the unescaped run.
    assert_eq!(
        to_inlines("\\textbf{a\\%b}"),
        Some(vec![Inline::Strong(vec![str_inline("a%b")])])
    );
    assert_eq!(
        to_inlines("\\textit{a\\%b}"),
        Some(vec![Inline::Emph(vec![str_inline("a%b")])])
    );
    assert_eq!(
        to_inlines("\\texttt{a\\%b}"),
        Some(vec![Inline::Code(Box::default(), "a%b".to_string().into())])
    );
}

#[test]
fn text_escape_swallows_a_following_space() {
    // A control symbol absorbs one following run of spaces, as in TeX.
    assert_eq!(
        to_inlines("\\text{50\\% off}"),
        Some(vec![str_inline("50%off")])
    );
    assert_eq!(to_inlines("\\text{a\\% b}"), Some(vec![str_inline("a%b")]));
}

#[test]
fn text_escape_in_typst_stays_inside_the_quoted_run() {
    assert_eq!(
        to_typst("\\text{a\\%b}").as_deref(),
        Some("upright(\"a%b\")")
    );
    assert_eq!(
        to_typst("\\textbf{a\\#b}").as_deref(),
        Some("bold(\"a#b\")")
    );
    // A literal double-quote inside the run is escaped for the Typst string.
    assert_eq!(
        to_typst("\\text{a\"b}").as_deref(),
        Some("upright(\"a\\\"b\")")
    );
}

// ----------------------------------------------------------------------------
// Text-mode spacing inside a wrapper (A1)
// ----------------------------------------------------------------------------

#[test]
fn text_spacing_emits_its_width_codepoint() {
    assert_eq!(
        to_inlines("\\text{a\\,b}"),
        Some(vec![str_inline("a\u{2006}b")])
    );
    assert_eq!(
        to_inlines("\\text{a\\;b}"),
        Some(vec![str_inline("a\u{2005}b")])
    );
    assert_eq!(
        to_inlines("\\text{a\\:b}"),
        Some(vec![str_inline("a\u{00A0}b")])
    );
    assert_eq!(
        to_inlines("\\text{a\\!b}"),
        Some(vec![str_inline("a\u{200A}b")])
    );
    // An escaped space is an ordinary space inside a text wrapper.
    assert_eq!(to_inlines("\\text{a\\ b}"), Some(vec![str_inline("a b")]));
}

#[test]
fn text_spacing_splits_the_wrapper_formatting_per_run() {
    // The wrapper applies to each side of the spacing independently; the spacing is a bare glyph.
    assert_eq!(
        to_inlines("\\textbf{a\\,b}"),
        Some(vec![
            Inline::Strong(vec![str_inline("a")]),
            str_inline("\u{2006}"),
            Inline::Strong(vec![str_inline("b")]),
        ])
    );
}

#[test]
fn text_spacing_splits_the_typst_run() {
    assert_eq!(
        to_typst("\\text{a\\,b}").as_deref(),
        Some("upright(\"a\") thin upright(\"b\")")
    );
    assert_eq!(
        to_typst("\\text{a\\;b}").as_deref(),
        Some("upright(\"a\") #h(0em) upright(\"b\")")
    );
    assert_eq!(
        to_typst("\\textbf{a\\:b}").as_deref(),
        Some("bold(\"a\") med bold(\"b\")")
    );
}

// ----------------------------------------------------------------------------
// `\operatorname` is math content set upright (A1)
// ----------------------------------------------------------------------------

#[test]
fn operatorname_drops_literal_spaces_and_folds_spacing() {
    // An operator name is math mode: inter-token spaces drop, an explicit spacing folds in. As a
    // function-like operator it also carries its own trailing thin space (U+2006).
    assert_eq!(
        to_inlines("\\operatorname{a b}"),
        Some(vec![str_inline("ab\u{2006}")])
    );
    assert_eq!(
        to_inlines("\\operatorname{a\\,b}"),
        Some(vec![str_inline("a\u{2006}b\u{2006}")])
    );
    assert_eq!(
        to_typst("\\operatorname{a\\,b}").as_deref(),
        Some("\"a\u{2006}b\"")
    );
}

// ----------------------------------------------------------------------------
// `~` inside a text wrapper is a non-breaking space (A2)
// ----------------------------------------------------------------------------

#[test]
fn tilde_inside_text_is_a_non_breaking_space() {
    assert_eq!(
        to_inlines("\\text{a~b}"),
        Some(vec![str_inline("a\u{00A0}b")])
    );
    assert_eq!(
        to_typst("\\text{a~b}").as_deref(),
        Some("upright(\"a\u{00A0}b\")")
    );
}

// ----------------------------------------------------------------------------
// Re-entrant math inside a text wrapper (A3)
// ----------------------------------------------------------------------------

#[test]
fn math_inside_text_renders_as_math() {
    // A `$…$` switches back to math: the inner `x` is an italic variable, not roman text.
    assert_eq!(to_inlines("\\text{$x$}"), Some(vec![var("x")]));
    assert_eq!(to_typst("\\text{$x$}").as_deref(), Some("x"));
}

#[test]
fn math_inside_text_ignores_the_wrapper_formatting() {
    // The bold wrapper does not apply to the spliced math: `x` stays an italic variable.
    assert_eq!(to_inlines("\\textbf{$x$}"), Some(vec![var("x")]));
}

#[test]
fn text_around_inner_math_keeps_its_run() {
    assert_eq!(
        to_typst("\\text{a $x+1$ b}").as_deref(),
        Some("upright(\"a \") x + 1 upright(\" b\")")
    );
}

#[test]
fn unbalanced_dollar_in_text_falls_back() {
    // A `$` with no closing `$` cannot be re-entered, so the whole node falls back to verbatim.
    assert_eq!(to_inlines("\\text{$x}"), None);
    assert_eq!(to_typst("\\text{a$b}"), None);
}

#[test]
fn paren_delimiters_in_text_switch_back_to_math() {
    // `\(…\)` inside a text wrapper re-enters math, exactly like `$…$`: the inner `x` is an italic
    // variable, and the literal run around it stays upright.
    assert_eq!(to_inlines("\\text{$x$}"), to_inlines("\\text{\\(x\\)}"));
    assert_eq!(
        to_typst("\\text{ if \\(x>0\\)}").as_deref(),
        Some("upright(\" if \") x > 0")
    );
}

#[test]
fn bracket_delimiters_in_text_switch_back_to_math() {
    // `\[…\]` re-enters math the same way, splicing the math between the literal runs.
    assert_eq!(
        to_typst("\\text{x \\[y\\] z}").as_deref(),
        Some("upright(\"x \") y upright(\" z\")")
    );
}

#[test]
fn unbalanced_math_delimiter_in_text_falls_back() {
    // A `\(` with no closing `\)` cannot be re-entered, so the whole node falls back to verbatim.
    assert_eq!(to_inlines("\\text{\\(x}"), None);
    assert_eq!(to_typst("\\text{a\\[b}"), None);
}

// ----------------------------------------------------------------------------
// Styled-alphabet aliases (A4)
// ----------------------------------------------------------------------------

#[test]
fn bold_italic_alias_wraps_strong_emph() {
    assert_eq!(
        to_inlines("\\mathbfit{x}"),
        Some(vec![Inline::Strong(vec![Inline::Emph(vec![str_inline(
            "x"
        )])])])
    );
    assert_eq!(
        to_typst("\\mathbfit{x}").as_deref(),
        Some("bold(italic(x))")
    );
}

#[test]
fn bold_script_alias_uses_the_bold_script_codepoint() {
    // `\mathbfcal{L}` is the bold-script L (U+1D4DB), wrapped in Strong.
    assert_eq!(
        to_inlines("\\mathbfcal{L}"),
        Some(vec![Inline::Strong(vec![str_inline("\u{1D4DB}")])])
    );
    assert_eq!(to_typst("\\mathbfcal{L}").as_deref(), Some("bold(cal(L))"));
}

#[test]
fn bold_fraktur_alias_uses_the_bold_fraktur_codepoint() {
    // `\mathbffrak{g}` is the bold-fraktur g (U+1D58C), wrapped in Strong.
    assert_eq!(
        to_inlines("\\mathbffrak{g}"),
        Some(vec![Inline::Strong(vec![str_inline("\u{1D58C}")])])
    );
    assert_eq!(
        to_typst("\\mathbffrak{g}").as_deref(),
        Some("bold(frak(g))")
    );
}

#[test]
fn sans_italic_alias_is_emphasised() {
    assert_eq!(to_inlines("\\mathsfit{x}"), Some(vec![var("x")]));
    assert_eq!(
        to_typst("\\mathsfit{x}").as_deref(),
        Some("italic(sans(x))")
    );
}

// ----------------------------------------------------------------------------
// Bare `\sqrt` (A5)
// ----------------------------------------------------------------------------

#[test]
fn bare_sqrt_with_no_argument_is_the_radical_sign() {
    // A `\sqrt` with no radicand is the bare radical glyph (U+221A).
    assert_eq!(to_inlines("\\sqrt"), Some(vec![str_inline("\u{221A}")]));
    assert_eq!(to_typst("\\sqrt").as_deref(), Some("\u{221A}"));
}

// ----------------------------------------------------------------------------
// Raw Unicode glyph → Typst name (B1)
// ----------------------------------------------------------------------------

#[test]
fn raw_unicode_glyph_maps_to_its_typst_name() {
    assert_eq!(to_typst("\u{3B1}").as_deref(), Some("alpha"));
    assert_eq!(to_typst("\u{2211}").as_deref(), Some("sum"));
    assert_eq!(to_typst("\u{222B}").as_deref(), Some("integral"));
    assert_eq!(to_typst("\u{2264}").as_deref(), Some("lt.eq"));
    assert_eq!(to_typst("\u{2260}").as_deref(), Some("eq.not"));
    assert_eq!(to_typst("\u{211D}").as_deref(), Some("RR"));
    assert_eq!(to_typst("\u{00D7}").as_deref(), Some("times"));
    assert_eq!(to_typst("\u{2192}").as_deref(), Some("arrow.r"));
    assert_eq!(to_typst("\u{2208}").as_deref(), Some("in"));
    assert_eq!(to_typst("\u{2202}").as_deref(), Some("partial"));
    assert_eq!(to_typst("\u{03A9}").as_deref(), Some("Omega"));
}

#[test]
fn raw_unicode_glyph_inside_text_stays_verbatim() {
    // Inside a text wrapper the glyph is literal text, never reverse-mapped.
    assert_eq!(
        to_typst("\\text{\u{3B1}}").as_deref(),
        Some("upright(\"\u{3B1}\")")
    );
}

// ----------------------------------------------------------------------------
// Function-call script must be parenthesized (B2)
// ----------------------------------------------------------------------------

#[test]
fn function_call_script_is_parenthesized() {
    assert_eq!(
        to_typst("a^{\\text{map}}").as_deref(),
        Some("a^(upright(\"map\"))")
    );
    assert_eq!(to_typst("x^{\\sqrt{2}}").as_deref(), Some("x^(sqrt(2))"));
    assert_eq!(to_typst("x^{\\hat a}").as_deref(), Some("x^(hat(a))"));
}

#[test]
fn bare_token_script_stays_unwrapped() {
    assert_eq!(to_typst("a^b").as_deref(), Some("a^b"));
    assert_eq!(to_typst("a^2").as_deref(), Some("a^2"));
}

// ----------------------------------------------------------------------------
// `\middle` delimiters (B3)
// ----------------------------------------------------------------------------

#[test]
fn middle_delimiter_renders_as_mid() {
    assert_eq!(
        to_typst("\\left( a \\middle| b \\right)").as_deref(),
        Some("(a mid(bar.v) b)")
    );
}

// ----------------------------------------------------------------------------
// `\not` over-translation in Typst (B4)
// ----------------------------------------------------------------------------

#[test]
fn not_does_not_precompose_unnegatable_relations_in_typst() {
    // The backend does not precompose these into a single glyph; the node falls back to verbatim
    // source rather than emitting a wrong precomposed character.
    assert_eq!(to_typst("\\not\\mid"), None);
    assert_eq!(to_typst("\\not\\exists"), None);
    assert_eq!(to_typst("\\not\\cup"), None);
    // A relation the backend does compose still renders.
    assert_eq!(to_typst("\\not\\subset").as_deref(), Some("subset.not"));
}

// ----------------------------------------------------------------------------
// `\DeclareMathOperator` (B5)
// ----------------------------------------------------------------------------

#[test]
fn lone_declare_math_operator_is_empty() {
    // A bare declaration has no typeset glyph; it lowers to empty output.
    assert_eq!(
        to_inlines("\\DeclareMathOperator{\\foo}{foo}"),
        Some(vec![])
    );
    assert_eq!(
        to_typst("\\DeclareMathOperator{\\foo}{foo}").as_deref(),
        Some("")
    );
    assert_eq!(
        to_typst("\\DeclareMathOperator*{\\argmax}{arg\\,max}").as_deref(),
        Some("")
    );
}

// ----------------------------------------------------------------------------
// Styled-alphabet fall-through: best-effort per missing character (W9-1)
// ----------------------------------------------------------------------------

#[test]
fn double_struck_maps_the_letterlike_specials() {
    // Five Greek/operator glyphs have dedicated double-struck codepoints; everything else in a
    // blackboard-bold group either maps to its styled codepoint or passes through unchanged.
    assert_eq!(
        to_inlines("\\mathbb{\\gamma}"),
        Some(vec![str_inline("\u{213D}")])
    );
    assert_eq!(
        to_inlines("\\mathbb{\\Gamma}"),
        Some(vec![str_inline("\u{213E}")])
    );
    assert_eq!(
        to_inlines("\\mathbb{\\pi}"),
        Some(vec![str_inline("\u{213C}")])
    );
    assert_eq!(
        to_inlines("\\mathbb{\\Pi}"),
        Some(vec![str_inline("\u{213F}")])
    );
    assert_eq!(
        to_inlines("\\mathbb{\\sum}"),
        Some(vec![str_inline("\u{2140}")])
    );
}

#[test]
fn styled_mixed_run_combines_into_one_wrapper() {
    // A blackboard-bold run with a letter, an uppercase letter, and a digit maps each glyph and
    // joins them into a single string: x→𝕩, Q→ℚ (letterlike special), 7→𝟟.
    assert_eq!(
        to_inlines("\\mathbb{xQ7}"),
        Some(vec![str_inline("\u{1D569}\u{211A}\u{1D7DF}")])
    );
}

#[test]
fn monospace_wraps_each_glyph_but_keeps_a_number_whole() {
    // `\mathtt` code-wraps every glyph, preserving each character's class; consecutive letters are
    // separate code spans, but a digit run is one number atom and so one code span.
    assert_eq!(
        to_inlines("\\mathtt{abc}"),
        Some(vec![
            Inline::Code(Box::default(), "a".to_string().into()),
            Inline::Code(Box::default(), "b".to_string().into()),
            Inline::Code(Box::default(), "c".to_string().into()),
        ])
    );
    assert_eq!(
        to_inlines("\\mathtt{12}"),
        Some(vec![Inline::Code(Box::default(), "12".to_string().into())])
    );
    // A monospaced operator is still code-wrapped, keeping its ordinary glyph.
    assert_eq!(
        to_inlines("\\mathtt{+}"),
        Some(vec![Inline::Code(Box::default(), "+".to_string().into())])
    );
}

// ----------------------------------------------------------------------------
// A style distributes independently into a scripted base (W9-2)
// ----------------------------------------------------------------------------

#[test]
fn style_wraps_the_base_alone_not_its_scripts() {
    // `\mathbf{a}^2`: the base is bold, the superscript keeps its own (digit) styling — the wrapper
    // is around the base only, not around base-plus-scripts.
    assert_eq!(
        to_inlines("\\mathbf{a}^2"),
        Some(vec![
            Inline::Strong(vec![str_inline("a")]),
            Inline::Superscript(vec![str_inline("2")]),
        ])
    );
    // `\mathbf{a}_i^2`: a plain-italic subscript and a digit superscript both stay independent of
    // the bold base.
    assert_eq!(
        to_inlines("\\mathbf{a}_i^2"),
        Some(vec![
            Inline::Strong(vec![str_inline("a")]),
            Inline::Subscript(vec![var("i")]),
            Inline::Superscript(vec![str_inline("2")]),
        ])
    );
    // `\mathit{x}_n`: an italic base with an italic subscript, each emphasised on its own.
    assert_eq!(
        to_inlines("\\mathit{x}_n"),
        Some(vec![var("x"), Inline::Subscript(vec![var("n")]),])
    );
}

// ----------------------------------------------------------------------------
// Raw-glyph limit operators carrying both scripts stay verbatim (W9-3)
// ----------------------------------------------------------------------------

#[test]
fn raw_glyph_limit_operator_with_both_scripts_is_verbatim() {
    // A bare large-operator glyph that takes both an under- and over-script stacks its limits, which
    // cannot be laid out on one line, so the whole expression is left to the verbatim fallback.
    assert_eq!(to_inlines("\u{2211}_a^b"), None);
    assert_eq!(to_inlines("\u{220F}_i^n"), None);
    assert_eq!(to_inlines("\u{22C3}_a^b"), None);
    assert_eq!(to_inlines("\u{2A06}_a^b"), None);
}

// ----------------------------------------------------------------------------
// An accent sits over a single non-ASCII glyph base (W9-4)
// ----------------------------------------------------------------------------

#[test]
fn accent_over_non_ascii_single_glyph() {
    // A combining accent attaches to a single Greek letter the same way it attaches to a Latin one.
    assert_eq!(
        to_inlines("\\hat{\\alpha}"),
        Some(vec![emph(vec![str_inline("\u{3B1}\u{302}")])])
    );
}

// ----------------------------------------------------------------------------
// Upright wrappers keep full math spacing (W9-5)
// ----------------------------------------------------------------------------

#[test]
fn upright_wrapper_keeps_math_spacing_and_minus() {
    // `\mathrm` sets its content upright while still resolving a function name and its trailing thin
    // space, and rendering a hyphen as the minus sign (U+2212).
    assert_eq!(
        to_inlines("\\mathrm{\\sin}"),
        Some(vec![str_inline("sin\u{2006}")])
    );
    assert_eq!(
        to_inlines("\\mathrm{-}"),
        Some(vec![str_inline("\u{2212}")])
    );
    // A bare `d` set upright stays upright; an adjacent unwrapped variable is italicised.
    assert_eq!(
        to_inlines("\\mathrm{d}x"),
        Some(vec![str_inline("d"), var("x")])
    );
    assert_eq!(to_typst("\\mathrm{d}x").as_deref(), Some("upright(d) x"));
    assert_eq!(to_typst("\\mathrm{-}").as_deref(), Some("upright(-)"));
}

// ----------------------------------------------------------------------------
// Adjacent manual spaces coalesce; a trailing manual space retypes its operator (W9-6)
// ----------------------------------------------------------------------------

#[test]
fn adjacent_manual_spaces_coalesce_into_one_run() {
    // Two thin spaces in a row collapse into a single string between the two variables.
    assert_eq!(
        to_inlines("a\\,\\,b"),
        Some(vec![var("a"), str_inline("\u{2006}\u{2006}"), var("b"),])
    );
}

#[test]
fn manual_space_after_operator_suppresses_automatic_spacing() {
    // A manual thin space immediately after a binary operator retypes it to ordinary, so the
    // automatic binary spacing on either side is dropped and only the thin space remains.
    assert_eq!(
        to_inlines("a +\\, b"),
        Some(vec![var("a"), str_inline("+\u{2006}"), var("b"),])
    );
}

// ----------------------------------------------------------------------------
// Typst single-symbol script parenthesisation (W9-7)
// ----------------------------------------------------------------------------

#[test]
fn typst_parenthesises_a_lone_symbol_script() {
    // A script that reduces to a single literal ASCII symbol is parenthesised so Typst binds the
    // whole symbol to the script rather than re-reading it.
    assert_eq!(to_typst("a^{+}").as_deref(), Some("a^(+)"));
    assert_eq!(to_typst("a^{\\%}").as_deref(), Some("a^(%)"));
    // An identifier, a digit, or a period stays bare.
    assert_eq!(to_typst("a^{x}").as_deref(), Some("a^x"));
    assert_eq!(to_typst("a^{2}").as_deref(), Some("a^2"));
    assert_eq!(to_typst("a^{.}").as_deref(), Some("a^."));
}

// ----------------------------------------------------------------------------
// Typst amsmath environment unwrapping (W9-8)
// ----------------------------------------------------------------------------

#[test]
fn typst_unwraps_single_line_equation_environment() {
    // `equation`/`equation*` carry a single line of math with no alignment, so the content is
    // spliced in transparently — no surrounding parentheses.
    assert_eq!(
        to_typst("\\begin{equation}a+b\\end{equation}").as_deref(),
        Some("a + b")
    );
    assert_eq!(
        to_typst("\\begin{equation*}a+b\\end{equation*}").as_deref(),
        Some("a + b")
    );
}

#[test]
fn typst_unwraps_multiline_amsmath_environments_as_grids() {
    // The multi-line amsmath families lower as line-broken alignment grids like `align`/`gather`.
    assert_eq!(
        to_typst("\\begin{multline}a\\\\b\\end{multline}").as_deref(),
        Some("a\\\nb")
    );
    assert_eq!(
        to_typst("\\begin{gather*}a\\\\b\\end{gather*}").as_deref(),
        Some("a\\\nb")
    );
    assert_eq!(
        to_typst("\\begin{flalign*}a&=b\\end{flalign*}").as_deref(),
        Some("a & = b")
    );
}

// ----------------------------------------------------------------------------
// A bare quote or backtick in math mode is unparsable (W9-9)
// ----------------------------------------------------------------------------

#[test]
fn bare_quote_or_backtick_in_math_falls_back_to_verbatim() {
    // Neither character has an ordinary-symbol meaning in math, so an expression containing one
    // cannot be linearised or translated and is emitted verbatim by every writer.
    assert_eq!(to_inlines("a\"b"), None);
    assert_eq!(to_inlines("a`b"), None);
    assert_eq!(to_typst("a\"b"), None);
    assert_eq!(to_typst("a`b"), None);
    // The same holds inside `\operatorname`, whose argument is math content.
    assert_eq!(to_inlines("\\operatorname{a\"b}"), None);
    assert_eq!(to_typst("\\operatorname{a`b}"), None);
}

#[test]
fn bare_dollar_in_math_falls_back_to_verbatim() {
    // A bare `$` is not an ordinary math symbol — it is a mode delimiter — so an expression that
    // carries one (here reached directly, not through a markdown `$…$` span) cannot be translated
    // and is emitted verbatim. The escaped `\$` form is the dollar glyph and is handled separately.
    assert_eq!(to_inlines("a$b"), None);
    assert_eq!(to_inlines("$"), None);
    assert_eq!(to_typst("a$b"), None);
    assert_eq!(to_typst("x+$"), None);
    // The escaped form still renders as the dollar-sign glyph.
    assert_eq!(
        to_inlines("a\\$b"),
        Some(vec![var("a"), str_inline("$"), var("b")])
    );
    assert_eq!(to_typst("\\$").as_deref(), Some("\\$"));
}

#[test]
fn quote_or_backtick_inside_a_text_wrapper_stays_literal() {
    // Inside a text wrapper a double quote is ordinary literal text and is escaped for the Typst
    // string rather than rejected.
    assert_eq!(to_inlines("\\text{a\"b}"), Some(vec![str_inline("a\"b")]));
    assert_eq!(
        to_typst("\\text{a\"b}").as_deref(),
        Some("upright(\"a\\\"b\")")
    );
    // The rejection is scoped to a bare math-mode quote: the same character inside a text wrapper is
    // kept and translated, so the two paths are distinguished.
    assert!(to_inlines("a\"b").is_none() && to_inlines("\\text{a\"b}").is_some());
}

// ----------------------------------------------------------------------------
// Text-mode input ligatures
// ----------------------------------------------------------------------------

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
    // A text run with no ligature trigger passes through unchanged.
    assert_eq!(to_inlines("\\text{a_b}"), Some(vec![str_inline("a_b")]));
    assert_eq!(to_inlines("\\text{ab cd}"), Some(vec![str_inline("ab cd")]));
}

#[test]
fn straight_double_quote_is_not_a_ligature_trigger_in_text() {
    // Only the backtick and apostrophe curl; a straight double quote stays literal.
    assert_eq!(to_inlines("\\text{a\"b}"), Some(vec![str_inline("a\"b")]));
}

#[test]
fn operatorname_argument_is_not_dash_ligatured() {
    // The `\operatorname` argument is upright math, not literal text: a double dash there stays two
    // hyphens rather than collapsing into an en dash.
    assert_eq!(
        to_typst("\\operatorname{a--b}").as_deref(),
        Some("\"a--b\"")
    );
}

// ----------------------------------------------------------------------------
// `\not` over a non-relation base
// ----------------------------------------------------------------------------

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
    // A braced base lowers its content, then carries the combining long solidus through Typst's
    // generic accent form.
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
    // The linear inline backend has no overlay for a struck group, so a braced `\not` falls back to
    // verbatim there.
    assert_eq!(to_inlines("\\not{a}"), None);
    assert_eq!(to_inlines("\\not{\\alpha}"), None);
    assert_eq!(to_inlines("\\not{}"), None);
}

// ----------------------------------------------------------------------------
// Typst environment lowering
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Bare vertical-bar retyping
// ----------------------------------------------------------------------------

#[test]
fn bare_bar_makes_a_following_sign_unary() {
    // A bare `|` opens a group, so the `-` after it is unary: the bar and the unary minus carry no
    // space between them and coalesce into one piece.
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

// ----------------------------------------------------------------------------
// Trailing `\bmod` guard
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Added symbol commands
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Single-token negated-relation gaps: the curly precedes/succeeds, plus the
// `\not`-prefixed slanted and curly forms
// ----------------------------------------------------------------------------

#[test]
fn n_prefixed_curly_precedes_succeeds_render() {
    // `\npreccurlyeq`/`\nsucccurlyeq` share the negated-precedes/succeeds glyphs (U+22E0/U+22E1)
    // with their non-curly siblings.
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
    // `\not` over a slanted or curly comparison strikes it to the precomposed negated glyph: the
    // slanted forms reuse the plain negated-≤/≥ glyphs, the curly forms the negated precede/succeed.
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
    // `\not\lesssim`/`\not\gtrsim` have no precomposed form, so the base glyph carries the combining
    // long solidus (U+0338).
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

// ----------------------------------------------------------------------------
// Text-mode wrappers: accents, foreign letters, text symbols, `\quad`, and
// transparent nested grouping
// ----------------------------------------------------------------------------

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
    // An accent over a dotless-letter control word (`\i`, `\j`) has no composed form: the accent is
    // dropped and the control word is kept as its literal source.
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
    // A nested brace group inside a text wrapper joins the surrounding run for the inline form; the
    // Typst form keeps each group as its own `upright` segment.
    assert_eq!(to_inlines("\\text{a{b}c}"), Some(vec![str_inline("abc")]));
    assert_eq!(
        to_typst("\\text{a{b}c}").as_deref(),
        Some("upright(\"a\") upright(\"b\") upright(\"c\")")
    );
}

#[test]
fn operatorname_does_not_resolve_text_commands() {
    // `\operatorname` is math mode: a text accent there is not resolved and the whole expression
    // falls back to verbatim.
    assert_eq!(to_inlines("\\operatorname{\\'a}"), None);
}

// ----------------------------------------------------------------------------
// Trailing control-space guard
// ----------------------------------------------------------------------------

#[test]
fn trailing_control_space_falls_back_for_inlines() {
    // A control space with no following operand has nothing to set its space against, so the inline
    // lowering falls back to verbatim; the Typst form still resolves it to a medium space.
    assert_eq!(to_inlines("a \\ "), None);
    assert!(to_typst("a \\ ").is_some());
}

#[test]
fn control_space_before_an_operand_is_kept() {
    // A control space that precedes further content keeps its space and lowers normally.
    assert!(to_inlines("a\\ b").is_some());
}

// ----------------------------------------------------------------------------
// Added dingbat and symbol commands
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Starred matrix/cases environments: unstarred behaviour with a literal `[align]`
// ----------------------------------------------------------------------------

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
    // The optional `[align]` argument is presentational; its brackets and contents become literal
    // leading content of the first cell.
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

// ----------------------------------------------------------------------------
// A bare grid inside a matched `\left … \right` fuses into one mat/vec
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Mismatched balanced delimiters wrap in lr() with raw glyphs; unpaired
// paren/bracket glyphs are escaped
// ----------------------------------------------------------------------------

#[test]
fn mismatched_paired_delimiters_wrap_in_lr() {
    assert_eq!(to_typst("\\left( a \\right]").as_deref(), Some("lr((a])"));
    assert_eq!(to_typst("\\left[ a \\right)").as_deref(), Some("lr([a))"));
    assert_eq!(to_typst("\\left\\{ a \\right)").as_deref(), Some("lr({a))"));
    assert_eq!(to_typst("\\left| a \\right)").as_deref(), Some("lr(|a))"));
}

#[test]
fn mismatch_with_an_angle_side_escapes_the_paren_directly() {
    // A paren opposite an angle bracket cannot auto-pair, so the paren is escaped and the angle
    // prints its glyph, with no `lr(..)` wrapper.
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

// ----------------------------------------------------------------------------
// In-grid equation-numbering annotations are stripped
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// Equation labels: the first \label is lifted to a trailing Typst reference
// ----------------------------------------------------------------------------

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
    // An equation-numbering annotation labels the whole expression, so it is consumed wherever it
    // sits — after the content just as before it — rather than left as an unknown control sequence.
    // A trailing `\label` still lifts to a reference; `\nonumber` and `\tag` drop without a trace.
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

// ----------------------------------------------------------------------------
// Alternative spellings of the styled-alphabet wrappers
// ----------------------------------------------------------------------------

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

// ----------------------------------------------------------------------------
// A bare function as the final atom of a script drops its trailing thin space
// ----------------------------------------------------------------------------

#[test]
fn a_function_as_a_sole_script_atom_has_no_trailing_thin_space() {
    // `x^{\sin}` lowers the superscript to a bare `sin` with no dangling thin space.
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

// ----------------------------------------------------------------------------
// A binary operator with no right operand retypes to ordinary (TeXbook rule 6)
// ----------------------------------------------------------------------------

#[test]
fn a_binary_operator_before_a_relation_loses_its_spacing() {
    // `a+=b`: the `+` has no right operand (a relation follows), so it binds to `a`; the `=` keeps
    // its relation spacing. The retyped `+` and the relation's leading space merge into one run.
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
