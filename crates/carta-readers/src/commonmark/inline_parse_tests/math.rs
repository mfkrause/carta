//! TeX math and raw TeX inline-parse tests.

use super::*;

#[test]
fn inline_and_display_math() {
    assert_eq!(pe("$a+b$", math()), vec![math_inline("a+b")]);
    assert_eq!(pe("$$x=y$$", math()), vec![math_display("x=y")]);
    // Display math keeps interior spaces verbatim.
    assert_eq!(pe("$$ x $$", math()), vec![math_display(" x ")]);
}

#[test]
fn dollar_amounts_are_not_math() {
    // An opener must be followed by a non-space; a closer may not follow a digit or trail a space.
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
    // `_`/`*` inside math do not start emphasis.
    assert_eq!(pe("$x_1*y*$", math()), vec![math_inline("x_1*y*")]);
    // An escaped dollar inside content does not close the span.
    assert_eq!(pe(r"$a\$b$", math()), vec![math_inline(r"a\$b")]);
}

#[test]
fn failed_display_falls_back_to_inline() {
    // `$$x$` has no closing `$$`; the first `$` is literal and `$x$` parses as inline math.
    assert_eq!(pe("$$x$", math()), vec![str("$"), math_inline("x")]);
}

#[test]
fn dollar_is_literal_without_the_extension() {
    assert_eq!(p("$a+b$"), vec![str("$a+b$")]);
}

#[test]
fn raw_tex_commands_with_argument_groups() {
    // Consecutive `{…}` groups are all captured.
    assert_eq!(
        pe(r"\textbf{b}\emph{c}", raw_tex()),
        vec![tex(r"\textbf{b}"), tex(r"\emph{c}")]
    );
    // A leading optional `[…]` group precedes a `{…}` argument.
    assert_eq!(pe(r"\sqrt[3]{8}", raw_tex()), vec![tex(r"\sqrt[3]{8}")]);
    // Nested braces inside a group are balanced.
    assert_eq!(pe(r"\foo{a{b}c}", raw_tex()), vec![tex(r"\foo{a{b}c}")]);
}

#[test]
fn raw_tex_bare_command_absorbs_trailing_blanks() {
    // A command with no argument group swallows following spaces.
    assert_eq!(pe(r"\alpha y", raw_tex()), vec![tex(r"\alpha "), str("y")]);
    // A command followed by an argument group does not absorb the trailing space.
    assert_eq!(
        pe(r"\foo{a} y", raw_tex()),
        vec![tex(r"\foo{a}"), Inline::Space, str("y")]
    );
    // A command name carrying a digit does not absorb the trailing space.
    assert_eq!(
        pe(r"\foo1 y", raw_tex()),
        vec![tex(r"\foo1"), Inline::Space, str("y")]
    );
    // The first character must be a letter, so a digit after the backslash is not a command.
    assert_eq!(pe(r"\1foo", raw_tex()), vec![str(r"\1foo")]);
}

#[test]
fn raw_tex_unbalanced_brace_reverts_whole_command() {
    // An unclosed `{`-group reverts the entire command to literal text.
    assert_eq!(
        pe(r"\foo{a y", raw_tex()),
        vec![str(r"\foo{a"), Inline::Space, str("y")]
    );
    // An unclosed `[`-group merely stops the group run; the command stands.
    assert_eq!(
        pe(r"\foo[a y", raw_tex()),
        vec![tex(r"\foo"), str("[a"), Inline::Space, str("y")]
    );
}

#[test]
fn raw_tex_unclosed_openers_stay_literal_at_scale() {
    // the fast-fail and scan budget bound look-ahead cost without changing the parse: all-literal at any size
    let cases = [
        r"\a{".repeat(4096), // no `}` anywhere: close-delimiter fast-fail
        format!("{}}}", r"\a{".repeat(4096)), // a single far `}` that never balances: budget backstop
        r"\begin{x}".repeat(4096),            // no `\end{x}`: environment-close fast-fail
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
}

#[test]
fn raw_tex_off_leaves_escape_behavior() {
    // without the extension a command name is not raw TeX; `\t` is not punctuation, backslash stays literal
    assert_eq!(p(r"\textbf{b}"), vec![str(r"\textbf{b}")]);
    // A backslash escape of punctuation still works regardless of the extension.
    assert_eq!(pe(r"\*", raw_tex()), vec![str("*")]);
}

#[test]
fn raw_tex_environment_captured_as_one_inline() {
    // a complete `\begin{ENV}`…`\end{ENV}` is one raw inline, body and interior newlines included
    assert_eq!(
        pe("\\begin{equation}\nx\n\\end{equation}", raw_tex()),
        vec![tex("\\begin{equation}\nx\n\\end{equation}")]
    );
    // The environment may sit on a single line amid surrounding text.
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
    // A trailing `*` is part of the environment name, so the close must carry it too.
    assert_eq!(
        pe(r"\begin{equation*} x \end{equation*}", raw_tex()),
        vec![tex(r"\begin{equation*} x \end{equation*}")]
    );
}

#[test]
fn raw_tex_environment_balances_nested_begins() {
    // a same-name nested environment deepens nesting; capture ends at the matching outer close
    assert_eq!(
        pe(r"\begin{eq}\begin{eq}a\end{eq}\end{eq}", raw_tex()),
        vec![tex(r"\begin{eq}\begin{eq}a\end{eq}\end{eq}")]
    );
    // A nested environment of a different name is just part of the outer body.
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
    // Without a matching close, `\begin{ENV}` is literal text, not a raw command.
    assert_eq!(
        pe("\\begin{equation}\nx", raw_tex()),
        vec![str(r"\begin{equation}"), Inline::SoftBreak, str("x")]
    );
    // a bare `\begin` with no `{ENV}` group is not raw TeX: backslash and word stay plain text
    assert_eq!(
        pe(r"\begin x", raw_tex()),
        vec![str(r"\begin"), Inline::Space, str("x")]
    );
    // A standalone `\end{ENV}` is literal text.
    assert_eq!(
        pe(r"\end{equation}", raw_tex()),
        vec![str(r"\end{equation}")]
    );
    // A mismatched close does not satisfy the opener; the whole span reverts to text.
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
    // Inline content is trimmed; display content is verbatim.
    assert_eq!(pe(r"\( x \)", single_math()), vec![math_inline("x")]);
    assert_eq!(
        pe(r"\[ x = y \]", single_math()),
        vec![math_display(" x = y ")]
    );
}

#[test]
fn single_backslash_math_empty_and_unclosed_fall_back() {
    // Empty content is not a math span: `\(` and `\)` revert to escaped parentheses.
    assert_eq!(pe(r"\(\)", single_math()), vec![str("()")]);
    // No closer: the opener's backslash escapes the `(`.
    assert_eq!(pe(r"\(x", single_math()), vec![str("(x")]);
    // A span of only spaces is still a (trimmed-empty) span.
    assert_eq!(pe(r"\( \)", single_math()), vec![math_inline("")]);
}

#[test]
fn single_backslash_math_escapes_inside_content() {
    // An escaped delimiter inside the content does not close the span.
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
    // Without the extension `\(` is a plain escaped parenthesis.
    assert_eq!(p(r"\(x\)"), vec![str("(x)")]);
}
