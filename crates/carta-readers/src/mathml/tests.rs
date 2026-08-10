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
        "\\underset{0}{\\overset{1}{x}}"
    );
}

#[test]
fn takes_limits_recognizes_operators_but_not_symbols() {
    assert!(takes_limits('\u{2211}'));
    assert!(takes_limits('\u{222b}'));
    assert!(!takes_limits('\u{3b1}'));
    assert!(!takes_limits('x'));
}

#[test]
fn fenced_separators_cycle_and_repeat_the_last() {
    assert_eq!(
        tex("<math><mfenced separators=';,'><mi>a</mi><mi>b</mi><mi>c</mi></mfenced></math>"),
        "\\left( {a;b,c} \\right)"
    );
    // A single separator applies to every gap.
    assert_eq!(
        tex("<math><mfenced separators=';'><mi>a</mi><mi>b</mi><mi>c</mi></mfenced></math>"),
        "\\left( {a;b;c} \\right)"
    );
    // Defaults are parentheses and commas.
    assert_eq!(
        tex("<math><mfenced><mi>a</mi><mi>b</mi></mfenced></math>"),
        "\\left( {a,b} \\right)"
    );
    assert_eq!(
        tex("<math><mfenced open='[' close=']'><mi>x</mi></mfenced></math>"),
        "\\lbrack x\\rbrack"
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
fn character_mapping_touches_only_specials() {
    assert_eq!(map_characters("%", Faces::default()), "\\%");
    // An escape keeps the character after it from reading as part of the command.
    assert_eq!(map_characters("a#b", Faces::default()), "a\\# b");
    assert_eq!(map_characters("plain", Faces::default()), "plain");
}

#[test]
fn an_operator_symbol_maps_to_its_command() {
    assert_eq!(tex("<math><mo>\u{2264}</mo></math>"), "\\leq");
    assert_eq!(tex("<math><mo>\u{2208}</mo></math>"), "\\in");
    assert_eq!(tex("<math><mo>\u{2211}</mo></math>"), "\\sum");
    assert_eq!(tex("<math><mi>\u{211d}</mi></math>"), "\\mathbb{R}");
}

#[test]
fn a_face_written_around_a_fragment_is_not_written_again_inside_it() {
    let style =
        |variant, body| format!("<math><mstyle mathvariant='{variant}'>{body}</mstyle></math>");
    // The face the row is set in leaves the tokens it holds bare.
    assert_eq!(tex(&style("bold", "<mi>a</mi><mi>b</mi>")), "\\mathbf{ab}");
    // A letter of the alphanumeric plane comes down to the letter it is set from.
    assert_eq!(
        tex(&style("double-struck", "<mi>\u{211d}</mi>")),
        "\\mathbb{R}"
    );
    // A variant that differs is written even where it stands for the same command.
    assert_eq!(
        tex(&style("bold", "<mi mathvariant='bold-italic'>a</mi>")),
        "\\mathbf{\\mathbf{a}}"
    );
}

#[test]
fn a_variant_naming_no_face_reads_as_the_plain_one() {
    assert_eq!(tex("<math><mi mathvariant='initial'>a</mi></math>"), "a");
    assert_eq!(
        tex("<math><mstyle mathvariant='bold'><mi mathvariant='initial'>a</mi></mstyle></math>"),
        "\\mathbf{\\mathrm{a}}"
    );
    // A style wrapper left with no variant in force sets its row in the plain face.
    assert_eq!(
        tex("<math><mstyle mathvariant='bold'><mstyle><mi>a</mi></mstyle></mstyle></math>"),
        "\\mathbf{\\mathrm{a}}"
    );
}

#[test]
fn a_cell_carries_one_gap_after_the_ampersand() {
    assert_eq!(
        tex("<math><mtable><mtr><mtd><mi>a</mi></mtd><mtd><mo>+</mo></mtd></mtr></mtable></math>"),
        "\\begin{matrix}\na & + \n\\end{matrix}"
    );
}

#[test]
fn symbol_roles_and_tables_cover_each_lookup_shape() {
    use symbols::Role;

    let roles = [
        Role::Plain,
        Role::Spaced,
        Role::Styled,
        Role::Tight,
        Role::Infix,
        Role::Open,
        Role::Close,
        Role::Both,
        Role::Middle,
        Role::OpenSymbol,
        Role::CloseSymbol,
        Role::BothSymbol,
    ];
    assert_eq!(roles.iter().filter(|role| role.opens()).count(), 4);
    assert_eq!(roles.iter().filter(|role| role.closes()).count(), 4);
    assert_eq!(roles.iter().filter(|role| role.is_sign()).count(), 3);
    assert_eq!(roles.iter().filter(|role| role.is_delimiter()).count(), 7);
    assert!(symbols::stacks_over('^'));
    assert!(!symbols::stacks_over('x'));
    assert!(symbols::symbol('+').is_some());
    assert!(symbols::symbol('x').is_none());
    assert!(symbols::operator("||").is_some());
    assert!(symbols::operator("unknown").is_none());
    assert_eq!(
        symbols::styled_letter('\u{1d400}'),
        Some((Some("\\mathbf"), 'A'))
    );
    assert_eq!(symbols::styled_letter('A'), None);
}

#[test]
fn uncommon_layout_and_annotation_elements_keep_their_content() {
    assert_eq!(
        tex("<math><mroot><mi>x</mi><mn>3</mn></mroot></math>"),
        "\\sqrt[3]{x}"
    );
    assert_eq!(
        tex("<math><mphantom><mi>x</mi></mphantom></math>"),
        "\\phantom{x}"
    );
    assert_eq!(
        tex("<math><semantics><mi>x</mi><annotation>ignored</annotation></semantics></math>"),
        "x"
    );
    assert_eq!(
        tex("<math><semantics><annotation>ignored</annotation></semantics></math>"),
        ""
    );
    assert_eq!(
        tex("<math><ms lquote='[' rquote=']'>a#b</ms></math>"),
        "\\text{[a\\#b]}"
    );
    assert_eq!(
        tex("<math><menclose notation='box'><mi>x</mi></menclose></math>"),
        "\\boxed{x}"
    );
    assert_eq!(
        tex("<math><menclose notation='updiagonalstrike'><mi>x</mi></menclose></math>"),
        "\\cancel{x}"
    );
    assert_eq!(
        tex("<math><menclose notation='downdiagonalstrike'><mi>x</mi></menclose></math>"),
        "\\bcancel{x}"
    );
    assert_eq!(
        tex(
            "<math><menclose notation='updiagonalstrike downdiagonalstrike'><mi>x</mi></menclose></math>"
        ),
        "\\xcancel{x}"
    );
    assert_eq!(
        tex("<math><menclose notation='circle'><mi>x</mi></menclose></math>"),
        "x"
    );
}

#[test]
fn math_spaces_cover_named_measured_and_fallback_commands() {
    assert_eq!(space_mu("thinmathspace"), Some(3));
    assert_eq!(space_mu("negativeveryverythickmathspace"), Some(-7));
    assert_eq!(space_mu("0.5em"), Some(9));
    assert_eq!(space_mu("+1em"), None);
    assert_eq!(space_mu("1px"), None);
    assert_eq!(space_mu("NaNem"), None);
    assert_eq!(space_command(0), "");
    assert_eq!(space_command(3), "\\,");
    assert_eq!(space_command(4), "\\ ");
    assert_eq!(space_command(5), "\\;");
    assert_eq!(space_command(-3), "\\!");
    assert_eq!(space_command(18), "\\quad");
    assert_eq!(space_command(36), "\\qquad");
    assert_eq!(space_command(9), "\\mspace{9mu}");
}

#[test]
fn matrix_delimiter_helpers_cover_each_supported_pair() {
    assert_eq!(matrix_env("(", ")"), Some("pmatrix"));
    assert_eq!(matrix_env("[", "]"), Some("bmatrix"));
    assert_eq!(matrix_env("{", "}"), Some("Bmatrix"));
    assert_eq!(matrix_env("|", "|"), None);
    assert_eq!(left_right_delim("|"), Some("|"));
    assert_eq!(left_right_delim("\u{2016}"), Some("\\|"));
    assert_eq!(left_right_delim("("), None);
}

#[test]
fn fenced_matrices_and_scripted_fences_select_their_rendering_paths() {
    let matrix = "<mtable><mtr><mtd><mi>a</mi></mtd><mtd><mi>b</mi></mtd></mtr></mtable>";
    assert!(
        tex(&format!("<math><mfenced>{matrix}</mfenced></math>")).starts_with("\\begin{pmatrix}")
    );
    assert!(
        tex(&format!(
            "<math><mfenced open='[' close=']'>{matrix}</mfenced></math>"
        ))
        .starts_with("\\begin{bmatrix}")
    );
    assert!(
        tex(&format!(
            "<math><mfenced open='{{' close='}}'>{matrix}</mfenced></math>"
        ))
        .starts_with("\\begin{Bmatrix}")
    );
    let barred = tex(&format!(
        "<math><mfenced open='|' close='|'>{matrix}</mfenced></math>"
    ));
    assert!(barred.starts_with("\\left| \\begin{matrix}"));
    assert_eq!(
        tex("<math><msup><mfenced open='[' close=']'><mi>x</mi></mfenced><mn>2</mn></msup></math>"),
        "\\lbrack x\\rbrack^{2}"
    );
}

#[test]
fn unusual_fence_attributes_preserve_or_omit_their_operators() {
    assert_eq!(
        tex("<math><mfenced open='' close=''><mi>x</mi></mfenced></math>"),
        "x"
    );
    assert_eq!(tex("<math><mfenced open='' close=''/></math>"), "");
    assert!(
        tex("<math><mfenced open=']' close=''><mi>x</mi></mfenced></math>").contains("\\rbrack")
    );
    let named = tex("<math><mfenced open='begin' close='end'><mi>x</mi></mfenced></math>");
    assert!(named.contains("\\operatorname{begin}"));
    assert!(named.contains("\\operatorname{end}"));
    let mixed = tex("<math><mfenced open='|' close=')'><mi>x</mi></mfenced></math>");
    assert!(mixed.contains('x'));
    assert!(mixed.contains('|'));
}

#[test]
fn covering_scripts_and_operator_forms_keep_each_distinct_form() {
    assert_eq!(
        tex("<math><munderover><mi>x</mi><mi>i</mi><mo>^</mo></munderover></math>"),
        "\\hat{\\underset{i}{x}}"
    );
    assert_eq!(tex("<math><mo>||</mo></math>"), "||");
    assert_eq!(tex("<math><mo>sin</mo></math>"), "\\sin");
    assert_eq!(
        tex("<math><mo>custom</mo></math>"),
        "\\operatorname{custom}"
    );
    let escaped = escape_text("%&_#${}~^\\a", Faces::default());
    assert!(escaped.contains("\\textasciitilde"));
    assert!(escaped.contains("\\textasciicircum"));
    assert!(escaped.contains("\\textbackslash a"));
}
