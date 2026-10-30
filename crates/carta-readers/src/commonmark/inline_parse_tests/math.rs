//! TeX math and raw TeX inline-parse tests.

use super::*;

#[test]
fn inline_and_display_math() {
    assert_eq!(pe("$a+b$", math()), vec![math_inline("a+b")]);
    assert_eq!(pe("$$x=y$$", math()), vec![math_display("x=y")]);
    assert_eq!(pe("$$ x $$", math()), vec![math_display(" x ")]);
}

#[test]
fn dollar_amounts_are_not_math() {
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
    assert_eq!(pe("$x_1*y*$", math()), vec![math_inline("x_1*y*")]);
    assert_eq!(pe(r"$a\$b$", math()), vec![math_inline(r"a\$b")]);
}

#[test]
fn failed_display_falls_back_to_inline() {
    assert_eq!(pe("$$x$", math()), vec![str("$"), math_inline("x")]);
}

#[test]
fn dollar_is_literal_without_the_extension() {
    assert_eq!(p("$a+b$"), vec![str("$a+b$")]);
}

#[test]
fn raw_tex_commands_with_argument_groups() {
    assert_eq!(
        pe(r"\textbf{b}\emph{c}", raw_tex()),
        vec![tex(r"\textbf{b}"), tex(r"\emph{c}")]
    );
    assert_eq!(pe(r"\sqrt[3]{8}", raw_tex()), vec![tex(r"\sqrt[3]{8}")]);
    assert_eq!(pe(r"\foo{a{b}c}", raw_tex()), vec![tex(r"\foo{a{b}c}")]);
}

#[test]
fn raw_tex_definitions_keep_parameters_and_replacement_text() {
    for source in [
        r"\def\word#1{Hello #1}",
        r"\def \word #1,#2;{#2 and #1}",
        r"\def\word#1\stop{Hello #1}",
        r"\def\word#1\{{Hello #1}",
        "\\def\\word#1{Hello\n#1}",
    ] {
        assert_eq!(pe(source, raw_tex()), vec![tex(source)], "{source}");
    }
    assert_eq!(
        pe(r"\def\word#1{Hello #1} after", raw_tex()),
        vec![tex(r"\def\word#1{Hello #1}"), Inline::Space, str("after")]
    );
}

#[test]
fn raw_tex_bare_command_absorbs_trailing_blanks() {
    assert_eq!(pe(r"\alpha y", raw_tex()), vec![tex(r"\alpha "), str("y")]);
    assert_eq!(
        pe(r"\foo{a} y", raw_tex()),
        vec![tex(r"\foo{a}"), Inline::Space, str("y")]
    );
    assert_eq!(
        pe(r"\foo1 y", raw_tex()),
        vec![tex(r"\foo1"), Inline::Space, str("y")]
    );
    assert_eq!(pe(r"\1foo", raw_tex()), vec![str(r"\1foo")]);
}

#[test]
fn raw_tex_unbalanced_brace_reverts_whole_command() {
    assert_eq!(
        pe(r"\foo{a y", raw_tex()),
        vec![str(r"\foo{a"), Inline::Space, str("y")]
    );
    assert_eq!(
        pe(r"\foo[a y", raw_tex()),
        vec![tex(r"\foo"), str("[a"), Inline::Space, str("y")]
    );
}

