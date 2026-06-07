//! Writer-parity tests for the native writer across the full document model — every block and
//! inline node, including those the `CommonMark` reader never produces (tables, definition lists,
//! figures, footnotes, citations, math, raw passthrough, …). Each case feeds a small source through
//! the oracle to mint both the AST (as JSON our writer consumes) and the expected native rendering,
//! then diffs our writer's output byte-for-byte. Expected values are minted at run time, never
//! committed; the oracle is hard-required (its absence fails with provisioning instructions rather
//! than skipping).

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

const fn html(label: &'static str, input: &'static str) -> Case {
    Case {
        label,
        from: "html",
        input,
    }
}

/// Cases chosen to cover every writer node path. Markdown reaches most of the model; raw HTML is
/// used for the table spans markdown cannot express.
fn cases() -> Vec<Case> {
    vec![
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
            "code-block-lang-and-quote",
            "``` rust\nlet s = \"hi\";\nx < y && y > z\n```",
        ),
        md("headers", "# One\n\n## Two\n\n###### Six"),
        md("header-attr", "# Titled {#anchor .cls key=val}"),
        md("blockquote", "> quoted\n>\n> lines"),
        md("bullet-list-tight", "- a\n- b\n- c"),
        md("bullet-list-loose", "- a\n\n- b\n\n- c"),
        md("ordered-start", "5. five\n6. six"),
        md("ordered-lower-alpha", "a. one\nb. two"),
        md("ordered-upper-alpha", "A. one\nB. two"),
        md("ordered-lower-roman", "i. one\nii. two"),
        md("ordered-upper-roman", "I. one\nII. two"),
        md("ordered-example", "(@) first\n(@) second"),
        md(
            "definition-list",
            "Term\n:   Definition one\n\n:   Definition two",
        ),
        md("line-block", "| first line\n| second line"),
        md("horizontal-rule", "above\n\n---\n\nbelow"),
        md("raw-html-block", "<div class=\"x\">raw block</div>"),
        md("raw-html-inline", "text <cite>raw</cite> more"),
        md("raw-latex-block", "```{=latex}\n\\macro\n```"),
        md("raw-latex-inline", "a `\\macro`{=latex} b"),
        md("math-inline", "an equation $a^2 + b^2 = c^2$ inline"),
        md("math-display", "$$\\int_0^1 x \\, dx$$"),
        md("citation", "see [@knuth1984] for details"),
        md("span", "a [styled]{#s .c data-k=\"v\"} word"),
        md("div", "::: {#d .note}\nbody paragraph\n:::"),
        md("quoted-smart", "She said \"hello\" and 'hi'."),
        md(
            "link-title-attr",
            "[text](http://example.com \"a title\"){#l .ext}",
        ),
        md(
            "image-implicit-figure",
            "![alt text](pic.png \"t\"){width=200}",
        ),
        md("footnote-simple", "ref[^a]\n\n[^a]: the note body"),
        md(
            "footnote-multipara",
            "ref[^a]\n\n[^a]: first paragraph\n\n    second paragraph",
        ),
        md("footnote-two", "x[^a] y[^b]\n\n[^a]: first\n\n[^b]: second"),
        md("table-simple", "| h1 | h2 |\n|----|----|\n| a  | b  |"),
        md(
            "table-aligned",
            "| l | r | c | d |\n|:--|--:|:-:|---|\n| 1 | 2 | 3 | 4 |",
        ),
        md(
            "table-caption",
            "| h |\n|---|\n| v |\n\n: A caption for the table",
        ),
        md(
            "table-grid-widths",
            "+---+---------+\n| a | b       |\n+===+=========+\n| 1 | 2       |\n+---+---------+",
        ),
        html(
            "table-colspan",
            "<table><tr><td colspan=\"2\">wide</td></tr><tr><td>a</td><td>b</td></tr></table>",
        ),
        html(
            "table-rowspan",
            "<table><tr><td rowspan=\"2\">tall</td><td>a</td></tr><tr><td>b</td></tr></table>",
        ),
        md(
            "wide-and-combining-wrap",
            "This is a deliberately long paragraph with wide 中文字符 glyphs and a combining mark e\u{0301} so the writer measures column widths while wrapping at the fill column boundary.",
        ),
        md(
            "literal-control-chars",
            "a literal NUL a\u{0}b and a U+0001 c\u{1}d are preserved verbatim",
        ),
        md("nonascii-escape", "café résumé naïve"),
    ]
}

#[test]
fn writer_matches_oracle_native_across_the_model() {
    assert!(
        pandoc_bin().is_file(),
        "pinned pandoc binary not found at {}.\nRun tools/install-pandoc.sh.",
        pandoc_bin().display()
    );

    let mut failures = Vec::new();
    for case in cases() {
        match differential::writer("native", case.from, case.input).expect("run writer surface") {
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
