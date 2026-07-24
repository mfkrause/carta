use super::*;

/// Render a `<math>` fragment to TeX through the same path a container reader takes.
fn tex(mathml: &str) -> String {
    let root = crate::xml::parse(mathml.as_bytes(), 64).expect("well-formed test markup");
    to_tex(&root)
}

#[test]
fn token_elements_render_to_their_tex_form() {
    assert_eq!(tex("<math><mi>x</mi></math>"), "x");
    assert_eq!(tex("<math><mn>42</mn></math>"), "42");
    assert_eq!(tex("<math><mi>sin</mi></math>"), "\\sin");
    assert_eq!(tex("<math><mi>\u{3c0}</mi></math>"), "\\pi");
    assert_eq!(tex("<math><mtext>hi</mtext></math>"), "\\text{hi}");
}

#[test]
fn a_binary_operator_is_spaced_from_its_operands() {
    assert_eq!(tex("<math><mi>x</mi><mo>=</mo><mn>1</mn></math>"), "x = 1");
}

#[test]
fn layout_elements_wrap_their_children() {
    assert_eq!(
        tex("<math><mfrac><mn>1</mn><mn>2</mn></mfrac></math>"),
        "\\frac{1}{2}"
    );
    assert_eq!(tex("<math><msqrt><mi>x</mi></msqrt></math>"), "\\sqrt{x}");
    assert_eq!(
        tex("<math><msup><mi>x</mi><mn>2</mn></msup></math>"),
        "x^{2}"
    );
    assert_eq!(
        tex("<math><msubsup><mi>x</mi><mn>0</mn><mn>1</mn></msubsup></math>"),
        "x_{0}^{1}"
    );
}

#[test]
fn a_recognized_over_accent_maps_to_its_command() {
    // A spacing macron and a combining overline overline the base; a combining macron bars it.
    assert_eq!(
        tex("<math><mover><mi>x</mi><mo>^</mo></mover></math>"),
        "\\hat{x}"
    );
    assert_eq!(
        tex("<math><mover><mi>x</mi><mo>\u{af}</mo></mover></math>"),
        "\\overline{x}"
    );
    assert_eq!(
        tex("<math><mover><mi>x</mi><mo>\u{304}</mo></mover></math>"),
        "\\bar{x}"
    );
    assert_eq!(
        tex("<math><mover><mi>x</mi><mo>\u{20d7}</mo></mover></math>"),
        "\\overrightarrow{x}"
    );
}

#[test]
fn an_unrecognized_overscript_is_stacked_rather_than_dropped() {
    // A brace or label over the base must be preserved, not silently replaced by an accent.
    assert_eq!(
        tex("<math><mover><mi>x</mi><mtext>def</mtext></mover></math>"),
        "\\overset{\\text{def}}{x}"
    );
    // A near-miss glyph without a dedicated accent command stacks rather than borrowing another's.
    assert_eq!(
        tex("<math><mover><mi>x</mi><mo>\u{2192}</mo></mover></math>"),
        "\\overset{\\rightarrow}{x}"
    );
}

#[test]
fn only_limit_bearing_bases_take_limits() {
    // A large operator carries its script as a stacked limit.
    assert_eq!(
        tex("<math><munder><mo>\u{2211}</mo><mi>i</mi></munder></math>"),
        "\\sum\\limits_{i}"
    );
    assert_eq!(
        tex("<math><munderover><mo>\u{222b}</mo><mn>0</mn><mn>1</mn></munderover></math>"),
        "\\int\\limits_{0}^{1}"
    );
    // A Greek letter is not an operator, so `\limits` would be invalid TeX: it must use `\underset`.
    assert_eq!(
        tex("<math><munder><mi>\u{3b1}</mi><mi>i</mi></munder></math>"),
        "\\underset{i}{\\alpha}"
    );
    assert_eq!(
        tex("<math><munderover><mi>x</mi><mn>0</mn><mn>1</mn></munderover></math>"),
        "\\overset{1}{\\underset{0}{x}}"
    );
}

#[test]
fn takes_limits_recognizes_operators_but_not_symbols() {
    assert!(takes_limits("\\sum"));
    assert!(takes_limits("\\lim"));
    assert!(!takes_limits("\\alpha"));
    assert!(!takes_limits("x"));
}

#[test]
fn fenced_separators_cycle_and_repeat_the_last() {
    assert_eq!(
        tex("<math><mfenced separators=';,'><mi>a</mi><mi>b</mi><mi>c</mi></mfenced></math>"),
        "(a;b,c)"
    );
    // A single separator applies to every gap.
    assert_eq!(
        tex("<math><mfenced separators=';'><mi>a</mi><mi>b</mi><mi>c</mi></mfenced></math>"),
        "(a;b;c)"
    );
    // Defaults are parentheses and commas.
    assert_eq!(
        tex("<math><mfenced><mi>a</mi><mi>b</mi></mfenced></math>"),
        "(a,b)"
    );
    assert_eq!(
        tex("<math><mfenced open='[' close=']'><mi>x</mi></mfenced></math>"),
        "[x]"
    );
}

#[test]
fn multiscripts_collect_one_group_per_side() {
    // prescripts attach to an empty nucleus; an empty post-script side still emits its groups
    assert_eq!(
        tex(
            "<math><mmultiscripts><mi>C</mi><none/><none/><mprescripts/><mn>6</mn><mn>14</mn></mmultiscripts></math>"
        ),
        "{}_{6}^{14}C_{}^{}"
    );
    // Several post-script pairs collapse into a single subscript and superscript, never a second `_`.
    assert_eq!(
        tex(
            "<math><mmultiscripts><mi>R</mi><mi>a</mi><mi>b</mi><mi>c</mi><mi>d</mi></mmultiscripts></math>"
        ),
        "{}R_{ac}^{bd}"
    );
    // A `<none/>` slot leaves its group empty but the side, being present, still emits both groups.
    assert_eq!(
        tex("<math><mmultiscripts><mi>x</mi><mn>1</mn><none/></mmultiscripts></math>"),
        "{}x_{1}^{}"
    );
}

#[test]
fn an_operator_of_tex_specials_is_escaped() {
    assert_eq!(tex("<math><mo>%</mo></math>"), "\\%");
    assert_eq!(tex("<math><mo>$</mo></math>"), "\\$");
    assert_eq!(tex("<math><mo>#</mo></math>"), "\\#");
    assert_eq!(tex("<math><mo>&amp;</mo></math>"), "\\&");
    assert_eq!(tex("<math><mo>_</mo></math>"), "\\_");
}

#[test]
fn escape_operator_touches_only_specials() {
    assert_eq!(escape_operator("%"), "\\%");
    assert_eq!(escape_operator("a#b"), "a\\#b");
    assert_eq!(escape_operator("plain"), "plain");
}
