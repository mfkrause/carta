//! Writer-parity tests exercising the LaTeX writer across every document node it renders. Each case
//! feeds a small source through the oracle to mint both the AST (as JSON our writer consumes) and the
//! expected LaTeX output, then diffs our writer's output byte-for-byte. Expected values are minted at
//! run time, never committed; the oracle is hard-required (its absence fails with provisioning
//! instructions rather than skipping). Tables are excluded: the LaTeX writer does not yet render them.

// This whole file is test code, where panicking on a known case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oxidoc_testkit::differential::{self, Diff};
use oxidoc_testkit::pandoc_bin;

/// A writer-parity case: a human label, the oracle source format, and the source text.
struct Case {
    label: &'static str,
    from: &'static str,
    input: &'static str,
}

const fn md(label: &'static str, input: &'static str) -> Case {
    Case {
        label,
        from: "markdown",
        input,
    }
}

/// Cases chosen to cover every node the LaTeX writer handles. Markdown reaches the whole supported
/// model; tables are omitted because the writer does not render them yet.
fn cases() -> Vec<Case> {
    vec![
        md("paragraph", "a simple paragraph with words"),
        md("soft-break", "first line\nsecond line"),
        md("hard-break", "first line\\\nsecond line"),
        md(
            "emphasis-family",
            "*emph* **strong** ~~struck~~ super^2^ sub~3~",
        ),
        md(
            "underline-smallcaps",
            "[under]{.underline} [small]{.smallcaps}",
        ),
        md("inline-code", "a `let x = 1;` b"),
        md(
            "inline-code-specials",
            "before `a & b ~ c \\ d % e # f $ g _ h { i } j ^ k | l < m > n` after",
        ),
        md("escape-specials", "a & b % c # d _ e $ f { g } h ^ i ~ j"),
        md(
            "escape-brackets-backslash",
            "use [a] and \\ and | and < and >",
        ),
        md("escape-dashes-quote", "a -- b and it's mine"),
        md(
            "code-block",
            "``` rust\nlet s = \"hi\";\nx < y && y > z\n```",
        ),
        md("code-block-id", "``` {#snippet}\nplain code\n```"),
        md(
            "headers",
            "# One\n\n## Two\n\n#### Four\n\n##### Five\n\n###### Six",
        ),
        md("header-attr", "# Titled {#anchor .cls}"),
        md("header-unnumbered", "# Hidden {.unnumbered}"),
        md("header-texorpdfstring", "## A *fancy* heading"),
        md("blockquote", "> quoted\n>\n> more lines here"),
        md("bullet-list-tight", "- a\n- b\n- c"),
        md("bullet-list-loose", "- a\n\n- b\n\n- c"),
        md("bullet-nested", "- outer\n    - inner one\n    - inner two"),
        md("ordered-start", "5. five\n6. six"),
        md("ordered-period", "1. one\n2. two"),
        md("ordered-one-paren", "1) one\n2) two"),
        md("ordered-two-parens", "(1) one\n(2) two"),
        md("ordered-lower-alpha", "a. one\nb. two"),
        md("ordered-upper-alpha", "A. one\nB. two"),
        md("ordered-lower-roman", "i. one\nii. two"),
        md("ordered-upper-roman", "I. one\nII. two"),
        md("ordered-example", "(@) first\n(@) second"),
        md(
            "ordered-nested",
            "1. outer\n    1. inner one\n    2. inner two",
        ),
        md(
            "definition-list-tight",
            "Term\n:   Definition one\n:   Definition two",
        ),
        md(
            "definition-list-loose",
            "Term\n\n:   Definition one\n\n:   Definition two",
        ),
        md("line-block", "| first line\n| second line"),
        md("horizontal-rule", "above\n\n---\n\nbelow"),
        md("raw-html-block-dropped", "<div class=\"x\">raw block</div>"),
        md("raw-html-inline-dropped", "text <cite>raw</cite> more"),
        md("raw-latex-block", "```{=latex}\n\\macro{value}\n```"),
        md("raw-latex-inline", "a `\\emph{x}`{=latex} b"),
        md("math-inline", "an equation $a^2 + b^2 = c^2$ inline"),
        md("math-display", "$$\\int_0^1 x \\, dx$$"),
        md("citation", "see [@knuth1984] for details"),
        md("span-plain", "a [styled]{.c data-k=\"v\"} word"),
        md("span-id", "a [styled]{#s} word"),
        md("div-plain", "::: {.note}\nbody paragraph\n:::"),
        md("div-id", "::: {#d}\nbody paragraph\n:::"),
        md("quoted-smart", "She said \"hello\" and 'hi'."),
        md("link-text", "see [the text](http://example.com \"t\") now"),
        md("link-autolink", "<http://example.com>"),
        md("link-url-specials", "[x](http://example.com/a%20b#frag)"),
        md("image-implicit-figure", "![alt text](pic.png \"t\")"),
        md("image-inline", "an ![alt words](pic.png) inline"),
        md("image-no-alt", "![](pic.png)"),
        md("footnote-simple", "ref[^a]\n\n[^a]: the note body"),
        md(
            "footnote-multipara",
            "ref[^a]\n\n[^a]: first paragraph\n\n    second paragraph",
        ),
        md("footnote-two", "x[^a] y[^b]\n\n[^a]: first\n\n[^b]: second"),
        md(
            "wide-and-combining-wrap",
            "This is a deliberately long paragraph with wide 中文字符 glyphs and a combining mark e\u{0301} so the writer measures column widths while wrapping at the fill column boundary.",
        ),
        md(
            "smart-ellipsis-dashes",
            "wait... an en--dash and an em---dash",
        ),
    ]
}

#[test]
fn latex_writer_matches_oracle_across_the_model() {
    assert!(
        pandoc_bin().is_file(),
        "pinned pandoc binary not found at {}.\nRun tools/install-pandoc.sh.",
        pandoc_bin().display()
    );

    let cases = cases();
    let total = cases.len();
    let mut failures = Vec::new();
    for case in cases {
        match differential::writer("latex", case.from, case.input).expect("run writer surface") {
            Diff::Match | Diff::OracleRejected { .. } => {}
            Diff::Mismatch { detail } => failures.push(format!("{}: {detail}", case.label)),
            Diff::OxidocError { detail } => {
                failures.push(format!("{}: error: {detail}", case.label));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{} LaTeX writer cases diverged:\n{}",
        failures.len(),
        total,
        failures.join("\n")
    );
}
