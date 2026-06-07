//! Writer-parity tests exercising the reStructuredText writer across the document model — every
//! block and inline node the writer renders (tables are not yet handled and are excluded). Each case
//! feeds a small source through the oracle to mint both the AST (as JSON our writer consumes) and the
//! expected RST, then diffs our writer's output byte-for-byte. Expected values are minted at run
//! time, never committed; the oracle is hard-required (its absence fails with provisioning
//! instructions rather than skipping).

// This whole file is test code, where panicking on a known case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oxidoc_testkit::differential::{self, Diff};
use oxidoc_testkit::pandoc_bin;

/// A writer-parity case: a human label, the oracle source format (always markdown here), and the
/// source text.
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

/// Cases chosen to cover every RST writer node path the writer renders (tables excluded).
fn cases() -> Vec<Case> {
    vec![
        md("para-plain", "A simple paragraph of flowing text."),
        md(
            "para-wrap",
            "This is a deliberately long paragraph with enough words that the writer must wrap it across the fill column boundary at least once or twice.",
        ),
        md(
            "emphasis-family",
            "*emph* **strong** ~~struck~~ super^2^ sub~3~",
        ),
        md("nested-emphasis", "*emph with **strong** inside* end"),
        md(
            "emphasis-with-link",
            "*see [text](http://example.com) here*",
        ),
        md(
            "underline-smallcaps",
            "[under]{.underline} [small]{.smallcaps}",
        ),
        md("inline-code", "a `let x = 1;` b"),
        md("inline-code-backtick", "use ``a `b` c`` here"),
        md("code-block-plain", "    indented code\n    second line"),
        md(
            "code-block-lang",
            "``` rust\nlet s = \"hi\";\nx < y && y > z\n```",
        ),
        md(
            "code-block-numbered",
            "``` {.python .numberLines}\nprint(1)\nprint(2)\n```",
        ),
        md(
            "headers",
            "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five",
        ),
        md("header-explicit-id", "# Titled {#custom-anchor}"),
        md("blockquote", "> quoted\n>\n> lines of text"),
        md("bullet-list-tight", "- a\n- b\n- c"),
        md("bullet-list-loose", "- a\n\n- b\n\n- c"),
        md(
            "bullet-nested",
            "- outer\n\n    - inner one\n    - inner two",
        ),
        md("ordered-start", "5. five\n6. six"),
        md("ordered-lower-alpha", "a. one\nb. two"),
        md("ordered-upper-alpha", "A. one\nB. two"),
        md("ordered-lower-roman", "i. one\nii. two"),
        md("ordered-upper-roman", "I. one\nII. two"),
        md("ordered-paren", "1) one\n2) two"),
        md(
            "definition-list",
            "Term\n:   Definition one\n\n:   Definition two",
        ),
        md("line-block", "| first line\n| second line"),
        md("horizontal-rule", "above\n\n---\n\nbelow"),
        md("raw-rst-block", "```{=rst}\n.. custom directive\n```"),
        md("raw-other-block", "```{=html}\n<p>x</p>\n```"),
        md("raw-rst-inline", "a `:role:`{=rst} b"),
        md("math-inline", "an equation $a^2 + b^2 = c^2$ inline"),
        md("math-display", "$$\\int_0^1 x \\, dx$$"),
        md("citation", "see [@knuth1984] for details"),
        md("span", "a [styled]{#s .c} word"),
        md("div-container", "::: {.sidebar}\nbody paragraph\n:::"),
        md("div-admonition", "::: note\nbe careful here\n:::"),
        md("quoted-smart", "She said \"hello\" and 'hi'."),
        md("link-named", "[text](http://example.com)"),
        md("link-autolink", "<http://example.com>"),
        md("image-substitution", "![alt text](pic.png){width=200}"),
        md("image-no-alt", "![](pic.png)"),
        md("figure", "![a caption](pic.png \"fig title\")\n"),
        md("footnote-simple", "ref[^a]\n\n[^a]: the note body"),
        md(
            "footnote-multipara",
            "ref[^a]\n\n[^a]: first paragraph\n\n    second paragraph",
        ),
        md("footnote-two", "x[^a] y[^b]\n\n[^a]: first\n\n[^b]: second"),
        md("escape-special", "a * b ` c | d and trailing_ref_"),
        md(
            "wide-and-combining-wrap",
            "This is a deliberately long paragraph with wide 中文字符 glyphs and a combining mark e\u{0301} so the writer measures column widths while wrapping at the fill column boundary.",
        ),
    ]
}

#[test]
fn writer_matches_oracle_rst_across_the_model() {
    assert!(
        pandoc_bin().is_file(),
        "pinned pandoc binary not found at {}.\nRun tools/install-pandoc.sh.",
        pandoc_bin().display()
    );

    let mut failures = Vec::new();
    for case in cases() {
        match differential::writer("rst", case.from, case.input).expect("run writer surface") {
            Diff::Match | Diff::OracleRejected { .. } => {}
            Diff::Mismatch { detail } => failures.push(format!("{}: {detail}", case.label)),
            Diff::OxidocError { detail } => {
                failures.push(format!("{}: error: {detail}", case.label));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{} writer cases diverged:\n{}",
        failures.len(),
        cases().len(),
        failures.join("\n")
    );
}