#[test]
fn raw_tex_unclosed_openers_stay_literal_at_scale() {
    let cases = [
        r"\def ".repeat(4096),
        r"\a{".repeat(4096),
        r"\begin{x}".repeat(4096),
        r"\textbf{".repeat(4096),
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
    let input = format!("{}}} {}", r"\a{".repeat(4096), r"\textbf{x}");
    assert_eq!(pe(&input, raw_tex()).last(), Some(&tex(r"\textbf{x}")));
}

#[test]
fn raw_tex_off_leaves_escape_behavior() {
    assert_eq!(p(r"\textbf{b}"), vec![str(r"\textbf{b}")]);
    assert_eq!(pe(r"\*", raw_tex()), vec![str("*")]);
}

#[test]
fn raw_tex_environment_captured_as_one_inline() {
    assert_eq!(
        pe("\\begin{equation}\nx\n\\end{equation}", raw_tex()),
        vec![tex("\\begin{equation}\nx\n\\end{equation}")]
    );
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
    assert_eq!(
        pe(r"\begin{equation*} x \end{equation*}", raw_tex()),
        vec![tex(r"\begin{equation*} x \end{equation*}")]
    );
}

#[test]
fn raw_tex_environment_balances_nested_begins() {
    assert_eq!(
        pe(r"\begin{eq}\begin{eq}a\end{eq}\end{eq}", raw_tex()),
        vec![tex(r"\begin{eq}\begin{eq}a\end{eq}\end{eq}")]
    );
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
    assert_eq!(
        pe("\\begin{equation}\nx", raw_tex()),
        vec![str(r"\begin{equation}"), Inline::SoftBreak, str("x")]
    );
    assert_eq!(
        pe(r"\begin x", raw_tex()),
        vec![str(r"\begin"), Inline::Space, str("x")]
    );
    assert_eq!(
        pe(r"\end{equation}", raw_tex()),
        vec![str(r"\end{equation}")]
    );
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
    assert_eq!(pe(r"\( x \)", single_math()), vec![math_inline("x")]);
    assert_eq!(
        pe(r"\[ x = y \]", single_math()),
        vec![math_display(" x = y ")]
    );
}

#[test]
fn single_backslash_math_empty_and_unclosed_fall_back() {
    assert_eq!(pe(r"\(\)", single_math()), vec![str("()")]);
    assert_eq!(pe(r"\(x", single_math()), vec![str("(x")]);
    assert_eq!(pe(r"\( \)", single_math()), vec![math_inline("")]);
}

#[test]
fn single_backslash_math_escapes_inside_content() {
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
    assert_eq!(p(r"\(x\)"), vec![str("(x)")]);
}

#[test]
fn raw_tex_fixed_arguments_keep_trailing_text_and_links() {
    assert_eq!(
        pe(r"\textbf{x}{tail}[label](/url)", raw_tex()),
        vec![
            tex(r"\textbf{x}"),
            str("{tail}"),
            link(vec![str("label")], "/url")
        ]
    );
    assert_eq!(
        pe(r"\href{url}{label}{tail}", raw_tex()),
        vec![tex(r"\href{url}{label}"), str("{tail}")]
    );
    assert_eq!(pe("\\emph α", raw_tex()), vec![tex("\\emph α")]);
}

#[test]
fn raw_tex_optional_arguments_keep_later_brackets_literal() {
    assert_eq!(
        pe(r"\cite[a][b]{key}{tail}", raw_tex()),
        vec![tex(r"\cite[a][b]{key}"), str("{tail}")]
    );
    assert_eq!(
        pe(r"\foo[a]{x}[tail]", raw_tex()),
        vec![tex(r"\foo[a]{x}"), str("[tail]")]
    );
    assert_eq!(
        pe(r"\TeX{tail}", raw_tex()),
        vec![tex(r"\TeX"), str("{tail}")]
    );
}

#[test]
fn raw_tex_definitions_keep_options_after_the_command_name() {
    assert_eq!(
        pe(r"\newcommand{\foo}[1][d]{#1}{tail}", raw_tex()),
        vec![tex(r"\newcommand{\foo}[1][d]{#1}"), str("{tail}")]
    );
    assert_eq!(
        pe(r"\newcommand\foo[1]{#1}{tail}", raw_tex()),
        vec![tex(r"\newcommand\foo[1]{#1}"), str("{tail}")]
    );
}

#[test]
fn nested_raw_tex_text_arguments_do_not_overflow() {
    let source = format!("{}x", r"\textbf".repeat(2_000));
    assert_eq!(pe(&source, raw_tex()), vec![tex(&source)]);
}
