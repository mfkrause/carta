//! Symbol tables, negated relations, and sized delimiters.

use super::super::{to_inlines, to_typst};
use super::{str_inline, var};
use carta_ast::Inline;

#[test]
fn binom_has_no_inline_form() {
    assert_eq!(to_inlines("{n \\choose k}"), None);
}

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

#[test]
fn small_smile_and_frown_are_relations() {
    assert_eq!(
        to_inlines("\\smallsmile"),
        Some(vec![str_inline("\u{2323}")])
    );
    assert_eq!(to_typst("\\smallsmile").as_deref(), Some("smile"));
    assert_eq!(to_typst("\\smallfrown").as_deref(), Some("frown"));
}

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

#[test]
fn big_circled_operators_place_scripts_to_the_side() {
    // circled operators do not stack limits: scripts follow as ordinary sub/superscripts
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
    // depth guard: once tripped the whole expression has no structured form
    let deep = "a".to_string() + &"^".repeat(70) + "b";
    assert_eq!(to_typst(&deep), None);
    assert_eq!(to_inlines(&deep), None);
}

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
    // a non-letter base keeps the whole expression verbatim
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
