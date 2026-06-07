//! Reader-parity tests for the HTML reader. Each case feeds a small HTML fragment through the oracle
//! to mint the expected JSON AST, then diffs the AST our reader produces byte-for-byte. The corpus is
//! chosen to exercise every block and inline node the reader builds — paragraphs, headers, lists,
//! quotes, code, rules, divs, definition lists, tables, figures, the inline emphasis family, links,
//! images, raw spans, entities and head metadata. Expected values are minted at run time, never
//! committed; the oracle is hard-required (its absence fails with provisioning instructions rather
//! than skipping).

// This whole file is test code, where panicking on a known case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oxidoc_testkit::differential::{self, Diff};
use oxidoc_testkit::pandoc_bin;

/// A reader-parity case: a human label and the HTML source text.
struct Case {
    label: &'static str,
    input: &'static str,
}

const fn html(label: &'static str, input: &'static str) -> Case {
    Case { label, input }
}

#[allow(clippy::too_many_lines)]
fn cases() -> Vec<Case> {
    vec![
        html("paragraph", "<p>a plain paragraph</p>"),
        html("loose-text", "bare text without a block wrapper"),
        html(
            "headers",
            "<h1>One</h1><h2>Two</h2><h3>Three</h3><h4>Four</h4><h5>Five</h5><h6>Six</h6>",
        ),
        html("header-with-id", "<h2 id=\"anchor\">Titled</h2>"),
        html("header-duplicate-ids", "<h1>Sec</h1><h2>Sec</h2>"),
        html("header-classes", "<h1 class=\"a b\">Classed heading</h1>"),
        html("bullet-list", "<ul><li>a</li><li>b</li><li>c</li></ul>"),
        html("ordered-list-default", "<ol><li>one</li><li>two</li></ol>"),
        html(
            "ordered-list-start",
            "<ol start=\"5\"><li>five</li><li>six</li></ol>",
        ),
        html(
            "ordered-list-lower-alpha",
            "<ol type=\"a\"><li>one</li><li>two</li></ol>",
        ),
        html(
            "ordered-list-upper-roman",
            "<ol type=\"I\"><li>one</li><li>two</li></ol>",
        ),
        html("nested-list", "<ul><li>a<ul><li>b</li></ul></li></ul>"),
        html("loose-list", "<ul><li><p>a</p></li><li><p>b</p></li></ul>"),
        html("blockquote", "<blockquote><p>quoted</p></blockquote>"),
        html("pre-code-block", "<pre>let x = 1;\nx &lt; y</pre>"),
        html(
            "pre-code-lang",
            "<pre><code class=\"language-rust\">let s = 1;</code></pre>",
        ),
        html("horizontal-rule", "<p>above</p><hr><p>below</p>"),
        html("div", "<div id=\"d\" class=\"note\"><p>body</p></div>"),
        html("section", "<section><p>body</p></section>"),
        html(
            "definition-list",
            "<dl><dt>Term</dt><dd>Definition one</dd><dd>Definition two</dd></dl>",
        ),
        html(
            "figure",
            "<figure><img src=\"pic.png\"><figcaption>cap</figcaption></figure>",
        ),
        html(
            "table-simple",
            "<table><thead><tr><th>h1</th><th>h2</th></tr></thead><tbody><tr><td>a</td><td>b</td></tr></tbody></table>",
        ),
        html(
            "table-aligned",
            "<table><tr><td align=\"left\">l</td><td align=\"right\">r</td><td align=\"center\">c</td></tr></table>",
        ),
        html(
            "table-colspan",
            "<table><tr><td colspan=\"2\">wide</td></tr><tr><td>a</td><td>b</td></tr></table>",
        ),
        html(
            "table-rowspan",
            "<table><tr><td rowspan=\"2\">tall</td><td>a</td></tr><tr><td>b</td></tr></table>",
        ),
        html(
            "table-caption",
            "<table><caption>A caption</caption><tr><td>v</td></tr></table>",
        ),
        html("emphasis", "<p><em>emph</em> <i>also emph</i></p>"),
        html(
            "strong",
            "<p><strong>strong</strong> <b>also strong</b></p>",
        ),
        html(
            "strikeout",
            "<p><del>del</del> <s>s</s> <strike>strike</strike></p>",
        ),
        html("underline", "<p><ins>ins</ins> <u>u</u></p>"),
        html("super-sub", "<p>x<sup>2</sup> y<sub>3</sub></p>"),
        html("quoted", "<p><q>quoted text</q></p>"),
        html("line-break", "<p>a<br>b</p>"),
        html(
            "span",
            "<p>a <span id=\"s\" class=\"c\">styled</span> word</p>",
        ),
        html(
            "span-class-elements",
            "<p><mark>m</mark> <small>s</small> <abbr>a</abbr> <kbd>k</kbd> <dfn>d</dfn></p>",
        ),
        html("inline-code", "<p>a <code>let x = 1;</code> b</p>"),
        html("code-tt", "<p><tt>tt</tt></p>"),
        html("samp-var", "<p><samp>out</samp> <var>x</var></p>"),
        html(
            "link",
            "<p><a href=\"http://example.com\" title=\"t\">text</a></p>",
        ),
        html(
            "link-attr",
            "<p><a href=\"u\" id=\"l\" class=\"ext\">text</a></p>",
        ),
        html("anchor-name", "<p><a name=\"target\">label</a></p>"),
        html(
            "image",
            "<p><img src=\"pic.png\" title=\"t\" alt=\"alt text\"></p>",
        ),
        html("entities-named", "<p>a &amp; b &copy; c</p>"),
        html("entities-numeric", "<p>&#65; &#x41;</p>"),
        html("comment", "<p>a<!-- c -->b</p>"),
        html("script-dropped", "<script>var x = 1;</script><p>p</p>"),
        html(
            "head-metadata",
            "<head><title>T</title><meta name=\"author\" content=\"A\"></head><body><p>b</p></body>",
        ),
    ]
}

#[test]
fn reader_matches_oracle_html_across_the_model() {
    assert!(
        pandoc_bin().is_file(),
        "pinned pandoc binary not found at {}.\nRun tools/install-pandoc.sh.",
        pandoc_bin().display()
    );

    let mut failures = Vec::new();
    for case in cases() {
        match differential::reader_json("html", case.input).expect("run reader surface") {
            Diff::Match | Diff::OracleRejected { .. } => {}
            Diff::Mismatch { detail } => failures.push(format!("{}: {detail}", case.label)),
            Diff::OxidocError { detail } => {
                failures.push(format!("{}: error: {detail}", case.label));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{} reader cases diverged:\n{}",
        failures.len(),
        cases().len(),
        failures.join("\n")
    );
}
