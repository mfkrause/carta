//! Script restacking, sizing wrappers, and text-wrapper handling.

use super::super::{to_inlines, to_typst};
use super::{emph, str_inline, var};
use carta_ast::Inline;

#[test]
fn a_restart_run_renders_sub_before_sup_in_typst() {
    // after sub and sup, the next pair restarts on a fresh empty base, reordered sub-then-sup
    assert_eq!(to_typst("a^a_b^c_d").as_deref(), Some("a_b^a \"\"_d^c"));
    assert_eq!(to_typst("a^1_2^3_4^5").as_deref(), Some("a_2^1 \"\"_4^3^5"));
    assert_eq!(
        to_typst("\\sum^1_2^3_4").as_deref(),
        Some("sum_2^1 \"\"_4^3")
    );
}

#[test]
fn a_restart_run_renders_sub_before_sup_in_the_inline_tree() {
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

#[test]
fn fixed_size_wrapper_sizes_the_null_delimiter_as_a_literal_period() {
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
    assert_eq!(to_inlines("\\big a"), None);
    assert_eq!(to_inlines("\\big 1"), None);
    assert_eq!(to_inlines("\\big\\uparrow"), None);
    assert_eq!(to_typst("\\big a"), None);
    assert_eq!(to_typst("\\big 1"), None);
    assert_eq!(to_typst("\\big\\uparrow"), None);
}

#[test]
fn fixed_size_scale_matrix_covers_every_width_and_variant() {
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

#[test]
fn typst_colon_equals_digraph_is_tight() {
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
    assert_eq!(to_inlines(":="), Some(vec![str_inline(":=")]));
    assert_eq!(
        to_inlines("a := b"),
        Some(vec![var("a"), str_inline("\u{2004}:=\u{2004}"), var("b")])
    );
}

#[test]
fn typst_scripted_escaped_close_delimiter_has_no_leading_space() {
    assert_eq!(to_typst("(a)").as_deref(), Some("\\(a\\)"));
    assert_eq!(to_typst("[a]").as_deref(), Some("\\[a\\]"));
    assert_eq!(to_typst("(a)^2").as_deref(), Some("\\(a\\)^2"));
    assert_eq!(to_typst("(a)_n").as_deref(), Some("\\(a\\)_n"));
    assert_eq!(to_typst("((a))^2").as_deref(), Some("\\(\\(a\\)\\)^2"));
    assert_eq!(to_typst("\\left(a\\right)^2").as_deref(), Some("(a)^2"));
}

#[test]
fn text_escape_unescapes_to_the_literal_character() {
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
    assert_eq!(
        to_typst("\\text{a\"b}").as_deref(),
        Some("upright(\"a\\\"b\")")
    );
}

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
    assert_eq!(to_inlines("\\text{a\\ b}"), Some(vec![str_inline("a b")]));
}

#[test]
fn text_spacing_splits_the_wrapper_formatting_per_run() {
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

#[test]
fn operatorname_drops_literal_spaces_and_folds_spacing() {
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

#[test]
fn math_inside_text_renders_as_math() {
    assert_eq!(to_inlines("\\text{$x$}"), Some(vec![var("x")]));
    assert_eq!(to_typst("\\text{$x$}").as_deref(), Some("x"));
}

#[test]
fn math_inside_text_ignores_the_wrapper_formatting() {
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
    assert_eq!(to_inlines("\\text{$x}"), None);
    assert_eq!(to_typst("\\text{a$b}"), None);
}

#[test]
fn paren_delimiters_in_text_switch_back_to_math() {
    assert_eq!(to_inlines("\\text{$x$}"), to_inlines("\\text{\\(x\\)}"));
    assert_eq!(
        to_typst("\\text{ if \\(x>0\\)}").as_deref(),
        Some("upright(\" if \") x > 0")
    );
}

#[test]
fn bracket_delimiters_in_text_switch_back_to_math() {
    assert_eq!(
        to_typst("\\text{x \\[y\\] z}").as_deref(),
        Some("upright(\"x \") y upright(\" z\")")
    );
}

#[test]
fn unbalanced_math_delimiter_in_text_falls_back() {
    assert_eq!(to_inlines("\\text{\\(x}"), None);
    assert_eq!(to_typst("\\text{a\\[b}"), None);
}

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
    assert_eq!(
        to_inlines("\\mathbfcal{L}"),
        Some(vec![Inline::Strong(vec![str_inline("\u{1D4DB}")])])
    );
    assert_eq!(to_typst("\\mathbfcal{L}").as_deref(), Some("bold(cal(L))"));
}

#[test]
fn bold_fraktur_alias_uses_the_bold_fraktur_codepoint() {
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

#[test]
fn bare_sqrt_with_no_argument_is_the_radical_sign() {
    assert_eq!(to_inlines("\\sqrt"), Some(vec![str_inline("\u{221A}")]));
    assert_eq!(to_typst("\\sqrt").as_deref(), Some("\u{221A}"));
}

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
    assert_eq!(
        to_typst("\\text{\u{3B1}}").as_deref(),
        Some("upright(\"\u{3B1}\")")
    );
}

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

#[test]
fn middle_delimiter_renders_as_mid() {
    assert_eq!(
        to_typst("\\left( a \\middle| b \\right)").as_deref(),
        Some("(a mid(bar.v) b)")
    );
}

#[test]
fn not_does_not_precompose_unnegatable_relations_in_typst() {
    assert_eq!(to_typst("\\not\\mid"), None);
    assert_eq!(to_typst("\\not\\exists"), None);
    assert_eq!(to_typst("\\not\\cup"), None);
    assert_eq!(to_typst("\\not\\subset").as_deref(), Some("subset.not"));
}

#[test]
fn lone_declare_math_operator_is_empty() {
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

#[test]
fn double_struck_maps_the_letterlike_specials() {
    // five Greek/operator glyphs have dedicated double-struck codepoints
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
    // each glyph maps and the results join into one string: x→𝕩, Q→ℚ (letterlike), 7→𝟟
    assert_eq!(
        to_inlines("\\mathbb{xQ7}"),
        Some(vec![str_inline("\u{1D569}\u{211A}\u{1D7DF}")])
    );
}

#[test]
fn monospace_wraps_each_glyph_but_keeps_a_number_whole() {
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
    assert_eq!(
        to_inlines("\\mathtt{+}"),
        Some(vec![Inline::Code(Box::default(), "+".to_string().into())])
    );
}

#[test]
fn style_wraps_the_base_alone_not_its_scripts() {
    assert_eq!(
        to_inlines("\\mathbf{a}^2"),
        Some(vec![
            Inline::Strong(vec![str_inline("a")]),
            Inline::Superscript(vec![str_inline("2")]),
        ])
    );
    assert_eq!(
        to_inlines("\\mathbf{a}_i^2"),
        Some(vec![
            Inline::Strong(vec![str_inline("a")]),
            Inline::Subscript(vec![var("i")]),
            Inline::Superscript(vec![str_inline("2")]),
        ])
    );
    assert_eq!(
        to_inlines("\\mathit{x}_n"),
        Some(vec![var("x"), Inline::Subscript(vec![var("n")]),])
    );
}

#[test]
fn raw_glyph_limit_operator_with_both_scripts_is_verbatim() {
    assert_eq!(to_inlines("\u{2211}_a^b"), None);
    assert_eq!(to_inlines("\u{220F}_i^n"), None);
    assert_eq!(to_inlines("\u{22C3}_a^b"), None);
    assert_eq!(to_inlines("\u{2A06}_a^b"), None);
}

#[test]
fn accent_over_non_ascii_single_glyph() {
    assert_eq!(
        to_inlines("\\hat{\\alpha}"),
        Some(vec![emph(vec![str_inline("\u{3B1}\u{302}")])])
    );
}

#[test]
fn upright_wrapper_keeps_math_spacing_and_minus() {
    assert_eq!(
        to_inlines("\\mathrm{\\sin}"),
        Some(vec![str_inline("sin\u{2006}")])
    );
    assert_eq!(
        to_inlines("\\mathrm{-}"),
        Some(vec![str_inline("\u{2212}")])
    );
    assert_eq!(
        to_inlines("\\mathrm{d}x"),
        Some(vec![str_inline("d"), var("x")])
    );
    assert_eq!(to_typst("\\mathrm{d}x").as_deref(), Some("upright(d) x"));
    assert_eq!(to_typst("\\mathrm{-}").as_deref(), Some("upright(-)"));
}

#[test]
fn adjacent_manual_spaces_coalesce_into_one_run() {
    assert_eq!(
        to_inlines("a\\,\\,b"),
        Some(vec![var("a"), str_inline("\u{2006}\u{2006}"), var("b"),])
    );
}

#[test]
fn manual_space_after_operator_suppresses_automatic_spacing() {
    assert_eq!(
        to_inlines("a +\\, b"),
        Some(vec![var("a"), str_inline("+\u{2006}"), var("b"),])
    );
}

#[test]
fn typst_parenthesises_a_lone_symbol_script() {
    assert_eq!(to_typst("a^{+}").as_deref(), Some("a^(+)"));
    assert_eq!(to_typst("a^{\\%}").as_deref(), Some("a^(%)"));
    assert_eq!(to_typst("a^{x}").as_deref(), Some("a^x"));
    assert_eq!(to_typst("a^{2}").as_deref(), Some("a^2"));
    assert_eq!(to_typst("a^{.}").as_deref(), Some("a^."));
}

#[test]
fn typst_unwraps_single_line_equation_environment() {
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

#[test]
fn bare_quote_or_backtick_in_math_falls_back_to_verbatim() {
    // neither character has an ordinary-symbol meaning in math: verbatim in every writer
    assert_eq!(to_inlines("a\"b"), None);
    assert_eq!(to_inlines("a`b"), None);
    assert_eq!(to_typst("a\"b"), None);
    assert_eq!(to_typst("a`b"), None);
    assert_eq!(to_inlines("\\operatorname{a\"b}"), None);
    assert_eq!(to_typst("\\operatorname{a`b}"), None);
}
