//! Writer-parity tests exercising the plain-text writer across every document node it renders. Each
//! case feeds a small markdown source through the oracle to mint both the AST (as JSON our writer
//! consumes) and the expected plain-text output, then diffs our writer's output byte-for-byte.
//! Expected values are minted at run time, never committed; the oracle is hard-required (its absence
//! fails with provisioning instructions rather than skipping). Tables and math are excluded: the
//! plain writer does not yet render them.

// This whole file is test code, where panicking on a known case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oxidoc_testkit::differential::{self, Diff};
use oxidoc_testkit::pandoc_bin;

/// A writer-parity case: a human label and the markdown source text the oracle mints from.
struct Case {
    label: &'static str,
    input: &'static str,
}

const fn md(label: &'static str, input: &'static str) -> Case {
    Case { label, input }
}

/// Cases chosen to cover every node the plain writer handles. Markdown reaches the whole supported
/// model; tables and math are omitted because the writer does not render them yet.
fn cases() -> Vec<Case> {
    vec![
        md("paragraph", "a simple paragraph with words"),
        md("soft-break", "first line\nsecond line"),
        md("hard-break", "first line\\\nsecond line"),
        md("emphasis-stripped", "*emph* **strong** ~~struck~~"),
        md(
            "underline-smallcaps",
            "[under]{.underline} [small]{.smallcaps}",
        ),
        md("superscript-mapped", "x super^2^ here"),
        md("superscript-fallback", "x super^(ab)^ here"),
        md("subscript-mapped", "x sub~3~ here"),
        md("subscript-fallback", "water H~2~O and sub~ab~ here"),
        md("inline-code", "a `let x = 1;` b"),
        md("quoted-smart", "She said \"hello\" and 'hi'."),
        md(
            "link-stripped",
            "see [the text](http://example.com \"t\") now",
        ),
        md("image-stripped-inline", "an ![alt words](pic.png) inline"),
        md("span-stripped", "a [styled]{#s .c data-k=\"v\"} word"),
        md("citation", "see [@knuth1984] for details"),
        md("raw-html-inline-dropped", "text <cite>raw</cite> more"),
        md("raw-latex-inline-dropped", "a `\\macro`{=latex} b"),
        md("headers", "# One\n\n## Two\n\n###### Six"),
        md("header-with-break", "# title\\\nstill the header"),
        md("code-block", "``` rust\nlet s = \"hi\";\nx < y\n```"),
        md("raw-html-block-dropped", "<div class=\"x\">raw block</div>"),
        md("raw-latex-block-dropped", "```{=latex}\n\\macro\n```"),
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
            "definition-list-tight",
            "Term\n:   Definition one\n:   Definition two",
        ),
        md(
            "definition-list-loose",
            "Term\n\n:   Definition one\n\n:   Definition two",
        ),
        md("line-block", "| first line\n| second line"),
        md("horizontal-rule", "above\n\n---\n\nbelow"),
        md("div", "::: {#d .note}\nbody paragraph\n:::"),
        md("image-implicit-figure", "![alt text](pic.png \"t\")"),
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
            "literal-control-chars",
            "the reference writer preserves a literal NUL a\u{0}b and a U+0001 c\u{1}d verbatim rather than treating them as breaks",
        ),
    ]
}

#[test]
fn plain_writer_matches_oracle_across_the_model() {
    assert!(
        pandoc_bin().is_file(),
        "pinned pandoc binary not found at {}.\nRun tools/install-pandoc.sh.",
        pandoc_bin().display()
    );

    let cases = cases();
    let total = cases.len();
    let mut failures = Vec::new();
    for case in cases {
        match differential::writer("plain", "markdown", case.input).expect("run writer surface") {
            Diff::Match | Diff::OracleRejected { .. } => {}
            Diff::Mismatch { detail } => failures.push(format!("{}: {detail}", case.label)),
            Diff::OxidocError { detail } => {
                failures.push(format!("{}: error: {detail}", case.label));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{} plain writer cases diverged:\n{}",
        failures.len(),
        total,
        failures.join("\n")
    );
}
