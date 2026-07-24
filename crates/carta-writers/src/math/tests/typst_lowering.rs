//! Typst lowering of basic atoms, scripts, styles, fractions, and delimiters.

use super::super::{to_inlines, to_typst, to_typst_display};
use super::{str_inline, var};

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

#[test]
fn horizontal_dots_alias_is_the_ellipsis_glyph() {
    // `\hdots` is an alias for the horizontal ellipsis, an ordinary glyph.
    assert_eq!(to_inlines("\\hdots"), Some(vec![str_inline("\u{2026}")]));
    assert_eq!(to_typst("\\hdots").as_deref(), Some("dots.h"));
}

#[test]
fn named_parentheses_are_open_and_close_delimiters() {
    // open/close spacing: no space hugs the operand between them
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
    // U+2145-U+2149: ordinary class, upright, no spacing; Typst has no named symbol so raw glyph
    assert_eq!(to_inlines("\\dd"), Some(vec![str_inline("\u{2146}")]));
    assert_eq!(to_inlines("\\ee"), Some(vec![str_inline("\u{2147}")]));
    assert_eq!(to_inlines("\\ii"), Some(vec![str_inline("\u{2148}")]));
    assert_eq!(to_inlines("\\jj"), Some(vec![str_inline("\u{2149}")]));
    assert_eq!(to_inlines("\\DD"), Some(vec![str_inline("\u{2145}")]));
    assert_eq!(
        to_inlines("a\\dd b"),
        Some(vec![var("a"), str_inline("\u{2146}"), var("b")])
    );
    assert_eq!(to_typst("\\dd").as_deref(), Some("\u{2146}"));
    assert_eq!(to_typst("\\DD").as_deref(), Some("\u{2145}"));
}

#[test]
fn extended_relations_carry_relation_spacing() {
    // relation spacing (U+2004) flanks the operands
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
    // display: the prime stacks as a superscript so Typst's limit placement raises it
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
    // named limit functions stack their prime like the big operators
    assert_eq!(to_typst_display("\\lim'").as_deref(), Some("lim^(')"));
    assert_eq!(to_typst_display("\\max'").as_deref(), Some("max^(')"));
    assert_eq!(to_typst_display("\\Pr'").as_deref(), Some("Pr^(')"));
}

#[test]
fn typst_prime_on_a_limit_operator_stays_literal_inline() {
    // inline, Typst sets operator scripts to the side anyway, so the prime stays literal
    assert_eq!(to_typst("\\sum'").as_deref(), Some("sum'"));
    assert_eq!(to_typst("\\lim'").as_deref(), Some("lim'"));
}

#[test]
fn typst_display_stacked_prime_follows_a_subscript() {
    // subscript first, then the prime as the stacked superscript, in either source order
    assert_eq!(to_typst_display("\\sum_a'").as_deref(), Some("sum_a^(')"));
    assert_eq!(to_typst_display("\\sum'_a").as_deref(), Some("sum_a^(')"));
}

#[test]
fn typst_display_prime_with_a_real_superscript_is_not_restacked() {
    // a filled superscript keeps the prime inside it; prime then superscript restarts the base
    assert_eq!(to_typst_display("\\sum_a^b'").as_deref(), Some("sum_a^b'"));
    assert_eq!(
        to_typst_display("\\sum'^b").as_deref(),
        Some("sum^(') \"\"^b")
    );
}

#[test]
fn typst_display_prime_on_a_side_script_operator_stays_literal() {
    // side-script operators keep their scripts beside themselves even in display
    assert_eq!(to_typst_display("\\int'").as_deref(), Some("integral'"));
    assert_eq!(to_typst_display("\\bigoplus'").as_deref(), Some("xor.big'"));
    assert_eq!(to_typst_display("\\sin'").as_deref(), Some("sin'"));
}

#[test]
fn typst_display_stacked_prime_propagates_into_nested_content() {
    // the display context reaches into nested content, so the prime stacks there too
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
    // auto-paired and explicit forms are both the stretchy named double line
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
    // the parallel sign prints directly: not the stretchy double line, no lr(..) wrapper
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
    // corners act as \left/\right brackets printing the raw glyphs around their content
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
